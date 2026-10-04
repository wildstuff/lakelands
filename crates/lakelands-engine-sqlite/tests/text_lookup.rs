//! At the head, a count and a page by key whose filter holds an exact
//! text are handed the records holding that text, not the type
//! (lakelands#574). The suite's check runs on the memory engine too,
//! which keeps no index of exact text and reads the type, so its bound
//! cannot tell the index from a walk of the type; this one can.

use std::path::PathBuf;

use lakelands_api::Location;
use lakelands_api::engine::{EngineOpener, OpenMode, Scan};
use lakelands_api::id::{Position, TypeName, Version};
use lakelands_api::record::StoredRecord;
use lakelands_core::engines;
use lakelands_engine_sqlite::SqliteOpener;
use serde_json::json;

/// More than two batches of 1 024.
const N: u64 = 3000;

#[test]
fn a_count_and_a_page_under_an_exact_text_are_handed_the_records_holding_it() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("text-lookup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let engine = SqliteOpener
        .open(&Location::sqlite("texts", &dir), OpenMode::Create)
        .unwrap();
    // Three records hold `v = 'b'`, one in each batch of the type.
    let p = engine.projection();
    p.begin().unwrap();
    for i in 0..N {
        let v = if i % 1000 == 500 { "b" } else { "a" };
        p.put_record(&StoredRecord {
            ty: "t".into(),
            key: format!("k{i:04}").into(),
            version: Version(1),
            fields: serde_json::from_value(json!({"v": v})).unwrap(),
            meta: Default::default(),
            position: Position(1),
            retired: false,
            restricted: false,
        })
        .unwrap();
    }
    p.commit(Position(1)).unwrap();
    let (ty, view) = (TypeName::from("t"), engine.view(None).unwrap());
    let scan = Scan {
        filter: Some(lakelands_core::expr::parse("v = 'b'").unwrap()),
        limit: u32::MAX,
        ..Scan::default()
    };
    let before = engines::rows_read();
    let counted = view.count(&ty, &scan).unwrap();
    let read = engines::rows_read() - before;
    assert!(
        counted == 3 && read <= 3,
        "a count under `v = 'b'` at the head answered {counted} from {read} rows handed to the \
         rulebook, of a type of {N}; three records hold the text, and it reads those"
    );
    let before = engines::rows_read();
    let page = view.scan(&ty, &Scan { limit: 10, ..scan }).unwrap();
    let read = engines::rows_read() - before;
    assert!(
        page.len() == 3 && read <= 3,
        "a page of 10 by key under `v = 'b'` at the head answered {} rows from {read} handed to \
         the rulebook, of a type of {N}; three records hold the text, and it reads those",
        page.len()
    );
    drop(view);
    drop(engine);
    let _ = std::fs::remove_dir_all(&dir);
}
