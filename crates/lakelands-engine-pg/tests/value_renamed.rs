//! What renaming a value costs on PostgreSQL, beside the way a host had
//! before renames (both are in `measured/value_renamed.rs`). By hand, with
//! `LAKELANDS_TEST_PG_URL` set: `cargo test --release -p lakelands-engine-pg
//! --test value_renamed -- --ignored --nocapture`.

mod common;
#[path = "../../lakelands-store/tests/measured/value_renamed.rs"]
mod measured;

use std::sync::Arc;

use lakelands_api::LakeStore;
use lakelands_api::engine::OpenMode;
use lakelands_api::seam::Seams;
use lakelands_engine_pg::PgOpener;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "measures a rename over 100 000 records; run by hand with --release"]
async fn renaming_a_value_of_a_hundred_thousand_records() {
    let Some(url) = common::url_or_skip("the pg rename measurement") else {
        return;
    };
    for renames in [true, false] {
        let schema = common::fresh_schema();
        let seams = Seams {
            engines: vec![Arc::new(PgOpener)],
            ..Seams::default()
        };
        let location = common::location(&url, "renamed", &schema);
        let store: Arc<dyn LakeStore> = lakelands_store::open(location, seams, OpenMode::Create)
            .await
            .unwrap();
        match renames {
            true => measured::renamed("pg", store.as_ref()).await,
            false => measured::rewritten("pg", store.as_ref()).await,
        }
        store.close().await.unwrap();
        common::drop_schemas(&url);
    }
}
