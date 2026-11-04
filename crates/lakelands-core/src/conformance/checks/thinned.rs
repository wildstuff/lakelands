//! The thinning act itself: the walk, the first turn, its parts and its
//! `act_whole`, beside the writer, through the store's `thin_values`
//! (`docs/decisions/102-a-thinned-series-keeps-its-roll-ups.md`, S5).
//! The fold's side, the stranded groups, is `stranded.rs`'s.

use std::sync::Mutex;
use std::sync::atomic::AtomicU32;

use lakelands_api::Location;
use lakelands_api::engine::{CanonicalEvent, EngineOpener, LakeEngine, OpenMode, Scan};
use lakelands_api::event::{Digest, EventId};
use lakelands_api::read::GroupKey;
use lakelands_api::seam::{VIEW_KEPT_SETTING, kept_name};
use lakelands_api::write::{Thin, ThinReport};

use super::forgotten::{Fails, failing};
use super::stranded::{HORIZON, answers, daily, stranded, tick, tick_at};
use crate::conformance::Lines;
use crate::conformance::fixture::{canonical_line, invalid_case, source_record, sourced};
use crate::conformance::prelude::*;
use crate::export;

check!(
    THIN_TAKES_THE_RAW_ROWS = a_thinning_takes_the_rows_before_its_horizon,
    "102 (lakelands#1747): of `t1` to `t3`, before the horizon, `t4` after it, `t5` with no instant and `t6` retired before it, the preflight counts four and appends nothing; the act takes the same four in one part and its `act_whole`, `find` of the type then finds `t4` and `t5` alone, at the head and as of the position before the act, `t1` is not read, and a thinning by a field of text is refused `thin_by`",
    given
);

check!(
    THIN_KEPT_VIEW_ANSWERS_AS_BEFORE = a_kept_view_answers_as_before_the_act,
    "102 (lakelands#1747): the kept `daily` over `tick` answers each group after the act as it did before it, and again after a rebuild",
    given
);

check!(
    THIN_EXACT_GROUPS_ONLY = a_thinning_that_would_strand_an_inexact_group_is_refused,
    "102 (lakelands#1747): a kept view over the type holding a group whose sum is not exact refuses the act and its preflight `kept_not_exact`, naming the view and the type, whether or not the act would strand the group, and nothing is appended",
    given
);

check!(
    THIN_PARTS_AND_WRITES = a_killed_thinning_is_finished_by_the_writers_open,
    "102 (lakelands#1747): a thinning of 600 records appends parts of at most `part_events` keys; killed after its first part, the lake verifies, a write between the parts lands, and the writer's next open finishes the act with one `act_whole`, taking each record once and not the late one; the refusal of a write at a key a later part has not reached is S6's",
    given
);

check!(
    THIN_FOREIGN_WHOLE_KEEPS_IT_OWED = a_stopped_thinning_is_not_ended_by_another_acts_whole,
    "102 (lakelands#1747): a thinning stopped after its first part, whose log's next event is another act's `act_whole`, is still owed: the writer's open finishes it, taking each key once, and the log verifies",
    given
);

check!(
    THIN_VERIFY_EXPORT_IMPORT_REBUILD = an_export_between_parts_imports_and_its_writer_finishes,
    "102 (lakelands#1747): an export taken while a thinning is owed imports with the act unwhole; the imported writer finishes it and its type finds what the lake's finds once the lake's own writer finishes it; the log verifies, and a rebuild finds the same",
    given
);

check!(
    THIN_LATE_TICK_FOLDS = a_late_tick_folds_into_its_stranded_group,
    "102 (lakelands#1747): after the act, a new `tick` before the horizon in a stranded group folds into it, and `describe_type` counts the same stranded groups",
    given
);

check!(
    THIN_ZONED_CARRIED = a_zoned_views_stranded_groups_keep_the_copy_that_cut_them,
    "102 (lakelands#1746): a kept view grouped by a day of Europe/Berlin, two of whose groups a thinning stranded, is marked below the store as cut under the copy `2000a`; the writer's open makes it again, and it answers as before: the stranded groups carry their folds and name `2000a` as the copy that cut them, and the live group is cut again under the binary's copy",
    given
);

fn by(field: &str) -> Thin {
    Thin {
        by: field.into(),
        before: Timestamp::parse(HORIZON).expect("an instant"),
    }
}

async fn thinned(store: &Store, preflight: bool) -> Result<ThinReport, Failure> {
    let (ty, thin) = (TypeName::from("tick"), by("at"));
    let r = store.thin_values(&ty, &thin, preflight).await;
    r.must("thinning `tick`")
}

/// `tick`'s `t1` to `t3` before the horizon in groups `a` and `b`, `t4`
/// after it in `b`, `t5` with no instant, and `t6` retired in `c`; `daily`
/// kept over it.
async fn ticks(l: &Lake) -> Held {
    l.declare(tick(&[])).await?;
    l.declare(daily(None, "sum(n)")).await?;
    l.set(&format!("{VIEW_KEPT_SETTING}daily"), json!(true))
        .await?;
    for (key, m, n, day) in [("t1", "a", 1, 1), ("t2", "a", 2, 2), ("t3", "b", 5, 3)] {
        tick_at(l, key, m, n, day).await?;
    }
    let late = json!({"at": "2026-07-01T00:00:00Z", "m": "b", "n": 7});
    l.upsert("tick", "t4", late).await?;
    l.upsert("tick", "t5", json!({"m": "b", "n": 9})).await?;
    tick_at(l, "t6", "c", 4, 4).await?;
    l.retire("tick", "t6").await?;
    Ok(())
}

async fn keys(l: &Lake, at: Option<Position>) -> Result<Vec<String>, Failure> {
    let found = l.all_found("tick", find_at(at)).await?;
    Ok(found.iter().map(|r| r.key.to_string()).collect())
}

async fn a_thinning_takes_the_rows_before_its_horizon(store: Store, _given: Given) -> Held {
    let l = Lake::of(store.as_ref());
    ticks(&l).await?;
    let before = store.head().await.must("reading the head")?;
    let preflight = thinned(&store, true).await?;
    let unmoved = store.head().await.must("reading the head")?;
    let report = thinned(&store, false).await?;
    let kinds: Vec<_> = (events_after(store.as_ref(), before).await?.iter())
        .map(|e| e.body.kind())
        .collect();
    let by_text = store.thin_values(&"tick".into(), &by("m"), false).await;
    let mut red = Vec::new();
    if preflight.records != 4 || unmoved != before || report.records != 4 {
        red.push(format!(
            "the preflight {preflight:?} and the act {report:?} count 4"
        ));
    }
    if kinds != ["values_thinned", "act_whole"] {
        red.push(format!("the act appends one part and its whole: {kinds:?}"));
    }
    for at in [None, Some(before)] {
        let found = keys(&l, at).await?;
        if found != ["t4", "t5"] {
            red.push(format!("`tick` as of {at:?} finds {found:?}"));
        }
    }
    if l.read("tick", "t1").await.is_ok() {
        red.push("`t1` is still read".into());
    }
    if !matches!(invalid_case(&by_text), Some(Case::ThinBy { .. })) {
        red.push(format!("thinning by `m` answers {by_text:?}"));
    }
    ensure!(
        red.is_empty(),
        "a thinning takes its rows: {}",
        red.join("; ")
    );
    Ok(())
}

async fn a_kept_view_answers_as_before_the_act(store: Store, _given: Given) -> Held {
    let l = Lake::of(store.as_ref());
    ticks(&l).await?;
    let was = answers(&l, "daily", &["c", "t"]).await?;
    thinned(&store, false).await?;
    let now = answers(&l, "daily", &["c", "t"]).await?;
    store.rebuild().await.must("rebuilding the projection")?;
    let rebuilt = answers(&l, "daily", &["c", "t"]).await?;
    ensure!(
        now == was && rebuilt == was,
        "`daily` answers {was:?} before the act, {now:?} after it, {rebuilt:?} rebuilt"
    );
    Ok(())
}

async fn a_thinning_that_would_strand_an_inexact_group_is_refused(
    store: Store,
    _given: Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    let field = |n: &str, k: &str| json!({"name": n, "type": {"kind": k}});
    l.declare_json(json!({"declare": "type", "name": "tick",
        "fields": [field("at", "date_time"), field("m", "text"), field("n", "decimal")]}))
        .await?;
    l.declare_json(json!({"declare": "type", "name": "spent", "key": ["m"],
        "fields": [field("m", "text"), field("t", "decimal")],
        "view": {"source": "tick", "group_by": ["m"],
                 "measures": [{"name": "t", "expr": "sum(n)"}]}}))
        .await?;
    l.set(&format!("{VIEW_KEPT_SETTING}spent"), json!(true))
        .await?;
    for (key, n) in [
        ("i1", "70000000000000000000000000000"),
        ("i2", "-69999999999999999999999999890"),
    ] {
        let at = "2026-01-01T00:00:00Z";
        l.upsert("tick", key, json!({"at": at, "m": "c1", "n": n}))
            .await?;
    }
    let head = store.head().await.must("reading the head")?;
    let mut red = Vec::new();
    for preflight in [true, false] {
        let r = store
            .thin_values(&"tick".into(), &by("at"), preflight)
            .await;
        let named = matches!(invalid_case(&r), Some(Case::KeptNotExact { view, ty })
            if view.as_str() == "spent" && ty.as_str() == "tick");
        if !named {
            red.push(format!("preflight {preflight}: {r:?}"));
        }
    }
    let now = store.head().await.must("reading the head")?;
    ensure!(
        red.is_empty() && now == head,
        "a thinning that would strand an inexact group is refused, appending nothing \
         (head {head} then {now}): {}",
        red.join("; ")
    );
    Ok(())
}

/// A lake whose thinning of 600 `tick`s stopped after its first part,
/// the position before the act, and its failing appends.
async fn owed(store: Store, given: &Given) -> Result<(Store, Position), Failure> {
    let fails: Fails = Arc::default();
    let store = (given.fault)(store, Seams::default(), failing(&fails))
        .await
        .map_err(|e| Failure::new(format!("the harness opened no lake: {e}")))?;
    let l = Lake::of(store.as_ref());
    let kinds = [("at", StorageKind::DateTime), ("n", StorageKind::Number)];
    let fields = kinds.iter().map(|(n, k)| sourced(n, *k)).collect();
    l.declare(type_decl("tick", fields)).await?;
    let records = (0..600)
        .map(|i| {
            source_record(
                "tick",
                &format!("r{i:03}"),
                json!({"at": "2026-01-01T00:00:00Z", "n": i}),
            )
        })
        .collect();
    l.ingest("meter", records).await.must("feeding 600 ticks")?;
    let start = store.head().await.must("reading the head")?;
    fails[12].store(2, Ordering::SeqCst);
    let stopped = thinned(&store, false).await;
    let after = events_after(store.as_ref(), start).await?;
    let kinds: Vec<_> = after.iter().map(|e| e.body.kind()).collect();
    ensure!(
        stopped.is_err() && kinds == ["values_thinned"],
        "the act stops after its first part: {stopped:?}, the log {kinds:?}"
    );
    Ok((store, start))
}

async fn a_killed_thinning_is_finished_by_the_writers_open(store: Store, given: Given) -> Held {
    let (store, start) = owed(store, &given).await?;
    store
        .verify(None, None)
        .await
        .must("verifying between parts")?;
    let late = source_record(
        "tick",
        "late",
        json!({"at": "2026-01-02T00:00:00Z", "n": 1}),
    );
    let l = Lake::of(store.as_ref());
    l.ingest("meter", vec![late])
        .await
        .must("a write between parts")?;
    let store = (given.reopen)(store, Seams::default())
        .await
        .map_err(|e| Failure::new(format!("the writer's open: {e}")))?;
    let l = Lake::of(store.as_ref());
    settles(6_000, Duration::from_millis(5), || async {
        let after = events_after(store.as_ref(), start).await?;
        let parts = after.iter().filter(|e| e.body.kind() == "values_thinned");
        let taken: usize = parts
            .map(|e| match &e.body {
                EventBody::ValuesThinned { keys, .. } => keys.len(),
                _ => 0,
            })
            .sum();
        let wholes = after
            .iter()
            .filter(|e| e.body.kind() == "act_whole")
            .count();
        let found = keys(&l, None).await?;
        ensure!(
            wholes == 1 && taken == 600 && found == ["late"],
            "the open finishes the act: {wholes} wholes, {taken} keys taken, `tick` finds {found:?}"
        );
        Ok(())
    })
    .await?;
    store
        .verify(None, None)
        .await
        .must("verifying the whole act")?;
    Ok(())
}

async fn a_stopped_thinning_is_not_ended_by_another_acts_whole(store: Store, given: Given) -> Held {
    let (store, start) = owed(store, &given).await?;
    let lake = store.lake().clone();
    let events = log_events(store.as_ref()).await?;
    let taken = |e: &&Event| matches!(e.body, EventBody::LeaseTaken { .. });
    let (Some(template), Some(last)) = (events.iter().find(taken).cloned(), events.last()) else {
        return Err(Failure::new(
            "the lake's log holds its writer's `lease_taken`",
        ));
    };
    let known_at = last.known_at;
    // The next event is an `act_whole` naming `start`, an event the
    // thinning did not begin at: another act's whole, landing right after
    // the stopped part, which must not end this act.
    let foreign = start;
    let line: Lines = Box::new(move |(tip, prev), fence| {
        let event = |position: Position, prev, body| Event {
            position,
            id: EventId(format!("{:032x}", position.0)),
            prev,
            known_at,
            fence,
            body,
            ..template.clone()
        };
        let whole = EventBody::ActWhole { act: foreign };
        (vec![canonical_line(event(tip.next(), prev, whole))], 0)
    });
    let c = (given.continued)(Some(store), lake, line, OpenMode::Write, Seams::default());
    let store = c
        .await
        .map_err(|e| Failure::new(format!("appending another act's whole: {e}")))?
        .opened
        .map_err(|e| Failure::new(format!("the writer's open: {e}")))?;
    let l = Lake::of(store.as_ref());
    settles(6_000, Duration::from_millis(5), || async {
        let after = events_after(store.as_ref(), start).await?;
        let parts: Vec<&Event> = (after.iter())
            .filter(|e| e.body.kind() == "values_thinned")
            .collect();
        let taken: usize = parts
            .iter()
            .map(|e| match &e.body {
                EventBody::ValuesThinned { keys, .. } => keys.len(),
                _ => 0,
            })
            .sum();
        let wholes = after
            .iter()
            .filter(|e| e.body.kind() == "act_whole")
            .count();
        let found = keys(&l, None).await?;
        ensure!(
            wholes == 2 && taken == 600 && found.is_empty(),
            "the open finishes the act another act's whole did not end: {wholes} wholes, \
             {taken} keys taken, `tick` finds {found:?}"
        );
        Ok(())
    })
    .await?;
    store
        .verify(None, None)
        .await
        .must("verifying the whole act")?;
    Ok(())
}

async fn an_export_between_parts_imports_and_its_writer_finishes(
    store: Store,
    given: Given,
) -> Held {
    let scratch = super::export::Scratch::new(given.scratch.clone());
    let dir = scratch.dir("thinning");
    let (store, _) = owed(store, &given).await?;
    export::write(store.as_ref(), &dir)
        .await
        .must("an export between two parts")?;
    let imported = (given.import)(dir, store.lake().clone(), None, Seams::default())
        .await
        .map_err(|e| Failure::new(format!("importing the export: {e}")))?;
    let writer = (imported.writer)(imported.store, Seams::default())
        .await
        .map_err(|e| Failure::new(format!("opening the import for writing: {e}")))?;
    let store = (given.reopen)(store, Seams::default())
        .await
        .map_err(|e| Failure::new(format!("the writer's open: {e}")))?;
    let (l, w) = (Lake::of(store.as_ref()), Lake::of(writer.as_ref()));
    settles(6_000, Duration::from_millis(5), || async {
        let (ours, theirs) = (keys(&l, None).await?, keys(&w, None).await?);
        ensure!(
            ours.is_empty() && theirs.is_empty(),
            "both writers finish the act: the lake finds {} and the import {}",
            ours.len(),
            theirs.len()
        );
        Ok(())
    })
    .await?;
    writer
        .verify(None, None)
        .await
        .must("verifying the import")?;
    store.rebuild().await.must("rebuilding the projection")?;
    let rebuilt = keys(&l, None).await?;
    ensure!(rebuilt.is_empty(), "a rebuild finds {rebuilt:?}");
    Ok(())
}

async fn a_late_tick_folds_into_its_stranded_group(store: Store, _given: Given) -> Held {
    let l = Lake::of(store.as_ref());
    ticks(&l).await?;
    thinned(&store, false).await?;
    let (was, groups) = (
        answers(&l, "daily", &["c", "t"]).await?,
        stranded(&l, "daily").await?,
    );
    tick_at(&l, "t9", "a", 10, 5).await?;
    let now = answers(&l, "daily", &["c", "t"]).await?;
    let later = stranded(&l, "daily").await?;
    ensure!(
        was.first().map(String::as_str) == Some("a 2/3")
            && now.first().map(String::as_str) == Some("a 3/13")
            && groups > 0
            && later == groups,
        "`t9` folds into `a`: {was:?} then {now:?}, stranded {groups} then {later}"
    );
    Ok(())
}

async fn a_zoned_views_stranded_groups_keep_the_copy_that_cut_them(
    store: Store,
    given: Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(tick(&[])).await?;
    let mut day = daily(None, "sum(n)");
    let v = day.view.as_mut().expect("a view");
    let term = json!({"name": "m", "expr": "date_bucket(at, 'day', 'Europe/Berlin')"});
    v.group_by = vec![serde_json::from_value::<GroupKey>(term).expect("a term")];
    day.fields[0] = field("m", StorageKind::DateTime);
    l.declare(day).await?;
    l.set(&format!("{VIEW_KEPT_SETTING}daily"), json!(true))
        .await?;
    for (key, n, day) in [("t1", 1, 1), ("t2", 2, 2)] {
        tick_at(&l, key, "-", n, day).await?;
    }
    let late = json!({"at": "2026-07-01T12:00:00Z", "m": "-", "n": 7});
    l.upsert("tick", "t3", late).await?;
    thinned(&store, false).await?;
    let was = answers(&l, "daily", &["c", "t"]).await?;
    let marks = Arc::new(Marks::default());
    let wrap: crate::conformance::Wrap = {
        let marks = marks.clone();
        Arc::new(move |inner| Arc::new(Marked(inner, marks.clone())) as Arc<dyn EngineOpener>)
    };
    let mut store = store;
    for _ in 0..2 {
        store = (given.fault)(store, Seams::default(), wrap.clone())
            .await
            .map_err(|e| Failure::new(format!("the writer's open: {e}")))?;
    }
    let l = Lake::of(store.as_ref());
    let now = answers(&l, "daily", &["c", "t"]).await?;
    let groups = stranded(&l, "daily").await?;
    let (cut, copy) = lock(&marks.seen).clone();
    ensure!(
        now == was
            && groups == 2
            && cut == [Some("2000a".into()), Some("2000a".into()), None]
            && copy.as_deref().is_some_and(|c| c != "2000a"),
        "`daily` answers {now:?} (before {was:?}), {groups} stranded; its groups name the \
         copies {cut:?}, the view {copy:?}"
    );
    Ok(())
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// What [`Marked`]'s opens did and saw: each kept group's copy, in key
/// order, and the view's.
#[derive(Default)]
struct Marks {
    opens: AtomicU32,
    seen: Mutex<(Vec<Option<String>>, Option<String>)>,
}

/// The harness's engine opener whose first writer's open marks `daily`'s
/// state cut under the copy `2000a`, and whose second reads what the
/// make between them left.
struct Marked(Arc<dyn EngineOpener>, Arc<Marks>);

impl EngineOpener for Marked {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn log_ends(
        &self,
        at: &Location,
    ) -> lakelands_api::Result<Option<(CanonicalEvent, CanonicalEvent)>> {
        self.0.log_ends(at)
    }
    fn open_scratch(&self, at: &Location) -> lakelands_api::Result<Box<dyn LakeEngine>> {
        self.0.open_scratch(at)
    }
    fn open(&self, at: &Location, mode: OpenMode) -> lakelands_api::Result<Box<dyn LakeEngine>> {
        let engine = self.0.open(at, mode)?;
        if mode != OpenMode::Write {
            return Ok(engine);
        }
        let input = Digest(lakelands_api::canonical::sha256_hex(&json!("daily")));
        let view = engine.view(None)?;
        let state = view.output("sys:kept", &input)?.map(|(_, s)| s);
        let copy = |s: &Value| s.get("tzdb").and_then(Value::as_str).map(str::to_owned);
        if self.1.opens.fetch_add(1, Ordering::SeqCst) > 0 {
            let scan = Scan {
                limit: 100,
                restricted: true,
                ..Scan::default()
            };
            let cut = (view.scan(&kept_name(&"daily".into()), &scan)?.iter())
                .map(|r| {
                    r.fields
                        .get("sys:folds")
                        .and_then(|f| f.get("cut")?.as_str().map(str::to_owned))
                })
                .collect();
            *lock(&self.1.seen) = (cut, state.as_ref().and_then(copy));
            drop(view);
            return Ok(engine);
        }
        drop(view);
        let Some(mut state) = state else {
            return Ok(engine);
        };
        state["tzdb"] = json!("2000a");
        let p = engine.projection();
        let through = p
            .folded_through()?
            .expect("the check's lake holds its writes");
        p.begin()?;
        p.put_output("sys:kept", &input, &state, through)?;
        p.commit(through)?;
        Ok(engine)
    }
}
