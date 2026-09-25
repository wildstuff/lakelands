//! Conformance: runs the Lakelands conformance suite against the `sqlite`
//! engine, through the store.
//!
//! The suite is the specification: every registered check in
//! `lakelands_core::conformance` runs here, each on a fresh lake in a
//! directory of its own under the target's scratch directory.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use lakelands_api::engine::OpenMode;
use lakelands_api::seam::Seams;
use lakelands_api::{LakeStore, Location};
use lakelands_core::conformance::checks::ALL;
use lakelands_core::conformance::{
    FreshLake, Harness, LARGE_LAKE, Meter, Reopen, run_all, run_one,
};
use lakelands_engine_sqlite::SqliteOpener;
use peak_alloc::PeakAlloc;

struct Sqlite {
    root: PathBuf,
    large: u32,
}

impl Harness for Sqlite {
    fn name(&self) -> &str {
        "sqlite"
    }

    fn fresh(&self, host: Seams) -> FreshLake<'_> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let lake = format!("conformance-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let dir = self.root.join(&lake);
        Box::pin(async move {
            let seams = Seams {
                engines: vec![Arc::new(SqliteOpener)],
                ..host
            };
            let store: Arc<dyn LakeStore> =
                lakelands_store::open(Location::sqlite(lake, dir), seams, OpenMode::Create).await?;
            Ok(store)
        })
    }

    fn reopener(&self) -> Reopen {
        let root = self.root.clone();
        Arc::new(move |store, host| {
            let root = root.clone();
            Box::pin(async move {
                let lake = store.lake().to_string();
                store.close().await?;
                let seams = Seams {
                    engines: vec![Arc::new(SqliteOpener)],
                    ..host
                };
                let dir = root.join(&lake);
                let store: Arc<dyn LakeStore> =
                    lakelands_store::open(Location::sqlite(lake, dir), seams, OpenMode::Write)
                        .await?;
                Ok(store)
            })
        })
    }

    fn scratch(&self) -> PathBuf {
        self.root.join("scratch")
    }

    fn meter(&self) -> Option<Arc<dyn Meter>> {
        Some(Arc::new(Counted))
    }

    fn large_lake(&self) -> u32 {
        self.large
    }
}

// The heap meter of the suite's streaming check. nextest runs each test
// in a process of its own, so the count is this test's alone.
#[global_allocator]
static HEAP: PeakAlloc = PeakAlloc;

struct Counted;

impl Meter for Counted {
    fn live(&self) -> u64 {
        HEAP.current_usage() as u64
    }

    fn peak(&self) -> u64 {
        HEAP.peak_usage() as u64
    }

    fn reset_peak(&self) {
        HEAP.reset_peak_usage();
    }
}

/// The streaming check's name, for the long run.
const STREAMING: &str =
    "a_large_lake_goes_through_ingest_the_feed_and_export_in_memory_that_does_not_grow_with_it";

/// The records of the large lake in the long run: it grows to 100 000.
const LONG_RUN: u32 = 25_000;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_registered_check_passes_on_the_sqlite_engine() {
    assert!(!ALL.is_empty(), "no conformance check is registered");
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("sqlite-conformance-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let report = run_all(&Sqlite {
        root: root.clone(),
        large: LARGE_LAKE,
    })
    .await;
    assert!(report.is_green(), "{report}");
    assert_eq!(report.passed.len(), ALL.len());
    let _ = std::fs::remove_dir_all(&root);
}

/// The streaming check at 25 000 and 100 000 records, by hand:
/// `cargo test --release -p lakelands-engine-sqlite --test conformance -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "the long run of the streaming check; run by hand with --ignored"]
async fn a_large_lake_streams_on_the_sqlite_engine_at_a_hundred_thousand_records() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("sqlite-streaming-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let harness = Sqlite {
        root: root.clone(),
        large: LONG_RUN,
    };
    let outcome = run_one(&harness, STREAMING)
        .await
        .expect("the check is registered");
    let _ = std::fs::remove_dir_all(&root);
    assert!(outcome.is_ok(), "{}", outcome.unwrap_err());
}
