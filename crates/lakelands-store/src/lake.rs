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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use lakelands_api::engine::{CanonicalEvent, LakeEngine, Lease, OpenMode, ProjectionView};
use lakelands_api::error::{Detail, Flaw, Part, Seam, Situation};
use lakelands_api::event::{Digest, Event, EventBody, LOG_FORMAT_VERSION, SegmentDigest};
use lakelands_api::location::{EngineConfig, Limits, Location};
use lakelands_api::ontology::{FieldDecl, StorageKind, TypeDecl};
use lakelands_api::principal::Renamed;
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::{EmbedBackend, Seams};
use lakelands_api::write::{
    Declaration, Migration, Proposal, ProposalFilter, ProposalState, ProposedWrite,
};
use lakelands_api::{
    Error, FieldName, LakeId, Position, Principal, ProposalId, RecordKey, Refusal, Result,
    SourceId, Timestamp, TypeName, Value, ValueTypeName, Version, WriteContext,
};
use lakelands_core::sight::{InForce, Sight};
use lakelands_core::{damaged, shape};

use crate::backed::{Due, Of, Outputs, Owed, Recorded};
use crate::kept;
use crate::log::{self, Draft};
use crate::ontology::Ontology;
use crate::record;

pub use lakelands_api::seam::{AGING_RULE_TYPE, SETTING_TYPE, aging_rule_type, setting_type};

/// What only the writer changes.
pub struct Writer {
    /// The lease, while this handle holds it.
    pub lease: Option<Lease>,
    /// The last event's position and hash.
    pub tip: Option<(Position, Digest)>,
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
    /// it lived in closed under it (lakelands#979). The next write, rebuild
    /// or compaction takes the lease again before it decides anything
    /// ([`Lake::regain`]); until its `lease_taken` is appended this stays.
    pub lost: bool,
    /// The sources the log forgot that a fold has met (058): a fact of
    /// the log, so kept over a rebuild.
    pub forgotten: BTreeSet<SourceId>,
    /// The acts of removal a fold met or a rebuild was told of (065),
    /// whose selected values the fold erases: a fact of the log, so kept
    /// over a rebuild.
    pub(crate) removals: Vec<crate::remove::Removal>,
    /// A rebuild is folding: an act it meets after its source's keys were
    /// destroyed has erased what they sealed.
    pub rebuilding: bool,
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
    /// Bounds on one call's work.
    pub limits: Limits,
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
    /// The sources' keys the lake was given, and the key store (058).
    pub(crate) keys: Arc<crate::sealing::Keys>,
    /// Why a forgetting's reclaim is still owed, where the last one failed
    /// (lakelands#1161).
    reclaim_owed: Mutex<Option<String>>,
    /// Why the writer's open rebuilt the projection: an older build laid
    /// it out (lakelands#1151).
    rebuilt_at_open: Option<String>,
}

/// Why a writer's open rebuilds the projection `e` refuses: an older
/// build laid it out. None for any other refusal.
fn older_layout(e: &Error) -> Option<String> {
    match e.situation() {
        Some(Situation::ProjectionLayout {
            found: Some(found),
            wanted,
            ..
        }) if found < wanted => Some(format!(
            "an older build laid it out (layout {found}; this build's is {wanted})"
        )),
        _ => None,
    }
}

/// The opener `seams` passes for the engine `location` names.
pub(crate) fn opener(
    location: &Location,
    seams: &Seams,
) -> Result<Arc<dyn lakelands_api::engine::EngineOpener>> {
    let name: String = match &location.engine {
        EngineConfig::Sqlite { .. } => "sqlite".into(),
        EngineConfig::Memory => "memory".into(),
        EngineConfig::Postgres { .. } => "pg".into(),
        EngineConfig::Other { name, .. } => name.clone(),
        _ => String::new(),
    };
    seams
        .engines
        .iter()
        .find(|e| e.name() == name)
        .cloned()
        .ok_or_else(|| {
            Error::unbound(Seam::Engine {
                name: name.to_owned(),
            })
        })
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

/// What the kept views' hooks are handed in the fold of one event.
macro_rules! kept_at {
    ($w:expr, $view:expr, $projection:expr) => {
        kept::At {
            view: $view,
            projection: Some($projection),
            ontology: &$w.ontology,
            settings: &$w.settings,
        }
    };
}

impl Lake {
    /// Open a lake: find its engine among the openers the host passed,
    /// take the lease unless opening read-only, create the lake if asked
    /// and absent, and fold whatever the log holds beyond the projection.
    /// Opened for a rebuild, the projection is cleared under the lease
    /// first, so the whole log is folded: that open is the rebuild. So is
    /// a writer's open of a projection an older build laid out.
    pub fn open(location: &Location, seams: Seams, mode: OpenMode) -> Result<Lake> {
        let opener = opener(location, &seams)?;
        // A lake of a format this release does not read is not opened: its
        // engine judges, makes and changes nothing of it, in any mode (054).
        // One of an older format it reads is upgraded by a writer's open,
        // and is not read as it is.
        let written = log::one_format(&location.lake, opener.log_ends(location)?.as_ref())?;
        if mode == OpenMode::Read {
            log::read_as_is(&location.lake, written)?;
        }
        // A writer's open of a projection an older build laid out rebuilds
        // it, as an upgrade's open is the place a release pays for its
        // steps; a reader's, and one of a newer layout or of none, is
        // refused (004).
        let (engine, rebuilt_at_open) = match opener.open(location, mode) {
            Err(e) if mode != OpenMode::Read && older_layout(&e).is_some() => {
                (opener.open(location, OpenMode::Rebuild)?, older_layout(&e))
            }
            opened => (opened?, None),
        };
        let engine: Arc<dyn LakeEngine> = Arc::from(engine);
        // A writer is judged before it takes the lease: an engine may cut
        // its log back to what is committed as it hands the lease out,
        // and a lake that is refused is left as it was found. A rebuild
        // is not judged: it makes the projection from the log as it
        // stands.
        if matches!(mode, OpenMode::Write | OpenMode::Create) {
            ends(engine.as_ref())?;
        }
        let lease = match mode {
            OpenMode::Read => None,
            OpenMode::Write | OpenMode::Create | OpenMode::Rebuild => {
                Some(engine.take_lease(&format!("lakelands process {}", std::process::id()))?)
            }
        };
        if mode == OpenMode::Rebuild || rebuilt_at_open.is_some() {
            engine.projection().clear()?;
        }
        let mut lake = Lake::assemble(location, seams, engine, lease.clone());
        lake.rebuilt_at_open = rebuilt_at_open;
        lake.catch_up()?;
        if let Some(lease) = lease {
            let mut w = lake.writer();
            let now = lake.clock(&mut w);
            let mut drafts = Vec::new();
            if w.tip.is_none() {
                drafts.push(Draft::system(
                    now,
                    EventBody::LakeCreated {
                        format: LOG_FORMAT_VERSION,
                    },
                ));
            }
            drafts.push(Draft::system(
                now,
                EventBody::LeaseTaken {
                    fence: lease.fence,
                    holder: lease.holder.clone(),
                },
            ));
            // A lake written in an older format is written in this one from
            // here on, the step marked right after the new writer's lease.
            // The events before it stay as they were written; the step from
            // 14 changes no fold, so the projection stays
            // (`docs/log-format.md` §6).
            // Judged by the tip read under the lease: another writer may
            // have upgraded the lake since its ends were read.
            let tip = w.tip.as_ref().map(|t| t.0);
            let tip = tip.map(|p| lake.read_events(p, 1)).transpose()?;
            let newest = tip.and_then(|t| t.first().map(|e| e.v));
            if let Some(newest) = newest.filter(|v| *v < LOG_FORMAT_VERSION) {
                let (from, to) = (newest, LOG_FORMAT_VERSION);
                drafts.push(Draft::system(now, EventBody::LakeUpgraded { from, to }));
            }
            // The system types this lake may hold and does not hold yet are
            // declared on every open for writing, so a host type added after
            // the lake was created reaches it too.
            let mut types = vec![setting_type(), aging_rule_type()];
            if let Some(v) = &lake.seams.vocabulary {
                let held: Vec<lakelands_api::ontology::TypeDecl> = v
                    .types()
                    .into_iter()
                    .filter(|t| v.may_hold(&lake.id, &t.name))
                    .collect();
                // Every vocabulary type is checked before anything is
                // appended, held already or not: a process whose
                // vocabulary collides with the store's own names must not
                // write.
                for t in &held {
                    crate::declare::check_vocabulary_type(t)?;
                }
                types.extend(held);
            }
            // The store's own types are declared again where the lake holds
            // an older form, one without a field they have since gained: a
            // field added, which reshapes no record (063, extended).
            let own = |t: &lakelands_api::ontology::TypeDecl| {
                [SETTING_TYPE, AGING_RULE_TYPE].contains(&t.name.as_str())
            };
            let older = |t: &lakelands_api::ontology::TypeDecl| {
                let held = w.ontology.types.get(&t.name);
                held.is_some_and(|h| {
                    let has = |f: &lakelands_api::ontology::FieldDecl| {
                        h.decl.fields.iter().any(|g| g.name == f.name)
                    };
                    !t.fields.iter().all(has)
                })
            };
            types.retain(|t| !w.ontology.types.contains_key(&t.name) || (own(t) && older(t)));
            // No door leaves a lake with a problem among its tools' names,
            // and this is one (027, extended): each type is judged beside
            // the lake and the types before it, before anything is
            // appended. A type the lake holds came through a door.
            let mut with = w.ontology.clone();
            for d in types.iter().cloned().map(Declaration::Type) {
                crate::declare::tool_names(&with, &d)?;
                crate::writes::put_as_appended(&mut with, &d, Some);
            }
            let grouped = types.iter().any(|t| t.name.as_str() == SETTING_TYPE)
                && w.ontology.types.contains_key(&TypeName::from(SETTING_TYPE));
            for t in types {
                let hash = shape::schema_hash(&t).map_err(|e| {
                    let _ = e;
                    let flaw = Flaw::NoSchemaHash { ty: t.name.clone() };
                    damaged(Part::Projection, None, flaw)
                })?;
                let declaration = Declaration::Type(t);
                let version = w.ontology.next_version(&declaration);
                drafts.push(Draft::system(
                    now,
                    EventBody::Declared {
                        declaration,
                        schema_hash: Some(hash),
                        version,
                        with_the_set: false,
                    },
                ));
            }
            // The settings the lake held before `sys.setting` had a
            // `group` are stamped in the same append, so no record stands
            // in a group its declaration does not name (063, extended).
            if grouped {
                let ty = TypeName::from(SETTING_TYPE);
                for r in lake.engine.view(None)?.scan(&ty, &every(false))? {
                    let group = crate::writes::setting_group(&w.ontology, r.key.as_str());
                    if r.fields.get("group").unwrap_or(&Value::Null) == &group {
                        continue;
                    }
                    let body = EventBody::RecordUpserted {
                        ty: ty.clone(),
                        key: r.key.clone(),
                        version: Version(r.version.0 + 1),
                        patch: [("group".into(), group)].into(),
                        made: false,
                    };
                    drafts.push(Draft::system(now, body));
                }
            }
            lake.append(&mut w, drafts)?;
            // What a crash left of a forgetting is finished first (058).
            // An owed rebuild refuses the open, since the projection is not
            // whole without it; a reclaim that fails again stays owed, is
            // told and is tried at the next open, never a lock-out: the
            // values are erased already (lakelands#1161).
            if let Err(e) = lake.finish_forgetting(&mut w)
                && lake.reclaim_owed().is_none()
            {
                return Err(e);
            }
        }
        {
            // A read-only handle keeps the folded position `catch_up`
            // published; only the writer publishes its tip.
            let w = lake.writer();
            if w.lease.is_some() {
                lake.publish(&w);
            }
        }
        Ok(lake)
    }

    /// A lake over `engine`, holding `lease` if one was taken, before
    /// anything is read from its log.
    pub(crate) fn assemble(
        location: &Location,
        mut seams: Seams,
        engine: Arc<dyn LakeEngine>,
        lease: Option<Lease>,
    ) -> Lake {
        let seams_keys = seams.keys.clone();
        Lake {
            id: location.lake.clone(),
            engine,
            embed: Mutex::new(seams.embed.take()),
            seams,
            limits: location.limits,
            calls: tokio::sync::Mutex::new(()),
            waiting: Mutex::new(BTreeMap::new()),
            sorted_whole: Mutex::new(BTreeMap::new()),
            writer: Mutex::new(Writer {
                lease,
                tip: None,
                last_known_at: Timestamp(i64::MIN),
                ontology: Ontology::default().named(),
                settings: BTreeMap::new(),
                owed: Vec::new(),
                held: BTreeMap::new(),
                behind: false,
                failed: None,
                answers: Vec::new(),
                plans: record::Plans::default(),
                lost: false,
                forgotten: BTreeSet::new(),
                removals: Vec::new(),
                rebuilding: false,
            }),
            keys: Arc::new(crate::sealing::Keys::new(location.lake.clone(), seams_keys)),
            head: AtomicU64::new(0),
            heads: tokio::sync::watch::Sender::new(0),
            holds_lease: AtomicBool::new(false),
            shapes: Mutex::new(Vec::new()),
            rebuilds: AtomicU64::new(0),
            reclaim_owed: Mutex::new(None),
            rebuilt_at_open: None,
        }
    }

    /// A writer that holds nothing, over `view`: what a write would decide
    /// there, for a preflight that reads and never takes the lease. It
    /// appends nothing; its drafts' positions follow the view's.
    pub(crate) fn reading(&self, view: &dyn ProjectionView) -> Result<Writer> {
        let shape = self.shape(view)?;
        let tip = view.position().map(|p| (p, Digest::zero()));
        Ok(Writer {
            lease: None,
            tip,
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
            rebuilding: false,
        })
    }

    /// Make the writer's tip and lease readable without its lock.
    fn publish(&self, w: &Writer) {
        self.set_head(w.tip.as_ref().map_or(0, |t| t.0.0));
        let holds = w.lease.is_some() && !w.lost;
        self.holds_lease.store(holds, Ordering::Release);
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

    /// The writer state, locked.
    pub fn writer(&self) -> MutexGuard<'_, Writer> {
        lakelands_core::lock(&self.writer)
    }

    /// The writer for a write, locked and caught up. A writer whose fold
    /// failed is behind its log: it folds from the projection's mark before
    /// anything is decided over the projection, or the write is answered
    /// `unavailable`, with that fold's words, and appends nothing.
    /// Everything that decides a write takes the writer here;
    /// [`Self::append_after`] refuses one that is behind. Only the lease's
    /// holder folds: a handle that gave its lease up while it was behind
    /// folds nothing and is refused here, as its append would be. A held
    /// answer goes in first; failing again, it stops no write, and one
    /// whose fold fails is caught up from as any other.
    pub fn for_write(&self) -> Result<MutexGuard<'_, Writer>> {
        let mut w = self.writer();
        self.regain(&mut w)?;
        self.caught_up(&mut w)?;
        let _ = self.record_held(&mut w, None);
        self.caught_up(&mut w)?;
        Ok(w)
    }

    /// Marks the writer lost where the engine said its lease is not held
    /// (lakelands#979): the lease goes, and the handle is no writer until
    /// it takes one again.
    fn lose(&self, w: &mut Writer, e: &Error) {
        if matches!(e.situation(), Some(Situation::LeaseNotHeld { .. })) {
            (w.lost, w.lease) = (true, None);
            self.holds_lease.store(false, Ordering::Release);
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
    fn regain(&self, w: &mut Writer) -> Result<()> {
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

    /// Fold the log's events the projection does not reflect yet, and load
    /// the head's ontology and settings.
    pub(crate) fn catch_up(&self) -> Result<()> {
        self.catch_up_in(&mut self.writer())
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

    /// Seal drafts after the tip, append them durably, fold them. Answers
    /// the sealed events.
    pub fn append(&self, w: &mut Writer, drafts: Vec<Draft>) -> Result<Vec<Event>> {
        self.append_after(w, drafts, Reads::default())
    }

    /// [`Self::append`], for drafts decided over the records in `reads`:
    /// the fold takes each from there instead of reading it again, while
    /// its view is at the position they were read at.
    pub fn append_after(
        &self,
        w: &mut Writer,
        drafts: Vec<Draft>,
        reads: Reads,
    ) -> Result<Vec<Event>> {
        let (lease, events, stored) = self.stamp(w, drafts)?;
        if let Err(e) = self.engine.append(&lease, &stored) {
            self.lose(w, &e);
            return Err(e);
        }
        self.landed(w, events, reads)
    }

    /// The events `drafts` become after the tip, as the log writes them
    /// and as the engine stores them, under the writer's lease: refused
    /// where an append of them would be. Nothing is changed.
    ///
    /// The seal judges every event as the log's own reading will
    /// (`docs/log-format.md` §1.2, §1.3): one that holds a number written
    /// as an integer beyond ±(2^53 − 1), or a `known_at` or `seen_at` out
    /// of the years 0000 to 9999, refuses the whole append, whatever door
    /// its value came through. Every door refuses such a value itself and
    /// names where it stands; a refusal from here names no place, and is a
    /// defect of the door that let the value pass.
    fn stamp(
        &self,
        w: &Writer,
        drafts: Vec<Draft>,
    ) -> Result<(Lease, Vec<Event>, Vec<CanonicalEvent>)> {
        let Some(lease) = w.lease.clone() else {
            return Err(no_lease());
        };
        // Nothing is appended while the projection is behind the log, and
        // nothing is folded this late: the drafts were decided already.
        if w.behind {
            return Err(Error::unavailable(Situation::DecidedBehind));
        }
        // A host's cause names an event the log holds: at or below the
        // head (021). What it names is the host's business.
        let head = w.tip.as_ref().map(|(p, _)| *p);
        if let Some(cause) = drafts
            .iter()
            .filter_map(|d| d.context.cause)
            .find(|c| Some(*c) > head)
        {
            let case = lakelands_api::error::Case::CausePastHead { cause, head };
            return Err(lakelands_core::invalid(case));
        }
        // A reaction that names a trigger's firing as its cause is bound by
        // that trigger's own ceiling too, as the head declares it (021).
        // It is read here, under the writer, so the firing the cause names
        // is the one this append follows, never one appended meanwhile.
        if let Some(d) = drafts.iter().find(|d| d.context.cause.is_some()) {
            self.check_trigger_ceiling(w, d.context)?;
        }
        let mut events = Vec::with_capacity(drafts.len());
        let mut stored: Vec<CanonicalEvent> = Vec::with_capacity(drafts.len());
        let mut prev_pos = head;
        let mut prev_hash = w.tip.as_ref().map_or_else(Digest::zero, |(_, h)| h.clone());
        for d in drafts {
            let position = prev_pos.map_or(Position::GENESIS, Position::next);
            // Position 0 is written before any lease exists in the lake.
            let fence = if position == Position::GENESIS {
                0
            } else {
                lease.fence
            };
            let mut d = d;
            if let Some(sealed) = &d.sealed
                && let Some(source) = sealed.source.as_ref().or(d.source.as_ref())
            {
                let key = self.keys.key(source, &sealed.key)?;
                crate::sealing::seal(&mut d.body, sealed, source, &key);
            }
            let (event, canon, fault) = log::seal_judged(&self.id, position, &prev_hash, fence, d);
            if let Some(case) = fault {
                return Err(lakelands_core::invalid(case));
            }
            prev_pos = Some(position);
            prev_hash = event.hash.clone();
            events.push(event);
            stored.push(canon);
        }
        Ok((lease, events, stored))
    }

    /// What follows once the engine holds `events`: the writer stands on
    /// them, and they are folded and published.
    fn landed(&self, w: &mut Writer, events: Vec<Event>, reads: Reads) -> Result<Vec<Event>> {
        if let Some(last) = events.last() {
            w.tip = Some((last.position, last.hash.clone()));
            w.last_known_at = w.last_known_at.max(last.known_at);
        }
        // The events are durable whatever the fold answers: its failure is
        // told as that, so that it is not read as a write that did not land.
        self.fold_all(w, &events, reads, false).map_err(|e| {
            let at = events.first().map_or(0, |e| e.position.0);
            let (from, cause) = (Position(at), Box::new(e));
            Error::unavailable(Situation::NotFolded { from, cause })
        })?;
        // A shape holds over what is committed now unless an event ends a
        // range; it is carried only from where it stands, so a fold that
        // began after its range's end (another handle wrote, or a fold failed
        // and was caught up) leaves it to its own range.
        // An append whose only such events declare leaves what makes the
        // shape after it, the shape before and its declarations, for the
        // first read of its position to build (lakelands#1056).
        let declares = |e: &&Event| matches!(e.body, EventBody::Declared { .. });
        if let (Some(first), Some(last)) = (events.first(), events.last()) {
            let (first, mut held) = (first.position.0, lakelands_core::lock(&self.shapes));
            let mut ending = events.iter().filter(|e| ends_a_range(&e.body)).peekable();
            if ending.peek().is_none() {
                for (range, _) in held.iter_mut().filter(|(r, _)| r.end() + 1 == first) {
                    *range = *range.start()..=last.position.0;
                }
            } else if ending.all(|e| declares(&e)) {
                let before = held.iter().find(|(r, _)| r.end() + 1 == first);
                let links = |before: &HeldShape| match before {
                    HeldShape::Built(_) => 1,
                    HeldShape::Left(left) => left.links + 1,
                };
                let before = before.filter(|(_, b)| links(b) <= LEFT_LINKS);
                if let Some((_, before)) = before {
                    let declared = (events.iter())
                        .filter_map(|e| match &e.body {
                            EventBody::Declared {
                                declaration,
                                schema_hash,
                                version,
                                ..
                            } => Some(lakelands_api::engine::Declared {
                                declaration: declaration.clone(),
                                version: *version,
                                schema_hash: schema_hash.clone(),
                                since: e.position,
                            }),
                            _ => None,
                        })
                        .collect();
                    let left = Left {
                        before: Some(before.clone()),
                        declared,
                        links: links(before),
                    };
                    held.sort_by_key(|(range, _)| *range.end());
                    if held.len() > 1 {
                        held.remove(0);
                    }
                    let at = last.position.0..=last.position.0;
                    held.push((at, HeldShape::Left(Arc::new(left))));
                }
            }
        }
        // Published only once the fold has committed: a position `head()`
        // answers is one a read can be answered at, on every engine.
        self.publish(w);
        Ok(events)
    }

    /// Refuse a context deeper than the ceiling of the trigger whose firing
    /// its cause names; any other cause binds nothing more.
    fn check_trigger_ceiling(&self, w: &Writer, context: WriteContext) -> Result<()> {
        let Some(cause) = context.cause else {
            return Ok(());
        };
        let Some(stored) = self.engine.read(cause, 1)?.into_iter().next() else {
            return Ok(());
        };
        if let EventBody::TriggerFired { trigger, .. } = log::open(&stored)?.body
            && let Some(m) = w
                .ontology
                .triggers
                .get(trigger.as_str())
                .and_then(|e| e.decl.max_depth)
            && context.depth > m
            && !self.withdrawn_since(trigger.as_str(), cause)?
        {
            return Err(Error::Refused {
                field: Some(trigger.as_str().into()),
                reason: Refusal::TooDeep { max: m },
            });
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

    /// Fold events into the projection: an append's in one transaction, a
    /// page `of_log` in one unless a recomputation was kept as a step.
    ///
    /// The steps a `declared` left pending are flushed before the first
    /// event that reads a record ([`reads_a_record`]) and before the commit.
    /// A fold of the log then ends its transaction when the flush kept a
    /// step: it commits through the last event folded and begins again, so
    /// the next walk reads those records in its view and no step is stacked
    /// on another. Each commit moves the engine's `folded_through`, and
    /// what it commits is whole: every event through that position, their
    /// recomputations put. An append is never split, and it stacks only if
    /// it holds a second flush, events that read records between two runs
    /// of declarations: that flush applies the first's kept steps to every
    /// row again. No door drafts such an append.
    ///
    /// An append hands out the calls its events make due; a fold of the
    /// log hands out only those of an append whose fold failed.
    fn fold_all(&self, w: &mut Writer, events: &[Event], reads: Reads, of_log: bool) -> Result<()> {
        let Some(last) = events.last() else {
            return Ok(());
        };
        let owed = (!of_log).then_some(Position::GENESIS).or(w.failed);
        let projection = self.engine.projection();
        let (start, mut due) = (w.owed.len(), w.owed.len());
        // A migration carries the entries in place: a fold that fails puts
        // them back as its last commit left them, for the fold that
        // catches up to carry once.
        let carries = events
            .iter()
            .any(|e| matches!(e.body, EventBody::Migrated { .. }));
        let mut kept = carries.then(|| w.owed.clone());
        let mut staged = Staged {
            reads,
            keys: Some(self.keys.clone()),
            ..Staged::default()
        };
        let result = (|| {
            // Only a damaged log ends inside a set: an append holds its set
            // whole, and a page of the log never ends inside one.
            if open_set(events) {
                return Err(damaged(Part::Log, Some(last.position), Flaw::EndsInsideSet));
            }
            // A `begin` that is refused leaves the writer behind like any
            // other failure of the fold: the events are in the log.
            projection.begin()?;
            let mut view = self.engine.view(None)?;
            for (i, e) in events.iter().enumerate() {
                if reads_a_record(&e.body)
                    && self.flush(w, view.as_ref(), &mut staged, &events[..i], owed)?
                    && of_log
                {
                    drop(view);
                    projection.commit(events[i - 1].position)?;
                    // What is committed stays owed, whatever fails next.
                    due = w.owed.len();
                    if let Some(kept) = kept.as_mut() {
                        kept.clone_from(&w.owed);
                    }
                    projection.begin()?;
                    // No door drafts one inside a set; a foreign log may.
                    staged = Staged {
                        with_the_set: std::mem::take(&mut staged.with_the_set),
                        keys: Some(self.keys.clone()),
                        ..Staged::default()
                    };
                    view = self.engine.view(None)?;
                }
                let owes = owed.is_some_and(|p| e.position >= p);
                self.fold(w, view.as_ref(), &mut staged, e, owes)?;
            }
            self.flush(w, view.as_ref(), &mut staged, events, owed)
                .map(drop)
        })()
        .and_then(|()| projection.commit(last.position));
        match result {
            Ok(()) => {
                w.behind = false;
                let causes = w.owed[start..].iter().map(|o| o.cause);
                OWING.with(|o| o.borrow_mut().extend(causes));
                Ok(())
            }
            Err(e) => {
                // The projection is as its last commit left it; so is what
                // the writer read from it while folding.
                projection.rollback();
                w.behind = true;
                self.lose(w, &e);
                match kept {
                    Some(kept) => w.owed = kept,
                    None => w.owed.truncate(due),
                }
                if !of_log {
                    w.failed = Some(events[0].position);
                }
                if let Ok(view) = self.engine.view(None)
                    && let Ok(o) = Ontology::at(view.as_ref())
                {
                    if let Ok(s) = settings_in(view.as_ref(), &o) {
                        w.settings = s;
                    }
                    w.ontology = o.named();
                }
                Err(e)
            }
        }
    }

    /// Fold one `migrated` event's step over every record it reaches: the
    /// migrated type's, for a re-key those of every type with moves and of
    /// every type that references one, and for a renamed relation carried
    /// by link records those of its link type, which move to the link type
    /// of its new name and leave their old keys ended. Each type is walked
    /// once, under the name the view knows it by, a page
    /// of the view at a time, and nothing of the walk is kept: a record
    /// this transaction wrote is stepped where it is kept, and a later read
    /// of any other goes through the step ([`Staged::record`]). An old key
    /// that is no one's new key ends: it reads as not found from here on,
    /// and a later record at it starts at version 1. Every listed move is
    /// applied, or the fold fails.
    fn step(
        &self,
        w: &Writer,
        view: &dyn ProjectionView,
        projection: &dyn lakelands_api::engine::ProjectionStore,
        staged: &mut Staged,
        step: Migrated,
    ) -> Result<()> {
        let migrated = &step.migration.to.name;
        let moved = step.decls.keys().chain(step.types.keys());
        let reached: BTreeSet<&TypeName> = moved.chain([migrated]).collect();
        // The old keys whose records moved.
        let mut applied = BTreeSet::new();
        let mut put = |pre: &StoredRecord, post: &StoredRecord| -> Result<()> {
            if post.ty == *migrated || post != pre {
                projection.put_record(post)?;
            }
            // A record of the migrated type, and every record whose key the
            // step moves, a link of a re-keyed end among them, takes its
            // vectors along; any other type's declaration is as it was.
            let moved = post.key != pre.key;
            let before = match pre.ty == *migrated {
                true => Some(&step.before),
                false => w.ontology.ty(&pre.ty).ok().filter(|_| moved),
            };
            if let Some(before) = before.filter(|_| post.ty == pre.ty) {
                let left = moved && !step.arrives(&pre.ty, &pre.key);
                self.reindex(&w.ontology, before, pre, post, left)?;
            }
            if post.ty != pre.ty {
                // A link type's records moved to another: its keys end.
                let ended = StoredRecord::ended(pre.ty.clone(), pre.key.clone(), step.at);
                projection.put_record(&ended)?;
                self.index_vectors(&w.ontology, &ended)?;
                self.index_vectors_as(&w.ontology, &pre.ty, post)?;
            }
            if post.key != pre.key {
                if !step.arrives(&pre.ty, &pre.key) {
                    let ended = StoredRecord::ended(pre.ty.clone(), pre.key.clone(), step.at);
                    projection.put_record(&ended)?;
                }
                applied.insert((pre.ty.clone(), pre.key.clone()));
            }
            Ok(())
        };
        for ty in &reached {
            // The view holds a record under the type it had before the
            // transaction: an earlier step of it may have moved the type.
            let ty = staged.known(ty);
            walk(view, ty, true, |row| {
                // A record this transaction wrote is its own, under the
                // key the earlier steps left it: tested before anything
                // of the view's row is converted.
                if staged.wrote(ty, &row.key) {
                    return Ok(());
                }
                let pre = staged.stepped(view, row)?;
                put(&pre, &step.apply(pre.clone())?)
            })?;
        }
        // What the transaction wrote is stepped where it is kept: all are
        // taken out before any is put back, a new key may be another's old.
        let (stepped, kept) = std::mem::take(&mut staged.records)
            .into_iter()
            .partition::<Vec<_>, _>(|((ty, _), _)| reached.contains(ty));
        staged.records = kept.into_iter().collect();
        for (_, pre) in stepped {
            let post = step.apply(pre.clone())?;
            put(&pre, &post)?;
            staged.put(post);
        }
        let listed = (step.moves.iter()).flat_map(|(ty, m)| m.keys().map(move |from| (ty, from)));
        for (ty, from) in listed {
            if !applied.contains(&(ty.clone(), from.clone())) {
                let (ty, key) = (ty.clone(), from.clone());
                return Err(damaged(
                    Part::Log,
                    Some(step.at),
                    Flaw::MovesUnheld { ty, key },
                ));
            }
        }
        // Object-set membership moves with the record: the new key is in
        // every set the old key was in, and an ended key is in none. Every
        // membership is read before any is put.
        let mut members = Vec::new();
        for s in w.ontology.object_sets.values().map(|s| &s.decl) {
            let Some(moves) = step.moves.get(&s.of) else {
                continue;
            };
            // Read once for the set, not looked up for each moved record; a
            // membership this page put is not in the view yet.
            let held = view.members(&s.name, &s.of)?;
            for (from, to) in moves {
                let put = staged
                    .members
                    .get(&(s.name.clone(), s.of.clone(), from.clone()));
                let member = put.copied().unwrap_or_else(|| held.contains(from));
                members.push((&s.name, &s.of, from, to, member));
            }
        }
        for (set, ty, from, to, member) in members {
            staged.member(projection, set, ty, to, member, step.at)?;
            if member && !step.arrives(ty, from) {
                staged.member(projection, set, ty, from, false, step.at)?;
            }
        }
        // A set of a link type whose records moved holds them under the
        // type they moved to: read under the type the view knows them by,
        // with what the transaction put laid over.
        for s in w.ontology.object_sets.values().map(|s| &s.decl) {
            let Some(to) = step.types.get(&s.of) else {
                continue;
            };
            let known = staged.known(&s.of);
            let mut keys = view.members(&s.name, known)?;
            for ty in [known, &s.of] {
                let put = staged.members.iter();
                for ((_, _, key), member) in put.filter(|((n, t, _), _)| n == &s.name && t == ty) {
                    match member {
                        true => keys.insert(key.clone()),
                        false => keys.remove(key),
                    };
                }
            }
            for key in keys {
                staged.member(projection, &s.name, to, &key, true, step.at)?;
                staged.member(projection, &s.name, &s.of, &key, false, step.at)?;
            }
        }
        staged.steps.push(Step::Migrated(Box::new(step)));
        Ok(())
    }

    /// Fold one `value_type_migrated` event's step over every record of
    /// the types it renames in, retired and restricted ones included, as
    /// [`Self::step`] folds a migration's: each type walked once, a page
    /// of the view at a time, nothing of the walk kept, a record this
    /// transaction wrote stepped where it is kept, and a later read of any
    /// other taken through the step. Only a record the step changes is
    /// put; no key moves and no type.
    fn step_values(
        &self,
        view: &dyn ProjectionView,
        projection: &dyn lakelands_api::engine::ProjectionStore,
        staged: &mut Staged,
        step: Values,
    ) -> Result<()> {
        for ty in step.fields.keys() {
            walk(view, staged.known(ty), true, |row| {
                if staged.wrote(ty, &row.key) {
                    return Ok(());
                }
                let pre = staged.stepped(view, row)?;
                let post = step.apply(pre.clone());
                match post != pre {
                    true => projection.put_record(&post),
                    false => Ok(()),
                }
            })?;
            let written = (staged.records)
                .range_mut((ty.clone(), RecordKey::from(""))..)
                .take_while(|((t, _), _)| t == ty);
            for (_, r) in written {
                let post = step.apply(r.clone());
                if post != *r {
                    projection.put_record(&post)?;
                    *r = post;
                }
            }
        }
        staged.steps.push(Step::Values(Box::new(step)));
        Ok(())
    }

    /// Compute derived fields again for the `declared` events that are
    /// pending, each type walked once however many of them recompute it:
    /// every live record of the view the transaction did not write, as the
    /// kept steps leave it, and every one it wrote, where it is kept, goes
    /// through the pending steps in order. Each computes with its own
    /// entries and settings and the outputs recorded before its own event,
    /// and a record is put, and its vectors indexed, at each step that
    /// changes it: one state a step, as if each had walked at its event.
    /// A set of declarations is one step, at its last (`with_the_set`).
    /// Nothing of the view is kept, and what the transaction wrote stays
    /// kept, changed or not. A step whose event is at or past `owed`
    /// leaves a sweep owed of each type of whose records it made a call
    /// due, by step and type. A step that put nothing is dropped: a later read
    /// through it would change nothing either. Answers whether a step is
    /// kept.
    fn flush(
        &self,
        w: &mut Writer,
        view: &dyn ProjectionView,
        staged: &mut Staged,
        folded: &[Event],
        owed: Option<Position>,
    ) -> Result<bool> {
        let (false, Some(last)) = (staged.pending.is_empty(), folded.last()) else {
            return Ok(false);
        };
        let pending = std::mem::take(&mut staged.pending);
        let projection = self.engine.projection();
        let mut put = vec![false; pending.len()];
        let mut owing: BTreeSet<(usize, TypeName)> = BTreeSet::new();
        let mut through = |r: &mut StoredRecord, recorded: &Recorded| {
            for (i, (s, put)) in pending.iter().zip(&mut put).enumerate() {
                let owes = owed.is_some_and(|p| s.event.position >= p);
                let mut due: Vec<Due> = Vec::new();
                if let Some(next) = s.apply(r, view, recorded, owes.then_some(&mut due))? {
                    projection.put_record(&next)?;
                    self.index_vectors(&s.entries, &next)?;
                    (*r, *put) = (next, true);
                }
                if !due.is_empty() {
                    owing.insert((i, r.ty.clone()));
                }
            }
            Ok(())
        };
        let types = pending.iter().flat_map(|s| s.entries.types.keys());
        let types: BTreeSet<&TypeName> = types.collect();
        // The view holds a record under the type it had before the
        // transaction, and a kept step may have moved a type's records: so
        // each of the view's types is walked once, and a step computes a
        // row by the type it has now.
        let known: BTreeSet<&TypeName> = types.iter().map(|ty| staged.known(ty)).collect();
        for ty in known {
            walk(view, ty, false, |row| {
                // Tested before anything of the row is computed, by the
                // type and key the kept steps give it.
                if staged.wrote(ty, &row.key) {
                    return Ok(());
                }
                let mut r = staged.stepped(view, row)?;
                through(&mut r, &staged.outputs)
            })?;
        }
        for ty in types {
            let written = (staged.records)
                .range_mut((ty.clone(), RecordKey::from(""))..)
                .take_while(|((t, _), _)| t == ty);
            for (_, r) in written {
                through(r, &staged.outputs)?;
            }
        }
        for (i, ty) in owing {
            let event = &pending[i].event;
            let renames = matches!(event.body, EventBody::ValueTypeMigrated { .. });
            let sweep = Of::Sweep {
                after: None,
                retried: (!renames).then(BTreeSet::new),
            };
            w.owed.push(Owed::new(event, ty, sweep));
        }
        let kept = put.contains(&true);
        let steps = pending.into_iter().zip(put);
        staged
            .steps
            .extend(steps.filter_map(|(s, put)| put.then(|| Step::Recomputed(Box::new(s)))));
        kept::made(&kept_at!(w, view, projection), staged, last.position)?;
        Ok(kept)
    }

    /// Fold the log from `from` on, a page at a time, each page in its own
    /// transaction that commits `folded_through` at its last event, or in
    /// several, each committing at the last event it folded
    /// ([`Self::fold_all`]). A fold stops in one of two ways: reading a
    /// page of the log fails, or folding it does, and the open transaction
    /// rolls back. Either way the projection is folded through the last
    /// commit, the writer is `behind`, and the head names that position;
    /// nothing is appended until the next fold (the catch-up before a
    /// write, a `rebuild`, or the next open's) has continued from there.
    /// Every read is answered at or before `folded_through`, so no read
    /// sees half a transaction. A fold that reaches the tip has handed out
    /// the calls of an append whose fold had failed.
    fn fold_from(&self, w: &mut Writer, from: Position) -> Result<()> {
        let folded = (|| {
            for page in self.pages(from) {
                self.fold_all(w, &page?, Reads::default(), true)?;
            }
            Ok(())
        })();
        if folded.is_err() {
            // Whatever stopped it, a read of the log or a page's fold, the
            // projection is behind the log: nothing is appended until a
            // fold has caught up, and the head names the last commit, as
            // `head()` promises.
            w.behind = true;
            let through = self.engine.projection().folded_through().ok().flatten();
            self.set_head(through.map_or(0, |p| p.0));
        } else {
            w.failed = None;
        }
        folded
    }

    /// Fold the events the log holds past the projection, a page at a
    /// time, and publish the head once they are folded.
    ///
    /// The writer's ontology and settings are read at the mark first: what
    /// it holds may be what a failed fold left, and a fold compares a
    /// declaration with the one the writer holds. A writer that cannot
    /// read them stays behind.
    fn refold(&self, w: &mut Writer) -> Result<()> {
        let folded = self.engine.projection().folded_through()?;
        {
            let view = self.engine.view(None)?;
            w.ontology = Ontology::at(view.as_ref())?.named();
            w.settings = settings_in(view.as_ref(), &w.ontology)?;
        }
        let from = folded.map_or(Position::GENESIS, Position::next);
        self.fold_from(w, from)?;
        w.behind = false;
        self.publish(w);
        Ok(())
    }

    /// What one event does to the projection; the calls it makes due are
    /// handed out when it `owes` them.
    fn fold(
        &self,
        w: &mut Writer,
        view: &dyn ProjectionView,
        staged: &mut Staged,
        e: &Event,
        owes: bool,
    ) -> Result<()> {
        let opened;
        // What a forgotten source sealed is erased: its key is destroyed,
        // and the log says it was forgotten. A key gone without that is
        // lost, and fails closed (058).
        let (mut erased, mut dropped) = (Vec::new(), false);
        let source = (e.sealed.as_ref()).and_then(|s| s.source.as_ref().or(e.source.as_ref()));
        // The index keeps the source as the log names it, before an erasure
        // takes the seal away: an ingest's is its envelope's, as forgetting
        // walks it, and any other event's the one its seal names first.
        let indexed = match &e.body {
            EventBody::RecordIngested { .. } => e.source.clone(),
            _ => source.or(e.source.as_ref()).cloned(),
        };
        let e = match (&e.sealed, source) {
            (Some(sealed), Some(source)) => {
                let mut plain = e.clone();
                // With no key store at all, as an import made without one,
                // a value the log forgot is erased, and any other refused
                // as unbound rather than lost.
                let key = match self.keys.opened(source, &sealed.key) {
                    Ok(Some(key)) => Ok(key),
                    Ok(None) => {
                        let (source, key) = (source.clone(), sealed.key.clone());
                        Err(Error::unavailable(Situation::KeyMissing { source, key }))
                    }
                    Err(e @ Error::Unbound { .. }) => Err(e),
                    Err(e) => return Err(e),
                };
                match key {
                    Ok(key) => crate::sealing::open(&mut plain, &key)?,
                    Err(_) if self.forgot(w, view, staged, source, e.position)? => {
                        dropped = matches!(plain.body, EventBody::OutputRecorded { .. });
                        erased = crate::sealing::erase(&mut plain);
                    }
                    Err(lost) => return Err(lost),
                }
                opened = plain;
                &opened
            }
            _ => e,
        };
        // What an act of removal selects is erased too, sealed or not (065).
        let taken;
        let e = match crate::remove::redacted(&w.removals, &w.ontology, e) {
            Some((plain, cleared)) => {
                erased.extend(cleared);
                taken = plain;
                &taken
            }
            None => e,
        };
        let projection = self.engine.projection();
        // The versions of each source's key the log sealed with, each with
        // the position of its first use ([`crate::sealing::USED`]).
        if let (EventBody::RecordIngested { .. }, Some(sealed), Some(source)) =
            (&e.body, &e.sealed, &e.source)
        {
            let used = (source, &sealed.key, e.position, e.known_at);
            let read = (&staged.outputs, view, &*self.keys);
            if let Some((at, list)) = crate::sealing::first_use(read, used)? {
                projection.put_output(&at.0, &at.1, &list, e.position)?;
                staged
                    .outputs
                    .entry(at)
                    .or_default()
                    .push((e.position, list));
            }
        }
        // A name of the kept form, and a seam that begins `sys:`, are the
        // store's own, and a view's records are computed: no door declares,
        // writes or records under the one, nor writes the other, so only a
        // log another build wrote names them, and such a log is not folded.
        if let Some(flaw) = foreign(&w.ontology, &e.body) {
            return Err(damaged(Part::Log, Some(e.position), flaw));
        }
        projection.mark_applied(&lakelands_api::engine::Applied {
            id: e.id.clone(),
            at: e.position,
            kind: e.body.kind(),
            about: lakelands_core::feed::about(&e.body),
            source: indexed,
        })?;
        match &e.body {
            EventBody::LakeCreated { .. }
            | EventBody::LakeUpgraded { .. }
            | EventBody::LeaseTaken { .. }
            | EventBody::Checkpoint { .. }
            | EventBody::VerbCalled { .. }
            | EventBody::TriggerFired { .. } => {}
            // The act is owed until a rebuild after its keys were
            // destroyed has erased what they sealed, and the reclaim ran.
            EventBody::SourceForgotten { source, .. } => {
                w.forgotten.insert(source.clone());
                let erased = w.rebuilding && self.keys.ids(source)?.is_empty();
                let mut list = forgotten_list(view, staged)?;
                let state = if erased { "rebuilt" } else { "owed" };
                list.insert(source.clone(), state.to_owned());
                let (seam, at) = (crate::sealing::FORGOTTEN, crate::sealing::forgotten_at());
                let list = serde_json::to_value(&list).expect("a list serialises");
                projection.put_output(seam, &at, &list, e.position)?;
                staged
                    .outputs
                    .entry((seam.to_owned(), at))
                    .or_default()
                    .push((e.position, list));
            }
            // The act is owed until a rebuild that knew it before it began
            // has folded the log, and the reclaim ran (065).
            EventBody::ValuesRemoved { .. } => {
                let act = crate::remove::Removal::of(e).expect("the event is an act");
                let known = w.removals.contains(&act);
                if !known {
                    w.removals.push(act);
                }
                let mut list = crate::remove::removed_list(view, staged)?;
                let state = if known && w.rebuilding {
                    "rebuilt"
                } else {
                    "owed"
                };
                list.insert(e.position, state.to_owned());
                let (seam, at) = (crate::remove::REMOVED, crate::remove::removed_at());
                let list = crate::remove::list_value(&list);
                projection.put_output(seam, &at, &list, e.position)?;
                staged
                    .outputs
                    .entry((seam.to_owned(), at))
                    .or_default()
                    .push((e.position, list));
            }
            EventBody::Declared {
                declaration,
                schema_hash,
                version,
                with_the_set,
            } => {
                let rederive = computes_again(&w.ontology, declaration);
                w.ontology.put_at(
                    declaration.clone(),
                    *version,
                    schema_hash.clone(),
                    e.position,
                );
                projection.put_declaration(
                    declaration,
                    *version,
                    schema_hash.as_deref(),
                    e.position,
                )?;
                // A declaration of a set that goes on computes nothing yet:
                // what it would compute again is computed at the set's last
                // declaration, once, as that event leaves the ontology.
                staged.with_the_set.extend(rederive);
                let rederive = match with_the_set {
                    true => Vec::new(),
                    false => Vec::from_iter(std::mem::take(&mut staged.with_the_set)),
                };
                // A redeclared function, or revised derived expressions,
                // compute every live record's derived fields again, as of
                // this event: what that reads is kept as it stands now, and
                // the records are walked at the flush.
                staged.rederive(w, e, &rederive);
            }
            EventBody::Migrated {
                migration,
                schema_hash,
                version,
                rekeyed,
                ..
            } => {
                let before = w.ontology.ty(&migration.to.name)?.clone();
                let decl = Declaration::Type(migration.to.clone());
                // From format 15 on (lakelands#1070): a log of format 14
                // folds as it was written, and a rebuild answers as its lake did.
                let again = e.v >= 15 && computing_changed(&before, &migration.to);
                w.ontology.put_at(
                    decl.clone(),
                    *version,
                    Some(schema_hash.clone()),
                    e.position,
                );
                projection.put_declaration(&decl, *version, Some(schema_hash), e.position)?;
                // The lake's list of renames takes the fields this migration
                // renames, and each link type a renamed relation moves.
                let renamed = lakelands_core::links::renamed(migration, e.position);
                if !renamed.is_empty() {
                    let key = (RENAMES.to_owned(), Digest::zero());
                    let mut list = match staged.outputs.get(&key).and_then(|l| l.last()) {
                        Some((_, list)) => renames_in(Some(list.clone()))?,
                        None => renames_in(view.output(RENAMES, &key.1)?.map(|(_, l)| l))?,
                    };
                    list.extend(renamed);
                    let list = serde_json::json!(list);
                    projection.put_output(RENAMES, &key.1, &list, e.position)?;
                    // No step reads the renames: the last list is all that
                    // is kept of them.
                    staged.outputs.insert(key, vec![(e.position, list)]);
                }
                let step = Migrated::new(&w.ontology, e.position, before, migration, rekeyed)
                    .filling(&w.ontology, e);
                step.carry(&mut w.owed);
                self.step(w, view, projection, staged, step)?;
                // What reads a filled field is computed again, as of this
                // event, as a create computes it (050): queued as a value's
                // rename queues it.
                // So is a type whose computed fields it changes (lakelands#1070).
                let filled =
                    !migration.fill.is_empty() && computes(&w.ontology, &migration.to.name);
                if filled || again {
                    staged.rederive(w, e, std::slice::from_ref(&migration.to.name));
                }
            }
            EventBody::ValueTypeMigrated {
                declaration,
                version,
                renames,
                types,
                ..
            } => {
                // Only an imported or damaged log names other types than
                // those that use the value type: the writer named them.
                if using_types(&w.ontology, &declaration.name) != *types {
                    let value_type = declaration.name.to_string();
                    let flaw = Flaw::RenamesOtherTypes { value_type };
                    return Err(damaged(Part::Log, Some(e.position), flaw));
                }
                let decl = Declaration::ValueType(declaration.clone());
                w.ontology.put_at(decl.clone(), *version, None, e.position);
                projection.put_declaration(&decl, *version, None, e.position)?;
                let step = Values::new(&w.ontology, e.position, &declaration.name, renames);
                // What reads a renamed value is computed again, as of this
                // event, at the flush: a derived field, one computed by a
                // function, an embedding, whose calls are owed with this
                // event as their cause. Queued here, so that a kept view
                // over such a type is ended at this event.
                let rederive: Vec<TypeName> = (types.iter())
                    .filter(|t| computes(&w.ontology, t))
                    .cloned()
                    .collect();
                self.step_values(view, projection, staged, step)?;
                staged.rederive(w, e, &rederive);
            }
            EventBody::RecordUpserted { ty, key, .. }
            | EventBody::RecordIngested { ty, key, .. }
            | EventBody::RecordRetired { ty, key, .. }
            | EventBody::RecordRestricted { ty, key, .. }
            | EventBody::RecordUnrestricted { ty, key, .. } => {
                let before = staged.record(view, ty, key)?;
                let mut due = Vec::new();
                let mut outputs = Outputs {
                    view,
                    staged: Some(&staged.outputs),
                    before: None,
                    due: owes.then_some(&mut due),
                    waiting: false,
                    plans: &w.plans,
                    keys: Some(&self.keys),
                };
                let after =
                    record::apply(&w.ontology, before.as_ref(), e, &w.settings, &mut outputs)?;
                if !due.is_empty() {
                    w.owed
                        .push(Owed::new(e, ty.clone(), Of::Record(key.clone())));
                }
                if let Some(mut after) = after {
                    for f in &erased {
                        if let Some(m) = after.meta.get_mut(f) {
                            m.erased = true;
                        }
                    }
                    if ty.as_str() == SETTING_TYPE {
                        match after.fields.get("value").filter(|_| !after.retired) {
                            Some(v) => w.settings.insert(key.to_string(), v.clone()),
                            None => w.settings.remove(key.as_str()),
                        };
                    }
                    projection.put_record(&after)?;
                    self.index_vectors(&w.ontology, &after)?;
                    let before = before.as_ref();
                    kept::note(&kept_at!(w, view, projection), staged, before, &after)?;
                    staged.put(after);
                }
            }
            EventBody::WriteProposed {
                proposal,
                write,
                why,
                ..
            } => {
                let p = Proposal {
                    id: proposal.clone(),
                    position: e.position,
                    proposer: e.writer.clone(),
                    proposed_at: e.known_at,
                    write: Some(write.clone()),
                    why: why.clone(),
                    state: ProposalState::Pending,
                    decided_by: None,
                    reason: None,
                    more: false,
                };
                projection.put_proposal(&p, e.position)?;
                staged.proposals.insert(proposal.clone(), p);
            }
            EventBody::WriteAccepted { proposal } | EventBody::WriteRejected { proposal, .. } => {
                let mut p = match staged.proposals.get(proposal) {
                    Some(p) => p.clone(),
                    None => find_proposal(view, proposal)?,
                };
                p.decided_by = Some(e.writer.clone());
                match &e.body {
                    EventBody::WriteRejected { reason, .. } => {
                        p.state = ProposalState::Rejected;
                        p.reason = Some(reason.clone());
                    }
                    _ => p.state = ProposalState::Accepted,
                }
                // A decision on a proposed call is carried out after the
                // append, as the event that set it off made it due (064).
                // A closure by the system, caused by the output that decided
                // the call or by its record's retirement, leaves nothing owed.
                if owes
                    && e.origin != lakelands_api::record::Origin::System
                    && let Some(
                        write
                        @ (ProposedWrite::Compute { .. } | ProposedWrite::ComputeSweep { .. }),
                    ) = &p.write
                {
                    let proposed = self.read_events(p.position, 1)?;
                    let proposed = proposed.first().filter(|x| x.position == p.position);
                    let by = match proposed.and_then(|x| x.cause) {
                        Some(at) => self.read_events(at, 1)?.into_iter().next(),
                        None => proposed.cloned(),
                    };
                    if let Some(by) = by {
                        w.owed.extend(Owed::decided(e, write, &by));
                    }
                }
                projection.put_proposal(&p, e.position)?;
                staged.proposals.insert(proposal.clone(), p);
            }
            EventBody::ObjectSetChanged {
                set,
                ty,
                key,
                joined,
            } => staged.member(projection, set, ty, key, *joined, e.position)?,
            EventBody::OutputRecorded {
                seam,
                input,
                output,
                output_at,
                field,
            } => {
                // An output a forgotten source's key sealed is erased, and
                // a fill that names it fills nothing.
                let held;
                let output = match (output, output_at) {
                    _ if dropped => None,
                    (Some(output), None) => {
                        projection.put_output(seam, input, output, e.position)?;
                        let recorded = staged.outputs.entry((seam.clone(), input.clone()));
                        recorded.or_default().push((e.position, output.clone()));
                        Some(output)
                    }
                    (None, Some(at)) => match named(view, &staged.outputs, seam, input, *at, e) {
                        Ok(named) => {
                            held = named;
                            Some(&held)
                        }
                        Err(_) if self.erased_output(w, view, staged, *at)? => None,
                        Err(e) => return Err(e),
                    },
                    _ => return Err(damaged(Part::Log, Some(e.position), Flaw::OutputOnce)),
                };
                if let (Some(output), Some((ty, key, name))) = (output, field)
                    && let Some(r) = staged.record(view, ty, key)?
                {
                    let mut due = Vec::new();
                    let mut outputs = Outputs {
                        view,
                        staged: Some(&staged.outputs),
                        before: None,
                        due: owes.then_some(&mut due),
                        waiting: false,
                        plans: &w.plans,
                        keys: Some(&self.keys),
                    };
                    let next = record::fill(
                        &w.ontology,
                        &r,
                        name,
                        seam,
                        input,
                        output,
                        e,
                        &w.settings,
                        &mut outputs,
                    )?;
                    if !due.is_empty() {
                        w.owed
                            .push(Owed::new(e, ty.clone(), Of::Record(key.clone())));
                    }
                    if let Some(next) = next {
                        projection.put_record(&next)?;
                        self.index_vectors(&w.ontology, &next)?;
                        kept::note(&kept_at!(w, view, projection), staged, Some(&r), &next)?;
                        staged.put(next);
                    }
                }
            }
        }
        kept::settle(&kept_at!(w, view, projection), staged, e)
    }

    /// Whether the log forgot `source` (058): its list says so, or an act
    /// from `from` on, which a rebuild meets after the values it erases.
    /// A source once forgotten stays so.
    fn forgot(
        &self,
        w: &mut Writer,
        view: &dyn ProjectionView,
        staged: &Staged,
        source: &SourceId,
        from: Position,
    ) -> Result<bool> {
        if w.forgotten.contains(source) || forgotten_list(view, staged)?.contains_key(source) {
            return Ok(true);
        }
        for page in self.pages(from) {
            for e in page? {
                if matches!(&e.body, EventBody::SourceForgotten { source: s, .. } if s == source) {
                    w.forgotten.insert(source.clone());
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Whether the output recorded at `at` was erased: sealed under the
    /// key of a source the log forgot.
    fn erased_output(
        &self,
        w: &mut Writer,
        view: &dyn ProjectionView,
        staged: &Staged,
        at: Position,
    ) -> Result<bool> {
        let Some(e) = self.read_events(at, 1)?.pop().filter(|e| e.position == at) else {
            return Ok(false);
        };
        let sealed = e
            .sealed
            .as_ref()
            .filter(|_| matches!(e.body, EventBody::OutputRecorded { .. }));
        let Some((sealed, source)) = sealed.and_then(|s| Some((s, s.source.as_ref()?))) else {
            return Ok(false);
        };
        Ok(self.keys.opened(source, &sealed.key)?.is_none()
            && self.forgot(w, view, staged, source, at)?)
    }

    /// Keep the engine's vector index in step with a record's vector
    /// fields: a live record's vectors are indexed, a retired or cleared
    /// one's removed.
    fn index_vectors(&self, ontology: &Ontology, r: &StoredRecord) -> Result<()> {
        self.index_vectors_as(ontology, &r.ty, r)
    }

    /// Keep the vector index in step with a record a migration's step
    /// reaches, of the migrated type or of one whose keys a re-key moves,
    /// its declaration `before` the migration: at its old key, the vectors of
    /// each field the declaration now in force does not hold are taken out,
    /// and every one where the key is `left` with no record moving to it;
    /// then its vectors are indexed by that declaration, but for a field it
    /// held as it holds it, under the same key, which is indexed already. A
    /// field renamed is ranked under its new name, and one removed by no
    /// name (lakelands#768).
    fn reindex(
        &self,
        ontology: &Ontology,
        before: &TypeDecl,
        pre: &StoredRecord,
        post: &StoredRecord,
        left: bool,
    ) -> Result<()> {
        let Some(index) = self.engine.vectors() else {
            return Ok(());
        };
        let now = ontology.ty(&post.ty).ok();
        let vector = |f: &&FieldDecl| f.ty.kind == StorageKind::Vector;
        for f in before.fields.iter().filter(vector) {
            let kept =
                now.is_some_and(|d| d.fields.iter().filter(vector).any(|g| g.name == f.name));
            if !kept || left {
                index.remove(&pre.ty, &pre.key, f.name.as_str())?;
            }
        }
        let had = |f: &FieldDecl| {
            before
                .fields
                .iter()
                .filter(vector)
                .any(|g| g.name == f.name)
        };
        let same = |f: &FieldDecl| {
            pre.key == post.key && had(f) && pre.fields.get(&f.name) == post.fields.get(&f.name)
        };
        self.index_vectors_where(ontology, &post.ty, post, |f| !same(f))
    }

    /// [`Self::index_vectors`] for a record whose vector fields are those
    /// `declared` declares: a link moved to a link type that is declared
    /// only by the next event has the fields of the one it came from.
    fn index_vectors_as(
        &self,
        ontology: &Ontology,
        declared: &TypeName,
        r: &StoredRecord,
    ) -> Result<()> {
        self.index_vectors_where(ontology, declared, r, |_| true)
    }

    /// [`Self::index_vectors_as`] for the vector fields `due` takes.
    fn index_vectors_where(
        &self,
        ontology: &Ontology,
        declared: &TypeName,
        r: &StoredRecord,
        due: impl Fn(&FieldDecl) -> bool,
    ) -> Result<()> {
        let (Some(index), Ok(decl)) = (self.engine.vectors(), ontology.ty(declared)) else {
            return Ok(());
        };
        for f in (decl.fields.iter()).filter(|f| f.ty.kind == StorageKind::Vector && due(f)) {
            let vector: Option<Vec<f32>> = r
                .fields
                .get(&f.name)
                .and_then(Value::as_array)
                .filter(|_| !r.retired)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_f64)
                        .map(|x| x as f32)
                        .collect()
                });
            match vector {
                Some(v) => index.put(&r.ty, &r.key, f.name.as_str(), &v)?,
                None => index.remove(&r.ty, &r.key, f.name.as_str())?,
            }
        }
        Ok(())
    }

    /// Clear the projection and fold the whole log again. Only the lease's
    /// holder does: a handle without it would clear under the holder's fold,
    /// and a lost one takes the lease again first, as a write does.
    pub fn rebuild(&self) -> Result<Position> {
        let mut w = self.writer();
        self.regain(&mut w)?;
        if w.lease.is_none() {
            return Err(no_lease());
        }
        self.rebuild_held(&mut w)?;
        self.finish_forgetting(&mut w)?;
        Ok(w.tip.as_ref().map_or(Position::GENESIS, |t| t.0))
    }

    /// [`Self::rebuild`], under the writer's lock its caller holds.
    fn rebuild_held(&self, w: &mut Writer) -> Result<()> {
        // No shape is kept from here until the fold is whole again, and no
        // list of a source's versions.
        // The acts of removal the projection lists are told to the fold
        // before it meets the values they select (065). A list it cannot
        // read tells none: the fold meets each act unknown, and leaves it
        // owed for the rebuild after, which knows it.
        let view = self.engine.view(None);
        let listed = view.and_then(|v| crate::remove::acts(self, v.as_ref()));
        for act in listed.unwrap_or_default() {
            if !w.removals.contains(&act) {
                w.removals.push(act);
            }
        }
        self.rebuilds.fetch_add(1, Ordering::SeqCst);
        lakelands_core::lock(&self.shapes).clear();
        self.keys.unlist();
        let folded = (|| {
            self.engine.projection().clear()?;
            w.ontology = Ontology::default().named();
            w.settings = BTreeMap::new();
            // A page at a time: a rebuild that stops is continued by the
            // next fold from where it committed, as a failed fold is.
            // It reads the recorded outputs and makes no call due but a
            // failed append's; the calls a write before it made due stay
            // that write's.
            w.rebuilding = true;
            let folded = self.fold_from(w, Position::GENESIS);
            w.rebuilding = false;
            folded
        })();
        self.rebuilds.fetch_add(1, Ordering::SeqCst);
        lakelands_core::lock(&self.shapes).clear();
        self.keys.unlist();
        folded?;
        // Everything the log holds is folded again: the head is its tip.
        self.publish(w);
        Ok(())
    }

    /// Finish what a forgetting (058) or a removal (065) left owed, as the
    /// act itself does and the writer's open after a crash between its
    /// steps: each source whose act the projection may still hold the
    /// values of has its keys destroyed, the projection is rebuilt, and the
    /// storage reclaimed. Each step is idempotent. `None` where nothing was
    /// owed.
    pub(crate) fn finish_forgetting(
        &self,
        w: &mut Writer,
    ) -> Result<Option<lakelands_api::write::CompactReport>> {
        let listed = |lake: &Lake| {
            let view = lake.engine.view(None)?;
            forgotten_list(view.as_ref(), &Staged::default())
        };
        let removed = |lake: &Lake| {
            let view = lake.engine.view(None)?;
            crate::remove::removed_list(view.as_ref(), &Staged::default())
        };
        let (list, acts) = (listed(self)?, removed(self)?);
        if list.values().chain(acts.values()).all(|s| s == "done") {
            return Ok(None);
        }
        if list.values().chain(acts.values()).any(|s| s == "owed") {
            for (source, _) in list.iter().filter(|(_, s)| *s == "owed") {
                self.keys.destroy(source)?;
            }
            self.rebuild_held(w)?;
        }
        let (mut list, mut acts) = (listed(self)?, removed(self)?);
        let reclaimed = self.engine.projection().compact();
        *lakelands_core::lock(&self.reclaim_owed) = reclaimed.as_ref().err().map(Error::to_string);
        let reclaimed = reclaimed?;
        let states = list.values_mut().chain(acts.values_mut());
        for state in states.filter(|s| *s == "rebuilt") {
            *state = "done".to_owned();
        }
        let head = self.head();
        let projection = self.engine.projection();
        let list = serde_json::to_value(&list).expect("a list serialises");
        let at = crate::sealing::forgotten_at();
        projection.begin()?;
        if !list.as_object().is_some_and(serde_json::Map::is_empty) {
            projection.put_output(crate::sealing::FORGOTTEN, &at, &list, head)?;
        }
        if !acts.is_empty() {
            let (at, acts) = (
                crate::remove::removed_at(),
                crate::remove::list_value(&acts),
            );
            projection.put_output(crate::remove::REMOVED, &at, &acts, head)?;
        }
        projection.commit(head)?;
        Ok(Some(lakelands_api::write::CompactReport {
            before: reclaimed.before,
            after: reclaimed.after,
            head,
        }))
    }

    /// Why a forgetting's reclaim is still owed, where the last one tried
    /// failed: the writer's open goes on without it (lakelands#1161).
    pub fn reclaim_owed(&self) -> Option<String> {
        lakelands_core::lock(&self.reclaim_owed).clone()
    }

    /// Why the writer's open rebuilt the projection, where it did.
    pub fn rebuilt_at_open(&self) -> Option<String> {
        self.rebuilt_at_open.clone()
    }

    /// Check the hash chain between two positions, every checkpoint in
    /// that range against the segments the engine keeps, that each
    /// segment after the first begins with one, and that no event holds a
    /// number written as an integer beyond ±(2^53 − 1), which the seal
    /// lets none write and another reader would hash differently.
    pub fn verify(
        &self,
        from: Option<Position>,
        through: Option<Position>,
    ) -> Result<lakelands_api::Verification> {
        let from = from.unwrap_or(Position::GENESIS);
        let prev = if from == Position::GENESIS {
            None
        } else {
            self.read_events(Position(from.0 - 1), 1)?.pop()
        };
        let mut before = prev.as_ref().map(|e| (e.position, e.hash.clone()));
        let mut formats = lakelands_core::log::FormatSteps::after(prev.as_ref());
        let mut fence = prev.as_ref().map(|e| e.fence);
        let mut known = prev.map(|e| e.known_at);
        let segments = self.engine.segments()?;
        // Where a segment after the first begins: behind each sealed one.
        let begins: BTreeSet<Position> = segments.iter().map(|s| s.last.next()).collect();
        let mut last = None;
        // Page by page, never past `through`: an event after it is not
        // read, so damage there does not answer for the range asked
        // (lakelands#1156).
        let mut at = from;
        while through.is_none_or(|t| at <= t) {
            let left = through.map_or(u64::from(PAGE), |t| (t.0 - at.0).saturating_add(1));
            let page = self.read_events(at, left.min(u64::from(PAGE)) as u32)?;
            let Some(next) = page.last().map(|e| e.position.next()) else {
                break;
            };
            at = next;
            for e in page {
                log::check_link(before.as_ref().map(|(p, h)| (p, h)), &e)?;
                // No number written as an integer beyond ±(2^53 − 1)
                // (`docs/log-format.md` §1.2). The seal lets none in; a
                // log that holds one was not written through it.
                let written = serde_json::to_value(&e).expect("an event serialises to JSON");
                if let Some(n) = lakelands_core::canonical::integer_too_large(&written) {
                    let flaw = Flaw::IntegerTooLarge { n: n.clone() };
                    return Err(damaged(Part::Log, Some(e.position), flaw));
                }
                // `known_at` never goes back (`docs/log-format.md` §1.3).
                if let Some(k) = known
                    && e.known_at < k
                {
                    let (known_at, before) = (e.known_at, k);
                    let flaw = Flaw::KnownAtBack { known_at, before };
                    return Err(damaged(Part::Log, Some(e.position), flaw));
                }
                known = Some(e.known_at);
                // A lake's format changes only where a new writer upgrades
                // it, and never falls (054).
                if let Some(flaw) = formats.next(&e) {
                    return Err(damaged(Part::Log, Some(e.position), flaw));
                }
                // Fences never fall (`docs/log-format.md` §4): the engines
                // refuse a fall at append and at import, and this finds one
                // written below them.
                if let Some(floor) = fence.filter(|f| e.fence < *f) {
                    let flaw = Flaw::FenceBelow {
                        fence: e.fence,
                        floor,
                    };
                    return Err(damaged(Part::Log, Some(e.position), flaw));
                }
                fence = Some(e.fence);
                if let EventBody::Checkpoint {
                    through,
                    hash,
                    segments: listed,
                } = &e.body
                {
                    check_checkpoint(&e, *through, hash, listed)?;
                    check_segments(listed, &segments, e.position)?;
                } else if begins.contains(&e.position) {
                    return Err(damaged(Part::Log, Some(e.position), Flaw::NoCheckpoint));
                }
                before = Some((e.position, e.hash.clone()));
                last = Some(e);
            }
        }
        let last =
            last.ok_or_else(|| crate::writes::refused(Refusal::OutOfRange { head: self.head() }))?;
        // A writer's `lease_taken` that raised the format at the head, with
        // no `lake_upgraded` after it.
        if let Some(flaw) = formats.unmarked().filter(|_| last.position == self.head()) {
            return Err(damaged(Part::Log, Some(last.position), flaw));
        }
        Ok(lakelands_api::Verification {
            from,
            through: last.position,
            hash: last.hash,
        })
    }

    /// The last position no later than the head whose event's `known_at`
    /// is at or before `t`; none before the lake's first event. A binary
    /// search, reading one event per probe: `known_at` never decreases
    /// along the log (`docs/log-format.md` §1.3).
    pub fn position_at(&self, t: Timestamp) -> Result<Option<Position>> {
        let head = self.head();
        let known_at = |p: u64| -> Result<Timestamp> {
            self.read_events(Position(p), 1)?
                .first()
                .map(|e| e.known_at)
                .ok_or_else(|| {
                    let _ = head;
                    let flaw = Flaw::EndsBefore {
                        position: Position(p),
                    };
                    damaged(Part::Log, None, flaw)
                })
        };
        if known_at(0)? > t {
            return Ok(None);
        }
        // `lo` is always a position at or before `t`.
        let (mut lo, mut hi) = (0, head.0);
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            if known_at(mid)? <= t {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        Ok(Some(Position(lo)))
    }

    /// For each embedding field of `ty` at the head: the live records with
    /// a vector, from the engine's index, and those without one whose
    /// `embed.from` fields hold text, counted over pages of the records
    /// the engine selects by `<field> is null`, restricted ones included.
    pub fn vector_coverage(
        &self,
        ty: &TypeName,
    ) -> Result<Vec<lakelands_api::read::VectorCoverage>> {
        let view = self.engine.view(None)?;
        let decl = Ontology::at(view.as_ref())?.ty(ty)?.clone();
        let mut out = Vec::new();
        for f in &decl.fields {
            let Some(embed) = &f.embed else {
                continue;
            };
            let embedded = match self.engine.vectors() {
                Some(index) => index.count(ty, f.name.as_str())?,
                None => 0,
            };
            let mut scan = lakelands_api::engine::Scan {
                filter: Some(lakelands_api::expr::Expr::IsNull {
                    arg: Box::new(lakelands_api::expr::Expr::Path {
                        path: vec![f.name.clone()],
                    }),
                }),
                limit: PAGE,
                restricted: true,
                ..Default::default()
            };
            let mut due = 0;
            loop {
                let rows = view.scan(ty, &scan)?;
                due += rows
                    .iter()
                    .filter(|r| crate::backed::embed_text(embed, &r.fields).is_some())
                    .count() as u64;
                match rows.last() {
                    Some(last) if rows.len() == PAGE as usize => {
                        scan.after = Some(lakelands_api::engine::After {
                            values: vec![],
                            key: last.key.clone(),
                        });
                    }
                    _ => break,
                }
            }
            out.push(lakelands_api::read::VectorCoverage {
                field: f.name.clone(),
                embedded,
                due,
            });
        }
        Ok(out)
    }

    /// Seal the active segment and write the checkpoint that opens the
    /// next, as one step.
    pub fn checkpoint(&self) -> Result<Position> {
        // A segment is sealed only over events the projection has folded.
        let mut w = self.for_write()?;
        let lease = w.lease.clone().ok_or_else(read_only)?;
        let Some((through, hash)) = w.tip.clone() else {
            return Ok(Position::GENESIS);
        };
        let before = self.engine.segments()?;
        let now = self.clock(&mut w);
        // The engine closes the segment and begins the next with the
        // checkpoint in one step: one refused before it has sealed
        // nothing, and is sent again.
        let mut stamped = None;
        self.engine.seal_segment(&lease, false, &mut |sealed| {
            let segments = before.iter().chain([sealed]).cloned().collect();
            let body = EventBody::Checkpoint {
                through,
                hash: hash.clone(),
                segments,
            };
            let (_, events, stored) = self.stamp(&w, vec![Draft::system(now, body)])?;
            stamped = Some(events);
            Ok(stored)
        })?;
        let Some(events) = stamped else {
            return Ok(through);
        };
        let events = self.landed(&mut w, events, Reads::default())?;
        Ok(events.last().map_or(through, |e| e.position))
    }

    /// Give back the storage the projection no longer uses. It runs under
    /// the writer's lock, so a write waits and lands before or after it;
    /// it appends nothing, and the head stays where it is
    /// (`docs/decisions/040-maintenance-never-changes-an-answer.md`). A
    /// lost writer takes the lease again before it, as a write does: its
    /// `lease_taken` changes no answer.
    pub fn compact(&self) -> Result<lakelands_api::write::CompactReport> {
        let mut w = self.writer();
        self.regain(&mut w)?;
        if w.lease.is_none() {
            return Err(read_only());
        }
        let reclaimed = self.engine.projection().compact()?;
        Ok(lakelands_api::write::CompactReport {
            before: reclaimed.before,
            after: reclaimed.after,
            head: self.head(),
        })
    }

    /// Give the lease up. The handle reads on; it no longer writes.
    pub fn close(&self) -> Result<()> {
        let mut w = self.writer();
        if let Some(lease) = w.lease.take() {
            // The head stays the last folded position; only the lease goes.
            self.holds_lease.store(false, Ordering::Release);
            self.engine.release_lease(&lease)?;
        }
        Ok(())
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

/// A checkpoint's own claims (`docs/log-format.md` §2.6, §4.3): it follows
/// the segment it seals, repeats its last hash, and lists sealed ranges
/// that start at 0, follow each other with no gap or overlap, and end
/// where it says the sealed segment ends.
fn check_checkpoint(
    e: &Event,
    through: Position,
    hash: &Digest,
    listed: &[SegmentDigest],
) -> Result<()> {
    let at = e.position;
    let broken = |flaw: Flaw| damaged(Part::Checkpoint, Some(at), flaw);
    if e.position.0 != through.0 + 1 {
        return Err(broken(Flaw::CheckpointFollows { through }));
    }
    if *hash != e.prev {
        return Err(broken(Flaw::CheckpointHash));
    }
    let mut next = 0u64;
    for s in listed {
        if s.first.0 != next || s.last < s.first {
            let (first, last) = (s.first, s.last);
            return Err(broken(Flaw::CheckpointRange { first, last, next }));
        }
        next = s.last.0 + 1;
    }
    if next != through.0 + 1 {
        return Err(broken(Flaw::CheckpointEnd { through }));
    }
    Ok(())
}

fn check_segments(listed: &[SegmentDigest], kept: &[SegmentDigest], at: Position) -> Result<()> {
    for s in listed {
        let first = s.first;
        let flaw = match kept.iter().find(|k| k.first == s.first) {
            Some(k) if k == s => continue,
            Some(_) => Flaw::SegmentDiffers { first, at },
            None => Flaw::SegmentMissing { first, at },
        };
        return Err(damaged(Part::Segment, Some(first), flaw));
    }
    Ok(())
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
const PAGE: u32 = 1024;

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
fn read_only() -> Error {
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

/// One event a transaction folded, kept in place of the records it
/// changed: what it does to a record. A later read of a record the
/// transaction did not write takes the view's row through every step, in
/// the order of their events ([`Staged::record`]).
pub(crate) enum Step {
    /// A `migrated` event.
    Migrated(Box<Migrated>),
    /// A `declared` event that computed derived fields again and changed a
    /// record.
    Recomputed(Box<Recomputed>),
    /// A `value_type_migrated` event.
    Values(Box<Values>),
}

impl Step {
    /// A record, not an ended key, after the step. A recomputation reads
    /// the outputs in `recorded` that stand before its own event, makes no
    /// call due and indexes nothing: the event that reads writes the
    /// record, and indexes what it writes.
    fn apply(
        &self,
        r: StoredRecord,
        view: &dyn ProjectionView,
        recorded: &Recorded,
    ) -> Result<StoredRecord> {
        match self {
            Step::Migrated(s) => s.apply(r),
            Step::Recomputed(s) => Ok(s.apply(&r, view, recorded, None)?.unwrap_or(r)),
            Step::Values(s) => Ok(s.apply(r)),
        }
    }

    /// The migration, of a step that may move a key; a recomputation moves
    /// none.
    fn migrated(&self) -> Option<&Migrated> {
        match self {
            Step::Migrated(s) => Some(s),
            Step::Recomputed(_) | Step::Values(_) => None,
        }
    }
}

/// One `declared` event that computes derived fields again: all that the
/// computation reads, as it stood at the event, so that the flush's walk
/// and a later read through the step compute the same, whatever the writer
/// holds by then.
pub(crate) struct Recomputed {
    /// The event, whole: its position, and the provenance it gives every
    /// field it changes.
    event: Event,
    /// The types whose records it computes again, and what computing them
    /// reads of the ontology after the event ([`Ontology::computing`]):
    /// each entry shared with the writer's ontology and with the other
    /// steps that read it.
    entries: Ontology,
    /// The settings at the event.
    settings: Arc<BTreeMap<String, Value>>,
    /// The plans its records are computed by: its own, since a read
    /// computes through it without the writer.
    plans: record::Plans,
    /// The lake's keys, for a call that reads a source's erasable values.
    keys: Option<Arc<crate::sealing::Keys>>,
}

/// The types whose records `declaration`, over `o`, computes again: whose
/// fields name a function declared again, and a type in force declared
/// again with other computed fields (saying `write` where nothing was said
/// changes none) or with fields computed outside the lake, as an operator
/// retries a recorded failure (022, extended). The fold computes them again
/// and the declaration's append decides their sets (lakelands#941).
pub(crate) fn computes_again(o: &Ontology, declaration: &Declaration) -> Vec<TypeName> {
    match declaration {
        Declaration::Function(f) => (o.types.values())
            .filter(|t| {
                (t.decl.fields.iter()).any(|d| d.function.as_deref() == Some(f.name.as_str()))
            })
            .map(|t| t.decl.name.clone())
            .collect(),
        Declaration::Type(t) => match o.types.get(&t.name) {
            Some(old)
                if computing_changed(&old.decl, t) || crate::backed::computed_outside(o, t) =>
            {
                vec![t.name.clone()]
            }
            _ => vec![],
        },
        _ => vec![],
    }
}

/// The fields `declaration` computes again, per type: those naming a
/// function it declares; of a type it declares, those computed otherwise
/// than in force, and those computed outside the lake, which it settles
/// again (062). What reads them is not among them.
pub(crate) fn fields_again(o: &Ontology, declaration: &Declaration) -> Vec<(TypeName, FieldName)> {
    let computed = |f: &FieldDecl| f.derived.is_some() || f.function.is_some() || f.embed.is_some();
    let how = |f: &FieldDecl| {
        (
            f.derived.clone(),
            f.function.clone(),
            f.embed.clone(),
            f.at_read(),
        )
    };
    match declaration {
        Declaration::Function(fun) => (o.types.values())
            .flat_map(|t| {
                let named = |d: &&FieldDecl| d.function.as_deref() == Some(fun.name.as_str());
                (t.decl.fields.iter().filter(named)).map(|d| (t.decl.name.clone(), d.name.clone()))
            })
            .collect(),
        Declaration::Type(t) => {
            let Some(old) = o.types.get(&t.name) else {
                return vec![];
            };
            let outside = crate::backed::computed_outside(o, t);
            let backed = |n: &str| {
                o.functions
                    .get(n)
                    .is_some_and(|f| crate::backed::is_backed(&f.decl))
            };
            (t.fields.iter().filter(|f| computed(f)))
                .filter(|f| {
                    let was = old.decl.fields.iter().find(|x| x.name == f.name);
                    was.is_none_or(|was| how(was) != how(f))
                        || outside
                            && (f.embed.is_some() || f.function.as_deref().is_some_and(backed))
                })
                .map(|f| (t.name.clone(), f.name.clone()))
                .collect()
        }
        _ => vec![],
    }
}

/// Whether `new` computes its records otherwise than `old`: its derived
/// expressions, the functions its fields name, what its embeddings are made
/// from, or which are computed at the read (saying `write` where nothing
/// was said changes none). A migration that does computes them again
/// (lakelands#1070); a declaration does too, and settles a type computed
/// outside the lake again besides.
pub(crate) fn computing_changed(old: &TypeDecl, new: &TypeDecl) -> bool {
    let computes = |t: &TypeDecl| -> Vec<_> {
        (t.fields.iter())
            .filter(|f| f.derived.is_some() || f.function.is_some() || f.embed.is_some())
            .map(|f| {
                (
                    f.name.clone(),
                    f.derived.clone(),
                    f.function.clone(),
                    f.embed.clone(),
                    f.at_read(),
                )
            })
            .collect()
    };
    computes(old) != computes(new)
}

impl Recomputed {
    /// A record after the step when the step changes it: a live record of
    /// one of its types whose derived fields come out other than they are.
    /// A record of another type is as it was. The calls it makes due go to
    /// `due`.
    fn apply(
        &self,
        r: &StoredRecord,
        view: &dyn ProjectionView,
        recorded: &Recorded,
        due: Option<&mut Vec<Due>>,
    ) -> Result<Option<StoredRecord>> {
        if !self.entries.types.contains_key(&r.ty) {
            return Ok(None);
        }
        let mut outputs = Outputs {
            view,
            staged: Some(recorded),
            before: Some(self.event.position),
            due,
            waiting: false,
            plans: &self.plans,
            keys: self.keys.as_deref(),
        };
        record::rederive(&self.entries, r, &self.event, &self.settings, &mut outputs)
    }
}

/// One `value_type_migrated` event a transaction folded: what it does to
/// a record, kept in place of the records it changed, as [`Migrated`] is.
pub(crate) struct Values {
    /// The event's position.
    at: Position,
    /// Per type that uses the value type, the fields it renames in: every
    /// field of the value type but one computed outside the lake, whose
    /// answer the lake does not write.
    fields: BTreeMap<TypeName, Vec<FieldName>>,
    /// Old value to new value.
    renames: BTreeMap<String, String>,
}

impl Values {
    fn new(o: &Ontology, at: Position, vt: &ValueTypeName, renames: &[(String, String)]) -> Values {
        let fields = (o.types.values().map(|t| &t.decl))
            .filter(|t| t.view.is_none())
            .map(|t| (t.name.clone(), renamed_fields(o, t, vt)))
            .filter(|(_, fields)| !fields.is_empty())
            .collect();
        Values {
            at,
            fields,
            renames: renames.iter().cloned().collect(),
        }
    }

    /// A record after the step: each field it renames in that holds an
    /// old value holds the new one, at the step's position. Its version,
    /// its flags and each field's provenance are never touched, and a
    /// record holding no old value is as it was.
    fn apply(&self, mut r: StoredRecord) -> StoredRecord {
        let Some(fields) = self.fields.get(&r.ty) else {
            return r;
        };
        for f in fields {
            let new = r.fields.get(f).and_then(Value::as_str);
            if let Some(new) = new.and_then(|v| self.renames.get(v)) {
                r.fields.insert(f.clone(), Value::from(new.as_str()));
                r.position = self.at;
            }
        }
        r
    }
}

/// The types that use a value type, in name order: each, not a view,
/// with a field of it. A `value_type_migrated` names them, and a fold
/// refuses one that names others.
pub(crate) fn using_types(o: &Ontology, vt: &ValueTypeName) -> Vec<TypeName> {
    let uses = |t: &TypeDecl| (t.fields.iter()).any(|f| f.ty.value_type.as_ref() == Some(vt));
    let types = o.types.values().map(|t| &t.decl);
    types
        .filter(|t| t.view.is_none() && uses(t))
        .map(|t| t.name.clone())
        .collect()
}

/// The fields of `t` a value type's renames reach: every field of the
/// value type, derived ones and those computed by an expression included,
/// but one computed outside the lake.
pub(crate) fn renamed_fields(o: &Ontology, t: &TypeDecl, vt: &ValueTypeName) -> Vec<FieldName> {
    let backed = |n: &str| {
        o.functions
            .get(n)
            .is_some_and(|f| crate::backed::is_backed(&f.decl))
    };
    let of = |f: &&lakelands_api::ontology::FieldDecl| {
        f.ty.value_type.as_ref() == Some(vt) && !f.function.as_deref().is_some_and(backed)
    };
    t.fields.iter().filter(of).map(|f| f.name.clone()).collect()
}

/// Whether a record of `t` holds a field the lake computes: a derived
/// field, one computed by a function, an embedding.
pub(crate) fn computes(o: &Ontology, t: &TypeName) -> bool {
    let computed = |f: &FieldDecl| f.derived.is_some() || f.function.is_some() || f.embed.is_some();
    o.ty(t).is_ok_and(|t| t.fields.iter().any(computed))
}

/// One `migrated` event a transaction folded: what it does to a record,
/// kept in place of the records it changed. The fold's walk, the records
/// the transaction wrote and a later read all take a record through
/// [`Migrated::apply`], so the three cannot disagree.
pub(crate) struct Migrated {
    /// The event's position.
    at: Position,
    /// The migrated type's declaration before.
    before: TypeDecl,
    migration: Migration,
    /// The moves the event lists, per type: old key to new key.
    moves: crate::rekey::Moves,
    /// The same, new key to old.
    back: crate::rekey::Moves,
    /// The link types a renamed relation's records move between, old name
    /// to new: the type's whole, every key as it is (035, extended).
    types: BTreeMap<TypeName, TypeName>,
    /// For a step with moves, as declared once the migration is in force:
    /// every type with moves, and every type, not a view, that has a
    /// reference field to one.
    decls: BTreeMap<TypeName, TypeDecl>,
    /// What the migration fills (`Migration::fill`): each field's
    /// `initial` and its provenance at the step, worked out once.
    fills: Vec<(FieldName, Value, lakelands_api::record::FieldMeta)>,
}

impl Migrated {
    fn new(
        ontology: &Ontology,
        at: Position,
        before: TypeDecl,
        migration: &Migration,
        rekeyed: &[lakelands_api::write::Rekey],
    ) -> Migrated {
        let (mut moves, mut back) = (crate::rekey::Moves::new(), crate::rekey::Moves::new());
        for r in rekeyed {
            let ty = || r.ty.clone();
            moves
                .entry(ty())
                .or_default()
                .insert(r.from.clone(), r.to.clone());
            back.entry(ty())
                .or_default()
                .insert(r.to.clone(), r.from.clone());
        }
        let reaches = |d: &&TypeDecl| {
            moves.contains_key(&d.name)
                || d.view.is_none() && !crate::rekey::moved_references(d, &moves).is_empty()
        };
        let decls = (ontology.types.values().map(|t| &t.decl).filter(reaches))
            .map(|d| (d.name.clone(), d.clone()))
            .collect();
        let types = lakelands_core::links::moved(migration);
        Migrated {
            at,
            before,
            migration: migration.clone(),
            moves,
            back,
            types,
            decls,
            fills: Vec::new(),
        }
    }

    /// The step at `event`, with the values its migration fills, of
    /// origin `initial`, as a create fills them (050).
    fn filling(mut self, ontology: &Ontology, event: &Event) -> Migrated {
        let filled = |f: &&FieldDecl| self.migration.fill.contains(&f.name);
        let fills = (self.migration.to.fields.iter().filter(filled))
            .filter_map(|f| Some((f.name.clone(), record::initial_of(ontology, f)?)));
        let fills = fills
            .map(|(name, v)| (name, v, record::initial_meta(event)))
            .collect();
        self.fills = fills;
        self
    }

    /// A record, not an ended key, after the step. Of the migrated type:
    /// its fields converted, its per-field provenance carried over the
    /// renames for the fields it still has, at the step's position. Under
    /// a re-key: its references to moved keys rewritten and, when its own
    /// key moved, its new key; at the step's position when either changed
    /// it. Its version and its flags are never touched.
    fn apply(&self, mut r: StoredRecord) -> Result<StoredRecord> {
        if r.ty == self.migration.to.name {
            let cannot = || {
                let (ty, key) = (r.ty.clone(), r.key.clone());
                damaged(Part::Log, Some(self.at), Flaw::CannotConvert { ty, key })
            };
            let converted = shape::migrate_record(&self.before, &self.migration, &r.fields)
                .map_err(|_| cannot())?
                .ok_or_else(cannot)?;
            let renames = &self.migration.renames;
            let renamed = |old: FieldName| match renames.iter().find(|(from, _)| *from == old) {
                Some((_, to)) => to.clone(),
                None => old,
            };
            r.meta = (std::mem::take(&mut r.meta).into_iter())
                .map(|(old, meta)| (renamed(old), meta))
                .filter(|(new, _)| converted.contains_key(new))
                .collect();
            r.fields = converted;
            r.position = self.at;
            // A live record takes what the migration fills where it holds
            // no value; a retired one is history, and takes its `initial`
            // if it is written again (050).
            for (name, v, meta) in self.fills.iter().filter(|_| !r.retired) {
                if !r.fields.contains_key(name) {
                    r.fields.insert(name.clone(), v.clone());
                    r.meta.insert(name.clone(), meta.clone());
                }
            }
        }
        if let Some(decl) = self.decls.get(&r.ty) {
            let fields = crate::rekey::rewritten(decl, &r.fields, &self.moves);
            let to = self.moves.get(&r.ty).and_then(|m| m.get(&r.key));
            if to.is_some() || fields != r.fields {
                r.position = self.at;
            }
            r.key = to.cloned().unwrap_or(r.key);
            r.fields = fields;
        }
        if let Some(to) = self.types.get(&r.ty) {
            (r.ty, r.position) = (to.clone(), self.at);
        }
        Ok(r)
    }

    /// Carry what is owed over the step: a record's entry to its new key,
    /// an entry of a link type that moved to the type it moved to, and a
    /// sweep of a type whose keys moved back to its first key.
    fn carry(&self, owed: &mut [Owed]) {
        for o in owed {
            let (before, moves) = (o.clone(), self.moves.get(&o.ty));
            let restart = moves.is_some() && matches!(o.of, Of::Sweep { .. });
            match &mut o.of {
                Of::Record(key) => *key = moves.and_then(|m| m.get(key)).unwrap_or(key).clone(),
                Of::Sweep { after, .. } if restart => *after = None,
                Of::Sweep { .. } => {}
            }
            o.ty = self.types.get(&o.ty).unwrap_or(&o.ty).clone();
            if restart || *o != before {
                o.moved += 1;
            }
        }
    }

    /// Whether `key` is the new key of a move: a record arrives at it.
    fn arrives(&self, ty: &TypeName, key: &RecordKey) -> bool {
        self.back.get(ty).is_some_and(|m| m.contains_key(key))
    }

    /// Where the record of `ty` at `key` after the step was before it: the
    /// old key of the move that arrives there, else the key itself, under
    /// the type its records moved from, if they did; `None` for a key that
    /// ended at the step, which a record moved away from and none arrived
    /// at, as every key of a type whose records moved to another.
    fn source<'a>(
        &'a self,
        ty: &'a TypeName,
        key: &'a RecordKey,
    ) -> Option<(&'a TypeName, &'a RecordKey)> {
        if self.types.contains_key(ty) {
            return None;
        }
        match self.back.get(ty).and_then(|m| m.get(key)) {
            Some(old) => Some((ty, old)),
            None => (self.moved(ty, key) == key).then_some((self.origin(ty), key)),
        }
    }

    /// The type whose records the step moved to `ty`, else `ty` itself.
    fn origin<'a>(&'a self, ty: &'a TypeName) -> &'a TypeName {
        let from = self.types.iter().find(|(_, to)| *to == ty);
        from.map_or(ty, |(from, _)| from)
    }

    /// The key a record at `key` before the step has after it.
    fn moved<'a>(&'a self, ty: &TypeName, key: &'a RecordKey) -> &'a RecordKey {
        let to = self.moves.get(ty).and_then(|m| m.get(key));
        to.unwrap_or(key)
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

    /// Put a set membership, and remember it: the view a fold of several
    /// appends opened does not show it, and a re-key later in the page
    /// carries it to the record's new key.
    fn member(
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
