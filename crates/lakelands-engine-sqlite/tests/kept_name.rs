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
//! way, by a seek. An
//! order name (`lakelands_api::seam::ORDER_PREFIX`, lakelands#629) keeps
//! one state for each key and is read by the statements of a stored type,
//! its page after a key a seek into the primary key; nothing indexes it.

use std::path::PathBuf;

use lakelands_api::Location;
use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::event::Digest;
use lakelands_api::id::{Position, TypeName, Version};
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::{kept_name, order_name};
use lakelands_core::conformance::fixture::stored_record;
use lakelands_engine_sqlite::{SqliteOpener, read_statements};
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
fn an_order_name_is_read_by_a_seek_and_indexed_nowhere() {
    let (dir, engine) = lake("order");
    let name = order_name(&TypeName::from("v"));
    let p = engine.projection();
    p.begin().unwrap();
    for at in 1..=100 {
        let mut entry = state(&name, &format!("k{:02}", at % 50), at);
        entry.fields =
            serde_json::from_value(json!({"sys:entry": {"v": {"text": "1"}}, "t": "word"}))
                .unwrap();
        p.put_record(&entry).unwrap();
    }
    // Ten keys ended one by one: an end leaves neither a state nor the
    // key's number, whose key holds the value the entry orders by.
    for at in 101..=110 {
        let mut ended = state(&name, &format!("k{:02}", at % 50), at);
        ended.version = Version(0);
        p.put_record(&ended).unwrap();
    }
    p.commit(Position(110)).unwrap();
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
    let full_text: i64 = db
        .query_row("SELECT COUNT(*) FROM full_text", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        (rows, full_text),
        ([40, 40, 0, 0, 0, 0], 0),
        "one state and one number for each of the 40 keys not ended, no other row"
    );
    let [_, later, _, _] = read_statements(&name, true);
    let plan = plan(&db, &later);
    assert!(
        plan == [
            "SEARCH k USING PRIMARY KEY (ty=? AND key>?)",
            "SEARCH r USING INDEX sqlite_autoindex_records_1 (id=? AND since<?)",
        ],
        "an order name's page after a key is not a seek into the primary keys:\n  {later}\n  \
         {plan:#?}"
    );
    // Every key ended at once leaves no number either.
    let engine = SqliteOpener
        .open(&Location::sqlite("kept", &dir), OpenMode::Write)
        .unwrap();
    let p = engine.projection();
    p.begin().unwrap();
    p.end_head_only(&name, Position(111)).unwrap();
    p.commit(Position(111)).unwrap();
    drop(engine);
    assert_eq!(
        [
            count("SELECT COUNT(*) FROM records WHERE ty = ?1"),
            count("SELECT COUNT(*) FROM keys WHERE ty = ?1")
        ],
        [0, 0],
        "every key of the name ended at once leaves no state and no number"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
