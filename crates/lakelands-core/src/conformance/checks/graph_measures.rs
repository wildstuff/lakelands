//! The whole-graph measures of the pattern query: degree centrality and
//! connected components, over the links a walk would follow, in one order,
//! bounded by the lake's `max_graph_nodes`
//! (`docs/decisions/018-one-pattern-query.md`,
//! `docs/decisions/026-the-store-walks-the-graph.md`,
//! `docs/decisions/029-every-answer-has-one-order.md`).

use std::collections::BTreeMap;
use std::sync::Arc;

use lakelands_api::location::Limits;
use lakelands_api::ontology::{Cardinality, RelationDecl, StorageKind, TypeDecl};
use lakelands_api::query::{Depth, Pattern, Projection, QueryResult, SearchMode, Step, Walk};
use lakelands_api::seam::Seams;
use lakelands_api::write::WriteOpts;
use lakelands_api::{Catalog, Cursor, LakeStore, Position, Principal, Refusal, Value};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, doors, field, invalid_naming, order, owner, patch, pattern, reads_all, reason,
    reference, restrict, searchable, step, type_decl,
};
use crate::conformance::{Failure, Given};

check!(
    /// Degree counts what a walk would follow.
    DEGREE_COUNTS = degree_counts_each_records_links_to_ends_it_may_see_highest_first,
    "026, 029 and 035: `degree` counts each start record's links over the steps' relations in each step's direction, a via reference and a link record alike, but not a link to a retired end or, without the grant, a restricted link; highest first, ties by key, paged by limit; a start by key or by `where` counts those records only, and a key no record has counts none"
);

check!(
    /// Components partition what the start records reach.
    COMPONENTS_PARTITION = components_partition_what_the_start_reaches_largest_first,
    "026 and 029: `components` puts every record reached from the start over the steps' relations, both ways, in exactly one component, across types; components largest first, ties by their least (type, key), members by (type, key); a start by key or by a search reaches from those records only"
);

check!(
    /// A measure reads the graph at its position.
    MEASURES_AT_THEIR_POSITION = a_measure_reads_the_graph_at_the_position_it_is_asked_at,
    "009 and 026: a measure `as_of` an earlier position counts the links live there, not a link written later"
);

check!(
    /// Past the cap, components are refused, not cut.
    GRAPH_TOO_LARGE = components_past_the_lakes_max_graph_nodes_are_refused_too_large,
    "018 and the location's limits: `components` whose records, over all components, would number more than `max_graph_nodes` are refused as too_large naming the ceiling, and nothing partial is answered",
    given
);

check!(
    /// A measure's page may be as large as its answer.
    MEASURE_IN_ONE_PAGE = a_measure_is_read_whole_in_one_page_up_to_max_graph_nodes,
    "026, extended, and the location's limits: a measure's `limit` may go up to the lake's `max_graph_nodes` where that is above `max_limit`, so one page answers the rows its small pages answer in turn, with no cursor; above that bound, and above `max_limit` for a `rows` projection, a `limit` is refused as invalid naming the bound",
    given
);

check!(
    /// A measure refuses what it would otherwise ignore.
    MEASURE_REFUSALS = a_measure_refuses_depth_where_to_order_by_and_one_way_components,
    "018: a measure refuses a step `depth` or `where`, `to`, `order_by`, and a `components` step that does not walk `both`, each as invalid, rather than answer less than it seems to"
);

/// Partners p1..p3; invoices i0..i5 (i0 linked to nothing); `billed_to` by the reference field
/// `partner`, and `approved_by` carried by link records.
///
/// - billed_to: i1→p1, i2→p1, i3→p2, i5→p3 (p3 retired);
/// - approved_by: i2→p1, i4→p1 (restricted), then i1→p2, written last.
///
/// Answers the position before the last link.
async fn lake(store: &dyn LakeStore) -> Result<Position, Failure> {
    let l = Lake::of(store);
    l.declare(type_decl("partner", vec![field("name", StorageKind::Text)]))
        .await?;
    let mut invoice: TypeDecl = type_decl(
        "invoice",
        vec![searchable("note"), reference("partner", "partner")],
    );
    invoice.relations = vec![
        RelationDecl {
            name: "billed_to".into(),
            target: "partner".into(),
            cardinality: Cardinality::ManyToOne,
            via: Some("partner".into()),
            ..Default::default()
        },
        RelationDecl {
            name: "approved_by".into(),
            target: "partner".into(),
            cardinality: Cardinality::ManyToMany,
            ..Default::default()
        },
    ];
    l.declare(invoice).await?;
    for p in ["p1", "p2", "p3"] {
        l.upsert("partner", p, json!({"name": p})).await?;
    }
    for (i, p) in [
        ("i0", None),
        ("i1", Some("p1")),
        ("i2", Some("p1")),
        ("i3", Some("p2")),
        ("i4", None),
        ("i5", Some("p3")),
    ] {
        let fields = p.map_or_else(|| json!({"note": i}), |p| json!({"note": i, "partner": p}));
        l.upsert("invoice", i, fields).await?;
    }
    l.retire("partner", "p3").await?;
    for (from, to) in [("i2", "p1"), ("i4", "p1")] {
        link(&l, from, to).await?;
    }
    let link_key = lakelands_api::key::link_key(&"i4".into(), &"p1".into());
    restrict(store, "sys.link.invoice.approved_by", link_key, "hold").await?;
    let before = store.head().await.must("reading the head")?;
    link(&l, "i1", "p2").await?;
    Ok(before)
}

async fn link(l: &Lake, from: &str, to: &str) -> Result<(), Failure> {
    let (cat, who) = (l.cat.as_ref(), &l.own);
    cat.link(
        who,
        &"invoice".into(),
        &from.into(),
        &"approved_by".into(),
        &to.into(),
        patch(json!({})),
        WriteOpts::default(),
    )
    .await
    .must(&format!("linking {from} to {to}"))?;
    Ok(())
}

/// A measure over all the records of `ty`.
fn measure(ty: &str, steps: Vec<Step>, project: Projection) -> Pattern {
    pattern(ty, None, steps, project)
}

/// A measure over all the records of `ty`, walking one relation.
fn one(ty: &str, relation: &str, walk: Walk, project: Projection) -> Pattern {
    measure(ty, vec![step(relation, walk)], project)
}

fn both() -> Vec<Step> {
    vec![
        step("billed_to", Walk::Both),
        step("approved_by", Walk::Both),
    ]
}

/// The rows a measure answers, each as `column=value` joined by spaces,
/// and the cursor to the next page.
async fn rows(
    cat: &dyn Catalog,
    who: &Principal,
    p: Pattern,
    what: &str,
) -> Result<(Vec<String>, Option<Cursor>), Failure> {
    let reply = cat.query(who, p).await.must(what)?;
    let QueryResult::Rows { rows, next_cursor } = reply.data else {
        return Err(Failure::new(format!(
            "{what} answered something other than rows"
        )));
    };
    Ok((rows.iter().map(show).collect(), next_cursor))
}

fn show(row: &BTreeMap<String, Value>) -> String {
    let cell = |c: &str| match row.get(c) {
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => "-".to_owned(),
    };
    if row.contains_key("degree") {
        format!("{}:{}={}", cell("type"), cell("key"), cell("degree"))
    } else {
        format!(
            "{}/{} {}:{}",
            cell("component"),
            cell("size"),
            cell("type"),
            cell("key")
        )
    }
}

async fn degree_counts_each_records_links_to_ends_it_may_see_highest_first(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let out = || {
        measure(
            "invoice",
            vec![step("billed_to", Walk::Out), step("approved_by", Walk::Out)],
            Projection::Degree,
        )
    };
    let (got, _) = rows(cat.as_ref(), own, out(), "the invoices' out-degree").await?;
    ensure!(
        got == [
            "invoice:i1=2",
            "invoice:i2=2",
            "invoice:i3=1",
            "invoice:i4=1",
            "invoice:i0=0",
            "invoice:i5=0"
        ],
        "the owner's out-degree over `billed_to` and `approved_by` is i1 2 (p1, p2), i2 2 \
         (p1 by each relation), i3 1, i4 1 (the restricted link), i5 0 (its partner is \
         retired) and i0 0, highest first and ties by key; it answered {got:?}"
    );
    let reader = reads_all(store.as_ref(), "reader");
    let (got, _) = rows(cat.as_ref(), &reader, out(), "a reader's out-degree").await?;
    ensure!(
        got == [
            "invoice:i1=2",
            "invoice:i2=2",
            "invoice:i3=1",
            "invoice:i0=0",
            "invoice:i4=0",
            "invoice:i5=0"
        ],
        "without the `restricted` grant the link i4 → p1 is not counted; it answered {got:?}"
    );
    let inn = one("partner", "billed_to", Walk::In, Projection::Degree);
    let (got, _) = rows(cat.as_ref(), own, inn, "the partners' in-degree").await?;
    ensure!(
        got == ["partner:p1=2", "partner:p2=1"],
        "the partners' in-degree over `billed_to` is p1 2, p2 1, and the retired p3 is no \
         start; it answered {got:?}"
    );
    let started = |key: Option<&str>, filter: Option<&str>| {
        let mut p = out();
        p.start.key = key.map(Into::into);
        p.start.filter = filter.map(Into::into);
        p
    };
    for (p, want, what) in [
        (
            started(Some("i1"), None),
            vec!["invoice:i1=2"],
            "started on i1",
        ),
        (
            started(Some("i9"), None),
            vec![],
            "started on a key no record has",
        ),
        (
            started(None, Some("note = 'i3' or note = 'i4'")),
            vec!["invoice:i3=1", "invoice:i4=1"],
            "started where `note` is i3 or i4",
        ),
    ] {
        let (got, _) = rows(cat.as_ref(), own, p, what).await?;
        ensure!(
            got == want,
            "the out-degree {what} holds its start's rows only, {want:?}; it answered {got:?}"
        );
    }
    let mut first = out();
    first.limit = Some(2);
    let (page, next_cursor) = rows(cat.as_ref(), own, first, "the first page of degrees").await?;
    ensure!(
        page == ["invoice:i1=2", "invoice:i2=2"] && next_cursor.is_some(),
        "a first page of two holds i1 and i2 and a cursor; it answered {page:?}"
    );
    let mut next = out();
    next.limit = Some(2);
    next.cursor = next_cursor;
    let (got, _) = rows(cat.as_ref(), own, next, "the second page of degrees").await?;
    ensure!(
        got == ["invoice:i3=1", "invoice:i4=1"],
        "the second page holds i3 and i4; it answered {got:?}"
    );
    Ok(())
}

async fn components_partition_what_the_start_reaches_largest_first(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let all = || measure("invoice", both(), Projection::Components);
    let (got, _) = rows(cat.as_ref(), own, all(), "the invoices' components").await?;
    let want = [
        "0/6 invoice:i1",
        "0/6 invoice:i2",
        "0/6 invoice:i3",
        "0/6 invoice:i4",
        "0/6 partner:p1",
        "0/6 partner:p2",
        "1/1 invoice:i0",
        "2/1 invoice:i5",
    ];
    ensure!(
        got == want,
        "the invoices and the partners they reach form one component of six (i3 joins \
         through p2 and the link i1 → p2); i0 and i5 (p3 is retired) stand alone and \
         come after it, though i0 sorts first; it answered {got:?}"
    );
    let started = |key: Option<&str>, search: Option<&str>| {
        let mut p = all();
        p.start.key = key.map(Into::into);
        p.start.search = search.map(SearchMode::lexical);
        p
    };
    for (p, want, what) in [
        (
            started(Some("i0"), None),
            vec!["0/1 invoice:i0"],
            "started on i0",
        ),
        (
            started(Some("i3"), None),
            want[..6].to_vec(),
            "started on i3",
        ),
        (
            started(None, Some("i3")),
            want[..6].to_vec(),
            "started by a search for i3",
        ),
    ] {
        let (got, _) = rows(cat.as_ref(), own, p, what).await?;
        ensure!(
            got == want,
            "the components {what} are what its start reaches only, {want:?}; it answered \
             {got:?}"
        );
    }
    let reader = reads_all(store.as_ref(), "reader");
    let (got, _) = rows(cat.as_ref(), &reader, all(), "a reader's components").await?;
    let want = [
        "0/5 invoice:i1",
        "0/5 invoice:i2",
        "0/5 invoice:i3",
        "0/5 partner:p1",
        "0/5 partner:p2",
        "1/1 invoice:i0",
        "2/1 invoice:i4",
        "3/1 invoice:i5",
    ];
    ensure!(
        got == want,
        "without the `restricted` grant i4 is alone, and the two single components come \
         by their least member; it answered {got:?}"
    );
    Ok(())
}

async fn a_measure_reads_the_graph_at_the_position_it_is_asked_at(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let before = lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let mut then = measure("invoice", both(), Projection::Components);
    then.at.as_of = Some(before);
    let (got, _) = rows(
        cat.as_ref(),
        own,
        then,
        "the components before the last link",
    )
    .await?;
    let want = [
        "0/4 invoice:i1",
        "0/4 invoice:i2",
        "0/4 invoice:i4",
        "0/4 partner:p1",
        "1/2 invoice:i3",
        "1/2 partner:p2",
        "2/1 invoice:i0",
        "3/1 invoice:i5",
    ];
    ensure!(
        got == want,
        "at position {before}, before the link i1 → p2, i3 and p2 are a component of \
         their own; it answered {got:?}"
    );
    let mut then = one("invoice", "approved_by", Walk::Out, Projection::Degree);
    then.at.as_of = Some(before);
    let (got, _) = rows(cat.as_ref(), own, then, "the degrees before the last link").await?;
    ensure!(
        got.first().map(String::as_str) == Some("invoice:i2=1")
            && got.contains(&"invoice:i1=0".to_owned()),
        "at position {before} i1 has no `approved_by` link yet; it answered {got:?}"
    );
    Ok(())
}

async fn components_past_the_lakes_max_graph_nodes_are_refused_too_large(
    store: Arc<dyn LakeStore>,
    given: Given,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let limits = |max_graph_nodes| Limits {
        max_graph_nodes,
        ..crate::conformance::fixture::limits()
    };
    let store = (given.relimit)(store, Seams::default(), limits(8))
        .await
        .map_err(|e| {
            Failure::new(format!(
                "opening the lake again with max_graph_nodes 7 failed: {e}"
            ))
        })?;
    let all = || measure("invoice", both(), Projection::Components);
    let (got, _) = rows(
        doors(store.as_ref()).as_ref(),
        &owner(store.as_ref()),
        all(),
        "components of eight records under a cap of 8",
    )
    .await
    .map(|r| (r.0.len(), r.1))
    .map_err(|e| Failure::new(format!("{e}: the eight records fit a cap of 8")))?;
    ensure!(
        got == 8,
        "under a cap of 8 the components answer eight rows; they answered {got}"
    );
    let store = (given.relimit)(store, Seams::default(), limits(7))
        .await
        .map_err(|e| {
            Failure::new(format!(
                "opening the lake again with max_graph_nodes 6 failed: {e}"
            ))
        })?;
    let before = store.head().await.must("reading the head")?;
    let err = doors(store.as_ref())
        .query(&owner(store.as_ref()), all())
        .await
        .must_fail("components past a cap of 7")?;
    ensure!(
        matches!(reason(&err), Some(Refusal::TooLarge { max: 7 })),
        "components that reach eight records past a cap of 7 are refused as too_large \
         naming 7; it failed with {err:?}"
    );
    crate::conformance::fixture::head_unmoved(store.as_ref(), before, "a measure past the cap")
        .await
}

async fn a_measure_refuses_depth_where_to_order_by_and_one_way_components(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let degree = || one("invoice", "billed_to", Walk::Out, Projection::Degree);
    let mut cases: Vec<(&str, Pattern)> = Vec::new();
    let mut p = degree();
    p.steps[0].depth = Depth { min: 1, max: 2 };
    cases.push(("a step depth", p));
    let mut p = degree();
    p.steps[0].filter = Some("name = 'p1'".into());
    cases.push(("a step where", p));
    let mut p = degree();
    p.to = Some(("partner".into(), "p1".into()));
    cases.push(("`to`", p));
    let mut p = degree();
    p.order_by = vec![order("degree", Default::default(), Default::default())];
    cases.push(("`order_by`", p));
    let one_way = one("invoice", "billed_to", Walk::Out, Projection::Components);
    cases.push(("components walking one way", one_way));
    cases.push((
        "a measure with no step",
        measure("invoice", Vec::new(), Projection::Degree),
    ));
    for (what, p) in cases {
        let err = cat
            .query(own, p)
            .await
            .must_fail(&format!("a measure with {what}"))?;
        ensure!(
            matches!(reason(&err), Some(Refusal::Invalid { .. })),
            "a measure with {what} must be refused as invalid; it failed with {err:?}"
        );
    }
    Ok(())
}

async fn a_measure_is_read_whole_in_one_page_up_to_max_graph_nodes(
    store: Arc<dyn LakeStore>,
    given: Given,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    // Pages of two, and room for the eight records the components reach.
    let limits = Limits {
        default_limit: 2,
        max_limit: 2,
        max_graph_nodes: 8,
        ..crate::conformance::fixture::limits()
    };
    let store = (given.relimit)(store, Seams::default(), limits)
        .await
        .map_err(|e| {
            Failure::new(format!(
                "opening the lake again with max_limit 2 failed: {e}"
            ))
        })?;
    let (cat, own) = (doors(store.as_ref()), owner(store.as_ref()));
    let paged = |project: &Projection, limit, cursor| Pattern {
        limit: Some(limit),
        cursor,
        ..measure("invoice", both(), project.clone())
    };
    for project in [Projection::Components, Projection::Degree] {
        let one = paged(&project, 8, None);
        let (whole, next) = rows(cat.as_ref(), &own, one, "a measure in one page of 8").await?;
        let (mut turns, mut cursor) = (Vec::new(), None);
        loop {
            let two = paged(&project, 2, cursor);
            let (page, next) = rows(cat.as_ref(), &own, two, "a measure in pages of 2").await?;
            turns.extend(page);
            match next {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        ensure!(
            next.is_none() && whole.len() > 2 && whole == turns,
            "under `max_limit` 2 and `max_graph_nodes` 8, {project:?} with `limit` 8 must answer \
             in one page, without a cursor, the rows its pages of 2 answer in turn; it answered \
             {whole:?} (cursor: {}), where the pages answered {turns:?}",
            next.is_some()
        );
    }
    let rows_of_three = Pattern {
        limit: Some(3),
        ..one("invoice", "billed_to", Walk::Out, Projection::default())
    };
    for (p, max) in [
        (paged(&Projection::Components, 9, None), 8),
        (rows_of_three, 2),
    ] {
        let what = format!("{:?} with `limit` {:?}", p.project, p.limit);
        let names = format!("between 1 and {max}");
        invalid_naming(&what, cat.query(&own, p).await, &[&names])?;
    }
    Ok(())
}
