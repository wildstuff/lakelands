//! The shortest walk from one record to another.
//!
//! Breadth-first, one level at a time, so the first time the target is
//! reached is by the fewest hops. Among equally short walks the choice
//! is fixed, not left to the order an engine answers in: each record's
//! predecessor is the first record, by `(type, key)`, of the level before
//! it that links to it, over the first such link by relation.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::query::{Depth, Edge, Walk};
use lakelands_api::{RelationName, Result};

use super::{EdgeSource, Links, Node, check_depth};

/// A walk from one record to another.
#[derive(Clone, Debug, PartialEq)]
pub struct Path {
    /// The records along it, the start first and the target last.
    pub nodes: Vec<Node>,
    /// The links between them: `edges[i]` joins `nodes[i]` and
    /// `nodes[i + 1]`, in whichever direction it was declared.
    pub edges: Vec<Edge>,
}

/// The shortest walk from `from` to `to` over `relations` (all of them
/// when the slice is empty), walking in the direction `walk`, of at most
/// `depth` hops.
///
/// `None` when the target is not reachable within `depth`, or is hidden.
/// A walk from a record to itself is that record alone, with no links,
/// whatever its links are. Refused as invalid when `depth` goes past
/// `max_depth`, the lake's ceiling.
pub fn shortest_path<S: EdgeSource + ?Sized>(
    source: &S,
    from: &Node,
    to: &Node,
    relations: &[RelationName],
    walk: Walk,
    depth: u32,
    max_depth: u32,
) -> Result<Option<Path>> {
    check_depth(Depth { min: 0, max: depth }, max_depth)?;
    if from == to {
        return Ok(Some(Path {
            nodes: vec![from.clone()],
            edges: Vec::new(),
        }));
    }
    let mut links = Links::new(source);
    let mut prev: BTreeMap<Node, (Node, Edge)> = BTreeMap::new();
    let mut frontier: BTreeSet<Node> = BTreeSet::from([from.clone()]);
    for _ in 1..=depth {
        let nears: Vec<Node> = frontier.iter().cloned().collect();
        let mut next = BTreeSet::new();
        for (near, around) in nears.iter().zip(links.around(&nears, relations, walk)?) {
            for (other, edge) in around {
                if other == *from || prev.contains_key(&other) {
                    continue;
                }
                prev.insert(other.clone(), (near.clone(), edge));
                if other == *to {
                    return Ok(Some(unwind(prev, from, to)));
                }
                next.insert(other);
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    Ok(None)
}

/// Follow the predecessors back from the target to the start.
fn unwind(mut prev: BTreeMap<Node, (Node, Edge)>, from: &Node, to: &Node) -> Path {
    let mut nodes = vec![to.clone()];
    let mut edges = Vec::new();
    let mut at = to.clone();
    while at != *from {
        let (before, edge) = prev
            .remove(&at)
            .expect("every reached record but the start has a predecessor");
        edges.push(edge);
        nodes.push(before.clone());
        at = before;
    }
    nodes.reverse();
    edges.reverse();
    Path { nodes, edges }
}

#[cfg(test)]
mod tests;
