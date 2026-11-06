use lakelands_api::RecordKey;
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

/// A link of `n` to `n` over `w` weighing `weight`.
fn weighed(from: &str, to: &str, weight: Value) -> Edge {
    let mut link = e(from, "w", to);
    link.fields.insert(FieldName::new("cost"), weight);
    link
}

/// The cheapest walk from `from` to `to` over `w`, walked out to `depth`.
fn cheap(g: &Graph, from: &str, to: &str, depth: u32) -> Result<Option<Cheap>> {
    let steps = over(&["w"], Walk::Out, depth);
    cheapest(
        g,
        (&n(from), &n(to)),
        &steps,
        (16, 100),
        &FieldName::new("cost"),
        every,
    )
}

fn case(e: Error) -> Case {
    e.case()
        .map(|s| s.case.clone())
        .expect("a refusal with a case")
}

/// Each hop relaxes every arc from the hop before, so a walk of more
/// hops that costs less wins within the depth and not past it, and the
/// walk is unwound with what it cost so far at each record.
#[test]
fn the_cheapest_walk_is_relaxed_by_hops_and_unwound_with_its_costs() {
    let g = Graph::new(vec![
        weighed("a", "d", Value::from(10)),
        weighed("a", "b", Value::from(1)),
        weighed("b", "c", Value::from(1)),
        weighed("c", "d", Value::from(1)),
    ]);
    let got = cheap(&g, "a", "d", 3).unwrap().unwrap();
    assert_eq!(keys(&got.nodes), vec!["a", "b", "c", "d"]);
    assert_eq!(
        got.costs,
        vec![0, 1_000_000_000, 2_000_000_000, 3_000_000_000]
    );
    assert_eq!(keys(got.edges.iter().map(|e| &e.to)), vec!["b", "c", "d"]);
    let near = cheap(&g, "a", "d", 2).unwrap().unwrap();
    assert_eq!(
        (keys(&near.nodes), near.costs[1]),
        (vec!["a", "d"], 10_000_000_000)
    );
    assert_eq!(cheap(&g, "d", "a", 3).unwrap(), None, "no walk back");
    let itself = cheap(&g, "b", "b", 3).unwrap().unwrap();
    assert_eq!((keys(&itself.nodes), itself.costs), (vec!["b"], vec![0]));
}

/// Over a cycle that costs nothing, equal costs take the fewest hops and
/// then the least record they came from, whatever order links come in.
#[test]
fn equal_costs_take_the_fewest_hops_then_the_least_record_came_from() {
    let links = vec![
        weighed("a", "b", Value::from(0)),
        weighed("b", "a", Value::from(0)),
        weighed("a", "c", Value::from(1)),
        weighed("b", "c", Value::from(1)),
        weighed("c", "e", Value::from(2)),
        weighed("a", "d", Value::from(1)),
        weighed("d", "e", Value::from(2)),
    ];
    let mut reversed = links.clone();
    reversed.reverse();
    for links in [links, reversed] {
        let got = cheap(&Graph::new(links), "a", "e", 5).unwrap().unwrap();
        assert_eq!(keys(&got.nodes), vec!["a", "c", "e"]);
        assert_eq!(got.costs[2], 3_000_000_000);
    }
}

/// Every link held is weighed, in the order of its relation and ends,
/// before a walk is chosen: a missing weight is refused even off the
/// cheapest walk, and a negative one too.
#[test]
fn a_held_link_with_no_weight_or_a_negative_one_is_refused() {
    let missing = Graph::new(vec![
        weighed("a", "b", Value::from(1)),
        e("a", "w", "c"),
        weighed("a", "d", Value::from(-1)),
    ]);
    let (relation, from) = (RelationName::new("w"), RecordKey::new("a"));
    let to = RecordKey::new("c");
    assert_eq!(
        case(cheap(&missing, "a", "b", 1).unwrap_err()),
        Case::WeightMissing {
            relation: relation.clone(),
            from: from.clone(),
            to
        }
    );
    let negative = Graph::new(vec![
        weighed("a", "b", Value::from(1)),
        weighed("a", "d", Value::from(-1)),
    ]);
    let to = RecordKey::new("d");
    assert_eq!(
        case(cheap(&negative, "a", "b", 1).unwrap_err()),
        Case::WeightNegative { relation, from, to }
    );
}

/// A sum past `i128` is refused, never wrapped.
#[test]
fn a_cost_past_i128_is_refused_overflow() {
    let big = Value::String(format!("1{}", "0".repeat(29)));
    let g = Graph::new(vec![weighed("a", "b", big.clone()), weighed("b", "c", big)]);
    assert_eq!(case(cheap(&g, "a", "c", 2).unwrap_err()), Case::Overflow);
}

/// A weight is a decimal at 9 places, rounded half away from zero.
#[test]
fn a_weight_is_held_at_nine_places() {
    assert_eq!(units("1.5"), Ok(1_500_000_000));
    assert_eq!(units("0.0000000015"), Ok(2));
    assert_eq!(units("-0.0000000015"), Ok(-2));
    assert_eq!(units("0.00000000149"), Ok(1));
    assert_eq!(units("1e-9"), Ok(1));
    assert_eq!(units("2.5E+3"), Ok(2_500_000_000_000));
    assert_eq!(units("0e999999999999"), Ok(0));
    assert_eq!(units("1e-999999999999"), Ok(0));
    assert_eq!(units("1e40"), Err(Some(Case::Overflow)));
    for bad in ["", ".5", "1.", "1e", "-", "1,5", "+1"] {
        assert_eq!(units(bad), Err(None), "{bad}");
    }
    assert_eq!(cost_text(2_500_000_000), "2.5");
    assert_eq!(cost_text(3_000_000_000), "3");
    assert_eq!(cost_text(1), "0.000000001");
    assert_eq!(cost_text(0), "0");
}
