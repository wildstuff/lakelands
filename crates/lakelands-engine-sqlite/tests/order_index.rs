//! The order index keeps the fields a lake's `sort_index.<type>` setting
//! names, and is used only while it describes the head
//! (`docs/decisions/041-a-sorted-page-reads-its-page.md`). A lake whose
//! index a writer did not keep, one written by a binary without it,
//! answers field-ordered pages by sorting until a writer's open has
//! rebuilt the index, and the same afterwards.

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
    // What a writer without the index leaves: rows gone or wrong, and the
    // type's mark behind the fold (it folded on) or gone (it wrote the
    // setting).
    for (i, (tamper, left)) in [
        (
            "UPDATE meta SET v = 0 WHERE k = 'sorted.t'",
            "the mark behind the fold",
        ),
        ("DELETE FROM meta WHERE k = 'sorted.t'", "no mark"),
    ]
    .into_iter()
    .enumerate()
    {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("order-index-{}-{i}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let location = Location::sqlite("sorted", &dir);
        fill(
            SqliteOpener
                .open(&location, OpenMode::Create)
                .unwrap()
                .as_ref(),
        );
        let db = || rusqlite::Connection::open(dir.join("projection.db")).unwrap();
        db().execute_batch(&format!(
            "DELETE FROM sort_index WHERE rowid % 2 = 0; \
             UPDATE sort_index SET sort_key = x'00'; {tamper};"
        ))
        .unwrap();
        let reader = SqliteOpener.open(&location, OpenMode::Read).unwrap();
        walks_as_one_scan(reader.as_ref(), &format!("read-only, {left}"));
        let writer = SqliteOpener.open(&location, OpenMode::Write).unwrap();
        let (sorted, rows): (Option<i64>, i64) = db()
            .query_row(
                "SELECT (SELECT v FROM meta WHERE k = 'sorted.t'), \
                 (SELECT count(*) FROM sort_index)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (sorted, rows),
            (Some(1), holding("v") + holding("w")),
            "{left}: a writer's open rebuilds the index from the head and marks it kept \
             through the fold"
        );
        walks_as_one_scan(writer.as_ref(), &format!("{left}, after the writer's open"));
        walks_as_one_scan(
            reader.as_ref(),
            &format!("{left}, read-only, the index rebuilt"),
        );
        drop((reader, writer));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn the_setting_keeps_the_rows_of_the_fields_it_names_and_no_others() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("order-fields-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let engine = SqliteOpener
        .open(&Location::sqlite("sorted", &dir), OpenMode::Create)
        .unwrap();
    fill(engine.as_ref());
    let rows = || -> (Option<i64>, i64, i64) {
        rusqlite::Connection::open(dir.join("projection.db"))
            .unwrap()
            .query_row(
                "SELECT (SELECT v FROM meta WHERE k = 'sorted.t'), \
                 (SELECT count(*) FROM sort_index WHERE field = 'v'), \
                 (SELECT count(*) FROM sort_index WHERE field = 'w')",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
    };
    assert_eq!(
        rows(),
        (Some(1), holding("v"), holding("w")),
        "the setting on `v` and `w`"
    );
    ask(engine.as_ref(), json!(["v"]), 2);
    assert_eq!(
        rows(),
        (Some(2), holding("v"), 0),
        "the setting's write without `w` drops the rows of `w`"
    );
    walks_as_one_scan(engine.as_ref(), "the setting on `v` alone");
    let p = engine.projection();
    p.begin().unwrap();
    p.put_record(&StoredRecord {
        retired: true,
        ..setting(json!(["v"]), 3)
    })
    .unwrap();
    p.commit(Position(3)).unwrap();
    assert_eq!(
        rows(),
        (None, 0, 0),
        "the setting retired drops every row and the type's mark"
    );
    walks_as_one_scan(engine.as_ref(), "the setting retired");
    drop(engine);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_field_ordered_page_reads_its_page_not_the_type() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("order-page-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let engine = SqliteOpener
        .open(&Location::sqlite("sorted", &dir), OpenMode::Create)
        .unwrap();
    fill(engine.as_ref());
    // The last record in either direction holds no `v`; a state no read
    // can decode there fails any read that reaches it.
    let last = last_missing();
    rusqlite::Connection::open(dir.join("projection.db"))
        .unwrap()
        .execute(
            "UPDATE records SET state = 'unreadable' WHERE ty = 't' AND key = ?1",
            [&last],
        )
        .unwrap();
    first_pages_answer(engine.as_ref());
    drop(engine);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A setting may be written before the declaration that first declares
/// its fields, in the same transaction or in one before. The setting's
/// write finds no field to keep and leaves no mark; the declaration,
/// whose records fill the index, marks it, so a view reads the index from
/// that commit on.
#[test]
fn a_setting_written_before_its_fields_are_declared_is_marked_by_the_declaration() {
    for (apart, how) in [
        (false, "before the declaration, in its transaction"),
        (true, "in the transaction before the declaration's"),
    ] {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("order-declared-{}-{apart}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let engine = SqliteOpener
            .open(&Location::sqlite("sorted", &dir), OpenMode::Create)
            .unwrap();
        let p = engine.projection();
        p.begin().unwrap();
        p.put_record(&setting(json!(["v", "w"]), 1)).unwrap();
        if apart {
            p.commit(Position(1)).unwrap();
            p.begin().unwrap();
        }
        p.put_declaration(&declaration(), 1, None, Position(1))
            .unwrap();
        for r in states() {
            p.put_record(&r).unwrap();
        }
        p.commit(Position(2)).unwrap();
        let (sorted, rows): (Option<i64>, i64) =
            rusqlite::Connection::open(dir.join("projection.db"))
                .unwrap()
                .query_row(
                    "SELECT (SELECT v FROM meta WHERE k = 'sorted.t'), \
                     (SELECT count(*) FROM sort_index)",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
        assert_eq!(
            (sorted, rows),
            (Some(2), holding("v") + holding("w")),
            "the setting {how}: the declaration marks the index its records fill, kept \
             through the commit"
        );
        walks_as_one_scan(engine.as_ref(), how);
        drop(engine);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The records the lake holds: a declared `v` of every kind the rulebook
/// ranks, missing from every seventh, and a declared `w` of three values.
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
            let mut fields = if i % 7 == 0 {
                json!({})
            } else {
                json!({"v": values[(i as usize * 5) % values.len()]})
            };
            fields["w"] = json!(format!("w{}", i % 3));
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
        "fields": [
            {"name": "v", "type": {"kind": "text"}},
            {"name": "w", "type": {"kind": "text"}}
        ]
    }))
    .unwrap()
}

/// Fold the records into a fresh lake, then the setting that asks for the
/// order index on `v` and `w`.
fn fill(engine: &dyn LakeEngine) {
    let p = engine.projection();
    p.begin().unwrap();
    p.put_declaration(&declaration(), 1, None, Position(0))
        .unwrap();
    for r in states() {
        p.put_record(&r).unwrap();
    }
    p.put_record(&setting(json!(["v", "w"]), 1)).unwrap();
    p.commit(Position(1)).unwrap();
}

/// The state of the setting `sort_index.t` asking for `fields`, at `at`.
fn setting(fields: Value, at: u64) -> StoredRecord {
    StoredRecord {
        ty: TypeName::from(lakelands_api::seam::SETTING_TYPE),
        key: RecordKey::from("sort_index.t"),
        version: Version(at),
        fields: serde_json::from_value(json!({"key": "sort_index.t", "value": fields})).unwrap(),
        meta: Default::default(),
        position: Position(at),
        retired: false,
        restricted: false,
    }
}

/// Fold one write of the setting, at `at`.
fn ask(engine: &dyn LakeEngine, fields: Value, at: u64) {
    let p = engine.projection();
    p.begin().unwrap();
    p.put_record(&setting(fields, at)).unwrap();
    p.commit(Position(at)).unwrap();
}

/// How many records hold `field` with a value.
fn holding(field: &str) -> i64 {
    states()
        .iter()
        .filter(|r| r.fields.contains_key(field))
        .count() as i64
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
