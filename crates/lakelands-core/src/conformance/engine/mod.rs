//! The engine-seam checks: what every engine promises through
//! [`LakeEngine`], checked on the seam itself.
//!
//! The suite in the parent module checks meaning through a lake's doors;
//! it reaches an engine only through the store. These checks reach the
//! engine directly, for promises the store relies on and cannot see
//! through its doors: that the bytes of an event come back unchanged,
//! that a stale or held lease is refused, that a dropped writer gives its
//! lease up, that a read-only handle never writes, that an append is whole
//! or nothing, that a view at a position reads the states then, and that
//! edges are derived the same way on every engine.
//!
//! # Running them
//!
//! An engine's test implements [`EngineHarness`], which opens a brand-new,
//! empty lake for every check and reopens one as another process would,
//! and calls [`run_engine_checks`]; the
//! [`Report`] lists every pass and every failure. The seam is synchronous,
//! so the checks are too. The run never stops at the first failure, and a
//! check that panics is reported as failed.
//!
//! # Adding a check
//!
//! A check is one `fn(&dyn LakeEngine) -> Result<(), Failure>` whose name
//! is a sentence in snake_case stating what must hold, turned into an
//! [`EngineCheck`] constant by one `engine_check!` line in the file of its
//! subject, and one line in [`ENGINE_CHECKS`]. Nothing else registers a
//! check.

use std::fmt;
use std::panic::AssertUnwindSafe;

use lakelands_api::engine::{CanonicalEvent, LakeEngine, Lease, OpenMode};
use lakelands_api::event::{Digest, EventId};
use lakelands_api::id::LakeId;
use lakelands_api::id::Position;
use serde_json::{Value, json};

use super::{Failure, Report, panic_message};

/// Turns a check function into an [`EngineCheck`] constant whose `name`
/// is the function's own name.
/// A check marked `reopening` also gets the harness, to open more
/// handles onto the same lake.
macro_rules! engine_check {
    ($(#[$doc:meta])* $konst:ident = reopening $run:ident, $about:expr) => {
        $(#[$doc])*
        pub const $konst: $crate::conformance::engine::EngineCheck =
            $crate::conformance::engine::EngineCheck {
                name: stringify!($run),
                about: $about,
                run: $run,
            };
    };
    ($(#[$doc:meta])* $konst:ident = $run:ident, $about:expr) => {
        $(#[$doc])*
        pub const $konst: $crate::conformance::engine::EngineCheck =
            $crate::conformance::engine::EngineCheck {
                name: stringify!($run),
                about: $about,
                run: |engine, _| $run(engine),
            };
    };
}

pub mod log;
pub mod projection;
pub mod reads;

/// Opens lakes for the engine checks: one engine, as its test configures
/// it.
pub trait EngineHarness: Send + Sync {
    /// The engine's name, as the report shows it.
    fn name(&self) -> &str;

    /// A brand-new, empty lake, opened for writing. Every check gets its
    /// own, so no check sees another's events or states.
    fn fresh(&self) -> lakelands_api::Result<Box<dyn LakeEngine>>;

    /// Another handle onto the lake `lake` names, opened in `mode`, as a
    /// second process would open it.
    fn reopen(&self, lake: &LakeId, mode: OpenMode) -> lakelands_api::Result<Box<dyn LakeEngine>>;
}

/// One promise of the engine seam.
#[derive(Clone, Copy)]
pub struct EngineCheck {
    /// A sentence in snake_case stating what must hold.
    pub name: &'static str,
    /// Where the rule is stated, and why it matters, in one sentence.
    pub about: &'static str,
    /// The check itself, run against a fresh lake; the harness reopens it.
    pub run: fn(&dyn LakeEngine, &dyn EngineHarness) -> Result<(), Failure>,
}

impl fmt::Debug for EngineCheck {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineCheck")
            .field("name", &self.name)
            .finish()
    }
}

/// Every engine-seam check, in the order they run.
pub const ENGINE_CHECKS: &[EngineCheck] = &[
    log::BYTES_UNCHANGED,
    log::STALE_LEASE,
    log::HELD_LEASE,
    log::GREATER_FENCE,
    log::WHOLE_OR_NOTHING,
    log::SEGMENT_DIGEST,
    log::RELEASE_STALE,
    log::READ_BOUNDS,
    log::FENCE_AT_ZERO,
    log::IMPORT_KEEPS_FENCES,
    log::IMPORT_ONLY_INTO_IMPORTED,
    log::SEALS,
    log::OPENING,
    log::READ_ONLY_OPEN,
    log::DROPPED_WRITER,
    log::READ_ONLY_HANDLE,
    projection::VIEW_AT_A_POSITION,
    projection::PAST_THE_FOLD,
    projection::ROLLBACK,
    projection::EDGES_IN_OUT_BOTH,
    projection::EDGES_OF_MANY_KEYS,
    projection::GET_MANY,
    projection::EDGES_TO_RETIRED,
    projection::EDGES_FOLLOW_THE_LATEST_STATE,
    projection::COMMIT_INVALID,
    projection::OPEN_TRANSACTION,
    projection::VIEW_KEEPS_POSITION,
    projection::SAME_POSITION,
    projection::PUT_BEFORE_A_LATER_STATE,
    projection::PUTS_IN_ONE_TRANSACTION,
    projection::CLEAR,
    reads::MIXED_KINDS,
    reads::AFTER_AND_LIMIT,
    reads::KEY_BATCHES,
    reads::FIELD_ORDER_WALK,
    reads::SETTINGS_ORDERED,
    reads::SORTED_PAGE,
    reads::SORTED_PAGE_PINNED,
    reads::FILTER_AS_EVALUATOR,
    reads::EQUAL_IS_IDENTITY,
    reads::FLAGS_AND_VALID_AT,
    reads::COUNT_IS_THE_SCAN,
    reads::LEXICAL_HITS,
    reads::LEXICAL_BEYOND_ASCII,
    reads::LEXICAL_EARLIER,
    reads::LEXICAL_NAMED_FIELDS,
    reads::LEXICAL_REDECLARED_MID_FOLD,
    reads::NEAREST_BY_COSINE,
    reads::ZERO_AND_WRONG_LENGTH,
    reads::PUT_AND_REMOVE,
    reads::EARLIER_STATES,
    reads::LATER_OUTPUT_HOLDS,
    reads::FIRST_APPLIED,
    reads::PROPOSALS,
    reads::LINK_EDGES,
    reads::EDGES_READ_WHAT_THEY_REACH,
    log::APPENDED_OUTLIVES_ITS_HANDLE,
];

/// Runs every engine check against fresh lakes from `harness`, in
/// registry order, and reports them all.
pub fn run_engine_checks(harness: &dyn EngineHarness) -> Report {
    let mut report = Report {
        harness: harness.name().to_owned(),
        ..Report::default()
    };
    for check in ENGINE_CHECKS {
        match run_engine_check(harness, check) {
            Ok(()) => report.passed.push(check.name),
            Err(failure) => report.failed.push((check.name, failure)),
        }
    }
    report
}

/// Runs one engine check on a fresh lake. A harness that cannot open
/// one, and a check that panics, are failures of that check.
pub fn run_engine_check(harness: &dyn EngineHarness, check: &EngineCheck) -> Result<(), Failure> {
    let engine = match std::panic::catch_unwind(AssertUnwindSafe(|| harness.fresh())) {
        Ok(Ok(engine)) => engine,
        Ok(Err(e)) => {
            return Err(Failure::new(format!(
                "the harness could not open a fresh lake: {e}"
            )));
        }
        Err(panic) => {
            return Err(Failure::new(format!(
                "the harness panicked opening a fresh lake: {}",
                panic_message(panic.as_ref())
            )));
        }
    };
    match std::panic::catch_unwind(AssertUnwindSafe(|| (check.run)(engine.as_ref(), harness))) {
        Ok(outcome) => outcome,
        Err(panic) => Err(Failure::new(format!(
            "the check panicked: {}",
            panic_message(panic.as_ref())
        ))),
    }
}

/// One event whose canonical bytes are those of `body` with the envelope
/// members the engine checks, hashed as the log hashes events.
pub fn event(position: u64, prev: &Digest, fence: u64, body: Value) -> CanonicalEvent {
    let preimage = json!({
        "position": position,
        "prev": prev.0,
        "fence": fence,
        "body": body,
    });
    let hash = Digest(crate::canonical::sha256_hex(&preimage));
    let mut whole = preimage;
    whole["hash"] = Value::String(hash.0.clone());
    CanonicalEvent {
        position: Position(position),
        id: EventId(format!("{position:032x}")),
        prev: prev.clone(),
        hash,
        fence,
        bytes: crate::canonical::to_vec(&whole),
    }
}

/// `n` chained events from position `start`, following `prev`, under
/// `lease`.
pub fn chain(start: u64, n: u64, prev: &Digest, lease: &Lease) -> Vec<CanonicalEvent> {
    let mut prev = prev.clone();
    (start..start + n)
        .map(|p| {
            // Position 0 is written before any lease exists, under fence 0.
            let fence = if p == 0 { 0 } else { lease.fence };
            let e = event(p, &prev, fence, json!({"kind": "note", "n": p}));
            prev = e.hash.clone();
            e
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Broken;

    impl EngineHarness for Broken {
        fn name(&self) -> &str {
            "broken"
        }

        fn fresh(&self) -> lakelands_api::Result<Box<dyn LakeEngine>> {
            Err(lakelands_api::Error::Unavailable {
                message: "no lake on purpose".into(),
            })
        }

        fn reopen(&self, _: &LakeId, _: OpenMode) -> lakelands_api::Result<Box<dyn LakeEngine>> {
            self.fresh()
        }
    }

    struct Panics;

    impl EngineHarness for Panics {
        fn name(&self) -> &str {
            "panics"
        }

        fn fresh(&self) -> lakelands_api::Result<Box<dyn LakeEngine>> {
            panic!("opening panicked on purpose")
        }

        fn reopen(&self, _: &LakeId, _: OpenMode) -> lakelands_api::Result<Box<dyn LakeEngine>> {
            self.fresh()
        }
    }

    #[test]
    fn every_check_is_reported_when_no_lake_opens() {
        let report = run_engine_checks(&Broken);
        assert_eq!(report.harness, "broken");
        assert!(report.passed.is_empty());
        assert_eq!(report.failed.len(), ENGINE_CHECKS.len());
        assert!(report.failed[0].1.message.contains("no lake on purpose"));
    }

    #[test]
    fn a_panicking_harness_fails_each_check_without_stopping_the_run() {
        let report = run_engine_checks(&Panics);
        assert_eq!(report.failed.len(), ENGINE_CHECKS.len());
        assert!(report.failed[0].1.message.contains("on purpose"));
    }

    #[test]
    fn check_names_are_unique_sentences() {
        let mut names: Vec<_> = ENGINE_CHECKS.iter().map(|c| c.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ENGINE_CHECKS.len());
        assert!(names.iter().all(|n| n.contains('_') && !n.is_empty()));
    }

    #[test]
    fn events_chain_by_their_hashes() {
        let lease = Lease {
            fence: 3,
            holder: "h".into(),
        };
        let events = chain(0, 3, &Digest::zero(), &lease);
        assert_eq!(events[0].prev, Digest::zero());
        assert_eq!(events[1].prev, events[0].hash);
        assert_eq!(events[2].prev, events[1].hash);
        assert_eq!(events[0].fence, 0, "position 0 is written before any lease");
        assert!(events[1..].iter().all(|e| e.fence == 3));
    }
}
