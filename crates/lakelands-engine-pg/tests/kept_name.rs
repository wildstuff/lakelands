//! A view's kept records bear a name of the kept form
//! (`lakelands_api::seam::KEPT_PREFIX`), and a kept record is put again at
//! every write to its group, so a key of such a name may hold thousands
//! of states. Every name is read so that none of a key's other states is
//! (lakelands#575, lakelands#537): at the head a walk of the heads,
//! `records_head`, each joined to its state; at an earlier position the
//! same walk with a seek of each key's state there. A read by keys seeks
//! each key's state, and a recorded output is read by a seek. The engine check
//! holds the answers. Here each statement's plan is read, as the engine
//! itself writes it, on a small lake the planner may or may not hold
//! statistics of (the server's autoanalyze decides that, not the test),
//! on that lake analysed, and on a large one the writer analysed; and
//! what the reads cost is counted where the server counts it: the tuples
//! of `records` and of `outputs` it fetched (`pg_stat_user_tables`),
//! which a backend reports when it ends. An order name
//! (`lakelands_api::seam::ORDER_PREFIX`, lakelands#629) keeps one state
//! for each key and is read by the same statements, its page after a key
//! a seek into the heads; nothing indexes it.

mod common;

use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode, Scan};
use lakelands_api::event::Digest;
use lakelands_api::id::{Position, RecordKey, TypeName};
use lakelands_api::read::{Direction, Order};
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::{kept_name, order_name};
use lakelands_core::conformance::fixture::stored_record;
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
    stored_record(
        ty.clone(),
        key,
        serde_json::from_value(json!({"n": at})).unwrap(),
        at,
    )
}

/// The plan of each statement the engine names for a kept name, at the
/// head `last` and at the earlier position `at`, with the values a read
/// binds, and as the server ran it (a plan not run does not show how the
/// next key is found): the pages of the name with many keys, the count
/// of the one with few (a count of most of a table is rightly planned as
/// a read of the table). Under `analyse` the tables read are analysed
/// first.
fn plans(
    url: &str,
    schema: &str,
    names: &[TypeName; 2],
    (last, at): (u64, u64),
    analyse: bool,
) -> Vec<(bool, Vec<String>)> {
    let (url, schema, names) = (url.to_owned(), schema.to_owned(), names.clone());
    let [many, few] = names.clone();
    std::thread::spawn(move || {
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        if analyse {
            let tables =
                format!("ANALYZE {schema}.records, {schema}.records_head, {schema}.outputs");
            client.batch_execute(&tables).unwrap();
        }
        // As the engine's sessions plan: each run for its values.
        client
            .batch_execute("SET plan_cache_mode = force_custom_plan")
            .unwrap();
        let mut explain = |statement: &str, args: &[&(dyn ToSql + Sync)]| -> Vec<String> {
            let rows = client
                .query(
                    &format!("EXPLAIN (ANALYZE, BUFFERS OFF, COSTS OFF, TIMING OFF, SUMMARY OFF) {statement}"),
                    args,
                )
                .unwrap();
            rows.iter().map(|row| row.get(0)).collect()
        };
        let (many, few, limit, after) = (many.as_str(), few.as_str(), 10_i64, "a");
        let mut out = Vec::new();
        for (head, at) in [(true, last as i64), (false, at as i64)] {
            let [first, later, count, by_key, output] = read_statements(&schema, head);
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
/// knowing the tables (`analysed`) or not: no table read whole, and each
/// key's state found by a seek, so none of its other states is read. With
/// statistics the heads are walked in key order too, with no sort; without
/// them the planner may sort the heads of a small table, one row a key.
fn seeks(plans: &[(bool, Vec<String>)], analysed: bool) {
    for (i, (head, plan)) in plans.iter().enumerate() {
        let has = |step: &str| plan.iter().any(|line| line.contains(step));
        let told = format!("with statistics: {analysed}; at the head: {head}\n{plan:#?}");
        // Only the read by keys sorts, the few records it read.
        assert!(
            (i % 5 == 3 || !analysed || !has("Sort")) && !has("Seq Scan"),
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
        } else {
            // The heads, the outer side of the one join, each joined to
            // its state by a seek cut at one row: at the head the state it
            // names, at an earlier position the key's latest one there.
            let at = |step: &str| plan.iter().position(|line| line.contains(step));
            assert!(
                at("Nested Loop").is_some()
                    && at("on records_head h") < at("on records x")
                    && plan[at("on records x").unwrap_or(0) - 1].contains("Limit"),
                "a kept name's read does not seek each head's state; {told}"
            );
            if !analysed {
                continue;
            }
            // Known to the planner, a page walks the heads in key order, with
            // no sort, and no further than its limit of ten states asks; a
            // count reads its name's heads by their primary key.
            let walked = plan.iter().find_map(|line| {
                let (_, rows) = line.split_once("on records_head h (actual rows=")?;
                rows.split(' ')
                    .next()?
                    .parse::<f64>()
                    .ok()
                    .map(|n| n as u64)
            });
            // At the head the state is the one at the head's position, by
            // either index that holds it; earlier, the key's primary key read
            // backward from the position.
            let seek = match head {
                true => "using records_",
                false => "Backward using records_pkey",
            };
            let sought = plan
                .iter()
                .any(|l| l.contains(seek) && l.contains("on records x"));
            assert!(
                sought
                    && (i % 5 == 2 && has("records_head_pkey")
                        || has("using records_head_pkey on records_head h") && walked <= Some(11)),
                "a kept name's read does not walk the heads in key order and seek each \
                 key's state by an index; {told}"
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
        let recorded = (view.output(SEAM, &Digest("many".into())).unwrap()).map(|(_, v)| v);
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
    // after, so the planner holds statistics of them only if the server's
    // autoanalyze took them. Then the other keys and the rest of `a`'s
    // states, which the writer analyses as it loads.
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
    let known = common::settled(
        &url,
        &schema,
        "SELECT reltuples::bigint FROM pg_class WHERE relname = 'records' \
         AND relnamespace = $1::text::regnamespace",
    ) >= 0;
    // The small lake as the planner holds it, then analysed: the plans
    // are seeks whether autoanalyze ran before them or not.
    let small = (early + UNANALYSED, early + UNANALYSED / 2);
    seeks(&plans(&url, &schema, &names, small, false), known);
    seeks(&plans(&url, &schema, &names, small, true), true);
    load(early + UNANALYSED + 1..=last);
    drop(engine);
    // The writer analysed its tables as it loaded them.
    let middle = last - (STATES - UNANALYSED) / 2;
    seeks(&plans(&url, &schema, &names, (last, middle), false), true);
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
         WHERE schemaname = $1 AND indexrelname = 'records_head_pkey'",
    );
    common::drop_schema(&url, &schema);
    assert!(
        each[0] < 400 && each[1] < 10 && through_head >= 60,
        "two pages, three keys, a count, a page in a field's order and the view's state, read at the head \
         and in the middle of the states of a key with {STATES} of them, fetched {each:?} \
         tuples of `records` and of `outputs`; the heads were read {through_head} times"
    );
}

#[test]
fn an_order_name_is_read_by_a_seek_and_indexed_nowhere() {
    let Some(url) = common::url_or_skip("the pg test of an order name") else {
        return;
    };
    let schema = common::fresh_schema();
    let engine = PgOpener
        .open(&common::location(&url, "order", &schema), OpenMode::Create)
        .unwrap();
    let name = order_name(&TypeName::from("v"));
    // Twenty thousand keys, and the first two thousand of them put again.
    let (keys, last) = (20_000, 22_000);
    for first in (1..=last).step_by(1_000) {
        let p = engine.projection();
        p.begin().unwrap();
        for at in first..first + 1_000 {
            let mut entry = state(&name, &format!("k{:05}", (at - 1) % keys), at);
            entry.fields =
                serde_json::from_value(json!({"sys:entry": {"v": {"text": "1"}}, "t": "word"}))
                    .unwrap();
            p.put_record(&entry).unwrap();
        }
        p.commit(Position(first + 999)).unwrap();
    }
    drop(engine);
    let rows = [
        "records",
        "records_head",
        "text_index",
        "sort_index",
        "full_text",
    ]
    .map(|table| {
        let query =
            format!("SELECT count(*) FROM {schema}.{table} WHERE ty = 'sys:order.v' AND $1 <> ''");
        common::settled(&url, &schema, &query)
    });
    assert_eq!(
        rows,
        [keys as i64, keys as i64, 0, 0, 0],
        "one state and one head for each key, no other row"
    );
    let (u, sc, n) = (url.clone(), schema.clone(), name.clone());
    let plan: Vec<String> = std::thread::spawn(move || {
        let mut client = postgres::Client::connect(&u, postgres::NoTls).unwrap();
        client
            .batch_execute(&format!(
                "ANALYZE {sc}.records, {sc}.records_head; SET plan_cache_mode = force_custom_plan"
            ))
            .unwrap();
        let [_, later, ..] = read_statements(&sc, true);
        let (ty, at, limit, after) = (n.as_str(), last as i64, 1_i64, "k10000");
        let explain =
            format!("EXPLAIN (ANALYZE, BUFFERS OFF, COSTS OFF, TIMING OFF, SUMMARY OFF) {later}");
        let rows = client.query(&explain, &[&ty, &at, &limit, &after]).unwrap();
        rows.iter().map(|row| row.get(0)).collect()
    })
    .join()
    .unwrap();
    common::drop_schema(&url, &schema);
    let has = |step: &str| plan.iter().any(|line| line.contains(step));
    assert!(
        has("records_head_pkey") && !has("Sort") && !has("Seq Scan"),
        "an order name's page after a key is not a seek into the heads: {plan:#?}"
    );
}
