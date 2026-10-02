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

use std::collections::BTreeMap;

use lakelands_api::engine::ProjectionView;
use lakelands_api::event::{Digest, Event, EventBody, Procedure};
use lakelands_api::ontology::{
    Authority, RelationDecl, Risk, TypeDecl, ValueTypeDecl, VerbDecl, VerbEffect,
};
use lakelands_api::principal::WriteContext;
use lakelands_api::record::{Origin, StoredRecord};
use lakelands_api::write::{
    Declaration, Migration, MigrationReport, Misfit, Outcome, ProposalState, ProposedWrite,
    ValueTypeReport, ValueTypeUse, WriteOpts,
};
use lakelands_api::{
    Error, FieldName, IdempotencyKey, Missing, Patch, Position, Principal, ProposalId, RecordKey,
    Refusal, RelationName, Result, Timestamp, TypeName, Value, VerbName, Version,
};
use lakelands_core::expr::{self, Place, RecordScope, Scope, WriteScope};
use lakelands_core::rights::{self, Door};
use lakelands_core::{invalid, links, shape, value};

use crate::backed::Outputs;
use crate::declare::{self, invalid_field};
use crate::lake::{AGING_RULE_TYPE, Lake, SETTING_TYPE, Writer, every, find_proposal};
use crate::log::{self, Draft};
use crate::record;

/// How many events after an external verb's call a retry reads for the
/// output recorded for it.
const RETRY_REACH: u32 = 4_096;

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
fn constraint(field: Option<FieldName>, message: impl Into<String>) -> Error {
    Error::Refused {
        field,
        reason: Refusal::Constraint {
            message: message.into(),
        },
    }
}

/// Refuse as not granted unless `ok`.
pub(crate) fn granted(ok: bool) -> Result<()> {
    if ok {
        return Ok(());
    }
    Err(Error::Refused {
        field: None,
        reason: Refusal::NotGranted,
    })
}

/// Refuse a write whose context the lake does not take: deeper than its
/// reactive ceiling, whatever the origin names
/// (`docs/decisions/021-the-write-context-belongs-to-the-host.md`). Every
/// write path calls it before it takes the writer, so a refused context
/// waits on no other write; a host's `cause` is checked against the head
/// where the write appends ([`crate::lake::Lake::append_after`]).
pub fn check_context(lake: &Lake, context: &WriteContext) -> Result<()> {
    let max = lake.limits.max_reactive_depth;
    if context.depth > max {
        return Err(Error::Refused {
            field: None,
            reason: Refusal::TooDeep { max },
        });
    }
    // A reaction that names a trigger's firing as its cause is bound by
    // that trigger's own ceiling too, as the head declares it (021). Any
    // other cause binds nothing more.
    let Some(cause) = context.cause else {
        return Ok(());
    };
    let Some(stored) = lake.engine.read(cause, 1)?.into_iter().next() else {
        return Ok(());
    };
    if let EventBody::TriggerFired { trigger, .. } = crate::log::open(&stored)?.body
        && let Some(m) = lake
            .writer()
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

/// The write-side context of one call.
pub struct Call<'a> {
    /// The lake.
    pub lake: &'a Lake,
    /// Who the call is made as.
    pub who: &'a Principal,
    /// The host's context.
    pub context: WriteContext,
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
    /// Refuse a verb whose executor the host has not bound.
    fn check_executor(&self, executor: &str) -> Result<()> {
        if self
            .lake
            .seams
            .executors
            .iter()
            .any(|e| e.name() == executor)
        {
            return Ok(());
        }
        Err(Error::Unbound {
            seam: format!("the verb executor `{executor}`"),
        })
    }

    fn draft(&self, now: Timestamp, origin: Origin, body: EventBody) -> Draft {
        Draft {
            upstream: None,
            known_at: now,
            writer: self.who.writer(),
            origin,
            source: None,
            seen_at: None,
            procedure: None,
            context: self.context,
            cause: None,
            idempotency_key: None,
            id: None,
            body,
        }
    }

    /// The first write's outcome, if this key was used before.
    fn replay(
        &self,
        key: &Option<IdempotencyKey>,
        view: &dyn ProjectionView,
    ) -> Result<Option<Outcome>> {
        let Some(k) = key else { return Ok(None) };
        let id = log::authored_id(&self.who.writer(), k);
        let Some(pos) = view.applied(&id)? else {
            return Ok(None);
        };
        let events = self.lake.read_events(pos, 3)?;
        if events.is_empty() {
            return Ok(None);
        }
        let outcome = outcome_of(&events);
        // An external verb answered with its executor's output, recorded in
        // a later append that other writes may precede: a retry answers
        // that output too, at the verb's own position.
        let external = events.iter().any(|e| {
            e.position == outcome.position() && matches!(e.body, EventBody::VerbCalled { .. })
        });
        match (outcome, external) {
            (
                Outcome::Applied {
                    position, version, ..
                },
                true,
            ) => Ok(Some(Outcome::Applied {
                position,
                version,
                output: self.recorded_output(position)?,
            })),
            (outcome, _) => Ok(Some(outcome)),
        }
    }

    /// The output recorded for the external verb called at `called`: the
    /// `output_recorded` that event caused, among the [`RETRY_REACH`]
    /// events after it. Its `input` digest holds the record as the executor
    /// was given it, which a retry cannot rebuild to look it up by digest;
    /// the read is bounded so that a retry never reads to the head of a
    /// large lake (`docs/seams.md` §`VerbExecutor`).
    fn recorded_output(&self, called: Position) -> Result<Option<Value>> {
        let events = self.lake.read_events(called.next(), RETRY_REACH)?;
        Ok(events.into_iter().find_map(|e| match e.body {
            EventBody::OutputRecorded { seam, output, .. }
                if e.cause == Some(called) && seam.starts_with("verb:") =>
            {
                Some(output)
            }
            _ => None,
        }))
    }
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
fn outcome_of(events: &[Event]) -> Outcome {
    let e = &events[0];
    let wrote = match (&e.body, events.get(1)) {
        // An acceptance always appends something after `write_accepted`.
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
        | EventBody::RecordRetired { version, .. } => Outcome::Applied {
            position: e.position,
            version: Some(*version),
            output: None,
        },
        EventBody::WriteProposed { proposal, why, .. } => Outcome::Proposed {
            position: e.position,
            proposal: proposal.clone(),
            why: why.clone(),
        },
        EventBody::VerbCalled { .. } | EventBody::WriteAccepted { .. } if wrote => {
            outcome_of(&events[1..])
        }
        _ => Outcome::Applied {
            position: e.position,
            version: None,
            output: None,
        },
    }
}

/// Check and normalise the values a patch carries against their fields.
pub fn check_values(
    lake_types: &crate::ontology::Ontology,
    decl: &TypeDecl,
    patch: &Patch,
    fields: &[lakelands_api::ontology::FieldDecl],
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

fn check_key(key: &RecordKey) -> Result<()> {
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

/// The next position in an append that already holds `n` drafts.
pub(crate) fn position_after(w: &Writer, n: usize) -> Position {
    let first = w.tip.as_ref().map_or(Position::GENESIS, |t| t.0.next());
    Position(first.0 + n as u64)
}

/// The record a retire of `before` makes, computed from its provisional
/// event.
pub(crate) fn retired(
    w: &Writer,
    view: &dyn ProjectionView,
    before: &StoredRecord,
    event: &Event,
) -> Result<StoredRecord> {
    let outputs = &mut Outputs {
        view,
        staged: None,
        due: None,
    };
    Ok(
        record::apply(&w.ontology, Some(before), event, &w.settings, outputs)?
            .expect("a retire makes a record"),
    )
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
    Err(Error::Refused {
        field: None,
        reason: Refusal::Precondition {
            message: format!(
                "verb `{}` requires what `{}` `{key}` does not hold",
                verb.name, verb.on
            ),
        },
    })
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
        match path {
            [p, name] if p.as_str() == "payload" => {
                self.payload.get(name).filter(|v| !v.is_null()).cloned()
            }
            _ => RecordScope {
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

impl Call<'_> {
    /// Decide one record write: authored (`upsert`, a verb) or from a source
    /// (`ingest`). `n` drafts precede it in the append.
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
    ) -> Result<RecordPlan> {
        self.plan_record_as(
            w,
            view,
            pending,
            ty,
            key,
            patch,
            door,
            expected,
            now,
            n,
            who,
            &|_| {},
        )
    }

    /// [`Self::plan_record`], with `stamp` setting the envelope of the
    /// record event before the record it makes is computed: aging writes
    /// as the system, with its rule as the procedure.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn plan_record_as(
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
        let (origin, body) = match door {
            Door::Authored => (
                Origin::Authored,
                EventBody::RecordUpserted {
                    ty: ty.clone(),
                    key: key.clone(),
                    version,
                    patch,
                },
            ),
            Door::Source => (
                Origin::Source,
                EventBody::RecordIngested {
                    ty: ty.clone(),
                    key: key.clone(),
                    version,
                    patch,
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
            let decl = lakelands_api::seam::index_setting_type(key.as_str())
                .and_then(|t| w.ontology.ty(&TypeName::from(t)).ok());
            lakelands_api::seam::check_index_setting(key.as_str(), after.fields.get("value"), decl)
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
                    let e = declare::condition(c, Place::Guard, "trigger")?;
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
            let is = !after.retired && {
                let e = declare::condition(&s.filter, Place::Record, "object set")?;
                expr::holds(
                    &e,
                    &RecordScope {
                        fields: &after.fields,
                        settings: Some(&w.settings),
                        now,
                    },
                )
                .unwrap_or(false)
            };
            if was != is {
                pending
                    .members
                    .insert((s.name.clone(), after.ty.clone(), after.key.clone()), is);
                out.push(system(EventBody::ObjectSetChanged {
                    set: s.name.clone(),
                    ty: after.ty.clone(),
                    key: after.key.clone(),
                    joined: is,
                }));
            }
        }
        pending
            .records
            .insert((after.ty.clone(), after.key.clone()), after.clone());
        Ok(out)
    }

    /// `upsert`.
    pub fn upsert(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        patch: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        ordinary_door(ty)?;
        self.write_record(ty, key, patch, opts, |_, _| Ok(()))
    }

    /// An authored write of one record, as `upsert` and `link` make it;
    /// `check` refuses it, under the writer's lock, after the record's own
    /// checks: it reads other records, so the grants come first (024).
    fn write_record(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        patch: Patch,
        opts: WriteOpts,
        check: impl FnOnce(&Writer, &dyn ProjectionView) -> Result<()>,
    ) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        let mut w = self.lake.writer();
        let view = self.lake.engine.view(None)?;
        if let Some(o) = self.replay(&opts.idempotency_key, view.as_ref())? {
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
        )?;
        check(&w, view.as_ref())?;
        if let Some(k) = &opts.idempotency_key {
            plan.draft.id = Some(log::authored_id(&self.who.writer(), k));
            plan.draft.idempotency_key = Some(k.clone());
        }
        let cause = position_after(&w, 0);
        let mut drafts = vec![plan.draft];
        drafts.extend(self.reactions(
            &w,
            view.as_ref(),
            &mut pending,
            plan.before.as_ref(),
            &plan.after,
            cause,
            now,
        )?);
        drop(view);
        let events = self.lake.append(&mut w, drafts)?;
        Ok(outcome_of(&events))
    }

    /// `retire`.
    pub fn retire(&self, ty: &TypeName, key: &RecordKey, opts: WriteOpts) -> Result<Outcome> {
        ordinary_door(ty)?;
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
        let (lt, r) = link_relation(&self.lake.writer(), ty, relation)?;
        if r.authority != Authority::Authored {
            return Err(Error::Refused {
                field: None,
                reason: Refusal::WrongDoor,
            });
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
        let (lt, _) = link_relation(&self.lake.writer(), ty, relation)?;
        self.retire_record(&lt, &links::link_key(key, to), opts)
    }

    /// A retirement of one record, as `retire` and `unlink` make it.
    fn retire_record(&self, ty: &TypeName, key: &RecordKey, opts: WriteOpts) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        let mut w = self.lake.writer();
        let view = self.lake.engine.view(None)?;
        if let Some(o) = self.replay(&opts.idempotency_key, view.as_ref())? {
            return Ok(o);
        }
        let decl = w.ontology.ty(ty)?;
        granted(rights::may_retire(self.who, decl))?;
        let before = live_record(view.as_ref(), ty, key)?;
        cas(opts.expected_version, Some(&before))?;
        let now = self.lake.clock(&mut w);
        let mut d = self.draft(
            now,
            Origin::Authored,
            EventBody::RecordRetired {
                ty: ty.clone(),
                key: key.clone(),
                version: Version(before.version.0 + 1),
            },
        );
        if let Some(k) = &opts.idempotency_key {
            d.id = Some(log::authored_id(&self.who.writer(), k));
            d.idempotency_key = Some(k.clone());
        }
        let event = provisional(self.lake, position_after(&w, 0), &d);
        let after = retired(&w, view.as_ref(), &before, &event)?;
        let mut pending = Pending::default();
        let cause = event.position;
        let mut drafts = vec![d];
        drafts.extend(self.reactions(
            &w,
            view.as_ref(),
            &mut pending,
            Some(&before),
            &after,
            cause,
            now,
        )?);
        drop(view);
        let events = self.lake.append(&mut w, drafts)?;
        Ok(outcome_of(&events))
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
        let mut w = self.lake.writer();
        let view = self.lake.engine.view(None)?;
        if let Some(o) = self.replay(&opts.idempotency_key, view.as_ref())? {
            return Ok((o, None));
        }
        let decl = w.ontology.verb(verb)?.clone();
        let key = scoped(&decl, key, opts.expected_version)?;
        granted(self.verb_callable(&decl))?;
        if let VerbEffect::External { executor } = &decl.effect {
            self.check_executor(executor)?;
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
        if let Some(k) = &opts.idempotency_key {
            drafts[0].id = Some(log::authored_id(&self.who.writer(), k));
            drafts[0].idempotency_key = Some(k.clone());
        }
        drop(view);
        let events = self.lake.append(&mut w, drafts)?;
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
    pub fn propose(&self, write: ProposedWrite, why: String) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        let mut w = self.lake.writer();
        let view = self.lake.engine.view(None)?;
        let now = self.lake.clock(&mut w);
        let key_opt = match &write {
            ProposedWrite::Upsert { opts, .. }
            | ProposedWrite::Retire { opts, .. }
            | ProposedWrite::Call { opts, .. } => opts.idempotency_key.clone(),
            ProposedWrite::Declare { .. } | ProposedWrite::Migrate { .. } => None,
        };
        if let Some(o) = self.replay(&key_opt, view.as_ref())? {
            return Ok(o);
        }
        if let ProposedWrite::Upsert { ty, .. } | ProposedWrite::Retire { ty, .. } = &write {
            ordinary_door(ty)?;
        }
        let write = match write {
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
                    &ty,
                    &key,
                    &patch,
                    Door::Authored,
                    opts.expected_version,
                    now,
                    1,
                    self.who,
                )?;
                let EventBody::RecordUpserted { patch, .. } = plan.draft.body else {
                    unreachable!("an authored record write is an upsert")
                };
                ProposedWrite::Upsert {
                    ty,
                    key,
                    patch,
                    opts,
                }
            }
            ProposedWrite::Retire { ty, key, opts } => {
                granted(rights::may_retire(self.who, w.ontology.ty(&ty)?))?;
                let before = live_record(view.as_ref(), &ty, &key)?;
                cas(opts.expected_version, Some(&before))?;
                ProposedWrite::Retire { ty, key, opts }
            }
            ProposedWrite::Call {
                verb,
                key,
                payload,
                opts,
            } => {
                let decl = w.ontology.verb(&verb)?.clone();
                scoped(&decl, key.as_ref(), opts.expected_version)?;
                granted(self.verb_callable(&decl))?;
                // A call that could not run when accepted is not proposed (013).
                if let VerbEffect::External { executor } = &decl.effect {
                    self.check_executor(executor)?;
                }
                let payload = self.verb_payload(&w, &decl, &payload)?;
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
                } else if let Some(key) = &key {
                    let record = live_record(view.as_ref(), &decl.on, key)?;
                    cas(opts.expected_version, Some(&record))?;
                    requires(&w, &decl, key, &record, now)?;
                }
                ProposedWrite::Call {
                    verb,
                    key,
                    payload,
                    opts,
                }
            }
            // A change of the ontology is checked now as its door would
            // check it, and nothing changes until it is accepted; the
            // proposer needs no `schema` grant (013, extended).
            ProposedWrite::Declare { declarations } => {
                self.check_bundle(&w.ontology, &declarations)?;
                ProposedWrite::Declare { declarations }
            }
            ProposedWrite::Migrate { migration, .. } => {
                let (report, _) = self.preflight_plan(&w, view.as_ref(), &migration)?;
                if !report.lossless {
                    return Err(Error::Refused {
                        field: None,
                        reason: Refusal::Migration {
                            report: Box::new(report),
                        },
                    });
                }
                ProposedWrite::Migrate {
                    migration,
                    report: Box::new(report),
                }
            }
        };
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
        if let Some(k) = key_opt {
            d.id = Some(id);
            d.idempotency_key = Some(k);
        }
        drop(view);
        let events = self.lake.append(&mut w, vec![d])?;
        Ok(outcome_of(&events))
    }

    /// `accept`: apply a pending proposal's write, written as the proposer.
    pub fn accept(&self, proposal: &ProposalId) -> Result<(Outcome, Option<External>)> {
        check_context(self.lake, &self.context)?;
        granted(self.who.grants.review)?;
        let mut w = self.lake.writer();
        let view = self.lake.engine.view(None)?;
        let p = find_proposal(view.as_ref(), proposal)?;
        if p.state != ProposalState::Pending {
            return Err(Error::Refused {
                field: None,
                reason: Refusal::Decided,
            });
        }
        // A change of the ontology is the lake's `schema` grant's to make:
        // the accepter holds it as well as `review` (013, extended).
        granted(!p.write.is_schema() || self.who.grants.schema)?;
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
        let mut external = None;
        let mut pending = Pending::default();
        match &p.write {
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
                let d = as_proposer(self.draft(
                    now,
                    Origin::Authored,
                    EventBody::RecordRetired {
                        ty: ty.clone(),
                        key: key.clone(),
                        version: Version(before.version.0 + 1),
                    },
                ));
                let event = provisional(self.lake, Position(accepted_at.0 + 1), &d);
                let after = retired(&w, view.as_ref(), &before, &event)?;
                drafts.push(d);
                drafts.extend(self.reactions(
                    &w,
                    view.as_ref(),
                    &mut pending,
                    Some(&before),
                    &after,
                    event.position,
                    now,
                )?);
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
                if let VerbEffect::External { executor } = &decl.effect {
                    self.check_executor(executor)?;
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
                let own = self.bundle_drafts(&w, declarations.clone(), now)?;
                drafts.extend(own.into_iter().map(&as_accepted));
            }
            ProposedWrite::Migrate { migration, .. } => {
                let (report, rekeyed) = self.preflight_plan(&w, view.as_ref(), migration)?;
                let own = self.migration_drafts(&w, (**migration).clone(), report, rekeyed, now)?;
                drafts.extend(own.into_iter().map(&as_accepted));
            }
        }
        drop(view);
        let events = self.lake.append(&mut w, drafts)?;
        Ok((outcome_of(&events), external))
    }

    /// `reject`.
    pub fn reject(&self, proposal: &ProposalId, reason: String) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        granted(self.who.grants.review)?;
        let mut w = self.lake.writer();
        let view = self.lake.engine.view(None)?;
        let p = find_proposal(view.as_ref(), proposal)?;
        if p.state != ProposalState::Pending {
            return Err(Error::Refused {
                field: None,
                reason: Refusal::Decided,
            });
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
        let mut w = self.lake.writer();
        declare::check_declaration(&w.ontology, &declaration, false)?;
        self.check_bound(&declaration)?;
        let schema_hash = match &declaration {
            Declaration::Type(t) => {
                Some(shape::schema_hash(t).map_err(|e| invalid(e.to_string()))?)
            }
            _ => None,
        };
        let version = w.ontology.next_version(&declaration);
        let now = self.lake.clock(&mut w);
        let links = match &declaration {
            Declaration::Type(t) => link_type_drafts(&w.ontology, t, now)?,
            _ => Vec::new(),
        };
        let d = self.draft(
            now,
            Origin::Authored,
            EventBody::Declared {
                declaration,
                schema_hash,
                version,
            },
        );
        let mut drafts = vec![d];
        drafts.extend(links);
        let events = self.lake.append(&mut w, drafts)?;
        Ok(outcome_of(&events))
    }

    /// `apply`: a set of declarations validated together and appended in
    /// one append, or refused whole. Each declaration is checked against
    /// the lake's ontology with every other declaration of the set already
    /// in it, so a reference may point forward within the set and tool
    /// names are checked over all of it; every problem is collected, and
    /// one refusal names them all.
    pub fn apply(&self, declarations: Vec<Declaration>) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        granted(self.who.grants.schema)?;
        let mut w = self.lake.writer();
        self.check_bundle(&w.ontology, &declarations)?;
        let now = self.lake.clock(&mut w);
        let drafts = self.bundle_drafts(&w, declarations, now)?;
        let events = self.lake.append(&mut w, drafts)?;
        Ok(Outcome::Applied {
            position: events.last().expect("the set is not empty").position,
            version: None,
            output: None,
        })
    }

    /// Check a set of declarations as `apply` checks it, against
    /// `ontology`, whoever applies it: each against the ontology with
    /// every other declaration of the set already in it, every problem
    /// collected into one refusal.
    fn check_bundle(
        &self,
        ontology: &crate::ontology::Ontology,
        declarations: &[Declaration],
    ) -> Result<()> {
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
    ) -> Result<Vec<Draft>> {
        let mut drafts = Vec::with_capacity(declarations.len());
        for declaration in declarations {
            let schema_hash = match &declaration {
                Declaration::Type(t) => {
                    Some(shape::schema_hash(t).map_err(|e| invalid(e.to_string()))?)
                }
                _ => None,
            };
            let version = w.ontology.next_version(&declaration);
            let links = match &declaration {
                Declaration::Type(t) => link_type_drafts(&w.ontology, t, now)?,
                _ => Vec::new(),
            };
            drafts.push(self.draft(
                now,
                Origin::Authored,
                EventBody::Declared {
                    declaration,
                    schema_hash,
                    version,
                },
            ));
            drafts.extend(links);
        }
        Ok(drafts)
    }

    /// `add_field`.
    pub fn add_field(
        &self,
        ty: &TypeName,
        field: lakelands_api::ontology::FieldDecl,
    ) -> Result<Outcome> {
        granted(self.who.grants.schema)?;
        let mut t = self.lake.writer().ontology.ty(ty)?.clone();
        if t.fields.iter().any(|f| f.name == field.name) {
            return Err(invalid_field(
                &field.name,
                format!("`{ty}` already has a field `{}`", field.name),
            ));
        }
        t.fields.push(field);
        self.declare(Declaration::Type(t))
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
        Ok(self.preflight_plan(w, view, m)?.0)
    }

    /// The preflight, and the record keys the migration moves.
    fn preflight_plan(
        &self,
        w: &Writer,
        view: &dyn ProjectionView,
        m: &Migration,
    ) -> Result<(MigrationReport, Vec<lakelands_api::write::Rekey>)> {
        let before = w.ontology.ty(&m.to.name)?.clone();
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
        probe.types.remove(&m.to.name);
        probe.put(Declaration::Type(before.clone()), 0, None);
        declare::check_declaration(&probe, &Declaration::Type(m.to.clone()), false).or_else(
            |e| match &e {
                Error::Refused {
                    reason: Refusal::Invalid { message },
                    ..
                } if message.contains("that is a migration") => Ok(()),
                _ => Err(e),
            },
        )?;
        let records: Vec<StoredRecord> = view
            .scan(&before.name, &every(true))?
            .into_iter()
            .filter(|r| !r.is_ended())
            .collect();
        let mut report = shape::preflight(
            &before,
            m,
            records.iter().map(|r| (r.key.clone(), r.fields.clone())),
        )
        .map_err(|e| invalid(e.to_string()))?;
        // A key the new key fields make differently moves the record, and
        // what names it moves with it (`crate::rekey`).
        let mut converted = Vec::new();
        if !m.to.key.is_empty() {
            for r in &records {
                // A record that does not convert is already not lossless;
                // its stored fields still tell whether it has a new key.
                let fields = shape::migrate_record(&before, m, &r.fields)
                    .map_err(|e| invalid(e.to_string()))?
                    .unwrap_or_else(|| r.fields.clone());
                converted.push((r.key.clone(), fields));
            }
        }
        let mut after = w.ontology.clone();
        after.put(Declaration::Type(m.to.clone()), 0, None);
        let plan = crate::rekey::plan(&after, view, &m.to.name, converted)?;
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
            let live = view.scan(&lt, &every(false))?;
            let mut from_seen: BTreeMap<String, u32> = BTreeMap::new();
            let mut to_seen: BTreeMap<String, u32> = BTreeMap::new();
            for l in &live {
                let end = |f: &str| {
                    l.fields
                        .get(f)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned()
                };
                *from_seen.entry(end(links::FROM)).or_default() += 1;
                *to_seen.entry(end(links::TO)).or_default() += 1;
            }
            let over = |seen: &BTreeMap<String, u32>| seen.values().filter(|n| **n > 1).count();
            let broken = if per_from { over(&from_seen) } else { 0 }
                + if per_to { over(&to_seen) } else { 0 };
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
        for (old, new) in changed_link_types(&w.ontology, &m.to) {
            let lm = Migration {
                to: new,
                renames: vec![],
            };
            let links: Vec<StoredRecord> = view
                .scan(&old.name, &every(true))?
                .into_iter()
                .filter(|r| !r.is_ended())
                .collect();
            let lr = shape::preflight(
                &old,
                &lm,
                links.iter().map(|r| (r.key.clone(), r.fields.clone())),
            )
            .map_err(|e| invalid(e.to_string()))?;
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
        Ok((report, listed))
    }

    /// `migrate_apply`.
    pub fn migrate_apply(&self, m: Migration, report_id: &str) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        granted(self.who.grants.schema)?;
        let mut w = self.lake.writer();
        let view = self.lake.engine.view(None)?;
        let (report, rekeyed) = self.preflight_plan(&w, view.as_ref(), &m)?;
        if report.report_id != report_id {
            return Err(invalid(
                "the lake moved since that preflight report; run the preflight again",
            ));
        }
        let now = self.lake.clock(&mut w);
        let drafts = self.migration_drafts(&w, m, report, rekeyed, now)?;
        drop(view);
        let events = self.lake.append(&mut w, drafts)?;
        Ok(outcome_of(&events))
    }

    /// The drafts that apply a migration its preflight answered `report`
    /// for, refused with the report when it is not lossless: the
    /// `migrated` event first, then the link types it changes or adds.
    fn migration_drafts(
        &self,
        w: &Writer,
        m: Migration,
        report: MigrationReport,
        rekeyed: Vec<lakelands_api::write::Rekey>,
        now: Timestamp,
    ) -> Result<Vec<Draft>> {
        if !report.lossless {
            return Err(Error::Refused {
                field: None,
                reason: Refusal::Migration {
                    report: Box::new(report),
                },
            });
        }
        self.check_embeddings(&m.to)?;
        let report_id = report.report_id;
        let schema_hash = shape::schema_hash(&m.to).map_err(|e| invalid(e.to_string()))?;
        let version = w.ontology.next_version(&Declaration::Type(m.to.clone()));
        let mut drafts = Vec::new();
        for (_, new) in changed_link_types(&w.ontology, &m.to) {
            let hash = shape::schema_hash(&new).map_err(|e| invalid(e.to_string()))?;
            let version = w.ontology.next_version(&Declaration::Type(new.clone()));
            drafts.push(Draft::system(
                now,
                EventBody::Migrated {
                    migration: Migration {
                        to: new,
                        renames: vec![],
                    },
                    schema_hash: hash,
                    version,
                    report_id: report_id.clone(),
                    rekeyed: Vec::new(),
                },
            ));
        }
        // A relation new with this migration gets its link type declared.
        let declared = link_type_drafts(&w.ontology, &m.to, now)?
            .into_iter()
            .filter(|d| {
                matches!(&d.body, EventBody::Declared { declaration: Declaration::Type(t), .. }
                    if !w.ontology.types.contains_key(&t.name))
            });
        drafts.extend(declared);
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
    /// restricted records included, checked as a write would check it.
    pub fn preflight_value_type(
        &self,
        w: &Writer,
        view: &dyn ProjectionView,
        to: &ValueTypeDecl,
    ) -> Result<ValueTypeReport> {
        granted(self.who.grants.schema)?;
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
        let all = every(true);
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
            let uses: Vec<&lakelands_api::ontology::FieldDecl> = decl
                .fields
                .iter()
                .filter(|f| f.ty.value_type.as_ref() == Some(&to.name))
                .collect();
            if uses.is_empty() || decl.view.is_some() {
                continue;
            }
            let records: Vec<StoredRecord> = view
                .scan(name, &all)?
                .into_iter()
                .filter(|r| !r.is_ended())
                .collect();
            report.records_scanned += records.len() as u64;
            for f in uses {
                let mut used = ValueTypeUse {
                    ty: name.clone(),
                    field: f.name.clone(),
                    values_scanned: 0,
                    misfits: 0,
                };
                for r in &records {
                    let Some(v) = r.fields.get(&f.name).filter(|v| !v.is_null()) else {
                        continue;
                    };
                    used.values_scanned += 1;
                    let patch: Patch = [(f.name.clone(), v.clone())].into();
                    if check_values(&probe, decl, &patch, &decl.fields).is_err() {
                        used.misfits += 1;
                        if report.first.len() < 10 {
                            report.first.push(Misfit {
                                ty: name.clone(),
                                key: r.key.clone(),
                                field: f.name.clone(),
                            });
                        }
                    }
                }
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
        let mut w = self.lake.writer();
        let view = self.lake.engine.view(None)?;
        let report = self.preflight_value_type(&w, view.as_ref(), &to)?;
        if report.report_id != report_id {
            return Err(invalid(
                "the lake moved since that preflight report; run the preflight again",
            ));
        }
        if !report.fits {
            let first: Vec<String> = report
                .first
                .iter()
                .map(|m| format!("`{}` `{}`.`{}`", m.ty, m.key, m.field))
                .collect();
            return Err(invalid(format!(
                "value type `{}` is not tightened: {} stored value(s) would not fit it; change \
                 them first. First: {}",
                to.name,
                report.misfits,
                first.join(", ")
            )));
        }
        let declaration = Declaration::ValueType(to);
        let version = w.ontology.next_version(&declaration);
        let now = self.lake.clock(&mut w);
        let d = self.draft(
            now,
            Origin::Authored,
            EventBody::Declared {
                declaration,
                schema_hash: None,
                version,
            },
        );
        drop(view);
        let events = self.lake.append(&mut w, vec![d])?;
        Ok(outcome_of(&events))
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
fn link_type_drafts(
    o: &crate::ontology::Ontology,
    t: &TypeDecl,
    now: Timestamp,
) -> Result<Vec<Draft>> {
    let mut out = Vec::new();
    for lt in links::link_types_of(t) {
        if o.types.get(&lt.name).map(|e| &e.decl) == Some(&lt) {
            continue;
        }
        let hash = shape::schema_hash(&lt).map_err(|e| invalid(e.to_string()))?;
        let version = o.next_version(&Declaration::Type(lt.clone()));
        out.push(Draft::system(
            now,
            EventBody::Declared {
                declaration: Declaration::Type(lt),
                schema_hash: Some(hash),
                version,
            },
        ));
    }
    Ok(out)
}

/// The link types in force whose derivation from `to` differs: each with
/// the declaration in force and the one `to` calls for.
fn changed_link_types(o: &crate::ontology::Ontology, to: &TypeDecl) -> Vec<(TypeDecl, TypeDecl)> {
    links::link_types_of(to)
        .into_iter()
        .filter_map(|new| {
            let old = &o.types.get(&new.name)?.decl;
            (old != &new).then(|| (old.clone(), new))
        })
        .collect()
}

/// Refuse a link type's records at a door other than the link doors.
fn ordinary_door(ty: &TypeName) -> Result<()> {
    if links::is_link_type(ty) {
        return Err(Error::Refused {
            field: None,
            reason: Refusal::WrongDoor,
        });
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
    let mut w = lake.writer();
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
    let mut w = lake.writer();
    w.ontology.ty(ty)?;
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
