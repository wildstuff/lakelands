//! The source door (`docs/decisions/016-two-write-doors-by-origin.md`,
//! `docs/log-format.md` §3).
//!
//! `ingest` is content-addressed: a record delivered twice with the same
//! values appends nothing the second time, even within one delivery,
//! while the same key with new values is a genuine change and moves the
//! version. And the two doors keep the authority line: a source writes
//! only source fields, an author only authored ones, and a record or
//! patch that crosses the line is refused whole.

use lakelands_api::ontology::{StorageKind, TypeDecl};
use lakelands_api::{Error, Refusal, TypeName, Version};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, committed, delivered, events_from, field, get_at, head_unmoved, log_events,
    refused_on, refused_unmoved, source_record, sourced, try_upsert, type_decl, value,
    windows_reach_head,
};
use crate::conformance::{Failure, Held, Store};

check!(
    SAME_CONTENT_TWICE = the_same_content_ingested_twice_appends_nothing_the_second_time,
    "016 and the log format: ingest is content-addressed, so a redelivery of the same values appends nothing, within one delivery or across two; a record with no field counts as a duplicate for a live key, and is invalid for a new one (016, extended)"
);

check!(
    SAME_KEY_TWICE = the_same_key_twice_in_one_ingest_with_new_values_advances_the_version,
    "the log format: every event that changes a record moves its version by one, including two in one delivery"
);

check!(
    FOREIGN_FIELD_REFUSED =
        an_ingested_record_carrying_a_field_the_source_does_not_write_is_refused_whole,
    "016 and the log format: ingest writes only source and shared fields; a record carrying any other is refused whole, never trimmed, and a record whose key is empty is refused as invalid before its fields are judged, as `upsert` orders it, after the ingest grant and the type; a field another authority writes is refused before any value is judged (016, extended)"
);

check!(
    UPSERT_REFUSES_SOURCE_FIELD =
        an_upsert_carrying_a_field_only_a_source_writes_is_refused_as_the_wrong_door,
    "016 and 015: an author never overwrites a figure the source is the authority on; the patch is refused as the wrong door"
);

check!(
    REVERT_IS_A_CHANGE = ingest_skips_redeliveries_and_unchanged_records_but_writes_a_revert,
    "016 and the log format: an ingest that changes nothing appends nothing, and a record set back to an earlier value is a new change, not a redelivery"
);

check!(
    UPSTREAM_PER_FIELD = an_ingested_record_carries_its_upstream_per_field,
    "010, 016 and log-format §1.1 and §6: a record ingested with `upstream` names that lake and position on every field it sets, the event is written in format version 3, and the member is not in the event's id; a later write of a field without it leaves that field without it"
);

/// `feed`: a source's `body` and an author's `verdict`.
fn feed() -> TypeDecl {
    let body = sourced("body", StorageKind::Text);
    type_decl("feed", vec![body, field("verdict", StorageKind::Text)])
}

async fn the_same_content_ingested_twice_appends_nothing_the_second_time(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(feed()).await?;

    let once = || vec![source_record("feed", "k1", json!({"body": "unchanged"}))];
    let first = delivered(store.as_ref(), &l, "bank", once(), "ingesting `k1`").await?;
    ensure!(
        committed(&first) == (1, 0),
        "the first delivery of `k1` must commit 1 record and 0 duplicates; the windows \
         answered {first:?}"
    );
    let head = store.head().await.must("reading the head")?;
    let what = "ingesting `k1` again with the same values";
    let second = delivered(store.as_ref(), &l, "bank", once(), what).await?;
    ensure!(
        committed(&second) == (0, 1),
        "delivering `k1` again with the same values must commit 0 records and 1 \
         duplicate; the windows answered {second:?}"
    );
    let after = store.head().await.must("reading the head")?;
    ensure!(
        after == head,
        "the redelivery moved the head from {head} to {after}; the same content appends \
         nothing"
    );
    // The duplicate test before an empty record (016, extended): for a live
    // key it changes nothing and counts; for a new key it is invalid.
    let empty = |k: &str| vec![source_record("feed", k, json!({}))];
    let live = delivered(store.as_ref(), &l, "bank", empty("k1"), "an empty `k1`").await?;
    let new = l.ingest("bank", empty("k9")).await;
    ensure!(
        committed(&live) == (0, 1)
            && matches!(
                new,
                Err(Error::Refused {
                    reason: Refusal::Invalid(..),
                    ..
                })
            ),
        "a record with no field counts as a duplicate for the live `k1`, answered {live:?}, \
         and is refused as invalid for the new `k9`, answered {new:?}"
    );

    let twice = vec![
        source_record("feed", "k2", json!({"body": "same"})),
        source_record("feed", "k2", json!({"body": "same"})),
    ];
    let what = "ingesting `k2` twice in one delivery";
    let windows = delivered(store.as_ref(), &l, "bank", twice, what).await?;
    ensure!(
        committed(&windows) == (1, 1),
        "one delivery carrying `k2` twice with the same values must commit 1 record and \
         1 duplicate; the windows answered {windows:?}"
    );
    for key in ["k1", "k2"] {
        let rec = l.read("feed", key).await?;
        ensure!(
            rec.version == Version(1),
            "`{key}` was delivered twice with the same values and must stay at version 1; \
             it is at version {}",
            rec.version
        );
    }
    Ok(())
}

async fn the_same_key_twice_in_one_ingest_with_new_values_advances_the_version(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(feed()).await?;

    let delivery = vec![
        source_record("feed", "k1", json!({"body": "first draft"})),
        source_record("feed", "k1", json!({"body": "second draft"})),
    ];
    let what = "ingesting `k1` twice with different values";
    let windows = delivered(store.as_ref(), &l, "bank", delivery, what).await?;
    ensure!(
        committed(&windows) == (2, 0),
        "two different values for `k1` are two changes, not a duplicate: the windows must \
         commit 2 records and 0 duplicates; they answered {windows:?}"
    );
    let rec = l.read("feed", "k1").await?;
    ensure!(
        value(&rec, "body") == Some(&json!("second draft")) && rec.version == Version(2),
        "the last record of the delivery stands, at the version after the first one's: \
         `k1` must read \"second draft\" at version 2, and reads {:?} at version {}",
        value(&rec, "body"),
        rec.version
    );
    Ok(())
}

async fn an_ingested_record_carrying_a_field_the_source_does_not_write_is_refused_whole(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(feed()).await?;

    let delivery = vec![source_record(
        "feed",
        "k1",
        json!({"body": "text", "verdict": "approved"}),
    )];
    let before = store.head().await.must("reading the head")?;
    let what = "ingesting a record that carries the authored field `verdict`";
    let answer = l.ingest("bank", delivery).await;
    let err = refused_unmoved(store.as_ref(), before, what, answer).await?;
    ensure!(
        matches!(
            &err,
            Error::Refused { reason: Refusal::WrongDoor, field }
                if field.as_ref().is_none_or(|f| f.as_str() == "verdict")
        ),
        "a source record carrying the authored field `verdict` must be refused as the \
         wrong door (naming `verdict` if it names a field); it failed with {err:?}"
    );
    let missing = cat
        .get(own, &TypeName::from("feed"), get_at("k1", None))
        .await;
    ensure!(
        matches!(missing, Err(Error::NotFound { .. })),
        "the refused record was written in part: `k1` reads {missing:?}. A record \
         carrying a field the source does not write is refused whole, never trimmed"
    );
    let keyless = vec![source_record(
        "feed",
        "",
        json!({"body": "text", "verdict": "approved"}),
    )];
    let err = l.ingest("bank", keyless).await;
    ensure!(
        matches!(
            &err,
            Err(Error::Refused {
                reason: Refusal::Invalid(..),
                field: None
            })
        ),
        "a source record with an empty key that carries `verdict` is refused for its key \
         first, as invalid naming no field; it answered {err:?}"
    );
    // The grant, then the type, then (after the key) the door before the
    // values, whatever the fields' order (016, extended).
    let no_ingest = lakelands_api::Principal {
        grants: lakelands_api::Grants {
            ingest: false,
            ..lakelands_api::Grants::owner()
        },
        ..own.clone()
    };
    let nowhere = || vec![source_record("nothing", "", json!({"body": "x"}))];
    let ungranted =
        crate::conformance::fixture::ingest(cat.as_ref(), &no_ingest, "bank", nowhere()).await;
    let untyped = l.ingest("bank", nowhere()).await;
    let door = l.ingest(
        "bank",
        vec![source_record(
            "feed",
            "k2",
            json!({"body": 5, "verdict": "x"}),
        )],
    );
    let door = door.await;
    ensure!(
        matches!(&ungranted, Err(e) if matches!(e, Error::Refused { reason: Refusal::NotGranted, .. }))
            && matches!(untyped, Err(Error::NotFound { .. }))
            && matches!(refused_on(&door), Some(("verdict", Refusal::WrongDoor))),
        "ingest refuses for its grant, then the type, then the door before the values; it \
         answered {ungranted:?}, {untyped:?} and {door:?}"
    );
    head_unmoved(
        store.as_ref(),
        before,
        "ingesting a record with an empty key",
    )
    .await
}

async fn an_upsert_carrying_a_field_only_a_source_writes_is_refused_as_the_wrong_door(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(feed()).await?;

    let before = store.head().await.must("reading the head")?;
    let fields = json!({"body": "typed in", "verdict": "approved"});
    let answer = try_upsert(cat.as_ref(), own, "feed", "k1", fields).await;
    let what = "the owner upserting the source field `body`";
    let err = refused_unmoved(store.as_ref(), before, what, answer).await?;
    ensure!(
        matches!(refused_on(&err), Some(("body", Refusal::WrongDoor))),
        "an upsert carrying `body`, which only a source writes, must be refused as the \
         wrong door on `body`, even for the owner; it failed with {err:?}"
    );
    let missing = cat
        .get(own, &TypeName::from("feed"), get_at("k1", None))
        .await;
    ensure!(
        matches!(missing, Err(Error::NotFound { .. })),
        "the refused upsert was applied in part: `k1` reads {missing:?}"
    );
    Ok(())
}

async fn ingest_skips_redeliveries_and_unchanged_records_but_writes_a_revert(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(feed()).await?;
    let body = |b: &str| vec![source_record("feed", "f", json!({ "body": b }))];

    for (step, b, want) in [
        ("the first delivery of `one`", "one", (1, 0)),
        ("a redelivery of `one`", "one", (0, 1)),
        ("a change to `two`", "two", (1, 0)),
        ("a revert to `one`", "one", (1, 0)),
        ("`one` again, unchanged", "one", (0, 1)),
    ] {
        let windows = l.ingest("wire", body(b));
        let windows = windows.await.must(&format!("ingesting {step}"))?;
        windows_reach_head(store.as_ref(), &windows, step).await?;
        let got = committed(&windows);
        ensure!(
            got == want,
            "{step} must commit {} record(s) and count {} as already in the lake; it \
             committed {} and counted {}",
            want.0,
            want.1,
            got.0,
            got.1
        );
    }
    let rec = l.read("feed", "f").await?;
    ensure!(
        value(&rec, "body") == Some(&json!("one")) && rec.version == Version(3),
        "after one, two and back to one, `f` must read `one` at version 3; it reads \
         {:?} at version {}",
        value(&rec, "body"),
        rec.version
    );
    Ok(())
}

check!(
    DUPLICATE_ACROSS_WINDOWS = a_record_delivered_twice_in_two_windows_is_written_once,
    "016 and the log format: ingest is content-addressed across windows too, so a record delivered again in a later window of the same ingest appends nothing and the feed holds one event"
);

async fn a_record_delivered_twice_in_two_windows_is_written_once(store: Store) -> Held {
    use crate::conformance::fixture::limits;
    use lakelands_api::event::EventBody;
    let l = Lake::of(store.as_ref());
    l.declare(feed()).await?;
    let from = store.head().await.must("reading the head")?.next();
    let window = limits().commit_window_events as usize;
    let mut records = vec![source_record("feed", "twice", json!({"body": "same"}))];
    for i in 0..window {
        records.push(source_record(
            "feed",
            &format!("filler-{i}"),
            json!({"body": "x"}),
        ));
    }
    records.push(source_record("feed", "twice", json!({"body": "same"})));
    let windows = l
        .ingest("wire", records)
        .await
        .must("ingesting `twice` at the start of one window and again in the next")?;
    windows_reach_head(store.as_ref(), &windows, "the two-window ingest").await?;
    ensure!(
        windows.len() >= 2 && committed(&windows) == (window as u32 + 1, 1),
        "the ingest must span two windows, committing {} records and counting the \
         second `twice` as already there; it answered {} window(s) and {:?}",
        window + 1,
        windows.len(),
        committed(&windows)
    );
    let events = events_from(store.as_ref(), from).await?;
    let twice = events
        .iter()
        .filter(
            |e| matches!(&e.body, EventBody::RecordIngested { key, .. } if key.as_str() == "twice"),
        )
        .count();
    ensure!(
        twice == 1,
        "the feed must hold one event for `twice`; it holds {twice}"
    );
    Ok(())
}

async fn an_ingested_record_carries_its_upstream_per_field(store: Store) -> Held {
    use lakelands_api::{Position, Upstream};
    let l = Lake::of(store.as_ref());
    let mut reading = feed();
    let unit = sourced("unit", StorageKind::Text);
    reading.fields.push(unit);
    l.declare(reading).await?;
    let from = Upstream {
        lake: "upstream-lake".into(),
        position: Position(42),
    };
    let mut first = source_record("feed", "r1", json!({"body": "warm", "unit": "C"}));
    first.upstream = Some(from.clone());
    let patch_first = first.fields.clone();
    let fut = l.ingest("mirror", vec![first]);
    fut.await.must("ingesting `r1` with an upstream")?;
    let rec = l.read("feed", "r1").await?;
    for f in ["body", "unit"] {
        let got = rec.meta.get(f).and_then(|m| m.upstream.clone());
        ensure!(
            got.as_ref() == Some(&from),
            "`r1`'s `{f}` was ingested with upstream {from:?} and must carry it; it carries \
             {got:?}"
        );
    }
    let events = log_events(store.as_ref()).await?;
    let ingested = events
        .iter()
        .rev()
        .find(|e| {
            matches!(
                e.body,
                lakelands_api::event::EventBody::RecordIngested { .. }
            )
        })
        .ok_or_else(|| Failure::new("the log holds no `record_ingested` event"))?;
    let id = crate::log::ingest_id(
        &"mirror".into(),
        &TypeName::from("feed"),
        &"r1".into(),
        0,
        &patch_first,
    );
    ensure!(
        ingested.v >= 3 && ingested.upstream.as_ref() == Some(&from) && ingested.id == id,
        "the ingest with an upstream must be an event of format 3 or newer carrying it, with \
         the id its source, type, key, version and patch make, which leaves `upstream` out; it \
         is format \
         {} carrying {:?} with id {}, and the id without the member is {}",
        ingested.v,
        ingested.upstream,
        ingested.id.0,
        id.0
    );
    // A later ingest of one field without an upstream.
    l.ingest(
        "mirror",
        vec![source_record("feed", "r1", json!({"unit": "F"}))],
    )
    .await
    .must("ingesting `r1`'s unit without an upstream")?;
    let rec = l.read("feed", "r1").await?;
    let (body, unit) = (
        rec.meta.get("body").and_then(|m| m.upstream.clone()),
        rec.meta.get("unit").and_then(|m| m.upstream.clone()),
    );
    ensure!(
        body.as_ref() == Some(&from) && unit.is_none(),
        "after `unit` was ingested again without an upstream, `body` must keep {from:?} and \
         `unit` carry none; they carry {body:?} and {unit:?}"
    );
    // An authored write over a field leaves it without one.
    let mut stamped = source_record("feed", "r2", json!({"body": "cold"}));
    stamped.upstream = Some(from.clone());
    let fut = l.ingest("mirror", vec![stamped]);
    fut.await.must("ingesting `r2` with an upstream")?;
    l.upsert("feed", "r2", json!({"verdict": "ok"})).await?;
    let rec = l.read("feed", "r2").await?;
    let (body, verdict) = (
        rec.meta.get("body").and_then(|m| m.upstream.clone()),
        rec.meta.get("verdict").and_then(|m| m.upstream.clone()),
    );
    ensure!(
        body.as_ref() == Some(&from) && verdict.is_none(),
        "an authored write of `verdict` must carry no upstream and leave `body`'s; they \
         carry {verdict:?} and {body:?}"
    );
    Ok(())
}
