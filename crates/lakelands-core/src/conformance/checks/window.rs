//! A read in a window, past the window's bounds
//! (`docs/decisions/097-a-read-in-a-window-walks-its-window.md`
//! §Past the bounds): an aggregate counts a window only to
//! `limits.window_keys` and a key page only to `FEW_HOLDERS`; past either,
//! the read takes the other side and answers the same.

use crate::conformance::fixture::source_record;
use crate::conformance::prelude::*;
use lakelands_api::read::Found;

const NARROW: &str = "v >= 100 and v <= 150";
const HALF: &str = "v <= 2000";
const ALL: &str = "v >= 0";

/// What the reads answer under one ceiling: the aggregate's groups, the
/// counts and two key pages, one of a window past `FEW_HOLDERS` and one
/// within it.
#[derive(Debug, PartialEq)]
struct Answers {
    narrow: BTreeMap<FieldName, Value>,
    half: BTreeMap<FieldName, Value>,
    all: BTreeMap<FieldName, Value>,
    count_narrow: u64,
    count_half: u64,
    count_all: u64,
    page_all: Vec<String>,
    page_window: Vec<String>,
}

/// The reads of `ty` in `store`, under the ceiling it was opened with.
async fn answers(store: &dyn LakeStore, ty: &TypeName) -> Result<Answers, Failure> {
    let l = Lake::of(store);
    let Lake { cat, own } = &l;
    Ok(Answers {
        narrow: group(cat.as_ref(), own, ty, sum(NARROW)).await?,
        half: group(cat.as_ref(), own, ty, sum(HALF)).await?,
        all: group(cat.as_ref(), own, ty, sum(ALL)).await?,
        count_narrow: total(cat.as_ref(), own, ty, NARROW).await?,
        count_half: total(cat.as_ref(), own, ty, HALF).await?,
        count_all: total(cat.as_ref(), own, ty, ALL).await?,
        page_all: page(cat.as_ref(), own, ty, ALL, 10).await?,
        page_window: page(cat.as_ref(), own, ty, "v >= 1000", 10).await?,
    })
}

check!(
    WINDOW_AGGREGATE_JUDGES_A_LINKS_ENDS =
        an_aggregate_over_a_window_judges_a_links_ends_as_a_page_does,
    "097 §As built, steps 3 and 4: a window's keys are answered by the engine, which holds no end, so an aggregate over a link type whose ends its reader does not hold reads the links as a page reads them, one record per key and the ends judged, and answers the same count and sum as a walk of every page; a count takes the page's road already",
    given
);

async fn an_aggregate_over_a_window_judges_a_links_ends_as_a_page_does(
    store: Store,
    _given: Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    let (cat, own) = (l.cat.clone(), l.own.clone());
    l.declare(type_decl("term", vec![field("code", StorageKind::Text)]))
        .await?;
    let mut deal = type_decl("deal", vec![field("n", StorageKind::Number)]);
    deal.relations = vec![RelationDecl {
        fields: vec![field("v", StorageKind::Number)],
        ..RelationDecl::to("with", "term")
    }];
    l.declare(deal).await?;
    for i in 0..5 {
        l.upsert("term", &format!("t{i}"), json!({"code": format!("c{i}")}))
            .await?;
    }
    for i in 0..10 {
        l.upsert("deal", &format!("d{i}"), json!({"n": i})).await?;
    }
    let link = link_type_name(&"deal".into(), &"with".into());
    for i in 0..10 {
        let (from, to) = (format!("d{i}"), format!("t{}", i % 5));
        let (rel, at, end) = ("with".into(), from.into(), to.into());
        let fields = patch(json!({"v": i}));
        cat.link(
            &own,
            &"deal".into(),
            &at,
            &rel,
            &end,
            fields,
            WriteOpts::default(),
        )
        .await
        .must("linking a deal to its term")?;
    }
    // One term is withheld: a page of the links refuses the links to it
    // as one scan over the links does.
    restrict(store.as_ref(), "term", "t1", "held here").await?;
    let setting = format!("sort_index.{}", link.as_str());
    l.set(&setting, json!(["v"])).await?;
    drop(l);
    let who = reads_all(store.as_ref(), "reader");
    let ty = TypeName::from(link.as_str());
    // `v` from 2 to 8 is seven links, one of them to the withheld term.
    let bound = "v >= 2 and v <= 8";
    let keys = page(cat.as_ref(), &who, &ty, bound, 100).await?;
    ensure!(
        keys.len() == 6,
        "a page over `{bound}` answered {} links {:?}; seven links lie in the bound and the \
         one to the withheld term must be left out, as a scan leaves it",
        keys.len(),
        keys
    );
    let totals = group(cat.as_ref(), &who, &ty, sum(bound)).await?;
    let counted = |name: &str| decimal(totals.get(&FieldName::from(name)));
    ensure!(
        counted("n") == Some(dec("6")) && counted("total") == Some(dec("29")),
        "an aggregate over `{bound}` answered {:?} links summed {:?}; the page answers 6 links \
         summed 29, and the aggregate must judge the ends as the page does",
        counted("n"),
        counted("total")
    );
    let counted_links = total(cat.as_ref(), &who, &ty, bound).await?;
    ensure!(
        counted_links == 6,
        "a count over `{bound}` answered {counted_links} links; the page answers 6"
    );
    Ok(())
}

check!(
    WINDOW_PAST_ITS_BOUNDS = a_read_past_its_window_ceiling_answers_as_one_inside_it,
    "097 §Past the bounds: an aggregate over a window wider than `limits.window_keys` takes the other side and answers what the same read answers under a ceiling wide enough to hold the window; a key page over a window of more than `FEW_HOLDERS` rows takes the type and answers the page, and one within the ceiling answers the page; a count has no ceiling and answers the same under either; each answer is the count, the sum and the ten keys a walk of every page gives",
    given
);

async fn a_read_past_its_window_ceiling_answers_as_one_inside_it(
    store: Store,
    given: Given,
) -> Held {
    const N: i64 = 4200;
    let l = Lake::of(store.as_ref());
    l.declare(type_decl(
        "row",
        vec![
            sourced("v", StorageKind::Number),
            sourced("s", StorageKind::Text),
        ],
    ))
    .await?;
    let records: Vec<SourceRecord> = (0..N)
        .map(|i| {
            let s = if i % 3 == 0 { "a" } else { "b" };
            source_record("row", &format!("k{i:04}"), json!({"v": i, "s": s}))
        })
        .collect();
    l.ingest("row", records).await.must("ingesting the rows")?;
    l.set("sort_index.row", json!(["v"])).await?;
    drop(l);
    let ty = TypeName::from("row");
    let narrow = "v >= 100 and v <= 150";
    let half = "v <= 2000";
    let all = "v >= 0";
    let wide = answers(store.as_ref(), &ty).await?;
    let counts = |g: &BTreeMap<FieldName, Value>| decimal(g.get("n"));
    let sums = |g: &BTreeMap<FieldName, Value>| decimal(g.get("total"));
    ensure!(
        counts(&wide.narrow) == Some(dec("51"))
            && counts(&wide.half) == Some(dec("2001"))
            && counts(&wide.all) == Some(dec("4200"))
            && sums(&wide.narrow) == Some(dec("6375"))
            && sums(&wide.half) == Some(dec("2001000"))
            && sums(&wide.all) == Some(dec("8817900")),
        "the aggregates at the default ceiling answered {:?} rows summed {:?} for `{narrow}`, \
         {:?} summed {:?} for `{half}` and {:?} summed {:?} for `{all}`; they must answer what \
         a walk of every page gives",
        counts(&wide.narrow),
        sums(&wide.narrow),
        counts(&wide.half),
        sums(&wide.half),
        counts(&wide.all),
        sums(&wide.all),
    );
    let first = |n: i64| -> Vec<String> { (0..n).map(|i| format!("k{i:04}")).collect() };
    ensure!(
        wide.count_narrow == 51
            && wide.count_half == 2001
            && wide.count_all == 4200
            && wide.page_all == first(10)
            && wide.page_window == (1000..1010).map(|i| format!("k{i:04}")).collect::<Vec<_>>(),
        "the counts answered {} for `{narrow}`, {} for `{half}` and {} for `{all}`, and the \
         pages {:?} and {:?}; a walk of every page gives 51, 2001, 4200 and the first ten \
         keys, k0000..=k0009 and k1000..=k1009",
        wide.count_narrow,
        wide.count_half,
        wide.count_all,
        wide.page_all,
        wide.page_window,
    );
    // A window of `limits.window_keys` four rows: every window past it, and
    // a key page still past `FEW_HOLDERS` for `all`, inside it for `v >= 1000`.
    let mut small = limits();
    small.window_keys = 4;
    let store = (given.relimit)(store, Seams::default(), small)
        .await
        .map_err(|e| Failure::new(format!("the harness opened no lake: {e}")))?;
    let tight = answers(store.as_ref(), &ty).await?;
    ensure!(
        tight == wide,
        "with `limits.window_keys` at 4, the aggregates, counts and pages answered {tight:?}; \
         under a ceiling wide enough for every window they answered {wide:?}, and a read past \
         its bounds must answer the same"
    );
    Ok(())
}

/// The one group `find` answers, its values.
async fn group(
    cat: &dyn Catalog,
    who: &Principal,
    ty: &TypeName,
    find: Find,
) -> Result<BTreeMap<FieldName, Value>, Failure> {
    let Found::Groups { mut rows, .. } = cat.find(who, ty, find).await.must("an aggregate")?.data
    else {
        return Err(Failure::new("an aggregate answered records, not groups"));
    };
    ensure!(
        rows.len() == 1,
        "an aggregate without a group key must answer one group; it answered {}",
        rows.len()
    );
    Ok(rows.remove(0).values)
}

/// The total `find` answers under `filter`.
async fn total(
    cat: &dyn Catalog,
    who: &Principal,
    ty: &TypeName,
    filter: &str,
) -> Result<u64, Failure> {
    let find = Find {
        filter: Some(filter.into()),
        total: true,
        ..Find::default()
    };
    match cat.find(who, ty, find).await.must("a count")?.data {
        Found::Records { total: Some(n), .. } => Ok(n),
        other => Err(Failure::new(format!(
            "a count answered no total: {other:?}"
        ))),
    }
}

/// The keys of a page of `limit` in key order under `filter`.
async fn page(
    cat: &dyn Catalog,
    who: &Principal,
    ty: &TypeName,
    filter: &str,
    limit: u32,
) -> Result<Vec<String>, Failure> {
    let find = Find {
        filter: Some(filter.into()),
        limit: Some(limit),
        ..Find::default()
    };
    match cat
        .find(who, ty, find)
        .await
        .must("a page in key order")?
        .data
    {
        Found::Records { rows, .. } => Ok(rows.iter().map(|r| r.key.to_string()).collect()),
        other => Err(Failure::new(format!("a page answered groups: {other:?}"))),
    }
}

/// A `find` of the rows' count and the sum of `v` under `filter`.
fn sum(filter: &str) -> Find {
    Find {
        filter: Some(filter.into()),
        ..aggregate(vec![
            agg("n", AggFunc::Count, None),
            agg("total", AggFunc::Sum, Some("v")),
        ])
    }
}
