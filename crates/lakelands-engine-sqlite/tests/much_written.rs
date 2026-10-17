//! A put ends the latest earlier state of its key, and a read of one
//! record at a position takes the latest state at or before it: neither
//! reads the key's other states (lakelands#596). The engine check holds
//! the answers. What the two statements cost is counted here, as the
//! engine itself writes them, in the steps SQLite's virtual machine
//! takes: for a key with five thousand states no more than for a key
//! written once, where a statement that ranges over a key's states takes
//! some steps for each of them.

use std::path::PathBuf;

use lakelands_api::Location;
use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::id::{Position, RecordKey, TypeName, Version};
use lakelands_api::record::StoredRecord;
use lakelands_engine_sqlite::{SqliteOpener, seek_statements};
use rusqlite::StatementStatus;
use serde_json::json;

/// The states of the much-written key `a`, and the other records of its
/// type, each written once.
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
        made: Position(at),
        retired: false,
        restricted: false,
    }
}

#[test]
fn a_put_and_a_read_of_a_much_written_key_take_the_steps_of_a_key_written_once() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("much-written-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let engine = SqliteOpener
        .open(&Location::sqlite("written", &dir), OpenMode::Create)
        .unwrap();
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
    // Each statement as the engine writes it, for `a` and for a key written
    // once, at the head and at a position in the middle of `a`'s states;
    // the put's is rolled back.
    let mut db = rusqlite::Connection::open(dir.join("projection.db")).unwrap();
    let (head, middle) = (OTHERS + STATES + 1, OTHERS + STATES / 2);
    let mut steps = Vec::new();
    for statement in seek_statements() {
        for at in [head, middle] {
            let tx = db.transaction().unwrap();
            let mut taken = Vec::new();
            for key in ["a", "k00001"] {
                let mut s = tx.prepare(&statement).unwrap();
                let mut rows = s.query(rusqlite::params!["t", key, at as i64]).unwrap();
                while rows.next().unwrap().is_some() {}
                drop(rows);
                taken.push(s.get_status(StatementStatus::VmStep));
            }
            drop(tx);
            steps.push((taken[0], taken[1]));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        steps.iter().all(|(much, once)| *much <= once + 10),
        "the steps of the read of one record and of a put's ending statement, each at the head \
         and in the middle of the key's states, for a key with {STATES} states and for a key \
         written once: {steps:?}"
    );
}
