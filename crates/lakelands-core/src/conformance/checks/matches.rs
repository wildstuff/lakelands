//! A hit says where its text matched, and only in fields its reader is
//! shown (lakelands#995;
//! `docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`).
//!
//! A search that asks for `matches` is answered, on each hit, the searched
//! fields its text matched, each with the char spans of its first
//! `MATCH_SPANS` matched tokens and the count of every one. The store
//! computes them over the page's records by the one token pipeline that
//! ranks, so a stem and a folded letter are marked as they are counted.

use lakelands_api::query::{Hit, MATCH_SPANS, Matched, Span};

use crate::conformance::prelude::*;

check!(
    MATCHES_NAMED = a_search_asked_for_matches_names_each_field_and_span_its_text_matched,
    "025, extended (lakelands#995): a lexical search asked for `matches` names on each hit the searched fields its text matched, and in each the char spans of the matched tokens, in text order, end exclusive, with their count: a match 400 chars into a long body and one in a title are told apart; a span marks what the pipeline counted, a stem (`running` by `run`) and a folded letter (`café` by `cafe`) alike, counted in Unicode scalar values across text of several bytes; a field the text did not match is not named; the search ranks and scores as it does without them, and a second stage's text moves no span, on a later page too"
);

check!(
    MATCHES_UNASKED = matches_are_not_answered_unless_asked,
    "025, extended (lakelands#995): a search that does not ask for `matches`, or asks for none, answers its hits as before, each with no `matches` member on the wire, and the same hits and scores as the search that asks"
);

check!(
    MATCHES_NEED_TEXT = matches_without_text_is_refused_by_its_shape,
    "025, extended (lakelands#995): a vector search asking for `matches` is refused as invalid, `matches_without_text`, by its shape alone: alike to a reader not shown its vector field, before any field is judged; a pattern that starts from a search takes no `matches`"
);

check!(
    MATCHES_SHOWN = a_masked_or_hidden_field_is_never_named_in_matches,
    "025 and 024, extended (lakelands#995): a field is named in `matches` only where the search searched it and the record answers it to its reader: a reader holding a field grant that hides the matching field, or masks it, is never told it, nor a match a search limited by `fields` did not search; such a reader's hits name what they name in a twin lake that never declared the field",
    given
);

check!(
    MATCHES_HYBRID = a_hybrid_hit_reports_its_text_matches_whichever_axis_brought_it,
    "025, extended (lakelands#995): a hybrid search asked for `matches` names on each hit the matches of its text, as a lexical search names them, whichever axis brought it; a hit the vector alone brought, whose text holds no query token, names none; the axes are as without them"
);

check!(
    MATCHES_CAPPED = a_field_with_more_matches_than_the_cap_answers_the_cap_and_the_count,
    "025, extended (lakelands#995): a field whose text matched more tokens than `MATCH_SPANS`, 32, answers the first 32 spans in text order and the count of every match"
);

check!(
    MATCHES_READ_THEN = matches_are_of_the_answered_record_at_the_read_position,
    "025, extended (lakelands#995): a search as of an earlier position ranks by the head and marks the record it answers, as it stood at that position: where the matching word moved since, the spans are where it stood then, and where the text then held no query token, the hit answers no `matches`; a reranked search as of that position marks the same"
);

/// `doc`: `title` and `body` searchable, in English, `tag` not, and `v` a
/// vector; `body` left out where `bodied` is not.
fn doc(bodied: bool) -> TypeDecl {
    let mut fields = vec![searchable("title"), field("tag", StorageKind::Text)];
    if bodied {
        fields.insert(1, searchable("body"));
    }
    fields.push(vector("v", 2));
    let mut decl = type_decl("doc", fields);
    decl.language = Some("en".into());
    decl
}

/// lakelands#995's minutes: the match 400 chars into the body.
fn minutes() -> String {
    format!(
        "{} The supplier confirmed the overdue invoice will be settled on Friday. {}",
        "Minutes of the weekly meeting. ".repeat(12),
        "Next item: warehouse keys. ".repeat(12)
    )
}

/// The issue's `d1` and `d2`, `d3` matching by stems, and `d4` by a
/// folded letter in text of several bytes; `tag` repeats the words, and
/// no search reads it. `body` is written where it is declared.
async fn docs(store: &dyn LakeStore, bodied: bool) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    l.declare(doc(bodied)).await?;
    let rows = [
        ("d1", "Meeting minutes", minutes(), [1.0, 0.0]),
        (
            "d2",
            "Overdue invoice 2026-118",
            "See attachment.".into(),
            [0.0, 1.0],
        ),
        ("d3", "Running late", "Café au lait".into(), [0.6, 0.8]),
        (
            "d4",
            "Grüße aus Zürich — café ☕ CAFÉ",
            "no match here".into(),
            [0.8, 0.6],
        ),
    ];
    for (key, title, body, v) in rows {
        let tag = "overdue invoice run cafe";
        let mut fields = json!({"title": title, "tag": tag, "v": v});
        if bodied {
            fields["body"] = json!(body);
        }
        l.upsert("doc", key, fields).await?;
    }
    Ok(l)
}

/// `who`'s search of `doc`, given as JSON.
async fn search(l: &Lake, who: &Principal, mut s: Value) -> Result<Hits, Failure> {
    s["type"] = json!("doc");
    let what = format!("the search {s}");
    let read: Search = serde_json::from_value(s).expect("the check's search reads");
    Ok(l.cat.search(who, read).await.must(&what)?.data)
}

/// A lexical search for `text`, asking for `matches`.
fn asked(text: &str) -> Value {
    json!({"mode": "lexical", "text": text, "matches": true})
}

/// A field, the char spans of its matches, and their count.
type Told<'a> = (&'a str, &'a [(u32, u32)], u32);

/// One field's matches, from its spans and count.
fn spans(fields: &[Told]) -> BTreeMap<FieldName, Matched> {
    (fields.iter())
        .map(|(f, s, count)| {
            let spans = s.iter().map(|&(start, end)| Span { start, end }).collect();
            let count = *count;
            (FieldName::from(*f), Matched { spans, count })
        })
        .collect()
}

/// Each hit's key and its matches.
fn marked(h: &Hits) -> Vec<(String, BTreeMap<FieldName, Matched>)> {
    (h.hits.iter())
        .map(|h| (h.record.key.0.clone(), h.matches.clone()))
        .collect()
}

/// Each hit's key and its score's bits.
fn scored(h: &Hits) -> Vec<(String, u64)> {
    (h.hits.iter())
        .map(|h| (h.record.key.0.clone(), h.score.to_bits()))
        .collect()
}

/// What `marked` should answer.
fn want(
    rows: Vec<(&str, BTreeMap<FieldName, Matched>)>,
) -> Vec<(String, BTreeMap<FieldName, Matched>)> {
    rows.into_iter().map(|(k, m)| (k.to_owned(), m)).collect()
}

/// The chars of `text` a span covers.
fn covered(text: &str, s: &Span) -> String {
    let len = (s.end - s.start) as usize;
    text.chars().skip(s.start as usize).take(len).collect()
}

async fn a_search_asked_for_matches_names_each_field_and_span_its_text_matched(
    store: Store,
) -> Held {
    let l = docs(store.as_ref(), true).await?;
    (store.bind_rerank(backing::reranker())).must("binding the reranker")?;
    let mut wrong = Vec::new();
    let got = search(&l, &l.own, asked("overdue invoice")).await?;
    let told = want(vec![
        ("d1", spans(&[("body", &[(400, 407), (408, 415)], 2)])),
        ("d2", spans(&[("title", &[(0, 7), (8, 15)], 2)])),
    ]);
    if marked(&got) != told {
        wrong.push(format!("`overdue invoice`: {:?}", marked(&got)));
    }
    let body = minutes();
    let words: Vec<String> = (got.hits.first().map(|h| &h.matches))
        .and_then(|m| m.get(&FieldName::from("body")))
        .map(|m| m.spans.iter().map(|s| covered(&body, s)).collect())
        .unwrap_or_default();
    if words != ["overdue", "invoice"] {
        wrong.push(format!("d1's spans cover {words:?}"));
    }
    // A stem and a folded letter, in chars across text of several bytes.
    let got = search(&l, &l.own, asked("run Cafe")).await?;
    let told = want(vec![
        (
            "d3",
            spans(&[("title", &[(0, 7)], 1), ("body", &[(0, 4)], 1)]),
        ),
        ("d4", spans(&[("title", &[(19, 23), (26, 30)], 2)])),
    ]);
    if marked(&got) != told {
        wrong.push(format!("`run Cafe`: {:?}", marked(&got)));
    }
    // The ranking is the one the search answers without them.
    for text in ["overdue invoice", "run Cafe"] {
        let plain = json!({"mode": "lexical", "text": text});
        let (with, without) = (
            search(&l, &l.own, asked(text)).await?,
            search(&l, &l.own, plain).await?,
        );
        if scored(&with) != scored(&without) || with.axes != without.axes {
            wrong.push(format!("{text:?} ranks otherwise asked for matches"));
        }
    }
    // A second stage ranks by its own text, and marks the search's.
    let mut staged = asked("invoice run");
    staged["rerank"] = json!({"top": 3, "text": "overdue reminder"});
    staged["k"] = json!(1);
    let mut pages = Vec::new();
    loop {
        let page = search(&l, &l.own, staged.clone()).await?;
        pages.extend(marked(&page));
        let Some(next) = page.next_cursor else { break };
        staged["cursor"] = json!(next);
    }
    pages.sort_by(|a, b| a.0.cmp(&b.0));
    let told = want(vec![
        ("d1", spans(&[("body", &[(408, 415)], 1)])),
        ("d2", spans(&[("title", &[(8, 15)], 1)])),
        ("d3", spans(&[("title", &[(0, 7)], 1)])),
    ]);
    if pages != told {
        wrong.push(format!("reranked, page by page: {pages:?}"));
    }
    ensure!(
        wrong.is_empty(),
        "a search asked for matches must name each field and span its text matched: {wrong:?}"
    );
    Ok(())
}

async fn matches_are_not_answered_unless_asked(store: Store) -> Held {
    let l = docs(store.as_ref(), true).await?;
    let mut wrong = Vec::new();
    let text = "overdue invoice run cafe";
    let hybrid = json!({"mode": "hybrid", "text": text, "field": "v", "vector": [1, 0]});
    for s in [json!({"mode": "lexical", "text": text}), hybrid] {
        let with = {
            let mut s = s.clone();
            s["matches"] = json!(true);
            search(&l, &l.own, s).await?
        };
        let mut none = s.clone();
        none["matches"] = json!(false);
        for unasked in [s.clone(), none] {
            let got = search(&l, &l.own, unasked.clone()).await?;
            let wire = serde_json::to_value(&got).expect("hits serialise");
            let members =
                (wire["hits"].as_array().into_iter().flatten()).any(|h| h.get("matches").is_some());
            if members || got.hits.iter().any(|h| !h.matches.is_empty()) {
                wrong.push(format!("{unasked}: a hit says matches: {wire}"));
            }
            let mut stripped = with.clone();
            stripped.hits.iter_mut().for_each(|h| h.matches.clear());
            if got != stripped {
                wrong.push(format!("{unasked}: other hits than asked"));
            }
        }
        if with.hits.iter().all(|h| h.matches.is_empty()) {
            wrong.push(format!("{s}: asked, no hit says where it matched"));
        }
    }
    ensure!(
        wrong.is_empty(),
        "matches must not be answered unless asked: {wrong:?}"
    );
    Ok(())
}

async fn matches_without_text_is_refused_by_its_shape(store: Store) -> Held {
    let l = docs(store.as_ref(), true).await?;
    let blind = field_access(store.as_ref(), "blind", "doc", &[("v", Access::None)]);
    let mut wrong = Vec::new();
    let vector = json!({"mode": "vector", "field": "v", "vector": [1, 0], "matches": true});
    for (who, name) in [(&l.own, "the owner"), (&blind, "a reader not shown `v`")] {
        let mut s = vector.clone();
        s["type"] = json!("doc");
        let s: Search = serde_json::from_value(s).expect("the check's search reads");
        let answer = l.cat.search(who, s).await;
        if invalid_case(&answer) != Some(&Case::MatchesWithoutText) {
            wrong.push(format!("{name}: {answer:?}"));
        }
    }
    let mut unasked = vector.clone();
    unasked["matches"] = json!(false);
    let fine = search(&l, &l.own, unasked).await?;
    if fine.hits.is_empty() {
        wrong.push("a vector search not asking for matches answers no hit".into());
    }
    let started = json!({"start": {"type": "doc", "search": asked("invoice")}});
    if serde_json::from_value::<Pattern>(started.clone()).is_ok() {
        wrong.push(format!("a pattern's start reads `matches`: {started}"));
    }
    ensure!(
        wrong.is_empty(),
        "matches without text must be refused by the search's shape: {wrong:?}"
    );
    Ok(())
}

async fn a_masked_or_hidden_field_is_never_named_in_matches(store: Store, given: Given) -> Held {
    let l = docs(store.as_ref(), true).await?;
    let twin = (given.twin)(store.lake().clone(), Default::default()).await;
    let twin = twin.map_err(|e| Failure::new(format!("the harness opened no twin: {e}")))?;
    let t = docs(twin.as_ref(), false).await?;
    let mut wrong = Vec::new();
    let body = FieldName::from("body");
    for access in [Access::None, Access::Masked] {
        let reader = field_access(store.as_ref(), "reader", "doc", &[("body", access)]);
        for text in ["overdue invoice", "run cafe"] {
            let (here, there) = (
                search(&l, &reader, asked(text)).await?,
                search(&t, &reader, asked(text)).await?,
            );
            if here.hits.iter().any(|h| h.matches.contains_key(&body)) {
                wrong.push(format!("`body` {access:?}, {text:?}: {:?}", marked(&here)));
            }
            if marked(&here) != marked(&there) {
                let (here, there) = (marked(&here), marked(&there));
                wrong.push(format!(
                    "`body` {access:?}, {text:?}: {here:?}; never declared: {there:?}"
                ));
            }
        }
    }
    // A match in a field the search does not search is not named.
    let mut titled = asked("run cafe");
    titled["fields"] = json!(["title"]);
    let got = search(&l, &l.own, titled).await?;
    let told = want(vec![
        ("d3", spans(&[("title", &[(0, 7)], 1)])),
        ("d4", spans(&[("title", &[(19, 23), (26, 30)], 2)])),
    ]);
    if marked(&got) != told {
        wrong.push(format!("`fields: [title]`: {:?}", marked(&got)));
    }
    ensure!(
        wrong.is_empty(),
        "a masked or hidden field must never be named in matches: {wrong:?}"
    );
    Ok(())
}

async fn a_hybrid_hit_reports_its_text_matches_whichever_axis_brought_it(store: Store) -> Held {
    let l = docs(store.as_ref(), true).await?;
    let mut wrong = Vec::new();
    let text = "overdue invoice";
    let hybrid = json!({"mode": "hybrid", "text": text, "field": "v", "vector": [0.8, 0.6], "matches": true});
    let fused = search(&l, &l.own, hybrid.clone()).await?;
    let lexical = search(&l, &l.own, asked(text)).await?;
    let by_text: BTreeMap<String, _> = marked(&lexical).into_iter().collect();
    let keys: BTreeSet<&str> = fused.hits.iter().map(|h| h.record.key.as_str()).collect();
    if !keys.is_superset(&BTreeSet::from(["d1", "d2", "d3", "d4"])) {
        wrong.push(format!("the hybrid search's hits: {keys:?}"));
    }
    for (key, matches) in marked(&fused) {
        let lexically = by_text.get(&key).cloned().unwrap_or_default();
        if matches != lexically {
            wrong.push(format!("{key}: {matches:?}; lexically {lexically:?}"));
        }
    }
    let mut unasked = hybrid;
    unasked["matches"] = json!(false);
    let plain = search(&l, &l.own, unasked).await?;
    if fused.axes != plain.axes || scored(&fused) != scored(&plain) {
        wrong.push(format!(
            "the axes: {:?}; unasked {:?}",
            fused.axes, plain.axes
        ));
    }
    ensure!(
        wrong.is_empty(),
        "a hybrid hit must report its text's matches whichever axis brought it: {wrong:?}"
    );
    Ok(())
}

async fn a_field_with_more_matches_than_the_cap_answers_the_cap_and_the_count(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(doc(true)).await?;
    let body = "invoice ".repeat(40);
    l.upsert("doc", "many", json!({"title": "invoices", "body": body}))
        .await?;
    let got = search(&l, &l.own, asked("invoice")).await?;
    let hit: Option<&Hit> = got.hits.first();
    let field = hit.and_then(|h| h.matches.get(&FieldName::from("body")));
    let want: Vec<Span> = (0..MATCH_SPANS as u32)
        .map(|i| Span {
            start: 8 * i,
            end: 8 * i + 7,
        })
        .collect();
    let title = hit.and_then(|h| h.matches.get(&FieldName::from("title")));
    ensure!(
        MATCH_SPANS == 32
            && field.is_some_and(|m| m.spans == want && m.count == 40)
            && title.is_some_and(|m| m.spans == [Span { start: 0, end: 8 }] && m.count == 1),
        "a field with more matches than the cap must answer the cap and the count: {:?}",
        hit.map(|h| &h.matches)
    );
    Ok(())
}

async fn matches_are_of_the_answered_record_at_the_read_position(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(doc(true)).await?;
    (store.bind_rerank(backing::reranker())).must("binding the reranker")?;
    l.upsert("doc", "moved", json!({"body": "invoice paid"}))
        .await?;
    l.upsert("doc", "late", json!({"body": "nothing yet"}))
        .await?;
    let then = store.head().await.must("reading the head")?;
    l.upsert("doc", "moved", json!({"body": "the paid invoice"}))
        .await?;
    l.upsert("doc", "late", json!({"body": "an invoice now"}))
        .await?;
    let mut wrong = Vec::new();
    let head = search(&l, &l.own, asked("invoice")).await?;
    let told = want(vec![
        ("late", spans(&[("body", &[(3, 10)], 1)])),
        ("moved", spans(&[("body", &[(9, 16)], 1)])),
    ]);
    if marked(&head) != told {
        wrong.push(format!("at the head: {:?}", marked(&head)));
    }
    let mut earlier = asked("invoice");
    earlier["as_of"] = json!(then);
    let told = want(vec![
        ("late", BTreeMap::new()),
        ("moved", spans(&[("body", &[(0, 7)], 1)])),
    ]);
    let got = search(&l, &l.own, earlier.clone()).await?;
    if marked(&got) != told {
        wrong.push(format!("as of {then:?}: {:?}", marked(&got)));
    }
    earlier["rerank"] = json!({"top": 2});
    let got = search(&l, &l.own, earlier).await?;
    let mut got = marked(&got);
    got.sort_by(|a, b| a.0.cmp(&b.0));
    if got != told {
        wrong.push(format!("reranked as of {then:?}: {got:?}"));
    }
    ensure!(
        wrong.is_empty(),
        "matches must be of the answered record at the read position: {wrong:?}"
    );
    Ok(())
}
