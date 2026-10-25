//! A lake is measured through the meter its host binds
//! (`docs/decisions/079-a-lake-is-measured-through-a-meter-the-host-binds.md`).
//!
//! The checks bind a meter that keeps every number it is told. One drives
//! writes alone and at once, a read, a rebuild, a checkpoint and the lease
//! given up, and reads the numbers back against the lake's own answers;
//! the other has another writer take the lease from under the handle, and
//! reads the lease told gone.

use std::sync::Mutex;

use lakelands_api::seam::{Count, Level, Metered, Spread};

use super::concurrency::items;
use super::erasable::again;
use super::forgotten::{Fails, failing};
use crate::conformance::fixture::{doors, owner, type_of};
use crate::conformance::prelude::*;

check!(
    METERED = a_bound_meter_is_told_the_lakes_writes_its_tip_and_its_fold,
    "079: a lake opened with a meter tells it, labelled with the lake and one engine name, the writes it made, alone and batched, as many as the batches' sizes add to; its tip and the position folded through, both last told at the head; its lease held, then held no more and its fence 0 once it is given up; a rebuild begun and ended; a checkpoint's seal; the time a read took; and every append both timed and counted",
    reopens
);

check!(
    METERED_LEASE = a_bound_meter_is_told_a_lease_another_writer_took,
    "079: a handle opened with a meter whose lease another writer took tells it, at the append the engine refuses, the append failed and the lease held no more, its fence 0 (lakelands#1385)",
    given
);

/// One number a meter was told: its lake, its engine, what and how much.
type Told = (String, String, &'static str, u64);

/// A meter that keeps every number it is told.
#[derive(Default)]
struct Kept(Mutex<Vec<Told>>);

impl Kept {
    fn keep(&self, at: Metered<'_>, what: &'static str, n: u64) {
        let told = (at.lake.to_string(), at.engine.to_string(), what, n);
        crate::lock(&self.0).push(told);
    }

    fn of(&self, what: &str) -> Vec<u64> {
        let told = crate::lock(&self.0);
        told.iter().filter(|t| t.2 == what).map(|t| t.3).collect()
    }
}

impl lakelands_api::seam::Meter for Kept {
    fn count(&self, at: Metered<'_>, n: Count, by: u64) {
        self.keep(at, n.name(), by);
    }
    fn level(&self, at: Metered<'_>, n: Level, value: u64) {
        self.keep(at, n.name(), value);
    }
    fn observe(&self, at: Metered<'_>, n: Spread, value: u64) {
        self.keep(at, n.name(), value);
    }
}

async fn a_bound_meter_is_told_the_lakes_writes_its_tip_and_its_fold(
    store: Store,
    reopen: Reopen,
) -> Held {
    let kept = Arc::new(Kept::default());
    let seams = Seams {
        meter: Some(kept.clone()),
        backings: backing::backings(),
        ..Seams::default()
    };
    let store = again(&reopen, store, seams, "with a meter").await?;
    let l = items(store.as_ref()).await?;
    for n in 0..3 {
        l.upsert("item", &format!("a{n}"), json!({"n": n})).await?;
    }
    let at_once = (0..8).map(|n| {
        let l = Lake {
            cat: l.cat.clone(),
            own: l.own.clone(),
        };
        Box::pin(async move { l.upsert("item", &format!("b{n}"), json!({"n": n})).await })
            as Caller<'_, Result<Outcome, Failure>>
    });
    for o in join_all(at_once.collect()).await {
        o?;
    }
    let read = all(l.cat.as_ref(), &l.own, "item").await?;
    ensure!(
        read.len() == 11,
        "eleven items were written, {} read",
        read.len()
    );
    store.rebuild().await.must("rebuilding")?;
    store.checkpoint().await.must("checkpointing")?;
    let head = store.head().await.must("reading the head")?;
    let (writes, batched) = (
        kept.of(Count::Writes.name()),
        kept.of(Spread::BatchWrites.name()),
    );
    ensure!(
        writes.iter().sum::<u64>() == batched.iter().sum::<u64>()
            && writes.iter().sum::<u64>() >= 11,
        "the meter was told writes {writes:?} and batches {batched:?}: at least eleven, as many as the batches add to"
    );
    let last = |what: &str| kept.of(what).last().copied();
    ensure!(
        last(Level::Tip.name()) == Some(head.0)
            && last(Level::FoldedThrough.name()) == Some(head.0),
        "the tip was last told {:?} and the fold {:?}, the head is {head}",
        last(Level::Tip.name()),
        last(Level::FoldedThrough.name())
    );
    let rebuilding = kept.of(Level::Rebuilding.name());
    ensure!(
        rebuilding.ends_with(&[1, 0]),
        "a rebuild was told as {rebuilding:?}, not begun then ended"
    );
    ensure!(
        last(Level::LeaseHeld.name()) == Some(1),
        "the lease was told held as {:?}",
        last(Level::LeaseHeld.name())
    );
    ensure!(
        !kept.of(Count::Seals.name()).is_empty(),
        "the checkpoint's seal was not told"
    );
    ensure!(
        !kept.of(Spread::Read.name()).is_empty(),
        "no read was timed"
    );
    let (timed, appends) = (
        kept.of(Spread::Append.name()).len(),
        kept.of(Count::Appends.name()).len(),
    );
    ensure!(
        timed > 0 && timed == appends && !kept.of(Count::Events.name()).is_empty(),
        "{timed} appends were timed and {appends} counted: every append is both"
    );
    store.close().await.must("giving the lease up")?;
    ensure!(
        last(Level::LeaseHeld.name()) == Some(0) && last(Level::LeaseFence.name()) == Some(0),
        "the lease given up was told held as {:?}, its fence as {:?}: 0 and 0",
        last(Level::LeaseHeld.name()),
        last(Level::LeaseFence.name())
    );
    let told = crate::lock(&kept.0);
    let engine = told.first().map(|t| t.1.clone()).unwrap_or_default();
    let alike = told
        .iter()
        .all(|t| t.0 == store.lake().to_string() && t.1 == engine);
    ensure!(
        alike && !engine.is_empty(),
        "every number is labelled with the lake `{}` and one engine, `{engine}`",
        store.lake()
    );
    Ok(())
}

async fn a_bound_meter_is_told_a_lease_another_writer_took(store: Store, given: Given) -> Held {
    let (kept, fails): (Arc<Kept>, Fails) = Default::default();
    let seams = Seams {
        meter: Some(kept.clone()),
        ..Seams::default()
    };
    let store = (given.fault)(store, seams, failing(&fails))
        .await
        .map_err(|e| Failure::new(format!("opening the lake again with a meter: {e}")))?;
    let l = Lake::of(store.as_ref());
    l.declare(type_of("note", &[("text", StorageKind::Text)]))
        .await?;
    let last = |what: &str| kept.of(what).last().copied();
    ensure!(
        last(Level::LeaseHeld.name()) == Some(1),
        "the lease was told held as {:?} before it was taken",
        last(Level::LeaseHeld.name())
    );
    let (cat, who) = (doors(store.as_ref()), owner(store.as_ref()));
    let before = store.head().await.must("reading the head")?;
    fails[8].store(1, Ordering::SeqCst);
    let taken = try_upsert(cat.as_ref(), &who, "note", "n1", json!({"text": "a"})).await;
    refused_unmoved(
        store.as_ref(),
        before,
        "a write once another writer took the lease",
        taken,
    )
    .await?;
    ensure!(
        last(Level::LeaseHeld.name()) == Some(0)
            && last(Level::LeaseFence.name()) == Some(0)
            && kept.of(Count::AppendFailures.name()) == [1],
        "the append the engine refused was told failed {:?}, the lease held as {:?} and its \
         fence as {:?}: once, 0 and 0",
        kept.of(Count::AppendFailures.name()),
        last(Level::LeaseHeld.name()),
        last(Level::LeaseFence.name())
    );
    Ok(())
}
