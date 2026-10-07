//! A bounded neighbourhood, as nodes and edges, for a viewer, a page at a
//! time.
//!
//! The starts are always present, at depth 0, even with no links, so an
//! isolated record still renders. The answer is closed: an edge appears
//! only if both of its ends are among the nodes, so a viewer never draws
//! a link into nothing. When the page's size cuts the walk short the
//! answer says so, because a prefix of a neighbourhood is otherwise
//! indistinguishable from the whole of a smaller one, and the next page
//! continues the same order.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::Result;
use lakelands_api::query::{Edge, Step};

use super::{EdgeId, EdgeSource, Links, Node, Reached, check_depth, edge_id, hop};

/// One page of a graph.
#[derive(Clone, Debug, PartialEq)]
pub struct Subgraph {
    /// The page's records, ordered by `(depth, type, key)`.
    pub nodes: Vec<Reached>,
    /// The links walked between a record of the page and one of this or
    /// an earlier page, ordered by `(from, relation, to)`.
    pub edges: Vec<Edge>,
    /// Whether records follow the page.
    pub truncated: bool,
}

/// The neighbourhood of `starts` over `steps`: one walk over any of the
/// steps' relations, each walked in its own step's direction to its own
/// step's `max`. A record reached over a step's relation is taken in,
/// and walked from, only where `keep(i, record)` holds for that step;
/// that is where the caller applies the step's `type` and `where`. Each
/// record is at the fewest hops any start reaches it in.
///
/// One page of it: the records at `offset..offset + limit` of the order
/// `(depth, type, key)`, and every link walked between a record of the
/// page and one of this or an earlier page, so the pages together are
/// the neighbourhood, each record and link once. The walk goes only as
/// deep as the page's last record. Refused as invalid when a step's
/// depth goes past `max_depth`, the lake's ceiling.
pub fn subgraph<S, K>(
    source: &S,
    starts: &[Node],
    steps: &[Step],
    max_depth: u32,
    offset: usize,
    limit: usize,
    keep: K,
) -> Result<Subgraph>
where
    S: EdgeSource + ?Sized,
    K: FnMut(usize, &Node) -> Result<bool>,
{
    for s in steps {
        check_depth(s.depth, max_depth)?;
    }
    let (mut links, mut keep) = (Links::new(source), keep);
    let end = offset + limit;
    let mut seen: BTreeSet<Node> = starts.iter().cloned().collect();
    let mut nodes: Vec<Reached> = seen
        .iter()
        .map(|node| Reached {
            node: node.clone(),
            depth: 0,
        })
        .collect();
    let mut walked: BTreeMap<EdgeId, Edge> = BTreeMap::new();
    let mut level = 0..nodes.len();
    let deepest = steps.iter().map(|s| s.depth.max).max().unwrap_or(0);
    for at in 1..=deepest {
        // Only the records up to the page's end are walked from: a link
        // between two of them is seen from either end.
        let nears: Vec<Node> = nodes[level.start..level.end.min(end)]
            .iter()
            .map(|r| r.node.clone())
            .collect();
        if nears.is_empty() {
            break;
        }
        let mut found = BTreeSet::new();
        for around in hop(&mut links, &nears, steps, at, &mut keep)? {
            for (other, edge) in around {
                walked.entry(edge_id(&edge)).or_insert(edge);
                if !seen.contains(&other) {
                    found.insert(other);
                }
            }
        }
        if nodes.len() > end {
            break;
        }
        let first = nodes.len();
        for node in found {
            seen.insert(node.clone());
            nodes.push(Reached { node, depth: at });
        }
        level = first..nodes.len();
    }
    Ok(page(nodes, walked, offset, limit))
}

/// The page at `offset..offset + limit` of `nodes`, already in their
/// order, with the links of `walked` between a record of the page and
/// one of this or an earlier page.
pub(super) fn page(
    mut nodes: Vec<Reached>,
    walked: BTreeMap<EdgeId, Edge>,
    offset: usize,
    limit: usize,
) -> Subgraph {
    let end = offset + limit;
    let truncated = nodes.len() > end;
    nodes.truncate(end);
    let at: BTreeMap<&Node, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, r)| (&r.node, i))
        .collect();
    let edges = walked
        .into_values()
        .filter(|e| match (at.get(&e.from), at.get(&e.to)) {
            (Some(a), Some(b)) => *a.max(b) >= offset,
            _ => false,
        })
        .collect();
    Subgraph {
        nodes: nodes.split_off(offset.min(nodes.len())),
        edges,
        truncated,
    }
}

#[cfg(test)]
mod tests;
