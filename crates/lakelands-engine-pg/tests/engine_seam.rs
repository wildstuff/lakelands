//! Engine seam: runs every registered engine-seam check against the `pg`
//! engine, each on a fresh lake in a schema of its own.

mod common;

use std::sync::Mutex;

use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode};
use lakelands_api::{LakeId, Location};
use lakelands_core::conformance::engine::{ENGINE_CHECKS, EngineHarness, run_engine_checks};
use lakelands_engine_pg::PgOpener;

struct Pg {
    url: String,
    /// Each lake's schema, so a reopen finds it.
    schemas: Mutex<std::collections::BTreeMap<String, String>>,
}

impl Pg {
    fn location(&self, lake: &str) -> Location {
        let schema = self
            .schemas
            .lock()
            .unwrap()
            .entry(lake.to_owned())
            .or_insert_with(common::fresh_schema)
            .clone();
        common::location(&self.url, lake, &schema)
    }
}

impl EngineHarness for Pg {
    fn name(&self) -> &str {
        "pg"
    }

    fn fresh(&self) -> lakelands_api::Result<Box<dyn LakeEngine>> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let lake = format!("engine-seam-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        PgOpener.open(&self.location(&lake), OpenMode::Create)
    }

    fn reopen(&self, lake: &LakeId, mode: OpenMode) -> lakelands_api::Result<Box<dyn LakeEngine>> {
        PgOpener.open(&self.location(lake.as_str()), mode)
    }

    fn mark_layout(&self, lake: &LakeId, layout: Option<u64>) -> bool {
        let schema = self.schemas.lock().unwrap()[lake.as_str()].clone();
        let (url, n) = (self.url.clone(), layout.map(|n| n as i64));
        // The synchronous client runs a runtime of its own, on a thread of
        // its own (`common::drop_schemas`).
        let marked = std::thread::spawn(move || {
            let mut db = postgres::Client::connect(&url, postgres::NoTls)?;
            db.batch_execute(&format!(
                "DROP TABLE IF EXISTS \"{schema}\".routes; \
                 CREATE TABLE \"{schema}\".routes (x INTEGER)"
            ))?;
            db.execute(
                &format!("DELETE FROM \"{schema}\".meta WHERE k = 'layout'"),
                &[],
            )?;
            if let Some(n) = n {
                let mark = format!("INSERT INTO \"{schema}\".meta (k, n) VALUES ('layout', $1)");
                db.execute(&mark, &[&n])?;
            }
            Ok::<_, postgres::Error>(())
        });
        let marked = marked.join().expect("the marking thread");
        marked.expect("marking the projection");
        true
    }

    fn fold_as_before(&self, lake: &LakeId) -> bool {
        let schema = self.schemas.lock().unwrap()[lake.as_str()].clone();
        let url = self.url.clone();
        let folded = std::thread::spawn(move || {
            let mut db = postgres::Client::connect(&url, postgres::NoTls)?;
            db.batch_execute(&format!(
                "UPDATE \"{schema}\".sort_index SET sort_key = '\\x00'::bytea; \
                 UPDATE \"{schema}\".meta SET n = n + 1 WHERE k = 'folded' OR k LIKE 'sorted.%'"
            ))
        });
        let folded = folded.join().expect("the folding thread");
        folded.expect("folding as a writer without the chosen fields");
        true
    }

    fn rebuild_as_before(&self, lake: &LakeId) -> bool {
        let schema = self.schemas.lock().unwrap()[lake.as_str()].clone();
        let url = self.url.clone();
        let rebuilt = std::thread::spawn(move || {
            let mut db = postgres::Client::connect(&url, postgres::NoTls)?;
            db.batch_execute(&format!("DELETE FROM \"{schema}\".sort_index"))
        });
        let rebuilt = rebuilt.join().expect("the rebuilding thread");
        rebuilt.expect("rebuilding as a writer without the chosen fields");
        true
    }

    fn stale_text_stats(&self, lake: &LakeId, behind: bool) -> bool {
        let schema = self.schemas.lock().unwrap()[lake.as_str()].clone();
        let url = self.url.clone();
        let stale = match behind {
            true => format!(
                "UPDATE \"{schema}\".meta SET n = n - 1 WHERE k = 'text_stats'; \
                 UPDATE \"{schema}\".text_stats SET records = records * 2, tokens = tokens * 2 + 1; \
                 DELETE FROM \"{schema}\".text_lengths"
            ),
            false => format!(
                "DROP TABLE \"{schema}\".text_stats; DROP TABLE \"{schema}\".text_lengths; \
                 DELETE FROM \"{schema}\".meta WHERE k = 'text_stats'"
            ),
        };
        let left = std::thread::spawn(move || {
            let mut db = postgres::Client::connect(&url, postgres::NoTls)?;
            db.batch_execute(&stale)
        });
        let left = left.join().expect("the staling thread");
        left.expect("leaving the text statistics stale");
        true
    }

    fn scratch(&self, lake: &LakeId) -> lakelands_api::Result<Box<dyn LakeEngine>> {
        PgOpener.open_scratch(&self.location(lake.as_str()))
    }

    fn forge_scratch_mark(&self, lake: &LakeId) -> bool {
        let schema = self.schemas.lock().unwrap()[lake.as_str()].clone();
        let (url, scratch) = (self.url.clone(), common::scratch_of(&schema));
        let forged = std::thread::spawn(move || {
            let mut db = postgres::Client::connect(&url, postgres::NoTls)?;
            db.batch_execute(&format!(
                "UPDATE \"{scratch}\".meta SET t = repeat('0', 64) WHERE k = 'built'"
            ))
        });
        forged
            .join()
            .expect("the forging thread")
            .expect("forging the mark");
        true
    }
}

#[test]
fn every_engine_check_passes_on_the_pg_engine() {
    let Some(url) = common::url_or_skip("the pg engine-seam checks") else {
        return;
    };
    assert!(!ENGINE_CHECKS.is_empty(), "no engine check is registered");
    let report = run_engine_checks(&Pg {
        url: url.clone(),
        schemas: Mutex::default(),
    });
    common::drop_schemas(&url);
    assert!(report.is_green(), "{report}");
    assert_eq!(report.passed.len(), ENGINE_CHECKS.len());
}
