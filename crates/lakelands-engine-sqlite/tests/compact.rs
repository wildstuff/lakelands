//! What a compact does to the SQLite engine's storage
//! (`docs/decisions/040-maintenance-never-changes-an-answer.md`).
//!
//! The conformance suite holds that a compact changes no answer; the
//! space it gives back is this engine's own measure, held only here.
//! After a clear frees the projection's pages, a compact leaves
//! `projection.db` without a free page, the file and its journal smaller
//! than they were, and the journal empty. The engine is driven directly:
//! the refold of a rebuild takes the freed pages up again, so the store's
//! rebuild would leave little for the check to see.

use std::path::{Path, PathBuf};

use lakelands_api::Location;
use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode};
use lakelands_api::id::{Position, RecordKey, TypeName, Version};
use lakelands_api::record::StoredRecord;
use lakelands_engine_sqlite::SqliteOpener;
use serde_json::json;

fn len(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}

/// Fold `n` records of a type through position `at`, in one transaction.
fn fold(engine: &dyn LakeEngine, n: u64, at: u64) {
    let p = engine.projection();
    p.begin().unwrap();
    for i in 0..n {
        let fields = json!({"text": format!("record {i}, with text enough to fill pages {}", "x".repeat(200))});
        p.put_record(&StoredRecord {
            ty: TypeName::from("note"),
            key: RecordKey::from(format!("n{i}")),
            version: Version(1),
            fields: serde_json::from_value(fields).unwrap(),
            meta: Default::default(),
            position: Position(at),
            made: Position(at),
            retired: false,
            restricted: false,
        })
        .unwrap();
    }
    p.commit(Position(at)).unwrap();
}

#[test]
fn a_compact_after_a_clear_leaves_no_free_page_and_an_empty_journal() {
    let dir =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("compact-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let engine = SqliteOpener
        .open(&Location::sqlite("tidy", &dir), OpenMode::Create)
        .unwrap();
    fold(engine.as_ref(), 2000, 1);
    engine.projection().clear().unwrap();
    fold(engine.as_ref(), 10, 2);

    let (db, wal) = (dir.join("projection.db"), dir.join("projection.db-wal"));
    let free = || -> i64 {
        rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap()
            .query_row("PRAGMA freelist_count", [], |r| r.get(0))
            .unwrap()
    };
    assert!(free() > 0, "the clear left free pages to give back");
    let stored = len(&db) + len(&wal);

    let reclaimed = engine.projection().compact().unwrap();
    let (before, after) = (reclaimed.before.unwrap(), reclaimed.after.unwrap());
    assert_eq!(before, stored, "before is the file and its journal");
    assert_eq!(free(), 0, "no free page is left in projection.db");
    assert_eq!(len(&wal), 0, "the journal is truncated");
    assert_eq!(after, len(&db), "after is the file and its empty journal");
    assert!(after < before, "{reclaimed:?}");
    assert_eq!(
        engine.projection().folded_through().unwrap(),
        Some(Position(2)),
        "the projection still reflects the log as far as it did"
    );
    drop(engine);
    let _ = std::fs::remove_dir_all(&dir);
}
