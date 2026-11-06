//! A rebuild while another handle reads the projection, on the SQLite
//! engine (lakelands#1322), whose swap waits for no reader but refuses
//! one: the store asks first, and folds in place without folding the log
//! beside the projection too. A rebuild leaves a build ahead as it was; an
//! upgrade's open folds the whole log in place, removes the build ahead it
//! could not swap in, and says so.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fs4::FileExt;
use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::ontology::StorageKind::Text;
use lakelands_api::seam::Seams;
use lakelands_api::{LakeStore, Location};
use lakelands_core::conformance::fixture::{Lake, source_record, sourced, type_decl};
use lakelands_engine_sqlite::SqliteOpener;
use serde_json::json;

fn seams() -> Seams {
    Seams {
        engines: vec![Arc::new(SqliteOpener)],
        ..Seams::default()
    }
}

async fn open(location: &Location, mode: OpenMode) -> Arc<dyn LakeStore> {
    lakelands_store::open(location.clone(), seams(), mode)
        .await
        .unwrap()
}

/// A lake of two items, written and built ahead beside its writer, which
/// writes one more after the build.
async fn built_ahead(name: &str) -> (PathBuf, Location, Arc<dyn LakeStore>) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "rebuild-with-a-reader-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let location = Location::sqlite("reader", &dir);
    let store = open(&location, OpenMode::Create).await;
    let l = Lake::of(store.as_ref());
    let item = |k: &str| source_record("item", k, json!({"label": k}));
    l.declare(type_decl("item", vec![sourced("label", Text)]))
        .await
        .unwrap();
    l.ingest("web", vec![item("i1"), item("i2")]).await.unwrap();
    let built = lakelands_store::build_ahead(location.clone(), seams()).await;
    assert!(
        built.unwrap().through.is_some(),
        "the build ahead is marked"
    );
    l.ingest("web", vec![item("i3")]).await.unwrap();
    (dir, location, store)
}

/// The scratch built ahead: its length and when it was last written.
fn scratch(dir: &Path) -> Option<(u64, std::time::SystemTime)> {
    let m = std::fs::metadata(dir.join("projection.db.rebuild")).ok()?;
    Some((m.len(), m.modified().unwrap()))
}

async fn labels(store: &dyn LakeStore) -> Vec<String> {
    let l = Lake::of(store);
    let mut labels = Vec::new();
    for k in ["i1", "i2", "i3"] {
        let r = l.read("item", k).await.unwrap();
        labels.push(r.fields["label"].as_str().unwrap().to_owned());
    }
    labels
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rebuild_with_a_reader_open_folds_in_place_and_never_beside() {
    let (dir, location, store) = built_ahead("rebuild").await;
    let before = scratch(&dir).expect("a scratch built ahead");
    let reader = SqliteOpener.open(&location, OpenMode::Read).unwrap();
    store.rebuild().await.unwrap();
    assert_eq!(
        scratch(&dir),
        Some(before),
        "with a reader open, nothing is folded beside the projection: the scratch is as it was"
    );
    assert_eq!(labels(store.as_ref()).await, ["i1", "i2", "i3"]);
    drop(reader);
    store.rebuild().await.unwrap();
    assert_eq!(
        scratch(&dir),
        None,
        "with no reader, the rebuild folds beside the projection and swaps the scratch in"
    );
    assert_eq!(labels(store.as_ref()).await, ["i1", "i2", "i3"]);
    store.close().await.unwrap();
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_upgrade_with_a_reader_open_says_it_removed_the_build_ahead() {
    let (dir, location, store) = built_ahead("upgrade").await;
    store.close().await.unwrap();
    drop(store);
    let db = rusqlite::Connection::open(dir.join("projection.db")).unwrap();
    let lowered = db.execute("UPDATE meta SET v = v - 1 WHERE k = 'layout'", []);
    assert_eq!(lowered.unwrap(), 1, "the projection is one layout back");
    drop(db);
    // A reader of this release is refused the older layout: one that
    // holds the projection is a release that reads it, holding it as every
    // handle of the engine does.
    let reader = std::fs::File::open(dir.join("lake")).unwrap();
    FileExt::lock_shared(&reader).unwrap();
    let store = open(&location, OpenMode::Write).await;
    let why = store.rebuilt_at_open().unwrap_or_default();
    assert!(
        why.contains("another handle read the projection, so the open folded the whole log in place, and removed the build ahead it could not swap in"),
        "the open says the build ahead was spent: {why:?}"
    );
    assert_eq!(scratch(&dir), None, "the build ahead is removed");
    assert_eq!(labels(store.as_ref()).await, ["i1", "i2", "i3"]);
    drop(reader);
    store.close().await.unwrap();
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}
