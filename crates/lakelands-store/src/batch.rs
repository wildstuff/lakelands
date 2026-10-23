//! Record writes queued as one entry of the writer's queue, for the tests
//! that hold a batch to what each of its writes answers alone
//! (lakelands#1332), and a clock they fix so two lakes written alike log
//! alike. A seam of the tests, not of the store: no host reaches it.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use lakelands_api::engine::OpenMode;
use lakelands_api::seam::Seams;
use lakelands_api::write::Outcome;
use lakelands_api::{LakeStore, Location, Principal, Result, Timestamp, WriteContext, WriteOpts};

use crate::doors::Doors;
use crate::lake::Lake;
pub use crate::writes::Write;

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
    /// `writes` queued as one batch, each answered as its door answers it.
    pub async fn write(
        &self,
        context: WriteContext,
        writes: Vec<(Principal, Write, WriteOpts)>,
    ) -> Vec<Result<Outcome>> {
        Doors::new(&self.lake, context).records(writes).await
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
