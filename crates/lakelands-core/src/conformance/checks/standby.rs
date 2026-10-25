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
use std::sync::atomic::AtomicUsize;

use lakelands_api::LakeId;
use lakelands_api::engine::{
    CanonicalEvent, EngineOpener, LakeEngine, Lease, OpenMode, ProjectionStore, ProjectionView,
    Ranked, Rankings, VectorIndex,
};
use lakelands_api::event::{Digest, SegmentDigest};
use lakelands_api::location::Location;

use crate::conformance::Standing;
use crate::conformance::fixture::{doors, get_at, owner};
use crate::conformance::prelude::*;

check!(
    STANDBY = a_standby_takes_the_lease_once_the_writer_is_gone,
    "076 (lakelands#1395): a standby opened beside the lake's writer reads what the writer wrote, answers a write as read-only with the lake leased elsewhere, appending nothing, and is no writer while the writer holds the lease, however long; once the writer's lock is gone it is the writer within a bound, its tries come to their end without an error, its `lease_taken` carries a fence greater than every earlier one, and it serves a write; the old writer's next write is refused by the engine as its lease not held or stale, appends nothing, and leaves it no writer",
    given
);

check!(
    STANDBY_FAILED = a_standby_whose_takeover_fails_answers_why_and_tries_no_more,
    "076 (lakelands#1395): a standby whose takeover fails once the writer's lock is gone, its first append under the lease refused below the store, answers that refusal for its tries, takes the lease no more, is no writer and gives the lease up, so a writer's open then holds it",
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

/// The writer of a check, opened through [`Planted`], and the slot its
/// lease is put in.
async fn writer(store: Store, given: &Given) -> Result<(Store, Taken), Failure> {
    let planted = Planted::new(false);
    let taken = planted.taken.clone();
    let writer = (given.fault)(store, Seams::default(), planted.wrap())
        .await
        .map_err(|e| Failure::new(format!("opening the lake again for writing: {e}")))?;
    Ok((writer, taken))
}

/// Let the writer's lock go behind its back, and answer its lease.
fn let_go(taken: &Taken) -> Result<Lease, Failure> {
    let held = crate::lock(taken).take();
    let (engine, lease) = held.ok_or_else(|| Failure::new("the writer took no lease"))?;
    engine
        .release_lease(&lease)
        .map_err(|e| Failure::new(format!("letting the writer's lock go: {e}")))?;
    Ok(lease)
}

/// What `standing` came to, once it came to something within [`WAIT`].
async fn settled(standing: &mut Standing) -> Result<lakelands_api::Result<()>, Failure> {
    let started = std::time::Instant::now();
    loop {
        if let Some(r) = now(standing) {
            return Ok(r);
        }
        if started.elapsed() > WAIT {
            return Err(Failure::new(format!(
                "the standby's tries came to nothing within {WAIT:?}"
            )));
        }
        pause(TRY).await;
    }
}

/// What `standing` has come to, if it has.
fn now(standing: &mut Standing) -> Option<lakelands_api::Result<()>> {
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match standing.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(r) => Some(r),
        std::task::Poll::Pending => None,
    }
}

async fn a_standby_takes_the_lease_once_the_writer_is_gone(store: Store, given: Given) -> Held {
    let (writer, taken) = writer(store, &given).await?;
    let l = Lake::of(writer.as_ref());
    l.declare(type_decl("note", vec![field("text", StorageKind::Text)]))
        .await?;
    let wrote = l
        .upsert("note", "n1", json!({"text": "one"}))
        .await?
        .position();
    let as_is: crate::conformance::Wrap = Arc::new(|inner| inner);
    let (standby, mut standing) =
        (given.standby)(writer.lake().clone(), Seams::default(), TRY, as_is)
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

    let lease = let_go(&taken)?;
    let came = settled(&mut standing).await?;
    ensure!(
        came.is_ok() && standby.is_writer(),
        "once the writer's lock is gone the standby must take the lease; its tries came to \
         {came:?}, and it is a writer: {}",
        standby.is_writer()
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

async fn a_standby_whose_takeover_fails_answers_why_and_tries_no_more(
    store: Store,
    given: Given,
) -> Held {
    let (writer, taken) = writer(store, &given).await?;
    let l = Lake::of(writer.as_ref());
    l.declare(type_decl("note", vec![field("text", StorageKind::Text)]))
        .await?;
    let planted = Planted::new(true);
    let takes = planted.takes.clone();
    let (standby, mut standing) =
        (given.standby)(writer.lake().clone(), Seams::default(), TRY, planted.wrap())
            .await
            .map_err(|e| Failure::new(format!("opening a standby beside the writer: {e}")))?;

    let_go(&taken)?;
    let came = settled(&mut standing).await?;
    ensure!(
        came.as_ref().is_err_and(|e| matches!(
            e.situation(),
            Some(Situation::Busy { lake }) if lake == PLANTED
        )),
        "a standby whose first append under the lease is refused must answer that refusal for \
         its tries; they came to {came:?}"
    );
    pause(HELD).await;
    let took = takes.load(Ordering::SeqCst);
    ensure!(
        took == 1 && !standby.is_writer(),
        "a standby whose takeover failed must try no more and be no writer; after {HELD:?} it \
         took the lease {took} times, and it is a writer: {}",
        standby.is_writer()
    );
    let as_is: crate::conformance::Wrap = Arc::new(|inner| inner);
    let next = (given.fault)(writer, Seams::default(), as_is).await;
    ensure!(
        next.as_ref().is_ok_and(|n| n.is_writer()),
        "the standby whose takeover failed must give the lease up, so a writer's open then holds \
         it; the open answered {:?}",
        next.as_ref().err()
    );
    let next =
        next.map_err(|e| Failure::new(format!("the writer after the failed takeover: {e}")))?;
    next.close().await.must("closing the writer")?;
    standby.close().await.must("closing the standby")?;
    Ok(())
}

/// What the planted refusal names.
const PLANTED: &str = "the planted fault";

/// The harness's engine opener, whose handles put the lease they take
/// where the check can let it go behind the writer's back, count their
/// takes, and, where it plants a fault, refuse every append of a handle
/// opened for writing.
#[derive(Clone)]
struct Planted {
    taken: Taken,
    takes: Arc<AtomicUsize>,
    fails: bool,
}

impl Planted {
    fn new(fails: bool) -> Planted {
        let (taken, takes) = (Arc::default(), Arc::default());
        Planted {
            taken,
            takes,
            fails,
        }
    }

    /// The wrap that opens through this.
    fn wrap(&self) -> crate::conformance::Wrap {
        let planted = self.clone();
        Arc::new(move |inner| {
            let planted = planted.clone();
            Arc::new(Letting { inner, planted }) as Arc<dyn EngineOpener>
        })
    }
}

/// The opener [`Planted::wrap`] makes.
struct Letting {
    inner: Arc<dyn EngineOpener>,
    planted: Planted,
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
        let planted = self.planted.clone();
        let fails = planted.fails && mode != OpenMode::Read;
        Ok(Box::new(Let {
            inner,
            planted,
            fails,
        }))
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
    planted: Planted,
    fails: bool,
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
        self.planted.takes.fetch_add(1, Ordering::SeqCst);
        *crate::lock(&self.planted.taken) = Some((self.inner.clone(), lease.clone()));
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
        if self.fails {
            let lake = PLANTED.to_owned();
            return Err(Error::unavailable(Situation::Busy { lake }));
        }
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
