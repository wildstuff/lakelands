//! What renaming a value costs on SQLite, beside the way a host had before
//! renames (both are in `measured/value_renamed.rs`). By hand: `cargo test
//! --release -p lakelands-engine-sqlite --test value_renamed -- --ignored
//! --nocapture`.

#[path = "../../lakelands-store/tests/measured/value_renamed.rs"]
mod measured;

use std::path::PathBuf;
use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::seam::Seams;
use lakelands_api::{LakeStore, Location};
use lakelands_engine_sqlite::SqliteOpener;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "measures a rename over 100 000 records; run by hand with --release"]
async fn renaming_a_value_of_a_hundred_thousand_records() {
    for renames in [true, false] {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("value-renamed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let seams = Seams {
            engines: vec![Arc::new(SqliteOpener)],
            ..Seams::default()
        };
        let location = Location::sqlite("renamed", &dir);
        let store: Arc<dyn LakeStore> = lakelands_store::open(location, seams, OpenMode::Create)
            .await
            .unwrap();
        match renames {
            true => measured::renamed("sqlite", store.as_ref()).await,
            false => measured::rewritten("sqlite", store.as_ref()).await,
        }
        store.close().await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
