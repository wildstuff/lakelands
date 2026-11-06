//! The active segment is sealed by size where the store may seal it, and
//! nowhere else (`docs/decisions/074-a-segment-is-sealed-by-size.md`),
//! the size its lines and its blocks take inflated, never its blocks'
//! stored bytes (`docs/decisions/098-a-lake-stores-what-it-cannot-derive.md`).
//!
//! A seal the disk refuses leaves the write that crossed the bound
//! answered as it landed, and is tried after the next append. A
//! read-only open and an import seal nothing, however far past the bound
//! the segment is; the next writer's open seals it.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::event::EventBody;
use lakelands_api::ontology::StorageKind::Text;
use lakelands_api::seam::Seams;
use lakelands_api::{LakeStore, Location};
use lakelands_core::conformance::fixture::{
    Lake, log_events, source_record, sourced, type_decl, type_of,
};
use lakelands_engine_sqlite::SqliteOpener;
use serde_json::json;

fn seams() -> Seams {
    Seams {
        engines: vec![Arc::new(SqliteOpener)],
        ..Seams::default()
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("sealed-by-size-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The lake at `dir`, sealed by size past `bound` (1 MiB at least).
fn at(dir: &Path, bound: u64) -> Location {
    let mut at = Location::sqlite("ledger", dir.to_owned());
    at.limits.seal_after_bytes = bound;
    at
}

/// Notes `from..to` of 300 KiB each: four take a segment past 1 MiB.
async fn notes(store: &dyn LakeStore, from: u32, to: u32) {
    let l = Lake::of(store);
    if from == 0 {
        l.declare(type_of("note", &[("text", Text)])).await.unwrap();
    }
    for i in from..to {
        let text = "n".repeat(300 << 10);
        l.upsert("note", &format!("n{i}"), json!({ "text": text }))
            .await
            .unwrap();
    }
}

/// Where the lake's checkpoints stand.
async fn checkpoints(store: &dyn LakeStore) -> Vec<u64> {
    let log = log_events(store).await.unwrap();
    let sealed = log
        .iter()
        .filter(|e| matches!(e.body, EventBody::Checkpoint { .. }));
    sealed.map(|e| e.position.0).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_seal_the_disk_refuses_is_tried_after_the_next_append() {
    let dir = scratch("refused");
    let store = lakelands_store::open(at(&dir, 1), seams(), OpenMode::Create)
        .await
        .unwrap();
    notes(store.as_ref(), 0, 3).await;
    // The seal writes the next segment's index first: no file is made in
    // `index/` while it is read-only, and the seal fails before its step.
    let index = dir.join("index");
    let mode = |m| std::fs::set_permissions(&index, std::fs::Permissions::from_mode(m));
    mode(0o555).unwrap();
    let crossed = Lake::of(store.as_ref())
        .upsert("note", "n3", json!({ "text": "n".repeat(300 << 10) }))
        .await;
    mode(0o755).unwrap();
    assert!(
        crossed.is_ok(),
        "the write that crossed the bound landed: {crossed:?}"
    );
    assert_eq!(checkpoints(store.as_ref()).await, Vec::<u64>::new());
    let next = Lake::of(store.as_ref())
        .upsert("note", "n4", json!({ "text": "after" }))
        .await
        .unwrap()
        .position();
    assert_eq!(checkpoints(store.as_ref()).await, vec![next.0 + 1]);
    store.close().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_open_and_an_import_seal_nothing_and_the_next_writer_does() {
    let base = scratch("read-only");
    let (lake, out, imported) = (
        base.join("lake"),
        base.join("export"),
        base.join("imported"),
    );
    let store = lakelands_store::open(at(&lake, 0), seams(), OpenMode::Create)
        .await
        .unwrap();
    notes(store.as_ref(), 0, 4).await;
    let written = lakelands_core::export::write(store.as_ref(), &out).await;
    written.unwrap();
    store.close().await.unwrap();
    drop(store);
    let imports = lakelands_store::import(at(&imported, 1), seams(), out);
    imports.await.unwrap();
    for dir in [&lake, &imported] {
        let read = lakelands_store::open(at(dir, 1), seams(), OpenMode::Read)
            .await
            .unwrap();
        assert_eq!(
            checkpoints(read.as_ref()).await,
            Vec::<u64>::new(),
            "{dir:?}"
        );
        read.close().await.unwrap();
        drop(read);
        let writer = lakelands_store::open(at(dir, 1), seams(), OpenMode::Write)
            .await
            .unwrap();
        assert_eq!(checkpoints(writer.as_ref()).await.len(), 1, "{dir:?}");
        writer.close().await.unwrap();
    }
    let _ = std::fs::remove_dir_all(&base);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_segment_of_blocks_is_sealed_by_what_its_blocks_inflate_to() {
    let dir = scratch("inflated");
    let store = lakelands_store::open(at(&dir, 1), seams(), OpenMode::Create)
        .await
        .unwrap();
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("reading", vec![sourced("text", Text)]))
        .await
        .unwrap();
    // Deliveries of 1 000 readings of a KiB each that zlib takes to a few
    // bytes, each one append of blocks, until the segment is sealed.
    let mut delivered = 0;
    while checkpoints(store.as_ref()).await.is_empty() {
        assert!(delivered < 4_000, "sealed by 4 000 readings");
        let records = (delivered..delivered + 1_000)
            .map(|i| {
                let text = format!("{i}{}", "x".repeat(1 << 10));
                source_record("reading", &format!("r{i}"), json!({ "text": text }))
            })
            .collect();
        let opts = lakelands_api::IngestOpts {
            commit_window: Some(1_000),
            ..lakelands_api::IngestOpts::default()
        };
        let source = lakelands_api::SourceId::from("meter");
        lakelands_api::ingest_all_with(l.cat.as_ref(), &l.own, &source, opts, records)
            .await
            .unwrap();
        delivered += 1_000;
    }
    store.close().await.unwrap();
    // The sealed segment: its lines and its blocks inflated take a MiB
    // and more, and the file that holds them a fraction of that.
    let sealed = dir.join("log").join(format!("{:020}.jsonl", 0));
    let file = std::fs::read(&sealed).unwrap();
    let (mut units, mut raw) = (&file[..], 0);
    while let Some(unit) = lakelands_core::log::read_unit(&mut units).unwrap() {
        raw += unit.raw();
    }
    assert!(
        raw >= 1 << 20 && (file.len() as u64) < raw / 4,
        "a segment sealed at {raw} bytes inflated, {} stored",
        file.len()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
