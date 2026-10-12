//! What the log holds when many records share a few texts and an
//! embedding is declared over them: one vector per text, and for every
//! other record a fill that names it
//! (`docs/decisions/053-a-recorded-output-is-recorded-once.md`). The
//! vectors are as wide in JSON as a real model's, so the bound holds the
//! size, not only the count.

use std::sync::Arc;

use async_trait::async_trait;
use lakelands_api::engine::OpenMode;
use lakelands_api::event::EventBody;
use lakelands_api::seam::{EmbedBackend, FeedFilter, Seams};
use lakelands_api::{Grants, LakeStore, Location, Principal, PrincipalKind, Result, WriteContext};
use lakelands_engine_mem::MemoryOpener;
use serde_json::json;

const DIMS: u32 = 1536;
const RECORDS: usize = 400;
const TEXTS: usize = 8;

/// Deterministic pseudo-random vectors of [`DIMS`] dimensions.
struct Noise;

#[async_trait]
impl EmbedBackend for Noise {
    fn model(&self) -> &str {
        "noise"
    }
    fn dimensions(&self) -> u32 {
        DIMS
    }
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let vector = |t: &String| {
            let mut x = (t.bytes()).fold(1469598103934665603u64, |h, b| {
                (h ^ b as u64).wrapping_mul(1099511628211)
            });
            let mut next = move || {
                x = x
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((x >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
            };
            (0..DIMS).map(|_| next()).collect()
        };
        Ok(texts.iter().map(vector).collect())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn records_sharing_a_few_texts_leave_one_vector_per_text_in_the_log() {
    let seams = Seams {
        engines: vec![Arc::new(MemoryOpener::new())],
        embed: Some(Arc::new(Noise)),
        ..Seams::default()
    };
    let location = Location::memory("output-bound");
    let store = lakelands_store::open(location, seams, OpenMode::Create);
    let store: Arc<dyn LakeStore> = store.await.expect("a new lake");
    let owner = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: store.lake().clone(),
        grants: Grants::owner(),
    };
    let cat = store.catalog(WriteContext::default());
    let doc = json!({"declare": "type", "name": "doc",
                     "fields": [{"name": "body", "type": {"kind": "text"}}]});
    let doc = serde_json::from_value(doc).expect("a declaration");
    cat.declare(&owner, doc).await.expect("declaring `doc`");
    let ty = "doc".into();
    for i in 0..RECORDS {
        let body = json!({"body": format!("text {} {}", i % TEXTS, "x".repeat(200))});
        let (key, patch) = (
            format!("d{i:04}").into(),
            serde_json::from_value(body).unwrap(),
        );
        let written = cat.upsert(&owner, &ty, &key, patch, Default::default());
        written.await.expect("writing a `doc`");
    }
    let before = store.head().await.expect("the head");
    let near = json!({"name": "near", "type": {"kind": "vector", "dimensions": DIMS},
                      "authority": "derived", "embed": {"from": ["body"], "model": "noise"}});
    let near = serde_json::from_value(near).expect("a field");
    let added = cat.add_field(&owner, &ty, near).await;
    added.expect("adding `near`");
    let outputs = FeedFilter {
        kinds: vec!["output_recorded".into()],
        ..FeedFilter::default()
    };
    let (mut from, mut carried, mut named) = (before.next(), Vec::new(), Vec::new());
    loop {
        let polled = store.feed().poll(from, &outputs, u32::MAX).await;
        let polled = polled.expect("a poll");
        for e in &polled.events {
            let bytes = serde_json::to_vec(e).expect("an event serialises").len();
            match &e.body {
                EventBody::OutputRecorded {
                    output_at: None, ..
                } => carried.push(bytes),
                _ => named.push(bytes),
            }
        }
        if polled.next > polled.head || polled.next == from {
            break;
        }
        from = polled.next;
    }
    assert_eq!((carried.len(), named.len()), (TEXTS, RECORDS - TEXTS));
    // A vector of 1 536 numbers takes some 30 KB; a fill that names one
    // takes well under one.
    let (vectors, fills): (usize, usize) = (carried.iter().sum(), named.iter().sum());
    assert!(
        named.iter().all(|b| *b < 1_000) && fills < vectors,
        "{TEXTS} vectors take {vectors} bytes; the {} fills naming them take {fills}",
        named.len()
    );
}
