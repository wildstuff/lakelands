//! The projection with its kept history.
//!
//! Every state the fold puts is kept, valid from a position, beside the
//! states before it; a [`History`] answers the state valid at any
//! position as the latest one put at or before it. Nothing is replaced
//! but a state put again at the same position, which is the same state
//! folded twice.
//!
//! The fold writes inside a transaction: the puts between `begin` and
//! `commit` are held as [`Put`]s and applied together when it commits,
//! with the position the projection then reflects. A rollback drops them,
//! so a view never sees half an append.
//!
//! Every put carries its own position (a record's `position`, the `at`
//! of everything else), and its state is valid from there, so a view at
//! a position inside a multi-event append sees each state from the event
//! that made it.

use std::collections::BTreeMap;

use lakelands_api::engine::Declared;
use lakelands_api::error::Result;
use lakelands_api::event::{Digest, EventId};
use lakelands_api::id::{Position, ProposalId, RecordKey, TypeName};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::write::{Declaration, Proposal};

use crate::invalid;

/// The states of one thing, each valid from a position, oldest first.
#[derive(Clone, Debug)]
pub(crate) struct History<T>(Vec<(Position, T)>);

impl<T> Default for History<T> {
    fn default() -> Self {
        History(Vec::new())
    }
}

impl<T> History<T> {
    /// Adds a state valid from `from`, keeping every state before it. A
    /// state put again at the same position replaces the one there.
    pub(crate) fn put(&mut self, from: Position, state: T) {
        let i = self.0.partition_point(|(p, _)| *p < from);
        match self.0.get_mut(i) {
            Some((p, old)) if *p == from => *old = state,
            _ => self.0.insert(i, (from, state)),
        }
    }

    /// The state valid at `at`: the latest put at or before it. Nothing is
    /// valid in a view of an empty projection.
    pub(crate) fn at(&self, at: Option<Position>) -> Option<&T> {
        let at = at?;
        let i = self.0.partition_point(|(p, _)| *p <= at);
        i.checked_sub(1).map(|i| &self.0[i].1)
    }
}

/// One write of the fold, held until its transaction commits.
#[derive(Clone, Debug)]
pub(crate) enum Put {
    /// A declaration in force from its `since`.
    Declaration(Declared),
    /// A record's whole state from its `position`.
    Record(StoredRecord),
    /// A proposal's state from `at`.
    Proposal {
        /// The proposal.
        proposal: Proposal,
        /// From when.
        at: Position,
    },
    /// Membership of a record in an object set from `at`.
    Membership {
        /// The object set.
        set: String,
        /// The record's type.
        ty: TypeName,
        /// The record's key.
        key: RecordKey,
        /// Whether it is a member.
        member: bool,
        /// From when.
        at: Position,
    },
    /// A seam's recorded output, from `at`.
    Output {
        /// The seam.
        seam: String,
        /// The input's digest.
        input: Digest,
        /// The output.
        output: Value,
        /// From when.
        at: Position,
    },
    /// An event id applied at `at`.
    Applied {
        /// The event.
        id: EventId,
        /// Where it was applied.
        at: Position,
    },
}

impl Put {
    /// The position the put's state is valid from.
    fn own_position(&self) -> Position {
        match self {
            Put::Declaration(d) => d.since,
            Put::Record(r) => r.position,
            Put::Membership { at, .. }
            | Put::Applied { at, .. }
            | Put::Proposal { at, .. }
            | Put::Output { at, .. } => *at,
        }
    }
}

/// One lake's projection: every kept state, and how far it reflects the
/// log.
#[derive(Debug, Default)]
pub(crate) struct Projection {
    pub(crate) declarations: BTreeMap<String, History<Declared>>,
    pub(crate) records: BTreeMap<(TypeName, RecordKey), History<StoredRecord>>,
    pub(crate) proposals: BTreeMap<ProposalId, History<Proposal>>,
    pub(crate) members: BTreeMap<(String, TypeName, RecordKey), History<bool>>,
    pub(crate) outputs: BTreeMap<(String, Digest), History<Value>>,
    pub(crate) applied: BTreeMap<EventId, Position>,
    pub(crate) folded: Option<Position>,
}

impl Projection {
    /// Applies one transaction's puts and records that the projection now
    /// reflects the log through `through`. Checked first, applied whole:
    /// a commit behind the projection, or a put valid only after
    /// `through`, refuses the transaction and changes nothing.
    pub(crate) fn commit(&mut self, puts: Vec<Put>, through: Position) -> Result<()> {
        if let Some(folded) = self.folded.filter(|f| *f > through) {
            return Err(invalid(format!(
                "the projection reflects the log through position {folded}; a transaction \
                 cannot commit through the earlier position {through}"
            )));
        }
        if let Some(late) = puts.iter().map(Put::own_position).find(|p| *p > through) {
            return Err(invalid(format!(
                "a state valid from position {late} cannot be committed through the earlier \
                 position {through}"
            )));
        }
        for put in puts {
            self.apply(put);
        }
        self.folded = Some(through);
        Ok(())
    }

    fn apply(&mut self, put: Put) {
        match put {
            Put::Declaration(d) => {
                let since = d.since;
                self.declarations
                    .entry(lakelands_core::engines::declaration_key(&d.declaration))
                    .or_default()
                    .put(since, d);
            }
            Put::Record(r) => {
                let from = r.position;
                self.records
                    .entry((r.ty.clone(), r.key.clone()))
                    .or_default()
                    .put(from, r);
            }
            Put::Proposal { proposal, at } => {
                self.proposals
                    .entry(proposal.id.clone())
                    .or_default()
                    .put(at, proposal);
            }
            Put::Membership {
                set,
                ty,
                key,
                member,
                at,
            } => self
                .members
                .entry((set, ty, key))
                .or_default()
                .put(at, member),
            Put::Output {
                seam,
                input,
                output,
                at,
            } => self
                .outputs
                .entry((seam, input))
                .or_default()
                .put(at, output),
            Put::Applied { id, at } => {
                self.applied.entry(id).or_insert(at);
            }
        }
    }

    /// Every declaration in force at `at`, in identity order.
    pub(crate) fn declarations_at(&self, at: Option<Position>) -> Vec<Declared> {
        self.declarations
            .values()
            .filter_map(|h| h.at(at).cloned())
            .collect()
    }

    /// The type declaration in force for `ty` at `at`.
    pub(crate) fn type_at(&self, ty: &TypeName, at: Option<Position>) -> Option<&TypeDecl> {
        match &self
            .declarations
            .get(&format!("type {ty}"))?
            .at(at)?
            .declaration
        {
            Declaration::Type(t) => Some(t),
            _ => None,
        }
    }

    /// Every type declaration in force at `at`.
    pub(crate) fn types_at(&self, at: Option<Position>) -> impl Iterator<Item = &TypeDecl> {
        self.declarations
            .values()
            .filter_map(move |h| match &h.at(at)?.declaration {
                Declaration::Type(t) => Some(t),
                _ => None,
            })
    }

    /// Every record of `ty` in its state at `at`, in key order, whatever
    /// its flags.
    pub(crate) fn records_at<'a>(
        &'a self,
        ty: &'a TypeName,
        at: Option<Position>,
    ) -> impl Iterator<Item = &'a StoredRecord> + 'a {
        self.records
            .range((ty.clone(), RecordKey::from(""))..)
            .take_while(move |((t, _), _)| t == ty)
            .filter_map(move |(_, h)| h.at(at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lakelands_api::id::Version;
    use serde_json::json;

    fn record(key: &str, n: i64, position: u64) -> StoredRecord {
        StoredRecord {
            ty: TypeName::from("t"),
            key: RecordKey::from(key),
            version: Version(1),
            fields: [("n".into(), json!(n))].into_iter().collect(),
            meta: BTreeMap::new(),
            position: Position(position),
            retired: false,
            restricted: false,
        }
    }

    #[test]
    fn a_history_answers_the_latest_state_at_or_before_a_position() {
        let mut h = History::default();
        h.put(Position(5), "five");
        h.put(Position(2), "two");
        h.put(Position(9), "nine");
        assert_eq!(h.at(Some(Position(1))), None);
        assert_eq!(h.at(Some(Position(2))), Some(&"two"));
        assert_eq!(h.at(Some(Position(8))), Some(&"five"));
        assert_eq!(h.at(Some(Position(100))), Some(&"nine"));
        assert_eq!(h.at(None), None, "an empty projection shows nothing");
        h.put(Position(5), "five again");
        assert_eq!(h.at(Some(Position(5))), Some(&"five again"));
    }

    #[test]
    fn a_commit_keeps_the_superseded_record_state() {
        let mut p = Projection::default();
        p.commit(vec![Put::Record(record("a", 1, 1))], Position(1))
            .unwrap();
        p.commit(vec![Put::Record(record("a", 2, 3))], Position(3))
            .unwrap();
        let ty = TypeName::from("t");
        let at = |n| {
            p.records_at(&ty, Some(Position(n)))
                .next()
                .map(|r| r.fields["n"].clone())
        };
        assert_eq!(at(0), None);
        assert_eq!(at(2), Some(json!(1)));
        assert_eq!(at(3), Some(json!(2)));
        assert_eq!(p.folded, Some(Position(3)));
    }

    #[test]
    fn a_commit_behind_the_projection_or_with_a_later_state_changes_nothing() {
        let mut p = Projection::default();
        p.commit(vec![], Position(4)).unwrap();
        assert!(
            p.commit(vec![Put::Record(record("a", 1, 2))], Position(3))
                .is_err()
        );
        assert!(
            p.commit(vec![Put::Record(record("a", 1, 9))], Position(5))
                .is_err()
        );
        assert!(p.records.is_empty());
        assert_eq!(p.folded, Some(Position(4)));
    }

    #[test]
    fn an_output_is_valid_from_its_own_event_not_the_commit() {
        let mut p = Projection::default();
        p.commit(
            vec![Put::Output {
                seam: "fn".into(),
                input: Digest::zero(),
                output: json!(7),
                at: Position(4),
            }],
            Position(6),
        )
        .unwrap();
        let h = &p.outputs[&("fn".to_owned(), Digest::zero())];
        assert_eq!(h.at(Some(Position(3))), None);
        assert_eq!(h.at(Some(Position(4))), Some(&json!(7)));
    }
}
