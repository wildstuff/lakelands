use lakelands_api::query::Walk;

use super::super::tests::fixture::{Graph, e, every, keys, n, over};
use super::*;

/// The shortest path from `from` to `to` over one relation.
fn path(
    g: &Graph,
    from: &str,
    to: &str,
    relation: &str,
    walk: Walk,
    depth: u32,
) -> Result<Option<Path>> {
    shortest_path(
        g,
        &n(from),
        &n(to),
        &over(&[relation], walk, depth),
        16,
        every,
    )
}

fn path_keys(p: &Path) -> Vec<&str> {
    keys(&p.nodes)
}

#[test]
fn the_path_to_oneself_is_a_single_record() {
    let g = Graph::new(vec![]);
    let got = path(&g, "a", "a", "r", Walk::Out, 5).unwrap().unwrap();
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
    let got = path(&g, "a", "z", "r", Walk::Out, 9).unwrap().unwrap();
    assert_eq!(path_keys(&got), vec!["a", "b", "z"]);
    assert_eq!(got.edges, vec![e("a", "r", "b"), e("b", "r", "z")]);

    let short = path(&Graph::chain(), "a", "d", "next", Walk::Out, 2);
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
    let got = path(&g, "a", "z", "r", Walk::Out, 3).unwrap().unwrap();
    assert_eq!(path_keys(&got), vec!["a", "x", "z"]);
}

#[test]
fn walking_in_finds_the_path_against_the_links() {
    let got = path(&Graph::chain(), "d", "a", "next", Walk::In, 3)
        .unwrap()
        .unwrap();
    assert_eq!(path_keys(&got), vec!["d", "c", "b", "a"]);
    assert_eq!(got.edges[0], e("c", "next", "d"));
}

#[test]
fn walking_out_does_not_find_a_path_against_the_links() {
    let got = path(&Graph::chain(), "d", "a", "next", Walk::Out, 3);
    assert_eq!(got.unwrap(), None);
}

#[test]
fn a_hidden_target_is_not_reached() {
    let g = Graph::chain().retire("c");
    let got = path(&g, "a", "c", "r", Walk::Out, 3);
    assert_eq!(got.unwrap(), None);
}

#[test]
fn only_the_named_relations_are_walked() {
    let g = Graph::new(vec![e("a", "r", "b"), e("a", "s", "m"), e("m", "s", "b")]);
    let got = path(&g, "a", "b", "s", Walk::Out, 3).unwrap().unwrap();
    assert_eq!(path_keys(&got), vec!["a", "m", "b"]);
}

#[test]
fn a_path_deeper_than_the_ceiling_is_refused() {
    let got = path(&Graph::chain(), "a", "d", "next", Walk::Out, 17);
    assert!(got.is_err());
}
