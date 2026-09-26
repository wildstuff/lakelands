//! The order index is used only while it describes the head
//! (`docs/decisions/041-a-sorted-page-reads-its-page.md`). A lake whose
//! index a writer did not keep, one made before it or written by a binary
//! without it, answers field-ordered pages by sorting until a writer's
//! open has rebuilt the index, and the same afterwards.

use std::path::PathBuf;

use lakelands_api::Location;
use lakelands_api::engine::{After, EngineOpener, LakeEngine, OpenMode, Scan};
use lakelands_api::id::{Position, RecordKey, TypeName, Version};
use lakelands_api::read::{Direction, Order, OrderBy};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::write::Declaration;
use lakelands_core::engines;
use lakelands_engine_sqlite::SqliteOpener;
use serde_json::json;

#[test]
fn a_stale_order_index_is_not_read_and_a_writers_open_rebuilds_it() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("order-index-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let location = Location::sqlite("sorted", &dir);
    fill(
        SqliteOpener
            .open(&location, OpenMode::Create)
            .unwrap()
            .as_ref(),
    );
    // What a writer without the index leaves: rows gone or wrong, and the
    // marker behind the fold.
    let db = || rusqlite::Connection::open(dir.join("projection.db")).unwrap();
    db().execute_batch(
        "DELETE FROM sort_index WHERE rowid % 2 = 0; \
         UPDATE sort_index SET sort_key = x'00'; \
         UPDATE meta SET v = 0 WHERE k = 'sorted';",
    )
    .unwrap();
    let reader = SqliteOpener.open(&location, OpenMode::Read).unwrap();
    walks_as_one_scan(reader.as_ref(), "read-only, the index behind the fold");
    let writer = SqliteOpener.open(&location, OpenMode::Write).unwrap();
    let (sorted, rows): (i64, i64) = db()
        .query_row(
            "SELECT (SELECT v FROM meta WHERE k = 'sorted'), (SELECT count(*) FROM sort_index)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
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
    let _ = std::fs::remove_dir_all(&dir);
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
