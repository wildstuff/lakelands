//! What a migration costs on SQLite when it decides the membership of the
//! records it migrates: the time `migrate_apply` holds the writer and the
//! peak memory of its append, at 100 000 live records, with no set, one
//! set and three sets, changing no membership and changing 20 000, each
//! on a fresh lake. By
//! hand: `cargo test --release -p lakelands-engine-sqlite --test
//! set_migrated -- --ignored --nocapture`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use lakelands_api::engine::OpenMode;
use lakelands_api::ontology::StorageKind::{Boolean, Number, Text};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::seam::{FeedFilter, Seams};
use lakelands_api::{LakeStore, Location, Position};
use lakelands_core::conformance::fixture::{
    Lake, migration, object_set_on, source_record, sourced, type_decl,
};
use lakelands_engine_sqlite::SqliteOpener;
use peak_alloc::PeakAlloc;
use serde_json::json;

#[global_allocator]
static HEAP: PeakAlloc = PeakAlloc;

const RECORDS: u64 = 100_000;
/// The records `flag` holds for: the lake's default ceiling of membership
/// changes.
const MOVED: u64 = 20_000;
/// The cells: how many sets the type has, and whether the migration
/// converts the field one of them reads (`flag`) or one none reads (`f0`).
const CELLS: [(usize, bool); 5] = [(0, false), (1, false), (1, true), (3, false), (3, true)];

/// `invoice`, with `boolean` converted from text.
fn invoice(boolean: Option<&str>) -> TypeDecl {
    let kind = |f: &str| if boolean == Some(f) { Boolean } else { Text };
    let flags = ["f0", "flag", "flagb"].map(|f| sourced(f, kind(f)));
    type_decl(
        "invoice",
        [vec![sourced("n", Number)], flags.to_vec()].concat(),
    )
}

/// The `object_set_changed` events from `from` on.
async fn changes(store: &dyn LakeStore, from: Position) -> usize {
    let only = FeedFilter {
        kinds: vec!["object_set_changed".into()],
        ..FeedFilter::default()
    };
    let (mut next, mut n) = (from, 0);
    loop {
        let b = store.feed().poll(next, &only, 10_000).await.unwrap();
        if b.events.is_empty() {
            return n;
        }
        n += b.events.len();
        next = b.next;
    }
}

/// One cell, on a fresh lake: loads the records, declares `sets` sets and
/// applies one migration, which changes `MOVED` memberships when `moves`.
/// Prints how long `migrate_apply` held the writer and its peak memory.
async fn cell(engine: &str, store: &dyn LakeStore, sets: usize, moves: bool) {
    let l = Lake::of(store);
    l.declare(invoice(None)).await.unwrap();
    let records = (0..RECORDS).map(|i| {
        let held = if i < MOVED { "true" } else { "false" };
        let fields = json!({"n": i, "f0": "true", "flag": held, "flagb": held});
        source_record("invoice", &format!("i{i:06}"), fields)
    });
    l.ingest("load", records.collect()).await.unwrap();
    let declared = [
        ("word", "flag = 'true'"),
        ("wordb", "flagb = 'true'"),
        ("low", "n < 10000"),
    ];
    for (name, filter) in &declared[..sets] {
        l.declare_as(object_set_on(name, "invoice", filter), name)
            .await
            .unwrap();
    }
    let convert = if moves { "flag" } else { "f0" };
    let m = migration(invoice(Some(convert)));
    let report = l.cat.migrate(&l.own, m.clone()).await.unwrap().data;
    let from = store.head().await.unwrap().next();
    let live = HEAP.current_usage();
    HEAP.reset_peak_usage();
    let t = Instant::now();
    l.cat
        .migrate_apply(&l.own, m, &report.report_id)
        .await
        .unwrap();
    let held = t.elapsed();
    let peak = HEAP.peak_usage().saturating_sub(live);
    let changed = changes(store, from).await;
    println!(
        "{engine}: {RECORDS} records, {sets} set(s), converting `{convert}`, {changed} \
         memberships changed: writer held {held:?}, peak memory {} MB",
        peak / 1_000_000
    );
    assert_eq!(changed as u64, if moves { MOVED } else { 0 });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "measures migrations over 100 000 records; run by hand with --release"]
async fn migrating_a_hundred_thousand_records_under_sets() {
    for (sets, moves) in CELLS {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("set-migrated-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let seams = Seams {
            engines: vec![Arc::new(SqliteOpener)],
            ..Seams::default()
        };
        let location = Location::sqlite("sets", &dir);
        let store: Arc<dyn LakeStore> = lakelands_store::open(location, seams, OpenMode::Create)
            .await
            .unwrap();
        cell("sqlite", store.as_ref(), sets, moves).await;
        store.close().await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
