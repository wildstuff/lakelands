use super::super::tests::fixture::{Graph, e, keys, n};
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
