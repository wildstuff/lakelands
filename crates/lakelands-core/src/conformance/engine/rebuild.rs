//! A rebuild through the seam: the projection is built beside the lake's,
//! in a scratch, and put in its place in one checked step
//! (`docs/decisions/071-a-rebuild-builds-beside-the-projection.md`).
//!
//! The projection the lake answers from is never emptied for a rebuild:
//! until the swap, every handle reads it as it was, whatever happens to
//! the scratch, and the swap refuses a scratch that does not reach the
//! log's end. A disk engine keeps a scratch marked built across handles,
//! so a build continues where it was marked; the memory engine keeps none,
//! and answers every call of it as unbound.

use lakelands_api::Error;
use lakelands_api::engine::{LakeEngine, Lease, OpenMode};
use lakelands_api::error::Situation;
use lakelands_api::event::Digest;
use lakelands_api::id::{LakeId, Position, RecordKey, TypeName};
use serde_json::json;

use super::projection::{fold_one, record, view};
use super::{EngineHarness, chain};
use crate::conformance::fixture::Must;
use crate::conformance::{Failure, Held};

engine_check!(
    BUILT_BESIDE = reopening a_rebuild_leaves_the_projection_answering_until_the_swap,
    "071 and seams §EngineOpener (lakelands#1322): a scratch opened for a rebuild folds beside the projection, which every handle reads as it was until the swap; a second scratch handle, a swap while one is open and a swap of a scratch not marked built are refused, leaving both as they were; an unmarked scratch whose handle is dropped is gone; the swap of a marked one puts it in place whole, and the scratch is gone after it. An engine that keeps no scratch answers each of these as unbound"
);

engine_check!(
    KEPT_SCRATCH = reopening a_scratch_marked_built_is_kept_while_the_log_holds_its_mark,
    "071 (lakelands#1322): a scratch marked built is opened again as it was, and a swap of it is refused while the log has moved past its mark, until it is folded on and marked again; a mark the log does not hold is discarded; a scratch is opened, folded and swapped in beside a projection of another layout while a writer holds the lease"
);

/// The situation an answer was refused with, if it was unavailable.
fn situation<T>(answer: &lakelands_api::Result<T>) -> Option<&Situation> {
    match answer {
        Err(Error::Unavailable { situation }) => Some(situation),
        _ => None,
    }
}

fn is_unbound<T>(answer: &lakelands_api::Result<T>) -> bool {
    matches!(answer, Err(Error::Unbound { .. }))
}

/// A scratch handle onto `lake`, or a failure saying what it was for.
fn scratch(
    harness: &dyn EngineHarness,
    lake: &LakeId,
    why: &str,
) -> Result<Box<dyn LakeEngine>, Failure> {
    harness
        .scratch(lake)
        .map_err(|e| Failure::new(format!("opening the scratch {why}: {e}")))
}

/// Whether `engine`'s head reads key `key` of type `t` at position `at`,
/// or reads none where `at` is none.
fn reads(engine: &dyn LakeEngine, key: &str, at: Option<u64>) -> Held {
    let got = view(engine, None)?
        .get(&TypeName::from("t"), &RecordKey::from(key))
        .must("reading the head")?;
    let got = got.map(|r| r.position.0);
    ensure!(
        got == at,
        "the head reads `{key}` at {got:?}, not at {at:?}"
    );
    Ok(())
}

/// Swap, expecting the refusal `expected` names, with the projection left
/// reading `a` at position 1.
fn swap_refused(
    engine: &dyn LakeEngine,
    lease: &Lease,
    expected: &str,
    is: impl Fn(&Situation) -> bool,
) -> Held {
    let swapped = engine.swap(lease);
    ensure!(
        situation(&swapped).is_some_and(is),
        "a swap {expected} answers {swapped:?}; it must be refused so"
    );
    reads(engine, "a", Some(1))
}

fn a_rebuild_leaves_the_projection_answering_until_the_swap(
    engine: &dyn LakeEngine,
    harness: &dyn EngineHarness,
) -> Held {
    let lake = engine.lake();
    let lease = engine.take_lease("conformance").must("taking the lease")?;
    engine
        .append(&lease, &chain(0, 3, &Digest::zero(), &lease))
        .must("appending three events")?;
    fold_one(engine, 1, record("t", "a", json!({}), 1))?;
    let built = harness.scratch(lake);
    if is_unbound(&built) {
        let (marked, swapped) = (engine.mark_built(), engine.swap(&lease));
        ensure!(
            is_unbound(&marked) && is_unbound(&swapped),
            "an engine that keeps no scratch marks one built as {marked:?} and swaps as {swapped:?}"
        );
        return Ok(());
    }
    let built = built.map_err(|e| Failure::new(format!("opening the scratch: {e}")))?;
    let leased = built.take_lease("scratch");
    ensure!(
        matches!(leased, Err(Error::Unavailable { .. })),
        "a scratch handle takes the lease: {leased:?}"
    );
    let second = harness.scratch(lake).map(drop);
    ensure!(
        matches!(situation(&second), Some(Situation::Rebuilding { .. })),
        "a second scratch handle opens beside the first: {second:?}"
    );
    fold_one(built.as_ref(), 2, record("t", "b", json!({}), 2))?;
    reads(engine, "b", None)?;
    reads(engine, "a", Some(1))?;
    swap_refused(engine, &lease, "while a scratch handle is open", |s| {
        matches!(s, Situation::Rebuilding { .. })
    })?;
    drop(built);
    swap_refused(engine, &lease, "of a scratch never marked built", |s| {
        matches!(s, Situation::NoScratch { .. })
    })?;
    let built = scratch(harness, lake, "after an unmarked one was dropped")?;
    let folded = built
        .projection()
        .folded_through()
        .must("reading its fold")?;
    ensure!(
        folded.is_none(),
        "an unmarked scratch whose handle was dropped is opened folded through {folded:?}"
    );
    fold_one(built.as_ref(), 2, record("t", "b", json!({}), 2))?;
    let marked = built.mark_built().must("marking the scratch built")?;
    ensure!(
        marked == Some(Position(2)),
        "the scratch is marked built through {marked:?}"
    );
    let refused = engine.mark_built();
    ensure!(
        matches!(refused, Err(Error::Unavailable { .. })),
        "a handle that is not a scratch's marks itself built: {refused:?}"
    );
    drop(built);
    let swapped = engine.swap(&lease).must("swapping the marked scratch in")?;
    ensure!(swapped == Some(Position(2)), "the swap answers {swapped:?}");
    reads(engine, "a", None)?;
    reads(engine, "b", Some(2))?;
    let reader = harness
        .reopen(lake, OpenMode::Read)
        .map_err(|e| Failure::new(format!("opening a reader after the swap: {e}")))?;
    reads(reader.as_ref(), "b", Some(2))?;
    let after = scratch(harness, lake, "after the swap")?;
    let folded = after
        .projection()
        .folded_through()
        .must("reading its fold")?;
    ensure!(
        folded.is_none(),
        "after the swap a scratch opens folded through {folded:?}"
    );
    Ok(())
}

fn a_scratch_marked_built_is_kept_while_the_log_holds_its_mark(
    engine: &dyn LakeEngine,
    harness: &dyn EngineHarness,
) -> Held {
    let lake = engine.lake();
    let lease = engine.take_lease("conformance").must("taking the lease")?;
    let events = chain(0, 5, &Digest::zero(), &lease);
    engine
        .append(&lease, &events[..3])
        .must("appending three events")?;
    fold_one(engine, 1, record("t", "a", json!({}), 1))?;
    let Ok(built) = harness.scratch(lake) else {
        return Ok(());
    };
    fold_one(built.as_ref(), 2, record("t", "b", json!({}), 2))?;
    built.mark_built().must("marking the scratch built")?;
    drop(built);
    engine
        .append(&lease, &events[3..])
        .must("appending two more")?;
    swap_refused(engine, &lease, "of a scratch behind the log", |s| {
        matches!(
            s,
            Situation::ScratchBehind {
                marked: Some(Position(2)),
                tip: Some(Position(4)),
                ..
            }
        )
    })?;
    let built = scratch(harness, lake, "again")?;
    let folded = built
        .projection()
        .folded_through()
        .must("reading its fold")?;
    ensure!(
        folded == Some(Position(2)),
        "a scratch marked through 2 opens folded through {folded:?}"
    );
    fold_one(built.as_ref(), 4, record("t", "c", json!({}), 4))?;
    built.mark_built().must("marking it built again")?;
    drop(built);
    engine.swap(&lease).must("swapping the scratch folded on")?;
    reads(engine, "b", Some(2))?;
    reads(engine, "c", Some(4))?;
    // A mark the log does not hold names no scratch of this lake.
    let built = scratch(harness, lake, "to forge")?;
    fold_one(built.as_ref(), 4, record("t", "d", json!({}), 4))?;
    built.mark_built().must("marking it built")?;
    drop(built);
    if harness.forge_scratch_mark(lake) {
        let swapped = engine.swap(&lease);
        ensure!(
            matches!(situation(&swapped), Some(Situation::NoScratch { .. })),
            "a swap of a scratch whose mark the log does not hold answers {swapped:?}"
        );
        let built = scratch(harness, lake, "after its mark was forged")?;
        let folded = built
            .projection()
            .folded_through()
            .must("reading its fold")?;
        ensure!(
            folded.is_none(),
            "a scratch with a forged mark opens folded through {folded:?}"
        );
    }
    // Beside a projection of another layout, which no open of it reads.
    if harness.mark_layout(lake, Some(999)) {
        let built = scratch(harness, lake, "beside another layout")?;
        fold_one(built.as_ref(), 4, record("t", "e", json!({}), 4))?;
        built.mark_built().must("marking it built")?;
        drop(built);
        engine
            .swap(&lease)
            .must("swapping it in over another layout")?;
        let writer = harness
            .reopen(lake, OpenMode::Write)
            .map_err(|e| Failure::new(format!("opening a writer after the swap: {e}")))?;
        reads(writer.as_ref(), "e", Some(4))?;
    }
    Ok(())
}
