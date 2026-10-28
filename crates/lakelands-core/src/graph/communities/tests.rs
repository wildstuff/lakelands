use super::super::tests::fixture::{keys, n};
use super::*;

/// A graph of `names`, linked by `links`, held as a step walked `both`
/// holds it: an arc each way, and one for a loop.
fn held(names: &[&str], links: &[(&str, &str)]) -> Held {
    let mut nodes: Vec<_> = names.iter().map(|k| n(k)).collect();
    nodes.sort();
    let at = |k: &str| nodes.iter().position(|n| n.1.as_str() == k).unwrap() as u32;
    let mut arcs = Vec::new();
    for &(u, v) in links {
        arcs.push((at(u), at(v)));
        if u != v {
            arcs.push((at(v), at(u)));
        }
    }
    arcs.sort_unstable();
    Held { nodes, arcs }
}

/// The communities' keys, in their order.
fn found(g: &Held) -> Vec<Vec<String>> {
    let all = communities(g).unwrap();
    let all = all.iter().map(|c| keys(c).into_iter().map(str::to_owned));
    all.map(Iterator::collect).collect()
}

const TWO: [(&str, &str); 6] = [
    ("a", "b"),
    ("b", "c"),
    ("c", "a"),
    ("d", "e"),
    ("e", "f"),
    ("f", "d"),
];

/// Two triangles a bridge joins are two communities, numbered by their
/// least record.
#[test]
fn a_bridge_does_not_join_two_dense_groups() {
    let links = [&TWO[..], &[("c", "d")]].concat();
    let got = found(&held(&["a", "b", "c", "d", "e", "f"], &links));
    assert_eq!(got, [["a", "b", "c"], ["d", "e", "f"]]);
}

/// A record linked once to each of two equal groups gains as much in
/// either: it goes to the one whose least record is least, so renaming
/// the first group past the second moves it.
#[test]
fn a_tie_goes_to_the_community_whose_least_record_is_least() {
    let mut links = TWO.to_vec();
    links.extend([("x", "c"), ("x", "d")]);
    let got = found(&held(&["a", "b", "c", "d", "e", "f", "x"], &links));
    assert_eq!(got, [vec!["a", "b", "c", "x"], vec!["d", "e", "f"]]);
    let renamed = |k: &'static str| match k {
        "a" => "m",
        "b" => "n",
        "c" => "o",
        k => k,
    };
    let links: Vec<_> = links
        .iter()
        .map(|&(u, v)| (renamed(u), renamed(v)))
        .collect();
    let got = found(&held(&["m", "n", "o", "d", "e", "f", "x"], &links));
    assert_eq!(got, [vec!["d", "e", "f", "x"], vec!["m", "n", "o"]]);
}

/// Louvain leaves `k01 k05 k08 k09` one community, though no link joins
/// `k01 k09` to `k05 k08`: it is answered as its two connected parts.
#[test]
fn a_community_louvain_leaves_in_parts_is_split_into_them() {
    let names = [
        "k00", "k01", "k02", "k03", "k04", "k05", "k06", "k07", "k08", "k09", "k10", "k11",
    ];
    let one_way = [
        ("k09", "k01"),
        ("k02", "k04"),
        ("k07", "k06"),
        ("k00", "k04"),
        ("k07", "k03"),
        ("k00", "k11"),
        ("k02", "k01"),
        ("k06", "k09"),
        ("k07", "k10"),
    ];
    let both_ways = [
        ("k00", "k09"),
        ("k05", "k08"),
        ("k00", "k02"),
        ("k00", "k10"),
        ("k10", "k11"),
        ("k06", "k05"),
        ("k06", "k03"),
        ("k02", "k11"),
        ("k00", "k08"),
    ];
    let mut links = one_way.to_vec();
    links.extend(both_ways.iter().flat_map(|&(u, v)| [(u, v), (v, u)]));
    let g = held(&names, &links);
    let of = louvain(&g).unwrap();
    assert_eq!(of, [0, 1, 0, 2, 0, 1, 2, 2, 1, 1, 0, 0]);
    assert_eq!(
        found(&g),
        [
            vec!["k00", "k02", "k04", "k10", "k11"],
            vec!["k03", "k06", "k07"],
            vec!["k01", "k09"],
            vec!["k05", "k08"],
        ]
    );
}

/// A record with no link, or with only a loop, is a community of its own;
/// no graph is no community.
#[test]
fn a_record_alone_is_its_own_community_and_no_graph_none() {
    let got = found(&held(&["a", "b", "c"], &[("a", "a")]));
    assert_eq!(got, [["a"], ["b"], ["c"]]);
    assert!(found(&held(&[], &[])).is_empty());
    let got = found(&held(&["a", "b", "c"], &[("a", "b"), ("a", "c")]));
    assert_eq!(got, [["a", "b", "c"]]);
}
