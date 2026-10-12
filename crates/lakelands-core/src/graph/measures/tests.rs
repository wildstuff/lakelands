use super::super::tests::fixture::{Graph, e, n};
use super::*;

fn rel(r: &str) -> RelationName {
    RelationName::new(r)
}

#[test]
fn degree_counts_links_in_the_named_direction_to_visible_ends() {
    let g = Graph::new(vec![
        e("a", "r", "b"),
        e("a", "r", "c"),
        e("d", "r", "a"),
        e("a", "s", "b"),
        e("a", "r", "gone"),
    ])
    .retire("gone");
    let d = |steps: &[(&str, Walk)]| {
        let steps: Vec<_> = steps.iter().map(|(r, w)| (rel(r), *w)).collect();
        degrees(&g, &[n("a")], &steps).unwrap()[0]
    };
    assert_eq!(d(&[("r", Walk::Out)]), 2, "the retired end is not counted");
    assert_eq!(d(&[("r", Walk::In)]), 1);
    assert_eq!(d(&[("r", Walk::Both)]), 3);
    assert_eq!(d(&[("r", Walk::Both), ("s", Walk::Out)]), 4);
    assert_eq!(
        d(&[("r", Walk::Out), ("r", Walk::Both)]),
        3,
        "a link named twice counts once"
    );
}

/// Every record is asked about once and counted when first seen, so
/// the walk holds each record once, however many links lead to it
/// before it is expanded: on a complete graph of 12 records, 11
/// questions (the start is not asked), and a cap of 12 is enough.
#[test]
fn each_record_is_asked_about_once_and_held_once() {
    let names: Vec<String> = (0..12).map(|i| format!("k{i:02}")).collect();
    let mut links = Vec::new();
    for (i, a) in names.iter().enumerate() {
        for b in &names[i + 1..] {
            links.push(e(a, "r", b));
        }
    }
    let g = Graph::new(links).retire("k11");
    let mut c = Components::new(&g, vec![rel("r")], 11);
    c.add(&[n("k00")]).unwrap();
    assert_eq!(
        g.visibility_asked.get(),
        11,
        "each far end is asked about once, the retired one included"
    );
    assert_eq!(c.finish()[0].len(), 11);
}

/// `count` records `k000`… each linked over `r` to one record of its own,
/// `t000`…
fn fan(count: usize, from: impl Fn(usize) -> String) -> Graph {
    Graph::new(
        (0..count)
            .map(|i| e(&from(i), "r", &format!("t{i:03}")))
            .collect(),
    )
}

/// 300 records' degrees are read a page at a time: two `edges` calls for
/// the one step, and two for the 300 far ends' visibility, not 300 each.
#[test]
fn degrees_read_a_page_of_records_at_a_time() {
    let g = fan(300, |i| format!("k{i:03}"));
    let nodes: Vec<Node> = (0..300).map(|i| n(&format!("k{i:03}"))).collect();
    let d = degrees(&g, &nodes, &[(rel("r"), Walk::Out)]).unwrap();
    assert!(d.iter().all(|&d| d == 1), "{d:?}");
    assert_eq!(g.edges_asked.get(), 2, "one call per page of 256");
    assert_eq!(g.visible_asked.get(), 2, "one call per page of far ends");
}

/// A component grows a page of its frontier at a time: a record linked to
/// 300 others is one call, then its 300 neighbours two.
#[test]
fn components_read_a_page_of_the_frontier_at_a_time() {
    let g = Graph::new(
        (0..300)
            .map(|i| e("hub", "r", &format!("t{i:03}")))
            .collect(),
    );
    let mut c = Components::new(&g, vec![rel("r")], 1_000);
    c.add(&[n("hub")]).unwrap();
    assert_eq!(c.finish()[0].len(), 301);
    assert_eq!(g.edges_asked.get(), 3, "the hub, then two pages of 256");
    assert_eq!(
        g.visible_asked.get(),
        2,
        "its 300 neighbours, a page at a time"
    );
}

/// A page of seeds grows together: 256 seeds, each linked to a record of
/// its own and every eighth to the next seed too, read their links in one
/// call and their far ends' in one more, not one each; seeds that reach
/// each other in the page are one component.
#[test]
fn components_grow_a_page_of_seeds_at_a_time() {
    let mut links: Vec<_> = (0..256)
        .map(|i| e(&format!("k{i:03}"), "r", &format!("t{i:03}")))
        .collect();
    links.extend(
        (0..255)
            .step_by(8)
            .map(|i| e(&format!("k{i:03}"), "r", &format!("k{:03}", i + 1))),
    );
    let g = Graph::new(links);
    let seeds: Vec<Node> = (0..256).map(|i| n(&format!("k{i:03}"))).collect();
    let mut c = Components::new(&g, vec![rel("r")], 1_000);
    c.add(&seeds).unwrap();
    let all = c.finish();
    assert_eq!(g.edges_asked.get(), 2, "the seeds, then their far ends");
    assert_eq!(
        g.visible_asked.get(),
        1,
        "the far ends not yet reached, once"
    );
    let sizes: Vec<usize> = all.iter().map(Vec::len).collect();
    assert_eq!(
        (sizes.len(), sizes[..32].iter().all(|&s| s == 4)),
        (224, true),
        "{sizes:?}"
    );
    assert_eq!(
        all[0],
        ["k000", "k001", "t000", "t001"].map(n),
        "joined, by (type, key)"
    );
}
