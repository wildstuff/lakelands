use std::sync::{Arc, Mutex};

use lakelands_api::engine::{LakeEngine, OpenMode, Rankings};
use lakelands_api::id::{Position, RecordKey, TypeName};
use rusqlite::trace::{TraceEvent, TraceEventCodes};
use rusqlite::{Error, ffi};

use crate::handle::SqliteLake;

/// Neither a driver error nor a failure SQLite reports quotes a statement
/// or a value (lakelands#739).
#[test]
fn a_sqlite_error_says_no_statement_and_no_value() {
    let (sql, msg) = ("SELECT 'a secret'".into(), "near \"a secret\"".into());
    let input = Error::SqlInputError {
        error: ffi::Error::new(1),
        msg,
        sql,
        offset: 7,
    };
    let unique = ffi::Error::new(ffi::SQLITE_CONSTRAINT_UNIQUE);
    let failure = Error::SqliteFailure(unique, Some("UNIQUE constraint failed: t.k".into()));
    let value = Error::IntegralValueOutOfRange(0, 9_007_199_254_740_993);
    let says = [
        ": IntegralValueOutOfRange",
        ": SqlInputError",
        ": SQLite 2067",
    ];
    for (e, says) in [value, input, failure].into_iter().zip(says) {
        let told = super::sql(e).to_string();
        let quotes = told.contains("9007199254740993") || told.contains("secret");
        assert!(told.contains(says) && !quotes, "answered {told}");
    }
}

/// What the traced connection runs once, when its read of the folded
/// position has finished.
static AFTER_FOLDED: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);

fn after_folded(event: TraceEvent<'_>) {
    if let TraceEvent::Profile(statement, _) = event
        && statement.sql().contains("'folded'")
        && let Some(write) = AFTER_FOLDED.lock().unwrap().take()
    {
        write();
    }
}

/// A fold of one vector, (1, 0) at `key`, through `through`.
fn fold(lake: &SqliteLake, key: &str, through: u64) {
    let p = lake.projection();
    p.begin().unwrap();
    let index = lake.vectors().expect("the engine keeps vectors");
    index
        .put(&"t".into(), &key.into(), "e", &[1.0, 0.0])
        .unwrap();
    p.commit(Position(through)).unwrap();
}

/// `ranked` reads its position and its lists in one read transaction
/// (`docs/seams.md` §`ranked`, lakelands#826). A reader in WAL mode never
/// waits for a writer, so no other connection can hold one call between
/// two of its statements; the reader's own trace does. When the call's
/// read of the folded position has finished, a fold commits from the
/// writer's connection. The vector's list, read after that commit, does
/// not hold the fold's vector, and the position is the one before it. The
/// call after it holds both.
#[test]
fn a_fold_committed_inside_one_ranked_call_is_in_none_of_its_lists() {
    let dir = std::env::temp_dir().join(format!("lakelands-ranked-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let tuning = Default::default();
    let dirs = (dir.clone(), dir.clone());
    let lake = SqliteLake::open("ranked".into(), dirs, tuning, OpenMode::Create).unwrap();
    let lake = Arc::new(lake);
    fold(&lake, "a", 1);
    let writer = lake.clone();
    *AFTER_FOLDED.lock().unwrap() = Some(Box::new(move || fold(&writer, "b", 2)));
    // On the one idle reader, which the call takes.
    lake.reader()
        .unwrap()
        .trace_v2(TraceEventCodes::SQLITE_TRACE_PROFILE, Some(after_folded))
        .unwrap();
    let asked = Rankings {
        vectors: vec![("e".into(), vec![1.0, 0.0])],
        ..Rankings::default()
    };
    let read = || {
        let ranked = lake.ranked(&TypeName::from("t"), &asked, 10).unwrap();
        let keys = |l: &Vec<(RecordKey, f64)>| -> Vec<String> {
            l.iter().map(|(k, _)| k.to_string()).collect()
        };
        (
            ranked.position,
            ranked.lists.iter().map(keys).collect::<Vec<_>>(),
        )
    };
    let inside = read();
    assert!(
        AFTER_FOLDED.lock().unwrap().is_none(),
        "the fold committed inside the call"
    );
    assert_eq!(inside, (Some(Position(1)), vec![vec!["a".to_owned()]]));
    let after = read();
    assert_eq!(
        after,
        (
            Some(Position(2)),
            vec![vec!["a".to_owned(), "b".to_owned()]]
        )
    );
    drop(lake);
    let _ = std::fs::remove_dir_all(&dir);
}
