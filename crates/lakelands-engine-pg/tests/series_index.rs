//! A `series_index.<type>` setting keeps an order index within each series
//! (`docs/decisions/097-a-read-in-a-window-walks-its-window.md` §The
//! series index), with its state in `meta` and its rows under a derived
//! field, as the SQLite test walks it.

mod common;

use lakelands_api::engine::{Choose, EngineOpener, OpenMode};
use lakelands_api::id::{FieldName, Position, TypeName, Version};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::seam;
use lakelands_api::write::Declaration;
use lakelands_core::conformance::fixture::stored_record;
use lakelands_engine_pg::PgOpener;
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

/// Run `statement` against the lake's schema and answer its rows.
fn exec(url: &str, statement: String) -> Vec<postgres::Row> {
    let url = url.to_owned();
    std::thread::spawn(move || {
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        client.query(&statement, &[]).unwrap()
    })
    .join()
    .unwrap()
}

#[test]
fn a_series_pair_is_filled_whole_kept_and_put_back_to_start() {
    let Some(url) = common::url_or_skip("the pg series-index test") else {
        return;
    };
    let schema = common::fresh_schema();
    let location = common::location(&url, "series", &schema);
    let engine = PgOpener.open(&location, OpenMode::Create).unwrap();
    let ty = TypeName::from("book");
    let field = FieldName::from(seam::series_field("at", "series"));
    let rows = || -> i64 {
        let row = exec(
            &url,
            format!(
                "SELECT count(*) FROM \"{schema}\".sort_index \
                 WHERE ty = 'book' AND field = $q${}$q$",
                field.as_str()
            ),
        );
        row[0].get(0)
    };
    let state = || -> Option<serde_json::Value> {
        let row = exec(
            &url,
            format!("SELECT t FROM \"{schema}\".meta WHERE k = 'series.book'"),
        );
        row.first()
            .and_then(|r| r.get::<_, Option<String>>(0))
            .map(|t| serde_json::from_str(&t).unwrap())
    };
    let kept = json!([{"series": "series", "order": "at"}]);
    let pairs = |key: &str| state().map(|s| s[key].clone()).unwrap_or(Value::Null);

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

    let key_of = |key: &str| -> Vec<u8> {
        let row = exec(
            &url,
            format!(
                "SELECT sort_key FROM \"{schema}\".sort_index \
                 WHERE ty = 'book' AND field = $q${}$q$ AND key = '{key}'",
                field.as_str()
            ),
        );
        row[0].get(0)
    };
    assert_eq!(key_of("k1")[..8], key_of("k2")[..8], "one series, one hash");
    assert_ne!(key_of("k1")[..8], key_of("k3")[..8], "another, elsewhere");
    assert_eq!(key_of("k4")[8..], [0xFF], "no order value ranks above one");

    // Taken back a chunk a call, the pair leaves with its last chunk.
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

    // Folded from the log again, it is whole again; a `sort_index` rebuild
    // leaves its rows.
    p.begin().unwrap();
    p.put_record(&setting("series_index.book", json!([["series", "at"]]), 2))
        .unwrap();
    p.commit(Position(2)).unwrap();
    assert_eq!(rows(), 4);
    p.begin().unwrap();
    p.put_record(&setting("sort_index.book", json!(["title"]), 3))
        .unwrap();
    p.commit(Position(3)).unwrap();
    assert_eq!(rows(), 4, "an order index of the lake's leaves the pair's");

    // A redeclaration puts the pair back to `Start`: no reader takes it
    // until its `Keep`, and a build fills it again.
    p.begin().unwrap();
    p.put_declaration(&declaration(), 2, None, Position(4))
        .unwrap();
    p.commit(Position(4)).unwrap();
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
    p.choose_order(&ty, &Choose::Keep(field.clone())).unwrap();
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
    common::drop_schema(&url, &schema);
}

/// A setting folded before the declaration it names starts its pair at the
/// declaration that first declares the fields, and fills its rows there.
#[test]
fn a_setting_put_before_the_declaration_starts_its_pair() {
    let Some(url) = common::url_or_skip("the pg early series test") else {
        return;
    };
    let schema = common::fresh_schema();
    let location = common::location(&url, "series", &schema);
    let engine = PgOpener.open(&location, OpenMode::Create).unwrap();
    let field = FieldName::from(seam::series_field("at", "series"));
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
    let row = exec(
        &url,
        format!(
            "SELECT count(*) FROM \"{schema}\".sort_index \
             WHERE ty = 'book' AND field = $q${}$q$",
            field.as_str()
        ),
    );
    assert_eq!(
        row[0].get::<_, i64>(0),
        4,
        "the declaration starts the pair"
    );
    let state = exec(
        &url,
        format!("SELECT t FROM \"{schema}\".meta WHERE k = 'series.book'"),
    );
    let state: Value =
        serde_json::from_str(&state[0].get::<_, String>(0)).expect("the pair's state");
    assert_eq!(state["kept"], json!([]));
    assert_eq!(
        state["building"],
        json!([{"series": "series", "order": "at"}]),
        "the pair waits on its build"
    );
    common::drop_schema(&url, &schema);
}
