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

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::engine::Declared;
use lakelands_api::error::Result;
use lakelands_api::event::{Digest, EventId};
use lakelands_api::id::{FieldName, Position, ProposalId, RecordKey, TypeName};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::read::Direction;
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

    /// The latest state put.
    pub(crate) fn latest(&self) -> Option<&T> {
        self.0.last().map(|(_, t)| t)
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
    /// The order index of the head (`engines::order_rows`): per type and
    /// declared field, the head records holding it, by sort key and key.
    pub(crate) sorted: BTreeMap<(TypeName, FieldName), SortedRows>,
    /// Each head record's rows in `sorted`, to take them out again.
    sorted_rows: BTreeMap<(TypeName, RecordKey), RecordRows>,
}

/// One record's rows of the order index: field, then sort key.
type RecordRows = Vec<(FieldName, Vec<u8>)>;

/// One field's rows of the order index: sort key, then record key.
pub(crate) type SortedRows = BTreeSet<(Vec<u8>, RecordKey)>;

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
        let mut records = BTreeSet::new();
        let mut types = BTreeSet::new();
        for put in puts {
            match &put {
                Put::Record(r) => {
                    records.insert((r.ty.clone(), r.key.clone()));
                }
                Put::Declaration(d) => {
                    if let Declaration::Type(t) = &d.declaration {
                        types.insert(t.name.clone());
                    }
                }
                _ => {}
            }
            self.apply(put);
        }
        // The order index follows the head: every record whose latest
        // state the transaction put, and every record of a type it
        // declared again.
        for ty in &types {
            let keys: Vec<RecordKey> = self
                .records
                .range((ty.clone(), RecordKey::from(""))..)
                .take_while(|((t, _), _)| t == ty)
                .map(|((_, k), _)| k.clone())
                .collect();
            records.extend(keys.into_iter().map(|k| (ty.clone(), k)));
        }
        for (ty, key) in records {
            self.index(&ty, &key);
        }
        self.folded = Some(through);
        Ok(())
    }

    /// Rewrite one record's rows in the order index from its latest state,
    /// under its type's declaration at the head.
    fn index(&mut self, ty: &TypeName, key: &RecordKey) {
        for (field, sort_key) in self
            .sorted_rows
            .remove(&(ty.clone(), key.clone()))
            .unwrap_or_default()
        {
            if let Some(set) = self.sorted.get_mut(&(ty.clone(), field)) {
                set.remove(&(sort_key, key.clone()));
            }
        }
        let head = self
            .records
            .get(&(ty.clone(), key.clone()))
            .and_then(History::latest);
        // The declaration in force after every state put: the head's.
        let (Some(record), Some(decl)) = (head, self.type_at(ty, Some(Position(u64::MAX)))) else {
            return;
        };
        let rows: RecordRows = lakelands_core::engines::order_rows(decl, record)
            .into_iter()
            .map(|(f, k)| (f.clone(), k))
            .collect();
        for (field, sort_key) in &rows {
            self.sorted
                .entry((ty.clone(), field.clone()))
                .or_default()
                .insert((sort_key.clone(), key.clone()));
        }
        self.sorted_rows.insert((ty.clone(), key.clone()), rows);
    }

    /// At most `n` rows of the order index for `field` of `ty`, in the
    /// index's order for `dir` (the sort key ascending or descending, the
    /// key ascending either way), strictly after `from`.
    pub(crate) fn sorted_after(
        &self,
        ty: &TypeName,
        field: &FieldName,
        dir: Direction,
        from: Option<(&[u8], &str)>,
        n: u32,
    ) -> Vec<(Vec<u8>, RecordKey)> {
        use std::ops::Bound;
        let Some(set) = self.sorted.get(&(ty.clone(), field.clone())) else {
            return Vec::new();
        };
        let n = n as usize;
        let from = from.map(|(k, key)| (k.to_vec(), RecordKey::from(key)));
        match dir {
            Direction::Asc => {
                let start = match &from {
                    Some(f) => Bound::Excluded(f.clone()),
                    None => Bound::Unbounded,
                };
                set.range((start, Bound::Unbounded))
                    .take(n)
                    .cloned()
                    .collect()
            }
            Direction::Desc => {
                let mut out = Vec::new();
                // The rest of the run `from` is in, then the runs below it,
                // each in key order.
                let below = match &from {
                    Some((k, key)) => {
                        out.extend(
                            set.range((
                                Bound::Excluded((k.clone(), key.clone())),
                                Bound::Unbounded,
                            ))
                            .take_while(|(sk, _)| sk == k)
                            .take(n)
                            .cloned(),
                        );
                        Bound::Excluded((k.clone(), RecordKey::from("")))
                    }
                    None => Bound::Unbounded,
                };
                let mut run: Vec<(Vec<u8>, RecordKey)> = Vec::new();
                for row in set.range((Bound::Unbounded, below)).rev() {
                    if out.len() >= n {
                        break;
                    }
                    if run.first().is_some_and(|(sk, _)| *sk != row.0) {
                        out.extend(run.drain(..).rev().take(n - out.len()));
                    }
                    run.push(row.clone());
                }
                if out.len() < n {
                    out.extend(run.drain(..).rev().take(n - out.len()));
                }
                out.truncate(n);
                out
            }
        }
    }

    /// Whether the head record `key` of `ty` has a row in the order index
    /// for `field`.
    pub(crate) fn has_sorted(&self, ty: &TypeName, key: &RecordKey, field: &FieldName) -> bool {
        self.sorted_rows
            .get(&(ty.clone(), key.clone()))
            .is_some_and(|rows| rows.iter().any(|(f, _)| f == field))
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

    /// At most `n` records of `ty` in their state at `at` after the key
    /// `after` (from the first when `None`), in key order, whatever their
    /// flags.
    pub(crate) fn records_after(
        &self,
        ty: &TypeName,
        at: Option<Position>,
        after: Option<&str>,
        n: u32,
    ) -> Vec<StoredRecord> {
        use std::ops::Bound;
        let from = match after {
            Some(k) => Bound::Excluded((ty.clone(), RecordKey::from(k))),
            None => Bound::Included((ty.clone(), RecordKey::from(""))),
        };
        self.records
            .range((from, Bound::Unbounded))
            .take_while(|((t, _), _)| t == ty)
            .filter_map(|(_, h)| h.at(at))
            .take(n as usize)
            .cloned()
            .collect()
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
