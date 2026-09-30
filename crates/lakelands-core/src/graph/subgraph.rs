//! A bounded neighbourhood, as nodes and edges, for a viewer.
//!
//! The start is always present, at depth 0, even with no links, so an
//! isolated record still renders. The answer is closed: an edge appears
//! only if both of its ends are among the nodes, so a viewer never draws
//! a link into nothing. When the node cap cuts the walk short the answer
//! says so, because a prefix of a neighbourhood is otherwise
//! indistinguishable from the whole of a smaller one.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::query::{Depth, Edge, Walk};
use lakelands_api::{RelationName, Result};

use super::{EdgeId, EdgeSource, Links, Node, Reached, check_depth, edge_id};

/// A bounded neighbourhood.
#[derive(Clone, Debug, PartialEq)]
pub struct Subgraph {
    /// The records reached, the start first, ordered by
    /// `(depth, type, key)`.
    pub nodes: Vec<Reached>,
    /// The links seen from the records expanded whose both ends are
    /// among `nodes`, ordered by `(from, relation, to)`.
    pub edges: Vec<Edge>,
    /// Whether the node cap left reachable records out.
    pub truncated: bool,
}

/// Everything within `depth` hops of `from` over `relations` (all of
/// them when the slice is empty), walking in the direction `walk`.
///
/// At most `max_nodes` records, the start included and always kept.
/// When the cap bites, the nearest records are kept, and among records
/// equally near those first by `(type, key)`. Refused as invalid when
/// `depth` goes past `max_depth`, the lake's ceiling.
pub fn subgraph<S: EdgeSource + ?Sized>(
    source: &S,
    from: &Node,
    relations: &[RelationName],
    walk: Walk,
    depth: u32,
    max_nodes: usize,
    max_depth: u32,
) -> Result<Subgraph> {
    check_depth(Depth { min: 0, max: depth }, max_depth)?;
    let mut links = Links::new(source);
    let mut depth_of: BTreeMap<Node, u32> = BTreeMap::from([(from.clone(), 0)]);
    let mut seen: BTreeMap<EdgeId, Edge> = BTreeMap::new();
    let mut frontier: BTreeSet<Node> = BTreeSet::from([from.clone()]);
    let mut truncated = false;

    for hop in 1..=depth {
        let mut found = BTreeSet::new();
        for near in &frontier {
            for (other, edge) in links.around(near, relations, walk)? {
                seen.entry(edge_id(&edge)).or_insert(edge);
                if !depth_of.contains_key(&other) {
                    found.insert(other);
                }
            }
        }
        let mut admitted = BTreeSet::new();
        for node in found {
            if depth_of.len() >= max_nodes.max(1) {
                truncated = true;
                break;
            }
            depth_of.insert(node.clone(), hop);
            admitted.insert(node);
        }
        if admitted.is_empty() {
            break;
        }
        frontier = admitted;
    }

    let edges = seen
        .into_values()
        .filter(|e| depth_of.contains_key(&e.from) && depth_of.contains_key(&e.to))
        .collect();
    let mut nodes: Vec<Reached> = depth_of
        .into_iter()
        .map(|(node, depth)| Reached { node, depth })
        .collect();
    nodes.sort_by(|a, b| (a.depth, &a.node).cmp(&(b.depth, &b.node)));
    Ok(Subgraph {
        nodes,
        edges,
        truncated,
    })
}

#[cfg(test)]
mod tests;
