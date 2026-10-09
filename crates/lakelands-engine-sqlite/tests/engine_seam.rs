//! Engine seam: runs every registered engine-seam check against the
//! `sqlite` engine, each on a fresh lake directory under the target's
//! scratch directory.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode};
use lakelands_api::{LakeId, Location};
use lakelands_core::conformance::engine::{ENGINE_CHECKS, EngineHarness, run_engine_checks};
use lakelands_engine_sqlite::SqliteOpener;

struct Sqlite {
    root: PathBuf,
}

impl Sqlite {
    fn location(&self, lake: &str) -> Location {
        Location::sqlite(lake, self.root.join(lake))
    }
}

impl EngineHarness for Sqlite {
    fn name(&self) -> &str {
        "sqlite"
    }

    fn fresh(&self) -> lakelands_api::Result<Box<dyn LakeEngine>> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let lake = format!("engine-seam-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        SqliteOpener.open(&self.location(&lake), OpenMode::Create)
    }

    fn reopen(&self, lake: &LakeId, mode: OpenMode) -> lakelands_api::Result<Box<dyn LakeEngine>> {
        SqliteOpener.open(&self.location(lake.as_str()), mode)
    }

    fn mark_layout(&self, lake: &LakeId, layout: Option<u64>) -> bool {
        let path = self.root.join(lake.as_str()).join("projection.db");
        let marked = rusqlite::Connection::open(path).and_then(|db| {
            db.execute_batch("DROP TABLE IF EXISTS routes; CREATE TABLE routes (x INTEGER);")?;
            match layout {
                Some(n) => db.execute(
                    "INSERT OR REPLACE INTO meta (k, v) VALUES ('layout', ?1)",
                    [n as i64],
                ),
                None => db.execute("DELETE FROM meta WHERE k = 'layout'", []),
            }
        });
        marked.expect("marking the projection");
        true
    }
}

#[test]
fn every_engine_check_passes_on_the_sqlite_engine() {
    assert!(!ENGINE_CHECKS.is_empty(), "no engine check is registered");
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("sqlite-engine-seam-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let report = run_engine_checks(&Sqlite { root: root.clone() });
    assert!(report.is_green(), "{report}");
    assert_eq!(report.passed.len(), ENGINE_CHECKS.len());
    let _ = std::fs::remove_dir_all(&root);
}
