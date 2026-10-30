//! After a load, the writer analyses its tables: once `ANALYZE_AFTER`
//! records have gone in since the last analysis, and not before
//! (`src/handle.rs`, "Statistics after a load"). The analysis is read
//! where the server records it, `pg_stat_user_tables.last_analyze`, which
//! PostgreSQL's own background analysis never sets.

mod common;

use std::time::{Duration, Instant};

use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode, ProjectionStore};
use lakelands_api::id::Position;
use lakelands_api::seam::SETTING_TYPE;
use lakelands_api::write::Declaration;
use lakelands_core::conformance::fixture::stored_record;
use lakelands_engine_pg::{ANALYZE_AFTER, PgOpener};
use serde_json::{Value, json};

/// How many times the server has recorded an `ANALYZE` of the lake's
/// `records`, asked on a connection of the test's own.
fn analyses(url: &str, schema: &str) -> i64 {
    let (url, schema) = (url.to_owned(), schema.to_owned());
    std::thread::spawn(move || {
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        let sql = "SELECT analyze_count FROM pg_stat_user_tables \
                   WHERE schemaname = $1 AND relname = 'records'";
        client.query_one(sql, &[&schema]).unwrap().get(0)
    })
    .join()
    .unwrap()
}

/// Put the record `(ty, key)` into the open transaction at `at`.
fn record(p: &dyn ProjectionStore, at: u64, (ty, key): (&str, String), fields: Value) {
    p.put_record(&stored_record(
        ty,
        key,
        serde_json::from_value(fields).unwrap(),
        at,
    ))
    .unwrap();
}

/// Put `n` records, from key `from`, and then the setting that orders `t`
/// by `v` if `ordered`, into the open transaction.
fn load(p: &dyn ProjectionStore, from: u64, n: u64, at: u64, ordered: bool) {
    for i in from..from + n {
        record(
            p,
            at,
            ("t", format!("k{i:06}")),
            json!({"v": format!("v{i}")}),
        );
    }
    if ordered {
        let key = (SETTING_TYPE, "sort_index.t".to_owned());
        record(p, at, key, json!({"key": "sort_index.t", "value": ["v"]}));
    }
}

/// Fold `n` records, from key `from`, and the order setting if `ordered`,
/// in one transaction committed through `at`.
fn fold(engine: &dyn LakeEngine, from: u64, n: u64, at: u64, ordered: bool) {
    let p = engine.projection();
    p.begin().unwrap();
    load(p, from, n, at, ordered);
    p.commit(Position(at)).unwrap();
}

/// A fresh lake in `schema` with the type `t` declared at position 1.
fn lake(url: &str, schema: &str) -> Box<dyn LakeEngine> {
    let engine = PgOpener
        .open(&common::location(url, "analysed", schema), OpenMode::Create)
        .unwrap();
    let decl: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "t",
        "fields": [{"name": "v", "type": {"kind": "text"}}]
    }))
    .unwrap();
    let p = engine.projection();
    p.begin().unwrap();
    p.put_declaration(&decl, 1, None, Position(1)).unwrap();
    p.commit(Position(1)).unwrap();
    engine
}

#[test]
fn the_writer_analyses_its_tables_once_enough_records_went_in_and_not_before() {
    let Some(url) = common::url_or_skip("the pg analysis test") else {
        return;
    };
    let schema = common::fresh_schema();
    let engine = lake(&url, &schema);
    // One record short of the threshold, in windows as a load commits them.
    let below = ANALYZE_AFTER - 1;
    let mut put = 0;
    let mut at = 1;
    while put < below {
        let n = (below - put).min(1000);
        at += 1;
        fold(engine.as_ref(), put, n, at, false);
        put += n;
    }
    // The server records an analysis when its backend next reports, at
    // most about ten seconds after going idle.
    std::thread::sleep(Duration::from_secs(12));
    assert!(
        analyses(&url, &schema) == 0,
        "{below} records went in, one fewer than {ANALYZE_AFTER}, and the tables were analysed"
    );
    at += 1;
    fold(engine.as_ref(), put, 1, at, false);
    let asked = Instant::now();
    while analyses(&url, &schema) == 0 {
        assert!(
            asked.elapsed() < Duration::from_secs(30),
            "{ANALYZE_AFTER} records went in and the tables were not analysed"
        );
        std::thread::sleep(Duration::from_millis(500));
    }
    drop(engine);
    common::drop_schema(&url, &schema);
}

/// The count is of the records a transaction staged, not of those its
/// buffer holds at the commit: the buffer is written a page at a time, so
/// at the commit of one transaction of `ANALYZE_AFTER` records it holds
/// the last page only.
#[test]
fn a_transaction_written_by_pages_is_counted_whole() {
    let Some(url) = common::url_or_skip("the pg analysis test") else {
        return;
    };
    let (short, whole) = (common::fresh_schema(), common::fresh_schema());
    let below = lake(&url, &short);
    fold(below.as_ref(), 0, ANALYZE_AFTER - 1, 2, false);
    let committed = Instant::now();
    let at = lake(&url, &whole);
    fold(at.as_ref(), 0, ANALYZE_AFTER, 2, false);
    let asked = Instant::now();
    while analyses(&url, &whole) == 0 {
        assert!(
            asked.elapsed() < Duration::from_secs(30),
            "one transaction put {ANALYZE_AFTER} records and the tables were not analysed"
        );
        std::thread::sleep(Duration::from_millis(500));
    }
    // The server records an analysis at most about ten seconds after its
    // backend went idle.
    std::thread::sleep(Duration::from_secs(12).saturating_sub(committed.elapsed()));
    assert!(
        analyses(&url, &short) == 0,
        "one transaction put one record fewer than {ANALYZE_AFTER} and the tables were analysed"
    );
    drop((below, at));
    common::drop_schema(&url, &short);
    common::drop_schema(&url, &whole);
}

/// A `sort_index.<type>` setting's write rebuilds the type's order index
/// at once, and the rows it writes count as records put do: the pages
/// read through the index after it are planned from the index the planner
/// knows, not from an index it last saw empty (lakelands#587).
#[test]
fn the_order_index_a_setting_rebuilds_counts_toward_the_analysis() {
    let Some(url) = common::url_or_skip("the pg analysis test") else {
        return;
    };
    let schema = common::fresh_schema();
    let engine = lake(&url, &schema);
    // Half the threshold and one: the records and their rows reach it.
    let half = ANALYZE_AFTER / 2 + 1;
    fold(engine.as_ref(), 0, half, 2, false);
    let p = engine.projection();
    p.begin().unwrap();
    load(p, 0, 0, 3, true);
    p.commit(Position(3)).unwrap();
    let asked = Instant::now();
    while analyses(&url, &schema) == 0 {
        assert!(
            asked.elapsed() < Duration::from_secs(30),
            "{half} records went in and a setting rebuilt their order index, {half} rows, and \
             the tables were not analysed"
        );
        std::thread::sleep(Duration::from_millis(500));
    }
    drop(engine);
    common::drop_schema(&url, &schema);
}

/// With `ANALYZE_AFTER` records and a `sort_index.t` setting in one
/// transaction, the order index's rebuild is preceded by an analysis
/// inside it, besides the one after the commit (lakelands#848).
#[test]
fn a_rebuild_after_a_large_load_in_one_transaction_analyses_first() {
    let Some(url) = common::url_or_skip("the pg analysis test") else {
        return;
    };
    let schema = common::fresh_schema();
    let engine = lake(&url, &schema);
    fold(engine.as_ref(), 0, ANALYZE_AFTER, 2, true);
    drop(engine);
    std::thread::sleep(Duration::from_secs(12));
    assert_eq!(analyses(&url, &schema), 2, "one inside, one after");
    common::drop_schema(&url, &schema);
}
