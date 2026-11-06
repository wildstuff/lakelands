use std::sync::{Arc, Mutex};

use lakelands_api::engine::{LakeEngine, OpenMode, Rankings};
use lakelands_api::id::{Position, RecordKey, TypeName};
use lakelands_api::location::SqliteTuning;
use lakelands_api::record::StoredRecord;
use lakelands_core::conformance::fixture::stored_record;
use rusqlite::trace::{TraceEvent, TraceEventCodes};
use rusqlite::{Error, StatementStatus, ffi, params};

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

/// A projection's write SQLite finds no room for is named as that: the
/// file may grow no further, as on a full disk (lakelands#1568).
#[test]
fn a_projection_write_with_no_room_left_is_named_as_that() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE t (v BLOB); PRAGMA max_page_count = 2;")
        .unwrap();
    let full = conn.execute("INSERT INTO t VALUES (zeroblob(100000))", []);
    let told = super::sql(full.unwrap_err()).to_string();
    assert!(
        told.contains("there is no room left to write the projection"),
        "{told}"
    );
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

/// The state of key `k<at>` that a put at `at` makes.
fn state(at: u64) -> StoredRecord {
    stored_record(
        "t",
        format!("k{at:03}"),
        serde_json::from_value(serde_json::json!({"n": at})).unwrap(),
        at,
    )
}

/// A page under a text more than `FEW_HOLDERS` records hold walks every
/// order row it passes, and runs the count of that walk only while a check
/// asked for walks (`docs/decisions/097-a-read-in-a-window-walks-its-window.md`
/// §What `rows_read` counts, and what is added): the switch keeps a product
/// read from paying a statement a batch.
#[test]
fn a_many_holders_walk_is_counted_only_when_a_check_asks() {
    use lakelands_api::engine::Scan;
    use lakelands_api::read::{Direction, OrderBy};
    use lakelands_api::write::Declaration;
    use lakelands_core::conformance::fixture::order;

    let dir = std::env::temp_dir().join(format!("lakelands-walked-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let tuning = SqliteTuning::default();
    let lake = SqliteLake::open(
        "walked".into(),
        (dir.clone(), dir.clone()),
        tuning,
        OpenMode::Create,
    )
    .unwrap();
    let decl: Declaration = serde_json::from_value(serde_json::json!({
        "declare": "type", "name": "t",
        "fields": [
            {"name": "v", "type": {"kind": "number"}},
            {"name": "s", "type": {"kind": "text"}}
        ]
    }))
    .unwrap();
    let p = lake.projection();
    p.begin().unwrap();
    p.put_declaration(&decl, 1, None, Position(0)).unwrap();
    for i in 0..4_200 {
        p.put_record(&stored_record(
            "t",
            format!("k{i:04}"),
            serde_json::from_value(serde_json::json!({"v": i, "s": "many"})).unwrap(),
            1,
        ))
        .unwrap();
    }
    p.commit(Position(1)).unwrap();
    p.begin().unwrap();
    p.put_record(&stored_record(
        lakelands_api::seam::SETTING_TYPE,
        "sort_index.t",
        serde_json::from_value(serde_json::json!({
            "key": "sort_index.t", "value": ["v"]
        }))
        .unwrap(),
        2,
    ))
    .unwrap();
    p.commit(Position(2)).unwrap();
    p.begin().unwrap();
    p.put_record(&stored_record(
        lakelands_api::seam::SETTING_TYPE,
        "lookup_index.t",
        serde_json::from_value(serde_json::json!({
            "key": "lookup_index.t", "value": ["s"]
        }))
        .unwrap(),
        3,
    ))
    .unwrap();
    p.commit(Position(3)).unwrap();
    let view = lake.view(None).unwrap();
    let filter = lakelands_core::expr::parse("s = 'many'").unwrap();
    let scan = Scan {
        order_by: vec![order("v", OrderBy::Value, Direction::Asc)],
        filter: Some(filter),
        limit: 10,
        ..Scan::default()
    };
    let read = |counting: bool| {
        lakelands_core::engines::count_walks(counting);
        let before = lakelands_core::engines::rows_walked();
        let rows = view.scan(&TypeName::from("t"), &scan).unwrap();
        let walked = lakelands_core::engines::rows_walked() - before;
        lakelands_core::engines::count_walks(false);
        (rows.len(), walked)
    };
    let (rows, walked) = read(false);
    assert_eq!((rows, walked), (10, 0), "no check asked for walks");
    let (rows, walked) = read(true);
    assert_eq!(
        (rows, walked),
        (10, 10),
        "a check asked: the count of the range walked is the page"
    );
    drop(view);
    drop(lake);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A statement the engine keeps prepared is compiled once: taken from the
/// connection's cache again and run with other values, it keeps its
/// plan. On SQLite 3.54 a kept statement with an equality on a column its
/// index does not consume was compiled again at every run, once the cache
/// had cleared its values, which made a rebuild, a full-text search and a
/// filtered `nearest()` 14 to 30 % slower (lakelands#1378). Counted here
/// in SQLite's own count of a statement's recompilations, for the two
/// statements a fold and a read at a position run once for each record,
/// each run as the engine runs it, on a connection the engine makes, for
/// every key at its own position (a put's ending by the key's id); the put's ending statement is rolled
/// back.
#[test]
fn a_kept_statement_run_again_with_other_values_is_not_compiled_again() {
    const RECORDS: u64 = 100;
    let dir = std::env::temp_dir().join(format!("lakelands-kept-plan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let tuning = SqliteTuning::default();
    let dirs = (dir.clone(), dir.clone());
    let lake = SqliteLake::open("kept".into(), dirs, tuning, OpenMode::Create).unwrap();
    let p = lake.projection();
    p.begin().unwrap();
    for at in 1..=RECORDS {
        p.put_record(&state(at)).unwrap();
    }
    p.commit(Position(RECORDS)).unwrap();
    drop(lake);
    let path = dir.join("projection.db");
    let mut conn = crate::handle::connect(&path, &tuning, true).unwrap();
    let tx = conn.transaction().unwrap();
    for at in 1..=RECORDS {
        let [read, ending] = crate::seek_statements();
        let mut s = tx.prepare_cached(&read).unwrap();
        let mut rows = s
            .query(params!["t", format!("k{at:03}"), at as i64])
            .unwrap();
        while rows.next().unwrap().is_some() {}
        drop(rows);
        let mut s = tx.prepare_cached(&ending).unwrap();
        s.execute(params![at as i64, at as i64]).unwrap();
    }
    let compiled: Vec<i32> = crate::seek_statements()
        .iter()
        .map(|s| tx.prepare_cached(s).unwrap())
        .map(|s| s.get_status(StatementStatus::RePrepare))
        .collect();
    drop(tx);
    drop(conn);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        compiled,
        [0, 0],
        "the recompilations of the read of one record at a position and of a put's ending \
         statement, each run for {RECORDS} keys"
    );
}

/// Every connection that may delete overwrites what it deletes, and a new
/// file holds pages of 4 KiB and incremental vacuum
/// (`ERASED_VALUE_LEAVES_THE_FILE`): a writer's connection, which the
/// writer, a scratch's fold and the writer after a swap each make, reads
/// `secure_delete` on, which no file holds for the next connection; a
/// reader's deletes nothing and is left as SQLite makes it.
#[test]
fn a_connection_that_deletes_overwrites_what_it_deletes() {
    let dir = std::env::temp_dir().join(format!("lakelands-secure-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("projection.db");
    let tuning = SqliteTuning::default();
    let read = |conn: &rusqlite::Connection, pragma: &str| -> i64 {
        conn.query_row(&format!("PRAGMA {pragma}"), [], |r| r.get(0))
            .unwrap()
    };
    let writer = crate::handle::connect(&path, &tuning, true).unwrap();
    writer.execute_batch("CREATE TABLE t (x)").unwrap();
    let settings = |conn| {
        (
            read(conn, "secure_delete"),
            read(conn, "page_size"),
            read(conn, "auto_vacuum"),
        )
    };
    assert_eq!(settings(&writer), (1, 4096, 2), "the writer's connection");
    let again = crate::handle::connect(&path, &tuning, true).unwrap();
    assert_eq!(
        settings(&again),
        (1, 4096, 2),
        "a second writer's connection"
    );
    let plain = rusqlite::Connection::open(&path).unwrap();
    assert_eq!(
        read(&plain, "secure_delete"),
        0,
        "the file holds no `secure_delete`: only the connection's setup sets it"
    );
    drop((writer, again, plain));
    let _ = std::fs::remove_dir_all(&dir);
}
