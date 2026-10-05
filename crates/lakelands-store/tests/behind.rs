//! A writer whose fold failed is behind its log (lakelands#603): the
//! events are durable and the projection does not hold them. It catches up
//! before the next write is decided, a fold that cannot begin leaves it
//! behind like any other that fails, and an append that was decided
//! without catching up is refused.
//!
//! Each row writes through the doors of a lake on the memory engine behind
//! the wrapper of `probed`, which refuses the `begin`, the commit or the
//! view the row names; the last one holds the writer itself.

mod probed;

use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::event::EventBody;
use lakelands_api::ontology::StorageKind::Number;
use lakelands_api::ontology::TypeDecl;
use lakelands_api::seam::{FeedFilter, Seams};
use lakelands_api::write::{Declaration, Outcome, WriteOpts};
use lakelands_api::{Error, LakeStore, Location, Position, Version};
use lakelands_core::conformance::fixture::{Lake, derived, field, get_at, patch, type_decl};
use lakelands_engine_mem::MemoryOpener;
use lakelands_store::log::Draft;
use probed::{Probe, ProbedOpener};
use serde_json::{Value, json};

/// A new lake on the probed memory engine, with the engine's own opener
/// for a second handle on it.
async fn lake(name: &str) -> (Arc<dyn LakeStore>, Arc<Probe>, MemoryOpener) {
    let (probe, opener) = (Arc::new(Probe::default()), MemoryOpener::new());
    let seams = Seams {
        engines: vec![Arc::new(ProbedOpener(opener.clone(), probe.clone()))],
        ..Seams::default()
    };
    let store = lakelands_store::open(Location::memory(name), seams, OpenMode::Create)
        .await
        .expect("a new lake");
    (store, probe, opener)
}

/// `doc` with `n` and `plus`, derived from it by `plus`.
fn doc(plus: &str) -> TypeDecl {
    let plus = derived("plus", Number, plus, &["n"]);
    type_decl("doc", vec![field("n", Number), plus])
}

/// An upsert of `doc` `key` with `n`, under an idempotency key or none, its
/// answer as it came.
async fn write(l: &Lake, key: &str, n: i64, under: Option<&str>) -> lakelands_api::Result<Outcome> {
    let opts = WriteOpts {
        idempotency_key: under.map(Into::into),
        ..WriteOpts::default()
    };
    let (doc, key) = ("doc".into(), key.into());
    (l.cat)
        .upsert(&l.own, &doc, &key, patch(json!({"n": n})), opts)
        .await
}

/// The position and the version a write that took effect answers.
fn applied(outcome: lakelands_api::Result<Outcome>) -> (Position, Option<Version>) {
    match outcome {
        Ok(Outcome::Applied {
            position, version, ..
        }) => (position, version),
        other => panic!("the write must take effect: {other:?}"),
    }
}

/// The sentence of a write answered `unavailable`.
fn unavailable(outcome: lakelands_api::Result<Outcome>) -> String {
    match outcome {
        Err(Error::Unavailable { message }) => message,
        other => panic!("the write must be answered `unavailable`: {other:?}"),
    }
}

/// `doc` `key` at `at` (`None`: the head): its fields, its version and the
/// position `plus` was computed at; `absent` where it is not found.
async fn state(l: &Lake, key: &str, at: Option<Position>) -> Value {
    match l.cat.get(&l.own, &"doc".into(), get_at(key, at)).await {
        Ok(r) => {
            let plus = r.data.meta.get("plus").map(|m| m.position.0);
            json!([r.data.fields, r.data.version.0, plus])
        }
        Err(Error::NotFound { .. }) => json!("absent"),
        Err(e) => panic!("the read of `{key}` at {at:?}: {e}"),
    }
}

/// The positions of the events the log holds that write `doc` `key`, and
/// those of its declarations of `doc`.
async fn log(store: &Arc<dyn LakeStore>, key: &str) -> (Vec<u64>, Vec<u64>) {
    let all = FeedFilter::default();
    let polled = store.feed().poll(Position(0), &all, 100).await.unwrap();
    let (mut wrote, mut declared) = (Vec::new(), Vec::new());
    for e in polled.events {
        match &e.body {
            EventBody::RecordUpserted { key: k, .. } if k.as_str() == key => {
                wrote.push(e.position.0);
            }
            EventBody::Declared {
                declaration: Declaration::Type(t),
                ..
            } if t.name.as_str() == "doc" => declared.push(e.position.0),
            _ => {}
        }
    }
    (wrote, declared)
}

/// Row 1. The first `begin` of an append's fold is refused: the write is
/// answered the error and is in the log; the next write folds both. Every
/// read, a second handle's and a rebuild's included, answers the same.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fold_that_cannot_begin_is_folded_before_the_next_write() {
    let (store, probe, opener) = lake("behind-begin").await;
    let l = Lake::of(store.as_ref());
    l.declare(doc("n + 1")).await.unwrap();
    applied(write(&l, "a", 1, None).await);
    *probe.refuse_begin.lock().unwrap() = Some(1);
    unavailable(write(&l, "b", 2, None).await);
    assert_eq!(state(&l, "b", None).await, json!("absent"), "at the mark");
    let (c, _) = applied(write(&l, "c", 3, None).await);
    let b = Position(c.0 - 1);
    assert_eq!(log(&store, "b").await.0, [b.0], "`b` is in the log once");
    let read = |l: Lake| async move {
        let mut read = Vec::new();
        for at in [None, Some(b), Some(c)] {
            read.push((state(&l, "b", at).await, state(&l, "c", at).await));
        }
        read
    };
    let (b_is, c_is) = (
        json!([{"n": 2, "plus": 3}, 1, b.0]),
        json!([{"n": 3, "plus": 4}, 1, c.0]),
    );
    let folded = vec![
        (b_is.clone(), c_is.clone()),
        (b_is.clone(), json!("absent")),
        (b_is, c_is),
    ];
    assert_eq!(
        read(Lake::of(store.as_ref())).await,
        folded,
        "the head, `b`, `c`"
    );
    let seams = Seams {
        engines: vec![Arc::new(opener)],
        ..Seams::default()
    };
    let location = Location::memory("behind-begin");
    let other = lakelands_store::open(location, seams, OpenMode::Read).await;
    let other = other.expect("a second handle");
    assert_eq!(
        read(Lake::of(other.as_ref())).await,
        folded,
        "a second handle"
    );
    store.rebuild().await.expect("a rebuild");
    assert_eq!(read(Lake::of(store.as_ref())).await, folded, "rebuilt");
}

/// Row 2. The fold of a write under a key cannot begin, and neither can
/// the catch-up before its retry: the retry appends nothing. Sent a third
/// time it is answered the first write's outcome, and the log holds it
/// once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retry_of_a_write_whose_fold_could_not_begin_appends_nothing() {
    let (store, probe, _) = lake("behind-begin-twice").await;
    let l = Lake::of(store.as_ref());
    l.declare(doc("n + 1")).await.unwrap();
    for _ in 0..2 {
        *probe.refuse_begin.lock().unwrap() = Some(1);
        unavailable(write(&l, "b", 2, Some("k1")).await);
    }
    let (at, version) = applied(write(&l, "b", 2, Some("k1")).await);
    assert_eq!(
        log(&store, "b").await.0,
        [at.0],
        "one event, the first write's"
    );
    assert_eq!(version, Some(Version(1)));
    assert_eq!(
        state(&l, "b", None).await,
        json!([{"n": 2, "plus": 3}, 1, at.0])
    );
}

/// Row 3. A write under a key whose fold fails at its commit is answered
/// the store's sentence with its position; sent again it is answered the
/// first write's outcome and nothing is appended, and another write of the
/// record is its version 2.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_after_a_failed_fold_is_decided_over_what_the_log_holds() {
    let (store, probe, _) = lake("behind-commit").await;
    let l = Lake::of(store.as_ref());
    l.declare(doc("n + 1")).await.unwrap();
    *probe.refuse_commit.lock().unwrap() = Some(1);
    let told = unavailable(write(&l, "b", 1, Some("k1")).await);
    let (at, version) = applied(write(&l, "b", 1, Some("k1")).await);
    assert_eq!(version, Some(Version(1)), "the first write's outcome");
    assert_eq!(
        log(&store, "b").await.0,
        [at.0],
        "the retry appended nothing"
    );
    let sentence = format!(
        "the write is in the log at position {} and is not folded yet; the next write folds \
         it, and it is sent again only under its idempotency key (the commit through {} was \
         refused (planted))",
        at.0, at.0
    );
    assert_eq!(told, sentence);
    let (next, version) = applied(write(&l, "b", 2, None).await);
    assert_eq!(version, Some(Version(2)), "the record's next version");
    assert_eq!(log(&store, "b").await.0, [at.0, next.0]);
    assert_eq!(
        state(&l, "b", None).await,
        json!([{"n": 2, "plus": 3}, 2, next.0])
    );
}

/// Row 4. A type declared again whose fold fails at its commit, and the
/// writer cannot read its state back (its view is refused): the next write
/// reads it at the mark before it folds, so the declaration is compared
/// with the one in force and its step is kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_writer_that_could_not_read_its_state_back_reads_it_before_it_folds() {
    let (store, probe, _) = lake("behind-reload").await;
    let l = Lake::of(store.as_ref());
    l.declare(doc("n + 1")).await.unwrap();
    applied(write(&l, "a", 3, None).await);
    *probe.refuse_commit.lock().unwrap() = Some(1);
    let (planted, mut once) = (probe.clone(), true);
    *probe.before_commit.lock().unwrap() = Some(Box::new(move |_| {
        if std::mem::take(&mut once) {
            *planted.refuse_view.lock().unwrap() = Some(1);
        }
    }));
    let refused = l.try_declare(doc("n + 10")).await;
    assert!(
        matches!(refused, Err(Error::Unavailable { .. })),
        "{refused:?}"
    );
    let (b, _) = applied(write(&l, "b", 5, None).await);
    let again = log(&store, "b").await.1[1];
    assert_eq!(
        state(&l, "a", None).await,
        json!([{"n": 3, "plus": 13}, 1, again])
    );
    assert_eq!(
        state(&l, "b", None).await,
        json!([{"n": 5, "plus": 15}, 1, b.0])
    );
}

/// Row 5. An append decided under a writer that is behind and was not
/// caught up is refused and appends nothing; taken for a write, the writer
/// folds what was left and the append goes.
#[test]
fn an_append_decided_by_a_writer_that_is_behind_is_refused() {
    let probe = Arc::new(Probe::default());
    let seams = Seams {
        engines: vec![Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()))],
        ..Seams::default()
    };
    let location = Location::memory("behind-append");
    let lake = lakelands_store::lake::Lake::open(&location, seams, OpenMode::Create).unwrap();
    let wrote = |key: &str| EventBody::RecordUpserted {
        ty: "doc".into(),
        key: key.into(),
        version: Version(1),
        patch: patch(json!({"n": 1})),
        made: true,
    };
    let draft =
        |w: &mut lakelands_store::lake::Writer, body| vec![Draft::system(lake.clock(w), body)];
    let declared = EventBody::Declared {
        schema_hash: Some(lakelands_core::shape::schema_hash(&doc("n + 1")).unwrap()),
        declaration: Declaration::Type(doc("n + 1")),
        version: 1,
    };
    let mut w = lake.for_write().unwrap();
    let load = draft(&mut w, declared);
    lake.append(&mut w, load).expect("the declaration");
    *probe.refuse_commit.lock().unwrap() = Some(1);
    let b = draft(&mut w, wrote("b"));
    lake.append(&mut w, b).expect_err("the fold fails");
    let tip = lake.engine.tip().unwrap();
    let c = draft(&mut w, wrote("c"));
    let late = lake.append(&mut w, c);
    assert!(matches!(late, Err(Error::Unavailable { .. })), "{late:?}");
    assert_eq!(lake.engine.tip().unwrap(), tip, "nothing was appended");
    drop(w);
    let mut w = lake.for_write().expect("the writer catches up");
    let c = draft(&mut w, wrote("c"));
    lake.append(&mut w, c).expect("the append of `c`");
    let view = lake.engine.view(None).unwrap();
    for key in ["b", "c"] {
        let held = view.get(&"doc".into(), &key.into()).unwrap();
        assert!(held.is_some(), "`{key}` is folded");
    }
}
