//! What a declaration costs on PostgreSQL when it recomputes a type's
//! derived fields: the time the `declare` holds the writer and the peak
//! memory of its append, at 100 000 live records, for a function declared
//! again, for the type's own derived expression declared again, and for
//! the type declared again with one more source field, which changes no
//! record, each on a fresh lake. By hand, with `LAKELANDS_TEST_PG_URL` set: `cargo test
//! --release -p lakelands-engine-pg --test recomputed -- --ignored
//! --nocapture`.

mod common;

use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

use lakelands_api::LakeStore;
use lakelands_api::engine::OpenMode;
use lakelands_api::ontology::StorageKind::{Decimal, Number, Text};
use lakelands_api::seam::Seams;
use lakelands_api::write::Declaration;
use lakelands_core::conformance::fixture::{
    Lake, computed_by, derived, source_record, sourced, type_decl,
};
use lakelands_engine_pg::PgOpener;
use peak_alloc::PeakAlloc;
use serde_json::json;

#[global_allocator]
static HEAP: PeakAlloc = PeakAlloc;

/// How many records a cell loads: 100 000, or `LAKELANDS_TEST_RECORDS`.
fn records() -> u64 {
    let asked = std::env::var("LAKELANDS_TEST_RECORDS").ok();
    asked.and_then(|n| n.parse().ok()).unwrap_or(100_000)
}

/// Runs `call` and prints how long it held and its peak memory.
async fn measured<T>(what: &str, call: impl Future<Output = T>) -> T {
    let live = HEAP.current_usage();
    HEAP.reset_peak_usage();
    let t = Instant::now();
    let answer = call.await;
    let held = t.elapsed();
    let peak = HEAP.peak_usage().saturating_sub(live);
    println!(
        "pg: {} records, {what}: writer held {held:?}, peak memory {} MB",
        records(),
        peak / 1_000_000
    );
    answer
}

/// The function `scaled`, computing `expr` from `net`.
fn scaled(expr: &str) -> Declaration {
    serde_json::from_value(json!({
        "declare": "function", "name": "scaled", "backing": "expr",
        "inputs": [{"name": "net", "type": {"kind": "decimal"}}],
        "expr": expr
    }))
    .unwrap()
}

/// One cell, on a fresh lake: loads the records of a type with one derived
/// field, then declares again what computes it, a function the field reads
/// (`by_function`) or the type's own expression, or (`unchanged`) the type
/// with one more source field, which computes every record again and
/// changes none; `plain` leaves the derived field out and declares the type
/// again with a description only, so that the store computes nothing and
/// the cost is the engine's alone. Prints the cost of that
/// declaration.
async fn cell(store: &dyn LakeStore, (by_function, unchanged, plain): (bool, bool, bool)) {
    let l = Lake::of(store);
    let bill = |expr: &str, more: bool| {
        let d = match by_function {
            true => computed_by("d", Decimal, "scaled", &["net"]),
            false => derived("d", Decimal, expr, &["net"]),
        };
        let sources = [
            sourced("n", Number),
            sourced("net", Decimal),
            sourced("memo", Text),
        ];
        // One more source field computes the type again; a description
        // alone, on a type with no derived field, computes nothing.
        let field = (more && !plain).then(|| sourced("note", Text));
        let rest = (!plain).then_some(d).into_iter().chain(field).collect();
        let mut ty = type_decl("bill", [sources.to_vec(), rest].concat());
        ty.description = (more && plain).then(|| "declared again".into());
        ty
    };
    l.declare_as(scaled("net * 2"), "the function")
        .await
        .unwrap();
    l.declare(bill("net * 2", false)).await.unwrap();
    let records = (0..records()).map(|i| {
        let fields = json!({"n": i, "net": "3", "memo": "a memo of some length"});
        source_record("bill", &format!("b{i:06}"), fields)
    });
    l.ingest("load", records.collect()).await.unwrap();
    if unchanged {
        let again = l.declare(bill("net * 2", true));
        let what = match plain {
            true => "a type with no derived field declared again",
            false => "the type declared again, no record changes",
        };
        measured(what, again).await.unwrap();
    } else if by_function {
        let again = l.declare_as(scaled("net * 3"), "the function again");
        measured("a function declared again", again).await.unwrap();
    } else {
        let again = l.declare(bill("net * 3", false));
        measured("a derived expression declared again", again)
            .await
            .unwrap();
    }
    let read = l.read("bill", "b000007").await.unwrap();
    let d = match (plain, unchanged) {
        (true, _) => None,
        (false, true) => Some(json!("6")),
        (false, false) => Some(json!("9")),
    };
    assert_eq!(read.fields.get("d"), d.as_ref());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "measures recomputations over 100 000 records; run by hand with --release"]
async fn recomputing_a_hundred_thousand_records() {
    let Some(url) = common::url_or_skip("the pg recomputation measurement") else {
        return;
    };
    let cells = [
        (true, false, false),
        (false, false, false),
        (false, true, false),
        (false, true, true),
    ];
    // All, or the one `LAKELANDS_TEST_CELLS` numbers from 0.
    let only = std::env::var("LAKELANDS_TEST_CELLS").ok();
    let only = only.and_then(|i| i.trim().parse::<usize>().ok());
    let chosen = |(i, c)| only.is_none_or(|o| o == i).then_some(c);
    for which in cells.into_iter().enumerate().filter_map(chosen) {
        let schema = common::fresh_schema();
        let seams = Seams {
            engines: vec![Arc::new(PgOpener)],
            ..Seams::default()
        };
        let location = common::location(&url, "bills", &schema);
        let store: Arc<dyn LakeStore> = lakelands_store::open(location, seams, OpenMode::Create)
            .await
            .unwrap();
        cell(store.as_ref(), which).await;
        store.close().await.unwrap();
        common::drop_schemas(&url);
    }
}
