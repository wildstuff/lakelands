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

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use lakelands_api::engine::OpenMode;
use lakelands_api::event::EventBody;
use lakelands_api::ontology::StorageKind::{Number, Text};
use lakelands_api::ontology::{Authority, Risk, TypeDecl, VerbDecl, VerbEffect};
use lakelands_api::seam::{
    EmbedBackend, FeedFilter, FunctionBacking, FunctionCall, Seams, VerbExecutor, VerbInvocation,
};
use lakelands_api::write::{Declaration, Outcome, WriteOpts};
use lakelands_api::{Error, LakeStore, Location, Position, Refusal, Timestamp, Version};
use lakelands_core::conformance::fixture::{
    Lake, computed_by, derived, embedding, field, get_at, patch, source_record, type_decl,
};
use lakelands_engine_mem::MemoryOpener;
use lakelands_store::log::Draft;
use probed::{Probe, ProbedOpener};
use serde_json::{Value, json};

/// A backing, an embedding backend and an executor that count what they
/// are handed; when it is gated, the first is held until the gate is
/// opened.
#[derive(Default)]
struct Counting {
    handed: AtomicUsize,
    /// The texts of each embedding request.
    texts: Mutex<Vec<Vec<String>>>,
    /// What the next call does to the engine when it is handed, once.
    arm: Mutex<Option<Box<dyn Fn() + Send>>>,
    gate: Option<tokio::sync::Notify>,
}

impl Counting {
    async fn hand(&self) -> usize {
        let n = self.handed.fetch_add(1, Ordering::SeqCst);
        if let Some(plant) = self.arm.lock().unwrap().take() {
            plant();
        }
        if let (0, Some(gate)) = (n, &self.gate) {
            gate.notified().await;
        }
        n + 1
    }

    fn gated() -> Arc<Counting> {
        Arc::new(Counting {
            gate: Some(tokio::sync::Notify::new()),
            ..Counting::default()
        })
    }

    /// Wait until the first is handed and held at the gate.
    async fn held(&self) {
        until(|| self.handed.load(Ordering::SeqCst) == 1).await;
    }

    /// The next call refuses the engine's next commit: its recording's.
    fn refuses_its_recording(&self, probe: &Arc<Probe>) {
        let probe = probe.clone();
        let plant = move || *probe.refuse_commit.lock().unwrap() = Some(1);
        *self.arm.lock().unwrap() = Some(Box::new(plant));
    }
}

#[async_trait]
impl EmbedBackend for Counting {
    fn model(&self) -> &str {
        "m"
    }

    fn dimensions(&self) -> u32 {
        2
    }

    async fn embed(&self, texts: &[String]) -> lakelands_api::Result<Vec<Vec<f32>>> {
        self.texts.lock().unwrap().push(texts.to_vec());
        self.hand().await;
        Ok(vec![vec![1.0, 0.0]; texts.len()])
    }
}

#[async_trait]
impl FunctionBacking for Counting {
    fn kind(&self) -> &str {
        "counting"
    }

    async fn call(&self, _: &FunctionCall) -> lakelands_api::Result<Value> {
        self.hand().await;
        Ok(json!(7))
    }
}

#[async_trait]
impl VerbExecutor for Counting {
    fn name(&self) -> &str {
        "bank"
    }

    async fn execute(&self, _: &VerbInvocation) -> lakelands_api::Result<Value> {
        Ok(json!({"ran": self.hand().await}))
    }
}

/// Wait until `done`, for a row that needs another task to have got
/// somewhere.
async fn until(done: impl Fn() -> bool) {
    for _ in 0..5_000 {
        if done() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
        tokio::task::yield_now().await;
    }
    panic!("waited five seconds");
}

/// A new lake on the probed memory engine, with the engine's own opener
/// for a second handle on it.
async fn lake(name: &str) -> (Arc<dyn LakeStore>, Arc<Probe>, MemoryOpener) {
    bound(name, Arc::default()).await
}

/// [`lake`], with `counting` bound as its backing and its executor.
async fn bound(
    name: &str,
    counting: Arc<Counting>,
) -> (Arc<dyn LakeStore>, Arc<Probe>, MemoryOpener) {
    let (probe, opener) = (Arc::new(Probe::default()), MemoryOpener::new());
    let seams = Seams {
        engines: vec![Arc::new(ProbedOpener(opener.clone(), probe.clone()))],
        backings: vec![counting.clone()],
        embed: Some(counting.clone()),
        executors: vec![counting],
        ..Seams::default()
    };
    let store = lakelands_store::open(Location::memory(name), seams, OpenMode::Create)
        .await
        .expect("a new lake");
    (store, probe, opener)
}

/// `doc` with `n`, which either door writes, and `plus`, derived from it
/// by `plus`.
fn doc(plus: &str) -> TypeDecl {
    let plus = derived("plus", Number, plus, &["n"]);
    let mut n = field("n", Number);
    n.authority = Authority::Shared;
    type_decl("doc", vec![n, plus])
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
fn unavailable<T: std::fmt::Debug>(outcome: lakelands_api::Result<T>) -> String {
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
        "what was appended is in the log from position {} and is not folded yet; the next \
         write folds it, and a write is sent again only under its idempotency key (the commit \
         through {} was refused (planted))",
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

/// Row 6. A handle that gave its lease up while it was behind folds
/// nothing: every door on it, `checkpoint` and `rebuild` are refused as on
/// any handle without the lease, and the engine is asked for no
/// transaction. The
/// handle that holds the lease folds what was left when it opens.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_handle_that_closed_behind_folds_nothing() {
    let (store, probe, opener) = lake("behind-closed").await;
    let l = Lake::of(store.as_ref());
    l.declare(doc("n + 1")).await.unwrap();
    *probe.refuse_commit.lock().unwrap() = Some(1);
    unavailable(write(&l, "b", 2, None).await);
    store.close().await.expect("the handle closes");
    let seams = Seams {
        engines: vec![Arc::new(opener)],
        ..Seams::default()
    };
    let location = Location::memory("behind-closed");
    let other = lakelands_store::open(location, seams, OpenMode::Write).await;
    let other = other.expect("a second handle, for writing");
    let before = probe.begins.load(Ordering::SeqCst);
    let no_lease = "this lake is open read-only here; another process holds its writer lease";
    assert_eq!(unavailable(write(&l, "c", 3, None).await), no_lease);
    let declared = l.try_declare(doc("n + 10")).await;
    assert_eq!(unavailable(declared), no_lease, "a declaration");
    let fed = l.ingest("feed", vec![source_record("doc", "d", json!({"n": 4}))]);
    assert_eq!(unavailable(fed.await), no_lease, "a feed");
    assert_eq!(
        unavailable(store.checkpoint().await),
        no_lease,
        "a checkpoint"
    );
    assert_eq!(unavailable(store.rebuild().await), no_lease, "a rebuild");
    assert_eq!(
        probe.begins.load(Ordering::SeqCst),
        before,
        "no `begin` from the closed handle"
    );
    let b = state(&Lake::of(other.as_ref()), "b", None).await;
    assert_eq!(
        b[0],
        json!({"n": 2, "plus": 3}),
        "the lease's holder folded it"
    );
}

/// Row 7. A write on a writer that cannot catch up is answered
/// `unavailable` in the store's sentence, whatever kind the fold answered,
/// and appends nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_that_cannot_catch_up_is_unavailable_whatever_the_fold_answered() {
    let (store, probe, _) = lake("behind-kind").await;
    let l = Lake::of(store.as_ref());
    l.declare(doc("n + 1")).await.unwrap();
    *probe.refuse_commit.lock().unwrap() = Some(1);
    unavailable(write(&l, "b", 2, None).await);
    probe.refuse_as_invalid.store(true, Ordering::SeqCst);
    *probe.refuse_begin.lock().unwrap() = Some(1);
    let told = unavailable(write(&l, "c", 3, None).await);
    let sentence = "the lake's projection is behind its log and could not catch up; nothing was \
                    appended (the `begin` was refused (planted))";
    assert_eq!(told, sentence);
    let (c, _) = applied(write(&l, "c", 3, None).await);
    assert_eq!(log(&store, "c").await.0, [c.0], "`c` is in the log once");
}

/// Row 8. `add_field` reads the type behind the catch-up: after a type
/// declared again whose fold failed, a field is added to that type, not
/// to the one before it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_field_is_added_to_the_type_the_log_holds() {
    let (store, probe, _) = lake("behind-add-field").await;
    let l = Lake::of(store.as_ref());
    l.declare(doc("n + 1")).await.unwrap();
    applied(write(&l, "a", 3, None).await);
    *probe.refuse_commit.lock().unwrap() = Some(1);
    let refused = l.try_declare(doc("n + 10")).await;
    assert!(
        matches!(refused, Err(Error::Unavailable { .. })),
        "{refused:?}"
    );
    let added = l
        .cat
        .add_field(&l.own, &"doc".into(), field("m", Number))
        .await;
    applied(added);
    let described = l.describe_type("doc").await.expect("the type");
    let plus = described.fields.iter().find(|f| f.name.as_str() == "plus");
    let expr = json!(plus.expect("`plus`").derived);
    assert!(
        expr.to_string().contains("10"),
        "the type declared again stands: {expr}"
    );
    assert!(
        described.fields.iter().any(|f| f.name.as_str() == "m"),
        "with the field"
    );
    assert_eq!(state(&l, "a", None).await[0], json!({"n": 3, "plus": 13}));
}

/// Row 9. A drain reads behind the catch-up, and its failure is not the
/// write's. The recording of `a`'s output is appended and its fold fails
/// while a second write waits for the drain: the first is answered its own
/// outcome, and the second's drain finds the output recorded and does not
/// call for it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_drain_reads_behind_the_catch_up_and_its_failure_is_not_the_writes() {
    let counting = Counting::gated();
    let (store, probe, _) = bound("behind-drain", counting.clone()).await;
    let l = Lake::of(store.as_ref());
    let f = json!({"declare": "function", "name": "f", "backing": "counting",
                   "inputs": [{"name": "n", "type": {"kind": "number"}}]});
    l.declare_json(f).await.unwrap();
    let sum = computed_by("sum", Number, "f", &["n"]);
    let summed = type_decl("doc", vec![field("n", Number), sum]);
    l.declare(summed).await.unwrap();
    let writes = |key: &'static str, n| {
        let l = Lake::of(store.as_ref());
        tokio::spawn(async move { write(&l, key, n, None).await })
    };
    let first = writes("a", 1);
    counting.held().await;
    let second = writes("b", 2);
    while log(&store, "b").await.0.is_empty() {
        tokio::task::yield_now().await;
    }
    *probe.refuse_commit.lock().unwrap() = Some(1);
    counting.gate.as_ref().unwrap().notify_one();
    let (a, _) = applied(first.await.unwrap());
    applied(second.await.unwrap());
    assert_eq!(
        [a.0],
        log(&store, "a").await.0[..],
        "the write's answer is the write's"
    );
    assert_eq!(
        counting.handed.load(Ordering::SeqCst),
        2,
        "one call for each record"
    );
    for (key, n) in [("a", 1), ("b", 2)] {
        assert_eq!(
            state(&l, key, None).await[0],
            json!({"n": n, "sum": 7}),
            "`{key}`"
        );
    }
    let all = FeedFilter::default();
    let polled = store.feed().poll(Position(0), &all, 100).await.unwrap();
    let recorded = |e: &&lakelands_api::event::Event| matches!(&e.body, EventBody::OutputRecorded { field: Some((_, key, _)), .. } if key.as_str() == "a");
    assert_eq!(
        polled.events.iter().filter(recorded).count(),
        1,
        "one output for `a`"
    );
}

/// `doc` with the verb `pay`, which the executor `bank` carries out, at
/// `risk`.
async fn payable(l: &Lake, risk: Risk) {
    l.declare(doc("n + 1")).await.unwrap();
    applied(write(l, "a", 1, None).await);
    let pay = Declaration::Verb(VerbDecl {
        name: "pay".into(),
        on: "doc".into(),
        scope: Default::default(),
        payload: vec![],
        risk,
        requires: None,
        effect: VerbEffect::External {
            executor: "bank".into(),
        },
        description: None,
    });
    l.declare_as(pay, "the verb").await.unwrap();
}

/// A call of `pay` on `a` under the key `k1`, its answer as it came.
async fn pay(l: &Lake) -> lakelands_api::Result<Outcome> {
    let opts = WriteOpts {
        idempotency_key: Some("k1".into()),
        ..WriteOpts::default()
    };
    let (pay, a) = ("pay".into(), "a".into());
    l.cat
        .call(&l.own, &pay, Some(&a), patch(json!({})), opts)
        .await
}

/// The position and the executor's output a call that took effect answers.
fn ran(outcome: lakelands_api::Result<Outcome>) -> (u64, Option<Value>) {
    match outcome {
        Ok(Outcome::Applied {
            position, output, ..
        }) => (position.0, output),
        other => panic!("the call must take effect: {other:?}"),
    }
}

/// Row 10. The call of an external verb whose fold failed is in the log
/// and its executor was not handed it: the store's sentence says so, and
/// that the call sent again under its key will not run it. Sent again it
/// is answered its position with no output, and the executor is handed
/// nothing. The same sentence for an acceptance, which sent again is
/// `decided`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_of_an_external_verb_whose_fold_failed_says_its_executor_was_not_handed_it() {
    let not_handed = "the verb's executor was not handed this call, and sending it again under \
                      its key will not run it: call the verb again";
    let counting = Arc::new(Counting::default());
    let (store, probe, _) = bound("behind-verb", counting.clone()).await;
    let l = Lake::of(store.as_ref());
    payable(&l, Risk::Low).await;
    *probe.refuse_commit.lock().unwrap() = Some(1);
    let said = unavailable(pay(&l).await);
    assert!(said.contains("is not folded yet"), "{said}");
    assert!(said.ends_with(not_handed), "a call: {said}");
    let (at, output) = ran(pay(&l).await);
    assert_eq!(output, None, "the call sent again has no output");
    assert!(said.contains(&format!("from position {at} ")), "{said}");

    let (store, probe, _) = bound("behind-accept", counting.clone()).await;
    let l = Lake::of(store.as_ref());
    payable(&l, Risk::High).await;
    let Ok(Outcome::Proposed { proposal, .. }) = pay(&l).await else {
        panic!("a high verb is proposed");
    };
    *probe.refuse_commit.lock().unwrap() = Some(1);
    let said = unavailable(l.cat.accept(&l.own, &proposal).await);
    assert!(said.ends_with(not_handed), "an acceptance: {said}");
    let decided = l.cat.accept(&l.own, &proposal).await;
    let refused = matches!(
        &decided,
        Err(Error::Refused {
            reason: Refusal::Decided,
            ..
        })
    );
    assert!(refused, "{decided:?}");
    assert_eq!(
        counting.handed.load(Ordering::SeqCst),
        0,
        "the executor was handed nothing"
    );
}

/// `f`, which the backing `counting` computes, and `doc` with `n`, which
/// either door writes, and `sum`, computed by `f` from it.
async fn summed(l: &Lake) {
    let f = json!({"declare": "function", "name": "f", "backing": "counting",
                   "inputs": [{"name": "n", "type": {"kind": "number"}}]});
    l.declare_json(f).await.unwrap();
    let mut n = field("n", Number);
    n.authority = Authority::Shared;
    let sum = computed_by("sum", Number, "f", &["n"]);
    l.declare(type_decl("doc", vec![n, sum])).await.unwrap();
}

/// Row 11. A feed, an aging run and a purge are answered their own
/// outcome when the drain behind them fails, as an upsert is (row 9). The
/// feed's own record makes the call due; the aging run and the purge make
/// it due by catching up over a write whose fold failed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_feed_an_aging_run_and_a_purge_are_answered_their_own_when_the_drain_fails() {
    let counting = Arc::new(Counting::default());
    let (store, probe, _) = bound("behind-feed", counting.clone()).await;
    let l = Lake::of(store.as_ref());
    summed(&l).await;
    counting.refuses_its_recording(&probe);
    let fed = l.ingest("feed", vec![source_record("doc", "d", json!({"n": 4}))]);
    let fed = fed.await.expect("the window is answered its own outcome");
    assert_eq!(fed.len(), 1, "one window");
    assert_eq!(counting.handed.load(Ordering::SeqCst), 1, "the feed's call");

    for door in ["age", "purge"] {
        let counting = Arc::new(Counting::default());
        let (store, probe, _) = bound(&format!("behind-{door}"), counting.clone()).await;
        let l = Lake::of(store.as_ref());
        summed(&l).await;
        *probe.refuse_commit.lock().unwrap() = Some(1);
        unavailable(write(&l, "a", 1, None).await);
        counting.refuses_its_recording(&probe);
        match door {
            "age" => drop(store.age(Timestamp(0)).await.expect("the aging run's own")),
            _ => drop(
                store
                    .purge_source(&"feed".into(), None)
                    .await
                    .expect("the purge's own"),
            ),
        }
        assert_eq!(counting.handed.load(Ordering::SeqCst), 1, "`{door}`'s call");
    }
}

/// Row 12. An answer is held with its due until it is recorded: where the
/// recording fails, the next drain records the answer and does not call
/// again. For a function, the recording fails before anything is
/// appended; for a batch of embeddings, at the first of two.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_answer_whose_recording_failed_is_recorded_without_a_second_call() {
    let counting = Arc::new(Counting::default());
    let (store, probe, _) = bound("behind-held", counting.clone()).await;
    let l = Lake::of(store.as_ref());
    summed(&l).await;
    let view = probe.clone();
    let plant = move || *view.refuse_view.lock().unwrap() = Some(1);
    *counting.arm.lock().unwrap() = Some(Box::new(plant));
    applied(write(&l, "a", 1, None).await);
    assert_eq!(
        state(&l, "a", None).await[0],
        json!({"n": 1}),
        "not recorded"
    );
    applied(write(&l, "b", 2, None).await);
    assert_eq!(
        counting.handed.load(Ordering::SeqCst),
        2,
        "one call for each record"
    );
    assert_eq!(state(&l, "a", None).await[0], json!({"n": 1, "sum": 7}));

    let counting = Arc::new(Counting::default());
    let (store, probe, _) = bound("behind-batch", counting.clone()).await;
    let l = Lake::of(store.as_ref());
    let mut text = field("text", Text);
    text.authority = Authority::Shared;
    let embedded = embedding("vec", 2, &["text"], "m");
    l.declare(type_decl("note", vec![text, embedded]))
        .await
        .unwrap();
    let note = |key: &str, text: &str| source_record("note", key, json!({"text": text}));
    counting.refuses_its_recording(&probe);
    let two = l.ingest("feed", vec![note("x", "one"), note("y", "two")]);
    two.await.expect("the window's own outcome");
    l.ingest("feed", vec![note("z", "three")]).await.unwrap();
    assert_eq!(
        *counting.texts.lock().unwrap(),
        [
            vec!["one".to_owned(), "two".to_owned()],
            vec!["three".to_owned()]
        ],
        "no text is sent twice"
    );
    for key in ["x", "y", "z"] {
        let got = l.cat.get(&l.own, &"note".into(), get_at(key, None)).await;
        let fields = got.unwrap().data.fields;
        assert_eq!(fields.get("vec"), Some(&json!([1, 0])), "`{key}`");
    }
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
