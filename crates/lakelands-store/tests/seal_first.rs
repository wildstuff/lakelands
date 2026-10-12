//! What the store hands a seal as the next segment's first events
//! (lakelands#623).
//!
//! An import seals each file after the first with that file's first
//! events, in the engine's one step, and checks the digest the engine
//! takes of the segment before against the file's inside that step: a
//! digest that differs is found before anything is sealed. A `checkpoint`
//! hands its event, built for the digest it is handed.

mod probed;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::seam::Seams;
use lakelands_api::{Grants, Location, Principal, PrincipalKind, WriteContext};
use lakelands_engine_mem::MemoryOpener;
use probed::{Probe, ProbedOpener};
use serde_json::json;

fn seams(opener: impl EngineOpener + 'static) -> Seams {
    Seams {
        engines: vec![Arc::new(opener)],
        ..Seams::default()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_import_finds_a_digest_that_differs_before_anything_is_sealed() {
    // A lake with one checkpoint: an export of two files.
    let from = Location::memory("ledger");
    let store = lakelands_store::open(from, seams(MemoryOpener::new()), OpenMode::Create)
        .await
        .unwrap();
    let owner = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: "ledger".into(),
        grants: Grants::owner(),
    };
    let cat = store.catalog(WriteContext::default());
    let declared = json!({"declare": "type", "name": "entry",
        "fields": [{"name": "note", "type": {"kind": "text"}}]});
    cat.declare(&owner, serde_json::from_value(declared).unwrap())
        .await
        .unwrap();
    let sealed = store.checkpoint().await.unwrap();
    let patch = serde_json::from_value(json!({"note": "after"})).unwrap();
    cat.upsert(
        &owner,
        &"entry".into(),
        &"a".into(),
        patch,
        Default::default(),
    )
    .await
    .unwrap();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("import-seal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let exported = lakelands_core::export::write(store.as_ref(), &dir).await;
    assert_eq!(exported.unwrap().segments.len(), 2);

    // An engine whose seal hands another digest than the file's.
    let (memory, probe) = (MemoryOpener::new(), Arc::new(Probe::default()));
    probe.another_digest.store(true, Ordering::SeqCst);
    let to = Location::memory("ledger");
    let through = seams(ProbedOpener(memory.clone(), probe));
    let stopped = lakelands_store::import(to.clone(), through, dir.clone()).await;
    let stopped = stopped.unwrap_err().to_string();
    assert!(
        stopped.contains("digests the segment before it") && stopped.contains("import stopped"),
        "{stopped}"
    );
    // The lake holds the first file's events and no sealed segment.
    let engine = memory.open(&to, OpenMode::Read).unwrap();
    assert_eq!(engine.segments().unwrap(), vec![], "{stopped}");
    let tip = engine.tip().unwrap().unwrap().0;
    assert_eq!(tip.0, sealed.0 - 1, "{stopped}");
    let _ = std::fs::remove_dir_all(&dir);
}
