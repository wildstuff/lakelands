//! The shortest walk from one record to another.
//!
//! Breadth-first, one level at a time, so the first time the target is
//! reached is by the fewest hops. Among equally short walks the choice
//! is fixed, not left to the order an engine answers in: each record's
//! predecessor is the first record, by `(type, key)`, of the level before
//! it that links to it, over the first such link by relation.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::Result;
use lakelands_api::query::{Edge, Step};

use super::{EdgeSource, Links, Node, check_depth, hop};

/// A walk from one record to another.
#[derive(Clone, Debug, PartialEq)]
pub struct Path {
    /// The records along it, the start first and the target last.
    pub nodes: Vec<Node>,
    /// The links between them: `edges[i]` joins `nodes[i]` and
    /// `nodes[i + 1]`, in whichever direction it was declared.
    pub edges: Vec<Edge>,
}

/// The shortest walk from `from` to `to` over `steps`, one walk over any
/// of their relations, each walked in its own step's direction to its
/// own step's `max`. A record reached over a step's relation is walked
/// through, or ends the walk, only where `keep(i, record)` holds for that
/// step.
///
/// `None` when the target is not reachable so, or is hidden. A walk from
/// a record to itself is that record alone, with no links, whatever its
/// links are. Refused as invalid when a step's depth goes past
/// `max_depth`, the lake's ceiling.
pub fn shortest_path<S, K>(
    source: &S,
    from: &Node,
    to: &Node,
    steps: &[Step],
    max_depth: u32,
    keep: K,
) -> Result<Option<Path>>
where
    S: EdgeSource + ?Sized,
    K: FnMut(usize, &Node) -> Result<bool>,
{
    for s in steps {
        check_depth(s.depth, max_depth)?;
    }
    if from == to {
        return Ok(Some(Path {
            nodes: vec![from.clone()],
            edges: Vec::new(),
        }));
    }
    let (mut links, mut keep) = (Links::new(source), keep);
    let mut prev: BTreeMap<Node, (Node, Edge)> = BTreeMap::new();
    let mut frontier: BTreeSet<Node> = BTreeSet::from([from.clone()]);
    let deepest = steps.iter().map(|s| s.depth.max).max().unwrap_or(0);
    for at in 1..=deepest {
        let nears: Vec<Node> = frontier.iter().cloned().collect();
        let mut next = BTreeSet::new();
        for (near, around) in nears
            .iter()
            .zip(hop(&mut links, &nears, steps, at, &mut keep)?)
        {
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
