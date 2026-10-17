//! A search may take a second stage
//! (`docs/decisions/067-a-search-may-take-a-second-stage.md`).
//!
//! The suite's reranker scores a text by its count of `overdue` (twice)
//! and `reminder`, and records every call it receives.

use lakelands_api::error::{Case, Seam};
use lakelands_api::query::{Hits, QueryResult};
use lakelands_api::seam::{RerankCall, Seams};
use lakelands_api::{Access, Error, LakeStore, Principal};
use serde_json::{Value, json};

use crate::conformance::fixture::{
    Lake, Must, field_access, head_unmoved, invalid_case, pattern, reads_all, restrict, searchable,
    type_decl,
};
use crate::conformance::{Failure, Given, Held, Store, backing};

check!(
    RERANK_REORDERS_THE_WINDOW =
        a_reranked_window_is_answered_in_the_stages_order_and_pages_without_asking_again,
    "067 (lakelands#993): a search with `rerank` answers the first `top` hits of its ranking in the order the bound reranker scores them, ties in the first stage's order and the scores its own, and ends at that window: `ranked_by` adds ` + rerank`, `axes` ends with a `rerank` axis naming the model and the window's size, and `truncated` says the first stage held more; a cursor pages the reranked window without a second call and reads its own position, so a write that would rank first is not in a later page, and a record of the window held since is not answered there, its slot left empty; a cursor continued with another `top`, or for a search without `rerank`, is refused as another read; a window with no hit asks the reranker nothing"
);

check!(
    RERANK_SEES_ONLY_WHAT_ITS_READER_READS =
        the_reranker_is_handed_only_the_text_and_the_records_its_reader_reads,
    "067 and 024 (lakelands#993): the text a reranker is handed for a record is its searchable fields its reader reads, in declared order, and nothing else: a field masked or hidden from the reader never reaches it, and a held record is no candidate for a reader without `restricted` and one for a reader with it; a reader's reranked answer is the same, to the score, in a lake that holds the hidden field's text and the held record and in a twin that never had them",
    given
);

check!(
    RERANK_STARTS_A_PATTERN = a_pattern_started_from_a_reranked_search_walks_the_reranked_window,
    "067 (lakelands#993): a pattern whose start has `search` and `rerank` starts from the reranked window, best first, ends at it, and pages with a cursor that asks the reranker no more; a start with `rerank` and no `search` is refused as invalid"
);

check!(
    RERANK_UNBOUND_OR_FAILING =
        a_search_is_refused_when_its_second_stage_is_not_there_or_fails_or_asks_the_impossible,
    "067 (lakelands#993): a search with `rerank` and no reranker bound is refused as unbound naming the seam, and a reranker that fails, answers other scores than candidates or a score that is no number is `unavailable`, never the first stage's order, and nothing is appended; a `top` of 0 or above the lake's `limits.max_rerank_top` (named in the refusal), and a vector search without `text`, are invalid"
);

/// `doc`: `body` and `note` are searchable. Six invoices, `d6` the one
/// the reranker puts first.
async fn invoices(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    l.declare(type_decl(
        "doc",
        vec![searchable("body"), searchable("note")],
    ))
    .await?;
    let docs = [
        ("d1", "invoice invoice invoice batch export"),
        ("d2", "invoice invoice archive copy"),
        ("d3", "invoice invoice ledger line"),
        ("d4", "invoice scan"),
        ("d5", "invoice paid"),
        (
            "d6",
            "the invoice is overdue: second reminder sent to the customer",
        ),
    ];
    for (key, body) in docs {
        l.upsert("doc", key, json!({ "body": body })).await?;
    }
    Ok(l)
}

/// `who`'s search of `doc` given as JSON.
async fn ask(l: &Lake, who: &Principal, search: Value) -> Result<Hits, Failure> {
    let what = format!("the search {search}");
    Ok(l.cat.search(who, read(search)?).await.must(&what)?.data)
}

fn read(mut search: Value) -> Result<lakelands_api::query::Search, Failure> {
    search["type"] = json!("doc");
    serde_json::from_value(search)
        .map_err(|e| Failure::new(format!("the check's search does not read: {e}")))
}

/// The keys and scores of a page, as one line.
fn line(h: &Hits) -> String {
    let hits = h
        .hits
        .iter()
        .map(|h| format!("{}={}", h.record.key, h.score));
    hits.collect::<Vec<_>>().join(" ")
}

/// The calls the reranker received after the first `since`.
fn calls_since(since: usize) -> Vec<RerankCall> {
    backing::rerank_calls().split_off(since)
}

fn texts(call: &RerankCall) -> Vec<(&str, &str)> {
    let c = call.candidates.iter();
    c.map(|c| (c.key.as_str(), c.text.as_str())).collect()
}

fn bound(store: &dyn LakeStore) -> Held {
    store
        .bind_rerank(backing::reranker())
        .must("binding the reranker")
}

async fn a_reranked_window_is_answered_in_the_stages_order_and_pages_without_asking_again(
    store: Store,
) -> Held {
    let l = invoices(store.as_ref()).await?;
    bound(store.as_ref())?;
    let since = backing::rerank_calls().len();
    let own = &l.own;
    let first = json!({"mode": "lexical", "text": "invoice", "k": 3, "rerank": {"top": 6}});
    let one = ask(&l, own, first.clone()).await?;
    ensure!(
        line(&one) == "d6=3 d1=0 d2=0",
        "a window of six reranked, page 1 of 3, must answer the stage's best first and its ties in \
         key order, with its scores: `d6=3 d1=0 d2=0`; it answered `{}`",
        line(&one)
    );
    ensure!(
        one.ranked_by == "lexical + rerank" && !one.truncated && one.next_cursor.is_some(),
        "the answer must say `lexical + rerank`, not truncated, with a next page; it said {:?}, \
         truncated {}, cursor {:?}",
        one.ranked_by,
        one.truncated,
        one.next_cursor
    );
    let axes: Vec<_> = (one.axes.iter())
        .map(|a| (a.axis.as_str(), a.model.as_deref(), a.candidates))
        .collect();
    ensure!(
        axes == [
            ("lexical", None, None),
            ("rerank", Some(backing::RERANK_MODEL), Some(6))
        ],
        "the axes must end with the `rerank` axis naming the model and the window's size; they \
         were {axes:?}"
    );
    let asked = calls_since(since);
    ensure!(
        asked.len() == 1 && asked[0].query == "invoice" && asked[0].candidates.len() == 6,
        "the first page must call the reranker once, with the query and the six candidates; it \
         made {} calls",
        asked.len()
    );
    ensure!(
        asked[0]
            .candidates
            .iter()
            .map(|c| c.key.as_str())
            .collect::<Vec<_>>()
            == ["d1", "d2", "d3", "d4", "d5", "d6"],
        "the candidates must come in the first stage's order; they came {:?}",
        texts(&asked[0])
    );
    // A write that would rank first lands between the pages.
    l.upsert("doc", "d0", json!({"body": "invoice overdue overdue"}))
        .await?;
    let mut next = first.clone();
    next["cursor"] = json!(one.next_cursor);
    let two = ask(&l, own, next).await?;
    ensure!(
        line(&two) == "d3=0 d4=0 d5=0" && two.next_cursor.is_none() && !two.truncated,
        "page 2 by the cursor must continue the reranked window at the position of page 1 and \
         end at it: `d3=0 d4=0 d5=0`, no next page; it answered `{}`, next {:?}",
        line(&two),
        two.next_cursor
    );
    ensure!(
        calls_since(since).len() == 1,
        "page 2 must ask the reranker nothing; the reranker has had {} calls",
        calls_since(since).len()
    );
    // A window of four of the six: the answer ends at it.
    let mut four = first.clone();
    four["k"] = json!(10);
    four["rerank"] = json!({"top": 4});
    let hits = ask(&l, own, four).await?;
    ensure!(
        hits.truncated && hits.next_cursor.is_none() && hits.hits.len() == 4,
        "a `top` of 4 over seven matches must answer four hits and say `truncated`, with no next \
         page; it answered `{}`, truncated {}",
        line(&hits),
        hits.truncated
    );
    // A cursor is of the read it was handed out for.
    let mut other = first.clone();
    other["cursor"] = json!(one.next_cursor);
    other["rerank"] = json!({"top": 5});
    let mut plain = first.clone();
    plain["cursor"] = json!(one.next_cursor);
    plain.as_object_mut().map(|o| o.remove("rerank"));
    for (what, search) in [("another `top`", other), ("no `rerank`", plain)] {
        let err = l.cat.search(own, read(search)?).await;
        let err = err.must_fail(&format!("a cursor continued with {what}"))?;
        ensure!(
            invalid_case(&err).is_some_and(|c| matches!(c, Case::CursorOtherRead { .. })),
            "a cursor continued with {what} must be refused as another read; it failed with \
             {err:?}"
        );
    }
    // A page answers what its own window holds: a record of the window held
    // since is not answered to a reader without `restricted`, and its slot
    // stays empty, so nothing after it is skipped or answered twice.
    let small = json!({"mode": "lexical", "text": "invoice", "k": 1, "rerank": {"top": 3}});
    let small_first = ask(&l, own, small.clone()).await?;
    restrict(store.as_ref(), "doc", "d1", "legal hold").await?;
    let plain = reads_all(store.as_ref(), "plain");
    let mut pages = Vec::new();
    let mut cursor = small_first.next_cursor.clone();
    while let Some(c) = cursor.take().filter(|_| pages.len() < 5) {
        let mut next = small.clone();
        next["cursor"] = json!(c);
        let h = ask(&l, &plain, next).await?;
        pages.push(line(&h));
        cursor = h.next_cursor;
    }
    ensure!(
        line(&small_first) == "d0=4" && pages == ["", "d2=0"],
        "a window of 3 (`d0 d1 d2`), `d1` held after page 1, paged on by a reader without \
         `restricted`, must answer page 2 empty and page 3 `d2=0`; page 1 was `{}`, then {pages:?}",
        line(&small_first)
    );
    // Nothing found, nothing to rank.
    let since = backing::rerank_calls().len();
    let none = json!({"mode": "lexical", "text": "zzz", "rerank": {"top": 6}});
    let none = ask(&l, own, none).await?;
    ensure!(
        none.hits.is_empty() && calls_since(since).is_empty(),
        "a window with no hit must answer none and ask the reranker nothing; it answered `{}` \
         after {} calls",
        line(&none),
        calls_since(since).len()
    );
    Ok(())
}

/// `doc` as `invoices` leaves it for the rights twins: `a` holds a note
/// the reader may be kept from, `h` is held; the twin has neither.
async fn notes(store: &dyn LakeStore, hidden: bool) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    l.declare(type_decl(
        "doc",
        vec![searchable("body"), searchable("note")],
    ))
    .await?;
    let note = |text: &str| json!({"body": "invoice paid", "note": text});
    l.upsert(
        "doc",
        "a",
        if hidden {
            note("overdue overdue overdue")
        } else {
            note("")
        },
    )
    .await?;
    l.upsert("doc", "b", json!({"body": "invoice overdue"}))
        .await?;
    if hidden {
        l.upsert("doc", "h", json!({"body": "invoice reminder reminder"}))
            .await?;
        restrict(store, "doc", "h", "legal hold").await?;
    }
    Ok(l)
}

/// What a reader is answered, and what the reranker is handed for it.
async fn answered(l: &Lake, who: &Principal) -> Result<(String, Vec<String>), Failure> {
    let since = backing::rerank_calls().len();
    let search = json!({"mode": "lexical", "text": "invoice", "rerank": {"top": 10}});
    let h = ask(l, who, search).await?;
    let axes = format!("{:?}", h.axes);
    let said = format!("{} | {} | {} | {axes}", line(&h), h.ranked_by, h.truncated);
    let handed = calls_since(since)
        .iter()
        .flat_map(texts)
        .map(|(k, t)| format!("{k}:{t:?}"))
        .collect();
    Ok((said, handed))
}

async fn the_reranker_is_handed_only_the_text_and_the_records_its_reader_reads(
    store: Store,
    given: Given,
) -> Held {
    let a = notes(store.as_ref(), true).await?;
    bound(store.as_ref())?;
    let seams = Seams {
        rerank: Some(backing::reranker()),
        ..Default::default()
    };
    let twin = (given.twin)(store.lake().clone(), seams).await;
    let twin = twin.map_err(|e| Failure::new(format!("the harness opened no lake: {e}")))?;
    let b = notes(twin.as_ref(), false).await?;
    let mut wrong = Vec::new();
    for (name, access) in [("masked", Access::Masked), ("hidden", Access::None)] {
        let reader = field_access(store.as_ref(), name, "doc", &[("note", access)]);
        let beside = field_access(twin.as_ref(), name, "doc", &[("note", access)]);
        let (here, handed) = answered(&a, &reader).await?;
        let (there, absent) = answered(&b, &beside).await?;
        if here != there {
            wrong.push(format!(
                "{name}: answered `{here}` where the twin that never had them answered `{there}`"
            ));
        }
        if handed != absent {
            wrong.push(format!(
                "{name}: handed {handed:?} where the twin was handed {absent:?}"
            ));
        }
        if handed
            .iter()
            .any(|h| h.contains("overdue overdue") || h.starts_with("h:"))
        {
            wrong.push(format!(
                "{name}: handed {handed:?}, which holds a hidden note or the held record"
            ));
        }
    }
    // The twin with the grants: the note and the held record are handed.
    let (said, handed) = answered(&a, &a.own).await?;
    let all = handed
        .iter()
        .any(|h| h == "a:\"invoice paid\\n\\noverdue overdue overdue\"")
        && handed.iter().any(|h| h.starts_with("h:"));
    if !all || !said.starts_with("a=6 b=2 h=2") {
        wrong.push(format!(
            "the owner: answered `{said}`, handed {handed:?}; it reads the note and the held record"
        ));
    }
    ensure!(
        wrong.is_empty(),
        "the reranker must see what its reader reads: {wrong:?}"
    );
    twin.close().await.must("closing the twin")?;
    Ok(())
}

async fn a_pattern_started_from_a_reranked_search_walks_the_reranked_window(store: Store) -> Held {
    let l = invoices(store.as_ref()).await?;
    bound(store.as_ref())?;
    let since = backing::rerank_calls().len();
    let start = |cursor: Option<lakelands_api::Cursor>| {
        let mut p = pattern(
            "doc",
            None,
            vec![],
            lakelands_api::query::Projection::default(),
        );
        p.start.search = Some(lakelands_api::query::SearchMode::lexical("invoice"));
        p.start.rerank = Some(lakelands_api::query::Rerank { top: 6, text: None });
        (p.limit, p.cursor) = (Some(3), cursor);
        p
    };
    let keys = |r: QueryResult| match r {
        QueryResult::Rows { rows, next_cursor } => {
            let keys = rows
                .iter()
                .map(|r| r["doc.key"].as_str().unwrap_or("?").to_owned());
            (keys.collect::<Vec<_>>(), next_cursor)
        }
        _ => (vec!["not rows".to_owned()], None),
    };
    let one = l
        .cat
        .query(&l.own, start(None))
        .await
        .must("a reranked start, page 1")?;
    let (first, cursor) = keys(one.data);
    ensure!(
        first == ["d6", "d1", "d2"] && cursor.is_some(),
        "a pattern started from the reranked window must walk `d6` first and page on; page 1 \
         was {first:?}, cursor {cursor:?}"
    );
    let two = l
        .cat
        .query(&l.own, start(cursor))
        .await
        .must("a reranked start, page 2")?;
    let (second, cursor) = keys(two.data);
    ensure!(
        second == ["d3", "d4", "d5"] && cursor.is_none(),
        "page 2 must continue the reranked window and end at it; it was {second:?}, cursor {cursor:?}"
    );
    ensure!(
        calls_since(since).len() == 1,
        "the pages must ask the reranker once; it had {} calls",
        calls_since(since).len()
    );
    let mut bare = pattern(
        "doc",
        None,
        vec![],
        lakelands_api::query::Projection::default(),
    );
    bare.start.rerank = Some(lakelands_api::query::Rerank { top: 6, text: None });
    let err = l
        .cat
        .query(&l.own, bare)
        .await
        .must_fail("a start with `rerank` and no `search`")?;
    ensure!(
        invalid_case(&err) == Some(&Case::RerankWithoutSearch),
        "a start with `rerank` and no `search` must be refused as invalid; it failed with {err:?}"
    );
    Ok(())
}

async fn a_search_is_refused_when_its_second_stage_is_not_there_or_fails_or_asks_the_impossible(
    store: Store,
) -> Held {
    let l = invoices(store.as_ref()).await?;
    let head = store.head().await.must("reading the head")?;
    let search = |q: &str, top: u64| json!({"mode": "lexical", "text": "invoice", "rerank": {"top": top, "text": q}});
    let err = l.cat.search(&l.own, read(search("invoice", 6))?).await;
    let err = err.must_fail("a reranked search with no reranker bound")?;
    ensure!(
        matches!(&err, Error::Unbound { seam } if **seam == Seam::Rerank),
        "a reranked search with no reranker bound must be refused as unbound, naming the \
         reranker; it failed with {err:?}"
    );
    bound(store.as_ref())?;
    for (what, query) in [
        ("fails", backing::RERANK_BROKEN),
        ("answers a score too few", backing::RERANK_SHORT),
        ("answers a score that is no number", backing::RERANK_NAN),
    ] {
        let err = l.cat.search(&l.own, read(search(query, 6))?).await;
        let err = err.must_fail(&format!("a reranked search whose reranker {what}"))?;
        ensure!(
            matches!(&err, Error::Unavailable { .. }),
            "a reranker that {what} must make the search unavailable, never the first stage's \
             order; it failed with {err:?}"
        );
    }
    let max = crate::conformance::fixture::limits().max_rerank_top;
    let vector =
        json!({"mode": "vector", "field": "v", "vector": [1.0, 0.0], "rerank": {"top": 3}});
    for (what, asked, want) in [
        ("a `top` of 0", search("invoice", 0), Case::RerankTopZero),
        (
            "a `top` above the lake's bound",
            search("invoice", u64::from(max) + 1),
            Case::RerankTopAbove { max },
        ),
        (
            "a vector search without `text`",
            vector,
            Case::RerankNeedsText,
        ),
    ] {
        let err = l.cat.search(&l.own, read(asked)?).await.must_fail(what)?;
        ensure!(
            invalid_case(&err) == Some(&want),
            "{what} must be refused as invalid with {want:?}; it failed with {err:?}"
        );
    }
    head_unmoved(store.as_ref(), head, "a reranked search").await
}
