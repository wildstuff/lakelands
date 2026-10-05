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
//! ([`crate::record::apply`]); from it the store decides which triggers
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

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use lakelands_api::engine::ProjectionView;
use lakelands_api::event::{Digest, Event, EventBody, Procedure};
use lakelands_api::ontology::{
    Authority, FieldDecl, RelationDecl, Risk, TriggerDecl, TypeDecl, ValueTypeDecl, VerbDecl,
    VerbEffect,
};
use lakelands_api::principal::WriteContext;
use lakelands_api::record::{Origin, StoredRecord};
use lakelands_api::seam::VerbExecutor;
use lakelands_api::write::{
    Carried, Carrier, Declaration, Migration, MigrationReport, Misfit, ObjectSetDecl, Outcome,
    ProposalState, ProposedWrite, Recompute, RelationRename, ValueTypeReport, ValueTypeUse,
    WriteOpts,
};
use lakelands_api::{
    Access, Error, FieldName, IdempotencyKey, Missing, Patch, Position, Principal, ProposalId,
    RecordKey, Refusal, RelationName, Result, Timestamp, TypeName, Value, VerbName, Version,
};
use lakelands_core::expr::{self, Place, RecordScope, Scope, WriteScope};
use lakelands_core::rights::{self, Door};
use lakelands_core::{describe, invalid, links, shape, value};

use crate::backed::Outputs;
use crate::declare::{self, invalid_field};
use crate::lake::{AGING_RULE_TYPE, Lake, SETTING_TYPE, Writer, every, find_proposal};
use crate::log::{self, Draft};
use crate::ontology::Ontology;
use crate::record;

/// How many events after an external verb's call a retry reads for the
/// output recorded for it.
const RETRY_REACH: u32 = 4_096;

/// A set membership that changes: the set, the record's type and its key
/// (after the migration, when one changes it), and whether it joined.
type Change = (String, TypeName, RecordKey, bool);

/// A type's sets, each with its members before a migration.
type Held<'a> = Vec<(&'a ObjectSetDecl, BTreeSet<RecordKey>)>;

/// What a migration's preflight finds: its report, the record keys it
/// moves and, when it is applied, the set memberships it changes.
pub(crate) struct MigrationPlan {
    report: MigrationReport,
    rekeyed: Vec<lakelands_api::write::Rekey>,
    members: Vec<Change>,
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
    match (verb.scope, key) {
        (On::Type, Some(_)) => Err(invalid(format!(
            "verb `{}` is called on the type `{}`, not on a record: it takes no key",
            verb.name, verb.on
        ))),
        (On::Record, None) => Err(invalid(format!(
            "verb `{}` is called on a `{}` record: it needs that record's key",
            verb.name, verb.on
        ))),
        (On::Type, None) if expected.is_some() => Err(invalid(format!(
            "verb `{}` is called on the type `{}`: there is no record version to compare",
            verb.name, verb.on
        ))),
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

/// A refusal of what a constraint does not allow, at `field` if one is named.
pub(crate) fn constraint(field: Option<FieldName>, message: impl Into<String>) -> Error {
    Error::Refused {
        field,
        reason: Refusal::Constraint {
            message: message.into(),
        },
    }
}

/// A refusal that names no field.
pub(crate) fn refused(reason: Refusal) -> Error {
    let field = None;
    Error::Refused { field, reason }
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
    bound.ok_or_else(|| Error::Unbound {
        seam: format!("the verb executor `{name}`"),
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
}

/// The record events of one append that are not in the projection yet.
#[derive(Default)]
pub struct Pending {
    records: BTreeMap<(TypeName, RecordKey), StoredRecord>,
    members: BTreeMap<(String, TypeName, RecordKey), bool>,
    /// The records read from the projection, each once; the fold of the
    /// append takes them over ([`crate::lake::Lake::append_after`]).
    reads: crate::lake::Reads,
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
    /// it set off: its append is read back as far as its reactions can
    /// reach, one per trigger and object set the lake declares.
    fn replay(
        &self,
        w: &Writer,
        key: &Option<IdempotencyKey>,
        view: &dyn ProjectionView,
    ) -> Result<Option<Outcome>> {
        let Some(k) = key else { return Ok(None) };
        let id = log::authored_id(&self.who.writer(), k);
        let Some(pos) = view.applied(&id)? else {
            return Ok(None);
        };
        let reach = 3 + w.ontology.triggers.len() + w.ontology.object_sets.len();
        let events = self.lake.read_events(pos, reach as u32)?;
        if events.is_empty() {
            return Ok(None);
        }
        let mut outcome = outcome_of(&events);
        // An external verb answered with its executor's output, recorded in
        // a later append that other writes may precede: a retry answers
        // that output too, at the verb's own position.
        let external = events.iter().any(|e| {
            e.position == outcome.position() && matches!(e.body, EventBody::VerbCalled { .. })
        });
        if external && let Some(found) = self.recorded_output(outcome.position())? {
            outcome = with_output(outcome, found);
        }
        Ok(Some(outcome))
    }

    /// The output recorded for the external verb called at `called`, and
    /// its position: the `output_recorded` that event caused, among the
    /// [`RETRY_REACH`] events after it. Its `input` digest holds the record
    /// as the executor was given it, which a retry cannot rebuild to look
    /// it up by digest; the read is bounded so that a retry never reads to
    /// the head of a large lake (`docs/seams.md` §`VerbExecutor`).
    fn recorded_output(&self, called: Position) -> Result<Option<(Position, Value)>> {
        let events = self.lake.read_events(called.next(), RETRY_REACH)?;
        Ok(events.into_iter().find_map(|e| match e.body {
            EventBody::OutputRecorded { seam, output, .. }
                if e.cause == Some(called) && seam.starts_with("verb:") =>
            {
                Some((e.position, output))
            }
            _ => None,
        }))
    }
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
                EventBody::TriggerFired { trigger, .. } => set_off.fired.push(trigger.clone()),
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

/// Whether `record` is a member of `set`: live, and the set's filter holds
/// over its fields at `now`, the time of the write or the declaration that
/// decides it, the sets it names read as `o` holds them. A write and a
/// set's declaration decide by this one rule.
fn member_of(
    (set, o): (&ObjectSetDecl, &Ontology),
    record: &StoredRecord,
    w: &Writer,
    now: Timestamp,
) -> Result<bool> {
    if record.retired {
        return Ok(false);
    }
    let e = declare::filter(
        &|n| o.set(n),
        &set.of,
        &set.filter,
        Place::Record,
        "object set",
    )?;
    let scope = RecordScope {
        fields: &record.fields,
        settings: Some(&w.settings),
        now,
    };
    Ok(expr::holds(&e, &scope).unwrap_or(false))
}

/// The preflight of `m` over the stored records of `decl`'s type, retired
/// and restricted ones included, read a page at a time. `each` sees every
/// record before the report counts it. An error of a page read or of
/// `each` ends the walk and is answered before the report is.
fn shaped(
    view: &dyn ProjectionView,
    decl: &TypeDecl,
    m: &Migration,
    mut each: impl FnMut(&StoredRecord) -> Result<()>,
) -> Result<MigrationReport> {
    let mut walk = crate::lake::Walk::new(view, &decl.name, true);
    let mut seen = Ok(());
    let rows = walk.by_ref().map_while(|r| {
        seen = each(&r);
        seen.is_ok().then_some((r.key, r.fields))
    });
    let report = shape::preflight(decl, m, rows);
    walk.failed()?;
    seen?;
    report.map_err(|e| invalid(e.to_string()))
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
) -> Result<()> {
    for (s, held) in sets {
        let member = held.contains(was);
        if changes.len() <= max && member != member_of((s, &w.ontology), is, w, now)? {
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
    }
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
        let f = fields
            .iter()
            .find(|f| &f.name == name)
            .ok_or_else(|| invalid_field(name, format!("`{}` has no field `{name}`", decl.name)))?;
        let vt =
            f.ty.value_type
                .as_ref()
                .and_then(|n| lake_types.value_types.get(n.as_str()))
                .map(|e| &e.decl);
        let checked = value::check(v, &f.ty, vt)
            .map_err(|e| constraint(Some(name.clone()), e.to_string()))?;
        if let (Some(vt), false) = (vt, checked.is_null())
            && let Some(c) = &vt.constraint
        {
            let e = declare::condition(c, Place::Record, "constraint")?;
            let scope_fields: BTreeMap<FieldName, Value> =
                [("value".into(), checked.clone())].into();
            let ok = expr::holds(
                &e,
                &RecordScope {
                    fields: &scope_fields,
                    settings: None,
                    now: Timestamp(0),
                },
            )
            .unwrap_or(false);
            if !ok {
                return Err(constraint(
                    Some(name.clone()),
                    format!("the value does not satisfy the constraint of `{}`", vt.name),
                ));
            }
        }
        out.insert(name.clone(), checked);
    }
    Ok(out)
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
        return Err(invalid(format!(
            "`{}` is keyed by {} field(s), and the key `{key}` has {} component(s)",
            decl.name,
            decl.key.len(),
            parts.len()
        )));
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
        let v = match f.ty.kind {
            lakelands_api::ontology::StorageKind::Number => {
                serde_json::from_str::<serde_json::Number>(&part)
                    .map(Value::Number)
                    .map_err(|_| {
                        invalid(format!(
                            "the key component `{part}` is not a number for `{name}`"
                        ))
                    })?
            }
            lakelands_api::ontology::StorageKind::Boolean => match part.as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => {
                    return Err(invalid(format!(
                        "the key component `{part}` is not a boolean for `{name}`"
                    )));
                }
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
/// target type.
fn check_references(
    view: &dyn ProjectionView,
    pending: &Pending,
    decl: &TypeDecl,
    ty: &TypeName,
    key: &RecordKey,
    patch: &Patch,
) -> Result<()> {
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
        let exists = pending
            .record(view, target, &to)?
            .is_some_and(|r| !r.retired);
        if !exists {
            return Err(constraint(
                Some(name.clone()),
                format!("`{name}` names `{target}` `{to}`, which is not a live record"),
            ));
        }
    }
    Ok(())
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
        Some(derived) if &derived != key => Err(invalid(format!(
            "the key `{key}` and the key fields of `{}` (which make `{derived}`) disagree",
            decl.name
        ))),
        _ => Ok(()),
    }
}

/// Refuse a key that is empty or holds a NUL character.
pub(crate) fn check_key(key: &RecordKey) -> Result<()> {
    if key.as_str().is_empty() || key.as_str().contains('\0') {
        return Err(invalid(
            "a record key is not empty and holds no NUL character",
        ));
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
    log::seal(&lake.id, position, &Digest::zero(), 0, d.clone()).0
}

/// The live record of `ty` at `key`; a retired one is not found.
fn live_record(view: &dyn ProjectionView, ty: &TypeName, key: &RecordKey) -> Result<StoredRecord> {
    view.get(ty, key)?
        .filter(|r| !r.retired)
        .ok_or_else(|| Error::NotFound {
            missing: Missing::Record {
                ty: ty.clone(),
                key: key.clone(),
            },
        })
}

/// Whether a migration changes nothing: its `to` is the type in force and
/// it renames no field (027, extended).
fn unchanged(o: &Ontology, m: &Migration) -> bool {
    m.renames.is_empty() && o.in_force(&Declaration::Type(m.to.clone()))
}

/// The lake's head: the position of its last event.
fn head(w: &Writer) -> Position {
    w.tip.as_ref().map_or(Position::GENESIS, |t| t.0)
}

/// The next position in an append that already holds `n` drafts.
pub(crate) fn position_after(w: &Writer, n: usize) -> Position {
    let first = w.tip.as_ref().map_or(Position::GENESIS, |t| t.0.next());
    Position(first.0 + n as u64)
}

/// Refuse a verb whose `requires` does not hold on `record`.
fn requires(
    w: &Writer,
    verb: &VerbDecl,
    key: &RecordKey,
    record: &StoredRecord,
    now: Timestamp,
) -> Result<()> {
    let Some(r) = &verb.requires else {
        return Ok(());
    };
    let e = declare::condition(r, Place::Record, "requires")?;
    let scope = RecordScope {
        fields: &record.fields,
        settings: Some(&w.settings),
        now,
    };
    if expr::holds(&e, &scope).unwrap_or(false) {
        return Ok(());
    }
    Err(refused(Refusal::Precondition {
        message: format!(
            "verb `{}` requires what `{}` `{key}` does not hold",
            verb.name, verb.on
        ),
    }))
}

/// The scope a verb's assignments see: the record's fields, and
/// `payload.<name>` for the call's payload.
struct VerbScope<'a> {
    record: &'a BTreeMap<FieldName, Value>,
    payload: &'a Patch,
    settings: &'a BTreeMap<String, Value>,
    now: Timestamp,
}

impl Scope for VerbScope<'_> {
    fn value(&self, path: &[FieldName]) -> Option<Value> {
        match lakelands_core::derive::payload_member(path) {
            Some(name) => self.payload.get(name).filter(|v| !v.is_null()).cloned(),
            None => RecordScope {
                fields: self.record,
                settings: None,
                now: self.now,
            }
            .value(path),
        }
    }

    fn setting(&self, key: &str) -> Option<Value> {
        self.settings.get(key).cloned()
    }

    fn now(&self) -> Timestamp {
        self.now
    }
}

/// The type `ty` for `who`: one it is not shown is answered as a type that
/// is not declared, at every door that writes a record or authors a type
/// (024, 027 and 028, extended). A refusal would tell that the type is
/// there, so this is asked before anything else is judged.
pub fn shown_type<'o>(o: &'o Ontology, ty: &TypeName, who: &Principal) -> Result<&'o TypeDecl> {
    let t = o.ty(ty)?;
    let missing = Missing::Type { name: ty.clone() };
    (describe::type_shown(t, &|n| o.ty(n).ok(), who).then_some(t))
        .ok_or(Error::NotFound { missing })
}

impl Call<'_> {
    /// Decide one record write: authored (`upsert`, a verb, aging) or from
    /// a source (`ingest`). `n` drafts precede it in the append. `stamp`
    /// sets the envelope of the record event before the record it makes is
    /// computed: aging writes as the system, with its rule as the procedure.
    #[allow(clippy::too_many_arguments)]
    pub fn plan_record(
        &self,
        w: &Writer,
        view: &dyn ProjectionView,
        pending: &Pending,
        ty: &TypeName,
        key: &RecordKey,
        patch: &Patch,
        door: Door,
        expected: Option<Version>,
        now: Timestamp,
        n: usize,
        who: &Principal,
        stamp: &dyn Fn(&mut Draft),
    ) -> Result<RecordPlan> {
        let decl = w.ontology.ty(ty)?;
        if patch.is_empty() {
            return Err(invalid("nothing to write: the patch carries no field"));
        }
        check_key(key)?;
        rights::check_write(who, decl, door, patch)?;
        let mut patch = check_values(&w.ontology, decl, patch, &decl.fields)?;
        let before = pending.record(view, ty, key)?;
        let live = before.as_ref().filter(|b| !b.retired);
        if live.is_none() {
            fill_key_fields(w, decl, key, &mut patch, door, who)?;
        }
        check_key_agrees(decl, key, live, &patch)?;
        check_references(view, pending, decl, ty, key, &patch)?;
        cas(expected, before.as_ref())?;
        let version = Version(before.as_ref().map_or(0, |b| b.version.0) + 1);
        // Said once, here, of what this writer holds, its own drafts too.
        let made = live.is_none();
        let (origin, body) = match door {
            Door::Authored => (
                Origin::Authored,
                EventBody::RecordUpserted {
                    ty: ty.clone(),
                    key: key.clone(),
                    version,
                    patch,
                    made,
                },
            ),
            Door::Source => (
                Origin::Source,
                EventBody::RecordIngested {
                    ty: ty.clone(),
                    key: key.clone(),
                    version,
                    patch,
                    made,
                },
            ),
        };
        let mut draft = self.draft(now, origin, body);
        stamp(&mut draft);
        let event = provisional(self.lake, position_after(w, n), &draft);
        let after = record::apply(
            &w.ontology,
            before.as_ref(),
            &event,
            &w.settings,
            &mut Outputs {
                view,
                staged: None,
                before: None,
                due: None,
            },
        )?
        .expect("a record event makes a record");
        record::check_required(decl, &after.fields)?;
        if ty.as_str() == AGING_RULE_TYPE {
            let rule = crate::aging::check(&w.ontology, &after.fields)
                .map_err(|(field, message)| invalid_field(&field, message))?;
            crate::aging::check_authority(who, &w.ontology, &rule)?;
        }
        if ty.as_str() == SETTING_TYPE {
            let declared = |t: &TypeName| w.ontology.ty(t).ok();
            let shown = |t: &TypeName| describe::one_type(t, &w.ontology.declared(), who);
            let value = after.fields.get("value");
            lakelands_api::seam::check_setting(key.as_str(), value, &declared, &shown)
                .map_err(|(field, message)| invalid_field(&field, message))?;
        }
        Ok(RecordPlan {
            draft,
            after,
            before,
        })
    }

    /// The triggers and object-set changes a record change causes.
    #[allow(clippy::too_many_arguments)]
    pub fn reactions(
        &self,
        w: &Writer,
        view: &dyn ProjectionView,
        pending: &mut Pending,
        before: Option<&StoredRecord>,
        after: &StoredRecord,
        cause: Position,
        now: Timestamp,
    ) -> Result<Vec<Draft>> {
        let mut out = Vec::new();
        // A reaction carries the context of the write that caused it, so
        // a host acting on it writes one level deeper (021).
        let system = |body| Draft {
            cause: Some(cause),
            context: self.context,
            ..Draft::system(now, body)
        };
        for t in w
            .ontology
            .triggers
            .values()
            .map(|e| &e.decl)
            .filter(|t| t.on == after.ty)
        {
            let fires = match &t.when {
                None => true,
                Some(c) => {
                    let sets = |n: &str| w.ontology.set(n);
                    let e = declare::filter(&sets, &t.on, c, Place::Guard, "trigger")?;
                    let live = before.filter(|b| !b.retired);
                    let scope = WriteScope {
                        after: &after.fields,
                        before: live.map(|b| &b.fields),
                        retiring: live.is_some() && after.retired,
                        meta: Some(&after.meta),
                        settings: Some(&w.settings),
                        now,
                    };
                    expr::holds(&e, &scope).unwrap_or(false)
                }
            };
            if fires {
                out.push(system(EventBody::TriggerFired {
                    trigger: t.name.clone(),
                    ty: after.ty.clone(),
                    key: after.key.clone(),
                }));
            }
        }
        for s in w
            .ontology
            .object_sets
            .values()
            .map(|e| &e.decl)
            .filter(|s| s.of == after.ty)
        {
            let was = pending.member(view, &s.name, &after.ty, &after.key)?;
            let is = member_of((s, &w.ontology), after, w, now)?;
            if was != is {
                pending
                    .members
                    .insert((s.name.clone(), after.ty.clone(), after.key.clone()), is);
                let change = (s.name.clone(), after.ty.clone(), after.key.clone(), is);
                out.push(self.changed(change, cause, now));
            }
        }
        pending
            .records
            .insert((after.ty.clone(), after.key.clone()), after.clone());
        Ok(out)
    }

    /// The drafts of a retirement: `d`, which retires `before` and lands
    /// at `at`, then the triggers and object-set changes it causes, judged
    /// on the record its provisional event makes.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn retiring(
        &self,
        w: &Writer,
        view: &dyn ProjectionView,
        pending: &mut Pending,
        before: &StoredRecord,
        d: Draft,
        at: Position,
        now: Timestamp,
    ) -> Result<Vec<Draft>> {
        let event = provisional(self.lake, at, &d);
        let outputs = &mut Outputs {
            view,
            staged: None,
            before: None,
            due: None,
        };
        let after = record::apply(&w.ontology, Some(before), &event, &w.settings, outputs)?
            .expect("a retire makes a record");
        let mut drafts = vec![d];
        drafts.extend(self.reactions(w, view, pending, Some(before), &after, at, now)?);
        Ok(drafts)
    }

    /// The drafts of a planned record write that lands at `at`: its own,
    /// then the triggers and object-set changes it causes.
    pub(crate) fn planned(
        &self,
        w: &Writer,
        view: &dyn ProjectionView,
        pending: &mut Pending,
        plan: RecordPlan,
        at: Position,
        now: Timestamp,
    ) -> Result<Vec<Draft>> {
        let (before, after) = (plan.before.as_ref(), &plan.after);
        let mut drafts = self.reactions(w, view, pending, before, after, at, now)?;
        drafts.insert(0, plan.draft);
        Ok(drafts)
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
        if let Some(first) = self.retried_unshown(ty, &opts)? {
            return Ok(first);
        }
        self.write_record(ty, key, patch, opts, |_, _| Ok(()))
    }

    /// An authored write of one record, as `upsert` and `link` make it,
    /// their context already checked; `check` refuses it, under the
    /// writer's lock, after the record's own checks: it reads other
    /// records, so the grants come first (024).
    fn write_record(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        patch: Patch,
        opts: WriteOpts,
        check: impl FnOnce(&Writer, &dyn ProjectionView) -> Result<()>,
    ) -> Result<Outcome> {
        let mut w = self.writer()?;
        let view = self.lake.engine.view(None)?;
        if let Some(o) = self.replay(&w, &opts.idempotency_key, view.as_ref())? {
            return Ok(o);
        }
        let now = self.lake.clock(&mut w);
        let mut pending = Pending::default();
        let mut plan = self.plan_record(
            &w,
            view.as_ref(),
            &pending,
            ty,
            key,
            &patch,
            Door::Authored,
            opts.expected_version,
            now,
            0,
            self.who,
            &|_| {},
        )?;
        check(&w, view.as_ref())?;
        self.keyed(&mut plan.draft, &opts.idempotency_key);
        let at = position_after(&w, 0);
        let drafts = self.planned(&w, view.as_ref(), &mut pending, plan, at, now)?;
        drop(view);
        let events = self.lake.append(&mut w, drafts)?;
        Ok(outcome_of(&events))
    }

    /// `retire`.
    pub fn retire(&self, ty: &TypeName, key: &RecordKey, opts: WriteOpts) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        ordinary_door(ty)?;
        if let Some(first) = self.retried_unshown(ty, &opts)? {
            return Ok(first);
        }
        self.retire_record(ty, key, opts)
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
        if let Some(first) = self.retried_unshown(ty, &opts)? {
            return Ok(first);
        }
        let (lt, r) = link_relation(&*self.writer()?, ty, relation)?;
        if r.authority != Authority::Authored {
            return Err(refused(Refusal::WrongDoor));
        }
        for name in [links::FROM, links::TO] {
            if fields.contains_key(name) {
                return Err(invalid_field(
                    &name.into(),
                    format!("a link's `{name}` is given as `key` and `to`, not among its fields"),
                ));
            }
        }
        let mut patch = fields;
        patch.insert(links::FROM.into(), Value::String(key.to_string()));
        patch.insert(links::TO.into(), Value::String(to.to_string()));
        let link_key = links::link_key(key, to);
        let (from, target) = (key.clone(), to.clone());
        self.write_record(&lt, &link_key, patch, opts, move |_, view| {
            cardinality(view, ty, &r, &from, &target)
        })
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
        if let Some(first) = self.retried_unshown(ty, &opts)? {
            return Ok(first);
        }
        let (lt, _) = link_relation(&*self.writer()?, ty, relation)?;
        self.retire_record(&lt, &links::link_key(key, to), opts)
    }

    /// The first question of `upsert`, `retire`, `link` and `unlink` (006
    /// and 024, extended): a caller not shown `ty` is answered as for a
    /// type that is not there, before a relation is named or the write is
    /// judged. Its retry of a write it made is answered that write, as
    /// every retry is before the grants are asked.
    fn retried_unshown(&self, ty: &TypeName, opts: &WriteOpts) -> Result<Option<Outcome>> {
        let w = self.writer()?;
        let Err(gone) = shown_type(&w.ontology, ty, self.who) else {
            return Ok(None);
        };
        let view = self.lake.engine.view(None)?;
        let first = self.replay(&w, &opts.idempotency_key, view.as_ref())?;
        first.map(Some).ok_or(gone)
    }

    /// A retirement of one record, as `retire` and `unlink` make it, their
    /// context already checked.
    fn retire_record(&self, ty: &TypeName, key: &RecordKey, opts: WriteOpts) -> Result<Outcome> {
        let mut w = self.writer()?;
        let view = self.lake.engine.view(None)?;
        if let Some(o) = self.replay(&w, &opts.idempotency_key, view.as_ref())? {
            return Ok(o);
        }
        let decl = w.ontology.ty(ty)?;
        rights::not_a_view(decl)?;
        granted(rights::may_retire(self.who, decl))?;
        let before = live_record(view.as_ref(), ty, key)?;
        cas(opts.expected_version, Some(&before))?;
        let now = self.lake.clock(&mut w);
        let mut d = self.draft(now, Origin::Authored, retirement(ty, key, before.version));
        self.keyed(&mut d, &opts.idempotency_key);
        let mut pending = Pending::default();
        let at = position_after(&w, 0);
        let drafts = self.retiring(&w, view.as_ref(), &mut pending, &before, d, at, now)?;
        drop(view);
        let events = self.lake.append(&mut w, drafts)?;
        Ok(outcome_of(&events))
    }

    /// The verb `verb` for its caller: one on a type the caller is not
    /// shown is answered as a verb that is not declared, before its scope
    /// is judged in a sentence that names the type.
    fn shown_verb<'o>(&self, o: &'o Ontology, verb: &VerbName) -> Result<&'o VerbDecl> {
        let v = o.verb(verb)?;
        let missing = Missing::Verb { name: verb.clone() };
        (shown_type(o, &v.on, self.who).map(|_| v)).map_err(|_| Error::NotFound { missing })
    }

    fn verb_callable(&self, verb: &VerbDecl) -> bool {
        match &verb.effect {
            VerbEffect::Write { set } => set
                .iter()
                .all(|a| self.who.grants.access(&verb.on, &a.field).writes()),
            VerbEffect::External { .. } => rights::type_access(self.who, &verb.on).writes(),
        }
    }

    /// Check a verb's payload against its declaration.
    fn verb_payload(&self, w: &Writer, verb: &VerbDecl, payload: &Patch) -> Result<Patch> {
        let decl = w.ontology.ty(&verb.on)?;
        let checked = check_values(&w.ontology, decl, payload, &verb.payload)?;
        for f in verb.payload.iter().filter(|f| f.required) {
            if checked.get(&f.name).is_none_or(Value::is_null) {
                return Err(constraint(
                    Some(f.name.clone()),
                    format!("verb `{}` requires `{}` in its payload", verb.name, f.name),
                ));
            }
        }
        Ok(checked)
    }

    /// The events a verb's call appends when it runs: `verb_called`, and
    /// for a write effect the record write it causes; then, apart, the
    /// triggers and object-set changes that write causes, as for any other
    /// write. `n` drafts precede. `key` is `None` for a verb on its type,
    /// which reads no record and writes none.
    #[allow(clippy::too_many_arguments)]
    fn run_verb(
        &self,
        w: &Writer,
        view: &dyn ProjectionView,
        pending: &mut Pending,
        verb: &VerbDecl,
        key: Option<&RecordKey>,
        payload: Patch,
        expected: Option<Version>,
        now: Timestamp,
        n: usize,
        as_whom: &Principal,
    ) -> Result<(Vec<Draft>, Vec<Draft>)> {
        let called = self.draft(
            now,
            Origin::Authored,
            EventBody::VerbCalled {
                verb: verb.name.clone(),
                key: key.cloned(),
                payload: payload.clone(),
            },
        );
        // A verb on its type is external and declares no `requires`: it
        // reads no record and writes none.
        let Some(key) = key else {
            return Ok((vec![called], Vec::new()));
        };
        let record = live_record(view, &verb.on, key)?;
        // The version before anything is judged on the record (024).
        cas(expected, Some(&record))?;
        requires(w, verb, key, &record, now)?;
        let called_at = position_after(w, n);
        let mut drafts = vec![called];
        let mut reactions = Vec::new();
        if let VerbEffect::Write { set } = &verb.effect {
            let mut patch = Patch::new();
            for a in set {
                let e = declare::condition(&a.value, Place::Record, "assignment")?;
                let v = expr::eval(
                    &e,
                    &VerbScope {
                        record: &record.fields,
                        payload: &payload,
                        settings: &w.settings,
                        now,
                    },
                )
                .map_err(|e| {
                    invalid(format!(
                        "verb `{}` cannot compute `{}`: {e}",
                        verb.name, a.field
                    ))
                })?;
                let kind = w
                    .ontology
                    .ty(&verb.on)?
                    .fields
                    .iter()
                    .find(|f| f.name == a.field)
                    .map(|f| f.ty.kind);
                let json = kind
                    .and_then(|k| expr::eval::to_json(&v, k, None))
                    .ok_or_else(|| {
                        invalid(format!(
                            "verb `{}` computes a value that does not fit `{}`",
                            verb.name, a.field
                        ))
                    })?;
                patch.insert(a.field.clone(), json);
            }
            let mut plan = self.plan_record(
                w,
                view,
                pending,
                &verb.on,
                key,
                &patch,
                Door::Authored,
                expected,
                now,
                n + 1,
                as_whom,
                &|_| {},
            )?;
            plan.draft.cause = Some(called_at);
            let version = w.ontology.verbs.get(&verb.name).map_or(1, |e| e.version);
            plan.draft.procedure = Some(Procedure {
                name: verb.name.to_string(),
                version: version.to_string(),
            });
            drafts.push(plan.draft);
            reactions = self.reactions(
                w,
                view,
                pending,
                plan.before.as_ref(),
                &plan.after,
                position_after(w, n + 1),
                now,
            )?;
        }
        Ok((drafts, reactions))
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
        let decl = self.shown_verb(&w.ontology, verb)?.clone();
        let key = scoped(&decl, key, opts.expected_version)?;
        if key.is_some() {
            rights::not_a_view(w.ontology.ty(&decl.on)?)?;
        }
        granted(self.verb_callable(&decl))?;
        if let VerbEffect::External { executor } = &decl.effect {
            bound_executor(self.lake, executor)?;
        }
        let payload = self.verb_payload(&w, &decl, &payload)?;
        let now = self.lake.clock(&mut w);
        let mut drafts = if decl.risk == Risk::Low {
            let (mut drafts, reactions) = self.run_verb(
                &w,
                view.as_ref(),
                &mut Pending::default(),
                &decl,
                key,
                payload.clone(),
                opts.expected_version,
                now,
                0,
                self.who,
            )?;
            drafts.extend(reactions);
            drafts
        } else {
            // A call that could not run now is not proposed: the version,
            // then `requires`, as when it runs; accepting it judges the
            // record again (013).
            if let Some(key) = key {
                let record = live_record(view.as_ref(), &decl.on, key)?;
                cas(opts.expected_version, Some(&record))?;
                requires(&w, &decl, key, &record, now)?;
            }
            let position = position_after(&w, 0);
            let id = opts.idempotency_key.as_ref().map_or_else(
                || log::positional_id(&self.lake.id, position),
                |k| log::authored_id(&self.who.writer(), k),
            );
            let why = format!(
                "`{verb}` is declared {} risk; it runs when accepted",
                risk_word(decl.risk)
            );
            vec![self.draft(
                now,
                Origin::Authored,
                EventBody::WriteProposed {
                    proposal: ProposalId::from(id.0.as_str()),
                    write: ProposedWrite::Call {
                        verb: verb.clone(),
                        key: key.cloned(),
                        payload: payload.clone(),
                        opts: opts.clone(),
                    },
                    why,
                },
            )]
        };
        self.keyed(&mut drafts[0], &opts.idempotency_key);
        drop(view);
        let due = matches!(decl.effect, VerbEffect::External { .. })
            && matches!(drafts[0].body, EventBody::VerbCalled { .. });
        let appended = self.lake.append(&mut w, drafts);
        let events = appended.map_err(|e| not_handed(e, due && w.behind, false))?;
        let outcome = outcome_of(&events);
        let external = match (&decl.effect, decl.risk, &events[0].body) {
            (VerbEffect::External { executor }, Risk::Low, EventBody::VerbCalled { .. }) => {
                Some(External {
                    executor: executor.clone(),
                    verb: verb.clone(),
                    ty: decl.on.clone(),
                    key: key.cloned(),
                    payload,
                    position: events[0].position,
                })
            }
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
        let now = self.lake.clock(&mut w);
        let key_opt = match &write {
            ProposedWrite::Upsert { opts, .. }
            | ProposedWrite::Retire { opts, .. }
            | ProposedWrite::Call { opts, .. } => opts.idempotency_key.clone(),
            _ => None,
        };
        if let Some(o) = self.replay(&w, &key_opt, view.as_ref())? {
            return Ok(o);
        }
        if let ProposedWrite::Upsert { ty, .. } | ProposedWrite::Retire { ty, .. } = &write {
            ordinary_door(ty)?;
            shown_type(&w.ontology, ty, self.who)?;
        }
        match &mut write {
            ProposedWrite::Upsert {
                ty,
                key,
                patch,
                opts,
            } => {
                let plan = self.plan_record(
                    &w,
                    view.as_ref(),
                    &Pending::default(),
                    ty,
                    key,
                    patch,
                    Door::Authored,
                    opts.expected_version,
                    now,
                    1,
                    self.who,
                    &|_| {},
                )?;
                let EventBody::RecordUpserted { patch: checked, .. } = plan.draft.body else {
                    unreachable!("an authored record write is an upsert")
                };
                *patch = checked;
            }
            ProposedWrite::Retire { ty, key, opts } => {
                let decl = w.ontology.ty(ty)?;
                rights::not_a_view(decl)?;
                granted(rights::may_retire(self.who, decl))?;
                let before = live_record(view.as_ref(), ty, key)?;
                cas(opts.expected_version, Some(&before))?;
            }
            ProposedWrite::Call {
                verb,
                key,
                payload,
                opts,
            } => {
                let decl = self.shown_verb(&w.ontology, verb)?.clone();
                if scoped(&decl, key.as_ref(), opts.expected_version)?.is_some() {
                    rights::not_a_view(w.ontology.ty(&decl.on)?)?;
                }
                granted(self.verb_callable(&decl))?;
                // A call that could not run when accepted is not proposed (013).
                if let VerbEffect::External { executor } = &decl.effect {
                    bound_executor(self.lake, executor)?;
                }
                *payload = self.verb_payload(&w, &decl, payload)?;
                if matches!(decl.effect, VerbEffect::Write { .. }) {
                    self.run_verb(
                        &w,
                        view.as_ref(),
                        &mut Pending::default(),
                        &decl,
                        key.as_ref(),
                        payload.clone(),
                        opts.expected_version,
                        now,
                        1,
                        self.who,
                    )?;
                } else if let Some(key) = key {
                    let record = live_record(view.as_ref(), &decl.on, key)?;
                    cas(opts.expected_version, Some(&record))?;
                    requires(&w, &decl, key, &record, now)?;
                }
            }
            // A change of the ontology is checked now as its door would
            // check it, and nothing changes until it is accepted; the
            // proposer needs no `schema` grant (013, extended).
            ProposedWrite::Declare { declarations } => {
                self.redeclared(&w.ontology, declarations)?;
                self.check_bundle(&w.ontology, declarations)?;
            }
            ProposedWrite::Migrate { migration, report } => {
                self.authored(&w.ontology, migration)?;
                let found = self
                    .preflight_plan(&w, view.as_ref(), migration, None)?
                    .report;
                if !found.lossless {
                    return Err(self.lossy(&w, migration, found));
                }
                **report = found;
            }
            ProposedWrite::MigrateValueType { to, report } => {
                **report = fits(self.preflight_value_type(&w, view.as_ref(), to)?, false)?;
            }
        }
        let position = position_after(&w, 0);
        let id = key_opt.as_ref().map_or_else(
            || log::positional_id(&self.lake.id, position),
            |k| log::authored_id(&self.who.writer(), k),
        );
        let mut d = self.draft(
            now,
            Origin::Authored,
            EventBody::WriteProposed {
                proposal: ProposalId::from(id.0.as_str()),
                write,
                why,
            },
        );
        self.keyed(&mut d, &key_opt);
        drop(view);
        let events = self.lake.append(&mut w, vec![d])?;
        Ok(outcome_of(&events))
    }

    /// `accept`: apply a pending proposal's write, written as the proposer.
    pub fn accept(&self, proposal: &ProposalId) -> Result<(Outcome, Option<External>)> {
        check_context(self.lake, &self.context)?;
        granted(self.who.grants.review)?;
        let mut w = self.writer()?;
        let view = self.lake.engine.view(None)?;
        let p = find_proposal(view.as_ref(), proposal)?;
        if p.state != ProposalState::Pending {
            return Err(refused(Refusal::Decided));
        }
        // A change of the ontology is the lake's `schema` grant's to make:
        // the accepter holds it as well as `review` (013, extended).
        // A stored proposal holds its write; a row without one is a
        // projection the store cannot use.
        let Some(write) = p.write.as_ref() else {
            let what = format!("the projection holds proposal `{proposal}` without its write");
            return Err(lakelands_core::unavailable(what));
        };
        granted(!write.is_schema() || self.who.grants.schema)?;
        let now = self.lake.clock(&mut w);
        // The write was checked under the proposer's grants when it was
        // proposed; it is applied as the proposer, with what was checked.
        let proposer = self.lake.system(p.proposer.clone());
        let accepted = self.draft(
            now,
            Origin::Authored,
            EventBody::WriteAccepted {
                proposal: proposal.clone(),
            },
        );
        let accepted_at = position_after(&w, 0);
        let mut drafts = vec![accepted];
        let as_proposer = |mut d: Draft| {
            d.writer = p.proposer.clone();
            d.cause = d.cause.or(Some(accepted_at));
            d
        };
        let as_accepted = |d: Draft| match d.origin {
            Origin::Authored => as_proposer(d),
            _ => Draft {
                cause: d.cause.or(Some(accepted_at)),
                ..d
            },
        };
        let (mut external, mut renamed) = (None, Vec::new());
        let mut pending = Pending::default();
        match write {
            ProposedWrite::Upsert {
                ty,
                key,
                patch,
                opts,
            } => {
                let plan = self.plan_record(
                    &w,
                    view.as_ref(),
                    &pending,
                    ty,
                    key,
                    patch,
                    Door::Authored,
                    opts.expected_version,
                    now,
                    1,
                    &proposer,
                    &|_| {},
                )?;
                drafts.push(as_proposer(plan.draft));
                drafts.extend(self.reactions(
                    &w,
                    view.as_ref(),
                    &mut pending,
                    plan.before.as_ref(),
                    &plan.after,
                    Position(accepted_at.0 + 1),
                    now,
                )?);
            }
            ProposedWrite::Retire { ty, key, opts } => {
                let before = live_record(view.as_ref(), ty, key)?;
                cas(opts.expected_version, Some(&before))?;
                let body = retirement(ty, key, before.version);
                let d = as_proposer(self.draft(now, Origin::Authored, body));
                let at = Position(accepted_at.0 + 1);
                let own = self.retiring(&w, view.as_ref(), &mut pending, &before, d, at, now)?;
                drafts.extend(own);
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
                if key.is_some() {
                    rights::not_a_view(w.ontology.ty(&decl.on)?)?;
                }
                // The reviewer applies what was checked: a verb declared
                // differently since the call was proposed refuses the
                // accept, and the proposal stays pending (013).
                let then = self.lake.engine.view(Some(p.position))?.declarations()?;
                if !then
                    .iter()
                    .any(|d| matches!(&d.declaration, Declaration::Verb(v) if *v == decl))
                {
                    return Err(invalid(format!(
                        "verb `{verb}` was declared again, differently, after this call was \
                         proposed; propose the call again"
                    )));
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
                let (own, reactions) = self.run_verb(
                    &w,
                    view.as_ref(),
                    &mut pending,
                    &decl,
                    key,
                    payload.clone(),
                    opts.expected_version,
                    now,
                    1,
                    &proposer,
                )?;
                drafts.extend(own.into_iter().map(as_proposer));
                drafts.extend(reactions);
            }
            // Checked again on the head, which may have moved: a change it
            // no longer admits refuses the accept, and the proposal stays
            // pending. The proposer writes what it declares; the link types
            // the store derives stay the system's.
            ProposedWrite::Declare { declarations } => {
                self.check_bundle(&w.ontology, declarations)?;
                let mut changed = declarations.clone();
                changed.retain(|d| !w.ontology.in_force(d));
                let own = self.bundle_drafts(&w, changed, now, drafts.len())?;
                drafts.extend(own.into_iter().map(&as_accepted));
            }
            ProposedWrite::Migrate { migration, report } => {
                // The shape is the type's whole: on a type changed since the
                // proposal it would remove what was added, shown to no one
                // (013, extended).
                let ty = &migration.to.name;
                let head = shape::schema_hash(w.ontology.ty(ty)?).ok();
                let changed = || {
                    let since = "changed since this migration was proposed; propose it again";
                    invalid(format!("`{ty}` {since}"))
                };
                if head.as_ref() != Some(&report.from_schema_hash) {
                    return Err(changed());
                }
                let plan = self.preflight_plan(&w, view.as_ref(), migration, Some(now))?;
                // So would it take away a mark set since, which the hash
                // does not cover: it restates what its report said, no more.
                if plan.report.restated != report.restated {
                    return Err(changed());
                }
                renamed = self.known_renames(&w.ontology, migration);
                let first = drafts.len();
                let own = self.migration_drafts(&w, (**migration).clone(), plan, now, first)?;
                let own = own
                    .into_iter()
                    .filter(|_| !unchanged(&w.ontology, migration));
                drafts.extend(own.map(&as_accepted));
            }
            ProposedWrite::MigrateValueType { to, report } => {
                // As a migration is accepted only on the shape it was
                // proposed on: a value type declared again since would lose
                // what that added, shown to no one (013, extended).
                let head = w.ontology.value_types.get(to.name.as_str());
                if head.map(|v| v.version) != Some(report.from_version) {
                    return Err(invalid(format!(
                        "value type `{}` changed since this was proposed; propose it again",
                        to.name
                    )));
                }
                fits(self.preflight_value_type(&w, view.as_ref(), to)?, false)?;
                let declared = vec![Declaration::ValueType((**to).clone())];
                let own = self.bundle_drafts(&w, declared, now, drafts.len())?;
                drafts.extend(own.into_iter().map(&as_accepted));
            }
        }
        drop(view);
        let appended = self.lake.append(&mut w, drafts);
        let events = appended.map_err(|e| not_handed(e, external.is_some() && w.behind, true))?;
        Ok((self.answered(&events, renamed), external))
    }

    /// The writer, held by this call. Its grants were followed through the
    /// lake's renames before it waited here; where a migration renamed a
    /// field or a relation meanwhile they may no longer hold it, and the
    /// call is refused to be sent again, in words that name no migration
    /// (006, extended).
    pub(crate) fn writer(&self) -> Result<std::sync::MutexGuard<'_, crate::lake::Writer>> {
        let w = self.lake.for_write()?;
        match self.followed {
            Some(n) if self.lake.renames(None)?.len() != n => Err(lakelands_core::unavailable(
                "the lake's names moved while this write waited; send it again",
            )),
            _ => Ok(w),
        }
    }

    /// A migration's renames as the one answered may know of them, taken
    /// before the append, while the type stands as it was.
    fn known_renames(&self, o: &Ontology, m: &Migration) -> Vec<(FieldName, FieldName)> {
        let mut m = m.clone();
        if let Ok(was) = o.ty(&m.to.name) {
            rights::known_migration(self.who, was, &mut m, &[]);
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
    pub fn reject(&self, proposal: &ProposalId, reason: String) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        granted(self.who.grants.review)?;
        let mut w = self.writer()?;
        let view = self.lake.engine.view(None)?;
        let p = find_proposal(view.as_ref(), proposal)?;
        if p.state != ProposalState::Pending {
            return Err(refused(Refusal::Decided));
        }
        let now = self.lake.clock(&mut w);
        let d = self.draft(
            now,
            Origin::Authored,
            EventBody::WriteRejected {
                proposal: proposal.clone(),
                reason,
            },
        );
        drop(view);
        let events = self.lake.append(&mut w, vec![d])?;
        Ok(outcome_of(&events))
    }

    /// A function computed outside the store is declared only where a
    /// backing of its kind is bound; replay never asks.
    fn check_bound(&self, d: &Declaration) -> Result<()> {
        // A trigger's ceiling lies within the lake's (021).
        if let Declaration::Trigger(t) = d
            && let Some(m) = t.max_depth
            && m > self.lake.limits.max_reactive_depth
        {
            return Err(invalid(format!(
                "trigger `{}`: its `max_depth` {m} is above the lake's `max_reactive_depth` {}",
                t.name, self.lake.limits.max_reactive_depth
            )));
        }
        let backings = &self.lake.seams.backings;
        if let Declaration::Function(f) = d
            && crate::backed::is_backed(f)
            && !backings.iter().any(|b| b.kind() == f.backing)
        {
            return Err(Error::Unbound {
                seam: format!(
                    "the `{}` function backing (the host bound none of that kind)",
                    f.backing
                ),
            });
        }
        if let Declaration::Type(t) = d {
            self.check_embeddings(t)?;
            // A relation's fields embed on its link type's records.
            for lt in links::link_types_of(t) {
                self.check_embeddings(&lt)?;
            }
        }
        Ok(())
    }

    /// A type's embeddings are declared only where the bound embedding
    /// backend names their model and makes vectors of their dimensions.
    fn check_embeddings(&self, t: &TypeDecl) -> Result<()> {
        for f in &t.fields {
            let Some(e) = &f.embed else {
                continue;
            };
            let dimensions = f.ty.dimensions.unwrap_or(0);
            let bound = self.lake.seams.embed.as_ref();
            if bound.is_some_and(|b| b.model() == e.model && b.dimensions() == dimensions) {
                continue;
            }
            let found = match bound {
                None => "none is bound".to_owned(),
                Some(b) => format!(
                    "the bound one is `{}` with {} dimensions",
                    b.model(),
                    b.dimensions()
                ),
            };
            return Err(Error::Unbound {
                seam: format!(
                    "the embedding model `{}` with {dimensions} dimensions, for `{}.{}` ({found})",
                    e.model, t.name, f.name
                ),
            });
        }
        Ok(())
    }

    /// `declare`.
    pub fn declare(&self, declaration: Declaration) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        granted(self.who.grants.schema)?;
        self.declare_granted(declaration)
    }

    /// `declare`, its context and the `schema` grant already checked. A
    /// declaration that is the one in force is answered at the head and
    /// appends nothing, after every refusal (027, extended).
    fn declare_granted(&self, mut declaration: Declaration) -> Result<Outcome> {
        let mut w = self.writer()?;
        self.redeclared(&w.ontology, std::slice::from_mut(&mut declaration))?;
        declare::check_declaration(&w.ontology, &declaration, false)?;
        self.check_bound(&declaration)?;
        if w.ontology.in_force(&declaration) {
            return Ok(applied(head(&w), None));
        }
        let now = self.lake.clock(&mut w);
        let drafts = self.bundle_drafts(&w, vec![declaration], now, 0)?;
        let events = self.lake.append(&mut w, drafts)?;
        Ok(outcome_of(&events))
    }

    /// `recompute`: the declaration in force appended again as it was
    /// declared, at its next version, so that the fold does at this event
    /// what it does at any `declared` of it (022, extended). A deliberate
    /// act: nothing is compared, and the log shows it.
    pub fn recompute(&self, what: &Recompute) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        granted(self.who.grants.schema)?;
        let mut w = self.writer()?;
        let declaration = match what {
            Recompute::Function(name) => match w.ontology.functions.get(name) {
                Some(f) => Declaration::Function(f.decl.clone()),
                None => {
                    let missing = Missing::Function { name: name.clone() };
                    return Err(Error::NotFound { missing });
                }
            },
            Recompute::Type(name) => {
                let t = shown_type(&w.ontology, name, self.who)?;
                if t.view.is_some() || !crate::backed::computed_outside(&w.ontology, t) {
                    return Err(invalid(format!(
                        "`{name}` has no field computed outside the lake (an embedding, or a \
                         field of a function with a backing), so nothing of it is computed \
                         again: a derived field is as of its record's last write, and a \
                         function is named itself"
                    )));
                }
                Declaration::Type(w.ontology.as_declared(name)?)
            }
        };
        self.check_bound(&declaration)?;
        let now = self.lake.clock(&mut w);
        let drafts = self.bundle_drafts(&w, vec![declaration], now, 0)?;
        let events = self.lake.append(&mut w, drafts)?;
        Ok(outcome_of(&events))
    }

    /// `apply`: a set of declarations validated together and appended in
    /// one append, or refused whole. Each declaration is checked against
    /// the lake's ontology with every other declaration of the set already
    /// in it, so a reference may point forward within the set and tool
    /// names are checked over all of it; every problem is collected, and
    /// one refusal names them all. Of a set that is admitted, what is in
    /// force before it is not appended again (027, extended).
    pub fn apply(&self, mut declarations: Vec<Declaration>) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        granted(self.who.grants.schema)?;
        let mut w = self.writer()?;
        self.redeclared(&w.ontology, &mut declarations)?;
        self.check_bundle(&w.ontology, &declarations)?;
        declarations.retain(|d| !w.ontology.in_force(d));
        if declarations.is_empty() {
            return Ok(applied(head(&w), None));
        }
        let now = self.lake.clock(&mut w);
        let drafts = self.bundle_drafts(&w, declarations, now, 0)?;
        let events = self.lake.append(&mut w, drafts)?;
        let last = events.last().expect("the set is not empty");
        Ok(applied(last.position, None))
    }

    /// A declaration reads only what its author may read (024 and 028,
    /// extended). The set is checked as sent against the ontology as its
    /// author is shown it, so a field the author may not know of answers as
    /// one the type does not have, whatever reads it. Then a field it knows
    /// of and may not read is refused as not granted, where what its author
    /// adds or changes reads its value.
    fn reads_as_author(&self, o: &Ontology, sent: &[Declaration], reshapes: bool) -> Result<()> {
        // A type its author is not shown is a type that is not declared,
        // for whatever names it; to declare it is not granted, and names
        // nothing of it.
        // The lake's declarations are listed once, and each type is judged
        // against the list.
        let declared = o.declared();
        let shown = |ty: &TypeName| describe::one_type(ty, &declared, self.who).is_some();
        let hidden = |d: &Declaration| matches!(d, Declaration::Type(t) if o.types.contains_key(&t.name) && !shown(&t.name));
        granted(!sent.iter().any(hidden))?;
        let mut seen = o.clone();
        (seen.types).retain(|name, _| shown(name));
        for e in seen.types.values_mut().map(Arc::make_mut) {
            rights::known_fields(self.who, &mut e.decl);
        }
        // What a type's shape names stands as declared, known to its author
        // or not: a key field, a valid time, a relation's `via`. Both sides
        // hold it, at its declared place.
        let mut shaped = sent.to_vec();
        for d in &mut shaped {
            let Declaration::Type(t) = d else { continue };
            let Some(e) = seen.types.get_mut(&t.name).map(Arc::make_mut) else {
                continue;
            };
            let via = t.relations.iter().filter_map(|r| r.via.as_ref());
            let named = t
                .key
                .iter()
                .chain(&t.valid_from)
                .chain(&t.valid_to)
                .chain(via);
            let named: Vec<FieldName> = named.cloned().collect();
            let was = o.ty(&t.name)?.fields.iter().enumerate();
            for (i, f) in was.filter(|(_, f)| named.contains(&f.name)) {
                for fields in [&mut t.fields, &mut e.decl.fields] {
                    if !fields.iter().any(|x| x.name == f.name) {
                        fields.insert(i.min(fields.len()), f.clone());
                    }
                }
            }
        }
        // A migration's shape is refused as a declaration; its preflight is next.
        match self.check_bundle(&seen, &shaped) {
            Err(e) if reshapes => declare::unless_a_migration(e)?,
            checked => checked?,
        }
        let (before, mut after) = (o.declared(), o.clone());
        sent.iter().for_each(|d| after.put(d.clone(), 0, None));
        let key = lakelands_core::engines::declaration_key;
        for d in sent {
            // Only what its author adds or changes: of a type, the fields,
            // the relations and the view that do not stand as they are.
            let mut was = before.iter().map(|b| &b.declaration);
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
            for t in o.types.values().map(|e| &e.decl) {
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
    /// totals and the id, which names the whole report, stay.
    pub fn shown_value_type(&self, o: &Ontology, mut report: ValueTypeReport) -> ValueTypeReport {
        // A caller that reads a field is shown its type.
        let read = |ty: &TypeName, field: &FieldName| {
            let Ok(t) = o.ty(ty) else {
                return false;
            };
            let f = t.fields.iter().find(|f| &f.name == field);
            f.is_some_and(|f| rights::field_access(self.who, t, f).reads())
        };
        report.fields.retain(|u| read(&u.ty, &u.field));
        report.first.retain(|m| read(&m.ty, &m.field));
        report
    }

    /// Reads a migration as its author's view of its type ([`as_author`]).
    fn authored(&self, o: &Ontology, m: &mut Migration) -> Result<()> {
        if o.types.contains_key(&m.to.name) {
            shown_type(o, &m.to.name, self.who)?;
        }
        self.reads_as_author(o, &[Declaration::Type(m.to.clone())], true)?;
        match o.types.get(&m.to.name) {
            Some(before) => {
                let (to, pairs) = (&mut m.to, &m.relation_renames);
                as_author(self.who, &before.decl, to, &m.renames, pairs)
            }
            None => Ok(()),
        }
    }

    /// Reads each type of `declarations` that is declared already as its
    /// author's view of it ([`as_author`]).
    fn redeclared(&self, o: &Ontology, declarations: &mut [Declaration]) -> Result<()> {
        self.reads_as_author(o, declarations, false)?;
        for d in declarations {
            if let Declaration::Type(t) = d
                && let Some(before) = o.types.get(&t.name)
            {
                as_author(self.who, &before.decl, t, &[], &[])?;
            }
        }
        Ok(())
    }

    /// Check a set of declarations as `apply` checks it, against
    /// `ontology`, whoever applies it: each against the ontology with
    /// every other declaration of the set already in it, every problem
    /// collected into one refusal.
    fn check_bundle(&self, ontology: &Ontology, declarations: &[Declaration]) -> Result<()> {
        if declarations.is_empty() {
            return Err(invalid("nothing to apply: the set holds no declaration"));
        }
        let identities: Vec<String> = declarations
            .iter()
            .map(lakelands_core::engines::declaration_key)
            .collect();
        let mut problems: Vec<(String, Error)> = Vec::new();
        for (i, id) in identities.iter().enumerate() {
            if identities[..i].contains(id) {
                problems.push((
                    id.clone(),
                    invalid(format!("the set declares {id} more than once")),
                ));
            }
        }
        for (i, d) in declarations.iter().enumerate() {
            let mut probe = ontology.clone();
            for (j, other) in declarations.iter().enumerate() {
                if identities[j] != identities[i] {
                    probe.put(other.clone(), 0, None);
                }
            }
            if let Err(e) =
                declare::check_declaration(&probe, d, false).and_then(|()| self.check_bound(d))
            {
                problems.push((identities[i].clone(), e));
            }
        }
        match problems.len() {
            0 => Ok(()),
            1 => Err(problems.remove(0).1),
            _ => {
                let all: Vec<String> = problems
                    .iter()
                    .map(|(id, e)| format!("{id}: {e}"))
                    .collect();
                Err(invalid(format!(
                    "the set is refused whole, with {} problems: {}",
                    all.len(),
                    all.join("; ")
                )))
            }
        }
    }

    /// The drafts that declare a checked set: each declaration at its next
    /// version, with the link types its relations need.
    fn bundle_drafts(
        &self,
        w: &Writer,
        declarations: Vec<Declaration>,
        now: Timestamp,
        first: usize,
    ) -> Result<Vec<Draft>> {
        let mut drafts = Vec::with_capacity(declarations.len());
        // A set is decided over the sets it names as the bundle leaves them.
        let (mut after, mut decided) = (w.ontology.clone(), Vec::new());
        let sets = |d: &&Declaration| matches!(d, Declaration::ObjectSet(_));
        for d in declarations.iter().filter(sets) {
            after.put(d.clone(), 0, None);
        }
        for declaration in declarations {
            let declared = position_after(w, first + drafts.len());
            // A type's hash and link types, or the memberships a set decides.
            let (schema_hash, with) = match &declaration {
                Declaration::Type(t) => {
                    let hash = shape::schema_hash(t).map_err(|e| invalid(e.to_string()))?;
                    (Some(hash), link_type_drafts(&w.ontology, t, now)?)
                }
                Declaration::ObjectSet(set) => {
                    let with = (w, &after, &mut decided);
                    (None, self.set_drafts(with, set, declared, now)?)
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
                },
            ));
            drafts.extend(with);
        }
        Ok(drafts)
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
        live(view.as_ref(), ty, |r| {
            for (s, held) in &sets {
                let is = member_of((s, after), r, w, now)?;
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
        })?;
        Ok(out)
    }

    /// `add_field`.
    pub fn add_field(&self, ty: &TypeName, field: FieldDecl) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        granted(self.who.grants.schema)?;
        let mut t = shown_type(&self.writer()?.ontology, ty, self.who)?.clone();
        // The field is added to its author's view of the type (027).
        rights::known_fields(self.who, &mut t);
        if t.fields.iter().any(|f| f.name == field.name) {
            return Err(invalid_field(
                &field.name,
                format!("`{ty}` already has a field `{}`", field.name),
            ));
        }
        t.fields.push(field);
        self.declare_granted(Declaration::Type(t))
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
        granted(self.who.grants.schema)?;
        let mut m = m.clone();
        self.authored(&w.ontology, &mut m)?;
        let report = self.preflight_plan(w, view, &m, None)?.report;
        self.shown(w, &m, report)
    }

    /// A report as the caller may know it (013, 024 and 027, extended):
    /// wherever one is answered or refuses a write, its reader is a reader
    /// like any other, the author of its own preview too. The id stays the
    /// whole report's, so a preview read trimmed is applied by it.
    fn shown(
        &self,
        w: &Writer,
        m: &Migration,
        mut report: MigrationReport,
    ) -> Result<MigrationReport> {
        let (view, declared) = (self.lake.engine.view(None)?, w.ontology.declared());
        let seen = |ty: &TypeName| describe::one_type(ty, &declared, self.who).is_some();
        let sees = |ty: &TypeName| w.ontology.ty(ty).ok().filter(|_| seen(ty));
        let answered = |ty: &TypeName, key: &RecordKey| {
            let stored = view.get(ty, key).ok().flatten();
            self.who.grants.restricted || !stored.is_some_and(|r| r.restricted)
        };
        let (was, mut m) = (w.ontology.ty(&m.to.name)?, m.clone());
        w.ontology.complete(&mut m.to);
        let described = || describe::lake(&self.who.lake, &declared, self.who);
        rights::shown_report(self.who, was, &m, &mut report, described, sees, answered);
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
        m: &Migration,
        now: Option<Timestamp>,
    ) -> Result<MigrationPlan> {
        let before = w.ontology.ty(&m.to.name)?.clone();
        // A migration reshapes stored records, and a view stores none. A
        // view changes, or becomes a stored type, by being declared again;
        // a stored type never becomes a view, since its records would
        // linger beside the computed ones: the view takes another name.
        if before.view.is_some() || m.to.view.is_some() {
            let ty = &m.to.name;
            return Err(invalid(
                match (before.view.is_some(), m.to.view.is_some()) {
                    (true, true) => format!(
                        "`{ty}` is a view, and a view changes by being declared again, never by a \
                     migration"
                    ),
                    (true, false) => format!(
                        "`{ty}` is a view, and it becomes a stored type by being declared as one, \
                     never by a migration"
                    ),
                    _ => format!(
                        "the migration would turn the stored type `{ty}` into a view; declare the view \
                     under another name (`{ty}` keeps its name and its records)"
                    ),
                },
            ));
        }
        if !before.key.is_empty() && m.to.key.is_empty() {
            return Err(invalid(format!(
                "the migration takes every key field of `{}` away, and leaves no rule to make \
                 its records' keys",
                before.name
            )));
        }
        if m.to.key != before.key {
            for k in &m.to.key {
                if !m.to.fields.iter().any(|f| &f.name == k && f.required) {
                    return Err(invalid_field(
                        k,
                        format!(
                            "`{}`: the key field `{k}` is a required field of the type",
                            m.to.name
                        ),
                    ));
                }
            }
        }
        let mut probe = w.ontology.clone();
        probe.put(Declaration::Type(before.clone()), 0, None);
        declare::check_declaration(&probe, &Declaration::Type(m.to.clone()), false)
            .or_else(declare::unless_a_migration)?;
        // A view over the type, as its source or a merged one, reads the
        // shape the migration leaves: `declare` must still take each such
        // view against it, or the view would refuse every read after.
        let mut after = probe.clone();
        after.put(Declaration::Type(m.to.clone()), 0, None);
        // The refusal of a migration that takes away a field `reader` reads:
        // one the type, or a link type of it, has before it and not after.
        let has = |t: &TypeDecl, n: &FieldName| t.fields.iter().any(|f| &f.name == n);
        let gone = |reader: String, names: &BTreeSet<FieldName>, was: &TypeDecl, is: &TypeDecl| {
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
                return Some(invalid(format!(
                    "after the migration of `{}`, a declaration reads a field the migration \
                     takes away",
                    m.to.name
                )));
            }
            let renamed = |(from, _): &&(FieldName, FieldName)| from == n && is.name == m.to.name;
            let how = match m.renames.iter().find(renamed) {
                Some((_, to)) => format!("renames to `{to}`"),
                None => "drops".to_owned(),
            };
            Some(invalid(format!(
                "after the migration of `{}`, {reader} reads `{n}` of `{}`, which the \
                 migration {how}",
                m.to.name, is.name
            )))
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
            let taken = |reader: String| {
                let mut of = reads.iter().filter(|(source, _)| source == &m.to.name);
                of.find_map(|(_, names)| gone(reader.clone(), names, &before, &m.to))
            };
            let checked = declare::check_declaration(&after, &view, false).map_err(|e| match e {
                Error::Refused {
                    field,
                    reason: Refusal::Invalid { message },
                } => Error::Refused {
                    field,
                    reason: Refusal::Invalid {
                        message: format!("after the migration of `{}`, {message}", m.to.name),
                    },
                },
                other => other,
            });
            let named =
                checked.and_then(|()| taken(format!("view `{}`", reader.name)).map_or(Ok(()), Err));
            // The view is named only to an author `describe` shows its
            // definition to (027, extended); another is told that a
            // declaration reads the field, or that one could not be declared.
            let Err(named) = named else { continue };
            if describe::shown(&view, &w.ontology.declared(), self.who) {
                return Err(named);
            }
            return Err(taken("a declaration".to_owned()).unwrap_or_else(|| {
                invalid(format!(
                    "after the migration of `{0}`, a declaration over `{0}` could no longer be \
                     declared",
                    m.to.name
                ))
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
                let reads = describe::declaration_reads(&d.declaration, &was.name, &|_| None);
                let Some(named) = gone(d.declaration.what(), &reads, was, is) else {
                    continue;
                };
                // The reader is named only to an author `describe` shows it
                // to, as it stands in force; another is told that a
                // declaration reads the field. The field is one the author
                // knows of: one hidden from it was put back (`as_author`).
                if describe::shown(&d.declaration, &w.ontology.declared(), self.who) {
                    return Err(named);
                }
                gone("a declaration".to_owned(), &reads, was, is).map_or(Ok(()), Err)?;
            }
        }
        // And the type it migrates to reads only fields it has: `declare`'s
        // check above ends at the reshaping, before it comes to the reads.
        declare::reads_its_type(&after, &Declaration::Type(m.to.clone()))?;
        // A record that does not convert is already not lossless; its
        // stored fields still tell whether it has a new key.
        let convert = |decl: &TypeDecl, m: &Migration, r: &StoredRecord| {
            let fields = shape::migrate_record(decl, m, &r.fields);
            let fields = fields.map_err(|e| invalid(e.to_string()))?;
            Ok::<_, Error>(fields.unwrap_or_else(|| r.fields.clone()))
        };
        // Applied, the migration decides again every record whose fields it
        // changes, as the fold leaves it (004, extended): converted, its
        // references to moved keys rewritten, under its key after `moves`.
        let mut members = Vec::new();
        let limit = self.lake.limits.max_set_changes as usize;
        let left = |moves: &crate::rekey::Moves,
                    sets: &Held<'_>,
                    decl: &TypeDecl,
                    r: &StoredRecord,
                    fields,
                    now,
                    members: &mut Vec<Change>| {
            let moved = moves.get(&r.ty).and_then(|m| m.get(&r.key));
            let is = StoredRecord {
                key: moved.unwrap_or(&r.key).clone(),
                fields: crate::rekey::rewritten(decl, &fields, moves),
                ..r.clone()
            };
            decide(sets, &r.key, &is, (w, now, limit), members)
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
        let mut report = shaped(view, &before, m, |r| {
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
        let plan = crate::rekey::plan(&after, view, &m.to.name, keyed, &m.renames)?;
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
        for r in m.to.relations.iter().filter(|r| links::carried_by_links(r)) {
            let Some(old) = before
                .relations
                .iter()
                .find(|o| o.name == r.name && links::carried_by_links(o))
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
                return Err(constraint(
                    None,
                    format!(
                        "narrowing `{}` to {:?} would break the links of {broken} record(s) \
                             that hold more than one; unlink them first",
                        r.name, r.cardinality
                    ),
                ));
            }
        }
        // The relations carried by link records whose fields change migrate
        // their link types with the type: their records are checked here,
        // each field named with its relation.
        for (old, new) in changed_link_types(&w.ontology, m, false) {
            let lm = Migration {
                to: new,
                renames: vec![],
                relation_renames: Vec::new(),
            };
            let sets = held(&old.name)?;
            let lr = shaped(view, &old, &lm, |r| {
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
        let pairs = m.renamed_relations(&before).map_err(invalid)?;
        let removed = |r: &&RelationDecl| {
            !m.to.relations.iter().any(|n| n.name == r.name)
                && !pairs.iter().any(|(was, _)| was.name == r.name)
        };
        for r in before.relations.iter().filter(removed) {
            let lt = links::link_type_name(&m.to.name, &r.name);
            let live = match links::carried_by_links(r) {
                true => view.count(&lt, &every(false))?,
                false => 0,
            };
            let name = FieldName::from(r.name.as_str());
            report.dropped.push((name, live));
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
                let wrong = |why: &str| {
                    let named = format!("renaming relation `{}` to `{}`", was.name, is.name);
                    invalid(format!("{named}: {why}"))
                };
                if !plan.moves.is_empty() {
                    return Err(wrong(
                        "the same migration re-keys records; rename in one migration and \
                         re-key in another",
                    ));
                }
                if stored(view, &links::link_type_name(&m.to.name, &is.name))? > 0 {
                    return Err(wrong(
                        "the lake keeps the links of an earlier relation of that name; take \
                         another name",
                    ));
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
                    let declared = w.ontology.declared();
                    let shown = |t: &&TypeDecl| describe::one_type(&t.name, &declared, self.who);
                    let named = referring.find(|t| shown(t).is_some());
                    let who = named.map_or("a declaration".into(), |t| format!("`{}`", t.name));
                    return Err(wrong(&format!(
                        "{who} refers to its link type `{old}`, whose records would move; \
                         retarget it first"
                    )));
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
        // Named by what it says, the moves it would make and where the lake
        // stood: an apply after the lake moved is refused.
        let listed = plan.listed();
        report.report_id = String::new();
        report.report_id = lakelands_core::canonical::sha256_hex(&serde_json::json!({
            "report": serde_json::to_value(&report).expect("a report serialises"),
            "rekeyed": serde_json::to_value(&listed).expect("moves serialise"),
            "head": view.position().map_or(0, |p| p.0),
        }));
        // A record of another type whose reference to a moved key the fold
        // rewrites: decided for the sets whose filter reads that field, a
        // page of the type at a time.
        for (name, decl) in w.ontology.types.iter().map(|(n, e)| (n, &e.decl)) {
            let rewritten = crate::rekey::moved_references(decl, &plan.moves);
            let reads = |s: &ObjectSetDecl| {
                let sets = |n: &str| w.ontology.set(n);
                let filter = declare::filter(&sets, &s.of, &s.filter, Place::Record, "object set");
                let reads = filter.map(|e| lakelands_core::derive::reads(&e));
                reads.is_ok_and(|r| rewritten.iter().any(|(f, _)| r.contains(*f)))
            };
            let other = name != &m.to.name && !links.iter().any(|(old, _)| &old.name == name);
            let Some(now) = now.filter(|_| other && !rewritten.is_empty()) else {
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
            rekeyed: listed,
            members,
        })
    }

    /// `migrate_apply`. A migration to the type in force that renames
    /// nothing is answered at the head and appends nothing, after every
    /// refusal, a stale report among them (027, extended).
    pub fn migrate_apply(&self, mut m: Migration, report_id: &str) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        granted(self.who.grants.schema)?;
        let mut w = self.writer()?;
        self.authored(&w.ontology, &mut m)?;
        let view = self.lake.engine.view(None)?;
        let now = self.lake.clock(&mut w);
        let plan = self.preflight_plan(&w, view.as_ref(), &m, Some(now))?;
        if plan.report.report_id != report_id {
            return Err(invalid(
                "the lake moved since that preflight report; run the preflight again",
            ));
        }
        let renamed = self.known_renames(&w.ontology, &m);
        let same = unchanged(&w.ontology, &m);
        let drafts = self.migration_drafts(&w, m, plan, now, 0)?;
        if same {
            return Ok(applied(head(&w), None));
        }
        drop(view);
        let events = self.lake.append(&mut w, drafts)?;
        Ok(self.answered(&events, renamed))
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
        self.check_embeddings(&m.to)?;
        let report_id = report.report_id;
        let schema_hash = shape::schema_hash(&m.to).map_err(|e| invalid(e.to_string()))?;
        let version = w.ontology.next_version(&Declaration::Type(m.to.clone()));
        let migrated = position_after(w, first);
        let changes = members.into_iter().map(|c| self.changed(c, migrated, now));
        let mut drafts: Vec<Draft> = changes.collect();
        for (_, new) in changed_link_types(&w.ontology, &m, false) {
            let hash = shape::schema_hash(&new).map_err(|e| invalid(e.to_string()))?;
            let version = w.ontology.next_version(&Declaration::Type(new.clone()));
            drafts.push(Draft::system(
                now,
                EventBody::Migrated {
                    migration: Migration {
                        to: new,
                        renames: vec![],
                        relation_renames: Vec::new(),
                    },
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
        Ok(drafts)
    }
}

impl Call<'_> {
    /// The preflight of a value type's new declaration: every value stored
    /// under it, in every type and field that uses it, retired and
    /// restricted records included, checked as a write would check it. It
    /// asks for no grant: its door asks for `schema`, a proposal for none.
    pub fn preflight_value_type(
        &self,
        w: &Writer,
        view: &dyn ProjectionView,
        to: &ValueTypeDecl,
    ) -> Result<ValueTypeReport> {
        let current = w
            .ontology
            .value_types
            .get(to.name.as_str())
            .ok_or_else(|| invalid(format!("`{}` is not a declared value type", to.name)))?;
        match declare::value_type_change(&current.decl, to) {
            declare::ValueTypeChange::Same => {
                return Err(invalid(format!(
                    "value type `{}` would accept exactly what it does; there is nothing to \
                     migrate",
                    to.name
                )));
            }
            declare::ValueTypeChange::Reshaping => {
                return Err(invalid(format!(
                    "value type `{}` would change its `kind` or `scale`, which rewrites stored \
                     values; that is not a value-type migration: migrate the fields that use it",
                    to.name
                )));
            }
            _ => {}
        }
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
        let mut report = ValueTypeReport {
            value_type: to.name.clone(),
            from_version: current.version,
            records_scanned: 0,
            fields: Vec::new(),
            misfits: 0,
            first: Vec::new(),
            fits: true,
            report_id: String::new(),
        };
        for (name, entry) in &probe.types {
            let decl = &entry.decl;
            let uses: Vec<&FieldDecl> = decl
                .fields
                .iter()
                .filter(|f| f.ty.value_type.as_ref() == Some(&to.name))
                .collect();
            if uses.is_empty() || decl.view.is_some() {
                continue;
            }
            // One walk of the type, a page at a time. Each field keeps its
            // tally and its first misfits, as many as the report still has
            // room for; they are joined in the fields' order afterwards, so
            // the report lists a field's misfits before the next field's.
            let room = 10 - report.first.len();
            let tally = |f: &&FieldDecl| ValueTypeUse {
                ty: name.clone(),
                field: f.name.clone(),
                values_scanned: 0,
                misfits: 0,
            };
            let mut used: Vec<(ValueTypeUse, Vec<Misfit>)> =
                uses.iter().map(|f| (tally(f), Vec::new())).collect();
            crate::lake::walk(view, name, true, |r| {
                report.records_scanned += 1;
                for (f, (used, first)) in uses.iter().zip(&mut used) {
                    let Some(v) = r.fields.get(&f.name).filter(|v| !v.is_null()) else {
                        continue;
                    };
                    used.values_scanned += 1;
                    let patch: Patch = [(f.name.clone(), v.clone())].into();
                    if check_values(&probe, decl, &patch, &decl.fields).is_err() {
                        used.misfits += 1;
                        if first.len() < room {
                            first.push(Misfit {
                                ty: name.clone(),
                                key: r.key.clone(),
                                field: f.name.clone(),
                            });
                        }
                    }
                }
                Ok(())
            })?;
            for (used, first) in used {
                let room = 10 - report.first.len();
                report.first.extend(first.into_iter().take(room));
                report.misfits += used.misfits;
                report.fields.push(used);
            }
        }
        report.fits = report.misfits == 0;
        // Named by what it says and where the lake stood: an apply after
        // the lake moved is refused.
        let head = view.position().map_or(0, |p| p.0);
        report.report_id = lakelands_core::canonical::sha256_hex(&serde_json::json!({
            "report": serde_json::to_value(&report).expect("a report serialises"),
            "to": serde_json::to_value(to).expect("a declaration serialises"),
            "head": head,
        }));
        Ok(report)
    }

    /// Put a tightened value type in force against the report the caller
    /// was shown: one `declared` at the next version, no record changed.
    pub fn migrate_value_type_apply(&self, to: ValueTypeDecl, report_id: &str) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        granted(self.who.grants.schema)?;
        let mut w = self.writer()?;
        let view = self.lake.engine.view(None)?;
        let report = self.preflight_value_type(&w, view.as_ref(), &to)?;
        if report.report_id != report_id {
            return Err(invalid(
                "the lake moved since that preflight report; run the preflight again",
            ));
        }
        fits(self.shown_value_type(&w.ontology, report), true)?;
        let now = self.lake.clock(&mut w);
        let drafts = self.bundle_drafts(&w, vec![Declaration::ValueType(to)], now, 0)?;
        drop(view);
        let events = self.lake.append(&mut w, drafts)?;
        Ok(outcome_of(&events))
    }
}

/// A value type's preflight report, refused while a stored value would not
/// fit: with how many, and, for a caller with the `schema` grant
/// (`named`), the first of them. A proposal is told the count alone, so no
/// record is named in a field its proposer may not know of.
fn fits(report: ValueTypeReport, named: bool) -> Result<ValueTypeReport> {
    if report.fits {
        return Ok(report);
    }
    let first = (report.first.iter().filter(|_| named))
        .map(|m| format!("`{}` `{}`.`{}`", m.ty, m.key, m.field))
        .collect::<Vec<_>>();
    let first = match named {
        true => format!(" First: {}", first.join(", ")),
        false => String::new(),
    };
    Err(invalid(format!(
        "value type `{}` is not tightened: {} stored value(s) would not fit it; change them \
         first.{first}",
        report.value_type, report.misfits
    )))
}

/// Reads `to`, a migration's or a redeclaration's shape of the type
/// declared as `before`, as its author's view of the type (027, extended):
/// a field `who` may not know of was not its to leave out, so it is put
/// back as declared, at its declared place, and so are such link fields of
/// each relation `to` keeps or renames. No refusal names one: a rename from it is from
/// no field, and a field it is built from is refused on that field.
fn as_author(
    who: &Principal,
    before: &TypeDecl,
    to: &mut TypeDecl,
    renames: &[(FieldName, FieldName)],
    relations: &[(RelationName, RelationName)],
) -> Result<()> {
    let hidden = |t: &TypeDecl, f: &FieldDecl| rights::field_access(who, t, f) == Access::None;
    let put_back = |t: &TypeDecl, declared: &[FieldDecl], fields: &mut Vec<FieldDecl>| {
        for (i, f) in declared.iter().enumerate().filter(|(_, f)| hidden(t, f)) {
            fields.insert(i.min(fields.len()), f.clone());
        }
    };
    let unknown: Vec<&FieldDecl> = (before.fields.iter().filter(|f| hidden(before, f))).collect();
    let renamed = |f: &&&FieldDecl| renames.iter().any(|(from, _)| from == &f.name);
    if let Some(f) = unknown.iter().find(renamed) {
        let from = f.name.clone();
        return Err(invalid(
            shape::ShapeError::RenameFromUnknown { from }.to_string(),
        ));
    }
    put_back(before, &before.fields, &mut to.fields);
    for r in &mut to.relations {
        // A renamed relation is the one it was, under its old name.
        let was = relations.iter().find(|(_, new)| new == &r.name);
        let name = was.map_or(&r.name, |(old, _)| old);
        if let Some(old) = before.relations.iter().find(|o| &o.name == name) {
            let link = links::link_type(&before.name, old);
            put_back(&link, &old.fields, &mut r.fields);
        }
    }
    let mut reads = (unknown.iter().copied())
        .flat_map(|f| (f.depends_on.iter()).chain(f.embed.iter().flat_map(|e| &e.from)));
    match reads.find(|n| !to.fields.iter().any(|t| &t.name == *n)) {
        Some(n) => {
            let ty = &to.name;
            let why = format!("`{n}` is read by another field of `{ty}` and cannot be removed");
            Err(invalid_field(n, why))
        }
        None => Ok(()),
    }
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

/// The system's `declared` of a declaration, at its next version.
fn system_declared(o: &Ontology, declaration: Declaration, now: Timestamp) -> Result<Draft> {
    let schema_hash = match &declaration {
        Declaration::Type(t) => Some(shape::schema_hash(t).map_err(|e| invalid(e.to_string()))?),
        _ => None,
    };
    let version = o.next_version(&declaration);
    let body = EventBody::Declared {
        declaration,
        schema_hash,
        version,
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

/// The relation carried by link records that `ty` declares as `relation`,
/// with its link type.
fn link_relation(
    w: &Writer,
    ty: &TypeName,
    relation: &RelationName,
) -> Result<(TypeName, RelationDecl)> {
    let decl = w.ontology.ty(ty)?;
    rights::not_a_view(decl)?;
    let r = decl
        .relations
        .iter()
        .find(|r| &r.name == relation && links::carried_by_links(r))
        .ok_or_else(|| {
            invalid(format!(
                "`{ty}` has no relation `{relation}` carried by link records"
            ))
        })?;
    let lt = links::link_type_name(ty, relation);
    w.ontology.ty(&lt)?;
    Ok((lt, r.clone()))
}

/// The relation's cardinality over live links, with the link from `from`
/// to `to` among them: `many_to_one` and `one_to_one` allow one live link
/// per `from`, `one_to_many` and `one_to_one` one per `to`. Answered from
/// the links the engine walks at the head, not by reading every link.
pub fn cardinality(
    view: &dyn ProjectionView,
    ty: &TypeName,
    r: &RelationDecl,
    from: &RecordKey,
    to: &RecordKey,
) -> Result<()> {
    use lakelands_api::query::Walk;
    let (per_from, per_to) = one_per(r.cardinality);
    let broken = |why: String| {
        constraint(
            Some(links::TO.into()),
            format!(
                "{why}, and `{}` allows one ({:?}); unlink it first",
                r.name, r.cardinality
            ),
        )
    };
    if per_from
        && let Some(e) = view
            .edges(ty, std::slice::from_ref(from), Some(&r.name), Walk::Out)?
            .remove(0)
            .into_iter()
            .find(|e| e.to.1 != *to)
    {
        return Err(broken(format!(
            "`{from}` already links over `{}` to `{}`",
            r.name, e.to.1
        )));
    }
    if per_to
        && let Some(e) = view
            .edges(&r.target, std::slice::from_ref(to), Some(&r.name), Walk::In)?
            .remove(0)
            .into_iter()
            .find(|e| e.from.0 == *ty && e.from.1 != *from)
    {
        return Err(broken(format!(
            "`{to}` is already linked over `{}` from `{}`",
            r.name, e.from.1
        )));
    }
    Ok(())
}

/// The relation's cardinality among the links of one ingest window that
/// came before: `(link type, from, to)` each.
pub fn cardinality_among(
    earlier: &[(TypeName, RecordKey, RecordKey)],
    lt: &TypeName,
    r: &RelationDecl,
    from: &RecordKey,
    to: &RecordKey,
) -> Result<()> {
    let (per_from, per_to) = one_per(r.cardinality);
    for (_, f, o) in earlier.iter().filter(|(t, _, _)| t == lt) {
        if f == from && o == to {
            continue;
        }
        if (per_from && f == from) || (per_to && o == to) {
            return Err(constraint(
                Some(links::TO.into()),
                format!(
                    "the window links `{f}` to `{o}` over `{}` already, and `{}` allows one \
                         ({:?})",
                    r.name, r.name, r.cardinality
                ),
            ));
        }
    }
    Ok(())
}

/// The answer to a call of an external verb, or to its acceptance when
/// `accepted`, that failed where `not_run` holds: its append is in the
/// log and not folded, or the drain before its hand-out failed. The
/// store's sentence for the failure, and that the verb's executor was not
/// handed the call. Sent again under its key, the call is answered its
/// position and runs nothing, and a proposal accepted again is decided;
/// the host calls the verb again, which is a new position.
pub(crate) fn not_handed(e: Error, not_run: bool, accepted: bool) -> Error {
    let again = match accepted {
        true => "the proposal is decided, so accepting it again",
        false => "sending it again under its key",
    };
    match e {
        Error::Unavailable { message } if not_run => Error::Unavailable {
            message: format!(
                "{message}; the verb's executor was not handed this call, and {again} will \
                 not run it: call the verb again"
            ),
        },
        e => e,
    }
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

/// Record an executor's answer, as `output_recorded`.
pub fn record_output(
    lake: &Lake,
    ext: &External,
    input: &Value,
    output: Value,
) -> Result<Position> {
    let mut w = lake.for_write()?;
    let now = lake.clock(&mut w);
    let d = Draft {
        cause: Some(ext.position),
        ..Draft::system(
            now,
            EventBody::OutputRecorded {
                seam: format!("verb:{}:{}", ext.executor, ext.verb),
                input: Digest(lakelands_core::canonical::sha256_hex(input)),
                output,
                field: None,
            },
        )
    };
    let events = lake.append(&mut w, vec![d])?;
    Ok(events[0].position)
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
    let before = live_record(view.as_ref(), ty, key)?;
    let version = Version(before.version.0 + 1);
    let body = match (&reason, before.restricted) {
        (Some(_), true) => return Err(invalid(format!("`{ty}` `{key}` is already restricted"))),
        (None, false) => return Err(invalid(format!("`{ty}` `{key}` is not restricted"))),
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
