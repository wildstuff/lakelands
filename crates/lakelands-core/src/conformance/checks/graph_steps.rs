//! A graph and a path answer what their steps say: each step's relation,
//! direction, depth, `type` and `where`, a page at a time
//! (`docs/decisions/018-one-pattern-query.md`, lakelands#556).

use lakelands_api::error::Case;
use lakelands_api::location::Limits;
use lakelands_api::ontology::{RelationDecl, StorageKind};
use lakelands_api::query::Walk::{Both, In, Out};
use lakelands_api::query::{Depth, Pattern, Projection, QueryResult, Step, Walk};
use lakelands_api::read::{AggFunc, Aggregate, Direction, OrderBy};
use lakelands_api::seam::Seams;
use lakelands_api::write::WriteOpts;
use lakelands_api::{Access, Catalog, Cursor, Find, LakeStore, Principal, Refusal, Value};
use serde_json::json;
use std::collections::BTreeMap;

use crate::conformance::fixture::{
    Lake, Must, agg, doors, field, field_access, invalid_case, is_invalid, order, owner, patch,
    pattern, reads_all, reason, reference, restrict, type_decl,
};
use crate::conformance::{Failure, Given, Held, Store};

check!(
    ROW_DEPTH = a_row_says_the_hops_each_step_took_and_a_graph_each_nodes_depth,
    "018 and 026, extended (lakelands#989): a row binds `<name>._depth` beside `<name>.key`, the hops its step took to reach the record and 0 for the start, which an order, a group key and `fields` read as any path and which nothing probes; a start a cycle reaches again is answered at the hop it comes back; a graph answers each node's depth in the order of its nodes, a start at 0 however a cycle comes back to it; a depth is counted over the records its reader is shown, the longer way's where the shorter passes a restricted record; and a new field whose name begins with `_` is refused, at `declare` and at `add_field`"
);

check!(
    GRAPH_PAGES = a_graph_cut_by_its_limit_pages_on_with_its_cursor,
    "018 and 029, extended: a graph cut by `limit` answers `truncated` and a `next_cursor`, and the next page holds the next records of the one order with every link between a record of that page and one of this or an earlier page, read at the first page's position; several starts are one walk, cut together, and their pages through the cursors are the graph read in one page, a neighbourhood's starts read a page at a time (lakelands#963); a neighbourhood's page that ends at a start names it, and the next page inside the starts reads those after it and walks from them each step both ways, so a page of one at a time is the graph read in one page with links between starts of earlier and later pages, along a step and against it, with a start filter, with a step `where` that holds of one end alone and over a cycle; a cursor that names no start, as one handed out before, reads on by its offset, and one whose `after` is no depth-0 start is refused as cursor_foreign (lakelands#1050)"
);

check!(
    GRAPH_BOUND = a_chain_graph_past_max_graph_nodes_is_refused_too_large,
    "026, extended (lakelands#963): a chain's graph is walked whole for every page, its starts a page at a time, and one whose walk would hold more records than the lake's `max_graph_nodes`, the starts and every record each walk reaches whether or not a match keeps it, is refused too_large naming the ceiling, nothing partial answered; within it, it answers",
    given
);

check!(
    GRAPH_CURSORS = a_graph_refuses_a_cursor_of_another_read_and_a_path_any,
    "018: a graph refuses a cursor handed out for rows, for `find` or for another pattern, and a path and an aggregate, which have no pages, refuse every cursor, each as invalid before anything is walked"
);

check!(
    GRAPH_STEPS = a_graph_honours_each_steps_direction_depth_and_where,
    "018, extended: a graph whose steps all start at depth 0 walks each relation in its own step's direction to its own `max`, keeping and walking on from a record only where its step's `where`, read against the type its relation reaches wherever it is declared, holds, and a step that reaches several types and reads a `where`, or one of whose fields rows read (lakelands#680, lakelands#990), names its `type`; any other is a chain walked as rows walk it, answered as the start, what rows bind, the records between and the links walked; an empty range and a chain's undeclared relation are refused; over a reference field and over link records alike"
);

check!(
    PATH_STEPS = a_path_honours_its_steps_and_has_one_start,
    "018, extended: a path walks each relation in its step's direction, only through records its step's `where` keeps, and no path is an empty graph; several starts, a projection other than `graph`, and a step starting below depth 0 are refused as invalid"
);

check!(
    GRAPH_RIGHTS = a_graph_shows_a_reader_no_more_than_its_rows,
    "024 and 026, extended: a graph's step `where` on a field the reader may not read is refused as in rows; a restricted record and its links are left out of a graph as out of rows, and a chain is not walked through a record the reader is not shown"
);

check!(
    REACH_STEPS = reach_answers_each_record_at_the_first_step_that_reaches_it,
    "018, extended (lakelands#991): `reach` answers what each step reaches from every record the step before it kept, a dead end included, each record once at the first step that reaches it with that step's fewest hops, the start only where a step reaches it (at depth 0 from a range at 0, at 2 back over its own link), ordered by (step, depth, type, key) and paged by `limit` and its cursor; over a reference field and over link records alike; a restricted record is neither answered nor walked through; `to`, `as`, `order_by` and no steps are refused as invalid"
);

/// Hubs `h1` (spokes `s1`..`s8`) and `h2` (`s9`); people `p3` → `p0` →
/// `p1` → `p2` over `reports_to`; `s1`, `s2` and `s5` owned by `p1`, `p0`
/// and `p2`, over `owned_by` (a reference field) and `kept_by` (link
/// records) alike.
async fn lake(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    let rel = |name: &str, target: &str, via: Option<&str>| RelationDecl {
        name: name.into(),
        target: target.into(),
        via: via.map(Into::into),
        ..Default::default()
    };
    let text = || field("name", StorageKind::Text);
    l.declare(type_decl("hub", vec![text()])).await?;
    let mut person = type_decl("person", vec![text(), reference("boss", "person")]);
    person.relations = vec![rel("reports_to", "person", Some("boss"))];
    l.declare(person).await?;
    let mut spoke = type_decl("spoke", vec![text(), reference("hub", "hub")]);
    spoke.fields.push(reference("owner", "person"));
    spoke.relations = vec![
        rel("of", "hub", Some("hub")),
        rel("owned_by", "person", Some("owner")),
    ];
    spoke.relations.push(rel("kept_by", "person", None));
    l.declare(spoke).await?;
    for h in ["h1", "h2"] {
        l.upsert("hub", h, json!({"name": h})).await?;
    }
    l.upsert("person", "p2", json!({"name": "p2"})).await?;
    for (p, boss) in [("p1", "p2"), ("p0", "p1"), ("p3", "p0")] {
        l.upsert("person", p, json!({"name": p, "boss": boss}))
            .await?;
    }
    for i in 1..=9 {
        let (s, hub) = (format!("s{i}"), if i == 9 { "h2" } else { "h1" });
        l.upsert("spoke", &s, json!({"name": s, "hub": hub}))
            .await?;
    }
    let (ty, kept_by) = ("spoke".into(), "kept_by".into());
    for (s, p) in [("s1", "p1"), ("s2", "p0"), ("s5", "p2")] {
        l.upsert("spoke", s, json!({"owner": p})).await?;
        let (from, to, what) = (s.into(), p.into(), format!("linking {s} to {p}"));
        let opts = WriteOpts::default();
        let linked = l
            .cat
            .link(&l.own, &ty, &from, &kept_by, &to, patch(json!({})), opts);
        linked.await.must(&what)?;
    }
    Ok(l)
}

fn st(relation: &str, walk: Walk, min: u32, max: u32) -> Step {
    Step {
        relation: relation.into(),
        walk,
        ty: None,
        filter: None,
        depth: Depth { min, max },
        named: None,
    }
}

fn graph_of(ty: &str, key: Option<&str>, steps: Vec<Step>) -> Pattern {
    pattern(ty, key, steps, Projection::Graph)
}

/// A graph's record keys in order, its links as `from relation to`, and
/// its cursor; `truncated` must say whether a cursor came, and each node
/// has its depth, in the graph's order (018, extended).
async fn graph(
    cat: &dyn Catalog,
    who: &Principal,
    p: Pattern,
    what: &str,
) -> Result<(Vec<String>, Vec<String>, Option<Cursor>), Failure> {
    let QueryResult::Graph {
        nodes,
        depths,
        edges,
        truncated,
        next_cursor,
    } = cat.query(who, p).await.must(what)?.data
    else {
        return Err(Failure::new(format!("{what} answered another shape")));
    };
    ensure!(
        truncated == next_cursor.is_some(),
        "{what}: `truncated` is {truncated} but the cursor is {next_cursor:?}; a cursor comes \
         exactly when the graph is cut"
    );
    ensure!(
        depths.len() == nodes.len() && depths.is_sorted(),
        "{what}: {} nodes have the depths {depths:?}; each has one, in the graph's order",
        nodes.len()
    );
    let edges = edges
        .iter()
        .map(|e| format!("{} {} {}", e.from.1, e.relation, e.to.1));
    let nodes = nodes.iter().map(|n| n.key.to_string()).collect();
    Ok((nodes, edges.collect(), next_cursor))
}

/// Each pattern must be refused as invalid.
async fn refused(cat: &dyn Catalog, who: &Principal, cases: Vec<(Pattern, &str)>) -> Held {
    for (p, what) in cases {
        let err = cat.query(who, p).await.must_fail(what)?;
        ensure!(
            is_invalid(&err),
            "{what} must be refused as invalid; it failed with {err:?}"
        );
    }
    Ok(())
}

async fn a_graph_cut_by_its_limit_pages_on_with_its_cursor(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let paged = |key| Pattern {
        limit: Some(5),
        ..graph_of("hub", key, vec![st("of", In, 0, 1)])
    };
    let (nodes, _, cursor) = graph(cat.as_ref(), own, paged(None), "every hub, 5 a page").await?;
    ensure!(
        nodes == ["h1", "h2", "s1", "s2", "s3"] && cursor.is_some(),
        "several starts are one walk cut at 5 with a cursor; it answered {nodes:?}, {cursor:?}"
    );
    let mut p = paged(Some("h1"));
    let (one, one_edges, cursor) = graph(cat.as_ref(), own, p.clone(), "h1, 5 a page").await?;
    // A spoke written between the pages sorts first, and is in neither.
    l.upsert("spoke", "s0", json!({"name": "s0", "hub": "h1"}))
        .await?;
    p.cursor = cursor.clone();
    let (two, two_edges, more) = graph(cat.as_ref(), own, p, "the second page").await?;
    let edge = |s: &str| format!("{s} of h1");
    ensure!(
        cursor.is_some()
            && one == ["h1", "s1", "s2", "s3", "s4"]
            && one_edges == ["s1", "s2", "s3", "s4"].map(edge)
            && two == ["s5", "s6", "s7", "s8"]
            && two_edges == ["s5", "s6", "s7", "s8"].map(edge)
            && more.is_none(),
        "h1 with 8 spokes at a limit of 5 must page as h1, s1..s4 and then s5..s8, each with \
         its links and the second at the first's position; it answered {one:?} {one_edges:?} \
         ({cursor:?}), then {two:?} {two_edges:?} ({more:?})"
    );
    // Ten spokes and the three people who own one, a record a page and in
    // one page: `s0`, the first page, owns nothing.
    let one = |p: &Pattern| Pattern {
        limit: Some(50),
        ..p.clone()
    };
    let hood = graph_of("spoke", None, vec![st("owned_by", Out, 0, 1)]);
    let mut whole = graph(cat.as_ref(), own, one(&hood), "every spoke").await?;
    whole.1.sort();
    let paged = through(cat.as_ref(), own, &hood, "every spoke, 1 a page").await?;
    ensure!(
        whole.0.len() == 13 && (&whole.0, &whole.1) == (&paged.0, &paged.1),
        "a neighbourhood's pages of 1 are the graph read in one page; it answered {whole:?}, \
         where its pages answered {paged:?}"
    );
    // The people report to each other, to earlier keys and to later ones
    // (p3, p0, p1, p2, and p4 to p1), and then in a cycle. Filtered out, p1
    // is no start, though p4's page comes after its key.
    l.upsert("person", "p4", json!({"name": "p4", "boss": "p1"}))
        .await?;
    l.upsert("person", "p5", json!({"name": "p5"})).await?;
    let people = |filter: Option<&str>, walk, step_where: Option<&str>| {
        let mut step = st("reports_to", walk, 0, 1);
        step.filter = step_where.map(Into::into);
        let mut p = graph_of("person", None, vec![step]);
        p.start.filter = filter.map(Into::into);
        p
    };
    for cycle in [false, true] {
        if cycle {
            l.upsert("person", "p2", json!({"boss": "p3"})).await?;
        }
        for (what, p) in [
            ("out", people(None, Out, None)),
            ("in", people(None, In, None)),
            (
                "filtered, both ways",
                people(Some("name != 'p1'"), Both, None),
            ),
            ("with a step where", people(None, Out, Some("name != 'p0'"))),
        ] {
            let mut whole = graph(cat.as_ref(), own, one(&p), what).await?;
            whole.1.sort();
            let paged = through(cat.as_ref(), own, &p, what).await?;
            ensure!(
                (&whole.0, &whole.1) == (&paged.0, &paged.1),
                "the people's neighbourhood {what} (a cycle: {cycle}), a start a page, is the \
                 graph read in one page; it answered {whole:?}, where its pages answered {paged:?}"
            );
        }
    }
    // A team's `reports_to` onto the people is not walked back from a
    // page's start: a reader masked from its field reads the pages as the
    // graph in one page.
    let mut team = type_decl("team", vec![field("name", StorageKind::Text)]);
    team.fields.push(reference("boss", "person"));
    team.relations = vec![RelationDecl {
        via: Some("boss".into()),
        ..RelationDecl::to("reports_to", "person")
    }];
    l.declare(team).await?;
    l.upsert("team", "t1", json!({"name": "t1", "boss": "p1"}))
        .await?;
    let masked = field_access(store.as_ref(), "masked", "team", &[("boss", Access::None)]);
    let p = people(None, Out, None);
    let mut whole = graph(cat.as_ref(), &masked, one(&p), "masked").await?;
    whole.1.sort();
    let paged = through(cat.as_ref(), &masked, &p, "masked, a start a page").await?;
    ensure!(
        (&whole.0, &whole.1) == (&paged.0, &paged.1),
        "a reader masked from a team's `boss` reads the people a start a page as in one page; \
         it answered {whole:?}, where its pages answered {paged:?}"
    );
    let first = Pattern {
        limit: Some(1),
        ..people(None, Out, None)
    };
    let (_, _, cursor) = graph(cat.as_ref(), own, first.clone(), "a person").await?;
    let cursor = cursor.ok_or_else(|| Failure::new("a person a page has a next page"))?;
    let next = |c| Pattern {
        cursor: Some(c),
        ..first.clone()
    };
    let by_key = graph(cat.as_ref(), own, next(cursor.clone()), "the next person").await?;
    let offset = super::row_pages::forged(&cursor, Value::Null);
    let by_offset = graph(cat.as_ref(), own, next(offset), "by an offset alone").await?;
    ensure!(
        (&by_key.0, &by_key.1) == (&by_offset.0, &by_offset.1),
        "a cursor that names no start reads on by its offset; it answered {by_offset:?}, where \
         the cursor that names one answered {by_key:?}"
    );
    let foreign = super::row_pages::forged(&cursor, json!([[1], "p0"]));
    let answer = cat.query(own, next(foreign)).await;
    ensure!(
        matches!(&answer, Err(e) if invalid_case(e) == Some(&Case::CursorForeign)),
        "a neighbourhood's cursor whose `after` is no depth-0 start is refused as cursor_foreign; \
         it answered {answer:?}"
    );
    Ok(())
}

/// Every page of `p`, a record at a time through its cursors: the nodes
/// in turn and the links together, sorted.
async fn through(
    cat: &dyn Catalog,
    who: &Principal,
    p: &Pattern,
    what: &str,
) -> Result<(Vec<String>, Vec<String>), Failure> {
    let (mut nodes, mut edges, mut cursor) = (Vec::new(), Vec::new(), None);
    loop {
        let q = Pattern {
            limit: Some(1),
            cursor,
            ..p.clone()
        };
        let (n, e, next) = graph(cat, who, q, what).await?;
        nodes.extend(n);
        edges.extend(e);
        cursor = next;
        if cursor.is_none() {
            edges.sort();
            return Ok((nodes, edges));
        }
    }
}

async fn a_chain_graph_past_max_graph_nodes_is_refused_too_large(
    store: Store,
    given: Given,
) -> Held {
    lake(store.as_ref()).await?;
    // What the walk holds counts, not what it answers: a chain through the
    // nine spokes to owners named `nobody`, of whom there are none.
    let mut nobody = st("owned_by", Out, 1, 1);
    nobody.filter = Some("name = 'nobody'".into());
    let chain = |to_nobody: bool| {
        let mut steps = vec![st("of", In, 1, 1)];
        steps.extend(to_nobody.then(|| nobody.clone()));
        graph_of("hub", None, steps)
    };
    let mut store = store;
    for (max, what) in [
        (11, "two hubs and nine spokes under a cap of 11"),
        (10, "past 10"),
        (3, "walking nine spokes to no owner under a cap of 3"),
    ] {
        let limits = Limits {
            max_graph_nodes: max,
            ..crate::conformance::fixture::limits()
        };
        let opened = (given.relimit)(store, Seams::default(), limits).await;
        store = opened.map_err(|e| Failure::new(format!("opening at a cap of {max}: {e}")))?;
        let (cat, own) = (doors(store.as_ref()), owner(store.as_ref()));
        let before = store.head().await.must("reading the head")?;
        let answer = cat.query(&own, chain(max == 3)).await;
        let fits = matches!(&answer, Ok(r) if matches!(&r.data, QueryResult::Graph { nodes, .. } if nodes.len() == 11));
        let refused = matches!(&answer, Err(e) if matches!(reason(e), Some(Refusal::TooLarge { max: m }) if *m == max));
        ensure!(
            if max < 11 { refused } else { fits },
            "a chain's graph of {what} answers its eleven records under 11 and is refused too_large \
             naming its cap below what it walks; it answered {answer:?}"
        );
        crate::conformance::fixture::head_unmoved(store.as_ref(), before, what).await?;
    }
    Ok(())
}

async fn a_graph_refuses_a_cursor_of_another_read_and_a_path_any(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    // From depth 0 the step reaches its hub too, and rows bind one type
    // under a name (lakelands#680): it names its `type`.
    let paged = |max, project, cursor| {
        let mut of = st("of", In, 0, max);
        of.ty = Some("spoke".into());
        Pattern {
            limit: Some(2),
            cursor,
            ..pattern("hub", Some("h1"), vec![of], project)
        }
    };
    let (_, _, g) = graph(
        cat.as_ref(),
        own,
        paged(1, Projection::Graph, None),
        "a graph",
    )
    .await?;
    let rows = cat.query(own, paged(1, Projection::default(), None));
    let rows = rows.await.must("rows")?;
    let QueryResult::Rows { next_cursor: r, .. } = rows.data else {
        return Err(Failure::new("a rows projection answered another shape"));
    };
    let find = Find {
        limit: Some(2),
        ..Find::default()
    };
    let f = cat
        .find(own, &"spoke".into(), find)
        .await
        .must("find")?
        .data;
    let lakelands_api::read::Found::Records { next_cursor: f, .. } = f else {
        return Err(Failure::new("a find answered groups"));
    };
    let count = Projection::Aggregate(Aggregate {
        measures: vec![agg("n", AggFunc::Count, None)],
        group_by: Vec::new(),
    });
    let mut path = paged(1, Projection::Graph, g.clone());
    path.to = Some(("spoke".into(), "s1".into()));
    let cases = vec![
        (
            paged(1, Projection::default(), g.clone()),
            "a graph's cursor handed to its rows",
        ),
        (
            paged(1, Projection::Graph, r),
            "a rows cursor handed to the graph",
        ),
        (
            paged(1, Projection::Graph, f),
            "a `find` cursor handed to a graph",
        ),
        (
            paged(2, Projection::Graph, g.clone()),
            "a cursor handed to another graph",
        ),
        (path, "a cursor handed to a path"),
        (paged(1, count, g), "a cursor handed to an aggregate"),
    ];
    refused(cat.as_ref(), own, cases).await
}

async fn a_graph_honours_each_steps_direction_depth_and_where(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let mut late = st("of", In, 0, 1);
    late.filter = Some("name > 's4'".into());
    let p = graph_of("hub", Some("h1"), vec![late, st("owned_by", Out, 0, 2)]);
    let (nodes, ..) = graph(cat.as_ref(), own, p, "h1's late spokes and their owners").await?;
    ensure!(
        nodes == ["h1", "s5", "s6", "s7", "s8", "p2"],
        "a step's `where` keeps the spokes rows keep, and a spoke it drops is not walked on \
         from (not to p0 or p1); the graph answered {nodes:?}"
    );
    // A step's `where` reads the type its own relation reaches, not the
    // type the step before it reached.
    let mut to_h1 = st("of", Out, 0, 1);
    to_h1.filter = Some("name = 'h1'".into());
    let p = graph_of("spoke", Some("s1"), vec![st("owned_by", Out, 0, 1), to_h1]);
    let (nodes, ..) = graph(cat.as_ref(), own, p, "s1's owner and its hub h1").await?;
    ensure!(
        nodes == ["s1", "h1", "p1"],
        "a step reads its `where` on its own relation's target; the graph answered {nodes:?}"
    );
    for r in ["owned_by", "kept_by"] {
        let up = "reports_to";
        for (key, (a, b), want) in [
            ("s1", (st(r, Out, 1, 1), st(up, In, 1, 2)), "s1 p1 p0 p3"),
            ("p3", (st(up, Out, 2, 2), st(r, In, 1, 1)), "p3 p0 p1 s1"),
            ("p1", (st(up, Out, 0, 1), st(r, In, 0, 1)), "p1 p2 s1"),
            ("p1", (st(up, In, 0, 2), st(r, In, 0, 1)), "p1 p0 s1 p3"),
        ] {
            let ty = ["person", "spoke"][usize::from(key.starts_with('s'))];
            let what = format!("from {key} over {a:?} and {b:?}");
            let p = graph_of(ty, Some(key), vec![a, b]);
            let (nodes, edges, _) = graph(cat.as_ref(), own, p, &what).await?;
            ensure!(
                nodes.join(" ") == want && edges.len() + 1 == nodes.len(),
                "{what}: the graph must be {want} with a link to each record but the start; it \
                 answered {nodes:?} {edges:?}"
            );
        }
    }
    // Several starts are one walk: each record at the fewest hops any start
    // reaches it in, by the links each start's walk took (lakelands#1027).
    let steps = vec![st("reports_to", Out, 1, 2), st("kept_by", In, 1, 1)];
    let p = graph_of("person", None, steps);
    let (nodes, edges, _) = graph(cat.as_ref(), own, p, "every person's bosses' spokes").await?;
    let by = ["p0 reports_to p1", "p1 reports_to p2", "p3 reports_to p0"];
    let kept = ["s1 kept_by p1", "s2 kept_by p0", "s5 kept_by p2"];
    ensure!(
        nodes.join(" ") == "p0 p1 p2 p3 s1 s2 s5" && edges == [by, kept].concat(),
        "several starts walk a chain as one; it answered {nodes:?} {edges:?}"
    );
    let of = |min, max| st("of", In, min, max);
    let mut both = st("owned_by", Walk::Both, 0, 1);
    both.filter = Some("name = 'p1'".into());
    let cases = vec![
        (
            graph_of("hub", Some("h1"), vec![of(2, 1)]),
            "a graph with an empty depth range",
        ),
        (
            graph_of("hub", Some("h1"), vec![of(1, 1), of(1, 1)]),
            "a chain's undeclared step",
        ),
        (
            graph_of("spoke", Some("s1"), vec![both]),
            "a step that reaches spokes and people, with a `where` and no `type`",
        ),
        (
            pattern(
                "spoke",
                Some("s1"),
                vec![st("owned_by", Walk::Both, 0, 1)],
                Projection::Rows {
                    fields: vec!["owned_by.name".into()],
                },
            ),
            "rows reading a field of a step that reaches spokes and people, with no `type`",
        ),
    ];
    refused(cat.as_ref(), own, cases).await
}

async fn a_path_honours_its_steps_and_has_one_start(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let path = |key: Option<&str>, to: &str, step: Step| Pattern {
        to: Some(("person".into(), to.into())),
        ..graph_of("person", key, vec![step])
    };
    let up = |walk, min| st("reports_to", walk, min, 3);
    let mut not_p0 = up(In, 0);
    not_p0.filter = Some("name != 'p0'".into());
    for (step, to, want) in [
        (up(Out, 0), "p0", ""),
        (up(In, 0), "p0", "p1 p0"),
        (up(Both, 0), "p0", "p1 p0"),
        (up(In, 0), "p3", "p1 p0 p3"),
        (not_p0, "p3", ""),
    ] {
        let what = format!("the path from p1 to {to} over {step:?}");
        let (nodes, ..) = graph(cat.as_ref(), own, path(Some("p1"), to, step), &what).await?;
        ensure!(
            nodes.join(" ") == want,
            "{what} must be [{want}]; it answered {nodes:?}"
        );
    }
    let mut rows = path(Some("p1"), "p0", up(In, 0));
    rows.project = Projection::default();
    let cases = vec![
        (path(None, "p0", up(In, 0)), "a path from every person"),
        (rows, "a path asked as rows"),
        (
            path(Some("p1"), "p0", up(In, 1)),
            "a path whose step starts at 1",
        ),
    ];
    refused(cat.as_ref(), own, cases).await
}

async fn a_graph_shows_a_reader_no_more_than_its_rows(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let blind = field_access(store.as_ref(), "blind", "spoke", &[("name", Access::None)]);
    let mut late = st("of", In, 0, 1);
    late.filter = Some("name > 's4'".into());
    let as_graph = graph_of("hub", Some("h1"), vec![late.clone()]);
    late.depth.min = 1;
    let as_rows = pattern("hub", Some("h1"), vec![late], Projection::default());
    let in_graph = cat.query(&blind, as_graph);
    let in_graph = in_graph.await.must_fail("a graph's `where`")?;
    let in_rows = cat.query(&blind, as_rows);
    let in_rows = in_rows.await.must_fail("rows' `where`")?;
    ensure!(
        reason(&in_graph).is_some() && reason(&in_graph) == reason(&in_rows),
        "a `where` on a field the reader may not read is refused in a graph as in rows; the \
         graph failed with {in_graph:?}, the rows with {in_rows:?}"
    );
    restrict(store.as_ref(), "spoke", "s5", "a held spoke").await?;
    restrict(store.as_ref(), "person", "p0", "a held person").await?;
    let eve = reads_all(store.as_ref(), "eve");
    let hub = graph_of("hub", Some("h1"), vec![st("of", In, 0, 1)]);
    let (nodes, edges, _) = graph(cat.as_ref(), &eve, hub, "h1's spokes, as eve").await?;
    let rows = pattern(
        "hub",
        Some("h1"),
        vec![st("of", In, 1, 1)],
        Projection::default(),
    );
    let QueryResult::Rows { rows, .. } = cat.query(&eve, rows).await.must("eve's rows")?.data
    else {
        return Err(Failure::new("a rows projection answered another shape"));
    };
    let spokes: Vec<_> = rows
        .iter()
        .filter_map(|r| r.get("spoke.key")?.as_str())
        .collect();
    ensure!(
        nodes[1..] == spokes[..] && !edges.iter().any(|e| e.contains("s5")),
        "a restricted spoke and its link are left out of eve's graph as out of her rows \
         {spokes:?}; the graph answered {nodes:?} {edges:?}"
    );
    let chain = vec![st("reports_to", Out, 2, 2), st("kept_by", In, 1, 1)];
    let chain = graph_of("person", Some("p3"), chain);
    let (nodes, ..) = graph(cat.as_ref(), &eve, chain.clone(), "p3's chain, as eve").await?;
    let (all, ..) = graph(cat.as_ref(), own, chain, "p3's chain, as the owner").await?;
    ensure!(
        nodes == ["p3"] && all == ["p3", "p0", "p1", "s1"],
        "a chain is not walked through p0, which eve is not shown: she must see [p3], the \
         owner [p3, p0, p1, s1]; they saw {nodes:?} and {all:?}"
    );
    Ok(())
}

async fn a_row_says_the_hops_each_step_took_and_a_graph_each_nodes_depth(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let mut node = type_decl("node", vec![field("name", StorageKind::Text)]);
    node.fields.push(reference("parent", "node"));
    node.relations = vec![RelationDecl {
        via: Some("parent".into()),
        ..RelationDecl::to("parent", "node")
    }];
    l.declare(node).await?;
    for (k, parent) in [
        ("a", None),
        ("b", Some("a")),
        ("c", Some("b")),
        ("d", Some("c")),
    ] {
        l.upsert("node", k, json!({"name": k, "parent": parent}))
            .await?;
    }
    // The ancestors of `d`, each with its hops; a row as `name key depth`.
    let ask = |who: Principal, max: u32, extra: serde_json::Value| {
        let mut p = json!({"start": {"type": "node", "key": "d", "as": "start"},
            "steps": [{"relation": "parent", "walk": "out", "depth": {"min": 1, "max": max}, "as": "up"}]});
        p.as_object_mut()
            .unwrap()
            .extend(extra.as_object().cloned().unwrap_or_default());
        // A graph binds no name.
        if p["project"]["as"] == "graph" {
            p["start"].as_object_mut().unwrap().remove("as");
            p["steps"][0].as_object_mut().unwrap().remove("as");
        }
        let (cat, p) = (l.cat.clone(), serde_json::from_value::<Pattern>(p));
        async move {
            Ok::<_, Failure>(
                cat.query(&who, p.map_err(|e| Failure::new(e.to_string()))?)
                    .await
                    .must("the ancestors")?
                    .data,
            )
        }
    };
    let text = |v: Option<&serde_json::Value>| {
        v.map_or("-".into(), |v| v.as_str().map_or(v.to_string(), Into::into))
    };
    let rows = |r: QueryResult, cols: &[&str]| match r {
        QueryResult::Rows { rows, .. } => rows
            .iter()
            .map(|r| {
                cols.iter()
                    .map(|c| text(r.get(*c)))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>(),
        QueryResult::Groups { rows } => rows
            .iter()
            .map(|g| {
                format!(
                    "{}={}",
                    text(g.by.values().next()),
                    text(g.values.values().next())
                )
            })
            .collect(),
        QueryResult::Graph { nodes, depths, .. } => (nodes.iter().zip(depths))
            .map(|(n, d)| format!("{} {d}", n.key))
            .collect(),
    };
    let own = l.own.clone();
    let cols = ["start._depth", "up.key", "up._depth"];
    let mut said = Vec::new();
    said.push(rows(ask(own.clone(), 3, json!({})).await?, &cols).join(", "));
    let desc = json!({"order_by": [{"field": "up._depth", "dir": "desc"}], "project": {"as": "rows", "fields": ["up.key", "up._depth"]}});
    said.push(rows(ask(own.clone(), 3, desc).await?, &cols).join(", "));
    let counted = json!({"project": {"as": "aggregate", "measures": [{"name": "n", "func": "count"}], "group_by": ["up._depth"]}});
    said.push(rows(ask(own.clone(), 3, counted).await?, &[]).join(", "));
    let far = json!({"project": {"as": "aggregate", "measures": [{"name": "n", "func": "count"}],
        "group_by": [{"name": "far", "expr": "up._depth > 1"}]}});
    said.push(rows(ask(own.clone(), 3, far).await?, &[]).join(", "));
    said.push(
        rows(
            ask(own.clone(), 3, json!({"project": {"as": "graph"}})).await?,
            &[],
        )
        .join(", "),
    );
    // A cycle: `a` points back at `d`, which rows answer at the hop it
    // comes back and a graph keeps at 0.
    l.upsert("node", "a", json!({"name": "a", "parent": "d"}))
        .await?;
    said.push(rows(ask(own.clone(), 4, json!({})).await?, &cols).join(", "));
    said.push(
        rows(
            ask(own.clone(), 4, json!({"project": {"as": "graph"}})).await?,
            &[],
        )
        .join(", "),
    );
    // `b` restricted: a reader without `restricted` reaches `c` alone.
    restrict(store.as_ref(), "node", "b", "held").await?;
    said.push(
        rows(
            ask(reads_all(store.as_ref(), "plain"), 4, json!({})).await?,
            &cols,
        )
        .join(", "),
    );
    // Two ways to `s2` around a ring, the shorter through `r1`, which is
    // restricted: a reader without `restricted` is answered the longer
    // way's depth, as a walk that never met `r1`.
    let edges = [
        ("t0", "v4"),
        ("r1", "t0"),
        ("s2", "r1"),
        ("u3", "s2"),
        ("v4", "u3"),
    ];
    for (k, _) in edges {
        l.upsert("node", k, json!({"name": k})).await?;
    }
    for (k, parent) in edges {
        l.upsert("node", k, json!({"name": k, "parent": parent}))
            .await?;
    }
    restrict(store.as_ref(), "node", "r1", "held").await?;
    let ring = json!({"start": {"type": "node", "key": "t0"},
        "steps": [{"relation": "parent", "walk": "both", "depth": {"min": 1, "max": 3}, "as": "up"}]});
    let ring: Pattern = serde_json::from_value(ring).map_err(|e| Failure::new(e.to_string()))?;
    for who in [own.clone(), reads_all(store.as_ref(), "plain")] {
        let got = l.cat.query(&who, ring.clone()).await.must("the ring")?.data;
        said.push(rows(got, &["up.key", "up._depth"]).join(", "));
    }
    let want = [
        "0 c 1, 0 b 2, 0 a 3",
        "- a 3, - b 2, - c 1",
        "1=1, 2=1, 3=1",
        "false=1, true=2",
        "d 0, c 1, b 2, a 3",
        "0 c 1, 0 b 2, 0 a 3, 0 d 4",
        "d 0, c 1, b 2, a 3",
        "0 c 1",
        "r1 1, v4 1, s2 2, t0 2, u3 2",
        "v4 1, t0 2, u3 2, s2 3",
    ];
    ensure!(
        said == want,
        "the ancestors of `d` with their depths answered {said:?}; want {want:?}"
    );
    // A new field that begins with `_` is the lake's name.
    let kept = |r: Result<lakelands_api::write::Outcome, lakelands_api::Error>| {
        matches!(
            invalid_case(&r),
            Some(lakelands_api::error::Case::FieldNameKept { .. })
        )
    };
    let mut bad = type_decl("bad", vec![field("_x", StorageKind::Text)]);
    bad.key = Vec::new();
    let declared = l
        .cat
        .declare(&own, lakelands_api::write::Declaration::Type(bad))
        .await;
    let added = l
        .cat
        .add_field(&own, &"node".into(), field("_y", StorageKind::Text))
        .await;
    ensure!(
        kept(declared.clone()) && kept(added.clone()),
        "a new field `_x` and an added field `_y` must be refused as names the lake keeps: {declared:?}, {added:?}"
    );
    Ok(())
}

async fn reach_answers_each_record_at_the_first_step_that_reaches_it(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    // From p3: its bosses, who reports to them, the spokes they own (or
    // keep), those spokes' hub, and the hub's spokes.
    let walk = |owns: &str| {
        let steps = vec![
            st("reports_to", Out, 1, 2),
            st("reports_to", In, 1, 1),
            st(owns, In, 1, 1),
            st("of", Out, 1, 1),
            st("of", In, 1, 1),
        ];
        pattern("person", Some("p3"), steps, Projection::Reach)
    };
    // Each page as its rows' `step/depth/key`, until no cursor comes.
    let read = |mut p: Pattern, who: Principal, what: &'static str| {
        let cat = cat.clone();
        async move {
            let mut got = Vec::new();
            loop {
                let reply = cat.query(&who, p.clone()).await.must(what)?.data;
                let QueryResult::Rows { rows, next_cursor } = reply else {
                    return Err(Failure::new(format!("{what} answered another shape")));
                };
                let row = |r: &BTreeMap<String, Value>| {
                    format!("{}/{}/{}", r["step"], r["depth"], r["key"])
                };
                let page: Vec<String> = rows.iter().map(row).collect();
                got.push(page.join(" ").replace('"', ""));
                p.cursor = next_cursor;
                if p.cursor.is_none() {
                    return Ok::<_, Failure>(got);
                }
            }
        }
    };
    let want = [
        "1/1/p0 1/2/p1 2/1/p3 3/1/s2 4/1/h1",
        "5/1/s1 5/1/s3 5/1/s4 5/1/s5 5/1/s6",
        "5/1/s7 5/1/s8",
    ];
    for owns in ["owned_by", "kept_by"] {
        let paged = Pattern {
            limit: Some(5),
            ..walk(owns)
        };
        let got = read(paged, own.clone(), "a reach in pages of 5").await?;
        ensure!(
            got == want,
            "over `{owns}`: p0 once at step 1 though step 2 reaches it again, p3 at step 2 that \
             reaches it back, p1 walked on from nowhere past step 2, s9 never; pages of 5 \
             {want:?}; it answered {got:?}"
        );
    }
    // A step whose range starts at 2 keeps what each record it walks from
    // reaches in 2, though another reaches it in 1: p2 from p0.
    let mut two = walk("owned_by");
    two.steps = vec![st("reports_to", Out, 1, 2), st("reports_to", Out, 2, 2)];
    let got = read(two, own.clone(), "a reach of 2..2").await?;
    ensure!(
        got == ["1/1/p0 1/2/p1 2/2/p2"],
        "p2 is 2 hops from p0, though 1 from p1; it answered {got:?}"
    );
    // The start at depth 0 where its step's range begins there, and at 2
    // where a `both` step comes back over its own link.
    for (step, want) in [
        (st("reports_to", Out, 0, 1), "1/0/p3 1/1/p0"),
        (st("reports_to", Both, 1, 4), "1/1/p0 1/2/p1 1/2/p3 1/3/p2"),
    ] {
        let mut one = walk("owned_by");
        one.steps = vec![step];
        let got = read(one, own.clone(), "a reach of one step").await?;
        ensure!(
            got == [want],
            "p3 at the hop its step reaches it; it answered {got:?}"
        );
    }
    restrict(store.as_ref(), "spoke", "s2", "hold").await?;
    let reader = reads_all(store.as_ref(), "reader");
    let got = read(walk("owned_by"), reader, "a reader's reach").await?;
    ensure!(
        got == ["1/1/p0 1/2/p1 2/1/p3"],
        "s2, restricted, is neither answered nor walked through to h1; it answered {got:?}"
    );
    let mut to = walk("owned_by");
    to.to = Some(("hub".into(), "h1".into()));
    let mut named = walk("owned_by");
    named.steps[0].named = Some("boss".into());
    let mut ordered = walk("owned_by");
    ordered.order_by = vec![order("p3.name", OrderBy::Value, Direction::Asc)];
    let answer = cat.query(own, to).await;
    refused_as!("reach with `to`", answer, Case::PatternToNotGraph)?;
    let mut none = walk("owned_by");
    none.steps.clear();
    let answer = cat.query(own, none).await;
    refused_as!("reach of no steps", answer, Case::MeasureNoSteps { measure: m } if m == "reach")?;
    let cases = vec![(named, "with `as`"), (ordered, "with `order_by`")];
    refused(cat.as_ref(), own, cases).await
}
