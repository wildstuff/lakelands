//! Record writes queued as batches on the memory engine probed: a batch
//! whose fold fails, a job that panics while it is decided, a batch that
//! panics in its fold, one whose tell panics after its append, one whose
//! panic escapes the turn with an entry queued behind it, a lake let go
//! on its answer and opened again, a write computed outside the store,
//! and a write a backing computes whose fold fails with writes queued
//! behind it (lakelands#1332, lakelands#1313).

#[path = "batched/mod.rs"]
mod batched;
mod probed;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use lakelands_api::engine::OpenMode;
use lakelands_api::error::Situation;
use lakelands_api::event::{Event, EventBody};
use lakelands_api::read::{Get, ReadAt};
use lakelands_api::seam::{EmbedBackend, FunctionBacking, FunctionCall, Seams};
use lakelands_api::write::{Declaration, Outcome};
use lakelands_api::{LakeStore, Location, Position, Result, WriteContext};
use lakelands_engine_mem::MemoryOpener;
use lakelands_store::batch::{Asked, Write};
use probed::{Probe, ProbedOpener};
use serde_json::{Value, json};

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

/// A backing that counts its calls.
#[derive(Default)]
struct Counting(AtomicUsize);

#[async_trait]
impl FunctionBacking for Counting {
    fn kind(&self) -> &str {
        "counting"
    }

    async fn call(&self, _: &FunctionCall) -> lakelands_api::Result<Value> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(json!(7))
    }
}

/// Lakes on the probed memory engine, `counting` bound as their backing,
/// and what makes the next fold fail.
fn place(counting: Arc<Counting>) -> (batched::Place, impl Fn()) {
    let probe = Arc::new(Probe::default());
    let opener = Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()));
    let place: batched::Place = Arc::new(move |lake, _| {
        let seams = Seams {
            engines: vec![opener.clone()],
            backings: vec![counting.clone()],
            ..Seams::default()
        };
        (Location::memory(lake), seams)
    });
    (place, move || {
        *probe.refuse_commit.lock().unwrap() = Some(1)
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_batch_whose_fold_fails_answers_each_job_at_its_own_position() {
    let (place, refuse) = place(Arc::default());
    batched::a_batch_whose_fold_fails_answers_each_job_at_its_own_position(
        &place, "folds", &refuse,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_backed_write_whose_fold_failed_is_caught_up_alone() {
    let counting = Arc::new(Counting::default());
    let (place, refuse) = place(counting.clone());
    let handed = || counting.0.load(Ordering::SeqCst);
    batched::a_backed_write_whose_fold_failed_is_caught_up_alone(
        &place, "backed", &refuse, &handed,
    )
    .await;
}

/// A job that panics while it is decided is answered `Stopped` and leaves
/// nothing behind; the jobs decided before it are appended, and those
/// after it are decided again and written.
#[tokio::test(flavor = "multi_thread")]
async fn a_job_that_panics_while_decided_is_stopped_and_the_rest_are_written() {
    let (probe, name) = (Arc::new(Probe::default()), "decide-panics");
    let batched::Opened { store, batches, .. } =
        batched::opened(&probed(&probe), name, 0, None).await;
    let head = store.head().await.expect("a head");
    *probe.panic_get.lock().unwrap() = Some(("item".into(), "boom".into()));
    let (a, boom, c) = (
        batched::upsert(name, "a", 1),
        batched::upsert(name, "boom", 2),
        batched::upsert(name, "c", 3),
    );
    let answers = batches.write(vec![a, boom, c]).await;
    applied(&answers[0]);
    stopped(&answers[1]);
    applied(&answers[2]);
    assert_eq!(items(store.as_ref(), head).await, ["a", "c"]);
    *probe.panic_get.lock().unwrap() = None;
    let again = batches.write(vec![batched::upsert(name, "boom", 2)]);
    applied(&again.await[0]);
    assert_eq!(items(store.as_ref(), head).await, ["a", "c", "boom"]);
    store.close().await.expect("the lake closes");
}

/// A batch whose fold panics, before the projection takes its commit or
/// after, answers every job `Stopped`, a refused one too; what landed is
/// folded by the next write and reads.
#[tokio::test(flavor = "multi_thread")]
async fn a_batch_whose_fold_panics_answers_every_job_and_what_landed_is_folded() {
    for taken in [false, true] {
        let probe = Arc::new(Probe::default());
        let name = ["fold-panics", "fold-panics-taken"][taken as usize];
        let batched::Opened { store, batches, .. } =
            batched::opened(&probed(&probe), name, 0, None).await;
        let head = store.head().await.expect("a head");
        *probe.panic_commit.lock().unwrap() = Some((1, taken));
        let (a, b) = (batched::upsert(name, "a", 1), batched::upsert(name, "b", 2));
        let answers = batches.write(vec![a, batched::refused(name, "r"), b]).await;
        for answer in &answers {
            stopped(answer);
        }
        let after = batches.write(vec![batched::upsert(name, "c", 3)]);
        applied(&after.await[0]);
        assert_eq!(items(store.as_ref(), head).await, ["a", "b", "c"]);
        for key in ["a", "b", "c"] {
            assert!(reads(store.as_ref(), name, key).await, "{key} reads");
        }
        let more = batches.write(vec![batched::upsert(name, "d", 4)]);
        applied(&more.await[0]);
        store.close().await.expect("the lake closes");
    }
}

/// A panic that escapes the turn, a fold's and then the view's the
/// writer reloads from, answers its batch `Stopped`; the entry queued
/// behind it runs, and the lake writes on. A job that runs alone stands
/// between them, as record writes queued right behind are taken into the
/// batch and answered with it.
#[tokio::test(flavor = "multi_thread")]
async fn a_panic_out_of_the_turn_hands_it_to_the_entry_behind() {
    let (probe, name) = (Arc::new(Probe::default()), "escapes");
    let batched::Opened { store, batches, .. } =
        batched::opened(&probed(&probe), name, 0, None).await;
    let head = store.head().await.expect("a head");
    let (entered, inside) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let mut once = Some((entered, wait));
    *probe.before_commit.lock().unwrap() = Some(Box::new(move |_| {
        if let Some((entered, wait)) = once.take() {
            entered.send(()).expect("the test waits");
            wait.recv().expect("the test releases");
        }
    }));
    let queue = |writes: Vec<Asked>| {
        let batches = batches.clone();
        tokio::spawn(async move { batches.write(writes).await })
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
    drop(batches.hold());
    queued(2).await;
    let behind = queue(vec![batched::upsert(name, "c", 3)]);
    queued(3).await;
    release.send(()).expect("the first write waits");
    applied(&first.await.expect("the first write ends")[0]);
    let escaped = escapes.await.expect("the batch ends");
    stopped(&escaped[0]);
    stopped(&escaped[1]);
    let behind = tokio::time::timeout(Duration::from_secs(60), behind).await;
    applied(&behind.expect("the entry behind runs").expect("it ends")[0]);
    assert_eq!(items(store.as_ref(), head).await, ["w", "a", "b", "c"]);
    let more = batches.write(vec![batched::upsert(name, "d", 4)]);
    applied(&more.await[0]);
    store.close().await.expect("the lake closes");
}

/// Record writes of two callers taken together, more events than a batch
/// holds, are written in two batches, and the first is answered before
/// the second is folded.
#[tokio::test(flavor = "multi_thread")]
async fn a_batch_is_answered_before_the_next_is_folded() {
    let (probe, name) = (Arc::new(Probe::default()), "answered-first");
    let batched::Opened { store, batches, .. } =
        batched::opened(&probed(&probe), name, 0, Some(1)).await;
    let (entered, inside) = std::sync::mpsc::channel();
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let (mut commits, mut once) = (0, Some((entered, wait)));
    *probe.before_commit.lock().unwrap() = Some(Box::new(move |_| {
        commits += 1;
        if commits == 2
            && let Some((entered, wait)) = once.take()
        {
            entered.send(()).expect("the test waits");
            wait.recv().expect("the test releases");
        }
    }));
    let held = batches.hold();
    batched::until(|| batches.queued() == 0).await;
    let mut callers = Vec::new();
    for (n, key) in [(1, "a"), (2, "b")] {
        let (b, asked) = (batches.clone(), batched::upsert(name, key, n));
        callers.push(tokio::spawn(async move { b.write(vec![asked]).await }));
        batched::until(|| batches.queued() == n as usize).await;
    }
    let (second, first) = (callers.pop().expect("two"), callers.pop().expect("two"));
    drop(held);
    tokio::task::spawn_blocking(move || inside.recv())
        .await
        .expect("the wait ends")
        .expect("the second batch is folded");
    let first = tokio::time::timeout(Duration::from_secs(10), first).await;
    applied(
        &first
            .expect("the first is answered meanwhile")
            .expect("it ends")[0],
    );
    release.send(()).expect("the second batch waits");
    applied(&second.await.expect("the second ends")[0]);
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
        let (_, who, write, _) = batched::upsert(name, "a", n);
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
        .write(vec![(
            WriteContext::default(),
            who,
            write,
            Default::default(),
        )])
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

/// The `rome` set's moves in `store`'s log after `from`: (key, joined).
async fn moves(store: &dyn LakeStore, from: Position) -> Vec<(String, bool)> {
    let moved = |e: &Event| match &e.body {
        EventBody::ObjectSetChanged {
            set, key, joined, ..
        } if set == "rome" && e.position > from => Some((key.to_string(), *joined)),
        _ => None,
    };
    batched::log(store).await.iter().filter_map(moved).collect()
}

/// A batch whose tell panics, its events appended and folded, answers
/// every job `Stopped`; what landed stays, once, and the lake folds and
/// writes on from it: a far write that set off a move is told as one at
/// the batch's tip, read as of a position, which panics here.
#[tokio::test(flavor = "multi_thread")]
async fn a_batch_whose_tell_panics_after_its_append_answers_every_job_and_keeps_what_landed() {
    let (probe, name) = (Arc::new(Probe::default()), "tell-panics");
    let batched::Opened { store, batches, .. } =
        batched::opened(&probed(&probe), name, 0, None).await;
    let who = batched::owner(name, "owner");
    let cat = store.catalog(WriteContext::default());
    let declarations = [
        json!({"declare": "type", "name": "cust",
            "fields": [{"name": "city", "type": {"kind": "text"}}]}),
        json!({"declare": "type", "name": "order",
            "fields": [{"name": "customer", "type": {"kind": "reference", "target": "cust"}}]}),
        json!({"declare": "object_set", "name": "rome", "of": "order",
            "where": "customer.city = 'Rome'"}),
    ];
    for d in declarations {
        let d: Declaration = serde_json::from_value(d).expect("a declaration");
        cat.declare(&who, d).await.expect("it is declared");
    }
    let write = |ty: &str, key: &str, patch: serde_json::Value| {
        let patch = serde_json::from_value(patch).expect("a patch");
        let write = Write::Upsert(ty.into(), key.into(), patch);
        (
            WriteContext::default(),
            who.clone(),
            write,
            Default::default(),
        )
    };
    let setup = vec![
        write("cust", "c1", json!({"city": "Paris"})),
        write("order", "o1", json!({"customer": "c1"})),
    ];
    batches.write(setup).await.iter().for_each(applied);
    let head = store.head().await.expect("a head");
    probe.panic_as_of.store(true, Ordering::SeqCst);
    let rome = write("cust", "c1", json!({"city": "Rome"}));
    let answers = batches
        .write(vec![rome, batched::upsert(name, "a", 1)])
        .await;
    assert!(
        !probe.panic_as_of.load(Ordering::SeqCst),
        "the tell read as of a position"
    );
    answers.iter().for_each(stopped);
    assert_eq!(moves(store.as_ref(), head).await, [("o1".into(), true)]);
    assert_eq!(items(store.as_ref(), head).await, ["a"]);
    let after = batches.write(vec![batched::upsert(name, "b", 2)]);
    applied(&after.await[0]);
    let paris = write("cust", "c1", json!({"city": "Paris"}));
    let left = batches.write(vec![paris]).await;
    let Ok(Outcome::Applied { set_off, .. }) = &left[0] else {
        panic!("the far write applies: {left:?}")
    };
    assert_eq!(set_off.left, ["rome"], "the far write is told the move");
    let moved = [("o1".into(), true), ("o1".into(), false)];
    assert_eq!(moves(store.as_ref(), head).await, moved);
    assert_eq!(items(store.as_ref(), head).await, ["a", "b"]);
    assert!(reads(store.as_ref(), name, "a").await, "a reads");
    store.close().await.expect("the lake closes");
}
