//! A hub serves each follower what its own reads would have.

use std::sync::Arc;
use std::time::Duration;

use lakelands_api::engine::OpenMode;
use lakelands_api::id::{Position, RecordKey, TypeName};
use lakelands_api::principal::{Grants, PrincipalKind};
use lakelands_api::seam::{FeedFilter, Seams};
use lakelands_api::write::{Declaration, WriteOpts};
use lakelands_api::{LakeStore, Location, Principal, WriteContext};
use serde_json::json;

use super::{Follow, Next, caught_up};
use lakelands_api::seam::FeedBatch;

const PACE: (Duration, Duration) = (Duration::from_millis(50), Duration::ZERO);

fn owner() -> Principal {
    Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: "hub".into(),
        grants: Grants::owner(),
    }
}

/// A lake with types `t` and `u`, written to through `write`.
async fn lake() -> Arc<dyn LakeStore> {
    let seams = Seams {
        engines: vec![Arc::new(lakelands_engine_mem::MemoryOpener::new())],
        ..Seams::default()
    };
    let store = lakelands_store::open(Location::memory("hub"), seams, OpenMode::Create)
        .await
        .unwrap();
    let cat = store.catalog(WriteContext::default());
    for ty in ["t", "u"] {
        let d = json!({"declare": "type", "name": ty, "fields": [{"name": "n", "type": {"kind": "number"}}]});
        let d: Declaration = serde_json::from_value(d).unwrap();
        cat.declare(&owner(), d).await.unwrap();
    }
    store
}

async fn write(store: &dyn LakeStore, ty: &str, key: &str, n: u64) {
    let patch = serde_json::from_value(json!({"n": n})).unwrap();
    let cat = store.catalog(WriteContext::default());
    let (t, k) = (TypeName::from(ty), RecordKey::from(key));
    cat.upsert(&owner(), &t, &k, patch, WriteOpts::default())
        .await
        .unwrap();
}

/// The positions a solo reader of `filter` reads from `at` to the head.
async fn solo(store: &dyn LakeStore, filter: &FeedFilter, mut at: Position) -> Vec<u64> {
    let (feed, mut seen) = (store.feed(), Vec::new());
    loop {
        let b = feed.poll(at, filter, 7).await.unwrap();
        seen.extend(b.events.iter().map(|e| e.position.0));
        let caught_up = b.next > b.head || b.next == at;
        at = b.next;
        if caught_up {
            return seen;
        }
    }
}

/// What `f` reads from `at` until it is caught up at `head`, and where it
/// then stands.
async fn follow(f: &mut Follow, mut at: Position, head: Position) -> (Vec<u64>, Position) {
    let mut seen = Vec::new();
    while at <= head {
        let Next::Batch(b) = f.next(at, Duration::from_secs(10)).await.unwrap() else {
            panic!("the hub said nothing within ten seconds, at {at:?} of {head:?}");
        };
        let new = b
            .batch
            .events
            .iter()
            .filter(|e| e.position >= at && f.admits(e));
        seen.extend(new.map(|e| e.position.0));
        at = at.max(b.batch.next);
    }
    (seen, at)
}

fn filter(kinds: &[&str], types: &[&str], keys: &[&str]) -> FeedFilter {
    FeedFilter {
        kinds: kinds.iter().map(|k| (*k).to_owned()).collect(),
        types: types.iter().map(|t| TypeName::from(*t)).collect(),
        keys: keys.iter().map(|k| RecordKey::from(*k)).collect(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn followers_of_one_kinds_and_types_share_a_hub_and_apply_their_keys() {
    let store = lake().await;
    let a = Follow::new(&store, filter(&[], &["t"], &["a"]), PACE)
        .await
        .unwrap();
    let b = Follow::new(&store, filter(&[], &["t"], &["b"]), PACE)
        .await
        .unwrap();
    let c = Follow::new(&store, filter(&[], &["u"], &[]), PACE)
        .await
        .unwrap();
    let other = lake().await;
    let d = Follow::new(&other, filter(&[], &["t"], &[]), PACE)
        .await
        .unwrap();
    assert!(
        Arc::ptr_eq(&a.hub, &b.hub),
        "keys are a follower's, not its hub's"
    );
    assert!(!Arc::ptr_eq(&a.hub, &c.hub), "types choose the hub");
    assert!(
        !Arc::ptr_eq(&a.hub, &d.hub),
        "a lake's handle chooses the hub"
    );
    // Each keyed follower is served its own keys from the one hub's reads.
    let (mut a, mut b) = (a, b);
    let start = store.head().await.unwrap().next();
    for f in [&mut a, &mut b] {
        f.next(start, Duration::from_secs(10)).await.unwrap();
    }
    for i in 0..6u64 {
        write(store.as_ref(), "t", ["a", "b"][(i % 2) as usize], i).await;
    }
    let head = store.head().await.unwrap();
    let (seen_a, _) = follow(&mut a, start, head).await;
    let (seen_b, _) = follow(&mut b, start, head).await;
    assert_eq!(
        seen_a,
        solo(store.as_ref(), &filter(&[], &["t"], &["a"]), start).await
    );
    assert_eq!(
        seen_b,
        solo(store.as_ref(), &filter(&[], &["t"], &["b"]), start).await
    );
    assert!(
        a.reads.0 > 0 && b.reads.0 > 0,
        "both took the hub's batches: {:?}, {:?}",
        a.reads,
        b.reads
    );
}

/// A lake dropped while its hub lives keeps that hub to itself: the hub
/// holds its store, so no new store takes its address, which keys the hub.
#[tokio::test]
async fn a_new_lake_never_joins_a_dropped_lakes_hub() {
    let store = lake().await;
    let old = Follow::new(&store, filter(&[], &[], &[]), PACE)
        .await
        .unwrap();
    let dropped = Arc::downgrade(&store);
    drop(store);
    assert!(
        dropped.upgrade().is_some(),
        "its hub holds the dropped lake"
    );
    let store = lake().await;
    let new = Follow::new(&store, filter(&[], &[], &[]), PACE)
        .await
        .unwrap();
    assert!(
        !Arc::ptr_eq(&old.hub, &new.hub),
        "a new lake joined a dropped lake's hub"
    );
}

/// A follower asked again from inside a batch it took (a pump that sends
/// a batch's last event again) reads alone from there: the hub's next
/// batch begins after it, and would pass over that event.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_follower_asked_from_inside_a_batch_reads_alone_from_there() {
    let store = lake().await;
    let mut f = Follow::new(&store, filter(&[], &[], &[]), PACE)
        .await
        .unwrap();
    let start = store.head().await.unwrap().next();
    f.next(start, Duration::from_secs(10)).await.unwrap();
    write(store.as_ref(), "t", "a", 1).await;
    write(store.as_ref(), "t", "a", 2).await;
    let head = store.head().await.unwrap();
    let (seen, _) = follow(&mut f, start, head).await;
    assert_eq!(f.reads.1, 1, "it took both from the hub: {:?}", f.reads);
    write(store.as_ref(), "t", "a", 3).await;
    let (again, _) = follow(&mut f, head, store.head().await.unwrap()).await;
    assert_eq!(again[0], seen[1], "the event at {head:?} is read again");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_follower_reads_alone_and_catches_up_without_holding_the_others() {
    let store = lake().await;
    let mut slow = Follow::new(&store, filter(&[], &[], &[]), PACE)
        .await
        .unwrap();
    let mut fast = Follow::new(&store, filter(&[], &[], &[]), PACE)
        .await
        .unwrap();
    let start = store.head().await.unwrap().next();
    // Its first read reaches the head, and joins it to the hub.
    let first = slow.next(start, Duration::from_secs(10)).await.unwrap();
    assert!(
        matches!(first, Next::Batch(_)),
        "a first read answers at once"
    );
    let at = start;
    // The fast one follows every append as it comes; the slow one reads
    // none while the hub hands out more batches than it holds.
    let writer = {
        let store = store.clone();
        tokio::spawn(async move {
            for i in 0..3 * super::BACKLOG as u64 {
                write(store.as_ref(), "t", "a", i).await;
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
    };
    let mut fast_at = start;
    let mut fast_seen = Vec::new();
    while !writer.is_finished() || fast_at <= store.head().await.unwrap() {
        if let Ok(Next::Batch(b)) = fast.next(fast_at, Duration::from_millis(200)).await {
            fast_seen.extend(
                b.batch
                    .events
                    .iter()
                    .filter(|e| e.position >= fast_at)
                    .map(|e| e.position.0),
            );
            fast_at = fast_at.max(b.batch.next);
        }
    }
    let head = store.head().await.unwrap();
    let (slow_seen, _) = follow(&mut slow, at, head).await;
    let all = solo(store.as_ref(), &filter(&[], &[], &[]), start).await;
    assert_eq!(fast_seen, all, "the fast follower read every append");
    assert_eq!(slow_seen, all, "the slow follower lost nothing");
    assert!(
        slow.reads.1 >= 2,
        "it read alone again after falling behind: {:?}",
        slow.reads
    );
    assert!(
        fast.reads.0 > slow.reads.0,
        "the fast one took the hub's batches: fast {:?}, slow {:?}",
        fast.reads,
        slow.reads
    );
}

/// A poll stops after `FEED_EXAMINED` events examined, so a batch with no
/// events short of the head is not the end of the log: a reader reads on
/// at once and waits only past the head, or when `next` did not move.
#[test]
fn an_empty_batch_short_of_the_head_is_not_caught_up() {
    let examined = Position(u64::from(lakelands_api::seam::FEED_EXAMINED));
    let batch = |next, head| FeedBatch {
        events: Vec::new(),
        next,
        head,
        overruled: Vec::new(),
        appends: Vec::new(),
    };
    let from = Position::GENESIS;
    assert!(!caught_up(&batch(examined, Position(10_000)), from));
    assert!(caught_up(&batch(Position(10_001), Position(10_000)), from));
    assert!(
        caught_up(&batch(examined, Position(10_000)), examined),
        "a batch whose `next` did not move pauses"
    );
}
