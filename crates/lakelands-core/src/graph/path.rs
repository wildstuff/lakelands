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
    let mut found = false;
    'levels: for _ in 1..=depth {
        let mut next = BTreeSet::new();
        for near in &frontier {
            for (other, edge) in links.around(near, relations, walk)? {
                if other == *from || prev.contains_key(&other) {
                    continue;
                }
                prev.insert(other.clone(), (near.clone(), edge));
                if other == *to {
                    found = true;
                    break 'levels;
                }
                next.insert(other);
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    if !found {
        return Ok(None);
    }
    Ok(Some(unwind(prev, from, to)))
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
mod tests {
    use super::super::fixture::{Graph, e, keys, n};
    use super::*;

    fn path_keys(p: &Path) -> Vec<&str> {
        keys(&p.nodes)
    }

    #[test]
    fn the_path_to_oneself_is_a_single_record() {
        let g = Graph::new(vec![]);
        let got = shortest_path(&g, &n("a"), &n("a"), &[], Walk::Out, 5, 16)
            .unwrap()
            .unwrap();
        assert_eq!(path_keys(&got), vec!["a"]);
        assert!(got.edges.is_empty());
    }

    #[test]
    fn the_shortest_path_takes_the_short_way_and_gives_up_at_the_depth() {
        let g = Graph::new(vec![
            e("a", "r", "b"),
            e("b", "r", "z"),
            e("a", "r", "m"),
            e("m", "r", "p"),
            e("p", "r", "z"),
        ]);
        let got = shortest_path(&g, &n("a"), &n("z"), &[], Walk::Out, 9, 16)
            .unwrap()
            .unwrap();
        assert_eq!(path_keys(&got), vec!["a", "b", "z"]);
        assert_eq!(got.edges, vec![e("a", "r", "b"), e("b", "r", "z")]);

        let short = shortest_path(&Graph::chain(), &n("a"), &n("d"), &[], Walk::Out, 2, 16);
        assert_eq!(short.unwrap(), None);
    }

    #[test]
    fn among_equally_short_paths_the_first_by_key_is_taken() {
        let g = Graph::new(vec![
            e("a", "r", "y"),
            e("y", "r", "z"),
            e("a", "r", "x"),
            e("x", "r", "z"),
        ]);
        let got = shortest_path(&g, &n("a"), &n("z"), &[], Walk::Out, 3, 16)
            .unwrap()
            .unwrap();
        assert_eq!(path_keys(&got), vec!["a", "x", "z"]);
    }

    #[test]
    fn walking_in_finds_the_path_against_the_links() {
        let got = shortest_path(&Graph::chain(), &n("d"), &n("a"), &[], Walk::In, 3, 16)
            .unwrap()
            .unwrap();
        assert_eq!(path_keys(&got), vec!["d", "c", "b", "a"]);
        assert_eq!(got.edges[0], e("c", "next", "d"));
    }

    #[test]
    fn walking_out_does_not_find_a_path_against_the_links() {
        let got = shortest_path(&Graph::chain(), &n("d"), &n("a"), &[], Walk::Out, 3, 16);
        assert_eq!(got.unwrap(), None);
    }

    #[test]
    fn a_hidden_target_is_not_reached() {
        let g = Graph::chain().retire("c");
        let got = shortest_path(&g, &n("a"), &n("c"), &[], Walk::Out, 3, 16);
        assert_eq!(got.unwrap(), None);
    }

    #[test]
    fn only_the_named_relations_are_walked() {
        let g = Graph::new(vec![e("a", "r", "b"), e("a", "s", "m"), e("m", "s", "b")]);
        let named = [RelationName::new("s")];
        let got = shortest_path(&g, &n("a"), &n("b"), &named, Walk::Out, 3, 16)
            .unwrap()
            .unwrap();
        assert_eq!(path_keys(&got), vec!["a", "m", "b"]);
    }

    #[test]
    fn a_path_deeper_than_the_ceiling_is_refused() {
        let got = shortest_path(&Graph::chain(), &n("a"), &n("d"), &[], Walk::Out, 17, 16);
        assert!(got.is_err());
    }
}
