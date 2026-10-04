//! What a declaration costs on SQLite when it recomputes a type's derived
//! fields: the time the `declare` holds the writer and the peak memory of
//! its append, at 100 000 live records, for a function declared again and
//! for the type's own derived expression declared again, each on a fresh
//! lake. By hand: `cargo test --release -p lakelands-engine-sqlite --test
//! recomputed -- --ignored --nocapture`.

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
use lakelands_engine_sqlite::SqliteOpener;
use peak_alloc::PeakAlloc;
use serde_json::json;

#[global_allocator]
static HEAP: PeakAlloc = PeakAlloc;

const RECORDS: u64 = 100_000;

/// Runs `call` and prints how long it held and its peak memory.
async fn measured<T>(what: &str, call: impl Future<Output = T>) -> T {
    let live = HEAP.current_usage();
    HEAP.reset_peak_usage();
    let t = Instant::now();
    let answer = call.await;
    let held = t.elapsed();
    let peak = HEAP.peak_usage().saturating_sub(live);
    println!(
        "sqlite: {RECORDS} records, {what}: writer held {held:?}, peak memory {} MB",
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
/// (`by_function`) or the type's own expression. Prints the cost of that
/// declaration.
async fn cell(store: &dyn LakeStore, by_function: bool) {
    let l = Lake::of(store);
    let bill = |expr: &str| {
        let d = match by_function {
            true => computed_by("d", Decimal, "scaled", &["net"]),
            false => derived("d", Decimal, expr, &["net"]),
        };
        let sources = [
            sourced("n", Number),
            sourced("net", Decimal),
            sourced("memo", Text),
        ];
        type_decl("bill", [sources.to_vec(), vec![d]].concat())
    };
    l.declare_as(scaled("net * 2"), "the function")
        .await
        .unwrap();
    l.declare(bill("net * 2")).await.unwrap();
    let records = (0..RECORDS).map(|i| {
        let fields = json!({"n": i, "net": "3", "memo": "a memo of some length"});
        source_record("bill", &format!("b{i:06}"), fields)
    });
    l.ingest("load", records.collect()).await.unwrap();
    if by_function {
        let again = l.declare_as(scaled("net * 3"), "the function again");
        measured("a function declared again", again).await.unwrap();
    } else {
        let again = l.declare(bill("net * 3"));
        measured("a derived expression declared again", again)
            .await
            .unwrap();
    }
    let read = l.read("bill", "b000007").await.unwrap();
    assert_eq!(read.fields.get("d"), Some(&json!("9")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "measures recomputations over 100 000 records; run by hand with --release"]
async fn recomputing_a_hundred_thousand_records() {
    for by_function in [true, false] {
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("recomputed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let seams = Seams {
            engines: vec![Arc::new(SqliteOpener)],
            ..Seams::default()
        };
        let location = lakelands_api::Location::sqlite("bills", &dir);
        let store: Arc<dyn LakeStore> = lakelands_store::open(location, seams, OpenMode::Create)
            .await
            .unwrap();
        cell(store.as_ref(), by_function).await;
        store.close().await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
