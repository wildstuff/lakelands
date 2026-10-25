//! A standby follows the log and takes the lease when the writer is gone
//! (`docs/decisions/076-a-lake-keeps-one-writer-and-a-dataset-that-outgrows-it-becomes-several-lakes.md`).
//!
//! A standby opened beside the lake's writer reads what the writer wrote
//! and answers a write as a handle opened read-only does, for as long as
//! the writer holds the lease's lock: no wait takes the lease from a
//! writer that holds it. Once the lock is gone, as when the writer's
//! process ends or its session is closed, the standby takes the lease
//! with a fence greater than every earlier one and serves writes, and the
//! old writer's next write is refused by the engine. The writer's lock is
//! let go behind its back through the harness's fault wrap.

use std::sync::Mutex;

use lakelands_api::LakeId;
use lakelands_api::engine::{
    CanonicalEvent, EngineOpener, LakeEngine, Lease, OpenMode, ProjectionStore, ProjectionView,
    Ranked, Rankings, VectorIndex,
};
use lakelands_api::event::{Digest, SegmentDigest};
use lakelands_api::location::Location;

use crate::conformance::fixture::{doors, get_at, owner};
use crate::conformance::prelude::*;

check!(
    STANDBY = a_standby_takes_the_lease_once_the_writer_is_gone,
    "076 (lakelands#1395): a standby opened beside the lake's writer reads what the writer wrote, answers a write as read-only with the lake leased elsewhere, appending nothing, and is no writer while the writer holds the lease, however long; once the writer's lock is gone it is the writer within a bound, its `lease_taken` carries a fence greater than every earlier one, and it serves a write; the old writer's next write is refused by the engine as its lease not held or stale, appends nothing, and leaves it no writer",
    given
);

/// How often the standby tries the lease.
const TRY: Duration = Duration::from_millis(20);

/// How long the standby is watched while the writer holds the lease:
/// many of its tries.
const HELD: Duration = Duration::from_millis(400);

/// A wait for what should come.
const WAIT: Duration = Duration::from_secs(30);

/// The engine handle that took the writer's lease, and that lease.
type Taken = Arc<Mutex<Option<(Arc<dyn LakeEngine>, Lease)>>>;

async fn a_standby_takes_the_lease_once_the_writer_is_gone(store: Store, given: Given) -> Held {
    let taken: Taken = Arc::default();
    let wrap = taken.clone();
    let wrap: crate::conformance::Wrap = Arc::new(move |inner| {
        let taken = wrap.clone();
        Arc::new(Letting { inner, taken }) as Arc<dyn EngineOpener>
    });
    let writer = (given.fault)(store, Seams::default(), wrap)
        .await
        .map_err(|e| Failure::new(format!("opening the lake again for writing: {e}")))?;
    let l = Lake::of(writer.as_ref());
    l.declare(type_decl("note", vec![field("text", StorageKind::Text)]))
        .await?;
    let wrote = l
        .upsert("note", "n1", json!({"text": "one"}))
        .await?
        .position();
    let standby = (given.standby)(writer.lake().clone(), Seams::default(), TRY)
        .await
        .map_err(|e| Failure::new(format!("opening a standby beside the writer: {e}")))?;

    standby
        .reaches(wrote, WAIT)
        .await
        .must("the standby reaching the writer's write")?;
    let (cat, who) = (doors(standby.as_ref()), owner(standby.as_ref()));
    let note = TypeName::from("note");
    let read = cat.get(&who, &note, get_at("n1", None)).await;
    ensure!(
        read.as_ref()
            .is_ok_and(|r| r.data.fields.get("text") == Some(&json!("one"))),
        "the standby must read `n1` as the writer wrote it; it reads {read:?}"
    );
    let before = writer.head().await.must("reading the writer's head")?;
    let refused = try_upsert(cat.as_ref(), &who, "note", "s1", json!({"text": "s"})).await;
    let what = "a write through the standby while the writer holds the lease";
    let err = refused_unmoved(writer.as_ref(), before, what, refused).await?;
    ensure!(
        matches!(
            err.situation(),
            Some(Situation::ReadOnly {
                leased_elsewhere: true,
                ..
            })
        ),
        "{what} must be answered as read-only with the lake leased elsewhere; it answered {err}"
    );
    pause(HELD).await;
    ensure!(
        !standby.is_writer() && writer.is_writer(),
        "while the writer holds the lease the standby must stay no writer, however long; after \
         {HELD:?} the standby is a writer: {}, the writer is one: {}",
        standby.is_writer(),
        writer.is_writer()
    );

    let held = crate::lock(&taken).take();
    let (engine, lease) = held.ok_or_else(|| Failure::new("the writer took no lease"))?;
    engine
        .release_lease(&lease)
        .map_err(|e| Failure::new(format!("letting the writer's lock go: {e}")))?;
    let started = std::time::Instant::now();
    while !standby.is_writer() && started.elapsed() < WAIT {
        pause(TRY).await;
    }
    ensure!(
        standby.is_writer(),
        "once the writer's lock is gone the standby must take the lease; after {WAIT:?} it is no \
         writer"
    );
    let fences: Vec<u64> = (log_events(standby.as_ref()).await?.into_iter())
        .filter_map(|e| match e.body {
            EventBody::LeaseTaken { fence, .. } => Some(fence),
            _ => None,
        })
        .collect();
    ensure!(
        matches!(fences.split_last(), Some((last, earlier))
            if *last > lease.fence && earlier.iter().all(|f| f < last)),
        "the standby's `lease_taken` must carry a fence greater than every earlier one, the old \
         writer's {} among them; the log's fences are {fences:?}",
        lease.fence
    );
    let s = Lake::of(standby.as_ref());
    s.upsert("note", "s2", json!({"text": "taken over"}))
        .await
        .map_err(|e| {
            Failure::new(format!(
                "a write through the standby once it took over: {e}"
            ))
        })?;

    let before = standby.head().await.must("reading the standby's head")?;
    let stale = try_upsert(l.cat.as_ref(), &l.own, "note", "n2", json!({"text": "two"})).await;
    let what = "a write through the old writer once the standby took the lease";
    let err = refused_unmoved(standby.as_ref(), before, what, stale).await?;
    ensure!(
        matches!(
            err.situation(),
            Some(Situation::LeaseNotHeld { .. } | Situation::LeaseStale { .. })
        ) && !writer.is_writer(),
        "{what} must be refused by the engine as its lease not held or stale, and leave it no \
         writer; it answered {err}, the old writer a writer: {}",
        writer.is_writer()
    );
    writer.close().await.must("closing the old writer")?;
    standby.close().await.must("closing the standby")?;
    Ok(())
}

/// The harness's engine opener, whose handles put the lease they take
/// where the check can let it go behind the writer's back.
struct Letting {
    inner: Arc<dyn EngineOpener>,
    taken: Taken,
}

impl EngineOpener for Letting {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn log_ends(
        &self,
        location: &Location,
    ) -> lakelands_api::Result<Option<(CanonicalEvent, CanonicalEvent)>> {
        self.inner.log_ends(location)
    }
    fn open(
        &self,
        location: &Location,
        mode: OpenMode,
    ) -> lakelands_api::Result<Box<dyn LakeEngine>> {
        let inner = Arc::from(self.inner.open(location, mode)?);
        let taken = self.taken.clone();
        Ok(Box::new(Let { inner, taken }))
    }
    fn open_scratch(&self, location: &Location) -> lakelands_api::Result<Box<dyn LakeEngine>> {
        self.inner.open_scratch(location)
    }
    fn room(&self, location: &Location) -> lakelands_api::Result<u64> {
        self.inner.room(location)
    }
}

/// A handle of [`Letting`].
struct Let {
    inner: Arc<dyn LakeEngine>,
    taken: Taken,
}

impl LakeEngine for Let {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn rebuild_rate(&self) -> u64 {
        self.inner.rebuild_rate()
    }
    fn lake(&self) -> &LakeId {
        self.inner.lake()
    }
    fn take_lease(&self, holder: &str) -> lakelands_api::Result<Lease> {
        let lease = self.inner.take_lease(holder)?;
        *crate::lock(&self.taken) = Some((self.inner.clone(), lease.clone()));
        Ok(lease)
    }
    fn release_lease(&self, lease: &Lease) -> lakelands_api::Result<()> {
        self.inner.release_lease(lease)
    }
    fn lease_free(&self) -> lakelands_api::Result<bool> {
        self.inner.lease_free()
    }
    fn tip(&self) -> lakelands_api::Result<Option<(Position, Digest)>> {
        self.inner.tip()
    }
    fn append(&self, lease: &Lease, events: &[CanonicalEvent]) -> lakelands_api::Result<()> {
        self.inner.append(lease, events)
    }
    fn import(&self, lease: &Lease, events: &[CanonicalEvent]) -> lakelands_api::Result<()> {
        self.inner.import(lease, events)
    }
    fn read(&self, from: Position, limit: u32) -> lakelands_api::Result<Vec<CanonicalEvent>> {
        self.inner.read(from, limit)
    }
    fn seal_segment(
        &self,
        lease: &Lease,
        imported: bool,
        first: &mut dyn FnMut(&SegmentDigest) -> lakelands_api::Result<Vec<CanonicalEvent>>,
    ) -> lakelands_api::Result<Option<SegmentDigest>> {
        self.inner.seal_segment(lease, imported, first)
    }
    fn segments(&self) -> lakelands_api::Result<Vec<SegmentDigest>> {
        self.inner.segments()
    }
    fn active_segment_bytes(&self) -> Option<u64> {
        self.inner.active_segment_bytes()
    }
    fn projection(&self) -> &dyn ProjectionStore {
        self.inner.projection()
    }
    fn view(&self, as_of: Option<Position>) -> lakelands_api::Result<Box<dyn ProjectionView + '_>> {
        self.inner.view(as_of)
    }
    fn ranked(&self, ty: &TypeName, asked: &Rankings, k: u32) -> lakelands_api::Result<Ranked> {
        self.inner.ranked(ty, asked, k)
    }
    fn vectors(&self) -> Option<&dyn VectorIndex> {
        self.inner.vectors()
    }
    fn mark_built(&self) -> lakelands_api::Result<Option<Position>> {
        self.inner.mark_built()
    }
    fn swap(&self, lease: &Lease) -> lakelands_api::Result<Option<Position>> {
        self.inner.swap(lease)
    }
    fn may_swap(&self) -> lakelands_api::Result<bool> {
        self.inner.may_swap()
    }
}
