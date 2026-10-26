//! The log, the writer lease and the segments of one lake.
//!
//! The log keeps each [`CanonicalEvent`] exactly as the store handed it,
//! its line and its frame included, and gives the same value back. An
//! append is checked before anything changes and then made whole: the
//! lease must be the current one and every event must carry its fence,
//! the positions must continue the tip with no gap and no overlap, each
//! `prev` must be the hash before it, no event's line may hold a line
//! feed, and the append must hold each frame it shares whole. The engine
//! does not recompute hashes; verifying them is the store's, over the
//! events it restores from what it reads back.
//!
//! The lease is a fencing token and a holder. Taking it while it is held
//! fails at once and names the holder; every take gets a token greater
//! than any before, so a writer that lost the lease and comes back is
//! refused by its stale token rather than forking the chain.
//!
//! Segments are ranges of the log. Sealing closes the active one,
//! digests it over its events as restored (each event's canonical bytes
//! followed by a line feed, `docs/log-format.md` §4.2), and begins the
//! next with its first events, in one change.

use lakelands_api::engine::{CanonicalEvent, Lease};
use lakelands_api::error::{Case, Error, Part, Result, Situation};
use lakelands_api::event::{Digest, SegmentDigest};
use lakelands_api::id::Position;

/// One lake's log, lease and segments.
#[derive(Debug, Default)]
pub(crate) struct Log {
    /// Every event; an event's position is its index.
    events: Vec<CanonicalEvent>,
    /// The sealed segments, oldest first.
    sealed: Vec<SegmentDigest>,
    /// The lease, while someone holds it.
    lease: Option<Lease>,
    /// The greatest fence ever handed out; 0 before the first take.
    last_fence: u64,
    /// The last position `import` appended, while every event in the log
    /// came through it.
    imported: Option<Position>,
}

impl Log {
    /// The first and the last event, when the log holds any.
    pub(crate) fn ends(&self) -> Option<(CanonicalEvent, CanonicalEvent)> {
        self.events
            .first()
            .cloned()
            .zip(self.events.last().cloned())
    }

    /// Takes the lease for `holder`, or says who holds it.
    pub(crate) fn take_lease(&mut self, holder: &str) -> Result<Lease> {
        if let Some(held) = &self.lease {
            return Err(lakelands_core::engines::lease_held(
                &held.holder,
                held.fence,
            ));
        }
        self.last_fence += 1;
        let lease = Lease {
            fence: self.last_fence,
            holder: holder.to_owned(),
        };
        self.lease = Some(lease.clone());
        Ok(lease)
    }

    /// Gives the lease up. Releasing a lease that is no longer the current
    /// one changes nothing: it was lost already, and the current holder
    /// keeps its own.
    pub(crate) fn release_lease(&mut self, lease: &Lease) {
        if self.lease.as_ref() == Some(lease) {
            self.lease = None;
        }
    }

    /// Whether no one holds the lease.
    pub(crate) fn lease_free(&self) -> bool {
        self.lease.is_none()
    }

    /// Refuses a lease that is not the current one.
    fn check_lease(&self, lease: &Lease) -> Result<()> {
        match &self.lease {
            Some(current) if current == lease => Ok(()),
            current => Err(Error::unavailable(Situation::LeaseStale {
                fence: lease.fence,
                holder: current.as_ref().map(|c| c.holder.clone()),
                current: current.as_ref().map(|c| c.fence),
                undone: None,
            })),
        }
    }

    /// The last event's position and hash.
    pub(crate) fn tip(&self) -> Option<(Position, Digest)> {
        self.events.last().map(|e| (e.position, e.hash.clone()))
    }

    /// Appends `events` under `lease`, all or none.
    pub(crate) fn append(&mut self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        self.check_lease(lease)?;
        self.add(Some(lease.fence), events)
    }

    /// Appends events carried over from another lake's log under `lease`,
    /// all or none, while the log holds nothing but imported events; each
    /// keeps its own fence, which never falls. Later leases are greater
    /// than every fence imported.
    pub(crate) fn import(&mut self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        self.check_lease(lease)?;
        if let Some((tip, _)) = self.tip()
            && self.imported != Some(tip)
        {
            return Err(Error::invalid(Case::ImportIntoHeld { tip }));
        }
        self.add(None, events)?;
        if let Some(last) = events.last() {
            self.imported = Some(last.position);
            self.last_fence = self.last_fence.max(last.fence);
        }
        Ok(())
    }

    /// Adds `events` after the tip, all or none. Each carries `fence`, or,
    /// with none, a fence no lower than the event's before it; the event at
    /// position 0 carries fence 0.
    fn add(&mut self, fence: Option<u64>, events: &[CanonicalEvent]) -> Result<()> {
        let first = self.events.len() as u64;
        let prev = self.tip().map_or_else(Digest::zero, |(_, hash)| hash);
        let floor = self.events.last().map_or(0, |e| e.fence);
        lakelands_core::engines::append_fault(events, first, prev, fence, floor)?;
        lakelands_core::log::frame_fault(events)?;
        self.events.extend_from_slice(events);
        Ok(())
    }

    /// At most `limit` events from `from` on, as appended.
    pub(crate) fn read(&self, from: Position, limit: u32) -> Vec<CanonicalEvent> {
        let start = usize::try_from(from.0).unwrap_or(usize::MAX);
        self.events
            .iter()
            .skip(start)
            .take(limit as usize)
            .cloned()
            .collect()
    }

    /// Closes the active segment, if it holds any event, and begins the
    /// next with the events `first` answers for it, added as `append` adds
    /// them or, when `imported`, as `import` does: both, or nothing.
    pub(crate) fn seal(
        &mut self,
        lease: &Lease,
        imported: bool,
        first: &mut dyn FnMut(&SegmentDigest) -> Result<Vec<CanonicalEvent>>,
    ) -> Result<Option<SegmentDigest>> {
        self.check_lease(lease)?;
        let from = self.sealed.last().map_or(0, |s| s.last.0 + 1);
        let active = &self.events[usize::try_from(from)
            .unwrap_or(usize::MAX)
            .min(self.events.len())..];
        let Some(last) = active.last() else {
            return Ok(None);
        };
        let digest = lakelands_core::log::digest_of(active)
            .map_err(|(at, flaw)| lakelands_core::damaged(Part::Log, Some(at), flaw))?;
        let segment = SegmentDigest {
            first: Position(from),
            last: last.position,
            digest,
        };
        let events = lakelands_core::engines::first_of_next(first, &segment)?;
        if imported {
            self.import(lease, &events)?;
        } else {
            self.append(lease, &events)?;
        }
        self.sealed.push(segment.clone());
        Ok(Some(segment))
    }

    /// The sealed segments, oldest first.
    pub(crate) fn segments(&self) -> Vec<SegmentDigest> {
        self.sealed.clone()
    }
}

#[cfg(test)]
mod tests;
