//! Reads of the projection at one position.
//!
//! A view is a position and the lake it reads. Every answer is computed
//! from the states valid at that position, under the lake's lock, so a
//! view keeps answering the same position while the fold moves on.
//!
//! What a view selects, orders, links and scores follows
//! `lakelands_core::engines`, the rules every engine shares; this engine
//! only finds the states they apply to. Edges are derived from the
//! records and the declarations in force at the view's position, and
//! full text from those at the head, kept when live at
//! the view's position; neither is stored: `Out` gives the links a record declares, `In` the links
//! that point at it, `Both` their union with each link once. A link to a
//! retired or missing record is returned as it is; hiding that end is the
//! store's.

use std::sync::{Arc, Mutex};

use lakelands_api::engine::{Declared, ProjectionView, Scan};
use lakelands_api::error::Result;
use lakelands_api::event::{Digest, EventId};
use lakelands_api::id::{Position, RecordKey, RelationName, TypeName, VerbName};
use lakelands_api::query::{Edge, Walk};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::write::{Proposal, ProposalFilter};
use lakelands_core::engines;

use crate::projection::Projection;
use crate::{Lake, lock};

/// A read view of one lake's projection at one position.
pub(crate) struct MemView {
    pub(crate) lake: Arc<Mutex<Lake>>,
    pub(crate) at: Option<Position>,
}

impl MemView {
    fn read<R>(&self, f: impl FnOnce(&Projection, Option<Position>) -> R) -> R {
        f(&lock(&self.lake).projection, self.at)
    }
}

/// The type a verb acts on, as declared at `at`.
fn verb_on(proj: &Projection, at: Option<Position>, verb: &VerbName) -> Option<TypeName> {
    match &proj
        .declarations
        .get(&format!("verb {verb}"))?
        .at(at)?
        .declaration
    {
        lakelands_api::write::Declaration::Verb(v) => Some(v.on.clone()),
        _ => None,
    }
}

impl ProjectionView for MemView {
    fn position(&self) -> Option<Position> {
        self.at
    }

    fn declarations(&self) -> Result<Vec<Declared>> {
        Ok(self.read(|p, at| p.declarations_at(at)))
    }

    fn get(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>> {
        Ok(self.read(|p, at| {
            p.records
                .get(&(ty.clone(), key.clone()))
                .and_then(|h| h.at(at))
                .cloned()
        }))
    }

    fn scan(&self, ty: &TypeName, scan: &Scan) -> Result<Vec<StoredRecord>> {
        Ok(self.read(|p, at| engines::scan(p.type_at(ty, at), p.records_at(ty, at), scan)))
    }

    fn edges(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        relation: Option<&RelationName>,
        walk: Walk,
    ) -> Result<Vec<Edge>> {
        Ok(self.read(|p, at| {
            let mut edges = Vec::new();
            if matches!(walk, Walk::Out | Walk::Both) {
                let record = p
                    .records
                    .get(&(ty.clone(), key.clone()))
                    .and_then(|h| h.at(at));
                if let (Some(decl), Some(record)) = (p.type_at(ty, at), record) {
                    edges.extend(engines::links_of(decl, record, relation));
                }
            }
            if matches!(walk, Walk::In | Walk::Both) {
                let into = engines::in_links(ty, key, relation, p.types_at(at), |name| {
                    Ok::<_, std::convert::Infallible>(p.records_at(name, at).cloned().collect())
                });
                edges.extend(into.unwrap_or_else(|never| match never {}));
            }
            // Relations carried by link records: the links at the view's
            // position whose end is this record.
            let linked = lakelands_core::links::link_edges(
                ty,
                key,
                relation,
                walk,
                p.types_at(at),
                |lt, field, k| {
                    Ok::<_, std::convert::Infallible>(
                        p.records_at(lt, at)
                            .filter(|r| {
                                r.fields.get(field).and_then(|v| v.as_str()) == Some(k.as_str())
                            })
                            .cloned()
                            .collect(),
                    )
                },
            );
            edges.extend(linked.unwrap_or_else(|never| match never {}));
            engines::order_edges(&mut edges);
            edges
        }))
    }

    fn lexical(&self, ty: &TypeName, text: &str, k: u32) -> Result<Vec<(RecordKey, f64)>> {
        Ok(self.read(|p, at| {
            // Ranked by the text at the head; kept when live and
            // live at the view's position (`docs/seams.md`).
            let Some(decl) = p.type_at(ty, p.folded) else {
                return Vec::new();
            };
            let visible = |key: &RecordKey| {
                engines::searchable(
                    p.records
                        .get(&(ty.clone(), key.clone()))
                        .and_then(|h| h.at(at)),
                )
            };
            engines::lexical(decl, p.records_at(ty, p.folded), visible, text, k)
        }))
    }

    fn proposals(&self, filter: &ProposalFilter) -> Result<Vec<Proposal>> {
        Ok(self.read(|p, at| {
            engines::proposals(p.proposals.values().filter_map(|h| h.at(at)), filter, |v| {
                verb_on(p, at, v)
            })
        }))
    }

    fn is_member(&self, set: &str, ty: &TypeName, key: &RecordKey) -> Result<bool> {
        Ok(self.read(|p, at| {
            p.members
                .get(&(set.to_owned(), ty.clone(), key.clone()))
                .and_then(|h| h.at(at))
                .copied()
                .unwrap_or(false)
        }))
    }

    fn applied(&self, id: &EventId) -> Result<Option<Position>> {
        Ok(self.read(|p, at| {
            p.applied
                .get(id)
                .copied()
                .filter(|applied| at.is_some_and(|at| *applied <= at))
        }))
    }

    fn output(&self, seam: &str, input: &Digest) -> Result<Option<Value>> {
        Ok(self.read(|p, at| {
            p.outputs
                .get(&(seam.to_owned(), input.clone()))
                .and_then(|h| h.at(at))
                .cloned()
        }))
    }
}
