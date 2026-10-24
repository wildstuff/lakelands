//! One open lake: its engine, its lease, the append pipeline and the fold.
//!
//! **Writing.** A write takes the lake's writer lock, decides the events
//! it appends (validation first, so nothing is appended that the fold
//! would refuse), seals them into the hash chain under the lease's fence,
//! appends them — durably, all or none — and folds them into the
//! projection in one transaction that commits with the position folded
//! through. One process holds a lake's lease at a time
//! (`docs/decisions/008-one-writer-per-lake.md`); a reader opens without
//! it, never appends and never folds: it reads what the writer folded.
//!
//! **Folding.** The fold is a pure function of the log: each event, in
//! order, changes the projection exactly as [`Lake::fold`] says, and
//! nothing else does. One kind of change comes later than its event: the
//! records a `declared` computes again are put by [`Lake::flush`], before
//! the next event that reads a record and before the commit, as of the
//! `declared`'s position. Opening a lake folds whatever the log holds beyond
//! the projection; `rebuild` clears the projection and folds everything
//! again, reaching the same state (`docs/decisions/001-the-log-is-the-truth.md`).
//!
//! **Time.** `known_at` is the writer's wall clock, never earlier than the
//! event before it, so it never decreases along the log (`docs/log-format.md`
//! §1.3).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::RangeInclusive;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use lakelands_api::engine::{CanonicalEvent, LakeEngine, Lease, OpenMode, ProjectionView};
use lakelands_api::error::{Detail, Flaw, Part, Seam, Situation};
use lakelands_api::event::{Digest, Event, EventBody, LOG_FORMAT_VERSION, SegmentDigest};
use lakelands_api::location::{EngineConfig, Limits, Location};
use lakelands_api::ontology::{FieldDecl, StorageKind, TypeDecl};
use lakelands_api::principal::Renamed;
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::{Count, EmbedBackend, Level, Meter, Metered, Reranker, Seams, Spread};
use lakelands_api::write::{
    Declaration, DeclarationKind, Migration, Proposal, ProposalFilter, ProposalState, ProposedWrite,
};
use lakelands_api::{
    Error, FieldName, LakeId, Position, Principal, ProposalId, RecordKey, Refusal, Result,
    SourceId, Timestamp, TypeName, Value, ValueTypeName, Version, WriteContext,
};
use lakelands_core::sight::{InForce, Sight};
use lakelands_core::{damaged, shape};

use crate::backed::{Due, Of, Outputs, Owed, Recorded};
use crate::declare::Held;
pub use crate::exec::Turn;
use crate::kept;
use crate::log::{self, Draft};
use crate::ontology::Ontology;
use crate::record;

pub use lakelands_api::seam::{AGING_RULE_TYPE, SETTING_TYPE, aging_rule_type, setting_type};
mod append;
mod fold;
mod open;
pub(crate) mod scrub;
mod verify;
pub use fold::Built;
use fold::{Beside, Migrated, Recomputed, Step, Swapped};
pub(crate) use fold::{
    computes, computes_again, computing_changed, fields_again, renamed_fields, using_types,
};
pub(crate) use open::opener;

/// What only the writer changes.
pub struct Writer {
    /// The lease, while this handle holds it.
    pub lease: Option<Lease>,
    /// The last event's position and hash.
    pub tip: Option<(Position, Digest)>,
    /// The last event a batch of record writes stamped after `tip` and
    /// has not appended yet: where its next write is decided from
    /// ([`crate::writes::write_many`]); cleared whenever the writer is
    /// taken ([`Turn::writer`]).
    pub staged: Option<(Position, Digest)>,
    /// The last event's `known_at`.
    pub last_known_at: Timestamp,
    /// The ontology at the head.
    pub ontology: Ontology,
    /// The settings at the head.
    pub settings: BTreeMap<String, Value>,
    /// What the appends left owed to the backings and
    /// [`crate::backed::settle`] has not recorded yet, in the order handed
    /// out. It is owed to the append that wrote its event, never to a fold
    /// of the log (022, extended).
    pub owed: Vec<Owed>,
    /// The answers the backings gave, by seam and digest, until they are
    /// recorded: a batch whose recording failed is recorded from them by
    /// the next, and its calls are not made again.
    pub held: BTreeMap<(String, Digest), Value>,
    /// A fold failed after its events were in the log: the projection is
    /// behind the log, and nothing is appended until it has caught up.
    pub behind: bool,
    /// The first position of an append whose fold failed: the fold that
    /// catches up hands out the calls of its events from there on, and
    /// forgets it once it has folded them.
    pub failed: Option<Position>,
    /// The executors' answers whose append failed `unavailable`, oldest
    /// first: the next write appends them before it is decided (013).
    pub answers: Vec<Draft>,
    /// The plans the fold, a step's walk and the write path compute
    /// records by.
    pub plans: record::Plans,
    /// The engine said this handle holds the lease no more: the session
    /// it lived in closed under it (lakelands#979), or the lease is stale,
    /// another writer's now or no one's (lakelands#1385). The next write,
    /// rebuild or compaction takes the lease again before it decides
    /// anything ([`Lake::regain`]); until its `lease_taken` is appended
    /// this stays.
    pub lost: bool,
    /// The sources the log forgot that a fold has met (058): a fact of
    /// the log, so kept over a rebuild.
    pub forgotten: BTreeSet<SourceId>,
    /// The acts of removal a fold met or a rebuild was told of (065),
    /// whose selected values the fold erases: a fact of the log, so kept
    /// over a rebuild.
    pub(crate) removals: Vec<crate::remove::Removal>,
    /// Those of them a fold learned by looking ahead, after it folded what
    /// they select: it leaves them owed when it meets them.
    pub(crate) ahead: BTreeSet<Position>,
    /// A rebuild is folding: an act it meets after its source's keys were
    /// destroyed has erased what they sealed.
    pub rebuilding: bool,
    /// While a fold of the log runs: the sets it decides itself, from the
    /// first value it erased that each reads (lakelands#1232).
    pub(crate) zone: Option<crate::writes::Zone>,
    /// The tip the active segment's size was last weighed at, for a seal
    /// by size ([`Turn::seal_by_size`]).
    pub(crate) sized: Option<Position>,
}

/// What the lake declares at one position, as a read needs it and before
/// any rule of who is shown what: nothing of a reader is in it. It is
/// built once and shared by every read at a position of its range.
pub struct Shape {
    /// The ontology.
    pub ontology: Ontology,
    /// Every declaration, as `Ontology::declared` lists them, each in
    /// force since the position the engine holds it from.
    pub declared: Vec<lakelands_api::engine::Declared>,
    /// The settings.
    pub settings: BTreeMap<String, Value>,
    /// The lake's list of renames.
    pub renames: Vec<Renamed>,
}

impl InForce for Shape {
    fn ty(&self, name: &TypeName) -> Option<&TypeDecl> {
        self.ontology.ty(name).ok()
    }

    fn types(&self) -> Vec<&TypeDecl> {
        self.ontology.types()
    }

    fn declared(&self) -> std::borrow::Cow<'_, [lakelands_api::engine::Declared]> {
        (&self.declared[..]).into()
    }
}

/// Whether folding `body` can change a [`Shape`]: it puts a declaration
/// (a type, a function, a value type, a set, a view, a verb, a trigger),
/// a rename, or a setting. A shape is carried over an append only when no
/// event of it ends a range. A new kind of event that changes what a read
/// is shown must end one here, or reads after it are answered with the
/// shape before it. A restriction ends none: a setting's value holds
/// restricted too, and what a reader may see is decided per read.
pub(crate) fn ends_a_range(body: &EventBody) -> bool {
    let setting = |ty: &TypeName| ty.as_str() == SETTING_TYPE;
    match body {
        EventBody::Declared { .. }
        | EventBody::Migrated { .. }
        | EventBody::ValueTypeMigrated { .. } => true,
        EventBody::RecordUpserted { ty, .. }
        | EventBody::RecordIngested { ty, .. }
        | EventBody::RecordRetired { ty, .. } => setting(ty),
        EventBody::OutputRecorded { field, .. } => field.as_ref().is_some_and(|f| setting(&f.0)),
        EventBody::LakeCreated { .. }
        | EventBody::LakeUpgraded { .. }
        | EventBody::LeaseTaken { .. }
        | EventBody::RecordRestricted { .. }
        | EventBody::RecordUnrestricted { .. }
        | EventBody::VerbCalled { .. }
        | EventBody::WriteProposed { .. }
        | EventBody::WriteAccepted { .. }
        | EventBody::WriteRejected { .. }
        | EventBody::TriggerFired { .. }
        | EventBody::TriggerDropped { .. }
        | EventBody::ObjectSetChanged { .. }
        | EventBody::SourceForgotten { .. }
        | EventBody::ValuesRemoved { .. }
        | EventBody::Checkpoint { .. } => false,
    }
}

/// The seam name the lake's renames are recorded under, as one output: the
/// store's own, which no seam records under (`docs/seams.md`).
pub(crate) const RENAMES: &str = "sys:renames";

/// The prefix of every seam name of the store's own, [`RENAMES`] and the
/// kept views' among them. A host's seams are named behind their kind's
/// prefix (`function:`, `embed:`, `verb:`), so no door records under one.
/// Every type name it begins is the store's own too, as no declared type
/// name holds a `:`.
pub(crate) const OWN_SEAM: &str = "sys:";

/// The lake's list of renames as its output holds it; empty where none is
/// recorded.
fn renames_in(recorded: Option<Value>) -> Result<Vec<Renamed>> {
    let list = recorded.map(serde_json::from_value).transpose();
    Ok(list
        .map_err(|e| {
            let (what, detail) = ("the lake's renames".to_owned(), Detail::json(&e));
            damaged(Part::Projection, None, Flaw::Unreadable { what, detail })
        })?
        .unwrap_or_default())
}

/// One open lake.
pub struct Lake {
    /// The lake's name.
    pub id: LakeId,
    /// The engine keeping it.
    pub engine: Arc<dyn LakeEngine>,
    /// What the host plugged in. Its `embed` is taken into `embed` at
    /// open, where a bind replaces it.
    pub seams: Seams,
    /// The embedding backend bound now: the one the open brought, or the
    /// last `bind_embed`. A reader clones it and holds no lock across a
    /// call.
    embed: Mutex<Option<Arc<dyn EmbedBackend>>>,
    /// The reranker bound now, as `embed` holds the embedding backend.
    rerank: Mutex<Option<Arc<dyn Reranker>>>,
    /// Bounds on one call's work.
    pub limits: Limits,
    /// Where the lake is kept: a rebuild opens its scratch there (071).
    pub(crate) location: Location,
    /// Held while a batch of the lake's owed calls is made: one batch at a
    /// time per lake.
    pub calls: tokio::sync::Mutex<()>,
    /// The causes writes wait for in [`crate::backed::settle`], each with
    /// how many wait: an entry of any other cause is an orphan.
    pub(crate) waiting: Mutex<BTreeMap<Position, usize>>,
    /// Per type and field, the `find` pages ordered first by a field the
    /// lake keeps no order index for, since it was opened here, and the
    /// microseconds they took (lakelands#1029).
    pub(crate) sorted_whole: Mutex<BTreeMap<(TypeName, FieldName), (u64, u64)>>,
    /// What the lake weighs of those pages to choose an order index
    /// itself, and what it chose (`crate::choose`).
    pub(crate) choosing: Mutex<crate::choose::Choosing>,
    /// The queue writes wait in for their turn at the writer, in the order
    /// they came ([`crate::exec::write`]).
    pub(crate) queue: Arc<Mutex<crate::exec::Queue>>,
    writer: Mutex<Writer>,
    /// The tip's position and whether the lease is held, as of the last
    /// change the writer made: read without the writer's lock, which an
    /// append holds across the engine's append and the whole fold.
    head: AtomicU64,
    /// The head again, for a wait on the next one (`ChangeFeed::wait`).
    heads: tokio::sync::watch::Sender<u64>,
    holds_lease: AtomicBool,
    /// The shapes reads were answered with or a declaring append left, at
    /// most two, each with the positions it holds for: no event among them
    /// ends a range ([`ends_a_range`]). One a declaring append left is
    /// built at the first read of its position. The lock is held to find
    /// or put one, never across a build or a read.
    shapes: Mutex<Vec<(RangeInclusive<u64>, HeldShape)>>,
    /// Moved when a rebuild begins and when it ends: a shape built across
    /// either read a projection cleared or half folded, and is not kept.
    rebuilds: AtomicU64,
    /// The events a second the last rebuild this handle ran folded, its
    /// own or its open's, 0 where none has run, and the milliseconds the
    /// last reclaim it ran took: the fold's cost is the ontology's, so a
    /// preflight reads these before the engine's constant
    /// ([`Self::expected_ms`], lakelands#1152).
    rebuilt_rate: AtomicU64,
    reclaim_ms: AtomicU64,
    /// The events a second the last scoped refold this handle ran folded,
    /// 0 where none has run ([`Self::scoped_ms`]).
    scoped_rate: AtomicU64,
    /// The projection the fold writes to, scoped while a forgetting
    /// refolds the records its sources wrote ([`scrub`]).
    scoped: scrub::Scoped,
    /// How the last forgetting this handle finished restated the
    /// projection.
    way: Mutex<Option<lakelands_api::write::ForgetWay>>,
    /// The position of the last act that erased values (058, 065) this
    /// handle found folded or folded itself, 0 where none, and its time:
    /// only a reaction before it, or of its own append, is decided again
    /// ([`crate::writes::overruled`]).
    acted: (AtomicU64, AtomicI64),
    /// The sources' keys the lake was given, and the key store (058).
    pub(crate) keys: Arc<crate::sealing::Keys>,
    /// Why a forgetting's reclaim is still owed, where the last one failed
    /// (lakelands#1161).
    reclaim_owed: Mutex<Option<String>>,
    /// Why the writer's open rebuilt the projection: an older build laid
    /// it out (lakelands#1151).
    rebuilt_at_open: Option<String>,
}

/// The milliseconds since `began`.
pub(crate) fn elapsed_ms(began: std::time::Instant) -> u64 {
    u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// A log's last event: its position and its hash.
type Tip = Option<(Position, Digest)>;

/// The position the projection is folded through and the log's tip, read
/// in that order: a writer appends an event before it folds it, so the
/// tip is never behind the projection in a lake that is whole, beside a
/// writer too. A projection folded past the tip stands for events the
/// log does not hold: the lake lost its log, or the projection is
/// another lake's, and nothing is answered from it or written behind it
/// (`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`).
pub(crate) fn ends(engine: &dyn LakeEngine) -> Result<(Option<Position>, Tip)> {
    let folded = engine.projection().folded_through()?;
    let tip = engine.tip()?;
    if let Some(position) = folded
        && tip.as_ref().is_none_or(|(at, _)| *at < position)
    {
        return Err(damaged(Part::Log, None, Flaw::EndsBefore { position }));
    }
    Ok((folded, tip))
}

thread_local! {
    /// The causes of what this thread's appends left owed since [`owing`]
    /// began.
    static OWING: RefCell<BTreeSet<Position>> = const { RefCell::new(BTreeSet::new()) };
}

/// Run a write's `work`, and answer with its result the causes of what
/// its appends left owed: the entries [`crate::backed::settle`] waits for.
/// So do those of an append whose fold failed, which the fold that
/// catches up before the write hands out.
pub(crate) fn owing<T>(work: impl FnOnce() -> Result<T>) -> Result<(T, BTreeSet<Position>)> {
    OWING.with(|o| o.borrow_mut().clear());
    let done = work()?;
    Ok((done, OWING.with(|o| std::mem::take(&mut *o.borrow_mut()))))
}

pub(crate) fn wall_clock() -> Timestamp {
    if let Some(at) = crate::batch::fixed() {
        return at;
    }
    let micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0);
    Timestamp(micros)
}

/// A scan over every record of a type, restricted ones included, and
/// retired ones when `retired`.
pub(crate) fn every(retired: bool) -> lakelands_api::engine::Scan {
    lakelands_api::engine::Scan {
        limit: u32::MAX,
        retired,
        restricted: true,
        ..Default::default()
    }
}

/// The settings a view holds, `ontology` being its own: the `value` of
/// every live `sys.setting` record, by key, a restricted one too. A
/// restriction hides the record from a reader; the value stays what the
/// lake computes with, as the fold keeps it, so a read, a writer opened
/// again and a rebuild agree.
pub fn settings_in(
    view: &dyn ProjectionView,
    ontology: &Ontology,
) -> Result<BTreeMap<String, Value>> {
    let ty = TypeName::from(SETTING_TYPE);
    if !ontology.types.contains_key(&ty) {
        return Ok(BTreeMap::new());
    }
    Ok(view
        .scan(&ty, &every(false))?
        .into_iter()
        .filter_map(|r| Some((r.key.to_string(), r.fields.get("value")?.clone())))
        .collect())
}

/// A shape the lake holds for a range: built, or left by an append whose
/// only events that end a range declare, and built at the first read of
/// its position (lakelands#1056).
#[derive(Clone)]
enum HeldShape {
    Built(Arc<Shape>),
    Left(Arc<Left>),
}

/// The appends a chain of left shapes holds at most. Past them an append
/// leaves nothing, and the next read builds from the engine. That bounds
/// what a chain keeps, and what its first read puts again: the
/// declarations of 1 024 appends (lakelands#1056).
const LEFT_LINKS: usize = 1024;

/// What makes the shape a declaring append leaves: the shape held for the
/// range just before it, whose settings and renames no declaration moves,
/// and the declarations the append made, each since its own position.
struct Left {
    /// None only while its chain is dropped.
    before: Option<HeldShape>,
    declared: Vec<lakelands_api::engine::Declared>,
    /// The links of its chain, itself included.
    links: usize,
}

impl Drop for Left {
    /// A chain is unlinked in a loop: dropped by each link's own drop in
    /// turn, it would take a frame of the stack a link.
    fn drop(&mut self) {
        let mut next = self.before.take();
        while let Some(HeldShape::Left(left)) = next {
            next = Arc::into_inner(left).and_then(|mut l| l.before.take());
        }
    }
}

impl Left {
    /// The shape: the one held before, each declaration put on it in the
    /// order the appends made them, as the writer's fold put them on its
    /// ontology; no declaration is read from the engine.
    fn build(&self) -> Shape {
        let (mut lefts, mut before) = (vec![self], self.before.as_ref());
        while let Some(HeldShape::Left(left)) = before {
            lefts.push(left);
            before = left.before.as_ref();
        }
        let Some(HeldShape::Built(base)) = before else {
            unreachable!("a chain of left shapes ends at a built one")
        };
        let mut ontology = base.ontology.clone();
        for d in lefts.iter().rev().flat_map(|left| &left.declared) {
            let (decl, hash) = (d.declaration.clone(), d.schema_hash.clone());
            ontology.put_at(decl, d.version, hash, d.since);
        }
        Shape {
            declared: ontology.declared(),
            settings: base.settings.clone(),
            renames: base.renames.clone(),
            ontology,
        }
    }
}

impl<'l> Turn<'l> {
    /// The writer state, locked: taken only on a turn, so a write that
    /// takes it has waited its turn in the queue first. A batch's staged
    /// tip lives only as long as the hold it was stamped under, so a hold
    /// a panic gave up leaves none behind.
    pub fn writer(&self) -> MutexGuard<'l, Writer> {
        let mut w = lakelands_core::lock(&self.lake().writer);
        w.staged = None;
        w
    }

    /// The writer for a write, locked and caught up. A writer whose fold
    /// failed is behind its log: it folds from the projection's mark before
    /// anything is decided over the projection, or the write is answered
    /// `unavailable`, with that fold's words, and appends nothing.
    /// Everything that decides a write takes the writer here;
    /// [`Lake::append_after`] refuses one that is behind. Only the lease's
    /// holder folds: a handle that gave its lease up while it was behind
    /// folds nothing and is refused here, as its append would be. A held
    /// answer goes in first; failing again, it stops no write, and one
    /// whose fold fails is caught up from as any other.
    pub fn for_write(&self) -> Result<MutexGuard<'l, Writer>> {
        let mut w = self.writer();
        self.regain(&mut w)?;
        self.caught_up(&mut w)?;
        let _ = self.record_held(&mut w, None);
        self.caught_up(&mut w)?;
        Ok(w)
    }

    /// Fold the log's events the projection does not reflect yet, and load
    /// the head's ontology and settings.
    pub(crate) fn catch_up(&self) -> Result<()> {
        self.catch_up_in(&mut self.writer())
    }
}

impl Lake {
    /// A writer that holds nothing, over `view`: what a write would decide
    /// there, for a preflight that reads and never takes the lease. It
    /// appends nothing; its drafts' positions follow the view's.
    pub(crate) fn reading(&self, view: &dyn ProjectionView) -> Result<Writer> {
        let shape = self.shape(view)?;
        let tip = view.position().map(|p| (p, Digest::zero()));
        Ok(Writer {
            lease: None,
            tip,
            staged: None,
            last_known_at: wall_clock(),
            ontology: shape.ontology.clone(),
            settings: shape.settings.clone(),
            owed: Vec::new(),
            held: BTreeMap::new(),
            behind: false,
            failed: None,
            answers: Vec::new(),
            plans: record::Plans::default(),
            lost: false,
            forgotten: BTreeSet::new(),
            removals: Vec::new(),
            ahead: BTreeSet::new(),
            rebuilding: false,
            zone: None,
            sized: None,
        })
    }

    /// The clock, where a meter is bound: what [`Self::since`] times from.
    pub(crate) fn timer(&self) -> Option<std::time::Instant> {
        self.seams.meter.as_ref().map(|_| std::time::Instant::now())
    }

    /// Tell the meter, if one is bound, as `tell` does: a meter that panics
    /// is dropped from this call alone, so no hook parts an append from its
    /// fold (079).
    fn meter(&self, tell: impl FnOnce(&dyn Meter, Metered<'_>)) {
        if let Some(m) = &self.seams.meter {
            let at = Metered {
                lake: &self.id,
                engine: self.engine.name(),
            };
            drop(catch_unwind(AssertUnwindSafe(|| tell(m.as_ref(), at))));
        }
    }

    /// The microseconds since `began`, observed as `n`.
    pub(crate) fn since(&self, n: Spread, began: Option<std::time::Instant>) {
        if let Some(t) = began {
            let us = u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX);
            self.meter(|m, at| m.observe(at, n, us));
        }
    }

    /// One value of `n`, observed.
    pub(crate) fn observe(&self, n: Spread, value: u64) {
        self.meter(|m, at| m.observe(at, n, value));
    }

    /// `n` counted up by `by`.
    pub(crate) fn count(&self, n: Count, by: u64) {
        self.meter(|m, at| m.count(at, n, by));
    }

    /// `n` told as it now stands.
    pub(crate) fn level(&self, n: Level, value: u64) {
        self.meter(|m, at| m.level(at, n, value));
    }

    /// Make the writer's tip and lease readable without its lock.
    fn publish(&self, w: &Writer) {
        self.set_head(w.tip.as_ref().map_or(0, |t| t.0.0));
        let holds = w.lease.is_some() && !w.lost;
        self.holds_lease.store(holds, Ordering::Release);
        self.level(Level::Tip, w.tip.as_ref().map_or(0, |t| t.0.0));
        self.level(Level::LeaseHeld, u64::from(holds));
        self.level(Level::LeaseFence, w.lease.as_ref().map_or(0, |l| l.fence));
    }

    /// A system principal of this lake, named `id`, with every grant: the
    /// store acting for itself, the host, or a proposer being applied.
    pub(crate) fn system(&self, id: impl Into<String>) -> lakelands_api::Principal {
        lakelands_api::Principal {
            kind: lakelands_api::PrincipalKind::System,
            id: id.into(),
            lake: self.id.clone(),
            grants: lakelands_api::Grants::owner(),
        }
    }

    /// The embedding backend bound now, if any.
    pub fn embed(&self) -> Option<Arc<dyn EmbedBackend>> {
        lakelands_core::lock(&self.embed).clone()
    }

    /// Bind `backend` in place of the one bound now. A batch holds the
    /// backend it read when it asked, so this reaches the next batch.
    pub fn bind_embed(&self, backend: Arc<dyn EmbedBackend>) {
        *lakelands_core::lock(&self.embed) = Some(backend);
    }

    /// The reranker bound now, if any.
    pub fn rerank(&self) -> Option<Arc<dyn Reranker>> {
        lakelands_core::lock(&self.rerank).clone()
    }

    /// Bind `backend` in place of the reranker bound now.
    pub fn bind_rerank(&self, backend: Arc<dyn Reranker>) {
        *lakelands_core::lock(&self.rerank) = Some(backend);
    }

    /// Marks the writer lost where the engine said its lease is not held
    /// (lakelands#979) or stale, another writer's now or no one's
    /// (lakelands#1385): the lease goes, and the handle is no writer until
    /// it takes one again.
    fn lose(&self, w: &mut Writer, e: &Error) {
        if matches!(
            e.situation(),
            Some(Situation::LeaseNotHeld { .. } | Situation::LeaseStale { .. })
        ) {
            (w.lost, w.lease) = (true, None);
            self.holds_lease.store(false, Ordering::Release);
            self.level(Level::LeaseHeld, 0);
        }
    }

    /// A lost writer takes the lease again before its next write, as an
    /// open does: a greater fence, what another writer appended meanwhile
    /// folded from the projection's mark, and a `lease_taken` in the log,
    /// the first event under the new fence (lakelands#979). It is no
    /// longer lost only once that is appended: a regain that took the
    /// lease and failed after it goes on with that lease. Where another
    /// holds the lease it is refused with the lease's situation, appends
    /// nothing, and stays lost.
    pub(crate) fn regain(&self, w: &mut Writer) -> Result<()> {
        if !w.lost {
            return Ok(());
        }
        let lease = match w.lease.clone() {
            Some(lease) => lease,
            None => {
                let holder = format!("lakelands process {}", std::process::id());
                let lease = self.engine.take_lease(&holder)?;
                w.lease = Some(lease.clone());
                lease
            }
        };
        // The fold from the mark catches up a writer that was behind too.
        w.behind = false;
        self.catch_up_in(w)?;
        let now = self.clock(w);
        let (fence, holder) = (lease.fence, lease.holder);
        self.append(
            w,
            vec![Draft::system(now, EventBody::LeaseTaken { fence, holder })],
        )?;
        w.lost = false;
        self.publish(w);
        Ok(())
    }

    /// Fold a writer that is behind from the projection's mark, or refuse.
    fn caught_up(&self, w: &mut Writer) -> Result<()> {
        if !w.behind {
            return Ok(());
        }
        if w.lease.is_none() {
            return Err(no_lease());
        }
        self.refold(w).map_err(|e| {
            let cause = Some(Box::new(e));
            Error::unavailable(Situation::Behind { cause })
        })
    }

    /// Append the answers held on the writer, the oldest first, then `d`,
    /// and answer `d`'s (013). A draft leaves `answers` once the engine holds
    /// it, before its fold; one refused `unavailable` before that is held,
    /// any other refusal drops it. Only `d`'s refusal is answered.
    pub(crate) fn record_held(&self, w: &mut Writer, d: Option<Draft>) -> Result<Vec<Event>> {
        let mut answer = Ok(Vec::new());
        for mut d in std::mem::take(&mut w.answers).into_iter().chain(d) {
            d.known_at = self.clock(w);
            let stamped = self.stamp(w, vec![d.clone()]);
            let held = stamped.and_then(|(lease, events, stored)| {
                self.engine.append(&lease, &stored).map(|()| events)
            });
            if let Err(e) = &held {
                self.lose(w, e);
            }
            answer = match held {
                Ok(events) => self.landed(w, events, Reads::default()),
                Err(e) => {
                    w.answers
                        .extend(matches!(e, Error::Unavailable { .. }).then_some(d));
                    Err(e)
                }
            };
        }
        answer
    }

    /// The next `known_at`: the wall clock, never before the last event's.
    pub fn clock(&self, w: &mut Writer) -> Timestamp {
        let now = wall_clock().max(w.last_known_at);
        w.last_known_at = now;
        now
    }

    /// [`Self::catch_up`] on the writer held.
    fn catch_up_in(&self, w: &mut Writer) -> Result<()> {
        let folded;
        (folded, w.tip) = ends(self.engine.as_ref())?;
        {
            let view = self.engine.view(None)?;
            w.ontology = Ontology::at(view.as_ref())?.named();
            w.settings = settings_in(view.as_ref(), &w.ontology)?;
        }
        if self.acts_known(w)? {
            let last = last_act(self.engine.view(None)?.as_ref())?;
            let at = self.read_events(last, 1)?.first().map(|e| e.known_at);
            self.act_at(last, at.unwrap_or(Timestamp(i64::MIN)));
        }
        // The last event's time.
        if let Some((pos, _)) = &w.tip
            && let Some(e) = self.read_events(*pos, 1)?.first()
        {
            w.last_known_at = e.known_at;
        }
        // Only the writer folds; a handle opened read-only reads what the
        // writer folded, and writes nothing.
        if w.lease.is_some() {
            let from = folded.map_or(Position::GENESIS, Position::next);
            self.fold_from(w, from)?;
            self.publish(w);
        } else {
            // A read-only handle reads what the writer folded: its head is
            // the projection's position, which it can be read at.
            self.set_head(folded.map_or(0, |p| p.0));
            self.holds_lease.store(false, Ordering::Release);
        }
        Ok(())
    }

    /// The shape at `view`'s position: the one known for a range that
    /// holds it, built there first if a declaring append left it, or one
    /// built from the view, kept in place of the known one that ends
    /// first. A shape built from the view is kept only if no rebuild began
    /// or ended while it was read and the projection is folded through its
    /// position: one read from a projection cleared or half folded again
    /// answers its read and nothing after it. A left one reads no
    /// projection, and is kept only in its own place.
    pub(crate) fn shape(&self, view: &dyn ProjectionView) -> Result<Arc<Shape>> {
        let at = view.position().map_or(0, |p| p.0);
        let held = lakelands_core::lock(&self.shapes);
        let found = held.iter().find(|(range, _)| range.contains(&at));
        let left = match found.map(|(_, shape)| shape.clone()) {
            Some(HeldShape::Built(shape)) => return Ok(shape),
            Some(HeldShape::Left(left)) => Some(left),
            None => None,
        };
        drop(held);
        // A left shape is built once, and put in its place unless it was
        // dropped meanwhile.
        if let Some(left) = left {
            let shape = Arc::new(left.build());
            let mut held = lakelands_core::lock(&self.shapes);
            for (_, kept) in held.iter_mut() {
                if matches!(kept, HeldShape::Left(l) if Arc::ptr_eq(l, &left)) {
                    *kept = HeldShape::Built(shape.clone());
                }
            }
            return Ok(shape);
        }
        let rebuilds = self.rebuilds.load(Ordering::SeqCst);
        let ontology = Ontology::from_declared(&view.declarations()?);
        let shape = Arc::new(Shape {
            declared: ontology.declared(),
            settings: settings_in(view, &ontology)?,
            renames: self.renames(Some(view))?,
            ontology,
        });
        let folded = self.engine.projection().folded_through()?;
        // Read under the lock: a rebuild moves the count before it takes
        // the lock to drop the shapes, so a shape put here is either seen
        // to be stale or dropped after.
        let mut held = lakelands_core::lock(&self.shapes);
        if self.rebuilds.load(Ordering::SeqCst) != rebuilds || folded.is_none_or(|f| f.0 < at) {
            return Ok(shape);
        }
        held.sort_by_key(|(range, _)| *range.end());
        if held.len() > 1 {
            held.remove(0);
        }
        held.push((at..=at, HeldShape::Built(shape.clone())));
        Ok(shape)
    }

    /// The lake's list of renames: what the fold recorded, read from
    /// `head` or from a view of the head. No list is no rename: the fold
    /// records it at every migration that renames, and every projection a
    /// store reads was made by such a fold (006, extended).
    pub(crate) fn renames(&self, head: Option<&dyn ProjectionView>) -> Result<Vec<Renamed>> {
        let recorded = match head {
            Some(view) => view.output(RENAMES, &Digest::zero())?,
            None => self.engine.view(None)?.output(RENAMES, &Digest::zero())?,
        }
        .map(|(_, list)| list);
        renames_in(recorded)
    }

    /// The sight of `who` over `force` (`Sight::new`), for a read at `at`
    /// or a write at the head, its grants held through the lake's renames,
    /// and how many entries of the list they were followed through:
    /// `head`'s, the list of a read's shape at the head, or the list read.
    /// The list is asked only for one whose grants name a type: a grant
    /// that is only a default follows a link type's rename too.
    pub(crate) fn sight<'o>(
        &self,
        who: Principal,
        at: Option<Position>,
        head: Option<&[Renamed]>,
        force: &'o dyn InForce,
    ) -> Result<(Sight<'o>, Option<usize>)> {
        if who.grants.types.is_empty() {
            return Ok((Sight::new(who, &[], at, force), None));
        }
        let read = head.is_none().then(|| self.renames(None)).transpose()?;
        let list = head.or(read.as_deref()).unwrap_or_default();
        Ok((Sight::new(who, list, at, force), Some(list.len())))
    }

    /// The log from a position, a page at a time, opened and checked: a
    /// caller holds one page, never the range (`Pages`).
    pub fn pages(&self, from: Position) -> Pages<'_> {
        Pages {
            lake: self,
            at: Some(from),
        }
    }

    /// Events from a position, opened and checked, at most `limit`. A read
    /// of a range goes through `pages`.
    pub fn read_events(&self, from: Position, limit: u32) -> Result<Vec<Event>> {
        let mut out = Vec::new();
        let mut at = from;
        // The format of the event before the first one read: a fall is
        // judged at the first event of a range too. A single lookup (a
        // search's probe, a record's version) reads no event more.
        let mut before = match from.0.checked_sub(1).filter(|_| limit > 1) {
            Some(p) => self
                .engine
                .read(Position(p), 1)?
                .first()
                .and_then(log::written_format)
                .filter(|v| lakelands_core::log::reads_format(*v)),
            None => None,
        };
        loop {
            let want = (limit as usize - out.len()).min(PAGE as usize) as u32;
            if want == 0 {
                break;
            }
            let batch = self.engine.read(at, want)?;
            if batch.is_empty() {
                break;
            }
            for stored in &batch {
                let e = log::open(stored)?;
                // A lake's format never falls: refused where it is read,
                // as an event of another format is (054).
                if let Some(b) = before.filter(|b| u64::from(e.v) < *b) {
                    let flaw = Flaw::FormatStep {
                        v: e.v.into(),
                        before: b,
                    };
                    return Err(damaged(Part::Log, Some(e.position), flaw));
                }
                before = Some(e.v.into());
                out.push(e);
            }
            at = Position(batch.last().expect("not empty").position.0 + 1);
            if out.len() >= limit as usize {
                break;
            }
        }
        Ok(out)
    }

    /// The events at `positions`, in order, opened and checked. A gap of
    /// at most [`READ_THROUGH`] positions between two of them is read
    /// through rather than sought again, and the events in it are dropped
    /// unopened: nothing is answered from them.
    pub fn read_these(&self, positions: &[Position]) -> Result<Vec<Event>> {
        let mut out = Vec::with_capacity(positions.len());
        let mut rest = positions;
        while let Some(&first) = rest.first() {
            let run = 1 + rest
                .windows(2)
                .take_while(|w| w[1].0 - w[0].0 <= READ_THROUGH)
                .count();
            let (these, after) = rest.split_at(run);
            let end = these[these.len() - 1];
            let mut at = first;
            let mut wanted = these.iter().peekable();
            while at <= end {
                let span = (end.0 - at.0 + 1).min(u64::from(PAGE)) as u32;
                let batch = self.engine.read(at, span)?;
                let Some(last) = batch.last() else { break };
                at = last.position.next();
                for stored in &batch {
                    if wanted.next_if(|p| **p == stored.position).is_some() {
                        out.push(log::open(stored)?);
                    }
                }
            }
            rest = after;
        }
        Ok(out)
    }

    /// Refuse a context deeper than the ceiling of the trigger whose firing
    /// its cause names, or whose firing was dropped there (lakelands#1216);
    /// any other cause binds nothing more.
    fn check_trigger_ceiling(&self, w: &Writer, context: WriteContext) -> Result<()> {
        let Some(cause) = context.cause else {
            return Ok(());
        };
        let Some(stored) = self.engine.read(cause, 1)?.into_iter().next() else {
            return Ok(());
        };
        if let EventBody::TriggerFired { trigger, .. } | EventBody::TriggerDropped { trigger, .. } =
            log::open(&stored)?.body
            && let Some(m) = w
                .ontology
                .triggers
                .get(trigger.as_str())
                .and_then(|e| e.decl.max_depth)
            && context.depth > m
            && !self.withdrawn_since(trigger.as_str(), cause)?
        {
            return Err(Error::refused(
                Some(trigger.as_str().into()),
                Refusal::TooDeep { max: m },
            ));
        }
        Ok(())
    }

    /// Whether the trigger `name` was withdrawn after `at`: a firing before
    /// that is one of the trigger it withdrew, which the one in force does
    /// not bind (lakelands#1219). Read only where a ceiling would refuse,
    /// from the states the engine kept; one that keeps none binds it.
    fn withdrawn_since(&self, name: &str, at: Position) -> Result<bool> {
        let view = self.engine.view(None)?;
        let Some(head) = view.position() else {
            return Ok(false);
        };
        let key = format!("trigger {name}");
        let replaced = view.replaced(at, head)?.unwrap_or_default();
        Ok(replaced.iter().any(|d| {
            matches!(d.declaration, Declaration::Withdraw(_))
                && lakelands_core::engines::declaration_key(&d.declaration) == key
        }))
    }

    /// Keep the rate of a rebuild that began at `began` and folded the
    /// log through `events`.
    fn measured(&self, began: std::time::Instant, events: u64) {
        let micros = u64::try_from(began.elapsed().as_micros()).unwrap_or(u64::MAX);
        if events > 0 {
            let rate = events.saturating_mul(1_000_000) / micros.max(1);
            self.rebuilt_rate.store(rate.max(1), Ordering::SeqCst);
        }
    }

    /// How long an act that rebuilds a log of `events` and reclaims is
    /// expected to hold the writer's lock, in milliseconds: the events at
    /// the rate of the last rebuild this handle ran, measured over its
    /// clear and fold, or before any at the engine's constant, and the
    /// time of the last reclaim it ran, where it ran one. An estimate
    /// either way.
    pub(crate) fn expected_ms(&self, events: u64) -> u64 {
        let rate = match self.rebuilt_rate.load(Ordering::SeqCst) {
            0 => self.engine.rebuild_rate().max(1),
            rate => rate,
        };
        let folded = events.saturating_mul(1000) / rate;
        folded.saturating_add(self.reclaim_ms.load(Ordering::SeqCst))
    }

    /// How long a scoped refold of `events` and the reclaim after it are
    /// expected to hold the writer's lock, in milliseconds: at the rate
    /// of the last scoped refold this handle ran, or before any at a
    /// rebuild's, as [`Self::expected_ms`] reckons that.
    pub(crate) fn scoped_ms(&self, events: u64) -> u64 {
        match self.scoped_rate.load(Ordering::SeqCst) {
            0 => self.expected_ms(events),
            rate => (events.saturating_mul(1000) / rate)
                .saturating_add(self.reclaim_ms.load(Ordering::SeqCst)),
        }
    }

    /// The projection the fold writes to.
    fn store(&self) -> &dyn lakelands_api::engine::ProjectionStore {
        &self.scoped
    }

    /// How the last forgetting this handle finished restated the
    /// projection, where one did since this was last asked.
    pub(crate) fn forget_way(&self) -> Option<lakelands_api::write::ForgetWay> {
        lakelands_core::lock(&self.way).take()
    }

    /// Finish what a forgetting (058) or a removal (065) left owed, as the
    /// act itself does and the writer's open after a crash between its
    /// steps: each source whose act the projection may still hold the
    /// values of has its keys destroyed, the projection is rebuilt, and the
    /// storage reclaimed ([`Self::reclaim`]). Each step is idempotent.
    /// `None` where nothing was owed, and where the reclaim failed: the
    /// values are erased already, so a failed reclaim is owed and told,
    /// never an error (lakelands#1179).
    pub(crate) fn finish_forgetting(
        &self,
        w: &mut Writer,
    ) -> Result<Option<lakelands_api::write::CompactReport>> {
        let (list, acts) = self.acts_listed()?;
        if list.values().chain(acts.values()).all(|s| s == "done") {
            return Ok(None);
        }
        if list.values().chain(acts.values()).any(|s| s == "owed") {
            for (source, _) in list.iter().filter(|(_, s)| *s == "owed") {
                self.keys.destroy(source)?;
            }
            let view = self.engine.view(None)?;
            let owed = |r: &crate::remove::Removal| acts.get(&r.at).is_some_and(|s| s == "owed");
            for act in crate::remove::acts(self, view.as_ref())?
                .iter()
                .filter(|r| owed(r))
            {
                self.keys.destroy_versions(&act.source, &act.keys)?;
                self.keys.destroy_apart(&act.source, &act.apart)?;
            }
            // The records the owed sources wrote are refolded alone where
            // nothing else reads them, and the projection rebuilt
            // otherwise: either restates it as the log now reads.
            let owed: Vec<SourceId> = (list.iter())
                .filter(|(_, s)| *s == "owed")
                .map(|(source, _)| source.clone())
                .collect();
            let plan = scrub::plan(self, view.as_ref(), &owed, self.limits.forget_rebuild)?;
            let events = view.position().map_or(0, |p| p.0);
            let way = plan.way(self, events);
            drop(view);
            match way.scoped {
                true => self.scrub_held(w, &plan, &owed)?,
                false => self.rebuild_held(w)?,
            }
            *lakelands_core::lock(&self.way) = Some(way);
        }
        Ok(self.reclaim()?.ok())
    }

    /// The forgotten sources and the acts of removal the projection
    /// lists, each with its state.
    fn acts_listed(&self) -> Result<(BTreeMap<SourceId, String>, BTreeMap<Position, String>)> {
        let view = self.engine.view(None)?;
        let list = forgotten_list(view.as_ref(), &Staged::default())?;
        let acts = crate::remove::removed_list(view.as_ref(), &Staged::default())?;
        Ok((list, acts))
    }

    /// Give back the storage the projection no longer uses, under the
    /// writer's lock its caller holds: the last step of an act and a
    /// manual compact alike. The engine's answer is the inner result. One
    /// that fails while an act waits on it is owed, its cause kept for
    /// [`Self::reclaim_owed`]; one that runs owes nothing, and each act
    /// whose rebuild waited on it is done. The outer error is the lists'
    /// own read or write failing.
    fn reclaim(&self) -> Result<Result<lakelands_api::write::CompactReport>> {
        let (mut list, mut acts) = self.acts_listed()?;
        let waiting = list.values().chain(acts.values()).any(|s| s == "rebuilt");
        let began = std::time::Instant::now();
        let reclaimed = self.engine.projection().compact();
        if reclaimed.is_ok() {
            self.reclaim_ms.store(elapsed_ms(began), Ordering::SeqCst);
        }
        // A compact that fails with no act waiting owes nothing.
        if waiting || reclaimed.is_ok() {
            *lakelands_core::lock(&self.reclaim_owed) = reclaimed.as_ref().err().map(Error::said);
        }
        let reclaimed = match reclaimed {
            Ok(r) => r,
            Err(e) => return Ok(Err(e)),
        };
        let head = self.head();
        if waiting {
            for state in list.values_mut().chain(acts.values_mut()) {
                if state == "rebuilt" {
                    *state = "done".to_owned();
                }
            }
            self.put_lists(&list, &acts, head)?;
        }
        Ok(Ok(lakelands_api::write::CompactReport {
            before: reclaimed.before,
            after: reclaimed.after,
            head,
        }))
    }

    /// Put the forgotten sources and the acts of removal, each with its
    /// state, at `head` in a transaction of their own: a list that is
    /// empty is left as it is.
    pub(crate) fn put_lists(
        &self,
        list: &BTreeMap<SourceId, String>,
        acts: &BTreeMap<Position, String>,
        head: Position,
    ) -> Result<()> {
        let projection = self.engine.projection();
        let at = crate::sealing::forgotten_at();
        projection.begin()?;
        let written = (|| {
            if !list.is_empty() {
                let list = serde_json::to_value(list).expect("a list serialises");
                projection.put_output(crate::sealing::FORGOTTEN, &at, &list, head)?;
            }
            if !acts.is_empty() {
                let (at, acts) = (crate::remove::removed_at(), crate::remove::list_value(acts));
                projection.put_output(crate::remove::REMOVED, &at, &acts, head)?;
            }
            projection.commit(head)
        })();
        if written.is_err() {
            projection.rollback();
        }
        written
    }

    /// Why a forgetting's or a removal's reclaim is still owed, where the
    /// last one tried failed: the cause alone. The act answered, and the
    /// writer's open goes on without it (lakelands#1161); the next open, a
    /// repeat of the act or a compact that runs clears it.
    pub fn reclaim_owed(&self) -> Option<String> {
        lakelands_core::lock(&self.reclaim_owed).clone()
    }

    /// Why the writer's open rebuilt the projection, where it did.
    pub fn rebuilt_at_open(&self) -> Option<String> {
        self.rebuilt_at_open.clone()
    }

    /// The last position folded into the projection, as this handle last
    /// folded it or, opened only to read, found it folded: a position a
    /// read can be answered at.
    pub fn head(&self) -> Position {
        Position(self.head.load(Ordering::Acquire))
    }

    /// Move the head, and wake whoever waits on it.
    fn set_head(&self, at: u64) {
        self.head.store(at, Ordering::Release);
        self.heads.send_replace(at);
    }

    /// The head once it is past `after`, or when `within` has passed: what
    /// a feed's poll reads up to, so a wait wakes when a poll would answer
    /// more, and never before.
    pub async fn wait_past(&self, after: Position, within: std::time::Duration) -> Position {
        let mut heads = self.heads.subscribe();
        let past = heads.wait_for(|h| *h > after.0);
        let _ = tokio::time::timeout(within, past).await;
        self.head()
    }

    /// The position and the time of the last act that erased values, as
    /// this handle knows it; none where it knows of none.
    pub(crate) fn acted(&self) -> Option<(Position, Timestamp)> {
        let at = Position(self.acted.0.load(Ordering::Acquire));
        (at.0 > 0).then(|| (at, Timestamp(self.acted.1.load(Ordering::Acquire))))
    }

    /// An act at `at`, at the time `known`, unless a later one is known.
    fn act_at(&self, at: Position, known: Timestamp) {
        if self.acted.0.load(Ordering::Acquire) < at.0 {
            self.acted.1.store(known.0, Ordering::Release);
            self.acted.0.store(at.0, Ordering::Release);
        }
    }

    /// Whether this handle holds the lease.
    pub fn is_writer(&self) -> bool {
        self.holds_lease.load(Ordering::Acquire)
    }
}

/// A lake whose last handle is dropped gives its lease up, as `close`
/// would, so the lake can be opened for writing again.
impl Drop for Lake {
    fn drop(&mut self) {
        let w = self
            .writer
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(lease) = w.lease.take() {
            let _ = self.engine.release_lease(&lease);
        }
    }
}

/// A proposal by id, from a view.
pub fn find_proposal(view: &dyn ProjectionView, id: &ProposalId) -> Result<Proposal> {
    view.proposals(&ProposalFilter {
        limit: Some(u32::MAX),
        ..Default::default()
    })?
    .into_iter()
    .find(|p| &p.id == id)
    .ok_or_else(|| Error::NotFound {
        missing: lakelands_api::Missing::Proposal { id: id.clone() },
    })
}

/// How many events a page of the log holds at most.
pub(crate) const PAGE: u32 = 1024;

/// The widest gap between two positions [`Lake::read_these`] reads
/// through: reading 64 events from the engine costs about what seeking one
/// does (lakelands#678).
const READ_THROUGH: u64 = 64;

/// What a write is answered on a handle that does not hold the lease.
fn no_lease() -> Error {
    let (lake, leased_elsewhere) = (None, true);
    Error::unavailable(Situation::ReadOnly {
        lake,
        leased_elsewhere,
    })
}

/// What a seal or a compaction is answered on a handle opened read-only.
pub(crate) fn read_only() -> Error {
    let (lake, leased_elsewhere) = (None, false);
    Error::unavailable(Situation::ReadOnly {
        lake,
        leased_elsewhere,
    })
}

/// The log read a page at a time (`Lake::pages`): each item is up to
/// `PAGE` events, or more to finish a set of declarations, ending with it
/// and read at most a chunk beyond it; opened and checked; the iteration
/// ends after the tip, or after the first error.
pub struct Pages<'a> {
    lake: &'a Lake,
    at: Option<Position>,
}

impl Iterator for Pages<'_> {
    type Item = Result<Vec<Event>>;

    fn next(&mut self) -> Option<Result<Vec<Event>>> {
        let at = self.at.take()?;
        let mut page = match self.lake.read_events(at, PAGE) {
            Ok(page) => page,
            Err(e) => return Some(Err(e)),
        };
        // A page never ends inside a set of declarations: its commit would
        // be a head where half the set is in force. It reads on only to the
        // set's end, its last `declared` and the memberships after it, and
        // is cut there: the chunk that closed the set may begin the next,
        // which goes to the next page.
        let (open, mut end, mut closed) = (open_set(&page), page.len(), false);
        while let Some(last) = page.last().filter(|_| open && end == page.len()) {
            match self.lake.read_events(last.position.next(), PAGE) {
                Ok(more) if more.is_empty() => break,
                Ok(more) => page.extend(more),
                Err(e) => return Some(Err(e)),
            }
            for e in &page[end..] {
                if closed && !matches!(e.body, EventBody::ObjectSetChanged { .. }) {
                    break;
                }
                closed |= set_mark(e) == Some(false);
                end += 1;
            }
        }
        page.truncate(end);
        self.at = Some(page.last()?.position.next());
        Some(Ok(page))
    }
}

/// Records read from the projection, each once: a record's state, or its
/// absence, by the view's position, its type and its key; and the state
/// the fold kept of a view, by the position and the view. A view at one
/// position always answers the same, so an answer kept is an answer read
/// again. The writer lock is held from the decision over them to the
/// fold, so the fold's view is at the same position as the decision's
/// unless something else was folded in between; then nothing kept is
/// used, since the position is part of the key.
#[derive(Default)]
pub struct Reads(
    RefCell<BTreeMap<ReadAt, Option<StoredRecord>>>,
    RefCell<BTreeMap<StateAt, Option<crate::kept::State>>>,
);

/// A read of one record: the view's position, the type, the key.
type ReadAt = (Option<Position>, TypeName, RecordKey);

/// A read of a kept view's state: the view's position, the view.
type StateAt = (Option<Position>, TypeName);

impl Reads {
    /// Take in what `other` read.
    pub(crate) fn extend(&self, other: Reads) {
        self.0.borrow_mut().extend(other.0.into_inner());
        self.1.borrow_mut().extend(other.1.into_inner());
    }

    /// The state the fold kept of the view `ty` as `view` holds it, read
    /// from the view the first time.
    fn state(
        &self,
        view: &dyn ProjectionView,
        ty: &TypeName,
    ) -> Result<Option<crate::kept::State>> {
        let at = (view.position(), ty.clone());
        if let Some(s) = self.1.borrow().get(&at) {
            return Ok(s.clone());
        }
        let s = crate::kept::state(view, ty)?;
        self.1.borrow_mut().insert(at, s.clone());
        Ok(s)
    }

    /// A record as `view` holds it, read from the view the first time.
    pub fn get(
        &self,
        view: &dyn ProjectionView,
        ty: &TypeName,
        key: &RecordKey,
    ) -> Result<Option<StoredRecord>> {
        let at = (view.position(), ty.clone(), key.clone());
        if let Some(r) = self.0.borrow().get(&at) {
            return Ok(r.clone());
        }
        let r = view.get(ty, key)?;
        self.0.borrow_mut().insert(at, r.clone());
        Ok(r)
    }

    /// Reads together the records of `ty` at `keys` not read yet, so that
    /// [`Self::get`] answers each of them as it would have read it alone.
    pub fn prime(
        &self,
        view: &dyn ProjectionView,
        ty: &TypeName,
        keys: &[RecordKey],
    ) -> Result<()> {
        let at = |key: &RecordKey| (view.position(), ty.clone(), key.clone());
        let held = self.0.borrow();
        let unread: BTreeSet<RecordKey> = keys
            .iter()
            .filter(|k| !held.contains_key(&at(k)))
            .cloned()
            .collect();
        drop(held);
        let unread: Vec<RecordKey> = unread.into_iter().collect();
        let mut found: BTreeMap<RecordKey, StoredRecord> = BTreeMap::new();
        for page in unread.chunks(1_000) {
            found.extend(
                view.get_many(ty, page)?
                    .into_iter()
                    .map(|r| (r.key.clone(), r)),
            );
        }
        let mut held = self.0.borrow_mut();
        for key in unread {
            let r = found.remove(&key);
            held.insert(at(&key), r);
        }
        Ok(())
    }
}

/// The records of `ty` a view holds, a page at a time by key, restricted
/// ones included and retired ones when `retired`; an ended key is passed
/// over. Whether a page is the last, and where the next continues, are
/// decided on the rows as the view returned them. A page that cannot be
/// read ends the walk: it yields nothing more, and whoever drove it asks
/// [`Walk::failed`] before it uses anything the walk produced.
pub(crate) struct Walk<'v> {
    view: &'v dyn ProjectionView,
    ty: &'v TypeName,
    scan: lakelands_api::engine::Scan,
    page: std::vec::IntoIter<StoredRecord>,
    more: bool,
    failed: Option<Error>,
}

impl<'v> Walk<'v> {
    pub(crate) fn new(view: &'v dyn ProjectionView, ty: &'v TypeName, retired: bool) -> Self {
        let scan = lakelands_api::engine::Scan {
            limit: PAGE,
            ..every(retired)
        };
        Walk {
            view,
            ty,
            scan,
            page: Vec::new().into_iter(),
            more: true,
            failed: None,
        }
    }

    /// The error of the page read that ended the walk, if one did.
    pub(crate) fn failed(&mut self) -> Result<()> {
        self.failed.take().map_or(Ok(()), Err)
    }
}

impl Iterator for Walk<'_> {
    type Item = StoredRecord;

    fn next(&mut self) -> Option<StoredRecord> {
        loop {
            match self.page.next() {
                Some(r) if r.is_ended() => continue,
                Some(r) => return Some(r),
                None if !self.more => return None,
                None => {}
            }
            let rows = match self.view.scan(self.ty, &self.scan) {
                Ok(rows) => rows,
                Err(e) => {
                    (self.more, self.failed) = (false, Some(e));
                    return None;
                }
            };
            self.more = rows.len() == PAGE as usize;
            self.scan.after = rows.last().map(|r| lakelands_api::engine::After {
                values: Vec::new(),
                key: r.key.clone(),
            });
            self.page = rows.into_iter();
        }
    }
}

/// [`Walk`], each record handed to `each`: the first error, of a page read
/// or of `each`, ends it.
pub(crate) fn walk(
    view: &dyn ProjectionView,
    ty: &TypeName,
    retired: bool,
    mut each: impl FnMut(StoredRecord) -> Result<()>,
) -> Result<()> {
    let mut walk = Walk::new(view, ty, retired);
    walk.by_ref().try_for_each(&mut each)?;
    walk.failed()
}

/// The output an `output_recorded` names by `output_at`: the call's
/// latest, recorded earlier in the fold or in the view. A name of any
/// other position is a damaged log, never folded past
/// (`docs/decisions/053-a-recorded-output-is-recorded-once.md`).
fn named(
    view: &dyn ProjectionView,
    staged: &Recorded,
    seam: &str,
    input: &Digest,
    at: Position,
    e: &Event,
) -> Result<Value> {
    let latest = match staged.get(&(seam.to_owned(), input.clone())) {
        Some(all) => all.last().cloned(),
        None => view.output(seam, input)?,
    };
    match latest {
        Some((p, output)) if p == at => Ok(output),
        _ => Err(damaged(
            Part::Log,
            Some(e.position),
            Flaw::NotLatestOutput { at },
        )),
    }
}

/// What an event names that only the store's fold makes, said as the
/// refusal says it: a type of the kept form, an output under a seam of
/// the store's own, or a record of a view. `None` for every event a door
/// appends.
fn foreign(o: &Ontology, body: &EventBody) -> Option<Flaw> {
    let (ty, declares) = match body {
        EventBody::OutputRecorded { seam, .. } => {
            let seam = seam.clone();
            return (seam.starts_with(OWN_SEAM)).then_some(Flaw::OwnSeam { seam });
        }
        EventBody::Declared {
            declaration: Declaration::Type(TypeDecl { name, .. }),
            ..
        } => (name, true),
        EventBody::RecordUpserted { ty, .. }
        | EventBody::RecordIngested { ty, .. }
        | EventBody::RecordRetired { ty, .. }
        | EventBody::RecordRestricted { ty, .. }
        | EventBody::RecordUnrestricted { ty, .. } => (ty, false),
        // A set of a view holds its groups (lakelands#1185).
        EventBody::ObjectSetChanged { ty, .. } if o.ty(ty).is_ok_and(|t| t.view.is_some()) => {
            return None;
        }
        EventBody::ObjectSetChanged { ty, .. } => (ty, false),
        _ => return None,
    };
    let ty = ty.clone();
    if ty.as_str().starts_with(OWN_SEAM) {
        return Some(Flaw::OwnType { ty });
    }
    let view = !declares && o.ty(&ty).is_ok_and(|t| t.view.is_some());
    view.then_some(Flaw::ViewRecord { ty })
}

/// Whether `events` end inside a set of declarations: their last
/// `declared` goes on in its append. The events a declaration of the set
/// drafts after it, an object set's changed memberships, carry no mark.
fn open_set(events: &[Event]) -> bool {
    events.iter().rev().find_map(set_mark) == Some(true)
}

/// A `declared`'s `with_the_set`: whether its set goes on after it.
fn set_mark(e: &Event) -> Option<bool> {
    match e.body {
        EventBody::Declared { with_the_set, .. } => Some(with_the_set),
        _ => None,
    }
}

/// The sources the log forgot, each with what of its act is owed
/// ([`crate::sealing::FORGOTTEN`]), as the transaction leaves the list.
pub(crate) fn forgotten_list(
    view: &dyn ProjectionView,
    staged: &Staged,
) -> Result<BTreeMap<SourceId, String>> {
    let at = (
        crate::sealing::FORGOTTEN.to_owned(),
        crate::sealing::forgotten_at(),
    );
    let list = match staged.outputs.get(&at).and_then(|l| l.last()) {
        Some((_, v)) => Some(v.clone()),
        None => view.output(&at.0, &at.1)?.map(|(_, v)| v),
    };
    Ok(crate::sealing::forgotten(list.as_ref()))
}

/// The position of the last act that erased values `view` has folded, by
/// the feed's index; `view`'s own position where the index decides
/// nothing, so a reaction it cannot place is decided again.
fn last_act(view: &dyn ProjectionView) -> Result<Position> {
    let Some(to) = view.position() else {
        return Ok(Position::GENESIS);
    };
    let kinds = vec!["source_forgotten".into(), "values_removed".into()];
    let acts = lakelands_api::seam::FeedFilter {
        kinds,
        ..Default::default()
    };
    let (mut from, mut last) = (Position::GENESIS, Position::GENESIS);
    loop {
        let m = view.matching(&acts, from, to, lakelands_api::seam::FEED_EXAMINED)?;
        last = m.positions.last().copied().unwrap_or(last);
        match m.through {
            Some(t) if t < to => from = t.next(),
            Some(_) => return Ok(last),
            None => return Ok(to),
        }
    }
}

/// The source the feed index keeps an event under, as the log names it
/// before an erasure takes the seal away: the one its seal names first,
/// else its envelope's. An ingest's seal names no other source than its
/// envelope's; a reader refuses one that does
/// (`lakelands_core::log::misplaced`).
pub(crate) fn indexed(e: &Event) -> Option<SourceId> {
    let sealed = e.sealed.as_ref().and_then(|s| s.source.as_ref());
    sealed.or(e.source.as_ref()).cloned()
}

/// Whether the fold of an event reads a record: such an event is folded
/// only after the recomputations pending before it are flushed
/// ([`Lake::fold_all`]), so it reads what they left. Every kind is named: a
/// new one does not compile until it is said here whether it reads one.
fn reads_a_record(body: &EventBody) -> bool {
    match body {
        // The record's state before, which the event changes.
        EventBody::RecordUpserted { .. }
        | EventBody::RecordIngested { .. }
        | EventBody::RecordRetired { .. }
        | EventBody::RecordRestricted { .. }
        | EventBody::RecordUnrestricted { .. }
        // Every record of the migrated type, and those a re-key reaches;
        // every record of a type that uses the migrated value type.
        | EventBody::Migrated { .. }
        | EventBody::ValueTypeMigrated { .. } => true,
        // The record whose field it fills; one that names no field is put
        // and fills nothing.
        EventBody::OutputRecorded { field, .. } => field.is_some(),
        // Nothing of the projection is changed for these but their mark.
        EventBody::LakeCreated { .. }
        | EventBody::LakeUpgraded { .. }
        | EventBody::LeaseTaken { .. }
        | EventBody::Checkpoint { .. }
        | EventBody::VerbCalled { .. }
        | EventBody::TriggerFired { .. }
        | EventBody::TriggerDropped { .. }
        // The writer's ontology, and the declaration put; a recomputation
        // is left pending, and reads its records at the flush.
        | EventBody::Declared { .. }
        // A proposal is put, or read and put: a row of its own, no record.
        | EventBody::WriteProposed { .. }
        | EventBody::WriteAccepted { .. }
        | EventBody::WriteRejected { .. }
        // A membership is put by the key the event names; the record at
        // that key is not read.
        | EventBody::ObjectSetChanged { .. }
        // The list of forgotten sources is put; the records it erases are
        // erased by the rebuild after it. So for an act of removal.
        | EventBody::SourceForgotten { .. }
        | EventBody::ValuesRemoved { .. } => false,
    }
}

impl crate::writes::Written for Staged {
    fn record(
        &self,
        view: &dyn ProjectionView,
        ty: &TypeName,
        key: &RecordKey,
    ) -> Result<Option<StoredRecord>> {
        Ok(Staged::record(self, view, ty, key)?.filter(|r| !r.is_ended()))
    }

    fn wrote<'a>(&'a self, ty: &'a TypeName) -> Box<dyn Iterator<Item = &'a StoredRecord> + 'a> {
        Box::new(self.of_type(ty))
    }

    /// The view holds a record under the type and the key it had before
    /// the steps this transaction folded: a reference to a key a step
    /// moved is asked under the key it moved from too, and what is found
    /// is taken through the steps.
    fn scan(
        &self,
        view: &dyn ProjectionView,
        ty: &TypeName,
        asked: &[Vec<(FieldName, Value)>],
    ) -> Result<Vec<StoredRecord>> {
        let steps: Vec<&Migrated> = self.steps.iter().filter_map(Step::migrated).collect();
        // A text that is a key a step moved to is asked as the key it
        // moved from too; one asked wrongly so finds a row the walk drops.
        let was = |v: &Value| {
            let key = RecordKey::from(v.as_str()?);
            let old = steps.iter().rev().fold(key, |k, s| {
                let from = s.back.values().find_map(|m| m.get(&k));
                from.cloned().unwrap_or(k)
            });
            Some(Value::from(old.as_str()))
        };
        let mut then = asked.to_vec();
        for a in asked.iter().filter(|_| !steps.is_empty()) {
            let old = |(f, v): &(FieldName, Value)| (f.clone(), was(v).unwrap_or(v.clone()));
            then.push(a.iter().map(old).collect());
        }
        let scan = lakelands_api::engine::Scan {
            limit: u32::MAX,
            restricted: true,
            equal_any: then,
            ..Default::default()
        };
        let known = self.known(ty);
        let rows = view.scan(known, &scan)?.into_iter();
        let mine = rows.filter(|r| !self.wrote(known, &r.key));
        mine.map(|r| self.stepped(view, r)).collect()
    }
}

/// What one append has written so far, read before the projection.
#[derive(Default)]
pub struct Staged {
    /// The records this transaction wrote, each under the key and in the
    /// state it has now: ended only where the fold ended a kept view's
    /// record, which `each` leaves out.
    pub(crate) records: BTreeMap<(TypeName, RecordKey), StoredRecord>,
    /// The steps this transaction folded and kept, in the order of their
    /// events.
    pub(crate) steps: Vec<Step>,
    /// The recomputations whose records are not walked yet: every
    /// `declared` that recomputes since the last event that read a record.
    pending: Vec<Recomputed>,
    /// The types the declarations of a set that goes on compute again:
    /// computed at the set's last declaration.
    with_the_set: BTreeSet<TypeName>,
    /// What the fold and the decision before it read from the projection.
    reads: Reads,
    /// Proposals changed in this append.
    pub proposals: BTreeMap<ProposalId, Proposal>,
    /// Outputs recorded in this append: every recording, since a step
    /// reads the last one before its own event.
    pub outputs: Recorded,
    /// Set memberships put in this append, by set, type and key.
    members: BTreeMap<(String, TypeName, RecordKey), bool>,
    /// The views the lake keeps, as this append works on them.
    pub(crate) kept: crate::kept::Views,
    /// The entries of the kept views' orders this append put or ended, by
    /// the name they are kept under.
    pub(crate) orders: BTreeMap<TypeName, crate::kept::Orders>,
    /// The lake's keys, which a recomputation it keeps computes with.
    keys: Option<Arc<crate::sealing::Keys>>,
    /// What the write path's run of the hooks overwrote, where it keeps
    /// that ([`crate::kept::Undo`]).
    pub(crate) undo: Option<crate::kept::Undo>,
}

impl Staged {
    /// Queue `types`' records to be computed again as of `event`, at the
    /// flush. No record and no setting changes before it, so the settings
    /// are one map for all that are pending.
    fn rederive(&mut self, w: &Writer, event: &Event, types: &[TypeName]) {
        if types.is_empty() {
            return;
        }
        let settings = match self.pending.last() {
            Some(s) => s.settings.clone(),
            None => Arc::new(w.settings.clone()),
        };
        self.pending.push(Recomputed {
            event: event.clone(),
            entries: w.ontology.computing(types),
            settings,
            plans: record::Plans::default(),
            keys: self.keys.clone(),
        });
    }

    /// A record as this append leaves it so far: written earlier in it,
    /// or else as the projection holds it, taken through the steps the
    /// append folded: read at the key it had before them, or ended at the
    /// step it moved away at. What the append wrote always comes first; a
    /// read kept from before it never answers over it.
    pub fn record(
        &self,
        view: &dyn ProjectionView,
        ty: &TypeName,
        key: &RecordKey,
    ) -> Result<Option<StoredRecord>> {
        if let Some(r) = self.records.get(&(ty.clone(), key.clone())) {
            return Ok(Some(r.clone()));
        }
        let (mut from, mut before) = (ty, key);
        for s in self.steps.iter().rev().filter_map(Step::migrated) {
            match s.source(from, before) {
                Some(old) => (from, before) = old,
                None => return Ok(Some(StoredRecord::ended(ty.clone(), key.clone(), s.at))),
            }
        }
        match self.reads.get(view, from, before)? {
            Some(r) if !r.is_ended() => self.stepped(view, r).map(Some),
            held => Ok(held),
        }
    }

    /// The type the view holds the records of `ty` under: the one they
    /// had before the transaction, which a step of it may have moved them
    /// from.
    fn known<'a>(&'a self, ty: &'a TypeName) -> &'a TypeName {
        let moves = self.steps.iter().rev().filter_map(Step::migrated);
        moves.fold(ty, |t, s| s.origin(t))
    }

    /// A record of the view after every step the append kept.
    fn stepped(&self, view: &dyn ProjectionView, r: StoredRecord) -> Result<StoredRecord> {
        let mut steps = self.steps.iter();
        steps.try_fold(r, |r, s| s.apply(r, view, &self.outputs))
    }

    /// Whether the append wrote the record the view holds at `key`: it is
    /// kept under the key the steps moved it to.
    fn wrote(&self, ty: &TypeName, key: &RecordKey) -> bool {
        let (mut ty, mut key) = (ty, key);
        for s in self.steps.iter().filter_map(Step::migrated) {
            (ty, key) = (s.types.get(ty).unwrap_or(ty), s.moved(ty, key));
        }
        self.records.contains_key(&(ty.clone(), key.clone()))
    }

    /// Every record of `ty` as the append leaves it so far, ended ones
    /// left out, handed to `each` one at a time: the view's, a page at a
    /// time, each taken through the append's steps, then what the append
    /// wrote. Nothing of the walk is held.
    pub(crate) fn each(
        &self,
        view: &dyn ProjectionView,
        ty: &TypeName,
        mut each: impl FnMut(StoredRecord) -> Result<()>,
    ) -> Result<()> {
        let known = self.known(ty);
        walk(view, known, true, |row| {
            let stepped = match self.wrote(known, &row.key) {
                true => None,
                false => Some(self.stepped(view, row)?).filter(|r| &r.ty == ty),
            };
            stepped.map_or(Ok(()), &mut each)
        })?;
        (self.of_type(ty).filter(|r| !r.is_ended()).cloned()).try_for_each(each)
    }

    /// Whether a recomputation of the records of `ty` is pending: their
    /// derived fields are not what the declarations in force compute.
    pub(crate) fn recomputes(&self, ty: &TypeName) -> bool {
        let pending = |s: &Recomputed| s.entries.types.contains_key(ty);
        self.pending.iter().any(pending) || self.with_the_set.contains(ty)
    }

    /// The types whose records the append's last step reached: the
    /// migrated one, those with moved keys or references to them, and the
    /// link types records moved between; or those a value type's renames
    /// reach.
    pub(crate) fn stepped_types(&self) -> BTreeSet<TypeName> {
        let s = match self.steps.last() {
            Some(Step::Migrated(s)) => s,
            Some(Step::Values(s)) => return s.fields.keys().cloned().collect(),
            _ => return BTreeSet::new(),
        };
        let reached = s.decls.keys().chain(s.types.keys()).chain(s.types.values());
        reached.chain([&s.migration.to.name]).cloned().collect()
    }

    /// The records of one type written in this append.
    pub fn of_type<'a>(&'a self, ty: &'a TypeName) -> impl Iterator<Item = &'a StoredRecord> + 'a {
        self.records
            .range((ty.clone(), RecordKey::from(""))..)
            .take_while(move |((t, _), _)| t == ty)
            .map(|(_, r)| r)
    }

    /// Whether the record of `ty` at `key` is a member of `set` as the
    /// append leaves it so far.
    pub(crate) fn is_member(
        &self,
        view: &dyn ProjectionView,
        set: &str,
        ty: &TypeName,
        key: &RecordKey,
    ) -> Result<bool> {
        match self.members.get(&(set.to_owned(), ty.clone(), key.clone())) {
            Some(m) => Ok(*m),
            None => view.is_member(set, ty, key),
        }
    }

    /// The members of `set` of the type the view knows as `known`, and of
    /// `ty`, as the append leaves them so far.
    pub(crate) fn members(
        &self,
        view: &dyn ProjectionView,
        set: &str,
        (known, ty): (&TypeName, &TypeName),
    ) -> Result<BTreeSet<RecordKey>> {
        let mut keys = view.members(set, known)?;
        for t in [known, ty] {
            let put = self.members.iter();
            for ((_, _, key), member) in put.filter(|((n, of, _), _)| n == set && of == t) {
                match member {
                    true => keys.insert(key.clone()),
                    false => keys.remove(key),
                };
            }
        }
        Ok(keys)
    }

    /// Put a set membership, and remember it: the view a fold of several
    /// appends opened does not show it, and a re-key later in the page
    /// carries it to the record's new key.
    pub(crate) fn member(
        &mut self,
        projection: &dyn lakelands_api::engine::ProjectionStore,
        set: &str,
        ty: &TypeName,
        key: &RecordKey,
        member: bool,
        at: Position,
    ) -> Result<()> {
        self.members
            .insert((set.to_owned(), ty.clone(), key.clone()), member);
        projection.put_membership(set, ty, key, member, at)
    }

    /// The records this transaction read from the projection.
    pub(crate) fn into_reads(self) -> Reads {
        self.reads
    }

    /// Remember a record written in this append.
    pub fn put(&mut self, r: StoredRecord) {
        let at = (r.ty.clone(), r.key.clone());
        match &mut self.undo {
            Some(undo) => undo.record(at.clone(), self.records.insert(at, r)),
            None => drop(self.records.insert(at, r)),
        }
    }

    /// A record as `view` holds it, before anything this append wrote.
    pub(crate) fn before(
        &self,
        view: &dyn ProjectionView,
        ty: &TypeName,
        key: &RecordKey,
    ) -> Result<Option<StoredRecord>> {
        self.reads.get(view, ty, key)
    }

    /// The state the fold kept of `ty` as `view` holds it, read once.
    pub(crate) fn kept_state(
        &self,
        view: &dyn ProjectionView,
        ty: &TypeName,
    ) -> Result<Option<crate::kept::State>> {
        self.reads.state(view, ty)
    }
}

#[cfg(test)]
mod tests;
