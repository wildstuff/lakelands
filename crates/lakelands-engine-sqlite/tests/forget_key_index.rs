//! A read of the feed index by a key and a type takes the key's index, not
//! the type's: a scoped forget names the keys of one source, and the type
//! holds most of the lake's routes (lakelands#1659).

use std::path::PathBuf;

use lakelands_api::Location;
use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::id::Position;
use lakelands_core::conformance::fixture::stored_record;
use lakelands_engine_sqlite::{SqliteOpener, keyed_routes_statement};
use serde_json::json;

#[test]
fn a_keyed_read_of_the_feed_takes_the_key_index_where_the_type_is_the_less_selective_column() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("forget-key-index-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let engine = SqliteOpener
        .open(&Location::sqlite("forget", &dir), OpenMode::Create)
        .unwrap();
    // One type for every record, a key for each.
    for first in (1..=20_000u64).step_by(500) {
        let p = engine.projection();
        p.begin().unwrap();
        for at in first..first + 500 {
            let record = stored_record(
                "t",
                format!("k{at:05}"),
                serde_json::from_value(json!({"n": at})).unwrap(),
                at,
            );
            p.put_record(&record).unwrap();
        }
        p.commit(Position(first + 499)).unwrap();
    }
    drop(engine);
    let db = rusqlite::Connection::open(dir.join("projection.db")).unwrap();
    let mut plan = db
        .prepare(&format!("EXPLAIN QUERY PLAN {}", keyed_routes_statement()))
        .unwrap();
    let details: Vec<String> = plan
        .query_map(rusqlite::params_from_iter([1, 1, 1, 1, 1]), |r| {
            r.get::<_, String>(3)
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        details.iter().any(|d| d.contains("routes_key")),
        "the plan takes the key index: {details:?}"
    );
    assert!(
        !details.iter().any(|d| d.contains("routes_ty")),
        "the plan does not take the type index: {details:?}"
    );
}
