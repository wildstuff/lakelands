//! What a migration costs on PostgreSQL when it decides the membership of
//! the records it migrates (the cells are in `measured/set_migrated.rs`).
//! By hand, with `LAKELANDS_TEST_PG_URL` set: `cargo test --release -p
//! lakelands-engine-pg --test set_migrated -- --ignored --nocapture`.

mod common;
#[path = "../../lakelands-store/tests/measured/set_migrated.rs"]
mod measured;

use std::sync::Arc;

use lakelands_api::LakeStore;
use lakelands_api::engine::OpenMode;
use lakelands_api::seam::Seams;
use lakelands_engine_pg::PgOpener;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "measures migrations over 100 000 records; run by hand with --release"]
async fn migrating_a_hundred_thousand_records_under_sets() {
    let Some(url) = common::url_or_skip("the pg set-migration measurement") else {
        return;
    };
    for c in measured::cells() {
        let schema = common::fresh_schema();
        let seams = Seams {
            engines: vec![Arc::new(PgOpener)],
            ..Seams::default()
        };
        let location = common::location(&url, "sets", &schema);
        let store: Arc<dyn LakeStore> = lakelands_store::open(location, seams, OpenMode::Create)
            .await
            .unwrap();
        measured::cell("pg", store.as_ref(), c).await;
        store.close().await.unwrap();
        common::drop_schemas(&url);
    }
}
