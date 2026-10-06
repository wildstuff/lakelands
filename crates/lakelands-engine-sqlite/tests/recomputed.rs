//! What a declaration costs on SQLite when it recomputes a type's derived
//! fields: the time the `declare` holds the writer and the peak memory of
//! its append, at 100 000 live records, each on a fresh lake. For one
//! declaration: a function declared again, the type's own derived
//! expression declared again, the same with an expression that changes no
//! record, and a type with no derived field declared again. For several
//! (lakelands#523): a bundle of 50 changed functions of one type, a bundle
//! of two, a bundle of a function and the type that uses it, and a rebuild
//! of ten appends that each change a function. And a rebuild of a log of
//! embedded writes, which holds no call for them (lakelands#604). By
//! hand: `cargo test
//! --release -p lakelands-engine-sqlite --test recomputed -- --ignored
//! --nocapture`.

use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

use lakelands_api::LakeStore;
use lakelands_api::engine::OpenMode;
use lakelands_api::ontology::StorageKind::{Decimal, Number, Text};
use lakelands_api::seam::Seams;
use lakelands_api::write::Declaration;
use lakelands_core::conformance::backing::embedder_as;
use lakelands_core::conformance::fixture::{
    Lake, computed_by, derived, embedding, source_record, sourced, type_decl,
};
use lakelands_engine_sqlite::SqliteOpener;
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
        "sqlite: {} records, {what}: writer held {held:?}, peak memory {} MB",
        records(),
        peak / 1_000_000
    );
    answer
}

/// The function `name`, computing `expr` from `net`.
fn function(name: &str, expr: &str) -> Declaration {
    serde_json::from_value(json!({
        "declare": "function", "name": name, "backing": "expr",
        "inputs": [{"name": "net", "type": {"kind": "decimal"}}],
        "expr": expr
    }))
    .unwrap()
}

/// The function `scaled`, computing `expr` from `net`.
fn scaled(expr: &str) -> Declaration {
    function("scaled", expr)
}

/// Loads the records of `bill`.
async fn load(l: &Lake) {
    let records = (0..records()).map(|i| {
        let fields = json!({"n": i, "net": "3", "memo": "a memo of some length"});
        source_record("bill", &format!("b{i:06}"), fields)
    });
    l.ingest("load", records.collect()).await.unwrap();
}

/// One cell, on a fresh lake: loads the records of a type with one derived
/// field, then declares again what computes it, a function the field reads
/// (`by_function`) or the type's own expression, or (`unchanged`) the type
/// with an expression that computes every record again and changes none;
/// `plain` leaves the derived field out and declares the type again with a
/// description only, so that the store computes nothing and the cost is
/// the engine's alone. Prints the cost of that declaration.
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
        // A description alone, on a type with no derived field, computes
        // nothing.
        let rest = (!plain).then_some(d).into_iter().collect();
        let mut ty = type_decl("bill", [sources.to_vec(), rest].concat());
        ty.description = (more && plain).then(|| "declared again".into());
        ty
    };
    l.declare_as(scaled("net * 2"), "the function")
        .await
        .unwrap();
    l.declare(bill("net * 2", false)).await.unwrap();
    load(&l).await;
    if unchanged {
        // Another expression of the same value: every record is computed
        // again and none is put.
        let again = l.declare(bill("net + net", true));
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

/// A cell of several declarations.
#[derive(Clone, Copy)]
enum Several {
    /// One bundle that declares this many functions of one type again,
    /// each with a change.
    Functions(usize),
    /// One bundle that declares a function and the type that uses it
    /// again, both with a change.
    FunctionAndType,
    /// Ten appends, each the function declared again with a change, and
    /// then a rebuild, which is what is measured.
    Rebuilt,
}

/// One cell of several declarations, on a fresh lake: loads the records of
/// a type with a field for each function, `d0` by `f0` and so on, and `e`,
/// derived by the type's own expression; then the declarations, and their
/// cost.
async fn several(store: &dyn LakeStore, which: Several) {
    let l = Lake::of(store);
    let k = match which {
        Several::Functions(k) => k,
        _ => 1,
    };
    let bill = |e: &str| {
        let by = (0..k).map(|i| computed_by(&format!("d{i}"), Decimal, &format!("f{i}"), &["net"]));
        let sources = [
            sourced("n", Number),
            sourced("net", Decimal),
            sourced("memo", Text),
        ];
        let e = derived("e", Decimal, e, &["net"]);
        type_decl("bill", sources.into_iter().chain(by).chain([e]).collect())
    };
    let functions = |expr: &str| -> Vec<Declaration> {
        (0..k).map(|i| function(&format!("f{i}"), expr)).collect()
    };
    for f in functions("net * 2") {
        l.declare_as(f, "a function").await.unwrap();
    }
    l.declare(bill("net * 2")).await.unwrap();
    load(&l).await;
    let (d, e) = match which {
        Several::Functions(_) => {
            let bundle = l.cat.apply(&l.own, functions("net * 3"));
            let what = format!("a bundle of {k} changed functions of one type");
            measured(&what, bundle).await.unwrap();
            ("9", "6")
        }
        Several::FunctionAndType => {
            let mut both = functions("net * 3");
            both.push(Declaration::Type(bill("net * 3")));
            let bundle = l.cat.apply(&l.own, both);
            measured("a bundle of a function and its type, both changed", bundle)
                .await
                .unwrap();
            ("9", "9")
        }
        Several::Rebuilt => {
            for by in 3..=12 {
                let again = function("f0", &format!("net * {by}"));
                l.declare_as(again, "the function again").await.unwrap();
            }
            let rebuilt = store.rebuild();
            measured("a rebuild of ten appends, each a changed function", rebuilt)
                .await
                .unwrap();
            ("36", "6")
        }
    };
    let read = l.read("bill", "b000007").await.unwrap();
    let last = format!("d{}", k - 1);
    assert_eq!(
        (read.fields.get(last.as_str()), read.fields.get("e")),
        (Some(&json!(d)), Some(&json!(e)))
    );
}

/// The cell of lakelands#604: a record whose text of 1 000 bytes is
/// embedded, written once for each record a window of 1 000 at a time, its
/// embeddings made; then a rebuild, which reads them from the log and
/// holds no call.
async fn embedded(store: &dyn LakeStore) {
    let l = Lake::of(store);
    let fields = vec![
        sourced("n", Number),
        sourced("body", Text),
        embedding("v", 8, &["body"], "m"),
    ];
    l.declare(type_decl("note", fields)).await.unwrap();
    let memo = "a memo of some length, as a note's is; ".repeat(26);
    let body = |i: u64| format!("{i:08} {memo}");
    for from in (0..records()).step_by(1_000) {
        let window = (from..(from + 1_000).min(records())).map(|i| {
            let fields = json!({"n": i, "body": &body(i)[..1_000]});
            source_record("note", &format!("n{i:08}"), fields)
        });
        l.ingest("load", window.collect()).await.unwrap();
    }
    let rebuilt = store.rebuild();
    measured("a rebuild of embedded writes", rebuilt)
        .await
        .unwrap();
    let read = l.read("note", "n00000007").await.unwrap();
    assert!(read.fields.contains_key("v"), "the embedding is read back");
}

/// A cell: one declaration, or several, or the embedded writes.
#[derive(Clone, Copy)]
enum Cell {
    One((bool, bool, bool)),
    Several(Several),
    Embedded,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "measures recomputations over 100 000 records; run by hand with --release"]
async fn recomputing_a_hundred_thousand_records() {
    let cells = [
        Cell::One((true, false, false)),
        Cell::One((false, false, false)),
        Cell::One((false, true, false)),
        Cell::One((false, true, true)),
        Cell::Several(Several::Functions(50)),
        Cell::Several(Several::Functions(2)),
        Cell::Several(Several::FunctionAndType),
        Cell::Several(Several::Rebuilt),
        Cell::Embedded,
    ];
    // All, or the one `LAKELANDS_TEST_CELLS` numbers from 0.
    let only = std::env::var("LAKELANDS_TEST_CELLS").ok();
    let only = only.and_then(|i| i.trim().parse::<usize>().ok());
    let chosen = |(i, c)| only.is_none_or(|o| o == i).then_some(c);
    for which in cells.into_iter().enumerate().filter_map(chosen) {
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("recomputed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let seams = Seams {
            engines: vec![Arc::new(SqliteOpener)],
            embed: Some(embedder_as("m", 8)),
            ..Seams::default()
        };
        let location = lakelands_api::Location::sqlite("bills", &dir);
        let store: Arc<dyn LakeStore> = lakelands_store::open(location, seams, OpenMode::Create)
            .await
            .unwrap();
        match which {
            Cell::One(one) => cell(store.as_ref(), one).await,
            Cell::Several(more) => several(store.as_ref(), more).await,
            Cell::Embedded => embedded(store.as_ref()).await,
        }
        store.close().await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
