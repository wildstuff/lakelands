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
use lakelands_core::conformance::{FreshLake, Harness, Reopen, run_all};
use lakelands_engine_sqlite::SqliteOpener;

struct Sqlite {
    root: PathBuf,
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_registered_check_passes_on_the_sqlite_engine() {
    assert!(!ALL.is_empty(), "no conformance check is registered");
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("sqlite-conformance-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let report = run_all(&Sqlite { root: root.clone() }).await;
    assert!(report.is_green(), "{report}");
    assert_eq!(report.passed.len(), ALL.len());
    let _ = std::fs::remove_dir_all(&root);
}
