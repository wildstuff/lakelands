//! A search says what each axis it ranked by brought (lakelands#560,
//! `docs/decisions/046-a-hybrid-search-fuses-the-ranks-a-read-may-see.md`,
//! extended).
//!
//! Every answer lists the axes that ranked it. Where a search fuses, each
//! entry counts the candidates its axis handed the fusion for this reader,
//! read to the lake's `max_limit` whatever the page: never the lake's
//! matches. A search of one axis reads no further than its page, and
//! counts nothing.

use std::sync::Arc;

use lakelands_api::location::Limits;
use lakelands_api::ontology::StorageKind;
use lakelands_api::principal::TypeGrants;
use lakelands_api::query::{Hits, Search};
use lakelands_api::seam::Seams;
use lakelands_api::{Access, LakeStore, Outcome, Principal};
use serde_json::{Value, json};

use super::fusion::docs;
use crate::conformance::fixture::{
    Lake, Must, alike_renamed, field, field_access, field_writer, is_invalid, limits,
    no_such_field, restrict, searchable, try_upsert, type_decl, vector,
};
use crate::conformance::{Failure, Given};

check!(
    /// A search lists its axes, counted where it fuses.
    AXES_SAID = a_search_lists_its_axes_and_counts_them_where_it_fuses,
    "046, extended (lakelands#560): every search answers `axes`, one entry for each axis that ranked in the order the fusion sums them, a vector axis naming its field; a hybrid search and a vector search with `also` count each axis's candidates, so a text that matches nothing answers 0 beside the vector's count; a search of one axis lists it without a count; a page continued by a cursor answers the first page's axes"
);

check!(
    /// An axis counts only what its reader is shown.
    AXES_READER = an_axis_counts_only_what_its_reader_is_shown,
    "046 and 024, extended (lakelands#560): an axis's count is of the candidates its reader may be answered, after the search's `where` and the rights that decide a hit, so a reader kept from a record the text matches by `restricted`, and one kept from it by its `where`, are each answered the count of what it is shown, and the two differ"
);

check!(
    /// A fused axis is counted to its window on every page.
    AXES_WINDOW = a_fused_axis_is_counted_to_its_window_on_every_page,
    "046, extended (lakelands#560): a fused list is read to the lake's `max_limit` of what its reader may see, or to its end, before a page is answered, so an axis's count is the same on every page, also where hidden candidates leave a list short of its window when the first page could already be ranked",
    given
);

check!(
    /// A type's setting weighs the rankings a search does not.
    WEIGHTS_SET = a_types_setting_weighs_what_a_search_does_not,
    "046, extended (lakelands#561): the setting `search_weights.<type>` weighs each ranking a fused search gives no weight, a vector field's by `fields` and the text's by `text_weight`, the search's own weight winning, and the axes name the weight that ranked; a read as of a position before the setting ranks without it; a field the setting names that a migration removed weighs nothing and is no error, and declared again it takes its weight again; the setting is refused as invalid for a field the type does not declare, one that is no vector, a weight a search could not give, a member it does not know, and an object or `fields` that names nothing; a reader not shown a weighted field is answered a search naming it as before the setting, and a writer of the setting not shown a field it names is refused as for a field the type does not declare (024)"
);

/// `doc`: `held`, `out` and `a` say "harbour", `b` and `c` do not; `held`
/// is restricted, `out` alone has `n` 1.
async fn harbours(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let rows = [
        ("held", "harbour", 0, 0),
        ("out", "harbour", 10, 1),
        ("a", "harbour light", 20, 0),
        ("b", "field", 30, 0),
        ("c", "road", 40, 0),
    ];
    let l = docs(store, rows.map(|(k, b, d, n)| (k.to_owned(), b, d, n))).await?;
    restrict(store, "doc", "held", "legal hold").await?;
    Ok(l)
}

/// `who`'s search of `doc`, given as JSON, and its axes as one line.
async fn axes(l: &Lake, who: &Principal, search: Value) -> Result<(String, Hits), Failure> {
    let mut search = search;
    search["type"] = json!("doc");
    let what = format!("the search {search}");
    let s: Search = serde_json::from_value(search)
        .map_err(|e| Failure::new(format!("the check's search does not read: {e}")))?;
    let hits = l.cat.search(who, s).await.must(&what)?.data;
    let line = (hits.axes.iter())
        .map(|a| {
            let field = a.field.as_ref().map_or(String::new(), |f| format!("({f})"));
            let n = a.candidates.map_or(String::new(), |n| format!(":{n}"));
            let w = a
                .weight
                .filter(|w| *w != 1.0)
                .map_or(String::new(), |w| format!("@{w}"));
            format!("{}{field}{n}{w}", a.axis)
        })
        .collect::<Vec<_>>()
        .join(" ");
    Ok((line, hits))
}

fn hybrid(text: &str) -> Value {
    json!({"mode": "hybrid", "text": text, "field": "v", "vector": [1.0, 0.0]})
}

/// Each line: who searches (`own` or `plain`, without `restricted`), the
/// search, and its axes.
const SAID: &str = r#"
    own | {"mode": "hybrid", "text": "harbour", "field": "v", "vector": [1, 0]} | lexical:3 vector(v):5
    own | {"mode": "hybrid", "text": "zzz", "field": "v", "vector": [1, 0]} | lexical:0 vector(v):5
    own | {"mode": "vector", "field": "v", "vector": [1, 0], "also": [{"field": "w", "vector": [1, 0]}]} | vector(v):5 vector(w):0
    own | {"mode": "lexical", "text": "harbour"} | lexical
    own | {"mode": "vector", "field": "v", "vector": [1, 0]} | vector(v)
"#;

const READERS: &str = r#"
    plain | {"mode": "hybrid", "text": "harbour", "field": "v", "vector": [1, 0]} | lexical:2 vector(v):4
    own | {"mode": "hybrid", "text": "harbour", "field": "v", "vector": [1, 0], "where": "n = 0"} | lexical:2 vector(v):4
    plain | {"mode": "hybrid", "text": "harbour", "field": "v", "vector": [1, 0], "where": "n = 1"} | lexical:1 vector(v):1
"#;

/// The lines of `table` whose search does not answer its axes.
async fn wrong(store: &dyn LakeStore, l: &Lake, table: &str) -> Result<Vec<String>, Failure> {
    let plain = field_writer(store, "plain", "doc", &[]);
    let mut wrong = Vec::new();
    for row in table.lines().map(str::trim).filter(|r| !r.is_empty()) {
        let [who, search, want] = row.split(" | ").collect::<Vec<_>>()[..] else {
            panic!("a row is three cells: {row}");
        };
        let search = serde_json::from_str(search).expect("a search reads");
        let (said, _) = axes(l, if who == "own" { &l.own } else { &plain }, search).await?;
        if said != want {
            wrong.push(format!("{row}: `{said}`"));
        }
    }
    Ok(wrong)
}

async fn a_search_lists_its_axes_and_counts_them_where_it_fuses(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = harbours(store.as_ref()).await?;
    let mut wrong = wrong(store.as_ref(), &l, SAID).await?;
    let mut first = hybrid("harbour");
    first["k"] = json!(2);
    let (said, hits) = axes(&l, &l.own, first.clone()).await?;
    first["cursor"] = json!(hits.next_cursor);
    let (next, _) = axes(&l, &l.own, first).await?;
    if next != said || hits.next_cursor.is_none() {
        wrong.push(format!("its next page: `{next}`, the first `{said}`"));
    }
    ensure!(wrong.is_empty(), "a search must say its axes: {wrong:?}");
    Ok(())
}

async fn an_axis_counts_only_what_its_reader_is_shown(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = harbours(store.as_ref()).await?;
    let wrong = wrong(store.as_ref(), &l, READERS).await?;
    ensure!(
        wrong.is_empty(),
        "an axis must count only what its reader is shown: {wrong:?}"
    );
    Ok(())
}

async fn a_fused_axis_is_counted_to_its_window_on_every_page(
    store: Arc<dyn LakeStore>,
    given: Given,
) -> Result<(), Failure> {
    // Nine say "harbour"; the six restricted sort first by key, the three
    // others lie along the query. Under `max_limit` 4 a page of one ranks
    // its first eight candidates, which hold two of the three a reader
    // without `restricted` is shown, and it could stop there.
    let keys = ["r1", "r2", "r3", "r4", "r5", "r6", "v1", "v2", "v3"];
    let rows = keys.map(|k| (k.to_owned(), "harbour", if k < "v" { 90 } else { 0 }, 0));
    docs(store.as_ref(), rows).await?;
    for k in &keys[..6] {
        restrict(store.as_ref(), "doc", *k, "legal hold").await?;
    }
    let four = Limits {
        max_limit: 4,
        ..limits()
    };
    let store = (given.relimit)(store, Seams::default(), four).await;
    let store = store.map_err(|e| Failure::new(format!("reopening at max_limit 4: {e}")))?;
    let (l, plain) = (
        Lake::of(store.as_ref()),
        field_writer(store.as_ref(), "plain", "doc", &[]),
    );
    let mut page = hybrid("harbour");
    page["k"] = json!(1);
    let mut said = Vec::new();
    for _ in 0..3 {
        let (axes, hits) = axes(&l, &plain, page.clone()).await?;
        said.push(axes);
        let Some(cursor) = hits.next_cursor else {
            break;
        };
        page["cursor"] = json!(cursor);
    }
    ensure!(
        said.len() == 3 && said.iter().all(|s| s == "lexical:3 vector(v):3"),
        "each page of one must count both axes to their window, `lexical:3 vector(v):3`; \
         the pages said {said:?}"
    );
    Ok(())
}

/// Each line: a search, and its axes under the setting
/// `{"fields": {"v": 0.5, "w": 2}, "text_weight": 0.25}`.
const WEIGHED: &str = r#"
    own | {"mode": "hybrid", "text": "harbour", "field": "v", "vector": [1, 0]} | lexical:3@0.25 vector(v):5@0.5
    own | {"mode": "hybrid", "text": "harbour", "field": "v", "vector": [1, 0], "weight": 3} | lexical:3@0.25 vector(v):5@3
    own | {"mode": "vector", "field": "v", "vector": [1, 0], "also": [{"field": "w", "vector": [1, 0]}]} | vector(v):5@0.5 vector(w):0@2
    own | {"mode": "vector", "field": "v", "vector": [1, 0]} | vector(v)
"#;

/// `who`'s search along `v` and, in `also`, `w`, as it is answered: a
/// refusal by its wire form, its sentence with it.
async fn naming_w(l: &Lake, who: &Principal) -> String {
    let also = json!([{"field": "w", "vector": [1, 0]}]);
    let s = json!({"type": "doc", "mode": "vector", "field": "v", "vector": [1, 0], "also": also});
    let answer = l
        .cat
        .search(who, serde_json::from_value(s).expect("a search reads"));
    match answer.await {
        Err(e) if no_such_field(&e, "w") => json!(e).to_string(),
        other => format!("{other:?}"),
    }
}

/// `who`'s write of the setting `search_weights.doc` holding `value`.
async fn weigh(l: &Lake, who: &Principal, value: &Value) -> lakelands_api::Result<Outcome> {
    let set = json!({"key": "search_weights.doc", "value": value});
    try_upsert(
        l.cat.as_ref(),
        who,
        "sys.setting",
        "search_weights.doc",
        set,
    )
    .await
}

async fn a_types_setting_weighs_what_a_search_does_not(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = harbours(store.as_ref()).await?;
    // A reader, and a writer of settings, neither shown `w`.
    let hide = [("w", Access::None)];
    let blind = field_access(store.as_ref(), "blind", "doc", &hide);
    let mut tuner = field_access(store.as_ref(), "tuner", "doc", &hide);
    let setting = TypeGrants {
        default: Some(Access::Write),
        ..Default::default()
    };
    (tuner.grants.types).insert("sys.setting".into(), setting);
    let unweighed = naming_w(&l, &blind).await;
    let before = store.head().await.must("reading the head")?;
    let weights = json!({"fields": {"v": 0.5, "w": 2}, "text_weight": 0.25});
    l.set("search_weights.doc", weights).await?;
    let mut wrong = wrong(store.as_ref(), &l, WEIGHED).await?;
    // The setting tells the reader nothing of `w`, in an axis or a refusal.
    let weighed = naming_w(&l, &blind).await;
    if weighed != unweighed {
        wrong.push(format!(
            "not shown `w`: `{weighed}`, before the setting `{unweighed}`"
        ));
    }
    let hidden = weigh(&l, &tuner, &json!({"fields": {"w": 1}})).await;
    // The setting's weights rank as the search's own: the same bits.
    let mut given = hybrid("harbour");
    (given["weight"], given["text_weight"]) = (json!(0.5), json!(0.25));
    let bits = |h: &Hits| -> Vec<(String, u64)> {
        (h.hits.iter())
            .map(|h| (h.record.key.0.clone(), h.score.to_bits()))
            .collect()
    };
    let (_, set) = axes(&l, &l.own, hybrid("harbour")).await?;
    let (_, own) = axes(&l, &l.own, given).await?;
    if bits(&set) != bits(&own) {
        wrong.push("the setting's weights rank otherwise than the same weights given".into());
    }
    let mut then = hybrid("harbour");
    then["as_of"] = json!(before);
    if axes(&l, &l.own, then).await?.0 != "lexical:3 vector(v):5" {
        wrong.push("a read before the setting ranks by it".into());
    }
    // `w` removed: the setting still names it, and nothing fails for it.
    let text = searchable("body");
    let n = field("n", StorageKind::Number);
    l.migrate(type_decl("doc", vec![text, vector("v", 2), n]))
        .await?;
    if axes(&l, &l.own, hybrid("harbour")).await?.0 != "lexical:3@0.25 vector(v):5@0.5" {
        wrong.push("a search after `w` was removed".into());
    }
    // To a writer not shown it, `w` was a field `doc` does not declare.
    let undeclared = weigh(&l, &l.own, &json!({"fields": {"w": 1}})).await;
    match (&hidden, &undeclared) {
        (Err(h), Err(u)) => {
            if let Err(f) = alike_renamed("not shown `w`, its setting", h, u, ("", "")) {
                wrong.push(f.message);
            }
        }
        _ => wrong.push(format!(
            "not shown `w`, its setting: {hidden:?}; undeclared: {undeclared:?}"
        )),
    }
    for value in [
        json!({"fields": {"w": 1}}),
        json!({"fields": {"body": 1}}),
        json!({"fields": {"v": 0}}),
        json!({"text_weight": 2_000_000}),
        json!({"weights": {"v": 1}}),
        json!({}),
        json!({"fields": {}}),
    ] {
        if !is_invalid(&weigh(&l, &l.own, &value).await) {
            wrong.push(format!("the setting {value} was not refused as invalid"));
        }
    }
    // Declared again, `w` takes the setting's weight again.
    let (text, n) = (searchable("body"), field("n", StorageKind::Number));
    l.migrate(type_decl(
        "doc",
        vec![text, vector("v", 2), vector("w", 2), n],
    ))
    .await?;
    let also = json!({"mode": "vector", "field": "v", "vector": [1, 0], "also": [{"field": "w", "vector": [1, 0]}]});
    if axes(&l, &l.own, also).await?.0 != "vector(v):5@0.5 vector(w):0@2" {
        wrong.push("a search after `w` was declared again".into());
    }
    ensure!(
        wrong.is_empty(),
        "a type's setting must weigh its searches: {wrong:?}"
    );
    Ok(())
}
