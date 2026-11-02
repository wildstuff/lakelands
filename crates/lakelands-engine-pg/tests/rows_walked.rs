//! A page under a text more than `FEW_HOLDERS` records hold walks every
//! order row it passes, and runs the count of that walk only while a check
//! asked for walks
//! (`docs/decisions/097-a-read-in-a-window-walks-its-window.md`
//! §What `rows_read` counts, and what is added): the switch keeps a
//! product read from paying a statement a batch.

mod common;

use lakelands_api::engine::{EngineOpener, OpenMode, Scan};
use lakelands_api::id::{Position, TypeName};
use lakelands_api::read::{Direction, OrderBy};
use lakelands_api::write::Declaration;
use lakelands_core::conformance::fixture::{order, stored_record};
use lakelands_engine_pg::PgOpener;
use serde_json::json;

#[test]
fn a_many_holders_walk_is_counted_only_when_a_check_asks() {
    let Some(url) = common::url_or_skip("the pg rows-walked test") else {
        return;
    };
    let schema = common::fresh_schema();
    let location = common::location(&url, "walked", &schema);
    let engine = PgOpener.open(&location, OpenMode::Create).unwrap();
    let decl: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "t",
        "fields": [
            {"name": "v", "type": {"kind": "number"}},
            {"name": "s", "type": {"kind": "text"}}
        ]
    }))
    .unwrap();
    let p = engine.projection();
    p.begin().unwrap();
    p.put_declaration(&decl, 1, None, Position(0)).unwrap();
    for i in 0..4_200 {
        p.put_record(&stored_record(
            "t",
            format!("k{i:04}"),
            serde_json::from_value(json!({"v": i, "s": "many"})).unwrap(),
            1,
        ))
        .unwrap();
    }
    p.commit(Position(1)).unwrap();
    for (at, key, value) in [
        (2, "sort_index.t", json!(["v"])),
        (3, "lookup_index.t", json!(["s"])),
    ] {
        p.begin().unwrap();
        p.put_record(&stored_record(
            lakelands_api::seam::SETTING_TYPE,
            key,
            serde_json::from_value(json!({"key": key, "value": value})).unwrap(),
            at,
        ))
        .unwrap();
        p.commit(Position(at)).unwrap();
    }
    let view = engine.view(None).unwrap();
    let filter = lakelands_core::expr::parse("s = 'many'").unwrap();
    let scan = Scan {
        order_by: vec![order("v", OrderBy::Value, Direction::Asc)],
        filter: Some(filter),
        limit: 10,
        ..Scan::default()
    };
    let read = |counting: bool| {
        lakelands_core::engines::count_walks(counting);
        let before = lakelands_core::engines::rows_walked();
        let rows = view.scan(&TypeName::from("t"), &scan).unwrap();
        let walked = lakelands_core::engines::rows_walked() - before;
        lakelands_core::engines::count_walks(false);
        (rows.len(), walked)
    };
    let (rows, walked) = read(false);
    assert_eq!((rows, walked), (10, 0), "no check asked for walks");
    let (rows, walked) = read(true);
    assert_eq!(
        (rows, walked),
        (10, 10),
        "a check asked: the count of the range walked is the page"
    );
    drop(view);
    drop(engine);
    common::drop_schema(&url, &schema);
}
