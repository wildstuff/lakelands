use sha2::{Digest as _, Sha256};

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
