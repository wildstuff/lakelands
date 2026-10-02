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

use lakelands_api::query::{Depth, Pattern, Projection, QueryResult, Search, Walk};
use lakelands_api::read::{Get, ReadAt};
use lakelands_api::{Error, RecordKey, RelationName, Result, Value};
use lakelands_core::expr::Place;
use lakelands_core::graph::{self, Components, Node};

use super::{CursorState, Reader, Visible, condition, core_derive, encode, invalid, rights};

/// The records a seed page reads at once.
const PAGE: usize = 256;

/// Whether a projection is a whole-graph measure.
pub(super) fn is_measure(p: &Projection) -> bool {
    matches!(p, Projection::Degree | Projection::Components)
}

/// What a measure refuses: a member it would otherwise ignore, so that an
/// answer never looks like more than it is.
pub(super) fn check(p: &Pattern) -> Result<()> {
    let what = match p.project {
        Projection::Degree => "degree",
        _ => "components",
    };
    let refuse = |member: &str| {
        Err(invalid(format!(
            "`{what}` is a measure over the steps' relations and takes no {member}"
        )))
    };
    if p.to.is_some() {
        return refuse("`to`");
    }
    if !p.order_by.is_empty() {
        return refuse("`order_by`: its rows come in the measure's own order");
    }
    if p.steps.is_empty() {
        return Err(invalid(format!(
            "`{what}` walks the relations its steps name; name at least one"
        )));
    }
    for step in &p.steps {
        if step.depth != Depth::default() {
            return refuse("step `depth`: it reads each record's links, not hops");
        }
        if step.filter.is_some() {
            return refuse("step `where`");
        }
        if p.project == Projection::Components && step.walk != Walk::Both {
            return Err(invalid(format!(
                "`components` takes every link both ways; the step over `{}` must walk `both`",
                step.relation
            )));
        }
    }
    Ok(())
}

impl Reader<'_> {
    /// A measure's answer; the pattern has passed [`check`] and its steps
    /// have been checked against the declarations.
    pub(super) fn measure(&self, p: &Pattern, limit: usize) -> Result<QueryResult> {
        let params = self.bound_params(&p.start.ty);
        let offset = super::offset(p.cursor.as_ref(), &p.start.ty, &params)?;
        let hidden = Visible { reader: self };
        let (rows, more) = match p.project {
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
                    let degrees = graph::degrees(&hidden, seeds, &steps)?;
                    for (node, d) in seeds.drain(..).zip(degrees) {
                        best.push((Reverse(d), node.1));
                        if best.len() > keep {
                            best.pop();
                        }
                    }
                    Ok(())
                };
                self.seeds(p, |node| {
                    seeds.push(node);
                    if seeds.len() == graph::PAGE {
                        rank(&mut seeds)?;
                    }
                    Ok(())
                })?;
                rank(&mut seeds)?;
                let ty = &p.start.ty;
                let all: Vec<BTreeMap<String, Value>> = best
                    .into_sorted_vec()
                    .into_iter()
                    .map(|(Reverse(d), key)| {
                        BTreeMap::from([
                            ("type".to_owned(), Value::String(ty.to_string())),
                            ("key".to_owned(), Value::String(key.to_string())),
                            ("degree".to_owned(), Value::from(d)),
                        ])
                    })
                    .collect();
                page(all, offset, limit)
            }
            _ => {
                let relations: Vec<RelationName> =
                    p.steps.iter().map(|s| s.relation.clone()).collect();
                let mut found =
                    Components::new(&hidden, relations, self.lake.limits.max_graph_nodes);
                self.seeds(p, |node| found.add(node))?;
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
                page(all, offset, limit)
            }
        };
        let next_cursor = more.then(|| {
            encode(&CursorState {
                at: self.position,
                after: None,
                offset: (offset + limit) as u64,
                params,
                door: "query".into(),
                ty: Some(p.start.ty.clone()),
            })
        });
        Ok(QueryResult::Rows { rows, next_cursor })
    }

    /// Hands every seed of the pattern's start to `each`, in key order: the
    /// one record a key names, what a search finds, or the records a filter
    /// (or none) selects, read a page at a time.
    fn seeds(&self, p: &Pattern, mut each: impl FnMut(Node) -> Result<()>) -> Result<()> {
        let decl = self.visible_type(&p.start.ty)?.clone();
        let ty = decl.name.clone();
        if let Some(key) = &p.start.key {
            let get = Get {
                key: key.clone(),
                with_meta: false,
                at: ReadAt::default(),
                params: p.start.params.clone(),
            };
            return match self.get(&ty, &get) {
                Ok(_) => each((ty, key.clone())),
                Err(Error::NotFound { .. }) => Ok(()),
                Err(e) => Err(e),
            };
        }
        if let Some(mode) = &p.start.search {
            let s = Search {
                ty: ty.clone(),
                mode: mode.clone(),
                k: self.lake.limits.max_limit,
                cursor: None,
                filter: p.start.filter.clone(),
                with_meta: false,
                at: ReadAt::default(),
            };
            let mut keys: Vec<RecordKey> = self
                .search(&s)?
                .hits
                .into_iter()
                .map(|h| h.record.key)
                .collect();
            keys.sort();
            for key in keys {
                each((ty.clone(), key))?;
            }
            return Ok(());
        }
        let filter = p
            .start
            .filter
            .as_ref()
            .map(|c| condition(c, Place::Record, "start where"))
            .transpose()?;
        if let Some(e) = &filter {
            rights::check_probe(self.who, &decl, core_derive::reads(e).iter())?;
        }
        if decl.view.is_some() {
            for r in self.records(&decl, filter.as_ref())? {
                each((r.ty, r.key))?;
            }
            return Ok(());
        }
        let mut after = None;
        loop {
            let rows = self.page(&decl, filter.as_ref(), &[], after.take(), PAGE)?;
            let last_page = rows.len() < PAGE;
            if let Some(last) = rows.last() {
                after = Some((Vec::new(), last.key.clone()));
            }
            for r in rows {
                each((r.ty, r.key))?;
            }
            if last_page {
                return Ok(());
            }
        }
    }
}

/// One page of rows already in their order, and whether more follow.
fn page(
    all: Vec<BTreeMap<String, Value>>,
    offset: usize,
    limit: usize,
) -> (Vec<BTreeMap<String, Value>>, bool) {
    let more = all.len() > offset + limit;
    (all.into_iter().skip(offset).take(limit).collect(), more)
}
