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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use lakelands_api::engine::{CanonicalEvent, LakeEngine, Lease, OpenMode, ProjectionView};
use lakelands_api::event::{Digest, Event, EventBody, LOG_FORMAT_BASE, SegmentDigest};
use lakelands_api::location::{EngineConfig, Limits, Location};
use lakelands_api::ontology::{StorageKind, TypeDecl};
use lakelands_api::principal::Renamed;
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::Seams;
use lakelands_api::write::{Declaration, Migration, Proposal, ProposalFilter, ProposalState};
use lakelands_api::{
    Error, FieldName, LakeId, Position, Principal, ProposalId, RecordKey, Refusal, Result,
    Timestamp, TypeName, Value, WriteContext,
};
use lakelands_core::{shape, unavailable};

use crate::backed::{Due, Outputs, Recorded};
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
    /// The calls of backed functions the appends made due and
    /// [`crate::backed::settle`] has not taken yet.
    pub due: Vec<Due>,
    /// A fold failed after its events were in the log: the projection is
    /// behind the log, and nothing is appended until it has caught up.
    pub behind: bool,
    /// The log format version the lake writes: the version of its last
    /// event, raised by an append that needs a newer one, never lowered.
    pub format: u32,
}

/// The seam name the lake's renames are recorded under, as one output: the
/// store's own, which no seam records under (`docs/seams.md`).
pub(crate) const RENAMES: &str = "sys:renames";

/// The lake's list of renames as its output holds it; empty where none is
/// recorded.
fn renames_in(recorded: Option<Value>) -> Result<Vec<Renamed>> {
    let list = recorded.map(serde_json::from_value).transpose();
    Ok(list
        .map_err(|e| unavailable(e.to_string()))?
        .unwrap_or_default())
}

/// One open lake.
pub struct Lake {
    /// The lake's name.
    pub id: LakeId,
    /// The engine keeping it.
    pub engine: Arc<dyn LakeEngine>,
    /// What the host plugged in.
    pub seams: Seams,
    /// Bounds on one call's work.
    pub limits: Limits,
    /// Held while the lake's due calls are made: one at a time per lake.
    pub calls: tokio::sync::Mutex<()>,
    /// How many times due calls were drained: a writer that reads it
    /// before its write knows, after, whether a neighbour made its calls.
    pub drains: AtomicU64,
    writer: Mutex<Writer>,
    /// The tip's position and whether the lease is held, as of the last
    /// change the writer made: read without the writer's lock, which an
    /// append holds across the engine's append and the whole fold.
    head: AtomicU64,
    /// The renames an open found where no fold had recorded them, for a
    /// handle that records nothing.
    walked: OnceLock<Vec<Renamed>>,
    holds_lease: AtomicBool,
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
        .ok_or_else(|| Error::Unbound {
            seam: format!("the engine `{name}` (no opener for it was passed in)"),
        })
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

/// The settings a view holds: the `value` of every live `sys.setting`
/// record, by key, a restricted one too. A restriction hides the record
/// from a reader; the value stays what the lake computes with, as the
/// fold keeps it, so a read, a writer opened again and a rebuild agree.
pub fn settings_in(view: &dyn ProjectionView) -> Result<BTreeMap<String, Value>> {
    let ty = TypeName::from(SETTING_TYPE);
    if !view
        .declarations()?
        .iter()
        .any(|d| matches!(&d.declaration, Declaration::Type(t) if t.name == ty))
    {
        return Ok(BTreeMap::new());
    }
    Ok(view
        .scan(&ty, &every(false))?
        .into_iter()
        .filter_map(|r| Some((r.key.to_string(), r.fields.get("value")?.clone())))
        .collect())
}

impl Lake {
    /// Open a lake: find its engine among the openers the host passed,
    /// take the lease unless opening read-only, create the lake if asked
    /// and absent, and fold whatever the log holds beyond the projection.
    pub fn open(location: &Location, seams: Seams, mode: OpenMode) -> Result<Lake> {
        let opener = opener(location, &seams)?;
        let engine: Arc<dyn LakeEngine> = Arc::from(opener.open(location, mode)?);
        let lease = match mode {
            OpenMode::Read => None,
            OpenMode::Write | OpenMode::Create => {
                Some(engine.take_lease(&format!("lakelands process {}", std::process::id()))?)
            }
        };
        let lake = Lake::assemble(location, seams, engine, lease.clone());
        lake.catch_up()?;
        if let Some(lease) = lease {
            let mut w = lake.writer();
            let now = lake.clock(&mut w);
            let mut drafts = Vec::new();
            if w.tip.is_none() {
                drafts.push(Draft::system(
                    now,
                    EventBody::LakeCreated {
                        format: LOG_FORMAT_BASE,
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
            types.retain(|t| !w.ontology.types.contains_key(&t.name));
            for t in types {
                let hash = shape::schema_hash(&t).map_err(|e| {
                    unavailable(format!(
                        "the system type `{}` has no schema hash: {e}",
                        t.name
                    ))
                })?;
                drafts.push(Draft::system(
                    now,
                    EventBody::Declared {
                        declaration: Declaration::Type(t),
                        schema_hash: Some(hash),
                        version: 1,
                    },
                ));
            }
            lake.append(&mut w, drafts)?;
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
        seams: Seams,
        engine: Arc<dyn LakeEngine>,
        lease: Option<Lease>,
    ) -> Lake {
        Lake {
            id: location.lake.clone(),
            engine,
            seams,
            limits: location.limits,
            calls: tokio::sync::Mutex::new(()),
            drains: AtomicU64::new(0),
            writer: Mutex::new(Writer {
                lease,
                tip: None,
                last_known_at: Timestamp(i64::MIN),
                ontology: Ontology::default(),
                settings: BTreeMap::new(),
                due: Vec::new(),
                behind: false,
                format: LOG_FORMAT_BASE,
            }),
            head: AtomicU64::new(0),
            walked: OnceLock::new(),
            holds_lease: AtomicBool::new(false),
        }
    }

    /// Make the writer's tip and lease readable without its lock.
    fn publish(&self, w: &Writer) {
        self.head
            .store(w.tip.as_ref().map_or(0, |t| t.0.0), Ordering::Release);
        self.holds_lease.store(w.lease.is_some(), Ordering::Release);
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
    /// folds nothing and is refused here, as its append would be.
    pub fn for_write(&self) -> Result<MutexGuard<'_, Writer>> {
        let mut w = self.writer();
        if w.behind {
            if w.lease.is_none() {
                return Err(unavailable(NO_LEASE));
            }
            self.refold(&mut w).map_err(|e| {
                unavailable(format!(
                    "the lake's projection is behind its log and could not catch up; nothing \
                     was appended ({})",
                    crate::backed::why(&e)
                ))
            })?;
        }
        Ok(w)
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
        let mut w = self.writer();
        w.tip = self.engine.tip()?;
        let folded = self.engine.projection().folded_through()?;
        {
            let view = self.engine.view(None)?;
            w.ontology = Ontology::at(view.as_ref())?;
            w.settings = settings_in(view.as_ref())?;
            // A lake made before the fold recorded renames: its list is
            // found once, and recorded by the handle that writes.
            let recorded = view.output(RENAMES, &Digest::zero())?.is_some();
            if let (false, Some(list), Some(at)) = (recorded, self.walk(view.as_ref())?, folded) {
                let p = self.engine.projection();
                if w.lease.is_none() {
                    let _ = self.walked.set(list);
                } else {
                    p.begin()?;
                    let put = p.put_output(RENAMES, &Digest::zero(), &serde_json::json!(list), at);
                    put.and_then(|()| p.commit(at))
                        .inspect_err(|_| p.rollback())?;
                }
            }
        }
        // The last event's time and format: versions only go up, so the
        // tip's is the log's newest.
        if let Some((pos, _)) = &w.tip
            && let Some(e) = self.read_events(*pos, 1)?.first()
        {
            w.last_known_at = e.known_at;
            w.format = w.format.max(e.v);
        }
        // Only the writer folds; a handle opened read-only reads what the
        // writer folded, and writes nothing.
        if w.lease.is_some() {
            let from = folded.map_or(Position::GENESIS, Position::next);
            self.fold_from(&mut w, from)?;
            // Opening a lake never calls a backing.
            w.due.clear();
            self.publish(&w);
        } else {
            // A read-only handle reads what the writer folded: its head is
            // the projection's position, which it can be read at.
            self.head
                .store(folded.map_or(0, |p| p.0), Ordering::Release);
            self.holds_lease.store(false, Ordering::Release);
        }
        Ok(())
    }

    /// The renames of a lake whose fold recorded none, from what the
    /// projection keeps (006, extended): a rename happens only in a
    /// `migrated` event, and a type's declaration is in force since the
    /// event that put it. So for each type past its first version the event
    /// at its `since` is read, then the type as of the position before it,
    /// down to its first version. `None` where no type is past its first.
    fn walk(&self, head: &dyn ProjectionView) -> Result<Option<Vec<Renamed>>> {
        let key = lakelands_core::engines::declaration_key;
        let mut list = Vec::new();
        let later = |d: &lakelands_api::engine::Declared| d.version > 1;
        let types = head.declarations()?.into_iter().filter(later);
        let types: Vec<_> = types
            .filter(|d| matches!(d.declaration, Declaration::Type(_)))
            .collect();
        for mut d in types.clone() {
            while d.version > 1 && d.since > Position::GENESIS {
                let event = self.read_events(d.since, 1)?.pop();
                if let Some(EventBody::Migrated { migration, .. }) = event.map(|e| e.body) {
                    list.extend(lakelands_core::links::renamed(&migration, d.since));
                }
                let before = self.engine.view(Some(Position(d.since.0 - 1)))?;
                let was = before
                    .declarations()?
                    .into_iter()
                    .find(|b| key(&b.declaration) == key(&d.declaration));
                let Some(was) = was else { break };
                d = was;
            }
        }
        list.sort_by_key(|r| r.at);
        Ok((!types.is_empty()).then_some(list))
    }

    /// The lake's list of renames: what the fold recorded, read from
    /// `head` or from a view of the head, or what this handle's open found.
    pub(crate) fn renames(&self, head: Option<&dyn ProjectionView>) -> Result<Vec<Renamed>> {
        let recorded = match head {
            Some(view) => view.output(RENAMES, &Digest::zero())?,
            None => self.engine.view(None)?.output(RENAMES, &Digest::zero())?,
        };
        match recorded {
            None => Ok(self.walked.get().cloned().unwrap_or_default()),
            recorded => renames_in(recorded),
        }
    }

    /// A principal with its grants held through the lake's renames
    /// (`Grants::follow`), for a read at `at` or a write at the head, and
    /// how many entries of the list it was followed through. The list is
    /// asked only for one whose grants name a type: a grant that is only
    /// a default follows a link type's rename too.
    pub(crate) fn followed(
        &self,
        mut who: Principal,
        at: Option<Position>,
        head: Option<&dyn ProjectionView>,
    ) -> Result<(Principal, Option<usize>)> {
        if who.grants.types.is_empty() {
            return Ok((who, None));
        }
        let list = self.renames(head)?;
        who.grants.follow(&list, at);
        Ok((who, Some(list.len())))
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
                out.push(log::open(stored)?);
            }
            at = Position(batch.last().expect("not empty").position.0 + 1);
            if out.len() >= limit as usize {
                break;
            }
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
        let Some(lease) = w.lease.clone() else {
            return Err(unavailable(NO_LEASE));
        };
        // Nothing is appended while the projection is behind the log, and
        // nothing is folded this late: the drafts were decided already.
        if w.behind {
            return Err(unavailable(
                "this write was decided while the lake's projection was behind its log; \
                 nothing was appended",
            ));
        }
        // A host's cause names an event the log holds: at or below the
        // head (021). What it names is the host's business.
        let head = w.tip.as_ref().map(|(p, _)| *p);
        if let Some(cause) = drafts
            .iter()
            .filter_map(|d| d.context.cause)
            .find(|c| Some(*c) > head)
        {
            return Err(lakelands_core::invalid(format!(
                "the cause {cause} is past the lake's head {}",
                head.map_or_else(|| "(none)".to_owned(), |h| h.to_string())
            )));
        }
        // A reaction that names a trigger's firing as its cause is bound by
        // that trigger's own ceiling too, as the head declares it (021).
        // It is read here, under the writer, so the firing the cause names
        // is the one this append follows, never one appended meanwhile.
        if let Some(d) = drafts.iter().find(|d| d.context.cause.is_some()) {
            self.check_trigger_ceiling(w, d.context)?;
        }
        // An event that needs a newer format raises the lake to it, from
        // this append on (`lakelands_core::versions`).
        let v = drafts
            .iter()
            .map(|d| lakelands_core::versions::needed(&d.body, d.upstream.is_some()))
            .fold(w.format, u32::max);
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
            let (event, canon) = log::seal_at(v, &self.id, position, &prev_hash, fence, d);
            prev_pos = Some(position);
            prev_hash = event.hash.clone();
            events.push(event);
            stored.push(canon);
        }
        self.engine.append(&lease, &stored)?;
        w.tip = prev_pos.map(|p| (p, prev_hash));
        w.format = v;
        if let Some(last) = events.last() {
            w.last_known_at = w.last_known_at.max(last.known_at);
        }
        // The events are durable whatever the fold answers: its failure is
        // told as that, so that it is not read as a write that did not land.
        self.fold_all(w, &events, reads, false).map_err(|e| {
            let at = events.first().map_or(0, |e| e.position.0);
            unavailable(format!(
                "what was appended is in the log from position {at} and is not folded yet; the \
                 next write folds it, and a write is sent again only under its idempotency key \
                 ({})",
                crate::backed::why(&e)
            ))
        })?;
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
        {
            return Err(Error::Refused {
                field: Some(trigger.as_str().into()),
                reason: Refusal::TooDeep { max: m },
            });
        }
        Ok(())
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
    fn fold_all(&self, w: &mut Writer, events: &[Event], reads: Reads, of_log: bool) -> Result<()> {
        let Some(last) = events.last() else {
            return Ok(());
        };
        let projection = self.engine.projection();
        let mut due = w.due.len();
        let mut staged = Staged {
            reads,
            ..Staged::default()
        };
        let result = (|| {
            // A `begin` that is refused leaves the writer behind like any
            // other failure of the fold: the events are in the log.
            projection.begin()?;
            let mut view = self.engine.view(None)?;
            for (i, e) in events.iter().enumerate() {
                if reads_a_record(&e.body) && self.flush(w, view.as_ref(), &mut staged)? && of_log {
                    drop(view);
                    projection.commit(events[i - 1].position)?;
                    // What is committed stays due, whatever fails next.
                    due = w.due.len();
                    projection.begin()?;
                    staged = Staged::default();
                    view = self.engine.view(None)?;
                }
                self.fold(w, view.as_ref(), &mut staged, e)?;
            }
            self.flush(w, view.as_ref(), &mut staged).map(drop)
        })()
        .and_then(|()| projection.commit(last.position));
        match result {
            Ok(()) => {
                w.behind = false;
                Ok(())
            }
            Err(e) => {
                // The projection is as its last commit left it; so is what
                // the writer read from it while folding.
                projection.rollback();
                w.behind = true;
                w.due.truncate(due);
                if let Ok(view) = self.engine.view(None) {
                    if let Ok(o) = Ontology::at(view.as_ref()) {
                        w.ontology = o;
                    }
                    if let Ok(s) = settings_in(view.as_ref()) {
                        w.settings = s;
                    }
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
                return Err(unavailable(format!(
                    "the migration at position {} moves `{ty}` `{from}`, which the lake does \
                     not hold",
                    step.at
                )));
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

    /// Compute derived fields again for the `declared` events that are
    /// pending, each type walked once however many of them recompute it:
    /// every live record of the view the transaction did not write, as the
    /// kept steps leave it, and every one it wrote, where it is kept, goes
    /// through the pending steps in order. Each computes with its own
    /// entries and settings and the outputs recorded before its own event,
    /// and a record is put, and its vectors indexed, at each step that
    /// changes it: one state a step, as if each had walked at its event.
    /// Nothing of the view is kept, and what the transaction wrote stays
    /// kept, changed or not. The calls made due come by step, type and key,
    /// a record's fields in the order they settle. A step that put nothing
    /// is dropped: a later read through it would change nothing either.
    /// Answers whether a step is kept.
    fn flush(
        &self,
        w: &mut Writer,
        view: &dyn ProjectionView,
        staged: &mut Staged,
    ) -> Result<bool> {
        if staged.pending.is_empty() {
            return Ok(false);
        }
        let pending = std::mem::take(&mut staged.pending);
        let projection = self.engine.projection();
        let (from, mut put) = (w.due.len(), vec![false; pending.len()]);
        let mut through = |r: &mut StoredRecord, recorded: &Recorded, due: &mut Vec<Due>| {
            for (s, put) in pending.iter().zip(&mut put) {
                if let Some(next) = s.apply(r, view, recorded, Some(due))? {
                    projection.put_record(&next)?;
                    self.index_vectors(&s.entries, &next)?;
                    (*r, *put) = (next, true);
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
                through(&mut r, &staged.outputs, &mut w.due)
            })?;
        }
        for ty in types {
            let written = (staged.records)
                .range_mut((ty.clone(), RecordKey::from(""))..)
                .take_while(|((t, _), _)| t == ty);
            for (_, r) in written {
                through(r, &staged.outputs, &mut w.due)?;
            }
        }
        w.due[from..].sort_by(|a, b| (a.cause, &a.ty, &a.key).cmp(&(b.cause, &b.ty, &b.key)));
        let kept = put.contains(&true);
        let steps = pending.into_iter().zip(put);
        staged
            .steps
            .extend(steps.filter_map(|(s, put)| put.then(|| Step::Recomputed(Box::new(s)))));
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
    /// sees half a transaction.
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
            self.head
                .store(through.map_or(0, |p| p.0), Ordering::Release);
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
            (w.ontology, w.settings) = (Ontology::at(view.as_ref())?, settings_in(view.as_ref())?);
        }
        let from = folded.map_or(Position::GENESIS, Position::next);
        self.fold_from(w, from)?;
        w.behind = false;
        self.publish(w);
        Ok(())
    }

    /// What one event does to the projection.
    fn fold(
        &self,
        w: &mut Writer,
        view: &dyn ProjectionView,
        staged: &mut Staged,
        e: &Event,
    ) -> Result<()> {
        let projection = self.engine.projection();
        projection.mark_applied(&e.id, e.position)?;
        match &e.body {
            EventBody::LakeCreated { .. }
            | EventBody::LeaseTaken { .. }
            | EventBody::Checkpoint { .. }
            | EventBody::VerbCalled { .. }
            | EventBody::TriggerFired { .. } => {}
            EventBody::Declared {
                declaration,
                schema_hash,
                version,
            } => {
                // What computes a type's derived fields: its derived
                // expressions, the functions its fields name and what its
                // embeddings are made from.
                let computes = |t: &lakelands_api::ontology::TypeDecl| -> Vec<_> {
                    t.fields
                        .iter()
                        .filter(|f| {
                            f.derived.is_some() || f.function.is_some() || f.embed.is_some()
                        })
                        .map(|f| {
                            (
                                f.name.clone(),
                                f.derived.clone(),
                                f.function.clone(),
                                f.embed.clone(),
                            )
                        })
                        .collect()
                };
                let rederive: Vec<TypeName> = match declaration {
                    Declaration::Function(f) => w
                        .ontology
                        .types
                        .values()
                        .filter(|t| {
                            t.decl
                                .fields
                                .iter()
                                .any(|d| d.function.as_deref() == Some(f.name.as_str()))
                        })
                        .map(|t| t.decl.name.clone())
                        .collect(),
                    // A type whose fields are computed outside the lake is
                    // settled again whenever it is declared again: that is
                    // how an operator retries a recorded failure (022,
                    // extended).
                    Declaration::Type(t) => match w.ontology.types.get(&t.name) {
                        Some(old)
                            if computes(&old.decl) != computes(t)
                                || crate::backed::computed_outside(&w.ontology, t) =>
                        {
                            vec![t.name.clone()]
                        }
                        _ => vec![],
                    },
                    _ => vec![],
                };
                w.ontology
                    .put(declaration.clone(), *version, schema_hash.clone());
                projection.put_declaration(
                    declaration,
                    *version,
                    schema_hash.as_deref(),
                    e.position,
                )?;
                // A redeclared function, or revised derived expressions,
                // compute every live record's derived fields again, as of
                // this event: what that reads is kept as it stands now, and
                // the records are walked at the flush. No record and no
                // setting changes before it, so the settings are one map
                // for all that are pending.
                if !rederive.is_empty() {
                    let settings = match staged.pending.last() {
                        Some(s) => s.settings.clone(),
                        None => Arc::new(w.settings.clone()),
                    };
                    staged.pending.push(Recomputed {
                        event: e.clone(),
                        entries: w.ontology.computing(&rederive),
                        settings,
                    });
                }
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
                w.ontology
                    .put(decl.clone(), *version, Some(schema_hash.clone()));
                projection.put_declaration(&decl, *version, Some(schema_hash), e.position)?;
                // The lake's list of renames takes the fields this migration
                // renames, and each link type a renamed relation moves.
                let renamed = lakelands_core::links::renamed(migration, e.position);
                if !renamed.is_empty() {
                    let key = (RENAMES.to_owned(), Digest::zero());
                    let mut list = match staged.outputs.get(&key).and_then(|l| l.last()) {
                        Some((_, list)) => renames_in(Some(list.clone()))?,
                        None => renames_in(view.output(RENAMES, &key.1)?)?,
                    };
                    list.extend(renamed);
                    let list = serde_json::json!(list);
                    projection.put_output(RENAMES, &key.1, &list, e.position)?;
                    // No step reads the renames: the last list is all that
                    // is kept of them.
                    staged.outputs.insert(key, vec![(e.position, list)]);
                }
                let step = Migrated::new(&w.ontology, e.position, before, migration, rekeyed);
                self.step(w, view, projection, staged, step)?;
            }
            EventBody::RecordUpserted { ty, key, .. }
            | EventBody::RecordIngested { ty, key, .. }
            | EventBody::RecordRetired { ty, key, .. }
            | EventBody::RecordRestricted { ty, key, .. }
            | EventBody::RecordUnrestricted { ty, key, .. } => {
                let before = staged.record(view, ty, key)?;
                let mut outputs = Outputs {
                    view,
                    staged: Some(&staged.outputs),
                    before: None,
                    due: Some(&mut w.due),
                };
                let after =
                    record::apply(&w.ontology, before.as_ref(), e, &w.settings, &mut outputs)?;
                if let Some(after) = after {
                    if ty.as_str() == SETTING_TYPE {
                        match after.fields.get("value").filter(|_| !after.retired) {
                            Some(v) => w.settings.insert(key.to_string(), v.clone()),
                            None => w.settings.remove(key.as_str()),
                        };
                    }
                    projection.put_record(&after)?;
                    self.index_vectors(&w.ontology, &after)?;
                    staged.put(after);
                }
            }
            EventBody::WriteProposed {
                proposal,
                write,
                why,
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
                field,
            } => {
                projection.put_output(seam, input, output, e.position)?;
                let recorded = staged.outputs.entry((seam.clone(), input.clone()));
                recorded.or_default().push((e.position, output.clone()));
                if let Some((ty, key, name)) = field
                    && let Some(r) = staged.record(view, ty, key)?
                {
                    let mut outputs = Outputs {
                        view,
                        staged: Some(&staged.outputs),
                        before: None,
                        due: Some(&mut w.due),
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
                    if let Some(next) = next {
                        projection.put_record(&next)?;
                        self.index_vectors(&w.ontology, &next)?;
                        staged.put(next);
                    }
                }
            }
        }
        Ok(())
    }

    /// Keep the engine's vector index in step with a record's vector
    /// fields: a live record's vectors are indexed, a retired or cleared
    /// one's removed.
    fn index_vectors(&self, ontology: &Ontology, r: &StoredRecord) -> Result<()> {
        self.index_vectors_as(ontology, &r.ty, r)
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
        let (Some(index), Ok(decl)) = (self.engine.vectors(), ontology.ty(declared)) else {
            return Ok(());
        };
        for f in decl
            .fields
            .iter()
            .filter(|f| f.ty.kind == StorageKind::Vector)
        {
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
    /// holder does: a handle without it would clear under the holder's fold.
    pub fn rebuild(&self) -> Result<Position> {
        let mut w = self.writer();
        if w.lease.is_none() {
            return Err(unavailable(NO_LEASE));
        }
        self.engine.projection().clear()?;
        w.ontology = Ontology::default();
        w.settings = BTreeMap::new();
        // A page at a time: a rebuild that stops is continued by the next
        // fold from where it committed, as a failed fold is.
        let folded = self.fold_from(&mut w, Position::GENESIS);
        // A rebuild reads the recorded outputs; it never calls a backing,
        // whether it finished or stopped.
        w.due.clear();
        folded?;
        // Everything the log holds is folded again: the head is its tip.
        self.publish(&w);
        Ok(w.tip.as_ref().map_or(Position::GENESIS, |t| t.0))
    }

    /// Check the hash chain between two positions, and every checkpoint in
    /// that range against the segments the engine keeps.
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
        let mut known = prev.map(|e| e.known_at);
        let segments = self.engine.segments()?;
        let mut last = None;
        let mut last_v = 0;
        'pages: for page in self.pages(from) {
            for e in page? {
                if through.is_some_and(|t| e.position > t) {
                    break 'pages;
                }
                log::check_link(before.as_ref().map(|(p, h)| (p, h)), &e)?;
                // Versions go only upward (`docs/log-format.md` §6).
                if e.v < last_v {
                    return Err(unavailable(format!(
                        "the log is damaged at position {}: it is written in format {} after \
                         an event in format {last_v}",
                        e.position, e.v
                    )));
                }
                last_v = e.v;
                // `known_at` never goes back (`docs/log-format.md` §1.3).
                if let Some(k) = known
                    && e.known_at < k
                {
                    return Err(unavailable(format!(
                        "the log is damaged at position {}: its known_at {} is before the \
                             {k} of the event before it",
                        e.position, e.known_at
                    )));
                }
                known = Some(e.known_at);
                if let EventBody::Checkpoint {
                    through,
                    hash,
                    segments: listed,
                } = &e.body
                {
                    check_checkpoint(&e, *through, hash, listed)?;
                    check_segments(listed, &segments, e.position)?;
                }
                before = Some((e.position, e.hash.clone()));
                last = Some(e);
            }
        }
        let last =
            last.ok_or_else(|| crate::writes::refused(Refusal::OutOfRange { head: self.head() }))?;
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
        let known_at =
            |p: u64| -> Result<Timestamp> {
                self.read_events(Position(p), 1)?
                .first()
                .map(|e| e.known_at)
                .ok_or_else(|| unavailable(format!(
                        "the log ends before position {p}, which is at or before the head {head}"
                    )))
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

    /// Seal the active segment and write the checkpoint that opens the next.
    pub fn checkpoint(&self) -> Result<Position> {
        // A segment is sealed only over events the projection has folded.
        let mut w = self.for_write()?;
        let lease = w
            .lease
            .clone()
            .ok_or_else(|| unavailable("this lake is open read-only here"))?;
        let Some((through, hash)) = w.tip.clone() else {
            return Ok(Position::GENESIS);
        };
        if self.engine.seal_segment(&lease)?.is_none() {
            return Ok(through);
        }
        let segments = self.engine.segments()?;
        let now = self.clock(&mut w);
        let events = self.append(
            &mut w,
            vec![Draft::system(
                now,
                EventBody::Checkpoint {
                    through,
                    hash,
                    segments,
                },
            )],
        )?;
        Ok(events.last().map_or(through, |e| e.position))
    }

    /// Give back the storage the projection no longer uses. It runs under
    /// the writer's lock, so a write waits and lands before or after it;
    /// it appends nothing, and the head stays where it is
    /// (`docs/decisions/040-maintenance-never-changes-an-answer.md`).
    pub fn compact(&self) -> Result<lakelands_api::write::CompactReport> {
        let w = self.writer();
        if w.lease.is_none() {
            return Err(unavailable("this lake is open read-only here"));
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
    let broken =
        |what: &str| unavailable(format!("the checkpoint at position {at} is broken: {what}"));
    if e.position.0 != through.0 + 1 {
        return Err(broken(&format!("it says it follows position {through}")));
    }
    if *hash != e.prev {
        return Err(broken("its hash is not the hash of the event before it"));
    }
    let mut next = 0u64;
    for s in listed {
        if s.first.0 != next || s.last < s.first {
            return Err(broken(&format!(
                "its segment list has the range {}..={} where one starting at {next} belongs",
                s.first, s.last
            )));
        }
        next = s.last.0 + 1;
    }
    if next != through.0 + 1 {
        return Err(broken(&format!(
            "its segment list does not end at position {through}"
        )));
    }
    Ok(())
}

fn check_segments(listed: &[SegmentDigest], kept: &[SegmentDigest], at: Position) -> Result<()> {
    for s in listed {
        let why = match kept.iter().find(|k| k.first == s.first) {
            Some(k) if k == s => continue,
            Some(_) => format!("differs from what the checkpoint at {at} recorded"),
            None => format!("is missing; the checkpoint at {at} records it"),
        };
        return Err(unavailable(format!(
            "the segment from position {} {why}",
            s.first
        )));
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

/// What a write is answered on a handle that does not hold the lease.
const NO_LEASE: &str = "this lake is open read-only here; another process holds its writer lease";

/// The log read a page at a time (`Lake::pages`): each item is up to
/// `PAGE` events, opened and checked; the iteration ends after the tip, or
/// after the first error.
pub struct Pages<'a> {
    lake: &'a Lake,
    at: Option<Position>,
}

impl Iterator for Pages<'_> {
    type Item = Result<Vec<Event>>;

    fn next(&mut self) -> Option<Result<Vec<Event>>> {
        let at = self.at.take()?;
        let page = match self.lake.read_events(at, PAGE) {
            Ok(page) => page,
            Err(e) => return Some(Err(e)),
        };
        self.at = Some(page.last()?.position.next());
        Some(Ok(page))
    }
}

/// Records read from the projection, each once: a record's state, or its
/// absence, by the view's position, its type and its key. A view at one
/// position always answers the same, so an answer kept is an answer read
/// again. The writer lock is held from the decision over them to the
/// fold, so the fold's view is at the same position as the decision's
/// unless something else was folded in between; then nothing kept is
/// used, since the position is part of the key.
#[derive(Default)]
pub struct Reads(RefCell<BTreeMap<ReadAt, Option<StoredRecord>>>);

/// A read of one record: the view's position, the type, the key.
type ReadAt = (Option<Position>, TypeName, RecordKey);

impl Reads {
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
        // Every record of the migrated type, and those a re-key reaches.
        | EventBody::Migrated { .. } => true,
        // The record whose field it fills; one that names no field is put
        // and fills nothing.
        EventBody::OutputRecorded { field, .. } => field.is_some(),
        // Nothing of the projection is changed for these but their mark.
        EventBody::LakeCreated { .. }
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
        | EventBody::ObjectSetChanged { .. } => false,
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
        }
    }

    /// The migration, of a step that may move a key; a recomputation moves
    /// none.
    fn migrated(&self) -> Option<&Migrated> {
        match self {
            Step::Migrated(s) => Some(s),
            Step::Recomputed(_) => None,
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
        };
        record::rederive(&self.entries, r, &self.event, &self.settings, &mut outputs)
    }
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
        }
    }

    /// A record, not an ended key, after the step. Of the migrated type:
    /// its fields converted, its per-field provenance carried over the
    /// renames for the fields it still has, at the step's position. Under
    /// a re-key: its references to moved keys rewritten and, when its own
    /// key moved, its new key; at the step's position when either changed
    /// it. Its version and its flags are never touched.
    fn apply(&self, mut r: StoredRecord) -> Result<StoredRecord> {
        if r.ty == self.migration.to.name {
            let converted = shape::migrate_record(&self.before, &self.migration, &r.fields)
                .map_err(|err| unavailable(err.to_string()))?
                .ok_or_else(|| {
                    unavailable(format!(
                        "the migration at position {} cannot convert `{}` `{}`",
                        self.at, r.ty, r.key
                    ))
                })?;
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
    /// state it has now; never an ended key.
    records: BTreeMap<(TypeName, RecordKey), StoredRecord>,
    /// The steps this transaction folded and kept, in the order of their
    /// events.
    steps: Vec<Step>,
    /// The recomputations whose records are not walked yet: every
    /// `declared` that recomputes since the last event that read a record.
    pending: Vec<Recomputed>,
    /// What the fold and the decision before it read from the projection.
    reads: Reads,
    /// Proposals changed in this append.
    pub proposals: BTreeMap<ProposalId, Proposal>,
    /// Outputs recorded in this append: every recording, since a step
    /// reads the last one before its own event.
    pub outputs: Recorded,
    /// Set memberships put in this append, by set, type and key.
    members: BTreeMap<(String, TypeName, RecordKey), bool>,
}

impl Staged {
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

    /// Remember a record written in this append.
    pub fn put(&mut self, r: StoredRecord) {
        self.records.insert((r.ty.clone(), r.key.clone()), r);
    }
}

#[cfg(test)]
mod tests;
