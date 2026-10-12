//! A put ends the latest earlier state of its key, and a read of one
//! record at a position takes the latest state at or before it: neither
//! reads the key's other states (lakelands#596). The engine check holds
//! the answers. What a put and a read cost is counted here, where the
//! server counts it: the tuples of the lake's `records` it fetched
//! (`pg_stat_user_tables`), which a backend reports when it ends.

mod common;

use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::id::{Position, RecordKey, TypeName, Version};
use lakelands_api::record::StoredRecord;
use lakelands_engine_pg::PgOpener;
use serde_json::json;

/// The states `a` has before the puts and reads that are counted, and
/// the other records of its type, each written once: a much-written key
/// is one among many, and a planner that sees a table of one key reads
/// it whole whatever the statement says.
const STATES: u64 = 5_000;
const OTHERS: u64 = 20_000;

fn state(key: &str, at: u64) -> StoredRecord {
    StoredRecord {
        ty: TypeName::from("t"),
        key: RecordKey::from(key),
        version: Version(1),
        fields: serde_json::from_value(json!({"n": at})).unwrap(),
        meta: Default::default(),
        position: Position(at),
        retired: false,
        restricted: false,
    }
}

#[test]
fn a_put_and_a_read_of_a_much_written_key_fetch_a_few_tuples() {
    let Some(url) = common::url_or_skip("the pg test of a much-written key") else {
        return;
    };
    let schema = common::fresh_schema();
    let open = |mode| {
        PgOpener
            .open(&common::location(&url, "written", &schema), mode)
            .unwrap()
    };
    // The other records and then the history, two hundred puts a
    // transaction; then the engine is closed, so that its backends report
    // what they fetched.
    let engine = open(OpenMode::Create);
    for first in (1..=OTHERS + STATES).step_by(200) {
        let p = engine.projection();
        p.begin().unwrap();
        for at in first..first + 200 {
            let other = format!("k{at:05}");
            let key = if at > OTHERS { "a" } else { other.as_str() };
            p.put_record(&state(key, at)).unwrap();
        }
        p.commit(Position(first + 199)).unwrap();
    }
    drop(engine);
    let before = common::fetched(&url, &schema, "records");
    // A hundred puts, a transaction each, and after each a read of the
    // record at the head and at an earlier position.
    let engine = open(OpenMode::Write);
    let (ty, key) = (TypeName::from("t"), RecordKey::from("a"));
    for at in OTHERS + STATES + 1..=OTHERS + STATES + 100 {
        let p = engine.projection();
        p.begin().unwrap();
        p.put_record(&state("a", at)).unwrap();
        p.commit(Position(at)).unwrap();
        for as_of in [None, Some(Position(at - STATES / 2))] {
            let read = engine.view(as_of).unwrap().get(&ty, &key).unwrap();
            let n = read.and_then(|r| r.fields.get("n").cloned());
            assert_eq!(n, Some(json!(as_of.map_or(at, |p| p.0))), "as of {as_of:?}");
        }
    }
    drop(engine);
    let each = (common::fetched(&url, &schema, "records") - before) / 100;
    common::drop_schema(&url, &schema);
    assert!(
        each < 50,
        "a put and two reads of a key with {STATES} states fetched {each} tuples of `records`"
    );
}
