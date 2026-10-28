//! The source door (`docs/decisions/016-two-write-doors-by-origin.md`,
//! `docs/log-format.md` §3).
//!
//! `ingest` is content-addressed: a record delivered twice with the same
//! values appends nothing the second time, even within one delivery,
//! while the same key with new values is a genuine change and moves the
//! version. And the two doors keep the authority line: a source writes
//! only source fields, an author only authored ones, and a record or
//! patch that crosses the line is refused whole.

use crate::conformance::fixture::{committed, delivered, get_at, read, source_record};
use crate::conformance::prelude::*;

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
    "010, 016 and log-format §1.1: a record ingested with `upstream` names that lake and position on every field it sets, the event carries the member, and the member is not in the event's id; a later write of a field without it leaves that field without it"
);

check!(
    DELIVERY_NAMED = an_ingests_delivery_is_the_callers_or_the_stores_own,
    "log-format §1.1 and §3: every `record_ingested` of a call carries its delivery, the caller's id or else the store's own `d` and the position of the call's first event, and each window answers it; the id is not in the event's id, so the same values delivered again under another id append nothing; a call of several windows carries one id, and a window before its first event names none; an empty id, one holding a NUL, or one of the store's own form, is refused and appends nothing"
);

check!(
    A_SOURCE_PER_RECORD = one_ingest_carries_a_source_per_record,
    "016 and 039: one ingest may name a source per record, the call's where a record names none: each record's provenance holds its own, a redelivery under it is a duplicate, the same record under another source is a duplicate unchanged and a new event under it changed, and `purge_source` of one takes only its records"
);

check!(
    A_DELIVERY_PER_RECORD = one_ingest_carries_a_delivery_per_record,
    "016 and 065, extended (lakelands#1528): one call in `page` carries `k0` naming `page`, `k1` naming no delivery, `k2` naming `item-2` and `k3` and `k4` naming `item-3`: their events name `page`, `page`, `item-2`, `item-3` and `item-3`, alike at a `commit_window` of 1 and of 256; taking back `item-3` selects `k3` and `k4` alone, `[item-2, item-3]` three, `page` two, and the act on `item-3` leaves `k1` and `k2` and retires `k3` and `k4`; the same record under `item-5` and then `item-6` in one call is one event in `item-5` and a duplicate; a record naming `d7` or the empty id is refused `delivery_id_form` with its `index` and `from`, nothing of its window appended"
);

async fn one_ingest_carries_a_delivery_per_record(store: Store) -> Held {
    use lakelands_api::error::Case;
    use lakelands_api::write::Selection;
    use lakelands_api::{DeliveryId, IngestOpts, ingest_all_with};
    let l = Lake::of(store.as_ref());
    l.declare(feed()).await?;
    let r = |key: &str, delivery: Option<&str>| SourceRecord {
        delivery: delivery.map(DeliveryId::from),
        ..source_record("feed", key, json!({"body": key}))
    };
    let page = |window: u32| IngestOpts {
        delivery: Some("page".into()),
        commit_window: Some(window),
        ..IngestOpts::default()
    };
    let named = [
        ("k0", Some("page")),
        ("k1", None),
        ("k2", Some("item-2")),
        ("k3", Some("item-3")),
        ("k4", Some("item-3")),
    ];
    // Each run under keys of its own, so `all`'s records are its alone.
    for (window, source, of) in [(1, "one", "-1"), (256, "all", "")] {
        let before = store.head().await.must("reading the head")?;
        let key = |k: &str| format!("{k}{of}");
        let batch = named.iter().map(|(k, d)| r(&key(k), *d)).collect();
        let source = SourceId::from(source);
        let said = ingest_all_with(l.cat.as_ref(), &l.own, &source, page(window), batch);
        let what = format!("ingesting five records in three deliveries at a window of {window}");
        said.await.must(&what)?;
        let events = events_from(store.as_ref(), before.next()).await?;
        let got: Vec<_> = (events.into_iter())
            .filter_map(|e| match e.body {
                EventBody::RecordIngested { key, .. } => Some((key.0, e.delivery?.to_string())),
                _ => None,
            })
            .collect();
        let want: Vec<_> = (named.iter())
            .map(|(k, d)| (key(k), d.unwrap_or("page").to_owned()))
            .collect();
        ensure!(got == want, "{what}, the events name {got:?}, not {want:?}");
    }
    let all = SourceId::from("all");
    for (selection, want) in [
        (Selection::Delivery("item-3".into()), 2),
        (
            Selection::Deliveries(vec!["item-2".into(), "item-3".into()]),
            3,
        ),
        (Selection::Delivery("page".into()), 2),
    ] {
        let report = store.remove_values_preflight(&all, &selection).await;
        let report = report.must("the preflight")?;
        ensure!(
            report.events == want,
            "taking back {selection:?} selects {want} events: {report:?}"
        );
    }
    store
        .remove_values(&all, &Selection::Delivery("item-3".into()), false, None)
        .await
        .must("taking `item-3` back")?;
    for (key, live) in [("k1", true), ("k2", true), ("k3", false), ("k4", false)] {
        let got = l.cat.get(&l.own, &"feed".into(), get_at(key, None)).await;
        ensure!(
            got.is_ok() == live,
            "after taking `item-3` back, `{key}` read {got:?}"
        );
    }
    let before = store.head().await.must("reading the head")?;
    let twice = vec![r("k5", Some("item-5")), r("k5", Some("item-6"))];
    let windows = delivered(store.as_ref(), &l, "all", twice, "one record in two").await?;
    let events = events_from(store.as_ref(), before.next()).await?;
    let in_five = (events.iter()).all(|e| e.delivery == Some("item-5".into()));
    ensure!(
        committed(&windows) == (1, 1) && events.len() == 1 && in_five,
        "the same record under `item-5` then `item-6` is one event in `item-5` and a \
         duplicate: {windows:?}, {events:?}"
    );
    for id in ["d7", ""] {
        let before = store.head().await.must("reading the head")?;
        let batch = vec![r("k6", None), r("k7", Some(id))];
        let said = ingest_all_with(l.cat.as_ref(), &l.own, &all, page(256), batch).await;
        let what = format!("a record naming the delivery {id:?}");
        let e = refused_unmoved(store.as_ref(), before, &what, said).await?;
        let formed = match &e {
            Error::Ingested {
                index: 1,
                from: 0,
                error,
                ..
            } => crate::conformance::fixture::invalid_case(&**error).cloned(),
            _ => None,
        };
        ensure!(
            formed == Some(Case::DeliveryIdForm),
            "{what} is refused `delivery_id_form` at index 1 from 0: {e}"
        );
    }
    Ok(())
}

/// `feed`: a source's `body` and an author's `verdict`.
fn feed() -> TypeDecl {
    let body = sourced("body", StorageKind::Text);
    type_decl("feed", vec![body, field("verdict", StorageKind::Text)])
}

async fn one_ingest_carries_a_source_per_record(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(feed()).await?;
    let sources = [("k1", None), ("k2", Some("dev-a")), ("k3", Some("dev-b"))];
    let batch = || {
        let own = |(key, source): (&str, Option<&str>)| SourceRecord {
            source: source.map(SourceId::from),
            ..source_record("feed", key, json!({"body": key}))
        };
        sources.into_iter().map(own).collect::<Vec<_>>()
    };
    let what = "ingesting three sources in one call";
    let first = delivered(store.as_ref(), &l, "bank", batch(), what).await?;
    ensure!(committed(&first) == (3, 0), "{what} answered {first:?}");
    for (key, source) in sources {
        let r = read(l.cat.as_ref(), &l.own, "feed", key).await?;
        let got = r.meta.get("body").and_then(|m| m.source.clone());
        let want = SourceId::from(source.unwrap_or("bank"));
        ensure!(
            got.as_ref() == Some(&want),
            "`{key}`'s source is {got:?}, not {want}"
        );
    }
    let again = delivered(store.as_ref(), &l, "bank", batch(), "delivering them again").await?;
    ensure!(
        committed(&again) == (0, 3),
        "delivered again, they answered {again:?}"
    );
    // Under another source, `k1` unchanged appends nothing and keeps its
    // source; changed, it is a new event under that source.
    let under_c = |body: &str| {
        vec![SourceRecord {
            source: Some("dev-c".into()),
            ..source_record("feed", "k1", json!({ "body": body }))
        }]
    };
    for (body, want, source) in [("k1", (0, 1), "bank"), ("k1 again", (1, 0), "dev-c")] {
        let what = format!("delivering `k1` as {body:?} under `dev-c`");
        let got = delivered(store.as_ref(), &l, "bank", under_c(body), &what).await?;
        let r = read(l.cat.as_ref(), &l.own, "feed", "k1").await?;
        let held = r.meta.get("body").and_then(|m| m.source.clone());
        ensure!(
            committed(&got) == want && held == Some(SourceId::from(source)),
            "{what} answered {got:?} and left `k1`'s source {held:?}, not {source}"
        );
    }
    store
        .purge_source(&"dev-a".into(), None, None)
        .await
        .must("purging `dev-a`")?;
    let live: Vec<String> = all(l.cat.as_ref(), &l.own, "feed")
        .await?
        .into_iter()
        .map(|r| r.key.0)
        .collect();
    ensure!(
        live == ["k1", "k3"],
        "after purging `dev-a`, `feed` holds {live:?}"
    );
    Ok(())
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
    // A key or a type holding NUL is refused as `upsert` refuses it, alone
    // or among good records, never sent to the engine (lakelands#1030).
    let good = || source_record("feed", "k3", json!({"body": "text"}));
    let nul = || source_record("feed", "k\u{0}", json!({"body": "text"}));
    for window in [vec![nul()], vec![good(), nul()], vec![nul(), good()]] {
        let err = l.ingest("bank", window).await;
        ensure!(
            matches!(
                &err,
                Err(Error::Refused {
                    reason: Refusal::Invalid(..),
                    field: None
                })
            ),
            "a window holding a key with NUL is refused for its key; it answered {err:?}"
        );
    }
    let err = l.ingest(
        "bank",
        vec![source_record("fe\u{0}ed", "k3", json!({"body": "x"}))],
    );
    let err = err.await;
    ensure!(
        matches!(err, Err(Error::NotFound { .. })),
        "a type holding NUL is no type; it answered {err:?}"
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
        ingested.upstream.as_ref() == Some(&from) && ingested.id == id,
        "the ingest with an upstream must be an event carrying it, with the id its source, \
         type, key, version and patch make, which leaves `upstream` out; it carries {:?} with \
         id {}, and the id without the member is {}",
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

check!(
    REFUSAL_NAMES = an_ingest_refusal_names_its_record_and_where_its_window_began,
    "016, extended (lakelands#1043): a record an ingest refuses is named by the refusal, by its type and key as its source sent them, with its place in the stream and the place of the first record of its window, from which nothing was written while every window before it was: for a field the source is not the authority on, an empty record on a free key, a type its writer is not shown and a link past its relation's cardinality within the window, each the refusal it is without the stream, with `index`, `from` and `record` beside it on the wire"
);

async fn an_ingest_refusal_names_its_record_and_where_its_window_began(store: Store) -> Held {
    use crate::conformance::fixture::{limits, principal};
    use lakelands_api::PrincipalKind;
    let l = Lake::of(store.as_ref());
    l.declare(feed()).await?;
    l.declare_json(json!({"declare": "type", "name": "box", "fields": [{"name": "n", "type": {"kind": "text"}}],
        "relations": [{"name": "in", "target": "feed", "cardinality": "many_to_one", "authority": "source"}]}))
        .await?;
    l.upsert("box", "b1", json!({"n": "b"})).await?;
    let window = limits().commit_window_events as u64;
    let kept = principal(
        store.as_ref(),
        PrincipalKind::Agent,
        "kept",
        serde_json::from_value(
            json!({"default": "write", "ingest": true, "types": {"box": {"default": "none"}}}),
        )
        .unwrap(),
    );
    let link = |to: &str| {
        source_record(
            "sys.link.box.in",
            &format!("b1:{to}"),
            json!({"from": "b1", "to": to}),
        )
    };
    // Each refused record is the third after a full window: at
    // `window + 2`, its window begun at `window`.
    let cases = [
        (
            "an authored field",
            &l.own,
            source_record("feed", "x3", json!({"verdict": "no"})),
        ),
        (
            "an empty record on a free key",
            &l.own,
            source_record("feed", "x3", json!({})),
        ),
        (
            "a type its writer is not shown",
            &kept,
            source_record("box", "x3", json!({"n": "x"})),
        ),
        ("a second `in` from `b1` in the window", &l.own, link("y2")),
    ];
    for (round, (what, who, refused)) in cases.into_iter().enumerate() {
        let mut records: Vec<_> = (0..window)
            .map(|i| source_record("feed", &format!("r{round}-{i}"), json!({"body": "x"})))
            .collect();
        records.push(source_record(
            "feed",
            &format!("y{round}"),
            json!({"body": "y"}),
        ));
        records.push(match round {
            3 => link(&format!("y{round}")),
            _ => source_record("feed", &format!("z{round}"), json!({"body": "z"})),
        });
        let (ty, key) = (refused.ty.clone(), refused.key.clone());
        records.push(refused);
        let fed = lakelands_api::ingest_all(l.cat.as_ref(), who, &"wire".into(), records).await;
        let Err(e @ Error::Ingested { index, from, .. }) = &fed else {
            return Err(Failure::new(format!(
                "{what}: the stream must be refused naming the record; it answered {fed:?}"
            )));
        };
        let mut wire = serde_json::to_value(e).map_err(|x| Failure::new(x.to_string()))?;
        let inner = serde_json::to_value(e.unwrapped()).map_err(|x| Failure::new(x.to_string()))?;
        let record = json!({"type": ty.as_str(), "key": key.as_str()});
        let places = (wire
            .as_object_mut()
            .map(|m| (m.remove("index"), m.remove("from"), m.remove("record"))))
        .unwrap_or_default();
        let named = places
            .2
            .as_ref()
            .is_some_and(|r| r["type"] == record["type"] && r["key"] == record["key"]);
        ensure!(
            (*index, *from) == (window + 2, window)
                && wire == inner
                && named
                && places.0 == Some(json!(window + 2))
                && places.1 == Some(json!(window)),
            "{what}: the refusal must name `{ty}` `{key}` at {} from {window}, its own code and shape kept; it answered {e:?}",
            window + 2
        );
        // The window before it is written, its own window is not.
        let first = l
            .cat
            .get(
                &l.own,
                &"feed".into(),
                serde_json::from_value(json!({"key": format!("r{round}-0")})).unwrap(),
            )
            .await;
        let mate = l
            .cat
            .get(
                &l.own,
                &"feed".into(),
                serde_json::from_value(json!({"key": format!("y{round}")})).unwrap(),
            )
            .await;
        ensure!(
            first.is_ok() && mate.is_err(),
            "{what}: the first window must be written and the refused one not: {first:?}, {mate:?}"
        );
    }
    Ok(())
}

check!(
    OWN_WINDOW = a_delivery_may_ask_its_own_commit_window_up_to_the_lakes_ceiling,
    "016, extended (lakelands#1496): an ingest that asks its own `commit_window` is committed in windows of that many records: at 2 a refused record takes back only its own window, `from` its start, and at twice the lake's `commit_window_events` the lake's two windows are one; a window of 0 or above `limits.commit_window_max` is refused as `limit_range` on `commit_window`, naming the ceiling, and nothing is appended; a location whose `commit_window_events` is above its ceiling is refused at open, naming the setting",
    given
);

async fn a_delivery_may_ask_its_own_commit_window_up_to_the_lakes_ceiling(
    store: Store,
    given: Given,
) -> Held {
    use crate::conformance::fixture::limits;
    use lakelands_api::{IngestOpts, ingest_all_with};
    let l = Lake::of(store.as_ref());
    l.declare(feed()).await?;
    let (src, rec) = (SourceId::from("wire"), |k: String| {
        source_record("feed", &k, json!({"body": "x"}))
    });
    let at = |w: u32, records| {
        let opts = IngestOpts {
            commit_window: Some(w),
            ..IngestOpts::default()
        };
        ingest_all_with(l.cat.as_ref(), &l.own, &src, opts, records)
    };
    // At 2 the sixth record is refused: `s0`..`s3` are written, its own
    // window, from `s4`, is not.
    let mut small: Vec<_> = (0..5).map(|i| rec(format!("s{i}"))).collect();
    small.push(source_record("feed", "s5", json!({"verdict": "no"})));
    let fed = at(2, small).await;
    let (s3, s4) = (l.read("feed", "s3").await, l.read("feed", "s4").await);
    ensure!(
        matches!(
            fed,
            Err(Error::Ingested {
                index: 5,
                from: 4,
                ..
            })
        ) && s3.is_ok()
            && s4.is_err(),
        "at a window of 2 the refused sixth record takes back only its own window, from 4, \
         `s3` written and `s4` not; it answered {fed:?}, {s3:?}, {s4:?}"
    );
    let lake = limits().commit_window_events;
    let bulk = |p: &str| (0..2 * lake).map(|i| rec(format!("{p}{i}"))).collect();
    let own = at(2 * lake, bulk("b"))
        .await
        .must("ingesting at twice the lake's window")?;
    let theirs = l
        .ingest("wire", bulk("c"))
        .await
        .must("ingesting at the lake's window")?;
    ensure!(
        own.len() == 1 && theirs.len() == 2,
        "{} records commit in one window at twice the lake's window and in two at its own; \
         they answered {own:?} and {theirs:?}",
        2 * lake
    );
    let max = limits().commit_window_max;
    let head = store.head().await.must("reading the head")?;
    for w in [0, max + 1] {
        let what = format!("an ingest asking a window of {w}");
        let said = at(w, vec![rec("r".into())]).await;
        let e = refused_unmoved(store.as_ref(), head, &what, said).await?;
        ensure!(
            matches!(&e, Error::Refused { field: Some(f), .. } if f == "commit_window")
                && invalid_case(&e) == Some(&Case::LimitRange { max }),
            "{what} is refused on `commit_window` as `limit_range` naming {max}: {e:?}"
        );
    }
    let over = lakelands_api::location::Limits {
        commit_window_max: lake - 1,
        ..limits()
    };
    let err = (given.relimit)(store, Seams::default(), over).await.err();
    ensure!(
        matches!(&err, Some(e @ Error::Refused { field: Some(f), .. })
            if f == "limits.commit_window_events"
                && invalid_case(e) == Some(&Case::LimitRange { max: lake - 1 })),
        "a location whose `commit_window_events` is above its ceiling is refused at open, \
         naming the setting; it answered {err:?}"
    );
    Ok(())
}

check!(
    INSERT_ONLY = an_insert_only_type_is_written_once_by_its_source,
    "016 and 027, extended (lakelands#1044): a type marked `insert_only` takes a record from its source once: the same record again is a duplicate, other values for a key that holds a record, live or retired, or that the window wrote already, are refused as `written_once`, naming the record and appending nothing, and the pair cut after its first record on a signal is refused alike at its second, the first written and `from` 1 (lakelands#886), while an author still writes its authored fields; a key held from its writer answers as on an unmarked type; `describe_type` says the mark; declared on a type that holds records it is taken with no migration, its version moving and its schema hash not, and taken off, a key moves its version again"
);

async fn an_insert_only_type_is_written_once_by_its_source(store: Store) -> Held {
    use crate::conformance::fixture::{alike_renamed, principal, restrict};
    use lakelands_api::error::{Case, RecordName};
    let l = Lake::of(store.as_ref());
    let marked = |name: &str, once: bool| TypeDecl {
        insert_only: once,
        ..TypeDecl {
            name: name.into(),
            ..feed()
        }
    };
    l.declare(marked("feed", true)).await?;
    l.declare(marked("plain", false)).await?;
    // Each step: the records fed, and `w` written, `d` a duplicate, `x`
    // refused as written once naming the last record, appending nothing.
    let step = async |ty: &str, recs: &[(&str, &str)], want: char| -> Held {
        let records = recs
            .iter()
            .map(|(k, b)| source_record(ty, k, json!({"body": b})))
            .collect();
        let before = store.head().await.must("reading the head")?;
        let fed = lakelands_api::ingest_all(l.cat.as_ref(), &l.own, &"dev".into(), records).await;
        let last = recs.last().map(|(k, _)| *k).unwrap_or_default();
        let ok = match (&fed, want) {
            (Ok(w), 'w') => committed(w) == (recs.len() as u32, 0),
            (Ok(w), 'd') => committed(w) == (0, recs.len() as u32),
            (
                Err(
                    e @ Error::Ingested {
                        record: RecordName::Of { key, .. },
                        ..
                    },
                ),
                'x',
            ) => {
                key == last
                    && matches!(e.case().map(|s| &s.case), Some(Case::WrittenOnce { .. }))
                    && store.head().await.must("reading the head")? == before
            }
            _ => false,
        };
        ensure!(
            ok,
            "{ty} {recs:?} must answer `{want}`; it answered {fed:?}"
        );
        Ok(())
    };
    for (recs, want) in [
        (&[("r1", "a")][..], 'w'),
        (&[("r1", "a")], 'd'),
        (&[("r1", "b")], 'x'),
        (&[("w1", "a"), ("w1", "b")], 'x'),
    ] {
        step("feed", recs, want).await?;
    }
    // The pair cut on a signal answers alike: its second record refused,
    // the first written, as `from` tells (lakelands#886).
    let pair = ["a", "b"].map(|b| source_record("feed", "c1", json!({"body": b})));
    let fed = crate::conformance::fixture::fed(l.cat.as_ref(), &l.own, "dev", pair.into(), Some(1));
    let cut = match fed.await {
        (
            done,
            Some(Error::Ingested {
                index, from, error, ..
            }),
        ) => Some((
            committed(&done),
            index,
            from,
            error.case().map(|s| s.case.clone()),
        )),
        _ => None,
    };
    let once = Case::WrittenOnce {
        ty: "feed".into(),
        key: "c1".into(),
    };
    ensure!(
        cut == Some(((1, 0), 1, 1, Some(once))),
        "`c1` twice, cut after the first, writes the first and refuses the second as written \
         once, from 1; it answered {cut:?}"
    );
    // An author still writes its fields; a retired key stays written.
    l.upsert("feed", "r1", json!({"verdict": "fine"})).await?;
    l.retire("feed", "r1").await?;
    step("feed", &[("r1", "a")], 'x').await?;
    ensure!(
        l.describe_type("feed").await?.lineage.insert_only,
        "`describe_type` must say `feed` is insert-only"
    );
    // A held key answers a writer without `restricted` as on an unmarked type.
    let feeder = principal(
        store.as_ref(),
        lakelands_api::PrincipalKind::Agent,
        "feeder",
        serde_json::from_value(json!({"default": "write", "ingest": true})).unwrap(),
    );
    let mut held = Vec::new();
    for ty in ["feed", "plain"] {
        step(ty, &[("h1", "a")], 'w').await?;
        restrict(store.as_ref(), ty, "h1", "held").await?;
        let records = vec![source_record(ty, "h1", json!({"body": "b"}))];
        held.push(
            crate::conformance::fixture::ingest(l.cat.as_ref(), &feeder, "dev", records).await,
        );
    }
    match (&held[0], &held[1]) {
        (Err(m), Err(p)) => alike_renamed("a held key", m, p, ("feed", "plain"))?,
        _ => {
            return Err(Failure::new(format!(
                "a held key must be refused alike: {held:?}"
            )));
        }
    }
    // Marked while it holds `p1`, and the mark taken off again.
    step("plain", &[("p1", "a")], 'w').await?;
    let was = l.describe_type("plain").await?.lifecycle;
    l.declare(marked("plain", true)).await?;
    let is = l.describe_type("plain").await?.lifecycle;
    ensure!(
        (is.version, &is.schema_hash) == (was.version + 1, &was.schema_hash),
        "the mark must move `plain`'s version by one and not its hash: {was:?}, {is:?}"
    );
    step("plain", &[("p1", "b")], 'x').await?;
    l.declare(marked("plain", false)).await?;
    step("plain", &[("p1", "b")], 'w').await
}

async fn an_ingests_delivery_is_the_callers_or_the_stores_own(store: Store) -> Held {
    use lakelands_api::error::Case;
    use lakelands_api::event::EventBody;
    use lakelands_api::{DeliveryId, IngestOpts, ingest_all_with};
    let l = Lake::of(store.as_ref());
    l.declare(feed()).await?;
    let source = SourceId::from("sensor");
    let with = |delivery: Option<&str>| IngestOpts {
        delivery: delivery.map(DeliveryId::from),
        ..IngestOpts::default()
    };
    let r = |key: &str, body: &str| source_record("feed", key, json!({"body": body}));
    // Each call's events and its answer, as the log and the windows hold them.
    let call = async |delivery: Option<&str>, records: Vec<SourceRecord>| {
        let before = store.head().await.must("reading the head")?;
        let said = ingest_all_with(l.cat.as_ref(), &l.own, &source, with(delivery), records).await;
        let windows = said.must("ingesting with a delivery")?;
        let events = events_from(store.as_ref(), before.next()).await?;
        let ingested: Vec<_> = (events.into_iter())
            .filter(|e| matches!(e.body, EventBody::RecordIngested { .. }))
            .map(|e| (e.position, e.delivery))
            .collect();
        Ok::<_, Failure>((before, windows, ingested))
    };
    let (_, windows, ingested) = call(Some("batch-7"), vec![r("r1", "a"), r("r2", "b")]).await?;
    let named = Some(DeliveryId::from("batch-7"));
    ensure!(
        ingested.len() == 2
            && ingested.iter().all(|(_, d)| *d == named)
            && windows.iter().all(|w| w.delivery == named),
        "a call under `batch-7` carries it on each `record_ingested` and in each window; the \
         log holds {ingested:?}, the windows {windows:?}"
    );
    let (before, windows, ingested) = call(None, vec![r("r3", "c")]).await?;
    let own = Some(DeliveryId::from(format!("d{}", before.next())));
    ensure!(
        ingested.len() == 1
            && ingested.iter().all(|(_, d)| *d == own)
            && windows.iter().all(|w| w.delivery == own),
        "a call under no id carries the store's own {own:?}, the position of its first event; \
         the log holds {ingested:?}, the windows {windows:?}"
    );
    // A call of many windows, the first of them all duplicates: one id,
    // from the first event the call appended, and none before it.
    let bulk = |n: usize| (0..n).map(|i| r(&format!("b{i}"), "e")).collect::<Vec<_>>();
    let window = limits().commit_window_events.max(1) as usize;
    call(Some("bulk"), bulk(window)).await?;
    let (before, windows, ingested) = call(None, bulk(3 * window + 4)).await?;
    let own = Some(DeliveryId::from(format!("d{}", before.next())));
    let (empty, appended): (Vec<_>, Vec<_>) = windows.iter().partition(|w| w.records == 0);
    ensure!(
        appended.len() >= 2
            && ingested.len() == 2 * window + 4
            && ingested.iter().all(|(_, d)| *d == own)
            && appended.iter().all(|w| w.delivery == own)
            && empty.iter().all(|w| w.delivery.is_none()),
        "a call of several windows under no id carries one id, {own:?}, from its first event, \
         and a window before that event names none; the log holds {} events, the windows \
         {windows:?}",
        ingested.len()
    );
    let (before, windows, _) = call(Some("batch-8"), vec![r("r1", "a")]).await?;
    head_unmoved(store.as_ref(), before, "the same values under `batch-8`").await?;
    ensure!(
        windows.iter().map(|w| w.duplicates).sum::<u32>() == 1,
        "the same values delivered again under another id are a duplicate: {windows:?}"
    );
    for id in ["", "d12", "a\0b"] {
        let before = store.head().await.must("reading the head")?;
        let what = format!("ingesting under the id {id:?}");
        let said = ingest_all_with(
            l.cat.as_ref(),
            &l.own,
            &source,
            with(Some(id)),
            vec![r("r4", "d")],
        );
        let e = refused_unmoved(store.as_ref(), before, &what, said.await).await?;
        ensure!(
            matches!(
                crate::conformance::fixture::invalid_case(&e),
                Some(Case::DeliveryIdForm)
            ),
            "{what} is refused as `delivery_id_form`: {e}"
        );
    }
    Ok(())
}

check!(
    REFERENCE_WITH_NUL = an_ingested_reference_holding_nul_is_refused_on_its_field,
    "lakelands#1167: a reference an ingested record writes that holds U+0000 is refused naming the field, alone or among good records, never sent to the engine, as a key holding it is refused"
);

async fn an_ingested_reference_holding_nul_is_refused_on_its_field(store: Store) -> Held {
    use crate::conformance::fixture::reference;
    let l = Lake::of(store.as_ref());
    l.declare(feed()).await?;
    let r = lakelands_api::ontology::FieldDecl {
        authority: lakelands_api::ontology::Authority::Source,
        ..reference("r", "feed")
    };
    l.declare(type_decl("link", vec![r])).await?;
    let nul = || source_record("link", "l1", json!({"r": "a\u{0}b"}));
    let good = || source_record("feed", "k1", json!({"body": "text"}));
    for window in [vec![nul()], vec![good(), nul()], vec![nul(), good()]] {
        let err = l.ingest("bank", window).await;
        ensure!(
            matches!(refused_on(&err), Some(("r", Refusal::Constraint(..)))),
            "a reference holding NUL is refused on `r` as a text holding NUL; it answered {err:?}"
        );
    }
    Ok(())
}
