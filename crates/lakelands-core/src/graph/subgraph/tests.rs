use lakelands_api::query::Walk;

use super::super::tests::fixture::{Graph, e, every, keys, n, over};
use super::*;

/// The first page, of at most `cap` records, of `a`'s neighbourhood.
fn around(g: &Graph, relations: &[&str], walk: Walk, depth: u32, cap: usize) -> Result<Subgraph> {
    subgraph(
        g,
        &[n("a")],
        &over(relations, walk, depth),
        16,
        0,
        cap,
        every,
    )
}

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
fn a_capped_subgraph_says_so_and_stays_closed() {
    let got = around(&Graph::chain(), &["next"], Walk::Out, 5, 2).unwrap();
    assert!(got.truncated, "the cap was spent with records left");
    assert_eq!(node_keys(&got), vec!["a", "b"]);
    assert_eq!(edge_keys(&got), vec![("a", "next", "b")]);
}

#[test]
fn a_cap_that_is_not_reached_is_not_reported() {
    let got = around(&Graph::chain(), &["next"], Walk::Out, 5, 4).unwrap();
    assert!(!got.truncated);
    assert_eq!(node_keys(&got), vec!["a", "b", "c", "d"]);
}

#[test]
fn the_cap_keeps_the_nearest_records_first_by_key() {
    let g = Graph::new(vec![e("a", "r", "z"), e("a", "r", "y"), e("z", "r", "b")]);
    let got = around(&g, &["r", "s"], Walk::Out, 2, 2).unwrap();
    assert_eq!(node_keys(&got), vec!["a", "y"]);
    assert!(got.truncated);
}

#[test]
fn nodes_are_ordered_by_depth_then_key_and_edges_by_their_ends() {
    let g = Graph::new(vec![e("c", "r", "a"), e("a", "r", "b"), e("b", "s", "d")]);
    let got = around(&g, &["r", "s"], Walk::Both, 2, 100).unwrap();
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
    let got = around(&g, &["r", "s"], Walk::Both, 3, 100).unwrap();
    assert_eq!(edge_keys(&got), vec![("a", "r", "b"), ("b", "r", "a")]);
}

#[test]
fn only_the_named_relations_are_walked() {
    let g = Graph::new(vec![e("a", "r", "b"), e("a", "s", "c")]);
    let got = around(&g, &["s"], Walk::Out, 1, 100).unwrap();
    assert_eq!(node_keys(&got), vec!["a", "c"]);
}

#[test]
fn a_hidden_record_and_its_links_are_left_out() {
    let g = Graph::new(vec![e("a", "r", "b"), e("a", "r", "c"), e("c", "r", "d")]).retire("c");
    let got = around(&g, &["r", "s"], Walk::Out, 3, 100).unwrap();
    assert_eq!(node_keys(&got), vec!["a", "b"]);
    assert_eq!(edge_keys(&got), vec![("a", "r", "b")]);
}

#[test]
fn a_subgraph_deeper_than_the_ceiling_is_refused() {
    assert!(around(&Graph::chain(), &["next"], Walk::Out, 17, 100).is_err());
}
