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

struct Memory;

impl EngineHarness for Memory {
    fn name(&self) -> &str {
        "memory"
    }

    fn fresh(&self) -> lakelands_api::Result<Box<dyn LakeEngine>> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let lake = format!("engine-seam-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        MemoryOpener.open(&Location::memory(lake), OpenMode::Create)
    }

    fn reopen(&self, lake: &LakeId, mode: OpenMode) -> lakelands_api::Result<Box<dyn LakeEngine>> {
        MemoryOpener.open(&Location::memory(lake.as_str()), mode)
    }
}

#[test]
fn every_engine_check_passes_on_the_memory_engine() {
    assert!(!ENGINE_CHECKS.is_empty(), "no engine check is registered");
    let report = run_engine_checks(&Memory);
    assert!(report.is_green(), "{report}");
    assert_eq!(report.passed.len(), ENGINE_CHECKS.len());
}
