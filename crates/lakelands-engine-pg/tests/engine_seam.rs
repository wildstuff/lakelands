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
