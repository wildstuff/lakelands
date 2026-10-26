//! A projection an older build laid out (lakelands#1151): its `routes`
//! naming kinds, types and sources in text, its states without the `procedure` a
//! computed field's metadata names (lakelands#1191), its states holding
//! their rows' type, key and position and their false flags, a rowid
//! beside every row of the tables whose primary key is the whole row, and
//! a search row for every record (lakelands#1318), and its mark one
//! layout back. A reader's open is refused naming the layouts; the
//! writer's open rebuilds it from the log under its lease, saying why,
//! and the next writer's open rebuilds nothing.

use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::error::Situation;
use lakelands_api::ontology::FieldDecl;
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
        FieldDecl {
            searchable: true,
            ..sourced("text", Text)
        },
        sourced("n", Decimal),
        computed_by("double", Decimal, "twice", &["n"]),
    ];
    l.declare(type_decl("note", fields)).await.unwrap();
    let a = source_record("note", "a", json!({"text": "kept", "n": "2"}));
    let b = source_record("note", "b", json!({"n": "3"}));
    l.ingest("crm", vec![a, b]).await.unwrap();
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
    for table in ROWID_FREE {
        let sql: String = (db().query_row(
            "SELECT sql FROM sqlite_master WHERE name = ?1",
            [table],
            |r| r.get(0),
        ))
        .unwrap();
        let with_rowid = (sql.replace("WITHOUT ROWID", "")).replacen(table, "older", 1);
        db().execute_batch(&format!(
            "{with_rowid}; INSERT INTO older SELECT * FROM {table}; DROP TABLE {table}; \
             ALTER TABLE older RENAME TO {table};"
        ))
        .unwrap();
    }
    db().execute_batch(
        "UPDATE records SET state = json_set(state, '$.type', ty, '$.key', key, \
         '$.position', since, '$.retired', json('false'), '$.restricted', json('false')); \
         INSERT OR IGNORE INTO search_rows (ty, key) SELECT ty, key FROM records;",
    )
    .unwrap();
    assert_eq!(rowid_free(), 0, "the older tables keep a rowid");
    db().execute_batch(&format!(
        "DROP TABLE routes; DROP TABLE names;
         CREATE TABLE routes (at INTEGER NOT NULL, kind TEXT NOT NULL, ty TEXT, key TEXT, source TEXT);
         CREATE INDEX routes_source ON routes (source, at);
         INSERT INTO routes VALUES (1, 'lake_created', NULL, NULL, 'crm');
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
        "SELECT count(*) FROM routes JOIN names ON names.id = routes.source \
         WHERE names.name = 'crm' AND typeof(routes.kind) = 'integer'",
        [],
        |r| r.get(0),
    ))
    .unwrap();
    assert!(
        sourced > 0 && layout() == wanted,
        "the rebuild indexed crm's events at layout {wanted}"
    );
    assert_eq!(rowid_free(), 3, "no table keeps a rowid beside its key");
    let count = |sql: &str| -> i64 { db().query_row(sql, [], |r| r.get(0)).unwrap() };
    let older = count(
        "SELECT count(*) FROM records WHERE json_type(state, '$.type') IS NOT NULL \
         OR json_type(state, '$.key') IS NOT NULL OR json_type(state, '$.position') IS NOT NULL \
         OR json_type(state, '$.retired') = 'false' OR json_type(state, '$.restricted') = 'false'",
    );
    assert_eq!(
        older, 0,
        "no state holds what its row holds, or a false flag"
    );
    let searched = count("SELECT count(*) FROM search_rows");
    assert_eq!(
        searched, 1,
        "only the record with tokens keeps a search row"
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

/// The tables whose primary key is the whole row.
const ROWID_FREE: [&str; 3] = ["applied", "text_index", "sort_index"];

/// How many of [`ROWID_FREE`] the projection lays out without a rowid.
fn rowid_free() -> i64 {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("older-layout-{}", std::process::id()));
    let db = rusqlite::Connection::open(dir.join("projection.db")).unwrap();
    let sql = "SELECT count(*) FROM sqlite_master WHERE name IN (?1, ?2, ?3) \
               AND sql LIKE '%WITHOUT ROWID%'";
    db.query_row(sql, ROWID_FREE, |r| r.get(0)).unwrap()
}
