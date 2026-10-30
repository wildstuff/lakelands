//! A search may take a second stage
//! (`docs/decisions/067-a-search-may-take-a-second-stage.md`).
//!
//! The suite's reranker scores a text by its count of `overdue` (twice)
//! and `reminder`, and records every call it receives.

use lakelands_api::seam::{RerankCall, Reranker};
use lakelands_api::write::WriteOpts;

use crate::conformance::prelude::*;

check!(
    RERANK_REORDERS_THE_WINDOW =
        a_reranked_window_is_answered_in_the_stages_order_and_pages_without_asking_again,
    "067 (lakelands#993): a search with `rerank` answers the first `top` hits of its ranking in the order the bound reranker scores them, ties in the first stage's order and the scores its own, and ends at that window: `ranked_by` adds ` + rerank`, `axes` ends with a `rerank` axis naming the model and the window's size, and `truncated` says the first stage held more; a cursor pages the reranked window without a second call and reads its own position, so a write that would rank first is not in a later page, and a record of the window held since is not answered there, its slot left empty; a cursor continued with another `top`, or for a search without `rerank`, is refused as another read; a window with no hit asks the reranker nothing"
);

check!(
    RERANK_ONE_WINDOW =
        a_reranked_search_answers_every_hit_it_scored_whatever_lands_after_its_window,
    "067 (lakelands#993): the answer of a reranked search is the hits the reranker scored, each read by its key where the window was read: a write during the reranker's call that edits a record outside the window so that it would rank into it, and one that retires a scored record, change no hit of the first page or of any later one; nor does an edit between two pages that would move another record into the window; every scored key is answered once, in the stage's order"
);

check!(
    RERANK_ONE_WINDOW_ONLY =
        a_holder_of_only_is_answered_every_hit_it_was_scored_whatever_lands_after_its_window,
    "067 and 051 (lakelands#993): a holder of `only` is answered every hit of its window the reranker scored, judged where the window was read, never at the head: a scored record retired, or moved out of the holder's `only`, during the reranker's call or between two pages is still answered, once, in the stage's order"
);

check!(
    RERANK_ONLY_ANY = a_holder_of_a_set_is_reranked_over_the_records_its_set_admits,
    "067 and 006 (lakelands#1588): a holder of an `only_any` set on a type is answered, reranked, only the records whose field holds one of its values, and the reranker is handed those alone"
);

check!(
    RERANK_RANKS_AGAINST =
        a_vector_search_is_reranked_against_its_text_and_a_hybrid_one_against_its_own,
    "067 (lakelands#993): a vector search's window is ranked against the `text` its `rerank` gives, and a hybrid search's against the search's own text where its `rerank` gives none; each answers `ranked_by` with ` + rerank`"
);

check!(
    RERANK_PAGES_WITHOUT_EMBEDDING =
        a_reranked_hybrid_search_embeds_its_text_on_the_first_page_alone,
    "067 and 025 (lakelands#993): a reranked hybrid search sent without a vector has its text embedded once, for the window on its first page; a later page embeds nothing and answers its keys though the embedding backend now fails"
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
    "067 (lakelands#993): a search with `rerank` and no reranker bound is refused as unbound naming the seam, and a reranker that fails, answers other scores than candidates or a score that is no number is `unavailable`, never the first stage's order, and nothing is appended; a `top` of 0 or above the lake's `limits.max_rerank_top` (named in the refusal), a vector search without `text`, a blank `text` and a blank search text with none are invalid; a location whose `limits.max_rerank_top` is 0 is refused at open, naming the setting",
    given
);

/// `doc`: `body` and `note` are searchable, and every record's `tag` is
/// `x`. Six invoices, `d6` the one the reranker puts first.
async fn invoices(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    l.declare(type_decl(
        "doc",
        vec![
            searchable("body"),
            searchable("note"),
            field("tag", StorageKind::Text),
            vector("v", 2),
        ],
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
        l.upsert(
            "doc",
            key,
            json!({ "body": body, "tag": "x", "v": [1.0, 0.0] }),
        )
        .await?;
    }
    Ok(l)
}

/// `cursor` with one key more in its stage's order than the lake handed
/// out, as hex.
fn longer(cursor: Option<&lakelands_api::Cursor>) -> String {
    let hex = cursor.map(|c| c.0.clone()).unwrap_or_default();
    let bytes: Vec<u8> = (0..hex.len() / 2)
        .filter_map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok())
        .collect();
    let mut state: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if let Some(order) = state["stage"]["order"].as_array_mut() {
        order.push(json!(["d9", 1.0]));
    }
    let text = serde_json::to_string(&state).unwrap_or_default();
    text.bytes().map(|b| format!("{b:02x}")).collect()
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
    let d0 = json!({"body": "invoice invoice invoice invoice overdue overdue"});
    l.upsert("doc", "d0", d0).await?;
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
    // A cursor holds at most the window: one whose order is longer is not
    // one this lake handed out.
    let long = longer(one.next_cursor.as_ref());
    let mut forged = first.clone();
    forged["cursor"] = json!(long);
    let err = l.cat.search(own, read(forged)?).await;
    let err = err.must_fail("a cursor whose order is longer than its window")?;
    ensure!(
        invalid_case(&err) == Some(&Case::CursorForeign),
        "a cursor whose order is longer than its `top` must be refused as foreign; it failed \
         with {err:?}"
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
    // A search that names its fields hands those alone, whatever else its
    // reader reads.
    let since = backing::rerank_calls().len();
    let named =
        json!({"mode": "lexical", "text": "invoice", "fields": ["body"], "rerank": {"top": 10}});
    ask(&a, &a.own, named).await?;
    let handed: Vec<String> = calls_since(since)
        .iter()
        .flat_map(texts)
        .map(|(k, t)| format!("{k}:{t:?}"))
        .collect();
    if !handed.contains(&"a:\"invoice paid\"".to_owned())
        || handed.iter().any(|h| h.contains("overdue overdue"))
    {
        wrong.push(format!(
            "the owner, naming `body`: handed {handed:?}; `note` is not named"
        ));
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
    let forged = longer(cursor.as_ref());
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
    // A cursor longer than the window is not one the lake handed out.
    let err = l
        .cat
        .query(&l.own, start(Some(lakelands_api::Cursor(forged))))
        .await;
    let err = err.must_fail("a pattern's cursor whose order is longer than its window")?;
    ensure!(
        invalid_case(&err) == Some(&Case::CursorForeign),
        "a pattern's cursor whose order is longer than its `top` must be refused as foreign; \
         it failed with {err:?}"
    );
    // A pattern refused for itself asks the reranker nothing.
    let since = backing::rerank_calls().len();
    let mut unlimited = start(None);
    unlimited.limit = Some(0);
    let err = l.cat.query(&l.own, unlimited).await;
    let err = err.must_fail("a reranked start with a limit of 0")?;
    ensure!(
        matches!(invalid_case(&err), Some(Case::LimitRange { .. }))
            && calls_since(since).is_empty(),
        "a reranked start with a limit of 0 must be refused for its limit before the reranker \
         is asked; it failed with {err:?} after {} calls",
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
        invalid_case(&err) == Some(&Case::RerankWithoutSearch)
            && matches!(&err, Error::Refused { field: Some(f), .. } if f.as_str() == "rerank"),
        "a start with `rerank` and no `search` must be refused as invalid; it failed with {err:?}"
    );
    Ok(())
}

async fn a_search_is_refused_when_its_second_stage_is_not_there_or_fails_or_asks_the_impossible(
    store: Store,
    given: Given,
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
    let mut started = pattern(
        "doc",
        None,
        vec![],
        lakelands_api::query::Projection::default(),
    );
    started.start.search = Some(lakelands_api::query::SearchMode::lexical("invoice"));
    started.start.rerank = Some(lakelands_api::query::Rerank { top: 6, text: None });
    let err = l.cat.query(&l.own, started).await;
    let err = err.must_fail("a pattern started from a reranked search with no reranker bound")?;
    ensure!(
        matches!(&err, Error::Unbound { seam } if **seam == Seam::Rerank),
        "a pattern started from a reranked search with no reranker bound must be refused as \
         unbound, naming the reranker; it failed with {err:?}"
    );
    bound(store.as_ref())?;
    use lakelands_api::error::{Exchange, Remote, Situation};
    let exchange = |err: &Error| match err.situation() {
        Some(Situation::Remote {
            remote: Remote::Reranker { model },
            exchange,
        }) if model == backing::RERANK_MODEL => Some(exchange.clone()),
        _ => None,
    };
    for (what, query, want) in [
        ("is unavailable", backing::RERANK_BROKEN, "status"),
        ("refuses the request", backing::RERANK_REFUSED, "failed"),
        ("answers a score too few", backing::RERANK_SHORT, "scores"),
        (
            "answers a score that is no number",
            backing::RERANK_NAN,
            "not finite",
        ),
    ] {
        let err = l.cat.search(&l.own, read(search(query, 6))?).await;
        let err = err.must_fail(&format!("a reranked search whose reranker {what}"))?;
        let got = exchange(&err);
        let fits = match (want, &got) {
            ("status", Some(Exchange::Status { status: 503 })) => true,
            ("failed", Some(Exchange::Failed { cause })) => {
                matches!(&**cause, Error::Refused { .. })
            }
            (
                "scores",
                Some(Exchange::Scores {
                    answered: 5,
                    asked: 6,
                }),
            ) => true,
            ("not finite", Some(Exchange::ScoreNotFinite)) => true,
            _ => false,
        };
        ensure!(
            fits,
            "a reranker that {what} must make the search unavailable, calling the reranker \
             `{}` and saying `{want}`, never the first stage's order; it failed with {err:?}",
            backing::RERANK_MODEL
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
        ("an empty `text`", search(" ", 6), Case::RerankNeedsText),
        (
            "a blank search's text with no `text`",
            json!({"mode": "lexical", "text": " ", "rerank": {"top": 6}}),
            Case::RerankNeedsText,
        ),
    ] {
        let err = l.cat.search(&l.own, read(asked)?).await.must_fail(what)?;
        ensure!(
            invalid_case(&err) == Some(&want),
            "{what} must be refused as invalid with {want:?}; it failed with {err:?}"
        );
    }
    head_unmoved(store.as_ref(), head, "a reranked search").await?;
    let none = lakelands_api::location::Limits {
        max_rerank_top: 0,
        ..crate::conformance::fixture::limits()
    };
    let err = (given.relimit)(store, Seams::default(), none).await;
    let err = err.err().ok_or_else(|| {
        Failure::new("a location whose `limits.max_rerank_top` is 0 must not be opened; it was")
    })?;
    ensure!(
        matches!(&err, Error::Refused { field: Some(f), .. } if f == "limits.max_rerank_top")
            && invalid_case(&err) == Some(&Case::RerankNoWindow),
        "a location whose `limits.max_rerank_top` is 0 must be refused at open as invalid, \
         naming the setting; it failed with {err:?}"
    );
    Ok(())
}

/// The suite's reranker, which first makes `writes` through the lake's
/// doors, once, as the owner: each an upsert of a `doc`'s fields, or a
/// retirement where it names none.
struct Writing {
    store: Store,
    writes: std::sync::Mutex<Vec<(&'static str, Option<Value>)>>,
}

#[async_trait::async_trait]
impl Reranker for Writing {
    fn model(&self) -> &str {
        backing::RERANK_MODEL
    }

    async fn rerank(&self, call: &RerankCall) -> lakelands_api::Result<Vec<f64>> {
        let writes = std::mem::take(&mut *self.writes.lock().unwrap_or_else(|p| p.into_inner()));
        let (cat, own) = (
            crate::conformance::fixture::doors(self.store.as_ref()),
            crate::conformance::fixture::owner(self.store.as_ref()),
        );
        for (key, body) in writes {
            let (ty, key) = (&"doc".into(), &key.into());
            match body {
                Some(fields) => {
                    let patch = crate::conformance::fixture::patch(fields);
                    cat.upsert(&own, ty, key, patch, WriteOpts::default())
                        .await?;
                }
                None => {
                    cat.retire(&own, ty, key, WriteOpts::default()).await?;
                }
            }
        }
        backing::reranker().rerank(call).await
    }
}

/// The keys of every page of `search` by `who`, page by page, and the
/// first page's answer; `between` runs after the first page.
async fn pages<F, W>(
    l: &Lake,
    who: &Principal,
    search: Value,
    between: F,
) -> Result<(Vec<String>, Hits), Failure>
where
    F: FnOnce() -> W,
    W: std::future::Future<Output = Held>,
{
    let first = ask(l, who, search.clone()).await?;
    let mut keys: Vec<String> = first
        .hits
        .iter()
        .map(|h| h.record.key.to_string())
        .collect();
    between().await?;
    let mut cursor = first.next_cursor.clone();
    while let Some(c) = cursor.take().filter(|_| keys.len() < 20) {
        let mut next = search.clone();
        next["cursor"] = json!(c);
        let h = ask(l, who, next).await?;
        keys.extend(h.hits.iter().map(|h| h.record.key.to_string()));
        cursor = h.next_cursor;
    }
    Ok((keys, first))
}

async fn a_reranked_search_answers_every_hit_it_scored_whatever_lands_after_its_window(
    store: Store,
) -> Held {
    let l = invoices(store.as_ref()).await?;
    // `invoice reminder`, a window of 3: `d6` holds both words, then `d1`
    // and `d2` by key. During the call `d4` is edited to hold both, so a
    // ranking now would take it into the window, and `d1` is retired.
    let writing = Writing {
        store: store.clone(),
        writes: std::sync::Mutex::new(vec![
            ("d4", Some(json!({"body": "invoice reminder note"}))),
            ("d1", None),
        ]),
    };
    store
        .bind_rerank(std::sync::Arc::new(writing))
        .must("binding the writing reranker")?;
    let since = backing::rerank_calls().len();
    let search =
        json!({"mode": "lexical", "text": "invoice reminder", "k": 1, "rerank": {"top": 3}});
    // Between the first page and the next, `d3` is edited to hold both
    // words: a ranking then would take it into the window too.
    let between = || async {
        l.upsert("doc", "d3", json!({"body": "invoice reminder again"}))
            .await
            .map(drop)
    };
    let (got, first) = pages(&l, &l.own, search, between).await?;
    let calls = calls_since(since);
    ensure!(
        calls.len() == 1,
        "the search must ask the reranker once; it asked {} times",
        calls.len()
    );
    let mut scored: Vec<(String, f64)> = (calls[0].candidates.iter())
        .map(|c| (c.key.to_string(), backing::rerank_score(&c.text)))
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let want: Vec<String> = scored.into_iter().map(|(k, _)| k).collect();
    ensure!(
        want == ["d6", "d1", "d2"],
        "the window of `invoice reminder` must be `d6 d1 d2`, scored so; the reranker was \
         handed {:?}",
        texts(&calls[0])
    );
    ensure!(
        got == want,
        "a reranked search must answer every hit it scored, once, in the stage's order, \
         whatever was written after its window: {want:?}; its pages answered {got:?}"
    );
    ensure!(
        first.truncated && first.axes.last().and_then(|a| a.candidates) == Some(3),
        "the first page must say the window it scored: truncated, 3 candidates; it said \
         truncated {}, axes {:?}",
        first.truncated,
        first.axes
    );
    Ok(())
}

async fn a_holder_of_only_is_answered_every_hit_it_was_scored_whatever_lands_after_its_window(
    store: Store,
) -> Held {
    let l = invoices(store.as_ref()).await?;
    let mut grants = lakelands_api::Grants {
        default: Access::Read,
        ..Default::default()
    };
    let only = [("tag".into(), json!("x"))].into_iter().collect();
    grants.types.insert(
        "doc".into(),
        lakelands_api::principal::TypeGrants {
            only,
            ..Default::default()
        },
    );
    let kind = lakelands_api::principal::PrincipalKind::Agent;
    let holder = crate::conformance::fixture::principal(store.as_ref(), kind, "holder", grants);
    // During the call: `d1`, scored, is retired, and `d2`, scored, is moved
    // out of the holder's `only`.
    let writing = Writing {
        store: store.clone(),
        writes: std::sync::Mutex::new(vec![("d1", None), ("d2", Some(json!({"tag": "y"})))]),
    };
    store
        .bind_rerank(std::sync::Arc::new(writing))
        .must("binding the writing reranker")?;
    let during =
        json!({"mode": "lexical", "text": "invoice reminder", "k": 1, "rerank": {"top": 3}});
    let (got, _) = pages(&l, &holder, during, || async { Ok(()) }).await?;
    ensure!(
        got == ["d6", "d1", "d2"],
        "a holder of `only` scored `d6 d1 d2` must be answered all three though `d1` was \
         retired and `d2` moved out of its `only` during the reranker's call; it was answered \
         {got:?}"
    );
    // Between pages: of the window `d3 d4 d5`, `d4` is retired and `d5`
    // moved out of the holder's `only` after page 1.
    let between = || async {
        l.retire("doc", "d4").await?;
        l.upsert("doc", "d5", json!({"tag": "y"})).await.map(drop)
    };
    let after = json!({"mode": "lexical", "text": "invoice", "k": 1, "rerank": {"top": 3}});
    let (got, _) = pages(&l, &holder, after, between).await?;
    ensure!(
        got == ["d3", "d4", "d5"],
        "a holder of `only` scored `d3 d4 d5` must be answered all three though `d4` was \
         retired and `d5` moved out of its `only` between pages; it was answered {got:?}"
    );
    Ok(())
}

async fn a_vector_search_is_reranked_against_its_text_and_a_hybrid_one_against_its_own(
    store: Store,
) -> Held {
    let l = invoices(store.as_ref()).await?;
    bound(store.as_ref())?;
    let mut wrong = Vec::new();
    for (search, query, ranked) in [
        (
            json!({"mode": "vector", "field": "v", "vector": [1.0, 0.0], "rerank": {"top": 3, "text": "overdue reminder"}}),
            "overdue reminder",
            "vector + rerank",
        ),
        (
            json!({"mode": "hybrid", "text": "invoice reminder", "field": "v", "vector": [1.0, 0.0], "rerank": {"top": 3}}),
            "invoice reminder",
            "hybrid + rerank",
        ),
    ] {
        let since = backing::rerank_calls().len();
        let h = ask(&l, &l.own, search.clone()).await?;
        let calls = calls_since(since);
        let asked: Vec<&str> = calls.iter().map(|c| c.query.as_str()).collect();
        if asked != [query] || h.ranked_by != ranked {
            wrong.push(format!(
                "{search}: asked {asked:?}, ranked by {:?}",
                h.ranked_by
            ));
        }
    }
    ensure!(
        wrong.is_empty(),
        "the window must be ranked against the stage's text: {wrong:?}"
    );
    Ok(())
}

async fn a_reranked_hybrid_search_embeds_its_text_on_the_first_page_alone(store: Store) -> Held {
    store
        .bind_embed(backing::embedder())
        .must("binding the suite's embedding backend")?;
    bound(store.as_ref())?;
    let l = Lake::of(store.as_ref());
    let v = crate::conformance::fixture::embedding("v", 2, &["body"], backing::EMBED_MODEL);
    l.declare(type_decl("doc", vec![searchable("body"), v]))
        .await?;
    for (key, body) in [
        ("e1", "invoice batch"),
        ("e2", "invoice overdue"),
        ("e3", "invoice copy"),
        ("e4", "invoice reminder"),
        ("e5", "invoice scan"),
    ] {
        l.upsert("doc", key, json!({ "body": body })).await?;
    }
    let text = "invoice paged once";
    let search =
        json!({"mode": "hybrid", "text": text, "field": "v", "k": 2, "rerank": {"top": 5}});
    let one = ask(&l, &l.own, search.clone()).await?;
    // A backend that fails after the first page: a later one that asked
    // it again would fail with it.
    backing::unplug_text(text);
    let (mut keys, mut cursor) = (one.hits.len(), one.next_cursor);
    let mut later = Ok(());
    while let Some(c) = cursor.take() {
        let mut next = search.clone();
        next["cursor"] = json!(c.0);
        match l.cat.search(&l.own, read(next)?).await {
            Ok(page) => {
                keys += page.data.hits.len();
                cursor = page.data.next_cursor;
            }
            Err(e) => later = Err(e),
        }
    }
    backing::plug_text(text);
    let calls = backing::embedded(text);
    ensure!(
        later.is_ok() && keys == 5 && calls == 1,
        "a reranked hybrid search over three pages must embed its text once and answer \
         every scored key though the backend fails after the first page; it embedded it \
         {calls} times, answered {keys} keys, and a later page answered {later:?}"
    );
    Ok(())
}

async fn a_holder_of_a_set_is_reranked_over_the_records_its_set_admits(store: Store) -> Held {
    let l = invoices(store.as_ref()).await?;
    bound(store.as_ref())?;
    // `d2` is tagged `y`, `d6` and `d3` `z`: the set admits `x` and `y`.
    for (key, tag) in [("d2", "y"), ("d3", "z"), ("d6", "z")] {
        l.upsert("doc", key, json!({"tag": tag})).await?;
    }
    let set = json!({"default": "read", "types": {"doc": {"only_any": {"tag": ["x", "y"]}}}});
    let holder = super::only::holder(store.as_ref(), "holder", set);
    let since = backing::rerank_calls().len();
    let search = json!({"mode": "lexical", "text": "invoice", "rerank": {"top": 10}});
    let hits = ask(&l, &holder, search).await?;
    let mut answered: Vec<&str> = hits.hits.iter().map(|h| h.record.key.as_str()).collect();
    answered.sort_unstable();
    let calls = calls_since(since);
    let mut handed: Vec<&str> = calls.iter().flat_map(texts).map(|(k, _)| k).collect();
    handed.sort_unstable();
    let admitted = ["d1", "d2", "d4", "d5"];
    ensure!(
        answered == admitted && handed == admitted,
        "a holder of the set `x`, `y` must be answered and have the reranker handed `d1 d2 d4 d5` alone; it was answered {answered:?}, the reranker handed {handed:?}"
    );
    Ok(())
}
