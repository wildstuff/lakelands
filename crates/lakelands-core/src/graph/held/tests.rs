use lakelands_api::query::Walk;

use super::super::tests::fixture::{Graph, e, keys, n};
use super::*;

fn steps(walk: Walk) -> Vec<(RelationName, Walk)> {
    vec![(RelationName::new("r"), walk)]
}

/// The graph is closed under the steps' walks and indexed by `(type,
/// key)`, so the order an engine hands links in moves nothing; a hidden
/// far end is no record of it.
#[test]
fn a_held_graph_is_the_same_whatever_order_links_come_in() {
    let links = vec![
        e("c", "r", "a"),
        e("a", "r", "b"),
        e("b", "r", "c"),
        e("b", "r", "gone"),
    ];
    let mut reversed = links.clone();
    reversed.reverse();
    let hold = |links: Vec<_>| {
        let g = Graph::new(links).retire("gone");
        let mut h = Holding::new(&g, steps(Walk::Out), 10, true);
        h.add(&[n("b")]).unwrap();
        h.finish()
    };
    let got = hold(links);
    assert_eq!(keys(&got.nodes), vec!["a", "b", "c"]);
    assert_eq!(got.arcs, vec![(0, 1), (1, 2), (2, 0)]);
    assert_eq!(hold(reversed), got);
}

/// A step walked `both` gives a link an arc each way; with no start type
/// a seed with no link it may see is no record of the graph.
#[test]
fn both_ways_is_two_arcs_and_a_lone_seed_stands_only_with_a_type() {
    let g = Graph::new(vec![e("a", "r", "b"), e("c", "r", "gone")]).retire("gone");
    let mut h = Holding::new(&g, steps(Walk::Both), 10, false);
    h.add(&[n("a"), n("c"), n("d")]).unwrap();
    let got = h.finish();
    assert_eq!(keys(&got.nodes), vec!["a", "b"]);
    assert_eq!(got.arcs, vec![(0, 1), (1, 0)]);
    let mut alone = Holding::new(&g, steps(Walk::Both), 10, true);
    alone.add(&[n("a"), n("c"), n("d")]).unwrap();
    assert_eq!(keys(&alone.finish().nodes), vec!["a", "b", "c", "d"]);
}

/// Past `max_nodes` records the graph is refused, never cut.
#[test]
fn a_graph_past_max_nodes_is_refused_too_large() {
    let g = Graph::new(vec![e("a", "r", "b"), e("b", "r", "c"), e("c", "r", "d")]);
    let mut h = Holding::new(&g, steps(Walk::Out), 3, true);
    let err = h.add(&[n("a")]).unwrap_err();
    let refusal = lakelands_api::Refusal::TooLarge { max: 3 };
    assert!(matches!(err, lakelands_api::Error::Refused { reason, .. } if reason == refusal));
    let mut h = Holding::new(&g, steps(Walk::Out), 4, true);
    h.add(&[n("a")]).unwrap();
    assert_eq!(h.finish().nodes.len(), 4);
}
