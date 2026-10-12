//! The log and the lease through the seam (`docs/log-format.md` §4,
//! `docs/decisions/008-one-writer-per-lake.md`).
//!
//! The store canonicalises and the engine keeps bytes: what the store
//! appended is what it reads back, member for member and byte for byte,
//! or the hash chain it verifies breaks on the first engine that tidies
//! an event up. The lease keeps the chain from forking: a held lease is
//! refused at once and names its holder, every take has a greater fence,
//! and an append under a lease that is no longer current is refused. An
//! append is whole or nothing, so a refused append leaves the tip where
//! it was.

use std::time::{Duration, Instant};

use lakelands_api::Error;
use lakelands_api::engine::{CanonicalEvent, LakeEngine, OpenMode};
use lakelands_api::error::Situation;
use lakelands_api::event::Digest;
use lakelands_api::id::Position;
use serde_json::json;
use sha2::{Digest as _, Sha256};

use super::{EngineHarness, chain, event, seal};
use crate::conformance::fixture::{Must, is_invalid};
use crate::conformance::{Failure, Held};

engine_check!(
    BYTES_UNCHANGED = appended_bytes_come_back_unchanged,
    "log format §1.2: the engine keeps each event's canonical bytes and returns them unchanged, so the store can verify the hash chain over what it reads"
);

engine_check!(
    IMPORT_KEEPS_FENCES = an_import_keeps_each_events_fence_and_the_next_lease_is_greater,
    "030 and log format §4.1: `import` stores events carried over from another lake with the fences they were written under, which never fall, refuses a fence below the one before it, and the next lease taken is greater than every fence imported"
);

engine_check!(
    IMPORT_ONLY_INTO_IMPORTED = an_import_is_refused_once_the_log_holds_an_appended_event,
    "030: `import` continues only a log that holds nothing but imported events; once anything was appended, an import is refused and nothing of it is written"
);

engine_check!(
    STALE_LEASE = a_stale_lease_cannot_append,
    "008: every append is checked against the current fencing token, so a writer that lost the lease cannot fork the chain"
);

engine_check!(
    HELD_LEASE = a_held_lease_is_refused_at_once_naming_its_holder,
    "008: a second writer fails at once with an unavailable error that names the lease's holder; it never waits"
);

engine_check!(
    GREATER_FENCE = every_take_of_the_lease_has_a_greater_fence,
    "008: the fencing token increases with every new writer, which is what makes an older token stale"
);

engine_check!(
    WHOLE_OR_NOTHING = an_append_with_a_gap_or_a_wrong_prev_is_refused_whole,
    "log format §4.1: an append whose positions do not continue the tip, or whose prev is not the hash before it, is refused, and none of it is appended"
);

engine_check!(
    SEGMENT_DIGEST = a_sealed_segment_is_digested_as_its_lines,
    "log format §4.2: a segment's digest is SHA-256 over its file's bytes, each event's canonical bytes followed by a line feed, whatever the engine keeps them in"
);

engine_check!(
    RELEASE_STALE = releasing_a_stale_lease_leaves_the_current_holders_lease,
    "008: a writer that lost the lease cannot release its successor's, so the successor goes on appending"
);

engine_check!(
    READ_BOUNDS = reading_past_the_tip_or_with_limit_zero_answers_nothing,
    "seams §LakeEngine: read answers at most `limit` events from `from` on, nothing past the tip or for a limit of 0, and an empty lake has no tip"
);

engine_check!(
    FENCE_AT_ZERO = an_event_at_position_zero_with_a_fence_is_refused,
    "log format §4.1: the event at position 0 is written before any lease exists, under fence 0, and an engine refuses it under any other"
);

engine_check!(
    SEALS = sealing_under_a_stale_lease_is_refused_and_two_seals_list_contiguous_ranges,
    "log format §4.2: only the writer seals, and the sealed segments' ranges start at 0 and follow each other with no gap"
);

engine_check!(
    SEAL_OR_NOTHING = a_seal_that_cannot_begin_the_next_segment_leaves_the_log_as_it_was,
    "log format §4.2: a seal closes the active segment and begins the next with its first event in one step; one whose events do not continue the log by hash or by position, that is handed none, or whose events are imported behind appended ones is refused as invalid, one whose `first` answers an error answers that error, and one under a lease no longer current is unavailable before `first` is asked; each seals nothing and leaves the tip, with no segment sealed and behind a sealed one, and the next append lands in the segment that was active"
);

engine_check!(
    OPENING = reopening opening_a_missing_lake_to_read_or_write_is_unavailable_and_create_opens_an_existing_one,
    "seams §EngineOpener: reading or writing a lake that does not exist is unavailable, and create opens a lake that exists, as it is"
);

engine_check!(
    READ_ONLY_OPEN = reopening a_read_only_open_creates_and_changes_nothing,
    "008 and seams §EngineOpener: a reader opens a lake as it is; opening a missing lake read-only makes none, and a read-only handle writes neither the log nor the projection"
);

engine_check!(
    DROPPED_WRITER = reopening a_dropped_writer_releases_its_lease,
    "008: a lake is closed when its last handle goes, so a writer that ends without closing does not keep every later writer out"
);

engine_check!(
    READ_ONLY_HANDLE = reopening a_read_only_handle_refuses_the_lease_and_appends,
    "008: any number of readers may open a lake, and none of them can take the lease, append or seal, so only the writer extends the chain"
);

engine_check!(
    UNLEASED_HANDLE = reopening a_handle_that_never_took_the_lease_cannot_append_import_or_seal,
    "008: only the handle that took the lease extends the chain; another opened for writing, handed a copy of that lease, is refused an append, an import and a seal, and the writer goes on"
);

engine_check!(
    APPENDED_OUTLIVES_ITS_HANDLE = reopening an_append_that_returned_is_read_by_the_next_handle,
    "seams, append: an append is durable before it returns, so the events are there, byte for byte, for a handle opened after the writer's handle was dropped without closing"
);

fn an_append_that_returned_is_read_by_the_next_handle(
    engine: &dyn LakeEngine,
    harness: &dyn EngineHarness,
) -> Held {
    let writer = harness
        .reopen(engine.lake(), OpenMode::Write)
        .map_err(|e| Failure::new(format!("opening a handle for writing: {e}")))?;
    let lease = writer.take_lease("the writer").must("taking the lease")?;
    let events = chain(0, 3, &Digest::zero(), &lease);
    writer
        .append(&lease, &events)
        .must("appending three events")?;
    // The writer ends without releasing its lease or closing the lake.
    drop(writer);
    let next = harness
        .reopen(engine.lake(), OpenMode::Read)
        .map_err(|e| Failure::new(format!("opening the next handle: {e}")))?;
    let read = next
        .read(Position(0), 10)
        .must("reading through the next handle")?;
    ensure!(
        read == events,
        "three events were appended and the append returned; the writer's handle was then \
         dropped without closing, and a handle opened after it reads {} event(s){}",
        read.len(),
        if read.len() == events.len() {
            " whose bytes differ"
        } else {
            ""
        }
    );
    Ok(())
}

fn a_dropped_writer_releases_its_lease(
    engine: &dyn LakeEngine,
    harness: &dyn EngineHarness,
) -> Held {
    let writer = harness
        .reopen(engine.lake(), OpenMode::Write)
        .map_err(|e| Failure::new(format!("opening a second handle for writing: {e}")))?;
    writer
        .take_lease("the first writer")
        .must("taking the lease on the second handle")?;
    drop(writer);
    engine.take_lease("the next writer").map_err(|e| {
        Failure::new(format!(
            "the handle that held the lease was dropped without releasing it, and taking the \
             lease again was refused: {e}; a dropped handle must give its lease up"
        ))
    })?;
    Ok(())
}

fn a_read_only_handle_refuses_the_lease_and_appends(
    engine: &dyn LakeEngine,
    harness: &dyn EngineHarness,
) -> Held {
    let lease = engine.take_lease("conformance").must("taking the lease")?;
    let first = chain(0, 2, &Digest::zero(), &lease);
    engine.append(&lease, &first).must("appending two events")?;
    let reader = harness
        .reopen(engine.lake(), OpenMode::Read)
        .map_err(|e| Failure::new(format!("opening a read-only handle: {e}")))?;
    ensure!(
        reader
            .read(Position(0), 10)
            .must("reading through the reader")?
            == first,
        "a read-only handle does not read the events the writer appended"
    );
    ensure!(
        reader.take_lease("a reader").is_err(),
        "a handle opened read-only took the lease"
    );
    let next = chain(2, 1, &first[1].hash, &lease);
    ensure!(
        reader.append(&lease, &next).is_err(),
        "a handle opened read-only appended under the writer's lease"
    );
    ensure!(
        seal(reader.as_ref(), &lease, &next).is_err(),
        "a handle opened read-only sealed a segment"
    );
    let log = engine.read(Position(0), 10).must("reading the log")?;
    ensure!(
        log == first,
        "the log holds {} events after the reader's refused append; it must hold the writer's 2",
        log.len()
    );
    Ok(())
}

fn a_handle_that_never_took_the_lease_cannot_append_import_or_seal(
    engine: &dyn LakeEngine,
    harness: &dyn EngineHarness,
) -> Held {
    let lease = engine.take_lease("the writer").must("taking the lease")?;
    let other = harness
        .reopen(engine.lake(), OpenMode::Write)
        .map_err(|e| Failure::new(format!("opening a second handle for writing: {e}")))?;
    let imported = other.import(&lease, &another_lakes_log());
    let tip = engine.tip().must("reading the tip")?;
    ensure!(
        matches!(imported, Err(Error::Unavailable { .. })) && tip.is_none(),
        "a handle opened for writing that never took the lease imported under the writer's \
         lease, answering {imported:?}, and the log ends at {tip:?}; it must be unavailable and \
         leave the lake empty"
    );
    let first = chain(0, 2, &Digest::zero(), &lease);
    engine.append(&lease, &first).must("appending two events")?;
    let next = chain(2, 1, &first[1].hash, &lease);
    let appended = other.append(&lease, &next);
    ensure!(
        matches!(appended, Err(Error::Unavailable { .. })),
        "a handle opened for writing that never took the lease appended under the writer's \
         lease, answering {appended:?}; it must be unavailable"
    );
    let sealed = seal(other.as_ref(), &lease, &next);
    ensure!(
        matches!(sealed, Err(Error::Unavailable { .. })),
        "a handle opened for writing that never took the lease sealed under the writer's \
         lease, answering {sealed:?}; it must be unavailable"
    );
    drop(other);
    let log = engine.read(Position(0), 10).must("reading the log")?;
    let sealed = engine.segments().must("listing the segments")?;
    ensure!(
        log == first && sealed.is_empty(),
        "after the other handle's refused append and seal the log holds {} event(s) and {} \
         sealed segment(s); it must hold the writer's 2 and none",
        log.len(),
        sealed.len()
    );
    engine
        .append(&lease, &next)
        .must("appending under the writer's lease after the other handle was dropped")?;
    Ok(())
}

fn appended_bytes_come_back_unchanged(engine: &dyn LakeEngine) -> Held {
    let lease = engine.take_lease("conformance").must("taking the lease")?;
    // Traps for an engine that parses and re-serialises: a member name
    // beyond the basic plane sorts before one inside it in canonical
    // order and after it in byte order; a large number is written with
    // an explicit exponent sign; the text holds non-ASCII letters, quotes,
    // a backslash, a control character and a line separator.
    let odd = json!({
        "kind": "note",
        "\u{FF61}": 1,
        "\u{1F600}": 2,
        "large": 1.5e300,
        "text": "café \"quoted\" back\\slash \u{7} \u{2028} Grüße 🌊",
    });
    // Position 0 is written before any lease exists, under fence 0.
    let first = event(0, &Digest::zero(), 0, odd);
    let second = event(1, &first.hash, lease.fence, json!({"kind": "note", "n": 1}));
    let appended = vec![first, second];
    engine
        .append(&lease, &appended)
        .must("appending two events")?;

    let whole = engine.read(Position(0), 10).must("reading the log")?;
    ensure!(
        whole == appended,
        "the log read back is not what was appended; the engine must store and return \
         each event's members and bytes unchanged. appended {:?}, read {:?}",
        render(&appended),
        render(&whole)
    );
    let tail = engine
        .read(Position(1), 10)
        .must("reading from position 1")?;
    ensure!(
        tail == appended[1..],
        "reading from position 1 answered {:?}; it must answer the one event there",
        render(&tail)
    );
    let one = engine.read(Position(0), 1).must("reading one event")?;
    ensure!(
        one == appended[..1],
        "reading at most one event answered {} events",
        one.len()
    );
    let tip = engine.tip().must("reading the tip")?;
    ensure!(
        tip == Some((Position(1), appended[1].hash.clone())),
        "the tip after two events is {tip:?}; it must be position 1 and the second event's hash"
    );
    Ok(())
}

/// The events' bytes as text, for a failure message.
fn render(events: &[CanonicalEvent]) -> Vec<String> {
    events
        .iter()
        .map(|e| String::from_utf8_lossy(&e.bytes).into_owned())
        .collect()
}

fn a_stale_lease_cannot_append(engine: &dyn LakeEngine) -> Held {
    let old = engine.take_lease("first writer").must("taking the lease")?;
    engine.release_lease(&old).must("releasing the lease")?;
    let new = engine
        .take_lease("second writer")
        .must("taking the lease again")?;

    // The lake exists; the fence matters from position 1 on.
    let created = chain(0, 1, &Digest::zero(), &new);
    engine
        .append(&new, &created)
        .must("creating the lake under the current lease")?;
    let stale = chain(1, 1, &created[0].hash, &old);
    let refused = engine
        .append(&old, &stale)
        .must_fail("appending under a released lease")?;
    ensure!(
        matches!(refused, Error::Unavailable { .. }),
        "an append under a stale lease was refused as {refused:?}; it must be unavailable"
    );
    let wrong_fence = engine
        .append(&new, &stale)
        .must_fail("appending events that carry the old fence under the new lease")?;
    ensure!(
        is_invalid(&wrong_fence),
        "an event carrying another fence was refused as {wrong_fence:?}; it must be invalid"
    );
    let tip = engine.tip().must("reading the tip")?;
    ensure!(
        tip.as_ref().map(|t| t.0) == Some(Position(0)),
        "the stale appends were refused but the log now ends at {tip:?}; nothing may be appended"
    );
    engine
        .append(&new, &chain(1, 1, &created[0].hash, &new))
        .must("appending under the current lease")?;
    Ok(())
}

fn a_held_lease_is_refused_at_once_naming_its_holder(engine: &dyn LakeEngine) -> Held {
    let holder = "writer 7 on host lake-a";
    engine.take_lease(holder).must("taking the lease")?;
    let started = Instant::now();
    let refused = engine
        .take_lease("a second writer")
        .must_fail("taking a lease that is held")?;
    let waited = started.elapsed();
    let Some(Situation::LeaseHeld { holder: named, .. }) = refused.situation() else {
        return Err(Failure::new(format!(
            "taking a held lease was refused as {refused:?}; it must be unavailable, the lease \
             held"
        )));
    };
    ensure!(
        named == holder,
        "taking a held lease answered \"{refused}\", which does not name the holder \
         `{holder}`; the operator must learn who holds the lake"
    );
    ensure!(
        waited < Duration::from_secs(1),
        "taking a held lease took {waited:?} to be refused; it must fail at once, never wait"
    );
    Ok(())
}

fn every_take_of_the_lease_has_a_greater_fence(engine: &dyn LakeEngine) -> Held {
    let mut fences = Vec::new();
    for holder in ["one", "two", "three"] {
        let lease = engine.take_lease(holder).must("taking the lease")?;
        fences.push(lease.fence);
        engine.release_lease(&lease).must("releasing the lease")?;
    }
    ensure!(
        fences.windows(2).all(|w| w[0] < w[1]),
        "three takes of the lease got fences {fences:?}; each must be greater than the one before"
    );
    Ok(())
}

fn an_append_with_a_gap_or_a_wrong_prev_is_refused_whole(engine: &dyn LakeEngine) -> Held {
    let lease = engine.take_lease("conformance").must("taking the lease")?;
    let first = chain(0, 2, &Digest::zero(), &lease);
    engine.append(&lease, &first).must("appending two events")?;
    let tip_hash = first[1].hash.clone();

    let gap = chain(3, 2, &tip_hash, &lease);
    engine
        .append(&lease, &gap)
        .must_fail("appending from position 3 when the next position is 2")?;
    let overlap = chain(1, 2, &tip_hash, &lease);
    engine
        .append(&lease, &overlap)
        .must_fail("appending from position 1 when the next position is 2")?;
    let mut wrong_prev = chain(2, 3, &tip_hash, &lease);
    wrong_prev[2].prev = Digest::zero();
    engine
        .append(&lease, &wrong_prev)
        .must_fail("appending three events whose last names the wrong prev")?;
    let off_tip = chain(2, 1, &Digest::zero(), &lease);
    engine
        .append(&lease, &off_tip)
        .must_fail("appending an event whose prev is not the tip's hash")?;

    let log = engine.read(Position(0), 100).must("reading the log")?;
    ensure!(
        log == first,
        "after four refused appends the log holds {} events; a refused append must append \
         nothing, not even its valid first events",
        log.len()
    );
    Ok(())
}

fn a_sealed_segment_is_digested_as_its_lines(engine: &dyn LakeEngine) -> Held {
    let lease = engine.take_lease("conformance").must("taking the lease")?;
    let events = chain(0, 3, &Digest::zero(), &lease);
    let empty = seal(engine, &lease, &events).must("sealing an empty segment")?;
    let tip = engine.tip().must("reading the tip")?;
    ensure!(
        empty.is_none() && tip.is_none(),
        "sealing an empty active segment answered {empty:?} and left the tip {tip:?}; there is \
         nothing to seal, and the events that would begin the next segment are not added"
    );
    engine
        .append(&lease, &events)
        .must("appending three events")?;
    let next = chain(3, 1, &events[2].hash, &lease);
    let mut handed = None;
    let sealed = engine
        .seal_segment(&lease, false, &mut |closed| {
            handed = Some(closed.clone());
            Ok(next.clone())
        })
        .must("sealing the segment")?
        .ok_or_else(|| Failure::new("sealing a segment of three events answered none"))?;

    let mut file = Vec::new();
    for e in &events {
        file.extend_from_slice(&e.bytes);
        file.push(b'\n');
    }
    let digest = crate::canonical::hex(&Sha256::digest(&file));
    ensure!(
        sealed.first == Position(0) && sealed.last == Position(2) && sealed.digest.0 == digest,
        "the sealed segment is {sealed:?}; it must run from 0 to 2 with digest {digest}"
    );
    let segments = engine.segments().must("listing the segments")?;
    let begun = engine.read(Position(3), 10).must("reading the log")?;
    ensure!(
        segments == vec![sealed.clone()] && handed.as_ref() == Some(&sealed) && begun == next,
        "the segments listed are {segments:?}, `first` was handed {handed:?} and the next \
         segment holds {begun:?}; they must be the one just sealed, twice, and the event the \
         seal was handed"
    );
    Ok(())
}

fn releasing_a_stale_lease_leaves_the_current_holders_lease(engine: &dyn LakeEngine) -> Held {
    let old = engine.take_lease("first writer").must("taking the lease")?;
    engine.release_lease(&old).must("releasing the lease")?;
    let new = engine
        .take_lease("second writer")
        .must("taking the lease again")?;
    engine
        .release_lease(&old)
        .must("releasing the stale lease again")?;
    engine
        .append(&new, &chain(0, 2, &Digest::zero(), &new))
        .must("appending under the current lease after a stale release")?;
    let held = engine.take_lease("a third writer");
    ensure!(
        held.is_err(),
        "after the stale lease was released a third writer took the lease: {held:?}; the \
         current holder must keep it"
    );
    Ok(())
}

fn reading_past_the_tip_or_with_limit_zero_answers_nothing(engine: &dyn LakeEngine) -> Held {
    let empty = engine.tip().must("reading the tip of an empty lake")?;
    ensure!(
        empty.is_none(),
        "an empty lake has the tip {empty:?}; it has none"
    );
    let lease = engine.take_lease("conformance").must("taking the lease")?;
    let events = chain(0, 3, &Digest::zero(), &lease);
    engine
        .append(&lease, &events)
        .must("appending three events")?;
    for (from, limit, expected) in [
        (0, 0, 0),
        (3, 10, 0),
        (u64::MAX, 10, 0),
        (1, 1, 1),
        (1, 10, 2),
    ] {
        let got = engine
            .read(Position(from), limit)
            .must(&format!("reading from {from}, at most {limit}"))?;
        ensure!(
            got.len() == expected && got.first().is_none_or(|e| e.position == Position(from)),
            "reading from {from}, at most {limit}, of a log holding 0 to 2 answered {} event(s) \
             starting at {:?}; it must answer {expected} starting at {from}",
            got.len(),
            got.first().map(|e| e.position)
        );
    }
    Ok(())
}

fn an_event_at_position_zero_with_a_fence_is_refused(engine: &dyn LakeEngine) -> Held {
    let lease = engine.take_lease("conformance").must("taking the lease")?;
    let fenced = event(0, &Digest::zero(), lease.fence, json!({"kind": "note"}));
    let refused = engine
        .append(&lease, &[fenced])
        .must_fail("appending position 0 under the lease's fence")?;
    ensure!(
        is_invalid(&refused),
        "the event at position 0 carrying fence {} was refused as {refused:?}; it must be invalid",
        lease.fence
    );
    let tip = engine.tip().must("reading the tip")?;
    ensure!(tip.is_none(), "the refused event left the tip {tip:?}");
    Ok(())
}

fn sealing_under_a_stale_lease_is_refused_and_two_seals_list_contiguous_ranges(
    engine: &dyn LakeEngine,
) -> Held {
    let old = engine.take_lease("first writer").must("taking the lease")?;
    engine.release_lease(&old).must("releasing the lease")?;
    let lease = engine
        .take_lease("second writer")
        .must("taking the lease again")?;
    let first = chain(0, 2, &Digest::zero(), &lease);
    engine.append(&lease, &first).must("appending two events")?;
    let more = chain(2, 3, &first[1].hash, &lease);
    let stale = seal(engine, &old, &more);
    ensure!(
        matches!(stale, Err(Error::Unavailable { .. })),
        "sealing under a released lease answered {stale:?}; it must be unavailable"
    );
    seal(engine, &lease, &more).must("sealing the first segment")?;
    let last = chain(5, 1, &more[2].hash, &lease);
    seal(engine, &lease, &last).must("sealing the second segment")?;
    let ranges: Vec<(u64, u64)> = engine
        .segments()
        .must("listing the segments")?
        .iter()
        .map(|s| (s.first.0, s.last.0))
        .collect();
    ensure!(
        ranges == [(0, 1), (2, 4)],
        "two seals listed the ranges {ranges:?}; they must start at 0 and follow each other: \
         [(0, 1), (2, 4)]"
    );
    Ok(())
}

fn a_seal_that_cannot_begin_the_next_segment_leaves_the_log_as_it_was(
    engine: &dyn LakeEngine,
) -> Held {
    let old = engine.take_lease("first writer").must("taking the lease")?;
    engine.release_lease(&old).must("releasing the lease")?;
    let lease = engine.take_lease("conformance").must("taking the lease")?;
    let mut log = chain(0, 2, &Digest::zero(), &lease);
    engine.append(&lease, &log).must("appending two events")?;
    let fails = Error::unavailable(Situation::Moved);
    // With nothing sealed, and again behind a sealed segment.
    for sealed in [vec![], vec![(0, 2)]] {
        let (at, last) = (log.len() as u64, log.len() - 1);
        let next = |at, prev: usize| Ok(chain(at, 1, &log[prev].hash, &lease));
        let (own, import, stale) = ((&lease, false), (&lease, true), (&old, false));
        for (what, (held, imported), answer) in [
            ("a `prev` other than the tip's hash", own, next(at, 0)),
            ("a position past the next", own, next(at + 1, last)),
            ("no event", own, Ok(vec![])),
            ("an error from `first`", own, Err(fails.clone())),
            ("an import behind appended events", import, next(at, last)),
            ("its events under a stale lease", stale, next(at, last)),
        ] {
            let (mut asked, invalid) = (false, answer.is_ok() && held == &lease);
            let mut answer = Some(answer);
            let refused = engine.seal_segment(held, imported, &mut |_| {
                asked = true;
                answer.take().unwrap_or_else(|| Ok(vec![]))
            });
            let as_it_must = match &refused {
                Err(e) if invalid => is_invalid(e),
                Err(e) => matches!(e, Error::Unavailable { .. }) && asked == (held == &lease),
                Ok(_) => false,
            };
            let segments = engine.segments().must("listing the segments")?;
            let ranges: Vec<_> = segments.iter().map(|s| (s.first.0, s.last.0)).collect();
            let tip = engine.tip().must("reading the tip")?;
            let want = Some((Position(at - 1), log[last].hash.clone()));
            ensure!(
                as_it_must && ranges == sealed && tip == want,
                "a seal handed {what} answered {refused:?} (`first` asked: {asked}) and left the \
                 segments {ranges:?} and the tip {tip:?}; it must be refused (invalid for \
                 events that cannot enter and for none, `first`'s own error, unavailable under \
                 a stale lease before `first` is asked) and leave {sealed:?} and {want:?}"
            );
        }
        if sealed.is_empty() {
            let more = chain(2, 1, &log[1].hash, &lease);
            engine.append(&lease, &more).must("appending behind them")?;
            let next = chain(3, 1, &more[0].hash, &lease);
            let closed = seal(engine, &lease, &next).must("sealing after them")?;
            log = [log, more, next].concat();
            let read = engine.read(Position(0), 10).must("reading the log")?;
            ensure!(
                closed.as_ref().map(|s| (s.first.0, s.last.0)) == Some((0, 2)) && read == log,
                "after the refused seals the next one closed {closed:?} and the log reads \
                 {read:?}; the append between landed in the first segment, closed as one, 0 to 2"
            );
        }
    }
    Ok(())
}

fn opening_a_missing_lake_to_read_or_write_is_unavailable_and_create_opens_an_existing_one(
    engine: &dyn LakeEngine,
    harness: &dyn EngineHarness,
) -> Held {
    let missing = lakelands_api::id::LakeId::from(format!("{}-never-created", engine.lake()));
    for mode in [OpenMode::Read, OpenMode::Write] {
        let opened = harness.reopen(&missing, mode).map(|e| e.lake().clone());
        ensure!(
            matches!(opened, Err(Error::Unavailable { .. })),
            "opening a lake that does not exist {mode:?} answered {opened:?}; it must be \
             unavailable"
        );
    }
    let lease = engine.take_lease("conformance").must("taking the lease")?;
    let events = chain(0, 2, &Digest::zero(), &lease);
    engine
        .append(&lease, &events)
        .must("appending two events")?;
    let again = harness
        .reopen(engine.lake(), OpenMode::Create)
        .map_err(|e| Failure::new(format!("creating a lake that exists failed: {e}")))?;
    let tip = again.tip().must("reading the tip through the new handle")?;
    ensure!(
        tip.as_ref().map(|t| t.0) == Some(Position(1)),
        "create on an existing lake opened it with the tip {tip:?}; it must open it as it is, \
         ending at 1"
    );
    Ok(())
}

fn a_read_only_open_creates_and_changes_nothing(
    engine: &dyn LakeEngine,
    harness: &dyn EngineHarness,
) -> Held {
    let missing = lakelands_api::id::LakeId::from(format!("{}-read-only", engine.lake()));
    for attempt in ["first", "second"] {
        let opened = harness
            .reopen(&missing, OpenMode::Read)
            .map(|e| e.lake().clone());
        ensure!(
            matches!(opened, Err(Error::Unavailable { .. })),
            "the {attempt} read-only open of a lake that does not exist answered {opened:?}; it \
             must be unavailable, and create nothing a later open would find"
        );
    }
    let lease = engine.take_lease("conformance").must("taking the lease")?;
    let events = chain(0, 2, &Digest::zero(), &lease);
    engine
        .append(&lease, &events)
        .must("appending two events")?;
    let p = engine.projection();
    p.begin().must("beginning a transaction")?;
    p.commit(Position(1)).must("committing through 1")?;
    let reader = harness
        .reopen(engine.lake(), OpenMode::Read)
        .map_err(|e| Failure::new(format!("opening a read-only handle: {e}")))?;
    let begun = reader.projection().begin();
    ensure!(
        matches!(begun, Err(Error::Unavailable { .. })),
        "a read-only handle began a projection transaction: {begun:?}; it must be unavailable"
    );
    let cleared = reader.projection().clear();
    ensure!(
        matches!(cleared, Err(Error::Unavailable { .. })),
        "a read-only handle cleared the projection: {cleared:?}; it must be unavailable"
    );
    let tip = engine.tip().must("reading the tip")?;
    let folded = engine
        .projection()
        .folded_through()
        .must("reading the folded position")?;
    ensure!(
        tip.as_ref().map(|t| t.0) == Some(Position(1)) && folded == Some(Position(1)),
        "after a read-only handle opened, the log ends at {tip:?} and the projection is folded \
         through {folded:?}; both must be as the writer left them, at 1"
    );
    Ok(())
}

/// The events of another lake's log: fence 0 at position 0, then two
/// writers' events under fences 5 and 9.
fn another_lakes_log() -> Vec<CanonicalEvent> {
    let mut out: Vec<CanonicalEvent> = Vec::new();
    for (position, fence) in [(0, 0), (1, 5), (2, 5), (3, 9), (4, 9)] {
        let prev = out.last().map_or_else(Digest::zero, |e| e.hash.clone());
        out.push(event(
            position,
            &prev,
            fence,
            json!({"kind": "note", "n": position}),
        ));
    }
    out
}

fn an_import_keeps_each_events_fence_and_the_next_lease_is_greater(
    engine: &dyn LakeEngine,
) -> Held {
    let lease = engine
        .take_lease("conformance import")
        .must("taking the lease")?;
    let log = another_lakes_log();
    engine
        .import(&lease, &log[..3])
        .must("importing the first three events")?;
    let falling = event(3, &log[2].hash, 4, json!({"kind": "note", "n": 3}));
    let refused = engine
        .import(&lease, &[falling])
        .must_fail("importing an event whose fence falls below the one before it")?;
    ensure!(
        matches!(refused, lakelands_api::Error::Refused { .. }),
        "an imported event whose fence falls was refused as {refused:?}; it must be invalid"
    );
    engine
        .import(&lease, &log[3..])
        .must("importing the rest")?;
    let read = engine.read(Position(0), 10).must("reading the log")?;
    ensure!(
        read == log,
        "the imported log reads back as {:?}; it must be the events imported, fences and bytes \
         unchanged: {:?}",
        render(&read),
        render(&log)
    );
    engine.release_lease(&lease).must("releasing the lease")?;
    let next = engine
        .take_lease("conformance")
        .must("taking the lease again")?;
    ensure!(
        next.fence > 9,
        "the lease taken after an import of fences up to 9 has fence {}; it must be greater",
        next.fence
    );
    Ok(())
}

fn an_import_is_refused_once_the_log_holds_an_appended_event(engine: &dyn LakeEngine) -> Held {
    let lease = engine.take_lease("conformance").must("taking the lease")?;
    let first = event(0, &Digest::zero(), 0, json!({"kind": "note", "n": 0}));
    engine
        .append(&lease, std::slice::from_ref(&first))
        .must("appending position 0")?;
    let carried = event(1, &first.hash, 5, json!({"kind": "note", "n": 1}));
    let refused = engine
        .import(&lease, &[carried])
        .must_fail("importing into a log that holds an appended event")?;
    ensure!(
        matches!(refused, lakelands_api::Error::Refused { .. }),
        "an import into a log that holds an appended event was refused as {refused:?}; it \
         must be invalid"
    );
    let log = engine.read(Position(0), 10).must("reading the log")?;
    ensure!(
        log == [first],
        "the log holds {} events after the refused import; it must hold the one appended",
        log.len()
    );
    Ok(())
}
