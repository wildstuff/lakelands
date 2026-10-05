//! A fold that cannot begin leaves the writer behind (lakelands#603).
//!
//! Another connection holds the projection's write lock while a write is
//! appended: the log takes the event, the fold's `BEGIN IMMEDIATE` waits
//! out the busy timeout of ten seconds and is refused. The write is
//! answered that it is in the log and not folded; once the lock is gone
//! the next write folds it before it is decided, and every read answers
//! both.

use std::path::PathBuf;
use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::ontology::StorageKind::Number;
use lakelands_api::seam::Seams;
use lakelands_api::write::Outcome;
use lakelands_api::{Error, LakeStore, Location, Position};
use lakelands_core::conformance::fixture::{Lake, get_at, try_upsert, type_of};
use lakelands_engine_sqlite::SqliteOpener;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fold_whose_begin_is_busy_is_folded_before_the_next_write() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("busy-fold-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let seams = Seams {
        engines: vec![Arc::new(SqliteOpener)],
        ..Seams::default()
    };
    let store: Arc<dyn LakeStore> =
        lakelands_store::open(Location::sqlite("busy", &dir), seams, OpenMode::Create)
            .await
            .unwrap();
    let l = Lake::of(store.as_ref());
    l.declare(type_of("doc", &[("n", Number)])).await.unwrap();
    l.upsert("doc", "a", json!({"n": 1})).await.unwrap();

    let other = rusqlite::Connection::open(dir.join("projection.db")).unwrap();
    other.execute_batch("BEGIN IMMEDIATE").unwrap();
    let refused = try_upsert(l.cat.as_ref(), &l.own, "doc", "b", json!({"n": 2})).await;
    other.execute_batch("ROLLBACK").unwrap();
    let Err(Error::Unavailable { message }) = refused else {
        panic!("the write's fold cannot begin: {refused:?}");
    };

    let c = l.upsert("doc", "c", json!({"n": 3})).await.unwrap();
    let Outcome::Applied { position: c, .. } = c else {
        panic!("the next write takes effect: {c:?}");
    };
    let b = Position(c.0 - 1);
    let told = format!(
        "what was appended is in the log from position {} and is not folded yet",
        b.0
    );
    assert!(message.starts_with(&told), "{message}");
    for (key, n, at) in [("b", 2, None), ("b", 2, Some(b)), ("c", 3, None)] {
        let read = l.cat.get(&l.own, &"doc".into(), get_at(key, at)).await;
        let read = read.unwrap_or_else(|e| panic!("`{key}` at {at:?}: {e}"));
        assert_eq!(
            json!(read.data.fields),
            json!({"n": n}),
            "`{key}` at {at:?}"
        );
        assert_eq!(read.data.version.0, 1, "`{key}` at {at:?}");
    }
    let before = l.cat.get(&l.own, &"doc".into(), get_at("c", Some(b))).await;
    assert!(matches!(before, Err(Error::NotFound { .. })), "{before:?}");

    store.close().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
