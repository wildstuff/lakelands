use lakelands_api::{RecordKey, TypeName};

use super::super::tests::fixture::{Graph, e, keys, link, n};
use super::*;

fn next(walk: Walk, min: u32, max: u32) -> Step {
    Step {
        relation: RelationName::new("next"),
        walk,
        ty: None,
        filter: None,
        depth: Depth { min, max },
    }
}

fn nodes(reached: &[Reached]) -> Vec<&str> {
    keys(reached.iter().map(|r| &r.node))
}

fn depths(reached: &[Reached]) -> Vec<u32> {
    reached.iter().map(|r| r.depth).collect()
}

fn all(_: usize, _: &Node) -> Result<bool> {
    Ok(true)
}

#[test]
fn a_link_to_a_missing_or_retired_record_is_not_followed() {
    let g = Graph::new(vec![
        e("a", "next", "gone"),
        e("a", "next", "hidden"),
        e("a", "next", "ok"),
    ])
    .missing("gone")
    .retire("hidden");
    let got = neighbors(&g, &n("a"), &RelationName::new("next"), Walk::Out).unwrap();
    assert_eq!(nodes(&got), vec!["ok"]);
}

#[test]
fn neighbors_are_ordered_by_type_then_key_whatever_order_the_source_answers_in() {
    let g = Graph::new(vec![
        e("a", "r", "c"),
        link(n("a"), "r", (TypeName::new("m"), RecordKey::new("z"))),
        e("a", "r", "b"),
    ]);
    let got = neighbors(&g, &n("a"), &RelationName::new("r"), Walk::Out).unwrap();
    assert_eq!(nodes(&got), vec!["z", "b", "c"]);
    assert_eq!(depths(&got), vec![1, 1, 1]);
}

#[test]
fn walking_out_follows_links_forward_and_walking_in_follows_them_back() {
    let g = Graph::chain();
    let out = step(&g, &n("a"), &next(Walk::Out, 1, 10), 16).unwrap();
    assert_eq!(nodes(&out), vec!["b", "c", "d"]);
    let back = step(&g, &n("d"), &next(Walk::In, 1, 10), 16).unwrap();
    assert_eq!(nodes(&back), vec!["c", "b", "a"]);
    assert_eq!(depths(&back), vec![1, 2, 3]);
}

#[test]
fn walking_both_ways_reaches_either_side() {
    let g = Graph::chain();
    let got = step(&g, &n("b"), &next(Walk::Both, 1, 1), 16).unwrap();
    assert_eq!(nodes(&got), vec!["a", "c"]);
}

#[test]
fn a_record_reachable_twice_reports_its_shortest_depth() {
    let g = Graph::new(vec![
        e("a", "next", "b"),
        e("b", "next", "c"),
        e("a", "next", "c"),
    ]);
    let got = step(&g, &n("a"), &next(Walk::Out, 1, 5), 16).unwrap();
    assert_eq!(nodes(&got), vec!["b", "c"]);
    assert_eq!(depths(&got), vec![1, 1]);
}

#[test]
fn a_cycle_terminates_and_reports_the_start_where_it_closes() {
    let g = Graph::new(vec![e("a", "next", "b"), e("b", "next", "a")]);
    let got = step(&g, &n("a"), &next(Walk::Out, 1, 3), 16).unwrap();
    assert_eq!(nodes(&got), vec!["b", "a"]);
    assert_eq!(depths(&got), vec![1, 2]);
}

#[test]
fn a_zero_depth_step_yields_the_start_alone_never_the_whole_graph() {
    let got = step(&Graph::chain(), &n("a"), &next(Walk::Out, 0, 0), 16).unwrap();
    assert_eq!(nodes(&got), vec!["a"]);
    assert_eq!(depths(&got), vec![0]);
}

#[test]
fn a_range_from_zero_includes_the_start_once_even_on_a_cycle() {
    let g = Graph::new(vec![e("a", "next", "b"), e("b", "next", "a")]);
    let got = step(&g, &n("a"), &next(Walk::Out, 0, 3), 16).unwrap();
    assert_eq!(nodes(&got), vec!["a", "b"]);
    assert_eq!(depths(&got), vec![0, 1]);
}

#[test]
fn the_minimum_depth_drops_records_reached_sooner() {
    let got = step(&Graph::chain(), &n("a"), &next(Walk::Out, 2, 3), 16).unwrap();
    assert_eq!(nodes(&got), vec!["c", "d"]);
}

#[test]
fn a_chain_of_steps_yields_one_binding_per_match() {
    let g = Graph::chain();
    let steps = vec![next(Walk::Out, 1, 1); 3];
    let got = traverse(&g, &n("a"), &steps, 16, 100, all).unwrap();
    assert_eq!(got.bindings.len(), 1);
    assert_eq!(nodes(&got.bindings[0]), vec!["b", "c", "d"]);
    assert!(!got.truncated);

    let got = traverse(&g, &n("a"), &steps[..1], 16, 100, all).unwrap();
    assert_eq!(nodes(&got.bindings[0]), vec!["b"]);
}

#[test]
fn a_chain_past_the_end_of_the_links_yields_nothing() {
    let steps = vec![next(Walk::Out, 1, 1); 4];
    let got = traverse(&Graph::chain(), &n("a"), &steps, 16, 100, all).unwrap();
    assert!(got.bindings.is_empty());
}

#[test]
fn a_chain_along_a_missing_relation_yields_nothing() {
    let mut nope = next(Walk::Out, 1, 1);
    nope.relation = RelationName::new("nope");
    let got = traverse(&Graph::chain(), &n("a"), &[nope], 16, 100, all).unwrap();
    assert!(got.bindings.is_empty());
}

#[test]
fn a_chain_with_one_step_too_deep_is_refused_before_walking() {
    let g = Graph::chain();
    let steps = vec![next(Walk::Out, 1, 1), next(Walk::Out, 1, 17)];
    assert!(traverse(&g, &n("a"), &steps, 16, 100, all).is_err());
    assert_eq!(g.visibility_asked.get(), 0);
}

#[test]
fn a_chain_of_no_steps_is_the_start_alone() {
    let got = traverse(&Graph::chain(), &n("a"), &[], 16, 100, all).unwrap();
    assert_eq!(got.bindings, vec![Vec::<Reached>::new()]);
}

#[test]
fn bindings_come_depth_first_in_record_order() {
    // a has children x and y; each has leaves 1 and 2.
    let g = Graph::new(vec![
        e("a", "c", "y"),
        e("a", "c", "x"),
        e("x", "c", "x2"),
        e("x", "c", "x1"),
        e("y", "c", "y1"),
    ]);
    let mut child = next(Walk::Out, 1, 1);
    child.relation = RelationName::new("c");
    let got = traverse(&g, &n("a"), &[child.clone(), child], 16, 100, all).unwrap();
    let flat: Vec<Vec<&str>> = got.bindings.iter().map(|b| nodes(b)).collect();
    assert_eq!(
        flat,
        vec![vec!["x", "x1"], vec!["x", "x2"], vec!["y", "y1"]]
    );
}

#[test]
fn keep_drops_a_record_and_everything_past_it() {
    let g = Graph::new(vec![
        e("a", "next", "b"),
        e("a", "next", "c"),
        e("b", "next", "d"),
    ]);
    let steps = vec![next(Walk::Out, 1, 1); 2];
    let got = traverse(&g, &n("a"), &steps, 16, 100, |i, node| {
        Ok(!(i == 0 && node.1.as_str() == "b"))
    })
    .unwrap();
    assert!(got.bindings.is_empty(), "c has no next, and b was dropped");
}

#[test]
fn the_cap_on_bindings_says_it_cut_the_answer_short() {
    let g = Graph::new(vec![
        e("a", "next", "b"),
        e("a", "next", "c"),
        e("a", "next", "d"),
    ]);
    let steps = vec![next(Walk::Out, 1, 1)];
    let got = traverse(&g, &n("a"), &steps, 16, 2, all).unwrap();
    assert_eq!(got.bindings.len(), 2);
    assert!(got.truncated);
    let exact = traverse(&g, &n("a"), &steps, 16, 3, all).unwrap();
    assert!(!exact.truncated);
}

#[test]
fn visibility_is_asked_once_per_record_per_walk() {
    let g = Graph::new(vec![
        e("a", "next", "b"),
        e("a", "next", "c"),
        e("b", "next", "d"),
        e("c", "next", "d"),
    ]);
    step(&g, &n("a"), &next(Walk::Both, 1, 3), 16).unwrap();
    // a, b, c, d: each asked about once, the start included when a
    // walk both ways comes back to it.
    assert_eq!(g.visibility_asked.get(), 4);
}
