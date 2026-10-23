//! Record writes queued as batches, held to the same writes made alone
//! (lakelands#1332), on any engine: each engine's tests include this
//! file and say where its lakes are kept ([`Place`]).
//!
//! A batch of one answers and logs what the write alone does, and so does
//! one batch of every write, under a clock fixed for both lakes. A failed
//! or stopped append answers every job of its batch, a job refused keeps
//! its refusal, and the lake writes again.

#![allow(dead_code)]

use std::panic::panic_any;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use lakelands_api::engine::{
    CanonicalEvent, EngineOpener, LakeEngine, Lease, OpenMode, ProjectionStore, ProjectionView,
    Ranked, Rankings, VectorIndex,
};
use lakelands_api::error::Situation;
use lakelands_api::event::{Digest, Event, EventBody, SegmentDigest};
use lakelands_api::principal::TypeGrants;
use lakelands_api::seam::{FeedFilter, Seams};
use lakelands_api::write::{Declaration, Outcome};
use lakelands_api::{
    Access, Error, FieldName, Grants, IdempotencyKey, LakeId, LakeStore, Location, Patch, Position,
    Principal, PrincipalKind, RecordKey, Result, Timestamp, TypeName, Version, WriteContext,
    WriteOpts,
};
use lakelands_store::batch::{self, Batches, Write};
use serde_json::json;

/// Where an engine keeps a lake for a test: the location and seams of the
/// lake of a name, for each of the lakes a test compares, by index. Twins
/// share the lake's name and are kept apart.
pub type Place = Arc<dyn Fn(&str, usize) -> (Location, Seams) + Send + Sync>;

/// The time both lakes of a comparison read.
const AT: Timestamp = Timestamp(1_790_000_000_000_000);

pub fn owner(lake: &str, id: &str) -> Principal {
    Principal {
        kind: PrincipalKind::Human,
        id: id.into(),
        lake: lake.into(),
        grants: Grants::owner(),
    }
}

/// A writer of `item` who may write its tag and never read it: what
/// each write answers it leaves the tag out.
fn blind(lake: &str) -> Principal {
    let item = TypeGrants {
        default: Some(Access::Write),
        fields: [(FieldName::from("tag"), Access::WriteOnly)].into(),
        ..TypeGrants::default()
    };
    let grants = Grants {
        default: Access::None,
        types: [(TypeName::from("item"), item)].into(),
        ..Grants::default()
    };
    Principal {
        grants,
        ..owner(lake, "cy")
    }
}

pub async fn opened(place: &Place, lake: &str, twin: usize) -> (Arc<dyn LakeStore>, Batches) {
    let (location, seams) = place(lake, twin);
    let (store, batches) = batch::open(location, seams, OpenMode::Create)
        .await
        .expect("the lake opens");
    declared(store.as_ref(), lake).await;
    (store, batches)
}

/// `item`, a number and a tag, linked one to one over `next` and many to
/// many over `near`; the items whose number passes 5; and the items by
/// tag, a view kept.
pub async fn declared(store: &dyn LakeStore, lake: &str) {
    let cat = store.catalog(WriteContext::default());
    let who = owner(lake, "owner");
    let text = json!({"kind": "text"});
    let declarations = [
        json!({"declare": "type", "name": "item",
            "fields": [{"name": "n", "type": {"kind": "number"}}, {"name": "tag", "type": text}],
            "relations": [{"name": "next", "target": "item", "cardinality": "one_to_one"},
                          {"name": "near", "target": "item", "cardinality": "many_to_many"}]}),
        json!({"declare": "object_set", "name": "big", "of": "item", "where": "n > 5"}),
        json!({"declare": "type", "name": "by_tag", "key": ["tag"],
            "view": {"source": "item", "group_by": ["tag"],
                     "measures": [{"name": "n", "expr": "count()"}]},
            "fields": [{"name": "tag", "type": text}, {"name": "n", "type": {"kind": "number"}}]}),
    ];
    for d in declarations {
        let d: Declaration = serde_json::from_value(d).expect("a declaration reads");
        cat.declare(&who, d).await.expect("it is declared");
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
}

/// A generator of record writes, the same for the same seed: upserts,
/// some at a version, retirements, links and unlinks over a few keys, by
/// two owners and a writer blind to the tag, some under an idempotency
/// key used again.
pub struct Writes(u64);

impl Writes {
    pub fn new(seed: u64) -> Writes {
        Writes(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self, below: u64) -> u64 {
        self.0 = (self.0.wrapping_mul(6_364_136_223_846_793_005))
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % below
    }

    fn key(&mut self) -> RecordKey {
        RecordKey::from(format!("k{}", self.next(6)))
    }

    /// `n` writes to `lake`.
    pub fn take(&mut self, lake: &str, n: usize) -> Vec<(Principal, Write, WriteOpts)> {
        let item = TypeName::from("item");
        (0..n)
            .map(|_| {
                let who = match self.next(3) {
                    2 => blind(lake),
                    w => owner(lake, ["ann", "bob"][w as usize]),
                };
                let relation = ["next", "near"][self.next(2) as usize].into();
                let write = match self.next(10) {
                    0..=4 => {
                        let (n, tag) = (self.next(10), ["a", "b", "c"][self.next(3) as usize]);
                        let patch = serde_json::from_value(json!({"n": n, "tag": tag}));
                        Write::Upsert(item.clone(), self.key(), patch.expect("a patch"))
                    }
                    5 => Write::Retire(item.clone(), self.key()),
                    6..=8 => {
                        Write::Link(item.clone(), self.key(), relation, self.key(), Patch::new())
                    }
                    _ => Write::Unlink(item.clone(), self.key(), relation, self.key()),
                };
                let keyed = self.next(4) == 0;
                let idempotency_key =
                    keyed.then(|| IdempotencyKey::from(format!("i{}", self.next(3))));
                let at = matches!(write, Write::Upsert(..)) && self.next(5) == 0;
                let expected_version = at.then(|| Version(self.next(3)));
                let opts = WriteOpts {
                    expected_version,
                    idempotency_key,
                };
                (who, write, opts)
            })
            .collect()
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

/// What a lake answered and logged: the answers, written as the wire
/// writes them, and the log.
type Written = (Vec<String>, Vec<Event>);

pub fn wire(answer: &Result<Outcome>) -> String {
    match answer {
        Ok(o) => serde_json::to_string(o).expect("an outcome writes"),
        Err(e) => format!("{e:?}"),
    }
}

/// The writes of `seed` made in the lake `twin` of `place`, as `how`
/// queues them.
async fn written(place: &Place, lake: &str, twin: usize, seed: u64, how: How) -> Written {
    let (store, batches) = opened(place, lake, twin).await;
    let writes = Writes::new(seed).take(lake, 60);
    let ctx = WriteContext::default();
    let mut answers = Vec::new();
    match how {
        How::Alone => {
            for w in writes {
                answers.push(batches.alone(ctx, w).await);
            }
        }
        How::OfOne => {
            for w in writes {
                answers.extend(batches.write(ctx, vec![w]).await);
            }
        }
        How::OfAll => answers = batches.write(ctx, writes).await,
    }
    let written = (
        answers.iter().map(wire).collect(),
        log(store.as_ref()).await,
    );
    store.close().await.expect("the lake closes");
    written
}

#[derive(Clone, Copy)]
enum How {
    Alone,
    OfOne,
    OfAll,
}

/// Batches of one, and one batch of them all, answer and log what each
/// write alone does.
pub async fn a_batch_answers_and_logs_as_its_writes_alone(place: &Place, name: &str) {
    batch::fix_clock(Some(AT));
    for seed in 0..6 {
        let lake = format!("{name}-{seed}");
        let alone = written(place, &lake, 0, seed, How::Alone).await;
        let applied = alone.0.iter().filter(|a| a.contains("\"applied\"")).count();
        assert!(
            applied > 10,
            "seed {seed}: the writes mostly apply: {:?}",
            alone.0
        );
        for (twin, how) in [(1, How::OfOne), (2, How::OfAll)] {
            let batched = written(place, &lake, twin, seed, how).await;
            for (i, (a, b)) in alone.0.iter().zip(&batched.0).enumerate() {
                assert_eq!(a, b, "seed {seed}, twin {twin}: answer {i}");
            }
            assert_eq!(
                alone.1.len(),
                batched.1.len(),
                "seed {seed}, twin {twin}: the log's length"
            );
            for (a, b) in alone.1.iter().zip(&batched.1) {
                assert_eq!(a, b, "seed {seed}, twin {twin}: the log");
            }
        }
    }
    batch::fix_clock(None);
}

/// What an engine's appends are to do: fail, or panic, the next so many.
#[derive(Default)]
pub struct Faults {
    pub fail: AtomicUsize,
    pub panic: AtomicUsize,
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

/// The engine `inner` opens, its appends failing as `faults` say.
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
        self.0.tip()
    }
    fn append(&self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        if due(&self.1.panic) {
            panic_any("an append panicked (planted)");
        }
        if due(&self.1.fail) {
            let lake = "an append failed (planted)".to_owned();
            return Err(Error::unavailable(Situation::Busy { lake }));
        }
        self.0.append(lease, events)
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
        self.0.view(as_of)
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

pub fn upsert(lake: &str, key: &str, n: u64) -> (Principal, Write, WriteOpts) {
    let patch = serde_json::from_value(json!({"n": n})).expect("a patch");
    let write = Write::Upsert("item".into(), key.into(), patch);
    (owner(lake, "ann"), write, WriteOpts::default())
}

/// A write the lake refuses: an upsert of a record that exists at no
/// version yet, at version 7.
pub fn refused(lake: &str, key: &str) -> (Principal, Write, WriteOpts) {
    let (who, write, _) = upsert(lake, key, 1);
    let opts = WriteOpts {
        expected_version: Some(Version(7)),
        idempotency_key: None,
    };
    (who, write, opts)
}

/// A setting written, which runs alone and ends a batch: the kept view
/// by tag bounded.
pub fn setting(lake: &str) -> (Principal, Write, WriteOpts) {
    let n = WRITTEN.fetch_add(1, Ordering::SeqCst);
    let patch = json!({"key": "view_kept.by_tag", "value": n.is_multiple_of(2)});
    let patch = serde_json::from_value(patch).expect("a patch");
    let write = Write::Upsert("sys.setting".into(), "view_kept.by_tag".into(), patch);
    (owner(lake, "owner"), write, WriteOpts::default())
}

static WRITTEN: AtomicUsize = AtomicUsize::new(0);

pub fn situation(answer: &Result<Outcome>) -> Option<&Situation> {
    answer.as_ref().err().and_then(Error::situation)
}

/// An append that panics, then one that fails, answer every job of their
/// batch so, a refused job its refusal; a batch queued behind each still
/// runs, and the lake writes again, from where its log ended.
pub async fn a_failed_batch_answers_every_job_and_the_lake_writes_again(place: &Place, name: &str) {
    let (location, mut seams) = place(name, 0);
    let faults = Arc::new(Faults::default());
    let inner = seams.engines.remove(0);
    seams.engines = vec![Arc::new(Faulty(inner, faults.clone()))];
    let (store, batches) = batch::open(location, seams, OpenMode::Create)
        .await
        .expect("the lake opens");
    declared(store.as_ref(), name).await;
    let ctx = WriteContext::default();
    let head = store.head().await.expect("a head");
    for (fault, stopped) in [(&faults.panic, true), (&faults.fail, false)] {
        fault.store(1, Ordering::SeqCst);
        let (a, b) = (upsert(name, "a", 1), upsert(name, "b", 2));
        let behind = setting(name);
        let failed = batches
            .write(ctx, vec![a, refused(name, "r"), b, behind])
            .await;
        assert!(
            situation(&failed[1]).is_none(),
            "the refusal stays: {:?}",
            failed[1]
        );
        assert!(failed[1].is_err(), "{:?}", failed[1]);
        for answer in [&failed[0], &failed[2]] {
            let s = situation(answer);
            match stopped {
                true => assert!(matches!(s, Some(Situation::Stopped { .. })), "{answer:?}"),
                false => assert!(matches!(s, Some(Situation::Busy { .. })), "{answer:?}"),
            }
        }
        assert!(
            matches!(failed[3], Ok(Outcome::Applied { .. })),
            "{:?}",
            failed[3]
        );
        let after = batches.write(ctx, vec![upsert(name, "c", 3)]).await;
        assert!(
            matches!(after[0], Ok(Outcome::Applied { .. })),
            "{:?}",
            after[0]
        );
    }
    let again = batches.write(ctx, vec![upsert(name, "d", 4)]).await;
    let Ok(Outcome::Applied { position, .. }) = &again[0] else {
        panic!("the lake writes again: {:?}", again[0]);
    };
    let written: Vec<_> = log(store.as_ref())
        .await
        .into_iter()
        .filter(|e| e.position > head)
        .collect();
    assert_eq!(written.last().map(|e| e.position), Some(*position));
    let keys = |e: &Event| match &e.body {
        EventBody::RecordUpserted { key, .. } => Some(key.to_string()),
        _ => None,
    };
    let keys: Vec<_> = written.iter().filter_map(keys).collect();
    let upserts = ["view_kept.by_tag", "c", "view_kept.by_tag", "c", "d"];
    assert_eq!(
        keys, upserts,
        "only what was answered applied is in the log"
    );
    store.close().await.expect("the lake closes");
}

/// A batch whose fold fails, as `refuse` makes the next fold of the lake
/// at `place` fail, answers each job that appended so at its own first
/// position, a refused job its refusal, and a retry of a job before it in
/// the batch at the batch's first; the next write folds what landed.
pub async fn a_batch_whose_fold_fails_answers_each_job_at_its_own_position(
    place: &Place,
    name: &str,
    refuse: &dyn Fn(),
) {
    let (store, batches) = opened(place, name, 0).await;
    let keyed = |(who, write, _): (Principal, Write, WriteOpts)| {
        let idempotency_key = Some(IdempotencyKey::from("k1"));
        let opts = WriteOpts {
            expected_version: None,
            idempotency_key,
        };
        (who, write, opts)
    };
    let (a, b) = (keyed(upsert(name, "a", 1)), upsert(name, "b", 2));
    refuse();
    let writes = vec![a.clone(), refused(name, "r"), b, a];
    let failed = batches.write(WriteContext::default(), writes).await;
    let from = |answer: &Result<Outcome>| match situation(answer) {
        Some(Situation::NotFolded { from, .. }) => *from,
        _ => panic!("not folded: {answer:?}"),
    };
    assert!(
        failed[1].is_err() && situation(&failed[1]).is_none(),
        "{:?}",
        failed[1]
    );
    assert!(from(&failed[0]) < from(&failed[2]), "{failed:?}");
    assert_eq!(
        from(&failed[3]),
        from(&failed[0]),
        "a retry is told at the batch's first"
    );
    let after = batches
        .write(WriteContext::default(), vec![upsert(name, "c", 3)])
        .await;
    let Ok(Outcome::Applied { position, .. }) = &after[0] else {
        panic!("the lake writes again: {:?}", after[0]);
    };
    assert!(*position > from(&failed[2]));
    let keys = |e: &Event| match &e.body {
        EventBody::RecordUpserted { key, ty, .. } if ty.as_str() == "item" => Some(key.to_string()),
        _ => None,
    };
    let keys: Vec<_> = log(store.as_ref()).await.iter().filter_map(keys).collect();
    assert_eq!(keys, ["a", "b", "c"], "what landed stays");
    store.close().await.expect("the lake closes");
}
