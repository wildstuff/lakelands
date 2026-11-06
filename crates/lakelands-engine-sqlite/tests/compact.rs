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
use lakelands_api::id::Position;
use lakelands_core::conformance::fixture::stored_record;
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
        p.put_record(&stored_record(
            "note",
            format!("n{i}"),
            serde_json::from_value(fields).unwrap(),
            at,
        ))
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

    let reclaimed = engine
        .projection()
        .compact_step(lakelands_api::engine::Compact::Whole)
        .unwrap();
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

/// An act's reclaim in steps: the journal truncated, then free pages a
/// bounded number at a time, each step saying whether any are left, and
/// the last leaving none and an empty journal (099, R6).
#[test]
fn a_reclaim_in_steps_gives_back_every_free_page_a_bound_at_a_time() {
    use lakelands_api::engine::Compact;
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("compact-steps-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let engine = SqliteOpener
        .open(&Location::sqlite("steps", &dir), OpenMode::Create)
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
    let p = engine.projection();
    let journal = p.compact_step(Compact::Journal).unwrap();
    assert!(journal.more, "the clear left free pages: {journal:?}");
    assert_eq!(len(&wal), 0, "the journal is truncated first");
    let (left, mut steps) = (free(), 0);
    // A bound of 64 pages a step.
    while p.compact_step(Compact::Pages(64 * 4096)).unwrap().more {
        steps += 1;
        assert!(steps < 10_000, "every step gives pages back");
    }
    assert!(
        left > 64 && steps >= 1,
        "{left} free pages took {steps} steps and the last"
    );
    assert_eq!(free(), 0, "no free page is left in projection.db");
    assert_eq!(len(&wal), 0, "the last step truncates the journal again");
    assert_eq!(
        p.compact_step(Compact::Beside).unwrap().before,
        None,
        "nothing runs beside"
    );
    drop(engine);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A `projection.db` made without incremental vacuum, as one made before
/// it was the default, which the engine leaves as it is: a pages step
/// gives nothing back and says so, so an act's reclaim ends (099, R6).
#[test]
fn a_reclaim_in_steps_ends_on_a_file_without_incremental_vacuum() {
    use lakelands_api::engine::Compact;
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("compact-none-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let at = Location::sqlite("none", &dir);
    drop(SqliteOpener.open(&at, OpenMode::Create).unwrap());
    let db = dir.join("projection.db");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch("PRAGMA auto_vacuum = NONE; VACUUM;")
        .unwrap();
    drop(conn);
    let engine = SqliteOpener.open(&at, OpenMode::Write).unwrap();
    fold(engine.as_ref(), 2000, 1);
    engine.projection().clear().unwrap();
    fold(engine.as_ref(), 10, 2);
    let p = engine.projection();
    assert!(
        p.compact_step(Compact::Journal).unwrap().more,
        "the clear left free pages"
    );
    let mut steps = 0;
    while p.compact_step(Compact::Pages(64 * 4096)).unwrap().more {
        steps += 1;
        assert!(steps < 50, "a step that gives nothing back is the last");
    }
    assert_eq!(steps, 0, "no step gave pages back");
    drop(engine);
    let _ = std::fs::remove_dir_all(&dir);
}
