//! Record writes queued in batches, held to the same writes made one by
//! one in queue order (lakelands#1332, lakelands#1313), on any engine:
//! each engine's tests include this file and say where its lakes are kept
//! ([`Place`]).
//!
//! Serial equivalence is the criterion: the log, and each write's answer
//! as the wire writes it, equal those of the same writes made alone, in
//! the order the queue took them, under a clock fixed for both lakes.
//! The writes come from a seeded generator, and from a soak whose callers
//! race and whose order the queue records. Each of the three stated
//! exceptions, a failed append, a failed fold and a panic in the shared
//! phase, has its own oracle over an engine that fails as told.

#![allow(dead_code)]

use std::panic::panic_any;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use lakelands_api::engine::{
    CanonicalEvent, Chosen, Declared, EngineOpener, LakeEngine, Lease, Matched, OpenMode,
    ProjectionStore, ProjectionView, Ranked, Rankings, Scan, VectorIndex,
};
use lakelands_api::error::Situation;
use lakelands_api::event::{Digest, Event, EventBody, EventId, SegmentDigest};
use lakelands_api::principal::TypeGrants;
use lakelands_api::query::{Edge, Walk};
use lakelands_api::record::SourceRecord;
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::{FeedFilter, Seams};
use lakelands_api::write::{Declaration, Outcome, Proposal, ProposalFilter};
use lakelands_api::{
    Access, Error, FieldName, Grants, IdempotencyKey, LakeId, LakeStore, Location, Patch, Position,
    Principal, PrincipalKind, RecordKey, RelationName, Result, SourceId, Timestamp, TypeName,
    Version, WriteContext, WriteOpts,
};
use lakelands_store::batch::{self, Asked, Batches, Write};
use serde_json::{Value, json};

/// Where an engine keeps a lake for a test: the location and seams of the
/// lake of a name, for each of the lakes a test compares, by index. Twins
/// share the lake's name, and so its id, and are kept apart.
pub type Place = Arc<dyn Fn(&str, usize) -> (Location, Seams) + Send + Sync>;

/// The time both lakes of a comparison read.
const AT: Timestamp = Timestamp(1_790_000_000_000_000);

/// Held while a test fixes the clock, which every lake of the process
/// reads.
static CLOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub fn owner(lake: &str, id: &str) -> Principal {
    Principal {
        kind: PrincipalKind::Human,
        id: id.into(),
        lake: lake.into(),
        grants: Grants::owner(),
    }
}

/// A writer of every type who is shown, and may write, only the items
/// tagged `a`: the grant field is `tag`.
fn tagged(lake: &str, id: &str) -> Principal {
    let item = TypeGrants {
        default: Some(Access::Write),
        only: [(FieldName::from("tag"), json!("a"))].into(),
        ..TypeGrants::default()
    };
    let grants = Grants {
        default: Access::Write,
        types: [(TypeName::from("item"), item)].into(),
        ..Grants::default()
    };
    Principal {
        grants,
        ..owner(lake, id)
    }
}

/// A writer of every type, `restricted` held, who is shown only the kids
/// tagged `a`: nothing across a kid's edge is held from it, so it is told
/// what a far write sets off at a kid it is shown.
fn kin(lake: &str, id: &str) -> Principal {
    let kid = TypeGrants {
        only: [(FieldName::from("tag"), json!("a"))].into(),
        ..TypeGrants::default()
    };
    let grants = Grants {
        default: Access::Write,
        types: [(TypeName::from("kid"), kid)].into(),
        restricted: true,
        ..Grants::default()
    };
    Principal {
        grants,
        ..owner(lake, id)
    }
}

/// What an engine is to do: fail or panic at the next so many appends,
/// panic after the next so many made their events durable, fail the next
/// so many reads of its tip, and of proposals; and how many appends it
/// made.
#[derive(Default)]
pub struct Faults {
    pub fail: AtomicUsize,
    pub panic: AtomicUsize,
    pub panic_after: AtomicUsize,
    pub tip: AtomicUsize,
    pub proposals: AtomicUsize,
    pub appends: AtomicUsize,
}

/// Whether the next of the `n` calls left to fail is due, counting it.
fn due(n: &AtomicUsize) -> bool {
    let mut left = n.load(Ordering::SeqCst);
    while left > 0 {
        match n.compare_exchange(left, left - 1, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return true,
            Err(now) => left = now,
        }
    }
    false
}

/// A lake opened for a test, its engine failing as `faults` say.
pub struct Opened {
    pub store: Arc<dyn LakeStore>,
    pub batches: Arc<Batches>,
    pub faults: Arc<Faults>,
}

impl Opened {
    /// The appends its engine made.
    fn appends(&self) -> usize {
        self.faults.appends.load(Ordering::SeqCst)
    }
}

/// The lake `twin` of `lake` at `place`, `bound` its batches' bound where
/// given, opened bare.
async fn bare(place: &Place, lake: &str, twin: usize, bound: Option<u32>) -> Opened {
    let (mut location, mut seams) = place(lake, twin);
    // Low enough that a far write is refused after its record was staged.
    location.limits.max_set_changes = 2;
    if let Some(bound) = bound {
        location.limits.commit_window_events = bound;
    }
    let faults = Arc::new(Faults::default());
    seams.engines = (seams.engines.drain(..))
        .map(|e| Arc::new(Faulty(e, faults.clone())) as Arc<dyn EngineOpener>)
        .collect();
    let (store, batches) = batch::open(location, seams, OpenMode::Create)
        .await
        .expect("the lake opens");
    let batches = Arc::new(batches);
    Opened {
        store,
        batches,
        faults,
    }
}

/// [`bare`], with the declarations of [`declared`].
pub async fn opened(place: &Place, lake: &str, twin: usize, bound: Option<u32>) -> Opened {
    let o = bare(place, lake, twin, bound).await;
    declared(o.store.as_ref(), lake).await;
    o
}

fn declaration(d: Value) -> Declaration {
    serde_json::from_value(d).expect("a declaration reads")
}

/// `item`, a number, a tag and a reference to an item, linked one to one
/// over `next` and many to many over `near`; the items whose number
/// passes 5, and those whose referred item's does; a trigger on those
/// whose referred item's number passes 7, across the edge; `kid`, a tag
/// and a reference to an item, and the kids whose item's number passes 5;
/// the items by tag, a view kept, and a trigger on it; and `obs`, which a
/// source feeds.
pub async fn declared(store: &dyn LakeStore, lake: &str) {
    let cat = store.catalog(WriteContext::default());
    let who = owner(lake, "owner");
    let text = json!({"kind": "text"});
    let declarations = [
        json!({"declare": "type", "name": "item",
            "fields": [{"name": "n", "type": {"kind": "number"}}, {"name": "tag", "type": text},
                       {"name": "up", "type": {"kind": "reference", "target": "item"}}],
            "relations": [{"name": "next", "target": "item", "cardinality": "one_to_one"},
                          {"name": "near", "target": "item", "cardinality": "many_to_many"}]}),
        json!({"declare": "object_set", "name": "big", "of": "item", "where": "n > 5"}),
        json!({"declare": "object_set", "name": "far", "of": "item", "where": "up.n > 5"}),
        json!({"declare": "trigger", "name": "hot", "on": "item", "when": "up.n > 7"}),
        json!({"declare": "type", "name": "kid",
            "fields": [{"name": "tag", "type": text},
                       {"name": "up", "type": {"kind": "reference", "target": "item"}}]}),
        json!({"declare": "object_set", "name": "kin", "of": "kid", "where": "up.n > 5"}),
        json!({"declare": "type", "name": "by_tag", "key": ["tag"],
            "view": {"source": "item", "group_by": ["tag"],
                     "measures": [{"name": "n", "expr": "count()"}]},
            "fields": [{"name": "tag", "type": text}, {"name": "n", "type": {"kind": "number"}}]}),
        json!({"declare": "type", "name": "obs",
            "fields": [{"name": "v", "type": {"kind": "number"}, "authority": "shared"}]}),
    ];
    for d in declarations {
        cat.declare(&who, declaration(d))
            .await
            .expect("it is declared");
    }
    let setting = json!({"key": "view_kept.by_tag", "value": true});
    let setting: Patch = serde_json::from_value(setting).expect("a patch");
    let (ty, key) = (
        TypeName::from("sys.setting"),
        RecordKey::from("view_kept.by_tag"),
    );
    cat.upsert(&who, &ty, &key, setting, WriteOpts::default())
        .await
        .expect("the view is kept");
    let crowded = json!({"declare": "trigger", "name": "crowded", "on": "by_tag", "when": "n > 1"});
    cat.declare(&who, declaration(crowded))
        .await
        .expect("it is declared");
}

fn patch(v: Value) -> Patch {
    serde_json::from_value(v).expect("a patch")
}

fn plain(who: Principal, write: Write) -> Asked {
    (WriteContext::default(), who, write, WriteOpts::default())
}

fn upsert_as(who: Principal, key: &str, v: Value) -> Asked {
    plain(who, Write::Upsert("item".into(), key.into(), patch(v)))
}

/// A generator of record writes, the same for the same seed, over 3 to 5
/// keys and two principals, an owner and one shown only the items tagged
/// `a`: upserts, some at a version from none to 3, some with values the
/// lake refuses, some referring to another item, retirements, links and
/// unlinks, some under one of 3 idempotency keys; and, each once at a
/// place the seed picks, the cases a batch could get wrong: a one-to-one
/// link moved within a batch, a far write that sets a kid in a set across
/// an edge a job before it laid, its writer shown the kid until a later
/// job moves it out of sight, a writer told of a group of a kept view that
/// later jobs empty, the retry of a write after an earlier job retired, or
/// moved out of its retrier's sight, the record it set something off at, a
/// retirement refused past the lake's `max_set_changes` after it retired a
/// link, then that link, that group and the record written again, a job
/// deeper than the ceiling beside one that is not, and a job that names a
/// cause between two that do not.
pub struct Writes {
    state: u64,
    keys: u64,
}

impl Writes {
    pub fn new(seed: u64) -> Writes {
        let state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        Writes {
            state,
            keys: 3 + seed % 3,
        }
    }

    fn next(&mut self, below: u64) -> u64 {
        self.state = (self.state.wrapping_mul(6_364_136_223_846_793_005))
            .wrapping_add(1_442_695_040_888_963_407);
        (self.state >> 33) % below
    }

    fn key(&mut self) -> String {
        format!("k{}", self.next(self.keys))
    }

    fn who(&mut self, lake: &str) -> Principal {
        match self.next(2) {
            0 => owner(lake, "ann"),
            _ => tagged(lake, "bob"),
        }
    }

    fn patch(&mut self) -> Patch {
        let (n, tag) = (self.next(10), ["a", "b", "a"][self.next(3) as usize]);
        patch(match self.next(14) {
            0 => json!({"n": "x"}),
            1 => json!({"n": 1u64 << 60}),
            2 => json!({"zz": 1}),
            3..=6 => json!({"n": n, "tag": tag, "up": self.key()}),
            7 => json!({"n": n, "up": null}),
            _ => json!({"n": n, "tag": tag}),
        })
    }

    /// One write at random.
    fn one(&mut self, lake: &str) -> Asked {
        let item = TypeName::from("item");
        let who = self.who(lake);
        let relation = ["next", "near"][self.next(2) as usize].into();
        let (key, to) = (RecordKey::from(self.key()), RecordKey::from(self.key()));
        let write = match self.next(10) {
            0..=4 => Write::Upsert(item, key, self.patch()),
            5 => Write::Retire(item, key),
            6..=8 => Write::Link(item, key, relation, to, Patch::new()),
            _ => Write::Unlink(item, key, relation, to),
        };
        let keyed = self.next(4) == 0;
        let idempotency_key = keyed.then(|| IdempotencyKey::from(format!("i{}", self.next(3))));
        let at = self.next(4) == 0;
        let expected_version = at.then(|| Version(self.next(4)));
        let opts = WriteOpts {
            expected_version,
            idempotency_key,
        };
        (WriteContext::default(), who, write, opts)
    }

    /// The case `which` of those a batch could get wrong.
    fn case(&mut self, lake: &str, which: u64) -> Vec<Asked> {
        let ann = || owner(lake, "ann");
        let item = || TypeName::from("item");
        let kid = |who, v| plain(who, Write::Upsert("kid".into(), "c1".into(), patch(v)));
        let link = |w: fn(TypeName, RecordKey, _, RecordKey) -> Write, to: &str| {
            plain(ann(), w(item(), "k0".into(), "next".into(), to.into()))
        };
        match which {
            0 => vec![
                upsert_as(ann(), "k1", json!({"n": 1, "tag": "a"})),
                upsert_as(ann(), "k2", json!({"n": 1, "tag": "a"})),
                link(|t, k, r, to| Write::Link(t, k, r, to, Patch::new()), "k1"),
                link(Write::Unlink, "k1"),
                link(|t, k, r, to| Write::Link(t, k, r, to, Patch::new()), "k2"),
            ],
            1 => vec![
                upsert_as(ann(), "p1", json!({"n": 0, "tag": "a"})),
                kid(ann(), json!({"tag": "a", "up": "p1"})),
                upsert_as(kin(lake, "cy"), "p1", json!({"n": 9})),
                kid(ann(), json!({"tag": "b"})),
                upsert_as(tagged(lake, "bob"), "b1", json!({"n": 1, "tag": "a"})),
                upsert_as(ann(), "f2", json!({"tag": "b"})),
                upsert_as(ann(), "b1", json!({"tag": "b"})),
            ],
            2 => {
                let retried = || {
                    let (ctx, who, write, _) =
                        upsert_as(tagged(lake, "bob"), "k2", json!({"n": 9, "tag": "a"}));
                    let idempotency_key = Some(IdempotencyKey::from("i9"));
                    let opts = WriteOpts {
                        expected_version: None,
                        idempotency_key,
                    };
                    (ctx, who, write, opts)
                };
                let between = match self.next(2) {
                    0 => plain(ann(), Write::Retire(item(), "k1".into())),
                    _ => upsert_as(ann(), "k1", json!({"tag": "b"})),
                };
                vec![
                    upsert_as(ann(), "k2", json!({"n": 0, "tag": "a"})),
                    upsert_as(ann(), "k1", json!({"n": 1, "tag": "a", "up": "k2"})),
                    retried(),
                    between,
                    retried(),
                ]
            }
            3 => {
                let at = |depth| {
                    let ctx = WriteContext {
                        depth,
                        ..WriteContext::default()
                    };
                    let (_, who, write, opts) = upsert_as(ann(), "k0", json!({"n": depth}));
                    (ctx, who, write, opts)
                };
                vec![at(9), at(8)]
            }
            4 => {
                let mut writes = vec![upsert_as(ann(), "k1", json!({"n": 6, "tag": "c"}))];
                for r in ["r0", "r1", "r2"] {
                    writes.push(upsert_as(ann(), r, json!({"n": 1, "tag": "b", "up": "k1"})));
                }
                let near = |w: fn(TypeName, RecordKey, _, RecordKey) -> Write| {
                    plain(ann(), w(item(), "k1".into(), "near".into(), "r0".into()))
                };
                writes.push(near(|t, k, r, to| Write::Link(t, k, r, to, Patch::new())));
                writes.push(plain(ann(), Write::Retire(item(), "k1".into())));
                writes.push(upsert_as(ann(), "q1", json!({"n": 0, "tag": "c"})));
                writes.push(near(Write::Unlink));
                writes.push(upsert_as(ann(), "k1", json!({"n": 2})));
                writes
            }
            _ => {
                let (_, who, write, opts) = upsert_as(ann(), "k1", json!({"n": 3}));
                let ctx = WriteContext {
                    depth: 1,
                    cause: Some(Position(1)),
                    ..WriteContext::default()
                };
                vec![
                    upsert_as(ann(), "k0", json!({"n": 2})),
                    (ctx, who, write, opts),
                    upsert_as(ann(), "k2", json!({"n": 4})),
                ]
            }
        }
    }

    /// `n` writes at random to `lake`, with each case placed once among
    /// them.
    pub fn take(&mut self, lake: &str, n: usize) -> Vec<Asked> {
        let mut writes: Vec<Asked> = (0..n).map(|_| self.one(lake)).collect();
        for which in 0..6 {
            let at = 1 + self.next(writes.len() as u64) as usize;
            let case = self.case(lake, which);
            writes.splice(at..at, case);
        }
        writes
    }
}

/// The whole log of `store`.
pub async fn log(store: &dyn LakeStore) -> Vec<Event> {
    let (feed, mut from, mut events) = (store.feed(), Position::GENESIS, Vec::new());
    loop {
        let page = feed
            .poll(from, &FeedFilter::default(), 500)
            .await
            .expect("the log reads");
        events.extend(page.events);
        if page.next > page.head {
            return events;
        }
        from = page.next;
    }
}

/// An answer as the wire writes it.
fn wire(answer: &Result<Outcome>) -> String {
    match answer {
        Ok(o) => serde_json::to_string(o).expect("an outcome writes"),
        Err(e) => serde_json::to_string(e).unwrap_or_else(|_| format!("{e:?}")),
    }
}

/// How writes are queued.
#[derive(Clone, Copy, Debug, PartialEq)]
enum How {
    /// Each decided and appended alone, after the one before is answered.
    Alone,
    /// Each queued as an entry of its own, after the one before is
    /// answered.
    OfOne,
    /// All queued as one entry.
    OfAll,
    /// Each queued by a caller of its own while the turn is held, and
    /// taken from the queue together when it is let go.
    Queued,
}

/// Wait until `done`.
pub async fn until(done: impl Fn() -> bool) {
    let began = Instant::now();
    while !done() {
        assert!(began.elapsed() < Duration::from_secs(30), "waited 30 s");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

/// `writes` queued as `how` says in `o`, their answers in order.
async fn queued(o: &Opened, writes: Vec<Asked>, how: How) -> Vec<Result<Outcome>> {
    let mut answers = Vec::new();
    match how {
        How::Alone => {
            for (ctx, who, write, opts) in writes {
                answers.push(o.batches.alone(ctx, (who, write, opts)).await);
            }
        }
        How::OfOne => {
            for w in writes {
                answers.extend(o.batches.write(vec![w]).await);
            }
        }
        How::OfAll => answers = o.batches.write(writes).await,
        How::Queued => {
            let held = o.batches.hold();
            until(|| o.batches.queued() == 0).await;
            let mut callers = Vec::new();
            for (i, w) in writes.into_iter().enumerate() {
                let batches = o.batches.clone();
                callers.push(tokio::spawn(async move { batches.write(vec![w]).await }));
                until(|| o.batches.queued() == i + 1).await;
            }
            drop(held);
            for c in callers {
                answers.extend(c.await.expect("a caller ends"));
            }
        }
    }
    answers
}

/// What a lake answered, logged and holds, and how many appends its
/// writes took.
struct Written {
    answers: Vec<String>,
    log: Vec<Event>,
    held: String,
    appends: usize,
}

/// What `lake` answers its owner of the items, the kids and the kept view by
/// tag, as the wire writes it.
async fn held(store: &dyn LakeStore, lake: &str) -> String {
    let (cat, who) = (store.catalog(WriteContext::default()), owner(lake, "owner"));
    let mut held = String::new();
    for ty in ["item", "kid", "by_tag"] {
        let all = serde_json::from_value(json!({"limit": 500})).expect("a find");
        let found = cat.find(&who, &ty.into(), all).await;
        held += &serde_json::to_string(&found.map_err(|e| e.to_string())).expect("it writes");
    }
    held
}

/// `writes` made in the lake `twin` of `place`, as `how` queues them.
async fn written(
    place: &Place,
    (lake, twin, bound): (&str, usize, Option<u32>),
    writes: &[Asked],
    how: How,
) -> Written {
    let o = opened(place, lake, twin, bound).await;
    let before = o.appends();
    let answers = queued(&o, writes.to_vec(), how).await;
    let written = Written {
        answers: answers.iter().map(wire).collect(),
        log: log(o.store.as_ref()).await,
        held: held(o.store.as_ref(), lake).await,
        appends: o.appends() - before,
    };
    o.store.close().await.expect("the lake closes");
    written
}

/// `b` answers and logs what `a` does.
fn same(a: &Written, b: &Written, what: &str) {
    assert_eq!(a.answers.len(), b.answers.len(), "{what}: the answers");
    for (i, (x, y)) in a.answers.iter().zip(&b.answers).enumerate() {
        assert_eq!(x, y, "{what}: answer {i}");
    }
    assert_eq!(a.log.len(), b.log.len(), "{what}: the log's length");
    for (x, y) in a.log.iter().zip(&b.log) {
        assert_eq!(x, y, "{what}: the log");
    }
    assert_eq!(a.held, b.held, "{what}: what the lake holds");
}

/// Over seeded writes, batches answer and log what the writes alone do,
/// in queue order: batches of one, one entry of them all, under the
/// default bound and at a bound of 1, and callers of their own taken
/// together at a bound of 4; and the batches share appends.
pub async fn a_batch_answers_and_logs_as_its_writes_one_by_one(place: &Place, name: &str) {
    let _clock = CLOCK.lock().await;
    batch::fix_clock(Some(AT));
    for seed in 0..6 {
        let lake = format!("{name}-{seed}");
        let writes = Writes::new(seed).take(&lake, 60);
        let alone = written(place, (&lake, 0, None), &writes, How::Alone).await;
        let applied = alone.answers.iter().filter(|a| a.contains("applied"));
        assert!(applied.count() > 20, "seed {seed}: {:?}", alone.answers);
        let refused = alone.answers.iter().filter(|a| !a.contains("applied"));
        assert!(refused.count() > 5, "seed {seed}: {:?}", alone.answers);
        let twins = [
            (How::OfOne, None),
            (How::OfAll, None),
            (How::OfAll, Some(1)),
            (How::Queued, Some(4)),
        ];
        for (twin, (how, bound)) in twins.into_iter().enumerate() {
            let batched = written(place, (&lake, twin + 1, bound), &writes, how).await;
            same(
                &alone,
                &batched,
                &format!("seed {seed}, {how:?} at {bound:?}"),
            );
            if how != How::OfOne && bound != Some(1) {
                assert!(
                    batched.appends < alone.appends,
                    "seed {seed}, {how:?}: {} appends, alone {}",
                    batched.appends,
                    alone.appends
                );
            }
        }
    }
    batch::fix_clock(None);
}

/// The engine `inner` opens, failing as `faults` say.
pub struct Faulty(pub Arc<dyn EngineOpener>, pub Arc<Faults>);

struct FaultyEngine(Box<dyn LakeEngine>, Arc<Faults>);

impl EngineOpener for Faulty {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn log_ends(&self, location: &Location) -> Result<Option<(CanonicalEvent, CanonicalEvent)>> {
        self.0.log_ends(location)
    }
    fn open(&self, location: &Location, mode: OpenMode) -> Result<Box<dyn LakeEngine>> {
        Ok(Box::new(FaultyEngine(
            self.0.open(location, mode)?,
            self.1.clone(),
        )))
    }
    fn open_scratch(&self, location: &Location) -> Result<Box<dyn LakeEngine>> {
        self.0.open_scratch(location)
    }
    fn room(&self, location: &Location) -> Result<u64> {
        self.0.room(location)
    }
}

fn planted(what: &str) -> Error {
    let lake = format!("{what} (planted)");
    Error::unavailable(Situation::Busy { lake })
}

impl LakeEngine for FaultyEngine {
    fn name(&self) -> &str {
        self.0.name()
    }
    fn rebuild_rate(&self) -> u64 {
        self.0.rebuild_rate()
    }
    fn lake(&self) -> &LakeId {
        self.0.lake()
    }
    fn take_lease(&self, holder: &str) -> Result<Lease> {
        self.0.take_lease(holder)
    }
    fn release_lease(&self, lease: &Lease) -> Result<()> {
        self.0.release_lease(lease)
    }
    fn tip(&self) -> Result<Option<(Position, Digest)>> {
        if due(&self.1.tip) {
            return Err(planted("the tip's read failed"));
        }
        self.0.tip()
    }
    fn append(&self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        self.1.appends.fetch_add(1, Ordering::SeqCst);
        if due(&self.1.panic) {
            panic_any("an append panicked (planted)");
        }
        if due(&self.1.fail) {
            return Err(planted("an append failed"));
        }
        let appended = self.0.append(lease, events);
        if due(&self.1.panic_after) {
            panic_any("an append panicked once durable (planted)");
        }
        appended
    }
    fn import(&self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        self.0.import(lease, events)
    }
    fn read(&self, from: Position, limit: u32) -> Result<Vec<CanonicalEvent>> {
        self.0.read(from, limit)
    }
    fn seal_segment(
        &self,
        lease: &Lease,
        imported: bool,
        first: &mut dyn FnMut(&SegmentDigest) -> Result<Vec<CanonicalEvent>>,
    ) -> Result<Option<SegmentDigest>> {
        self.0.seal_segment(lease, imported, first)
    }
    fn segments(&self) -> Result<Vec<SegmentDigest>> {
        self.0.segments()
    }
    fn projection(&self) -> &dyn ProjectionStore {
        self.0.projection()
    }
    fn view(&self, as_of: Option<Position>) -> Result<Box<dyn ProjectionView + '_>> {
        Ok(Box::new(FaultyView(self.0.view(as_of)?, &self.1)))
    }
    fn ranked(&self, ty: &TypeName, asked: &Rankings, k: u32) -> Result<Ranked> {
        self.0.ranked(ty, asked, k)
    }
    fn vectors(&self) -> Option<&dyn VectorIndex> {
        self.0.vectors()
    }
    fn mark_built(&self) -> Result<Option<Position>> {
        self.0.mark_built()
    }
    fn swap(&self, lease: &Lease) -> Result<Option<Position>> {
        self.0.swap(lease)
    }
    fn may_swap(&self) -> Result<bool> {
        self.0.may_swap()
    }
}

pub fn upsert(lake: &str, key: &str, n: u64) -> Asked {
    upsert_as(owner(lake, "ann"), key, json!({"n": n}))
}

/// A write the lake refuses: an upsert of a record that exists at no
/// version yet, at version 7.
pub fn refused(lake: &str, key: &str) -> Asked {
    let (ctx, who, write, _) = upsert(lake, key, 1);
    let opts = WriteOpts {
        expected_version: Some(Version(7)),
        idempotency_key: None,
    };
    (ctx, who, write, opts)
}

/// `w` under the idempotency key `key`.
pub fn keyed((ctx, who, write, _): Asked, key: &str) -> Asked {
    let opts = WriteOpts {
        expected_version: None,
        idempotency_key: Some(IdempotencyKey::from(key)),
    };
    (ctx, who, write, opts)
}

/// A setting written, which runs alone and ends a batch: an order index
/// on the items' numbers.
fn setting(lake: &str) -> Asked {
    let p = patch(json!({"key": "sort_index.item", "value": ["n"]}));
    let write = Write::Upsert("sys.setting".into(), "sort_index.item".into(), p);
    plain(owner(lake, "owner"), write)
}

pub fn situation(answer: &Result<Outcome>) -> Option<&Situation> {
    answer.as_ref().err().and_then(Error::situation)
}

/// The position a write that applied answers.
pub fn position(answer: &Result<Outcome>) -> Position {
    match answer {
        Ok(Outcome::Applied { position, .. }) => *position,
        other => panic!("the write applies: {other:?}"),
    }
}

/// A batch: an upsert, a refused write, an upsert, a retry of the first.
fn mixed(lake: &str) -> Vec<Asked> {
    let a = keyed(upsert(lake, "a", 1), "r1");
    vec![a.clone(), refused(lake, "r"), upsert(lake, "b", 2), a]
}

/// A batch whose append fails answers every job that failure, the
/// refused job and the retry too, and leaves the log as it was; the lake
/// writes again, and the batch made again answers and logs what its
/// writes alone do.
pub async fn a_failed_append_answers_every_job_and_a_rerun_is_serial(place: &Place, name: &str) {
    let _clock = CLOCK.lock().await;
    batch::fix_clock(Some(AT));
    let o = opened(place, name, 0, None).await;
    let before = log(o.store.as_ref()).await;
    o.faults.fail.store(1, Ordering::SeqCst);
    let failed = o.batches.write(mixed(name)).await;
    for answer in &failed {
        let busy = matches!(situation(answer), Some(Situation::Busy { .. }));
        assert!(busy, "every job is answered the failure: {failed:?}");
    }
    assert_eq!(log(o.store.as_ref()).await, before, "the log is as it was");
    let again = o.batches.write(mixed(name)).await;
    let again = Written {
        answers: again.iter().map(wire).collect(),
        log: log(o.store.as_ref()).await,
        held: held(o.store.as_ref(), name).await,
        appends: 0,
    };
    o.store.close().await.expect("the lake closes");
    let alone = written(place, (name, 1, None), &mixed(name), How::Alone).await;
    same(&alone, &again, "the batch made again");
    batch::fix_clock(None);
}

/// A view of a faulty engine: it fails the reads of proposals it is told
/// to.
struct FaultyView<'v>(Box<dyn ProjectionView + 'v>, &'v Faults);

impl ProjectionView for FaultyView<'_> {
    fn position(&self) -> Option<Position> {
        self.0.position()
    }
    fn declarations(&self) -> Result<Vec<Declared>> {
        self.0.declarations()
    }
    fn replaced(&self, from: Position, to: Position) -> Result<Option<Vec<Declared>>> {
        self.0.replaced(from, to)
    }
    fn chosen(&self, ty: &TypeName) -> Result<Chosen> {
        self.0.chosen(ty)
    }
    fn get(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>> {
        self.0.get(ty, key)
    }
    fn scan(&self, ty: &TypeName, scan: &Scan) -> Result<Vec<StoredRecord>> {
        self.0.scan(ty, scan)
    }
    fn count(&self, ty: &TypeName, scan: &Scan) -> Result<u64> {
        self.0.count(ty, scan)
    }
    fn get_many(&self, ty: &TypeName, keys: &[RecordKey]) -> Result<Vec<StoredRecord>> {
        self.0.get_many(ty, keys)
    }
    fn edges(
        &self,
        ty: &TypeName,
        keys: &[RecordKey],
        relation: Option<&RelationName>,
        walk: Walk,
    ) -> Result<Vec<Vec<Edge>>> {
        self.0.edges(ty, keys, relation, walk)
    }
    fn proposals(&self, filter: &ProposalFilter) -> Result<Vec<Proposal>> {
        if due(&self.1.proposals) {
            return Err(planted("a read of proposals failed"));
        }
        self.0.proposals(filter)
    }
    fn proposals_before(
        &self,
        filter: &ProposalFilter,
        at: Option<Position>,
    ) -> Result<Option<Vec<Proposal>>> {
        self.0.proposals_before(filter, at)
    }
    fn is_member(&self, set: &str, ty: &TypeName, key: &RecordKey) -> Result<bool> {
        self.0.is_member(set, ty, key)
    }
    fn members(&self, set: &str, ty: &TypeName) -> Result<std::collections::BTreeSet<RecordKey>> {
        self.0.members(set, ty)
    }
    fn applied(&self, id: &EventId) -> Result<Option<Position>> {
        self.0.applied(id)
    }
    fn applied_many(&self, ids: &[EventId]) -> Result<Vec<Option<Position>>> {
        self.0.applied_many(ids)
    }
    fn output(&self, seam: &str, input: &Digest) -> Result<Option<(Position, Value)>> {
        self.0.output(seam, input)
    }
    fn matching(
        &self,
        filter: &FeedFilter,
        from: Position,
        through: Position,
        limit: u32,
    ) -> Result<Matched> {
        self.0.matching(filter, from, through, limit)
    }
    fn of_source(
        &self,
        source: &SourceId,
        from: Position,
        through: Position,
        limit: u32,
    ) -> Result<Matched> {
        self.0.of_source(source, from, through, limit)
    }
}

/// `x` made, then a batch of its retirement, whose read of the record's
/// pending calls fails after its reactions are staged, and of an upsert
/// of `x`, in the lake `twin` of `place`, as `how` queues them.
async fn refused_after_staging(place: &Place, lake: &str, twin: usize, how: How) -> Written {
    let o = opened(place, lake, twin, None).await;
    position(&o.batches.write(vec![upsert(lake, "x", 6)]).await[0]);
    o.faults.proposals.store(1, Ordering::SeqCst);
    let retire = plain(owner(lake, "ann"), Write::Retire("item".into(), "x".into()));
    let answers = queued(&o, vec![retire, upsert(lake, "x", 2)], how).await;
    let read = matches!(situation(&answers[0]), Some(Situation::Busy { .. }));
    assert!(read, "the retirement is refused its read: {answers:?}");
    let written = Written {
        answers: answers.iter().map(wire).collect(),
        log: log(o.store.as_ref()).await,
        held: held(o.store.as_ref(), lake).await,
        appends: 0,
    };
    o.store.close().await.expect("the lake closes");
    written
}

/// A job refused after it staged what it changes leaves its batch as it
/// found it: a retirement refused after its reactions are staged, then an
/// upsert of its record in the same batch, answer and log what they do
/// alone.
pub async fn a_job_refused_after_staging_leaves_its_batch_as_it_found_it(
    place: &Place,
    name: &str,
) {
    let _clock = CLOCK.lock().await;
    batch::fix_clock(Some(AT));
    let alone = refused_after_staging(place, name, 0, How::Alone).await;
    let batched = refused_after_staging(place, name, 1, How::OfAll).await;
    same(&alone, &batched, "a batch with a job refused after staging");
    batch::fix_clock(None);
}

/// The fences of the `lease_taken` events of `log`, each once.
fn fences_once(log: &[Event]) {
    let fences: Vec<u64> = (log.iter())
        .filter_map(|e| match &e.body {
            EventBody::LeaseTaken { fence, .. } => Some(*fence),
            _ => None,
        })
        .collect();
    let mut once = fences.clone();
    once.dedup();
    assert_eq!(fences, once, "no second `lease_taken` with an equal fence");
}

/// A panic in a batch's shared phase answers every job of it `Stopped`,
/// the refused one too; a setting queued behind it still runs, the lake
/// writes again from its log's end, only what was answered applied is in
/// the log, and the lease was not taken again.
pub async fn a_panicked_batch_answers_every_job_stopped(place: &Place, name: &str) {
    let o = opened(place, name, 0, None).await;
    let head = o.store.head().await.expect("a head");
    o.faults.panic.store(1, Ordering::SeqCst);
    let mut writes = mixed(name);
    writes.push(setting(name));
    let stopped = o.batches.write(writes).await;
    for answer in &stopped[..4] {
        let s = situation(answer);
        assert!(matches!(s, Some(Situation::Stopped { .. })), "{stopped:?}");
    }
    position(&stopped[4]);
    let again = position(&o.batches.write(vec![upsert(name, "c", 3)]).await[0]);
    let written: Vec<_> = (log(o.store.as_ref()).await.into_iter())
        .filter(|e| e.position > head)
        .collect();
    assert_eq!(written.last().map(|e| e.position), Some(again));
    let keys = |e: &Event| match &e.body {
        EventBody::RecordUpserted { key, .. } => Some(key.to_string()),
        _ => None,
    };
    let keys: Vec<_> = written.iter().filter_map(keys).collect();
    assert_eq!(keys, ["sort_index.item", "c"], "only what was answered");
    fences_once(&log(o.store.as_ref()).await);
    o.store.close().await.expect("the lake closes");
}

/// A batch whose append panics once its events are durable is answered
/// `Stopped`; after a `rebuild`, or after one more write that meets the
/// engine's tip failing, the next write lands at the engine's position:
/// right after what the panicked batch left in the log.
pub async fn a_batch_that_panicked_once_durable_is_caught_up(place: &Place, name: &str) {
    for (twin, tip_fails) in [(0, false), (1, true)] {
        let o = opened(place, name, twin, None).await;
        o.faults.panic_after.store(1, Ordering::SeqCst);
        let stopped = o.batches.write(mixed(name)).await;
        let s = situation(&stopped[0]);
        assert!(matches!(s, Some(Situation::Stopped { .. })), "{stopped:?}");
        match tip_fails {
            false => drop(o.store.rebuild().await.expect("it rebuilds")),
            true => {
                o.faults.tip.store(1, Ordering::SeqCst);
                let failed = o.batches.write(vec![upsert(name, "t", 1)]).await;
                assert!(failed[0].is_err(), "the tip's read fails: {failed:?}");
            }
        }
        let next = position(&o.batches.write(vec![upsert(name, "c", 3)]).await[0]);
        let after = log(o.store.as_ref()).await;
        let at = |key: &str| {
            let upserted = |e: &&Event| matches!(&e.body, EventBody::RecordUpserted { key: k, ty, .. } if ty.as_str() == "item" && k.as_str() == key);
            after.iter().find(upserted).map(|e| e.position)
        };
        assert_eq!(after.last().map(|e| e.position), Some(next));
        let (b, c) = (at("b").expect("the durable batch stays"), at("c"));
        assert!(at("a") < Some(b), "the durable batch stays");
        assert_eq!(c, Some(next), "the engine's position, after {b:?}");
        assert!(after.iter().all(|e| e.position <= b || e.position >= next));
        fences_once(&after);
        o.store.close().await.expect("the lake closes");
    }
}

/// A batch whose fold fails, as `refuse` makes the next fold of the lake
/// at `place` fail, answers each job that appended so at its own first
/// position, a refused job its refusal, and a retry of a job before it in
/// the batch at the batch's first; what landed is durable, and the next
/// write folds it.
pub async fn a_batch_whose_fold_fails_answers_each_job_at_its_own_position(
    place: &Place,
    name: &str,
    refuse: &dyn Fn(),
) {
    let o = opened(place, name, 0, None).await;
    refuse();
    let failed = o.batches.write(mixed(name)).await;
    let from = |answer: &Result<Outcome>| match situation(answer) {
        Some(Situation::NotFolded { from, .. }) => *from,
        _ => panic!("not folded: {answer:?}"),
    };
    assert!(
        failed[1].is_err() && situation(&failed[1]).is_none(),
        "the refusal stays: {:?}",
        failed[1]
    );
    assert!(from(&failed[0]) < from(&failed[2]), "{failed:?}");
    assert_eq!(
        from(&failed[3]),
        from(&failed[0]),
        "a retry is told at the batch's first"
    );
    let after = position(&o.batches.write(vec![upsert(name, "c", 3)]).await[0]);
    assert!(after > from(&failed[2]));
    let keys = |e: &Event| match &e.body {
        EventBody::RecordUpserted { key, ty, .. } if ty.as_str() == "item" => Some(key.to_string()),
        _ => None,
    };
    let keys: Vec<_> = (log(o.store.as_ref()).await.iter())
        .filter_map(keys)
        .collect();
    assert_eq!(keys, ["a", "b", "c"], "what landed stays");
    o.store.close().await.expect("the lake closes");
}

/// An ingest window queued between record writes closes their batch:
/// the writes before it share one append, it takes its own, and those
/// after it share another, in the order they were queued.
pub async fn an_ingest_window_closes_a_batch(place: &Place, name: &str) {
    let o = opened(place, name, 0, None).await;
    let held = o.batches.hold();
    until(|| o.batches.queued() == 0).await;
    let mut callers = Vec::new();
    for (i, key) in ["u1", "u2", "ingest", "u3", "u4"].into_iter().enumerate() {
        let (batches, store, lake) = (o.batches.clone(), o.store.clone(), name.to_owned());
        callers.push(tokio::spawn(async move {
            if key != "ingest" {
                return position(&batches.write(vec![upsert(&lake, key, 1)]).await[0]);
            }
            let cat = store.catalog(WriteContext::default());
            let record = SourceRecord {
                ty: "obs".into(),
                key: "o1".into(),
                fields: patch(json!({"v": 1})),
                seen_at: None,
                upstream: None,
                source: None,
            };
            let source = SourceId::from("feed");
            let who = owner(&lake, "owner");
            let fed = lakelands_api::ingest_all(cat.as_ref(), &who, &source, vec![record]);
            fed.await.expect("it is ingested")[0].position
        }));
        until(|| o.batches.queued() == i + 1).await;
    }
    let before = o.appends();
    drop(held);
    let mut at = Vec::new();
    for c in callers {
        at.push(c.await.expect("a caller ends"));
    }
    assert_eq!(o.appends() - before, 3, "two batches and the window");
    assert!(at.windows(2).all(|w| w[0] < w[1]), "in queue order: {at:?}");
    o.store.close().await.expect("the lake closes");
}

/// Callers racing over seeded writes, under a fixed clock, the order the
/// queue took their writes in recorded; the same writes made alone in
/// that order, in a fresh lake of the same id, answer and log the same.
pub async fn a_soak_replays_serially(place: &Place, name: &str) {
    let _clock = CLOCK.lock().await;
    batch::fix_clock(Some(AT));
    let o = opened(place, name, 0, None).await;
    batch::record_order(true);
    let (callers, each) = (12, 40);
    let mut running = Vec::new();
    for c in 0..callers {
        // Each caller writes as principals of its own, so that the order
        // recorded says whose each write is.
        let mut writes = Writes::new(c).take(name, each);
        for (_, who, ..) in &mut writes {
            who.id = format!("{}.{c}", who.id);
        }
        let batches = o.batches.clone();
        running.push(tokio::spawn(async move {
            let mut answers = Vec::new();
            for w in writes {
                answers.extend(batches.write(vec![w]).await);
            }
            answers
        }));
    }
    let mut answered = Vec::new();
    for r in running {
        answered.push(r.await.expect("a caller ends").into_iter());
    }
    let order = batch::order(name);
    batch::record_order(false);
    let (raced, holds) = (
        log(o.store.as_ref()).await,
        held(o.store.as_ref(), name).await,
    );
    let appends = o.appends();
    o.store.close().await.expect("the lake closes");
    let caller = |who: &Principal| -> usize {
        let id = who.id.as_str();
        id[id.rfind('.').expect("a caller's") + 1..]
            .parse()
            .expect("a caller")
    };
    let mut answers = Vec::new();
    for (_, who, ..) in &order {
        answers.push(wire(&answered[caller(who)].next().expect("an answer")));
    }
    assert!(answered.iter_mut().all(|a| a.next().is_none()), "each once");
    let raced = Written {
        answers,
        log: raced,
        held: holds,
        appends,
    };
    let serial = written(place, (name, 1, None), &order, How::Alone).await;
    same(&serial, &raced, "the soak, replayed");
    assert!(raced.appends < serial.appends, "the callers shared appends");
    batch::fix_clock(None);
}

/// Writes a second of `callers` callers, each writing a record of a plain
/// type after another, and the median latency: printed, never asserted.
/// Run by hand, `--release`, against the engine `place` keeps.
pub async fn throughput(place: &Place, name: &str) {
    for (twin, callers) in [1usize, 10, 64, 100, 500].into_iter().enumerate() {
        let (location, seams) = place(name, twin);
        let store = lakelands_store::open(location, seams, OpenMode::Create)
            .await
            .expect("the lake opens");
        let who = owner(name, "owner");
        let cat = store.catalog(WriteContext::default());
        let plain = json!({"declare": "type", "name": "plain",
                           "fields": [{"name": "n", "type": {"kind": "number"}}]});
        cat.declare(&who, declaration(plain))
            .await
            .expect("declared");
        let each = (3000 / callers).max(8);
        let began = Instant::now();
        let mut running = Vec::new();
        for c in 0..callers {
            let (cat, who) = (store.catalog(WriteContext::default()), who.clone());
            running.push(tokio::spawn(async move {
                let mut took = Vec::with_capacity(each);
                for i in 0..each {
                    let (ty, key) = ("plain".into(), format!("c{c}-{i}").into());
                    let at = Instant::now();
                    let p = patch(json!({"n": i}));
                    cat.upsert(&who, &ty, &key, p, WriteOpts::default())
                        .await
                        .expect("it applies");
                    took.push(at.elapsed());
                }
                took
            }));
        }
        let mut took = Vec::new();
        for r in running {
            took.extend(r.await.expect("a caller ends"));
        }
        let secs = began.elapsed().as_secs_f64();
        took.sort();
        let p50 = took[took.len() / 2];
        let rate = took.len() as f64 / secs;
        println!("{name}: {callers:>3} callers, {rate:>7.0} writes/s, p50 {p50:?}");
        store.close().await.expect("the lake closes");
    }
}

/// A write of a type a backing computes, whose fold `refuse` makes fail,
/// then plain upserts queued behind it by callers of their own: the
/// first runs alone, folds what the failed write left, makes its call,
/// once, as `handed` counts, and is answered at its recorded output; the
/// rest share one append again.
pub async fn a_backed_write_whose_fold_failed_is_caught_up_alone(
    place: &Place,
    name: &str,
    refuse: &dyn Fn(),
    handed: &dyn Fn() -> usize,
) {
    let o = opened(place, name, 0, None).await;
    let (cat, who) = (
        o.store.catalog(WriteContext::default()),
        owner(name, "owner"),
    );
    let f = json!({"declare": "function", "name": "f", "backing": "counting",
                   "inputs": [{"name": "n", "type": {"kind": "number"}}]});
    let doc = json!({"declare": "type", "name": "doc",
        "fields": [{"name": "n", "type": {"kind": "number"}},
                   {"name": "sum", "type": {"kind": "number"}, "authority": "derived",
                    "function": "f", "depends_on": ["n"]}]});
    for d in [f, doc] {
        cat.declare(&who, declaration(d))
            .await
            .expect("it is declared");
    }
    refuse();
    let backed = plain(
        who,
        Write::Upsert("doc".into(), "d".into(), patch(json!({"n": 1}))),
    );
    let failed = o.batches.write(vec![backed]).await;
    let s = situation(&failed[0]);
    assert!(matches!(s, Some(Situation::NotFolded { .. })), "{failed:?}");
    assert_eq!(handed(), 0, "no call before the fold");
    let held = o.batches.hold();
    until(|| o.batches.queued() == 0).await;
    let mut callers = Vec::new();
    for (i, key) in ["u1", "u2", "u3", "u4"].into_iter().enumerate() {
        let (batches, lake) = (o.batches.clone(), name.to_owned());
        let w = upsert(&lake, key, 1);
        callers.push(tokio::spawn(async move { batches.write(vec![w]).await }));
        until(|| o.batches.queued() == i + 1).await;
    }
    let before = o.appends();
    drop(held);
    let mut at = Vec::new();
    for c in callers {
        at.push(position(&c.await.expect("a caller ends")[0]));
    }
    assert_eq!(handed(), 1, "the call is made once");
    let recorded = (log(o.store.as_ref()).await.into_iter())
        .find(|e| matches!(e.body, EventBody::OutputRecorded { .. }))
        .map(|e| e.position)
        .expect("the output is recorded");
    let u1 = (log(o.store.as_ref()).await.into_iter())
        .find(|e| matches!(&e.body, EventBody::RecordUpserted { key, .. } if key.as_str() == "u1"))
        .map(|e| e.position);
    assert!(u1 < Some(recorded), "the first appended before its call");
    assert_eq!(at[0], recorded, "the first is answered after the output");
    assert_eq!(o.appends() - before, 3, "the output, the first, the rest");
    o.store.close().await.expect("the lake closes");
}
