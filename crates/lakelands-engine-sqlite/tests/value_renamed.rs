//! What renaming a value costs on SQLite: the time
//! `migrate_value_type_apply` holds the writer and the peak memory of its
//! append, at 100 000 records, half of them holding the value renamed,
//! beside the way a host had before renames: widen the value type, write
//! every record that holds the old value, tighten it. By hand: `cargo test
//! --release -p lakelands-engine-sqlite --test value_renamed -- --ignored
//! --nocapture`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use lakelands_api::engine::OpenMode;
use lakelands_api::ontology::{StorageKind, ValueTypeDecl};
use lakelands_api::seam::Seams;
use lakelands_api::write::{Declaration, ValueTypeMigration};
use lakelands_api::{LakeStore, Location};
use lakelands_core::conformance::fixture::{Lake, source_record, sourced, type_decl};
use lakelands_engine_sqlite::SqliteOpener;
use peak_alloc::PeakAlloc;
use serde_json::json;

#[global_allocator]
static HEAP: PeakAlloc = PeakAlloc;

/// How many records the lake holds: 100 000, or `LAKELANDS_TEST_RECORDS`.
fn size() -> u64 {
    let asked = std::env::var("LAKELANDS_TEST_RECORDS").ok();
    asked.and_then(|n| n.parse().ok()).unwrap_or(100_000)
}

fn status(one_of: &[&str]) -> ValueTypeDecl {
    serde_json::from_value(json!({"name": "status", "kind": "text", "one_of": one_of})).unwrap()
}

/// A lake of `size()` tasks, every other one `open`, the rest `done`.
async fn lake(store: &dyn LakeStore) -> Lake {
    let l = Lake::of(store);
    let d = Declaration::ValueType(status(&["open", "done"]));
    l.declare_as(d, "status").await.unwrap();
    let mut state = sourced("state", StorageKind::Text);
    state.ty.value_type = Some("status".into());
    l.declare(type_decl(
        "task",
        vec![state, sourced("n", StorageKind::Number)],
    ))
    .await
    .unwrap();
    let records = (0..size()).map(|i| {
        let state = if i % 2 == 0 { "open" } else { "done" };
        source_record("task", &format!("t{i:06}"), json!({"state": state, "n": i}))
    });
    l.ingest("load", records.collect()).await.unwrap();
    l
}

/// Renames `open` to `active` in one migration.
async fn renamed(store: &dyn LakeStore) {
    let l = lake(store).await;
    let m = ValueTypeMigration {
        to: status(&["active", "done"]),
        renames: vec![("open".into(), "active".into())],
    };
    let report = l
        .cat
        .migrate_value_type(&l.own, m.clone())
        .await
        .unwrap()
        .data;
    let live = HEAP.current_usage();
    HEAP.reset_peak_usage();
    let t = Instant::now();
    l.cat
        .migrate_value_type_apply(&l.own, m, &report.report_id)
        .await
        .unwrap();
    let held = t.elapsed();
    let peak = HEAP.peak_usage().saturating_sub(live) / 1_000_000;
    println!(
        "sqlite: {} records, half renamed by one migration: writer held {held:?}, peak {peak} MB",
        size()
    );
}

/// Widens `status`, writes `active` into every open task through ingest's
/// windows, and tightens it.
async fn rewritten(store: &dyn LakeStore) {
    let l = lake(store).await;
    let live = HEAP.current_usage();
    HEAP.reset_peak_usage();
    let t = Instant::now();
    let wide = Declaration::ValueType(status(&["open", "active", "done"]));
    l.declare_as(wide, "widened").await.unwrap();
    let records = (0..size()).step_by(2).map(|i| {
        source_record(
            "task",
            &format!("t{i:06}"),
            json!({"state": "active", "n": i}),
        )
    });
    l.ingest("load", records.collect()).await.unwrap();
    let to = ValueTypeMigration::from(status(&["active", "done"]));
    let report = l
        .cat
        .migrate_value_type(&l.own, to.clone())
        .await
        .unwrap()
        .data;
    l.cat
        .migrate_value_type_apply(&l.own, to, &report.report_id)
        .await
        .unwrap();
    let took = t.elapsed();
    let peak = HEAP.peak_usage().saturating_sub(live) / 1_000_000;
    println!(
        "sqlite: {} records, half written again and tightened: {took:?}, peak {peak} MB",
        size()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "measures a rename over 100 000 records; run by hand with --release"]
async fn renaming_a_value_of_a_hundred_thousand_records() {
    for renames in [true, false] {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("value-renamed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let seams = Seams {
            engines: vec![Arc::new(SqliteOpener)],
            ..Seams::default()
        };
        let location = Location::sqlite("renamed", &dir);
        let store: Arc<dyn LakeStore> = lakelands_store::open(location, seams, OpenMode::Create)
            .await
            .unwrap();
        match renames {
            true => renamed(store.as_ref()).await,
            false => rewritten(store.as_ref()).await,
        }
        store.close().await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
