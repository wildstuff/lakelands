//! An erased value leaves the projection's file
//! (`docs/decisions/098-a-lake-stores-what-it-cannot-derive.md`,
//! `ERASED_VALUE_LEAVES_THE_FILE`): once a record's states are dropped and
//! the projection is closed, neither the value's bytes nor its search
//! terms are in any page of the file, whichever connection deleted them,
//! the writer's in a scoped pass, a scratch's fold before it is swapped in,
//! or the writer's after the swap. No reclaim runs here, so no `VACUUM`
//! rewrites the file and hides a connection that left its pages as they
//! were; the values the drop kept are found, so the scan reads the pages.

use std::path::{Path, PathBuf};

use lakelands_api::Location;
use lakelands_api::engine::{EngineOpener, LakeEngine, Lease, OpenMode, StatesOf};
use lakelands_api::event::Digest;
use lakelands_api::id::{Position, RecordKey, TypeName};
use lakelands_api::record::StoredRecord;
use lakelands_api::write::Declaration;
use lakelands_core::conformance::engine::chain;
use lakelands_core::conformance::fixture::stored_record;
use lakelands_engine_sqlite::SqliteOpener;
use serde_json::json;

/// The erased value: a word no page holds by chance.
const SECRET: &str = "otterquillvanish";
/// A value the drop keeps, to show the scan reads the pages.
const KEPT: &str = "troutstillhere";
/// Records of each value, more than one page holds.
const EACH: usize = 60;

fn dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("erased-value-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn at(dir: &Path) -> Location {
    Location::sqlite("erased", dir)
}

/// A lake of `events` events, nothing folded, and the writer's lease.
fn lake(dir: &Path, events: u64) -> (Box<dyn LakeEngine>, Lease) {
    let writer = SqliteOpener.open(&at(dir), OpenMode::Create).unwrap();
    let lease = writer.take_lease("writer").unwrap();
    writer
        .append(&lease, &chain(0, events, &Digest::zero(), &lease))
        .unwrap();
    (writer, lease)
}

fn note(key: &str, text: &str, at: u64) -> StoredRecord {
    let fields = json!({"text": text, "n": at});
    stored_record("note", key, serde_json::from_value(fields).unwrap(), at)
}

fn secret_key(i: usize) -> String {
    format!("s{i:03}")
}

/// `note`, its text searchable, and
/// `EACH` notes holding the secret beside `EACH` holding the kept value,
/// folded through `through`.
fn put(engine: &dyn LakeEngine, through: u64) {
    let decl: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "note",
        "fields": [{"name": "text", "type": {"kind": "text"}, "searchable": true},
                   {"name": "n", "type": {"kind": "number"}}]
    }))
    .unwrap();
    let p = engine.projection();
    p.begin().unwrap();
    p.put_declaration(&decl, 1, None, Position(0)).unwrap();
    for i in 0..EACH {
        p.put_record(&note(&secret_key(i), &format!("{SECRET} {i}"), through))
            .unwrap();
        p.put_record(&note(&format!("k{i:03}"), &format!("{KEPT} {i}"), through))
            .unwrap();
    }
    p.commit(Position(through)).unwrap();
}

/// The notes holding the secret dropped, folded through `through`.
fn erase(engine: &dyn LakeEngine, through: u64) {
    let p = engine.projection();
    p.begin().unwrap();
    let ty = TypeName::from("note");
    for i in 0..EACH {
        let key = RecordKey::from(secret_key(i));
        p.drop_states(StatesOf::Record(&ty, &key)).unwrap();
    }
    p.commit(Position(through)).unwrap();
}

/// The journal folded into the file and emptied, as a reclaim leaves it:
/// pages a commit wrote before a drop are in the journal until then.
fn checkpointed(dir: &Path) {
    let conn = rusqlite::Connection::open(dir.join("projection.db")).unwrap();
    let busy: i64 = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))
        .unwrap();
    assert_eq!(busy, 0, "the journal is checkpointed whole");
}

/// What of `word` the projection's files in `dir` hold: the word itself
/// and each term the search index makes of it, one line a find.
fn held(dir: &Path, word: &str) -> Vec<String> {
    let mut needles = vec![word.as_bytes().to_vec()];
    for token in lakelands_core::engines::tokens(word, "") {
        needles.push(lakelands_core::canonical::hex(token.as_bytes()).into_bytes());
    }
    let mut found = Vec::new();
    for name in ["projection.db", "projection.db-wal"] {
        let Ok(bytes) = std::fs::read(dir.join(name)) else {
            continue;
        };
        for needle in &needles {
            let at = bytes.windows(needle.len()).position(|w| w == needle);
            if let Some(at) = at {
                found.push(format!(
                    "{name} holds {:?} at byte {at}",
                    String::from_utf8_lossy(needle)
                ));
            }
        }
    }
    found
}

fn assert_erased(dir: &Path, by: &str) {
    checkpointed(dir);
    let left = held(dir, SECRET);
    assert!(
        left.is_empty(),
        "{by}: the erased value is in the file: {left:?}"
    );
    assert!(
        !held(dir, KEPT).is_empty(),
        "{by}: the value the drop kept is in no page, so the scan read nothing"
    );
}

#[test]
fn an_erased_value_leaves_the_file_by_the_scoped_pass() {
    let dir = dir("scoped");
    let (writer, _lease) = lake(&dir, 8);
    put(writer.as_ref(), 1);
    erase(writer.as_ref(), 2);
    drop(writer);
    assert_erased(&dir, "the writer's scoped pass");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_erased_value_leaves_the_file_by_the_swap() {
    let dir = dir("swap");
    let (writer, lease) = lake(&dir, 5);
    put(writer.as_ref(), 1);
    let built = SqliteOpener.open_scratch(&at(&dir)).unwrap();
    put(built.as_ref(), 2);
    erase(built.as_ref(), 4);
    assert_eq!(built.mark_built().unwrap(), Some(Position(4)));
    drop(built);
    assert_eq!(writer.swap(&lease).unwrap(), Some(Position(4)));
    drop(writer);
    assert_erased(&dir, "a scratch's fold, swapped in");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_erased_value_leaves_the_file_by_the_writer_after_a_swap() {
    let dir = dir("after-swap");
    let (writer, lease) = lake(&dir, 5);
    let built = SqliteOpener.open_scratch(&at(&dir)).unwrap();
    let p = built.projection();
    p.begin().unwrap();
    p.commit(Position(4)).unwrap();
    assert_eq!(built.mark_built().unwrap(), Some(Position(4)));
    drop(built);
    assert_eq!(writer.swap(&lease).unwrap(), Some(Position(4)));
    put(writer.as_ref(), 5);
    erase(writer.as_ref(), 6);
    drop(writer);
    assert_erased(&dir, "the writer's pass after a swap");
    let _ = std::fs::remove_dir_all(&dir);
}
