//! Part of a source is taken back in one act
//! (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`): what it
//! wrote before a time, or in one delivery, reads as erased at every
//! position, a record left holding nothing of another hand is retired,
//! and a record another hand also wrote keeps that hand's values.

use lakelands_api::event::{Event, EventBody};
use lakelands_api::write::{RemoveReport, Selection};
use lakelands_api::{DeliveryId, IngestOpts, LakeStore, Position, SourceId, Timestamp};
use serde_json::{Value, json};

use crate::conformance::fixture::{
    Lake, Must, count_view, events_after, events_from, get_at, refused_like, source_record,
};
use crate::conformance::{Failure, Held, Store};

check!(
    REMOVED_BEFORE = a_sources_values_before_a_time_read_as_erased_at_every_position,
    "065: `remove_values_preflight` of what `tracker` wrote before the first event of the second day names the events selected and the first position it does not reach, the records holding a value it takes, the values per field, the two records `tracker` alone wrote as retired and the one with an author's `note` as kept, the derived field and the kept view computed again, and the record left breaking an admitting constraint; `remove_values` with that report appends one `values_removed` and the two retirements; after it the kept record holds its `note` and no `duration` nor what is derived from it, its provenance saying `erased` and naming `tracker`, at the head and `as_of` before the act, the second day's records keep theirs, and a rebuild answers the same; a field that is not erasable is taken all the same; a stale report is refused, and so is a time past the year 9999; a repeat appends nothing, and a selection of nothing appends nothing"
);

check!(
    REMOVED_DELIVERY = a_delivery_taken_back_reads_as_erased_and_a_later_one_stays,
    "065: `remove_values` of one delivery, by the caller's id or by the store's own, takes what its events wrote and nothing else: a record the same source wrote again in a later delivery keeps those values from that event on and reads as erased before it, a record only that delivery wrote is retired, a redelivery of the same values under another id appended nothing and so selects nothing, and a later delivery under the same id stays"
);

check!(
    REMOVE_CRASH = a_removal_a_crash_stopped_is_finished_by_the_next_writers_open,
    "065: a removal stopped after its event, before its rebuild, is finished by the writer's next open, which rebuilds knowing the act and reclaims, and by the act's repeat on the same handle, each appending no second event; the values read as erased at every position after either",
    given
);

check!(
    REMOVED_TWINS = a_value_taken_from_a_hidden_field_is_told_as_one_never_written,
    "065 (reader's twins): in a lake where `tracker` also wrote `secret`, a field kept from a reader, and in its twin where it never did, the act takes the same records back; that reader is answered alike by `get` of each record at every position of the log, by `find` with its total at every position, filtered on `duration is null`, and by `describe`, while the owner reads `secret`'s provenance as erased in the one lake alone",
    given
);

/// `visit` keyed by `id`: `duration`, `place` and `secret` from a source,
/// `note` an
/// author's, `twice` derived from `duration`, the admitting constraint `timed`, and
/// the kept view `by_duration`.
async fn visits(l: &Lake) -> Held {
    let text = json!({"kind": "text"});
    let number = json!({"kind": "number"});
    l.declare_json(json!({"declare": "type", "name": "visit", "key": ["id"],
        "fields": [{"name": "id", "type": text, "authority": "source"},
                   {"name": "duration", "type": number, "authority": "source"},
                   {"name": "place", "type": text, "authority": "source"},
                   {"name": "secret", "type": number, "authority": "source"},
                   {"name": "note", "type": text},
                   {"name": "twice", "type": number, "authority": "derived",
                    "derived": "duration * 2"}],
        "constraints": [{"name": "timed", "holds": "duration > 0", "at_ingest": "admit"}]}))
        .await?;
    l.declare(count_view("by_duration", "visit", "duration"))
        .await?;
    l.set("view_kept.by_duration", json!(true)).await?;
    Ok(())
}

/// `tracker`'s visits `keys`, each with `duration`.
async fn tracked(l: &Lake, visits: &[(&str, u32)]) -> Held {
    let records = (visits.iter())
        .map(|(k, d)| source_record("visit", k, json!({"id": k, "duration": d})))
        .collect();
    l.ingest("tracker", records)
        .await
        .must("ingesting from tracker")?;
    Ok(())
}

/// What a report names, for comparison.
fn shape(r: &RemoveReport) -> Value {
    json!({
        "events": r.events,
        "types": r.types.iter().map(|t| json!([t.ty, t.records])).collect::<Vec<_>>(),
        "fields": r.fields.iter().map(|f| json!([f.ty, f.field, f.redacted])).collect::<Vec<_>>(),
        "retired": r.retired.iter().map(|t| json!([t.ty, t.retired])).collect::<Vec<_>>(),
        "kept": r.kept.iter().map(|t| json!([t.ty, t.first])).collect::<Vec<_>>(),
        "recomputed": r.recomputed.iter().map(|f| json!(f.field)).collect::<Vec<_>>(),
        "views": r.views,
        "breaches": r.breaches.iter().map(|b| json!([b.constraint, b.sample_keys])).collect::<Vec<_>>(),
    })
}

/// `key`'s fields of `ty` as of `at` (the head when `None`), with
/// provenance; `None` where it is not found.
async fn at(
    l: &Lake,
    ty: &str,
    key: &str,
    at: Option<Position>,
) -> Result<Option<lakelands_api::Record>, Failure> {
    match l.cat.get(&l.own, &ty.into(), get_at(key, at)).await {
        Ok(r) => Ok(Some(r.data)),
        Err(lakelands_api::Error::NotFound { .. }) => Ok(None),
        Err(e) => Err(Failure::new(format!("reading `{key}` as of {at:?}: {e}"))),
    }
}

/// Whether `r` holds no value of `field` and its provenance says it was
/// erased, from `source`.
fn erased(r: &lakelands_api::Record, field: &str, source: &str) -> bool {
    let m = r.meta.get(field);
    !r.fields.contains_key(field)
        && m.is_some_and(|m| m.erased && m.source.as_ref().is_some_and(|s| s == source))
}

/// The `values_removed` events after `after`.
async fn acts(store: &dyn LakeStore, after: Position) -> Result<Vec<Event>, Failure> {
    let events = events_after(store, after).await?;
    let act = |e: &Event| matches!(e.body, EventBody::ValuesRemoved { .. });
    Ok(events.into_iter().filter(act).collect())
}

/// What the owner reads of `visit` `v1`..`v6` at `at`: each record's
/// fields, or none.
async fn read_all(l: &Lake, when: Option<Position>) -> Result<Vec<Value>, Failure> {
    let mut all = Vec::new();
    for k in ["v1", "v2", "v3", "v4", "v5", "v6"] {
        let r = at(l, "visit", k, when).await?;
        all.push(json!(r.map(|r| r.fields)));
    }
    Ok(all)
}

async fn a_sources_values_before_a_time_read_as_erased_at_every_position(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    visits(&l).await?;
    tracked(&l, &[("v1", 10), ("v2", 20), ("v3", 30)]).await?;
    l.upsert("visit", "v3", json!({"note": "late"})).await?;
    // The second day begins after the first's last `known_at`.
    std::thread::sleep(std::time::Duration::from_millis(3));
    let first_day = store.head().await.must("reading the head")?;
    tracked(&l, &[("v4", 40), ("v5", 50), ("v6", 60)]).await?;
    let second = events_from(store.as_ref(), first_day.next()).await?;
    let day2 = second
        .first()
        .ok_or_else(|| Failure::new("the second day was logged"))?;
    let (cut, p) = (day2.known_at, day2.position);
    let tracker: SourceId = "tracker".into();
    let selection = Selection::WrittenBefore(cut);
    let report = store.remove_values_preflight(&tracker, &selection).await;
    let report = report.must("the preflight")?;
    let want = json!({
        "events": 3, "types": [["visit", 3]], "fields": [["visit", "duration", 3]],
        "retired": [["visit", 2]], "kept": [["visit", ["v3"]]], "recomputed": ["twice"],
        "views": ["by_duration"], "breaches": [["timed", ["v3"]]],
    });
    ensure!(
        shape(&report) == want && report.before == Some(p),
        "the preflight names {want}, and position {p} as the first it does not reach; it \
         names {} in {report:?}",
        shape(&report)
    );
    let said = report.said();
    ensure!(
        said.iter()
            .any(|s| s.starts_with("3 values of `visit.duration`"))
            && said
                .iter()
                .any(|s| s.starts_with("2 records of `visit` are retired")),
        "the report says in sentences what it takes and retires: {said:?}"
    );
    ensure!(
        report.log_events > 0 && !report.report_id.is_empty() && report.reclaimed.is_none(),
        "the preflight counts the log's events, carries an id and reclaims nothing: {report:?}"
    );
    // A report of another head is refused.
    l.upsert("visit", "v4", json!({"note": "moved"})).await?;
    refused_like(
        "removing with a report of another head",
        store
            .remove_values(&tracker, &selection, Some(&report.report_id))
            .await,
        &json!({"code": "remove_report_stale"}),
    )?;
    let report = store.remove_values_preflight(&tracker, &selection).await;
    let report = report.must("the preflight again")?;
    let before = store.head().await.must("reading the head")?;
    let done = store
        .remove_values(&tracker, &selection, Some(&report.report_id))
        .await;
    let done = done.must("removing tracker's first day")?;
    let events = events_after(store.as_ref(), before).await?;
    let kinds: Vec<&str> = events.iter().map(|e| e.body.kind()).collect();
    ensure!(
        kinds == ["values_removed", "record_retired", "record_retired"]
            && shape(&done) == shape(&report)
            && done.reclaimed.is_some(),
        "the act appends `values_removed` and two retirements, answers its preflight's report \
         and reclaims; it appended {kinds:?} and answered {done:?}"
    );
    // At the head and before the act, the first day reads as erased.
    for when in [None, Some(before), Some(p)] {
        let v3 = at(&l, "visit", "v3", when)
            .await?
            .ok_or_else(|| Failure::new("v3 stays"))?;
        ensure!(
            erased(&v3, "duration", "tracker")
                && !v3.fields.contains_key("twice")
                && v3.fields.get("note") == Some(&json!("late")),
            "as of {when:?}, `v3` keeps its note and holds no `duration` nor `twice`, \
             `duration` saying it was erased from tracker: {v3:?}"
        );
        let v4 = at(&l, "visit", "v4", when).await?;
        ensure!(
            v4.as_ref()
                .is_some_and(|r| r.fields.get("duration") == Some(&json!(40))),
            "as of {when:?}, `v4` of the second day keeps its duration: {v4:?}"
        );
    }
    let v1 = at(&l, "visit", "v1", Some(before)).await?;
    ensure!(
        v1.as_ref()
            .is_some_and(|r| erased(r, "duration", "tracker")),
        "as of before the act, `v1` reads its duration as erased: {v1:?}"
    );
    let gone = (
        at(&l, "visit", "v1", None).await?,
        at(&l, "visit", "v2", None).await?,
    );
    ensure!(
        gone.0.is_none() && gone.1.is_none(),
        "`v1` and `v2`, tracker's alone, are retired: {gone:?}"
    );
    let head = store.head().await.must("reading the head")?;
    let answers = (read_all(&l, None).await?, read_all(&l, Some(before)).await?);
    store.rebuild().await.must("rebuilding")?;
    let again = (read_all(&l, None).await?, read_all(&l, Some(before)).await?);
    ensure!(
        answers == again,
        "a rebuild answers as the act left the lake: {answers:?}, then {again:?}"
    );
    // The act again appends nothing; so does a selection of nothing.
    let repeat = store.remove_values(&tracker, &selection, None).await;
    repeat.must("removing tracker's first day again")?;
    let none = Selection::WrittenBefore(Timestamp(0));
    refused_like(
        "removing what tracker wrote before a time past the year 9999",
        store
            .remove_values(
                &tracker,
                &Selection::WrittenBefore(Timestamp(i64::MAX)),
                None,
            )
            .await,
        &json!({"code": "not_date_time"}),
    )?;
    let empty = store.remove_values(&tracker, &none, None).await;
    let empty = empty.must("removing what tracker wrote before 1970")?;
    let after = store.head().await.must("reading the head")?;
    let removed = acts(store.as_ref(), before).await?.len();
    ensure!(
        after == head && empty.events == 0 && removed == 1,
        "a repeat and a selection of nothing append nothing: the head moved from {head} to \
         {after}, {removed} acts, the empty selection answered {empty:?}"
    );
    Ok(())
}

async fn a_delivery_taken_back_reads_as_erased_and_a_later_one_stays(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    visits(&l).await?;
    let tracker: SourceId = "tracker".into();
    let deliver = async |id: Option<&str>, visits: &[(&str, Value)]| {
        let opts = IngestOpts {
            delivery: id.map(DeliveryId::from),
        };
        let records = (visits.iter())
            .map(|(k, f)| source_record("visit", k, f.clone()))
            .collect();
        let said = lakelands_api::ingest_all_with(l.cat.as_ref(), &l.own, &tracker, opts, records);
        let windows = said.await.must("ingesting a delivery")?;
        let first = windows.first().ok_or_else(|| Failure::new("a window"))?;
        Ok::<_, Failure>((first.position, first.delivery.clone()))
    };
    let v = |k: &str, d: u32| json!({"id": k, "duration": d});
    let (b1, _) = deliver(Some("b1"), &[("v1", v("v1", 10)), ("v2", v("v2", 20))]).await?;
    // `v1` again, its duration unchanged and a place beside it: the event
    // carries the duration too.
    let v1 = json!({"id": "v1", "duration": 10, "place": "Rome"});
    let (b2, _) = deliver(Some("b2"), &[("v1", v1), ("v3", v("v3", 30))]).await?;
    let (_, own) = deliver(None, &[("v4", v("v4", 40))]).await?;
    // The same values under another id append nothing.
    let head = store.head().await.must("reading the head")?;
    deliver(Some("b3"), &[("v2", v("v2", 20))]).await?;
    let b3 = Selection::Delivery("b3".into());
    let none = store
        .remove_values(&tracker, &b3, None)
        .await
        .must("removing b3")?;
    let after = store.head().await.must("reading the head")?;
    ensure!(
        none.events == 0 && after == head,
        "the redelivery under `b3` appended nothing, so its removal selects nothing and \
         appends nothing: {none:?}, the head moved from {head} to {after}"
    );
    let own = own.ok_or_else(|| Failure::new("the store named its delivery"))?;
    let theirs = Selection::Delivery(own.clone());
    let report = store.remove_values(&tracker, &theirs, None).await;
    let report = report.must("removing the store's own delivery")?;
    let v4 = at(&l, "visit", "v4", None).await?;
    ensure!(
        report.events == 1 && v4.is_none(),
        "the delivery the store named `{own}` is taken back by that id, and `v4`, which it \
         alone wrote, is retired: {report:?}, {v4:?}"
    );
    let b1_sel = Selection::Delivery("b1".into());
    let report = store
        .remove_values(&tracker, &b1_sel, None)
        .await
        .must("removing b1")?;
    let (v1_then, v1_now) = (
        at(&l, "visit", "v1", Some(b1)).await?,
        at(&l, "visit", "v1", None).await?,
    );
    let v1_b2 = at(&l, "visit", "v1", Some(b2)).await?;
    let v2 = at(&l, "visit", "v2", None).await?;
    ensure!(
        report.events == 2
            && v1_then
                .as_ref()
                .is_some_and(|r| erased(r, "duration", "tracker"))
            && [&v1_b2, &v1_now].iter().all(|r| {
                r.as_ref()
                    .is_some_and(|r| r.fields.get("duration") == Some(&json!(10)))
            })
            && v2.is_none(),
        "taking back `b1` selects its two events: `v1` reads its duration as erased at \
         `b1`'s position and holds `b2`'s from there on, and `v2`, which only `b1` wrote, is \
         retired: {report:?}; `v1` at b1 {v1_then:?}, at b2 {v1_b2:?}, now {v1_now:?}; \
         `v2` {v2:?}"
    );
    // A delivery under the same id after the act is not taken back by it.
    deliver(Some("b1"), &[("v5", v("v5", 50))]).await?;
    store.rebuild().await.must("rebuilding")?;
    let v5 = at(&l, "visit", "v5", None).await?;
    ensure!(
        v5.as_ref()
            .is_some_and(|r| r.fields.get("duration") == Some(&json!(50))),
        "a delivery under `b1` after its removal stays, a rebuild after it too: {v5:?}"
    );
    Ok(())
}

async fn a_removal_a_crash_stopped_is_finished_by_the_next_writers_open(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    use std::sync::atomic::{AtomicU32, Ordering};
    let fails: super::forgotten::Fails =
        std::sync::Arc::new([AtomicU32::new(0), AtomicU32::new(0)]);
    let wrap = super::forgotten::failing(&fails);
    let fault = async |store: Store| {
        let open = (given.fault)(store, Default::default(), wrap.clone()).await;
        open.map_err(|e| Failure::new(format!("the writer's open was refused: {e}")))
    };
    let mut store = fault(store).await?;
    let l = Lake::of(store.as_ref());
    visits(&l).await?;
    tracked(&l, &[("v1", 10), ("v2", 20)]).await?;
    l.upsert("visit", "v2", json!({"note": "kept"})).await?;
    let start = store.head().await.must("reading the head")?;
    let tracker: SourceId = "tracker".into();
    // The act's rebuild fails; the writer's next open finishes it.
    let all = Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a date"));
    fails[1].store(1, Ordering::SeqCst);
    let stopped = store.remove_values(&tracker, &all, None).await;
    ensure!(
        stopped.is_err(),
        "the removal whose rebuild fails answers {stopped:?}"
    );
    store = fault(store).await?;
    let finished = async |store: &Store, key: &str, gone: &str, n: usize, by: &str| {
        let l = Lake::of(store.as_ref());
        let kept = at(&l, "visit", key, Some(start)).await?;
        let gone = at(&l, "visit", gone, None).await?;
        let removed = acts(store.as_ref(), start).await?.len();
        ensure!(
            kept.as_ref()
                .is_some_and(|r| erased(r, "duration", "tracker"))
                && gone.is_none()
                && removed == n,
            "{by} finishes the removal: `{key}` reads its duration as erased before the act, \
             the record tracker alone wrote is retired, and the log holds {n} acts: {kept:?}, \
             {gone:?}, {removed} acts"
        );
        Ok::<_, Failure>(())
    };
    finished(&store, "v2", "v1", 1, "the writer's next open").await?;
    // Again, finished by the act's repeat on the same handle.
    let l = Lake::of(store.as_ref());
    tracked(&l, &[("v3", 30), ("v4", 40)]).await?;
    l.upsert("visit", "v3", json!({"note": "kept"})).await?;
    let again = Selection::WrittenBefore(Timestamp::from_date(9998, 1, 1).expect("a date"));
    fails[1].store(1, Ordering::SeqCst);
    let stopped = store.remove_values(&tracker, &again, None).await;
    ensure!(
        stopped.is_err(),
        "the second removal whose rebuild fails answers {stopped:?}"
    );
    let repeat = store.remove_values(&tracker, &again, None).await;
    repeat.must("repeating the act")?;
    finished(&store, "v2", "v4", 2, "the act's repeat").await?;
    let v3 = at(&l, "visit", "v3", None).await?;
    ensure!(
        v3.as_ref()
            .is_some_and(|r| erased(r, "duration", "tracker")),
        "the repeat erases `v3`'s duration too: {v3:?}"
    );
    Ok(())
}

async fn a_value_taken_from_a_hidden_field_is_told_as_one_never_written(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    use lakelands_api::{Find, Found};
    let twin = (given.twin)(store.lake().clone(), Default::default()).await;
    let twin = twin.map_err(|e| Failure::new(format!("the harness opened no twin: {e}")))?;
    let reader = super::only::holder(
        store.as_ref(),
        "reader",
        json!({"default": "read", "types": {"visit": {"fields": {"secret": "none"}}}}),
    );
    let tracker: SourceId = "tracker".into();
    let all = Selection::WrittenBefore(Timestamp::from_date(9999, 1, 1).expect("a date"));
    // The one lake's `v1` carries `secret`; the twin's never does.
    for (s, secret) in [(&store, true), (&twin, false)] {
        let l = Lake::of(s.as_ref());
        visits(&l).await?;
        let mut v1 = json!({"id": "v1", "duration": 10});
        if secret {
            v1["secret"] = json!(5);
        }
        let records = vec![
            source_record("visit", "v1", v1),
            source_record("visit", "v2", json!({"id": "v2", "duration": 20})),
        ];
        l.ingest("tracker", records)
            .await
            .must("ingesting from tracker")?;
        l.upsert("visit", "v2", json!({"note": "kept"})).await?;
        s.remove_values(&tracker, &all, None)
            .await
            .must("removing tracker's values")?;
    }
    let told = async |s: &Store| {
        let l = Lake::of(s.as_ref());
        let head = s.head().await.must("reading the head")?;
        let mut said = Vec::new();
        for p in 0..=head.0 {
            let at_p = Some(Position(p));
            for k in ["v1", "v2"] {
                let r = l.cat.get(&reader, &"visit".into(), get_at(k, at_p)).await;
                said.push(format!("{:?}", r.map(|r| r.data)));
            }
            for filter in [None, Some("duration is null")] {
                let find = Find {
                    total: true,
                    filter: filter.map(Into::into),
                    ..crate::conformance::fixture::find_at(at_p)
                };
                let found = l.cat.find(&reader, &"visit".into(), find).await;
                said.push(format!(
                    "{:?}",
                    found.map(|f| match f.data {
                        Found::Records { rows, total, .. } => format!("{rows:?} {total:?}"),
                        other => format!("{other:?}"),
                    })
                ));
            }
        }
        let d = l.cat.describe(&reader, Default::default()).await;
        said.push(format!("{:?}", d.map(|d| d.data.types)));
        Ok::<_, Failure>(said)
    };
    // Each lake has its own clock: the times are left out.
    let (here, there) = (
        timeless(&told(&store).await?),
        timeless(&told(&twin).await?),
    );
    let apart: Vec<_> = (here.iter().zip(&there))
        .filter(|(a, b)| a != b)
        .take(3)
        .collect();
    ensure!(
        here.len() == there.len() && apart.is_empty(),
        "a reader kept from `secret` is answered alike in the lake where tracker wrote it and in \
         its twin; apart: {apart:?}"
    );
    // Before the act, the act and `v1`'s retirement.
    let owner = async |s: &Store| {
        let l = Lake::of(s.as_ref());
        let head = s.head().await.must("reading the head")?;
        let r = at(&l, "visit", "v1", Some(Position(head.0 - 2))).await?;
        Ok::<_, Failure>(r.is_some_and(|r| r.meta.get("secret").is_some_and(|m| m.erased)))
    };
    let (marked, unmarked) = (owner(&store).await?, owner(&twin).await?);
    ensure!(
        marked && !unmarked,
        "the owner reads `secret` as erased in the lake where tracker wrote it, and not in its \
         twin: {marked}, {unmarked}"
    );
    Ok(())
}

/// `said` with every `known_at: Timestamp(…)` left out.
fn timeless(said: &[String]) -> Vec<String> {
    said.iter()
        .map(|s| {
            let mut out = String::new();
            let mut rest = s.as_str();
            while let Some(i) = rest.find("known_at: Timestamp(") {
                out.push_str(&rest[..i]);
                rest = &rest[i..];
                rest = rest.find(')').map_or("", |j| &rest[j + 1..]);
            }
            out.push_str(rest);
            out
        })
        .collect()
}
