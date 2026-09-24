//! Reads of the projection at one position.
//!
//! A view is a position and the lake it reads. Every answer is computed
//! from the states valid at that position, under the lake's lock, so a
//! view keeps answering the same position while the fold moves on.
//!
//! What a view derives rather than stores:
//!
//! - **Edges** come from records and the relations in force. A relation
//!   carried by a reference field (`via`) links a record to the record of
//!   the relation's `target` whose key is that field's value, when the
//!   value is text. A relation without `via` has no links to store in
//!   this engine and yields none. `Out` gives the links a record
//!   declares, `In` the links that point at it, `Both` their union with
//!   each link once. A link to a retired or missing record is returned as
//!   it is; hiding that end is the store's.
//! - **Full text** is the store's one token pipeline
//!   (`lakelands_core::stem::stem_text`) over a type's searchable text
//!   fields, stemmed by the type's language. A record scores the number of
//!   distinct query tokens it holds; records holding none are not hits.
//!   Only live, unrestricted records are searched.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use lakelands_api::engine::{Declared, ProjectionView, Scan};
use lakelands_api::error::Result;
use lakelands_api::event::{Digest, EventId};
use lakelands_api::expr::Expr;
use lakelands_api::id::{Position, RecordKey, RelationName, TypeName};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::query::{Edge, Walk};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::time::Timestamp;
use lakelands_api::write::{Proposal, ProposalFilter, ProposedWrite};
use lakelands_core::expr::{RecordScope, holds};

use crate::order::{Place, cmp_places};
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

/// The clock a filter is evaluated with. The store hands the engine only
/// filters that do not read the clock, so any fixed instant will do.
const NO_CLOCK: Timestamp = Timestamp(0);

/// Whether `filter` selects `record`; a filter that cannot be evaluated
/// does not.
fn selects(filter: Option<&Expr>, record: &StoredRecord) -> bool {
    filter.is_none_or(|f| {
        let scope = RecordScope {
            fields: &record.fields,
            settings: None,
            now: NO_CLOCK,
        };
        holds(f, &scope).unwrap_or(false)
    })
}

/// Whether `record` holds at `valid_at` under its type's declaration. A
/// type not in force is not valid-timed.
fn valid(decl: Option<&TypeDecl>, record: &StoredRecord, valid_at: Option<Timestamp>) -> bool {
    match (decl, valid_at) {
        (Some(decl), Some(t)) => lakelands_core::valid_time::record_holds(decl, &record.fields, t),
        _ => true,
    }
}

/// The links `record` declares over the relations of `decl`.
fn links_of(decl: &TypeDecl, record: &StoredRecord, relation: Option<&RelationName>) -> Vec<Edge> {
    decl.relations
        .iter()
        .filter(|r| relation.is_none_or(|want| *want == r.name))
        .filter_map(|r| {
            let to = record.fields.get(r.via.as_ref()?)?.as_str()?;
            Some(Edge {
                from: (record.ty.clone(), record.key.clone()),
                relation: r.name.clone(),
                to: (r.target.clone(), RecordKey::from(to)),
                fields: BTreeMap::new(),
            })
        })
        .collect()
}

/// The distinct tokens of `text` under the language `tag`.
fn tokens(text: &str, tag: &str) -> BTreeSet<String> {
    lakelands_core::stem::stem_text(text, tag)
        .split(' ')
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The type a proposal's write touches, if the view can tell.
fn proposal_type(p: &Proposal, proj: &Projection, at: Option<Position>) -> Option<TypeName> {
    match &p.write {
        ProposedWrite::Upsert { ty, .. } | ProposedWrite::Retire { ty, .. } => Some(ty.clone()),
        ProposedWrite::Call { verb, .. } => proj
            .declarations
            .get(&format!("verb {verb}"))
            .and_then(|h| h.at(at))
            .and_then(|d| match &d.declaration {
                lakelands_api::write::Declaration::Verb(v) => Some(v.on.clone()),
                _ => None,
            }),
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
        Ok(self.read(|p, at| {
            let decl = p.type_at(ty, at);
            let mut rows: Vec<&StoredRecord> = p
                .records_at(ty, at)
                .filter(|r| (scan.retired || !r.retired) && (scan.restricted || !r.restricted))
                .filter(|r| valid(decl, r, scan.valid_at))
                .filter(|r| selects(scan.filter.as_ref(), r))
                .collect();
            rows.sort_by(|a, b| {
                cmp_places(
                    &Place::of(a, &scan.order_by),
                    &Place::of(b, &scan.order_by),
                    &scan.order_by,
                )
            });
            if let Some(after) = &scan.after {
                let after = Place::after(after);
                rows.retain(|r| {
                    cmp_places(&Place::of(r, &scan.order_by), &after, &scan.order_by).is_gt()
                });
            }
            rows.into_iter()
                .take(scan.limit as usize)
                .cloned()
                .collect()
        }))
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
                    edges.extend(links_of(decl, record, relation));
                }
            }
            if matches!(walk, Walk::In | Walk::Both) {
                for decl in p.types_at(at) {
                    if !decl
                        .relations
                        .iter()
                        .any(|r| r.target == *ty && r.via.is_some())
                    {
                        continue;
                    }
                    for record in p.records_at(&decl.name, at) {
                        edges.extend(
                            links_of(decl, record, relation)
                                .into_iter()
                                .filter(|e| e.to.0 == *ty && e.to.1 == *key),
                        );
                    }
                }
            }
            let mut seen = BTreeSet::new();
            edges.retain(|e| seen.insert((e.from.clone(), e.relation.clone(), e.to.clone())));
            edges
        }))
    }

    fn lexical(&self, ty: &TypeName, text: &str, k: u32) -> Result<Vec<(RecordKey, f64)>> {
        Ok(self.read(|p, at| {
            let Some(decl) = p.type_at(ty, at) else {
                return Vec::new();
            };
            let tag = decl.language.as_deref().unwrap_or("");
            let query = tokens(text, tag);
            let searchable: Vec<_> = decl
                .fields
                .iter()
                .filter(|f| f.searchable)
                .map(|f| &f.name)
                .collect();
            let mut hits: Vec<(RecordKey, f64)> = p
                .records_at(ty, at)
                .filter(|r| !r.retired && !r.restricted)
                .filter_map(|r| {
                    let held: BTreeSet<String> = searchable
                        .iter()
                        .filter_map(|f| r.fields.get(*f)?.as_str())
                        .flat_map(|s| tokens(s, tag))
                        .collect();
                    let score = query.intersection(&held).count();
                    (score > 0).then(|| (r.key.clone(), score as f64))
                })
                .collect();
            hits.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            hits.truncate(k as usize);
            hits
        }))
    }

    fn proposals(&self, filter: &ProposalFilter) -> Result<Vec<Proposal>> {
        Ok(self.read(|p, at| {
            let mut found: Vec<&Proposal> = p
                .proposals
                .values()
                .filter_map(|h| h.at(at))
                .filter(|pr| filter.state.is_none_or(|s| pr.state == s))
                .filter(|pr| {
                    filter
                        .ty
                        .as_ref()
                        .is_none_or(|ty| proposal_type(pr, p, at).as_ref() == Some(ty))
                })
                .collect();
            found.sort_by(|a, b| b.position.cmp(&a.position).then_with(|| a.id.cmp(&b.id)));
            found
                .into_iter()
                .take(filter.limit.map_or(usize::MAX, |l| l as usize))
                .cloned()
                .collect()
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
