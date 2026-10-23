//! Conformance: runs the Lakelands conformance suite against the `pg`
//! engine, through the store.
//!
//! The suite is the specification: every registered check in
//! `lakelands_core::conformance` runs here, each on a fresh lake in a
//! schema of its own on the server `LAKELANDS_TEST_PG_URL` names.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::seam::Seams;
use lakelands_api::{LakeStore, Location};
use lakelands_core::conformance::Check;
use lakelands_core::conformance::checks::ALL;
use lakelands_core::conformance::{
    Ahead, BuiltAhead, Continue, Continued, Fault, FreshLake, Harness, Import, ImportedLake, Lower,
    Meter, Relimit, Reopen, Twin, below, continue_below, run_checks, run_one,
};
use lakelands_engine_pg::PgOpener;
use peak_alloc::PeakAlloc;

struct Pg {
    url: String,
    large: u32,
}

impl Harness for Pg {
    fn name(&self) -> &str {
        "pg"
    }

    fn fresh(&self, host: Seams) -> FreshLake<'_> {
        // The lake is named after its schema, so it can be found again.
        let schema = common::fresh_schema();
        let location = common::location(&self.url, &schema, &schema);
        Box::pin(async move {
            let seams = Seams {
                engines: vec![Arc::new(PgOpener)],
                ..host
            };
            let store: Arc<dyn LakeStore> =
                lakelands_store::open(location, seams, OpenMode::Create).await?;
            Ok(store)
        })
    }

    fn reopener(&self) -> Reopen {
        let url = self.url.clone();
        Arc::new(move |store, host| {
            let url = url.clone();
            Box::pin(async move {
                let schema = store.lake().to_string();
                store.close().await?;
                drop(store);
                let seams = Seams {
                    engines: vec![Arc::new(PgOpener)],
                    ..host
                };
                let location = common::location(&url, &schema, &schema);
                let store: Arc<dyn LakeStore> =
                    lakelands_store::open(location, seams, OpenMode::Write).await?;
                Ok(store)
            })
        })
    }

    fn faulter(&self) -> Fault {
        let url = self.url.clone();
        Arc::new(move |store, host, wrap| {
            let url = url.clone();
            Box::pin(async move {
                let schema = store.lake().to_string();
                store.close().await?;
                drop(store);
                let seams = Seams {
                    engines: vec![wrap(Arc::new(PgOpener))],
                    ..host
                };
                let location = common::location(&url, &schema, &schema);
                let store: Arc<dyn LakeStore> =
                    lakelands_store::open(location, seams, OpenMode::Write).await?;
                Ok(store)
            })
        })
    }

    fn relimiter(&self) -> Relimit {
        let url = self.url.clone();
        Arc::new(move |store, host, limits| {
            let url = url.clone();
            Box::pin(async move {
                let schema = store.lake().to_string();
                store.close().await?;
                drop(store);
                let seams = Seams {
                    engines: vec![Arc::new(PgOpener)],
                    ..host
                };
                let location = Location {
                    limits,
                    ..common::location(&url, &schema, &schema)
                };
                let store: Arc<dyn LakeStore> =
                    lakelands_store::open(location, seams, OpenMode::Write).await?;
                Ok(store)
            })
        })
    }

    fn continuer(&self) -> Continue {
        let url = self.url.clone();
        Arc::new(move |held, lake, lines, mode, host| {
            let url = url.clone();
            Box::pin(async move {
                if let Some(store) = held {
                    store.close().await?;
                }
                let location = common::location(&url, lake.as_str(), lake.as_str());
                let seams = Seams {
                    engines: vec![Arc::new(PgOpener)],
                    ..host
                };
                // The engine is reached off the runtime's threads, as the
                // store reaches it.
                let (o, l) = (PgOpener, location.clone());
                let before = tokio::task::spawn_blocking(move || continue_below(&o, &l, lines));
                let before = before.await.expect("the engine's work ran")?;
                let opened = lakelands_store::open(location.clone(), seams, mode).await;
                let after = match &opened {
                    Ok(_) => None,
                    Err(_) => {
                        let o = PgOpener;
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
            .join(format!("pg-conformance-{}", std::process::id()))
    }

    fn twinner(&self) -> Twin {
        let url = self.url.clone();
        Arc::new(move |lake, host| {
            // In a schema of its own, apart from the lake's, which is named
            // after it: the lake is named by the location, the schema is
            // where the engine keeps it.
            let location = common::location(&url, lake.as_str(), &common::fresh_schema());
            Box::pin(async move {
                let seams = Seams {
                    engines: vec![Arc::new(PgOpener)],
                    ..host
                };
                lakelands_store::open(location, seams, OpenMode::Create).await
            })
        })
    }

    fn importer(&self) -> Import {
        let url = self.url.clone();
        Arc::new(move |from, lake, into, host| {
            let url = url.clone();
            Box::pin(async move {
                // Into a lake handed out already, in its schema; a
                // brand-new one in a schema of its own.
                let schema = match into {
                    Some(store) => {
                        store.close().await?;
                        lake.to_string()
                    }
                    None => common::fresh_schema(),
                };
                let location = common::location(&url, lake.as_str(), &schema);
                let seams = Seams {
                    engines: vec![Arc::new(PgOpener)],
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
                            engines: vec![Arc::new(PgOpener)],
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
        Some(common::location(&self.url, lake.as_str(), lake.as_str()))
    }

    fn aheader(&self) -> Ahead {
        Arc::new(|location, host| {
            Box::pin(async move {
                let seams = Seams {
                    engines: vec![Arc::new(PgOpener)],
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

    fn lowerer(&self) -> Lower {
        let url = self.url.clone();
        Arc::new(move |location| {
            let lakelands_api::location::EngineConfig::Postgres { schema, .. } = &location.engine
            else {
                return false;
            };
            let (url, schema) = (url.clone(), schema.clone().unwrap_or_default());
            // The synchronous client runs a runtime of its own, on a
            // thread of its own (`common::drop_schemas`).
            let lowered = std::thread::spawn(move || {
                let mut db = postgres::Client::connect(&url, postgres::NoTls)?;
                let lower = format!("UPDATE \"{schema}\".meta SET n = n - 1 WHERE k = 'layout'");
                db.execute(&lower, &[])
            });
            let lowered = lowered.join().expect("the marking thread");
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

/// The large lake's first size on every pull request, below the suite's
/// `LARGE_LAKE`: each ingest window waits on a PostgreSQL commit, and at
/// 2 000 records the check alone added over two minutes to this job.
/// 1 100 records are still more than one page of the feed at the first
/// size.
const SHORT_RUN: u32 = 1_100;

/// How many tests share the registered checks. Each runs in its own
/// process under nextest, with its own connections and its own schemas.
const SHARDS: usize = 4;

/// The checks of shard `k`: those at registry positions `i % SHARDS == k`.
fn shard(k: usize) -> impl Iterator<Item = &'static Check> {
    ALL.iter()
        .enumerate()
        .filter(move |(i, _)| i % SHARDS == k)
        .map(|(_, c)| c)
}

/// Runs shard `k` against the server. The shards are separate tests so
/// that the job takes the longest shard's time, not all of them in a row.
/// They stand on one process each (nextest): in one process they would
/// share the heap meter and each would drop the others' schemas.
async fn run_shard(k: usize) {
    let Some(url) = common::url_or_skip("the pg conformance suite") else {
        return;
    };
    let report = run_checks(
        &Pg {
            url: url.clone(),
            large: SHORT_RUN,
        },
        shard(k),
    )
    .await;
    common::drop_schemas(&url);
    assert!(report.is_green(), "{report}");
    assert_eq!(report.passed.len(), shard(k).count());
}

macro_rules! shards {
    ($($name:ident => $k:expr),* $(,)?) => {$(
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn $name() {
            run_shard($k).await;
        }
    )*};
}

shards! {
    every_registered_check_passes_on_the_pg_engine_shard_0 => 0,
    every_registered_check_passes_on_the_pg_engine_shard_1 => 1,
    every_registered_check_passes_on_the_pg_engine_shard_2 => 2,
    every_registered_check_passes_on_the_pg_engine_shard_3 => 3,
}

#[test]
fn every_registered_check_is_in_exactly_one_pg_shard() {
    assert!(!ALL.is_empty(), "no conformance check is registered");
    let mut seen: Vec<&str> = (0..SHARDS).flat_map(shard).map(|c| c.name).collect();
    seen.sort_unstable();
    let mut all: Vec<&str> = ALL.iter().map(|c| c.name).collect();
    all.sort_unstable();
    assert_eq!(
        seen, all,
        "the shards' union is not ALL, or a check is in two"
    );
}

/// The streaming check at 25 000 and 100 000 records, by hand:
/// `cargo test --release -p lakelands-engine-pg --test conformance -- --ignored --nocapture`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "the long run of the streaming check; run by hand with --ignored"]
async fn a_large_lake_streams_on_the_pg_engine_at_a_hundred_thousand_records() {
    let Some(url) = common::url_or_skip("the long streaming run") else {
        return;
    };
    let harness = Pg {
        url: url.clone(),
        large: LONG_RUN,
    };
    let outcome = run_one(&harness, STREAMING)
        .await
        .expect("the check is registered");
    common::drop_schemas(&url);
    assert!(outcome.is_ok(), "{}", outcome.unwrap_err());
}
