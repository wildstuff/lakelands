//! The log, the writer lease and the segments of one lake.
//!
//! The log keeps each [`CanonicalEvent`] exactly as the store handed it,
//! bytes included, and gives the same value back. An append is checked
//! before anything changes and then made whole: the lease must be the
//! current one and every event must carry its fence, the positions must
//! continue the tip with no gap and no overlap, each `prev` must be
//! the hash before it, and no event's bytes may hold a line feed. The engine does not recompute hashes; verifying
//! them is the store's, over the bytes it reads back.
//!
//! The lease is a fencing token and a holder. Taking it while it is held
//! fails at once and names the holder; every take gets a token greater
//! than any before, so a writer that lost the lease and comes back is
//! refused by its stale token rather than forking the chain.
//!
//! Segments are ranges of the log. Sealing closes the active one,
//! digests it as a segment file would hold it (each event's bytes
//! followed by a line feed, `docs/log-format.md` §4.2), and begins the
//! next with its first events, in one change.

use lakelands_api::engine::{CanonicalEvent, Lease};
use lakelands_api::error::{Case, Error, Result, Situation};
use lakelands_api::event::{Digest, SegmentDigest};
use lakelands_api::id::Position;
use sha2::{Digest as _, Sha256};

use crate::invalid;

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
            return Err(invalid(Case::ImportIntoHeld { tip }));
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
        let mut hasher = Sha256::new();
        for event in active {
            hasher.update(&event.bytes);
            hasher.update(b"\n");
        }
        let segment = SegmentDigest {
            first: Position(from),
            last: last.position,
            digest: Digest(lakelands_core::canonical::hex(&hasher.finalize())),
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
mod tests {
    use super::*;
    use crate::tests::chain;

    fn is_unavailable(r: &Result<impl std::fmt::Debug>) -> bool {
        matches!(r, Err(Error::Unavailable { .. }))
    }

    /// The case an append was refused for as invalid.
    fn refused(r: &Result<impl std::fmt::Debug>) -> Option<Case> {
        match r {
            Err(e @ Error::Refused { .. }) => e.case().map(|said| said.case.clone()),
            _ => None,
        }
    }

    #[test]
    fn a_held_lease_is_refused_naming_its_holder() {
        let mut log = Log::default();
        log.take_lease("writer one").unwrap();
        let e = log.take_lease("writer two").unwrap_err();
        let Some(Situation::LeaseHeld { holder, .. }) = e.situation() else {
            panic!("a second take was refused for another reason: {e:?}");
        };
        assert_eq!(holder, "writer one");
    }

    #[test]
    fn every_take_has_a_greater_fence() {
        let mut log = Log::default();
        let a = log.take_lease("a").unwrap();
        log.release_lease(&a);
        let b = log.take_lease("b").unwrap();
        assert!(a.fence >= 1 && b.fence > a.fence);
    }

    #[test]
    fn releasing_a_stale_lease_leaves_the_current_holder_in_place() {
        let mut log = Log::default();
        let a = log.take_lease("a").unwrap();
        log.release_lease(&a);
        let b = log.take_lease("b").unwrap();
        log.release_lease(&a);
        assert!(
            log.append(&b, &chain(0, 1, b.fence, Digest::zero()))
                .is_ok()
        );
    }

    #[test]
    fn a_stale_lease_cannot_append_or_seal() {
        let mut log = Log::default();
        let a = log.take_lease("a").unwrap();
        log.release_lease(&a);
        let b = log.take_lease("b").unwrap();
        assert!(is_unavailable(
            &log.append(&a, &chain(0, 1, a.fence, Digest::zero()))
        ));
        assert!(is_unavailable(&log.seal(&a, false, &mut |_| Ok(vec![]))));
        assert!(
            log.append(&b, &chain(0, 1, b.fence, Digest::zero()))
                .is_ok()
        );
    }

    #[test]
    fn an_event_whose_fence_differs_from_the_lease_is_refused() {
        let mut log = Log::default();
        let a = log.take_lease("a").unwrap();
        let mut first = chain(0, 1, a.fence, Digest::zero());
        first[0].fence = a.fence;
        assert!(
            matches!(
                refused(&log.append(&a, &first)),
                Some(Case::FenceWrong { wanted: 0, .. })
            ),
            "position 0 carries fence 0"
        );
        assert!(log.tip().is_none());
        let created = chain(0, 1, a.fence, Digest::zero());
        log.append(&a, &created).unwrap();
        let wrong = chain(1, 1, a.fence + 1, created[0].hash.clone());
        assert!(matches!(
            refused(&log.append(&a, &wrong)),
            Some(Case::FenceWrong { .. })
        ));
        assert_eq!(log.tip().map(|t| t.0), Some(Position(0)));
    }

    #[test]
    fn a_gap_an_overlap_or_a_wrong_prev_refuses_the_whole_append() {
        let mut log = Log::default();
        let l = log.take_lease("a").unwrap();
        let first = chain(0, 2, l.fence, Digest::zero());
        log.append(&l, &first).unwrap();
        let tip = log.tip().unwrap().1;

        let gap = chain(3, 1, l.fence, tip.clone());
        let next = Some(Case::AppendNotNext {
            position: Position(3),
            next: 2,
        });
        assert_eq!(refused(&log.append(&l, &gap)), next);
        let overlap = chain(1, 2, l.fence, tip.clone());
        let next = Some(Case::AppendNotNext {
            position: Position(1),
            next: 2,
        });
        assert_eq!(refused(&log.append(&l, &overlap)), next);
        let mut wrong_prev = chain(2, 3, l.fence, tip.clone());
        wrong_prev[2].prev = Digest::zero();
        assert!(matches!(
            refused(&log.append(&l, &wrong_prev)),
            Some(Case::AppendPrev {
                position: Position(4),
                ..
            })
        ));
        assert_eq!(log.read(Position(0), 100), first, "nothing was appended");
    }

    #[test]
    fn events_come_back_as_appended() {
        let mut log = Log::default();
        let l = log.take_lease("a").unwrap();
        let mut events = chain(0, 3, l.fence, Digest::zero());
        events[1].bytes = "{\"é\":\"\\\"\\u0001\"}".as_bytes().to_vec();
        log.append(&l, &events).unwrap();
        assert_eq!(log.read(Position(0), 10), events);
        assert_eq!(log.read(Position(1), 1), events[1..2].to_vec());
        assert!(log.read(Position(3), 10).is_empty());
        assert!(log.read(Position(u64::MAX), 10).is_empty());
        assert_eq!(log.tip(), Some((Position(2), events[2].hash.clone())));
    }

    #[test]
    fn an_empty_append_changes_nothing() {
        let mut log = Log::default();
        let l = log.take_lease("a").unwrap();
        log.append(&l, &[]).unwrap();
        assert!(log.tip().is_none());
    }

    #[test]
    fn sealing_digests_the_active_segment_as_lines() {
        let mut log = Log::default();
        let l = log.take_lease("a").unwrap();
        let events = chain(0, 2, l.fence, Digest::zero());
        assert_eq!(
            log.seal(&l, false, &mut |_| Ok(events.clone())).unwrap(),
            None,
            "an empty segment is not sealed"
        );
        log.append(&l, &events).unwrap();
        let more = chain(2, 1, l.fence, events[1].hash.clone());
        let sealed = log.seal(&l, false, &mut |_| Ok(more.clone()));
        let sealed = sealed.unwrap().unwrap();
        let mut file = Vec::new();
        for e in &events {
            file.extend_from_slice(&e.bytes);
            file.push(b'\n');
        }
        let digest = lakelands_core::canonical::hex(&Sha256::digest(&file));
        assert_eq!(
            sealed,
            SegmentDigest {
                first: Position(0),
                last: Position(1),
                digest: Digest(digest)
            }
        );
        assert!(log.seal(&l, false, &mut |_| Ok(vec![])).is_err());
        assert_eq!(log.segments(), vec![sealed.clone()], "sealed with no event");

        let last = chain(3, 1, l.fence, more[0].hash.clone());
        let second = log.seal(&l, false, &mut |_| Ok(last.clone()));
        let second = second.unwrap().unwrap();
        assert_eq!((second.first, second.last), (Position(2), Position(2)));
        assert_eq!(log.segments(), vec![sealed, second]);
    }
}
