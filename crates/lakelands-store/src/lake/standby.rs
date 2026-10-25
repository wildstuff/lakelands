//! A standby: a lake opened beside its writer that follows the log as a
//! reader does and takes the lease when the writer is gone
//! (`docs/decisions/076-a-lake-keeps-one-writer-and-a-dataset-that-outgrows-it-becomes-several-lakes.md`).
//!
//! Its engine answers from a handle opened read-only until the lease is
//! free, then from one opened for writing, which takes the lease; from
//! then on the standby is the lake's writer, as an open for writing
//! leaves it. Only the lease's lock says the writer is gone: a writer
//! that hangs while it holds the lock keeps the lease, however long, and
//! one whose process ended, or whose session the server closed, has let
//! it go.

use std::sync::OnceLock;

use lakelands_api::engine::{EngineOpener, ProjectionStore, Ranked, Rankings, VectorIndex};
use lakelands_api::id::{LakeId, TypeName};

use super::*;

/// The engine of a standby: read-only until it takes the lease.
pub(crate) struct Standby {
    opener: Arc<dyn EngineOpener>,
    location: Location,
    read: Box<dyn LakeEngine>,
    write: OnceLock<Box<dyn LakeEngine>>,
}

impl Standby {
    /// A standby over `read`, a handle opened read-only at `location`.
    pub(crate) fn new(
        opener: Arc<dyn EngineOpener>,
        location: &Location,
        read: Box<dyn LakeEngine>,
    ) -> Standby {
        let (location, write) = (location.clone(), OnceLock::new());
        Standby {
            opener,
            location,
            read,
            write,
        }
    }

    /// The handle answering now.
    fn now(&self) -> &dyn LakeEngine {
        self.write.get().unwrap_or(&self.read).as_ref()
    }
}

impl LakeEngine for Standby {
    fn name(&self) -> &str {
        self.read.name()
    }

    fn rebuild_rate(&self) -> u64 {
        self.read.rebuild_rate()
    }

    fn lake(&self) -> &LakeId {
        self.read.lake()
    }

    /// The first take opens the lake for writing, judged as an open for
    /// writing judges it before its lease, and keeps that handle once it
    /// holds the lease; a take refused drops it.
    fn take_lease(&self, holder: &str) -> Result<Lease> {
        if let Some(write) = self.write.get() {
            return write.take_lease(holder);
        }
        let write = self.opener.open(&self.location, OpenMode::Write)?;
        ends(write.as_ref())?;
        let lease = write.take_lease(holder)?;
        if let Err(write) = self.write.set(write) {
            let _ = write.release_lease(&lease);
            return Err(Error::unavailable(Situation::LeaseHeld {
                holder: lease.holder,
                fence: lease.fence,
            }));
        }
        Ok(lease)
    }

    fn release_lease(&self, lease: &Lease) -> Result<()> {
        self.now().release_lease(lease)
    }

    fn lease_free(&self) -> Result<bool> {
        self.now().lease_free()
    }

    fn tip(&self) -> Result<Option<(Position, Digest)>> {
        self.now().tip()
    }

    fn append(&self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        self.now().append(lease, events)
    }

    fn import(&self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        self.now().import(lease, events)
    }

    fn read(&self, from: Position, limit: u32) -> Result<Vec<CanonicalEvent>> {
        self.now().read(from, limit)
    }

    fn seal_segment(
        &self,
        lease: &Lease,
        imported: bool,
        first: &mut dyn FnMut(&SegmentDigest) -> Result<Vec<CanonicalEvent>>,
    ) -> Result<Option<SegmentDigest>> {
        self.now().seal_segment(lease, imported, first)
    }

    fn segments(&self) -> Result<Vec<SegmentDigest>> {
        self.now().segments()
    }

    fn active_segment_bytes(&self) -> Option<u64> {
        self.now().active_segment_bytes()
    }

    fn projection(&self) -> &dyn ProjectionStore {
        self.now().projection()
    }

    fn view(&self, as_of: Option<Position>) -> Result<Box<dyn ProjectionView + '_>> {
        self.now().view(as_of)
    }

    fn ranked(&self, ty: &TypeName, asked: &Rankings, k: u32) -> Result<Ranked> {
        self.now().ranked(ty, asked, k)
    }

    fn vectors(&self) -> Option<&dyn VectorIndex> {
        self.now().vectors()
    }

    fn mark_built(&self) -> Result<Option<Position>> {
        self.now().mark_built()
    }

    fn swap(&self, lease: &Lease) -> Result<Option<Position>> {
        self.now().swap(lease)
    }

    fn may_swap(&self) -> Result<bool> {
        self.now().may_swap()
    }
}

impl Lake {
    /// Open the lake at `location` as a standby: read-only, as a reader
    /// opens it, until [`Turn::take_over`] takes the lease.
    pub(crate) fn standby(location: &Location, seams: Seams) -> Result<Lake> {
        let opener = opener(location, &seams)?;
        let written = log::one_format(&location.lake, opener.log_ends(location)?.as_ref())?;
        log::read_as_is(&location.lake, written)?;
        let read = opener.open(location, OpenMode::Read)?;
        let engine: Arc<dyn LakeEngine> = Arc::new(Standby::new(opener, location, read));
        let lake = Lake::assemble(location, seams, engine, None);
        lake.catch_up_in(&mut lakelands_core::lock(&lake.writer))?;
        Ok(lake)
    }
}

impl Turn<'_> {
    /// Take the lease where no process holds it, as a writer's open takes
    /// it: a fence greater than every earlier one, the log folded from the
    /// projection's mark, then what the open writes under the lease
    /// ([`Turn::leased`]). Answers whether this handle holds the lease;
    /// `false`, with nothing changed, while another holds it. A take that
    /// fails after the lease is taken gives it up again.
    pub(crate) fn take_over(&self) -> Result<bool> {
        let mut w = self.writer();
        if w.lease.is_some() || w.lost {
            return Ok(true);
        }
        if !self.engine.lease_free()? {
            return Ok(false);
        }
        let lease = match self.engine.take_lease(&holder()) {
            Ok(lease) => lease,
            Err(e) if held(&e) => return Ok(false),
            Err(e) => return Err(e),
        };
        w.lease = Some(lease.clone());
        let caught = self.catch_up_in(&mut w);
        drop(w);
        let taken = caught.and_then(|()| self.leased(&lease));
        let mut w = self.writer();
        if let Err(e) = taken {
            w.lease = None;
            let _ = self.engine.release_lease(&lease);
            self.unleased();
            return Err(e);
        }
        self.publish(&w);
        Ok(true)
    }
}

/// Whether `e` refuses the lease because another holds it.
fn held(e: &Error) -> bool {
    matches!(
        e.situation(),
        Some(Situation::LeaseHeld { .. } | Situation::LeaseLocked { .. })
    )
}
