//! The cheapest walk of the pattern query: from one record to another,
//! within the steps' hops, over the links whose weight field sums least,
//! in integers at nine places, so every engine answers the same walk
//! whatever order it reads links in (`docs/decisions/018-one-pattern-query.md`,
//! `docs/decisions/026-the-store-walks-the-graph.md`).

use lakelands_api::Value;
use lakelands_api::query::Depth;
use lakelands_api::write::WriteOpts;

use crate::conformance::fixture::{doors, order, owner, reference, step};
use crate::conformance::prelude::*;

check!(
    CHEAPEST_TAKES_LEAST = the_cheapest_walk_takes_the_least_cost_over_fewer_hops,
    "018 and 026: `cheapest` answers the walk from its start to `to` whose links' weights sum least, though it takes more hops than the shortest, one row a record in hop order with the relation and the way each link was walked and what the walk has cost so far; a start that is `to` is a walk of one record at cost 0, and a `to` no walk reaches answers no row"
);

check!(
    CHEAPEST_WITHIN_DEPTH = a_cheaper_walk_past_the_steps_depth_is_not_taken,
    "026: `cheapest` relaxes the walks within its steps' hops, one hop at a time, so a cheaper walk of more hops than the steps allow is not taken, and the least within them is"
);

check!(
    CHEAPEST_TIES = equal_costs_take_fewest_hops_then_the_least_predecessor_whatever_order_links_were_written_in,
    "026: between walks of equal cost `cheapest` takes the one of fewest hops, then at each hop the one from the least record by (type, key), over a cycle of links that weigh 0 too, and answers it byte for byte alike on two lakes whose links were written in opposite orders",
    given
);

check!(
    CHEAPEST_PLACES = a_weight_reads_number_and_decimal_at_nine_places_and_cost_answers_its_kind,
    "018 and 026: a weight is read at nine places, a tenth rounding half away from zero, and summed exactly, so 0.1 and 0.2 cost 0.3; the cost is a number where the weight is a number and a decimal, as a decimal field reads, where it is a decimal; a number cost past 2^53 - 1, 10^21 and beyond included, is refused overflow, never answered rounded"
);

check!(
    CHEAPEST_MISSING_NEGATIVE = a_missing_weight_is_refused_naming_the_link_and_a_negative_one_too,
    "018 and 026: a link the walk holds whose weight is null or absent is refused `weight_missing`, and one that weighs less than 0 `weight_negative`, each naming the relation and the link's ends, though the cheapest walk would not take it; the lake's head does not move"
);

check!(
    CHEAPEST_JUDGED = a_weight_on_a_via_or_derived_relation_or_an_undeclared_field_is_refused_before_a_record_is_read,
    "018: a weight on a relation carried by `via` or derived, or that its links do not declare, is refused `weight_not_on_link`, and one of another kind than number or decimal `weight_not_number`, each naming the relation and the field, from a start no record has as from one that has"
);

check!(
    CHEAPEST_REFUSALS = cheapest_without_to_with_order_by_or_with_several_starts_is_refused,
    "018: `cheapest` is refused `cheapest_needs_to` without `to`, then as a measure is with `order_by`, both before any step is judged, `path_starts` from several starts, and with a step's `join`, `optional`, `absent` or a `from` that names another record than the start as a path is, naming the member"
);

check!(
    CHEAPEST_TOO_LARGE = a_cheapest_walk_past_max_graph_nodes_is_refused_too_large,
    "018, 026 and the location's limits: `cheapest` holds every record within its steps' hops and, past `max_graph_nodes`, is refused too_large naming the ceiling, never answering a walk over part of them",
    given
);

/// Towns linked by `road` records weighing `km` (a number), `toll` (a
/// decimal) and a `note`, and by `capital`, carried by a reference, and
/// `namesake`, derived from equal names. Each road `(from, to, fields)`.
async fn lake(store: &dyn LakeStore, roads: &[(&str, &str, Value)]) -> Held {
    let l = Lake::of(store);
    let mut town = type_decl(
        "town",
        vec![
            field("name", StorageKind::Text),
            reference("capital", "town"),
        ],
    );
    town.relations = vec![
        RelationDecl {
            cardinality: Cardinality::ManyToMany,
            fields: vec![
                field("km", StorageKind::Number),
                field("toll", StorageKind::Decimal),
                field("note", StorageKind::Text),
            ],
            ..RelationDecl::to("road", "town")
        },
        RelationDecl {
            cardinality: Cardinality::ManyToOne,
            via: Some("capital".into()),
            ..RelationDecl::to("capital", "town")
        },
        RelationDecl {
            cardinality: Cardinality::ManyToMany,
            derived: Some(Derived {
                pairs: vec![MatchPair {
                    from: "name".into(),
                    to: "name".into(),
                }],
                filter: None,
            }),
            ..RelationDecl::to("namesake", "town")
        },
    ];
    l.declare(town).await?;
    let mut towns: Vec<&str> = roads.iter().flat_map(|(a, b, _)| [*a, *b]).collect();
    towns.sort_unstable();
    towns.dedup();
    for t in towns {
        l.upsert("town", t, json!({"name": t})).await?;
    }
    for (from, to, fields) in roads {
        l.cat
            .link(
                &l.own,
                &"town".into(),
                &(*from).into(),
                &"road".into(),
                &(*to).into(),
                patch(fields.clone()),
                WriteOpts::default(),
            )
            .await
            .must(&format!("building the road from {from} to {to}"))?;
    }
    Ok(())
}

/// Roads by `km`: a → d is 25, a → b → d 20, and a → c → e 2, where d → e,
/// walked in from e, is 1.
fn country() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        ("a", "d", json!({"km": 25})),
        ("a", "b", json!({"km": 10})),
        ("b", "d", json!({"km": 10})),
        ("a", "c", json!({"km": 1})),
        ("c", "e", json!({"km": 1})),
        ("d", "e", json!({"km": 1})),
    ]
}

/// One step over `road`, walked `walk`, of 0 to `max` hops.
fn road(walk: Walk, max: u32) -> Step {
    Step {
        depth: Depth { min: 0, max },
        ..step("road", walk)
    }
}

/// The cheapest walk by `weight` from the town `from` to `to` over `steps`.
fn cheap(from: &str, to: &str, steps: Vec<Step>, weight: &str) -> Pattern {
    let project = Projection::Cheapest {
        weight: weight.into(),
    };
    Pattern {
        to: Some(("town".into(), to.into())),
        ..pattern("town", Some(from), steps, project)
    }
}

/// The rows a cheapest walk answers, each as `hop key relation walk cost`,
/// a cost that is a decimal quoted.
async fn walked(
    cat: &dyn Catalog,
    who: &Principal,
    p: Pattern,
    what: &str,
) -> Result<Vec<String>, Failure> {
    let reply = cat.query(who, p).await.must(what)?;
    let QueryResult::Rows { rows, next_cursor } = reply.data else {
        return Err(Failure::new(format!(
            "{what} answered something other than rows"
        )));
    };
    ensure!(
        next_cursor.is_none(),
        "{what} answers its walk whole, with no cursor"
    );
    Ok(rows.iter().map(show).collect())
}

fn show(row: &BTreeMap<String, Value>) -> String {
    let cell = |c: &str| match row.get(c) {
        Some(Value::String(s)) if c != "cost" => s.clone(),
        Some(Value::Null) | None => "-".to_owned(),
        Some(v) => v.to_string(),
    };
    ["hop", "key", "relation", "walk", "cost"]
        .map(cell)
        .join(" ")
}

async fn the_cheapest_walk_takes_the_least_cost_over_fewer_hops(store: Store) -> Held {
    lake(store.as_ref(), &country()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let both = || vec![road(Walk::Both, 3)];
    let got = walked(cat.as_ref(), own, cheap("a", "d", both(), "km"), "a to d").await?;
    let want = [
        "0 a - - 0",
        "1 c road out 1",
        "2 e road out 2",
        "3 d road in 3",
    ];
    ensure!(
        got == want,
        "the cheapest walk from a to d by `km` is a, c, e, d at 3, over d → e walked in, not \
         a → d at 25 or a, b, d at 20; it answered {got:?}"
    );
    let mut shortest = cheap("a", "d", both(), "km");
    shortest.project = Projection::Graph;
    let reply = cat.query(own, shortest).await.must("the shortest a to d")?;
    let QueryResult::Graph { nodes, .. } = reply.data else {
        return Err(Failure::new("the shortest walk answered no graph"));
    };
    let keys: Vec<&str> = nodes.iter().map(|n| n.key.as_str()).collect();
    ensure!(
        keys == ["a", "d"],
        "the shortest walk over the same roads is a, d; it answered {keys:?}"
    );
    for (from, to, want, what) in [
        ("a", "a", vec!["0 a - - 0"], "from a to itself"),
        (
            "e",
            "a",
            vec!["0 e - - 0", "1 c road in 1", "2 a road in 2"],
            "from e back to a",
        ),
    ] {
        let got = walked(cat.as_ref(), own, cheap(from, to, both(), "km"), what).await?;
        ensure!(
            got == want,
            "the cheapest walk {what} is {want:?}; it answered {got:?}"
        );
    }
    let out = cheap("e", "a", vec![road(Walk::Out, 3)], "km");
    let got = walked(cat.as_ref(), own, out, "from e to a, out only").await?;
    ensure!(
        got.is_empty(),
        "walking out only, no road leads from e to a, and no row is answered; it answered \
         {got:?}"
    );
    Ok(())
}

async fn a_cheaper_walk_past_the_steps_depth_is_not_taken(store: Store) -> Held {
    lake(store.as_ref(), &country()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    for (max, want) in [
        (
            3,
            vec![
                "0 a - - 0",
                "1 c road out 1",
                "2 e road out 2",
                "3 d road in 3",
            ],
        ),
        (2, vec!["0 a - - 0", "1 b road out 10", "2 d road out 20"]),
        (1, vec!["0 a - - 0", "1 d road out 25"]),
    ] {
        let p = cheap("a", "d", vec![road(Walk::Both, max)], "km");
        let what = format!("the cheapest a to d within {max} hops");
        let got = walked(cat.as_ref(), own, p, &what).await?;
        ensure!(
            got == want,
            "{what} is the least walk of at most {max} hops, {want:?}; it answered {got:?}"
        );
    }
    Ok(())
}

/// Roads by `km` from s: to t at 2 straight, or over x or y at 1 and 1;
/// to u over x or y at 1 and 1, or on from x over z at 0 and 1, where
/// x → z → x is a cycle that weighs 0.
fn ties() -> Vec<(&'static str, &'static str, Value)> {
    [
        ("s", "t", 2),
        ("s", "x", 1),
        ("s", "y", 1),
        ("x", "t", 1),
        ("y", "t", 1),
        ("x", "u", 1),
        ("y", "u", 1),
        ("x", "z", 0),
        ("z", "x", 0),
        ("z", "u", 1),
    ]
    .map(|(a, b, km)| (a, b, json!({"km": km})))
    .to_vec()
}

async fn equal_costs_take_fewest_hops_then_the_least_predecessor_whatever_order_links_were_written_in(
    store: Store,
    given: Given,
) -> Held {
    lake(store.as_ref(), &ties()).await?;
    let twin = (given.twin)(store.lake().clone(), Default::default()).await;
    let twin = twin.map_err(|e| Failure::new(format!("the harness opened no twin: {e}")))?;
    let mut reversed = ties();
    reversed.reverse();
    lake(twin.as_ref(), &reversed).await?;
    let mut answers = Vec::new();
    for (lake, written) in [(&store, "in order"), (&twin, "in reverse")] {
        let (cat, own) = (doors(lake.as_ref()), owner(lake.as_ref()));
        for (to, want) in [
            ("t", vec!["0 s - - 0", "1 t road out 2"]),
            ("u", vec!["0 s - - 0", "1 x road out 1", "2 u road out 2"]),
        ] {
            let p = cheap("s", to, vec![road(Walk::Out, 3)], "km");
            let what = format!("the cheapest s to {to}, the roads written {written}");
            let reply = cat.query(&own, p).await.must(&what)?;
            answers.push(serde_json::to_string(&reply.data).expect("an answer serialises"));
            let QueryResult::Rows { rows, .. } = reply.data else {
                return Err(Failure::new(format!("{what} answered no rows")));
            };
            let got: Vec<String> = rows.iter().map(show).collect();
            ensure!(
                got == want,
                "{what} is {want:?}: of the walks that cost 2, the one of fewest hops, then the \
                 one through x, which sorts before y; it answered {got:?}"
            );
        }
    }
    ensure!(
        answers[..2] == answers[2..],
        "the cheapest walks answer the same bytes whatever order the roads were written in; \
         in order {:?}, in reverse {:?}",
        &answers[..2],
        &answers[2..]
    );
    Ok(())
}

async fn a_weight_reads_number_and_decimal_at_nine_places_and_cost_answers_its_kind(
    store: Store,
) -> Held {
    let roads = [
        ("x1", "x2", json!({"km": 0.1, "toll": "0.1000000004"})),
        ("x2", "x3", json!({"km": 0.2, "toll": "0.2000000005"})),
        (
            "x3",
            "x4",
            json!({"km": 0.0000000005, "toll": "0.0000000004"}),
        ),
        ("y1", "y2", json!({"km": 9_007_199_254_740_991u64})),
        ("y2", "y3", json!({"km": 1})),
        ("z1", "z2", json!({"km": 1e21})),
    ];
    lake(store.as_ref(), &roads).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    for (weight, want) in [
        (
            "km",
            [
                "0 x1 - - 0",
                "1 x2 road out 0.1",
                "2 x3 road out 0.3",
                "3 x4 road out 0.300000001",
            ],
        ),
        (
            "toll",
            [
                "0 x1 - - \"0\"",
                "1 x2 road out \"0.1\"",
                "2 x3 road out \"0.300000001\"",
                "3 x4 road out \"0.300000001\"",
            ],
        ),
    ] {
        let p = cheap("x1", "x4", vec![road(Walk::Out, 3)], weight);
        let what = format!("the cheapest x1 to x4 by `{weight}`");
        let got = walked(cat.as_ref(), own, p, &what).await?;
        ensure!(
            got == want,
            "{what} sums each weight at nine places, the tenth rounding half away from zero, \
             and answers a {} cost: {want:?}; it answered {got:?}",
            if weight == "km" { "number" } else { "decimal" }
        );
    }
    // A number cost is exact or refused: 2^53 - 1 is answered, one more
    // and 10^21 are past the exact integers.
    let p = cheap("y1", "y2", vec![road(Walk::Out, 2)], "km");
    let got = walked(cat.as_ref(), own, p, "the cheapest y1 to y2").await?;
    ensure!(
        got == ["0 y1 - - 0", "1 y2 road out 9007199254740991"],
        "a cost of 2^53 - 1 is answered exactly; it answered {got:?}"
    );
    for (from, to, what) in [("y1", "y3", "2^53"), ("z1", "z2", "10^21")] {
        let p = cheap(from, to, vec![road(Walk::Out, 2)], "km");
        refused_as!(
            &format!("a number cost of {what}"),
            cat.query(own, p).await,
            Case::Overflow
        )?;
    }
    Ok(())
}

async fn a_missing_weight_is_refused_naming_the_link_and_a_negative_one_too(store: Store) -> Held {
    let roads = [
        ("m1", "m2", json!({"note": "unweighed"})),
        ("m1", "m3", json!({"km": 1})),
        ("o1", "o2", json!({"km": null})),
        ("o1", "o3", json!({"km": 1})),
        ("n1", "n2", json!({"km": -1})),
        ("n2", "n3", json!({"km": 1})),
    ];
    lake(store.as_ref(), &roads).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let before = store.head().await.must("reading the head")?;
    let out = || vec![road(Walk::Out, 2)];
    let p = cheap("m1", "m3", out(), "km");
    refused_as!(
        "the cheapest m1 to m3, beside a road of no `km`",
        cat.query(own, p).await,
        Case::WeightMissing { relation, from, to }
            if relation.as_str() == "road" && from.as_str() == "m1" && to.as_str() == "m2"
    )?;
    let p = cheap("o1", "o3", out(), "km");
    refused_as!(
        "the cheapest o1 to o3, beside a road of null `km`",
        cat.query(own, p).await,
        Case::WeightMissing { relation, from, to }
            if relation.as_str() == "road" && from.as_str() == "o1" && to.as_str() == "o2"
    )?;
    let p = cheap("n1", "n3", out(), "km");
    refused_as!(
        "the cheapest n1 to n3, over a road of -1 `km`",
        cat.query(own, p).await,
        Case::WeightNegative { relation, from, to }
            if relation.as_str() == "road" && from.as_str() == "n1" && to.as_str() == "n2"
    )?;
    crate::conformance::fixture::head_unmoved(store.as_ref(), before, "a refused weight").await
}

async fn a_weight_on_a_via_or_derived_relation_or_an_undeclared_field_is_refused_before_a_record_is_read(
    store: Store,
) -> Held {
    lake(store.as_ref(), &country()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let gone = cheap("nowhere", "a", vec![road(Walk::Out, 2)], "km");
    let got = walked(
        cat.as_ref(),
        own,
        gone,
        "the cheapest from a town no record has",
    )
    .await?;
    ensure!(
        got.is_empty(),
        "from a town no record has no walk is answered; it answered {got:?}"
    );
    let over = |relations: &[&str], weight: &str, from: &str| {
        let steps = relations.iter().map(|r| Step {
            depth: Depth { min: 0, max: 2 },
            ..step(r, Walk::Out)
        });
        cheap(from, "a", steps.collect(), weight)
    };
    for from in ["a", "nowhere"] {
        for (relations, weight, named) in [
            (&["capital"][..], "km", "capital"),
            (&["namesake"][..], "km", "namesake"),
            (&["road", "capital"][..], "km", "capital"),
            (&["road"][..], "nope", "road"),
        ] {
            let what = format!("`{weight}` over {relations:?} from {from}");
            refused_as!(
                &what,
                cat.query(own, over(relations, weight, from)).await,
                Case::WeightNotOnLink { relation, field }
                    if relation.as_str() == named && field.as_str() == weight
            )?;
        }
        let what = format!("`note` over `road` from {from}");
        refused_as!(
            &what,
            cat.query(own, over(&["road"], "note", from)).await,
            Case::WeightNotNumber { relation, field }
                if relation.as_str() == "road" && field.as_str() == "note"
        )?;
    }
    Ok(())
}

async fn cheapest_without_to_with_order_by_or_with_several_starts_is_refused(store: Store) -> Held {
    lake(store.as_ref(), &country()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let good = || cheap("a", "d", vec![road(Walk::Both, 3)], "km");
    let mut no_to = good();
    no_to.to = None;
    refused_as!(
        "`cheapest` without `to`",
        cat.query(own, no_to).await,
        Case::CheapestNeedsTo
    )?;
    let mut ordered = good();
    ordered.order_by = vec![order("key", Default::default(), Default::default())];
    refused_as!(
        "`cheapest` with `order_by`",
        cat.query(own, ordered).await,
        Case::MeasureNoOrder { measure } if measure == "cheapest"
    )?;
    let mut several = good();
    several.start.key = None;
    several.start.filter = Some("name = 'a' or name = 'b'".into());
    refused_as!(
        "`cheapest` from a and b",
        cat.query(own, several).await,
        Case::PathStarts { count: 2 }
    )?;
    for member in ["join", "optional", "absent", "from"] {
        let mut p = good();
        let s = &mut p.steps[0];
        match member {
            "join" => s.join = Some("other".into()),
            "optional" => s.optional = true,
            "absent" => s.absent = true,
            _ => s.from = Some("other".into()),
        }
        // `to` and the order are judged before any step.
        let mut no_to = p.clone();
        no_to.to = None;
        no_to.order_by = vec![order("key", Default::default(), Default::default())];
        refused_as!(
            &format!("`cheapest` with `{member}` and `order_by`, without `to`"),
            cat.query(own, no_to).await,
            Case::CheapestNeedsTo
        )?;
        let mut ordered = p.clone();
        ordered.order_by = vec![order("key", Default::default(), Default::default())];
        refused_as!(
            &format!("`cheapest` with `{member}` and `order_by`"),
            cat.query(own, ordered).await,
            Case::MeasureNoOrder { measure } if measure == "cheapest"
        )?;
        let err = cat
            .query(own, p)
            .await
            .must_fail(&format!("`cheapest` with `{member}`"))?;
        let words = Case::PatternTreeMember {
            member: member.into(),
            projection: "cheapest".into(),
        };
        ensure!(
            invalid_case(&err) == Some(&words),
            "`cheapest` with a step's `{member}` is refused as `{words:?}`; it failed with {err:?}"
        );
    }
    Ok(())
}

async fn a_cheapest_walk_past_max_graph_nodes_is_refused_too_large(
    store: Store,
    given: Given,
) -> Held {
    lake(store.as_ref(), &country()).await?;
    let limits = |max_graph_nodes| Limits {
        max_graph_nodes,
        ..crate::conformance::fixture::limits()
    };
    let p = || cheap("a", "b", vec![road(Walk::Both, 3)], "km");
    let store = (given.relimit)(store, Seams::default(), limits(5)).await;
    let store = store.map_err(|e| Failure::new(format!("reopening the lake: {e}")))?;
    let got = walked(
        doors(store.as_ref()).as_ref(),
        &owner(store.as_ref()),
        p(),
        "the cheapest a to b, five towns within reach, under a ceiling of 5",
    )
    .await?;
    ensure!(
        got == ["0 a - - 0", "1 b road out 10"],
        "under a ceiling of 5 the cheapest a to b is the road between them; it answered {got:?}"
    );
    let store = (given.relimit)(store, Seams::default(), limits(4)).await;
    let store = store.map_err(|e| Failure::new(format!("reopening the lake: {e}")))?;
    let err = doors(store.as_ref())
        .query(&owner(store.as_ref()), p())
        .await
        .must_fail("the cheapest a to b past a ceiling of 4")?;
    ensure!(
        matches!(reason(&err), Some(Refusal::TooLarge { max: 4 })),
        "the five towns within three hops of a, past a ceiling of 4, are refused too_large \
         naming 4, though b is one hop away; it failed with {err:?}"
    );
    Ok(())
}
