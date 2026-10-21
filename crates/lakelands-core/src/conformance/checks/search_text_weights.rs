//! A type weighs a match of its text by the field it is in (lakelands#1230,
//! `docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`,
//! extended).
//!
//! The setting `search_weights.<type>` takes `text_fields`: each distinct
//! query token counts at the weight of the heaviest searched field that
//! holds it, 1 for a field it does not name, so with none the score is
//! the count of the tokens held. It weighs every ranking of text, a
//! lexical search's and a hybrid search's lexical half before the fusion.
//!
//! A lexical or hybrid search may give its own `text_fields`, each
//! field's weight winning over the setting's (lakelands#1237), judged as
//! the setting's are, at the search.

use lakelands_api::principal::TypeGrants;
use lakelands_api::query::{Hits, Projection, QueryResult, Search};
use lakelands_api::{Access, Principal, Refusal};
use serde_json::{Value, json};

use crate::conformance::fixture::{
    Lake, Must, alike_renamed, embedding, field, field_access, get_at, invalid_like, pattern,
    reason, refused_on, searchable, try_upsert, type_decl, vector,
};
use crate::conformance::{Failure, Given, Held, Reopen, Store, backing};
use lakelands_api::ontology::{StorageKind, TypeDecl};

check!(
    WEIGHTS_TEXT = a_types_text_fields_weigh_each_token_at_its_heaviest_field,
    "025, extended (lakelands#1230): under `search_weights.<type>`'s `text_fields`, a lexical search scores each distinct query token at the weight of the heaviest searched field holding it, a field not named at 1, summed: a word in the title at 4 outranks it in a body, and a record holding it in both scores 4, not 5; a search limited to `fields` reads no weight of a field it does not name; a read as of a position before the setting ranks without it, and a field the setting names that a migration removed weighs nothing and is no error; a field renamed since a read's position is weighed there by the name it bore there, and at the head, where the setting names it no more, at 1"
);

check!(
    WEIGHTS_TEXT_UNWEIGHTED = without_text_fields_a_lexical_score_is_the_tokens_count,
    "025, extended (lakelands#1230): with no `text_fields`, no setting or one of other members, a lexical score is the count of the distinct query tokens a record holds, to the bit, a token held in two fields counted once"
);

check!(
    WEIGHTS_TEXT_FUSED = a_hybrid_search_fuses_its_weighed_lexical_ranking,
    "025, extended (lakelands#1230): a hybrid search's lexical half is ranked by the weighed score before the fusion, so its ranks, and the fused scores' bits, are those of the weighed order; its fallback to the text alone, over an embedding with no backend, answers the weighed scores; the axes are as without the weights",
    reopens
);

check!(
    WEIGHTS_TEXT_HIDDEN = a_text_weight_of_a_field_not_shown_is_answered_as_never_declared,
    "025 and 024, extended (lakelands#1230): to a reader not shown a weighed field, or shown it masked, a lexical search and a hybrid one, page by page, and the setting, are answered as in a twin lake that never declared the field; the setting is told to neither, and a writer of the setting not shown it is refused it as not granted, as in the twin, whatever it names (063)",
    given
);

check!(
    WEIGHTS_TEXT_REFUSED = a_text_fields_weight_is_refused_for_what_a_search_could_not_weigh,
    "025, extended (lakelands#1230): `text_fields` is refused as invalid naming the field: `unknown_field` for a field the type does not declare, `not_searchable` for a text, number or vector field that is not searchable, `weight_range` for a weight above 0 and at most 1 000 000 it is not, and `setting_weights_shape` for an empty object; a text field in `fields` is still `not_vector_field`"
);

check!(
    WEIGHTS_TEXT_OWN = a_searchs_own_text_fields_win_over_its_types,
    "025, extended (lakelands#1237): a lexical or hybrid search's own `text_fields` weighs each field it names, over `search_weights.<type>`'s weight for it; a field it does not name takes the setting's, else 1, and a read as of before the setting takes its own and 1; a weight for a field its `fields` leaves out weighs nothing; a hybrid search fuses the lexical ranks so weighed; its pages are of the weighed ranking, a cursor continues no search of other weights, and a pattern that starts from the search walks that ranking"
);

check!(
    WEIGHTS_TEXT_OWN_HIDDEN = a_searchs_weight_of_a_field_not_shown_is_answered_as_never_declared,
    "025 and 024, extended (lakelands#1237): to a reader not shown a field the setting weighs, or shown it masked, a search weighing the fields it is shown is answered, page by page, as in a twin lake that never declared the field; a search weighing a field it is not shown is refused as in the twin, and one weighing a field shown masked is not granted naming it, each before its weight is judged",
    given
);

check!(
    WEIGHTS_TEXT_OWN_REFUSED = a_searchs_text_fields_are_refused_as_the_settings_are,
    "025, extended (lakelands#1237): a search's own `text_fields` is refused as invalid naming the field: `unknown_field` for a field the type does not declare, `not_searchable` for one that is not searchable, also where `fields` leaves it out, and `weight_range` for a weight above 0 and at most 1 000 000 it is not, each field judged whole in name order, so the first faulty field is the one named; an empty object is refused as `search_text_fields_empty`, as an empty `fields` is"
);

/// `memo`: `title`, `heading` and `body` searchable, `note` and `n` not,
/// `v` a vector; `title` left out where `titled` is not.
fn memo(titled: bool) -> TypeDecl {
    let mut fields = vec![searchable("heading"), searchable("body")];
    if titled {
        fields.insert(0, searchable("title"));
    }
    let (note, n) = (
        field("note", StorageKind::Text),
        field("n", StorageKind::Number),
    );
    fields.extend([note, n, vector("v", 2)]);
    type_decl("memo", fields)
}

/// The issue's two memos, `c` naming the invoice twice, `d` the rent in
/// its heading; `title` written where it is declared.
async fn memos(store: &dyn lakelands_api::LakeStore, titled: bool) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    l.declare(memo(titled)).await?;
    let rows = [
        (
            "a",
            "rent",
            "",
            "the invoice for the shipping of the goods",
            [1.0, 0.0],
        ),
        (
            "b",
            "invoice",
            "",
            "the rent for the office in the city centre",
            [0.0, 1.0],
        ),
        ("c", "invoice", "", "an invoice copy", [0.6, 0.8]),
        ("d", "", "rent", "the invoice", [0.8, 0.6]),
    ];
    for (key, title, heading, body, v) in rows {
        let mut fields = json!({"body": body, "note": "invoice rent", "v": v});
        if titled && !title.is_empty() {
            fields["title"] = json!(title);
        }
        if !heading.is_empty() {
            fields["heading"] = json!(heading);
        }
        l.upsert("memo", key, fields).await?;
    }
    Ok(l)
}

/// `who`'s search of `memo`, given as JSON.
async fn search(l: &Lake, who: &Principal, mut search: Value) -> Result<Hits, Failure> {
    search["type"] = json!("memo");
    let what = format!("the search {search}");
    let s: Search = serde_json::from_value(search).expect("the check's search reads");
    Ok(l.cat.search(who, s).await.must(&what)?.data)
}

/// The keys and the scores' bits a search answers.
fn scored(h: &Hits) -> Vec<(String, u64)> {
    (h.hits.iter())
        .map(|h| (h.record.key.0.clone(), h.score.to_bits()))
        .collect()
}

/// `scored` as written: keys and scores.
fn want(hits: &[(&str, f64)]) -> Vec<(String, u64)> {
    hits.iter()
        .map(|(k, s)| ((*k).to_owned(), s.to_bits()))
        .collect()
}

fn lexical(text: &str) -> Value {
    json!({"mode": "lexical", "text": text})
}

/// Each search the lake `l` answers its owner otherwise than `table` says.
async fn misranked(l: &Lake, table: &[(Value, &[(&str, f64)])]) -> Result<Vec<String>, Failure> {
    let mut wrong = Vec::new();
    for (s, hits) in table {
        let said = scored(&search(l, &l.own, s.clone()).await?);
        if said != want(hits) {
            wrong.push(format!("{s}: {said:?}"));
        }
    }
    Ok(wrong)
}

async fn a_types_text_fields_weigh_each_token_at_its_heaviest_field(store: Store) -> Held {
    let l = memos(store.as_ref(), true).await?;
    let before = store.head().await.must("reading the head")?;
    l.set("search_weights.memo", json!({"text_fields": {"title": 4}}))
        .await?;
    let mut body = lexical("invoice");
    body["fields"] = json!(["body"]);
    let mut then = lexical("invoice");
    then["as_of"] = json!(before);
    let mut wrong = misranked(
        &l,
        &[
            (
                lexical("invoice"),
                &[("b", 4.0), ("c", 4.0), ("a", 1.0), ("d", 1.0)],
            ),
            (body, &[("a", 1.0), ("c", 1.0), ("d", 1.0)]),
            (then, &[("a", 1.0), ("b", 1.0), ("c", 1.0), ("d", 1.0)]),
        ],
    )
    .await?;
    let three = json!({"text_fields": {"title": 4, "heading": 2, "body": 0.5}});
    l.set("search_weights.memo", three).await?;
    let both = [("a", 4.5), ("b", 4.5), ("c", 4.0), ("d", 2.5)];
    wrong.extend(misranked(&l, &[(lexical("invoice rent"), &both)]).await?);
    // `heading` removed: the setting still names it, and weighs nothing.
    let mut gone = memo(true);
    gone.fields.retain(|f| f.name.as_str() != "heading");
    l.migrate(gone).await?;
    let left = [("a", 4.5), ("b", 4.5), ("c", 4.0), ("d", 0.5)];
    wrong.extend(misranked(&l, &[(lexical("invoice rent"), &left)]).await?);
    // `title` renamed `headline`: as of before, the setting names it.
    let named = store.head().await.must("reading the head")?;
    let mut renamed = memo(true);
    renamed.fields.retain(|f| f.name.as_str() != "heading");
    renamed.fields[0].name = "headline".into();
    l.migrate_renaming(renamed, &[("title", "headline")])
        .await?;
    let mut then = lexical("invoice rent");
    then["as_of"] = json!(named);
    let head = [("a", 1.5), ("b", 1.5), ("c", 1.0), ("d", 0.5)];
    wrong.extend(misranked(&l, &[(then, &left), (lexical("invoice rent"), &head)]).await?);
    ensure!(
        wrong.is_empty(),
        "a type's `text_fields` must weigh each token at its heaviest field: {wrong:?}"
    );
    Ok(())
}

async fn without_text_fields_a_lexical_score_is_the_tokens_count(store: Store) -> Held {
    let l = memos(store.as_ref(), true).await?;
    let table: [(Value, &[(&str, f64)]); 2] = [
        (
            lexical("invoice"),
            &[("a", 1.0), ("b", 1.0), ("c", 1.0), ("d", 1.0)],
        ),
        (
            lexical("invoice rent"),
            &[("a", 2.0), ("b", 2.0), ("d", 2.0), ("c", 1.0)],
        ),
    ];
    let mut wrong = misranked(&l, &table).await?;
    l.set(
        "search_weights.memo",
        json!({"text_weight": 0.5, "fields": {"v": 2}}),
    )
    .await?;
    wrong.extend(misranked(&l, &table).await?);
    ensure!(
        wrong.is_empty(),
        "without `text_fields` a lexical score must be the count of the tokens held: {wrong:?}"
    );
    Ok(())
}

async fn a_hybrid_search_fuses_its_weighed_lexical_ranking(store: Store, reopen: Reopen) -> Held {
    let l = memos(store.as_ref(), true).await?;
    let hybrid = json!({"mode": "hybrid", "text": "invoice", "field": "v", "vector": [1.0, 0.0]});
    let axes = |h: &Hits| format!("{:?}", h.axes);
    let unweighed = search(&l, &l.own, hybrid.clone()).await?;
    l.set("search_weights.memo", json!({"text_fields": {"title": 4}}))
        .await?;
    let weighed = search(&l, &l.own, hybrid).await?;
    // Near (1, 0): `a`, `d`, `c`, `b`. Weighed, `invoice` ranks `b` and
    // `c` (title) above `a` and `d` (body): ranks 2, 0, 1, 3.
    let fused = |text: f64, near: f64| 1.0 / (60.0 + text) + 1.0 / (60.0 + near);
    let ranks = [
        ("a", 2.0, 0.0),
        ("b", 0.0, 3.0),
        ("c", 1.0, 2.0),
        ("d", 3.0, 1.0),
    ];
    let mut expected: Vec<(&str, f64)> = ranks.map(|(k, t, n)| (k, fused(t, n))).to_vec();
    expected.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(y.0)));
    let mut wrong = Vec::new();
    if scored(&weighed) != want(&expected) {
        wrong.push(format!("fused: {:?}, not {expected:?}", scored(&weighed)));
    }
    if axes(&weighed) != axes(&unweighed) {
        wrong.push(format!(
            "axes: {} against {}",
            axes(&weighed),
            axes(&unweighed)
        ));
    }
    // Without a vector, over `e`, the embedding of `note`, declared where
    // the suite's backend is bound and searched where none is.
    use super::embeddings::{reopen_failed, with_embedder};
    let store = reopen(store, with_embedder(Some(backing::embedder())))
        .await
        .map_err(|e| reopen_failed("with the suite's embedding backend", e))?;
    let mut e = memo(true);
    e.fields
        .push(embedding("e", 2, &["note"], backing::EMBED_MODEL));
    Lake::of(store.as_ref()).migrate(e).await?;
    let store = reopen(store, with_embedder(None))
        .await
        .map_err(|e| reopen_failed("with no embedding backend", e))?;
    let l = Lake::of(store.as_ref());
    let fallback = json!({"mode": "hybrid", "text": "invoice", "field": "e"});
    let alone = search(&l, &l.own, fallback).await?;
    let text = [("b", 4.0), ("c", 4.0), ("a", 1.0), ("d", 1.0)];
    if scored(&alone) != want(&text) || !alone.ranked_by.starts_with("lexical") {
        wrong.push(format!(
            "its fallback: {} {:?}",
            alone.ranked_by,
            scored(&alone)
        ));
    }
    ensure!(
        wrong.is_empty(),
        "a hybrid search must fuse its weighed lexical ranking: {wrong:?}"
    );
    Ok(())
}

/// A principal of `memo` given `title` as `title`, writing settings
/// where `settings`.
fn blind(
    store: &dyn lakelands_api::LakeStore,
    id: &str,
    title: Access,
    settings: bool,
) -> Principal {
    let mut p = field_access(store, id, "memo", &[("title", title)]);
    if settings {
        let write = TypeGrants {
            default: Some(Access::Write),
            ..Default::default()
        };
        (p.grants.types).insert("sys.setting".into(), write);
    }
    p
}

/// What `who` is answered by `l`: the pages of each search, by keys and
/// scores' bits, their axes and what ranked them, and the setting; each
/// search weighs its text by `own` where it is not null.
async fn told(l: &Lake, who: &Principal, own: &Value) -> Result<Vec<String>, Failure> {
    let mut told = Vec::new();
    let hybrid = json!({"mode": "hybrid", "text": "invoice rent", "field": "v", "vector": [1, 0]});
    for mut s in [lexical("invoice rent"), hybrid] {
        s["k"] = json!(1);
        if !own.is_null() {
            s["text_fields"] = own.clone();
        }
        loop {
            let page = search(l, who, s.clone()).await?;
            told.push(format!(
                "{:?} {:?} {}",
                scored(&page),
                page.axes,
                page.ranked_by
            ));
            let Some(next) = page.next_cursor else { break };
            s["cursor"] = json!(next);
        }
    }
    let key = get_at("search_weights.memo", None);
    let setting = l.cat.get(who, &"sys.setting".into(), key).await;
    told.push(format!("{:?}", setting.map(|r| r.data.fields)));
    Ok(told)
}

async fn a_text_weight_of_a_field_not_shown_is_answered_as_never_declared(
    store: Store,
    given: Given,
) -> Held {
    let l = memos(store.as_ref(), true).await?;
    let twin = (given.twin)(store.lake().clone(), Default::default()).await;
    let twin = twin.map_err(|e| Failure::new(format!("the harness opened no twin: {e}")))?;
    let t = memos(twin.as_ref(), false).await?;
    l.set(
        "search_weights.memo",
        json!({"text_fields": {"title": 4, "heading": 2}}),
    )
    .await?;
    t.set(
        "search_weights.memo",
        json!({"text_fields": {"heading": 2}}),
    )
    .await?;
    let writer = blind(store.as_ref(), "tuner", Access::None, true);
    let mut wrong = Vec::new();
    for title in [Access::None, Access::Masked] {
        let reader = blind(store.as_ref(), "reader", title, false);
        let (here, there) = (
            told(&l, &reader, &Value::Null).await?,
            told(&t, &reader, &Value::Null).await?,
        );
        if here != there {
            wrong.push(format!(
                "`title` {title:?}: {here:?}; never declared: {there:?}"
            ));
        }
        let get = async |l: &Lake| {
            let key = get_at("search_weights.memo", None);
            let got = l.cat.get(&reader, &"sys.setting".into(), key).await;
            matches!(got, Err(lakelands_api::Error::NotFound { .. }))
        };
        if !(get(&l).await && get(&t).await) {
            wrong.push(format!("`title` {title:?}: the setting is told"));
        }
    }
    let weigh = async |l: &Lake, value: Value| {
        let set = json!({"key": "search_weights.memo", "value": value});
        try_upsert(
            l.cat.as_ref(),
            &writer,
            "sys.setting",
            "search_weights.memo",
            set,
        )
        .await
    };
    // Not reading `memo` whole, it is refused the setting in both lakes.
    for value in [
        json!({"text_fields": {"title": 0}}),
        json!({"text_fields": {"heading": 3}}),
    ] {
        let (here, there) = (
            weigh(&l, value.clone()).await,
            weigh(&t, value.clone()).await,
        );
        if reason(&here) != Some(&Refusal::NotGranted)
            || format!("{here:?}") != format!("{there:?}")
        {
            wrong.push(format!(
                "not shown `title`, the setting {value}: {here:?}; never declared: {there:?}"
            ));
        }
    }
    ensure!(
        wrong.is_empty(),
        "a weight of a field not shown must be answered as never declared: {wrong:?}"
    );
    Ok(())
}

async fn a_text_fields_weight_is_refused_for_what_a_search_could_not_weigh(store: Store) -> Held {
    let l = memos(store.as_ref(), true).await?;
    let mut wrong = Vec::new();
    for (value, named, code) in [
        (
            json!({"text_fields": {"subject": 1}}),
            "subject",
            "unknown_field",
        ),
        (
            json!({"text_fields": {"note": 1}}),
            "note",
            "not_searchable",
        ),
        (json!({"text_fields": {"v": 1}}), "v", "not_searchable"),
        (json!({"text_fields": {"n": 1}}), "n", "not_searchable"),
        (json!({"text_fields": {"body": 0}}), "body", "weight_range"),
        (
            json!({"text_fields": {"body": 1_000_001}}),
            "body",
            "weight_range",
        ),
        (json!({"text_fields": {}}), "value", "setting_weights_shape"),
        (json!({"fields": {"body": 1}}), "body", "not_vector_field"),
    ] {
        let set = json!({"key": "search_weights.memo", "value": value});
        let key = "search_weights.memo";
        let answer = try_upsert(l.cat.as_ref(), &l.own, "sys.setting", key, set).await;
        let on = refused_on(&answer).map(|(f, _)| f);
        if on != Some(named) || !invalid_like(&answer, &json!({"code": code})) {
            wrong.push(format!("{value}: {answer:?}"));
        }
    }
    ensure!(
        wrong.is_empty(),
        "`text_fields` must be refused for what a search could not weigh: {wrong:?}"
    );
    Ok(())
}

async fn a_searchs_own_text_fields_win_over_its_types(store: Store) -> Held {
    let l = memos(store.as_ref(), true).await?;
    let before = store.head().await.must("reading the head")?;
    let set = json!({"text_fields": {"title": 4, "heading": 2}});
    l.set("search_weights.memo", set).await?;
    let own = |text: &str, weights: Value| {
        let mut s = lexical(text);
        s["text_fields"] = weights;
        s
    };
    // `title` at 0.25, its own; `heading` at 2, the setting's; `body` at 1.
    let quarter = || own("invoice rent", json!({"title": 0.25}));
    let mut then = quarter();
    then["as_of"] = json!(before);
    let mut body = own("invoice rent", json!({"title": 4, "body": 2}));
    body["fields"] = json!(["body"]);
    let mut wrong = misranked(
        &l,
        &[
            (
                quarter(),
                &[("d", 3.0), ("a", 1.25), ("b", 1.25), ("c", 1.0)],
            ),
            (then, &[("d", 2.0), ("a", 1.25), ("b", 1.25), ("c", 1.0)]),
            (body, &[("a", 2.0), ("b", 2.0), ("c", 2.0), ("d", 2.0)]),
        ],
    )
    .await?;
    // Weighed, `invoice` ranks a, c, d, b; near (1, 0) is a, d, c, b.
    let hybrid = json!({"mode": "hybrid", "text": "invoice", "field": "v",
                        "vector": [1.0, 0.0], "text_fields": {"title": 0.25}});
    let fused = |text: f64, near: f64| 1.0 / (60.0 + text) + 1.0 / (60.0 + near);
    let ranks = [
        ("a", 0.0, 0.0),
        ("b", 3.0, 3.0),
        ("c", 1.0, 2.0),
        ("d", 2.0, 1.0),
    ];
    let mut expected: Vec<(&str, f64)> = ranks.map(|(k, t, n)| (k, fused(t, n))).to_vec();
    expected.sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(y.0)));
    wrong.extend(misranked(&l, &[(hybrid, &expected)]).await?);
    // Two pages of the weighed ranking; its cursor continues no other.
    let mut paged = quarter();
    paged["k"] = json!(2);
    let first = search(&l, &l.own, paged.clone()).await?;
    let Some(next) = first.next_cursor.clone() else {
        return Err(Failure::new("a page of two of four hits has no cursor"));
    };
    paged["cursor"] = json!(next);
    let second = search(&l, &l.own, paged).await?;
    let pages = [scored(&first), scored(&second)].concat();
    let whole = [("d", 3.0), ("a", 1.25), ("b", 1.25), ("c", 1.0)];
    if pages != want(&whole) {
        wrong.push(format!("its pages: {pages:?}"));
    }
    let mut other = own("invoice rent", json!({"title": 0.5}));
    (other["type"], other["k"], other["cursor"]) = (json!("memo"), json!(2), json!(next));
    let s: Search = serde_json::from_value(other).expect("the check's search reads");
    let moved = l.cat.search(&l.own, s).await;
    if !invalid_like(&moved, &json!({"code": "cursor_other_read"})) {
        wrong.push(format!("its cursor under other weights: {moved:?}"));
    }
    // A pattern that starts from the search walks the weighed ranking.
    let mut p = pattern("memo", None, vec![], Projection::default());
    let start = serde_json::from_value(quarter()).expect("the check's mode reads");
    p.start.search = Some(start);
    let rows = l.cat.query(&l.own, p).await.must("a weighed start")?.data;
    let keys = match &rows {
        QueryResult::Rows { rows, .. } => (rows.iter())
            .map(|r| r["memo.key"].as_str().unwrap_or("?"))
            .collect(),
        _ => vec!["not rows"],
    };
    if keys != ["d", "a", "b", "c"] {
        wrong.push(format!("a pattern started from it: {rows:?}"));
    }
    ensure!(
        wrong.is_empty(),
        "a search's own `text_fields` must win over its type's: {wrong:?}"
    );
    Ok(())
}

async fn a_searchs_weight_of_a_field_not_shown_is_answered_as_never_declared(
    store: Store,
    given: Given,
) -> Held {
    let l = memos(store.as_ref(), true).await?;
    let twin = (given.twin)(store.lake().clone(), Default::default()).await;
    let twin = twin.map_err(|e| Failure::new(format!("the harness opened no twin: {e}")))?;
    let t = memos(twin.as_ref(), false).await?;
    let set = json!({"text_fields": {"title": 4, "heading": 2}});
    l.set("search_weights.memo", set).await?;
    let set = json!({"text_fields": {"heading": 2}});
    t.set("search_weights.memo", set).await?;
    let mut wrong = Vec::new();
    let own = json!({"heading": 0.5, "body": 3});
    for title in [Access::None, Access::Masked] {
        let reader = blind(store.as_ref(), "reader", title, false);
        let (here, there) = (
            told(&l, &reader, &own).await?,
            told(&t, &reader, &own).await?,
        );
        if here != there {
            wrong.push(format!(
                "`title` {title:?}: {here:?}; never declared: {there:?}"
            ));
        }
    }
    let reader = blind(store.as_ref(), "reader", Access::None, false);
    let mut zero = lexical("invoice");
    (zero["type"], zero["text_fields"]) = (json!("memo"), json!({"title": 0}));
    let zero: Search = serde_json::from_value(zero).expect("the check's search reads");
    let (h, u) = (
        l.cat.search(&reader, zero.clone()).await,
        t.cat.search(&reader, zero).await,
    );
    match (h, u) {
        (Err(h), Err(u)) => {
            if let Err(f) = alike_renamed("not shown `title`, its weight", &h, &u, ("", "")) {
                wrong.push(f.message);
            }
            let unknown = json!({"code": "unknown_field", "field": "title"});
            if !invalid_like(&h, &unknown) {
                wrong.push(format!("not shown `title`, a weight of 0: {h:?}"));
            }
        }
        other => wrong.push(format!("not shown `title`, a weight of 0: {other:?}")),
    }
    // A field shown masked is known but not read: a weight of it, in range
    // or not, is not granted.
    let reader = blind(store.as_ref(), "reader", Access::Masked, false);
    for w in [1, 0] {
        let mut s = lexical("invoice");
        (s["type"], s["text_fields"]) = (json!("memo"), json!({"title": w}));
        let s: Search = serde_json::from_value(s).expect("the check's search reads");
        let masked = l.cat.search(&reader, s).await;
        if !matches!(refused_on(&masked), Some(("title", Refusal::NotGranted))) {
            wrong.push(format!("masked `title`, a weight of {w}: {masked:?}"));
        }
    }
    ensure!(
        wrong.is_empty(),
        "a search's weight of a field not shown must be answered as never declared: {wrong:?}"
    );
    Ok(())
}

async fn a_searchs_text_fields_are_refused_as_the_settings_are(store: Store) -> Held {
    let l = memos(store.as_ref(), true).await?;
    let hybrid = json!({"mode": "hybrid", "text": "invoice", "field": "v", "vector": [1.0, 0.0]});
    let mut body = lexical("invoice");
    body["fields"] = json!(["body"]);
    let mut wrong = Vec::new();
    for (mode, weights, named, code) in [
        (
            lexical("invoice"),
            json!({"subject": 1}),
            Some("subject"),
            "unknown_field",
        ),
        (
            lexical("invoice"),
            json!({"note": 1}),
            Some("note"),
            "not_searchable",
        ),
        (
            lexical("invoice"),
            json!({"v": 1}),
            Some("v"),
            "not_searchable",
        ),
        (hybrid.clone(), json!({"n": 1}), Some("n"), "not_searchable"),
        (body, json!({"note": 1}), Some("note"), "not_searchable"),
        (
            lexical("invoice"),
            json!({"body": 0}),
            Some("body"),
            "weight_range",
        ),
        (
            lexical("invoice"),
            json!({"subject": 0}),
            Some("subject"),
            "unknown_field",
        ),
        (
            lexical("invoice"),
            json!({"note": 0}),
            Some("note"),
            "not_searchable",
        ),
        (
            lexical("invoice"),
            json!({"body": 0, "note": 1}),
            Some("body"),
            "weight_range",
        ),
        (
            lexical("invoice"),
            json!({"body": 0, "subject": 1}),
            Some("body"),
            "weight_range",
        ),
        (
            hybrid,
            json!({"body": 1_000_001}),
            Some("body"),
            "weight_range",
        ),
        (
            lexical("invoice"),
            json!({}),
            None,
            "search_text_fields_empty",
        ),
    ] {
        let mut s = mode;
        (s["type"], s["text_fields"]) = (json!("memo"), weights);
        let what = s.to_string();
        let s: Search = serde_json::from_value(s).expect("the check's search reads");
        let answer = l.cat.search(&l.own, s).await;
        let on = refused_on(&answer).map(|(f, _)| f);
        if on != named || !invalid_like(&answer, &json!({"code": code})) {
            wrong.push(format!("{what}: {answer:?}"));
        }
    }
    ensure!(
        wrong.is_empty(),
        "a search's `text_fields` must be refused as the setting's are: {wrong:?}"
    );
    Ok(())
}
