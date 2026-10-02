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
use lakelands_core::conformance::checks::ALL;
use lakelands_core::conformance::{
    FreshLake, Harness, Import, ImportedLake, Meter, Relimit, Reopen, run_all, run_one,
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

    fn relimiter(&self) -> Relimit {
        let url = self.url.clone();
        Arc::new(move |store, host, limits| {
            let url = url.clone();
            Box::pin(async move {
                let schema = store.lake().to_string();
                store.close().await?;
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

    fn scratch(&self) -> PathBuf {
        PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("pg-conformance-{}", std::process::id()))
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
                let writer: Reopen = Arc::new(move |store, host| {
                    let location = location.clone();
                    Box::pin(async move {
                        store.close().await?;
                        let seams = Seams {
                            engines: vec![Arc::new(PgOpener)],
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

/// The large lake's first size on every pull request, below the suite's
/// `LARGE_LAKE`: each ingest window waits on a PostgreSQL commit, and at
/// 2 000 records the check alone added over two minutes to this job.
/// 1 100 records are still more than one page of the feed at the first
/// size.
const SHORT_RUN: u32 = 1_100;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_registered_check_passes_on_the_pg_engine() {
    let Some(url) = common::url_or_skip("the pg conformance suite") else {
        return;
    };
    assert!(!ALL.is_empty(), "no conformance check is registered");
    let report = run_all(&Pg {
        url: url.clone(),
        large: SHORT_RUN,
    })
    .await;
    common::drop_schemas(&url);
    assert!(report.is_green(), "{report}");
    assert_eq!(report.passed.len(), ALL.len());
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
