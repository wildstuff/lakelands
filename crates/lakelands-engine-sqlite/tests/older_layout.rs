//! A projection an older build laid out (lakelands#1151): its `routes`
//! without the `source` column, its states without the `procedure` a
//! computed field's metadata names (lakelands#1191), and its mark one
//! layout back. A reader's open is refused naming the layouts; the
//! writer's open rebuilds it from the log under its lease, saying why,
//! and the next writer's open rebuilds nothing.

use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::error::Situation;
use lakelands_api::ontology::StorageKind::{Decimal, Text};
use lakelands_api::seam::Seams;
use lakelands_api::{LakeStore, Location};
use lakelands_core::conformance::fixture::{Lake, computed_by, source_record, sourced, type_decl};
use lakelands_engine_sqlite::SqliteOpener;
use serde_json::json;

async fn open(location: &Location, mode: OpenMode) -> lakelands_api::Result<Arc<dyn LakeStore>> {
    let seams = Seams {
        engines: vec![Arc::new(SqliteOpener)],
        ..Seams::default()
    };
    lakelands_store::open(location.clone(), seams, mode).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_writers_open_rebuilds_a_projection_an_older_build_laid_out() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("older-layout-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let location = Location::sqlite("older", &dir);
    let store = open(&location, OpenMode::Create).await.unwrap();
    let l = Lake::of(store.as_ref());
    let twice = json!({"declare": "function", "name": "twice", "backing": "expr",
        "inputs": [{"name": "n", "type": {"kind": "decimal"}}], "expr": "n * 2"});
    let twice = serde_json::from_value(twice).unwrap();
    l.declare_as(twice, "declaring `twice`").await.unwrap();
    let fields = vec![
        sourced("text", Text),
        sourced("n", Decimal),
        computed_by("double", Decimal, "twice", &["n"]),
    ];
    l.declare(type_decl("note", fields)).await.unwrap();
    let a = source_record("note", "a", json!({"text": "kept", "n": "2"}));
    l.ingest("crm", vec![a]).await.unwrap();
    store.close().await.unwrap();
    // The upgrade's swap waits for every handle onto the projection (071).
    drop((l, store));
    let db = || rusqlite::Connection::open(dir.join("projection.db")).unwrap();
    let layout = || -> i64 {
        db().query_row("SELECT v FROM meta WHERE k = 'layout'", [], |r| r.get(0))
            .unwrap()
    };
    let wanted = layout();
    let unnamed = db().execute(
        "UPDATE records SET state = replace(state, '\"procedure\":\"twice\",', '')",
        [],
    );
    assert!(
        unnamed.unwrap() > 0,
        "a state names `twice` as its procedure"
    );
    db().execute_batch(&format!(
        "DROP INDEX routes_source; ALTER TABLE routes DROP COLUMN source;
         UPDATE meta SET v = {} WHERE k = 'layout';",
        wanted - 1
    ))
    .unwrap();
    let refused = open(&location, OpenMode::Read).await.err().unwrap();
    assert!(
        matches!(refused.situation(), Some(Situation::ProjectionLayout { found, .. }) if *found == Some(wanted as u64 - 1)),
        "a reader's open of the older layout is refused: {refused:?}"
    );
    let store = open(&location, OpenMode::Write).await.unwrap();
    let why = store.rebuilt_at_open();
    assert!(
        why.as_ref()
            .is_some_and(|w| w.contains(&format!("layout {}", wanted - 1))),
        "the writer's open says why it rebuilt: {why:?}"
    );
    let note = Lake::of(store.as_ref()).read("note", "a").await.unwrap();
    assert_eq!(note.fields["text"], "kept");
    let procedure = note.meta["double"].procedure.as_deref();
    assert_eq!(procedure, Some("twice"), "the rebuild names the function");
    let sourced: i64 = (db().query_row(
        "SELECT count(*) FROM routes WHERE source = 'crm'",
        [],
        |r| r.get(0),
    ))
    .unwrap();
    assert!(
        sourced > 0 && layout() == wanted,
        "the rebuild indexed crm's events at layout {wanted}"
    );
    store.close().await.unwrap();
    let store = open(&location, OpenMode::Write).await.unwrap();
    assert_eq!(
        store.rebuilt_at_open(),
        None,
        "the next writer's open rebuilds nothing"
    );
    store.close().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
