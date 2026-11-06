//! A view's kept records bear a name of the kept form
//! (`lakelands_api::seam::KEPT_PREFIX`), and a kept record is put again at
//! every write to its group: a put under such a name deletes the states
//! its key held, so a key of such a name keeps one state. The engine
//! reads such a name by statements of its own (lakelands#575): at the
//! head the states with no end; at an earlier position a step from key to
//! key and a seek of each key's state there, a key put after the position
//! answering none. The engine check holds the answers. Here each
//! statement's plan is read, as the engine itself writes it, and what a
//! read costs is counted in the steps SQLite's virtual machine takes: for
//! a name whose key was written five thousand times no more than for a
//! name whose keys were written once. A recorded output is read the same
//! way, by a seek. A
//! view's order (`lakelands_api::seam::ORDER_PREFIX`, lakelands#1690) is
//! kept in the `view_order` table, one entry for each key and no row of
//! the record tables: an entry is read by its key, and a page after a key
//! in key order.

use std::path::PathBuf;

use lakelands_api::Location;
use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::event::Digest;
use lakelands_api::id::{Position, RecordKey, TypeName};
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::{kept_name, order_name};
use lakelands_core::conformance::fixture::stored_record;
use lakelands_engine_sqlite::{SqliteOpener, order_statements, range_statements, read_statements};
use rusqlite::types::Value as Sql;
use rusqlite::{Connection, StatementStatus, params_from_iter};
use serde_json::json;

/// The states of the much-written key `a` and of the much-recorded
/// output, the keys each name holds beside it, and the records of a
/// stored type written before them.
const STATES: u64 = 5_000;
const KEYS: u64 = 30;
const OTHERS: u64 = 20_000;

fn state(ty: &TypeName, key: &str, at: u64) -> StoredRecord {
    stored_record(
        ty.clone(),
        key,
        serde_json::from_value(json!({"n": at})).unwrap(),
        at,
    )
}

/// A new lake's projection, opened beside the engine.
fn lake(name: &str) -> (PathBuf, Box<dyn lakelands_api::engine::LakeEngine>) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("kept-name-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let engine = SqliteOpener
        .open(&Location::sqlite("kept", &dir), OpenMode::Create)
        .unwrap();
    (dir, engine)
}

/// The plan of a statement, a line for each step.
fn plan(db: &Connection, statement: &str) -> Vec<String> {
    let mut explain = db
        .prepare(&format!("EXPLAIN QUERY PLAN {statement}"))
        .unwrap();
    let unbound = vec![rusqlite::types::Null; explain.parameter_count()];
    explain
        .query_map(params_from_iter(unbound), |row| row.get(3))
        .and_then(Iterator::collect)
        .unwrap()
}

#[test]
fn every_statement_of_a_kept_name_is_planned_as_a_seek() {
    let (dir, engine) = lake("plans");
    drop(engine);
    let db = Connection::open(dir.join("projection.db")).unwrap();
    let kept = kept_name(&TypeName::from("v"));
    let has = |plan: &[String], step: &str| plan.iter().any(|s| s.contains(step));
    // At the head: the keys in their order, through `keys`, and each
    // one's state with no end by its number, through `records_at`, and
    // nothing else.
    let [first, later, count, output] = read_statements(&kept, true);
    for statement in [&first, &later, &count] {
        let plan = plan(&db, statement);
        assert!(
            plan.len() == 2
                && plan[0].starts_with("SEARCH k USING PRIMARY KEY (ty=? AND key>")
                && plan[1] == "SEARCH r USING INDEX records_at (ty=? AND until=? AND id=?)",
            "a kept name's read at the head does not seek the keys in their order through \
             `keys` and each one's state with no end through `records_at`:\n  {statement}\n  \
             {plan:#?}"
        );
    }
    // The output: the latest state at or before the position, the first
    // row of a seek into the primary key read from the position down. The
    // plan of a range over the states names the same index: what tells
    // them apart is the limit, and the steps counted below.
    let seek = plan(&db, &output);
    assert!(
        has(
            &seek,
            "SEARCH outputs USING INDEX sqlite_autoindex_outputs_1 (seam=? AND input=? AND since<?)"
        ) && !has(&seek, "TEMP B-TREE")
            && output.contains("ORDER BY since DESC LIMIT 1"),
        "the read of a recorded output is not a seek of its latest state:\n  {output}\n  {seek:#?}"
    );
    // At an earlier position: the keys in their order, through `keys`;
    // each key's state by a seek into the primary key from the position
    // down, which covers it; its row by its id. The keys are the outer
    // loop of the join and nothing sorts, so the rows come in the keys'
    // order.
    let [first, later, count, _] = read_statements(&kept, false);
    for statement in [&first, &later, &count] {
        let plan = plan(&db, statement);
        let want = [
            "SEARCH k USING PRIMARY KEY (ty=? AND key>?)",
            "SEARCH r USING INTEGER PRIMARY KEY (rowid=?)",
            "CORRELATED SCALAR SUBQUERY 1",
            "SEARCH records USING COVERING INDEX sqlite_autoindex_records_1 (id=? AND since<?)",
        ];
        assert!(
            plan == want,
            "a kept name's read at an earlier position does not step from key to key through \
             `keys` and seek each key's state, in the order of the keys:\n  {statement}\n  \
             {plan:#?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A read of a kept name inside a key interval is a seek of `keys`'
/// primary key inside the interval as well
/// (`docs/decisions/103-a-filter-on-a-kept-view-narrows-by-the-group-key.md`):
/// the interval's ends bound the same key the walk starts from, and no
/// plan walks the name's keys or sorts them.
#[test]
fn a_kept_name_read_inside_an_interval_seeks_the_keys_inside_it() {
    let (dir, engine) = lake("range-plans");
    drop(engine);
    let db = Connection::open(dir.join("projection.db")).unwrap();
    let kept = kept_name(&TypeName::from("v"));
    let has = |plan: &[String], step: &str| plan.iter().any(|s| s.contains(step));
    for head in [true, false] {
        for statement in range_statements(&kept, head) {
            let plan = plan(&db, &statement);
            assert!(
                plan[0].starts_with("SEARCH k USING PRIMARY KEY (ty=? AND key")
                    && plan[0].contains("key<?")
                    && !has(&plan, "SCAN k")
                    && !has(&plan, "TEMP B-TREE"),
                "a kept name's read inside an interval does not seek `keys`' primary key inside \
                 it, at the head ({head}):\n  {statement}\n  {plan:#?}"
            );
        }
    }
    // At the head the interval is the whole statement's first step, with
    // each key's state with no end through `records_at`.
    let [page, _] = range_statements(&kept, true);
    let plan = plan(&db, &page);
    assert_eq!(
        plan.len(),
        2,
        "a kept name's read inside an interval at the head takes more than the key's seek and \
         its state's:\n  {page}\n  {plan:#?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// An order's entry and its page are seeks of the `view_order` table's
/// primary key, the page in the key order that key keeps, so nothing
/// sorts: the entries stand apart from the records and cost a seek each.
#[test]
fn an_orders_entry_and_page_are_planned_as_a_seek() {
    let (dir, engine) = lake("order-plans");
    drop(engine);
    let db = Connection::open(dir.join("projection.db")).unwrap();
    let [at, page] = order_statements();
    for (statement, step) in [
        (
            &at,
            "SEARCH view_order USING PRIMARY KEY (name=? AND key=?)",
        ),
        (
            &page,
            "SEARCH view_order USING PRIMARY KEY (name=? AND key>?)",
        ),
    ] {
        let plan = plan(&db, statement);
        assert!(
            plan.len() == 1 && plan[0] == step,
            "an order's read is not one seek of the `view_order` key, with nothing sorting \
             its page:\n  {statement}\n  {plan:#?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The steps a statement takes with these arguments.
fn steps(db: &Connection, statement: &str, args: &[Sql]) -> i32 {
    let mut s = db.prepare(statement).unwrap();
    let mut rows = s
        .query(params_from_iter(&args[..s.parameter_count()]))
        .unwrap();
    while rows.next().unwrap().is_some() {}
    drop(rows);
    s.get_status(StatementStatus::VmStep)
}

#[test]
fn a_read_of_a_kept_name_takes_the_steps_of_one_whose_keys_were_written_once() {
    let (dir, engine) = lake("steps");
    let stored = TypeName::from("t");
    let names = ["much", "once"].map(|v| kept_name(&TypeName::from(v)));
    let seam = "sys:kept";
    // The stored type's records, then each name's keys once, then the
    // history of `a` under the first name and of that name's output.
    let last = OTHERS + KEYS + STATES;
    for first in (1..=last).step_by(200) {
        let p = engine.projection();
        p.begin().unwrap();
        for at in first..(first + 200).min(last + 1) {
            if at <= OTHERS {
                p.put_record(&state(&stored, &format!("k{at:05}"), at))
                    .unwrap();
                continue;
            }
            let n = at - OTHERS;
            let key = if n <= KEYS {
                format!("k{n:02}")
            } else {
                "a".to_owned()
            };
            let recorded = |name: &str| {
                p.put_output(seam, &Digest(name.into()), &json!({"at": at}), Position(at))
                    .unwrap();
            };
            p.put_record(&state(&names[0], &key, at)).unwrap();
            recorded("much");
            if n <= KEYS + 1 {
                p.put_record(&state(&names[1], &key, at)).unwrap();
            }
            if n == 1 {
                recorded("once");
            }
        }
        p.commit(Position((first + 199).min(last))).unwrap();
    }
    // What the engine answers, before its statements are counted: every
    // key at the head, and the keys written once before `middle` alone
    // there, in key order, since `a` was put again after it.
    let middle = OTHERS + KEYS;
    for (as_of, at, n, held) in [
        (None, last, last, KEYS + 1),
        (Some(Position(middle)), middle, OTHERS + 1, KEYS),
    ] {
        let view = engine.view(as_of).unwrap();
        let scan = lakelands_api::engine::Scan {
            limit: 100,
            ..Default::default()
        };
        let page = view.scan(&names[0], &scan).unwrap();
        let keys: Vec<&str> = page.iter().map(|r| r.key.as_str()).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(keys, sorted, "as of {as_of:?}");
        assert_eq!(
            (
                keys.len() as u64,
                page[0].fields.get("n"),
                view.count(&names[0], &scan).unwrap()
            ),
            (held, Some(&json!(n)), held),
            "as of {as_of:?}"
        );
        let output = (view.output(seam, &Digest("much".into())).unwrap()).map(|(_, v)| v);
        assert_eq!(output, Some(json!({"at": at})), "as of {as_of:?}");
    }
    drop(engine);
    let db = Connection::open(dir.join("projection.db")).unwrap();
    let mut taken = Vec::new();
    for (head, at) in [(true, last), (false, middle)] {
        // Each statement the engine names for the kept name: a first page
        // of ten, a page of ten after the first key, the count, and the
        // output's read.
        let each = names.each_ref().map(|name| {
            let [first, later, count, output] = read_statements(name, head);
            let input = name.as_str().rsplit('.').next().unwrap().to_owned();
            let (name, at) = (Sql::from(name.to_string()), Sql::from(at as i64));
            let page = [name.clone(), at.clone(), 10.into(), "a".to_owned().into()];
            let flags = [name, at.clone(), 1.into(), 1.into()];
            let recorded = [seam.to_owned().into(), input.into(), at];
            [
                steps(&db, &first, &page),
                steps(&db, &later, &page),
                steps(&db, &count, &flags),
                steps(&db, &output, &recorded),
            ]
        });
        taken.push((head, each[0], each[1]));
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        taken
            .iter()
            .all(|(_, much, once)| much.iter().zip(once).all(|(m, o)| *m <= o + 10)),
        "the steps of a first page, a later page, a count and the read of the recorded output, \
         at the head (true) and in the middle of the key's states, for a kept name with a key \
         of {STATES} states and an output recorded as often, and for one whose keys and output \
         were written once: {taken:?}"
    );
}

#[test]
fn an_order_entry_is_kept_apart_from_the_records() {
    let (dir, engine) = lake("order");
    let name = order_name(&TypeName::from("v"));
    let key = |at: u64| RecordKey::from(format!("k{at:02}").as_str());
    let p = engine.projection();
    p.begin().unwrap();
    for at in 1..=40 {
        let entry = format!(r#"{{"v":"{at}"}}"#);
        p.put_order(&name, &key(at), Some(entry.as_bytes()))
            .unwrap();
    }
    // A key put again replaces its entry.
    p.put_order(&name, &key(7), Some(br#"{"v":"7b"}"#)).unwrap();
    p.commit(Position(41)).unwrap();
    let view = engine.view(None).unwrap();
    assert_eq!(
        view.order(&name, &key(7)).unwrap().as_deref(),
        Some(&br#"{"v":"7b"}"#[..]),
        "an entry is read by its key"
    );
    let page = view.order_page(&name, Some(&key(7)), 3).unwrap();
    assert_eq!(
        page.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        ["k08", "k09", "k10"],
        "a page after a key is the next keys in order"
    );
    drop(view);
    drop(engine);
    let db = Connection::open(dir.join("projection.db")).unwrap();
    let count = |sql: &str| {
        db.query_row(sql, [name.as_str()], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    let rows = [
        "SELECT COUNT(*) FROM records WHERE ty = ?1",
        "SELECT COUNT(*) FROM keys WHERE ty = ?1",
        "SELECT COUNT(*) FROM text_index WHERE ty = ?1",
        "SELECT COUNT(*) FROM sort_index WHERE ty = ?1",
        "SELECT COUNT(*) FROM search_rows WHERE ty = ?1",
        "SELECT COUNT(*) FROM edges WHERE src_ty = ?1 OR dst_ty = ?1",
    ]
    .map(count);
    assert_eq!(
        rows,
        [0, 0, 0, 0, 0, 0],
        "an order name holds no row of the record tables"
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM view_order WHERE name = ?1"),
        40,
        "one entry for each key, in `view_order`"
    );
    // An entry ended leaves no row, and so does an order ended whole.
    let engine = SqliteOpener
        .open(&Location::sqlite("kept", &dir), OpenMode::Write)
        .unwrap();
    let p = engine.projection();
    p.begin().unwrap();
    p.put_order(&name, &key(7), None).unwrap();
    p.commit(Position(42)).unwrap();
    let view = engine.view(None).unwrap();
    assert!(view.order(&name, &key(7)).unwrap().is_none());
    let page = view.order_page(&name, Some(&key(6)), 3).unwrap();
    assert_eq!(
        page.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        ["k08", "k09", "k10"],
        "an ended key is not read between the entries beside it"
    );
    drop(view);
    drop(engine);
    let db = Connection::open(dir.join("projection.db")).unwrap();
    let left = |sql: &str| {
        db.query_row(sql, [name.as_str()], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    assert_eq!(
        left("SELECT COUNT(*) FROM view_order WHERE name = ?1"),
        39,
        "a key ended leaves no entry"
    );
    let engine = SqliteOpener
        .open(&Location::sqlite("kept", &dir), OpenMode::Write)
        .unwrap();
    let p = engine.projection();
    p.begin().unwrap();
    p.end_order(&name).unwrap();
    p.commit(Position(43)).unwrap();
    assert!(
        engine
            .view(None)
            .unwrap()
            .order_page(&name, None, 100)
            .unwrap()
            .is_empty()
    );
    drop(engine);
    let db = Connection::open(dir.join("projection.db")).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT COUNT(*) FROM view_order WHERE name = ?1",
            [name.as_str()],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0,
        "an order ended whole leaves no entry"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The fold keeps a view's order entries in `view_order`, apart from the
/// records (lakelands#1690): no `sys:order.` record is put, and the
/// entries the extreme reads its next holder from are there.
#[tokio::test(flavor = "multi_thread")]
async fn the_fold_keeps_an_extremes_order_apart_from_the_records() {
    use lakelands_api::ontology::StorageKind;
    use lakelands_api::seam::{Seams, VIEW_KEPT_SETTING};
    use lakelands_core::conformance::fixture::{Lake, field, type_decl, view_of};
    use std::sync::Arc;

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("kept-name-order-fold-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let seams = Seams {
        engines: vec![Arc::new(SqliteOpener)],
        ..Seams::default()
    };
    let store = lakelands_store::open(Location::sqlite("kept", &dir), seams, OpenMode::Create)
        .await
        .unwrap();
    let l = Lake::of(store.as_ref());
    l.declare(type_decl(
        "line",
        vec![
            field("g", StorageKind::Text),
            field("n", StorageKind::Number),
        ],
    ))
    .await
    .unwrap();
    let fields = vec![
        field("g", StorageKind::Text),
        field("low", StorageKind::Number),
        field("high", StorageKind::Number),
    ];
    l.declare(view_of(
        "ends",
        "line",
        &["g"],
        fields,
        &[("low", "min(n)"), ("high", "max(n)")],
    ))
    .await
    .unwrap();
    l.set(&format!("{VIEW_KEPT_SETTING}ends"), json!(true))
        .await
        .unwrap();
    for (key, fields) in [
        ("a", json!({"g": "x", "n": 9})),
        ("b", json!({"g": "x", "n": 5})),
    ] {
        l.upsert("line", key, fields).await.unwrap();
    }
    drop(store);
    let db = Connection::open(dir.join("projection.db")).unwrap();
    let count = |sql: &str| db.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(
        (count("SELECT COUNT(*) FROM records WHERE ty = 'sys:order.ends'")
            + count("SELECT COUNT(*) FROM keys WHERE ty = 'sys:order.ends'")),
        0,
        "the fold writes no `sys:order.` record"
    );
    let entries = count("SELECT COUNT(*) FROM view_order WHERE name = 'sys:order.ends'");
    assert!(
        entries > 0,
        "the extreme's order holds its entries in `view_order`, not {entries}"
    );
    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}
