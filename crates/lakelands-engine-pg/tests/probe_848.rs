//! lakelands#848 measurement (throwaway): a first load of N records with the
//! `sort_index.t` setting in the same projection transaction, against the
//! same load in two. `PROBE_848_N` sets the count; run with `--ignored
//! --nocapture`.

mod common;

use std::time::Instant;

use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::id::{Position, RecordKey, TypeName, Version};
use lakelands_api::record::StoredRecord;
use lakelands_api::write::Declaration;
use lakelands_engine_pg::PgOpener;
use serde_json::json;

fn declaration() -> Declaration {
    serde_json::from_value(json!({"declare": "type", "name": "t", "fields": [
        {"name": "v", "type": {"kind": "text"}}, {"name": "w", "type": {"kind": "text"}}]}))
    .unwrap()
}

fn record(ty: &str, key: &str, fields: serde_json::Value, at: u64) -> StoredRecord {
    StoredRecord {
        ty: TypeName::from(ty),
        key: RecordKey::from(key),
        version: Version(1),
        fields: serde_json::from_value(fields).unwrap(),
        meta: Default::default(),
        position: Position(at),
        made: Position(at),
        retired: false,
        restricted: false,
    }
}

fn setting(at: u64) -> StoredRecord {
    record(
        lakelands_api::seam::SETTING_TYPE,
        "sort_index.t",
        json!({"key": "sort_index.t", "value": ["v", "w"]}),
        at,
    )
}

#[test]
#[ignore]
fn load() {
    let Some(url) = common::url_or_skip("probe 848") else { return };
    let n: u64 = std::env::var("PROBE_848_N").ok().and_then(|v| v.parse().ok()).unwrap_or(41_000);
    let layout = std::env::var("PROBE_848_LAYOUT").unwrap_or_else(|_| "one".into());
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(&common::location(&url, "probe", &schema), OpenMode::Create)
        .unwrap();
    let recs = |from: u64| {
        (from..n).map(|i| {
            record("t", &format!("k{i:07}"), json!({"v": format!("v{}", (i * 7919) % 10007), "w": format!("w{}", i % 31)}), 1)
        })
    };
    let t0 = Instant::now();
    let p = engine.projection();
    match layout.as_str() {
        // setting and records in one fold transaction
        "one" => {
            p.begin().unwrap();
            p.put_declaration(&declaration(), 1, None, Position(0)).unwrap();
            p.put_record(&setting(1)).unwrap();
            for r in recs(0) {
                p.put_record(&r).unwrap();
            }
            p.commit(Position(1)).unwrap();
        }
        // records first, the setting last in the same transaction (order_index.rs's fill)
        "one-last" => {
            p.begin().unwrap();
            p.put_declaration(&declaration(), 1, None, Position(0)).unwrap();
            for r in recs(0) {
                p.put_record(&r).unwrap();
            }
            p.put_record(&setting(1)).unwrap();
            p.commit(Position(1)).unwrap();
        }
        // the setting first, then the records
        _ => {
            p.begin().unwrap();
            p.put_declaration(&declaration(), 1, None, Position(0)).unwrap();
            p.put_record(&setting(1)).unwrap();
            p.commit(Position(1)).unwrap();
            let t1 = Instant::now();
            p.begin().unwrap();
            for r in recs(0) {
                p.put_record(&r).unwrap();
            }
            p.commit(Position(1)).unwrap();
            eprintln!("RESULT second transaction (records): {:?}", t1.elapsed());
        }
    }
    eprintln!("RESULT layout={layout} n={n} total {:?}", t0.elapsed());
    drop(engine);
    common::drop_schema(&url, &schema);
}
