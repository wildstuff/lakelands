//! What a thinning gives back on the SQLite engine, `THIN_BYTES_RETURNED`
//! (`docs/decisions/102-a-thinned-series-keeps-its-roll-ups.md`).
//!
//! The conformance suite holds what a thinning answers; the bytes it
//! returns to the file are this engine's own measure, held only here.
//! `tick`, 4 000 records with a note of 200 bytes, three in four before
//! the horizon, is thinned: the act's report names the projection's bytes
//! before and after its reclaim, the file and its journal are smaller
//! than they were, and it keeps a tombstone a key. The test prints what it
//! measured, the sizes and the times `docs/hosting.md` states, for a run
//! by hand:
//! `cargo test -p lakelands-engine-sqlite --test thin_bytes -- --nocapture`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use lakelands_api::engine::OpenMode;
use lakelands_api::ontology::StorageKind;
use lakelands_api::seam::Seams;
use lakelands_api::write::Thin;
use lakelands_api::{LakeStore, Location, Timestamp, TypeName};
use lakelands_core::conformance::fixture::{Lake, field, type_decl};
use lakelands_engine_sqlite::SqliteOpener;
use serde_json::json;

fn len(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |m| m.len())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_thinning_gives_its_bytes_back_to_the_file() {
    let dir =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("thin-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let seams = Seams {
        engines: vec![Arc::new(SqliteOpener)],
        ..Seams::default()
    };
    let store: Arc<dyn LakeStore> =
        lakelands_store::open(Location::sqlite("thin", &dir), seams, OpenMode::Create)
            .await
            .unwrap();
    let l = Lake::of(store.as_ref());
    let fields = vec![
        field("at", StorageKind::DateTime),
        field("note", StorageKind::Text),
    ];
    l.declare(type_decl("tick", fields)).await.unwrap();
    let note = "x".repeat(200);
    for i in 0..4000 {
        let at = if i % 4 == 3 {
            "2026-07-01"
        } else {
            "2026-05-01"
        };
        let fields = json!({"at": format!("{at}T00:00:{:02}Z", i % 60), "note": note});
        l.upsert("tick", &format!("t{i:04}"), fields).await.unwrap();
    }
    let (db, wal) = (dir.join("projection.db"), dir.join("projection.db-wal"));
    let (file, journal) = (len(&db), len(&wal));
    let stored = file + journal;
    let (ty, thin) = (
        TypeName::from("tick"),
        Thin {
            by: "at".into(),
            before: Timestamp::parse("2026-06-01T00:00:00Z").unwrap(),
        },
    );
    let clock = Instant::now();
    let preflight = store.thin_values(&ty, &thin, true).await.unwrap();
    let asked = clock.elapsed();
    let clock = Instant::now();
    let report = store.thin_values(&ty, &thin, false).await.unwrap();
    let took = clock.elapsed();
    let left = len(&db) + len(&wal);
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY;
    let tombs: (i64, i64) = rusqlite::Connection::open_with_flags(&db, flags)
        .unwrap()
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(length(ty) + length(key) + 16), 0) FROM thinned",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let reclaimed = report.reclaimed.clone().expect("the act reclaims");
    let (before, after) = (reclaimed.before.unwrap(), reclaimed.after.unwrap());
    println!(
        "thin_bytes: {} records taken; preflight {asked:?}, act {took:?}; \
         file {file} and journal {journal} bytes before, {left} after; reclaim {before} -> {after}; \
         {} tombstones of {} bytes",
        report.records, tombs.0, tombs.1
    );
    assert_eq!(tombs.0, 3000, "a tombstone a key");
    assert_eq!((preflight.records, report.records), (3000, 3000));
    assert!(
        after < before,
        "the reclaim gives bytes back: {reclaimed:?}"
    );
    assert!(
        left < stored,
        "the file and its journal shrink: {stored} -> {left}"
    );
    drop((l, store));
    let _ = std::fs::remove_dir_all(&dir);
}
