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
    let mut c = Components::new(&g, vec![rel("r")], 11, true);
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
    let mut c = Components::new(&g, vec![rel("r")], 1_000, true);
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
    let mut c = Components::new(&g, vec![rel("r")], 1_000, true);
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

/// With no start type, a seed that reaches nothing it may see is no
/// component and counts nothing against the cap: a seed linked only to a
/// retired record is none either. Seeds that reach each other read their
/// links once, as seeds.
#[test]
fn a_seed_that_reaches_nothing_is_no_component_where_none_stands_alone() {
    let g = Graph::new(vec![e("a", "r", "b"), e("c", "r", "gone")]).retire("gone");
    let seeds = ["a", "b", "c", "d"].map(n);
    let mut c = Components::new(&g, vec![rel("r")], 2, false);
    c.add(&seeds).unwrap();
    assert_eq!(g.edges_asked.get(), 1, "b's links were read as a seed's");
    assert_eq!(c.finish(), vec![vec![n("a"), n("b")]]);
    let mut alone = Components::new(&g, vec![rel("r")], 4, true);
    alone.add(&seeds).unwrap();
    assert_eq!(
        alone.finish().len(),
        3,
        "a and b, c, and d with a start type"
    );
}

/// A graph of `n` records `k0`… with the given arcs, by index.
fn held(n: usize, arcs: &[(u32, u32)]) -> Held {
    let nodes = (0..n)
        .map(|i| super::super::tests::fixture::n(&format!("k{i}")))
        .collect();
    let mut arcs = arcs.to_vec();
    arcs.sort_unstable();
    Held { nodes, arcs }
}

/// Every round keeps the whole mass, whatever floors it drops: a cycle,
/// a record with no arc out, a record linked to itself and one linked to
/// nothing, over 37 rounds that each hand a record a share of 7.
#[test]
fn every_round_of_page_rank_keeps_the_whole_mass() {
    let g = held(
        7,
        &[
            (0, 1),
            (1, 2),
            (2, 0),
            (3, 3),
            (3, 0),
            (4, 1),
            (4, 2),
            (4, 3),
            (0, 5),
        ],
    );
    let mut out = vec![0u64; g.nodes.len()];
    for &(u, _) in &g.arcs {
        out[u as usize] += 1;
    }
    let mut mass = spread(RANK_MASS, g.nodes.len());
    assert_eq!(
        mass[..2],
        [142_857_142_858, 142_857_142_857],
        "the unit left over goes first"
    );
    for round_at in 0..37 {
        mass = round(&out, &g.arcs, &mass);
        assert_eq!(mass.iter().sum::<u64>(), RANK_MASS, "round {round_at}");
    }
    let ranked = page_rank(&g);
    assert_eq!(ranked.iter().sum::<u64>(), RANK_MASS);
    assert!(
        ranked[0] > ranked[6],
        "a record linked to outranks one linked to nothing"
    );
}

/// The rounds stop at the first that changes no unit, or at 100: a lone
/// record keeps the whole mass, and no graph is no scores.
#[test]
fn page_rank_of_one_record_is_the_whole_mass_and_of_none_is_nothing() {
    assert_eq!(page_rank(&held(1, &[])), vec![RANK_MASS]);
    assert_eq!(page_rank(&held(1, &[(0, 0)])), vec![RANK_MASS]);
    assert_eq!(page_rank(&held(0, &[])), Vec::<u64>::new());
    assert_eq!(
        page_rank(&held(2, &[(0, 1), (1, 0)])),
        vec![RANK_MASS / 2; 2]
    );
}
