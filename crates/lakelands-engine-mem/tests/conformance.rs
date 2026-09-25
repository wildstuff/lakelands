//! Conformance: runs the Lakelands conformance suite against the `memory`
//! engine, through the store.
//!
//! The suite is the specification: every registered check in
//! `lakelands_core::conformance` runs here, each on a fresh lake. Each lake
//! gets a name no other check uses, since lakes held in memory live as long
//! as the process. The lakes are the harness's one opener's, so a check
//! can open its lake again.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use lakelands_api::engine::OpenMode;
use lakelands_api::seam::Seams;
use lakelands_api::{LakeStore, Location};
use lakelands_core::conformance::checks::ALL;
use lakelands_core::conformance::{FreshLake, Harness, Reopen, run_all};
use lakelands_engine_mem::MemoryOpener;

struct Memory {
    opener: MemoryOpener,
}

impl Harness for Memory {
    fn name(&self) -> &str {
        "memory"
    }

    fn fresh(&self, host: Seams) -> FreshLake<'_> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let lake = format!("conformance-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let opener = self.opener.clone();
        Box::pin(async move {
            let seams = Seams {
                engines: vec![Arc::new(opener)],
                ..host
            };
            let store: Arc<dyn LakeStore> =
                lakelands_store::open(Location::memory(lake), seams, OpenMode::Create).await?;
            Ok(store)
        })
    }

    fn reopener(&self) -> Reopen {
        let opener = self.opener.clone();
        Arc::new(move |store, host| {
            let opener = opener.clone();
            Box::pin(async move {
                let lake = store.lake().to_string();
                store.close().await?;
                let seams = Seams {
                    engines: vec![Arc::new(opener)],
                    ..host
                };
                let store: Arc<dyn LakeStore> =
                    lakelands_store::open(Location::memory(lake), seams, OpenMode::Write).await?;
                Ok(store)
            })
        })
    }

    fn scratch(&self) -> PathBuf {
        PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("memory-conformance-{}", std::process::id()))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_registered_check_passes_on_the_memory_engine() {
    assert!(!ALL.is_empty(), "no conformance check is registered");
    let report = run_all(&Memory {
        opener: MemoryOpener::new(),
    })
    .await;
    assert!(report.is_green(), "{report}");
    assert_eq!(report.passed.len(), ALL.len());
}
