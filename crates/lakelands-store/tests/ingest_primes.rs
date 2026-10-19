//! An ingest reads the records its window names, and the targets its
//! written references name, a type at a time: no record of either is
//! read alone (lakelands#1167).

mod probed;

use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::error::Case;
use lakelands_api::ontology::StorageKind::Text;
use lakelands_api::seam::Seams;
use lakelands_api::{Error, Location};
use lakelands_core::conformance::fixture::{
    Lake, field, reference, source_record, sourced, type_decl,
};
use lakelands_engine_mem::MemoryOpener;
use probed::{Probe, ProbedOpener};
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_target_of_a_written_reference_is_read_alone() {
    let probe = Arc::new(Probe::default());
    let seams = Seams {
        engines: vec![Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()))],
        ..Seams::default()
    };
    let store = lakelands_store::open(Location::memory("primes"), seams, OpenMode::Create)
        .await
        .unwrap();
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("p", vec![field("name", Text)]))
        .await
        .unwrap();
    let r = lakelands_api::ontology::FieldDecl {
        authority: lakelands_api::ontology::Authority::Source,
        ..reference("r", "p")
    };
    l.declare(type_decl("c", vec![sourced("a", Text), r]))
        .await
        .unwrap();
    for i in 0..5 {
        l.upsert("p", &format!("p{i}"), json!({"name": "x"}))
            .await
            .unwrap();
    }
    let record =
        |i: usize, to: &str| source_record("c", &format!("c{i}"), json!({"a": "x", "r": to}));
    let window: Vec<_> = (0..30).map(|i| record(i, &format!("p{}", i % 5))).collect();
    probe.alone.lock().unwrap().clear();
    l.ingest("feed", window).await.unwrap();
    let alone = probe.alone.lock().unwrap().clone();
    assert!(alone.is_empty(), "records read alone: {alone:?}");

    // A reference to no live record is still refused, naming its field.
    let refused = l.ingest("feed", vec![record(99, "nobody")]).await;
    let on_r = matches!(&refused, Err(Error::Refused { field: Some(f), .. }) if f.as_str() == "r");
    assert!(on_r, "{refused:?}");
    let case = refused.unwrap_err().case().map(|s| s.case.clone());
    assert!(matches!(case, Some(Case::NotLive { .. })), "{case:?}");
}
