//! The write doors: what a write checks, and which events it appends.
//!
//! Every write is decided before anything is appended — grants, the door
//! a field belongs to, values against their types, required fields,
//! compare-and-set — and refused whole when one check fails; a refused
//! write appends nothing (`docs/decisions/012-writes-touch-only-their-fields.md`,
//! `docs/decisions/015-errors-are-a-closed-set-writes-answer-an-outcome.md`).
//! An empty patch is refused as invalid, never a silent no-op, so every
//! applied write moves the position.
//!
//! **Consequences in the same append.** The record a write will produce is
//! computed first, with the fold's own function
//! (`record::apply`); from it the store decides which triggers
//! fire (a trigger fires on every write of its type whose guard holds over
//! the record after the write — `changed` and `previous` see the record
//! before it; a guard that cannot be evaluated does not fire) and which
//! object sets the record joins or leaves. Those facts are appended with
//! the write, each naming it as its `cause`
//! (`docs/decisions/004-the-change-feed-is-the-log.md`).
//!
//! **Retries.** A write that carries an idempotency key takes an id derived
//! from the writer and the key; if that id is already in the log, the
//! write answers the first write's outcome and appends nothing.
//!
//! **Versions.** A record's version counts every event that changed it,
//! across retirement: writing a retired key again continues its count.
//! Compare-and-set compares with the record's version, a retired record's
//! included, so a version from before a retirement never applies to the
//! record written after it; only a key never written is at version 0.
//!
//! **References.** A `reference` value a write carries names a live record
//! of its target type — at the head, or written earlier in the same append
//! or ingest window, or the record being written itself — or the write is
//! refused whole on that field. Retiring a record that others refer to
//! stays allowed.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use lakelands_api::engine::ProjectionView;
use lakelands_api::error::{
    Case, Flaw, Named, Naming, Part, Place, RecordName, Retry, Said, Seam, Situation,
};
use lakelands_api::event::{Digest, Event, EventBody, EventId, Procedure};
use lakelands_api::ontology::{
    Assignment, Authority, ConstraintDecl, EmbedDecl, FieldDecl, RelationDecl, Risk, TriggerDecl,
    TypeDecl, ValueTypeDecl, VerbDecl, VerbEffect,
};
use lakelands_api::principal::WriteContext;
use lakelands_api::record::{Origin, StoredRecord};
use lakelands_api::seam::VerbExecutor;
use lakelands_api::write::{
    Breach, Carried, Carrier, Declaration, FieldFill, InForce, Migration, MigrationReport, Misfit,
    ObjectSetDecl, Outcome, ProposalState, ProposedWrite, Recompute, RelationRename, SettingDecl,
    ValueRenamed, ValueTypeMigration, ValueTypeReport, ValueTypeUse, WriteOpts,
};
use lakelands_api::{
    Access, ConstraintName, Error, FieldName, IdempotencyKey, Missing, Patch, Position, Principal,
    ProposalId, RecordKey, Refusal, RelationName, Result, SourceId, Timestamp, TypeName, Value,
    VerbName, Version,
};
use lakelands_core::expr::{self, Place as Eval, RecordScope, Scope, WriteScope};
use lakelands_core::rights::{self, Door};
use lakelands_core::sight::Sight;
use lakelands_core::{describe, invalid, links, shape, value};

use crate::backed::{Outputs, is_backed};
use crate::declare::{self, invalid_field};
use crate::lake::{
    AGING_RULE_TYPE, Lake, SETTING_TYPE, Writer, computes, computes_again, computing_changed,
    every, fields_again, find_proposal,
};
use crate::log::{self, Draft};
use crate::ontology::Ontology;
use crate::record;

mod across;
mod plan;

use across::Heads;
pub(crate) use across::reads_across;
pub(crate) use plan::Against;
pub(crate) use plan::When;
pub(crate) use plan::shown_plan;

/// A set membership that changes: the set, the record's type and its key
/// (after the migration, when one changes it), and whether it joined.
type Change = (String, TypeName, RecordKey, bool);

/// A type's sets, each with its members before a migration.
type Held<'a> = Vec<(&'a ObjectSetDecl, BTreeSet<RecordKey>)>;

/// What a migration's preflight finds: its report, the record keys it
/// moves and, applied, the set memberships it changes and the links it retires.
pub(crate) struct MigrationPlan {
    report: MigrationReport,
    /// The sections of its plan of the other types whose references a
    /// re-key rewrites, that would be left naming no live record (062).
    pub(crate) others: Vec<MigrationReport>,
    rekeyed: Vec<lakelands_api::write::Rekey>,
    members: Vec<Change>,
    unlinked: Vec<StoredRecord>,
}

/// The record a call names, checked against the verb's scope before
/// anything is read (013, extended): a verb on a record needs its key, a
/// verb on its type takes none, and with no record there is no version
/// to compare.
fn scoped<'k>(
    verb: &VerbDecl,
    key: Option<&'k RecordKey>,
    expected: Option<Version>,
) -> Result<Option<&'k RecordKey>> {
    use lakelands_api::ontology::VerbScope as On;
    let verb_scope = verb.scope;
    let (verb, ty) = (verb.name.clone(), verb.on.clone());
    match (verb_scope, key) {
        (On::Type, Some(_)) => Err(invalid(Case::VerbTypeTakesNoKey { verb, ty })),
        (On::Record, None) => Err(invalid(Case::VerbNeedsKey { verb, ty })),
        (On::Type, None) if expected.is_some() => {
            Err(invalid(Case::VerbTypeNoVersion { verb, ty }))
        }
        _ => Ok(key),
    }
}

/// Whether a cardinality allows one link per `from`, and one per `to`.
fn one_per(c: lakelands_api::ontology::Cardinality) -> (bool, bool) {
    use lakelands_api::ontology::Cardinality::*;
    (
        matches!(c, ManyToOne | OneToOne),
        matches!(c, OneToMany | OneToOne),
    )
}

/// What a walk that changes stored records leaves of a type's constraints
/// (lakelands#1119): a declaration's or a migration's computing again, a
/// re-key's rewritten references, a value type's renames. A constraint is
/// judged where the walk may move a field it reads, directly or through
/// the fields that feed it, which the declarations alone tell; a record
/// is judged as the walk leaves it where such a field moved, with
/// `previous` as the record itself: a walk is no transition.
pub(crate) struct Walked {
    plan: std::sync::Arc<record::Plan>,
    ty: TypeName,
    /// Per constraint the walk may move, the records it would leave
    /// breaking it that its reader is told of (a held one only to a holder
    /// of `restricted`, 062), all of them, and the first keys of those its
    /// reader is answered.
    broken: Vec<Option<(u64, u64, Vec<RecordKey>)>>,
    /// Whether its reader holds `restricted`.
    held: bool,
    /// Per constraint, whether its reader reads every field it reads: one
    /// that admits is reported only to such a reader (lakelands#1119).
    told: Vec<bool>,
}

impl Walked {
    /// For `decl` as `o` declares it, over the fields the walk may move.
    /// One who does not read every field a constraint that refuses and
    /// that it may move reads is not granted the walk, decided before any
    /// record is read, so the answer is the same whatever they hold.
    /// Where `telling`, as a proposal's plan: one it may not read is not
    /// judged and is not told, and nothing is refused (062).
    pub(crate) fn of(
        o: &crate::ontology::Ontology,
        decl: &TypeDecl,
        moving: &BTreeSet<FieldName>,
        (sight, telling): (&Sight<'_>, bool),
    ) -> Result<Walked> {
        let plan = record::Plans::default().get(o, decl);
        let reaches = |c: &record::Constraint| !c.fed.is_disjoint(moving);
        let reads =
            |c: &record::Constraint| c.reads.iter().all(|f| sight.reads_from(&decl.name, f));
        for c in (plan.constraints.iter()).filter(|c| reaches(c) && c.decl.at_ingest.refuses()) {
            granted(telling || reads(c))?;
        }
        let broken = (plan.constraints.iter())
            .map(|c| (reaches(c) && (!telling || reads(c))).then(|| (0, 0, Vec::new())))
            .collect();
        let held = sight.who().grants.restricted;
        let told = plan.constraints.iter().map(reads).collect();
        let ty = decl.name.clone();
        Ok(Walked {
            plan,
            ty,
            broken,
            held,
            told,
        })
    }

    /// Whether it judges anything: a constraint that refuses, or, where
    /// `admits`, one that admits, which a report names.
    pub(crate) fn judges(&self, admits: bool) -> bool {
        let judged = |(c, b): (&record::Constraint, &Option<_>)| {
            b.is_some() && (admits || c.decl.at_ingest.refuses())
        };
        self.plan.constraints.iter().zip(&self.broken).any(judged)
    }

    /// Whether it judges a constraint its reader is told of.
    pub(crate) fn tells(&self) -> bool {
        (self.broken.iter().zip(&self.told)).any(|(b, told)| b.is_some() && *told)
    }

    /// Counts `is`, `was` as the walk leaves it, against each constraint
    /// it moves; `shown` tells whether its reader is answered the record.
    pub(crate) fn judge(
        &mut self,
        (was, is): (&StoredRecord, &StoredRecord),
        (w, now): (&Writer, Timestamp),
        shown: &dyn Fn(&StoredRecord) -> Result<bool>,
    ) -> Result<()> {
        let moved = |f: &FieldName| was.fields.get(f) != is.fields.get(f);
        for (c, b) in self.plan.constraints.iter().zip(&mut self.broken) {
            let Some((records, all, first)) = b.as_mut().filter(|_| !is.retired) else {
                continue;
            };
            if !c.reads.iter().any(moved) {
                continue;
            }
            let scope = WriteScope {
                after: &is.fields,
                before: Some(&is.fields),
                retiring: false,
                meta: Some(&is.meta),
                settings: Some(&w.settings),
                now,
            };
            if !expr::holds(&c.holds, &scope).unwrap_or(false) {
                *all += 1;
                *records += u64::from(self.held || !was.restricted);
                if first.len() < 5 && shown(was)? {
                    first.push(was.key.clone());
                }
            }
        }
        Ok(())
    }

    /// Refused where a record breaks one that refuses, appending nothing,
    /// naming the constraint, the type and the count; otherwise each that
    /// admits and is broken, for the walk's report. Where `telling`, one
    /// that refuses is listed too, and the walk is told it is not lossless
    /// where its reader is told of a record that breaks it.
    pub(crate) fn kept(self, telling: bool) -> Result<(Vec<Breach>, bool)> {
        let mut lossless = true;
        let mut breaches = Vec::new();
        let walked = self.plan.constraints.iter().zip(self.broken).zip(self.told);
        for ((c, b), told) in walked {
            let Some((records, all, sample_keys)) = b.filter(|b| b.1 > 0 && told) else {
                continue;
            };
            let (name, ty) = (c.decl.name.clone(), self.ty.clone());
            if c.decl.at_ingest.refuses() && !telling {
                let records = all;
                let case = Case::ConstraintLeftBroken {
                    constraint: name,
                    ty,
                    records,
                };
                return Err(constraint(None, case));
            }
            // A plan only told is lossy by what its reader is told of: a
            // record held from it moves nothing it is answered (062).
            if records == 0 {
                continue;
            }
            lossless &= !c.decl.at_ingest.refuses();
            breaches.push(Breach {
                ty,
                constraint: name,
                records,
                sample_keys,
            });
        }
        Ok((breaches, lossless))
    }
}

/// What [`Call::bundle_drafts`] makes of a checked set of declarations.
pub(crate) struct Bundled {
    /// Its drafts.
    pub drafts: Vec<Draft>,
    /// The sections of its plan: each type it computes again, as its author
    /// is shown it (062).
    pub types: Vec<MigrationReport>,
}

/// A key a record would name, the record's own and whether it may be told.
type Pointed = (String, RecordKey, bool);

/// What a walk that changes stored records leaves of a type's references
/// (062): per reference field the walk may move, the key each live record
/// would name. A held record is noted only for a reader who holds
/// `restricted`, so a count tells no other reader of a hold; what the
/// reader is told of the fields is decided after, as a report's is.
pub(crate) struct Unlived {
    /// Per field: its target, and each record's key named, its own key and
    /// whether its reader may be told it as a first key.
    fields: Vec<(FieldName, TypeName, Vec<Pointed>)>,
    held: bool,
}

impl Unlived {
    /// For `decl`, over the fields the walk may move.
    pub(crate) fn of(decl: &TypeDecl, moving: &BTreeSet<FieldName>, sight: &Sight<'_>) -> Unlived {
        let fields = (decl.fields.iter())
            .filter(|f| moving.contains(&f.name))
            .filter_map(|f| f.ty.target.clone().map(|t| (f.name.clone(), t, Vec::new())))
            .collect();
        let held = sight.who().grants.restricted;
        Unlived { fields, held }
    }

    /// Whether it notes any field.
    pub(crate) fn watches(&self) -> bool {
        !self.fields.is_empty()
    }

    /// Notes `is`, a live record as the walk leaves it; `was`, as stored.
    pub(crate) fn note(&mut self, (was, is): (&StoredRecord, &StoredRecord), sight: &Sight<'_>) {
        if is.retired || (was.restricted && !self.held) {
            return;
        }
        let shown = !sight.withheld(was);
        for (field, _, named) in &mut self.fields {
            if let Some(k) = is.fields.get(field).and_then(Value::as_str) {
                named.push((k.to_owned(), was.key.clone(), shown));
            }
        }
    }

    /// Per field, the records left naming no live record, and the first
    /// keys of those `sight`'s principal is answered: each target read once,
    /// a page of keys at a time.
    pub(crate) fn counted(
        self,
        view: &dyn ProjectionView,
        sight: &Sight<'_>,
        ty: &TypeName,
    ) -> Result<Vec<lakelands_api::write::Unlive>> {
        let mut out = Vec::new();
        for (field, target, named) in self.fields {
            let keys: BTreeSet<&str> = named.iter().map(|(k, ..)| k.as_str()).collect();
            let keys: Vec<RecordKey> = keys.into_iter().map(RecordKey::from).collect();
            let mut live = BTreeSet::new();
            for page in keys.chunks(1_000) {
                let found = view.get_many(&target, page)?.into_iter();
                live.extend(found.filter(|r| !r.retired).map(|r| r.key.to_string()));
            }
            let (mut records, mut sample_keys) = (0, Vec::new());
            for (_, key, shown) in named.iter().filter(|(k, ..)| !live.contains(k)) {
                records += 1;
                if sample_keys.len() < 5 && *shown && crate::reads::answered(view, sight, ty, key)?
                {
                    sample_keys.push(key.clone());
                }
            }
            if records > 0 {
                out.push(lakelands_api::write::Unlive {
                    field,
                    records,
                    sample_keys,
                });
            }
        }
        Ok(out)
    }
}

/// A refusal of what a constraint does not allow, at `field` if one is named.
pub(crate) fn constraint(
    field: Option<FieldName>,
    said: impl Into<lakelands_api::error::Said>,
) -> Error {
    let reason = Refusal::Constraint(Box::new(said.into()));
    Error::Refused { field, reason }
}

/// A refusal that names no field.
pub(crate) fn refused(reason: Refusal) -> Error {
    let field = None;
    Error::Refused { field, reason }
}

/// Refuse a write or retire of a setting its writer is not told whether it
/// exists, a source's (063), before anything of it is read: the same in a
/// lake that holds it and in one that does not, a proposal's and an
/// ingest's too.
pub(crate) fn untold_setting(sees: &Sight<'_>, decl: &TypeDecl, key: &RecordKey) -> Result<()> {
    granted(decl.name.as_str() != SETTING_TYPE || sees.setting_told(key.as_str()))
}

/// Refuse as not granted unless `ok`.
pub(crate) fn granted(ok: bool) -> Result<()> {
    if ok {
        return Ok(());
    }
    Err(refused(Refusal::NotGranted))
}

/// Refuse a write whose context is deeper than the lake's reactive
/// ceiling, whatever the origin names
/// (`docs/decisions/021-the-write-context-belongs-to-the-host.md`). Every
/// write door calls it first, before any other check and before it takes
/// the writer, so a refused context waits on no other write and every
/// door refuses it alike. It reads nothing. A host's `cause` is checked
/// where the write appends, under the writer: that it is at or below the
/// head, then the ceiling of the trigger whose firing it names
/// ([`crate::lake::Lake::append_after`]).
pub fn check_context(lake: &Lake, context: &WriteContext) -> Result<()> {
    let max = lake.limits.max_reactive_depth;
    if context.depth > max {
        return Err(refused(Refusal::TooDeep { max }));
    }
    Ok(())
}

/// The verb executor the host bound as `name`; a verb whose executor is
/// not bound is refused.
pub(crate) fn bound_executor<'a>(lake: &'a Lake, name: &str) -> Result<&'a Arc<dyn VerbExecutor>> {
    let bound = lake.seams.executors.iter().find(|e| e.name() == name);
    bound.ok_or_else(|| {
        Error::unbound(Seam::Executor {
            name: name.to_owned(),
        })
    })
}

/// The write-side context of one call.
pub struct Call<'a> {
    /// The lake.
    pub lake: &'a Lake,
    /// Who the call is made as.
    pub who: &'a Principal,
    /// The host's context.
    pub context: WriteContext,
    /// How many entries of the lake's renames `who`'s grants were followed
    /// through; `None` for one whose grants name no field.
    pub followed: Option<usize>,
    /// What `who` was shown as the call was made, for one whose grants
    /// hide something: judged again under the writer's lock.
    pub shown: Option<Shown>,
}

/// A write's principal as the head's types showed it when its call was
/// made (024, extended): its grants as asked, the types' versions, and
/// the references to types it is not shown, held at `none` in `who`.
pub struct Shown {
    /// The principal as the call gave it: its sight is made again from it.
    pub asked: Principal,
    /// The types' versions it was shown by.
    pub versions: Vec<(TypeName, u32)>,
    /// The fields hidden.
    pub hidden: Vec<(TypeName, FieldName)>,
}

/// The record events of one append that are not in the projection yet.
#[derive(Default)]
pub struct Pending {
    records: BTreeMap<(TypeName, RecordKey), StoredRecord>,
    members: BTreeMap<(String, TypeName, RecordKey), bool>,
    /// The memberships and firings its far writes set off across an edge.
    pub across: usize,
    /// The records read from the projection, each once; the fold of the
    /// append takes them over ([`crate::lake::Lake::append_after`]).
    reads: crate::lake::Reads,
    /// The versions of a source's key the append's own events use first,
    /// as the fold will keep them ([`crate::sealing::first_use`]).
    sealed: crate::backed::Recorded,
}

impl Pending {
    /// A record as this append leaves it, or as the projection holds it.
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

    /// Reads the records of `ty` at `keys` together, ahead of the writes
    /// that will read them one by one; a record this append already
    /// planned at one of them is still answered as planned.
    pub fn prime(
        &self,
        view: &dyn ProjectionView,
        ty: &TypeName,
        keys: &[RecordKey],
    ) -> Result<()> {
        self.reads.prime(view, ty, keys)
    }

    /// The records this append read from the projection, for its fold.
    pub fn into_reads(self) -> crate::lake::Reads {
        self.reads
    }

    fn member(
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
}

/// A record write, decided: the event to append and the record it makes.
pub struct RecordPlan {
    /// The record event.
    pub draft: Draft,
    /// The record after it.
    pub after: StoredRecord,
    /// The record before it.
    pub before: Option<StoredRecord>,
}

/// One write of a record as its door states it, which [`Decide::plan`]
/// decides.
#[derive(Clone, Copy)]
pub(crate) enum Op<'a> {
    /// `upsert`: the type, the key and the patch.
    Upsert(&'a TypeName, &'a RecordKey, &'a Patch),
    /// `retire`: the type and the key.
    Retire(&'a TypeName, &'a RecordKey),
    /// A verb's run from its record on, its key fit to its scope and its
    /// payload checked.
    Call(&'a VerbDecl, Option<&'a RecordKey>, &'a Patch),
    /// `link`, with the link's own fields.
    Link(Ends<'a>, &'a Patch),
    /// `unlink`.
    Unlink(Ends<'a>),
}

/// A link as its doors name it: the type and the key it starts from, its
/// relation, and the key it ends at.
type Ends<'a> = (&'a TypeName, &'a RecordKey, &'a RelationName, &'a RecordKey);

/// An op, decided: what it appends, and the record it leaves written.
pub(crate) struct Plan {
    /// Its own event, then what that sets off.
    pub drafts: Vec<Draft>,
    /// The record an upsert, a link or a verb's write leaves; a
    /// retirement and a verb that writes none leave none.
    pub after: Option<StoredRecord>,
}

impl Call<'_> {
    fn draft(&self, now: Timestamp, origin: Origin, body: EventBody) -> Draft {
        Draft {
            writer: self.who.writer(),
            origin,
            context: self.context,
            ..Draft::system(now, body)
        }
    }

    /// The `object_set_changed` of one membership change, caused by the
    /// event at `cause` and carrying this call's context.
    fn changed(&self, (set, ty, key, joined): Change, cause: Position, now: Timestamp) -> Draft {
        let body = EventBody::ObjectSetChanged {
            set,
            ty,
            key,
            joined,
        };
        Draft {
            cause: Some(cause),
            context: self.context,
            ..Draft::system(now, body)
        }
    }

    /// Give a draft the id its idempotency key derives, and the key.
    fn keyed(&self, d: &mut Draft, key: &Option<IdempotencyKey>) {
        if let Some(k) = key {
            d.id = Some(log::authored_id(&self.who.writer(), k));
            d.idempotency_key = Some(k.clone());
        }
    }

    /// The first write's outcome, if this key was used before, with what
    /// it set off, told as the first write was told it: its append is
    /// read back whole ([`Self::appended`]).
    fn replay(
        &self,
        w: &Writer,
        key: &Option<IdempotencyKey>,
        view: &dyn ProjectionView,
    ) -> Result<Option<Outcome>> {
        let Some(k) = key else { return Ok(None) };
        if k.as_str().contains('\0') {
            return Err(invalid(Case::IdempotencyKeyNul));
        }
        let id = log::authored_id(&self.who.writer(), k);
        let Some(pos) = view.applied(&id)? else {
            return Ok(None);
        };
        let events = self.appended(w, pos)?;
        if events.is_empty() {
            return Ok(None);
        }
        let outcome = self.told_of(&w.ontology, &events, outcome_of(&events))?;
        // An external verb is answered its executor's answer, recorded in
        // a later append that other writes may precede.
        match called_out(&events, &outcome) {
            Some(at) => self.recorded(view, outcome, at, false).map(Some),
            None => Ok(Some(outcome)),
        }
    }

    /// The events from `pos` an outcome of the append there reads: its own,
    /// then every reaction that follows them, a far write's set off across
    /// an edge among them, however many, read on a page at a time while
    /// the events read end in a reaction this append caused. A later
    /// write's reactions end the paging, so a retry on a busy lake reads
    /// a page past its own append at most, never up to the head.
    fn appended(&self, w: &Writer, pos: Position) -> Result<Vec<Event>> {
        let reach = 3 + w.ontology.triggers.len() + w.ontology.object_sets.len();
        let mut events = self.lake.read_events(pos, reach as u32)?;
        let reaction = |e: &Event| {
            matches!(
                e.body,
                EventBody::TriggerFired { .. } | EventBody::ObjectSetChanged { .. }
            )
        };
        // The append's events: its first, and each one caused by one of them.
        let (mut ours, mut read) = (BTreeSet::from([pos]), 0);
        loop {
            for e in &events[read..] {
                if e.cause.is_some_and(|c| ours.contains(&c)) {
                    ours.insert(e.position);
                }
            }
            read = events.len();
            let next = match events.last() {
                Some(last) if reaction(last) && ours.contains(&last.position) => {
                    last.position.next()
                }
                _ => break,
            };
            let more = self.lake.read_events(next, 1024)?;
            if more.is_empty() {
                break;
            }
            events.extend(more);
        }
        Ok(events)
    }

    /// An external verb's outcome with the answer of its executor to the
    /// event at `due`, found by a seek of the id it is recorded under
    /// (`docs/log-format.md` §3), however far the log has grown since.
    /// With no answer recorded, the retry runs nothing and is refused as
    /// `unanswered`: the call, or its acceptance, is in the log.
    fn recorded(
        &self,
        view: &dyn ProjectionView,
        outcome: Outcome,
        due: Position,
        accepted: bool,
    ) -> Result<Outcome> {
        let found = match view.applied(&log::output_id(&self.lake.id, due))? {
            Some(at) => self.lake.read_events(at, 1)?.pop(),
            None => None,
        };
        // The id is the store's to derive; a foreign log may hold it on
        // another event.
        match found
            .filter(|e| e.cause == Some(due))
            .map(|e| (e.position, e.body))
        {
            Some((
                at,
                EventBody::OutputRecorded {
                    seam,
                    output: Some(output),
                    ..
                },
            )) if seam.starts_with("verb:") => Ok(with_output(outcome, (at, output))),
            _ => Err(unanswered(due, accepted)),
        }
    }

    /// An accept sent again: a call of a verb that was external when it
    /// was accepted is answered as a retry of a call is, whatever the verb
    /// is declared as since; anything else is `decided`. The acceptance is
    /// found by a seek of its id.
    fn accepted_again(
        &self,
        w: &Writer,
        view: &dyn ProjectionView,
        proposal: &ProposalId,
    ) -> Result<Outcome> {
        let decided = || refused(Refusal::Decided);
        let at = view.applied(&log::accepted_id(&self.lake.id, proposal))?;
        let Some(at) = at else { return Err(decided()) };
        let events = self.appended(w, at)?;
        let outcome = self.told_of(&w.ontology, &events, outcome_of(&events))?;
        match called_out(&events, &outcome) {
            Some(_) => self.recorded(view, outcome, at, true),
            None => Err(decided()),
        }
    }
}

/// The position of the call an outcome answers, where it is the call of a
/// verb that reaches outside the lake: a write verb's outcome is its write.
fn called_out(events: &[Event], outcome: &Outcome) -> Option<Position> {
    let at = outcome.position();
    let called = |e: &Event| e.position == at && matches!(e.body, EventBody::VerbCalled { .. });
    events.iter().any(called).then_some(at)
}

/// An outcome with an external verb's recorded output in it, and the
/// position it was recorded at as the last of what the call set off.
pub(crate) fn with_output(mut outcome: Outcome, (at, recorded): (Position, Value)) -> Outcome {
    if let Outcome::Applied {
        output, set_off, ..
    } = &mut outcome
    {
        *output = Some(recorded);
        set_off.through = Some(at);
    }
    outcome
}

/// An append's outcome, as [`own_outcome`] tells it, with what the write
/// set off: the reactions that follow its own event in the append, each
/// caused by it, and the position of the last of them. Each name is
/// listed once: a set's declaration moves many records, a write one. The
/// names are all of them; [`crate::doors`] keeps those the caller may
/// know of.
fn outcome_of(events: &[Event]) -> Outcome {
    let mut outcome = own_outcome(events);
    if let Outcome::Applied {
        position, set_off, ..
    } = &mut outcome
    {
        let caused = |e: &&Event| e.cause == Some(*position);
        for e in events
            .iter()
            .skip_while(|e| e.position <= *position)
            .take_while(caused)
        {
            match &e.body {
                EventBody::TriggerFired { trigger, .. } => {
                    if !set_off.fired.contains(trigger) {
                        set_off.fired.push(trigger.clone());
                    }
                }
                EventBody::ObjectSetChanged { set, joined, .. } => {
                    let names = if *joined {
                        &mut set_off.joined
                    } else {
                        &mut set_off.left
                    };
                    if !names.contains(set) {
                        names.push(set.clone());
                    }
                }
                _ => break,
            }
            set_off.through = Some(e.position);
        }
    }
    outcome
}

/// `r` as `computing`, the lake after a declaration or a migration,
/// computes its fields again as of `event`; a field computed outside the
/// lake keeps its value, which lands with its output (004).
fn computed_again(
    (computing, view, plans, keys): (
        &Ontology,
        &dyn ProjectionView,
        &record::Plans,
        &crate::sealing::Keys,
    ),
    r: &StoredRecord,
    event: &Event,
    settings: &BTreeMap<String, Value>,
) -> Result<StoredRecord> {
    let mut outputs = Outputs::planning(view, plans, keys);
    let is = record::rederive(computing, r, event, settings, &mut outputs)?;
    let mut is = is.unwrap_or_else(|| r.clone());
    let backed = |n: &str| {
        computing
            .functions
            .get(n)
            .is_some_and(|f| is_backed(&f.decl))
    };
    for f in &computing.ty(&r.ty)?.fields {
        if f.embed.is_some() || f.function.as_deref().is_some_and(backed) {
            match r.fields.get(&f.name) {
                Some(v) => is.fields.insert(f.name.clone(), v.clone()),
                None => is.fields.remove(&f.name),
            };
        }
    }
    Ok(is)
}

/// Whether `record` is a member of `set`: live, and the set's filter holds
/// over its fields at `now`, the time of the write or the declaration that
/// decides it, the sets it names read as `o` holds them, and what a path
/// reads across an edge as `heads` reaches it. A write and a set's
/// declaration decide by this one rule.
fn member_of(
    (set, o): (&ObjectSetDecl, &Ontology),
    record: &StoredRecord,
    w: &Writer,
    now: Timestamp,
    heads: Heads<'_>,
) -> Result<bool> {
    if record.retired {
        return Ok(false);
    }
    let at = [Place::ObjectSet {
        name: set.name.clone(),
    }];
    let e = declare::set_filter(&|n| o.set(n), set, &at)?;
    let scope = RecordScope {
        fields: &record.fields,
        meta: Some(&record.meta),
        settings: Some(&w.settings),
        now,
    };
    let on = o.ty(&set.of)?;
    Ok(across::holds(
        (heads, o, on),
        record,
        &e,
        scope,
        &w.settings,
    ))
}

/// What a path reads across an edge at the head of `view`, to `max`
/// steps.
fn at_head<R>(view: &dyn ProjectionView, max: u32, f: impl FnOnce(Heads<'_>) -> R) -> R {
    let far = |t: &TypeName, k: &RecordKey| view.get(t, k);
    let link = |t: &TypeName, r: &RelationName, k: &RecordKey| link_from(view, t, r, k);
    f(Heads {
        far: &far,
        link: &link,
        max,
    })
}

/// Whether a field of a type is declared and kept from the principal of
/// `sees`: what a write of a setting naming it keeps (063).
fn hidden_from<'a>(sees: &'a Sight, o: &'a Ontology) -> impl Fn(&str, &str) -> bool + 'a {
    move |ty, f| {
        let declared = |t: &TypeDecl| t.fields.iter().any(|d| d.name.as_str() == f);
        o.ty(&ty.into()).is_ok_and(declared) && !sees.reads(&ty.into(), &f.into())
    }
}

/// The preflight of `m` over the stored records of `decl`'s type, retired
/// and restricted ones included, read a page at a time. `each` sees every
/// record before the report counts it. An error of a page read or of
/// `each` ends the walk and is answered before the report is. A sample
/// key is one of a record the principal of `sees` is answered; a
/// retargeted reference converts onto a record `onto`'s principal is.
fn shaped(
    view: &dyn ProjectionView,
    (sees, onto): (&Sight, &Sight),
    decl: &TypeDecl,
    m: &Migration,
    mut each: impl FnMut(&StoredRecord) -> Result<()>,
) -> Result<MigrationReport> {
    let mut walk = crate::lake::Walk::new(view, &decl.name, true);
    let mut seen = Ok(());
    let held = std::cell::Cell::new(false);
    let rows = walk.by_ref().map_while(|r| {
        seen = each(&r);
        held.set(sees.withheld(&r));
        seen.is_ok().then_some((r.key, r.fields))
    });
    // The record in hand is asked no read where it is held as it stands. A
    // read the engine fails is kept, and answered after (lakelands#927).
    let failed = std::cell::RefCell::new(None);
    let answered = |key: &RecordKey| {
        let read = (!held.get()).then(|| crate::reads::answered(view, sees, &decl.name, key));
        match read.transpose() {
            Ok(shown) => shown.unwrap_or(false),
            Err(e) => {
                failed.borrow_mut().get_or_insert(e);
                false
            }
        }
    };
    // A reference given another target converts only where it names a
    // live record of that target (032, lakelands#1071).
    let gone = retargeted(view, onto, decl, m)?;
    let dangling = |f: &FieldName, v: &Value| {
        let key = v.as_str();
        gone.get(f)
            .is_some_and(|g| key.is_some_and(|k| g.contains(k)))
    };
    let report = shape::referring(decl, m, rows, &answered, &dangling);
    walk.failed()?;
    seen?;
    if let Some(e) = failed.into_inner() {
        return Err(e);
    }
    report.map_err(invalid)
}

/// Of each field of `m.to` whose reference target is not the one `decl`
/// gave it, the values `decl`'s records hold there that name no record of
/// the new target live to `sees`'s principal, as a write of the reference
/// finds one ([`live_for`]): one read of the target per field, of the
/// distinct values ([`retarget_values`]), and of a link's ends.
fn retargeted(
    view: &dyn ProjectionView,
    sees: &Sight,
    decl: &TypeDecl,
    m: &Migration,
) -> Result<BTreeMap<FieldName, BTreeSet<String>>> {
    let mut gone = BTreeMap::new();
    for (field, target, keys) in retarget_values(view, decl, m)? {
        let asked: Vec<RecordKey> = keys.iter().map(|k| k.as_str().into()).collect();
        let found = view.get_many(&target, &asked)?;
        let rows: Vec<_> = found.iter().map(|r| &r.fields).collect();
        let ends = match sees.ty(&target) {
            Ok(d) => sees.ends(d, &rows, &mut |t, k| view.get_many(t, k))?,
            Err(_) => vec![false; rows.len()],
        };
        let live: BTreeSet<String> = (found.iter().zip(ends))
            .filter(|(r, ends)| *ends && !r.retired && sees.answers(&target, r))
            .map(|(r, _)| r.key.to_string())
            .collect();
        gone.insert(
            field,
            keys.into_iter().filter(|k| !live.contains(k)).collect(),
        );
    }
    Ok(gone)
}

/// Of each field of `m.to` whose reference target is not the one `decl`
/// gave it, the new target and the distinct values `decl`'s records hold
/// there: one walk of the type.
fn retarget_values(
    view: &dyn ProjectionView,
    decl: &TypeDecl,
    m: &Migration,
) -> Result<Vec<(FieldName, TypeName, BTreeSet<String>)>> {
    let moved = retargeted_fields(decl, m);
    let mut named = vec![BTreeSet::new(); moved.len()];
    if !moved.is_empty() {
        crate::lake::walk(view, &decl.name, true, |r| {
            for ((old, _, _), keys) in moved.iter().zip(named.iter_mut()) {
                keys.extend(r.fields.get(old).and_then(Value::as_str).map(str::to_owned));
            }
            Ok(())
        })?;
    }
    let values = moved.into_iter().zip(named);
    Ok(values
        .map(|((_, field, target), keys)| (field, target, keys))
        .collect())
}

/// Of each field of `m.to` whose reference target is not the one `decl`
/// gave it, renames followed: its name in `decl`, in `m.to`, and its new
/// target.
fn retargeted_fields(decl: &TypeDecl, m: &Migration) -> Vec<(FieldName, FieldName, TypeName)> {
    let was = |to: &FieldName| {
        let from = m
            .renames
            .iter()
            .find(|(_, t)| t == to)
            .map_or(to, |(f, _)| f);
        decl.fields.iter().find(|f| &f.name == from)
    };
    (m.to.fields.iter())
        .filter_map(|f| {
            let (target, old) = (f.ty.target.as_ref()?, was(&f.name)?);
            let other = old.ty.target.as_ref() != Some(target);
            other.then(|| (old.name.clone(), f.name.clone(), target.clone()))
        })
        .collect()
}

/// A migration's type and each link type it changes, as each is migrated.
fn retargets(o: &Ontology, m: &Migration) -> Result<Vec<(TypeDecl, Migration)>> {
    let links = changed_link_types(o, m, false).into_iter();
    let links = links.map(|(old, new)| (old, Migration::to(new)));
    Ok(std::iter::once((o.ty(&m.to.name)?.clone(), m.clone()))
        .chain(links)
        .collect())
}

/// The types a proposed retarget reaches records of: each new target and,
/// of a link type, its ends' types; none of any other write.
pub(crate) fn retarget_types(o: &Ontology, write: &ProposedWrite) -> Result<BTreeSet<TypeName>> {
    let ProposedWrite::Migrate { migration: m, .. } = write else {
        return Ok(BTreeSet::new());
    };
    let mut out = BTreeSet::new();
    let mut next: Vec<TypeName> = Vec::new();
    for (decl, m) in retargets(o, m)? {
        next.extend(retargeted_fields(&decl, &m).into_iter().map(|(.., t)| t));
    }
    while let Some(t) = next.pop() {
        if let (true, Ok(link)) = (links::is_link_type(&t), o.ty(&t)) {
            let ends = link
                .fields
                .iter()
                .filter(|f| [links::FROM, links::TO].contains(&f.name.as_str()));
            next.extend(
                ends.filter_map(|f| f.ty.target.clone())
                    .filter(|e| !out.contains(e)),
            );
        }
        out.insert(t);
    }
    Ok(out)
}

/// The declarations that name a link type whose records a migration
/// moves, each as the lake declares it again under the new name: a trigger
/// or a verb on it, an object set of it, a view that reads it (035,
/// extended).
fn carried(o: &Ontology, m: &Migration) -> Vec<Declaration> {
    let moved = links::moved(m);
    let to = |t: &TypeName| moved.get(t).cloned();
    let mut out = Vec::new();
    for t in o.triggers.values().map(|e| &e.decl) {
        out.extend(to(&t.on).map(|on| Declaration::Trigger(TriggerDecl { on, ..t.clone() })));
    }
    for v in o.verbs.values().map(|e| &e.decl) {
        out.extend(to(&v.on).map(|on| Declaration::Verb(VerbDecl { on, ..v.clone() })));
    }
    for s in o.object_sets.values().map(|e| &e.decl) {
        out.extend(to(&s.of).map(|of| Declaration::ObjectSet(ObjectSetDecl { of, ..s.clone() })));
    }
    for t in o.types.values().map(|e| &e.decl) {
        let mut view = t.view.clone();
        for v in view.iter_mut().map(|v| &mut **v) {
            let sources = v.merge.iter_mut().map(|s| &mut s.source);
            for source in std::iter::once(&mut v.source).chain(sources) {
                *source = to(source).unwrap_or_else(|| source.clone());
            }
        }
        if view != t.view {
            out.push(Declaration::Type(TypeDecl { view, ..t.clone() }));
        }
    }
    out
}

/// How many records of `ty` the view holds, retired ones included, an
/// ended key not: read a page at a time.
fn stored(view: &dyn ProjectionView, ty: &TypeName) -> Result<u64> {
    // A count answers none at once; past that the walk leaves out an ended
    // key by the store's own rule.
    if view.count(ty, &every(true))? == 0 {
        return Ok(0);
    }
    let mut n = 0;
    crate::lake::walk(view, ty, true, |_| {
        n += 1;
        Ok(())
    })?;
    Ok(n)
}

/// Every live record of `ty`, restricted ones included, a page at a time.
pub(crate) fn live(
    view: &dyn ProjectionView,
    ty: &TypeName,
    mut each: impl FnMut(&StoredRecord) -> Result<()>,
) -> Result<()> {
    crate::lake::walk(view, ty, false, |r| each(&r))
}

/// The sets of `ty` that `pick` takes, each with its members so far, read
/// once and not looked up for each record.
fn sets_of<'a>(
    w: &'a Writer,
    view: &dyn ProjectionView,
    ty: &TypeName,
    pick: impl Fn(&ObjectSetDecl) -> bool,
) -> Result<Held<'a>> {
    let sets = (w.ontology.object_sets.values()).map(|e| &e.decl);
    let of = sets.filter(|s| &s.of == ty && pick(s));
    of.map(|s| Ok((s, view.members(&s.name, ty)?))).collect()
}

/// Decide `is`, a record as a migration leaves it, against each of its
/// type's sets: a membership that differs from the one it had under its
/// key before, `was`, is a change under its key now. Past `max` changes
/// nothing more is decided.
fn decide(
    sets: &Held<'_>,
    was: &RecordKey,
    is: &StoredRecord,
    (w, now, max): (&Writer, Timestamp, usize),
    changes: &mut Vec<Change>,
    heads: Heads<'_>,
) -> Result<()> {
    for (s, held) in sets {
        let member = held.contains(was);
        if changes.len() <= max && member != member_of((s, &w.ontology), is, w, now, heads)? {
            changes.push((s.name.clone(), is.ty.clone(), is.key.clone(), !member));
        }
    }
    Ok(())
}

/// The outcome the first of an append's events stands for. A verb call or
/// an acceptance answers what it wrote itself: an acceptance's own events
/// follow it in its append (an accepted verb proposal: `write_accepted`,
/// then `verb_called`, then the record write), so the outcome names the
/// position the write is visible from and the record's new version.
///
/// `events` may run past the append, as a retry reads them, and a later
/// write may name the call as its `cause` (a host's write of an effect's
/// result, 021). So the call's own write is told by what the store wrote,
/// never by `cause` alone: it is the very next event, caused by the call,
/// with the verb as its `procedure` (which no door sets) and the call's
/// writer. A verb with no write of its own answers at its own position.
fn own_outcome(events: &[Event]) -> Outcome {
    let e = &events[0];
    let wrote = match (&e.body, events.get(1)) {
        // An acceptance of declarations all in force appends nothing more.
        (EventBody::WriteAccepted { .. }, Some(n)) => n.cause == Some(e.position),
        (EventBody::VerbCalled { verb, .. }, Some(n)) => {
            n.cause == Some(e.position)
                && n.writer == e.writer
                && n.procedure
                    .as_ref()
                    .is_some_and(|p| p.name == verb.as_str())
        }
        _ => false,
    };
    match &e.body {
        EventBody::RecordUpserted { version, .. }
        | EventBody::RecordIngested { version, .. }
        | EventBody::RecordRetired { version, .. } => applied(e.position, Some(*version)),
        EventBody::WriteProposed { proposal, why, .. } => Outcome::Proposed {
            position: e.position,
            proposal: proposal.clone(),
            why: why.clone(),
            plan: None,
        },
        EventBody::VerbCalled { .. } | EventBody::WriteAccepted { .. } if wrote => {
            own_outcome(&events[1..])
        }
        _ => applied(e.position, None),
    }
}

/// A write applied at `position`, before what it set off is told.
fn applied(position: Position, version: Option<Version>) -> Outcome {
    Outcome::Applied {
        position,
        version,
        output: None,
        set_off: Default::default(),
        renamed: Vec::new(),
        declared: Vec::new(),
        plan: None,
    }
}

/// `outcome` naming each of `sent` as it stands in `o` after the call,
/// appended where it stands from `from` on (027, extended).
fn with_declared(
    mut outcome: Outcome,
    o: &Ontology,
    sent: &[Declaration],
    from: Position,
) -> Outcome {
    if let Outcome::Applied { declared, .. } = &mut outcome {
        *declared = (sent.iter())
            .filter_map(|d| {
                let (version, since) = o.entry_of(d)?;
                let (kind, name) = d.named();
                Some(InForce {
                    kind,
                    name: name.to_owned(),
                    version,
                    since,
                    appended: since >= from,
                })
            })
            .collect();
    }
    outcome
}

/// Check and normalise the values a patch carries against their fields.
pub fn check_values(
    lake_types: &Ontology,
    decl: &TypeDecl,
    patch: &Patch,
    fields: &[FieldDecl],
) -> Result<Patch> {
    let mut out = Patch::new();
    for (name, v) in patch {
        let f = fields.iter().find(|f| &f.name == name).ok_or_else(|| {
            let (ty, field) = (decl.name.clone(), name.clone());
            invalid_field(name, Case::UnknownField { ty, field })
        })?;
        let vt =
            f.ty.value_type
                .as_ref()
                .and_then(|n| lake_types.value_types.get(n.as_str()))
                .map(|e| &e.decl);
        let checked = value::check(v, &f.ty, vt).map_err(|e| constraint(Some(name.clone()), e))?;
        if let (Some(vt), false) = (vt, checked.is_null())
            && !satisfies(vt, &checked)?
        {
            let value_type = vt.name.clone();
            let case = Case::ValueConstraint { value_type };
            return Err(constraint(Some(name.clone()), case));
        }
        out.insert(name.clone(), checked);
    }
    Ok(out)
}

/// Whether a value, checked against its value type's kind, holds its
/// `constraint`; one without a constraint holds.
fn satisfies(vt: &ValueTypeDecl, checked: &Value) -> Result<bool> {
    let Some(c) = &vt.constraint else {
        return Ok(true);
    };
    let at = [
        Place::ValueType {
            name: vt.name.clone(),
        },
        Place::Constraint,
    ];
    let e = declare::condition(c, Eval::Record, &at)?;
    let fields: BTreeMap<FieldName, Value> = [("value".into(), checked.clone())].into();
    let scope = RecordScope {
        fields: &fields,
        meta: None,
        settings: None,
        now: Timestamp(0),
    };
    Ok(expr::holds(&e, &scope).unwrap_or(false))
}

/// On a create, set the key fields the patch leaves out from the key's
/// components (`docs/log-format.md` §2.3), each read in its field's kind.
fn fill_key_fields(
    w: &Writer,
    decl: &TypeDecl,
    key: &RecordKey,
    patch: &mut Patch,
    door: Door,
    who: &Principal,
) -> Result<()> {
    if decl.key.is_empty() || decl.key.iter().all(|k| patch.contains_key(k)) {
        return Ok(());
    }
    let parts = lakelands_core::log::key_components(key);
    if parts.len() != decl.key.len() {
        let (ty, fields, key, parts) =
            (decl.name.clone(), decl.key.len(), key.clone(), parts.len());
        return Err(invalid(Case::KeyComponents {
            ty,
            fields,
            key,
            parts,
        }));
    }
    let mut filled = Patch::new();
    for (name, part) in decl.key.iter().zip(parts) {
        if patch.contains_key(name) {
            continue;
        }
        let f = decl
            .fields
            .iter()
            .find(|f| &f.name == name)
            .expect("a key names a field");
        let kind = f.ty.kind;
        let wrong = |part: &str| {
            let (part, field) = (part.to_owned(), name.clone());
            invalid(Case::KeyComponentKind { part, kind, field })
        };
        let v = match kind {
            lakelands_api::ontology::StorageKind::Number => {
                serde_json::from_str::<serde_json::Number>(&part)
                    .map(Value::Number)
                    .map_err(|_| wrong(&part))?
            }
            lakelands_api::ontology::StorageKind::Boolean => match part.as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => return Err(wrong(&part)),
            },
            _ => Value::String(part),
        };
        filled.insert(name.clone(), v);
    }
    rights::check_write(who, decl, door, &filled)?;
    patch.extend(check_values(&w.ontology, decl, &filled, &decl.fields)?);
    Ok(())
}

/// Refuse a write whose `reference` values name no live record of their
/// target type that its writer is answered.
fn check_references(
    view: &dyn ProjectionView,
    pending: &Pending,
    ontology: &Ontology,
    ty: &TypeName,
    key: &RecordKey,
    patch: &Patch,
    who: &Principal,
) -> Result<()> {
    let decl = ontology.ty(ty)?;
    for (name, v) in patch {
        let Some(f) = decl.fields.iter().find(|f| &f.name == name) else {
            continue;
        };
        let (Some(target), Some(to)) = (&f.ty.target, v.as_str()) else {
            continue;
        };
        let to = RecordKey::from(to);
        if target == ty && to == *key {
            continue;
        }
        if live_for(view, pending, ontology, target, &to, who)?.is_none() {
            let (field, ty, key) = (name.clone(), target.clone(), to);
            return Err(constraint(
                Some(name.clone()),
                Case::NotLive { field, ty, key },
            ));
        }
    }
    Ok(())
}

/// The live record of `target` at `to` that `who` is answered, as the
/// drafts so far leave it: what a reference may name (032).
fn live_for(
    view: &dyn ProjectionView,
    pending: &Pending,
    ontology: &Ontology,
    target: &TypeName,
    to: &RecordKey,
    who: &Principal,
) -> Result<Option<StoredRecord>> {
    // A record a holder of `only` is not admitted, or one restricted, is
    // to it not there; under an `only` that cannot be applied, none is
    // (lakelands#771); nor is a record of a type it is not shown
    // (lakelands#1003).
    let sees = Sight::held(who, ontology);
    let held = sees.ty(target).ok();
    let found = pending.record(view, target, to)?;
    let mut exists =
        (found.as_ref()).is_some_and(|r| !r.retired && held.is_some() && sees.answers(target, r));
    // A link record is there only with its ends, as a read of it
    // answers (006, 051; lakelands#871).
    if let (true, Some(d), Some(r)) = (exists, held, &found) {
        exists = ends(view, pending, &sees, d, &r.fields)?;
    }
    Ok(found.filter(|_| exists))
}

/// The references a record as planned is judged for: each field with a
/// target that holds another value than the live record its writer
/// reaches held, and each computed one that reads a field the write
/// `sent`, whatever its value: a key sent again is judged again, and a
/// writer who may not read the reference would else learn it by sending
/// guesses (032). One the write neither moves nor sends a field of is
/// not judged again, as a written one outside the patch is not.
fn computed_references(
    decl: &TypeDecl,
    before: Option<&BTreeMap<FieldName, Value>>,
    after: &BTreeMap<FieldName, Value>,
    sent: &[FieldName],
) -> Patch {
    let references = decl.fields.iter().filter(|f| f.ty.target.is_some());
    (references.filter_map(|f| Some((f, after.get(&f.name)?.clone()))))
        .filter(|(f, v)| {
            before.and_then(|b| b.get(&f.name)) != Some(v)
                || computed_from(decl, f, sent, &mut Vec::new())
        })
        .map(|(f, v)| (f.name.clone(), v))
        .collect()
}

/// Whether `f` is computed, by an expression or a function, from a field
/// among `sent`, directly or through other fields so computed: a type in
/// force lists what each is computed from. What a written field lists
/// clears or keeps it and computes nothing. `seen` holds the fields asked
/// already, none at first.
fn computed_from<'d>(
    decl: &'d TypeDecl,
    f: &'d FieldDecl,
    sent: &[FieldName],
    seen: &mut Vec<&'d FieldName>,
) -> bool {
    let computed = f.derived.is_some() || f.function.is_some();
    computed
        && f.depends_on.iter().any(|d| {
            let new = !seen.contains(&d);
            seen.push(d);
            let field = decl.fields.iter().find(|x| &x.name == d);
            sent.contains(d) || new && field.is_some_and(|x| computed_from(decl, x, sent, seen))
        })
}

/// Refuse a write whose key disagrees with its key fields.
fn check_key_agrees(
    decl: &TypeDecl,
    key: &RecordKey,
    live: Option<&StoredRecord>,
    patch: &Patch,
) -> Result<()> {
    let mut fields = live.map(|b| b.fields.clone()).unwrap_or_default();
    for (k, v) in patch {
        fields.insert(k.clone(), v.clone());
    }
    match lakelands_core::log::record_key(decl, &fields) {
        Some(derived) if &derived != key => {
            let (key, ty) = (key.clone(), decl.name.clone());
            Err(invalid(Case::KeyDisagrees { key, ty, derived }))
        }
        _ => Ok(()),
    }
}

/// Refuse a key that is empty or holds a NUL character.
pub(crate) fn check_key(key: &RecordKey) -> Result<()> {
    if key.as_str().is_empty() || key.as_str().contains('\0') {
        return Err(invalid(Case::KeyEmpty));
    }
    Ok(())
}

/// Refuse a source id that is empty or holds a NUL character: it is a
/// part of an ingested event's id, whose parts NUL separates.
pub(crate) fn check_source(source: &SourceId) -> Result<()> {
    if source.as_str().is_empty() || source.as_str().contains('\0') {
        return Err(invalid(Case::SourceIdForm));
    }
    Ok(())
}

/// A caller's delivery id: not empty, no NUL, and not of the store's own
/// form, `d` and digits, so a selection by delivery names one.
pub(crate) fn check_delivery(d: &lakelands_api::DeliveryId) -> Result<()> {
    let s = d.as_str();
    let own = s
        .strip_prefix('d')
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    if s.is_empty() || s.contains('\0') || own {
        return Err(invalid(Case::DeliveryIdForm));
    }
    Ok(())
}

fn cas(expected: Option<Version>, before: Option<&StoredRecord>) -> Result<()> {
    let actual = before.map_or(Version(0), |b| b.version);
    match expected {
        Some(e) if e != actual => Err(Error::Conflict {
            expected: e,
            actual,
        }),
        _ => Ok(()),
    }
}

/// The `record_retired` that takes the record of `ty` at `key` past the version `was`.
pub(crate) fn retirement(ty: &TypeName, key: &RecordKey, was: Version) -> EventBody {
    EventBody::RecordRetired {
        ty: ty.clone(),
        key: key.clone(),
        version: Version(was.0 + 1),
    }
}

/// A provisional event at `position`, to compute the record a draft makes.
pub(crate) fn provisional(lake: &Lake, position: Position, d: &Draft) -> Event {
    log::unsealed(&lake.id, position, &Digest::zero(), 0, d.clone())
}

/// The record at a key as a principal's write reaches it: the one read of
/// a record a write makes ([`Decide::target`]).
pub(crate) struct Target {
    /// The record at the key, whoever holds it.
    pub before: Option<StoredRecord>,
    /// Whether the principal reaches it: its grant admits the record and
    /// it is answered, with its ends where it is a link (051, 006).
    mine: bool,
}

impl Target {
    /// The record as the principal's write judges it, retired or not;
    /// another's is none.
    pub fn own(&self) -> Option<&StoredRecord> {
        self.before.as_ref().filter(|_| self.mine)
    }

    /// The live record the principal reaches.
    pub fn live(&self) -> Option<&StoredRecord> {
        self.own().filter(|b| !b.retired)
    }
}

/// Whether a link of `decl` holding `fields` is there with its ends for
/// the sight's principal, as the drafts so far leave them
/// ([`Sight::ends`]).
fn ends(
    view: &dyn ProjectionView,
    pending: &Pending,
    sees: &Sight,
    decl: &TypeDecl,
    fields: &BTreeMap<FieldName, Value>,
) -> Result<bool> {
    let mut get = |end: &TypeName, keys: &[RecordKey]| {
        let found = keys.iter().map(|k| pending.record(view, end, k));
        found
            .filter_map(Result::transpose)
            .collect::<Result<Vec<_>>>()
    };
    Ok(sees.ends(decl, &[fields], &mut get)?[0])
}

/// Whether the grant of the sight's principal admits a record of `decl`
/// holding `fields`, a link with its ends, none of them held from it
/// (051, 006); a grant that cannot be applied admits none
/// (lakelands#771).
fn owns(
    view: &dyn ProjectionView,
    pending: &Pending,
    sees: &Sight,
    decl: &TypeDecl,
    fields: &BTreeMap<FieldName, Value>,
) -> Result<bool> {
    Ok(sees.admit(&decl.name).holds(fields) && ends(view, pending, sees, decl, fields)?)
}

/// Whether the proposal `p` reached a record restricted when it was logged
/// ([`reached`]).
fn held_then(
    lake: &Lake,
    p: &lakelands_api::write::Proposal,
    write: &ProposedWrite,
) -> Result<bool> {
    // What a write reached is computed wholly as it stood then: the
    // declarations, its record, the settings in force and the proposal's
    // time. A record held, a field retyped, a setting changed since is no
    // proof of the grant.
    let then = lake.engine.view(Some(p.position))?;
    for (ty, key) in reached_then(lake, then.as_ref(), p.proposed_at, write, true)? {
        if then.get(&ty, &key)?.is_some_and(|r| r.restricted) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `o` with the types `who` is shown, each as it knows it.
pub(crate) fn types_shown(who: &Principal, o: &Ontology) -> Ontology {
    let (sees, mut seen) = (Sight::held(who, o), o.clone());
    (seen.types).retain(|name, _| sees.ty(name).is_ok());
    // A type copied only where its author is kept from some of it
    // (lakelands#943): any other is the lake's own.
    let find = |n: &TypeName| o.ty(n).ok();
    for e in seen.types.values_mut() {
        if rights::keeps_from(who, &e.decl, &find) {
            rights::known_fields(who, &mut Arc::make_mut(e).decl, &find);
        }
    }
    seen
}

/// A tool's name is taken as a trigger's is (lakelands#896): where the
/// lake with `set` in it has a problem with its tools' names that the
/// lake as `who` is shown it has not, a side of it is a type, a
/// relation or a verb it is not shown, which the refusal would name.
/// A type of `set` keeps the relations in force `who` is not
/// shown: an author's view is completed with them ([`as_author`]),
/// and a proposal holds them. `seen` is [`types_shown`] of `o`.
/// A side its author is not shown is first looked for among the names
/// of what `seen` does not hold ([`declare::unshown_tool_problems`]):
/// a declaration that meets none there, as nearly every one, costs no
/// copy of the lake and no pass over its names.
pub(crate) fn tools_as_shown(
    who: &Principal,
    o: &Ontology,
    seen: &Ontology,
    set: &[Declaration],
) -> Result<()> {
    if declare::unshown_tool_problems(o, seen, set).is_empty() {
        return Ok(());
    }
    let (declared, mut whole) = (o.declared(), set.to_vec());
    let sees = Sight::held(who, &declared);
    for d in &mut whole {
        if let Declaration::Type(t) = d
            && let Ok(was) = o.ty(&t.name)
        {
            let named = |r: &RelationDecl| t.relations.iter().any(|x| x.name == r.name);
            let kept = |r: &&RelationDecl| !sees.relation(was, r) && !named(r);
            let kept: Vec<_> = was.relations.iter().filter(kept).cloned().collect();
            t.relations.extend(kept);
        }
    }
    // With the link types the set brings, as its check reads them.
    let problems = |mut o: Ontology, set: &[Declaration]| {
        set.iter().for_each(|d| put_as_appended(&mut o, d, Some));
        Vec::from_iter(set.iter().map(|d| declare::tool_problems(&o, d)))
    };
    // What it is shown is part of the lake: no problem there, none here.
    let all = problems(o.clone(), &whole);
    if all.iter().all(Vec::is_empty) {
        return Ok(());
    }
    let mut seen = seen.clone();
    (seen.verbs).retain(|_, v| sees.lists(&Declaration::Verb(v.decl.clone())));
    granted(all == problems(seen, set))
}

/// The group a setting's record holds: its declaration's, `store` for one
/// of the store's own, none for a host's key it does not declare (063,
/// extended).
pub(crate) fn setting_group(o: &Ontology, key: &str) -> Value {
    use lakelands_api::settings::{STORE_GROUP, lake_setting, unlisted};
    if let Some(e) = o.settings.get(key) {
        return e.decl.group.clone().map_or(Value::Null, Value::String);
    }
    match lake_setting(key).is_some() || unlisted(key).is_some() {
        true => Value::String(STORE_GROUP.to_owned()),
        false => Value::Null,
    }
}

/// A host's setting's value judged as a write of a field of its declared
/// type, in the form that write keeps (063, extended).
fn judged(o: &Ontology, decl: &TypeDecl, s: &SettingDecl, value: &Value) -> Result<Value> {
    let field = FieldDecl {
        ty: s.ty.clone(),
        ..FieldDecl::of("value", s.ty.kind)
    };
    let patch = [("value".into(), value.clone())].into();
    let place = Place::Setting { key: s.key.clone() };
    let kept = check_values(o, decl, &patch, &[field]).map_err(|e| e.within(place))?;
    Ok(kept.get("value").cloned().unwrap_or_else(|| value.clone()))
}

/// A write of `sys.setting` with its group stamped by the store, where the
/// record does not hold it yet, and a declared setting's value judged by
/// its kind: a write that names another group is refused, naming
/// `group` (063, extended). To a writer the declaration is not told
/// (`told`), the key is undeclared: a group it names is judged against
/// none and its value is not judged, so no refusal tells it the
/// declaration; the true group is stamped all the same, and the grant
/// then refuses the write as it refuses an undeclared key's.
fn stamped(
    o: &Ontology,
    decl: &TypeDecl,
    key: &RecordKey,
    before: Option<&BTreeMap<FieldName, Value>>,
    told: &dyn Fn(&SettingDecl) -> bool,
    patch: &mut Patch,
) -> Result<()> {
    let group = setting_group(o, key.as_str());
    let declared = o.settings.get(key.as_str()).filter(|s| told(&s.decl));
    let named = match o.settings.get(key.as_str()) {
        Some(_) if declared.is_none() => Value::Null,
        _ => group.clone(),
    };
    let field = FieldName::from("group");
    if patch.get(&field).is_some_and(|g| *g != named) {
        let at = vec![Place::Setting {
            key: key.to_string(),
        }];
        let case = Case::SettingGroupStamped;
        return Err(invalid_field(&field, Said { at, case }));
    }
    if before.and_then(|b| b.get(&field)).unwrap_or(&Value::Null) != &group {
        patch.insert(field, group);
    }
    let value = FieldName::from("value");
    let held = patch.get(&value).filter(|v| !v.is_null()).cloned();
    if let (Some(s), Some(v)) = (declared, held) {
        let kept = judged(o, decl, &s.decl, &v)?;
        patch.insert(value, kept);
    }
    Ok(())
}

/// What a declaration is and its name, as a refusal names them.
fn naming(d: &Declaration) -> (Naming, String) {
    match d {
        Declaration::Type(t) => (Naming::Type, t.name.to_string()),
        Declaration::ValueType(v) => (Naming::ValueType, v.name.to_string()),
        Declaration::Verb(v) => (Naming::Verb, v.name.to_string()),
        Declaration::Trigger(t) => (Naming::Trigger, t.name.to_string()),
        Declaration::ObjectSet(o) => (Naming::ObjectSet, o.name.to_string()),
        Declaration::Function(f) => (Naming::Function, f.name.to_string()),
        Declaration::Setting(s) => (Naming::Setting, s.key.clone()),
        _ => unreachable!("a declaration of a kind this release does not know"),
    }
}

/// Whether a type computes a reference field, at the write or by a
/// function.
fn computes_a_reference(t: &TypeDecl) -> bool {
    let computes = |f: &FieldDecl| f.derived.is_some() || f.function.is_some();
    t.fields
        .iter()
        .any(|f| f.ty.target.is_some() && computes(f))
}

/// Whether what `write` reaches beside its own record ([`reached`]) can
/// turn on more than the declarations in `o`: on its record, the settings
/// or the clock, through a reference its type computes or, of a call, a
/// path across a reference or a reference it assigns. Where it cannot, and
/// no declaration moved since, it reached when proposed what it reaches now.
pub(crate) fn reaches_by_state(o: &Ontology, write: &ProposedWrite) -> bool {
    let call = |verb: &VerbName| {
        let (Ok(v), Ok(t)) = (o.verb(verb), o.verb(verb).and_then(|v| o.ty(&v.on))) else {
            return true;
        };
        let across = v.reads_across(t, &|n| o.ty(n).ok());
        let reference = |a: &Assignment| {
            t.fields
                .iter()
                .any(|f| f.name == a.field && f.ty.target.is_some())
        };
        let sets = match &v.effect {
            VerbEffect::Write { set } => set.iter().any(reference),
            VerbEffect::External { .. } => false,
        };
        computes_a_reference(t) || across.is_none_or(|a| !a.is_empty()) || sets
    };
    match write {
        ProposedWrite::Upsert { ty, .. } => o.ty(ty).map_or(true, computes_a_reference),
        ProposedWrite::Call { verb, .. } => call(verb),
        _ => false,
    }
}

/// What a proposed write reached when it was proposed at `proposed_at`
/// ([`reached`]), `then` being the lake at the proposal: computed wholly
/// as it stood then, the declarations, the settings in force and the time.
pub(crate) fn reached_then(
    lake: &Lake,
    then: &dyn ProjectionView,
    proposed_at: Timestamp,
    write: &ProposedWrite,
    own: bool,
) -> Result<Vec<(TypeName, RecordKey)>> {
    let ontology = Ontology::at(then)?;
    let settings = crate::lake::settings_in(then, &ontology)?;
    reached(lake, &ontology, (then, &settings, proposed_at), write, own)
}

/// The records the call of `verb` proposed by `logged` read across a
/// reference when it was proposed, computed wholly as it stood then, as
/// [`held_then`] computes its reach.
fn read_then(
    lake: &Lake,
    logged: &Event,
    verb: &VerbName,
    key: Option<&RecordKey>,
    payload: &Patch,
) -> Result<Vec<(TypeName, RecordKey)>> {
    let then = lake.engine.view(Some(logged.position))?;
    let ontology = Ontology::at(then.as_ref())?;
    let Ok(v) = ontology.verb(verb) else {
        return Ok(Vec::new());
    };
    let settings = crate::lake::settings_in(then.as_ref(), &ontology)?;
    let at = (then.as_ref(), &settings, logged.known_at);
    Ok(call_reads(&ontology, at, v, key, payload)?.1)
}

/// The record of `ty` at `key` and, where it is a link, its end records,
/// theirs too where an end is a link, by the end types the link type
/// declares, which [`Sight::ends`] follows: a link's key is made of its
/// ends' keys, so it ends with them.
fn with_ends(o: &Ontology, ty: &TypeName, key: &RecordKey) -> Vec<(TypeName, RecordKey)> {
    let mut out = vec![(ty.clone(), key.clone())];
    let mut next = 0;
    while let Some((t, k)) = out.get(next).cloned() {
        next += 1;
        let parts = lakelands_api::key::key_components(&k);
        let (true, Ok(link), [from, to]) = (links::is_link_type(&t), o.ty(&t), &parts[..]) else {
            continue;
        };
        for (end, part) in [(links::FROM, from), (links::TO, to)] {
            let end = link.fields.iter().find(|f| f.name.as_str() == end);
            let end = end.and_then(|f| f.ty.target.clone());
            out.extend(end.map(|e| (e, RecordKey::from(part.as_str()))));
        }
    }
    out
}

/// The records a proposed write reaches: the record it writes, those its
/// references name, an upsert's or what a call's assignments compute for
/// the record in `view` with these settings at `now`, those the record
/// as planned computes anew or from a field the write sends, those a call
/// reads across a reference ([`call_reads`]), and the end records of each
/// link among them ([`with_ends`]); without the record it writes and its
/// ends where `own` is false.
pub(crate) fn reached(
    lake: &Lake,
    o: &Ontology,
    (view, settings, now): (&dyn ProjectionView, &BTreeMap<String, Value>, Timestamp),
    write: &ProposedWrite,
    own: bool,
) -> Result<Vec<(TypeName, RecordKey)>> {
    let mut across = Vec::new();
    let (ty, key, mut refs) = match write {
        ProposedWrite::Upsert { ty, key, patch, .. } => (ty, Some(key), patch.clone()),
        ProposedWrite::Retire { ty, key, .. } => (ty, Some(key), Patch::new()),
        ProposedWrite::Call {
            verb, key, payload, ..
        } => {
            let Ok(v) = o.verb(verb) else {
                return Ok(Vec::new());
            };
            let (set, far) = call_reads(o, (view, settings, now), v, key.as_ref(), payload)?;
            across = far;
            (&v.on, key.as_ref(), set)
        }
        // A retarget reaches the records its values name in the new target,
        // of the type's fields and of its link types' (lakelands#1071).
        ProposedWrite::Migrate { migration: m, .. } => {
            let mut out = Vec::new();
            for (decl, m) in retargets(o, m)? {
                for (_, target, keys) in retarget_values(view, &decl, &m)? {
                    out.extend(
                        keys.iter()
                            .flat_map(|k| with_ends(o, &target, &k.as_str().into())),
                    );
                }
            }
            return Ok(out);
        }
        _ => return Ok(Vec::new()),
    };
    let decl = o.ty(ty)?;
    if let (false, Some(key), true) = (refs.is_empty(), key, computes_a_reference(decl)) {
        let before = view.get(ty, key)?;
        if let Some(after) = planned(
            lake,
            o,
            (view, settings, now),
            before.as_ref(),
            (ty, key),
            &refs,
        )? {
            let sent: Vec<FieldName> = refs.keys().cloned().collect();
            let was = before.as_ref().filter(|b| !b.retired).map(|b| &b.fields);
            refs.extend(computed_references(decl, was, &after.fields, &sent));
        }
    }
    let mut out: Vec<_> = key
        .filter(|_| own)
        .map(|k| (ty.clone(), k.clone()))
        .into_iter()
        .collect();
    out.extend(refs.iter().filter_map(|(n, v)| {
        let f = decl.fields.iter().find(|f| &f.name == n)?;
        Some((f.ty.target.clone()?, RecordKey::from(v.as_str()?)))
    }));
    out.extend(across);
    Ok(out.iter().flat_map(|(t, k)| with_ends(o, t, k)).collect())
}

/// The record `before` at `key` of `ty` as the fold would leave it after a
/// write of `patch`, with these settings at `now`: its derived values, an
/// initial value and a field waiting for its backing too.
pub(crate) fn planned(
    lake: &Lake,
    o: &Ontology,
    (view, settings, now): (&dyn ProjectionView, &BTreeMap<String, Value>, Timestamp),
    before: Option<&StoredRecord>,
    (ty, key): (&TypeName, &RecordKey),
    patch: &Patch,
) -> Result<Option<StoredRecord>> {
    let made = before.is_none_or(|b| b.retired);
    let version = Version(before.map_or(0, |b| b.version.0) + 1);
    let (ty, key, patch) = (ty.clone(), key.clone(), patch.clone());
    let body = EventBody::RecordUpserted {
        ty,
        key,
        version,
        patch,
        made,
    };
    let event = provisional(lake, Position::GENESIS, &Draft::system(now, body));
    let plans = record::Plans::default();
    let outputs = &mut Outputs::planning(view, &plans, &lake.keys);
    record::apply(o, before, &event, settings, outputs)
}

/// What a call of the verb `v` with `payload` would write to the record at
/// `key` in `view`, with these settings at `now`, whatever form an
/// assignment takes (one that cannot be computed refuses the accept by
/// itself); and the records its `requires` and its values read across a
/// reference there, whoever holds them.
pub(crate) fn call_reads(
    o: &Ontology,
    (view, settings, now): (&dyn ProjectionView, &BTreeMap<String, Value>, Timestamp),
    v: &VerbDecl,
    key: Option<&RecordKey>,
    payload: &Patch,
) -> Result<(Patch, Vec<(TypeName, RecordKey)>)> {
    let far = |ty: &TypeName, k: &RecordKey| view.get(ty, k);
    let link = |r: &RelationName| key.map_or(Ok(None), |k| link_from(view, &v.on, r, k));
    let across = Across::new(
        o,
        o.ty(&v.on)?,
        (&far, &link, &far),
        (u32::MAX, (settings, now)),
    );
    let mut set = None;
    if let Some(k) = key
        && let Some(r) = view.get(&v.on, k)?
    {
        let _ = requires(settings, v, k, &r, now, &across);
        set = assigned(o, settings, v, &r, payload, (now, &across)).ok();
    }
    let read = across.read.borrow().keys().cloned().collect();
    Ok((set.unwrap_or_default(), read))
}

/// Whether a migration changes nothing: its `to` is the type in force, it
/// renames no field and its plan fills no record (027 and 050, extended).
fn unchanged(o: &Ontology, m: &Migration, plan: &MigrationPlan) -> bool {
    let fills = plan.report.filled.iter().any(|f| f.records > 0);
    m.renames.is_empty() && !fills && o.in_force(&Declaration::Type(m.to.clone()))
}

/// The lake's head: the position of its last event.
fn head(w: &Writer) -> Position {
    w.tip.as_ref().map_or(Position::GENESIS, |t| t.0)
}

/// A migration report's id: named by what it says to its reader and where
/// the lake stood, so that an apply after the lake moved is refused, and
/// computed by its reader from what it holds, which names nothing hidden
/// (024, extended).
pub(crate) fn report_id(report: &MigrationReport, head: Option<Position>) -> String {
    let report = MigrationReport {
        report_id: String::new(),
        ..report.clone()
    };
    lakelands_core::canonical::sha256_hex(&serde_json::json!({
        "report": serde_json::to_value(&report).expect("a report serialises"),
        "head": head.map_or(0, |p| p.0),
    }))
}

/// The next position in an append that already holds `n` drafts.
pub(crate) fn position_after(w: &Writer, n: usize) -> Position {
    let first = w.tip.as_ref().map_or(Position::GENESIS, |t| t.0.next());
    Position(first.0 + n as u64)
}

/// Refuse a verb whose `requires` does not hold on `record`, and before
/// that one whose `requires` reads across a reference that reaches no
/// record (`across`).
fn requires(
    settings: &BTreeMap<String, Value>,
    verb: &VerbDecl,
    key: &RecordKey,
    record: &StoredRecord,
    now: Timestamp,
    across: &Across<'_>,
) -> Result<()> {
    let Some(r) = &verb.requires else {
        return Ok(());
    };
    let at = [
        Place::Verb {
            name: verb.name.clone(),
        },
        Place::Requires,
    ];
    let e = declare::condition(r, Eval::Verb, &at)?;
    let record = RecordScope {
        fields: &record.fields,
        meta: Some(&record.meta),
        settings: Some(settings),
        now,
    };
    let payload = None;
    let scope = VerbScope {
        fields: record.fields,
        record,
        payload,
        across,
    };
    let holds = expr::holds(&e, &scope).unwrap_or(false);
    across.reached()?;
    if holds {
        return Ok(());
    }
    let (verb, ty, key) = (verb.name.clone(), verb.on.clone(), key.clone());
    let case = Case::RequiresUnmet { verb, ty, key };
    Err(refused(Refusal::Precondition(Box::new(case.into()))))
}

/// The values a write verb's assignments give `record` for a call with
/// `payload`, as the call computes them; one that reads across a
/// reference that reaches no record is refused as `across` says.
fn assigned(
    o: &Ontology,
    settings: &BTreeMap<String, Value>,
    verb: &VerbDecl,
    record: &StoredRecord,
    payload: &Patch,
    (now, across): (Timestamp, &Across<'_>),
) -> Result<Patch> {
    let VerbEffect::Write { set } = &verb.effect else {
        return Ok(Patch::new());
    };
    let mut patch = Patch::new();
    let record = RecordScope {
        fields: &record.fields,
        meta: Some(&record.meta),
        settings: Some(settings),
        now,
    };
    let payload = Some(payload);
    let scope = VerbScope {
        fields: record.fields,
        record,
        payload,
        across,
    };
    for a in set {
        let at = [
            Place::Verb {
                name: verb.name.clone(),
            },
            Place::Field {
                name: a.field.clone(),
            },
        ];
        let e = declare::condition(&a.value, Eval::Verb, &at)?;
        let v = expr::eval(&e, &scope);
        across.reached()?;
        let v = v.map_err(|why| {
            let (verb, field, why) = (verb.name.clone(), a.field.clone(), Box::new(why));
            invalid(Case::VerbCannotCompute { verb, field, why })
        })?;
        let kind = o
            .ty(&verb.on)?
            .fields
            .iter()
            .find(|f| f.name == a.field)
            .map(|f| f.ty.kind);
        let json = kind
            .and_then(|k| expr::eval::to_json(&v, k, None))
            .ok_or_else(|| {
                let (verb, field) = (verb.name.clone(), a.field.clone());
                invalid(Case::VerbValueUnfit { verb, field })
            })?;
        patch.insert(a.field.clone(), json);
    }
    Ok(patch)
}

/// The record a reference names, as one reader of it reaches it.
type Far<'a> = &'a dyn Fn(&TypeName, &RecordKey) -> Result<Option<StoredRecord>>;

/// The link the call's record holds under a relation ([`link_from`]).
type Link<'a> = &'a dyn Fn(&RelationName) -> Result<Option<RecordKey>>;

/// The key of the one live link `from` of `ty` holds over the relation
/// `r` in `view`, whoever is answered it: a relation a path reads holds one
/// at most from a record (055, extended). A verb is decided before what it
/// sets off, so no draft of its decision is a link.
fn link_from(
    view: &dyn ProjectionView,
    ty: &TypeName,
    r: &RelationName,
    from: &RecordKey,
) -> Result<Option<RecordKey>> {
    use lakelands_api::query::Walk;
    let edges = view.edges(ty, std::slice::from_ref(from), Some(r), Walk::Out)?;
    let mut keys = edges.into_iter().flatten();
    Ok(keys.next().map(|e| links::link_key(from, &e.to.1)))
}

/// What a verb's `requires` and values read across a reference: a path
/// steps through a reference field to the one record it names and reads a
/// field there ([`TypeDecl::stepped`]), each record read once a call, as
/// `far` reaches it, and only where the evaluation comes to the path.
struct Across<'a> {
    o: &'a Ontology,
    on: &'a TypeDecl,
    far: Far<'a>,
    link: Link<'a>,
    /// The record at a key, whoever is answered it.
    stored: Far<'a>,
    /// The most steps a path may take: the lake's `max_depth` at a call.
    max: u32,
    /// The settings and the clock a derived relation's `where` reads: the
    /// call's.
    clock: (&'a BTreeMap<String, Value>, Timestamp),
    /// The records read, none where the reference reached none.
    read: RefCell<BTreeMap<(TypeName, RecordKey), Option<StoredRecord>>>,
    /// The refusal of the first path that reached no record.
    refusal: RefCell<Option<Error>>,
}

impl<'a> Across<'a> {
    fn new(
        o: &'a Ontology,
        on: &'a TypeDecl,
        (far, link, stored): (Far<'a>, Link<'a>, Far<'a>),
        (max, clock): (u32, (&'a BTreeMap<String, Value>, Timestamp)),
    ) -> Self {
        let (read, refusal) = (RefCell::default(), RefCell::default());
        Across {
            o,
            on,
            far,
            link,
            stored,
            max,
            clock,
            read,
            refusal,
        }
    }

    /// The record `named`, read once a call as `far` reaches it.
    fn reach(&self, named: &(TypeName, RecordKey)) -> Option<StoredRecord> {
        let known = self.read.borrow().get(named).cloned();
        let far = || {
            (self.far)(&named.0, &named.1)
                .map_err(|e| self.refuse(e))
                .ok()
        };
        let found = known.unwrap_or_else(|| far().flatten());
        self.read.borrow_mut().insert(named.clone(), found.clone());
        found
    }

    /// Note the call's first refusal.
    fn refuse(&self, e: Error) {
        self.refusal.borrow_mut().get_or_insert(e);
    }

    /// The one record the derived relation `r` reaches from a record
    /// holding `fields`: its target's record at the key the match names,
    /// where it holds the matched values as stored and the relation's
    /// `where` holds over the two with the call's clock (044, extended).
    /// None reached is no edge, never a refusal: no reference names it.
    fn matched(
        &self,
        fields: &BTreeMap<FieldName, Value>,
        r: &RelationDecl,
    ) -> Option<StoredRecord> {
        let d = r.derived.as_ref()?;
        let target = self.o.ty(&r.target).ok()?;
        let value = |p: &lakelands_api::ontology::MatchPair| {
            Some((
                p.to.clone(),
                fields.get(&p.from).filter(|v| !v.is_null())?.clone(),
            ))
        };
        let values: BTreeMap<FieldName, Value> =
            d.pairs.iter().map(value).collect::<Option<_>>()?;
        let key = lakelands_api::key::record_key(target, &values)?;
        let named = (r.target.clone(), key);
        let Some(far) = self.reach(&named) else {
            // One there and not answered is held from the judged principal,
            // which a hold placed since a proposal can make so: never no
            // match, which would let a hold grant `not exists` (055).
            let stored = (self.stored)(&named.0, &named.1).ok().flatten();
            if stored.is_some_and(|r| !r.retired) {
                self.refuse(refused(Refusal::NotGranted));
            }
            return None;
        };
        // Every pair, two onto one field included, holds as stored.
        let holds =
            |p: &lakelands_api::ontology::MatchPair| far.fields.get(&p.to) == fields.get(&p.from);
        if !d.pairs.iter().all(holds) {
            return None;
        }
        let Some(c) = &d.filter else { return Some(far) };
        let (settings, now) = self.clock;
        let e = declare::condition(c, Eval::Record, &[]).ok()?;
        let ends = crate::reads::Ends {
            ends: [fields, &far.fields],
            settings,
            now,
        };
        expr::holds(&e, &ends).unwrap_or(false).then_some(far)
    }

    /// Whether the path that begins at `relation` reaches a record: the one
    /// a derived relation matches, or the one link the record holds under a
    /// relation carried by links.
    fn exists(&self, fields: &BTreeMap<FieldName, Value>, relation: &FieldName) -> bool {
        // The match or the link is a step toward `max_depth`.
        if self.max == 0 {
            self.refuse(invalid(Case::StepTooDeep { max: 0, asked: 1 }));
            return false;
        }
        if let Some(r) = self.on.derived_step(relation) {
            return self.matched(fields, r).is_some();
        }
        let relation = RelationName::from(relation.as_str());
        let lt = links::link_type_name(&self.on.name, &relation);
        let Some(link) = (self.link)(&relation)
            .map_err(|e| self.refuse(e))
            .ok()
            .flatten()
        else {
            return false;
        };
        // Its caller holds nothing of the link from it (055).
        let found = self.reach(&(lt, link)).is_some();
        if !found {
            self.refuse(refused(Refusal::NotGranted));
        }
        found
    }

    /// The value at `path` from a record of the verb's type holding
    /// `fields`. A reference that is null reaches nothing and the path is
    /// null; one that names a record not reached is noted
    /// ([`Self::reached`]). A path that begins at a relation reads the one
    /// link the record holds under it, none reading null.
    fn value(&self, fields: &BTreeMap<FieldName, Value>, path: &[FieldName]) -> Option<Value> {
        let ty = |n: &TypeName| self.o.ty(n).ok();
        // A path that begins at a derived relation goes on from the record
        // it matches; the match is a step.
        let derived = self.on.derived_step(&path[0]).filter(|_| path.len() > 1);
        let (from, path) = match derived {
            Some(r) => (ty(&r.target)?, &path[1..]),
            None => (self.on, path),
        };
        let steps = from.stepped(path, &ty);
        let asked = steps.len().saturating_sub(1) as u32 + u32::from(derived.is_some());
        let refuse = |e: Error| self.refuse(e);
        if asked > self.max {
            refuse(invalid(Case::StepTooDeep {
                max: self.max,
                asked,
            }));
            return None;
        }
        let reach = |named: &(TypeName, RecordKey)| self.reach(named);
        let mut at: Option<StoredRecord> = match derived {
            Some(r) => Some(self.matched(fields, r)?),
            None => None,
        };
        let by_link = steps
            .first()
            .filter(|s| derived.is_none() && s.0.name != self.on.name);
        if let Some((lt, _)) = by_link {
            let relation = RelationName::from(path[0].as_str());
            let link = (self.link)(&relation).map_err(refuse).ok()??;
            // Its caller holds nothing of the link from it (055): one not
            // reached anyway is refused as the verb not its, naming nothing.
            let found = reach(&(lt.name.clone(), link));
            if found.is_none() {
                refuse(refused(Refusal::NotGranted));
            }
            at = Some(found?);
        }
        for step in steps[usize::from(by_link.is_some())..].windows(2) {
            let (via, to) = (&step[0].1.name, &step[1].0.name);
            let held = at.as_ref().map_or(fields, |r| &r.fields);
            let named = (to.clone(), RecordKey::from(held.get(via)?.as_str()?));
            let found = reach(&named);
            if found.is_none() {
                let (field, ty, key) = (via.clone(), to.clone(), named.1.clone());
                refuse(constraint(
                    Some(via.clone()),
                    Case::NotLive { field, ty, key },
                ));
            }
            at = Some(found?);
        }
        let rest = &path[steps.len().saturating_sub(1)..];
        expr::eval::lookup(at.as_ref().map_or(fields, |r| &r.fields), rest)
    }

    /// Refuse a call that read across a reference to no record: as a
    /// write that sends that reference is refused (032), whatever else
    /// the condition would answer.
    fn reached(&self) -> Result<()> {
        self.refusal.borrow_mut().take().map_or(Ok(()), Err)
    }
}

/// The scope a verb's `requires` and assignments see: the record's fields
/// and their provenance, what a path reads across a reference, and in an
/// assignment `payload.<name>` for the call's payload. A set's `where`
/// and a trigger's `when` see it over their own scope, `record`, with no
/// payload (055, extended).
struct VerbScope<'a, S = RecordScope<'a>> {
    record: S,
    fields: &'a BTreeMap<FieldName, Value>,
    payload: Option<&'a Patch>,
    across: &'a Across<'a>,
}

impl<S: Scope> Scope for VerbScope<'_, S> {
    fn value(&self, path: &[FieldName]) -> Option<Value> {
        let member = lakelands_core::derive::payload_member(path);
        match self.payload.zip(member) {
            Some((payload, name)) => payload.get(name).filter(|v| !v.is_null()).cloned(),
            None => self.across.value(self.fields, path),
        }
    }

    fn previous(&self, field: &FieldName) -> Option<Value> {
        self.record.previous(field)
    }

    fn in_write(&self) -> bool {
        self.record.in_write()
    }

    fn retiring(&self) -> bool {
        self.record.retiring()
    }

    fn meta(&self, field: &FieldName) -> Option<lakelands_api::FieldMeta> {
        self.record.meta(field)
    }

    fn setting(&self, key: &str) -> Option<Value> {
        self.record.setting(key)
    }

    fn now(&self) -> Timestamp {
        self.record.now()
    }

    fn exists(&self, relation: &FieldName) -> Option<bool> {
        Some(self.across.exists(self.fields, relation))
    }
}

/// What a decision's own events carry beside the call's writer and
/// context, set before the record they make is computed, so that what is
/// judged on that record (a trigger's guard) reads the provenance the fold
/// keeps: aging writes as the system, with its rule as the procedure,
/// ingest as its source states each record, an accept as the proposer and
/// caused by the acceptance, and a verb's write caused by its call.
#[derive(Clone)]
pub(crate) struct Envelope {
    /// The authority that writes them.
    pub origin: Origin,
    /// Who writes them, where it is not the caller.
    pub writer: Option<String>,
    /// The event they follow from.
    pub cause: Option<Position>,
    /// The procedure that produced their values.
    pub procedure: Option<Procedure>,
    /// A chosen id.
    pub id: Option<EventId>,
    /// The source they come from.
    pub source: Option<lakelands_api::SourceId>,
    /// When the source observed the values.
    pub seen_at: Option<Timestamp>,
    /// Where the source read them.
    pub upstream: Option<lakelands_api::Upstream>,
    /// What the source's key seals of a `record_ingested`'s patch.
    pub sealed: Option<lakelands_api::event::Sealed>,
}

impl Envelope {
    /// That of events `origin` writes, carrying nothing more.
    pub(crate) fn of(origin: Origin) -> Envelope {
        Envelope {
            origin,
            writer: None,
            cause: None,
            procedure: None,
            id: None,
            source: None,
            seen_at: None,
            upstream: None,
            sealed: None,
        }
    }
}

/// What one write is decided in, from the writer's lock to its append: the
/// writer and the view it reads, what its drafts so far leave, its time,
/// where its next event lands, who is judged at which door, and the
/// envelope of its own events.
pub(crate) struct Decide<'a> {
    call: &'a Call<'a>,
    w: &'a Writer,
    view: &'a dyn ProjectionView,
    /// The records and memberships its drafts change, not in the view yet.
    pub pending: Pending,
    now: Timestamp,
    /// Where the event being decided lands; [`Self::reactions`] moves it
    /// past that event and what it sets off.
    next: Position,
    /// Who is judged: the caller, or at an accept the proposer.
    pub who: &'a Principal,
    /// The door its records are written through.
    pub door: Door,
    /// What its own events carry.
    pub envelope: Envelope,
    /// Whether what its far writes set off is held to `max_set_changes`:
    /// a purge's preflight counts it instead.
    pub bound: bool,
}

impl Decide<'_> {
    /// One of the decision's own events.
    pub(crate) fn draft(&self, body: EventBody) -> Draft {
        let e = self.envelope.clone();
        let ingested = matches!(body, EventBody::RecordIngested { .. });
        let own = self.call.draft(self.now, e.origin, body);
        Draft {
            sealed: e.sealed.filter(|_| ingested),
            writer: e.writer.unwrap_or(own.writer),
            cause: e.cause,
            procedure: e.procedure,
            id: e.id,
            source: e.source,
            seen_at: e.seen_at,
            upstream: e.upstream,
            ..own
        }
    }

    /// The record of `decl` at `key`, as the drafts so far leave it, for
    /// the judged principal.
    pub(crate) fn target(&self, decl: &TypeDecl, key: &RecordKey) -> Result<Target> {
        let (view, pending, who) = (self.view, &self.pending, self.who);
        let before = pending.record(view, &decl.name, key)?;
        let mine = match &before {
            Some(b) => {
                let sees = Sight::held(who, &self.w.ontology);
                sees.answers(&decl.name, b) && ends(view, pending, &sees, decl, &b.fields)?
            }
            None => false,
        };
        Ok(Target { before, mine })
    }

    /// The live record the judged principal reaches at `key`, at the
    /// version `expected`, compared before anything else is judged on the
    /// record (024): what a retire takes and a verb runs on. A retired one
    /// is not found, and to a holder of `only` another's is not found, as a
    /// missing one is (051); so is a restricted one, or a link with a
    /// restricted end, to a writer without the grant (006), and a setting
    /// it is not told (063).
    fn reached(
        &self,
        decl: &TypeDecl,
        key: &RecordKey,
        expected: Option<Version>,
    ) -> Result<StoredRecord> {
        let t = self.target(decl, key)?;
        let told = |b: &StoredRecord| Sight::held(self.who, &self.w.ontology).told(b).is_some();
        match t.before {
            Some(b) if t.mine && !b.retired && told(&b) => {
                cas(expected, Some(&b))?;
                Ok(b)
            }
            _ => Err(Error::NotFound {
                missing: Missing::Record {
                    ty: decl.name.clone(),
                    key: key.clone(),
                },
            }),
        }
    }

    /// A retirement of `ty` at `key`. It answers in its own order: whether
    /// the judged principal may retire the type before whether the record
    /// is there, since a key it does not reach leaves nothing it could
    /// make.
    fn retire(
        &mut self,
        ty: &TypeName,
        key: &RecordKey,
        expected: Option<Version>,
    ) -> Result<Plan> {
        let decl = self.w.ontology.ty(ty)?;
        rights::not_a_view(decl)?;
        granted(rights::may_retire(self.who, decl))?;
        untold_setting(&Sight::held(self.who, &self.w.ontology), decl, key)?;
        let before = self.reached(decl, key, expected)?;
        // A setting naming what its writer is not told keeps that: the rest
        // of it is cleared, and the setting is not told to it after (063).
        let sees = Sight::held(self.who, &self.w.ontology);
        let stored = before.fields.get("value").filter(|_| sees.narrows(ty));
        let hidden = hidden_from(&sees, &self.w.ontology);
        let kept =
            stored.map(|v| lakelands_api::settings::kept(key.as_str(), v, Value::Null, &hidden));
        if kept.is_some_and(|k| !k.is_null()) {
            let patch = Patch::from([("value".into(), Value::Null)]);
            let (patch, t) = (self.checked(decl, key, &patch)?, self.target(decl, key)?);
            let plan = self.write_as(decl, key, t, patch, expected, true)?;
            return self.planned(plan);
        }
        let own = self.draft(retirement(ty, key, before.version));
        let (drafts, after) = (self.retiring(&before, own)?, None);
        Ok(Plan { drafts, after })
    }

    /// What `op` names, as the judged principal is shown it (006 and 024,
    /// extended): one not shown its type is answered as for a type that is
    /// not there, and one not shown the relation a link door names as for
    /// a relation the type does not carry. A verb is shown by its door.
    fn sight(&self, op: Op<'_>) -> Result<()> {
        let (ty, relation) = match op {
            Op::Upsert(ty, ..) | Op::Retire(ty, ..) => (ty, None),
            Op::Link((ty, _, r, _), _) | Op::Unlink((ty, _, r, _)) => (ty, Some(r)),
            Op::Call(..) => return Ok(()),
        };
        let sees = Sight::held(self.who, &self.w.ontology);
        let decl = sees.ty(ty)?;
        let mut named = decl.relations.iter().filter(|r| Some(&r.name) == relation);
        match relation {
            Some(n) if named.any(|r| !sees.relation(decl, r)) => Err(no_link_relation(ty, n)),
            _ => Ok(()),
        }
    }

    /// A link door's own questions, with the link type and the relation
    /// they find: the relation is carried by link records, on a type that
    /// is no view; and for `link`, its links are authored and no end is
    /// among the fields. Another door's op has none.
    fn shape(&self, op: Op<'_>) -> Result<Option<(TypeName, RelationDecl)>> {
        let ((ty, _, relation, _), fields) = match op {
            Op::Link(ends, fields) => (ends, Some(fields)),
            Op::Unlink(ends) => (ends, None),
            _ => return Ok(None),
        };
        let decl = self.w.ontology.ty(ty)?;
        rights::not_a_view(decl)?;
        let mut carried = decl.relations.iter();
        let r = carried.find(|r| &r.name == relation && r.carried_by_links());
        let r = r.ok_or_else(|| no_link_relation(ty, relation))?;
        let lt = links::link_type_name(ty, relation);
        self.w.ontology.ty(&lt)?;
        if fields.is_some() && r.authority != Authority::Authored {
            return Err(refused(Refusal::WrongDoor));
        }
        let ends = [links::FROM, links::TO].map(FieldName::from);
        let mut ends = ends
            .iter()
            .filter(|e| fields.is_some_and(|f| f.contains_key(*e)));
        if let Some(end) = ends.next() {
            return Err(invalid_field(
                end,
                Case::LinkEndAsField { end: end.clone() },
            ));
        }
        Ok(Some((lt, r.clone())))
    }

    /// Decide `op` for the judged principal, its own event landing next,
    /// in the one order every door shares: what the principal is shown of
    /// what the op names ([`Self::sight`]); a link door's own questions
    /// ([`Self::shape`]); the record's own checks, by [`Self::record`],
    /// [`Self::retire`] or [`Self::run_verb`]; a link's cardinality, which
    /// reads other records, so the grants come first (024); then what the
    /// write sets off. `call`, `propose` and `accept` run it as the four
    /// record doors do, so what one refuses the others refuse in the same
    /// words.
    pub(crate) fn plan(&mut self, op: Op<'_>, expected: Option<Version>) -> Result<Plan> {
        self.sight(op)?;
        match (op, self.shape(op)?) {
            (Op::Upsert(ty, key, patch), _) => {
                let plan = self.record(ty, key, patch, expected, None)?;
                self.planned(plan)
            }
            (Op::Retire(ty, key), _) => self.retire(ty, key, expected),
            (Op::Call(verb, key, payload), _) => self.run_verb(verb, key, payload, expected),
            (Op::Link((ty, key, _, to), fields), Some((lt, r))) => {
                let mut patch = fields.clone();
                for (end, k) in [(links::FROM, key), (links::TO, to)] {
                    patch.insert(end.into(), Value::String(k.to_string()));
                }
                let plan = self.record(&lt, &links::link_key(key, to), &patch, expected, None)?;
                let sees = Sight::held(self.who, &self.w.ontology);
                cardinality((self.view, &sees), None, (ty, &r), key, to)?;
                self.planned(plan)
            }
            (Op::Unlink((_, key, _, to)), Some((lt, _))) => {
                self.retire(&lt, &links::link_key(key, to), expected)
            }
            (Op::Link(..) | Op::Unlink(..), None) => unreachable!("a link door has its relation"),
        }
    }

    /// A patch as the door takes it from the judged principal: the key's
    /// form, then the door and the grant of every field it carries, then
    /// its values, each in its written form.
    pub(crate) fn checked(&self, decl: &TypeDecl, key: &RecordKey, patch: &Patch) -> Result<Patch> {
        check_key(key)?;
        rights::check_write(self.who, decl, self.door, patch)
            .map_err(|e| self.verbs_only(decl, e))?;
        check_values(&self.w.ontology, decl, patch, &decl.fields)
    }

    /// A field only verbs write, refused here: named the verbs its writer is
    /// shown and may call, else `wrong_door` (lakelands#1130).
    fn verbs_only(&self, decl: &TypeDecl, e: Error) -> Error {
        let Error::Refused {
            field: Some(field),
            reason: Refusal::WrongDoor,
        } = &e
        else {
            return e;
        };
        let ours = |f: &FieldDecl| &f.name == field && f.authority == Authority::Verb;
        let sees = Sight::held(self.who, &self.w.ontology);
        let sets = |v: &VerbDecl| matches!(&v.effect, VerbEffect::Write { set } if set.iter().any(|a| &a.field == field));
        let verbs: Vec<VerbName> = (self.w.ontology.verbs.values().map(|e| &e.decl))
            .filter(|v| v.on == decl.name && sets(v) && sees.verb_shown(v))
            .map(|v| v.name.clone())
            .collect();
        match decl.fields.iter().any(ours) && !verbs.is_empty() {
            true => Error::Refused {
                field: Some(field.clone()),
                reason: Refusal::VerbsOnly { verbs },
            },
            false => e,
        }
    }

    /// Decide one authored record write (`upsert`, a verb, aging): its
    /// event, which lands next, and the record it makes. `reached` is the
    /// record a verb read at the key already ([`Self::reached`]).
    pub(crate) fn record(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        patch: &Patch,
        expected: Option<Version>,
        reached: Option<StoredRecord>,
    ) -> Result<RecordPlan> {
        let decl = self.w.ontology.ty(ty)?;
        if patch.is_empty() {
            return Err(invalid(Case::PatchEmpty));
        }
        let patch = self.checked(decl, key, patch)?;
        let t = match reached {
            Some(r) => Target {
                before: Some(r),
                mine: true,
            },
            None => self.target(decl, key)?,
        };
        self.write(decl, key, t, patch, expected)
    }

    /// Each constraint of `decl` that the write moves holds on `after`
    /// (lakelands#1119). Whether the write is judged is told by what it
    /// creates and sends, and the derived fields those feed, never by a
    /// value: one who does not read every field a judged constraint reads
    /// is not granted the write, whether it would hold or not. A source's
    /// window does not judge a constraint that admits at ingest.
    fn constraints_hold(
        &self,
        decl: &TypeDecl,
        live: Option<&StoredRecord>,
        after: &StoredRecord,
        sent: &[FieldName],
    ) -> Result<()> {
        let (w, who) = (self.w, self.who);
        let plan = w.plans.get(&w.ontology, decl);
        let reads = |f: &FieldName| {
            let mut fields = decl.fields.iter().filter(|d| &d.name == f);
            fields.any(|d| rights::field_access(who, decl, d).reads())
        };
        for c in &plan.constraints {
            let admits = matches!(self.door, Door::Source) && !c.decl.at_ingest.refuses();
            if admits || (live.is_some() && !sent.iter().any(|f| c.fed.contains(f))) {
                continue;
            }
            granted(c.reads.iter().all(reads))?;
            let before = live.map(|b| &b.fields);
            let moved = |f: &FieldName| before.and_then(|b| b.get(f)) != after.fields.get(f);
            if before.is_some() && !c.reads.iter().any(moved) {
                continue;
            }
            let scope = WriteScope {
                after: &after.fields,
                before,
                retiring: false,
                meta: Some(&after.meta),
                settings: Some(&w.settings),
                now: self.now,
            };
            if !expr::holds(&c.holds, &scope).unwrap_or(false) {
                let (constraint, ty) = (c.decl.name.clone(), decl.name.clone());
                let key = after.key.clone();
                let case = Case::ConstraintBroken {
                    constraint,
                    ty,
                    key,
                };
                return Err(refused(Refusal::Constraint(Box::new(case.into()))));
            }
        }
        Ok(())
    }

    /// The rest of a record write, authored or from a source (`ingest`),
    /// once its patch is checked and its target read.
    pub(crate) fn write(
        &self,
        decl: &TypeDecl,
        key: &RecordKey,
        t: Target,
        patch: Patch,
        expected: Option<Version>,
    ) -> Result<RecordPlan> {
        self.write_as(decl, key, t, patch, expected, false)
    }

    /// [`Self::write`], or, `clearing`, a retire's write of a setting that
    /// keeps what its writer is not told: its null value clears the rest.
    fn write_as(
        &self,
        decl: &TypeDecl,
        key: &RecordKey,
        t: Target,
        mut patch: Patch,
        expected: Option<Version>,
        clearing: bool,
    ) -> Result<RecordPlan> {
        let (w, view, who) = (self.w, self.view, self.who);
        let sees = Sight::held(who, &w.ontology);
        untold_setting(&sees, decl, key)?;
        // A holder of `only` is judged against the record it owns at the
        // key: another's is, to its write, a key that is free, refused
        // only once every check has passed, so no probe tells them apart
        // (051).
        let live = t.live();
        // A retired record the writer is not answered is another's too: its
        // hold is not written into (006).
        let other = live.is_none()
            && t.before
                .as_ref()
                .is_some_and(|b| !b.retired || sees.withheld(b));
        if live.is_none() {
            fill_key_fields(w, decl, key, &mut patch, self.door, who)?;
        }
        if decl.name.as_str() == SETTING_TYPE {
            let told = |s: &SettingDecl| sees.setting_visible(s);
            let before = live.map(|b| &b.fields);
            stamped(&w.ontology, decl, key, before, &told, &mut patch)?;
        }
        // A setting keeps what its writer is not told of the value it
        // replaces, after what it writes, and is judged as it was written,
        // before its version is: one of a type it is not shown is answered
        // as one never declared (063).
        let declared = |t: &TypeName| w.ontology.ty(t).ok();
        let shown = |t: &TypeName| sees.describe_type(t);
        let (mut written, mut untold) = (None, false);
        if sees.narrows(&decl.name) {
            let value = patch.get("value").cloned();
            lakelands_api::seam::check_setting(key.as_str(), value.as_ref(), &declared, &shown)?;
            // A record it is not told is none to its write, its version too.
            untold = live.is_some_and(|b| sees.told(b).is_none());
            // A null it writes is no value: refused as a null over its twin is.
            let merged = clearing || value.as_ref().is_none_or(|v| !v.is_null());
            written = Some(value);
            if let (Some(b), Some(v), true) = (live, patch.get_mut("value"), merged) {
                let stored = b.fields.get("value").unwrap_or(&Value::Null);
                let hidden = hidden_from(&sees, &w.ontology);
                *v = lakelands_api::settings::kept(key.as_str(), stored, v.take(), &hidden);
            }
        }
        check_key_agrees(decl, key, live, &patch)?;
        // An authored write of a link has both ends live and answered
        // (035). `link` sends them; a verb's patch holds only what it sets,
        // so the ends the record holds are judged beside it. A source's
        // window is judged by what it sends (016).
        let end = |f: &FieldName| [links::FROM, links::TO].contains(&f.as_str());
        let authored = !matches!(self.door, Door::Source) && links::is_link_type(&decl.name);
        let held = live.filter(|_| authored);
        let held = held.into_iter().flat_map(|b| &b.fields);
        let ends: Patch = (held.filter(|(f, _)| end(f) && !patch.contains_key(*f)))
            .map(|(f, v)| (f.clone(), v.clone()))
            .collect();
        for judged in [&patch, &ends] {
            let (pending, o) = (&self.pending, &w.ontology);
            check_references(view, pending, o, &decl.name, key, judged, who)?;
        }
        cas(expected, t.own().filter(|_| !untold))?;
        // One that leaves the value of a record it is not told is refused
        // as a write without a value is.
        if untold && written.as_ref().is_some_and(Option::is_none) {
            let (ty, field) = (decl.name.clone(), FieldName::from("value"));
            let case = Case::Required {
                ty,
                field: field.clone(),
            };
            return Err(constraint(Some(field), case));
        }
        let sent: Vec<FieldName> = patch.keys().cloned().collect();
        let version = Version(t.before.as_ref().map_or(0, |b| b.version.0) + 1);
        // Said once, here, of what this writer holds, its own drafts too.
        let made = live.is_none();
        let (ty, key) = (decl.name.clone(), key.clone());
        let draft = self.draft(match self.door {
            Door::Authored | Door::Verb => EventBody::RecordUpserted {
                ty,
                key,
                version,
                patch,
                made,
            },
            Door::Source => EventBody::RecordIngested {
                ty,
                key,
                version,
                patch,
                made,
            },
        });
        let before = t.before.as_ref().filter(|_| !other);
        let after = self.apply(before, &draft)?;
        record::check_required(decl, &after.fields)?;
        self.constraints_hold(decl, live.filter(|_| !other), &after, &sent)?;
        if after.ty.as_str() == AGING_RULE_TYPE {
            let rule = crate::aging::check(&w.ontology, &after.fields)
                .map_err(|(field, said)| invalid_field(&field, *said))?;
            crate::aging::check_authority(who, &w.ontology, &rule)?;
        }
        if after.ty.as_str() == SETTING_TYPE && written.is_none() {
            let value = after.fields.get("value");
            lakelands_api::seam::check_setting(after.key.as_str(), value, &declared, &shown)?;
        }
        // A reference the record computes is judged as a written one, from
        // the record as planned (032). Its key is computed from fields its
        // writer may not all read, so one who may not read the reference
        // is told only that the write is not granted. An accept judges
        // as one who reads every field; what its reviewer is told is
        // `told`'s.
        let computed = computed_references(decl, live.map(|b| &b.fields), &after.fields, &sent);
        let reads = |f: &FieldName| {
            let mut fields = decl.fields.iter().filter(|d| &d.name == f);
            fields.any(|d| rights::field_access(who, decl, d).reads())
        };
        let (pending, o) = (&self.pending, &w.ontology);
        match check_references(view, pending, o, &decl.name, &after.key, &computed, who) {
            Err(Error::Refused { field: Some(f), .. }) if !reads(&f) => granted(false)?,
            judged => judged?,
        }
        granted(!other && owns(view, &self.pending, &sees, decl, &after.fields)?)?;
        Ok(RecordPlan {
            draft,
            after,
            before: t.before,
        })
    }

    /// The record `d`, which lands next, makes of `before`: computed with
    /// the fold's own function, which hands out no call here.
    fn apply(&self, before: Option<&StoredRecord>, d: &Draft) -> Result<StoredRecord> {
        let (w, event) = (self.w, provisional(self.call.lake, self.next, d));
        let staged = match self.first_use(d)? {
            Some((at, list)) => {
                let mut staged = self.pending.sealed.clone();
                staged.entry(at).or_default().push((self.next, list));
                std::borrow::Cow::Owned(staged)
            }
            None => std::borrow::Cow::Borrowed(&self.pending.sealed),
        };
        let outputs = &mut Outputs {
            staged: Some(&staged),
            ..Outputs::planning(self.view, &w.plans, &self.call.lake.keys)
        };
        let after = record::apply(&w.ontology, before, &event, &w.settings, outputs)?;
        Ok(after.expect("a record event makes a record"))
    }

    /// The triggers and object-set changes a record change causes, the
    /// event that makes it landing next: its own, then what it sets off
    /// across an edge ([`Self::across`]). A refused write leaves the
    /// append's drafts as they were.
    pub(crate) fn reactions(
        &mut self,
        before: Option<&StoredRecord>,
        after: &StoredRecord,
    ) -> Result<Vec<Draft>> {
        let (w, now, cause, context) = (self.w, self.now, self.next, self.call.context);
        let mut out = Vec::new();
        // A reaction carries the context of the write that caused it, so
        // a host acting on it writes one level deeper (021).
        let system = |body| Draft {
            cause: Some(cause),
            context,
            ..Draft::system(now, body)
        };
        let at = (after.ty.clone(), after.key.clone());
        let prior = self.pending.records.insert(at.clone(), after.clone());
        let mut own = BTreeMap::new();
        let mut far = Vec::new();
        let decided = (|| {
            let on = |t: &&TriggerDecl| t.on == after.ty;
            for t in w.ontology.triggers.values().map(|e| &e.decl).filter(on) {
                if self.fires(t, before, after)? {
                    out.push(system(EventBody::TriggerFired {
                        trigger: t.name.clone(),
                        ty: after.ty.clone(),
                        key: after.key.clone(),
                    }));
                }
            }
            let of = |s: &&ObjectSetDecl| s.of == after.ty;
            for s in w.ontology.object_sets.values().map(|e| &e.decl).filter(of) {
                let was = self
                    .pending
                    .member(self.view, &s.name, &after.ty, &after.key)?;
                let is = self.member(s, after)?;
                if was != is {
                    own.insert((s.name.clone(), at.0.clone(), at.1.clone()), is);
                    let change = (s.name.clone(), at.0.clone(), at.1.clone(), is);
                    out.push(self.call.changed(change, cause, now));
                }
            }
            self.across((before, after), &own, &mut far)
        })();
        if let Err(e) = decided {
            match prior {
                Some(p) => self.pending.records.insert(at, p),
                None => self.pending.records.remove(&at),
            };
            return Err(e);
        }
        self.pending.members.extend(own);
        self.pending.across += far.len();
        for (draft, change) in far {
            if let Some((set, ty, key, is)) = change {
                self.pending.members.insert((set, ty, key), is);
            }
            out.push(draft);
        }
        self.next = Position(cause.0 + 1 + out.len() as u64);
        Ok(out)
    }

    /// The drafts of a retirement: `d`, which retires `before` and lands
    /// next, then the triggers and object-set changes it causes, judged
    /// on the record its provisional event makes.
    pub(crate) fn retiring(&mut self, before: &StoredRecord, d: Draft) -> Result<Vec<Draft>> {
        let after = self.apply(Some(before), &d)?;
        let mut drafts = vec![d];
        drafts.extend(self.reactions(Some(before), &after)?);
        Ok(drafts)
    }

    /// The list of the version `d`, landing next, uses first, if it is the
    /// first use of that version of its source's key.
    fn first_use(&self, d: &Draft) -> Result<Option<((String, Digest), Value)>> {
        let ingested = matches!(d.body, EventBody::RecordIngested { .. });
        let (Some(sealed), Some(source), true) = (&d.sealed, d.source.as_ref(), ingested) else {
            return Ok(None);
        };
        let used = (source, &sealed.key, self.next, self.now);
        let read = (&self.pending.sealed, self.view, &*self.call.lake.keys);
        crate::sealing::first_use(read, used)
    }

    /// A planned record write with what it sets off: its own event, which
    /// lands next, then the triggers and object-set changes it causes.
    pub(crate) fn planned(&mut self, plan: RecordPlan) -> Result<Plan> {
        if let Some((at, list)) = self.first_use(&plan.draft)? {
            self.pending
                .sealed
                .entry(at)
                .or_default()
                .push((self.next, list));
        }
        let mut drafts = self.reactions(plan.before.as_ref(), &plan.after)?;
        drafts.insert(0, plan.draft);
        let after = Some(plan.after);
        Ok(Plan { drafts, after })
    }

    /// The events a verb's call appends when it runs: `verb_called`, and
    /// for a write effect the record write it causes, then the triggers
    /// and object-set changes that write causes, as for any other write;
    /// and the record that write leaves. `key` is `None` for a verb on its
    /// type, which reads no record and writes none. The record is read as
    /// the judged principal's write reaches it (051). The write is caused
    /// by the call and carries the verb as its procedure before its record
    /// is computed.
    fn run_verb(
        &mut self,
        verb: &VerbDecl,
        key: Option<&RecordKey>,
        payload: &Patch,
        expected: Option<Version>,
    ) -> Result<Plan> {
        let (w, now) = (self.w, self.now);
        let called = self.draft(EventBody::VerbCalled {
            verb: verb.name.clone(),
            key: key.cloned(),
            payload: payload.clone(),
        });
        let mut plan = Plan {
            drafts: vec![called],
            after: None,
        };
        // A verb on its type is external and declares no `requires`: it
        // reads no record and writes none.
        let Some(key) = key else {
            return Ok(plan);
        };
        let (o, on) = (&w.ontology, w.ontology.ty(&verb.on)?);
        let record = self.reached(on, key, expected)?;
        // A record read across a reference is reached where a write that
        // sends the reference would be accepted, and by nothing else.
        let (view, pending, who) = (self.view, &self.pending, self.who);
        let far = |ty: &TypeName, k: &RecordKey| live_for(view, pending, o, ty, k, who);
        let link = |r: &RelationName| link_from(view, &verb.on, r, key);
        let limits = (self.call.lake.limits.max_depth, (&w.settings, now));
        let stored = |ty: &TypeName, k: &RecordKey| pending.record(view, ty, k);
        let across = Across::new(o, on, (&far, &link, &stored), limits);
        requires(&w.settings, verb, key, &record, now, &across)?;
        // A verb on a link finds both its ends live and answered, whatever
        // its effect (035): one that writes the link has them judged where
        // the record is written, any other here, as `link` judges them.
        if links::is_link_type(&verb.on) && !matches!(verb.effect, VerbEffect::Write { .. }) {
            let end = |f: &&FieldName| [links::FROM, links::TO].contains(&f.as_str());
            let ends: Patch = (record.fields.iter().filter(|(f, _)| end(f)))
                .map(|(f, v)| (f.clone(), v.clone()))
                .collect();
            check_references(view, pending, o, &verb.on, key, &ends, who)?;
        }
        let patch = assigned(o, &w.settings, verb, &record, payload, (now, &across))?;
        let called_at = self.next;
        self.next = called_at.next();
        if let VerbEffect::Write { .. } = &verb.effect {
            let version = w.ontology.verbs.get(&verb.name).map_or(1, |e| e.version);
            let procedure = Some(Procedure {
                name: verb.name.to_string(),
                version: version.to_string(),
            });
            let of_the_write = Envelope {
                cause: Some(called_at),
                procedure,
                ..self.envelope.clone()
            };
            let of_the_call = std::mem::replace(&mut self.envelope, of_the_write);
            // Through the verbs' own door: a field only its verbs write is
            // written here and refused at `upsert` (lakelands#1130).
            let door = std::mem::replace(&mut self.door, Door::Verb);
            let write = self.record(&verb.on, key, &patch, expected, Some(record));
            (self.envelope, self.door) = (of_the_call, door);
            let write = self.planned(write?)?;
            plan.drafts.extend(write.drafts);
            plan.after = write.after;
        }
        Ok(plan)
    }
}

impl Call<'_> {
    /// The context a write of this call is decided in, under the writer
    /// `w` and over `view`, at `now`, `n` drafts already in its append:
    /// the caller is judged, at the authored door.
    pub(crate) fn decide<'a>(
        &'a self,
        w: &'a Writer,
        view: &'a dyn ProjectionView,
        now: Timestamp,
        n: usize,
    ) -> Decide<'a> {
        Decide {
            call: self,
            w,
            view,
            pending: Pending::default(),
            now,
            next: position_after(w, n),
            who: self.who,
            door: Door::Authored,
            envelope: Envelope::of(Origin::Authored),
            bound: true,
        }
    }

    /// `upsert`.
    pub fn upsert(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        patch: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        ordinary_door(ty)?;
        self.write(Op::Upsert(ty, key, &patch), opts)
    }

    /// An authored write of one record, as `upsert`, `retire`, `link` and
    /// `unlink` make it, their context already checked, under one hold of
    /// the writer: the plan and its append. A retry of a write the caller
    /// made is answered that write, as every retry is before the grants
    /// are asked: before a refusal for what the caller is not shown, and
    /// after a link door's own questions (035, extended).
    fn write(&self, op: Op<'_>, opts: WriteOpts) -> Result<Outcome> {
        let mut w = self.writer()?;
        let view = self.lake.engine.view(None)?;
        let first = self.replay(&w, &opts.idempotency_key, view.as_ref());
        let now = self.lake.clock(&mut w);
        let mut d = self.decide(&w, view.as_ref(), now, 0);
        if let Err(gone) = d.sight(op) {
            return first?.ok_or(gone);
        }
        d.shape(op)?;
        if let Some(first) = first? {
            return Ok(first);
        }
        let mut drafts = d.plan(op, opts.expected_version)?.drafts;
        self.keyed(&mut drafts[0], &opts.idempotency_key);
        drop(view);
        let events = self.lake.append(&mut w, drafts)?;
        self.told_of(&w.ontology, &events, outcome_of(&events))
    }

    /// `outcome` as its caller is told it: a set joined or left, or a
    /// trigger fired, at a record other than the write's own, which a far
    /// write sets off across an edge, only where the caller is answered
    /// one of the records it moved, as a write's own record is its
    /// writer's (055, extended).
    fn told_of(&self, o: &Ontology, events: &[Event], mut outcome: Outcome) -> Result<Outcome> {
        let Outcome::Applied {
            position, set_off, ..
        } = &mut outcome
        else {
            return Ok(outcome);
        };
        let at = events.iter().find(|e| e.position == *position);
        let own = match at.map(|e| &e.body) {
            Some(
                EventBody::RecordUpserted { ty, key, .. }
                | EventBody::RecordIngested { ty, key, .. }
                | EventBody::RecordRetired { ty, key, .. },
            ) => (ty, key),
            _ => return Ok(outcome),
        };
        let mut moved: BTreeMap<(&str, &TypeName), Vec<&RecordKey>> = BTreeMap::new();
        let mut mine = BTreeSet::new();
        for e in events.iter().filter(|e| e.cause == Some(*position)) {
            let (name, ty, key) = match &e.body {
                EventBody::ObjectSetChanged { set, ty, key, .. } => (set.as_str(), ty, key),
                EventBody::TriggerFired { trigger, ty, key } => (trigger.as_str(), ty, key),
                _ => continue,
            };
            match (ty, key) == own {
                true => mine.insert(name),
                false => {
                    moved.entry((name, ty)).or_default().push(key);
                    false
                }
            };
        }
        if moved.is_empty() {
            return Ok(outcome);
        }
        let view = self.lake.engine.view(None)?;
        let sees = Sight::held(self.who, o);
        let mut told = mine;
        for ((name, ty), keys) in &moved {
            if !told.contains(name) && crate::reads::answered_of(view.as_ref(), &sees, ty, keys, 1)?
            {
                told.insert(name);
            }
        }
        let told: BTreeSet<String> = told.into_iter().map(str::to_owned).collect();
        set_off.fired.retain(|t| told.contains(t.as_str()));
        set_off.joined.retain(|s| told.contains(s));
        set_off.left.retain(|s| told.contains(s));
        Ok(outcome)
    }

    /// `retire`.
    pub fn retire(&self, ty: &TypeName, key: &RecordKey, opts: WriteOpts) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        ordinary_door(ty)?;
        self.write(Op::Retire(ty, key), opts)
    }

    /// `link`: the link over `relation` from `key` of `ty` to `to`, as a
    /// write of its link record (`docs/decisions/035-a-link-is-a-record.md`).
    pub fn link(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        relation: &RelationName,
        to: &RecordKey,
        fields: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        self.write(Op::Link((ty, key, relation, to), &fields), opts)
    }

    /// `unlink`: the link's record retired, its history kept.
    pub fn unlink(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        relation: &RelationName,
        to: &RecordKey,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        self.write(Op::Unlink((ty, key, relation, to)), opts)
    }

    /// The verb `verb` for its caller: one on a type the caller is not
    /// shown is answered as a verb that is not declared, before its scope
    /// is judged in a sentence that names the type.
    fn shown_verb<'o>(&self, o: &'o Ontology, verb: &VerbName) -> Result<&'o VerbDecl> {
        let v = o.verb(verb)?;
        let missing = Missing::Verb { name: verb.clone() };
        let shown = Sight::held(self.who, o).ty(&v.on);
        shown.map(|_| v).map_err(|_| Error::NotFound { missing })
    }

    /// Check a verb's payload against its declaration: a member the verb
    /// does not declare is refused, naming the verb, before any value is
    /// read, even when its type has a field of that name.
    fn verb_payload(&self, w: &Writer, verb: &VerbDecl, payload: &Patch) -> Result<Patch> {
        let decl = w.ontology.ty(&verb.on)?;
        let declared = |n: &&FieldName| verb.payload.iter().any(|f| &f.name == *n);
        if let Some(name) = payload.keys().find(|n| !declared(n)) {
            let (verb, member) = (verb.name.clone(), name.clone());
            return Err(invalid_field(
                name,
                Case::PayloadUndeclared { verb, member },
            ));
        }
        let checked = check_values(&w.ontology, decl, payload, &verb.payload)?;
        for f in verb.payload.iter().filter(|f| f.required) {
            if checked.get(&f.name).is_none_or(Value::is_null) {
                let (verb, member) = (verb.name.clone(), f.name.clone());
                let case = Case::PayloadRequired { verb, member };
                return Err(constraint(Some(f.name.clone()), case));
            }
        }
        Ok(checked)
    }

    /// A call's first questions, as `call` and `propose` ask them of the
    /// caller, in one order (013, the order of refusals): the verb is
    /// declared and shown, the key fits its scope, the caller may call it,
    /// an external verb's executor is bound, and the payload. Whether a
    /// call that is proposed may be pinned is part of whether its caller
    /// may call it (051). Answers the verb, the payload as checked, and
    /// whether its proposal is pinned.
    fn callable(
        &self,
        w: &Writer,
        verb: &VerbName,
        proposed: bool,
        key: Option<&RecordKey>,
        payload: &Patch,
        expected: Option<Version>,
    ) -> Result<(VerbDecl, Patch, bool)> {
        let decl = self.shown_verb(&w.ontology, verb)?.clone();
        scoped(&decl, key, expected)?;
        let on = w.ontology.ty(&decl.on)?;
        granted(Sight::held(self.who, &w.ontology).callable(on, &decl))?;
        // A call is pinned to the records it reads across a reference too,
        // where its caller holds `only` on their type.
        let far = decl.reads_across(on, &|n| w.ontology.ty(n).ok());
        let mut far = far.iter().flatten();
        let pinned = (proposed || decl.risk != Risk::Low)
            && (self.pin(w, on, Some(&decl))?
                || far.any(|r| rights::only_holder(self.who, r.0, &w.ontology)));
        if let VerbEffect::External { executor } = &decl.effect {
            bound_executor(self.lake, executor)?;
        }
        let payload = self.verb_payload(w, &decl, payload)?;
        Ok((decl, payload, pinned))
    }

    /// The `write_proposed` of `write`, which lands next under the writer
    /// `w`: its proposal is named by the event's id.
    fn proposal(
        &self,
        w: &Writer,
        now: Timestamp,
        (write, why, pinned): (ProposedWrite, String, bool),
        key: &Option<IdempotencyKey>,
    ) -> Draft {
        let id = key.as_ref().map_or_else(
            || log::positional_id(&self.lake.id, position_after(w, 0)),
            |k| log::authored_id(&self.who.writer(), k),
        );
        let proposal = ProposalId::from(id.0.as_str());
        let body = EventBody::WriteProposed {
            proposal,
            write,
            why,
            pinned,
        };
        let mut d = self.draft(now, Origin::Authored, body);
        self.keyed(&mut d, key);
        d
    }

    /// `call`. Answers the outcome, and for an external verb that ran,
    /// the invocation the caller must hand its executor.
    pub fn call(
        &self,
        verb: &VerbName,
        key: Option<&RecordKey>,
        payload: Patch,
        opts: WriteOpts,
    ) -> Result<(Outcome, Option<External>)> {
        check_context(self.lake, &self.context)?;
        let mut w = self.writer()?;
        let view = self.lake.engine.view(None)?;
        if let Some(o) = self.replay(&w, &opts.idempotency_key, view.as_ref())? {
            return Ok((o, None));
        }
        let now = self.lake.clock(&mut w);
        let expected = opts.expected_version;
        let (decl, payload, pinned) = self.callable(&w, verb, false, key, &payload, expected)?;
        // A low-risk call runs. Any other is proposed, and one that could
        // not run now is not: the version, then `requires`, and the write
        // it would make, as the caller, as when it runs; accepting it
        // judges the record again (013).
        let low = decl.risk == Risk::Low;
        let mut d = self.decide(&w, view.as_ref(), now, usize::from(!low));
        let mut drafts = d.plan(Op::Call(&decl, key, &payload), expected)?.drafts;
        if low {
            self.keyed(&mut drafts[0], &opts.idempotency_key);
        } else {
            let write = ProposedWrite::Call {
                verb: verb.clone(),
                key: key.cloned(),
                payload: payload.clone(),
                opts: opts.clone(),
            };
            let why = format!(
                "`{verb}` is declared {} risk; it runs when accepted",
                risk_word(decl.risk)
            );
            let proposed = (write, why, pinned);
            drafts = vec![self.proposal(&w, now, proposed, &opts.idempotency_key)];
        }
        drop(view);
        let due = low && matches!(decl.effect, VerbEffect::External { .. });
        let appended = self.lake.append(&mut w, drafts);
        let keyed = opts.idempotency_key.is_some();
        let by = MadeDue::Call { keyed };
        let events = appended.map_err(|e| not_handed(e, due && w.behind, by))?;
        let outcome = self.told_of(&w.ontology, &events, outcome_of(&events))?;
        let external = match &decl.effect {
            VerbEffect::External { executor } if low => Some(External {
                executor: executor.clone(),
                verb: verb.clone(),
                ty: decl.on.clone(),
                key: key.cloned(),
                payload,
                position: events[0].position,
            }),
            _ => None,
        };
        Ok((outcome, external))
    }

    /// `propose`: record a write, checked whole as if it were applied, as a
    /// proposal.
    ///
    /// What is logged is the write as checked — its values normalised, its
    /// key fields filled — so the proposal holds the one written form of
    /// every value (`docs/log-format.md` §1.3), exactly as an applied write
    /// would.
    pub fn propose(&self, mut write: ProposedWrite, why: String) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        let mut w = self.writer()?;
        let view = self.lake.engine.view(None)?;
        let key_opt = match &write {
            ProposedWrite::Upsert { opts, .. }
            | ProposedWrite::Retire { opts, .. }
            | ProposedWrite::Call { opts, .. } => opts.idempotency_key.clone(),
            _ => None,
        };
        if let Some(o) = self.replay(&w, &key_opt, view.as_ref())? {
            return Ok(o);
        }
        let now = self.lake.clock(&mut w);
        // A holder of `only` changes no shape, and proposes no write that
        // anything but the write itself could move off its pairs (051).
        granted(!write.is_schema() || !rights::holds_only(self.who))?;
        // A setting its proposer may be told narrowed, as one who does not
        // read every field may, is proposed merged with what stands then,
        // so its accept is refused once that changed (063). A source's
        // setting is refused whole to one not told it, so nothing is merged
        // into it.
        let pin = |ty: &TypeName| {
            ordinary_door(ty)?;
            let sees = Sight::held(self.who, &w.ontology);
            let narrowed = sees.narrows(ty) && !rights::reads_all(self.who);
            Ok::<_, Error>(self.pin(&w, sees.ty(ty)?, None)? || narrowed)
        };
        let mut d = self.decide(&w, view.as_ref(), now, 1);
        let mut pinned = false;
        // A change of the ontology keeps what it would do as its proposer
        // is shown it without `restricted`, so no count it keeps tells of a
        // hold; each reader is shown it as its listing shows it (062).
        let mut plain = self.who.clone();
        plain.grants.restricted = false;
        let (who, followed, shown) = (&plain, None, None);
        let as_plain = Call {
            who,
            followed,
            shown,
            ..*self
        };
        // A re-key's sections of the other types, for the outcome (062).
        let mut others = Vec::new();
        let mut instead = None;
        match &mut write {
            ProposedWrite::Upsert {
                ty,
                key,
                patch,
                opts,
            } => {
                pinned = pin(ty)?;
                let mut plan = d.plan(Op::Upsert(ty, key, patch), opts.expected_version)?;
                let EventBody::RecordUpserted {
                    patch: mut checked, ..
                } = plan.drafts.remove(0).body
                else {
                    unreachable!("an authored record write is an upsert")
                };
                // A setting's group is stamped at the accept, from the
                // declaration then in force (063, extended).
                if ty.as_str() == SETTING_TYPE && !patch.contains_key("group") {
                    checked.remove("group");
                }
                *patch = checked;
            }
            ProposedWrite::Retire { ty, key, opts } => {
                pinned = pin(ty)?;
                let mut plan = d.plan(Op::Retire(ty, key), opts.expected_version)?;
                // A retire of a setting that keeps what its proposer is not
                // told is proposed as the write it is, its group stamped at
                // the accept (063).
                if let EventBody::RecordUpserted { mut patch, .. } = plan.drafts.remove(0).body {
                    patch.remove("group");
                    let (ty, key, opts) = (ty.clone(), key.clone(), opts.clone());
                    instead = Some(ProposedWrite::Upsert {
                        ty,
                        key,
                        patch,
                        opts,
                    });
                }
            }
            ProposedWrite::Call {
                verb,
                key,
                payload,
                opts,
            } => {
                let (key, expected) = (key.as_ref(), opts.expected_version);
                let (decl, checked, may) = self.callable(&w, verb, true, key, payload, expected)?;
                // A call that could not run when accepted is not proposed (013).
                d.plan(Op::Call(&decl, key, &checked), expected)?;
                (*payload, pinned) = (checked, may);
            }
            // A change of the ontology is checked now as its door would
            // check it, and nothing changes until it is accepted; the
            // proposer needs no `schema` grant (013, extended).
            ProposedWrite::Declare { declarations, plan } => {
                self.redeclared(&w.ontology, declarations)?;
                self.check_bundle(&w.ontology, declarations)?;
                // Its plan, telling only: what the walk finds is the accept's
                // to refuse, as before (062).
                let mut changed = declarations.clone();
                changed.retain(|d| !w.ontology.in_force(d));
                let bundled = match changed.is_empty() {
                    true => Some(Vec::new()),
                    false => Some(as_plain.bundle_drafts(&w, changed, (now, 0, true))?.types),
                };
                *plan = bundled.map(|types| Box::new(plan::plan_of(&w, (types, Vec::new()))));
            }
            ProposedWrite::Migrate { migration, report } => {
                let author = self.authored(&w.ontology, migration)?;
                // A retarget converts as its author is answered (lakelands#1071).
                let judged = (&**migration, author.as_ref(), Some(self.who));
                let planned = as_plain.preflight_plan(&w, view.as_ref(), judged, None)?;
                let found = planned.report;
                if !found.lossless {
                    return Err(self.lossy(&w, migration, found));
                }
                **report = found;
                others = planned.others;
            }
            ProposedWrite::MigrateValueType {
                to,
                renames,
                report,
            } => {
                let m = ValueTypeMigration {
                    to: (**to).clone(),
                    renames: renames.clone(),
                };
                let found = self
                    .preflight_value_type(&w, view.as_ref(), &m, None)?
                    .report;
                self.renames_shown(&w.ontology, &m, &found)?;
                **report = fits(found, false)?;
            }
        }
        // Its plan as its proposer is shown it, for the outcome (062).
        let mut shown = match &write {
            ProposedWrite::Declare { plan, .. } => plan.as_deref().cloned(),
            ProposedWrite::Migrate { migration, report } => {
                let mut types = vec![self.shown(&w, migration, (**report).clone())?];
                let sight = Sight::held(self.who, &w.ontology);
                types.extend(plan::others_shown(&sight, std::mem::take(&mut others)));
                Some(plan::plan_of(&w, (types, Vec::new())))
            }
            ProposedWrite::MigrateValueType { report, .. } => {
                let report = self.shown_value_type(&w.ontology, (**report).clone());
                Some(plan::plan_of(&w, (Vec::new(), vec![report])))
            }
            _ => None,
        };
        if let Some(p) = &mut shown {
            plan::shown_plan(p, &Sight::held(self.who, &w.ontology), view.as_ref())?;
        }
        let write = instead.unwrap_or(write);
        let d = self.proposal(&w, now, (write, why, pinned), &key_opt);
        drop(view);
        let events = self.lake.append(&mut w, vec![d])?;
        let mut outcome = outcome_of(&events);
        if let Outcome::Proposed { plan, .. } = &mut outcome {
            *plan = shown.map(Box::new);
        }
        Ok(outcome)
    }

    /// Whether a proposal by this caller on `decl`, or of a call of `verb`,
    /// is pinned, refusing one that may not be (051).
    fn pin(&self, w: &Writer, decl: &TypeDecl, verb: Option<&VerbDecl>) -> Result<bool> {
        let pinned = rights::only_holder(self.who, decl, &w.ontology);
        granted(!pinned || rights::pinnable(self.who, decl, verb))?;
        Ok(pinned)
    }

    /// The proposal as the head's listing shows it to its reviewer, with
    /// whether it holds more ([`crate::reads::Reader::entry`]). One that
    /// is not there to it is answered as an id that does not exist, as its
    /// listing leaves it out, before anything else of the proposal is told
    /// or done (lakelands#762).
    fn listed(
        &self,
        p: &lakelands_api::write::Proposal,
        asked: &Principal,
    ) -> Result<(Option<ProposedWrite>, bool)> {
        let Some(write) = &p.write else {
            return Ok((None, false));
        };
        let view = self.lake.engine.view(None)?;
        let shape = self.lake.shape(view.as_ref())?;
        let (sight, at) = (Sight::held(self.who, shape.as_ref()), Default::default());
        let read = crate::reads::Reader::open(self.lake, &sight, (at, None), view, shape.clone())?;
        let missing = Missing::Proposal { id: p.id.clone() };
        let once = (&Default::default(), &Default::default());
        let listed = read.entry(asked, write, (p.position, p.proposed_at), once)?;
        listed.ok_or(Error::NotFound { missing })
    }

    /// Refuse a reviewer holding `only` a proposal it is not listed whole:
    /// a change of shape, which it never makes, and a write to a type it
    /// holds `only` on that its listing would trim or withhold (051). A
    /// write that reaches a held record is not listed to one without
    /// `restricted`, so it is not found before this (006, lakelands#1008).
    /// It is judged before the reviewer is told whether the proposal is
    /// decided.
    fn decides(
        &self,
        w: &Writer,
        p: &lakelands_api::write::Proposal,
        listed: &(Option<ProposedWrite>, bool),
    ) -> Result<()> {
        let o = &w.ontology;
        let Some(write) = p.write.as_ref().filter(|_| rights::holds_only(self.who)) else {
            return Ok(());
        };
        let ty = match write {
            ProposedWrite::Upsert { ty, .. } | ProposedWrite::Retire { ty, .. } => ty,
            ProposedWrite::Call { verb, .. } => match o.verb(verb) {
                Ok(v) => &v.on,
                Err(_) => return granted(false),
            },
            _ => return granted(false),
        };
        if o.ty(ty).is_ok_and(|t| !rights::only_holder(self.who, t, o)) {
            return Ok(());
        }
        granted(listed.0.is_some() && !listed.1)
    }

    /// Refuse the accept of a pinned proposal, the one logged at `at`,
    /// once the record it writes or its type changed after it, or, for a
    /// link, an end record, as deep as its ends go, or a record its call
    /// reads across a reference: its proposer was judged on them as they
    /// stood (051).
    fn unmoved(&self, w: &Writer, view: &dyn ProjectionView, at: Position) -> Result<()> {
        let o = &w.ontology;
        let logged = self.lake.read_events(at, 1)?;
        let Some(EventBody::WriteProposed {
            write,
            pinned: true,
            ..
        }) = logged.first().map(|e| &e.body)
        else {
            return Ok(());
        };
        let mut far = Vec::new();
        let (ty, key) = match write {
            ProposedWrite::Upsert { ty, key, .. } | ProposedWrite::Retire { ty, key, .. } => {
                (ty, Some(key))
            }
            ProposedWrite::Call {
                verb, key, payload, ..
            } => {
                // The records it read across then and those it reads now: a
                // condition a setting or the clock decides may come to a
                // record at one and not at the other.
                let (v, now) = (
                    o.verb(verb)?,
                    crate::lake::wall_clock().max(w.last_known_at),
                );
                far = call_reads(o, (view, &w.settings, now), v, key.as_ref(), payload)?.1;
                far.extend(read_then(
                    self.lake,
                    &logged[0],
                    verb,
                    key.as_ref(),
                    payload,
                )?);
                (&v.on, key.as_ref())
            }
            _ => return Ok(()),
        };
        let declared = self.lake.engine.view(Some(at))?.declarations()?;
        let then = declared.iter().find_map(|d| match &d.declaration {
            Declaration::Type(t) if &t.name == ty => Some(d.version),
            _ => None,
        });
        let moved_at = |ty: &TypeName, key: &RecordKey| -> Result<bool> {
            Ok(view.get(ty, key)?.is_some_and(|r| r.position > at))
        };
        // A link is its holder's by its end records too, and a change to
        // an end moves no link.
        let mut moved = false;
        let own = key.iter().map(|k| (ty.clone(), (*k).clone()));
        for (t, k) in own.chain(far).flat_map(|(t, k)| with_ends(o, &t, &k)) {
            moved = moved || moved_at(&t, &k)?;
        }
        if !moved && then == o.types.get(ty).map(|e| e.version) {
            return Ok(());
        }
        let (ty, key) = (ty.clone(), key.cloned());
        Err(invalid(Case::WriteChangedSinceProposed { ty, key }))
    }

    /// `accept`: apply a pending proposal's write, written as the proposer.
    pub fn accept(
        &self,
        proposal: &ProposalId,
        asked: &Principal,
        plan_id: Option<&str>,
    ) -> Result<(Outcome, Option<External>)> {
        check_context(self.lake, &self.context)?;
        granted(self.who.grants.review)?;
        let mut w = self.writer()?;
        let view = self.lake.engine.view(None)?;
        let p = find_proposal(view.as_ref(), proposal)?;
        let listed = self.listed(&p, asked)?;
        // Before a retry is answered: an accept sent again would hand a
        // holder of `only` what an earlier accepter was answered (051).
        self.decides(&w, &p, &listed)?;
        // It accepts only what it is listed whole, as a reviewer without
        // the grant would be answered; listed in part, it may reject
        // (051, lakelands#867).
        granted(listed.0.is_some() && !listed.1)?;
        if p.state != ProposalState::Pending {
            let again = self.accepted_again(&w, view.as_ref(), proposal)?;
            return Ok((again, None));
        }
        // A change of the ontology is the lake's `schema` grant's to make:
        // the accepter holds it as well as `review` (013, extended).
        // A stored proposal holds its write; a row without one is a
        // projection the store cannot use.
        let Some(write) = p.write.as_ref() else {
            let flaw = Flaw::ProposalWithoutWrite {
                proposal: proposal.clone(),
            };
            return Err(lakelands_core::damaged(Part::Projection, None, flaw));
        };
        granted(!write.is_schema() || self.who.grants.schema)?;
        // Accepted against a plan: the plan of its change made now, as the
        // reviewer is shown it, by `plan`'s own path, is that one (062).
        if let Some(id) = plan_id {
            let Some(change) = write.change() else {
                let proposal = proposal.clone();
                return Err(invalid(Case::ProposalPlansNothing { proposal }));
            };
            let against = Against {
                plan: Some(id),
                report: None,
            };
            self.planned(&mut w, change, When::Plan, against)?;
        }
        self.unmoved(&w, view.as_ref(), p.position)?;
        let now = self.lake.clock(&mut w);
        // The write was checked under the proposer's grants when it was
        // proposed; it is applied as the proposer, with what was checked.
        // Its `restricted` grant is not logged: it held it where the write
        // reached a record restricted then, and else a record restricted
        // since is not there to the accept (006).
        let mut proposer = self.lake.system(p.proposer.clone());
        proposer.grants.restricted = held_then(self.lake, &p, write)?;
        let accepted = Draft {
            id: Some(log::accepted_id(&self.lake.id, proposal)),
            ..self.draft(
                now,
                Origin::Authored,
                EventBody::WriteAccepted {
                    proposal: proposal.clone(),
                },
            )
        };
        let accepted_at = position_after(&w, 0);
        let mut drafts = vec![accepted];
        let as_accepted = |mut d: Draft| {
            if d.origin == Origin::Authored {
                d.writer = p.proposer.clone();
            }
            d.cause = d.cause.or(Some(accepted_at));
            d
        };
        let (mut external, mut renamed) = (None, Vec::new());
        // The sections of the plan that applies, for a change of the
        // ontology, as the reviewer is shown them (062).
        let mut sections = (Vec::new(), Vec::new());
        // The proposer's events are written as the proposer and caused by
        // the acceptance before their record is computed.
        let mut d = self.decide(&w, view.as_ref(), now, 1);
        d.who = &proposer;
        (d.envelope.writer, d.envelope.cause) = (Some(p.proposer.clone()), Some(accepted_at));
        // What the write leaves is judged by the reviewer's pairs too: a
        // holder of `only` accepts no write that takes a record out of its
        // reach, as it writes none (051). A refusal of the plan is told as
        // the reviewer's listing shows the write, whatever the op.
        let mut run = |op: Op<'_>, opts: &WriteOpts| -> Result<Vec<Draft>> {
            let told = |e| self.told(&w.ontology, write, listed.0.as_ref(), e);
            let plan = d.plan(op, opts.expected_version).map_err(told)?;
            if let Some(after) = &plan.after {
                let (o, decl) = (&w.ontology, w.ontology.ty(&after.ty)?);
                let sees = Sight::held(self.who, o);
                let reach = owns(view.as_ref(), &d.pending, &sees, decl, &after.fields)?;
                granted(reach)?;
            }
            Ok(plan.drafts)
        };
        match write {
            ProposedWrite::Upsert {
                ty,
                key,
                patch,
                opts,
            } => drafts.extend(run(Op::Upsert(ty, key, patch), opts)?),
            ProposedWrite::Retire { ty, key, opts } => {
                drafts.extend(run(Op::Retire(ty, key), opts)?)
            }
            ProposedWrite::Call {
                verb,
                key,
                payload,
                opts,
            } => {
                let decl = w.ontology.verb(verb)?.clone();
                // The verb may have been declared again since: its scope at
                // the head must still fit the proposal, or the accept is
                // refused and the proposal stays pending.
                let key = scoped(&decl, key.as_ref(), opts.expected_version)?;
                // The reviewer applies what was checked: a verb declared
                // differently since the call was proposed refuses the
                // accept, and the proposal stays pending (013).
                let then = self.lake.engine.view(Some(p.position))?.declarations()?;
                if !then
                    .iter()
                    .any(|d| matches!(&d.declaration, Declaration::Verb(v) if *v == decl))
                {
                    let verb = verb.clone();
                    return Err(invalid(Case::VerbRedeclared { verb }));
                }
                if let VerbEffect::External { executor } = &decl.effect {
                    bound_executor(self.lake, executor)?;
                    external = Some(External {
                        executor: executor.clone(),
                        verb: verb.clone(),
                        ty: decl.on.clone(),
                        key: key.cloned(),
                        payload: payload.clone(),
                        position: accepted_at,
                    });
                }
                drafts.extend(run(Op::Call(&decl, key, payload), opts)?);
            }
            // Checked again on the head, which may have moved: a change it
            // no longer admits refuses the accept, and the proposal stays
            // pending. The proposer writes what it declares; the link types
            // the store derives stay the system's.
            ProposedWrite::Declare { declarations, .. } => {
                // One declared since under a name the bundle declares was
                // shown to no one at the proposal; the bundle would take
                // its place as its proposer's (lakelands#915).
                let key = lakelands_core::engines::declaration_key;
                let since = view.declarations()?;
                let since = |d: &Declaration| {
                    let mut held = since.iter().filter(|x| x.since > p.position);
                    !w.ontology.in_force(d) && held.any(|x| key(&x.declaration) == key(d))
                };
                if let Some(d) = declarations.iter().find(|d| since(d)) {
                    let (what, name) = naming(d);
                    return Err(invalid(Case::DeclaredSinceProposed { what, name }));
                }
                self.check_bundle(&w.ontology, declarations)?;
                let mut changed = declarations.clone();
                changed.retain(|d| !w.ontology.in_force(d));
                let own = self.bundle_drafts(&w, changed, (now, drafts.len(), false))?;
                sections.0 = own.types;
                drafts.extend(own.drafts.into_iter().map(&as_accepted));
            }
            ProposedWrite::Migrate { migration, report } => {
                // The shape is the type's whole: on a type changed since the
                // proposal it would remove what was added, shown to no one
                // (013, extended).
                let ty = &migration.to.name;
                let head = shape::schema_hash(w.ontology.ty(ty)?).ok();
                let changed = || invalid(Case::ChangedSinceProposed { ty: ty.clone() });
                if head.as_ref() != Some(&report.from_schema_hash) {
                    return Err(changed());
                }
                // Judged as its author's at the proposal: the lake's own
                // names are judged again.
                let judged = (&**migration, None, Some(&proposer));
                let plan = self.preflight_plan(&w, view.as_ref(), judged, Some(now))?;
                // So would it take away a mark set since, which the hash
                // does not cover: it restates what its report said, no more.
                if plan.report.restated != report.restated {
                    return Err(changed());
                }
                renamed = self.known_renames(&w.ontology, migration);
                sections.0 = vec![self.shown(&w, migration, plan.report.clone())?];
                let sight = Sight::held(self.who, &w.ontology);
                (sections.0).extend(plan::others_shown(&sight, plan.others.clone()));
                let (first, same) = (drafts.len(), unchanged(&w.ontology, migration, &plan));
                let own = self.migration_drafts(&w, (**migration).clone(), plan, now, first)?;
                let own = own.into_iter().filter(|_| !same);
                drafts.extend(own.map(&as_accepted));
            }
            ProposedWrite::MigrateValueType {
                to,
                renames,
                report,
            } => {
                // As a migration is accepted only on the shape it was
                // proposed on: a value type declared again since would lose
                // what that added, shown to no one (013, extended).
                let head = w.ontology.value_types.get(to.name.as_str());
                let changed = || {
                    let value_type = to.name.clone();
                    invalid(Case::ValueTypeChangedSinceProposed { value_type })
                };
                if head.map(|v| v.version) != Some(report.from_version) {
                    return Err(changed());
                }
                let m = ValueTypeMigration {
                    to: (**to).clone(),
                    renames: renames.clone(),
                };
                let plan = self.preflight_value_type(&w, view.as_ref(), &m, Some(now))?;
                // Renames reach only the fields the proposal's report named:
                // one that came to use the value type since would be
                // rewritten unseen by the proposer and the reviewer.
                let uses = |r: &ValueTypeReport| {
                    let fields = r.fields.iter();
                    fields
                        .map(|u| (u.ty.clone(), u.field.clone()))
                        .collect::<Vec<_>>()
                };
                if !renames.is_empty() && uses(&plan.report) != uses(report) {
                    return Err(changed());
                }
                fits(plan.report.clone(), false)?;
                sections.1 = vec![self.shown_value_type(&w.ontology, plan.report.clone())];
                let own = self.value_type_drafts(&w, m, plan, now, drafts.len())?;
                drafts.extend(own.into_iter().map(&as_accepted));
            }
        }
        drop(view);
        let appended = self.lake.append(&mut w, drafts);
        let by = MadeDue::Accepted;
        let events = appended.map_err(|e| not_handed(e, external.is_some() && w.behind, by))?;
        let outcome = self.answered(&events, renamed);
        let mut outcome = self.told_of(&w.ontology, &events, outcome)?;
        if let (true, Outcome::Applied { plan, .. }) = (write.is_schema(), &mut outcome) {
            *plan = Some(Box::new(plan::plan_of(&w, sections)));
        }
        Ok((outcome, external))
    }

    /// A refusal of the proposer's plan as an accept answers its reviewer
    /// (lakelands#780), judged by the reviewer's view as its listing is:
    /// whole where the listing shows the reviewer all it names, and else
    /// as a proposal stale since it was proposed, which names only what
    /// the listing does. A conflict names versions only. Its proposer is
    /// judged as any reviewer, by its grants at the accept: it hears what
    /// its own write would tell it now (lakelands#791).
    fn told(
        &self,
        o: &Ontology,
        write: &ProposedWrite,
        listed: Option<&ProposedWrite>,
        e: Error,
    ) -> Error {
        let (ty, key) = match write {
            ProposedWrite::Upsert { ty, key, .. } | ProposedWrite::Retire { ty, key, .. } => {
                (ty.clone(), Some(key.clone()))
            }
            ProposedWrite::Call { verb, key, .. } => match o.verb(verb) {
                Ok(v) => (v.on.clone(), key.clone()),
                Err(_) => return e,
            },
            _ => return e,
        };
        let judged = matches!(e, Error::Refused { .. } | Error::NotFound { .. });
        if !judged || self.who.grants.reads_everything() {
            return e;
        }
        let sight = Sight::held(self.who, o);
        let read = |f: &FieldName| sight.reads_from(&ty, f);
        let sees = |t: &TypeName| sight.ty(t).is_ok();
        let requires = |v: &VerbName| o.verb(v).ok().and_then(|v| v.requires.clone());
        let mut named = Named::default();
        e.names(&mut named);
        // A constraint is told to one who reads every field it reads, as
        // `describe_type` tells it (lakelands#1119).
        let constraints = o
            .ty(&ty)
            .map(|t| t.constraints.as_slice())
            .unwrap_or_default();
        let told = |n: &ConstraintName| {
            let c = constraints.iter().find(|c| &c.name == n);
            let e = c.and_then(|c| c.holds.typed().ok());
            e.is_some_and(|e| e.fields_read().into_iter().all(read))
        };
        if named.constraints.iter().all(told) {
            named.constraints.clear();
        }
        match listed {
            Some(l) if named.shown_by(l, &sees, &read, &requires) => e,
            _ => invalid(Case::WriteChangedSinceProposed { ty, key }),
        }
    }

    /// The writer, held by this call. Its grants were followed through the
    /// lake's renames before it waited here; where a migration renamed a
    /// field or a relation meanwhile they may no longer hold it, and the
    /// call is refused to be sent again, in words that name no migration
    /// (006, extended).
    pub(crate) fn writer(&self) -> Result<std::sync::MutexGuard<'_, crate::lake::Writer>> {
        let w = self.lake.for_write()?;
        if let Some(n) = self.followed
            && self.lake.renames(None)?.len() != n
        {
            return Err(Error::unavailable(Situation::Moved));
        }
        if let Some(s) = &self.shown
            && s.versions != w.ontology.versions()
            && (self.lake.sight(s.asked.clone(), None, None, &w.ontology)?.0).hidden() != s.hidden
        {
            return Err(Error::unavailable(Situation::TypesMoved));
        }
        Ok(w)
    }

    /// A migration's renames as the one answered may know of them, taken
    /// before the append, while the type stands as it was.
    fn known_renames(&self, o: &Ontology, m: &Migration) -> Vec<(FieldName, FieldName)> {
        let mut m = m.clone();
        if let Ok(was) = o.ty(&m.to.name) {
            rights::known_migration(self.who, was, &mut m, &|n| o.ty(n).ok());
        }
        m.renames
    }

    /// [`outcome_of`], with the fields the migration it applied renamed.
    fn answered(&self, events: &[Event], known: Vec<(FieldName, FieldName)>) -> Outcome {
        let mut outcome = outcome_of(events);
        if let Outcome::Applied { renamed, .. } = &mut outcome {
            *renamed = known;
        }
        outcome
    }

    /// `reject`.
    pub fn reject(&self, proposal: &ProposalId, asked: &Principal, why: String) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        granted(self.who.grants.review)?;
        let mut w = self.writer()?;
        let view = self.lake.engine.view(None)?;
        let p = find_proposal(view.as_ref(), proposal)?;
        self.decides(&w, &p, &self.listed(&p, asked)?)?;
        if p.state != ProposalState::Pending {
            return Err(refused(Refusal::Decided));
        }
        let now = self.lake.clock(&mut w);
        let d = self.draft(
            now,
            Origin::Authored,
            EventBody::WriteRejected {
                proposal: proposal.clone(),
                reason: why,
            },
        );
        drop(view);
        let events = self.lake.append(&mut w, vec![d])?;
        Ok(outcome_of(&events))
    }

    /// A function computed outside the store is declared only where a
    /// backing of its kind is bound; replay never asks. An embedding is
    /// compared with the one `in_force` holds, where there is one to
    /// compare with: across a migration's renames, with the field or the
    /// relation it is renamed from, whose vectors it carries.
    fn check_bound(
        &self,
        d: &Declaration,
        in_force: Option<&Ontology>,
        m: Option<&Migration>,
    ) -> Result<()> {
        // A trigger's ceiling lies within the lake's (021).
        if let Declaration::Trigger(t) = d
            && let Some(m) = t.max_depth
            && m > self.lake.limits.max_reactive_depth
        {
            let (depth, max) = (m, self.lake.limits.max_reactive_depth);
            let said = Said::from(Case::TriggerTooDeep { depth, max });
            let name = t.name.clone();
            return Err(invalid(said.within(Place::Trigger { name })));
        }
        let backings = &self.lake.seams.backings;
        if let Declaration::Function(f) = d
            && crate::backed::is_backed(f)
            && !backings.iter().any(|b| b.kind() == f.backing)
        {
            return Err(Error::unbound(Seam::FunctionBacking {
                kind: f.backing.clone(),
            }));
        }
        if let Declaration::Type(t) = d {
            let renames = m.map_or(&[][..], |m| &m.renames);
            self.check_embeddings(t, in_force, (&t.name, renames))?;
            // A relation's fields embed on its link type's records.
            let moved = m.map(links::moved).unwrap_or_default();
            for lt in links::link_types_of(t) {
                let was = moved.iter().find(|(_, new)| **new == lt.name);
                let was = was.map_or(&lt.name, |(old, _)| old);
                self.check_embeddings(&lt, in_force, (was, &[]))?;
            }
        }
        Ok(())
    }

    /// A type's embeddings are declared only where the bound embedding
    /// backend names their model and makes vectors of their dimensions,
    /// unless `in_force` holds the field with the same embedding: one
    /// declared unchanged makes nothing due, so a lake with no backend
    /// bound can still restate or grow the type around it. `named` and
    /// `renames` give the type and its fields as `in_force` holds them,
    /// before a migration renames them.
    fn check_embeddings(
        &self,
        t: &TypeDecl,
        in_force: Option<&Ontology>,
        (named, renames): (&TypeName, &[(FieldName, FieldName)]),
    ) -> Result<()> {
        // The name a field holds in force: the one it is renamed from, or
        // its own, unless a rename frees that name and `to` declares it
        // anew, a field with none in force.
        let old = |n: &FieldName| match renames.iter().find(|(_, new)| new == n) {
            Some((old, _)) => Some(old.clone()),
            None => (!renames.iter().any(|(old, _)| old == n)).then(|| n.clone()),
        };
        let (bound, mut was) = (self.lake.embed(), None);
        for f in &t.fields {
            let Some(e) = &f.embed else {
                continue;
            };
            let dimensions = f.ty.dimensions.unwrap_or(0);
            if bound
                .as_ref()
                .is_some_and(|b| b.model() == e.model && b.dimensions() == dimensions)
            {
                continue;
            }
            let was = was.get_or_insert_with(|| in_force.and_then(|o| o.as_declared(named).ok()));
            let kept = was
                .iter()
                .flat_map(|w| &w.fields)
                .find(|k| Some(&k.name) == old(&f.name).as_ref());
            let from: Option<Vec<FieldName>> = e.from.iter().map(old).collect();
            let same = |k: &EmbedDecl| k.model == e.model && from.as_ref() == Some(&k.from);
            if kept.is_some_and(|k| {
                k.ty.dimensions == f.ty.dimensions && k.embed.as_ref().is_some_and(same)
            }) {
                continue;
            }
            let bound = bound.map(|b| lakelands_api::error::Bound {
                model: b.model().to_owned(),
                dimensions: b.dimensions(),
            });
            return Err(Error::unbound(Seam::Embedding {
                model: e.model.clone(),
                dimensions,
                ty: t.name.clone(),
                field: f.name.to_string(),
                bound,
            }));
        }
        Ok(())
    }

    /// `declare`.
    pub fn declare(&self, declaration: Declaration) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        granted(self.who.grants.schema && !rights::holds_only(self.who))?;
        self.declare_granted(declaration)
    }

    /// Refuse each type of `set` declared again with a change, as its
    /// migration would be refused for a verb that reads into it
    /// ([`verbs_judged_again`]): over `held`, the lake's own, with the set
    /// in it as its author's view is completed.
    fn verbs_still_read(&self, held: &Ontology, set: &[Declaration]) -> Result<()> {
        let changed = set.iter().filter(|d| !held.in_force(d));
        let changed: Vec<_> = (changed.filter_map(|d| match d {
            Declaration::Type(t) => Some((&held.types.get(&t.name)?.decl, t, d.place())),
            _ => None,
        }))
        .collect();
        // No verb on or into a changed type, in force or in the set, and no
        // set or trigger reading into it across an edge: nothing is judged,
        // and the lake is not copied.
        let verbs = || {
            let sent = set.iter().filter_map(|d| match d {
                Declaration::Verb(v) => Some(v),
                _ => None,
            });
            (held.verbs.values().map(|e| &e.decl)).chain(sent)
        };
        let judged = |(was, t, _): &(&TypeDecl, &TypeDecl, Place)| {
            verbs().any(judged_by(held, was, t)) || !across_judged(held, was, t).is_empty()
        };
        if !changed.iter().any(judged) {
            return Ok(());
        }
        let mut after = held.clone();
        set.iter()
            .for_each(|d| put_as_appended(&mut after, d, Some));
        let sees = Sight::held(self.who, held);
        for (was, t, at) in changed {
            let judged = verbs_judged_again((held, &after), was, t, &|d| sees.shows(d));
            judged.map_err(|said| invalid(said).within(at))?;
        }
        Ok(())
    }

    /// `recompute`: the declaration in force appended again as it was
    /// declared, at its next version, so that the fold does at this event
    /// what it does at any `declared` of it (022, extended). A deliberate
    /// act: nothing is compared, and the log shows it.
    pub fn recompute(&self, what: &Recompute) -> Result<Outcome> {
        let what = what.clone();
        self.make(
            lakelands_api::write::Change::Recompute { what },
            Against::default(),
        )
    }

    /// The declaration `recompute` appends again: the one in force that
    /// `what` names, whose purpose is the calls, so it needs the backend
    /// whatever is in force.
    fn recomputed(&self, w: &Writer, what: &Recompute) -> Result<Declaration> {
        let declaration = match what {
            Recompute::Function(name) => match w.ontology.functions.get(name) {
                Some(f) => Declaration::Function(f.decl.clone()),
                None => {
                    let missing = Missing::Function { name: name.clone() };
                    return Err(Error::NotFound { missing });
                }
            },
            Recompute::Type(name) => {
                let t = Sight::held(self.who, &w.ontology).ty(name)?;
                if t.view.is_some() || !crate::backed::computed_outside(&w.ontology, t) {
                    let ty = name.clone();
                    return Err(invalid(Case::NothingComputedOutside { ty }));
                }
                Declaration::Type(w.ontology.as_declared(name)?)
            }
        };
        self.check_bound(&declaration, None, None)?;
        Ok(declaration)
    }

    /// `apply`: a set of declarations validated together and appended in
    /// one append, or refused whole. Each declaration is checked against
    /// the lake's ontology with every other declaration of the set already
    /// in it, so a reference may point forward within the set and tool
    /// names are checked over all of it; every problem is collected, and
    /// one refusal names them all. Of a set that is admitted, what is in
    /// force before it is not appended again (027, extended).
    pub fn apply(&self, declarations: Vec<Declaration>) -> Result<Outcome> {
        let change = lakelands_api::write::Change::Declare { declarations };
        self.make(change, Against::default())
    }

    /// A declaration reads only what its author may read (024 and 028,
    /// extended). The set is checked as sent against the ontology as its
    /// author is shown it, so a field the author may not know of answers as
    /// one the type does not have, whatever reads it. Then a field it knows
    /// of and may not read is refused as not granted, where what its author
    /// adds or changes reads its value. Before either, a declaration under
    /// a name, or with a tool's name, that what its author is not shown
    /// holds is not granted.
    fn reads_as_author(&self, o: &Ontology, sent: &[Declaration], reshapes: bool) -> Result<()> {
        // A constraint under the name of one its author is not told, as
        // `describe_type` tells it, is not granted, naming nothing, before
        // anything judges the type as its author knows it (lakelands#1119).
        let sees = Sight::held(self.who, o);
        for t in sent.iter().filter_map(|d| match d {
            Declaration::Type(t) => o.types.get(&t.name).map(|held| (t, &held.decl)),
            _ => None,
        }) {
            let (t, held) = t;
            let untold = |c: &ConstraintDecl| {
                let e = c.holds.typed().ok();
                !e.is_some_and(|e| {
                    e.fields_read()
                        .into_iter()
                        .all(|f| sees.reads_from(&held.name, f))
                })
            };
            let named_like = |c: &ConstraintDecl| {
                (held.constraints.iter()).any(|h| h.name == c.name && untold(h))
            };
            granted(!t.constraints.iter().any(named_like))?;
        }
        // One who reads every field and holds `schema` is shown the lake
        // whole: its view is the lake, and nothing it adds or changes
        // probes a field it may not read.
        // A migration's `to` is judged for what it needs bound by its
        // preflight, across its renames (lakelands#1124).
        let set = |seen: &Ontology, shaped: &[Declaration]| match self.check_set(
            (seen, o),
            shaped,
            !reshapes,
        ) {
            Err(e) if reshapes => declare::unless_a_migration(e),
            checked => checked,
        };
        if rights::reads_all(self.who) && self.who.grants.schema {
            // But what stands in force listed to no one, as a verb whose
            // `requires` does not parse, is not its to declare again; nor is
            // a tool's name that such a verb bears (the slow path's `taken`).
            let sees = Sight::held(self.who, o);
            let computes = |f: &str| {
                let mut types = o.types.values();
                types.any(|e| {
                    e.decl
                        .fields
                        .iter()
                        .any(|x| x.function.as_deref() == Some(f))
                })
            };
            let unlisted = |d: &Declaration| match d {
                Declaration::Type(_) => false,
                Declaration::Function(f) if !computes(&f.name) => false,
                d => o.held(d).is_some_and(|x| !sees.lists(&x)),
            };
            granted(!sent.iter().any(unlisted))?;
            tools_as_shown(self.who, o, o, sent)?;
            return set(o, sent);
        }
        // A type its author is not shown is a type that is not declared,
        // for whatever names it; to declare it is not granted, and names
        // nothing of it.
        let mut seen = types_shown(self.who, o);
        let hidden = |d: &Declaration| matches!(d, Declaration::Type(t) if o.types.contains_key(&t.name) && seen.ty(&t.name).is_err());
        granted(!sent.iter().any(hidden))?;
        // A set its author is not shown is one that is not declared, as a
        // read of it answers (024): a proposal that names it learns nothing
        // of what its filter reads, and to declare it is not granted, as
        // for a type (027). A holder of `schema` is shown each set of a
        // type it is shown.
        let declared = o.declared();
        let sees = Sight::held(self.who, &declared);
        let shown = |d| sees.lists(&Declaration::ObjectSet(d));
        (seen.object_sets).retain(|_, e| shown(e.decl.clone()));
        // So for a trigger, a verb and a function that computes a field,
        // known by their name alone: the one in force would be gone with
        // the one declared (lakelands#893). A function stands on the types
        // whose fields it computes: one shown to every holder of `schema`
        // is taken all the same where it computes a field of a type its
        // author is not shown, whose records its new body would compute
        // again (lakelands#917).
        let computed = |x: &FieldDecl, f: &str| x.function.as_deref() == Some(f);
        let computes = |f: &str, unseen: bool| {
            let mut types = o.types.values().map(|e| &e.decl);
            let on = |t: &TypeDecl| !unseen || !seen.types.contains_key(&t.name);
            types.any(|t| on(t) && t.fields.iter().any(|x| computed(x, f)))
        };
        let taken = |d: &Declaration| {
            let mut held = declared.iter().map(|x| &x.declaration);
            let f = match d {
                Declaration::Function(f) => Some(f.name.as_str()),
                _ => None,
            };
            !matches!(d, Declaration::Type(_))
                && f.is_none_or(|f| computes(f, false))
                && (f.is_some_and(|f| computes(f, true))
                    || held.any(|x| x.what() == d.what() && !sees.lists(x)))
        };
        granted(!sent.iter().any(taken))?;
        tools_as_shown(self.who, o, &seen, sent)?;
        // A valid time left out where the one in force names a field outside
        // its author's view stands as declared: left out of both sides, it is
        // checked whole at apply. A valid time its author sends, the one in
        // force too, is read against its view, where such a field is no field
        // (027, extended, lakelands#726, lakelands#692). A key and a
        // relation's `via` name fields their author knows of, or the type and
        // the relation are not shown to it (006 and 024, extended).
        let mut shaped = sent.to_vec();
        for d in &mut shaped {
            let Declaration::Type(t) = d else { continue };
            let (Ok(held), Some(e)) = (o.ty(&t.name), seen.types.get_mut(&t.name)) else {
                continue;
            };
            let e = Arc::make_mut(e);
            let view: Vec<FieldName> = e.decl.fields.iter().map(|f| f.name.clone()).collect();
            let unseen = |f: &Option<FieldName>| f.as_ref().is_some_and(|f| !view.contains(f));
            for (sent, kept, was) in [
                (&mut t.valid_from, &mut e.decl.valid_from, &held.valid_from),
                (&mut t.valid_to, &mut e.decl.valid_to, &held.valid_to),
            ] {
                if sent.is_none() && unseen(was) {
                    (*sent, *kept) = (None, None);
                }
            }
        }
        // A migration's shape is refused as a declaration; its preflight is next.
        set(&seen, &shaped)?;
        let mut after = o.clone();
        sent.iter().for_each(|d| after.put(d.clone(), 0, None));
        for d in sent {
            // Only what its author adds or changes: of a type, the fields,
            // the relations and the view that do not stand as they are.
            let key = lakelands_core::engines::declaration_key;
            let mut was = declared.iter().map(|b| &b.declaration);
            let (was, mut new) = (was.find(|b| key(b) == key(d)), d.clone());
            match (&mut new, was) {
                (Declaration::Type(t), Some(Declaration::Type(w))) => {
                    *t = after.ty(&t.name)?.clone();
                    t.fields.retain(|f| !w.fields.contains(f));
                    t.relations.retain(|r| !w.relations.contains(r));
                    t.view = t.view.take().filter(|v| Some(v) != w.view.as_ref());
                }
                (_, Some(w)) if w == d => continue,
                _ => {}
            }
            let named = describe::probed_types(&new);
            for t in named.iter().filter_map(|n| o.ty(n).ok()) {
                let sets = |n: &str| after.set(n).map(|s| s.filter.clone());
                let mut names = describe::declaration_probes(&new, &t.name, &sets);
                // A function's inputs and what an embedding is made from.
                if let Declaration::Type(x) = &new
                    && x.name == t.name
                {
                    let read = |f| crate::backed::computed_reads(&after, f);
                    names.extend(x.fields.iter().filter_map(read).flatten());
                }
                let has = |n: &&FieldName| t.fields.iter().any(|f| &f.name == *n);
                rights::check_probe(self.who, t, names.iter().filter(has))?;
            }
        }
        Ok(())
    }

    /// A value type's report as the caller may know it (024): a use and a
    /// misfit are shown where it is shown the type and reads the field. The
    /// totals and `fits` stay whole, so that a reader never sees no misfit
    /// beside a report that does not fit, and the id is made over this.
    /// A report that renames values is never trimmed: one not shown every
    /// field it writes is not granted it ([`Call::renames_shown`]).
    pub fn shown_value_type(&self, o: &Ontology, mut report: ValueTypeReport) -> ValueTypeReport {
        // A use is shown in a field the caller reads, of a type it is
        // shown: one keyed by a reference it may not know of is not (024).
        let sees = Sight::held(self.who, o);
        report.fields.retain(|u| sees.reads(&u.ty, &u.field));
        report.first.retain(|m| sees.reads(&m.ty, &m.field));
        // A breach to one who reads every field its constraint reads, as
        // `describe_type` tells it (lakelands#1119).
        report.breaches.retain(|b| {
            let t = o.ty(&b.ty).ok();
            let c = t.and_then(|t| t.constraints.iter().find(|c| c.name == b.constraint));
            let e = c.and_then(|c| c.holds.typed().ok());
            e.is_some_and(|e| {
                e.fields_read()
                    .into_iter()
                    .all(|f| sees.reads_from(&b.ty, f))
            })
        });
        report
    }

    /// Reads a migration as its author's view of its type ([`as_author`]),
    /// and answers that view, for [`Self::preflight_plan`] to judge its
    /// names against.
    fn authored(&self, o: &Ontology, m: &mut Migration) -> Result<Option<Authored>> {
        if let Some(before) = o.types.get(&m.to.name) {
            let sees = Sight::held(self.who, o);
            sees.ty(&m.to.name)?;
            reads_stand(&sees, &before.decl, &m.to, Some(m))?;
        }
        self.reads_as_author(o, &[Declaration::Type(m.to.clone())], true)?;
        initials(o, &mut m.to);
        let Some(before) = o.types.get(&m.to.name) else {
            return Ok(None);
        };
        let find = |n: &TypeName| o.ty(n).ok();
        let (sent, mut known) = (m.clone(), before.decl.clone());
        rights::known_fields(self.who, &mut known, &find);
        let (to, pairs) = (&mut m.to, &m.relation_renames);
        as_author(&Sight::held(self.who, o), &before.decl, to, pairs)?;
        Ok(Some(Authored { sent, known }))
    }

    /// Reads each type of `declarations` that is declared already as its
    /// author's view of it ([`as_author`]).
    fn redeclared(&self, o: &Ontology, declarations: &mut [Declaration]) -> Result<()> {
        for d in declarations.iter() {
            if let Declaration::Type(t) = d
                && let Some(before) = o.types.get(&t.name)
            {
                reads_stand(&Sight::held(self.who, o), &before.decl, t, None)?;
            }
        }
        self.reads_as_author(o, declarations, false)?;
        // An `initial` is read against the value types the set declares.
        let mut after = std::borrow::Cow::Borrowed(o);
        let types = declarations
            .iter()
            .filter(|d| matches!(d, Declaration::ValueType(_)));
        types.for_each(|d| after.to_mut().put(d.clone(), 0, None));
        for d in declarations {
            let Declaration::Type(t) = d else { continue };
            initials(&after, t);
            if let Some(before) = o.types.get(&t.name) {
                as_author(&Sight::held(self.who, o), &before.decl, t, &[])?;
            }
        }
        Ok(())
    }

    /// Check a set of declarations as `apply` checks it, against
    /// `ontology`, whoever applies it: each against the ontology with
    /// every other declaration of the set already in it, every problem
    /// collected into one refusal.
    fn check_bundle(&self, ontology: &Ontology, declarations: &[Declaration]) -> Result<()> {
        self.check_set((ontology, ontology), declarations, true)?;
        self.verbs_still_read(ontology, declarations)
    }

    /// [`Self::check_bundle`] against `ontology` as the set's author is
    /// shown `held`, the lake's own; what each needs bound only if `bound`.
    fn check_set(
        &self,
        (ontology, held): (&Ontology, &Ontology),
        declarations: &[Declaration],
        bound: bool,
    ) -> Result<()> {
        if declarations.is_empty() {
            return Err(invalid(Case::SetEmpty));
        }
        let identities: Vec<String> = declarations
            .iter()
            .map(lakelands_core::engines::declaration_key)
            .collect();
        let mut problems: Vec<(Place, Error)> = Vec::new();
        for (i, id) in identities.iter().enumerate() {
            if identities[..i].contains(id) {
                let twice = invalid(Case::SetDeclaresTwice);
                problems.push((declarations[i].place(), twice));
            }
        }
        for (i, d) in declarations.iter().enumerate() {
            let mut probe = std::borrow::Cow::Borrowed(ontology);
            for (j, other) in declarations.iter().enumerate() {
                if identities[j] != identities[i] {
                    // A link type the lake holds is brought as the set's
                    // author is shown it: not at all where it is not shown
                    // the type, and without a field it is not shown.
                    let shown = |mut t: TypeDecl| {
                        let Some(h) = held.types.get(&t.name) else {
                            return Some(t);
                        };
                        let seen = &ontology.types.get(&t.name)?.decl;
                        let has =
                            |d: &TypeDecl, f: &FieldDecl| d.fields.iter().any(|x| x.name == f.name);
                        t.fields.retain(|f| has(seen, f) || !has(&h.decl, f));
                        Some(t)
                    };
                    put_as_appended(probe.to_mut(), other, shown);
                }
            }
            let checked = declare::check_declaration(&probe, d, false);
            let checked = checked.and_then(|()| match bound {
                true => self.check_bound(d, Some(ontology), None),
                false => Ok(()),
            });
            if let Err(e) = checked {
                problems.push((d.place(), e));
            }
        }
        match problems.len() {
            0 => Ok(()),
            1 => {
                let (at, e) = problems.remove(0);
                Err(e.within(at))
            }
            count => {
                let problems = problems.into_iter().map(|(at, e)| e.within(at)).collect();
                Err(invalid(Case::SetRefused { count, problems }))
            }
        }
    }

    /// The drafts that declare a checked set: each declaration at its next
    /// version, with the link types its relations need; and the sections
    /// of its plan, of the types it computes again (062). Where `telling`,
    /// as a proposal's plan, its sections alone: nothing it finds refuses
    /// it, and its sets are not decided.
    pub(crate) fn bundle_drafts(
        &self,
        w: &Writer,
        declarations: Vec<Declaration>,
        (now, first, telling): (Timestamp, usize, bool),
    ) -> Result<Bundled> {
        let mut drafts = Vec::with_capacity(declarations.len());
        let mut decided = Vec::new();
        // Only a function, or a type the lake holds, computes records again
        // when it is declared: a set without one is written as before.
        let computes = declarations.iter().any(|d| match d {
            Declaration::Function(_) => true,
            Declaration::Type(t) => w.ontology.types.contains_key(&t.name),
            _ => false,
        });
        // A set is decided over the sets it names as the bundle leaves them,
        // and the lake is copied only for a bundle that declares one.
        let sets = (declarations.iter()).filter(|d| matches!(d, Declaration::ObjectSet(_)));
        let after = sets.clone().next().map(|_| {
            let mut after = w.ontology.clone();
            sets.for_each(|d| after.put(d.clone(), 0, None));
            after
        });
        // The records the bundle computes again, and the lake it leaves.
        let again = declarations.iter();
        let again: BTreeSet<TypeName> =
            again.flat_map(|d| computes_again(&w.ontology, d)).collect();
        let mut fields: BTreeMap<TypeName, BTreeSet<FieldName>> = BTreeMap::new();
        for (ty, f) in declarations
            .iter()
            .flat_map(|d| fields_again(&w.ontology, d))
        {
            fields.entry(ty).or_default().insert(f);
        }
        let whole = (!again.is_empty()).then(|| {
            let mut whole = w.ontology.clone();
            (declarations.iter()).for_each(|d| whole.put(d.clone(), 0, None));
            whole
        });
        // A setting's held value is judged by the lake as the bundle
        // leaves it (063, extended).
        let settled = (declarations.iter())
            .any(|d| matches!(d, Declaration::Setting(_)))
            .then(|| {
                let mut whole = w.ontology.clone();
                (declarations.iter()).for_each(|d| whole.put(d.clone(), 0, None));
                whole
            });
        for declaration in declarations {
            let declared = position_after(w, first + drafts.len());
            // A type's hash and link types, or the memberships a set decides.
            let (schema_hash, with) = match &declaration {
                Declaration::Type(t) => {
                    let hash = shape::schema_hash(t).map_err(invalid)?;
                    (Some(hash), link_type_drafts(&w.ontology, t, now)?)
                }
                // A set of a type computed again is decided by its walk, over
                // the records as the bundle computes them.
                Declaration::ObjectSet(set) if again.contains(&set.of) => (None, Vec::new()),
                Declaration::ObjectSet(_) if telling => (None, Vec::new()),
                Declaration::ObjectSet(set) => {
                    let after = after.as_ref().expect("a bundle with a set has its copy");
                    let with = (w, after, &mut decided);
                    (None, self.set_drafts(with, set, declared, now)?)
                }
                // A proposal's plan judges no setting's held value: the
                // accept does, and the plan would tell of a value its
                // proposer may not be answered.
                Declaration::Setting(_) if telling => (None, Vec::new()),
                Declaration::Setting(s) => {
                    let whole = (settled.as_ref()).expect("a bundle with a setting has its copy");
                    (None, self.restamped((w, whole), s, now)?)
                }
                _ => (None, Vec::new()),
            };
            let version = w.ontology.next_version(&declaration);
            drafts.push(self.draft(
                now,
                Origin::Authored,
                EventBody::Declared {
                    declaration,
                    schema_hash,
                    version,
                    with_the_set: false,
                },
            ));
            drafts.extend(with);
        }
        // Every `declared` of such a set but the last, the link types the
        // system declares with a type among them, says that the set goes
        // on: the records it computes again are computed once, at its end.
        let last = drafts
            .iter()
            .rposition(|d| matches!(d.body, EventBody::Declared { .. }));
        for d in &mut drafts[..last.unwrap_or(0)] {
            if let EventBody::Declared { with_the_set, .. } = &mut d.body {
                *with_the_set = computes;
            }
        }
        // Their sets are decided in this append, caused by the last
        // declaration, where the fold computes them again (lakelands#941).
        let mut types = Vec::new();
        if let (Some(last), Some(whole)) = (last, &whole) {
            let event = provisional(self.lake, position_after(w, first + last), &drafts[last]);
            types = self.walked_again((w, whole), (&again, &fields), (&event, now, telling))?;
            if !telling {
                let changes = self.recomputed_sets((w, whole, &decided), &again, &event, now)?;
                drafts.extend(changes);
            }
        }
        Ok(Bundled { drafts, types })
    }

    /// The record of a host's setting the lake holds, stamped again in the
    /// declaration's append where the declaration moves it to another
    /// group, and its value rewritten in the form its declared kind keeps;
    /// one whose value that kind refuses is refused, naming the setting.
    /// The kind is judged by the lake as the whole bundle leaves it, so a
    /// value type declared beside the setting is the one it reads (063,
    /// extended).
    fn restamped(
        &self,
        (w, whole): (&Writer, &Ontology),
        s: &SettingDecl,
        now: Timestamp,
    ) -> Result<Vec<Draft>> {
        let ty = TypeName::from(SETTING_TYPE);
        let key = RecordKey::from(s.key.as_str());
        let view = self.lake.engine.view(None)?;
        let Some(held) = view.get(&ty, &key)?.filter(|r| !r.retired) else {
            return Ok(Vec::new());
        };
        let decl = w.ontology.ty(&ty)?;
        let mut patch = Patch::new();
        if let Some(v) = held.fields.get("value").filter(|v| !v.is_null()) {
            let kept = judged(whole, decl, s, v)?;
            if &kept != v {
                patch.insert("value".into(), kept);
            }
        }
        let group = s.group.clone().map_or(Value::Null, Value::String);
        if held.fields.get("group").unwrap_or(&Value::Null) != &group {
            patch.insert("group".into(), group);
        }
        if patch.is_empty() {
            return Ok(Vec::new());
        }
        let body = EventBody::RecordUpserted {
            ty,
            key,
            version: Version(held.version.0 + 1),
            patch,
            made: false,
        };
        Ok(vec![Draft::system(now, body)])
    }

    /// Each of `types` as `whole` computes its records again, walked once:
    /// a constraint that refuses is left held (lakelands#1119), and the
    /// change's plan is told the type's section, as its author is shown it
    /// (062): the records computed again, those that would be left naming
    /// no live record, and the constraints that admit that they would
    /// break. A type nothing is judged of is neither walked nor counted. A
    /// type its author is not shown, or knows of none of the fields it
    /// computes again (those `fields` names of it, and those derived or
    /// embedded from them), has no section, and one it is not told of a reference of has
    /// none of it; one judged of by nothing it is told counts nothing.
    /// Where `telling`, as a proposal's plan, nothing is refused and no
    /// held record is computed.
    fn walked_again(
        &self,
        (w, whole): (&Writer, &Ontology),
        (types, fields): (
            &BTreeSet<TypeName>,
            &BTreeMap<TypeName, BTreeSet<FieldName>>,
        ),
        (event, now, telling): (&Event, Timestamp, bool),
    ) -> Result<Vec<MigrationReport>> {
        let view = self.lake.engine.view(None)?;
        let computing = whole.computing(&types.iter().cloned().collect::<Vec<_>>());
        let (plans, sight) = (record::Plans::default(), Sight::held(self.who, &w.ontology));
        let mut out = Vec::new();
        let v = view.as_ref();
        for ty in types {
            // What it may move: the fields the type computes.
            let decl = whole.ty(ty)?;
            let computed = (decl.fields.iter())
                .filter(|f| f.derived.is_some() || f.function.is_some() || f.embed.is_some());
            let moving: BTreeSet<FieldName> = computed.map(|f| f.name.clone()).collect();
            let mut walked = Walked::of(whole, decl, &moving, (&sight, telling))?;
            // Its author is told of the fields it knows of alone, of those
            // the bundle computes again and what is derived from them: a
            // type none of whose such fields it knows of has no section,
            // and what is judged of one hidden from it counts nothing it is
            // told.
            // What is derived from a field computed again is too, and an
            // embedding made from one.
            let mut again = fields.get(ty).cloned().unwrap_or_default();
            while let Some(f) = (decl.fields.iter()).find(|f| {
                let from = f
                    .embed
                    .as_ref()
                    .map(|e| e.from.as_slice())
                    .unwrap_or_default();
                !again.contains(&f.name)
                    && (f.depends_on.iter().chain(from)).any(|d| again.contains(d))
            }) {
                again.insert(f.name.clone());
            }
            let known: BTreeSet<FieldName> = (decl.fields.iter())
                .filter(|f| moving.contains(&f.name) && again.contains(&f.name))
                .filter(|f| rights::field_access(self.who, decl, f) != Access::None)
                .map(|f| f.name.clone())
                .collect();
            let mut unlived = Unlived::of(decl, &known, &sight);
            // Walked for what its author is told. A constraint that refuses
            // is told wherever the act judges it, as `Walked::of` grants it
            // only to one who reads it; `judges(false)` guards that.
            let tells = walked.tells() || unlived.watches();
            let judges = walked.judges(false) || tells;
            let shown = |r: &StoredRecord| {
                Ok(!sight.withheld(r) && crate::reads::answered(v, &sight, ty, &r.key)?)
            };
            // Nothing judged: every record is computed again, uncounted. A
            // held record is counted only for one who holds `restricted`.
            let recomputed = match judges {
                false => None,
                true => Some({
                    let (with, mut n) = ((&computing, v, &plans, &*self.lake.keys), 0);
                    live(v, ty, |r| {
                        // A plan that only tells computes no held record: it
                        // counts none, and its inputs are not its author's.
                        if telling && sight.withheld(r) {
                            return Ok(());
                        }
                        n += u64::from(!sight.withheld(r));
                        let is = computed_again(with, r, event, &w.settings)?;
                        unlived.note((r, &is), &sight);
                        walked.judge((r, &is), (w, now), &shown)
                    })?;
                    n
                }),
            };
            let (breaches, lossless) = walked.kept(telling)?;
            // A reference's count to one who reads it and is shown its target.
            let target = |f: &FieldName| {
                (decl.fields.iter().find(|x| &x.name == f))?
                    .ty
                    .target
                    .clone()
            };
            let shows = |f: &FieldName| target(f).is_some_and(|t| sight.ty(&t).is_ok());
            let reads = |f: &FieldName| sight.reads_from(ty, f);
            let mut unlive = unlived.counted(v, &sight, ty)?;
            unlive.retain(|u| reads(&u.field) && shows(&u.field));
            if sight.ty(ty).is_err() || known.is_empty() {
                continue;
            }
            let hash = shape::schema_hash(decl).map_err(invalid)?;
            let mut report = MigrationReport {
                ty: ty.clone(),
                from_schema_hash: hash.clone(),
                to_schema_hash: hash,
                records_scanned: recomputed.unwrap_or(0),
                recomputed,
                unlive,
                breaches,
                lossless,
                ..MigrationReport::default()
            };
            let same = Migration::to(decl.clone());
            rights::shown_hashes(self.who, decl, &same, &mut report, &|n| whole.ty(n).ok());
            report.more = (report.unlive.iter()).any(|u| u.records > u.sample_keys.len() as u64)
                || (report.breaches.iter()).any(|b| b.records > b.sample_keys.len() as u64);
            out.push(report);
        }
        Ok(out)
    }

    /// The changes to the sets of `types`, as `whole` declares them, whose
    /// filter reads a field they compute or that `whole` declares other
    /// than the lake (a set naming one reads its filter too), but those
    /// `decided`; each record as `whole` computes it again, a field
    /// computed outside the lake kept as it was. Past `max_set_changes`,
    /// refused `too_large` naming the set.
    fn recomputed_sets(
        &self,
        (w, whole, decided): (&Writer, &Ontology, &[String]),
        types: &BTreeSet<TypeName>,
        event: &Event,
        now: Timestamp,
    ) -> Result<Vec<Draft>> {
        let (view, max) = (
            self.lake.engine.view(None)?,
            self.lake.limits.max_set_changes,
        );
        let computing = whole.computing(&types.iter().cloned().collect::<Vec<_>>());
        let (plans, mut out) = (record::Plans::default(), Vec::new());
        // A record of one of them as the fold computes it again.
        let again = |r: &StoredRecord| match types.contains(&r.ty) {
            true => computed_again(
                (&computing, view.as_ref(), &plans, &self.lake.keys),
                r,
                event,
                &w.settings,
            ),
            false => Ok(r.clone()),
        };
        // And the sets of other types that read into them across an edge.
        let sets = whole.object_sets.values().map(|e| &e.decl);
        let into = sets.filter(|s| across::reaches_into(whole, s, types));
        let referring: BTreeSet<&TypeName> = into.map(|s| &s.of).collect();
        for ty in types
            .iter()
            .chain(referring.into_iter().filter(|t| !types.contains(*t)))
        {
            let computed: BTreeSet<&FieldName> = (whole.ty(ty)?.fields.iter())
                .filter(|f| f.derived.is_some() || f.function.is_some() || f.embed.is_some())
                .filter(|_| types.contains(ty))
                .map(|f| &f.name)
                .collect();
            let reads = |s: &ObjectSetDecl| {
                let at = [Place::ObjectSet {
                    name: s.name.clone(),
                }];
                let filter = declare::set_filter(&|n| whole.set(n), s, &at);
                across::reaches_into(whole, s, types)
                    || filter.is_ok_and(|e| {
                        lakelands_core::derive::reads(&e)
                            .iter()
                            .any(|f| computed.contains(f))
                    })
            };
            let moved = |s: &ObjectSetDecl| w.ontology.set(&s.name) != Some(s);
            let mut sets = Vec::new();
            for s in (whole.object_sets.values().map(|e| &e.decl)).filter(|s| &s.of == ty) {
                let named = |m: &ObjectSetDecl| moved(m) && declare::names_set(whole, s, &m.name);
                let bundled = whole.object_sets.values().any(|m| named(&m.decl));
                if !decided.contains(&s.name) && (reads(s) || moved(s) || bundled) {
                    let held = match w.ontology.set(&s.name) {
                        Some(_) => view.members(&s.name, ty)?,
                        None => BTreeSet::new(),
                    };
                    sets.push((s, held));
                }
            }
            if sets.is_empty() {
                continue;
            }
            let far = |t: &TypeName, k: &RecordKey| view.get(t, k)?.map(|r| again(&r)).transpose();
            let link =
                |t: &TypeName, r: &RelationName, k: &RecordKey| link_from(view.as_ref(), t, r, k);
            let max_depth = self.lake.limits.max_depth;
            let heads = Heads {
                far: &far,
                link: &link,
                max: max_depth,
            };
            live(view.as_ref(), ty, |r| {
                let is = &again(r)?;
                for (s, held) in &sets {
                    let member = member_of((s, whole), is, w, now, heads)?;
                    if held.contains(&r.key) == member {
                        continue;
                    }
                    if out.len() as u64 >= u64::from(max) {
                        let (field, reason) =
                            (Some(s.name.as_str().into()), Refusal::TooLarge { max });
                        return Err(Error::Refused { field, reason });
                    }
                    let change = (s.name.clone(), ty.clone(), r.key.clone(), member);
                    out.push(self.changed(change, event.position, now));
                }
                Ok(())
            })?;
        }
        Ok(out)
    }

    /// The membership a set's declaration decides for the records already
    /// there: an `object_set_changed` for each record of its type whose
    /// membership differs from the index, read a page at a time, each
    /// caused by the `declared` at `declared` (004, extended). The sets in
    /// force that name a set are decided with it, over `after`, the
    /// ontology as the append leaves it: a set is its filter, so a change
    /// reaches every set built on it, in the same append, and each of them
    /// once, whatever number of the sets it names the append declares
    /// (`decided`). A set declared again unchanged decides nothing. Past the lake's
    /// `max_set_changes`, counted over all of them, the declaration is
    /// refused `too_large`, naming the set whose change passed it, before
    /// anything is appended; the walk stops there.
    fn set_drafts(
        &self,
        (w, after, decided): (&Writer, &Ontology, &mut Vec<String>),
        set: &ObjectSetDecl,
        declared: Position,
        now: Timestamp,
    ) -> Result<Vec<Draft>> {
        let old = w.ontology.object_sets.get(&set.name).map(|e| &e.decl);
        if old.is_some_and(|o| o.filter == set.filter) {
            return Ok(Vec::new());
        }
        let (view, ty) = (self.lake.engine.view(None)?, &set.of);
        // The sets in force that name this one and stand as they are, each
        // decided once in an append, with its members so far, read once.
        let mut sets = sets_of(w, view.as_ref(), ty, |s| {
            let stands = after.object_sets.get(&s.name).is_some_and(|e| &e.decl == s);
            stands && !decided.contains(&s.name) && declare::names_set(after, s, &set.name)
        })?;
        decided.extend(sets.iter().map(|(s, _)| s.name.clone()));
        // A new set has no member so far.
        let held = match old {
            Some(_) => view.members(&set.name, ty)?,
            None => BTreeSet::new(),
        };
        sets.insert(0, (set, held));
        let max = self.lake.limits.max_set_changes;
        let mut out = Vec::new();
        at_head(view.as_ref(), self.lake.limits.max_depth, |heads| {
            live(view.as_ref(), ty, |r| {
                for (s, held) in &sets {
                    let is = member_of((s, after), r, w, now, heads)?;
                    if held.contains(&r.key) != is {
                        if out.len() as u64 >= u64::from(max) {
                            return Err(Error::Refused {
                                field: Some(s.name.as_str().into()),
                                reason: Refusal::TooLarge { max },
                            });
                        }
                        let change = (s.name.clone(), ty.clone(), r.key.clone(), is);
                        out.push(self.changed(change, declared, now));
                    }
                }
                Ok(())
            })
        })?;
        Ok(out)
    }

    /// `add_field`.
    pub fn add_field(&self, ty: &TypeName, field: FieldDecl) -> Result<Outcome> {
        let (ty, field) = (ty.clone(), Box::new(field));
        self.make(
            lakelands_api::write::Change::AddField { ty, field },
            Against::default(),
        )
    }

    /// The preflight of a migration over every stored record of the type,
    /// retired and restricted ones included: they are history, the fold
    /// converts them with the rest, and a migration that cannot convert
    /// one is not lossless, so it is refused before anything is appended.
    ///
    /// A migration that makes a record's key differently (other key
    /// fields, or a key field's value converted) re-keys it: the preflight
    /// counts the moves, with the closure and the references they take
    /// along (`crate::rekey`), and a collision or a missing key value makes
    /// it not lossless. One that takes every key field away is refused.
    pub fn preflight(
        &self,
        w: &Writer,
        view: &dyn ProjectionView,
        m: &Migration,
    ) -> Result<MigrationReport> {
        granted(self.who.grants.schema && !rights::holds_only(self.who))?;
        let mut m = m.clone();
        let author = self.authored(&w.ontology, &mut m)?;
        let report = self
            .preflight_plan(w, view, (&m, author.as_ref(), None), None)?
            .report;
        self.shown(w, &m, report)
    }

    /// A report as the caller may know it (013, 024 and 027, extended):
    /// wherever one is answered or refuses a write, its reader is a reader
    /// like any other, the author of its own preview too. Its id is made
    /// over what it is shown, and an apply compares that.
    fn shown(
        &self,
        w: &Writer,
        m: &Migration,
        mut report: MigrationReport,
    ) -> Result<MigrationReport> {
        let view = self.lake.engine.view(None)?;
        let sight = Sight::held(self.who, &w.ontology);
        let sees = |ty: &TypeName| sight.ty(ty).ok();
        let (was, mut m) = (w.ontology.ty(&m.to.name)?, m.clone());
        w.ontology.complete(&mut m.to);
        let described = || sight.describe(&self.who.lake);
        rights::shown_report(self.who, was, &m, &mut report, described, sees);
        rights::shown_hashes(self.who, was, &m, &mut report, &|n| w.ontology.ty(n).ok());
        report.report_id = report_id(&report, view.position());
        Ok(report)
    }

    /// The refusal of a migration that is not lossless, with its report as
    /// the caller may know it.
    fn lossy(&self, w: &Writer, m: &Migration, report: MigrationReport) -> Error {
        match self.shown(w, m, report) {
            Ok(report) => refused(Refusal::Migration {
                report: Box::new(report),
            }),
            Err(e) => e,
        }
    }

    /// The preflight, the record keys the migration moves and, given the
    /// time it is applied at, the set memberships it changes: each record
    /// as migrated is decided by the rule a write uses, in the walk the
    /// preflight makes anyway. Past the lake's `max_set_changes` it decides
    /// no further, and the plan holds one change too many: a stale report
    /// and a migration that is not lossless are refused before that is.
    pub(crate) fn preflight_plan(
        &self,
        w: &Writer,
        view: &dyn ProjectionView,
        (m, author, judged): (&Migration, Option<&Authored>, Option<&Principal>),
        now: Option<Timestamp>,
    ) -> Result<MigrationPlan> {
        let before = w.ontology.ty(&m.to.name)?.clone();
        // A migration reshapes stored records, and a view stores none. A
        // view changes, or becomes a stored type, by being declared again;
        // a stored type never becomes a view, since its records would
        // linger beside the computed ones: the view takes another name.
        if before.view.is_some() || m.to.view.is_some() {
            let ty = m.to.name.clone();
            return Err(invalid(
                match (before.view.is_some(), m.to.view.is_some()) {
                    (true, true) => Case::MigrationOfView { ty },
                    (true, false) => Case::MigrationViewToStored { ty },
                    _ => Case::MigrationToView { ty },
                },
            ));
        }
        if !before.key.is_empty() && m.to.key.is_empty() {
            let ty = before.name.clone();
            return Err(invalid(Case::MigrationTakesKey { ty }));
        }
        if m.to.key != before.key {
            for k in &m.to.key {
                if !m.to.fields.iter().any(|f| &f.name == k && f.required) {
                    let said = Said::from(Case::KeyFieldRequired { field: k.clone() });
                    let name = m.to.name.clone();
                    return Err(invalid_field(k, said.within(Place::Type { name })));
                }
            }
        }
        let mut probe = w.ontology.clone();
        probe.put(Declaration::Type(before.clone()), 0, None);
        let to = Declaration::Type(m.to.clone());
        if let Err(e) = declare::check_declaration(&probe, &to, false) {
            // A reshape is answered before the tools' names are read, and a
            // migration leaves no problem among them either (027, extended).
            declare::unless_a_migration(e)?;
            declare::tool_names(&probe, &to)?;
        }
        // A view over the type, as its source or a merged one, reads the
        // shape the migration leaves: `declare` must still take each such
        // view against it, or the view would refuse every read after.
        let mut after = probe.clone();
        after.put(Declaration::Type(m.to.clone()), 0, None);
        // The refusal of a migration that takes away a field `reader` reads:
        // one the type, or a link type of it, has before it and not after.
        let has = |t: &TypeDecl, n: &FieldName| t.fields.iter().any(|f| &f.name == n);
        let after_it = |said: Said| {
            let name = m.to.name.clone();
            invalid(said.within(Place::AfterMigration { name }))
        };
        let gone = |reader: Option<Place>,
                    names: &BTreeSet<FieldName>,
                    was: &TypeDecl,
                    is: &TypeDecl| {
            let n = names.iter().find(|n| has(was, n) && !has(is, n))?;
            // At an accept the field may be one the accepter may not know of,
            // taken away knowingly by the proposer: it is told neither the
            // field nor its reader (013, extended).
            let unknown = |f: &&FieldDecl| rights::field_access(self.who, was, f) == Access::None;
            if was
                .fields
                .iter()
                .filter(|f| &f.name == n)
                .any(|f| unknown(&f))
            {
                return Some(after_it(Case::ReadsTakenAway.into()));
            }
            let renamed = |(from, _): &&(FieldName, FieldName)| from == n && is.name == m.to.name;
            let renamed = m.renames.iter().find(renamed).map(|(_, to)| to.clone());
            let (unnamed, field, ty) = (reader.is_none(), n.clone(), is.name.clone());
            let case = Case::MigrationTakesRead {
                unnamed,
                field,
                ty,
                renamed,
            };
            let said = Said::from(case);
            Some(after_it(match reader {
                Some(reader) => said.within(reader),
                None => said,
            }))
        };
        for reader in w.ontology.types.values().map(|e| &e.decl) {
            let Some(v) = reader.view.as_deref() else {
                continue;
            };
            if v.source != m.to.name && !v.merge.iter().any(|s| s.source == m.to.name) {
                continue;
            }
            let view = Declaration::Type(reader.clone());
            // `declare` checks a view's grouping against its sources; the
            // fields its `where` and measures read are checked by the rule
            // `describe` uses to show a view.
            let reads = describe::view_reads(v).unwrap_or_default();
            let taken = |reader: Option<Place>| {
                let mut of = reads.iter().filter(|(source, _)| source == &m.to.name);
                of.find_map(|(_, names)| gone(reader.clone(), names, &before, &m.to))
            };
            let after_place = Place::AfterMigration {
                name: m.to.name.clone(),
            };
            let checked =
                declare::check_declaration(&after, &view, false).map_err(|e| e.within(after_place));
            let name = reader.name.clone();
            let named =
                checked.and_then(|()| taken(Some(Place::View { name })).map_or(Ok(()), Err));
            // The view is named only to an author `describe` shows its
            // definition to (027, extended); another is told that a
            // declaration reads the field, or that one could not be declared.
            let Err(named) = named else { continue };
            if Sight::held(self.who, &w.ontology).shows(&view) {
                return Err(named);
            }
            return Err(taken(None).unwrap_or_else(|| {
                let ty = m.to.name.clone();
                after_it(Case::NoLongerDeclarable { ty }.into())
            }));
        }
        // So does every other declaration in force (027, extended): the
        // type's derived fields, another type's derived relation to it, and
        // the sets, triggers and verbs on it keep the fields they read; and
        // those on a link type it changes keep the relation's fields, one
        // that stands for a renamed relation's links to arrive in too.
        let links = changed_link_types(&w.ontology, m, true);
        let kept = links.iter().map(|(was, is)| (was, is));
        for (was, is) in std::iter::once((&before, &m.to)).chain(kept) {
            for d in after.declared() {
                let reads = declare::reads_of(&w.ontology, &d.declaration, &was.name);
                let Some(named) = gone(Some(d.declaration.place()), &reads, was, is) else {
                    continue;
                };
                // The reader is named only to an author `describe` shows it
                // to, as it stands in force, and by what it is shown of it: a
                // relation it is not shown reads nothing it is told of
                // (lakelands#662). Another is told that a declaration reads
                // the field. The field is one the author knows of: one
                // hidden from it was put back (`as_author`).
                if Sight::held(self.who, &w.ontology).shows(&d.declaration) {
                    let Declaration::Type(t) = &d.declaration else {
                        return Err(named);
                    };
                    let mut t = t.clone();
                    rights::known_fields(self.who, &mut t, &|n| w.ontology.ty(n).ok());
                    let seen = Declaration::Type(t);
                    let seen = describe::declaration_reads(&seen, &was.name, &|_| None);
                    if let Some(named) = gone(Some(d.declaration.place()), &seen, was, is) {
                        return Err(named);
                    }
                }
                gone(None, &reads, was, is).map_or(Ok(()), Err)?;
            }
        }
        // A verb is the one door left to a link type no relation carries,
        // so a relation is not removed while a verb in force is declared on
        // its link type (035; 027, extended). The verb is named to an author
        // `describe` shows it to.
        let (kept, sees) = (
            links::link_types_of(&m.to),
            Sight::held(self.who, &w.ontology),
        );
        for r in before.relations.iter().filter(|r| r.carried_by_links()) {
            let ty = links::link_type_name(&before.name, &r.name);
            // A renamed relation is the rename's to judge, below.
            let renamed = m.relation_renames.iter().any(|(old, _)| old == &r.name);
            if renamed || kept.iter().any(|t| t.name == ty) {
                continue;
            }
            let on = (w.ontology.verbs.values()).filter(|e| e.decl.on == ty);
            let mut on = on.map(|e| Declaration::Verb(e.decl.clone())).peekable();
            if on.peek().is_some() {
                let shown = on.find(|v| sees.shows(v));
                let (unnamed, relation) = (shown.is_none(), r.name.clone());
                let said = Said::from(Case::MigrationTakesLinkType {
                    unnamed,
                    relation,
                    ty,
                });
                return Err(after_it(match shown {
                    Some(verb) => said.within(verb.place()),
                    None => said,
                }));
            }
            // Nor while a relation or a reference field that stands after it
            // refers to the link type, a link type's own `to` aside, which is
            // its relation's: it would name the links of no relation (035);
            // named to one shown it.
            let types = after.types.values().map(|e| &e.decl);
            let field = |t: &TypeDecl, f: &FieldDecl| {
                let end = links::is_link_type(&t.name) && f.name.as_str() == links::TO;
                !end && f.ty.target.as_ref() == Some(&ty)
            };
            let refers = |t: &&TypeDecl| {
                let fields = t.fields.iter().any(|f| field(t, f));
                t.relations.iter().any(|o| o.target == ty) || fields
            };
            let mut referring = types.filter(refers).peekable();
            if referring.peek().is_some() {
                let shown = referring.find_map(|t| {
                    let related = |o: &&RelationDecl| o.target == ty && sees.relation(t, o);
                    let known = |f: &&FieldDecl| {
                        field(t, f) && rights::field_access(self.who, t, f) != Access::None
                    };
                    let place = match t.relations.iter().find(related) {
                        Some(o) => Place::Relation {
                            name: o.name.clone(),
                        },
                        None if sees.ty(&t.name).is_ok() => {
                            let f = t.fields.iter().find(known)?;
                            Place::Field {
                                name: f.name.clone(),
                            }
                        }
                        None => return None,
                    };
                    Some((t.name.clone(), place))
                });
                let (unnamed, relation) = (shown.is_none(), r.name.clone());
                let said = Said::from(Case::MigrationTakesTarget {
                    unnamed,
                    relation,
                    ty,
                });
                return Err(after_it(match shown {
                    Some((of, place)) => said.within(place).within(Place::Type { name: of }),
                    None => said,
                }));
            }
        }
        // And the type it migrates to reads only fields it has: `declare`'s
        // check above ends at the reshaping, before it comes to the reads.
        declare::reads_its_type(&after, &Declaration::Type(m.to.clone()))?;
        // So does every verb that reads into it (028, 055).
        let sees = Sight::held(self.who, &w.ontology);
        let shows = |d: &Declaration| sees.shows(d);
        verbs_judged_again((&w.ontology, &after), &before, &m.to, &shows).map_err(after_it)?;
        // A record that does not convert is already not lossless; its
        // stored fields still tell whether it has a new key.
        // A live record holds what the migration fills, as the fold leaves
        // it, so its key and its memberships are decided with it (050).
        let convert = |decl: &TypeDecl, m: &Migration, r: &StoredRecord| {
            let fields = shape::migrate_record(decl, m, &r.fields);
            let mut fields = fields.map_err(invalid)?.unwrap_or_else(|| r.fields.clone());
            for f in (m.to.fields.iter()).filter(|f| m.fill.contains(&f.name) && !r.retired) {
                if !fields.contains_key(&f.name) {
                    fields.extend(record::initial_of(&after, f).map(|v| (f.name.clone(), v)));
                }
            }
            Ok::<_, Error>(fields)
        };
        // Applied, the migration decides again every record whose fields it
        // changes, as the fold leaves it (004, extended): converted, its
        // references to moved keys rewritten, under its key after `moves`.
        let mut members = Vec::new();
        let limit = self.lake.limits.max_set_changes as usize;
        // Where the fold computes the type again, a record is decided as it
        // computes it, over the lake after the migration (lakelands#1070).
        let again = (!m.fill.is_empty() && computes(&after, &m.to.name))
            || computing_changed(&before, &m.to);
        let (computing, plans) = (
            after.computing(std::slice::from_ref(&m.to.name)),
            Default::default(),
        );
        // Its constraints are judged on the records as computed again, at
        // the lake's last time where nothing is applied (lakelands#1119).
        let judged_at = now.unwrap_or(w.last_known_at);
        let event = again.then(|| {
            let body = EventBody::Declared {
                declaration: Declaration::Type(m.to.clone()),
                schema_hash: None,
                version: 0,
                with_the_set: false,
            };
            provisional(
                self.lake,
                position_after(w, 0),
                &self.draft(judged_at, Origin::System, body),
            )
        });
        // A record as the migration leaves it: its references to moved keys
        // rewritten, under its key after `moves`, computed again where the
        // fold computes its type again.
        let migrated = |moves: &crate::rekey::Moves, decl: &TypeDecl, r: &StoredRecord, fields| {
            let moved = moves.get(&r.ty).and_then(|m| m.get(&r.key));
            let mut is = StoredRecord {
                key: moved.unwrap_or(&r.key).clone(),
                fields: crate::rekey::rewritten(decl, &fields, moves),
                ..r.clone()
            };
            if let Some(e) = event.as_ref().filter(|_| r.ty == m.to.name) {
                is = computed_again(
                    (&computing, view, &plans, &self.lake.keys),
                    &is,
                    e,
                    &w.settings,
                )?;
            }
            Ok::<_, Error>(is)
        };
        // What a set's path reads across an edge, as the migration leaves
        // it: the record at a key after `moves`, converted.
        let far = |moves: &crate::rekey::Moves, t: &TypeName, k: &RecordKey| {
            let mut back = moves.get(t).into_iter().flatten().filter(|(_, n)| *n == k);
            let old = back.next().map_or(k, |(o, _)| o);
            let Some(r) = view.get(t, old)? else {
                return Ok(None);
            };
            let fields = match t == &m.to.name {
                true => convert(&before, m, &r)?,
                false => r.fields.clone(),
            };
            migrated(moves, after.ty(t)?, &r, fields).map(Some)
        };
        let link = |t: &TypeName, r: &RelationName, k: &RecordKey| link_from(view, t, r, k);
        let max_depth = self.lake.limits.max_depth;
        let left = |moves: &crate::rekey::Moves,
                    sets: &Held<'_>,
                    decl: &TypeDecl,
                    r: &StoredRecord,
                    fields,
                    now,
                    members: &mut Vec<Change>| {
            let is = migrated(moves, decl, r, fields)?;
            let far = |t: &TypeName, k: &RecordKey| far(moves, t, k);
            let heads = Heads {
                far: &far,
                link: &link,
                max: max_depth,
            };
            decide(sets, &r.key, &is, (w, now, limit), members, heads)
        };
        let held = |ty: &TypeName| match now {
            Some(_) => sets_of(w, view, ty, |_| true),
            None => Ok(Vec::new()),
        };
        // One walk of the type, a page at a time, makes the report, the key
        // of every record as the new key fields make it, moved or not (a
        // key the new fields make differently moves the record, and what
        // names it moves with it: `crate::rekey`), and each record's
        // memberships as if no key moved.
        let (sets, mut keyed) = (held(&m.to.name)?, Vec::new());
        // The renames as the author sent them are judged first, against the
        // type as it sees it: a field it may not know of is no field to it,
        // here, as an absent one is (lakelands#750).
        // So is what it fills, against the fields it declared itself.
        if let Some(a) = author {
            a.sent.renamed_fields(&a.known).map_err(invalid)?;
            a.sent.fills().map_err(invalid)?;
        }
        m.fills().map_err(invalid)?;
        // Each field the migration fills: the live records that hold no
        // value for it, by the name they hold it under, and the first keys,
        // in key order, of those its reader is answered.
        let none = |f: &FieldName| (f.clone(), 0, Vec::new());
        let mut fills: Vec<(FieldName, u64, Vec<RecordKey>)> = m.fill.iter().map(none).collect();
        // The types a re-key may move, told by the declarations alone: the
        // migrated type where its key, or a key field's type, changes, and
        // each type whose key references one of them. The references to
        // them, in any other type, are what the re-key may rewrite.
        let renamed_to = |k: &FieldName| {
            let to = m.renames.iter().find(|(o, _)| o == k);
            to.map_or_else(|| k.clone(), |(_, n)| n.clone())
        };
        let typed = |t: &TypeDecl, k: &FieldName| {
            let f = t.fields.iter().find(|f| &f.name == k);
            f.map(|f| f.ty.clone())
        };
        let rekeys = before
            .key
            .iter()
            .map(renamed_to)
            .ne(m.to.key.iter().cloned())
            || (before.key.iter().zip(&m.to.key))
                .any(|(o, n)| typed(&before, o) != typed(&m.to, n));
        let mut moving: BTreeSet<TypeName> =
            rekeys.then(|| m.to.name.clone()).into_iter().collect();
        let targets = |t: &TypeDecl, f: &FieldName, among: &BTreeSet<TypeName>| {
            typed(t, f)
                .and_then(|ty| ty.target)
                .is_some_and(|x| among.contains(&x))
        };
        let decls = || (after.types.values().map(|e| &e.decl)).filter(|t| t.view.is_none());
        loop {
            let keyed = |t: &&TypeDecl| {
                !moving.contains(&t.name) && t.key.iter().any(|k| targets(t, k, &moving))
            };
            let more: Vec<TypeName> = decls().filter(keyed).map(|t| t.name.clone()).collect();
            if more.is_empty() {
                break;
            }
            moving.extend(more);
        }
        // What the migration may move of its own type, told by its
        // declaration alone: a field it converts, fills, renames or adds,
        // one it computes again, a reference it may rewrite. A constraint
        // its author is not told is judged by none of them: one that
        // refuses and that it may move is not granted, before any record
        // is read, and none is counted in its report (lakelands#1119).
        let was = |f: &FieldName| {
            let from = m.renames.iter().find(|(_, n)| n == f);
            from.map_or_else(|| f.clone(), |(o, _)| o.clone())
        };
        let computed =
            |f: &FieldDecl| f.derived.is_some() || f.function.is_some() || f.embed.is_some();
        let moves_field = |f: &FieldDecl| {
            let old = was(&f.name);
            m.fill.contains(&f.name)
                || old != f.name
                || typed(&before, &old) != Some(f.ty.clone())
                || (again && computed(f))
                || targets(&m.to, &f.name, &moving)
        };
        let moved: BTreeSet<FieldName> = (m.to.fields.iter())
            .filter(|f| moves_field(f))
            .map(|f| f.name.clone())
            .collect();
        let shape = record::Plans::default().get(&after, &m.to);
        let told = |c: &record::Constraint| {
            (c.reads.iter()).all(|f| rights::reads_both(self.who, &before, m, f))
        };
        for c in shape
            .constraints
            .iter()
            .filter(|c| !c.fed.is_disjoint(&moved))
        {
            granted(!c.decl.at_ingest.refuses() || told(c))?;
        }
        let untold: BTreeSet<&ConstraintName> = (shape.constraints.iter())
            .filter(|c| !told(c))
            .map(|c| &c.decl.name)
            .collect();
        // Each constraint `to` declares: the live records it would leave
        // breaking it, as converted and filled (lakelands#1119).
        let parse = |c: &ConstraintDecl| declare::typed(&c.holds, Eval::Guard).map_err(invalid);
        // The records told of (a held one only to a holder of
        // `restricted`, 062), the first keys, and whether any breaks it.
        type Breaking<'c> = (
            &'c ConstraintDecl,
            lakelands_api::expr::Expr,
            u64,
            Vec<RecordKey>,
            bool,
        );
        let breaking = (m.to.constraints.iter()).filter(|c| !untold.contains(&c.name));
        let breaking = breaking.map(|c| Ok((c, parse(c)?, 0, Vec::new(), false)));
        let mut breaking: Vec<Breaking<'_>> = breaking.collect::<Result<_>>()?;
        // A live record counted against each, as the migration leaves it.
        let tally = |breaking: &mut Vec<Breaking<'_>>, r: &StoredRecord, is: &StoredRecord| {
            let scope = WriteScope {
                after: &is.fields,
                before: Some(&is.fields),
                retiring: false,
                meta: Some(&is.meta),
                settings: Some(&w.settings),
                now: judged_at,
            };
            for (_, e, records, samples, any) in breaking.iter_mut() {
                if !expr::holds(e, &scope).unwrap_or(false) {
                    *any = true;
                    *records += u64::from(!sees.withheld(r));
                    let shown = || crate::reads::answered(view, &sees, &m.to.name, &r.key);
                    if samples.len() < 5 && !sees.withheld(r) && shown()? {
                        samples.push(r.key.clone());
                    }
                }
            }
            Ok::<_, Error>(())
        };
        // A retarget is judged as its author is answered; at an accept, its
        // proposer (lakelands#1071).
        let onto = Sight::held(judged.unwrap_or(self.who), &w.ontology);
        // The references it may move, told as `shown_report` tells them, and
        // the live records it computes again (062).
        let (mut unlived, mut recomputed) = (Unlived::of(&m.to, &moved, &sees), 0);
        let mut report = shaped(view, (&sees, &onto), &before, m, |r| {
            for (field, records, samples) in fills.iter_mut().filter(|_| !r.retired) {
                let held = m.renames.iter().find(|(_, to)| to == field);
                let held = held.map_or(&*field, |(from, _)| from);
                if !r.fields.contains_key(held) {
                    *records += u64::from(!sees.withheld(r));
                    let shown = || crate::reads::answered(view, &sees, &m.to.name, &r.key);
                    if samples.len() < 5 && !sees.withheld(r) && shown()? {
                        samples.push(r.key.clone());
                    }
                }
            }
            if !r.retired {
                recomputed += u64::from(again && !sees.withheld(r));
                if !breaking.is_empty() || unlived.watches() {
                    let is = migrated(&BTreeMap::new(), &m.to, r, convert(&before, m, r)?)?;
                    if !breaking.is_empty() {
                        tally(&mut breaking, r, &is)?;
                    }
                    unlived.note((r, &is), &sees);
                }
            }
            if m.to.key.is_empty() && sets.is_empty() {
                return Ok(());
            }
            let fields = convert(&before, m, r)?;
            if !m.to.key.is_empty() {
                let key = lakelands_core::log::record_key(&m.to, &fields);
                keyed.push((r.key.clone(), key));
            }
            match now.filter(|_| !sets.is_empty()) {
                Some(now) => left(&BTreeMap::new(), &sets, &m.to, r, fields, now, &mut members),
                None => Ok(()),
            }
        })?;
        let filled = |(field, records, sample_keys)| FieldFill {
            field,
            records,
            sample_keys,
        };
        report.filled = fills.into_iter().map(filled).collect();
        (report.recomputed, report.unlive) =
            (Some(recomputed), unlived.counted(view, &sees, &m.to.name)?);
        // A colliding key is the new key's fields' values: named to one
        // who reads each of them and is answered two of the records that
        // would take it, a collision it can see for itself.
        let reads = (m.to.key_fields().iter()).all(|f| rights::reads_both(self.who, &before, m, f));
        let seen = |ty: &TypeName, keys: &[&RecordKey]| match reads {
            true => crate::reads::answered_of(view, &sees, ty, keys, 2),
            false => Ok(false),
        };
        let plan = crate::rekey::plan(&after, view, &m.to.name, keyed, &m.renames, &seen)?;
        // With moves its constraints are judged again, on the references the
        // moves leave, a second walk; and the records of other types whose
        // references they rewrite hold theirs (lakelands#1119).
        if !breaking.is_empty() && !plan.moves.is_empty() {
            breaking
                .iter_mut()
                .for_each(|b| (b.2, b.3, b.4) = (0, Vec::new(), false));
            crate::lake::walk(view, &before.name, true, |r| match r.retired {
                true => Ok(()),
                false => tally(
                    &mut breaking,
                    &r,
                    &migrated(&plan.moves, &m.to, &r, convert(&before, m, &r)?)?,
                ),
            })?;
        }
        for (c, _, records, sample_keys, _) in breaking.into_iter().filter(|b| b.4) {
            report.lossless &= !c.at_ingest.refuses();
            if records == 0 {
                continue;
            }
            let constraint = c.name.clone();
            report.breaches.push(Breach {
                ty: m.to.name.clone(),
                constraint,
                records,
                sample_keys,
            });
        }
        let mut others = Vec::new();
        for decl in decls().filter(|t| t.name != m.to.name) {
            let refs = (decl.fields.iter()).filter(|f| targets(decl, &f.name, &moving));
            let refs: BTreeSet<FieldName> = refs.map(|f| f.name.clone()).collect();
            if refs.is_empty() {
                continue;
            }
            let mut walked = Walked::of(&after, decl, &refs, (&sees, false))?;
            // The references the moves rewrite, as they leave them (062).
            let mut unlived = Unlived::of(decl, &refs, &sees);
            if (walked.judges(true) || unlived.watches()) && !plan.moves.is_empty() {
                let shown = |r: &StoredRecord| match sees.withheld(r) {
                    true => Ok(false),
                    false => crate::reads::answered(view, &sees, &decl.name, &r.key),
                };
                live(view, &decl.name, |r| {
                    let fields = crate::rekey::rewritten(decl, &r.fields, &plan.moves);
                    let is = StoredRecord {
                        fields,
                        ..r.clone()
                    };
                    // A rewritten reference names the record its old key
                    // named, which moves with it: judged by the old key.
                    unlived.note((r, r), &sees);
                    walked.judge((r, &is), (w, judged_at), &shown)
                })?;
                let unlive = unlived.counted(view, &sees, &decl.name)?;
                if !unlive.is_empty() {
                    others.push(MigrationReport {
                        ty: decl.name.clone(),
                        recomputed: Some(0),
                        unlive,
                        lossless: true,
                        ..MigrationReport::default()
                    });
                }
            }
            report.breaches.extend(walked.kept(false)?.0);
        }
        // With moves the memberships are decided again, under the keys and
        // the references the moves leave: a second walk.
        if let Some(now) = now.filter(|_| !sets.is_empty() && !plan.moves.is_empty()) {
            members.clear();
            crate::lake::walk(view, &before.name, true, |r| {
                let fields = convert(&before, m, &r)?;
                left(&plan.moves, &sets, &m.to, &r, fields, now, &mut members)
            })?;
        }
        // A relation carried by link records whose cardinality narrows
        // keeps its links only if none would break the narrower rule.
        for r in m.to.relations.iter().filter(|r| r.carried_by_links()) {
            let Some(old) = before
                .relations
                .iter()
                .find(|o| o.name == r.name && o.carried_by_links())
            else {
                continue;
            };
            let ((was_from, was_to), (per_from, per_to)) =
                (one_per(old.cardinality), one_per(r.cardinality));
            if !(per_from && !was_from) && !(per_to && !was_to) {
                continue;
            }
            let lt = links::link_type_name(&m.to.name, &r.name);
            // The live links at each end, by the record there: a page of
            // the links at a time, the ends kept.
            let mut ends: [BTreeMap<String, u32>; 2] = Default::default();
            live(view, &lt, |l| {
                for (seen, at) in ends.iter_mut().zip([links::FROM, links::TO]) {
                    let key = l.fields.get(at).and_then(Value::as_str).unwrap_or_default();
                    *seen.entry(key.to_owned()).or_default() += 1;
                }
                Ok(())
            })?;
            // How many records hold more than one live link at an end.
            let over = |end: usize| ends[end].values().filter(|n| **n > 1).count();
            let broken = if per_from { over(0) } else { 0 } + if per_to { over(1) } else { 0 };
            if broken > 0 {
                let (relation, cardinality, count) = (r.name.clone(), r.cardinality, broken);
                let case = Case::NarrowingBreaksLinks {
                    relation,
                    cardinality,
                    count,
                };
                return Err(constraint(None, case));
            }
        }
        // The relations carried by link records whose fields change migrate
        // their link types with the type: their records are checked here,
        // each field named with its relation.
        for (old, new) in changed_link_types(&w.ontology, m, false) {
            let lm = Migration::to(new);
            let sets = held(&old.name)?;
            let lr = shaped(view, (&sees, &onto), &old, &lm, |r| {
                match now.filter(|_| !sets.is_empty()) {
                    Some(now) => {
                        let fields = convert(&old, &lm, r)?;
                        left(&plan.moves, &sets, &lm.to, r, fields, now, &mut members)
                    }
                    None => Ok(()),
                }
            })?;
            let relation = old.name.as_str().rsplit('.').next().unwrap_or_default();
            let named = |f: &FieldName| FieldName::from(format!("{relation}.{f}"));
            report.records_scanned += lr.records_scanned;
            report.lossless &= lr.lossless;
            report.fields.extend(lr.fields.into_iter().map(|mut c| {
                c.field = named(&c.field);
                c
            }));
            report
                .dropped
                .extend(lr.dropped.into_iter().map(|(f, n)| (named(&f), n)));
        }
        // A relation the migration removes is listed as a removed field
        // is: one carried by link records takes its live links from the
        // head, and is listed with their number, another with none. A
        // relation of another name is another relation; a rename is not
        // inferred.
        // So are its relation renames, against the relations it is shown.
        if let Some(a) = author {
            a.sent.renamed_relations(&a.known).map_err(invalid)?;
        }
        let pairs = m.renamed_relations(&before).map_err(invalid)?;
        let removed = |r: &&RelationDecl| {
            !m.to.relations.iter().any(|n| n.name == r.name)
                && !pairs.iter().any(|(was, _)| was.name == r.name)
        };
        let mut unlinked = Vec::new();
        for r in before.relations.iter().filter(removed) {
            let lt = links::link_type_name(&m.to.name, &r.name);
            let (carried, held) = (r.carried_by_links(), unlinked.len());
            // A re-key would move the links its retirements name: one
            // migration removes, another re-keys, as for a rename.
            if carried && !plan.moves.is_empty() {
                let name = r.name.clone();
                let said = Said::from(Case::RemoveWhileRekeying);
                return Err(invalid(said.within(Place::Relation { name })));
            }
            if carried && now.is_some() {
                live(view, &lt, |l| {
                    unlinked.push(l.clone());
                    Ok(())
                })?;
            }
            let links = match (carried, now) {
                (true, None) => view.count(&lt, &every(false))?,
                _ => (unlinked.len() - held) as u64, // the number it retires
            };
            let name = FieldName::from(r.name.as_str());
            report.dropped.push((name, links));
        }
        // What `to` states otherwise besides, each shape completed as one
        // in force is, so a `depends_on` left to the store restates nothing.
        let (mut was, mut is) = (before.clone(), m.clone());
        w.ontology.complete(&mut was);
        after.complete(&mut is.to);
        report.restated = is.restated(&was);
        // A renamed relation keeps its edges. One carried by link records
        // moves them to the link type of its new name, which must hold none
        // of an earlier relation's, and not in the migration that re-keys.
        for (was, is) in pairs {
            let (carrier, mut moved) = (Carrier::of(was), 0);
            if carrier == Carrier::Links {
                let wrong = |why: Case| {
                    let (from, to) = (was.name.clone(), is.name.clone());
                    invalid(Said::from(why).within(Place::RelationRename { from, to }))
                };
                if !plan.moves.is_empty() {
                    return Err(wrong(Case::RenameWhileRekeying));
                }
                if stored(view, &links::link_type_name(&m.to.name, &is.name))? > 0 {
                    return Err(wrong(Case::RenameNameHeld));
                }
                let old = links::link_type_name(&m.to.name, &was.name);
                // A reference into the link type would name keys that end.
                let refers = |t: &&TypeDecl| {
                    let field = |f: &FieldDecl| f.ty.target.as_ref() == Some(&old);
                    t.fields.iter().any(field) || t.relations.iter().any(|r| r.target == old)
                };
                let types = w.ontology.types.values().map(|e| &e.decl);
                let mut referring = types.filter(|t| t.name != old && refers(t)).peekable();
                if referring.peek().is_some() {
                    // The type is named only to an author `describe` shows it to.
                    let sees = Sight::held(self.who, &w.ontology);
                    let named = referring.find(|t| sees.ty(&t.name).is_ok());
                    let ty = named.map(|t| t.name.clone());
                    let links = old.clone();
                    return Err(wrong(Case::RenameReferredLinks { ty, links }));
                }
                moved = stored(view, &old)?;
            }
            report.relations.push(RelationRename {
                from: was.name.clone(),
                to: is.name.clone(),
                carrier,
                links: moved,
            });
        }
        // What `declare` needs bound, on the type and its link types, at
        // the preflight as at the apply, once the renames it reads are
        // judged, against the links stored too (lakelands#1124).
        self.check_bound(&to, Some(&w.ontology), Some(m))?;
        for d in carried(&w.ontology, m) {
            let key = lakelands_core::engines::declaration_key(&d);
            let (kind, name) = key.split_once(' ').unwrap_or_default();
            let (kind, name) = (kind.to_owned(), name.to_owned());
            report.carried.push(Carried { kind, name });
        }
        report.rekeyed = plan.rekeyed.clone();
        report.references = plan.references.clone();
        report.sample_collisions = plan.collisions.clone();
        report.lossless &= plan.lossless;
        let listed = plan.listed();
        report.report_id = report_id(&report, view.position());
        // A record of another type whose reference to a moved key the fold
        // rewrites: decided for the sets whose filter reads that field, a
        // page of the type at a time.
        for (name, decl) in w.ontology.types.iter().map(|(n, e)| (n, &e.decl)) {
            let rewritten = crate::rekey::moved_references(decl, &plan.moves);
            // And for the sets whose `where` reads into the type across an
            // edge (055, extended).
            let into = BTreeSet::from([m.to.name.clone()]);
            let reads = |s: &ObjectSetDecl| {
                let sets = |n: &str| w.ontology.set(n);
                let at = [Place::ObjectSet {
                    name: s.name.clone(),
                }];
                let filter = declare::set_filter(&sets, s, &at);
                let reads = filter.map(|e| lakelands_core::derive::reads(&e));
                reads.is_ok_and(|r| rewritten.iter().any(|(f, _)| r.contains(*f)))
                    || across::reaches_into(&w.ontology, s, &into)
            };
            let other = name != &m.to.name && !links.iter().any(|(old, _)| &old.name == name);
            let Some(now) = now.filter(|_| other) else {
                continue;
            };
            let sets = sets_of(w, view, name, reads)?;
            if !sets.is_empty() {
                live(view, name, |r| {
                    left(
                        &plan.moves,
                        &sets,
                        decl,
                        r,
                        r.fields.clone(),
                        now,
                        &mut members,
                    )
                })?;
            }
        }
        Ok(MigrationPlan {
            report,
            others,
            rekeyed: listed,
            members,
            unlinked,
        })
    }

    /// `migrate_apply`. A migration to the type in force that renames
    /// nothing is answered at the head and appends nothing only where it
    /// fills no record (050), after every refusal, a stale report among
    /// them (027, extended).
    pub fn migrate_apply(&self, m: Migration, report_id: &str) -> Result<Outcome> {
        let against = Against {
            plan: None,
            report: Some(report_id),
        };
        let migration = Box::new(m);
        self.make(lakelands_api::write::Change::Migrate { migration }, against)
    }

    /// The drafts that apply a migration its preflight planned, refused
    /// with the report when it is not lossless: the `migrated` event first,
    /// then the set memberships it changes, each caused by the `migrated`,
    /// which is the `first` draft of its append not yet counted (an answer
    /// reads what directly follows its event), then the link types it
    /// changes or adds.
    pub(crate) fn migration_drafts(
        &self,
        w: &Writer,
        m: Migration,
        plan: MigrationPlan,
        now: Timestamp,
        first: usize,
    ) -> Result<Vec<Draft>> {
        let MigrationPlan {
            report,
            rekeyed,
            members,
            unlinked,
            ..
        } = plan;
        if !report.lossless {
            return Err(self.lossy(w, &m, report));
        }
        // Past the lake's ceiling, naming the set whose change passed it.
        let max = self.lake.limits.max_set_changes;
        if let Some((set, ..)) = members.get(max as usize) {
            return Err(Error::Refused {
                field: Some(set.as_str().into()),
                reason: Refusal::TooLarge { max },
            });
        }
        let report_id = report.report_id;
        let schema_hash = shape::schema_hash(&m.to).map_err(invalid)?;
        let version = w.ontology.next_version(&Declaration::Type(m.to.clone()));
        let migrated = position_after(w, first);
        let changes = members.into_iter().map(|c| self.changed(c, migrated, now));
        let mut drafts: Vec<Draft> = changes.collect();
        for (_, new) in changed_link_types(&w.ontology, &m, false) {
            let hash = shape::schema_hash(&new).map_err(invalid)?;
            let version = w.ontology.next_version(&Declaration::Type(new.clone()));
            drafts.push(Draft::system(
                now,
                EventBody::Migrated {
                    migration: Migration::to(new),
                    schema_hash: hash,
                    version,
                    report_id: report_id.clone(),
                    rekeyed: Vec::new(),
                },
            ));
        }
        // A relation new with this migration gets its link type declared,
        // and so does a renamed one whose new name's link type stands in
        // another shape: declared again, in the shape of the links it takes.
        let arriving = arriving(&m);
        let declared = link_type_drafts(&w.ontology, &m.to, now)?
            .into_iter()
            .filter(|d| {
                matches!(&d.body, EventBody::Declared { declaration: Declaration::Type(t), .. }
                    if !w.ontology.types.contains_key(&t.name) || arriving.contains(&t.name))
            });
        drafts.extend(declared);
        // What names a link type whose records move is declared again by
        // the lake under the new name, after that link type's declaration.
        for declaration in carried(&w.ontology, &m) {
            drafts.push(system_declared(&w.ontology, declaration, now)?);
        }
        let d = self.draft(
            now,
            Origin::Authored,
            EventBody::Migrated {
                migration: m,
                schema_hash,
                version,
                report_id,
                rekeyed,
            },
        );
        drafts.insert(0, d);
        // The live links of a relation the migration removes are retired
        // with it, each by its own event and with what a retirement sets
        // off, as a purge retires them (035): caused by the `migrated`.
        if !unlinked.is_empty() {
            let view = self.lake.engine.view(None)?;
            let mut d = self.decide(w, view.as_ref(), now, first + drafts.len());
            for before in unlinked {
                let body = retirement(&before.ty, &before.key, before.version);
                let retired = Draft {
                    cause: Some(migrated),
                    context: self.context,
                    ..Draft::system(now, body)
                };
                drafts.extend(d.retiring(&before, retired)?);
            }
        }
        Ok(drafts)
    }
}

/// A value type's report named by what it says to its reader and where
/// the lake stood (`head`), its own id left empty: an apply after the lake
/// moved is refused. Its uses, its misfits named and its `conditions` are
/// its reader's, so two readers shown otherwise are given two ids for one
/// lake, and what a reader is not shown moves no id; one that renames is
/// shown whole or not at all, and named whole.
pub(crate) fn value_report_id(
    report: &ValueTypeReport,
    to: &ValueTypeDecl,
    renames: &[(String, String)],
    head: u64,
) -> String {
    let mut said = serde_json::json!({
        "report": serde_json::to_value(report).expect("a report serialises"),
        "to": serde_json::to_value(to).expect("a declaration serialises"),
        "head": head,
    });
    if !renames.is_empty() {
        said["renames"] = serde_json::json!(renames);
    }
    lakelands_core::canonical::sha256_hex(&said)
}

/// The declarations in `ontology` whose expressions hold an old value of
/// `renames` as a literal, and the aging rules on a type that uses the
/// value type that move from or to one, in the report: those `who` is
/// shown by what they are, the others by that they are there. A literal
/// is listed wherever it stands, compared with a field of the value type
/// or not: they may name one. A rule is shown as a read of its record
/// would answer it, with its `on`, `from` and `to` (024).
pub(crate) fn conditions(
    ontology: &Ontology,
    view: &dyn ProjectionView,
    who: &Principal,
    report: &mut ValueTypeReport,
    renames: &BTreeMap<&str, &str>,
) -> Result<()> {
    use lakelands_api::expr::{Expr, Literal};
    let old = |e: &Expr| matches!(e, Expr::Lit { value: Literal::Text(t) } if renames.contains_key(t.as_str()));
    let declared = ontology.declared();
    let sees = Sight::held(who, &declared);
    let (mut shown, mut unshown) = (Vec::new(), false);
    for d in declared
        .iter()
        .filter(|d| lakelands_core::expr::declares(&d.declaration, &old))
    {
        match sees.shows_where(&d.declaration, &old) {
            true => shown.push(d.declaration.what()),
            false => unshown = true,
        }
    }
    let using = crate::lake::using_types(ontology, &report.value_type);
    let of_it = |r: &StoredRecord| {
        let text = |n: &str| r.fields.get(n).and_then(Value::as_str);
        let old = |n| text(n).is_some_and(|v| renames.contains_key(v));
        text("on").is_some_and(|t| using.contains(&t.into())) && (old("from") || old("to"))
    };
    let rule = TypeName::from(AGING_RULE_TYPE);
    let readable = ["on", "from", "to"]
        .iter()
        .all(|f| sees.reads(&rule, &(*f).into()));
    live(view, &rule, |r| {
        if of_it(r) {
            match readable && sees.answers(&rule, r) {
                true => shown.push(format!("the aging rule `{}`", r.key)),
                false => unshown = true,
            }
        }
        Ok(())
    })?;
    report.conditions = shown;
    report.conditions_unshown = unshown;
    Ok(())
}

/// What a value type's migration does, as its preflight plans it: the
/// report and, for one that renames values and is applied, the types
/// whose fields' `initial` it renames and the set memberships it changes.
pub(crate) struct ValueTypePlan {
    pub(crate) report: ValueTypeReport,
    initials: Vec<TypeDecl>,
    members: Vec<Change>,
}

impl Call<'_> {
    /// The preflight of a value type's migration: every value stored under
    /// it, in every type and field that uses it, retired and restricted
    /// records included, renamed as the migration renames it and checked as
    /// a write would check it. It asks for no grant: its door asks for
    /// `schema`, a proposal for none. Given `now`, the time of an apply, it
    /// decides each record's set memberships as the step leaves the record,
    /// with what reads a renamed value computed again and the new
    /// declaration in force.
    pub(crate) fn preflight_value_type(
        &self,
        w: &Writer,
        view: &dyn ProjectionView,
        m: &ValueTypeMigration,
        now: Option<Timestamp>,
    ) -> Result<ValueTypePlan> {
        let to = &m.to;
        let current = w
            .ontology
            .value_types
            .get(to.name.as_str())
            .ok_or_else(|| {
                let value_type = to.name.clone();
                invalid(Case::NoValueType { value_type })
            })?;
        match declare::value_type_change(&current.decl, to) {
            declare::ValueTypeChange::Same if m.renames.is_empty() => {
                let value_type = to.name.clone();
                return Err(invalid(Case::ValueTypeSame { value_type }));
            }
            declare::ValueTypeChange::Reshaping => {
                let value_type = to.name.clone();
                return Err(invalid(Case::ValueTypeReshapes { value_type }));
            }
            _ => {}
        }
        m.check_renames(&current.decl, accepts).map_err(invalid)?;
        // The new declaration is checked as a declaration of its own; the
        // one in force would refuse it as a tightening.
        let mut probe = w.ontology.clone();
        probe.value_types.remove(to.name.as_str());
        declare::check_declaration(&probe, &Declaration::ValueType(to.clone()), false)?;
        probe.put(
            Declaration::ValueType(to.clone()),
            current.version + 1,
            None,
        );
        let renames: BTreeMap<&str, &str> = (m.renames.iter())
            .map(|(old, new)| (old.as_str(), new.as_str()))
            .collect();
        let renamed = |v: &Value| v.as_str().and_then(|v| renames.get(v).copied());
        let mut report = ValueTypeReport {
            value_type: to.name.clone(),
            from_version: current.version,
            records_scanned: 0,
            fields: Vec::new(),
            misfits: 0,
            first: Vec::new(),
            fits: true,
            report_id: String::new(),
            conditions: Vec::new(),
            conditions_unshown: false,
            breaches: Vec::new(),
        };
        let mut plan = ValueTypePlan {
            report: ValueTypeReport::default(),
            initials: Vec::new(),
            members: Vec::new(),
        };
        let limit = self.lake.limits.max_set_changes as usize;
        // What computing a record again reads, at the event the apply would
        // append: no call is made, an answer recorded is read.
        let event = provisional(
            self.lake,
            position_after(w, 0),
            &Draft::system(
                now.unwrap_or(Timestamp(0)),
                EventBody::Checkpoint {
                    through: Position::GENESIS,
                    hash: Digest::zero(),
                    segments: Vec::new(),
                },
            ),
        );
        // A record of a type that uses the value type as the step leaves
        // it, as what a set's path reads across an edge (055, extended).
        let using: BTreeSet<TypeName> = crate::lake::using_types(&probe, &to.name)
            .into_iter()
            .collect();
        let renamed_record = |r: StoredRecord| {
            let Some(decl) = probe.ty(&r.ty).ok().filter(|_| using.contains(&r.ty)) else {
                return Ok(r);
            };
            let reached = crate::lake::renamed_fields(&probe, decl, &to.name);
            let mut is = r.clone();
            for f in decl.fields.iter().filter(|f| reached.contains(&f.name)) {
                if let Some(new) = r.fields.get(&f.name).and_then(renamed) {
                    is.fields.insert(f.name.clone(), Value::from(new));
                }
            }
            let computing = probe.computing(std::slice::from_ref(&r.ty));
            let mut outputs = Outputs::planning(view, &w.plans, &self.lake.keys);
            let again = record::rederive(&computing, &is, &event, &w.settings, &mut outputs)?;
            Ok::<_, Error>(again.unwrap_or(is))
        };
        let far = |t: &TypeName, k: &RecordKey| view.get(t, k)?.map(renamed_record).transpose();
        let link = |t: &TypeName, r: &RelationName, k: &RecordKey| link_from(view, t, r, k);
        let heads = Heads {
            far: &far,
            link: &link,
            max: self.lake.limits.max_depth,
        };
        for name in crate::lake::using_types(&probe, &to.name) {
            let decl = probe.ty(&name)?;
            let uses: Vec<&FieldDecl> = (decl.fields.iter())
                .filter(|f| f.ty.value_type.as_ref() == Some(&to.name))
                .collect();
            let reached = crate::lake::renamed_fields(&probe, decl, &to.name);
            let reaches = |f: &FieldDecl| reached.contains(&f.name);
            // A value the lake computes from an expression that names an
            // old value would be the old value again at its next computation.
            for f in uses.iter().filter(|f| reaches(f)) {
                let function = f.function.as_deref().and_then(|n| probe.functions.get(n));
                let expr = f
                    .derived
                    .as_ref()
                    .or(function.and_then(|e| e.decl.expr.as_ref()));
                if expr.is_some_and(|c| {
                    lakelands_core::expr::names_text(c, &|t| renames.contains_key(t))
                }) {
                    let (ty, field) = (name.clone(), f.name.clone());
                    return Err(invalid(Case::ComputedHoldsRenamed { ty, field }));
                }
            }
            // A field's `initial` that names an old value takes the new one:
            // the type is declared again by the lake after the event.
            let mut again = decl.clone();
            for f in again
                .fields
                .iter_mut()
                .filter(|f| reached.contains(&f.name))
            {
                if let Some(new) = f.initial.as_ref().and_then(renamed) {
                    f.initial = Some(Value::from(new));
                }
            }
            // An `initial` value no door would take is never filled (050):
            // it is a misfit of its field, told as a stored one is, by its
            // count alone to one who does not read the field.
            let room = 10 - report.first.len();
            let unfit = |f: &FieldDecl, v: &Value| {
                let patch: Patch = [(f.name.clone(), v.clone())].into();
                check_values(&probe, decl, &patch, &decl.fields).is_err()
            };
            let tally = |f: &&FieldDecl| {
                let mapped = again.fields.iter().find(|g| g.name == f.name);
                let initial = mapped.and_then(|g| g.initial.as_ref());
                ValueTypeUse {
                    ty: name.clone(),
                    field: f.name.clone(),
                    values_scanned: 0,
                    misfits: u64::from(initial.is_some_and(|v| unfit(f, v))),
                    renamed: (m.renames.iter())
                        .map(|(from, to)| ValueRenamed {
                            from: from.clone(),
                            to: to.clone(),
                            records: 0,
                        })
                        .collect(),
                }
            };
            if again != *decl {
                plan.initials.push(again.clone());
            }
            let mut used: Vec<(ValueTypeUse, Vec<Misfit>)> =
                uses.iter().map(|f| (tally(f), Vec::new())).collect();
            let sets = match now {
                Some(_) if !renames.is_empty() => sets_of(w, view, &name, |_| true)?,
                _ => Vec::new(),
            };
            let computing = probe.computing(std::slice::from_ref(&name));
            let sees = Sight::held(self.who, &w.ontology);
            // And its records as the renames leave them, computed again, hold
            // its constraints (lakelands#1119): what moves is each field of
            // the value type the renames reach.
            let moving = match renames.is_empty() {
                true => BTreeSet::new(),
                false => reached.iter().cloned().collect(),
            };
            let mut walked = Walked::of(&probe, decl, &moving, (&sees, false))?;
            let shown = |r: &StoredRecord| match sees.withheld(r) {
                true => Ok(false),
                false => crate::reads::answered(view, &sees, &name, &r.key),
            };
            crate::lake::walk(view, &name, true, |r| {
                report.records_scanned += 1;
                let mut is = r.clone();
                for (f, (used, first)) in uses.iter().zip(&mut used) {
                    let Some(v) = r.fields.get(&f.name).filter(|v| !v.is_null()) else {
                        continue;
                    };
                    used.values_scanned += 1;
                    let misfit = match (renamed(v), reaches(f)) {
                        (Some(new), true) => {
                            if decl.key.contains(&f.name) {
                                let (ty, field) = (name.clone(), f.name.clone());
                                return Err(invalid(Case::KeyHoldsRenamed { ty, field }));
                            }
                            let pair = used.renamed.iter_mut().find(|p| p.from == *v);
                            pair.into_iter().for_each(|p| p.records += 1);
                            let new = Value::from(new);
                            is.fields.insert(f.name.clone(), new.clone());
                            unfit(f, &new)
                        }
                        // An answer computed outside the lake is the
                        // backing's, which the lake does not rename: an old
                        // one would be read back against the new value type.
                        (Some(_), false) => true,
                        (None, _) => unfit(f, v),
                    };
                    if misfit {
                        used.misfits += 1;
                        // Named as a sample key is: of a record its reader
                        // is answered, one held as it stands asked no read,
                        // and in a field it reads, or it takes no place.
                        let shown = || crate::reads::answered(view, &sees, &name, &r.key);
                        let read = sees.reads(&name, &f.name) && !sees.withheld(&r);
                        if first.len() < room && read && shown()? {
                            first.push(Misfit {
                                ty: name.clone(),
                                key: r.key.clone(),
                                field: f.name.clone(),
                            });
                        }
                    }
                }
                if walked.judges(true) {
                    let mut outputs = Outputs::planning(view, &w.plans, &self.lake.keys);
                    let again =
                        record::rederive(&computing, &is, &event, &w.settings, &mut outputs)?;
                    let at = (w, now.unwrap_or(w.last_known_at));
                    walked.judge((&r, again.as_ref().unwrap_or(&is)), at, &shown)?;
                }
                let Some(now) = now.filter(|_| !sets.is_empty()) else {
                    return Ok(());
                };
                let mut outputs = Outputs::planning(view, &w.plans, &self.lake.keys);
                let is = record::rederive(&computing, &is, &event, &w.settings, &mut outputs)?
                    .unwrap_or(is);
                for (s, held) in &sets {
                    let member = held.contains(&r.key);
                    if plan.members.len() <= limit
                        && member != member_of((s, &probe), &is, w, now, heads)?
                    {
                        plan.members
                            .push((s.name.clone(), is.ty.clone(), is.key.clone(), !member));
                    }
                }
                Ok(())
            })?;
            report.breaches.extend(walked.kept(false)?.0);
            for (used, first) in used {
                let room = 10 - report.first.len();
                report.first.extend(first.into_iter().take(room));
                report.misfits += used.misfits;
                report.fields.push(used);
            }
        }
        // The sets of other types whose `where` reads a renamed value across
        // an edge are decided with them.
        let others = probe.types.keys().filter(|t| !using.contains(*t));
        for name in others.filter(|_| now.is_some() && !renames.is_empty()) {
            let sets = sets_of(w, view, name, |s| across::reaches_into(&probe, s, &using))?;
            if sets.is_empty() {
                continue;
            }
            let now = now.unwrap_or(Timestamp(0));
            live(view, name, |r| {
                for (s, held) in &sets {
                    let member = held.contains(&r.key);
                    if plan.members.len() <= limit
                        && member != member_of((s, &probe), r, w, now, heads)?
                    {
                        plan.members
                            .push((s.name.clone(), r.ty.clone(), r.key.clone(), !member));
                    }
                }
                Ok(())
            })?;
        }
        report.fits = report.misfits == 0;
        if !renames.is_empty() {
            conditions(&w.ontology, view, self.who, &mut report, &renames)?;
        }
        let head = view.position().map_or(0, |p| p.0);
        // Named as its reader is shown it; one that renames is never trimmed.
        let shown = self.shown_value_type(&w.ontology, report.clone());
        let said = if renames.is_empty() { &shown } else { &report };
        report.report_id = value_report_id(said, to, &m.renames, head);
        plan.report = report;
        Ok(plan)
    }

    /// Put a value type's migration in force against the report the caller
    /// was shown: without renames, one `declared` at the next version, no
    /// record changed; with them, the drafts of [`Self::value_type_drafts`].
    pub fn migrate_value_type_apply(
        &self,
        m: ValueTypeMigration,
        report_id: &str,
    ) -> Result<Outcome> {
        let against = Against {
            plan: None,
            report: Some(report_id),
        };
        let migration = Box::new(m);
        let change = lakelands_api::write::Change::MigrateValueType { migration };
        self.make(change, against)
    }

    /// A migration that renames values writes every field that uses the
    /// value type: refused, naming nothing, to a caller not shown one of
    /// them.
    pub(crate) fn renames_shown(
        &self,
        o: &Ontology,
        m: &ValueTypeMigration,
        report: &ValueTypeReport,
    ) -> Result<()> {
        if m.renames.is_empty() {
            return Ok(());
        }
        let shown = self.shown_value_type(o, report.clone());
        granted(shown.fields.len() == report.fields.len())
    }

    /// The drafts that apply a value type's migration its preflight
    /// planned. Without renames, one `declared`. With them: the
    /// `value_type_migrated`, the `first` draft of its append not yet
    /// counted; then, as the system, a `declared` of each type whose field's
    /// `initial` it renames, one set (`with_the_set`), so that the value type
    /// in force accepts the new value (the fold never reads `initial`, so a
    /// page that ends between them is harmless); then the set memberships it
    /// changes, each caused by the `value_type_migrated`, refused
    /// `too_large` past the lake's `max_set_changes`, naming the set.
    pub(crate) fn value_type_drafts(
        &self,
        w: &Writer,
        m: ValueTypeMigration,
        plan: ValueTypePlan,
        now: Timestamp,
        first: usize,
    ) -> Result<Vec<Draft>> {
        if m.renames.is_empty() {
            let declared =
                self.bundle_drafts(w, vec![Declaration::ValueType(m.to)], (now, first, false));
            return declared.map(|b| b.drafts);
        }
        let max = self.lake.limits.max_set_changes;
        if let Some((set, ..)) = plan.members.get(max as usize) {
            return Err(Error::Refused {
                field: Some(set.as_str().into()),
                reason: Refusal::TooLarge { max },
            });
        }
        let version = w
            .ontology
            .next_version(&Declaration::ValueType(m.to.clone()));
        let types = crate::lake::using_types(&w.ontology, &m.to.name);
        let body = EventBody::ValueTypeMigrated {
            declaration: m.to,
            version,
            renames: m.renames,
            report_id: plan.report.report_id,
            types,
        };
        let mut drafts = vec![self.draft(now, Origin::Authored, body)];
        let initials = plan.initials.len();
        for (i, t) in plan.initials.into_iter().enumerate() {
            let mut d = system_declared(&w.ontology, Declaration::Type(t), now)?;
            if let EventBody::Declared { with_the_set, .. } = &mut d.body {
                *with_the_set = i + 1 < initials;
            }
            drafts.push(d);
        }
        let migrated = position_after(w, first);
        drafts.extend(
            plan.members
                .into_iter()
                .map(|c| self.changed(c, migrated, now)),
        );
        Ok(drafts)
    }
}

/// Whether a value type's declaration accepts a text value: its kind, its
/// `one_of`, its `format` and its `constraint`.
fn accepts(vt: &ValueTypeDecl, v: &str) -> bool {
    let ty = lakelands_api::ontology::FieldType {
        kind: vt.kind,
        value_type: Some(vt.name.clone()),
        ..Default::default()
    };
    value::check(&Value::from(v), &ty, Some(vt)).is_ok_and(|v| satisfies(vt, &v).is_ok_and(|ok| ok))
}

/// A value type's preflight report, refused while a stored value would not
/// fit: with how many, and, for a caller with the `schema` grant
/// (`named`), the first of them. A proposal is told the count alone, so no
/// record is named in a field its proposer may not know of.
fn fits(report: ValueTypeReport, named: bool) -> Result<ValueTypeReport> {
    if report.fits {
        return Ok(report);
    }
    let first = (report.first.iter().filter(|_| named)).map(|m| RecordName::FieldOf {
        ty: m.ty.clone(),
        key: m.key.clone(),
        field: m.field.clone(),
    });
    let (value_type, misfits, first) = (report.value_type, report.misfits, first.collect());
    Err(invalid(Case::ValueTypeNotTightened {
        value_type,
        misfits,
        first,
    }))
}

/// Each `initial` of `t` in the one form a write of its field stores, so
/// that two spellings of one value are one declaration (050). A value the
/// check refuses is left as sent, for the check to refuse.
fn initials(o: &Ontology, t: &mut TypeDecl) {
    for f in &mut t.fields {
        let vt =
            f.ty.value_type
                .as_ref()
                .and_then(|n| o.value_types.get(n.as_str()));
        if let Some(v) = &mut f.initial
            && let Ok(one) = value::check(v, &f.ty, vt.map(|e| &e.decl))
        {
            *v = one;
        }
    }
}

/// Reads `to`, a migration's or a redeclaration's shape of the type
/// declared as `before`, as its author's view of the type (027 and 006,
/// extended): a field `who` may not know of, or a relation it is not
/// shown, was not its to leave out, so it is put back as declared, at its
/// declared place, and so are such link fields of each relation `to`
/// keeps or renames that it is shown. No refusal names one: a rename from
/// it is judged against its author's view, where it is no field or
/// relation ([`Call::preflight_plan`]), and a field it reads is refused
/// on that field. `sees` is its author's sight.
fn as_author(
    sees: &Sight<'_>,
    before: &TypeDecl,
    to: &mut TypeDecl,
    relations: &[(RelationName, RelationName)],
) -> Result<()> {
    let who = sees.who();
    fn put_back<T: Clone>(declared: &[T], hidden: impl Fn(&T) -> bool, into: &mut Vec<T>) {
        for (i, x) in declared.iter().enumerate().filter(|(_, x)| hidden(x)) {
            into.insert(i.min(into.len()), x.clone());
        }
    }
    let hidden = |t: &TypeDecl, f: &FieldDecl| rights::field_access(who, t, f) == Access::None;
    let unshown = |r: &RelationDecl| !sees.relation(before, r);
    let unknown: Vec<&FieldDecl> = (before.fields.iter().filter(|f| hidden(before, f))).collect();
    put_back(&before.fields, |f| hidden(before, f), &mut to.fields);
    // A constraint over a field it does not read is not told to it, as
    // `describe_type` tells none, so left out it stands as declared
    // (lakelands#1119).
    let read = |c: &ConstraintDecl| -> Vec<FieldName> {
        let e = c.holds.typed().ok();
        e.map_or_else(Vec::new, |e| e.fields_read().into_iter().cloned().collect())
    };
    let untold = |c: &ConstraintDecl| !read(c).iter().all(|n| sees.reads_from(&before.name, n));
    put_back(&before.constraints, untold, &mut to.constraints);
    // A valid time over a field it may not know of is not named to it, so
    // left out it stands as declared (024 and 027, extended).
    let unseen = |v: &Option<FieldName>| {
        v.as_ref()
            .is_some_and(|v| unknown.iter().any(|f| &f.name == v))
    };
    // An end sent with no start is judged on the type as it is shown:
    // one alone there keeps one; else it is refused as on a type with no
    // valid time, before a hidden start is put back, which would tell
    // that one stands (028, lakelands#909).
    let alone = (before.valid_from.is_none() || unseen(&before.valid_from))
        && before.valid_to.is_some()
        && !unseen(&before.valid_to);
    if let (None, Some(field), false) = (&to.valid_from, &to.valid_to, alone) {
        let (name, field) = (to.name.clone(), field.clone());
        let said = Said::from(Case::ValidToAlone {
            field: field.clone(),
        });
        return Err(invalid_field(&field, said.within(Place::Type { name })));
    }
    for (axis, sent, held) in [
        ("valid_from", &mut to.valid_from, &before.valid_from),
        ("valid_to", &mut to.valid_to, &before.valid_to),
    ] {
        if sent.is_none() && unseen(held) {
            sent.clone_from(held);
        } else if *sent != *held && unseen(held) && !unseen(sent) {
            // A field its author may name, sent where a bound over a field
            // it may not know of stands: the bound is taken, as a name a
            // hidden field holds is (027), and stands (lakelands#692). A
            // name it may not give was refused before, as on any type.
            // A hidden field's name sent there is the author's own field
            // named like it, which the declaration refuses as declared
            // twice, as on any type.
            let (name, axis) = (to.name.clone(), axis.to_owned());
            let said = Said::from(Case::ValidTimeStands { axis });
            return Err(invalid(said.within(Place::Type { name })));
        }
    }
    for r in &mut to.relations {
        // A renamed relation is the one it was, under its old name.
        let was = relations.iter().find(|(_, new)| new == &r.name);
        let name = was.map_or(&r.name, |(old, _)| old);
        // To its author a relation it is not shown is none to rename.
        let shown = |o: &&RelationDecl| &o.name == name && !unshown(o);
        if let Some(old) = before.relations.iter().find(shown) {
            let link = links::link_type(&before.name, old);
            // A link type its author may not know of is a type it is not
            // shown: to give it a field is not granted, naming nothing, as
            // to declare it is (lakelands#965).
            granted(r.fields.is_empty() || sees.ty(&link.name).is_ok())?;
            put_back(&old.fields, |f| hidden(&link, f), &mut r.fields);
        }
    }
    put_back(&before.relations, unshown, &mut to.relations);
    Ok(())
}

/// A field a part its author may not know of reads stands, to that
/// author, as it is, judged on what it sent before anything else is: the
/// parts are an embedding, a derived or computed field, a relation it is
/// not shown, a constraint it is not told and a link field, and each
/// field of theirs it may know of (lakelands#1124).
fn reads_stand(
    sees: &Sight<'_>,
    before: &TypeDecl,
    to: &TypeDecl,
    m: Option<&Migration>,
) -> Result<()> {
    let (renames, relations, fill) = m.map_or((&[][..], &[][..], &[][..]), |m| {
        (&m.renames[..], &m.relation_renames[..], &m.fill[..])
    });
    let who = sees.who();
    let hidden = |t: &TypeDecl, f: &FieldDecl| rights::field_access(who, t, f) == Access::None;
    let known = |t: &TypeDecl, fields: &[FieldDecl], n: &FieldName| {
        fields.iter().any(|f| &f.name == n && !hidden(t, f))
    };
    let unshown = |r: &RelationDecl| !sees.relation(before, r);
    let read = |c: &ConstraintDecl| -> Vec<FieldName> {
        let e = c.holds.typed().ok();
        e.map_or_else(Vec::new, |e| e.fields_read().into_iter().cloned().collect())
    };
    let untold = |c: &ConstraintDecl| !read(c).iter().all(|n| sees.reads_from(&before.name, n));
    let constrained: Vec<FieldName> = (before.constraints.iter().filter(|c| untold(c)))
        .flat_map(read)
        .collect();
    // What a relation it is not shown reads: its `via`, its match's own side.
    let hid = before.relations.iter().filter(|r| unshown(r));
    let owner_reads: Vec<FieldName> = hid
        .clone()
        .flat_map(|r| r.derived.iter().flat_map(|d| d.reads()[0].clone()))
        .collect();
    let reads = (before.fields.iter().filter(|f| hidden(before, f)))
        .flat_map(inputs)
        .chain(hid.flat_map(|r| r.via.iter()))
        .chain(&owner_reads)
        .chain(&constrained)
        .filter(|n| known(before, &before.fields, n));
    stands(&to.name, reads, &before.fields, &to.fields, renames, fill)?;
    for r in &to.relations {
        // A renamed relation is the one it was, under its old name.
        let was = relations.iter().find(|(_, new)| new == &r.name);
        let name = was.map_or(&r.name, |(old, _)| old);
        let shown = |o: &&RelationDecl| &o.name == name && !unshown(o);
        let Some(old) = before.relations.iter().find(shown) else {
            continue;
        };
        let link = links::link_type(&before.name, old);
        let reads = (old.fields.iter().filter(|f| hidden(&link, f)))
            .flat_map(inputs)
            .filter(|n| known(&link, &old.fields, n));
        stands(&link.name, reads, &old.fields, &r.fields, &[], &[])?;
    }
    Ok(())
}

/// The fields a computed field reads: its `depends_on`, and what an
/// embedding is made from.
fn inputs(f: &FieldDecl) -> impl Iterator<Item = &FieldName> {
    (f.depends_on.iter()).chain(f.embed.iter().flat_map(|e| &e.from))
}

/// A field a part its author may not know of reads stands, to that
/// author, as it is: removed, renamed (its name freed, taken by another
/// or swapped), retyped, made sensitive, made computed or computed
/// otherwise, or filled, it is refused on that field
/// before anything judges the hidden part, which would name it
/// (lakelands#1124). The owner, or one shown the part, may.
fn stands<'a>(
    ty: &TypeName,
    reads: impl IntoIterator<Item = &'a FieldName>,
    before: &[FieldDecl],
    to: &[FieldDecl],
    renames: &[(FieldName, FieldName)],
    fill: &[FieldName],
) -> Result<()> {
    let moved = |n: &FieldName| renames.iter().any(|(old, new)| old == n || new == n);
    let computing = |f: &FieldDecl| (f.derived.clone(), f.function.clone(), f.embed.clone());
    let changed = |n: &FieldName| {
        let (was, is) = (
            before.iter().find(|f| &f.name == n),
            to.iter().find(|f| &f.name == n),
        );
        match (was, is) {
            // Made sensitive, or computed otherwise; taking `sensitive`
            // away is a change the report lists (027), and no part's check
            // minds it.
            (Some(w), Some(i)) => {
                let computed = computing(i) != (None, None, None);
                w.ty != i.ty
                    || (!w.sensitive && i.sensitive)
                    || (computed && (computing(w), &w.depends_on) != (computing(i), &i.depends_on))
            }
            (_, None) => true,
            (None, Some(_)) => false,
        }
    };
    match reads
        .into_iter()
        .find(|n| moved(n) || fill.contains(n) || changed(n))
    {
        Some(n) => {
            let (field, ty) = (n.clone(), ty.clone());
            Err(invalid_field(n, Case::FieldReadElsewhere { field, ty }))
        }
        None => Ok(()),
    }
}

/// A migration as its author sent it, and the type as its author sees it
/// (`rights::known_fields`): the fields it may know of and the relations
/// it is shown, with their link fields it may know of.
pub(crate) struct Authored {
    sent: Migration,
    known: TypeDecl,
}

fn risk_word(r: Risk) -> &'static str {
    match r {
        Risk::Low => "low",
        Risk::Medium => "medium",
        Risk::High => "high",
    }
}

/// The system declarations of the link types `t`'s relations call for
/// that differ from those in force: a new relation's, or one whose fields
/// changed. Appended with `t`'s declaration, in the same append
/// (`docs/log-format.md` §2.3).
fn link_type_drafts(o: &Ontology, t: &TypeDecl, now: Timestamp) -> Result<Vec<Draft>> {
    let mut out = Vec::new();
    for lt in links::link_types_of(t) {
        if o.types.get(&lt.name).map(|e| &e.decl) == Some(&lt) {
            continue;
        }
        out.push(system_declared(o, Declaration::Type(lt), now)?);
    }
    Ok(out)
}

/// Refuse `to`, a type declared again or migrated from `was`, where a verb
/// on it or on a link type its relations carry, before or after, or a
/// verb, a set or a trigger reading across into one of them, could not be
/// declared over `after` as `to` leaves it, its link types with it: a
/// path read across, a relation stepped through, a kind its condition
/// computes with (028, 055). A declaration `shows` names is named;
/// another is told one could not be declared. `o` is the lake before,
/// where they read.
fn verbs_judged_again(
    (o, after): (&Ontology, &Ontology),
    was: &TypeDecl,
    to: &TypeDecl,
    shows: &dyn Fn(&Declaration) -> bool,
) -> std::result::Result<(), Said> {
    let judges = judged_by(o, was, to);
    let judged = (after.verbs.values()).filter(|e| judges(&e.decl));
    let mut judged: Vec<_> = judged.map(|v| Declaration::Verb(v.decl.clone())).collect();
    judged.extend(across_judged(o, was, to));
    if judged.is_empty() {
        return Ok(());
    }
    let mut linked = after.clone();
    put_as_appended(&mut linked, &Declaration::Type(to.clone()), Some);
    for verb in judged {
        if let Err(e) = declare::reads_its_type(&linked, &verb) {
            let said = e.case().filter(|_| shows(&verb)).cloned();
            let ty = to.name.clone();
            return Err(said.unwrap_or(Case::NoLongerDeclarable { ty }.into()));
        }
    }
    Ok(())
}

/// The sets and triggers in `o` whose condition reads across an edge into
/// a type `was` becomes `to`, or into a link type its relations carry,
/// before or after: [`verbs_judged_again`] judges them as it judges a verb
/// that reads across into it (055, extended).
fn across_judged(o: &Ontology, was: &TypeDecl, to: &TypeDecl) -> Vec<Declaration> {
    let mut types = BTreeSet::from([to.name.clone()]);
    for t in [was, to] {
        types.extend(links::link_types_of(t).into_iter().map(|l| l.name));
    }
    let sets = o.object_sets.values().map(|e| &e.decl);
    let sets = sets.filter(|s| across::reaches_into(o, s, &types));
    let triggers = o.triggers.values().map(|e| &e.decl);
    let triggers = triggers.filter(|t| across::when_reaches_into(o, t, &types));
    (sets.map(|s| Declaration::ObjectSet(s.clone())))
        .chain(triggers.map(|t| Declaration::Trigger(t.clone())))
        .collect()
}

/// Whether [`verbs_judged_again`] judges a verb where `was` becomes `to`:
/// one on it or on a link type its relations carry, before or after, or
/// one reading across into one of them, as `o`, the lake before, reads it.
fn judged_by<'o>(
    o: &'o Ontology,
    was: &TypeDecl,
    to: &TypeDecl,
) -> impl Fn(&VerbDecl) -> bool + 'o {
    let mut moved = vec![to.name.clone()];
    for t in [was, to] {
        moved.extend(links::link_types_of(t).into_iter().map(|l| l.name));
    }
    move |v| {
        let far = o.ty(&v.on).ok();
        let far = far.and_then(|on| v.reads_across(on, &|n| o.ty(n).ok()));
        let into = far.is_some_and(|far| far.iter().any(|(t, _)| moved.contains(&t.name)));
        moved.contains(&v.on) || into
    }
}

/// Puts `d` into `o` as an append leaves it: a type with the link types
/// its relations call for, each as `brings` admits it (035).
pub(crate) fn put_as_appended(
    o: &mut Ontology,
    d: &Declaration,
    brings: impl Fn(TypeDecl) -> Option<TypeDecl>,
) {
    let with = match d {
        Declaration::Type(t) => links::link_types_of(t),
        _ => Vec::new(),
    };
    let with = with.into_iter().filter_map(brings).map(Declaration::Type);
    with.chain([d.clone()]).for_each(|d| o.put(d, 0, None));
}

/// The system's `declared` of a declaration, at its next version.
fn system_declared(o: &Ontology, declaration: Declaration, now: Timestamp) -> Result<Draft> {
    let schema_hash = match &declaration {
        Declaration::Type(t) => Some(shape::schema_hash(t).map_err(invalid)?),
        _ => None,
    };
    let version = o.next_version(&declaration);
    let body = EventBody::Declared {
        declaration,
        schema_hash,
        version,
        with_the_set: false,
    };
    Ok(Draft::system(now, body))
}

/// The link types in force whose derivation from the migration's `to`
/// differs: each with the declaration in force and the one `to` calls for.
/// One a renamed relation moves its links to is among them only where
/// `standing` asks: it stands from an earlier relation of that name and
/// holds no record, so it is declared again in the renamed relation's
/// shape, never migrated from its own ([`arriving`]; 035, extended).
fn changed_link_types(o: &Ontology, m: &Migration, standing: bool) -> Vec<(TypeDecl, TypeDecl)> {
    let arriving = arriving(m);
    links::link_types_of(&m.to)
        .into_iter()
        .filter_map(|new| {
            let old = &o.types.get(&new.name)?.decl;
            let asked = standing || !arriving.contains(&new.name);
            (old != &new && asked).then(|| (old.clone(), new))
        })
        .collect()
}

/// The link types a migration's renamed relations move their links to.
fn arriving(m: &Migration) -> BTreeSet<TypeName> {
    links::moved(m).into_values().collect()
}

/// Refuse a link type's records at a door other than the link doors.
fn ordinary_door(ty: &TypeName) -> Result<()> {
    if links::is_link_type(ty) {
        return Err(refused(Refusal::WrongDoor));
    }
    Ok(())
}

/// The answer for a relation `ty` does not carry by link records, which is
/// also the answer for one the caller is not shown.
fn no_link_relation(ty: &TypeName, relation: &RelationName) -> Error {
    let (ty, relation) = (ty.clone(), relation.clone());
    invalid(Case::NoLinkRelation { ty, relation })
}

/// A record as a write reads it, the head's or its append's.
pub type Get<'a> = &'a dyn Fn(&TypeName, &RecordKey) -> Result<Option<StoredRecord>>;

/// An ingest window's earlier links, `(link type, from, to)` each, and how
/// it reads a record.
pub type Window<'a> = (&'a [(TypeName, RecordKey, RecordKey)], Get<'a>);

/// The relation's cardinality over live links, with the link from `from`
/// to `to` among them: `many_to_one` and `one_to_one` allow one live link
/// per `from`, `one_to_many` and `one_to_one` one per `to`. Answered from
/// the links the engine walks at the head, not by reading every link, then
/// from an ingest window's earlier links, the records read as the window
/// plans them: one answer wherever the store cut the stream (016). The
/// other end of the link that holds is named only where a record holds
/// its key and the principal of `sees` is answered that link: by its ends,
/// as deep as they go, and by `only` on the link type itself (006, 051).
pub fn cardinality(
    (view, sees): (&dyn ProjectionView, &Sight),
    window: Option<Window>,
    (ty, r): (&TypeName, &RelationDecl),
    from: &RecordKey,
    to: &RecordKey,
) -> Result<()> {
    use lakelands_api::query::Walk;
    let (per_from, per_to) = one_per(r.cardinality);
    let (relation, cardinality) = (r.name.clone(), r.cardinality);
    let broken = |case: Case| constraint(Some(links::TO.into()), case);
    let lt = links::link_type_name(ty, &r.name);
    let head = |t: &TypeName, k: &RecordKey| view.get(t, k);
    let (earlier, get): (&[_], Get) = window.unwrap_or((&[], &head));
    let earlier = || {
        earlier
            .iter()
            .filter(|(t, f, o)| *t == lt && (f, o) != (from, to))
    };
    let walk = |end: &TypeName, key: &RecordKey, dir| -> Result<Vec<_>> {
        Ok(view
            .edges(end, std::slice::from_ref(key), Some(&r.name), dir)?
            .remove(0))
    };
    // `from`'s other link first, in the head and then the window, then
    // `to`'s: the one answered does not turn on where the store cut.
    let mut other = None;
    if per_from
        && let Some(e) = walk(ty, from, Walk::Out)?
            .into_iter()
            .find(|e| e.to.1 != *to)
    {
        other = Some((true, e.to.1));
    }
    if other.is_none() && per_from {
        other = earlier()
            .find(|(_, f, _)| f == from)
            .map(|(_, _, o)| (true, o.clone()));
    }
    if other.is_none() && per_to {
        let mut there = walk(&r.target, to, Walk::In)?.into_iter();
        other = (there.find(|e| e.from.0 == *ty && e.from.1 != *from)).map(|e| (false, e.from.1));
    }
    if other.is_none() && per_to {
        other = earlier()
            .find(|(_, _, o)| o == to)
            .map(|(_, f, _)| (false, f.clone()));
    }
    let Some((of_from, end)) = other else {
        return Ok(());
    };
    // A key no record holds names nothing either, nor does the end of a
    // link its writer is not answered.
    let link = match of_from {
        true => links::link_key(from, &end),
        false => links::link_key(&end, to),
    };
    let answered = || -> Result<bool> {
        let (Ok(decl), who) = (sees.ty(&lt), sees.who()) else {
            return Ok(false);
        };
        let Some(l) = get(&lt, &link)? else {
            return Ok(true);
        };
        let mut many = |t: &TypeName, ks: &[RecordKey]| -> Result<Vec<StoredRecord>> {
            ks.iter().filter_map(|k| get(t, k).transpose()).collect()
        };
        let restricted = who.grants.restricted && !rights::holds_only(who);
        Ok(restricted || sees.ends(decl, &[&l.fields], &mut many)?[0] && sees.answers(&lt, &l))
    };
    let end_ty = if of_from { &r.target } else { ty };
    let named = (get(end_ty, &end)?.is_some() && answered()?).then_some(end);
    Err(broken(match of_from {
        true => Case::LinkedAlreadyFrom {
            from: from.clone(),
            relation,
            to: named,
            cardinality,
        },
        false => Case::LinkedAlreadyTo {
            to: to.clone(),
            relation,
            from: named,
            cardinality,
        },
    }))
}

/// What made an external verb due: a call, under a key or with none, or
/// an acceptance.
#[derive(Clone, Copy)]
pub(crate) enum MadeDue {
    /// A call.
    Call {
        /// Whether it bore an idempotency key.
        keyed: bool,
    },
    /// A proposal's acceptance.
    Accepted,
}

impl MadeDue {
    /// How such a call is sent again.
    pub(crate) fn retry(self) -> Retry {
        match self {
            MadeDue::Call { keyed: false } => Retry::Call,
            MadeDue::Call { keyed: true } => Retry::OtherKey,
            MadeDue::Accepted => Retry::Propose,
        }
    }
}

/// The answer to a call of an external verb, or to its acceptance, that
/// failed where `not_run` holds: its append is in the log and not folded,
/// or what comes before its hand-out failed (the drain, the read of its
/// record). The store's sentence for the failure, and that the verb's
/// executor was not handed the call: sent again under its key, the call
/// runs nothing, and neither does a proposal accepted again, so the host
/// calls the verb again under another key or with none, or proposes the
/// call again. A failure that is not `unavailable` (the record retired
/// since the call) is said in the same words.
pub(crate) fn not_handed(e: Error, not_run: bool, by: MadeDue) -> Error {
    if !not_run {
        return e;
    }
    Error::unavailable(Situation::Executor {
        retry: by.retry(),
        cause: Box::new(e),
    })
}

/// A retry of a call of an external verb made due at `position`, or of
/// its acceptance when `accepted`, that finds no answer of its executor
/// recorded: it runs nothing, and says so in one sentence true whichever
/// way the call got there.
fn unanswered(position: Position, accepted: bool) -> Error {
    let case = Case::Unanswered { position, accepted };
    refused(Refusal::Unanswered(Box::new(case.into())))
}

/// An external verb that ran, to be handed to its executor; the store
/// records the executor's answer.
#[derive(Clone, Debug)]
pub struct External {
    /// The executor's name.
    pub executor: String,
    /// The verb.
    pub verb: VerbName,
    /// The record's type.
    pub ty: TypeName,
    /// The record's key; `None` for a verb called on its type.
    pub key: Option<RecordKey>,
    /// The payload.
    pub payload: Patch,
    /// The position of the event that made it due.
    pub position: Position,
}

/// Record an executor's answer, as `output_recorded`, after the answers
/// held before it ([`Lake::record_held`]). A refusal says whether the
/// answer is held: the next write records it.
pub fn record_output(
    lake: &Lake,
    ext: &External,
    input: &Value,
    output: Value,
) -> std::result::Result<Position, (Error, bool)> {
    let id = log::output_id(&lake.id, ext.position);
    let draft = Draft {
        cause: Some(ext.position),
        id: Some(id.clone()),
        ..Draft::system(
            Timestamp(i64::MIN),
            EventBody::OutputRecorded {
                seam: format!("verb:{}:{}", ext.executor, ext.verb),
                input: Digest(lakelands_core::canonical::sha256_hex(input)),
                output: Some(output),
                output_at: None,
                field: None,
            },
        )
    };
    // A writer that cannot catch up holds the answer for the next write.
    let mut w = match lake.for_write() {
        Ok(w) => w,
        Err(e) => {
            lake.writer().answers.push(draft);
            return Err((e, true));
        }
    };
    match lake.record_held(&mut w, Some(draft)) {
        Ok(events) => Ok(events[0].position),
        Err(e) => Err((e, w.answers.iter().any(|d| d.id.as_ref() == Some(&id)))),
    }
}

/// A host-side retention hold (`LakeStore::restrict` / `unrestrict`).
pub fn restrict(
    lake: &Lake,
    ty: &TypeName,
    key: &RecordKey,
    reason: Option<String>,
) -> Result<Position> {
    let mut w = lake.for_write()?;
    rights::not_a_view(w.ontology.ty(ty)?)?;
    let view = lake.engine.view(None)?;
    // A hold is placed and lifted on a retired record too: the host holds
    // what it keeps, live or not (006).
    let missing = || Error::NotFound {
        missing: Missing::Record {
            ty: ty.clone(),
            key: key.clone(),
        },
    };
    let before = view.get(ty, key)?.ok_or_else(missing)?;
    let version = Version(before.version.0 + 1);
    let restriction = |already| {
        let (ty, key) = (ty.clone(), key.clone());
        Err(invalid(Case::Restriction { ty, key, already }))
    };
    let body = match (&reason, before.restricted) {
        (Some(_), true) => return restriction(true),
        (None, false) => return restriction(false),
        (Some(r), false) => EventBody::RecordRestricted {
            ty: ty.clone(),
            key: key.clone(),
            version,
            reason: r.clone(),
        },
        (None, true) => EventBody::RecordUnrestricted {
            ty: ty.clone(),
            key: key.clone(),
            version,
        },
    };
    let now = lake.clock(&mut w);
    let d = Draft {
        writer: "system:host".into(),
        ..Draft::system(now, body)
    };
    drop(view);
    let events = lake.append(&mut w, vec![d])?;
    Ok(events[0].position)
}
