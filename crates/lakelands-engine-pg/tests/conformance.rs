//! Conformance: runs the Lakelands conformance suite against the `pg`
//! engine, through the store.
//!
//! The suite is the specification: every registered check in
//! `lakelands_core::conformance` runs here, each on a fresh lake in a
//! schema of its own on the server `LAKELANDS_TEST_PG_URL` names.

mod common;

use std::sync::Arc;

use lakelands_api::LakeStore;
use lakelands_api::engine::OpenMode;
use lakelands_api::seam::Seams;
use lakelands_core::conformance::checks::ALL;
use lakelands_core::conformance::{FreshLake, Harness, run_all};
use lakelands_engine_pg::PgOpener;

struct Pg {
    url: String,
}

impl Harness for Pg {
    fn name(&self) -> &str {
        "pg"
    }

    fn fresh(&self, host: Seams) -> FreshLake<'_> {
        let schema = common::fresh_schema();
        let location = common::location(&self.url, "conformance", &schema);
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
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_registered_check_passes_on_the_pg_engine() {
    let Some(url) = common::url_or_skip("the pg conformance suite") else {
        return;
    };
    assert!(!ALL.is_empty(), "no conformance check is registered");
    let report = run_all(&Pg { url: url.clone() }).await;
    common::drop_schemas(&url);
    assert!(report.is_green(), "{report}");
    assert_eq!(report.passed.len(), ALL.len());
}
