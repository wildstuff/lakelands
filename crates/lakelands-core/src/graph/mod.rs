//! The graph walks a pattern query runs: one step's multi-hop walk,
//! a chain of steps, a bounded neighbourhood, a shortest path, and the
//! whole-graph measures (degree centrality, connected components).
//!
//! Every engine keeps a lake's links and answers one narrow question
//! about them, [`ProjectionView::edges`]: the links of some records of
//! one type, over one relation or all, in one direction, one list per
//! record. The walks are written once, here, over that question and one
//! more (may these records appear?), so every engine walks the same way
//! and returns the same answer. An engine never implements a walk of its
//! own.
//!
//! Both questions are asked of a [`PAGE`] of records at a time: a walk
//! expands one hop of its frontier, a measure one page of its records,
//! together. That keeps an engine whose every read is a round trip from
//! paying one per record (lakelands#227). What a walk answers does not
//! depend on it: every hop is expanded whole before the next.
//!
//! ## The rules every walk keeps
//!
//! - **Order is part of the answer.** [`EdgeSource`] promises no order;
//!   every walk sorts what it is given. Records are ordered by
//!   `(type, key)`, reached records by `(depth, type, key)`, edges by
//!   `(from, relation, to)`. Two engines holding the same links return
//!   the same answer, byte for byte, and a caller paging or diffing
//!   results never sees a reordering as a change.
//! - **Visibility is asked of the far endpoint.** A walk follows a link
//!   only if the record at its far end may appear
//!   ([`EdgeSource::visible`]), asked once per record per walk. Through
//!   a projection view that means the record exists at the view's
//!   position and is not retired. A restricted record is the caller's
//!   concern: the store wraps the view in an [`EdgeSource`] that also
//!   hides restricted records from a principal without the grant. The start of a walk is not checked,
//!   because the caller has already read it; only a link that leads back
//!   to it asks about it, like any other far endpoint.
//! - **Cycles terminate by construction.** Every walk keeps the set of
//!   records it has reached and never expands one twice, and every walk
//!   is bounded by a depth the caller names.
//! - **Depth is the fewest hops.** A record reachable along several
//!   routes is reported once, at the shortest distance any route
//!   reaches it.
//! - **A depth over the lake's ceiling is refused, not clamped.** Each
//!   walk is handed the lake's `max_depth`; a step that asks for more,
//!   or whose range is empty, is refused as invalid rather than quietly
//!   answered with less than it asked for.

mod measures;
mod path;
mod step;
mod subgraph;

pub use measures::{Component, Components, degrees};
pub use path::{Path, shortest_path};
pub use step::{Reached, Traversal, neighbors, reach, spanned, step, traverse, traverse_each};
pub use subgraph::{Subgraph, subgraph};

use std::collections::{BTreeMap, BTreeSet};
use std::slice;

use lakelands_api::engine::ProjectionView;
use lakelands_api::query::{Depth, Edge, Step, Walk};
use lakelands_api::{RecordKey, RelationName, Result, TypeName};

use crate::invalid;
use lakelands_api::error::Case;

/// A record's identity: its type and its key. Ordered by type, then key.
pub type Node = (TypeName, RecordKey);

/// The records one call asks about at most: a walk reads the links of a
/// page of records at once, and asks about a page of far ends at once, so
/// what one call holds is a page's links, never the graph.
pub const PAGE: usize = 256;

/// The two questions the walks ask of a lake's projection, each for many
/// records at once.
///
/// Every [`ProjectionView`] is one: the view's `edges` answers the first
/// question and its `get_many` the second. The store wraps a view in its
/// own implementation when visibility depends on who is asking.
pub trait EdgeSource {
    /// The links of many records of one type, over one relation or (with
    /// `None`) all, in one direction: one list per key, in the keys'
    /// order. No order is promised within a list; the walks sort. `step`
    /// is the pattern step the hop is walked for, where the walk has
    /// steps: a source that reads each step's links by its own rule is
    /// told which.
    fn edges(
        &self,
        ty: &TypeName,
        keys: &[RecordKey],
        relation: Option<&RelationName>,
        walk: Walk,
        step: Option<usize>,
    ) -> Result<Vec<Vec<Edge>>>;

    /// Whether each record reached over a link may appear in an answer,
    /// in `nodes`' order.
    fn visible(&self, nodes: &[Node]) -> Result<Vec<bool>>;
}

/// Through a projection view, a record is visible when it exists at the
/// view's position and is not retired. A link to a record that does not
/// exist is not followed: an answer is made of records, and there is no
/// record to show. Restricted records stay visible here; hiding them is
/// the store's decision, made per principal.
impl<V: ProjectionView + ?Sized> EdgeSource for V {
    fn edges(
        &self,
        ty: &TypeName,
        keys: &[RecordKey],
        relation: Option<&RelationName>,
        walk: Walk,
        _step: Option<usize>,
    ) -> Result<Vec<Vec<Edge>>> {
        ProjectionView::edges(self, ty, keys, relation, walk)
    }

    fn visible(&self, nodes: &[Node]) -> Result<Vec<bool>> {
        let mut live = BTreeSet::new();
        for (ty, keys) in by_type(nodes) {
            for record in self.get_many(ty, &keys)? {
                if !record.retired {
                    live.insert((record.ty, record.key));
                }
            }
        }
        Ok(nodes.iter().map(|n| live.contains(n)).collect())
    }
}

/// `nodes`' keys grouped by type.
pub fn by_type(nodes: &[Node]) -> BTreeMap<&TypeName, Vec<RecordKey>> {
    let mut out: BTreeMap<&TypeName, Vec<RecordKey>> = BTreeMap::new();
    for (ty, key) in nodes {
        out.entry(ty).or_default().push(key.clone());
    }
    out
}

/// The links of each of `nodes`, over one relation or all, in `nodes`'
/// order: asked of `source` a type and a [`PAGE`] at a time.
fn edges_of<S: EdgeSource + ?Sized>(
    source: &S,
    nodes: &[Node],
    relation: Option<&RelationName>,
    walk: Walk,
    step: Option<usize>,
) -> Result<Vec<Vec<Edge>>> {
    let mut out = vec![Vec::new(); nodes.len()];
    let mut by_type: BTreeMap<&TypeName, Vec<usize>> = BTreeMap::new();
    for (i, (ty, _)) in nodes.iter().enumerate() {
        by_type.entry(ty).or_default().push(i);
    }
    for (ty, at) in by_type {
        for page in at.chunks(PAGE) {
            let keys: Vec<RecordKey> = page.iter().map(|&i| nodes[i].1.clone()).collect();
            for (&i, edges) in page
                .iter()
                .zip(source.edges(ty, &keys, relation, walk, step)?)
            {
                out[i].extend(edges);
            }
        }
    }
    Ok(out)
}

/// Refuse a depth range the lake does not allow: one whose `max` is
/// over the lake's `max_depth`, or one that is empty (`min > max`).
pub fn check_depth(depth: Depth, max_depth: u32) -> Result<()> {
    if depth.max > max_depth {
        let (max, asked) = (max_depth, depth.max);
        return Err(invalid(Case::StepTooDeep { max, asked }));
    }
    if depth.min > depth.max {
        let (min, max) = (depth.min, depth.max);
        return Err(invalid(Case::DepthEmpty { min, max }));
    }
    Ok(())
}

/// What a walk that would hold more than `max` records is refused with,
/// never cut (026).
fn too_large(max: u32) -> lakelands_api::Error {
    let reason = lakelands_api::Refusal::TooLarge { max };
    lakelands_api::Error::Refused {
        field: None,
        reason,
    }
}

/// The identity of a link, for deduplication and ordering:
/// `(from, relation, to)`.
type EdgeId = (Node, RelationName, Node);

fn edge_id(edge: &Edge) -> EdgeId {
    (edge.from.clone(), edge.relation.clone(), edge.to.clone())
}

/// The record at the other end of a link from `near`. A link from a
/// record to itself leads back to it.
fn far(edge: &Edge, near: &Node) -> Node {
    if edge.from == *near {
        edge.to.clone()
    } else {
        edge.from.clone()
    }
}

/// The walks' view of an [`EdgeSource`]: links sorted and deduplicated,
/// filtered to visible far endpoints, with each record's visibility asked
/// once per walk, a [`PAGE`] of records at a time.
struct Links<'a, S: EdgeSource + ?Sized> {
    source: &'a S,
    visible: BTreeMap<Node, bool>,
}

impl<'a, S: EdgeSource + ?Sized> Links<'a, S> {
    fn new(source: &'a S) -> Self {
        Links {
            source,
            visible: BTreeMap::new(),
        }
    }

    /// Ask about every one of `nodes` not asked about before in this walk.
    fn learn<'n>(&mut self, nodes: impl IntoIterator<Item = &'n Node>) -> Result<()> {
        let unknown: BTreeSet<Node> = nodes
            .into_iter()
            .filter(|n| !self.visible.contains_key(*n))
            .cloned()
            .collect();
        let unknown: Vec<Node> = unknown.into_iter().collect();
        for page in unknown.chunks(PAGE) {
            for (node, answer) in page.iter().zip(self.source.visible(page)?) {
                self.visible.insert(node.clone(), answer);
            }
        }
        Ok(())
    }

    /// The links of each of `nears` over the given relations (all of them
    /// when `relations` is empty), each paired with its far endpoint,
    /// keeping only visible far endpoints: one list per record, in
    /// `nears`' order, each ordered by `(relation, far, from, to)`
    /// without duplicates.
    fn around(
        &mut self,
        nears: &[Node],
        relations: &[RelationName],
        walk: Walk,
        step: Option<usize>,
    ) -> Result<Vec<Vec<(Node, Edge)>>> {
        let mut raw = vec![Vec::new(); nears.len()];
        if relations.is_empty() {
            raw = edges_of(self.source, nears, None, walk, step)?;
        } else {
            for relation in relations {
                for (all, more) in
                    raw.iter_mut()
                        .zip(edges_of(self.source, nears, Some(relation), walk, step)?)
                {
                    all.extend(more);
                }
            }
        }
        let mut sorted = Vec::with_capacity(nears.len());
        for (near, edges) in nears.iter().zip(raw) {
            let mut one: BTreeMap<(RelationName, Node, Node, Node), Edge> = BTreeMap::new();
            for edge in edges {
                let other = far(&edge, near);
                let key = (
                    edge.relation.clone(),
                    other,
                    edge.from.clone(),
                    edge.to.clone(),
                );
                one.entry(key).or_insert(edge);
            }
            sorted.push(one);
        }
        self.learn(
            sorted
                .iter()
                .flat_map(|one| one.keys().map(|(_, other, _, _)| other)),
        )?;
        Ok(sorted
            .into_iter()
            .map(|one| {
                one.into_iter()
                    .filter(|((_, other, _, _), _)| self.visible[other])
                    .map(|((_, other, _, _), edge)| (other, edge))
                    .collect()
            })
            .collect())
    }
}

/// One hop of a walk over several steps at once, from each of `nears`:
/// the links over every step whose `max` reaches `hop`, each relation in
/// its own step's direction, kept only where `keep` takes the far end
/// for that step. One list per record, in `nears`' order, each ordered
/// by `(relation, far, from, to)` without duplicates.
fn hop<S, K>(
    links: &mut Links<'_, S>,
    nears: &[Node],
    steps: &[Step],
    hop: u32,
    keep: &mut K,
) -> Result<Vec<Vec<(Node, Edge)>>>
where
    S: EdgeSource + ?Sized,
    K: FnMut(usize, &Node) -> Result<bool>,
{
    let mut out: Vec<BTreeMap<(RelationName, Node, Node, Node), Edge>> =
        vec![BTreeMap::new(); nears.len()];
    for (i, s) in steps.iter().enumerate() {
        if s.depth.max < hop {
            continue;
        }
        let around = links.around(nears, slice::from_ref(&s.relation), s.walk, Some(i))?;
        for (all, one) in out.iter_mut().zip(around) {
            for (far, edge) in one {
                if keep(i, &far)? {
                    let key = (
                        edge.relation.clone(),
                        far,
                        edge.from.clone(),
                        edge.to.clone(),
                    );
                    all.entry(key).or_insert(edge);
                }
            }
        }
    }
    Ok(out
        .into_iter()
        .map(|all| {
            all.into_iter()
                .map(|((_, far, _, _), e)| (far, e))
                .collect()
        })
        .collect())
}

#[cfg(test)]
mod tests;
