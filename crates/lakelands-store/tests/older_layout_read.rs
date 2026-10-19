//! A reader's open of a projection an older build laid out is refused and
//! rebuilds nothing, while a writer's open rebuilds it (lakelands#1151).
//! The engine is the memory engine behind a wrapper whose opens meet the
//! layout the test names, and which counts the opens for a rebuild.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use lakelands_api::engine::{CanonicalEvent, EngineOpener, LakeEngine, OpenMode};
use lakelands_api::error::Situation;
use lakelands_api::ontology::StorageKind::Number;
use lakelands_api::seam::Seams;
use lakelands_api::{Location, Result};
use lakelands_core::conformance::fixture::{Lake, field, type_decl};
use lakelands_engine_mem::MemoryOpener;
use serde_json::json;

/// The memory engine, its opens other than a rebuild's meeting `laid`.
struct Laid(MemoryOpener, Mutex<Option<u64>>, AtomicU32);

impl EngineOpener for Laid {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn log_ends(&self, at: &Location) -> Result<Option<(CanonicalEvent, CanonicalEvent)>> {
        self.0.log_ends(at)
    }
    fn open(&self, at: &Location, mode: OpenMode) -> Result<Box<dyn LakeEngine>> {
        let laid = *self.1.lock().unwrap();
        match (mode, laid) {
            (OpenMode::Rebuild, _) => {
                self.2.fetch_add(1, Ordering::SeqCst);
                *self.1.lock().unwrap() = None;
            }
            (_, Some(found)) => lakelands_core::engines::layout(&at.lake, Some(found), 2)?,
            _ => {}
        }
        self.0.open(at, mode)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_readers_open_of_an_older_layout_is_refused_and_rebuilds_nothing() {
    let laid = Arc::new(Laid(
        MemoryOpener::new(),
        Mutex::new(None),
        AtomicU32::new(0),
    ));
    let seams = || Seams {
        engines: vec![laid.clone() as Arc<dyn EngineOpener>],
        ..Seams::default()
    };
    let at = || Location::memory("older");
    let store = lakelands_store::open(at(), seams(), OpenMode::Create)
        .await
        .unwrap();
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("t", vec![field("n", Number)]))
        .await
        .unwrap();
    l.upsert("t", "a", json!({"n": 1})).await.unwrap();
    store.close().await.unwrap();
    *laid.1.lock().unwrap() = Some(1);
    let read = lakelands_store::open(at(), seams(), OpenMode::Read).await;
    let situation = read.as_ref().err().and_then(|e| e.situation());
    assert!(
        matches!(
            situation,
            Some(Situation::ProjectionLayout { found: Some(1), .. })
        ),
        "a reader's open of layout 1 is refused as `projection_layout`: {:?}",
        read.err()
    );
    assert_eq!(
        laid.2.load(Ordering::SeqCst),
        0,
        "the reader rebuilt nothing"
    );
    let store = lakelands_store::open(at(), seams(), OpenMode::Write)
        .await
        .unwrap();
    assert!(
        store.rebuilt_at_open().is_some(),
        "the writer's open rebuilds"
    );
    assert_eq!(laid.2.load(Ordering::SeqCst), 1, "once");
    let a = Lake::of(store.as_ref()).read("t", "a").await.unwrap();
    assert_eq!(a.fields["n"], 1, "and reads `a`");
}
