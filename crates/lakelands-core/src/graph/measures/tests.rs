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
        degree(&g, &n("a"), &steps).unwrap()
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
    c.add(n("k00")).unwrap();
    assert_eq!(
        g.visibility_asked.get(),
        11,
        "each far end is asked about once, the retired one included"
    );
    assert_eq!(c.finish()[0].len(), 11);
}
