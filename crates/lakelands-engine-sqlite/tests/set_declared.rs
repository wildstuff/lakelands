//! What declaring an object set over a large type costs on SQLite: the
//! time the writer is held, the peak memory of its append and the
//! append's size on disk, at 100 000 live records of which half join.
//! By hand: `cargo test --release -p lakelands-engine-sqlite --test
//! set_declared -- --ignored --nocapture`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use lakelands_api::engine::OpenMode;
use lakelands_api::event::{Event, EventBody};
use lakelands_api::ontology::StorageKind::{Number, Text};
use lakelands_api::seam::{FeedFilter, Seams};
use lakelands_api::{LakeStore, Location};
use lakelands_core::conformance::fixture::{
    Lake, object_set_on, source_record, sourced, type_decl,
};
use lakelands_engine_sqlite::SqliteOpener;
use peak_alloc::PeakAlloc;
use serde_json::json;

#[global_allocator]
static HEAP: PeakAlloc = PeakAlloc;

const RECORDS: u64 = 100_000;

/// The bytes of every file under `dir`.
fn size(dir: &Path) -> u64 {
    let entries = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap());
    entries
        .map(|e| match e.metadata().unwrap() {
            m if m.is_dir() => size(&e.path()),
            m => m.len(),
        })
        .sum()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "measures a set declared over 100 000 records; run by hand with --release"]
async fn declaring_a_set_over_a_hundred_thousand_records() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("set-declared-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let seams = Seams {
        engines: vec![Arc::new(SqliteOpener)],
        ..Seams::default()
    };
    let location = Location::sqlite("sets", &dir);
    let store: Arc<dyn LakeStore> = lakelands_store::open(location, seams, OpenMode::Create)
        .await
        .unwrap();
    let l = Lake::of(store.as_ref());
    let fields = vec![sourced("status", Text), sourced("n", Number)];
    l.declare(type_decl("invoice", fields)).await.unwrap();
    let records = (0..RECORDS).map(|i| {
        let status = if i % 2 == 0 { "open" } else { "paid" };
        source_record(
            "invoice",
            &format!("i{i:06}"),
            json!({"status": status, "n": i}),
        )
    });
    l.ingest("load", records.collect()).await.unwrap();
    // The walk alone (nothing joins), a quarter joining, half joining.
    for (name, filter, want) in [
        ("none", "status = 'none'", 0),
        ("quarter", "n < 25000", RECORDS / 4),
        ("open", "status = 'open'", RECORDS / 2),
    ] {
        let from = store.head().await.unwrap().next();
        let (before, live) = (size(&dir), HEAP.current_usage());
        HEAP.reset_peak_usage();
        let t = Instant::now();
        l.declare_as(object_set_on(name, "invoice", filter), name)
            .await
            .unwrap();
        let held = t.elapsed();
        let peak = HEAP.peak_usage().saturating_sub(live);
        let after = size(&dir);
        let (mut next, mut joined) = (from, 0);
        loop {
            let only = FeedFilter {
                kinds: vec!["object_set_changed".into()],
                ..FeedFilter::default()
            };
            let b = store.feed().poll(next, &only, 10_000).await.unwrap();
            if b.events.is_empty() {
                break;
            }
            let join =
                |e: &&Event| matches!(e.body, EventBody::ObjectSetChanged { joined: true, .. });
            joined += b.events.iter().filter(join).count();
            next = b.next;
        }
        println!(
            "sqlite: {RECORDS} records, set `{name}`, {joined} joined: writer held {held:?}, \
             peak memory {} MB, on disk {} MB",
            peak / 1_000_000,
            (after as i64 - before as i64).max(0) / 1_000_000
        );
        assert_eq!(joined as u64, want);
    }
    store.close().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
