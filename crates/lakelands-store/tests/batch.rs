//! Record writes queued as batches on the memory engine probed: a batch
//! whose fold fails, a job that panics while it is decided, a batch that
//! panics in its fold, one whose panic escapes the turn with an entry
//! queued behind it, a lake let go on its answer and opened again, and a
//! write computed outside the store (lakelands#1332).

#[path = "batched/mod.rs"]
mod batched;
mod probed;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use async_trait::async_trait;
use lakelands_api::engine::OpenMode;
use lakelands_api::error::Situation;
use lakelands_api::event::{Event, EventBody};
use lakelands_api::read::{Get, ReadAt};
use lakelands_api::seam::{EmbedBackend, Seams};
use lakelands_api::write::{Declaration, Outcome};
use lakelands_api::{LakeStore, Location, Position, Result, WriteContext};
use lakelands_engine_mem::MemoryOpener;
use lakelands_store::batch::Write;
use probed::{Probe, ProbedOpener};
use serde_json::json;

/// The lakes of the memory engine, probed by `probe`.
fn probed(probe: &Arc<Probe>) -> batched::Place {
    let opener = Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()));
    Arc::new(move |lake, _| {
        let seams = Seams {
            engines: vec![opener.clone()],
            ..Seams::default()
        };
        (Location::memory(lake), seams)
    })
}

/// The keys of the items upserted in `store`'s log after `from`.
async fn items(store: &dyn LakeStore, from: Position) -> Vec<String> {
    let key = |e: &Event| match &e.body {
        EventBody::RecordUpserted { ty, key, .. } if ty.as_str() == "item" && e.position > from => {
            Some(key.to_string())
        }
        _ => None,
    };
    batched::log(store).await.iter().filter_map(key).collect()
}

fn applied(answer: &Result<Outcome>) {
    assert!(matches!(answer, Ok(Outcome::Applied { .. })), "{answer:?}");
}

fn stopped(answer: &Result<Outcome>) {
    let s = batched::situation(answer);
    assert!(matches!(s, Some(Situation::Stopped { .. })), "{answer:?}");
}

/// Whether `key` reads as an item of the lake.
async fn reads(store: &dyn LakeStore, lake: &str, key: &str) -> bool {
    let get = Get {
        key: key.into(),
        with_meta: false,
        at: ReadAt::default(),
        params: Default::default(),
    };
    let who = batched::owner(lake, "owner");
    let cat = store.catalog(WriteContext::default());
    cat.get(&who, &"item".into(), get).await.is_ok()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_batch_whose_fold_fails_answers_each_job_at_its_own_position() {
    let probe = Arc::new(Probe::default());
    let refuse = || *probe.refuse_commit.lock().unwrap() = Some(1);
    batched::a_batch_whose_fold_fails_answers_each_job_at_its_own_position(
        &probed(&probe),
        "folds",
        &refuse,
    )
    .await;
}

/// A job that panics while it is decided is answered `Stopped` and leaves
/// nothing behind; the jobs decided before it are appended, and those
/// after it are decided again and written.
#[tokio::test(flavor = "multi_thread")]
async fn a_job_that_panics_while_decided_is_stopped_and_the_rest_are_written() {
    let (probe, name) = (Arc::new(Probe::default()), "decide-panics");
    let (store, batches) = batched::opened(&probed(&probe), name, 0).await;
    let head = store.head().await.expect("a head");
    *probe.panic_get.lock().unwrap() = Some(("item".into(), "boom".into()));
    let (a, boom, c) = (
        batched::upsert(name, "a", 1),
        batched::upsert(name, "boom", 2),
        batched::upsert(name, "c", 3),
    );
    let ctx = WriteContext::default();
    let answers = batches.write(ctx, vec![a, boom, c]).await;
    applied(&answers[0]);
    stopped(&answers[1]);
    applied(&answers[2]);
    assert_eq!(items(store.as_ref(), head).await, ["a", "c"]);
    *probe.panic_get.lock().unwrap() = None;
    let again = batches.write(ctx, vec![batched::upsert(name, "boom", 2)]);
    applied(&again.await[0]);
    assert_eq!(items(store.as_ref(), head).await, ["a", "c", "boom"]);
    store.close().await.expect("the lake closes");
}

/// A batch whose fold panics, before the projection takes its commit or
/// after, answers every job that appended `Stopped` and a refused job its
/// refusal; what landed is folded by the next write and reads.
#[tokio::test(flavor = "multi_thread")]
async fn a_batch_whose_fold_panics_answers_every_job_and_what_landed_is_folded() {
    for taken in [false, true] {
        let probe = Arc::new(Probe::default());
        let name = ["fold-panics", "fold-panics-taken"][taken as usize];
        let (store, batches) = batched::opened(&probed(&probe), name, 0).await;
        let head = store.head().await.expect("a head");
        *probe.panic_commit.lock().unwrap() = Some((1, taken));
        let (a, b) = (batched::upsert(name, "a", 1), batched::upsert(name, "b", 2));
        let ctx = WriteContext::default();
        let answers = batches
            .write(ctx, vec![a, batched::refused(name, "r"), b])
            .await;
        stopped(&answers[0]);
        assert!(
            answers[1].is_err() && batched::situation(&answers[1]).is_none(),
            "the refusal stays: {:?}",
            answers[1]
        );
        stopped(&answers[2]);
        let after = batches.write(ctx, vec![batched::upsert(name, "c", 3)]);
        applied(&after.await[0]);
        assert_eq!(items(store.as_ref(), head).await, ["a", "b", "c"]);
        for key in ["a", "b", "c"] {
            assert!(reads(store.as_ref(), name, key).await, "{key} reads");
        }
        let more = batches.write(ctx, vec![batched::upsert(name, "d", 4)]);
        applied(&more.await[0]);
        store.close().await.expect("the lake closes");
    }
}

/// A panic that escapes the turn, a fold's and then the view's the
/// writer reloads from, answers its batch `Stopped`; the entry queued
/// behind it runs, and the lake writes on.
#[tokio::test(flavor = "multi_thread")]
async fn a_panic_out_of_the_turn_hands_it_to_the_entry_behind() {
    let (probe, name) = (Arc::new(Probe::default()), "escapes");
    let (store, batches) = batched::opened(&probed(&probe), name, 0).await;
    let (batches, head) = (Arc::new(batches), store.head().await.expect("a head"));
    let (entered, inside) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let mut once = Some((entered, wait));
    *probe.before_commit.lock().unwrap() = Some(Box::new(move |_| {
        if let Some((entered, wait)) = once.take() {
            entered.send(()).expect("the test waits");
            wait.recv().expect("the test releases");
        }
    }));
    let ctx = WriteContext::default();
    let queue = |writes: Vec<(_, Write, _)>| {
        let batches = batches.clone();
        tokio::spawn(async move { batches.write(ctx, writes).await })
    };
    let first = queue(vec![batched::upsert(name, "w", 0)]);
    tokio::task::spawn_blocking(move || inside.recv())
        .await
        .expect("the wait ends")
        .expect("the first write holds the turn");
    *probe.panic_commit.lock().unwrap() = Some((2, false));
    probe.then_view.store(true, Ordering::SeqCst);
    let queued = |n: usize| {
        let batches = batches.clone();
        async move {
            while batches.queued() < n {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }
    };
    let escapes = queue(vec![
        batched::upsert(name, "a", 1),
        batched::upsert(name, "b", 2),
    ]);
    queued(1).await;
    let behind = queue(vec![batched::upsert(name, "c", 3)]);
    queued(2).await;
    release.send(()).expect("the first write waits");
    applied(&first.await.expect("the first write ends")[0]);
    let escaped = escapes.await.expect("the batch ends");
    stopped(&escaped[0]);
    stopped(&escaped[1]);
    let behind = tokio::time::timeout(Duration::from_secs(60), behind).await;
    applied(&behind.expect("the entry behind runs").expect("it ends")[0]);
    assert_eq!(items(store.as_ref(), head).await, ["w", "a", "b", "c"]);
    let more = batches.write(ctx, vec![batched::upsert(name, "d", 4)]);
    applied(&more.await[0]);
    store.close().await.expect("the lake closes");
}

/// A lake its caller lets go as soon as a write is answered has let its
/// lease go: it opens again at once.
#[tokio::test(flavor = "multi_thread")]
async fn a_lake_let_go_on_its_answer_opens_again() {
    let (opener, name) = (Arc::new(MemoryOpener::new()), "let-go");
    let seams = || Seams {
        engines: vec![opener.clone()],
        ..Seams::default()
    };
    let store = lakelands_store::open(Location::memory(name), seams(), OpenMode::Create);
    let store = store.await.expect("the lake opens");
    batched::declared(store.as_ref(), name).await;
    drop(store);
    for n in 0..50 {
        let store = lakelands_store::open(Location::memory(name), seams(), OpenMode::Write);
        let store = store.await.unwrap_or_else(|e| panic!("open {n}: {e:?}"));
        let (who, write, _) = batched::upsert(name, "a", n);
        let Write::Upsert(ty, key, patch) = write else {
            unreachable!("an upsert")
        };
        let cat = store.catalog(WriteContext::default());
        cat.upsert(&who, &ty, &key, patch, Default::default())
            .await
            .expect("the write applies");
        drop((cat, store));
    }
}

/// An embedder whose vector is the text's length.
struct Lengths;

#[async_trait]
impl EmbedBackend for Lengths {
    fn model(&self) -> &str {
        "lengths"
    }
    fn dimensions(&self) -> u32 {
        2
    }
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| vec![t.len() as f32, 1.0]).collect())
    }
}

/// A write to a type computed outside the store is decided alone, as
/// before batches: it is answered, and the call it makes due is made.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_computed_outside_is_decided_alone() {
    let (opener, name) = (Arc::new(MemoryOpener::new()), "outside");
    let seams = Seams {
        engines: vec![opener],
        embed: Some(Arc::new(Lengths)),
        ..Seams::default()
    };
    let opened = lakelands_store::batch::open(Location::memory(name), seams, OpenMode::Create);
    let (store, batches) = opened.await.expect("the lake opens");
    let doc = json!({"declare": "type", "name": "doc",
        "fields": [{"name": "body", "type": {"kind": "text"}},
                   {"name": "near", "type": {"kind": "vector", "dimensions": 2},
                    "authority": "derived", "embed": {"from": ["body"], "model": "lengths"}}]});
    let doc: Declaration = serde_json::from_value(doc).expect("a declaration");
    let who = batched::owner(name, "owner");
    let cat = store.catalog(WriteContext::default());
    cat.declare(&who, doc).await.expect("`doc` is declared");
    let patch = serde_json::from_value(json!({"body": "some text"})).expect("a patch");
    let write = Write::Upsert("doc".into(), "d".into(), patch);
    let answers = batches
        .write(
            WriteContext::default(),
            vec![(who, write, Default::default())],
        )
        .await;
    applied(&answers[0]);
    let output = |e: &Event| matches!(e.body, EventBody::OutputRecorded { .. });
    for _ in 0..600 {
        if batched::log(store.as_ref()).await.iter().any(output) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the call the write made due was never made");
}
