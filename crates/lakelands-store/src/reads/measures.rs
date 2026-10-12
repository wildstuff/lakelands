//! The whole-graph measures of the pattern query: degree centrality and
//! connected components, answered as rows.
//!
//! The graph is the one every walk reads through [`Visible`]: link records
//! and via references at the read's position, far ends the principal may
//! see, restricted links only with the grant. The start's records are the
//! seeds, read a page at a time, so a degree over a whole type holds one
//! page and the best rows asked for, never the type; components hold what
//! they reach, up to the lake's `max_graph_nodes`
//! (`docs/decisions/026-the-store-walks-the-graph.md`).

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

use lakelands_api::error::{Case, Place};
use lakelands_api::query::{Depth, Pattern, Projection, QueryResult, Walk};
use lakelands_api::record::StoredRecord;
use lakelands_api::{Cursor, Params, RecordKey, RelationName, Result, Value};
use lakelands_core::graph::{self, Components, Node};

use super::{CursorState, Judged, Pulled, Reader, Visible, encode, invalid};

/// The records a seed page reads at once.
const PAGE: usize = 256;

/// Whether a projection is a whole-graph measure.
pub(super) fn is_measure(p: &Projection) -> bool {
    matches!(p, Projection::Degree | Projection::Components)
}

/// What a measure refuses: a member it would otherwise ignore, so that an
/// answer never looks like more than it is.
pub(super) fn check(p: &Pattern) -> Result<()> {
    let measure = match p.project {
        Projection::Degree => "degree",
        _ => "components",
    }
    .to_owned();
    if p.to.is_some() {
        return Err(invalid(Case::MeasureNoTo { measure }));
    }
    if !p.order_by.is_empty() {
        return Err(invalid(Case::MeasureNoOrder { measure }));
    }
    if p.steps.is_empty() {
        return Err(invalid(Case::MeasureNoSteps { measure }));
    }
    for step in &p.steps {
        if step.depth != Depth::default() {
            return Err(invalid(Case::MeasureNoDepth { measure }));
        }
        if step.filter.is_some() {
            return Err(invalid(Case::MeasureNoWhere { measure }));
        }
        // It reads every link of a relation, whatever type it reaches.
        if step.ty.is_some() {
            return Err(invalid(Case::MeasureNoType { measure }));
        }
        if p.project == Projection::Components && step.walk != Walk::Both {
            let relation = step.relation.clone();
            return Err(invalid(Case::ComponentsBothWays { relation }));
        }
    }
    Ok(())
}

impl Reader<'_> {
    /// A measure's answer; the pattern has passed [`check`] and its steps
    /// have been checked against the declarations. A start by search
    /// judges `carried`, or hands a pull back, as `search` does.
    pub(super) fn measure(
        &self,
        p: &Pattern,
        limit: usize,
        carried: Option<Pulled>,
        hidden: &Visible<'_, '_>,
    ) -> Result<Judged<QueryResult>> {
        let params = self.bound_params(&p.start.ty);
        let offset = super::offset(p.cursor.as_ref(), p, &params)?.0;
        let all = match p.project {
            Projection::Degree => {
                let steps: Vec<(RelationName, Walk)> = p
                    .steps
                    .iter()
                    .map(|s| (s.relation.clone(), s.walk))
                    .collect();
                // The best `offset + limit + 1` rows by (degree desc, key):
                // the heap's top is the one ranking last.
                let keep = offset + limit + 1;
                let mut best: BinaryHeap<(Reverse<u64>, RecordKey)> = BinaryHeap::new();
                // The seeds come a page at a time; their degrees are read
                // a page at a time too.
                let mut seeds: Vec<Node> = Vec::with_capacity(graph::PAGE);
                let mut rank = |seeds: &mut Vec<Node>| -> Result<()> {
                    let degrees = graph::degrees(hidden, seeds, &steps)?;
                    for (node, d) in seeds.drain(..).zip(degrees) {
                        best.push((Reverse(d), node.1));
                        if best.len() > keep {
                            best.pop();
                        }
                    }
                    Ok(())
                };
                let moved = self.seeds(p, carried, |node| {
                    seeds.push(node);
                    if seeds.len() == graph::PAGE {
                        rank(&mut seeds)?;
                    }
                    Ok(())
                })?;
                if let Some(moved) = moved {
                    return Ok(Err(moved));
                }
                rank(&mut seeds)?;
                let ty = &p.start.ty;
                best.into_sorted_vec()
                    .into_iter()
                    .map(|(Reverse(d), key)| {
                        BTreeMap::from([
                            ("type".to_owned(), Value::String(ty.to_string())),
                            ("key".to_owned(), Value::String(key.to_string())),
                            ("degree".to_owned(), Value::from(d)),
                        ])
                    })
                    .collect()
            }
            _ => {
                let relations: Vec<RelationName> =
                    p.steps.iter().map(|s| s.relation.clone()).collect();
                let max = self.lake.limits.max_graph_nodes;
                let mut found = Components::new(hidden, relations, max);
                // The seeds come a page at a time, and grow a page at a time.
                let mut seeds: Vec<Node> = Vec::with_capacity(graph::PAGE);
                let moved = self.seeds(p, carried, |node| {
                    seeds.push(node);
                    if seeds.len() == graph::PAGE {
                        found.add(&seeds)?;
                        seeds.clear();
                    }
                    Ok(())
                })?;
                if let Some(moved) = moved {
                    return Ok(Err(moved));
                }
                found.add(&seeds)?;
                let mut all = Vec::new();
                for (i, component) in found.finish().into_iter().enumerate() {
                    let size = component.len();
                    for (ty, key) in component {
                        all.push(BTreeMap::from([
                            ("component".to_owned(), Value::from(i)),
                            ("size".to_owned(), Value::from(size)),
                            ("type".to_owned(), Value::String(ty.to_string())),
                            ("key".to_owned(), Value::String(key.to_string())),
                        ]));
                    }
                }
                all
            }
        };
        Ok(Ok(self.rows_page(p, all, offset, limit, params)))
    }

    /// One page of a pattern's rows already in their order, and the cursor
    /// that continues it when more follow.
    pub(super) fn rows_page(
        &self,
        p: &Pattern,
        all: Vec<BTreeMap<String, Value>>,
        offset: usize,
        limit: usize,
        params: Params,
    ) -> QueryResult {
        let next_cursor = (all.len() > offset + limit)
            .then(|| self.page_cursor(p, (offset + limit, None), params));
        let rows = all.into_iter().skip(offset).take(limit).collect();
        QueryResult::Rows { rows, next_cursor }
    }

    /// The cursor that continues a pattern's answer past the start `after`
    /// names, where it names one, and `offset` rows.
    pub(super) fn page_cursor(
        &self,
        p: &Pattern,
        (offset, after): (usize, Option<super::SortKey>),
        params: Params,
    ) -> Cursor {
        encode(&CursorState {
            at: self.position,
            after,
            offset: offset as u64,
            params,
            door: "query".into(),
            ty: Some(p.start.ty.clone()),
            of: super::pattern_read(p),
        })
    }

    /// Hands every seed of the pattern's start to `each`, in key order: the
    /// one record a key names, what a search finds, or the records a filter
    /// (or none) selects, read a page at a time. A search's pull read at
    /// another position than the read's head is handed back, and no seed.
    fn seeds(
        &self,
        p: &Pattern,
        carried: Option<Pulled>,
        mut each: impl FnMut(Node) -> Result<()>,
    ) -> Result<Option<Pulled>> {
        let decl = self.named_type(&p.start.ty)?.clone();
        let ty = decl.name.clone();
        if let Some(key) = &p.start.key {
            if self.start_found(p, key)? {
                each((ty, key.clone()))?;
            }
            return Ok(None);
        }
        if let Some(mode) = &p.start.search {
            let mut keys = match self.start_hits(p, mode, carried)? {
                Ok(keys) => keys,
                Err(moved) => return Ok(Some(moved)),
            };
            keys.sort();
            for key in keys {
                each((ty.clone(), key))?;
            }
            return Ok(None);
        }
        let filter = self.record_filter(
            &decl,
            p.start.filter.as_ref(),
            &[Place::Start, Place::Where],
        )?;
        let seed = |r: &StoredRecord| each((r.ty.clone(), r.key.clone()));
        self.each_member(&decl, filter.as_ref(), PAGE, seed)?;
        Ok(None)
    }
}
