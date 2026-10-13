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
    "010, 016 and log-format §1.1: a record ingested with `upstream` names that lake and position on every field it sets, the event carries the member, and the member is not in the event's id; a later write of a field without it leaves that field without it"
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
    INSERT_ONLY = an_insert_only_type_is_written_once_by_its_source,
    "016 and 027, extended (lakelands#1044): a type marked `insert_only` takes a record from its source once: the same record again is a duplicate, other values for a key that holds a record, live or retired, or that the window wrote already, are refused as `written_once`, naming the record and appending nothing, while an author still writes its authored fields; a key held from its writer answers as on an unmarked type; `describe_type` says the mark; declared on a type that holds records it is taken with no migration, its version moving and its schema hash not, and taken off, a key moves its version again"
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
