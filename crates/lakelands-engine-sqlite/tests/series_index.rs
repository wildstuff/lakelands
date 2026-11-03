//! A `series_index.<type>` setting keeps an order index within each series
//! (`docs/decisions/097-a-read-in-a-window-walks-its-window.md` §The
//! series index). Its rows sit in `sort_index` under a derived field, the
//! series first hashed and then the order value, and the pair's state sits
//! in `series.<type>`: made whole at a setting folded from the log, built
//! from `Start` beside the writer, put back to `Start` by a redeclaration,
//! and never touched by a `sort_index` rebuild.

use std::path::PathBuf;

use lakelands_api::Location;
use lakelands_api::engine::{Choose, EngineOpener, OpenMode};
use lakelands_api::id::{FieldName, Position, TypeName, Version};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::seam;
use lakelands_api::write::Declaration;
use lakelands_core::conformance::fixture::stored_record;
use lakelands_engine_sqlite::SqliteOpener;
use serde_json::json;

fn declaration() -> Declaration {
    serde_json::from_value(json!({
        "declare": "type", "name": "book",
        "fields": [
            {"name": "series", "type": {"kind": "text"}},
            {"name": "at", "type": {"kind": "number"}},
            {"name": "title", "type": {"kind": "text"}}
        ]
    }))
    .unwrap()
}

/// Four books: two in one series and one in another, and one of the first
/// series with no order value.
fn records() -> Vec<StoredRecord> {
    [
        ("k1", json!({"series": "red", "at": 2, "title": "a"})),
        ("k2", json!({"series": "red", "at": 1, "title": "b"})),
        ("k3", json!({"series": "blue", "at": 3, "title": "c"})),
        ("k4", json!({"series": "red", "title": "d"})),
    ]
    .into_iter()
    .map(|(key, fields)| stored_record("book", key, serde_json::from_value(fields).unwrap(), 1))
    .collect()
}

/// The state of a setting `key` of `sys.setting` holding `value`, at `at`.
fn setting(key: &str, value: Value, at: u64) -> StoredRecord {
    StoredRecord {
        version: Version(at),
        ..stored_record(
            seam::SETTING_TYPE,
            key,
            serde_json::from_value(json!({"key": key, "value": value})).unwrap(),
            at,
        )
    }
}

fn series_field() -> FieldName {
    FieldName::from(seam::series_field("at", "series"))
}

#[test]
fn a_series_pair_is_filled_whole_kept_and_put_back_to_start() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("series-index-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let location = Location::sqlite("series", &dir);
    let engine = SqliteOpener.open(&location, OpenMode::Create).unwrap();
    let ty = TypeName::from("book");
    let db = || rusqlite::Connection::open(dir.join("projection.db")).unwrap();
    let rows = || -> i64 {
        db().query_row(
            "SELECT count(*) FROM sort_index WHERE ty = 'book' AND field = ?1",
            [series_field().as_str()],
            |r| r.get(0),
        )
        .unwrap()
    };
    let state = || -> Option<serde_json::Value> {
        db().query_row("SELECT v FROM meta WHERE k = 'series.book'", [], |r| {
            r.get::<_, String>(0)
        })
        .ok()
        .map(|v| serde_json::from_str(&v).unwrap())
    };
    let kept = json!([{"series": "series", "order": "at"}]);
    let pairs = |key: &str| state().map(|s| s[key].clone()).unwrap_or(Value::Null);

    // A setting folded from the log fills the pair whole at its event.
    let p = engine.projection();
    p.begin().unwrap();
    p.put_declaration(&declaration(), 1, None, Position(0))
        .unwrap();
    for r in records() {
        p.put_record(&r).unwrap();
    }
    p.put_record(&setting("series_index.book", json!([["series", "at"]]), 1))
        .unwrap();
    p.commit(Position(1)).unwrap();
    assert_eq!(rows(), 4, "a row a book, the one with no order value too");
    assert_eq!(pairs("kept"), kept, "the pair is whole at the setting");
    assert_eq!(pairs("building"), json!([]));

    // The rows: the series' hash, then the order key, or `0xFF` for none.
    let row = |key: &str| -> Vec<u8> {
        db().query_row(
            "SELECT sort_key FROM sort_index WHERE ty = 'book' AND field = ?1 AND key = ?2",
            rusqlite::params![series_field().as_str(), key],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(row("k1")[..8], row("k2")[..8], "one series, one hash");
    assert_ne!(row("k1")[..8], row("k3")[..8], "another series, elsewhere");
    assert_eq!(row("k4")[8..], [0xFF], "no order value ranks above one");

    // Taken back a chunk a call, the pair leaves with its last chunk.
    let field = series_field();
    let mut after = None;
    loop {
        let step = Choose::DropChunk {
            field: field.clone(),
            after,
            limit: 2,
        };
        match p.choose_order(&ty, &step).unwrap() {
            Some(last) => after = Some(last),
            None => break,
        }
    }
    assert_eq!(rows(), 0);
    assert!(state().is_none(), "no pair, no row");

    // Folded from the log again, it is whole again.
    p.begin().unwrap();
    p.put_record(&setting("series_index.book", json!([["series", "at"]]), 2))
        .unwrap();
    p.commit(Position(2)).unwrap();
    assert_eq!(rows(), 4);
    assert_eq!(pairs("kept"), kept);

    // A `sort_index` rebuild leaves the pair's rows.
    p.begin().unwrap();
    p.put_record(&setting("sort_index.book", json!(["title"]), 3))
        .unwrap();
    p.commit(Position(3)).unwrap();
    assert_eq!(rows(), 4, "an order index of the lake's leaves the pair's");

    // A redeclaration puts the pair back to `Start`: no reader takes it
    // until its `Keep`, and the rows its declaration may have moved are
    // written again under the one now in force.
    p.begin().unwrap();
    p.put_declaration(&declaration(), 2, None, Position(4))
        .unwrap();
    p.commit(Position(4)).unwrap();
    assert_eq!(rows(), 4, "the pair's rows follow the declaration");
    assert_eq!(pairs("kept"), json!([]));
    assert_eq!(pairs("building"), kept);
    p.choose_order(&ty, &Choose::Start(field.clone())).unwrap();
    let mut after = None;
    loop {
        let step = Choose::Fill {
            field: field.clone(),
            after,
            limit: 3,
        };
        match p.choose_order(&ty, &step).unwrap() {
            Some(last) => after = Some(last),
            None => break,
        }
    }
    p.choose_order(&ty, &Choose::Keep(field)).unwrap();
    assert_eq!(rows(), 4);
    assert_eq!(pairs("kept"), kept);
    assert_eq!(pairs("building"), json!([]));

    // A setting put beside the writer takes its pair back in that write,
    // and the pair's rows go with it.
    p.begin().unwrap();
    p.put_record_for(&setting("series_index.book", Value::Null, 5), false)
        .unwrap();
    p.commit(Position(5)).unwrap();
    assert_eq!(rows(), 0, "a pair taken back leaves no row");
    assert!(state().is_none(), "no pair, no row");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A setting folded before the declaration it names starts its pair at the
/// declaration that first declares the fields, and fills its rows there.
#[test]
fn a_setting_put_before_the_declaration_starts_its_pair() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("series-early-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let location = Location::sqlite("series", &dir);
    let engine = SqliteOpener.open(&location, OpenMode::Create).unwrap();
    let p = engine.projection();
    p.begin().unwrap();
    p.put_record(&setting("series_index.book", json!([["series", "at"]]), 1))
        .unwrap();
    p.commit(Position(1)).unwrap();
    p.begin().unwrap();
    p.put_declaration(&declaration(), 1, None, Position(2))
        .unwrap();
    for r in records() {
        p.put_record(&r).unwrap();
    }
    p.commit(Position(2)).unwrap();
    let db = || rusqlite::Connection::open(dir.join("projection.db")).unwrap();
    let rows: i64 = db()
        .query_row(
            "SELECT count(*) FROM sort_index WHERE ty = 'book' AND field = ?1",
            [series_field().as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 4, "the declaration starts the pair and its rows");
    let state: String = db()
        .query_row("SELECT v FROM meta WHERE k = 'series.book'", [], |r| {
            r.get(0)
        })
        .unwrap();
    let state: Value = serde_json::from_str(&state).unwrap();
    assert_eq!(state["kept"], json!([]));
    assert_eq!(
        state["building"],
        json!([{"series": "series", "order": "at"}]),
        "the pair waits on its build"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
