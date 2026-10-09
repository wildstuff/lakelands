//! Engine seam: runs every registered engine-seam check against the
//! `memory` engine.
//!
//! The checks live in `lakelands_core::conformance::engine`; this file
//! only opens fresh lakes for them. Each lake gets a name no other check
//! uses, since lakes held in memory live as long as the process.

use std::sync::atomic::{AtomicU64, Ordering};

use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode};
use lakelands_api::{LakeId, Location};
use lakelands_core::conformance::engine::{ENGINE_CHECKS, EngineHarness, run_engine_checks};
use lakelands_engine_mem::MemoryOpener;

/// Opens every check's lakes through one opener, so a reopen finds them.
struct Memory {
    opener: MemoryOpener,
}

impl EngineHarness for Memory {
    fn name(&self) -> &str {
        "memory"
    }

    fn fresh(&self) -> lakelands_api::Result<Box<dyn LakeEngine>> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let lake = format!("engine-seam-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        self.opener.open(&Location::memory(lake), OpenMode::Create)
    }

    fn reopen(&self, lake: &LakeId, mode: OpenMode) -> lakelands_api::Result<Box<dyn LakeEngine>> {
        self.opener.open(&Location::memory(lake.as_str()), mode)
    }
}

#[test]
fn every_engine_check_passes_on_the_memory_engine() {
    assert!(!ENGINE_CHECKS.is_empty(), "no engine check is registered");
    let report = run_engine_checks(&Memory {
        opener: MemoryOpener::new(),
    });
    assert!(report.is_green(), "{report}");
    assert_eq!(report.passed.len(), ENGINE_CHECKS.len());
}

/// What every engine check asserts on the memory engine, one line per
/// check, as the suite's tally prints it, by hand:
/// `cargo test -p lakelands-engine-mem --test engine_seam -- --ignored --nocapture tally`.
#[test]
#[ignore = "prints the tallies; run by hand with --ignored"]
fn the_tally_of_every_engine_check_on_the_memory_engine() {
    let report = run_engine_checks(&Memory {
        opener: MemoryOpener::new(),
    });
    for (check, tally) in &report.tallies {
        println!("tally {} {} {check}", tally.evaluations, tally.sites);
    }
    assert!(report.is_green(), "{report}");
}
