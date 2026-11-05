//! The thinning act itself: the walk, the first turn, its parts and its
//! `act_whole`, beside the writer, through the store's `thin_values`
//! (`docs/decisions/102-a-thinned-series-keeps-its-roll-ups.md`, S5).
//! The fold's side, the stranded groups, is `stranded.rs`'s.

use std::sync::Mutex;
use std::sync::atomic::AtomicU32;

use lakelands_api::Location;
use lakelands_api::ParamName;
use lakelands_api::describe::Kept;
use lakelands_api::engine::{CanonicalEvent, EngineOpener, LakeEngine, OpenMode, Scan};
use lakelands_api::event::{Digest, EventId};
use lakelands_api::ontology::ViewParam;
use lakelands_api::read::GroupKey;
use lakelands_api::seam::{SETTING_TYPE, VIEW_KEPT_SETTING, kept_name};
use lakelands_api::write::{DeclarationKind, Thin, ThinReport, Withdrawal};

use super::forgotten::{Fails, failing};
use super::stranded::{HORIZON, answers, daily, stranded, tick, tick_at};
use crate::conformance::Lines;
use crate::conformance::fixture::{canonical_line, get_at, invalid_case, source_record, sourced};
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
    "102 (lakelands#1746): a kept view grouped by a day of Europe/Berlin, two of whose groups a thinning stranded, is marked below the store as cut under the copy `2000a`; the writer's open makes it again, and it answers as before: the stranded groups carry their folds and name `2000a` as the copy that cut them, and the live group is cut again under the binary's copy; a read of the view and `describe` name `2000a` (lakelands#1749)",
    given
);

check!(
    THIN_CORRECTION_REFUSED = a_write_an_ingest_and_a_retirement_at_a_thinned_key_are_refused,
    "102 (lakelands#1748): after a thinning takes `t1` and leaves `t2`, an upsert, an ingest and a retirement at `t1` are each refused `thinned`, naming the type, the key and the horizon; nothing is appended, and `tick` finds `t2` alone before and after",
    given
);

check!(
    THIN_VIEW_KEPT_AFTER_REFUSED = a_view_made_kept_over_a_thinned_type_is_refused,
    "102 (lakelands#1748): a `view_kept` setting that would make a view over a thinned type kept, a declaration of it asked and not whole, and a migration whose step declares it again are each refused `kept_over_thinned`, naming the view and the type, and append nothing; a view over a view whose group the act stranded is declared again and is not refused",
    given
);

check!(
    THIN_OWED_ACT_REFUSES_A_LATER_KEY = a_write_at_a_key_a_later_part_has_not_reached_is_refused,
    "102 (lakelands#1748): a thinning of 600 records stopped after its first part refuses an ingest at a key the act's selection at its first position holds and no part has reached, at the first and the last of those, and a retirement of one, each naming the type, the key and the horizon, and appends nothing; a key the selection does not hold lands, and the writer's open finishes the act, taking each of the 600 once and finding the late key alone",
    given
);

check!(
    THIN_AS_OF_SAYS_UNANSWERABLE = a_read_before_the_act_says_what_it_cannot_answer,
    "102 (lakelands#1749): after the act, `daily`'s page at the head names the act and its stranded groups say so; as of the position before it, `daily` and a view `rolled` over it answer `d`, the group the act did not strand, and count the three `describe` counts stranded, and a `get` of a stranded group is refused `stranded_at_head_only` naming `daily`, of `rolled` too, while `t1` as of then is refused `thinned`; `describe` of `tick` names the act",
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
    let read: Vec<_> = (l.all_found("daily", find_at(None)).await?.into_iter())
        .map(|r| r.cut)
        .collect();
    let told = l.describe_type("daily").await?.cut;
    ensure!(
        read == cut && told == ["2000a"],
        "a read of `daily` names the copies {read:?}, `describe` {told:?}, not {cut:?}"
    );
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

/// A series type `name`: its instants `at`, its group `m`, its value `n`
/// and a text `note`.
fn series(name: &str) -> TypeDecl {
    type_decl(
        name,
        vec![
            field("at", StorageKind::DateTime),
            field("m", StorageKind::Text),
            field("n", StorageKind::Number),
            field("note", StorageKind::Text),
        ],
    )
}

/// The view `view` over `source` by `m`: `c` its count, `t` the sum of
/// `sum`, a field the source holds.
fn over(source: &str, view: &str, sum: &str) -> TypeDecl {
    let fields = vec![
        field("m", StorageKind::Text),
        field("c", StorageKind::Number),
        field("t", StorageKind::Number),
    ];
    view_of(
        view,
        source,
        &["m"],
        fields,
        &[("c", "count()"), ("t", &format!("sum({sum})"))],
    )
}

/// The withdrawal of the type `name`.
fn withdraw(name: &str) -> Declaration {
    Declaration::Withdraw(Withdrawal {
        kind: DeclarationKind::Type,
        name: name.into(),
    })
}

/// Runs a thinning of `ty` by `at` at the horizon below the store.
async fn cut(store: &Store, ty: &str) -> Result<(), Failure> {
    let r = store
        .thin_values(&TypeName::from(ty), &by("at"), false)
        .await;
    r.must(&format!("thinning `{ty}`"))?;
    Ok(())
}

async fn a_write_an_ingest_and_a_retirement_at_a_thinned_key_are_refused(
    store: Store,
    _given: Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    let shared = |n: &str, k: StorageKind| FieldDecl {
        authority: Authority::Shared,
        ..field(n, k)
    };
    l.declare(type_decl(
        "tick",
        vec![
            shared("at", StorageKind::DateTime),
            shared("m", StorageKind::Text),
            shared("n", StorageKind::Number),
        ],
    ))
    .await?;
    l.upsert(
        "tick",
        "t1",
        json!({"at": "2026-01-01T00:00:00Z", "m": "a", "n": 1}),
    )
    .await?;
    let late = json!({"at": "2026-07-01T00:00:00Z", "m": "a", "n": 2});
    l.upsert("tick", "t2", late.clone()).await?;
    thinned(&store, false).await?;
    let head = store.head().await.must("reading the head")?;
    let was = keys(&l, None).await?;
    let (ty, key) = (TypeName::from("tick"), RecordKey::from("t1"));
    let write = try_upsert(
        l.cat.as_ref(),
        &l.own,
        "tick",
        "t1",
        json!({"at": "2026-01-01T00:00:00Z", "m": "a", "n": 3}),
    )
    .await;
    let fed = l
        .ingest("meter", vec![source_record("tick", "t1", late)])
        .await;
    let retired = l.cat.retire(&l.own, &ty, &key, WriteOpts::default()).await;
    let later = store.head().await.must("reading the head")?;
    let is = keys(&l, None).await?;
    let horizon = Timestamp::parse(HORIZON).expect("an instant");
    let mut red = Vec::new();
    for (what, said) in [
        ("the write", invalid_case(&write)),
        ("the ingest", invalid_case(&fed)),
        ("the retirement", invalid_case(&retired)),
    ] {
        let named = matches!(said, Some(Case::Thinned { ty, key, before })
            if ty.as_str() == "tick" && key.as_str() == "t1" && before == &horizon);
        if !named {
            red.push(format!("{what} answers {said:?}"));
        }
    }
    if later != head || is != was {
        red.push(format!(
            "the refusals moved the lake: head {head} then {later}, `tick` {was:?} then {is:?}"
        ));
    }
    ensure!(
        red.is_empty(),
        "a write, an ingest and a retirement at a thinned key are refused: {}",
        red.join("; ")
    );
    Ok(())
}

async fn a_view_made_kept_over_a_thinned_type_is_refused(store: Store, _given: Given) -> Held {
    let l = Lake::of(store.as_ref());
    let jan = json!({"at": "2026-01-01T00:00:00Z", "m": "a", "n": 1});
    let jul = json!({"at": "2026-07-01T00:00:00Z", "m": "a", "n": 2});
    fn named(said: Option<&Case>, view: &str, ty: &str) -> bool {
        matches!(said, Some(Case::KeptOverThinned { view: v, ty: t })
            if v.as_str() == view && t.as_str() == ty)
    }
    let mut red = Vec::new();
    // A setting that would make a view kept, its source thinned below it:
    // the view was never asked, so the setting is the event that makes it.
    l.declare(series("one")).await?;
    l.declare(over("one", "by_one", "n")).await?;
    l.upsert("one", "o1", jan.clone()).await?;
    l.upsert("one", "o2", jul.clone()).await?;
    cut(&store, "one").await?;
    let head = store.head().await.must("reading the head")?;
    let key = format!("{VIEW_KEPT_SETTING}by_one");
    let set = try_upsert(
        l.cat.as_ref(),
        &l.own,
        SETTING_TYPE,
        &key,
        json!({"key": key, "value": true}),
    )
    .await;
    if !named(invalid_case(&set), "by_one", "one") {
        red.push(format!("the setting answers {set:?}"));
    }
    let moved = store.head().await.must("reading the head")?;
    if moved != head {
        red.push(format!(
            "the refused setting moved the head from {head} to {moved}"
        ));
    }
    // A declaration that would make it kept: asked, withdrawn while the
    // act ran, then declared again over the thinned type.
    l.declare(series("two")).await?;
    l.declare(over("two", "by_two", "n")).await?;
    l.set(&format!("{VIEW_KEPT_SETTING}by_two"), json!(true))
        .await?;
    l.upsert("two", "p1", jan.clone()).await?;
    l.upsert("two", "p2", jul.clone()).await?;
    l.declare_as(withdraw("by_two"), "withdrawing `by_two`")
        .await?;
    cut(&store, "two").await?;
    let head = store.head().await.must("reading the head")?;
    let again = l.try_declare(over("two", "by_two", "n")).await;
    if !named(invalid_case(&again), "by_two", "two") {
        red.push(format!("the declaration answers {again:?}"));
    }
    let moved = store.head().await.must("reading the head")?;
    if moved != head {
        red.push(format!(
            "the refused declaration moved the head from {head} to {moved}"
        ));
    }
    // A migration's step that declares it again: asked, but a view with a
    // parameter is never kept, so the step is what would make it.
    l.declare(series("three")).await?;
    l.declare(over("three", "by_three", "n")).await?;
    l.set(&format!("{VIEW_KEPT_SETTING}by_three"), json!(true))
        .await?;
    l.upsert("three", "q1", jan.clone()).await?;
    l.upsert("three", "q2", jul.clone()).await?;
    let mut windowed = over("three", "by_three", "n");
    let view = windowed.view.as_mut().expect("a view");
    view.params = vec![ViewParam {
        name: ParamName::from("only"),
        kind: StorageKind::Number,
    }];
    view.r#where = Some("n >= param('only')".into());
    l.declare(windowed).await?;
    cut(&store, "three").await?;
    let head = store.head().await.must("reading the head")?;
    let mut to = series("three");
    for f in &mut to.fields {
        if f.name.as_str() == "note" {
            f.name = "remark".into();
        }
    }
    let mut m = migration(to);
    m.renames = vec![("note".into(), "remark".into())];
    m.views = vec![over("three", "by_three", "n")];
    let applied = match l.cat.migrate(&l.own, m.clone()).await {
        Ok(reply) => l
            .cat
            .migrate_apply(&l.own, m, &reply.data.report_id)
            .await
            .map(|_| ()),
        Err(e) => Err(e),
    };
    if !named(invalid_case(&applied), "by_three", "three") {
        red.push(format!("the migration answers {applied:?}"));
    }
    let moved = store.head().await.must("reading the head")?;
    if moved != head {
        red.push(format!(
            "the refused migration moved the head from {head} to {moved}"
        ));
    }
    // A view over a view, whose group the act stranded, is declared again
    // under its setting and is not refused.
    l.declare(series("four")).await?;
    l.declare(over("four", "by_four", "n")).await?;
    l.declare(over("by_four", "over_four", "t")).await?;
    l.set(&format!("{VIEW_KEPT_SETTING}by_four"), json!(true))
        .await?;
    l.set(&format!("{VIEW_KEPT_SETTING}over_four"), json!(true))
        .await?;
    l.upsert("four", "w1", jan).await?;
    l.upsert("four", "w2", jul).await?;
    cut(&store, "four").await?;
    let was = answers(&l, "over_four", &["c", "t"]).await?;
    l.declare_as(withdraw("over_four"), "withdrawing `over_four`")
        .await?;
    let again = l.declare(over("by_four", "over_four", "t")).await;
    let now = answers(&l, "over_four", &["c", "t"]).await?;
    let groups = stranded(&l, "over_four").await?;
    let kept = l.describe_type("over_four").await?.lifecycle.kept;
    if again.is_err() || now != was || groups == 0 || !matches!(kept, Some(Kept::Kept)) {
        red.push(format!(
            "`over_four`, over the kept `by_four` whose group the act stranded, {again:?}, \
             answers {now:?} (before {was:?}), {groups} stranded, {kept:?}"
        ));
    }
    ensure!(
        red.is_empty(),
        "a view made kept over a thinned type is refused: {}",
        red.join("; ")
    );
    Ok(())
}

async fn a_write_at_a_key_a_later_part_has_not_reached_is_refused(
    store: Store,
    given: Given,
) -> Held {
    let (store, start) = owed(store, &given).await?;
    let l = Lake::of(store.as_ref());
    let head = store.head().await.must("reading the head")?;
    let horizon = Timestamp::parse(HORIZON).expect("an instant");
    let jan = json!({"at": "2026-01-01T00:00:00Z", "n": 1});
    let fed = l
        .ingest("meter", vec![source_record("tick", "r550", jan.clone())])
        .await;
    let last = l
        .ingest("meter", vec![source_record("tick", "r599", jan.clone())])
        .await;
    let retired = l
        .cat
        .retire(
            &l.own,
            &TypeName::from("tick"),
            &RecordKey::from("r570"),
            WriteOpts::default(),
        )
        .await;
    let later = store.head().await.must("reading the head")?;
    let mut red = Vec::new();
    for (what, key, said) in [
        ("the ingest at `r550`", "r550", invalid_case(&fed)),
        ("the ingest at `r599`", "r599", invalid_case(&last)),
        ("the retirement at `r570`", "r570", invalid_case(&retired)),
    ] {
        let named = matches!(said, Some(Case::Thinned { ty, key: k, before })
            if ty.as_str() == "tick" && k.as_str() == key && before == &horizon);
        if !named {
            red.push(format!("{what} answers {said:?}"));
        }
    }
    if later != head {
        red.push(format!(
            "the refusals moved the head from {head} to {later}"
        ));
    }
    ensure!(
        red.is_empty(),
        "a write at a key a later part has not reached is refused: {}",
        red.join("; ")
    );
    // A key the act's selection at its first position does not hold — one
    // the log never saw before the act — lands.
    let fresh = source_record("tick", "new", json!({"at": "2026-01-03T00:00:00Z", "n": 2}));
    l.ingest("meter", vec![fresh])
        .await
        .must("a key the act does not take")?;
    let store = (given.reopen)(store, Seams::default())
        .await
        .map_err(|e| Failure::new(format!("the writer's open: {e}")))?;
    let l = Lake::of(store.as_ref());
    settles(6_000, Duration::from_millis(5), || async {
        let after = events_after(store.as_ref(), start).await?;
        let taken: usize = after
            .iter()
            .filter(|e| e.body.kind() == "values_thinned")
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
            wholes == 1 && taken == 600 && found == ["new"],
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

/// The page `find` answers for `ty` as of `at`, its rows' keys with
/// their stranded mark, and its acts and stranded count.
async fn page(l: &Lake, ty: &str, at: Option<Position>) -> Result<String, Failure> {
    let found = l.cat.find(&l.own, &TypeName::from(ty), find_at(at)).await;
    match found.must(&format!("finding `{ty}` as of {at:?}"))?.data {
        Found::Records {
            rows,
            thinned,
            stranded,
            ..
        } => {
            let rows: Vec<_> = rows.iter().map(|r| (r.key.as_str(), r.stranded)).collect();
            let acts: Vec<_> = thinned.iter().map(|a| (a.ty.as_str(), a.gone)).collect();
            Ok(format!("{rows:?} {acts:?} {stranded:?}"))
        }
        groups => Err(Failure::new(format!("`{ty}` answers {groups:?}"))),
    }
}

async fn a_read_before_the_act_says_what_it_cannot_answer(store: Store, _given: Given) -> Held {
    let l = Lake::of(store.as_ref());
    ticks(&l).await?;
    l.declare(over("daily", "rolled", "t")).await?;
    let late = json!({"at": "2026-07-01T00:00:00Z", "m": "d", "n": 3});
    l.upsert("tick", "t7", late).await?;
    let before = store.head().await.must("reading the head")?;
    thinned(&store, false).await?;
    let mut red = Vec::new();
    for (ty, at, want) in [
        (
            "daily",
            None,
            r#"[("a", true), ("b", true), ("d", false)] [("tick", 4)] None"#,
        ),
        (
            "daily",
            Some(before),
            r#"[("d", false)] [("tick", 4)] Some(3)"#,
        ),
        (
            "rolled",
            Some(before),
            r#"[("d", false)] [("tick", 4)] Some(3)"#,
        ),
    ] {
        let now = page(&l, ty, at).await?;
        if now != want {
            red.push(format!("`{ty}` as of {at:?} answers {now}, not {want}"));
        }
    }
    for (ty, key) in [("daily", "a"), ("rolled", "a"), ("tick", "t1")] {
        let (t, at) = (TypeName::from(ty), get_at(key, Some(before)));
        let got = l.cat.get(&l.own, &t, at).await;
        let said = match invalid_case(&got) {
            Some(Case::StrandedAtHeadOnly { ty: v, position }) => {
                ty != "tick" && v.as_str() == "daily" && *position == before
            }
            Some(Case::Thinned { ty: t, key: k, .. }) => t.as_str() == ty && k.as_str() == key,
            _ => false,
        };
        if !said {
            red.push(format!("`{ty}` `{key}` as of {before:?} answers {got:?}"));
        }
    }
    let (acts, groups) = (
        l.describe_type("tick").await?.thinned,
        stranded(&l, "daily").await?,
    );
    if acts.len() != 1 || groups != 3 {
        red.push(format!(
            "`tick` is thinned by {acts:?}, `daily` strands {groups}"
        ));
    }
    ensure!(red.is_empty(), "a read before the act: {}", red.join("; "));
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
