//! What a migration costs on SQLite when it decides the membership of the
//! records it migrates (the cells are in `measured/set_migrated.rs`). By
//! hand: `cargo test --release -p lakelands-engine-sqlite --test
//! set_migrated -- --ignored --nocapture`.

#[path = "../../lakelands-store/tests/measured/set_migrated.rs"]
mod measured;

use std::path::PathBuf;
use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::seam::Seams;
use lakelands_api::{LakeStore, Location};
use lakelands_engine_sqlite::SqliteOpener;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "measures migrations over 100 000 records; run by hand with --release"]
async fn migrating_a_hundred_thousand_records_under_sets() {
    for c in measured::cells() {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("set-migrated-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let seams = Seams {
            engines: vec![Arc::new(SqliteOpener)],
            ..Seams::default()
        };
        let location = Location::sqlite("sets", &dir);
        let store: Arc<dyn LakeStore> = lakelands_store::open(location, seams, OpenMode::Create)
            .await
            .unwrap();
        measured::cell("sqlite", store.as_ref(), c).await;
        store.close().await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
