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

    fn fold_as_before(&self, lake: &LakeId) -> bool {
        let path = self.root.join(lake.as_str()).join("projection.db");
        rusqlite::Connection::open(path)
            .and_then(|db| {
                db.execute_batch(
                    "UPDATE sort_index SET sort_key = x'00'; \
                     UPDATE meta SET v = v + 1 WHERE k = 'folded' OR k LIKE 'sorted.%';",
                )
            })
            .expect("folding as a writer without the chosen fields");
        true
    }

    fn stale_text_stats(&self, lake: &LakeId, behind: bool) -> bool {
        let path = self.root.join(lake.as_str()).join("projection.db");
        let stale = match behind {
            true => {
                "UPDATE meta SET v = v - 1 WHERE k = 'text_stats'; \
                 UPDATE text_stats SET records = records * 2, tokens = tokens * 2 + 1; \
                 DELETE FROM text_lengths;"
            }
            false => {
                "DROP TABLE text_stats; DROP TABLE text_lengths; \
                 DELETE FROM meta WHERE k = 'text_stats';"
            }
        };
        rusqlite::Connection::open(path)
            .and_then(|db| db.execute_batch(stale))
            .expect("leaving the text statistics stale");
        true
    }

    fn scratch(&self, lake: &LakeId) -> lakelands_api::Result<Box<dyn LakeEngine>> {
        SqliteOpener.open_scratch(&self.location(lake.as_str()))
    }

    fn forge_scratch_mark(&self, lake: &LakeId) -> bool {
        let path = self
            .root
            .join(lake.as_str())
            .join("projection.db.rebuild.mark");
        let text = std::fs::read_to_string(&path).expect("reading the scratch's mark");
        let mut mark: serde_json::Value = serde_json::from_str(&text).expect("a mark");
        mark["hash"] = serde_json::Value::String("0".repeat(64));
        std::fs::write(&path, mark.to_string()).expect("forging the mark");
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
