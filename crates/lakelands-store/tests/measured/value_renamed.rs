//! What renaming a value costs, measured alike on each durable engine: the
//! time `migrate_value_type_apply` holds the writer and the peak memory of
//! its append, at 100 000 records, half of them holding the value renamed,
//! beside the way a host had before renames: widen the value type, write
//! every record that holds the old value, tighten it. Each engine's
//! `tests/value_renamed.rs` opens the lakes and runs both.

use std::time::Instant;

use lakelands_api::LakeStore;
use lakelands_api::ontology::{StorageKind, ValueTypeDecl};
use lakelands_api::write::{Declaration, ValueTypeMigration};
use lakelands_core::conformance::fixture::{Lake, source_record, sourced, type_decl};
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
pub async fn renamed(engine: &str, store: &dyn LakeStore) {
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
        "{engine}: {} records, half renamed by one migration: writer held {held:?}, peak {peak} MB",
        size()
    );
}

/// Widens `status`, writes `active` into every open task through ingest's
/// windows, and tightens it.
pub async fn rewritten(engine: &str, store: &dyn LakeStore) {
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
        "{engine}: {} records, half written again and tightened: {took:?}, peak {peak} MB",
        size()
    );
}
