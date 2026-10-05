//! A view's kept records bear a name of the kept form
//! (`lakelands_api::seam::KEPT_PREFIX`), and a kept record is put again at
//! every write to its group, so a key of such a name may hold thousands
//! of states. The engine reads such a name by statements of its own
//! (lakelands#575): at the head the states with no end, through the index
//! `records_head`, which holds the kept names alone, so that a stored
//! type's put does not pay for it; at an earlier position a step from key
//! to key and a seek of each key's state there. A read by keys seeks each
//! key's state, and a recorded output is read by a seek. The engine check
//! holds the answers. Here each statement's plan is read, as the engine
//! itself writes it, on tables the planner has no statistics of and on
//! analysed ones, and what the reads cost is counted where the server
//! counts it: the tuples of `records` and of `outputs` it fetched
//! (`pg_stat_user_tables`), which a backend reports when it ends.

mod common;

use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode, Scan};
use lakelands_api::event::Digest;
use lakelands_api::id::{Position, RecordKey, TypeName, Version};
use lakelands_api::read::{Direction, Order};
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::kept_name;
use lakelands_engine_pg::{PgOpener, read_statements};
use postgres::types::ToSql;
use serde_json::json;

/// The states of the much-written key `a` under each kept name and of the
/// much-recorded output; the keys written once beside it under the name
/// `many` and under the name `few`.
const STATES: u64 = 5_000;
const MANY: u64 = 20_000;
const FEW: u64 = 30;

/// The states of `a` put while the planner has no statistics.
const UNANALYSED: u64 = 2_000;

/// The seam a view's state is recorded under.
const SEAM: &str = "sys:kept";

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

/// The plan of each statement the engine names for a kept name, at the
/// head `last` and at the earlier position `at`, with the values a read
/// binds, and as the server ran it (a plan not run does not show how the
/// next key is found): the pages of the name with many keys, the count
/// of the one with few (a count of most of a table is rightly planned as
/// a read of the table).
fn plans(
    url: &str,
    schema: &str,
    names: &[TypeName; 2],
    (last, at): (u64, u64),
) -> Vec<(bool, Vec<String>)> {
    let (url, schema, names) = (url.to_owned(), schema.to_owned(), names.clone());
    let [many, few] = names.clone();
    std::thread::spawn(move || {
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        // As the engine's sessions plan: each run for its values.
        client
            .batch_execute("SET plan_cache_mode = force_custom_plan")
            .unwrap();
        let mut explain = |statement: &str, args: &[&(dyn ToSql + Sync)]| -> Vec<String> {
            let rows = client
                .query(
                    &format!("EXPLAIN (ANALYZE, COSTS OFF, TIMING OFF, SUMMARY OFF) {statement}"),
                    args,
                )
                .unwrap();
            rows.iter().map(|row| row.get(0)).collect()
        };
        let (many, few, limit, after) = (many.as_str(), few.as_str(), 10_i64, "a");
        let mut out = Vec::new();
        for (head, at) in [(true, last as i64), (false, at as i64)] {
            let [first, later, count, by_key, output] = read_statements(&schema, &names[0], head);
            let keys = vec!["a", "k00001", "none"];
            out.push((head, explain(&first, &[&many, &at, &limit])));
            out.push((head, explain(&later, &[&many, &at, &limit, &after])));
            out.push((head, explain(&count, &[&few, &at, &true, &true])));
            out.push((head, explain(&by_key, &[&many, &keys, &at])));
            out.push((head, explain(&output, &[&SEAM, &"many", &at])));
        }
        out
    })
    .join()
    .unwrap()
}

/// What every plan of a kept name's statements must be, the planner
/// knowing the tables (`analysed`) or not.
fn seeks(plans: &[(bool, Vec<String>)], analysed: bool) {
    for (i, (head, plan)) in plans.iter().enumerate() {
        let has = |step: &str| plan.iter().any(|line| line.contains(step));
        let told = format!("with statistics: {analysed}; at the head: {head}\n{plan:#?}");
        // Only the read by keys sorts, the few records it read.
        assert!(
            (i % 5 == 3 || !has("Sort")) && !has("Seq Scan"),
            "a kept name's read sorts or reads a table whole; {told}"
        );
        if i % 5 == 3 {
            // By keys: each key's primary key read backward from the
            // position, cut at its first row.
            assert!(
                has("Nested Loop") && has("Limit") && has("Index Scan Backward using records_pkey"),
                "a read by keys is not a seek of each key's latest state; {told}"
            );
        } else if i % 5 == 4 {
            // The output: the primary key read backward from the position,
            // cut at its first row.
            assert!(
                has("Limit") && has("Index Scan Backward using outputs_pkey"),
                "the read of a recorded output is not a seek of its latest state; {told}"
            );
        } else if *head {
            assert!(
                has("records_head"),
                "a kept name's read at the head does not read the states with no end through \
                 `records_head`; {told}"
            );
        } else {
            // The first key and each next one by a seek into
            // `records_key_c`, which covers it. The steps are the outer
            // side of the one join, each joined to the seek of its key's
            // state: the rows come in the order of the steps, which is the
            // keys', with no `ORDER BY`. A page steps no further than its
            // limit of ten states asks.
            let lines = |step: &str| plan.iter().filter(|line| line.contains(step)).count();
            let at = |step: &str| plan.iter().position(|line| line.contains(step));
            let stepped = plan.iter().find_map(|line| {
                let rows = line.split("Recursive Union (actual rows=").nth(1)?;
                rows.split(' ').next()?.parse::<u64>().ok()
            });
            assert!(
                lines("Nested Loop") == 1
                    && lines("Index Only Scan using records_key_c") == 2
                    && at("CTE Scan on step") < at("Index Scan Backward using records_key_c")
                    && at("Nested Loop") < at("CTE Scan on step")
                    && (i % 5 == 2 || stepped <= Some(10)),
                "a kept name's read at an earlier position does not step from key to key \
                 through `records_key_c` and seek each key's state, in the order of the \
                 steps; {told}"
            );
        }
    }
}

/// The reads of a kept view's doors, at the head and in the middle of
/// `a`'s states: a first page, a page after the first key, three keys,
/// the count, a page in a field's order, and the view's state.
fn read(engine: &dyn LakeEngine, [many, few]: &[TypeName; 2], last: u64, middle: u64) {
    for (as_of, n) in [(None, last), (Some(Position(middle)), middle)] {
        let view = engine.view(as_of).unwrap();
        let page = |ty: &TypeName, scan: Scan| -> Vec<(String, u64)> {
            let rows = view.scan(ty, &scan).unwrap();
            let n = |r: &StoredRecord| r.fields.get("n").and_then(|n| n.as_u64()).unwrap();
            rows.iter().map(|r| (r.key.to_string(), n(r))).collect()
        };
        let ten = Scan {
            limit: 10,
            ..Default::default()
        };
        let first = page(many, ten.clone());
        let keys: Vec<&str> = first.iter().map(|(k, _)| k.as_str()).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!((keys.len(), &keys), (10, &sorted), "as of {as_of:?}");
        assert_eq!(first[0], ("a".to_owned(), n), "as of {as_of:?}");
        let later = Scan {
            after: Some(lakelands_api::engine::After {
                values: Vec::new(),
                key: RecordKey::from("a"),
            }),
            ..ten.clone()
        };
        assert_eq!(page(many, later)[0].0, "k00001", "as of {as_of:?}");
        let keys = ["a", "k00001", "none"].map(RecordKey::from);
        let by_key = view.get_many(many, &keys).unwrap();
        let read: Vec<&str> = by_key.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(read, ["a", "k00001"], "as of {as_of:?}");
        assert_eq!(view.count(few, &ten).unwrap(), FEW + 1, "as of {as_of:?}");
        let by_n = Scan {
            order_by: vec![Order {
                field: "n".into(),
                by: Default::default(),
                dir: Direction::Desc,
            }],
            ..ten
        };
        assert_eq!(page(few, by_n)[0], ("a".to_owned(), n), "as of {as_of:?}");
        let recorded = view.output(SEAM, &Digest("many".into())).unwrap();
        assert_eq!(recorded, Some(json!({"at": n})), "as of {as_of:?}");
    }
}

#[test]
fn a_kept_name_is_read_by_seeks_with_statistics_and_without() {
    let Some(url) = common::url_or_skip("the pg test of a kept name") else {
        return;
    };
    let schema = common::fresh_schema();
    let open = |mode| {
        PgOpener
            .open(&common::location(&url, "kept", &schema), mode)
            .unwrap()
    };
    let names = ["many", "few"].map(|v| kept_name(&TypeName::from(v)));
    let engine = open(OpenMode::Create);
    // Six hundred keys written once, then two thousand states of `a`
    // under both names: fewer records than the writer analyses its tables
    // after, so the planner has no statistics of them. Then the other
    // keys and the rest of `a`'s states, which it analyses as it loads.
    // The output is recorded at every position.
    let (early, last) = (600, MANY + STATES);
    let key = |at: u64| match at {
        at if at <= early => format!("k{at:05}"),
        at if (early + UNANALYSED..last - (STATES - UNANALYSED)).contains(&(at - 1)) => {
            format!("k{:05}", at - UNANALYSED)
        }
        _ => "a".to_owned(),
    };
    let load = |positions: std::ops::RangeInclusive<u64>| {
        for first in positions.step_by(200) {
            let p = engine.projection();
            p.begin().unwrap();
            for at in first..first + 200 {
                let key = key(at);
                p.put_record(&state(&names[0], &key, at)).unwrap();
                if key.as_str() <= "k00030" {
                    p.put_record(&state(&names[1], &key, at)).unwrap();
                }
                let recorded = json!({"at": at});
                p.put_output(SEAM, &Digest("many".into()), &recorded, Position(at))
                    .unwrap();
            }
            p.commit(Position(first + 199)).unwrap();
        }
    };
    load(1..=early + UNANALYSED);
    let unknown = common::settled(
        &url,
        &schema,
        "SELECT reltuples::bigint FROM pg_class WHERE relname = 'records' \
         AND relnamespace = $1::text::regnamespace",
    );
    assert_eq!(
        unknown, -1,
        "the planner has statistics of a lake below the threshold"
    );
    // The index of the head is over the kept names alone, as the server
    // prints its condition: a stored type's put does not maintain it.
    let narrowed = common::settled(
        &url,
        &schema,
        "SELECT count(*) FROM pg_index WHERE indexrelid = ($1 || '.records_head')::regclass \
         AND pg_get_expr(indpred, indrelid) = '((until IS NULL) AND \
         ((ty COLLATE \"C\") >= ''sys:kept.''::text) AND ((ty COLLATE \"C\") < ''sys:kept/''::text))'",
    );
    assert_eq!(
        narrowed, 1,
        "`records_head` is not over the kept names alone"
    );
    seeks(
        &plans(
            &url,
            &schema,
            &names,
            (early + UNANALYSED, early + UNANALYSED / 2),
        ),
        false,
    );
    load(early + UNANALYSED + 1..=last);
    drop(engine);
    // The writer analysed its tables as it loaded them.
    let middle = last - (STATES - UNANALYSED) / 2;
    seeks(&plans(&url, &schema, &names, (last, middle)), true);
    let before = ["records", "outputs"].map(|table| common::fetched(&url, &schema, table));
    let engine = open(OpenMode::Read);
    for _ in 0..20 {
        read(engine.as_ref(), &names, last, middle);
    }
    drop(engine);
    let each = ["records", "outputs"]
        .map(|table| common::fetched(&url, &schema, table))
        .iter()
        .zip(before)
        .map(|(after, before)| (after - before) / 20)
        .collect::<Vec<_>>();
    let through_head = common::settled(
        &url,
        &schema,
        "SELECT idx_scan FROM pg_stat_user_indexes \
         WHERE schemaname = $1 AND indexrelname = 'records_head'",
    );
    common::drop_schema(&url, &schema);
    assert!(
        each[0] < 400 && each[1] < 10 && through_head >= 60,
        "two pages, three keys, a count, a page in a field's order and the view's state, read at the head \
         and in the middle of the states of a key with {STATES} of them, fetched {each:?} \
         tuples of `records` and of `outputs`; `records_head` was read {through_head} times"
    );
}
