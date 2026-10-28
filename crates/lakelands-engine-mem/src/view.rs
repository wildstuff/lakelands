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

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use lakelands_api::engine::{Chosen, Declared, Matched, ProjectionView, Scan};
use lakelands_api::error::Result;
use lakelands_api::event::{Digest, EventId};
use lakelands_api::id::{FieldName, Position, RecordKey, RelationName, SourceId, TypeName};
use lakelands_api::quarantine::{QuarantineFilter, QuarantineId, Quarantined};
use lakelands_api::query::{Edge, Walk};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::seam::FeedFilter;
use lakelands_api::write::{Proposal, ProposalFilter};
use lakelands_core::engines;
use lakelands_core::feed::{self, Seek};

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
/// The type the verb, trigger or set in force under `ident` stands on.
fn stands(proj: &Projection, at: Option<Position>, ident: &str) -> Option<TypeName> {
    use lakelands_api::write::Declaration;
    match &proj.declarations.get(ident)?.at(at)?.declaration {
        Declaration::Verb(v) => Some(v.on.clone()),
        Declaration::Trigger(t) => Some(t.on.clone()),
        Declaration::ObjectSet(s) => Some(s.of.clone()),
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

    fn chosen(&self, ty: &TypeName) -> Result<Chosen> {
        Ok(self.read(|p, _| p.chosen(ty)))
    }

    fn replaced(&self, from: Position, to: Position) -> Result<Option<Vec<Declared>>> {
        Ok(Some(self.read(|p, at| {
            p.replaced(from, at.map_or(to, |a| a.min(to)))
        })))
    }

    fn get(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>> {
        Ok(self.read(|p, at| p.record_of(ty, key, at)))
    }

    fn get_many(&self, ty: &TypeName, keys: &[RecordKey]) -> Result<Vec<StoredRecord>> {
        Ok(self.read(|p, at| {
            let keys: std::collections::BTreeSet<&RecordKey> = keys.iter().collect();
            keys.into_iter()
                .filter_map(|key| p.record_of(ty, key, at))
                .collect()
        }))
    }

    fn count(&self, ty: &TypeName, scan: &Scan) -> Result<u64> {
        Ok(self.read(|p, at| {
            let count: std::result::Result<_, std::convert::Infallible> =
                engines::count_in_key_order(p.type_at(ty, at), scan, |after, n| {
                    Ok(p.records_after(ty, at, after, n))
                });
            count.unwrap_or_else(|never| match never {})
        }))
    }

    fn scan(&self, ty: &TypeName, scan: &Scan) -> Result<Vec<StoredRecord>> {
        Ok(self.read(|p, at| {
            let decl = p.type_at(ty, at);
            if scan.order_by.is_empty() {
                // Ordered by the key alone: read from `after`, stop when full.
                let page: std::result::Result<_, std::convert::Infallible> =
                    engines::scan_in_key_order(decl, scan, |after, n| {
                        Ok(p.records_after(ty, at, after, n))
                    });
                return page.unwrap_or_else(|never| match never {});
            }
            // Ordered by a field the lake indexes: read in the order index's
            // order, stop when full; at an earlier position, with the keys
            // changed since, and past the ceiling, sort the type.
            let sorted = if at.is_some() {
                p.read_fields(ty)
            } else {
                Vec::new()
            };
            if let Some(first) = engines::by_order_index(&sorted, scan) {
                let changed = match at {
                    Some(at) if Some(at) != p.folded => {
                        match p.changed(ty, at, engines::changed_ceiling(scan.limit)) {
                            Some(changed) => Some(changed),
                            None => {
                                let all: Vec<_> = p.records_at(ty, Some(at)).collect();
                                return engines::scan(decl, &all, scan);
                            }
                        }
                    }
                    _ => None,
                };
                // The index and the records without a row answer the head,
                // a key put since included: the changed keys' states at
                // `at` take their place.
                let head = |key: &RecordKey| p.record_of(ty, key, p.folded);
                let sorted = |from: Option<(&[u8], &str)>, n| {
                    Ok(p.sorted_after(ty, &first.field, first.dir, from, n)
                        .into_iter()
                        .filter_map(|(k, key)| Some((k, head(&key)?)))
                        .collect())
                };
                let missing = |after: Option<&str>, n| {
                    let mut out = Vec::new();
                    let mut from = after.map(str::to_owned);
                    // Key-ordered batches, less the records the index holds
                    // for the field.
                    loop {
                        let batch = p.records_after(ty, p.folded, from.as_deref(), n);
                        let done = batch.len() < n as usize;
                        for r in batch {
                            from = Some(r.key.to_string());
                            if !p.has_sorted(ty, &r.key, &first.field) {
                                out.push(r);
                                if out.len() >= n as usize {
                                    return Ok(out);
                                }
                            }
                        }
                        if done {
                            return Ok(out);
                        }
                    }
                };
                let page: std::result::Result<_, std::convert::Infallible> = match &changed {
                    None => engines::scan_in_field_order(decl, scan, sorted, missing),
                    Some(c) => engines::scan_in_field_order_at(decl, scan, c, sorted, missing),
                };
                return page.unwrap_or_else(|never| match never {});
            }
            engines::scan(decl, &p.records_at(ty, at).collect::<Vec<_>>(), scan)
        }))
    }

    fn edges(
        &self,
        ty: &TypeName,
        keys: &[RecordKey],
        relation: Option<&RelationName>,
        walk: Walk,
    ) -> Result<Vec<Vec<Edge>>> {
        Ok(self.read(|p, at| {
            // At the head the edges into a record, and the link records
            // whose end it is, are kept by it (`projection::index`); at an
            // earlier position the declaring types and link types are read.
            let head = at.is_some() && at == p.folded;
            let mut all: Vec<Vec<Edge>> = keys
                .iter()
                .map(|key| {
                    let mut edges = Vec::new();
                    if matches!(walk, Walk::Out | Walk::Both) {
                        let record = p.record_of(ty, key, at);
                        if let (Some(decl), Some(record)) = (p.type_at(ty, at), record) {
                            edges.extend(engines::links_of(decl, &record, relation));
                        }
                    }
                    if matches!(walk, Walk::In | Walk::Both) {
                        if head {
                            edges.extend(
                                p.in_edges
                                    .get(&(ty.clone(), key.clone()))
                                    .into_iter()
                                    .flat_map(|into| into.values())
                                    .filter(|e| relation.is_none_or(|want| *want == e.relation))
                                    .cloned(),
                            );
                        } else {
                            let into =
                                engines::in_links(ty, key, relation, p.types_at(at), |name| {
                                    Ok::<_, std::convert::Infallible>(
                                        p.records_at(name, at).collect(),
                                    )
                                });
                            edges.extend(into.unwrap_or_else(|never| match never {}));
                        }
                    }
                    edges
                })
                .collect();
            // Relations carried by link records: the links at the view's
            // position whose end is one of the records.
            let linked = lakelands_core::links::link_edges(
                ty,
                keys,
                relation,
                walk,
                p.types_at(at),
                |lt, field, ks| {
                    Ok::<_, std::convert::Infallible>(if head {
                        ks.iter()
                            .flat_map(|k| {
                                p.link_ends
                                    .get(&(lt.clone(), FieldName::from(field), k.to_string()))
                                    .into_iter()
                                    .flatten()
                            })
                            .filter_map(|lk| p.record_of(lt, lk, at))
                            .collect()
                    } else {
                        // The link type at the position, whole: the
                        // rulebook keeps the links whose end a record is.
                        p.records_at(lt, at).collect()
                    })
                },
            );
            for (edges, linked) in all
                .iter_mut()
                .zip(linked.unwrap_or_else(|never| match never {}))
            {
                edges.extend(linked);
                engines::order_edges(edges);
            }
            all
        }))
    }

    fn proposals_before(
        &self,
        filter: &ProposalFilter,
        before: Option<Position>,
    ) -> Result<Option<Vec<Proposal>>> {
        let limit = filter.limit.map_or(usize::MAX, |l| l as usize);
        Ok(Some(self.read(|p, at| {
            let ids = p
                .by_position
                .range(..before.unwrap_or(Position(u64::MAX)))
                .rev();
            let states = ids.filter_map(|(_, id)| p.proposals.get(id)?.at(at));
            let kept = states.filter(|q| filter.state.is_none_or(|s| q.state == s));
            kept.take(limit).cloned().collect()
        })))
    }

    fn proposals(&self, filter: &ProposalFilter) -> Result<Vec<Proposal>> {
        Ok(self.read(|p, at| {
            engines::proposals(p.proposals.values().filter_map(|h| h.at(at)), filter, |i| {
                stands(p, at, i)
            })
        }))
    }

    fn quarantined(&self, filter: &QuarantineFilter) -> Result<Vec<Quarantined>> {
        Ok(self.read(|p, at| {
            let rows = |id: &QuarantineId| p.quarantined.get(id)?.at(at).cloned();
            match (&filter.ty, &filter.key) {
                (Some(ty), Some(key)) => {
                    let ids = p.quarantined_at.get(&(ty.clone(), key.clone()));
                    filter.answer(ids.into_iter().flatten().filter_map(rows))
                }
                _ => filter.answer(p.quarantined.keys().filter_map(rows)),
            }
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

    fn members(&self, set: &str, ty: &TypeName) -> Result<BTreeSet<RecordKey>> {
        Ok(self.read(|p, at| {
            let of = p
                .members
                .iter()
                .filter(|((s, t, _), _)| s == set && t == ty);
            of.filter(|(_, h)| h.at(at) == Some(&true))
                .map(|((_, _, key), _)| key.clone())
                .collect()
        }))
    }

    fn applied(&self, id: &EventId) -> Result<Option<Position>> {
        Ok(self.applied_many(std::slice::from_ref(id))?.remove(0))
    }

    fn applied_many(&self, ids: &[EventId]) -> Result<Vec<Option<Position>>> {
        Ok(self.read(|p, at| {
            let at = |applied: &Position| at.is_some_and(|at| *applied <= at);
            (ids.iter())
                .map(|id| p.applied.get(id).copied().filter(at))
                .collect()
        }))
    }

    fn output(&self, seam: &str, input: &Digest) -> Result<Option<(Position, Value)>> {
        Ok(self.read(|p, at| {
            p.outputs
                .get(&(seam.to_owned(), input.clone()))
                .and_then(|h| h.since(at))
                .map(|(since, v)| (since, v.clone()))
        }))
    }

    fn matching(
        &self,
        filter: &FeedFilter,
        from: Position,
        through: Position,
        limit: u32,
    ) -> Result<Matched> {
        Ok(self.read(|p, at| {
            let r = &p.routes;
            let to = feed::decides_through(filter, from, through);
            let Some(last) = at.and_then(|at| feed::last_decided(from, at.min(to), p.folded))
            else {
                return feed::undecided();
            };
            let n = limit.max(1) as usize;
            let admitted = |set: Option<&BTreeSet<Position>>| -> Vec<Position> {
                let Some(set) = set else {
                    return Vec::new();
                };
                let admits = |p: &&Position| {
                    let (kind, about) = &r.events[*p];
                    feed::admits(filter, kind, about)
                };
                set.range(from..=last)
                    .filter(admits)
                    .take(n)
                    .copied()
                    .collect()
            };
            let lists: Vec<Vec<Position>> = match feed::seek(filter) {
                Some(Seek::Keys(keys)) => keys.iter().map(|k| admitted(r.keys.get(k))).collect(),
                Some(Seek::Types(types)) => {
                    types.iter().map(|t| admitted(r.types.get(t))).collect()
                }
                Some(Seek::Kinds(kinds)) => kinds
                    .iter()
                    .map(|k| admitted(r.kinds.get(k.as_str())))
                    .collect(),
                None => {
                    vec![
                        r.events
                            .range(from..=last)
                            .map(|(p, _)| *p)
                            .take(n)
                            .collect(),
                    ]
                }
            };
            feed::merged(lists, limit, last)
        }))
    }

    fn of_source(
        &self,
        source: &SourceId,
        from: Position,
        through: Position,
        limit: u32,
    ) -> Result<Matched> {
        Ok(self.read(|p, at| {
            let Some(last) = at.and_then(|at| feed::last_decided(from, at.min(through), p.folded))
            else {
                return feed::undecided();
            };
            let set = p.routes.sources.get(source).into_iter();
            let positions = set.flat_map(|s| s.range(from..=last).take(limit.max(1) as usize));
            feed::merged([positions.copied().collect()], limit, last)
        }))
    }
}
