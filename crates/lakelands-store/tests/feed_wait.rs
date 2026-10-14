//! A wait on the feed of a read-only handle beside the lake's writer
//! (lakelands#1061): its head moves only when it opens again, so its wait
//! ends at its bound, answering the head its poll reads up to.

use std::sync::Arc;
use std::time::{Duration, Instant};

use lakelands_api::engine::OpenMode;
use lakelands_api::ontology::StorageKind::Text;
use lakelands_api::seam::{FeedFilter, Seams};
use lakelands_api::{LakeStore, Location};
use lakelands_core::conformance::fixture::{Lake, type_of};
use lakelands_engine_mem::MemoryOpener;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_only_handles_wait_ends_at_its_bound() {
    let opener = MemoryOpener::new();
    let seams = || Seams {
        engines: vec![Arc::new(opener.clone())],
        ..Seams::default()
    };
    let location = Location::memory("waits");
    let open = |mode| lakelands_store::open(location.clone(), seams(), mode);
    let writer: Arc<dyn LakeStore> = open(OpenMode::Create).await.expect("the writer");
    let l = Lake::of(writer.as_ref());
    let note = type_of("note", &[("text", Text)]);
    l.declare(note).await.expect("declaring `note`");
    let reader: Arc<dyn LakeStore> = open(OpenMode::Read).await.expect("a read-only handle");
    let head = reader.head().await.expect("its head");
    let bound = Duration::from_millis(300);
    let (feed, t) = (reader.feed(), Instant::now());
    let (woke, written) = tokio::join!(
        feed.wait(head, bound),
        l.upsert("note", "n1", json!({"text": "hello"}))
    );
    let (woke, waited) = (woke.expect("waiting"), t.elapsed());
    let written = written.expect("writing").position();
    let all = FeedFilter::default();
    let batch = feed.poll(head.next(), &all, 10).await.expect("polling");
    assert!(
        written > head && woke == head && waited >= bound && batch.events.is_empty(),
        "the writer appended at {written}; the read-only handle's wait past {head} answered \
         {woke} after {waited:?}, and its poll answers {} events",
        batch.events.len()
    );
}
