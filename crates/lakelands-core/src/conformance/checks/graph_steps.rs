//! A graph and a path answer what their steps say: each step's relation,
//! direction, depth, `type` and `where`, a page at a time
//! (`docs/decisions/018-one-pattern-query.md`, lakelands#556).

use lakelands_api::ontology::{RelationDecl, StorageKind};
use lakelands_api::query::Walk::{Both, In, Out};
use lakelands_api::query::{Depth, Pattern, Projection, QueryResult, Step, Walk};
use lakelands_api::read::{AggFunc, Aggregate};
use lakelands_api::write::WriteOpts;
use lakelands_api::{Access, Catalog, Cursor, Find, LakeStore, Principal};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, agg, field, field_access, is_invalid, patch, pattern, reads_all, reason, reference,
    restrict, type_decl,
};
use crate::conformance::{Failure, Held, Store};

check!(
    GRAPH_PAGES = a_graph_cut_by_its_limit_pages_on_with_its_cursor,
    "018 and 029, extended: a graph cut by `limit` answers `truncated` and a `next_cursor`, and the next page holds the next records of the one order with every link between a record of that page and one of this or an earlier page, read at the first page's position; several starts are one walk, cut together"
);

check!(
    GRAPH_CURSORS = a_graph_refuses_a_cursor_of_another_read_and_a_path_any,
    "018: a graph refuses a cursor handed out for rows, for `find` or for another pattern, and a path and an aggregate, which have no pages, refuse every cursor, each as invalid before anything is walked"
);

check!(
    GRAPH_STEPS = a_graph_honours_each_steps_direction_depth_and_where,
    "018, extended: a graph whose steps all start at depth 0 walks each relation in its own step's direction to its own `max`, keeping and walking on from a record only where its step's `where`, read against the type its relation reaches wherever it is declared, holds, and a step that reaches several types and reads a `where` names its `type`; any other is a chain walked as rows walk it, answered as the start, what rows bind, the records between and the links walked; an empty range and a chain's undeclared relation are refused; over a reference field and over link records alike"
);

check!(
    PATH_STEPS = a_path_honours_its_steps_and_has_one_start,
    "018, extended: a path walks each relation in its step's direction, only through records its step's `where` keeps, and no path is an empty graph; several starts, a projection other than `graph`, and a step starting below depth 0 are refused as invalid"
);

check!(
    GRAPH_RIGHTS = a_graph_shows_a_reader_no_more_than_its_rows,
    "024 and 026, extended: a graph's step `where` on a field the reader may not read is refused as in rows; a restricted record and its links are left out of a graph as out of rows, and a chain is not walked through a record the reader is not shown"
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
    }
}

fn graph_of(ty: &str, key: Option<&str>, steps: Vec<Step>) -> Pattern {
    pattern(ty, key, steps, Projection::Graph)
}

/// A graph's record keys in order, its links as `from relation to`, and
/// its cursor; `truncated` must say whether a cursor came.
async fn graph(
    cat: &dyn Catalog,
    who: &Principal,
    p: Pattern,
    what: &str,
) -> Result<(Vec<String>, Vec<String>, Option<Cursor>), Failure> {
    let QueryResult::Graph {
        nodes,
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
    Ok(())
}

async fn a_graph_refuses_a_cursor_of_another_read_and_a_path_any(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let paged = |max, project, cursor| Pattern {
        limit: Some(2),
        cursor,
        ..pattern("hub", Some("h1"), vec![st("of", In, 0, max)], project)
    };
    let (_, _, g) = graph(
        cat.as_ref(),
        own,
        paged(1, Projection::Graph, None),
        "a graph",
    )
    .await?;
    let rows = cat
        .query(own, paged(1, Projection::default(), None))
        .await
        .must("rows")?;
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
    let in_graph = cat
        .query(&blind, as_graph)
        .await
        .must_fail("a graph's `where`")?;
    let in_rows = cat
        .query(&blind, as_rows)
        .await
        .must_fail("rows' `where`")?;
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
