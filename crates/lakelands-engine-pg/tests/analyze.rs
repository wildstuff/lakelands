//! After a load, the writer analyses its tables: once `ANALYZE_AFTER`
//! records have gone in since the last analysis, and not before
//! (`src/handle.rs`, "Statistics after a load"). The analysis is read
//! where the server records it, `pg_stat_user_tables.last_analyze`, which
//! PostgreSQL's own background analysis never sets.

mod common;

use std::time::{Duration, Instant};

use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode};
use lakelands_api::id::{Position, RecordKey, TypeName, Version};
use lakelands_api::record::StoredRecord;
use lakelands_api::write::Declaration;
use lakelands_engine_pg::{ANALYZE_AFTER, PgOpener};
use serde_json::json;

/// Whether the server has recorded an `ANALYZE` of the lake's `records`,
/// asked on a connection of the test's own.
fn analysed(url: &str, schema: &str) -> bool {
    let (url, schema) = (url.to_owned(), schema.to_owned());
    std::thread::spawn(move || {
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        client
            .query_one(
                "SELECT last_analyze IS NOT NULL FROM pg_stat_user_tables \
                 WHERE schemaname = $1 AND relname = 'records'",
                &[&schema],
            )
            .unwrap()
            .get(0)
    })
    .join()
    .unwrap()
}

/// Fold `n` records, from key `from`, in one transaction committed
/// through `at`.
fn fold(engine: &dyn LakeEngine, from: u64, n: u64, at: u64) {
    let p = engine.projection();
    p.begin().unwrap();
    for i in from..from + n {
        p.put_record(&StoredRecord {
            ty: TypeName::from("t"),
            key: RecordKey::from(format!("k{i:06}")),
            version: Version(1),
            fields: serde_json::from_value(json!({"v": format!("v{i}")})).unwrap(),
            meta: Default::default(),
            position: Position(at),
            retired: false,
            restricted: false,
        })
        .unwrap();
    }
    p.commit(Position(at)).unwrap();
}

#[test]
fn the_writer_analyses_its_tables_once_enough_records_went_in_and_not_before() {
    let Some(url) = common::url_or_skip("the pg analysis test") else {
        return;
    };
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(
            &common::location(&url, "analysed", &schema),
            OpenMode::Create,
        )
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
    // One record short of the threshold, in windows as a load commits them.
    let below = ANALYZE_AFTER - 1;
    let mut put = 0;
    let mut at = 1;
    while put < below {
        let n = (below - put).min(1000);
        at += 1;
        fold(engine.as_ref(), put, n, at);
        put += n;
    }
    // The server records an analysis when its backend next reports, at
    // most about ten seconds after going idle.
    std::thread::sleep(Duration::from_secs(12));
    assert!(
        !analysed(&url, &schema),
        "{below} records went in, one fewer than {ANALYZE_AFTER}, and the tables were analysed"
    );
    at += 1;
    fold(engine.as_ref(), put, 1, at);
    let asked = Instant::now();
    while !analysed(&url, &schema) {
        assert!(
            asked.elapsed() < Duration::from_secs(30),
            "{ANALYZE_AFTER} records went in and the tables were not analysed"
        );
        std::thread::sleep(Duration::from_millis(500));
    }
    drop(engine);
    common::drop_schema(&url, &schema);
}
