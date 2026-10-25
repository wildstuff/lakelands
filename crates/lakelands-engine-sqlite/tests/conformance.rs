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
    Ahead, BuiltAhead, Continue, Continued, Fault, FreshLake, Harness, Import, ImportedLake,
    LARGE_LAKE, Lower, Meter, Relimit, Reopen, Standby, Twin, below, continue_below, run_all,
    run_one,
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
                // No handle of the closed writer is left to hold the
                // projection from a swap at the open.
                drop(store);
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

    fn faulter(&self) -> Fault {
        let root = self.root.clone();
        Arc::new(move |store, host, wrap| {
            let root = root.clone();
            Box::pin(async move {
                let lake = store.lake().to_string();
                store.close().await?;
                drop(store);
                let seams = Seams {
                    engines: vec![wrap(Arc::new(SqliteOpener))],
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

    fn relimiter(&self) -> Relimit {
        let root = self.root.clone();
        Arc::new(move |store, host, limits| {
            let root = root.clone();
            Box::pin(async move {
                let lake = store.lake().to_string();
                store.close().await?;
                drop(store);
                let seams = Seams {
                    engines: vec![Arc::new(SqliteOpener)],
                    ..host
                };
                let dir = root.join(&lake);
                let location = Location {
                    limits,
                    ..Location::sqlite(lake, dir)
                };
                let store: Arc<dyn LakeStore> =
                    lakelands_store::open(location, seams, OpenMode::Write).await?;
                Ok(store)
            })
        })
    }

    fn continuer(&self) -> Continue {
        let root = self.root.clone();
        Arc::new(move |held, lake, lines, mode, host| {
            let root = root.clone();
            Box::pin(async move {
                if let Some(store) = held {
                    store.close().await?;
                }
                let location = Location::sqlite(lake.to_string(), root.join(lake.as_str()));
                let seams = Seams {
                    engines: vec![Arc::new(SqliteOpener)],
                    ..host
                };
                // The engine is reached off the runtime's threads, as the
                // store reaches it.
                let (o, l) = (SqliteOpener, location.clone());
                let before = tokio::task::spawn_blocking(move || continue_below(&o, &l, lines));
                let before = before.await.expect("the engine's work ran")?;
                let opened = lakelands_store::open(location.clone(), seams, mode).await;
                let after = match &opened {
                    Ok(_) => None,
                    Err(_) => {
                        let o = SqliteOpener;
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
        self.root.join("scratch")
    }

    fn twinner(&self) -> Twin {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = self.root.clone();
        Arc::new(move |lake, host| {
            // In a directory of its own: the lake's is the harness's.
            let dir = root.join(format!("twin-{}", NEXT.fetch_add(1, Ordering::Relaxed)));
            Box::pin(async move {
                let seams = Seams {
                    engines: vec![Arc::new(SqliteOpener)],
                    ..host
                };
                let location = Location::sqlite(lake.to_string(), dir);
                lakelands_store::open(location, seams, OpenMode::Create).await
            })
        })
    }

    fn importer(&self) -> Import {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = self.root.clone();
        Arc::new(move |from, lake, into, host| {
            let root = root.clone();
            Box::pin(async move {
                // Into a lake handed out already, in its directory; a
                // brand-new one in a directory of its own.
                let dir = match into {
                    Some(store) => {
                        store.close().await?;
                        root.join(lake.as_str())
                    }
                    None => root.join(format!("imported-{}", NEXT.fetch_add(1, Ordering::Relaxed))),
                };
                let location = Location::sqlite(lake.to_string(), dir);
                let seams = Seams {
                    engines: vec![Arc::new(SqliteOpener)],
                    ..host
                };
                lakelands_store::import(location.clone(), seams.clone(), from).await?;
                let store = lakelands_store::open(location.clone(), seams, OpenMode::Read).await?;
                let at = location.clone();
                let writer: Reopen = Arc::new(move |store, host| {
                    let location = location.clone();
                    Box::pin(async move {
                        store.close().await?;
                        drop(store);
                        let seams = Seams {
                            engines: vec![Arc::new(SqliteOpener)],
                            ..host
                        };
                        lakelands_store::open(location, seams, OpenMode::Write).await
                    })
                });
                Ok(ImportedLake {
                    store,
                    writer,
                    location: at,
                })
            })
        })
    }

    fn location(&self, lake: &lakelands_api::LakeId) -> Option<Location> {
        Some(Location::sqlite(
            lake.as_str(),
            self.root.join(lake.as_str()),
        ))
    }

    fn aheader(&self) -> Ahead {
        Arc::new(|location, host| {
            Box::pin(async move {
                let seams = Seams {
                    engines: vec![Arc::new(SqliteOpener)],
                    ..host
                };
                let built = lakelands_store::build_ahead(location, seams).await?;
                Ok(BuiltAhead {
                    ahead: built.ahead,
                    through: built.through,
                    events: built.events,
                })
            })
        })
    }

    fn standbyer(&self) -> Standby {
        let root = self.root.clone();
        Arc::new(move |lake, host, every| {
            let dir = root.join(lake.as_str());
            Box::pin(async move {
                let seams = Seams {
                    engines: vec![Arc::new(SqliteOpener)],
                    ..host
                };
                let location = Location::sqlite(lake.as_str(), dir);
                lakelands_store::standby(location, seams, every).await
            })
        })
    }

    fn lowerer(&self) -> Lower {
        Arc::new(|location| {
            let lakelands_api::location::EngineConfig::Sqlite { dir, .. } = &location.engine else {
                return false;
            };
            let db = rusqlite::Connection::open(dir.join("projection.db"));
            let lowered =
                db.and_then(|db| db.execute("UPDATE meta SET v = v - 1 WHERE k = 'layout'", []));
            lowered.expect("marking the projection one layout back") == 1
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
