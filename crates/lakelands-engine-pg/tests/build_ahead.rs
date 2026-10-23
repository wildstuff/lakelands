//! A projection built ahead on the PostgreSQL engine (lakelands#1322),
//! of a lake in this release's layout, the one 0.17.0 laid out, while its
//! writer holds it: the build reads the log and folds into its own scratch
//! schema only, so it finishes while another session holds every
//! projection table and the lease locked, and the writer writes on; the
//! writer's open of the layout before goes on from it. The room it takes is
//! measured before, and answered while the tables are held.

mod common;

use std::sync::{Arc, mpsc};
use std::time::Duration;

use lakelands_api::engine::OpenMode;
use lakelands_api::seam::Seams;
use lakelands_api::{LakeStore, Location};
use lakelands_core::conformance::fixture::{Lake, get_at, source_record, sourced, type_decl};
use lakelands_engine_pg::PgOpener;
use serde_json::json;

fn seams() -> Seams {
    Seams {
        engines: vec![Arc::new(PgOpener)],
        ..Seams::default()
    }
}

async fn open(location: &Location, mode: OpenMode) -> lakelands_api::Result<Arc<dyn LakeStore>> {
    lakelands_store::open(location.clone(), seams(), mode).await
}

/// Hold every table of `schema` but the log's and its meta locked, the
/// lease's too, in one session, until the sender returned is dropped.
fn hold(url: &str, schema: &str) -> mpsc::Sender<()> {
    let (url, schema) = (url.to_owned(), schema.to_owned());
    let (locked, wait) = mpsc::channel();
    let (release, done) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let mut db = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        let mut tx = db.transaction().unwrap();
        let tables = tx
            .query(
                "SELECT tablename FROM pg_tables WHERE schemaname = $1 \
                 AND tablename NOT IN ('log', 'meta', 'segments')",
                &[&schema],
            )
            .unwrap();
        for t in tables {
            let t: String = t.get(0);
            let lock = format!("LOCK TABLE \"{schema}\".\"{t}\" IN ACCESS EXCLUSIVE MODE");
            tx.batch_execute(&lock).unwrap();
        }
        locked.send(()).unwrap();
        let _ = done.recv();
        tx.rollback().unwrap();
    });
    wait.recv().unwrap();
    release
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lake_is_built_ahead_beside_its_writer_without_its_projection_or_lease() {
    let Some(url) = common::url_or_skip("build ahead") else {
        return;
    };
    let schema = common::fresh_schema();
    let location = common::location(&url, "ahead", &schema);
    let store = open(&location, OpenMode::Create).await.unwrap();
    let l = Lake::of(store.as_ref());
    l.declare(type_decl(
        "note",
        vec![sourced("text", lakelands_api::ontology::StorageKind::Text)],
    ))
    .await
    .unwrap();
    let note = |k: &str| source_record("note", k, json!({"text": k}));
    l.ingest("crm", vec![note("a"), note("b")]).await.unwrap();
    let head = store.head().await.unwrap();
    let room = lakelands_store::room_ahead(location.clone(), seams()).await;
    assert!(room.unwrap() > 0, "the projection's tables take room");
    let release = tokio::task::block_in_place(|| hold(&url, &schema));
    // Measured while every table is held, it answers what the catalogue
    // recorded, and waits on no one past its second.
    let held = lakelands_store::room_ahead(location.clone(), seams());
    let held = tokio::time::timeout(Duration::from_secs(10), held).await;
    held.expect("the measure waits on no projection table")
        .unwrap();
    let built = lakelands_store::build_ahead(location.clone(), seams());
    let built = tokio::time::timeout(Duration::from_secs(60), built).await;
    drop(release);
    let built = built
        .expect("the build waits on no projection table")
        .unwrap();
    assert_eq!((built.ahead, built.through), (None, Some(head)));
    l.ingest("crm", vec![note("c")]).await.unwrap();
    store.close().await.unwrap();
    drop((l, store));
    let lower = format!("UPDATE \"{schema}\".meta SET n = n - 1 WHERE k = 'layout'");
    tokio::task::block_in_place(|| {
        let mut db = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        db.batch_execute(&lower).unwrap();
    });
    let store = open(&location, OpenMode::Write).await.unwrap();
    let why = store.rebuilt_at_open().unwrap_or_default();
    assert!(why.contains("build ahead"), "{why}");
    let l = Lake::of(store.as_ref());
    for k in ["a", "b", "c"] {
        let read = l.cat.get(&l.own, &"note".into(), get_at(k, None)).await;
        assert!(read.is_ok(), "{k}: {read:?}");
    }
    store.close().await.unwrap();
    drop((l, store));
    common::drop_schema(&url, &schema);
}
