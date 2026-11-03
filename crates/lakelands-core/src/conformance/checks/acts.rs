//! An act runs beside the writer
//! (`docs/decisions/099-work-beside-the-writer-runs-in-parts.md`).
//!
//! A removal, a forgetting and a purge walk off the writer, append in
//! parts of at most `limits.part_events`, and end with `act_whole`; the
//! lake stays readable and writable while it is owed. These rows hold
//! what a caller and a reader see at a lake between an act's turns:
//! writes land before its first turn, a verify passes, an export imports
//! and its writer finishes the act, a poll stops before a reaction the
//! projection cannot decide yet, an act outrun by the writer answers
//! unavailable, and a write reads through what the restate will leave.

use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex, MutexGuard};

use lakelands_api::catalog::Verification;
use lakelands_api::seam::FeedBatch;
use lakelands_api::write::{Change, ChangeTo, RemoveReport, ValueTypeMigration};

use super::only::de;
use super::removed::{acts, at, erased};
use crate::conformance::fixture::source_record;
use crate::conformance::prelude::*;
use crate::export::{self, Manifest};

check!(
    ACT_WALKS_OFF_THE_WRITER = a_write_sent_during_an_acts_walk_answers_before_its_first_turn,
    "099: a write sent while an act walks the selection is in the log before the act's first turn: it answers at a position below the act's `values_removed`, and the first turn folds it in, so the record it wrote is taken with the rest",
    given
);

check!(
    ACT_BETWEEN_PARTS_VERIFIES = a_verify_between_two_parts_passes,
    "099: with `part_events` 1, a verify while the act is owed, after its first turn and before its `act_whole`, passes through the position the log stood at, and the act ends with its whole after it",
    given
);

check!(
    ACT_BETWEEN_PARTS_EXPORTS = an_export_between_two_parts_imports_and_its_writer_finishes_the_act,
    "099: an export taken while an act is owed, its cut between two parts, imports; the imported lake holds the act unwhole, and its writer, opened, finishes the act beside the open, leaving one `values_removed`, one `act_whole` and every record of the selection retired",
    given
);

check!(
    POLL_STOPS_BEFORE_AN_OWED_ACT = a_poll_stops_before_a_reaction_the_projection_cannot_decide,
    "099: while a removal is owed, its restate not yet run, a poll stops before the first reaction the projection cannot decide yet, even one written before the act: the batch ends at the trigger's firing and delivers nothing past it; once the act is whole and restated, the poll from there delivers the firing and names it overruled",
    given
);

check!(
    ACT_TAIL_OVERRUN_MOVES_THE_VIEW =
        an_act_outrun_by_the_writer_answers_unavailable_and_appends_nothing,
    "099: a lake written faster than an act's view moves up, a write of more than `limits.part_tail` events landing between each catch-up and the first turn, leaves the act unavailable after its tries, saying the writes past its view were more than one turn reads; it appends no `values_removed`, and the writer's own writes stand",
    given
);

check!(
    REPORT_NOT_STALE_BY_A_WRITE_ELSEWHERE =
        a_write_that_reaches_nothing_of_the_report_leaves_its_id_good,
    "099: a report names the position it was read at and the selection there; a write since elsewhere, another source's record of a type the report names and a delivery the report does not select, is neither of those, so the act under the id is taken and answers what the preflight named, for a removal and a purge alike; a plan of a change to records, a declaration's plan, a migration's report and a value type's hold alike when their caller writes a record of a type they do not name; a forgetting's half follows in lakelands#1700"
);

check!(
    REPORT_STALE_WHEN_REACHED = a_write_that_reaches_the_report_refuses_its_id,
    "099: a report's id names what it read, never the head the lake stood at; a change to records' plan names its selection, so a record joining it makes the plan another, and that refusal is its content; a write to a type a declaration's plan computes again, to the type a migration rewrites, and to a type that uses the value type a migration changes leaves the same change's plan saying one digest, yet each act under the id the caller was shown is refused `plan_stale` or `migration_stale`, appending nothing; reach is over the types a plan's sections name, so a write to a type none names is not followed"
);

check!(
    WRITE_READS_THROUGH_AN_OWED_ACT = a_write_between_an_acts_parts_reads_through_to_the_restate,
    "099: while a removal is owed, a verb's `requires`, the values its assignments read and a function field's inputs are judged on the record refolded through the erasure the act names: `name is null` passes where the projection still holds the value, no taken value is written back into the log, and the function's output names no taken value; once the act is whole the record reads as the restate left it",
    given
);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// `visit` keyed by `id`: `duration` from a source, `note` an author's.
async fn visits(l: &Lake) -> Held {
    let text = json!({"kind": "text"});
    l.declare_json(json!({"declare": "type", "name": "visit", "key": ["id"],
        "fields": [{"name": "id", "type": text, "authority": "source"},
                   {"name": "duration", "type": {"kind": "number"}, "authority": "source"},
                   {"name": "note", "type": text}]}))
        .await?;
    Ok(())
}

/// `tracker`'s visits, each with its `duration`.
async fn tracked(l: &Lake, visits: &[(&str, u32)]) -> Held {
    let records = (visits.iter())
        .map(|(k, d)| source_record("visit", k, json!({"id": k, "duration": d})))
        .collect();
    l.ingest("tracker", records)
        .await
        .must("ingesting from tracker")?;
    Ok(())
}

/// Append `records` of `ty` from `source` in delivery `id`.
async fn delivered(l: &Lake, source: &str, ty: &str, id: &str, records: &[(&str, Value)]) -> Held {
    let opts = lakelands_api::IngestOpts {
        delivery: Some(lakelands_api::DeliveryId::from(id)),
        ..lakelands_api::IngestOpts::default()
    };
    let source: SourceId = source.into();
    let records = (records.iter())
        .map(|(k, f)| source_record(ty, k, f.clone()))
        .collect();
    lakelands_api::ingest_all_with(l.cat.as_ref(), &l.own, &source, opts, records)
        .await
        .must("ingesting a delivery")?;
    Ok(())
}

/// `meter`'s declaration, with `flag` when asked.
fn meter(flag: bool) -> Value {
    let mut fields = vec![
        json!({"name": "id", "type": {"kind": "text"}}),
        json!({"name": "reading", "type": {"kind": "number"}}),
        json!({"name": "grade", "type": {"kind": "text", "value_type": "grade"}}),
        json!({"name": "twice", "type": {"kind": "number"}, "authority": "derived",
               "function": "dbl", "depends_on": ["reading"]}),
    ];
    if flag {
        fields.push(json!({"name": "flag", "type": {"kind": "text"}}));
    }
    json!({"name": "meter", "key": ["id"], "fields": fields})
}

/// `visit` and `invoice` from their sources, a `meter` whose `twice` is
/// `dbl`'s and whose `grade` is a `grade`, and the four declarations:
/// the lake a plan of each act is made on.
async fn planning(l: &Lake) -> Held {
    visits(l).await?;
    l.declare_json(json!({"declare": "type", "name": "invoice", "fields": [
        {"name": "amount", "type": {"kind": "number"}, "authority": "source"}]}))
        .await?;
    l.declare_json(
        json!({"declare": "value_type", "name": "grade", "kind": "text",
        "one_of": ["a", "b", "c"]}),
    )
    .await?;
    l.declare_json(
        json!({"declare": "function", "name": "dbl", "backing": "expr",
        "inputs": [{"name": "reading", "type": {"kind": "number"}}],
        "expr": "reading * 2"}),
    )
    .await?;
    let mut declared = meter(false);
    declared["declare"] = json!("type");
    l.declare_json(declared).await?;
    delivered(
        l,
        "tracker",
        "visit",
        "b1",
        &[("v1", json!({"id": "v1", "duration": 10}))],
    )
    .await?;
    delivered(l, "crm", "invoice", "c1", &[("c1", json!({"amount": 1}))]).await?;
    l.upsert("meter", "m1", json!({"reading": 2, "grade": "a"}))
        .await?;
    Ok(())
}

/// A change to `visit` where `duration` is 10 or more, its `note` set.
fn picked() -> Change {
    Change::Records {
        ty: "visit".into(),
        filter: Box::new("duration >= 10".into()),
        to: ChangeTo::Set(de(json!({"note": "picked"}))),
        only: BTreeMap::new(),
        skip_restricted: false,
    }
}

/// `dbl` declared again so that `twice` is three times the reading.
fn thrice() -> Change {
    Change::Declare {
        declarations: vec![de(json!({"declare": "function", "name": "dbl",
            "backing": "expr",
            "inputs": [{"name": "reading", "type": {"kind": "number"}}],
            "expr": "reading * 3"}))],
    }
}

/// A migration of `meter` adding `flag`.
fn flagged() -> Migration {
    Migration::to(de(meter(true)))
}

/// `grade` tightened to the two values the lake's records fit.
fn tighten() -> ValueTypeMigration {
    ValueTypeMigration::from(de::<ValueTypeDecl>(
        json!({"name": "grade", "kind": "text", "one_of": ["a", "b"]}),
    ))
}

/// A far date, so a `WrittenBefore` act selects every write before it.
fn far() -> Timestamp {
    Timestamp::from_date(9999, 1, 1).expect("a far date")
}

/// The first event of the kind `kind` after `after`.
fn first_of(events: &[Event], kind: &str) -> Option<Position> {
    events
        .iter()
        .find(|e| e.body.kind() == kind)
        .map(|e| e.position)
}

async fn a_write_sent_during_an_acts_walk_answers_before_its_first_turn(
    store: Store,
    given: Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    visits(&l).await?;
    let n = given.large_lake.max(20);
    let records: Vec<(String, u32)> = (0..n).map(|i| (format!("v{i}"), i)).collect();
    let records: Vec<(&str, u32)> = records.iter().map(|(k, d)| (k.as_str(), *d)).collect();
    tracked(&l, &records).await?;
    let start = store.head().await.must("reading the head")?;
    let all = Selection::WrittenBefore(far());
    let tracker: SourceId = "tracker".into();
    let act_slot: Arc<Mutex<Option<RemoveReport>>> = Arc::default();
    let write_slot: Arc<Mutex<Option<Position>>> = Arc::default();
    let (s, answer, written) = (store.clone(), act_slot.clone(), write_slot.clone());
    let (s2, source) = (s.clone(), tracker.clone());
    // The write is the first caller polled: it is sent before the act
    // starts its walk, and the first turn cannot pass it.
    let callers: Vec<Caller<'static, Held>> = vec![
        Box::pin(async move {
            let l = Lake::of(s.as_ref());
            let done = l
                .ingest(
                    "tracker",
                    vec![source_record(
                        "visit",
                        "late",
                        json!({"id": "late", "duration": 999}),
                    )],
                )
                .await
                .must("the write during the act's walk")?;
            let at = (done.last())
                .map(|w| w.position)
                .ok_or_else(|| Failure::new("the write answered no window"))?;
            *lock(&written) = Some(at);
            Ok(())
        }),
        Box::pin(async move {
            let done = s2
                .remove_values(&source, &all, false, None)
                .await
                .must("the act")?;
            *lock(&answer) = Some(done);
            Ok(())
        }),
    ];
    join_all(callers).await.into_iter().collect::<Held>()?;
    let done = lock(&act_slot).take().expect("the act answered");
    let at_write = lock(&write_slot).take().expect("the write answered");
    let act = acts(store.as_ref(), start).await?;
    let removed = (act.first())
        .map(|e| e.position)
        .ok_or_else(|| Failure::new("the act appended no `values_removed`"))?;
    let late = at(&l, "visit", "late", None).await?;
    ensure!(
        at_write < removed && done.events == u64::from(n) + 1 && late.is_none(),
        "the write sent during the walk must be in the log before the act's first turn, at a \
         position below its `values_removed`, and the first turn must fold it in: the write \
         answered {at_write}, the act's first event is at {removed}, the act {done:?}, the \
         record it wrote {late:?}"
    );
    Ok(())
}

async fn a_verify_between_two_parts_passes(store: Store, given: Given) -> Held {
    let mut few = limits();
    few.part_events = 1;
    let store = (given.relimit)(store, Seams::default(), few)
        .await
        .map_err(|e| Failure::new(format!("the harness opened no lake: {e}")))?;
    let l = Lake::of(store.as_ref());
    visits(&l).await?;
    let n = 200u32;
    let records: Vec<(String, u32)> = (0..n).map(|i| (format!("v{i}"), i)).collect();
    let records: Vec<(&str, u32)> = records.iter().map(|(k, d)| (k.as_str(), *d)).collect();
    tracked(&l, &records).await?;
    let start = store.head().await.must("reading the head")?;
    let all = Selection::WrittenBefore(far());
    let tracker: SourceId = "tracker".into();
    let slot: Arc<Mutex<Option<RemoveReport>>> = Arc::default();
    let seen: Arc<Mutex<Option<(Verification, bool)>>> = Arc::default();
    let (s, answer, s2, read) = (store.clone(), slot.clone(), store.clone(), seen.clone());
    let callers: Vec<Caller<'static, Held>> = vec![
        Box::pin(async move {
            let done = s
                .remove_values(&tracker, &all, false, None)
                .await
                .must("the act")?;
            *lock(&answer) = Some(done);
            Ok(())
        }),
        Box::pin(async move {
            settles(6_000, Duration::from_millis(1), || async {
                ensure!(
                    !acts(s2.as_ref(), start).await?.is_empty(),
                    "the act's first turn"
                );
                ensure!(
                    first_of(&events_after(s2.as_ref(), start).await?, "act_whole").is_none(),
                    "the act is still owed"
                );
                Ok(())
            })
            .await?;
            let verified = s2
                .verify(None, None)
                .await
                .must("a verify between two parts")?;
            let whole = first_of(&events_after(s2.as_ref(), start).await?, "act_whole");
            *lock(&read) = Some((verified, whole.is_some()));
            Ok(())
        }),
    ];
    join_all(callers).await.into_iter().collect::<Held>()?;
    let done = lock(&slot).take().expect("the act answered");
    let (verified, whole_then) = lock(&seen).take().expect("the verify ran");
    let after = events_after(store.as_ref(), start).await?;
    let removed = first_of(&after, "values_removed").expect("the act's first turn");
    let whole = first_of(&after, "act_whole").expect("the act's whole");
    ensure!(
        !whole_then
            && verified.through >= removed
            && whole > verified.through
            && done.events == u64::from(n),
        "a verify made between the act's first turn and its whole must pass through the log at \
         its position and leave the act to end after it: the verify said {verified:?}, the act \
         was whole then: {whole_then}, its first event at {removed}, its whole at {whole}, the \
         act {done:?}"
    );
    Ok(())
}

async fn an_export_between_two_parts_imports_and_its_writer_finishes_the_act(
    store: Store,
    given: Given,
) -> Held {
    let scratch = super::export::Scratch::new(given.scratch.clone());
    let dir = scratch.dir("between-parts");
    let mut few = limits();
    few.part_events = 1;
    let store = (given.relimit)(store, Seams::default(), few)
        .await
        .map_err(|e| Failure::new(format!("the harness opened no lake: {e}")))?;
    let l = Lake::of(store.as_ref());
    visits(&l).await?;
    let n = 120u32;
    let records: Vec<(String, u32)> = (0..n).map(|i| (format!("v{i}"), i)).collect();
    let records: Vec<(&str, u32)> = records.iter().map(|(k, d)| (k.as_str(), *d)).collect();
    tracked(&l, &records).await?;
    let start = store.head().await.must("reading the head")?;
    let all = Selection::WrittenBefore(far());
    let tracker: SourceId = "tracker".into();
    let slot: Arc<Mutex<Option<RemoveReport>>> = Arc::default();
    let taken: Arc<Mutex<Option<Manifest>>> = Arc::default();
    let (s, answer, s2, exported) = (store.clone(), slot.clone(), store.clone(), taken.clone());
    let writing = dir.clone();
    let callers: Vec<Caller<'static, Held>> = vec![
        Box::pin(async move {
            let done = s
                .remove_values(&tracker, &all, false, None)
                .await
                .must("the act")?;
            *lock(&answer) = Some(done);
            Ok(())
        }),
        Box::pin(async move {
            settles(6_000, Duration::from_millis(1), || async {
                ensure!(
                    !acts(s2.as_ref(), start).await?.is_empty(),
                    "the act's first turn"
                );
                ensure!(
                    first_of(&events_after(s2.as_ref(), start).await?, "act_whole").is_none(),
                    "the act is still owed"
                );
                Ok(())
            })
            .await?;
            let manifest = export::write(s2.as_ref(), &writing)
                .await
                .must("an export between two parts")?;
            *lock(&exported) = Some(manifest);
            Ok(())
        }),
    ];
    join_all(callers).await.into_iter().collect::<Held>()?;
    let done = lock(&slot).take().expect("the act answered");
    let manifest = lock(&taken).take().expect("the export ran");
    let after = events_after(store.as_ref(), start).await?;
    let whole = first_of(&after, "act_whole").expect("the act's whole");
    ensure!(
        manifest.through < whole && done.events == u64::from(n),
        "the export must be cut between two parts, before the act's whole: it cut at {}, the \
         whole is at {whole}, the act {done:?}",
        manifest.through
    );
    // The import holds the act owed, and its writer finishes it.
    let imported = (given.import)(dir, store.lake().clone(), None, Seams::default())
        .await
        .map_err(|e| Failure::new(format!("importing the export: {e}")))?;
    let fed = log_events(imported.store.as_ref()).await?;
    ensure!(
        first_of(&fed, "act_whole").is_none() && first_of(&fed, "values_removed").is_some(),
        "the imported lake must hold the act's `values_removed` and none of its `act_whole`: \
         {:?}",
        fed.iter().map(|e| e.body.kind()).collect::<Vec<_>>()
    );
    let writer = (imported.writer)(imported.store, Seams::default())
        .await
        .map_err(|e| Failure::new(format!("opening the import for writing: {e}")))?;
    settles(6_000, Duration::from_millis(5), || async {
        ensure!(
            first_of(
                &events_after(writer.as_ref(), Position::GENESIS).await?,
                "act_whole"
            )
            .is_some(),
            "the imported writer finishes the act"
        );
        Ok(())
    })
    .await?;
    let finished = events_after(writer.as_ref(), Position::GENESIS).await?;
    let kinds: Vec<&str> = finished.iter().map(|e| e.body.kind()).collect();
    let l = Lake::of(writer.as_ref());
    let mut live = 0;
    for i in 0..n {
        let key = format!("v{i}");
        if at(&l, "visit", key.as_str(), None).await?.is_some() {
            live += 1;
        }
    }
    ensure!(
        kinds.iter().filter(|k| **k == "values_removed").count() == 1
            && kinds.iter().filter(|k| **k == "act_whole").count() == 1
            && kinds.last() == Some(&"act_whole")
            && live == 0,
        "the imported writer must finish the act once, appending no second `values_removed`, \
         leave every record of the selection retired and end with the whole: it appended \
         {kinds:?}, {live} records stay"
    );
    Ok(())
}

async fn a_poll_stops_before_a_reaction_the_projection_cannot_decide(
    store: Store,
    given: Given,
) -> Held {
    let mut few = limits();
    few.part_events = 1;
    let store = (given.relimit)(store, Seams::default(), few)
        .await
        .map_err(|e| Failure::new(format!("the harness opened no lake: {e}")))?;
    let l = Lake::of(store.as_ref());
    visits(&l).await?;
    let n = 60u32;
    let records: Vec<(String, u32)> = (0..n).map(|i| (format!("v{i}"), i + 1)).collect();
    let records: Vec<(&str, u32)> = records.iter().map(|(k, d)| (k.as_str(), *d)).collect();
    tracked(&l, &records).await?;
    l.declare_json(json!({"declare": "trigger", "name": "timed", "on": "visit",
                           "when": "duration > 0"}))
        .await?;
    // The author's write fires the trigger over `duration`, which the act
    // takes back.
    let from = store.head().await.must("reading the head")?;
    l.upsert("visit", "v1", json!({"note": "kept"})).await?;
    let fired = first_of(&events_after(store.as_ref(), from).await?, "trigger_fired")
        .ok_or_else(|| Failure::new("the trigger fired on the author's write"))?;
    let start = store.head().await.must("reading the head")?;
    let all = Selection::WrittenBefore(far());
    let tracker: SourceId = "tracker".into();
    let slot: Arc<Mutex<Option<RemoveReport>>> = Arc::default();
    let seen: Arc<Mutex<Option<FeedBatch>>> = Arc::default();
    let (s, answer, s2, read) = (store.clone(), slot.clone(), store.clone(), seen.clone());
    let callers: Vec<Caller<'static, Held>> = vec![
        Box::pin(async move {
            let done = s
                .remove_values(&tracker, &all, false, None)
                .await
                .must("the act")?;
            *lock(&answer) = Some(done);
            Ok(())
        }),
        Box::pin(async move {
            settles(6_000, Duration::from_millis(1), || async {
                ensure!(
                    !acts(s2.as_ref(), start).await?.is_empty(),
                    "the act's first turn"
                );
                ensure!(
                    first_of(&events_after(s2.as_ref(), start).await?, "act_whole").is_none(),
                    "the act is still owed"
                );
                Ok(())
            })
            .await?;
            let batch = s2
                .feed()
                .poll(Position::GENESIS, &FeedFilter::default(), u32::MAX)
                .await
                .must("a poll while the act is owed")?;
            *lock(&read) = Some(batch);
            Ok(())
        }),
    ];
    join_all(callers).await.into_iter().collect::<Held>()?;
    let done = lock(&slot).take().expect("the act answered");
    let owed = lock(&seen).take().expect("the poll ran");
    ensure!(
        owed.events.iter().all(|e| e.position < fired)
            && owed.next == fired
            && !owed.overruled.contains(&fired),
        "a poll while the act is owed must stop before the trigger's firing at {fired}: it \
         answered next {}, events past it {:?}, overruled {:?}",
        owed.next,
        (owed.events.iter())
            .filter(|e| e.position >= fired)
            .map(|e| e.position)
            .collect::<Vec<_>>(),
        owed.overruled
    );
    // Once the act is whole and restated, the poll from where it stopped
    // delivers the firing and names it overruled.
    let then = store
        .feed()
        .poll(owed.next, &FeedFilter::default(), u32::MAX)
        .await
        .must("a poll after the act is whole")?;
    ensure!(
        then.events.iter().any(|e| e.position == fired)
            && then.overruled.contains(&fired)
            && done.events == u64::from(n),
        "after the act is whole, a poll from {} must deliver the firing at {fired} and name it \
         overruled: it answered {:?} and overruled {:?}, the act {done:?}",
        owed.next,
        then.events.iter().map(|e| e.position).collect::<Vec<_>>(),
        then.overruled
    );
    Ok(())
}

async fn an_act_outrun_by_the_writer_answers_unavailable_and_appends_nothing(
    store: Store,
    given: Given,
) -> Held {
    let mut few = limits();
    few.part_tail = 1;
    let store = (given.relimit)(store, Seams::default(), few)
        .await
        .map_err(|e| Failure::new(format!("the harness opened no lake: {e}")))?;
    let l = Lake::of(store.as_ref());
    visits(&l).await?;
    let records: Vec<(String, u32)> = (0..100).map(|i| (format!("v{i}"), i)).collect();
    let records: Vec<(&str, u32)> = records.iter().map(|(k, d)| (k.as_str(), *d)).collect();
    tracked(&l, &records).await?;
    let start = store.head().await.must("reading the head")?;
    let all = Selection::WrittenBefore(far());
    let tracker: SourceId = "tracker".into();
    let slot: Arc<Mutex<Option<lakelands_api::Result<RemoveReport>>>> = Arc::default();
    let stop = Arc::new(AtomicBool::new(false));
    let made = Arc::new(AtomicU64::new(0));
    let mut callers: Vec<Caller<'static, Held>> = Vec::new();
    {
        let (s, answer, stop) = (store.clone(), slot.clone(), stop.clone());
        callers.push(Box::pin(async move {
            let done = s.remove_values(&tracker, &all, false, None).await;
            *lock(&answer) = Some(done);
            stop.store(true, Ordering::SeqCst);
            Ok(())
        }));
    }
    // Two writers, each appending more than `part_tail` events at a time,
    // keep the queue ahead of the act's first turn.
    for w in 0..2u64 {
        let (s, stop, made) = (store.clone(), stop.clone(), made.clone());
        callers.push(Box::pin(async move {
            let l = Lake::of(s.as_ref());
            while !stop.load(Ordering::SeqCst) {
                let n = made.fetch_add(1, Ordering::SeqCst);
                let records: Vec<SourceRecord> = (0..10)
                    .map(|i| {
                        let k = format!("w{w}-{n}-{i}");
                        source_record("visit", &k, json!({"id": &k, "duration": 1}))
                    })
                    .collect();
                l.ingest("tracker", records)
                    .await
                    .must("a write beside the act")?;
            }
            Ok(())
        }));
    }
    join_all(callers).await.into_iter().collect::<Held>()?;
    let done = lock(&slot).take().expect("the act answered");
    let overran = match &done {
        Err(e) => e.situation().cloned(),
        Ok(report) => {
            return Err(Failure::new(format!(
                "an act outrun by the writer must answer unavailable; it answered {report:?}"
            )));
        }
    };
    ensure!(
        overran == Some(Situation::TailOverran { tries: 3, bound: 1 }),
        "an act outrun by the writer must answer `TailOverran` after its tries; it answered \
         {overran:?}"
    );
    ensure!(
        acts(store.as_ref(), start).await?.is_empty(),
        "an act that overran must append no `values_removed`"
    );
    Ok(())
}

async fn a_write_that_reaches_nothing_of_the_report_leaves_its_id_good(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    planning(&l).await?;
    let v = |k: &str, d: u32| json!({"id": k, "duration": d});
    let tracker: SourceId = "tracker".into();
    let sel = Selection::Delivery(lakelands_api::DeliveryId::from("b1"));
    let report = store
        .remove_values_preflight(&tracker, &sel)
        .await
        .must("the removal's preflight")?;
    let crm: SourceId = "crm".into();
    let purge = store
        .purge_preflight(&crm, None)
        .await
        .must("the purge's preflight")?;
    // Writes elsewhere: another source's record of a type the purge
    // names, and a delivery the removal's report did not select.
    delivered(&l, "web", "visit", "b2", &[("v9", v("v9", 90))]).await?;
    l.ingest(
        "erp",
        vec![source_record("invoice", "c9", json!({"amount": 9}))],
    )
    .await
    .must("ingesting from erp")?;
    let done = store
        .remove_values(&tracker, &sel, false, Some(&report.report_id))
        .await
        .must("the removal under its report's id after a write elsewhere")?;
    let kept = at(&l, "visit", "v9", None).await?;
    let gone = at(&l, "visit", "v1", None).await?;
    ensure!(
        done.events == report.events
            && done.retired == report.retired
            && gone.is_none()
            && kept
                .as_ref()
                .is_some_and(|r| r.fields.get("duration") == Some(&json!(90))),
        "another source's write elsewhere must leave the removal's id good, and the act answer \
         what the preflight named: the report {report:?}, the act {done:?}, v1 {gone:?}, v9 \
         {kept:?}"
    );
    let purged = store
        .purge_source(&crm, None, Some(&purge.report_id))
        .await
        .must("the purge under its report's id after a write elsewhere")?;
    let c1 = at(&l, "invoice", "c1", None).await?;
    let c9 = at(&l, "invoice", "c9", None).await?;
    ensure!(
        purged.retired == purge.retired && c1.is_none() && c9.is_some(),
        "another source's record of the type must leave the purge's id good, and the purge \
         answer what the preflight named: the report {purge:?}, the act {purged:?}, c1 {c1:?}, \
         c9 {c9:?}"
    );
    // A change to records, a declaration, a migration and a value type's:
    // each is applied after a write to a type its report does not name.
    let planned = l
        .cat
        .plan(&l.own, picked())
        .await
        .must("planning a change to records")?
        .data;
    delivered(&l, "web", "invoice", "b5", &[("c2", json!({"amount": 2}))]).await?;
    l.cat
        .enact(&l.own, picked(), &planned.report_id)
        .await
        .must("a change to records under its plan after a write elsewhere")?;
    let picked_note =
        (at(&l, "visit", "v9", None).await?).and_then(|r| r.fields.get("note").cloned());
    ensure!(
        picked_note == Some(json!("picked")),
        "a change to records under its plan after a write elsewhere must have set v9's note: \
         the plan {planned:?}, v9's note {picked_note:?}"
    );
    let planned = l
        .cat
        .plan(&l.own, thrice())
        .await
        .must("planning `dbl` declared again")?
        .data;
    delivered(&l, "web", "invoice", "b3", &[("c3", json!({"amount": 3}))]).await?;
    l.cat
        .enact(&l.own, thrice(), &planned.report_id)
        .await
        .must("a declaration under its plan after a write elsewhere")?;
    let twice = (at(&l, "meter", "m1", None).await?).and_then(|r| r.fields.get("twice").cloned());
    ensure!(
        twice == Some(json!(6)),
        "a declaration under its plan after a write elsewhere must have computed `twice` again: \
         the plan {planned:?}, m1's twice {twice:?}"
    );
    let planned = l
        .cat
        .plan(
            &l.own,
            Change::Migrate {
                migration: Box::new(flagged()),
            },
        )
        .await
        .must("planning the migration of `meter`")?
        .data;
    let id = (planned.types.first())
        .map(|t| t.report_id.clone())
        .ok_or_else(|| Failure::new("the migration's plan told no type"))?;
    delivered(
        &l,
        "tracker",
        "visit",
        "b7",
        &[("v3", json!({"id": "v3", "duration": 30}))],
    )
    .await?;
    l.cat
        .migrate_apply(&l.own, flagged(), &id)
        .await
        .must("a migration under its plan after a write elsewhere")?;
    ensure!(
        (l.describe_type("meter").await?.fields)
            .iter()
            .any(|f| f.name.as_str() == "flag"),
        "a migration under its plan after a write elsewhere must have added `flag` to `meter`: \
         the plan {planned:?}"
    );
    let migration = tighten();
    let planned = l
        .cat
        .plan(
            &l.own,
            Change::MigrateValueType {
                migration: Box::new(migration.clone()),
            },
        )
        .await
        .must("planning the value type's migration")?
        .data;
    let id = (planned.value_types.first())
        .map(|v| v.report_id.clone())
        .ok_or_else(|| Failure::new("the value type's plan told no value type"))?;
    delivered(&l, "web", "invoice", "b4", &[("c4", json!({"amount": 4}))]).await?;
    l.cat
        .migrate_value_type_apply(&l.own, migration, &id)
        .await
        .must("a value type's migration under its plan after a write elsewhere")?;
    let outside = try_upsert(
        l.cat.as_ref(),
        &l.own,
        "meter",
        "m2",
        json!({"reading": 1, "grade": "c"}),
    )
    .await;
    ensure!(
        outside.is_err(),
        "the value type's migration under its plan after a write elsewhere must have tightened \
         `grade`: a write of `c` answered {outside:?}"
    );
    Ok(())
}

/// The digest half of a position-named id: what the report said, apart
/// from where it was read.
fn digest_of(id: &str) -> &str {
    id.split_once('-').map_or(id, |(_, digest)| digest)
}

async fn a_write_that_reaches_the_report_refuses_its_id(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    planning(&l).await?;
    // A record joins a change to records' selection: its plan names the
    // selection alone, so the plan made now is another.
    let planned = l
        .cat
        .plan(&l.own, picked())
        .await
        .must("planning a change to records")?
        .data;
    delivered(
        &l,
        "tracker",
        "visit",
        "b2",
        &[("v2", json!({"id": "v2", "duration": 20}))],
    )
    .await?;
    let joined = l
        .cat
        .plan(&l.own, picked())
        .await
        .must("planning the change again")?
        .data;
    ensure!(
        joined.report_id != planned.report_id,
        "a change to records' plan id is its selection, so one a record joined must be \
         another: the plan {planned:?}, planned again {joined:?}"
    );
    let head = store.head().await.must("reading the head")?;
    refused_like(
        "a change to records after a record joined its selection",
        l.cat.enact(&l.own, picked(), &planned.report_id).await,
        &json!({"code": "plan_stale"}),
    )?;
    head_unmoved(store.as_ref(), head, "a change under a stale plan").await?;
    // A write to a type a declaration's plan computes again: the plan made
    // again says the same digest, so only the reach can refuse it.
    let planned = l
        .cat
        .plan(&l.own, thrice())
        .await
        .must("planning `dbl` declared again")?
        .data;
    l.upsert("meter", "m1", json!({"grade": "b"})).await?;
    let again = l
        .cat
        .plan(&l.own, thrice())
        .await
        .must("planning `dbl` again")?
        .data;
    ensure!(
        digest_of(&again.report_id) == digest_of(&planned.report_id),
        "a declaration's plan says what it says at another position, so its digest must \
         not move: the plan {planned:?}, planned again {again:?}"
    );
    let head = store.head().await.must("reading the head")?;
    refused_like(
        "a declaration after a write to the type it computes again",
        l.cat.enact(&l.own, thrice(), &planned.report_id).await,
        &json!({"code": "plan_stale"}),
    )?;
    head_unmoved(store.as_ref(), head, "a declaration under a stale plan").await?;
    // A write to the type a migration rewrites, its report the same.
    let planned = l
        .cat
        .plan(
            &l.own,
            Change::Migrate {
                migration: Box::new(flagged()),
            },
        )
        .await
        .must("planning the migration of `meter`")?
        .data;
    let id = (planned.types.first())
        .map(|t| t.report_id.clone())
        .ok_or_else(|| Failure::new("the migration's plan told no type"))?;
    l.upsert("meter", "m1", json!({"reading": 3})).await?;
    let again = l
        .cat
        .plan(
            &l.own,
            Change::Migrate {
                migration: Box::new(flagged()),
            },
        )
        .await
        .must("planning the migration again")?
        .data;
    let again_id = (again.types.first())
        .map(|t| t.report_id.clone())
        .ok_or_else(|| Failure::new("the plan made again told no type"))?;
    ensure!(
        digest_of(&again_id) == digest_of(&id),
        "a migration's report says what it says at another position, so its digest must \
         not move: the report {id}, planned again {again_id}"
    );
    let head = store.head().await.must("reading the head")?;
    refused_like(
        "a migration after a write to the type it rewrites",
        l.cat.migrate_apply(&l.own, flagged(), &id).await,
        &json!({"code": "migration_stale"}),
    )?;
    head_unmoved(store.as_ref(), head, "a migration under a stale report").await?;
    // A write to a type that uses the value type a migration changes.
    let migration = tighten();
    let planned = l
        .cat
        .plan(
            &l.own,
            Change::MigrateValueType {
                migration: Box::new(migration.clone()),
            },
        )
        .await
        .must("planning the value type's migration")?
        .data;
    let id = (planned.value_types.first())
        .map(|v| v.report_id.clone())
        .ok_or_else(|| Failure::new("the value type's plan told no value type"))?;
    l.upsert("meter", "m1", json!({"grade": "a"})).await?;
    let again = l
        .cat
        .plan(
            &l.own,
            Change::MigrateValueType {
                migration: Box::new(migration.clone()),
            },
        )
        .await
        .must("planning the value type's migration again")?
        .data;
    let again_id = (again.value_types.first())
        .map(|v| v.report_id.clone())
        .ok_or_else(|| Failure::new("the plan made again told no value type"))?;
    ensure!(
        digest_of(&again_id) == digest_of(&id),
        "a value type's report says what it says at another position, so its digest must \
         not move: the report {id}, planned again {again_id}"
    );
    let head = store.head().await.must("reading the head")?;
    refused_like(
        "a value type's migration after a write to a type that uses it",
        l.cat.migrate_value_type_apply(&l.own, migration, &id).await,
        &json!({"code": "migration_stale"}),
    )?;
    head_unmoved(store.as_ref(), head, "a value type under a stale report").await?;
    Ok(())
}

async fn a_write_between_an_acts_parts_reads_through_to_the_restate(
    store: Store,
    given: Given,
) -> Held {
    let mut few = limits();
    few.part_events = 1;
    let store = (given.relimit)(store, Seams::default(), few)
        .await
        .map_err(|e| Failure::new(format!("the harness opened no lake: {e}")))?;
    let l = Lake::of(store.as_ref());
    let text = json!({"kind": "text"});
    l.declare_json(
        json!({"declare": "function", "name": "shout", "backing": "expr",
        "inputs": [{"name": "name", "type": text}],
        "expr": "upper(name)"}),
    )
    .await?;
    l.declare_json(
        json!({"declare": "type", "name": "person", "key": ["id"], "fields": [
        {"name": "id", "type": text, "authority": "source"},
        {"name": "name", "type": text, "authority": "source"},
        {"name": "note", "type": text},
        {"name": "copied", "type": text},
        {"name": "loud", "type": text, "authority": "derived", "function": "shout",
         "depends_on": ["name"]}]}),
    )
    .await?;
    l.declare_json(
        json!({"declare": "verb", "name": "copy", "on": "person", "risk": "low",
        "effect": "write", "requires": "name is null",
        "set": [{"field": "note", "value": "'owed'"},
                {"field": "copied", "value": "name"}]}),
    )
    .await?;
    let n = 40u32;
    let records: Vec<SourceRecord> = (0..n)
        .map(|i| {
            let k = format!("p{i}");
            source_record("person", &k, json!({"id": &k, "name": format!("Ada {i}")}))
        })
        .collect();
    l.ingest("tracker", records)
        .await
        .must("ingesting from tracker")?;
    // An author's note keeps `p0`, whose `name` the act takes.
    l.upsert("person", "p0", json!({"note": "kept"})).await?;
    let start = store.head().await.must("reading the head")?;
    let all = Selection::WrittenBefore(far());
    let tracker: SourceId = "tracker".into();
    let slot: Arc<Mutex<Option<RemoveReport>>> = Arc::default();
    let written: Arc<Mutex<Option<Outcome>>> = Arc::default();
    let before: Arc<Mutex<Option<lakelands_api::Record>>> = Arc::default();
    let (s, answer, s2, made, seen) = (
        store.clone(),
        slot.clone(),
        store.clone(),
        written.clone(),
        before.clone(),
    );
    let callers: Vec<Caller<'static, Held>> = vec![
        Box::pin(async move {
            let done = s
                .remove_values(&tracker, &all, false, None)
                .await
                .must("the act")?;
            *lock(&answer) = Some(done);
            Ok(())
        }),
        Box::pin(async move {
            settles(6_000, Duration::from_millis(1), || async {
                ensure!(
                    !acts(s2.as_ref(), start).await?.is_empty(),
                    "the act's first turn"
                );
                ensure!(
                    first_of(&events_after(s2.as_ref(), start).await?, "act_whole").is_none(),
                    "the act is still owed"
                );
                Ok(())
            })
            .await?;
            let l = Lake::of(s2.as_ref());
            let shown = at(&l, "person", "p0", None)
                .await?
                .ok_or_else(|| Failure::new("`p0` before the act's whole"))?;
            *lock(&seen) = Some(shown);
            let called = l
                .cat
                .call(
                    &l.own,
                    &"copy".into(),
                    Some(&RecordKey::from("p0")),
                    patch(json!({})),
                    WriteOpts::default(),
                )
                .await
                .must("a verb on `p0` while the act is owed")?;
            *lock(&made) = Some(called);
            Ok(())
        }),
    ];
    join_all(callers).await.into_iter().collect::<Held>()?;
    let done = lock(&slot).take().expect("the act answered");
    let called = lock(&written).take().expect("the verb ran");
    let shown = lock(&before).take().expect("`p0` was read");
    let l = Lake::of(store.as_ref());
    let after = at(&l, "person", "p0", None)
        .await?
        .ok_or_else(|| Failure::new("`p0` after the act"))?;
    ensure!(
        done.events == u64::from(n)
            && shown.fields.get("name") == Some(&json!("Ada 0"))
            && erased(&after, "name", "tracker")
            && !after.fields.contains_key("copied")
            && !after.fields.contains_key("loud")
            && after.fields.get("note") == Some(&json!("owed"))
            && called.position() > start,
        "while the act is owed the projection still holds `p0`'s name ({:?}), yet the verb's \
         `name is null` must pass, its assignment of `name` must write no taken value and the \
         function over `name` must leave no output: the call {called:?}, `p0` after {after:?}, \
         the act {done:?}",
        shown.fields.get("name")
    );
    Ok(())
}
