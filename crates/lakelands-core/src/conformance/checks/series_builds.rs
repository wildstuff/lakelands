//! A `series_index.<type>` setting's build runs beside the writer
//! (`docs/decisions/097-a-read-in-a-window-walks-its-window.md` §How a
//! lake with records gets it).
//!
//! Writing the setting on a type with records marks its pair building in
//! the projection's `series.<type>` row, and a job on the road fills it a
//! key range of `limits.order_chunk` records a turn and keeps it. These
//! checks hold what a caller sees: the pair is told `building` while it is
//! filled and `kept` once it is whole, every read answers as before
//! throughout, a lake whose writer went between two turns finishes the
//! build at its next open without a write, a projection rebuilt from the
//! log holds the pair kept at its event, and taking the setting back
//! leaves no pair and every answer as it was.

use lakelands_api::describe::SeriesToldState;
use lakelands_api::engine::OpenMode;
use lakelands_api::seam::SETTING_TYPE;
use std::sync::atomic::AtomicBool;

use super::builds::{First, answers, chunked, rows, until_first};
use super::forgotten::{Fails, failing};
use crate::conformance::fixture::reviewer;
use crate::conformance::fixture::{doors, owner, type_of};
use crate::conformance::prelude::*;

check!(
    SERIES_SETTING_BUILDS = a_series_setting_fills_its_pair_beside_writes_and_keeps_it,
    "097, step 8: a `series_index.<type>` setting on a type with records builds its pair in parts of `limits` beside the writer, and its door answers when the pair is whole: `describe` tells the pair `building` meanwhile and `kept` once the door answered, and every page and `where` beside the build answers what it answered before it",
    given
);

check!(
    SERIES_BUILD_RESUMES = a_series_build_stopped_between_turns_is_finished_by_the_next_open,
    "097, step 8: a series pair whose build a writer left between two of its turns is owed in the projection's mark, and the next open finishes it with no read and no further write: the reopened lake answers at once as before, the pair is told `kept` without a setting written to it, and every read answers as before",
    given
);

check!(
    SERIES_TAKEN_BACK = a_series_setting_taken_back_leaves_no_pair_and_no_answer_moved,
    "097, step 8: a `series_index.<type>` setting taken back during its build and again after it is whole ends clean: `describe` names the pair no longer, every read taken beside the take-back answers what it answered before the setting, and every read after it does too",
    given
);

check!(
    SERIES_REBUILD_SAME_ROWS = a_rebuild_holds_the_series_pair_kept_at_its_event,
    "097, step 8: a `series_index.<type>` setting is in the log and the build is not, so a projection rebuilt from the log fills the pair whole at the setting's event and holds it `kept`, with no build owed; every read answers what the built lake answered",
    given
);

check!(
    SERIES_BUILD_REGAINED =
        a_series_build_owed_to_a_lease_the_writer_lost_is_finished_when_it_takes_it_again,
    "097, step 8: a series pair whose build stopped between two turns is owed in the projection's mark, and a writer that loses the lease and takes it again in the same process finishes the build on the write that takes it — with no read and no setting written to ask for it: the pair is told `kept` and every read answers as before",
    given
);

check!(
    SERIES_TOLD = describe_tells_a_series_pair_to_a_holder_of_schema_reading_both_fields,
    "097, step 8: `describe` tells a `series_index.<type>` pair, its series field and its order field, with the state `building` then `kept`, to a holder of `schema` who reads both fields, at the head only; a reader without `schema`, and one who reads only one of the two fields, is told no pair",
    given
);

/// The pair `series`/`order` of `row` as `who` is told it: its state, or
/// none when the pair is not in `describe`'s list.
async fn pair(l: &Lake, who: &Principal) -> Result<Option<SeriesToldState>, Failure> {
    let d = describe_type(l.cat.as_ref(), who, "row").await?;
    Ok(d.series
        .iter()
        .find(|p| p.series.as_str() == "s" && p.order.as_str() == "n")
        .map(|p| p.state))
}

/// Every pair `who` is told of `row`, as `series`, `order`, `state`.
async fn told(
    l: &Lake,
    who: &Principal,
) -> Result<Vec<(String, String, SeriesToldState)>, Failure> {
    let d = describe_type(l.cat.as_ref(), who, "row").await?;
    Ok(d.series
        .into_iter()
        .map(|p| (p.series.to_string(), p.order.to_string(), p.state))
        .collect())
}

/// Wait, up to a minute, for `describe` to name the pair of `row` `want`:
/// a build is filling it, or it is whole.
async fn until_told(l: &Lake, who: &Principal, want: SeriesToldState) -> Result<bool, Failure> {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while std::time::Instant::now() < deadline {
        if pair(l, who).await? == Some(want) {
            return Ok(true);
        }
        pause(Duration::from_millis(5)).await;
    }
    Ok(false)
}

/// What `describe` was told, briefly: how many answers, the first and the
/// last of them.
fn said(seen: &[Vec<(String, String, SeriesToldState)>]) -> String {
    format!(
        "{} describes; the first {:?}, the last {:?}",
        seen.len(),
        seen.first(),
        seen.last()
    )
}

/// The setting's value: the pair `s` as its series, `n` as its order.
fn setting() -> Value {
    json!([["s", "n"]])
}

async fn a_series_setting_fills_its_pair_beside_writes_and_keeps_it(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.clamp(200, 400));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    let before = answers(&l, size).await?;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let reader = {
        let seen = seen.clone();
        let l = &l;
        async move {
            ensure!(
                until_told(l, &l.own, SeriesToldState::Building).await?,
                "`describe` never told the pair `building`, so no build began"
            );
            for _ in 0..20 {
                let got = answers(l, size).await?;
                seen.lock().expect("the answers").push(got);
                pause(Duration::from_millis(1)).await;
            }
            Ok::<_, Failure>(())
        }
    };
    let setter = {
        let l = &l;
        async move {
            l.set("series_index.row", setting()).await?;
            Ok::<_, Failure>(())
        }
    };
    let out = join_all(vec![Box::pin(setter), Box::pin(reader)]).await;
    out[0]
        .as_ref()
        .map_err(|e| Failure::new(format!("the setting: {e}")))?;
    out[1]
        .as_ref()
        .map_err(|e| Failure::new(format!("a read: {e}")))?;
    let during = seen.lock().expect("the answers").clone();
    for (i, got) in during.iter().enumerate() {
        ensure!(
            *got == before,
            "read {i} beside the build of the pair answered differently from the lake before \
             the setting.\n  before: {before:?}\n  beside: {got:?}"
        );
    }
    ensure!(
        pair(&l, &l.own).await? == Some(SeriesToldState::Kept),
        "the setting's door answers when its pair is whole, and `describe` must then tell it \
         `kept`; it told {:?}",
        pair(&l, &l.own).await?
    );
    let after = answers(&l, size).await?;
    ensure!(
        after == before,
        "once the pair is whole, every read answers what the lake answered before the \
         setting: {before:?} became {after:?}"
    );
    Ok(())
}

async fn a_series_build_stopped_between_turns_is_finished_by_the_next_open(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.clamp(400, 800));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    let before = answers(&l, size).await?;
    let lake = store.lake().clone();
    // The setting's door appends its record, its fold marks the pair
    // building and sends the build; the caller then goes, as a process
    // that ended between two of the build's turns.
    {
        let l = &l;
        let made = l.set("series_index.row", setting());
        let done = async {
            ensure!(
                until_told(l, &l.own, SeriesToldState::Building).await?,
                "`describe` never told the pair `building`, so no build began"
            );
            Ok::<_, Failure>(())
        };
        match until_first(made, done).await {
            First::Done(seen) => seen?,
            First::Made(_) => {
                return Err(Failure::new(
                    "the pair was whole before the lake was let go, so nothing was owed",
                ));
            }
        }
    }
    drop(l);
    drop(store);
    // The next open of the same lake, a new writer: it finds the build
    // owed in the mark and sends it, and waits for no read to ask.
    let none: crate::conformance::Lines = Box::new(|_, _| (Vec::new(), 0));
    let continued = (given.continued)(None, lake, none, OpenMode::Write, Seams::default()).await;
    let continued = continued.map_err(|e| Failure::new(format!("opening the lake again: {e}")))?;
    let store = continued
        .opened
        .map_err(|e| Failure::new(format!("opening the lake again: {e}")))?;
    let l = Lake::of(store.as_ref());
    let during = answers(&l, size).await?;
    ensure!(
        during == before,
        "a lake reopened with a build owed answers at once as it answered before the \
         setting: {before:?} became {during:?}"
    );
    ensure!(
        until_told(&l, &l.own, SeriesToldState::Kept).await?,
        "the open did not finish the pair it owed: no read asked for it and no setting was \
         written, and `describe` never told it `kept`"
    );
    let after = answers(&l, size).await?;
    ensure!(
        after == before,
        "once the build the open took up is whole, every read answers as before: {before:?} \
         became {after:?}"
    );
    Ok(())
}

async fn a_series_setting_taken_back_leaves_no_pair_and_no_answer_moved(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.clamp(400, 800));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    let before = answers(&l, size).await?;
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let reader = {
        let seen = seen.clone();
        let l = &l;
        async move {
            ensure!(
                until_told(l, &l.own, SeriesToldState::Building).await?,
                "`describe` never told the pair `building`, so no build began"
            );
            for _ in 0..40 {
                let got = answers(l, size).await?;
                seen.lock().expect("the answers").push(got);
                pause(Duration::from_millis(1)).await;
            }
            Ok::<_, Failure>(())
        }
    };
    let setter = {
        let l = &l;
        async move {
            l.set("series_index.row", setting()).await?;
            Ok::<_, Failure>(())
        }
    };
    // Taken back between two of the build's turns: the write that retires
    // the setting lands while the pair is building.
    let taker = {
        let l = &l;
        async move {
            ensure!(
                until_told(l, &l.own, SeriesToldState::Building).await?,
                "`describe` never told the pair `building`, so no build began"
            );
            l.retire(SETTING_TYPE, "series_index.row").await?;
            Ok::<_, Failure>(())
        }
    };
    let out = join_all(vec![Box::pin(setter), Box::pin(taker), Box::pin(reader)]).await;
    out[0]
        .as_ref()
        .map_err(|e| Failure::new(format!("the setting: {e}")))?;
    out[1]
        .as_ref()
        .map_err(|e| Failure::new(format!("the take-back: {e}")))?;
    out[2]
        .as_ref()
        .map_err(|e| Failure::new(format!("a read: {e}")))?;
    let during = seen.lock().expect("the answers").clone();
    for (i, got) in during.iter().enumerate() {
        ensure!(
            *got == before,
            "read {i} beside the build taken back answered differently from the lake before \
             the setting.\n  before: {before:?}\n  beside: {got:?}"
        );
    }
    ensure!(
        told(&l, &l.own).await?.is_empty(),
        "a pair taken back leaves `describe`'s list; it told {:?}",
        told(&l, &l.own).await?
    );
    let after = answers(&l, size).await?;
    ensure!(
        after == before,
        "once the setting taken back is gone, every read answers what it answered before it: \
         {before:?} became {after:?}"
    );
    // And after a whole build, the same take-back leaves the list empty and
    // every answer as it was.
    l.set("series_index.row", setting()).await?;
    ensure!(
        pair(&l, &l.own).await? == Some(SeriesToldState::Kept),
        "the setting set again builds its pair whole; `describe` told {:?}",
        pair(&l, &l.own).await?
    );
    let built = answers(&l, size).await?;
    ensure!(
        built == before,
        "a whole pair changes no answer before a read takes it: {before:?} became {built:?}"
    );
    l.retire(SETTING_TYPE, "series_index.row").await?;
    ensure!(
        told(&l, &l.own).await?.is_empty(),
        "a whole pair taken back leaves `describe`'s list; it told {:?}",
        told(&l, &l.own).await?
    );
    let after = answers(&l, size).await?;
    ensure!(
        after == before,
        "after a whole pair is taken back, every read answers what it answered before the \
         setting: {before:?} became {after:?}"
    );
    Ok(())
}

async fn a_rebuild_holds_the_series_pair_kept_at_its_event(store: Store, given: Given) -> Held {
    let size = u64::from(given.large_lake.clamp(200, 400));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    let before = answers(&l, size).await?;
    l.set("series_index.row", setting()).await?;
    ensure!(
        pair(&l, &l.own).await? == Some(SeriesToldState::Kept),
        "the built lake's pair is whole; `describe` told {:?}",
        pair(&l, &l.own).await?
    );
    let built = answers(&l, size).await?;
    ensure!(
        built == before,
        "the built lake answers what it answered before the setting: {before:?} became \
         {built:?}"
    );
    store.rebuild().await.must("rebuilding the projection")?;
    ensure!(
        pair(&l, &l.own).await? == Some(SeriesToldState::Kept),
        "a projection rebuilt from the log fills the pair whole at the setting's event, so \
         `describe` must tell it `kept` and owe no build; it told {:?}",
        pair(&l, &l.own).await?
    );
    let rebuilt = answers(&l, size).await?;
    ensure!(
        rebuilt == before,
        "a projection rebuilt from the log answers what the built lake answered: {before:?} \
         became {rebuilt:?}"
    );
    Ok(())
}

async fn describe_tells_a_series_pair_to_a_holder_of_schema_reading_both_fields(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.clamp(200, 400));
    let store = chunked(&given, store).await?;
    let l = rows(store.as_ref(), size).await?;
    let schema = reviewer(store.as_ref(), true);
    let plain = reviewer(store.as_ref(), false);
    // A holder of `schema` who reads the series field and not the order
    // field: the pair is not told to it.
    let half = {
        let mut grants = Grants {
            default: Access::Read,
            review: true,
            schema: true,
            restricted: true,
            ..Grants::default()
        };
        grants.types.insert(
            TypeName::from("row"),
            TypeGrants {
                default: Some(Access::Read),
                fields: [("n".into(), Access::None)].into(),
                ..Default::default()
            },
        );
        principal(store.as_ref(), PrincipalKind::Agent, "half", grants)
    };
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    // The setting's door answers when the pair is whole, so the reader
    // follows the pair until that answer and reads it once more then,
    // rather than counting polls against a build that may outlast them.
    let done = Arc::new(AtomicBool::new(false));
    let reader = {
        let seen = seen.clone();
        let done = done.clone();
        let schema = schema.clone();
        let l = &l;
        async move {
            ensure!(
                until_told(l, &schema, SeriesToldState::Building).await?,
                "`describe` never told the pair `building` to a holder of `schema`"
            );
            for _ in 0..40 {
                let told = told(l, &schema).await?;
                seen.lock().expect("the pair").push(told);
                pause(Duration::from_millis(1)).await;
            }
            // The build may outlast the reads beside it: wait for the
            // setting's door, which answers only when the pair is whole,
            // reading nothing meanwhile so the build is not held back by
            // a reader, then read the pair once more.
            let deadline = std::time::Instant::now() + Duration::from_secs(300);
            while !done.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
                pause(Duration::from_millis(10)).await;
            }
            let told = told(l, &schema).await?;
            seen.lock().expect("the pair").push(told);
            Ok::<_, Failure>(())
        }
    };
    let at = Arc::new(std::sync::Mutex::new(None));
    let setter = {
        let l = &l;
        let at = at.clone();
        let done = done.clone();
        async move {
            let out = l.set("series_index.row", setting()).await;
            done.store(true, Ordering::SeqCst);
            let out = out?;
            at.lock().expect("the setting").replace(out.position());
            Ok::<_, Failure>(())
        }
    };
    let out = join_all(vec![Box::pin(setter), Box::pin(reader)]).await;
    out[0]
        .as_ref()
        .map_err(|e| Failure::new(format!("the setting: {e}")))?;
    out[1]
        .as_ref()
        .map_err(|e| Failure::new(format!("a `describe`: {e}")))?;
    let during = seen.lock().expect("the pair").clone();
    ensure!(
        during
            .iter()
            .any(|t| t == &[("s".into(), "n".into(), SeriesToldState::Building)]),
        "a holder of `schema` who reads both fields is told the pair `building` while it is \
         filled; {}",
        said(&during)
    );
    ensure!(
        during
            .iter()
            .any(|t| t == &[("s".into(), "n".into(), SeriesToldState::Kept)]),
        "and `kept` once it is whole; {}",
        said(&during)
    );
    ensure!(
        told(&l, &plain).await?.is_empty(),
        "a reader without `schema` is told no pair; it was told {:?}",
        told(&l, &plain).await?
    );
    ensure!(
        told(&l, &half).await?.is_empty(),
        "a holder of `schema` who reads only one of the pair's fields is told no pair; it was \
         told {:?}",
        told(&l, &half).await?
    );
    // At the head only: a read pinned to an `as_of` — here the setting's
    // own write's predecessor — is told no pair.
    let since = at
        .lock()
        .expect("the setting")
        .expect("the setting's position");
    let then = l
        .cat
        .describe_type(
            &schema,
            &TypeName::from("row"),
            ReadAt {
                as_of: Some(Position(since.0.saturating_sub(1))),
                valid_at: None,
                clock: None,
            },
        )
        .await
        .must("describing `row` as of before the setting")?
        .data;
    ensure!(
        then.series.is_empty(),
        "a read as of {} tells no pair; it was told {:?}",
        since.0.saturating_sub(1),
        then.series
    );
    Ok(())
}

async fn a_series_build_owed_to_a_lease_the_writer_lost_is_finished_when_it_takes_it_again(
    store: Store,
    given: Given,
) -> Held {
    let size = u64::from(given.large_lake.clamp(200, 400));
    let fails: Fails = Arc::default();
    // The lake is opened once, through the faults: a reopen for limits
    // would open it without them, and the build this check stops is the
    // one its setting's write runs, whole or in parts alike.
    let opened = (given.fault)(store, Seams::default(), failing(&fails));
    let store = opened
        .await
        .map_err(|e| Failure::new(format!("opening the lake for writing: {e}")))?;
    let l = rows(store.as_ref(), size).await?;
    let before = answers(&l, size).await?;
    l.declare(type_of("note", &[("text", StorageKind::Text)]))
        .await?;
    let (cat, who) = (doors(store.as_ref()), owner(store.as_ref()));
    // The setting's write appends its record and marks the pair building;
    // its build's first step fails, so the pair is left owed with no job
    // behind it, and the door answers that failure. Every step fails
    // while the write runs, so a job the lake sent before it cannot take
    // the one failure and let this build through.
    fails[11].store(1_000, Ordering::SeqCst);
    let set = l.set("series_index.row", setting()).await;
    fails[11].store(0, Ordering::SeqCst);
    ensure!(
        set.is_err(),
        "the setting's write must answer the failure of its build's first step: it answered \
         {set:?}"
    );
    ensure!(
        pair(&l, &l.own).await? == Some(SeriesToldState::Building),
        "a build stopped at its first step leaves its pair building, owed in the mark; \
         `describe` told {:?}",
        pair(&l, &l.own).await?
    );
    // Another writer takes the lease: the next append is refused and the
    // handle is no writer.
    fails[8].store(1, Ordering::SeqCst);
    let taken = try_upsert(cat.as_ref(), &who, "note", "n1", json!({"text": "a"})).await;
    ensure!(
        taken.is_err() && !store.is_writer(),
        "a write once another writer took the lease is refused and leaves the handle no \
         writer: it answered {taken:?}, the handle a writer: {}",
        store.is_writer()
    );
    // The other writer gives the lease up: the next write takes it again,
    // and the entry that takes it finishes what the mark owes.
    fails[9].store(1, Ordering::SeqCst);
    try_upsert(cat.as_ref(), &who, "note", "n2", json!({"text": "b"}))
        .await
        .must("a write once the other writer gave the lease up")?;
    ensure!(
        until_told(&l, &l.own, SeriesToldState::Kept).await?,
        "the write that took the lease again must finish the build the mark owed, with no \
         read and no setting written to ask for it; `describe` told {:?}",
        pair(&l, &l.own).await?
    );
    let after = answers(&l, size).await?;
    ensure!(
        after == before,
        "once the build the regained lease took up is whole, every read answers as before: \
         {before:?} became {after:?}"
    );
    Ok(())
}
