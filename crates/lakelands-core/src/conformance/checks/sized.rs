//! The active segment is sealed by size
//! (`docs/decisions/074-a-segment-is-sealed-by-size.md`).
//!
//! An engine that reports its active segment's bytes has it sealed, as
//! `checkpoint` seals it, at the writer's open that finds it past
//! `limits.seal_after_bytes` and after the append that takes it past; one
//! that reports none is never sealed by size. Whether the engine reports
//! is read through the harness's fault wrap, at the default bound.

use lakelands_api::event::EventBody;
use lakelands_api::location::Limits;
use lakelands_api::ontology::StorageKind::Text;
use lakelands_api::seam::Seams;

use super::forgotten::{Fails, failing};
use crate::conformance::fixture::type_of;
use crate::conformance::prelude::*;

check!(
    SEALED_BY_SIZE = a_segment_past_its_bound_is_sealed_where_the_engine_weighs_it,
    "074: on an engine that reports its active segment's bytes, a lake opened again with `limits.seal_after_bytes` below what the segment holds seals it at the writer's open, and again after the append that takes the next past it, each seal a checkpoint listing the segments sealed so far; the lake verifies to its head, and opened again reads across the seals; on an engine that reports none the same lake holds no checkpoint",
    given
);

/// Four notes of 300 KiB from `from` on, past the 1 MiB floor together.
async fn notes(store: &dyn LakeStore, from: u32) -> Held {
    let l = Lake::of(store);
    for i in from..from + 4 {
        let text = format!("{i}").repeat(300 << 10);
        l.upsert("note", &format!("n{i}"), json!({ "text": text }))
            .await?;
    }
    Ok(())
}

async fn a_segment_past_its_bound_is_sealed_where_the_engine_weighs_it(
    store: Store,
    given: Given,
) -> Held {
    Lake::of(store.as_ref())
        .declare(type_of("note", &[("text", Text)]))
        .await?;
    notes(store.as_ref(), 0).await?;
    let fails: Fails = Arc::default();
    let opened = (given.fault)(store, Seams::default(), failing(&fails));
    let store = opened
        .await
        .map_err(|e| Failure::new(format!("opening the lake to weigh its segment: {e}")))?;
    let weighs = fails[7].load(Ordering::SeqCst) > 0;
    let bound = Limits {
        seal_after_bytes: 1,
        ..limits()
    };
    let opened = (given.relimit)(store, Seams::default(), bound);
    let store = opened
        .await
        .map_err(|e| Failure::new(format!("opening the lake at a bound of 1 MiB: {e}")))?;
    notes(store.as_ref(), 4).await?;
    let sealed: Vec<_> = (log_events(store.as_ref()).await?.into_iter())
        .filter_map(|e| match e.body {
            EventBody::Checkpoint { segments, .. } => Some((e.position, segments)),
            _ => None,
        })
        .collect();
    let at: Vec<Position> = sealed.iter().map(|s| s.0).collect();
    ensure!(
        weighs || at.is_empty(),
        "an engine that reports no segment size is never sealed by size, and the lake holds \
         checkpoints at {at:?}"
    );
    if !weighs {
        return Ok(());
    }
    let firsts: Vec<Vec<Position>> = (sealed.iter())
        .map(|(_, s)| s.iter().map(|d| d.first).collect())
        .collect();
    let want = match sealed.first() {
        Some((at, _)) => vec![vec![Position::GENESIS], vec![Position::GENESIS, *at]],
        None => Vec::new(),
    };
    ensure!(
        sealed.len() == 2 && firsts == want,
        "a segment past the bound is sealed at the writer's open and after the append that takes \
         the next past it, each checkpoint listing the segments sealed so far; the checkpoints \
         list segments beginning at {firsts:?}"
    );
    let head = store.head().await.must("reading the head")?;
    let said = store.verify(None, None).await.must("verifying the lake")?;
    ensure!(
        said.through == head,
        "a lake sealed by size verifies to its head {head}, and it verifies to {}",
        said.through
    );
    let opened = (given.relimit)(store, Seams::default(), limits());
    let store = opened
        .await
        .map_err(|e| Failure::new(format!("opening the sealed lake again: {e}")))?;
    let read = Lake::of(store.as_ref()).read("note", "n0").await?;
    ensure!(
        value(&read, "text") == Some(&json!("0".repeat(300 << 10))),
        "a lake sealed by size reads across its seals once opened again, and `n0` holds another text"
    );
    Ok(())
}
