//! A record keeps a search row only while it has tokens (lakelands#1318):
//! a fold that clears its searchable field deletes its row from
//! `search_rows` and its document from `full_text`, and a record that
//! still has tokens keeps both.

use std::path::PathBuf;

use lakelands_api::Location;
use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::id::{Position, Version};
use lakelands_api::record::StoredRecord;
use lakelands_api::write::Declaration;
use lakelands_core::conformance::fixture::stored_record;
use lakelands_engine_sqlite::SqliteOpener;
use serde_json::json;

#[test]
fn a_record_whose_tokens_go_loses_its_search_row_and_its_document() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("search-row-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let engine = SqliteOpener
        .open(&Location::sqlite("searched", &dir), OpenMode::Create)
        .unwrap();
    let decl: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "note",
        "fields": [{"name": "text", "type": {"kind": "text"}, "searchable": true}]
    }))
    .unwrap();
    let state = |key: &str, text: Option<&str>, at: u64| StoredRecord {
        version: Version(at),
        made: Position(1),
        ..stored_record(
            "note",
            key,
            serde_json::from_value(text.map_or(json!({}), |t| json!({"text": t}))).unwrap(),
            at,
        )
    };
    let p = engine.projection();
    p.begin().unwrap();
    p.put_declaration(&decl, 1, None, Position(0)).unwrap();
    p.put_record(&state("a", Some("river otters"), 1)).unwrap();
    p.put_record(&state("b", Some("lake trout"), 1)).unwrap();
    p.commit(Position(1)).unwrap();
    let db = rusqlite::Connection::open(dir.join("projection.db")).unwrap();
    let rows = || -> (i64, i64) {
        let count = |sql: &str| db.query_row(sql, [], |r| r.get(0)).unwrap();
        (
            count("SELECT count(*) FROM search_rows"),
            count("SELECT count(*) FROM full_text"),
        )
    };
    assert_eq!(
        rows(),
        (2, 2),
        "each record with tokens has a row and a document"
    );
    p.begin().unwrap();
    p.put_record(&state("a", None, 2)).unwrap();
    p.commit(Position(2)).unwrap();
    let a: i64 = (db.query_row(
        "SELECT count(*) FROM search_rows WHERE ty = 'note' AND key = 'a'",
        [],
        |r| r.get(0),
    ))
    .unwrap();
    assert!(
        a == 0 && rows() == (1, 1),
        "the record whose tokens went keeps no search row ({a}) and no document; \
         rows and documents left: {:?}",
        rows()
    );
    drop(engine);
    let _ = std::fs::remove_dir_all(&dir);
}
