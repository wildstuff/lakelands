//! Conformance: runs the Lakelands conformance suite against the `memory`
//! engine, through the store.
//!
//! The suite is the specification: every registered check in
//! `lakelands_core::conformance` runs here, each on a fresh lake. Each lake
//! gets a name no other check uses, since lakes held in memory live as long
//! as the process.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use lakelands_api::engine::OpenMode;
use lakelands_api::seam::Seams;
use lakelands_api::{LakeStore, Location};
use lakelands_core::conformance::checks::ALL;
use lakelands_core::conformance::{FreshLake, Harness, run_all};
use lakelands_engine_mem::MemoryOpener;

struct Memory;

impl Harness for Memory {
    fn name(&self) -> &str {
        "memory"
    }

    fn fresh(&self) -> FreshLake<'_> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let lake = format!("conformance-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        Box::pin(async move {
            let seams = Seams {
                engines: vec![Arc::new(MemoryOpener::new())],
                backings: lakelands_core::conformance::backing::backings(),
                ..Seams::default()
            };
            let store: Arc<dyn LakeStore> =
                lakelands_store::open(Location::memory(lake), seams, OpenMode::Create).await?;
            Ok(store)
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_registered_check_passes_on_the_memory_engine() {
    assert!(!ALL.is_empty(), "no conformance check is registered");
    let report = run_all(&Memory).await;
    assert!(report.is_green(), "{report}");
    assert_eq!(report.passed.len(), ALL.len());
}
