//! A view's kept records bear a name of the kept form
//! (`lakelands_api::seam::KEPT_PREFIX`), and a kept record is put again at
//! every write to its group, so a key of such a name may hold thousands
//! of states. The engine reads such a name by statements of its own
//! (lakelands#575): at the head the states with no end; at an earlier
//! position a step from key to key and a seek of each key's state there.
//! The engine check holds the answers. Here each statement's plan is
//! read, as the engine itself writes it, and what a read costs is
//! counted in the steps SQLite's virtual machine takes: for a name with a
//! key of five thousand states no more than for a name whose keys were
//! written once. A recorded output is read the same way, by a seek.

use std::path::PathBuf;

use lakelands_api::Location;
use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::event::Digest;
use lakelands_api::id::{Position, RecordKey, TypeName, Version};
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::kept_name;
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
    StoredRecord {
        ty: ty.clone(),
        key: RecordKey::from(key),
        version: Version(1),
        fields: serde_json::from_value(json!({"n": at})).unwrap(),
        meta: Default::default(),
        position: Position(at),
        retired: false,
        restricted: false,
    }
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
    // At the head: the states with no end, through `records_at`, which
    // holds them in key order, and nothing else.
    let [first, later, count, output] = read_statements(&kept, true);
    for statement in [&first, &later, &count] {
        let plan = plan(&db, statement);
        assert!(
            plan.len() == 1
                && plan[0]
                    .starts_with("SEARCH r USING INDEX records_at (ty=? AND until=? AND key>"),
            "a kept name's read at the head does not seek the states with no end in key order \
             through `records_at`:\n  {statement}\n  {plan:#?}"
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
    // At an earlier position: the next key by a seek into the primary
    // key's index, which covers it, for the first step and for each later
    // one; the key's state by a seek from the position down; its row by
    // its id. The steps are the outer loop of the join and nothing sorts,
    // so the rows come in the order of the steps, which is the keys': the
    // statement has no `ORDER BY` of its own to say so, only the three of
    // its seeks.
    let [first, later, count, _] = read_statements(&kept, false);
    for statement in [&first, &later, &count] {
        let plan = plan(&db, statement);
        let step =
            "SEARCH records USING COVERING INDEX sqlite_autoindex_records_1 (ty=? AND key>?)";
        let joined = ["SCAN step", "SEARCH r USING INTEGER PRIMARY KEY (rowid=?)"];
        assert!(
            plan.iter().filter(|s| s.as_str() == step).count() == 2
                && has(
                    &plan,
                    "sqlite_autoindex_records_1 (ty=? AND key=? AND since<?)"
                )
                && plan.windows(2).any(|pair| pair == joined)
                && !has(&plan, "TEMP B-TREE")
                && statement.matches("ORDER BY").count() == 3,
            "a kept name's read at an earlier position does not step from key to key through \
             the primary key and seek each key's state, in the order of the steps:\n  \
             {statement}\n  {plan:#?}"
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
    // key at the head and in the middle of `a`'s states, in key order.
    let middle = OTHERS + KEYS + STATES / 2;
    for (as_of, n) in [(None, last), (Some(Position(middle)), middle)] {
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
            (KEYS + 1, Some(&json!(n)), KEYS + 1),
            "as of {as_of:?}"
        );
        let output = view.output(seam, &Digest("much".into())).unwrap();
        assert_eq!(output, Some(json!({"at": n})), "as of {as_of:?}");
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
