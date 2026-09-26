//! The order index is used only while it describes the head
//! (`docs/decisions/041-a-sorted-page-reads-its-page.md`). A lake whose
//! index a writer did not keep, one made before it or written by a binary
//! without it, answers field-ordered pages by sorting until a writer's
//! open has rebuilt the index, and the same afterwards.

mod common;

use lakelands_api::engine::{After, EngineOpener, LakeEngine, OpenMode, Scan};
use lakelands_api::id::{Position, RecordKey, TypeName, Version};
use lakelands_api::read::{Direction, Order, OrderBy};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::write::Declaration;
use lakelands_core::engines;
use lakelands_engine_pg::PgOpener;
use serde_json::json;

/// Run statements against the lake's schema from a connection of the
/// test's own.
fn exec(url: &str, statements: String) -> Vec<postgres::Row> {
    let url = url.to_owned();
    std::thread::spawn(move || {
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        client.query(&statements, &[]).unwrap()
    })
    .join()
    .unwrap()
}

#[test]
fn a_stale_order_index_is_not_read_and_a_writers_open_rebuilds_it() {
    let Some(url) = common::url_or_skip("the pg order-index test") else {
        return;
    };
    let schema = common::fresh_schema();
    let location = common::location(&url, "sorted", &schema);
    fill(PgOpener.open(&location, OpenMode::Create).unwrap().as_ref());
    // What a writer without the index leaves: rows gone or wrong, and the
    // marker behind the fold.
    exec(
        &url,
        format!(
            "WITH gone AS (DELETE FROM \"{schema}\".sort_index WHERE key < 'k100') \
             UPDATE \"{schema}\".sort_index SET sort_key = '\\x00'"
        ),
    );
    exec(
        &url,
        format!("UPDATE \"{schema}\".meta SET n = 0 WHERE k = 'sorted'"),
    );
    let reader = PgOpener.open(&location, OpenMode::Read).unwrap();
    walks_as_one_scan(reader.as_ref(), "read-only, the index behind the fold");
    let writer = PgOpener.open(&location, OpenMode::Write).unwrap();
    let row = exec(
        &url,
        format!(
            "SELECT (SELECT n FROM \"{schema}\".meta WHERE k = 'sorted'), \
             (SELECT count(*) FROM \"{schema}\".sort_index)"
        ),
    );
    let (sorted, rows): (i64, i64) = (row[0].get(0), row[0].get(1));
    let held = states()
        .iter()
        .filter(|r| r.fields.contains_key("v"))
        .count() as i64;
    assert_eq!(
        (sorted, rows),
        (1, held),
        "a writer's open rebuilds the index from the head and marks it kept through the fold"
    );
    walks_as_one_scan(writer.as_ref(), "after the writer's open");
    walks_as_one_scan(reader.as_ref(), "read-only, the index rebuilt");
    drop((reader, writer));
    common::drop_schema(&url, &schema);
}

#[test]
fn a_field_ordered_page_reads_its_page_not_the_type() {
    let Some(url) = common::url_or_skip("the pg order-index test") else {
        return;
    };
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(&common::location(&url, "sorted", &schema), OpenMode::Create)
        .unwrap();
    fill(engine.as_ref());
    // The last record in either direction holds no `v`; a state no read
    // can decode there fails any read that reaches it.
    exec(
        &url,
        format!(
            "UPDATE \"{schema}\".records SET state = 'unreadable' WHERE ty = 't' AND key = '{}'",
            last_missing()
        ),
    );
    first_pages_answer(engine.as_ref());
    drop(engine);
    common::drop_schema(&url, &schema);
}

/// The records the lake holds: a declared `v` of every kind the rulebook
/// ranks, missing from every seventh.
fn states() -> Vec<StoredRecord> {
    let values = [
        json!(3),
        json!("3.0"),
        json!(-2.5),
        json!("2026-01-01T00:00:00Z"),
        json!("apple"),
        json!("Apple"),
        json!(false),
        json!({"a": 1}),
    ];
    (0..200u64)
        .map(|i| {
            let fields = if i % 7 == 0 {
                json!({})
            } else {
                json!({"v": values[(i as usize * 5) % values.len()]})
            };
            StoredRecord {
                ty: TypeName::from("t"),
                key: RecordKey::from(format!("k{:03}", (i * 37) % 211)),
                version: Version(1),
                fields: serde_json::from_value(fields).unwrap(),
                meta: Default::default(),
                position: Position(1),
                retired: false,
                restricted: false,
            }
        })
        .collect()
}

fn declaration() -> Declaration {
    serde_json::from_value(json!({
        "declare": "type", "name": "t",
        "fields": [{"name": "v", "type": {"kind": "text"}}]
    }))
    .unwrap()
}

/// Fold the records into a fresh lake.
fn fill(engine: &dyn LakeEngine) {
    let p = engine.projection();
    p.begin().unwrap();
    p.put_declaration(&declaration(), 1, None, Position(0))
        .unwrap();
    for r in states() {
        p.put_record(&r).unwrap();
    }
    p.commit(Position(1)).unwrap();
}

/// Every field-ordered walk of `v`, in pages of 7, against one sort of
/// every state.
fn walks_as_one_scan(engine: &dyn LakeEngine, when: &str) {
    let Declaration::Type(decl) = declaration() else {
        unreachable!()
    };
    let states = states();
    let view = engine.view(None).unwrap();
    for dir in [Direction::Asc, Direction::Desc] {
        let order_by = vec![Order {
            field: "v".into(),
            by: OrderBy::Value,
            dir,
        }];
        let whole: Vec<String> = engines::scan(
            Some(&decl),
            &states,
            &Scan {
                order_by: order_by.clone(),
                limit: u32::MAX,
                ..Scan::default()
            },
        )
        .iter()
        .map(|r| r.key.to_string())
        .collect();
        let mut page = Scan {
            order_by: order_by.clone(),
            limit: 7,
            ..Scan::default()
        };
        let mut walked = Vec::new();
        for _ in 0..=states.len() {
            let rows = view.scan(&TypeName::from("t"), &page).unwrap();
            let Some(last) = rows.last() else { break };
            page.after = Some(After {
                values: vec![last.fields.get("v").cloned().unwrap_or(Value::Null)],
                key: last.key.clone(),
            });
            walked.extend(rows.iter().map(|r| r.key.to_string()));
        }
        assert_eq!(walked, whole, "{when}: walking `v` {dir:?}");
    }
}

/// The key of the record a field-ordered walk of `v` reaches last in
/// either direction: the last in key order of those without `v`.
fn last_missing() -> String {
    states()
        .iter()
        .filter(|r| !r.fields.contains_key("v"))
        .map(|r| r.key.to_string())
        .max()
        .unwrap()
}

/// The first page of `v` in either direction answers as one sort of every
/// state does, without reading the record the walk reaches last.
fn first_pages_answer(engine: &dyn LakeEngine) {
    let Declaration::Type(decl) = declaration() else {
        unreachable!()
    };
    let view = engine.view(None).unwrap();
    for dir in [Direction::Asc, Direction::Desc] {
        let scan = Scan {
            order_by: vec![Order {
                field: "v".into(),
                by: OrderBy::Value,
                dir,
            }],
            limit: 7,
            ..Scan::default()
        };
        let expected: Vec<String> = engines::scan(Some(&decl), &states(), &scan)
            .iter()
            .map(|r| r.key.to_string())
            .collect();
        let page: Vec<String> = view
            .scan(&TypeName::from("t"), &scan)
            .unwrap_or_else(|e| panic!("a page of 7 by `v` {dir:?} read past its page: {e}"))
            .iter()
            .map(|r| r.key.to_string())
            .collect();
        assert_eq!(page, expected, "the first page of `v` {dir:?}");
    }
}
