//! Record writes queued as one entry of the writer's queue, for the tests
//! that hold a batch to what each of its writes answers alone
//! (lakelands#1332, lakelands#1313), a clock they fix so two lakes written
//! alike log alike, the turn held while callers queue, and the order the
//! queue took record writes in. A seam of the tests, not of the store:
//! hidden from its documentation, and no host is to call it; a fixed clock
//! and a recorded order hold for every lake of the process.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use lakelands_api::engine::OpenMode;
use lakelands_api::seam::Seams;
use lakelands_api::write::Outcome;
use lakelands_api::{LakeStore, Location, Principal, Result, Timestamp, WriteContext, WriteOpts};

use crate::doors::Doors;
use crate::lake::Lake;
use crate::writes::Job;
pub use crate::writes::Write;

/// A record write asked: under what context, who asks, the write and its
/// options.
pub type Asked = (WriteContext, Principal, Write, WriteOpts);

/// The record writes the queues took, in the order taken, while recorded.
static ORDER: Mutex<Option<Vec<Asked>>> = Mutex::new(None);

/// Record the order record writes are taken from every queue in, or, at
/// `false`, stop.
pub fn record_order(on: bool) {
    *lakelands_core::lock(&ORDER) = on.then(Vec::new);
}

/// The record writes taken since [`record_order`], by the principal as
/// the writer judged it; those of the lake `lake` only.
pub fn order(lake: &str) -> Vec<Asked> {
    let order = lakelands_core::lock(&ORDER);
    let of = |a: &&Asked| a.1.lake.as_str() == lake;
    order.iter().flatten().filter(of).cloned().collect()
}

/// `jobs`, taken from a queue in this order.
pub(crate) fn taken(jobs: &[Job]) {
    if let Some(order) = lakelands_core::lock(&ORDER).as_mut() {
        let asked = |j: &Job| (j.context, j.who.clone(), j.write.clone(), j.opts.clone());
        order.extend(jobs.iter().map(asked));
    }
}

/// The time every lake of the process reads while it is fixed.
static FIXED: AtomicI64 = AtomicI64::new(i64::MIN);

/// Fix the time every lake of the process reads, or, at `None`, let it
/// read the system's again.
pub fn fix_clock(at: Option<Timestamp>) {
    FIXED.store(at.map_or(i64::MIN, |t| t.0), Ordering::SeqCst);
}

/// The time fixed, if it is.
pub(crate) fn fixed() -> Option<Timestamp> {
    let at = FIXED.load(Ordering::SeqCst);
    (at != i64::MIN).then_some(Timestamp(at))
}

/// Open the lake at `location` as [`crate::open`] does, with what queues
/// its record writes beside it.
pub async fn open(
    location: Location,
    seams: Seams,
    mode: OpenMode,
) -> Result<(Arc<dyn LakeStore>, Batches)> {
    let lake = crate::doors::opened(location, seams, mode).await?;
    let batches = Batches { lake: lake.clone() };
    Ok((Arc::new(crate::doors::Store { lake }), batches))
}

/// What queues record writes of one lake.
pub struct Batches {
    lake: Arc<Lake>,
}

impl Batches {
    /// `writes` queued as one entry, each answered as its door answers it.
    pub async fn write(&self, writes: Vec<Asked>) -> Vec<Result<Outcome>> {
        let doors = Doors::new(&self.lake, WriteContext::default());
        doors.records(writes).await
    }

    /// The turn held until what is answered is dropped: what is queued
    /// meanwhile waits behind it.
    pub fn hold(&self) -> std::sync::mpsc::Sender<()> {
        crate::exec::hold_turn(&self.lake)
    }

    /// How many entries wait in the queue.
    pub fn queued(&self) -> usize {
        crate::exec::queued(&self.lake)
    }

    /// One write decided and appended alone, as a door did before writes
    /// were batched.
    pub async fn alone(
        &self,
        context: WriteContext,
        (who, write, opts): (Principal, Write, WriteOpts),
    ) -> Result<Outcome> {
        Doors::new(&self.lake, context)
            .alone(&who, write, opts)
            .await
    }
}
