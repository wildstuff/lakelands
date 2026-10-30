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
    Case, Flaw, Named, Naming, Part, Place, Printed, RecordName, Retry, Said, Seam, Situation,
};
use lakelands_api::event::{ChangeKind, Digest, Event, EventBody, EventId, Procedure};
use lakelands_api::ontology::{
    Assignment, Authority, ConstraintDecl, EmbedDecl, FieldDecl, RelationDecl, Risk, TriggerDecl,
    TypeDecl, ValueTypeDecl, VerbDecl, VerbEffect,
};
use lakelands_api::principal::{OCCASION_MAX_BYTES, WriteContext};
use lakelands_api::record::{Origin, StoredRecord};
use lakelands_api::seam::VerbExecutor;
use lakelands_api::write::{
    Breach, Carried, Carrier, Declaration, DeclarationKind, FieldFill, InForce, Migration,
    MigrationReport, Misfit, ObjectSetDecl, Outcome, ProposalState, ProposedWrite, Recompute,
    RelationRename, SettingDecl, ValueRenamed, ValueTypeMigration, ValueTypeReport, ValueTypeUse,
    Withdrawal, WriteOpts,
};
use lakelands_api::{
    Access, ConstraintName, Error, FieldName, IdempotencyKey, Missing, Patch, Position,
    PreconditionName, Principal, ProposalId, RecordKey, Refusal, RelationName, Result, SourceId,
    Timestamp, TypeName, Value, VerbName, Version,
};
use lakelands_core::expr::{self, Place as Eval, RecordScope, Scope, WriteScope};
use lakelands_core::sight::{Door, Sight};
use lakelands_core::{describe, links, not_a_view, shape, value};

use crate::backed::{Outputs, is_backed};
use crate::declare::{self};
use crate::exec::Turn;
use crate::lake::{
    AGING_RULE_TYPE, Lake, SETTING_TYPE, Writer, computes, computes_again, computing_changed,
    every, fields_again, find_proposal,
};
use crate::log::{self, Draft};
use crate::ontology::Ontology;

mod across;
mod change;
mod declaration;
mod migrate;
mod plan;
mod propose;
mod record;
mod stands;
mod touched;
mod viewed;

use across::{Across, Heads, VerbScope, fires_on, link_from};
pub(crate) use across::{Written, reads_across};
use declaration::{
    Authored, arriving, changed_link_types, link_type_drafts, no_link_relation, ordinary_door,
    system_declared, verbs_judged_again,
};
pub(crate) use declaration::{put_as_appended, risk_word};
pub(crate) use migrate::{conditions, retarget_types, value_report_id};
use migrate::{fits, retarget_values, retargets, stored};
pub(crate) use plan::Against;
pub(crate) use plan::When;
pub(crate) use plan::shown_plan;
pub(crate) use record::Decide;
pub(crate) use record::Envelope;
pub use record::Write;
pub(crate) use record::{Job, write_many};
pub(crate) use stands::overruled;
pub(crate) use touched::{Folding, Zone};

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
        (On::Type, Some(_)) => Err(Error::invalid(Case::VerbTypeTakesNoKey { verb, ty })),
        (On::Record, None) => Err(Error::invalid(Case::VerbNeedsKey { verb, ty })),
        (On::Type, None) if expected.is_some() => {
            Err(Error::invalid(Case::VerbTypeNoVersion { verb, ty }))
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
    plan: std::sync::Arc<crate::record::Plan>,
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

/// How a change is judged.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Judging {
    /// As made: what it finds refuses it.
    Act,
    /// As a proposal's plan: only telling, on what its reader is told of;
    /// the accept is the one refused (062).
    Proposal,
}

impl Judging {
    /// Whether it only tells.
    pub(crate) fn tells(self) -> bool {
        self == Judging::Proposal
    }
}

impl Walked {
    /// For `decl` as `o` declares it, over the fields the walk may move.
    /// One who does not read every field a constraint that refuses and
    /// that it may move reads is not granted the walk, decided before any
    /// record is read, so the answer is the same whatever they hold.
    /// Judged as a proposal's plan: one it may not read is not judged and
    /// is not told, and nothing is refused (062).
    pub(crate) fn of(
        o: &crate::ontology::Ontology,
        decl: &TypeDecl,
        moving: &BTreeSet<FieldName>,
        (sight, judging): (&Sight<'_>, Judging),
    ) -> Result<Walked> {
        let plan = crate::record::Plans::default().get(o, decl);
        let reaches = |c: &crate::record::Constraint| !c.fed.is_disjoint(moving);
        let reads =
            |c: &crate::record::Constraint| c.reads.iter().all(|f| sight.reads_from(&decl.name, f));
        for c in (plan.constraints.iter()).filter(|c| reaches(c) && c.decl.at_ingest.refuses()) {
            granted(judging.tells() || reads(c))?;
        }
        let broken = (plan.constraints.iter())
            .map(|c| (reaches(c) && (!judging.tells() || reads(c))).then(|| (0, 0, Vec::new())))
            .collect();
        let held = sight.sees_restricted();
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
        let judged = |(c, b): (&crate::record::Constraint, &Option<_>)| {
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
    /// admits and is broken, for the walk's report. Judged as a proposal's
    /// plan, one that refuses is listed too, and the walk is told it is not lossless
    /// where its reader is told of a record that breaks it.
    pub(crate) fn kept(self, judging: Judging) -> Result<(Vec<Breach>, bool)> {
        let mut lossless = true;
        let mut breaches = Vec::new();
        let walked = self.plan.constraints.iter().zip(self.broken).zip(self.told);
        for ((c, b), told) in walked {
            let Some((records, all, sample_keys)) = b.filter(|b| b.1 > 0 && told) else {
                continue;
            };
            let (name, ty) = (c.decl.name.clone(), self.ty.clone());
            if c.decl.at_ingest.refuses() && !judging.tells() {
                let records = all;
                let case = Case::ConstraintLeftBroken {
                    constraint: name,
                    ty,
                    records,
                };
                return Err(Error::constraint(None, case));
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
                field: None,
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
        let held = sight.sees_restricted();
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

/// Refuse a write or retire of a setting its writer is not told whether it
/// exists, a source's or one naming fields of a type it does not read
/// whole (063), before anything of it is judged: the same in a lake that
/// holds it and in one that does not, a proposal's and an ingest's too.
pub(crate) fn untold_setting(sees: &Sight<'_>, decl: &TypeDecl, key: &RecordKey) -> Result<()> {
    granted(decl.name.as_str() != SETTING_TYPE || sees.setting_told(key.as_str()))
}

/// Refuse as not granted unless `ok`.
pub(crate) fn granted(ok: bool) -> Result<()> {
    if ok {
        return Ok(());
    }
    Err(Error::not_granted())
}

/// Refuse a write whose context is deeper than the lake's reactive
/// ceiling, whatever the origin names
/// (`docs/decisions/021-the-write-context-belongs-to-the-host.md`), or
/// whose `on_behalf_of` is not a principal the write may be made for:
/// `human`, `agent` or `service` with an id that is neither empty nor
/// holds a NUL, and not `who` itself, since one fact is recorded one way
/// (`docs/decisions/088-an-event-records-the-principal-a-write-was-made-for.md`),
/// or whose `occasion` is empty, longer than [`OCCASION_MAX_BYTES`] or
/// holds a control character: its shape is judged, never its meaning (088).
/// Every write door calls it first, before any other check and before it
/// takes the writer, so every door refuses it alike. It reads nothing. A
/// host's `cause` is checked where the write appends, under the writer:
/// that it is at or below the head, then the ceiling of the trigger whose
/// firing it names ([`crate::lake::Lake::append_after`]).
pub fn check_context(lake: &Lake, context: &WriteContext, who: &Principal) -> Result<()> {
    let max = lake.limits.max_reactive_depth;
    if context.depth > max {
        return Err(Error::refused(None, Refusal::TooDeep { max }));
    }
    if let Some(o) = &context.occasion
        && (o.is_empty() || o.len() > OCCASION_MAX_BYTES || o.chars().any(char::is_control))
    {
        let case = Case::OccasionForm {
            max: OCCASION_MAX_BYTES,
        };
        return Err(Error::invalid_at(&"occasion".into(), case));
    }
    let Some(value) = &context.on_behalf_of else {
        return Ok(());
    };
    let id = ["human:", "agent:", "service:"]
        .iter()
        .find_map(|kind| value.strip_prefix(kind));
    if id.is_none_or(|id| id.is_empty() || id.contains('\0')) || *value == who.writer() {
        let case = Case::OnBehalfOfForm {
            value: value.clone(),
        };
        return Err(Error::invalid_at(&"on_behalf_of".into(), case));
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
    pub lake: &'a Turn<'a>,
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
    /// The keys sealed apart its own ingests name, and those read from the
    /// log ([`crate::sealing::Apart`]).
    apart: crate::sealing::Apart,
    /// The pending proposals of records' calls, read once at the first
    /// retirement, which closes its record's (064).
    calls: Option<BTreeMap<(TypeName, RecordKey), Vec<ProposalId>>>,
    /// What it holds of the views its sets of views read.
    viewed: Option<Box<viewed::Viewed>>,
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
        if let Some(v) = self.viewed {
            self.reads.extend(v.into_reads());
        }
        self.reads
    }

    /// Begin a job of a batch over what the jobs before it left: its
    /// counters start again, and what it changes can be put back.
    pub(crate) fn mark(&mut self) -> Mark {
        self.across = 0;
        if let Some(v) = &mut self.viewed {
            v.open();
        }
        Mark {
            records: self.records.clone(),
            members: self.members.clone(),
            sealed: self.sealed.clone(),
            apart: self.apart.clone(),
            calls: self.calls.clone(),
            viewed: self.viewed.is_some(),
        }
    }

    /// Keep what the job marked last changed.
    pub(crate) fn keep(&mut self) {
        if let Some(v) = &mut self.viewed {
            v.close(true);
        }
    }

    /// Put back what the job marked by `m` changed.
    pub(crate) fn undo(&mut self, m: Mark) {
        (self.records, self.members) = (m.records, m.members);
        (self.sealed, self.apart, self.calls) = (m.sealed, m.apart, m.calls);
        match (&mut self.viewed, m.viewed) {
            (Some(v), true) => v.close(false),
            (viewed, _) => *viewed = None,
        }
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

/// What a [`Pending`] held when a job of a batch began
/// ([`Pending::mark`]); what a job reads and does not change, the records
/// read from the projection, stays.
pub(crate) struct Mark {
    records: BTreeMap<(TypeName, RecordKey), StoredRecord>,
    members: BTreeMap<(String, TypeName, RecordKey), bool>,
    sealed: crate::backed::Recorded,
    apart: crate::sealing::Apart,
    calls: Option<BTreeMap<(TypeName, RecordKey), Vec<ProposalId>>>,
    /// Whether it held what the views read, then opened to be put back.
    viewed: bool,
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

/// What a decision reads of its call: the lake, who it is made as, and
/// the host's context. A preflight decides through one with no turn, as
/// it takes no writer.
#[derive(Clone, Copy)]
pub(crate) struct Asked<'a> {
    pub lake: &'a Lake,
    pub who: &'a Principal,
    pub context: &'a WriteContext,
}

impl Asked<'_> {
    fn draft(&self, now: Timestamp, origin: Origin, body: EventBody) -> Draft {
        Draft {
            writer: self.who.writer(),
            origin,
            context: self.context.clone(),
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
            context: self.context.clone(),
            ..Draft::system(now, body)
        }
    }
}

impl Call<'_> {
    /// What a decision of this call reads of it.
    pub(crate) fn asked(&self) -> Asked<'_> {
        Asked {
            lake: self.lake,
            who: self.who,
            context: &self.context,
        }
    }

    pub(crate) fn draft(&self, now: Timestamp, origin: Origin, body: EventBody) -> Draft {
        self.asked().draft(now, origin, body)
    }

    /// Whether its caller may change a shape ([`Sight::authors`]), asked
    /// where it is refused before the writer is taken and before anything
    /// is read: so of a sight over no declarations, since the answer reads
    /// its grants alone.
    pub(crate) fn authors(&self) -> bool {
        Sight::held(self.who, &Vec::<lakelands_api::engine::Declared>::new()).authors()
    }

    /// [`Sight::sees_restricted`] of its caller, asked as [`Self::authors`] is.
    pub(crate) fn sees_restricted(&self) -> bool {
        Sight::held(self.who, &Vec::<lakelands_api::engine::Declared>::new()).sees_restricted()
    }

    fn changed(&self, change: Change, cause: Position, now: Timestamp) -> Draft {
        self.asked().changed(change, cause, now)
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
            return Err(Error::invalid(Case::IdempotencyKeyNul));
        }
        let id = log::authored_id(&self.who.writer(), k);
        let Some(pos) = view.applied(&id)? else {
            return Ok(None);
        };
        let events = self.appended(w, pos)?;
        if events.is_empty() {
            return Ok(None);
        }
        let outcome = self.told_of(&w.ontology, &events, outcome_of(&events), None)?;
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
                EventBody::TriggerFired { .. }
                    | EventBody::TriggerDropped { .. }
                    | EventBody::ObjectSetChanged { .. }
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
        let decided = || Error::refused(None, Refusal::Decided);
        let at = view.applied(&log::accepted_id(&self.lake.id, proposal))?;
        let Some(at) = at else { return Err(decided()) };
        let events = self.appended(w, at)?;
        let outcome = self.told_of(&w.ontology, &events, outcome_of(&events), None)?;
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
                // A firing dropped past its depth fired nothing.
                EventBody::TriggerDropped { .. } => {}
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
        &crate::record::Plans,
        &crate::sealing::Keys,
    ),
    r: &StoredRecord,
    event: &Event,
    settings: &BTreeMap<String, Value>,
) -> Result<StoredRecord> {
    let mut outputs = Outputs::planning(view, plans, keys);
    let is = crate::record::rederive(computing, r, event, settings, &mut outputs)?;
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
/// declaration decide by this one rule. A set decided at the read holds
/// no member: a read expands its `where` at its own clock.
fn member_of(
    (set, o): (&ObjectSetDecl, &Ontology),
    record: &StoredRecord,
    settings: &BTreeMap<String, Value>,
    now: Timestamp,
    heads: Heads<'_>,
) -> Result<bool> {
    if record.retired || set.at_read() {
        return Ok(false);
    }
    let at = [Place::ObjectSet {
        name: set.name.clone(),
    }];
    let e = declare::set_filter(&|n| o.set(n), set, &at)?;
    let scope = RecordScope {
        fields: &record.fields,
        meta: Some(&record.meta),
        settings: Some(settings),
        now,
    };
    let on = o.ty(&set.of)?;
    Ok(across::holds(
        (heads, o, on),
        record,
        &e,
        scope,
        (settings, false),
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
        if changes.len() <= max
            && member != member_of((s, &w.ontology), is, &w.settings, now, heads)?
        {
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
            Error::invalid_at(name, Case::UnknownField { ty, field })
        })?;
        let vt =
            f.ty.value_type
                .as_ref()
                .and_then(|n| lake_types.value_types.get(n.as_str()))
                .map(|e| &e.decl);
        let checked =
            value::check(v, &f.ty, vt).map_err(|e| Error::constraint(Some(name.clone()), e))?;
        if let (Some(vt), false) = (vt, checked.is_null())
            && !satisfies(vt, &checked)?
        {
            let value_type = vt.name.clone();
            let case = Case::ValueConstraint { value_type };
            return Err(Error::constraint(Some(name.clone()), case));
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
    o: &Ontology,
    decl: &TypeDecl,
    key: &RecordKey,
    patch: &mut Patch,
    door: (Door, Option<&SourceId>),
    sees: &Sight<'_>,
) -> Result<()> {
    if decl.key.is_empty() || decl.key.iter().all(|k| patch.contains_key(k)) {
        return Ok(());
    }
    let parts = lakelands_core::log::key_components(key);
    if parts.len() != decl.key.len() {
        let (ty, fields, key, parts) =
            (decl.name.clone(), decl.key.len(), key.clone(), parts.len());
        return Err(Error::invalid(Case::KeyComponents {
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
            Error::invalid(Case::KeyComponentKind { part, kind, field })
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
    sees.check_write(decl, door, &filled)?;
    patch.extend(check_values(o, decl, &filled, &decl.fields)?);
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
    sees: &Sight<'_>,
) -> Result<()> {
    let decl = ontology.ty(ty)?;
    for (name, v) in patch {
        let Some(f) = decl.field(name) else {
            continue;
        };
        let (Some(target), Some(to)) = (&f.ty.target, v.as_str()) else {
            continue;
        };
        let to = RecordKey::from(to);
        if target == ty && to == *key {
            continue;
        }
        if live_for(view, pending, target, &to, sees)?.is_none() {
            let (field, ty, key) = (name.clone(), target.clone(), to);
            return Err(Error::constraint(
                Some(name.clone()),
                Case::NotLive { field, ty, key },
            ));
        }
    }
    Ok(())
}

/// The live record of `target` at `to` that the principal `sees` is of
/// is answered, as the drafts so far leave it: what a reference may name
/// (032).
fn live_for(
    view: &dyn ProjectionView,
    pending: &Pending,
    target: &TypeName,
    to: &RecordKey,
    sees: &Sight<'_>,
) -> Result<Option<StoredRecord>> {
    // A record a holder of `only` is not admitted, or one restricted, is
    // to it not there; under an `only` that cannot be applied, none is
    // (lakelands#771); nor is a record of a type it is not shown
    // (lakelands#1003).
    let held = sees.ty(target).ok();
    let found = pending.record(view, target, to)?;
    let mut exists =
        (found.as_ref()).is_some_and(|r| !r.retired && held.is_some() && sees.answers(target, r));
    // A link record is there only with its ends, as a read of it
    // answers (006, 051; lakelands#871).
    if let (true, Some(d), Some(r)) = (exists, held, &found) {
        exists = ends(view, pending, sees, d, &r.fields)?;
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
            let field = decl.field(d);
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
            Err(Error::invalid(Case::KeyDisagrees { key, ty, derived }))
        }
        _ => Ok(()),
    }
}

/// Refuse a key that is empty or holds a NUL character.
pub(crate) fn check_key(key: &RecordKey) -> Result<()> {
    if key.as_str().is_empty() || key.as_str().contains('\0') {
        return Err(Error::invalid(Case::KeyEmpty));
    }
    Ok(())
}

/// Refuse a source id that is empty or holds a NUL character: it is a
/// part of an ingested event's id, whose parts NUL separates.
pub(crate) fn check_source(source: &SourceId) -> Result<()> {
    match formed(source) {
        true => Ok(()),
        false => Err(Error::invalid(Case::SourceIdForm)),
    }
}

/// Whether a source id is of its form: not empty, and no NUL.
pub(crate) fn formed(source: &SourceId) -> bool {
    !source.as_str().is_empty() && !source.as_str().contains('\0')
}

/// A caller's delivery id: not empty, no NUL, and not of the store's own
/// form, `d` and digits, so a selection by delivery names one.
pub(crate) fn check_delivery(d: &lakelands_api::DeliveryId) -> Result<()> {
    let s = d.as_str();
    let own = s
        .strip_prefix('d')
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    if s.is_empty() || s.contains('\0') || own {
        return Err(Error::invalid(Case::DeliveryIdForm));
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

/// Whether `d` is a function declared again whose input a type in `o` holds
/// in another kind, that `seen`, the lake as its judge is shown it, does not
/// show: the refusal would name that type or field.
fn unseen_binding(o: &Ontology, seen: &Ontology, d: &Declaration) -> bool {
    let Declaration::Function(f) = d else {
        return false;
    };
    declare::input_kind_broken(o, f).is_some() && declare::input_kind_broken(seen, f).is_none()
}

/// `seen` without the object sets and triggers `sees` is not shown: what a
/// withdrawal is judged against is what its author or reviewer is shown.
fn retain_listed(seen: &mut Ontology, sees: &Sight) {
    (seen.object_sets).retain(|_, e| sees.lists(&Declaration::ObjectSet(e.decl.clone())));
    (seen.triggers).retain(|_, e| sees.lists(&Declaration::Trigger(e.decl.clone())));
}

/// `o` with the types `who` is shown, each as it knows it.
pub(crate) fn types_shown(who: &Principal, o: &Ontology) -> Ontology {
    let (sees, mut seen) = (Sight::held(who, o), o.clone());
    (seen.types).retain(|name, _| sees.ty(name).is_ok());
    // A type copied only where its author is kept from some of it
    // (lakelands#943): any other is the lake's own.
    let find = |n: &TypeName| o.ty(n).ok();
    for e in seen.types.values_mut() {
        if sees.keeps_from(&e.decl, &find) {
            sees.known_fields(&mut Arc::make_mut(e).decl, &find);
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
        return Err(Error::invalid_at(&field, Said { at, case }));
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

/// A write of `sys.setting` as the store writes it, `from` or `period`
/// resolved against the record `live` holds, as the head and the drafts
/// so far leave it (090): `from` adds the value from that date into
/// `periods`, `period` corrects the one from `start` (`value`) or from a
/// date (`periods`), and neither writes `value`, the first, where no value
/// holds from a date. Each refusal is said at the setting and names the
/// member it is about; a writer the declaration is not told is answered
/// as for a key nobody declared (063). A first dated period is refused
/// where a kept view or a set decided at the write reads the key without
/// a date.
fn dated(
    (o, settings): (&Ontology, &BTreeMap<String, Value>),
    decl: &TypeDecl,
    (key, live): (&RecordKey, Option<&StoredRecord>),
    told: &dyn Fn(&SettingDecl) -> bool,
    patch: &Patch,
) -> Result<Patch> {
    let refused = |member: &str, case: Case| {
        let at = vec![Place::Setting {
            key: key.to_string(),
        }];
        Err(Error::invalid_at(
            &FieldName::from(member),
            Said { at, case },
        ))
    };
    let mut rest = patch.clone();
    let (from, period) = (rest.remove("from"), rest.remove("period"));
    let held = live.and_then(|r| r.fields.get("periods")?.as_array().cloned());
    if patch.contains_key("periods") {
        return refused("periods", Case::SettingPeriodsStamped);
    }
    let (member, at) = match (from, period) {
        (None, None) if held.is_some() && patch.contains_key("value") => {
            return refused("value", Case::SettingPeriodsNamed);
        }
        (None, None) => return Ok(rest),
        (Some(_), Some(_)) => return refused("period", Case::SettingFromOrPeriod),
        (Some(at), None) => ("from", at),
        (None, Some(at)) => ("period", at),
    };
    let Some(s) = o.settings.get(key.as_str()).filter(|s| told(&s.decl)) else {
        return refused(member, Case::SettingPeriodUndeclared);
    };
    if member == "period" && at == "start" {
        return Ok(rest);
    }
    let day = FieldDecl::of(member, lakelands_api::ontology::StorageKind::Date);
    let day = check_values(o, decl, &[(member.into(), at.clone())].into(), &[day])?;
    let day = day.get(member).cloned().unwrap_or(at);
    let value = match rest.remove("value").filter(|v| !v.is_null()) {
        Some(v) => judged(o, decl, &s.decl, &v)?,
        None => {
            let (ty, field) = (decl.name.clone(), FieldName::from("value"));
            return Err(Error::constraint(
                Some(field.clone()),
                Case::Required { ty, field },
            ));
        }
    };
    let mut list = held.unwrap_or_default();
    let new = day.as_str().unwrap_or_default().to_owned();
    let from_of = |p: &Value| p.get("from").and_then(Value::as_str).map(str::to_owned);
    let found = list
        .iter()
        .position(|p| from_of(p).as_deref() == Some(&new));
    match (member, found) {
        ("from", _) if live.is_none() => return refused(member, Case::SettingPeriodFirst),
        ("from", Some(_)) => return refused(member, Case::SettingPeriodHeld { from: new }),
        ("from", None) if list.is_empty() => {
            if let Some(name) = crate::kept::reading_undated(o, settings, key.as_str()) {
                let key = key.to_string();
                return refused(member, Case::SettingDatedKept { key, name });
            }
            list.push(serde_json::json!({"from": new, "value": value}));
        }
        ("from", None) => {
            let at = list.partition_point(|p| from_of(p).is_some_and(|f| f < new));
            list.insert(at, serde_json::json!({"from": new, "value": value}));
        }
        (_, Some(i)) => list[i]["value"] = value,
        (_, None) => return refused(member, Case::SettingPeriodUnknown { period: new }),
    }
    rest.insert("periods".into(), Value::Array(list));
    Ok(rest)
}

/// A write of `view_kept.<view>` with `true` for a view that reads,
/// without a date, a key holding periods is refused as a view that cannot
/// be kept is (049), with that reason to a writer shown the view as
/// declared (090).
fn kept_dated(
    (o, settings): (&Ontology, &BTreeMap<String, Value>),
    key: &str,
    value: Option<&Value>,
    shown: &dyn Fn(&TypeName) -> Option<lakelands_api::describe::TypeDescription>,
) -> Result<()> {
    let asked = key.strip_prefix(lakelands_api::seam::VIEW_KEPT_SETTING);
    let Some(ty) = asked.filter(|_| lakelands_api::seam::view_kept(value)) else {
        return Ok(());
    };
    let ty = TypeName::from(ty);
    let read = o
        .ty(&ty)
        .map(|d| crate::kept::undated(o, d))
        .unwrap_or_default();
    let dated = |k: &String| settings.contains_key(&lakelands_api::settings::periods_key(k));
    let Some(read) = read.into_iter().find(dated) else {
        return Ok(());
    };
    let case = match shown(&ty).is_some_and(|s| s.view.is_some()) {
        true => Case::SettingDatedKept {
            key: read,
            name: ty.to_string(),
        },
        false => Case::ViewNotKeepable { ty },
    };
    let said = Said::from(case).within(Place::Setting {
        key: key.to_owned(),
    });
    Err(Error::refused(
        Some("key".into()),
        Refusal::Invalid(Box::new(said)),
    ))
}

/// A declaration of a set decided at the write, or of a view the lake
/// keeps, that reads without a date a key holding periods is refused,
/// naming it and the key (090).
fn declared_dated(
    (o, settings): (&Ontology, &BTreeMap<String, Value>),
    declarations: &[Declaration],
) -> Result<()> {
    let reads = |d: &Declaration| match d {
        Declaration::ObjectSet(_) => true,
        Declaration::Type(t) => t.view.is_some(),
        _ => false,
    };
    if !declarations.iter().any(reads) {
        return Ok(());
    }
    let mut after = o.clone();
    declarations
        .iter()
        .for_each(|d| after.put(d.clone(), 0, None));
    let kept = |t: &TypeDecl| {
        let asked = format!("{}{}", lakelands_api::seam::VIEW_KEPT_SETTING, t.name);
        lakelands_api::seam::view_kept(settings.get(&asked))
    };
    let dated = |k: &String| settings.contains_key(&lakelands_api::settings::periods_key(k));
    for d in declarations {
        let (name, read) = match d {
            Declaration::ObjectSet(s) => (s.name.clone(), crate::kept::set_undated(&after, s)),
            Declaration::Type(t) if kept(t) => {
                (t.name.to_string(), crate::kept::undated(&after, t))
            }
            _ => continue,
        };
        if let Some(key) = read.into_iter().find(dated) {
            let said = Said::from(Case::SettingDatedKept { key, name });
            return Err(Error::invalid(said.within(d.place())));
        }
    }
    Ok(())
}

/// `o` without each declaration `sent` withdraws that `sees` does not
/// list: its withdrawal is refused as one of a name not declared, where
/// the rest of its set is judged, as a read of it answers (024).
fn unlisted_withdrawn<'o>(
    o: &'o Ontology,
    sees: &Sight<'_>,
    sent: &[Declaration],
) -> std::borrow::Cow<'o, Ontology> {
    let mut out = std::borrow::Cow::Borrowed(o);
    for d in sent {
        let Declaration::Withdraw(w) = d else {
            continue;
        };
        if o.held(d).is_none_or(|held| sees.lists(&held)) {
            continue;
        }
        out.to_mut().take_out(w.kind, &w.name);
    }
    out
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
        Declaration::Withdraw(w) => (w.kind.naming(), w.name.clone()),
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
        let sets = v.effect.set().iter().any(reference);
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
            (ty, key, crate::record::STORE_WRITER),
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
        let f = decl.field(n)?;
        Some((f.ty.target.clone()?, RecordKey::from(v.as_str()?)))
    }));
    out.extend(across);
    Ok(out.iter().flat_map(|(t, k)| with_ends(o, t, k)).collect())
}

/// The record `before` at `key` of `ty` as the fold would leave it after a
/// write of `patch` by `writer`, with these settings at `now`: its derived
/// values, an initial value, a field the store fills with the writer, and a
/// field waiting for its backing too.
pub(crate) fn planned(
    lake: &Lake,
    o: &Ontology,
    (view, settings, now): (&dyn ProjectionView, &BTreeMap<String, Value>, Timestamp),
    before: Option<&StoredRecord>,
    (ty, key, writer): (&TypeName, &RecordKey, &str),
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
    let draft = Draft {
        writer: writer.to_owned(),
        ..Draft::system(now, body)
    };
    let event = provisional(lake, Position::GENESIS, &draft);
    let plans = crate::record::Plans::default();
    let outputs = &mut Outputs::planning(view, &plans, &lake.keys);
    crate::record::apply(o, before, &event, settings, outputs)
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
        // Only what the condition reads is wanted here, not its answer.
        let _ = requires(settings, (v, payload), k, &r, (now, &across), &|_| true);
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

/// The lake's head: the position of its last event, or of the last a
/// batch staged past it.
fn head(w: &Writer) -> Position {
    let tip = w.staged.as_ref().or(w.tip.as_ref());
    tip.map_or(Position::GENESIS, |t| t.0)
}

/// A migration report's id: named by what it says to its reader and where
/// the lake stood, its head as [`Lake::report_head`] names it, so that an
/// apply after the lake moved is refused, and computed by its reader from
/// what it holds, which names nothing hidden (024, extended).
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

/// The next position in an append that already holds `n` drafts, after
/// what a batch staged.
pub(crate) fn position_after(w: &Writer, n: usize) -> Position {
    let tip = w.staged.as_ref().or(w.tip.as_ref());
    let first = tip.map_or(Position::GENESIS, |t| t.0.next());
    Position(first.0 + n as u64)
}

/// Refuse a verb whose `requires` or a precondition does not hold on
/// `record` and the call's `payload`, in the write it guards, naming the
/// precondition and the first of its conjuncts that does not where every
/// setting it and the ones before it read is `told` its caller, and
/// before that one whose conditions read across a reference that reaches
/// no record (`across`). The conjuncts are evaluated in order, `requires`
/// first, as an `and` of them all evaluates them, so they read what it
/// reads (006).
fn requires(
    settings: &BTreeMap<String, Value>,
    (verb, payload): (&VerbDecl, &Patch),
    key: &RecordKey,
    record: &StoredRecord,
    (now, across): (Timestamp, &Across<'_>),
    told: &dyn Fn(&str) -> bool,
) -> Result<()> {
    let at = |n: Option<&PreconditionName>| {
        let name = verb.name.clone();
        let of = n.map_or(Place::Requires, |n| Place::Precondition { name: n.clone() });
        [Place::Verb { name }, of]
    };
    let named = (verb.preconditions.iter()).map(|p| (Some(&p.name), &p.holds));
    let mut conditions = Vec::new();
    for (n, c) in verb.requires.iter().map(|r| (None, r)).chain(named) {
        conditions.push((n, declare::condition(c, Eval::Verb, &at(n))?));
    }
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
    let parts: Vec<_> = (conditions.iter())
        .flat_map(|(n, e)| e.conjuncts().into_iter().map(move |p| (*n, p)))
        .collect();
    let unmet = parts
        .iter()
        .position(|(_, p)| !expr::holds(p, &scope).unwrap_or(false));
    across.reached()?;
    let Some(at) = unmet else {
        return Ok(());
    };
    let (verb, ty, key) = (verb.name.clone(), verb.on.clone(), key.clone());
    // Naming a part, or its precondition, tells that every part before it
    // held.
    let mut read = BTreeSet::new();
    for (_, p) in &parts[..=at] {
        lakelands_core::derive::settings(p, &mut read);
    }
    let (precondition, part) = parts[at];
    let shown = read.iter().all(|k| told(k));
    let part = shown.then(|| Box::new(Printed(expr::print(part))));
    let said = Said::from(Case::RequiresUnmet {
        verb,
        ty,
        key,
        part,
    });
    let said = match precondition.filter(|_| shown) {
        Some(name) => said.within(Place::Precondition { name: name.clone() }),
        None => said,
    };
    Err(Error::refused(None, Refusal::Precondition(Box::new(said))))
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
            Error::invalid(Case::VerbCannotCompute { verb, field, why })
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
                Error::invalid(Case::VerbValueUnfit { verb, field })
            })?;
        patch.insert(a.field.clone(), json);
    }
    Ok(patch)
}

impl Call<'_> {
    /// The writer, held by this call. Its grants were followed through the
    /// lake's renames at its turn; where a migration caught up with here
    /// (another handle's, or one whose fold the lake had not yet made)
    /// renamed a field or a relation since, they may no longer hold it, and the call is refused to be sent again, in words
    /// that name no migration (006, extended).
    pub(crate) fn writer(&self) -> Result<std::sync::MutexGuard<'_, crate::lake::Writer>> {
        let w = self.lake.for_write()?;
        self.moved(&w)?;
        Ok(w)
    }

    /// Refuse the call where what its grants were followed through moved
    /// under `w` ([`Self::writer`]).
    pub(crate) fn moved(&self, w: &Writer) -> Result<()> {
        if let Some(n) = self.followed
            && self.lake.renames(None)?.len() != n
        {
            return Err(Error::unavailable(Situation::Moved));
        }
        if let Some(s) = &self.shown
            && s.versions != w.ontology.versions()
            && (self
                .lake
                .sight(s.asked.clone(), (None, false), None, &w.ontology)?
                .0)
                .hidden()
                != s.hidden
        {
            return Err(Error::unavailable(Situation::TypesMoved));
        }
        Ok(())
    }
}

/// The relation's cardinality over live links, with the link from `from`
/// to `to` among them: `many_to_one` and `one_to_one` allow one live link
/// per `from`, `one_to_many` and `one_to_one` one per `to`. Answered from
/// the links the engine walks at the head, less those `pending` retired,
/// not by reading every link, then from the live links `pending` holds,
/// the records read as `pending` leaves them: one answer wherever the
/// store cut the stream (016), and wherever a batch of writes is cut. The
/// other end of the link that holds is named only where a record holds
/// its key and the principal of `sees` is answered that link: by its ends,
/// as deep as they go, and by `only` on the link type itself (006, 051).
pub fn cardinality(
    (view, sees): (&dyn ProjectionView, &Sight),
    pending: &Pending,
    (ty, r): (&TypeName, &RelationDecl),
    from: &RecordKey,
    to: &RecordKey,
) -> Result<()> {
    use lakelands_api::query::Walk;
    let (per_from, per_to) = one_per(r.cardinality);
    let (relation, cardinality) = (r.name.clone(), r.cardinality);
    let broken = |case: Case| Error::constraint(Some(links::TO.into()), case);
    let lt = links::link_type_name(ty, &r.name);
    let get = |t: &TypeName, k: &RecordKey| pending.record(view, t, k);
    let key_at = |l: &StoredRecord, f: &str| l.fields.get(f)?.as_str().map(RecordKey::from);
    let earlier = || {
        let live = pending
            .records
            .iter()
            .filter(|((t, _), l)| *t == lt && !l.retired);
        live.filter_map(|(_, l)| Some((key_at(l, links::FROM)?, key_at(l, links::TO)?)))
            .filter(|(f, o)| (f, o) != (from, to))
    };
    let retired = |f: &RecordKey, o: &RecordKey| {
        let at = (lt.clone(), links::link_key(f, o));
        pending.records.get(&at).is_some_and(|l| l.retired)
    };
    let walk = |end: &TypeName, key: &RecordKey, dir| -> Result<Vec<_>> {
        Ok(view
            .edges(end, std::slice::from_ref(key), Some(&r.name), dir)?
            .remove(0))
    };
    // `from`'s other link first, in the head and then in `pending`, then
    // `to`'s: the one answered does not turn on where the store cut.
    let mut other = None;
    if per_from
        && let Some(e) = walk(ty, from, Walk::Out)?
            .into_iter()
            .find(|e| e.to.1 != *to && !retired(from, &e.to.1))
    {
        other = Some((true, e.to.1));
    }
    if other.is_none() && per_from {
        other = earlier().find(|(f, _)| f == from).map(|(_, o)| (true, o));
    }
    if other.is_none() && per_to {
        let mut there = walk(&r.target, to, Walk::In)?.into_iter();
        let e = there.find(|e| e.from.0 == *ty && e.from.1 != *from && !retired(&e.from.1, to));
        other = e.map(|e| (false, e.from.1));
    }
    if other.is_none() && per_to {
        other = earlier().find(|(_, o)| o == to).map(|(f, _)| (false, f));
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
        let Ok(decl) = sees.ty(&lt) else {
            return Ok(false);
        };
        let Some(l) = get(&lt, &link)? else {
            return Ok(true);
        };
        let mut many = |t: &TypeName, ks: &[RecordKey]| -> Result<Vec<StoredRecord>> {
            ks.iter().filter_map(|k| get(t, k).transpose()).collect()
        };
        let all = sees.sees_all();
        Ok(all || sees.ends(decl, &[&l.fields], &mut many)?[0] && sees.answers(&lt, &l))
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
    Error::refused(None, Refusal::Unanswered(Box::new(case.into())))
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
    lake: &Turn<'_>,
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
    lake: &Turn<'_>,
    ty: &TypeName,
    key: &RecordKey,
    reason: Option<String>,
) -> Result<Position> {
    let mut w = lake.for_write()?;
    not_a_view(w.ontology.ty(ty)?)?;
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
        Err(Error::invalid(Case::Restriction { ty, key, already }))
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
