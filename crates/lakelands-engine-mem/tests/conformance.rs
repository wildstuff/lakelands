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
use lakelands_core::conformance::{
    Continue, Continued, Fault, FreshLake, Harness, Import, ImportedLake, LARGE_LAKE, Meter,
    Relimit, Reopen, Twin, below, continue_below, run_all, run_one,
};
use lakelands_engine_mem::MemoryOpener;
use peak_alloc::PeakAlloc;

struct Memory {
    opener: MemoryOpener,
    large: u32,
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

    fn faulter(&self) -> Fault {
        let opener = self.opener.clone();
        Arc::new(move |store, host, wrap| {
            let opener = opener.clone();
            Box::pin(async move {
                let lake = store.lake().to_string();
                store.close().await?;
                let seams = Seams {
                    engines: vec![wrap(Arc::new(opener))],
                    ..host
                };
                let store: Arc<dyn LakeStore> =
                    lakelands_store::open(Location::memory(lake), seams, OpenMode::Write).await?;
                Ok(store)
            })
        })
    }

    fn relimiter(&self) -> Relimit {
        let opener = self.opener.clone();
        Arc::new(move |store, host, limits| {
            let opener = opener.clone();
            Box::pin(async move {
                let lake = store.lake().to_string();
                store.close().await?;
                let seams = Seams {
                    engines: vec![Arc::new(opener)],
                    ..host
                };
                let location = Location {
                    limits,
                    ..Location::memory(lake)
                };
                let store: Arc<dyn LakeStore> =
                    lakelands_store::open(location, seams, OpenMode::Write).await?;
                Ok(store)
            })
        })
    }

    fn continuer(&self) -> Continue {
        let opener = self.opener.clone();
        Arc::new(move |held, lake, lines, mode, host| {
            let opener = opener.clone();
            Box::pin(async move {
                if let Some(store) = held {
                    store.close().await?;
                }
                let location = Location::memory(lake.to_string());
                let seams = Seams {
                    engines: vec![Arc::new(opener.clone())],
                    ..host
                };
                // The engine is reached off the runtime's threads, as the
                // store reaches it.
                let (o, l) = (opener.clone(), location.clone());
                let before = tokio::task::spawn_blocking(move || continue_below(&o, &l, lines));
                let before = before.await.expect("the engine's work ran")?;
                let opened = lakelands_store::open(location.clone(), seams, mode).await;
                let after = match &opened {
                    Ok(_) => None,
                    Err(_) => {
                        let o = opener.clone();
                        let read = tokio::task::spawn_blocking(move || below(&o, &location));
                        Some(read.await.expect("the engine's work ran")?)
                    }
                };
                Ok(Continued {
                    opened,
                    before,
                    after,
                })
            })
        })
    }

    fn scratch(&self) -> PathBuf {
        PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("memory-conformance-{}", std::process::id()))
    }

    fn twinner(&self) -> Twin {
        Arc::new(|lake, host| {
            Box::pin(async move {
                // Under an opener of its own: the harness's holds the lake
                // that bears the id already.
                let seams = Seams {
                    engines: vec![Arc::new(MemoryOpener::new())],
                    ..host
                };
                let location = Location::memory(lake.to_string());
                lakelands_store::open(location, seams, OpenMode::Create).await
            })
        })
    }

    fn importer(&self) -> Import {
        let opener = self.opener.clone();
        Arc::new(move |dir, lake, into, host| {
            let opener = opener.clone();
            Box::pin(async move {
                // Into a lake handed out already, under its opener; a
                // brand-new one under an opener of its own, since an
                // imported lake has the name of the lake it came from.
                let target = match into {
                    Some(store) => {
                        store.close().await?;
                        opener
                    }
                    None => MemoryOpener::new(),
                };
                let location = Location::memory(lake.to_string());
                let seams = Seams {
                    engines: vec![Arc::new(target.clone())],
                    ..host
                };
                lakelands_store::import(location.clone(), seams.clone(), dir).await?;
                let store = lakelands_store::open(location.clone(), seams, OpenMode::Read).await?;
                let writer: Reopen = Arc::new(move |store, host| {
                    let (target, location) = (target.clone(), location.clone());
                    Box::pin(async move {
                        store.close().await?;
                        let seams = Seams {
                            engines: vec![Arc::new(target)],
                            ..host
                        };
                        lakelands_store::open(location, seams, OpenMode::Write).await
                    })
                });
                Ok(ImportedLake { store, writer })
            })
        })
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
async fn every_registered_check_passes_on_the_memory_engine() {
    assert!(!ALL.is_empty(), "no conformance check is registered");
    let report = run_all(&Memory {
        opener: MemoryOpener::new(),
        large: LARGE_LAKE,
    })
    .await;
    assert!(report.is_green(), "{report}");
    assert_eq!(report.passed.len(), ALL.len());
}

/// What every check asserts on the memory engine, one line per check
/// (`tally <evaluations> <sites> <check>`), for a change to the suite to
/// compare before and after, by hand:
/// `cargo test -p lakelands-engine-mem --test conformance -- --ignored --nocapture tally`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "prints the tallies; run by hand with --ignored"]
async fn the_tally_of_every_check_on_the_memory_engine() {
    let report = run_all(&Memory {
        opener: MemoryOpener::new(),
        large: LARGE_LAKE,
    })
    .await;
    for (check, tally) in &report.tallies {
        println!("tally {} {} {check}", tally.evaluations, tally.sites);
    }
    assert!(report.is_green(), "{report}");
}

/// The streaming check at 25 000 and 100 000 records, by hand:
/// `cargo test --release -p lakelands-engine-mem --test conformance -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "the long run of the streaming check; run by hand with --ignored"]
async fn a_large_lake_streams_on_the_memory_engine_at_a_hundred_thousand_records() {
    let harness = Memory {
        opener: MemoryOpener::new(),
        large: LONG_RUN,
    };
    let outcome = run_one(&harness, STREAMING)
        .await
        .expect("the check is registered");
    assert!(outcome.is_ok(), "{}", outcome.unwrap_err());
}
