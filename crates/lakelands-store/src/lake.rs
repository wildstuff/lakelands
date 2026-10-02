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
//! nothing else does. Opening a lake folds whatever the log holds beyond
//! the projection; `rebuild` clears the projection and folds everything
//! again, reaching the same state (`docs/decisions/001-the-log-is-the-truth.md`).
//!
//! **Time.** `known_at` is the writer's wall clock, never earlier than the
//! event before it, so it never decreases along the log (`docs/log-format.md`
//! §1.3).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use lakelands_api::engine::{CanonicalEvent, LakeEngine, Lease, OpenMode, ProjectionView};
use lakelands_api::event::{Digest, Event, EventBody, LOG_FORMAT_BASE, SegmentDigest};
use lakelands_api::location::{EngineConfig, Limits, Location};
use lakelands_api::ontology::StorageKind;
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::Seams;
use lakelands_api::write::{Declaration, Proposal, ProposalFilter, ProposalState};
use lakelands_api::{
    Error, LakeId, Position, ProposalId, RecordKey, Result, Timestamp, TypeName, Value,
};
use lakelands_core::{shape, unavailable};

use crate::backed::{Due, Outputs};
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
    holds_lease: AtomicBool,
}

/// The opener `seams` passes for the engine `location` names.
pub(crate) fn opener(
    location: &Location,
    seams: &Seams,
) -> Result<Arc<dyn lakelands_api::engine::EngineOpener>> {
    let name = engine_name(&location.engine);
    seams
        .engines
        .iter()
        .find(|e| e.name() == name)
        .cloned()
        .ok_or_else(|| Error::Unbound {
            seam: format!("the engine `{name}` (no opener for it was passed in)"),
        })
}

fn engine_name(config: &EngineConfig) -> String {
    match config {
        EngineConfig::Sqlite { .. } => "sqlite".into(),
        EngineConfig::Memory => "memory".into(),
        EngineConfig::Postgres { .. } => "pg".into(),
        EngineConfig::Other { name, .. } => name.clone(),
        _ => String::new(),
    }
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
/// record, by key.
pub fn settings_in(view: &dyn ProjectionView) -> Result<BTreeMap<String, Value>> {
    let scan = lakelands_api::engine::Scan {
        limit: u32::MAX,
        ..Default::default()
    };
    let ty = TypeName::from(SETTING_TYPE);
    if !view
        .declarations()?
        .iter()
        .any(|d| matches!(&d.declaration, Declaration::Type(t) if t.name == ty))
    {
        return Ok(BTreeMap::new());
    }
    Ok(view
        .scan(&ty, &scan)?
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
        self.writer
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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
            w.ontology = Ontology::from_declared(&view.declarations()?);
            w.settings = settings_in(view.as_ref())?;
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
            return Err(unavailable(
                "this lake is open read-only here; another process holds its writer lease",
            ));
        };
        // Nothing is appended while the projection is behind the log: a
        // fold that failed after its events were durable is done again
        // first, or the append answers its error and appends nothing.
        if w.behind {
            self.refold(w)?;
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
        self.fold_all(w, &events, reads)?;
        // Published only once the fold has committed: a position `head()`
        // answers is one a read can be answered at, on every engine.
        self.publish(w);
        Ok(events)
    }

    /// Fold events into the projection, in one transaction.
    fn fold_all(&self, w: &mut Writer, events: &[Event], reads: Reads) -> Result<()> {
        let Some(last) = events.last() else {
            return Ok(());
        };
        let projection = self.engine.projection();
        projection.begin()?;
        let due = w.due.len();
        let mut staged = Staged {
            reads,
            ..Staged::default()
        };
        let result = (|| {
            let view = self.engine.view(None)?;
            for e in events {
                self.fold(w, view.as_ref(), &mut staged, e)?;
            }
            Ok(())
        })()
        .and_then(|()| projection.commit(last.position));
        match result {
            Ok(()) => {
                w.behind = false;
                Ok(())
            }
            Err(e) => {
                // The projection is as it was; so is what the writer read
                // from it while folding.
                projection.rollback();
                w.behind = true;
                w.due.truncate(due);
                if let Ok(view) = self.engine.view(None) {
                    if let Ok(d) = view.declarations() {
                        w.ontology = Ontology::from_declared(&d);
                    }
                    if let Ok(s) = settings_in(view.as_ref()) {
                        w.settings = s;
                    }
                }
                Err(e)
            }
        }
    }

    /// Move the records a re-keying migration lists to their new keys,
    /// keeping their fields, versions and per-field provenance; end each
    /// old key that is no one's new key; and rewrite every reference to a
    /// moved key (`crate::rekey`). An ended key reads as not found from
    /// here on, and a later record at it starts at version 1.
    fn rekey(
        &self,
        w: &Writer,
        view: &dyn ProjectionView,
        projection: &dyn lakelands_api::engine::ProjectionStore,
        staged: &mut Staged,
        e: &Event,
        rekeyed: &[lakelands_api::write::Rekey],
    ) -> Result<()> {
        let mut moves: crate::rekey::Moves = BTreeMap::new();
        for r in rekeyed {
            moves
                .entry(r.ty.clone())
                .or_default()
                .insert(r.from.clone(), r.to.clone());
        }
        // Every moved record is read before any is written: a new key may
        // be another record's old one.
        let mut moved = Vec::new();
        for (ty, m) in &moves {
            let decl = w.ontology.ty(ty)?.clone();
            for (from, to) in m {
                let Some(r) = staged.record(view, ty, from)? else {
                    return Err(unavailable(format!(
                        "the migration at position {} moves `{ty}` `{from}`, which the lake does \
                         not hold",
                        e.position
                    )));
                };
                let fields = crate::rekey::rewritten(&decl, &r.fields, &moves);
                moved.push((
                    from.clone(),
                    StoredRecord {
                        key: to.clone(),
                        fields,
                        position: e.position,
                        ..r
                    },
                ));
            }
        }
        let arrivals: std::collections::BTreeSet<(TypeName, RecordKey)> = moved
            .iter()
            .map(|(_, r)| (r.ty.clone(), r.key.clone()))
            .collect();
        // Object-set membership moves with the record: the new key is in
        // every set the old key was in, and an ended key is in none.
        let mut members = Vec::new();
        for (from, r) in &moved {
            for s in w
                .ontology
                .object_sets
                .values()
                .filter(|s| s.decl.of == r.ty)
            {
                let name = &s.decl.name;
                members.push((
                    name.clone(),
                    r.ty.clone(),
                    from.clone(),
                    r.key.clone(),
                    view.is_member(name, &r.ty, from)?,
                ));
            }
        }
        for (set, ty, from, to, member) in members {
            projection.put_membership(&set, &ty, &to, member, e.position)?;
            if member && !arrivals.contains(&(ty.clone(), from.clone())) {
                projection.put_membership(&set, &ty, &from, false, e.position)?;
            }
        }
        for (from, r) in moved {
            if !arrivals.contains(&(r.ty.clone(), from.clone())) {
                let ended = StoredRecord::ended(r.ty.clone(), from, e.position);
                projection.put_record(&ended)?;
                staged.put(ended);
            }
            projection.put_record(&r)?;
            staged.put(r);
        }
        // References to a moved key, in records that did not move.
        for (name, entry) in &w.ontology.types {
            let decl = &entry.decl;
            if decl.view.is_some()
                || !decl.fields.iter().any(|f| {
                    f.ty.kind == StorageKind::Reference
                        && f.ty.target.as_ref().is_some_and(|t| moves.contains_key(t))
                })
            {
                continue;
            }
            for r in staged_scan(view, staged, name)?
                .into_iter()
                .filter(|r| !r.is_ended())
            {
                let touched = |k: &RecordKey| {
                    arrivals.contains(&(name.clone(), k.clone()))
                        || moves.get(name).is_some_and(|m| m.contains_key(k))
                };
                if touched(&r.key) {
                    continue;
                }
                let fields = crate::rekey::rewritten(decl, &r.fields, &moves);
                if fields != r.fields {
                    let next = StoredRecord {
                        fields,
                        position: e.position,
                        ..r
                    };
                    projection.put_record(&next)?;
                    staged.put(next);
                }
            }
        }
        Ok(())
    }

    /// Fold the log from `from` on, a page at a time, each page in its own
    /// transaction that commits `folded_through` at its last event. A fold
    /// stops in one of two ways: reading a page of the log fails, or
    /// folding it does, and that page rolls back. Either way the
    /// projection is folded through the last page committed, the writer is
    /// `behind`, and the head names that position; nothing is appended
    /// until the next fold (an append's `refold`, a `rebuild`, or the next
    /// open's catch-up) has continued from there. Every read is answered
    /// at or before `folded_through`, so no read sees half a page.
    fn fold_from(&self, w: &mut Writer, from: Position) -> Result<()> {
        let folded = (|| {
            for page in self.pages(from) {
                self.fold_all(w, &page?, Reads::default())?;
            }
            Ok(())
        })();
        if folded.is_err() {
            // Whatever stopped it, a read of the log or a page's fold, the
            // projection is behind the log: nothing is appended until a
            // fold has caught up, and the head names the last page
            // committed, as `head()` promises.
            w.behind = true;
            let through = self.engine.projection().folded_through().ok().flatten();
            self.head
                .store(through.map_or(0, |p| p.0), Ordering::Release);
        }
        folded
    }

    /// Fold the events the log holds past the projection, a page at a
    /// time, and publish the head once they are folded.
    fn refold(&self, w: &mut Writer) -> Result<()> {
        let folded = self.engine.projection().folded_through()?;
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
                                || t.fields.iter().any(|f| {
                                    f.embed.is_some()
                                        || f.function
                                            .as_deref()
                                            .and_then(|n| w.ontology.functions.get(n))
                                            .is_some_and(|func| {
                                                crate::backed::is_backed(&func.decl)
                                            })
                                }) =>
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
                // compute every live record's derived fields again, in this
                // event.
                for ty in rederive {
                    let records = view.scan(&ty, &every(false))?;
                    // The records written earlier in this append are staged,
                    // not yet in the view.
                    let mut all: BTreeMap<RecordKey, StoredRecord> =
                        records.into_iter().map(|r| (r.key.clone(), r)).collect();
                    all.extend(staged.of_type(&ty).map(|r| (r.key.clone(), r.clone())));
                    for r in all.into_values() {
                        let mut outputs = Outputs {
                            view,
                            staged: Some(&staged.outputs),
                            due: Some(&mut w.due),
                        };
                        let next = record::rederive(&w.ontology, &r, e, &w.settings, &mut outputs)?;
                        if let Some(next) = next {
                            projection.put_record(&next)?;
                            self.index_vectors(&w.ontology, &next)?;
                            staged.put(next);
                        }
                    }
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
                let records = staged_scan(view, staged, &migration.to.name)?;
                let decl = Declaration::Type(migration.to.clone());
                w.ontology
                    .put(decl.clone(), *version, Some(schema_hash.clone()));
                projection.put_declaration(&decl, *version, Some(schema_hash), e.position)?;
                for r in records.into_iter().filter(|r| !r.is_ended()) {
                    let converted = shape::migrate_record(&before, migration, &r.fields)
                        .map_err(|err| unavailable(err.to_string()))?
                        .ok_or_else(|| {
                            unavailable(format!(
                                "the migration at position {} cannot convert `{}` `{}`",
                                e.position, r.ty, r.key
                            ))
                        })?;
                    let mut meta = BTreeMap::new();
                    for (old, meta_of) in &r.meta {
                        let new = migration
                            .renames
                            .iter()
                            .find(|(from, _)| from == old)
                            .map_or(old, |(_, to)| to);
                        if converted.contains_key(new) {
                            meta.insert(new.clone(), meta_of.clone());
                        }
                    }
                    let next = StoredRecord {
                        fields: converted,
                        meta,
                        position: e.position,
                        ..r
                    };
                    projection.put_record(&next)?;
                    staged.put(next);
                }
                if !rekeyed.is_empty() {
                    self.rekey(w, view, projection, staged, e, rekeyed)?;
                }
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
                    write: write.clone(),
                    why: why.clone(),
                    state: ProposalState::Pending,
                    decided_by: None,
                    reason: None,
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
            } => {
                projection.put_membership(set, ty, key, *joined, e.position)?;
            }
            EventBody::OutputRecorded {
                seam,
                input,
                output,
                field,
            } => {
                projection.put_output(seam, input, output, e.position)?;
                staged
                    .outputs
                    .insert((seam.clone(), input.clone()), output.clone());
                if let Some((ty, key, name)) = field
                    && let Some(r) = staged.record(view, ty, key)?
                {
                    let mut outputs = Outputs {
                        view,
                        staged: Some(&staged.outputs),
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
        let (Some(index), Ok(decl)) = (self.engine.vectors(), ontology.ty(&r.ty)) else {
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

    /// Clear the projection and fold the whole log again.
    pub fn rebuild(&self) -> Result<Position> {
        let mut w = self.writer();
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
        let last = last.ok_or_else(|| Error::Refused {
            field: None,
            reason: lakelands_api::Refusal::OutOfRange { head: self.head() },
        })?;
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
        let decl = view
            .declarations()?
            .into_iter()
            .find_map(|d| match d.declaration {
                Declaration::Type(t) if &t.name == ty => Some(t),
                _ => None,
            })
            .ok_or_else(|| Error::NotFound {
                missing: lakelands_api::Missing::Type { name: ty.clone() },
            })?;
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
        let mut w = self.writer();
        let lease = w
            .lease
            .clone()
            .ok_or_else(|| unavailable("this lake is open read-only here"))?;
        // A segment is sealed only over events the projection has folded.
        if w.behind {
            self.refold(&mut w)?;
        }
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

fn broken(at: Position, what: String) -> Error {
    unavailable(format!("the checkpoint at position {at} is broken: {what}"))
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
    if e.position.0 != through.0 + 1 {
        return Err(broken(
            e.position,
            format!("it says it follows position {through}"),
        ));
    }
    if *hash != e.prev {
        return Err(broken(
            e.position,
            "its hash is not the hash of the event before it".into(),
        ));
    }
    let mut next = 0u64;
    for s in listed {
        if s.first.0 != next || s.last < s.first {
            return Err(broken(
                e.position,
                format!(
                    "its segment list has the range {}..={} where one starting at {next} belongs",
                    s.first, s.last
                ),
            ));
        }
        next = s.last.0 + 1;
    }
    if next != through.0 + 1 {
        return Err(broken(
            e.position,
            format!("its segment list does not end at position {through}"),
        ));
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

/// Every record of `ty` as the fold leaves it so far, retired and
/// restricted ones included: the view's, with what this append wrote
/// earlier over it. A fold of many events in one page (a rebuild, an
/// import) writes records the view it opened does not show yet.
fn staged_scan(
    view: &dyn ProjectionView,
    staged: &Staged,
    ty: &TypeName,
) -> Result<Vec<StoredRecord>> {
    let records = view.scan(ty, &every(true))?;
    let mut all: BTreeMap<RecordKey, StoredRecord> =
        records.into_iter().map(|r| (r.key.clone(), r)).collect();
    all.extend(staged.of_type(ty).map(|r| (r.key.clone(), r.clone())));
    Ok(all.into_values().collect())
}

/// What one append has written so far, read before the projection.
#[derive(Default)]
pub struct Staged {
    records: BTreeMap<(TypeName, RecordKey), StoredRecord>,
    /// What the fold and the decision before it read from the projection.
    reads: Reads,
    /// Proposals changed in this append.
    pub proposals: BTreeMap<ProposalId, Proposal>,
    /// Outputs recorded in this append, by seam and input.
    pub outputs: BTreeMap<(String, Digest), Value>,
}

impl Staged {
    /// A record as this append leaves it so far: written earlier in it,
    /// or else as the projection holds it. What the append wrote always
    /// comes first; a read kept from before it never answers over it.
    pub fn record(
        &self,
        view: &dyn ProjectionView,
        ty: &TypeName,
        key: &RecordKey,
    ) -> Result<Option<StoredRecord>> {
        match self.records.get(&(ty.clone(), key.clone())) {
            Some(r) => Ok(Some(r.clone())),
            None => self.reads.get(view, ty, key),
        }
    }

    /// The records of one type written in this append.
    pub fn of_type<'a>(&'a self, ty: &'a TypeName) -> impl Iterator<Item = &'a StoredRecord> + 'a {
        self.records
            .range((ty.clone(), RecordKey::from(""))..)
            .take_while(move |((t, _), _)| t == ty)
            .map(|(_, r)| r)
    }

    /// Remember a record written in this append.
    pub fn put(&mut self, r: StoredRecord) {
        self.records.insert((r.ty.clone(), r.key.clone()), r);
    }
}
