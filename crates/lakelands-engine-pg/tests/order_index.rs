//! The order index keeps the fields a lake's `sort_index.<type>` setting
//! names, and is used only while it describes the head
//! (`docs/decisions/041-a-sorted-page-reads-its-page.md`). A lake whose
//! index a writer did not keep, one written by a binary without it,
//! answers field-ordered pages by sorting until a writer's open has
//! rebuilt the index, and the same afterwards.

mod common;

use lakelands_api::engine::{After, EngineOpener, LakeEngine, OpenMode, Scan};
use lakelands_api::id::{Position, RecordKey, TypeName, Version};
use lakelands_api::read::{Direction, Order, OrderBy};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::write::Declaration;
use lakelands_core::conformance::fixture::stored_record;
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
    // What a writer without the index leaves: rows gone or wrong, and the
    // type's mark behind the fold (it folded on) or gone (it wrote the
    // setting).
    for (mark, left) in [
        ("n = 0", "the mark behind the fold"),
        ("n = NULL", "no mark"),
    ] {
        let schema = common::fresh_schema();
        let location = common::location(&url, "sorted", &schema);
        fill(PgOpener.open(&location, OpenMode::Create).unwrap().as_ref());
        exec(
            &url,
            format!(
                "WITH gone AS (DELETE FROM \"{schema}\".sort_index WHERE key < 'k100') \
                 UPDATE \"{schema}\".sort_index SET sort_key = '\\x00'"
            ),
        );
        let tamper = if mark == "n = NULL" {
            format!("DELETE FROM \"{schema}\".meta WHERE k = 'sorted.t'")
        } else {
            format!("UPDATE \"{schema}\".meta SET {mark} WHERE k = 'sorted.t'")
        };
        exec(&url, tamper);
        let reader = PgOpener.open(&location, OpenMode::Read).unwrap();
        walks_as_one_scan(reader.as_ref(), &format!("read-only, {left}"));
        let writer = PgOpener.open(&location, OpenMode::Write).unwrap();
        let row = exec(
            &url,
            format!(
                "SELECT (SELECT n FROM \"{schema}\".meta WHERE k = 'sorted.t'), \
                 (SELECT count(*) FROM \"{schema}\".sort_index)"
            ),
        );
        let (sorted, rows): (Option<i64>, i64) = (row[0].get(0), row[0].get(1));
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
        common::drop_schema(&url, &schema);
    }
}

/// A view's setting asks for its kept name's index, and the view's own
/// name keeps no field. A writer's open rebuilds nothing while the kept
/// name's index is kept through the fold, and rebuilds it when its mark is
/// gone.
#[test]
fn a_writers_open_catches_up_a_kept_names_index_and_not_its_views_name() {
    let Some(url) = common::url_or_skip("the pg order-index test") else {
        return;
    };
    let schema = common::fresh_schema();
    let location = common::location(&url, "sorted", &schema);
    let kept = fill_view(PgOpener.open(&location, OpenMode::Create).unwrap().as_ref());
    let held = || -> (Option<i64>, i64, i64) {
        let row = exec(
            &url,
            format!(
                "SELECT (SELECT n FROM \"{schema}\".meta WHERE k = 'sorted.{kept}'), \
                 (SELECT count(*) FROM \"{schema}\".sort_index WHERE ty = '{kept}'), \
                 (SELECT count(*) FROM \"{schema}\".sort_index WHERE ty = 'w')"
            ),
        );
        (row[0].get(0), row[0].get(1), row[0].get(2))
    };
    // A row under the view's own name, which a rebuild of that name drops.
    exec(
        &url,
        format!(
            "INSERT INTO \"{schema}\".sort_index (ty, field, key, sort_key) \
             VALUES ('w', 'v', 'k0', '\\x00')"
        ),
    );
    drop(PgOpener.open(&location, OpenMode::Write).unwrap());
    assert_eq!(
        held(),
        (Some(1), 3, 1),
        "an open after the index is built rebuilds nothing"
    );
    exec(
        &url,
        format!(
            "WITH gone AS (DELETE FROM \"{schema}\".sort_index WHERE ty = '{kept}') \
             DELETE FROM \"{schema}\".meta WHERE k = 'sorted.{kept}'"
        ),
    );
    drop(PgOpener.open(&location, OpenMode::Write).unwrap());
    assert_eq!(
        held(),
        (Some(1), 3, 1),
        "an open with the kept name's mark gone rebuilds its index"
    );
    common::drop_schema(&url, &schema);
}

#[test]
fn the_setting_keeps_the_rows_of_the_fields_it_names_and_no_others() {
    let Some(url) = common::url_or_skip("the pg order-index test") else {
        return;
    };
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(&common::location(&url, "sorted", &schema), OpenMode::Create)
        .unwrap();
    fill(engine.as_ref());
    let rows = || {
        let row = exec(
            &url,
            format!(
                "SELECT (SELECT n FROM \"{schema}\".meta WHERE k = 'sorted.t'), \
                 (SELECT count(*) FROM \"{schema}\".sort_index WHERE field = 'v'), \
                 (SELECT count(*) FROM \"{schema}\".sort_index WHERE field = 'w')"
            ),
        );
        (
            row[0].get::<_, Option<i64>>(0),
            row[0].get::<_, i64>(1),
            row[0].get::<_, i64>(2),
        )
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

/// A setting may be written before the declaration that first declares
/// its fields: a page or more before it in the same transaction (a
/// transaction's puts are written a page of 1 024 at a time), or in a
/// transaction before. The setting's write finds no field to keep and
/// leaves no mark; the declaration, whose records fill the index, marks
/// it, so a view reads the index from that commit on, wherever a page
/// ended.
#[test]
fn a_setting_written_before_its_fields_are_declared_is_marked_by_the_declaration() {
    let Some(url) = common::url_or_skip("the pg order-index test") else {
        return;
    };
    for (apart, how) in [
        (false, "a page before the declaration, in its transaction"),
        (true, "in the transaction before the declaration's"),
    ] {
        let schema = common::fresh_schema();
        let engine = PgOpener
            .open(&common::location(&url, "sorted", &schema), OpenMode::Create)
            .unwrap();
        let p = engine.projection();
        p.begin().unwrap();
        p.put_record(&setting(json!(["v", "w"]), 1)).unwrap();
        if apart {
            p.commit(Position(1)).unwrap();
            p.begin().unwrap();
        } else {
            // More than a page of other puts: the setting's page is written.
            for i in 0..1_100 {
                p.put_record(&StoredRecord {
                    ty: TypeName::from("u"),
                    key: RecordKey::from(format!("u{i:04}")),
                    ..setting(json!([]), 1)
                })
                .unwrap();
            }
        }
        p.put_declaration(&declaration(), 1, None, Position(1))
            .unwrap();
        for r in states() {
            p.put_record(&r).unwrap();
        }
        p.commit(Position(2)).unwrap();
        let row = exec(
            &url,
            format!(
                "SELECT (SELECT n FROM \"{schema}\".meta WHERE k = 'sorted.t'), \
                 (SELECT count(*) FROM \"{schema}\".sort_index)"
            ),
        );
        let (sorted, rows): (Option<i64>, i64) = (row[0].get(0), row[0].get(1));
        assert_eq!(
            (sorted, rows),
            (Some(2), holding("v") + holding("w")),
            "the setting {how}: the declaration marks the index its records fill, kept \
             through the commit"
        );
        walks_as_one_scan(engine.as_ref(), how);
        drop(engine);
        common::drop_schema(&url, &schema);
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
            stored_record(
                "t",
                format!("k{:03}", (i * 37) % 211),
                serde_json::from_value(fields).unwrap(),
                1,
            )
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

/// Fold a view `w` of `t`, three records its fold keeps under its kept
/// name, and the setting `sort_index.w` on `v`; answers the kept name.
fn fill_view(engine: &dyn LakeEngine) -> TypeName {
    let view: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "w",
        "fields": [{"name": "v", "type": {"kind": "text"}}],
        "view": {"source": "t", "group_by": ["v"], "measures": []}
    }))
    .unwrap();
    let kept = lakelands_api::seam::kept_name(&TypeName::from("w"));
    let record = |ty: &TypeName, key: &str, fields: Value| {
        stored_record(ty.clone(), key, serde_json::from_value(fields).unwrap(), 1)
    };
    let p = engine.projection();
    p.begin().unwrap();
    p.put_declaration(&declaration(), 1, None, Position(0))
        .unwrap();
    p.put_declaration(&view, 1, None, Position(0)).unwrap();
    for i in 0..3 {
        p.put_record(&record(
            &kept,
            &format!("k{i}"),
            json!({"v": format!("v{i}")}),
        ))
        .unwrap();
    }
    let setting = TypeName::from(lakelands_api::seam::SETTING_TYPE);
    p.put_record(&record(
        &setting,
        "sort_index.w",
        json!({"key": "sort_index.w", "value": ["v"]}),
    ))
    .unwrap();
    p.commit(Position(1)).unwrap();
    kept
}

/// The state of the setting `sort_index.t` asking for `fields`, at `at`.
fn setting(fields: Value, at: u64) -> StoredRecord {
    StoredRecord {
        version: Version(at),
        ..stored_record(
            lakelands_api::seam::SETTING_TYPE,
            "sort_index.t",
            serde_json::from_value(json!({"key": "sort_index.t", "value": fields})).unwrap(),
            at,
        )
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
