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
mod tests {
    use super::super::fixture::{Graph, e, keys, n};
    use super::*;

    fn node_keys(s: &Subgraph) -> Vec<&str> {
        keys(s.nodes.iter().map(|r| &r.node))
    }

    fn edge_keys(s: &Subgraph) -> Vec<(&str, &str, &str)> {
        s.edges
            .iter()
            .map(|e| (e.from.1.as_str(), e.relation.as_str(), e.to.1.as_str()))
            .collect()
    }

    #[test]
    fn a_subgraph_always_contains_its_start() {
        let g = Graph::new(vec![]);
        let got = subgraph(&g, &n("lonely"), &[], Walk::Both, 3, 100, 16).unwrap();
        assert_eq!(node_keys(&got), vec!["lonely"]);
        assert_eq!(got.nodes[0].depth, 0);
        assert!(got.edges.is_empty());
        assert!(!got.truncated);
    }

    #[test]
    fn a_capped_subgraph_says_so_and_stays_closed() {
        let got = subgraph(&Graph::chain(), &n("a"), &[], Walk::Out, 5, 2, 16).unwrap();
        assert!(got.truncated, "the cap was spent with records left");
        assert_eq!(node_keys(&got), vec!["a", "b"]);
        assert_eq!(edge_keys(&got), vec![("a", "next", "b")]);
    }

    #[test]
    fn a_cap_that_is_not_reached_is_not_reported() {
        let got = subgraph(&Graph::chain(), &n("a"), &[], Walk::Out, 5, 4, 16).unwrap();
        assert!(!got.truncated);
        assert_eq!(node_keys(&got), vec!["a", "b", "c", "d"]);
    }

    #[test]
    fn the_cap_keeps_the_nearest_records_first_by_key() {
        let g = Graph::new(vec![e("a", "r", "z"), e("a", "r", "y"), e("z", "r", "b")]);
        let got = subgraph(&g, &n("a"), &[], Walk::Out, 2, 2, 16).unwrap();
        assert_eq!(node_keys(&got), vec!["a", "y"]);
        assert!(got.truncated);
    }

    #[test]
    fn nodes_are_ordered_by_depth_then_key_and_edges_by_their_ends() {
        let g = Graph::new(vec![e("c", "r", "a"), e("a", "r", "b"), e("b", "s", "d")]);
        let got = subgraph(&g, &n("a"), &[], Walk::Both, 2, 100, 16).unwrap();
        assert_eq!(node_keys(&got), vec!["a", "b", "c", "d"]);
        assert_eq!(
            got.nodes.iter().map(|r| r.depth).collect::<Vec<_>>(),
            vec![0, 1, 1, 2]
        );
        assert_eq!(
            edge_keys(&got),
            vec![("a", "r", "b"), ("b", "s", "d"), ("c", "r", "a")]
        );
    }

    #[test]
    fn a_link_seen_from_both_ends_appears_once() {
        let g = Graph::new(vec![e("a", "r", "b"), e("b", "r", "a")]);
        let got = subgraph(&g, &n("a"), &[], Walk::Both, 3, 100, 16).unwrap();
        assert_eq!(edge_keys(&got), vec![("a", "r", "b"), ("b", "r", "a")]);
    }

    #[test]
    fn only_the_named_relations_are_walked() {
        let g = Graph::new(vec![e("a", "r", "b"), e("a", "s", "c")]);
        let named = [RelationName::new("s")];
        let got = subgraph(&g, &n("a"), &named, Walk::Out, 1, 100, 16).unwrap();
        assert_eq!(node_keys(&got), vec!["a", "c"]);
    }

    #[test]
    fn a_hidden_record_and_its_links_are_left_out() {
        let g = Graph::new(vec![e("a", "r", "b"), e("a", "r", "c"), e("c", "r", "d")]).retire("c");
        let got = subgraph(&g, &n("a"), &[], Walk::Out, 3, 100, 16).unwrap();
        assert_eq!(node_keys(&got), vec!["a", "b"]);
        assert_eq!(edge_keys(&got), vec![("a", "r", "b")]);
    }

    #[test]
    fn a_subgraph_deeper_than_the_ceiling_is_refused() {
        assert!(subgraph(&Graph::chain(), &n("a"), &[], Walk::Out, 17, 100, 16).is_err());
    }
}
