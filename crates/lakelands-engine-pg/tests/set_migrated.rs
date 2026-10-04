//! What a migration costs on PostgreSQL when it decides the membership
//! of the records it migrates: the time `migrate_apply` holds the writer
//! and the peak memory of its append, at 100 000 live records, with no
//! set, one set and three sets, changing no membership and changing
//! 20 000, each on a fresh lake. By hand, with `LAKELANDS_TEST_PG_URL` set: `cargo test
//! --release -p lakelands-engine-pg --test set_migrated -- --ignored
//! --nocapture`.

mod common;

use std::sync::Arc;
use std::time::Instant;

use lakelands_api::engine::OpenMode;
use lakelands_api::ontology::StorageKind::{Boolean, Number, Text};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::seam::{FeedFilter, Seams};
use lakelands_api::{LakeStore, Position};
use lakelands_core::conformance::fixture::{
    Lake, migration, object_set_on, source_record, sourced, type_decl,
};
use lakelands_engine_pg::PgOpener;
use peak_alloc::PeakAlloc;
use serde_json::json;

#[global_allocator]
static HEAP: PeakAlloc = PeakAlloc;

/// How many records a cell loads: 100 000, or `LAKELANDS_TEST_RECORDS`.
fn size() -> u64 {
    let asked = std::env::var("LAKELANDS_TEST_RECORDS").ok();
    asked.and_then(|n| n.parse().ok()).unwrap_or(100_000)
}

/// The records `flag` holds for: the lake's default ceiling of membership
/// changes.
const MOVED: u64 = 20_000;
/// The cells: how many sets the type has, whether the migration converts
/// the field one of them reads (`flag`) or one none reads (`f0`), and
/// whether it re-keys the type by `n` instead, which moves every record.
const CELLS: [(usize, bool, bool); 6] = [
    (0, false, false),
    (1, false, false),
    (1, true, false),
    (3, false, false),
    (3, true, false),
    (0, false, true),
];

/// The cells to run: all, or those `LAKELANDS_TEST_CELLS` numbers from 0
/// (`0,2,5`).
fn cells() -> Vec<(usize, bool, bool)> {
    let Ok(asked) = std::env::var("LAKELANDS_TEST_CELLS") else {
        return CELLS.to_vec();
    };
    let numbered = asked
        .split(',')
        .filter_map(|i| i.trim().parse::<usize>().ok());
    numbered.filter_map(|i| CELLS.get(i).copied()).collect()
}

/// `invoice`, with `boolean` converted from text, keyed by `n` when `rekeyed`.
fn invoice(boolean: Option<&str>, rekeyed: bool) -> TypeDecl {
    let kind = |f: &str| if boolean == Some(f) { Boolean } else { Text };
    let flags = ["f0", "flag", "flagb"].map(|f| sourced(f, kind(f)));
    let mut n = sourced("n", Number);
    n.required = true;
    let mut ty = type_decl("invoice", [vec![n], flags.to_vec()].concat());
    ty.key.extend(rekeyed.then(|| "n".into()));
    ty
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
/// applies one migration, which changes `MOVED` memberships when `moves`
/// and moves every record to the key `n` makes when `rekeyed`.
/// Prints how long `migrate_apply` held the writer and its peak memory.
async fn cell(engine: &str, store: &dyn LakeStore, (sets, moves, rekeyed): (usize, bool, bool)) {
    let l = Lake::of(store);
    l.declare(invoice(None, false)).await.unwrap();
    let records = (0..size()).map(|i| {
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
    let m = migration(invoice((!rekeyed).then_some(convert), rekeyed));
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
        "{engine}: {} records, {sets} set(s), {}, {changed} memberships changed: writer held \
         {held:?}, peak memory {} MB",
        size(),
        if rekeyed {
            "re-keying by `n`".into()
        } else {
            format!("converting `{convert}`")
        },
        peak / 1_000_000
    );
    assert_eq!(changed as u64, if moves { MOVED.min(size()) } else { 0 });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "measures migrations over 100 000 records; run by hand with --release"]
async fn migrating_a_hundred_thousand_records_under_sets() {
    let Some(url) = common::url_or_skip("the pg set-migration measurement") else {
        return;
    };
    for c in cells() {
        let schema = common::fresh_schema();
        let seams = Seams {
            engines: vec![Arc::new(PgOpener)],
            ..Seams::default()
        };
        let location = common::location(&url, "sets", &schema);
        let store: Arc<dyn LakeStore> = lakelands_store::open(location, seams, OpenMode::Create)
            .await
            .unwrap();
        cell("pg", store.as_ref(), c).await;
        store.close().await.unwrap();
        common::drop_schemas(&url);
    }
}
