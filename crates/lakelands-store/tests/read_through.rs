//! A filtered poll reads the events its index names in runs, and the
//! events between them that a run reads through are dropped unopened:
//! nothing is answered from them; past the view's position it reads every
//! event from the log, as it does where the index decides nothing
//! (lakelands#678). The engine is the memory engine
//! behind the wrapper of `probed`, which hands one event out with its
//! bytes damaged, so that a poll that opened it would refuse the log, and
//! hands the head's view out at an earlier position.

mod probed;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use lakelands_api::engine::OpenMode;
use lakelands_api::ontology::StorageKind::Number;
use lakelands_api::seam::{FeedFilter, Seams};
use lakelands_api::{Error, LakeStore, Location, Position};
use lakelands_core::conformance::fixture::{Lake, field, type_decl};
use lakelands_engine_mem::MemoryOpener;
use probed::{Probe, ProbedOpener};
use serde_json::json;

fn keyed(key: &str) -> FeedFilter {
    FeedFilter {
        types: vec!["doc".into()],
        keys: vec![key.into()],
        ..FeedFilter::default()
    }
}

/// A lake on the probed memory engine whose type `doc` holds `a`, then
/// `b`, then `a` again.
async fn lake(name: &str) -> (Arc<dyn LakeStore>, Arc<Probe>) {
    let probe = Arc::new(Probe::default());
    let seams = Seams {
        engines: vec![Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()))],
        ..Seams::default()
    };
    let location = Location::memory(name);
    let store = lakelands_store::open(location, seams, OpenMode::Create)
        .await
        .unwrap();
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("doc", vec![field("n", Number)]))
        .await
        .unwrap();
    for (key, n) in [("a", 1), ("b", 1), ("a", 2)] {
        l.upsert("doc", key, json!({"n": n})).await.unwrap();
    }
    (store, probe)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_event_read_through_is_dropped_unopened() {
    let (store, probe) = lake("read-through").await;
    let a = store
        .feed()
        .poll(Position(0), &keyed("a"), 10)
        .await
        .unwrap();
    let at: Vec<u64> = a.events.iter().map(|e| e.position.0).collect();
    let [first, last] = at[..] else {
        panic!("`a` has two events: {at:?}")
    };
    assert_eq!(last, first + 2, "`b`'s event stands between them");
    *probe.damaged.lock().unwrap() = Some(Position(first + 1));
    let again = store
        .feed()
        .poll(Position(0), &keyed("a"), 10)
        .await
        .unwrap();
    assert_eq!(
        again.events, a.events,
        "the damaged event between is read through, unopened"
    );
    for (what, filter) in [
        ("by its key", keyed("b")),
        ("with no filter", FeedFilter::default()),
    ] {
        let refused = store.feed().poll(Position(0), &filter, 10).await;
        assert!(
            matches!(refused, Err(Error::Unavailable { .. })),
            "a poll that answers the damaged event, {what}, opens it and refuses the log: \
             {refused:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn past_the_views_position_a_poll_reads_the_log() {
    let (store, probe) = lake("lagging").await;
    let a = store
        .feed()
        .poll(Position(0), &keyed("a"), 10)
        .await
        .unwrap();
    let at: Vec<u64> = a.events.iter().map(|e| e.position.0).collect();
    let [first, last] = at[..] else {
        panic!("`a` has two events: {at:?}")
    };
    *probe.lags.lock().unwrap() = Some(Position(first));
    for (key, want) in [("a", vec![first, last]), ("b", vec![first + 1])] {
        let b = store
            .feed()
            .poll(Position(0), &keyed(key), 10)
            .await
            .unwrap();
        let got: Vec<u64> = b.events.iter().map(|e| e.position.0).collect();
        assert_eq!(
            (got, b.next),
            (want, b.head.next()),
            "`{key}` with the view at {first}"
        );
    }
    *probe.lags.lock().unwrap() = None;
    probe.undecided.store(true, Ordering::SeqCst);
    let created = FeedFilter {
        kinds: vec!["lake_created".into()],
        ..FeedFilter::default()
    };
    for (filter, want) in [(keyed("a"), vec![first, last]), (created, vec![0])] {
        let b = store.feed().poll(Position(0), &filter, 10).await.unwrap();
        let got: Vec<u64> = b.events.iter().map(|e| e.position.0).collect();
        assert_eq!(
            (got, b.next),
            (want, b.head.next()),
            "{filter:?} from 0: an index that decides nothing, as while a rebuild refolds, \
             leaves the poll to the log"
        );
    }
}
