//! A state is written once (lakelands#537): superseding a record inserts
//! its new state and moves its head, a narrow row whose one changing
//! column no index holds, and touches no state written before. What the
//! fold writes is counted where the server counts it, in
//! `pg_stat_user_tables`, which a backend reports when it ends. A read at
//! the head walks the heads and seeks each head's state; the plans of a
//! stored type's pages are read as the engine writes them.

mod common;

use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode};
use lakelands_api::id::{Position, RecordKey, TypeName, Version};
use lakelands_api::record::StoredRecord;
use lakelands_engine_pg::{PgOpener, read_statements};
use serde_json::json;

/// The records put, then superseded.
const RECORDS: u64 = 1_024;

fn state(key: u64, at: u64) -> StoredRecord {
    StoredRecord {
        ty: TypeName::from("t"),
        key: RecordKey::from(format!("k{key:05}")),
        version: Version(at),
        fields: serde_json::from_value(json!({"n": at, "pad": "x".repeat(400)})).unwrap(),
        meta: Default::default(),
        position: Position(at),
        made: Position(at),
        retired: false,
        restricted: false,
    }
}

/// Put every record's state at `at`, in one transaction.
fn put_all(engine: &dyn LakeEngine, at: u64) {
    let p = engine.projection();
    p.begin().unwrap();
    for key in 0..RECORDS {
        p.put_record(&state(key, at)).unwrap();
    }
    p.commit(Position(at)).unwrap();
}

/// The rows inserted, updated, and updated in place (HOT) in `table`.
fn written(url: &str, schema: &str, table: &str) -> [i64; 3] {
    ["n_tup_ins", "n_tup_upd", "n_tup_hot_upd"].map(|column| {
        let query = format!(
            "SELECT {column} FROM pg_stat_user_tables WHERE schemaname = $1 AND relname = '{table}'"
        );
        common::settled(url, schema, &query)
    })
}

#[test]
fn a_supersede_writes_one_state_and_moves_its_head() {
    let Some(url) = common::url_or_skip("the pg test of a supersede") else {
        return;
    };
    let schema = common::fresh_schema();
    let location = common::location(&url, "heads", &schema);
    let engine = PgOpener.open(&location, OpenMode::Create).unwrap();
    put_all(engine.as_ref(), 1);
    drop(engine);
    let before = ["records", "records_head"].map(|t| written(&url, &schema, t));
    let engine = PgOpener.open(&location, OpenMode::Write).unwrap();
    put_all(engine.as_ref(), 2);
    let head = engine
        .view(None)
        .unwrap()
        .get(&TypeName::from("t"), &"k00007".into());
    drop(engine);
    let after = ["records", "records_head"].map(|t| written(&url, &schema, t));
    common::drop_schema(&url, &schema);
    let delta = |i: usize| -> [i64; 3] { std::array::from_fn(|c| after[i][c] - before[i][c]) };
    let n = RECORDS as i64;
    assert_eq!(head.unwrap().map(|r| r.position), Some(Position(2)));
    assert_eq!(
        delta(0)[..2],
        [n, 0],
        "superseding {n} records inserted and updated {:?} rows of `records`: \
         a state is written once, and no state before it is touched",
        delta(0)
    );
    let [inserted, updated, hot] = delta(1);
    assert!(
        inserted == 0 && updated == n && hot > 0,
        "superseding {n} records inserted {inserted} heads and moved {updated}, {hot} in \
         place: each head moves, and with room on its page in place"
    );
}

#[test]
fn a_heads_read_is_a_lookup_per_head() {
    let Some(url) = common::url_or_skip("the pg test of a heads read") else {
        return;
    };
    let schema = common::fresh_schema();
    let location = common::location(&url, "heads", &schema);
    let engine = PgOpener.open(&location, OpenMode::Create).unwrap();
    for at in 1..=5 {
        put_all(engine.as_ref(), at);
    }
    drop(engine);
    let (u, sc) = (url.clone(), schema.clone());
    let plans: Vec<(bool, Vec<String>)> = std::thread::spawn(move || {
        let mut client = postgres::Client::connect(&u, postgres::NoTls).unwrap();
        client
            .batch_execute(&format!(
                "ANALYZE {sc}.records, {sc}.records_head; SET plan_cache_mode = force_custom_plan"
            ))
            .unwrap();
        let mut out = Vec::new();
        for (head, at) in [(true, 5_i64), (false, 3)] {
            let [first, later, ..] = read_statements(&sc, head);
            let (ty, limit, after) = ("t", 10_i64, "k00500");
            for (statement, args) in [
                (
                    first,
                    vec![&ty as &(dyn postgres::types::ToSql + Sync), &at, &limit],
                ),
                (later, vec![&ty, &at, &limit, &after]),
            ] {
                let explain =
                    format!("EXPLAIN (ANALYZE, COSTS OFF, TIMING OFF, SUMMARY OFF) {statement}");
                let rows = client.query(&explain, &args).unwrap();
                out.push((head, rows.iter().map(|row| row.get(0)).collect()));
            }
        }
        out
    })
    .join()
    .unwrap();
    common::drop_schema(&url, &schema);
    for (head, plan) in plans {
        let has = |step: &str| plan.iter().any(|line| line.contains(step));
        let seek = plan.iter().position(|line| line.contains("on records x"));
        assert!(
            has("using records_head_pkey on records_head h")
                && !has("Sort")
                && !has("Seq Scan")
                && seek.is_some_and(|i| plan[i - 1].contains("Limit (actual rows=1 loops=10)")),
            "a page of ten at the head ({head}) is not a walk of ten heads, each with one \
             seek of its state: {plan:#?}"
        );
    }
}
