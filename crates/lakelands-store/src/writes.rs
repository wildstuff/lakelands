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

use lakelands_api::engine::{ProjectionView, Scan};
use lakelands_api::event::{Digest, Event, EventBody, Procedure};
use lakelands_api::ontology::{Risk, TypeDecl, VerbDecl, VerbEffect};
use lakelands_api::principal::{Grants, WriteContext};
use lakelands_api::record::{Origin, StoredRecord};
use lakelands_api::write::{
    Declaration, FieldConversion, Migration, MigrationReport, Outcome, ProposalState,
    ProposedWrite, WriteOpts,
};
use lakelands_api::{
    Error, FieldName, IdempotencyKey, Missing, Patch, Position, Principal, ProposalId, RecordKey,
    Refusal, Result, Timestamp, TypeName, Value, VerbName, Version,
};
use lakelands_core::expr::{self, Place, RecordScope, Scope, WriteScope};
use lakelands_core::rights::{self, Door};
use lakelands_core::{shape, value};

use crate::backed::Outputs;
use crate::declare;
use crate::lake::{Lake, Writer, find_proposal};
use crate::log::{self, Draft};
use crate::record;

fn invalid(message: impl Into<String>) -> Error {
    Error::Refused {
        field: None,
        reason: Refusal::Invalid {
            message: message.into(),
        },
    }
}

fn not_granted() -> Error {
    Error::Refused {
        field: None,
        reason: Refusal::NotGranted,
    }
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
            None => view.get(ty, key),
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
    fn writer(&self) -> String {
        self.who.writer()
    }

    fn draft(&self, now: Timestamp, origin: Origin, body: EventBody) -> Draft {
        Draft {
            known_at: now,
            writer: self.writer(),
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

    fn check_depth(&self) -> Result<()> {
        let max = self.lake.limits.max_reactive_depth;
        if self.context.depth > max {
            return Err(Error::Refused {
                field: None,
                reason: Refusal::TooDeep { max },
            });
        }
        Ok(())
    }

    /// The first write's outcome, if this key was used before.
    fn replay(
        &self,
        key: &Option<IdempotencyKey>,
        view: &dyn ProjectionView,
    ) -> Result<Option<Outcome>> {
        let Some(k) = key else { return Ok(None) };
        let id = log::authored_id(&self.writer(), k);
        let Some(pos) = view.applied(&id)? else {
            return Ok(None);
        };
        let events = self.lake.read_events(pos, Some(3))?;
        if events.is_empty() {
            return Ok(None);
        }
        Ok(Some(outcome_of(&events)))
    }
}

/// The outcome the first of an append's events stands for. A verb call or
/// an acceptance answers what it caused: the write it made, followed along
/// the causes (an accepted verb proposal: `write_accepted` causes
/// `verb_called`, which causes the record write), so the outcome names the
/// position the write is visible from and the record's new version.
fn outcome_of(events: &[Event]) -> Outcome {
    let e = &events[0];
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
        EventBody::VerbCalled { .. } | EventBody::WriteAccepted { .. } => {
            let caused = events.iter().skip(1).position(|n| {
                n.cause == Some(e.position)
                    && !matches!(
                        n.body,
                        EventBody::TriggerFired { .. } | EventBody::ObjectSetChanged { .. }
                    )
            });
            match caused {
                Some(i) => outcome_of(&events[i + 1..]),
                None => Outcome::Applied {
                    position: e.position,
                    version: None,
                    output: None,
                },
            }
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
            .ok_or_else(|| Error::Refused {
                field: Some(name.clone()),
                reason: Refusal::Invalid {
                    message: format!("`{}` has no field `{name}`", decl.name),
                },
            })?;
        let vt =
            f.ty.value_type
                .as_ref()
                .and_then(|n| lake_types.value_types.get(n.as_str()))
                .map(|e| &e.decl);
        let checked = value::check(v, &f.ty, vt).map_err(|e| Error::Refused {
            field: Some(name.clone()),
            reason: Refusal::Constraint {
                message: e.to_string(),
            },
        })?;
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
                return Err(Error::Refused {
                    field: Some(name.clone()),
                    reason: Refusal::Constraint {
                        message: format!(
                            "the value does not satisfy the constraint of `{}`",
                            vt.name
                        ),
                    },
                });
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
            return Err(Error::Refused {
                field: Some(name.clone()),
                reason: Refusal::Constraint {
                    message: format!(
                        "`{name}` names `{target}` `{to}`, which is not a live record"
                    ),
                },
            });
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

/// The next position in an append that already holds `n` drafts.
fn position_after(w: &Writer, n: usize) -> Position {
    let first = w.tip.as_ref().map_or(Position::GENESIS, |t| t.0.next());
    Position(first.0 + n as u64)
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
        let draft = self.draft(now, origin, body);
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
                    let e = declare::condition(c, Place::Write, "trigger")?;
                    let scope = WriteScope {
                        after: &after.fields,
                        before: before.filter(|b| !b.retired).map(|b| &b.fields),
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
        self.check_depth()?;
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
        if let Some(k) = &opts.idempotency_key {
            plan.draft.id = Some(log::authored_id(&self.writer(), k));
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
        self.check_depth()?;
        let mut w = self.lake.writer();
        let view = self.lake.engine.view(None)?;
        if let Some(o) = self.replay(&opts.idempotency_key, view.as_ref())? {
            return Ok(o);
        }
        let decl = w.ontology.ty(ty)?;
        if !rights::may_retire(self.who, decl) {
            return Err(not_granted());
        }
        let before = view
            .get(ty, key)?
            .filter(|r| !r.retired)
            .ok_or_else(|| Error::NotFound {
                missing: Missing::Record {
                    ty: ty.clone(),
                    key: key.clone(),
                },
            })?;
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
            d.id = Some(log::authored_id(&self.writer(), k));
            d.idempotency_key = Some(k.clone());
        }
        let event = provisional(self.lake, position_after(&w, 0), &d);
        let after = record::apply(
            &w.ontology,
            Some(&before),
            &event,
            &w.settings,
            &mut Outputs {
                view: view.as_ref(),
                staged: None,
                due: None,
            },
        )?
        .expect("a retire makes a record");
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
                return Err(Error::Refused {
                    field: Some(f.name.clone()),
                    reason: Refusal::Constraint {
                        message: format!(
                            "verb `{}` requires `{}` in its payload",
                            verb.name, f.name
                        ),
                    },
                });
            }
        }
        Ok(checked)
    }

    /// The events a verb's call appends when it runs: `verb_called`, and
    /// for a write effect the record write it causes; then, apart, the
    /// triggers and object-set changes that write causes, as for any other
    /// write. `n` drafts precede.
    #[allow(clippy::too_many_arguments)]
    fn run_verb(
        &self,
        w: &Writer,
        view: &dyn ProjectionView,
        pending: &mut Pending,
        verb: &VerbDecl,
        key: &RecordKey,
        payload: Patch,
        expected: Option<Version>,
        now: Timestamp,
        n: usize,
        as_whom: &Principal,
    ) -> Result<(Vec<Draft>, Vec<Draft>)> {
        let record = view
            .get(&verb.on, key)?
            .filter(|r| !r.retired)
            .ok_or_else(|| Error::NotFound {
                missing: Missing::Record {
                    ty: verb.on.clone(),
                    key: key.clone(),
                },
            })?;
        if let Some(r) = &verb.requires {
            let e = declare::condition(r, Place::Record, "requires")?;
            if !expr::holds(
                &e,
                &RecordScope {
                    fields: &record.fields,
                    settings: Some(&w.settings),
                    now,
                },
            )
            .unwrap_or(false)
            {
                return Err(Error::Refused {
                    field: None,
                    reason: Refusal::Precondition {
                        message: format!(
                            "verb `{}` requires what `{}` `{key}` does not hold",
                            verb.name, verb.on
                        ),
                    },
                });
            }
        }
        let called = self.draft(
            now,
            Origin::Authored,
            EventBody::VerbCalled {
                verb: verb.name.clone(),
                key: key.clone(),
                payload: payload.clone(),
            },
        );
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
        key: &RecordKey,
        payload: Patch,
        opts: WriteOpts,
    ) -> Result<(Outcome, Option<External>)> {
        self.check_depth()?;
        let mut w = self.lake.writer();
        let view = self.lake.engine.view(None)?;
        if let Some(o) = self.replay(&opts.idempotency_key, view.as_ref())? {
            return Ok((o, None));
        }
        let decl = w
            .ontology
            .verbs
            .get(verb)
            .map(|e| e.decl.clone())
            .ok_or_else(|| Error::NotFound {
                missing: Missing::Verb { name: verb.clone() },
            })?;
        if !self.verb_callable(&decl) {
            return Err(not_granted());
        }
        if let VerbEffect::External { executor } = &decl.effect
            && !self
                .lake
                .seams
                .executors
                .iter()
                .any(|e| e.name() == executor)
        {
            return Err(Error::Unbound {
                seam: format!("the verb executor `{executor}`"),
            });
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
            view.get(&decl.on, key)?
                .filter(|r| !r.retired)
                .ok_or_else(|| Error::NotFound {
                    missing: Missing::Record {
                        ty: decl.on.clone(),
                        key: key.clone(),
                    },
                })?;
            let position = position_after(&w, 0);
            let id = opts.idempotency_key.as_ref().map_or_else(
                || log::positional_id(&self.lake.id, position),
                |k| log::authored_id(&self.writer(), k),
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
                        key: key.clone(),
                        payload: payload.clone(),
                        opts: opts.clone(),
                    },
                    why,
                },
            )]
        };
        if let Some(k) = &opts.idempotency_key {
            drafts[0].id = Some(log::authored_id(&self.writer(), k));
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
                    key: key.clone(),
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
        self.check_depth()?;
        let mut w = self.lake.writer();
        let view = self.lake.engine.view(None)?;
        let now = self.lake.clock(&mut w);
        let key_opt = match &write {
            ProposedWrite::Upsert { opts, .. }
            | ProposedWrite::Retire { opts, .. }
            | ProposedWrite::Call { opts, .. } => opts.idempotency_key.clone(),
        };
        if let Some(o) = self.replay(&key_opt, view.as_ref())? {
            return Ok(o);
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
                if !rights::may_retire(self.who, w.ontology.ty(&ty)?) {
                    return Err(not_granted());
                }
                let before =
                    view.get(&ty, &key)?
                        .filter(|r| !r.retired)
                        .ok_or_else(|| Error::NotFound {
                            missing: Missing::Record {
                                ty: ty.clone(),
                                key: key.clone(),
                            },
                        })?;
                cas(opts.expected_version, Some(&before))?;
                ProposedWrite::Retire { ty, key, opts }
            }
            ProposedWrite::Call {
                verb,
                key,
                payload,
                opts,
            } => {
                let decl = w
                    .ontology
                    .verbs
                    .get(&verb)
                    .map(|e| e.decl.clone())
                    .ok_or_else(|| Error::NotFound {
                        missing: Missing::Verb { name: verb.clone() },
                    })?;
                if !self.verb_callable(&decl) {
                    return Err(not_granted());
                }
                let payload = self.verb_payload(&w, &decl, &payload)?;
                if matches!(decl.effect, VerbEffect::Write { .. }) {
                    self.run_verb(
                        &w,
                        view.as_ref(),
                        &mut Pending::default(),
                        &decl,
                        &key,
                        payload.clone(),
                        opts.expected_version,
                        now,
                        1,
                        self.who,
                    )?;
                }
                ProposedWrite::Call {
                    verb,
                    key,
                    payload,
                    opts,
                }
            }
        };
        let position = position_after(&w, 0);
        let id = key_opt.as_ref().map_or_else(
            || log::positional_id(&self.lake.id, position),
            |k| log::authored_id(&self.writer(), k),
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
        if !self.who.grants.review {
            return Err(not_granted());
        }
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
        // The write was checked under the proposer's grants when it was
        // proposed; it is applied as the proposer, with what was checked.
        let proposer = Principal {
            kind: lakelands_api::PrincipalKind::System,
            id: p.proposer.clone(),
            lake: self.lake.id.clone(),
            grants: Grants::owner(),
        };
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
                let before =
                    view.get(ty, key)?
                        .filter(|r| !r.retired)
                        .ok_or_else(|| Error::NotFound {
                            missing: Missing::Record {
                                ty: ty.clone(),
                                key: key.clone(),
                            },
                        })?;
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
                let after = record::apply(
                    &w.ontology,
                    Some(&before),
                    &event,
                    &w.settings,
                    &mut Outputs {
                        view: view.as_ref(),
                        staged: None,
                        due: None,
                    },
                )?
                .expect("a retire makes a record");
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
                let decl = w
                    .ontology
                    .verbs
                    .get(verb)
                    .map(|e| e.decl.clone())
                    .ok_or_else(|| Error::NotFound {
                        missing: Missing::Verb { name: verb.clone() },
                    })?;
                if let VerbEffect::External { executor } = &decl.effect {
                    if !self
                        .lake
                        .seams
                        .executors
                        .iter()
                        .any(|e| e.name() == executor)
                    {
                        return Err(Error::Unbound {
                            seam: format!("the verb executor `{executor}`"),
                        });
                    }
                    external = Some(External {
                        executor: executor.clone(),
                        verb: verb.clone(),
                        ty: decl.on.clone(),
                        key: key.clone(),
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
        }
        drop(view);
        let events = self.lake.append(&mut w, drafts)?;
        Ok((outcome_of(&events), external))
    }

    /// `reject`.
    pub fn reject(&self, proposal: &ProposalId, reason: String) -> Result<Outcome> {
        if !self.who.grants.review {
            return Err(not_granted());
        }
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
        Ok(Outcome::Applied {
            position: events[0].position,
            version: None,
            output: None,
        })
    }

    /// A function computed outside the store is declared only where a
    /// backing of its kind is bound; replay never asks.
    fn check_bound(&self, d: &Declaration) -> Result<()> {
        if let Declaration::Function(f) = d
            && crate::backed::is_backed(f)
            && !self
                .lake
                .seams
                .backings
                .iter()
                .any(|b| b.kind() == f.backing)
        {
            return Err(Error::Unbound {
                seam: format!(
                    "the `{}` function backing (the host bound none of that kind)",
                    f.backing
                ),
            });
        }
        Ok(())
    }

    /// `declare`.
    pub fn declare(&self, declaration: Declaration) -> Result<Outcome> {
        if !self.who.grants.schema {
            return Err(not_granted());
        }
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
        let d = self.draft(
            now,
            Origin::Authored,
            EventBody::Declared {
                declaration,
                schema_hash,
                version,
            },
        );
        let events = self.lake.append(&mut w, vec![d])?;
        Ok(Outcome::Applied {
            position: events[0].position,
            version: None,
            output: None,
        })
    }

    /// `apply`: a set of declarations validated together and appended in
    /// one append, or refused whole. Each declaration is checked against
    /// the lake's ontology with every other declaration of the set already
    /// in it, so a reference may point forward within the set and tool
    /// names are checked over all of it; every problem is collected, and
    /// one refusal names them all.
    pub fn apply(&self, declarations: Vec<Declaration>) -> Result<Outcome> {
        if !self.who.grants.schema {
            return Err(not_granted());
        }
        if declarations.is_empty() {
            return Err(invalid("nothing to apply: the set holds no declaration"));
        }
        let mut w = self.lake.writer();
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
            let mut probe = w.ontology.clone();
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
            0 => {}
            1 => return Err(problems.remove(0).1),
            _ => {
                let all: Vec<String> = problems
                    .iter()
                    .map(|(id, e)| format!("{id}: {e}"))
                    .collect();
                return Err(invalid(format!(
                    "the set is refused whole, with {} problems: {}",
                    all.len(),
                    all.join("; ")
                )));
            }
        }
        let now = self.lake.clock(&mut w);
        let mut drafts = Vec::with_capacity(declarations.len());
        for declaration in declarations {
            let schema_hash = match &declaration {
                Declaration::Type(t) => {
                    Some(shape::schema_hash(t).map_err(|e| invalid(e.to_string()))?)
                }
                _ => None,
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
        }
        let events = self.lake.append(&mut w, drafts)?;
        Ok(Outcome::Applied {
            position: events.last().expect("the set is not empty").position,
            version: None,
            output: None,
        })
    }

    /// `add_field`.
    pub fn add_field(
        &self,
        ty: &TypeName,
        field: lakelands_api::ontology::FieldDecl,
    ) -> Result<Outcome> {
        let mut t = self.lake.writer().ontology.ty(ty)?.clone();
        if t.fields.iter().any(|f| f.name == field.name) {
            return Err(Error::Refused {
                field: Some(field.name.clone()),
                reason: Refusal::Invalid {
                    message: format!("`{ty}` already has a field `{}`", field.name),
                },
            });
        }
        t.fields.push(field);
        self.declare(Declaration::Type(t))
    }

    /// The preflight of a migration over every stored record of the type,
    /// retired and restricted ones included: they are history, the fold
    /// converts them with the rest, and a migration that cannot convert
    /// one is not lossless, so it is refused before anything is appended.
    ///
    /// A migration keeps the type's key: the fold converts records where
    /// they are and never re-keys one, so changing `key` is refused as not
    /// supported in this release. What is checked instead is that every
    /// converted record's key fields still make its key; a conversion that
    /// changes a key field's value is reported not convertible for that
    /// record.
    pub fn preflight(
        &self,
        w: &Writer,
        view: &dyn ProjectionView,
        m: &Migration,
    ) -> Result<MigrationReport> {
        if !self.who.grants.schema {
            return Err(not_granted());
        }
        let before = w.ontology.ty(&m.to.name)?.clone();
        if m.to.key != before.key {
            return Err(invalid(format!(
                "the migration changes the key of `{}`; a migration keeps a type's key in this \
                 release, since records are converted where they are and never re-keyed",
                before.name
            )));
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
        let records = view.scan(
            &before.name,
            &Scan {
                limit: u32::MAX,
                retired: true,
                restricted: true,
                ..Default::default()
            },
        )?;
        let mut report = shape::preflight(
            &before,
            m,
            records.iter().map(|r| (r.key.clone(), r.fields.clone())),
        )
        .map_err(|e| invalid(e.to_string()))?;
        let mut rekeyed: Vec<RecordKey> = Vec::new();
        if !m.to.key.is_empty() {
            for r in &records {
                let converted = shape::migrate_record(&before, m, &r.fields)
                    .map_err(|e| invalid(e.to_string()))?;
                let Some(fields) = converted else { continue };
                if lakelands_core::log::record_key(&m.to, &fields).as_ref() != Some(&r.key) {
                    rekeyed.push(r.key.clone());
                }
            }
        }
        if !rekeyed.is_empty() {
            rekeyed.sort();
            let k0 = &m.to.key[0];
            let to =
                m.to.fields
                    .iter()
                    .find(|f| &f.name == k0)
                    .map(|f| f.ty.clone());
            let from = before
                .fields
                .iter()
                .find(|f| &f.name == k0)
                .map(|f| f.ty.clone());
            let bad = rekeyed.len() as u64;
            match report.fields.iter_mut().find(|f| &f.field == k0) {
                Some(f) => {
                    f.convertible = f.convertible.saturating_sub(bad);
                    f.not_convertible += bad;
                    f.sample_keys.extend(rekeyed.iter().cloned());
                    f.sample_keys.sort();
                    f.sample_keys.dedup();
                    f.sample_keys.truncate(5);
                }
                None => report.fields.push(FieldConversion {
                    field: k0.clone(),
                    from: from.clone().or(to.clone()).expect("a key names a field"),
                    to: to.or(from).expect("a key names a field"),
                    convertible: records.len() as u64 - bad,
                    not_convertible: bad,
                    sample_keys: rekeyed.into_iter().take(5).collect(),
                }),
            }
            report.lossless = false;
            report.report_id = String::new();
            report.report_id = lakelands_core::canonical::sha256_hex(
                &serde_json::to_value(&report).expect("a report serialises"),
            );
        }
        Ok(report)
    }

    /// `migrate_apply`.
    pub fn migrate_apply(&self, m: Migration, report_id: &str) -> Result<Outcome> {
        let mut w = self.lake.writer();
        let view = self.lake.engine.view(None)?;
        let report = self.preflight(&w, view.as_ref(), &m)?;
        if report.report_id != report_id {
            return Err(invalid(
                "the lake moved since that preflight report; run the preflight again",
            ));
        }
        if !report.lossless {
            return Err(Error::Refused {
                field: None,
                reason: Refusal::Migration {
                    report: Box::new(report),
                },
            });
        }
        let schema_hash = shape::schema_hash(&m.to).map_err(|e| invalid(e.to_string()))?;
        let version = w.ontology.next_version(&Declaration::Type(m.to.clone()));
        let now = self.lake.clock(&mut w);
        let d = self.draft(
            now,
            Origin::Authored,
            EventBody::Migrated {
                migration: m,
                schema_hash,
                version,
                report_id: report_id.to_owned(),
            },
        );
        drop(view);
        let events = self.lake.append(&mut w, vec![d])?;
        Ok(Outcome::Applied {
            position: events[0].position,
            version: None,
            output: None,
        })
    }
}

fn risk_word(r: Risk) -> &'static str {
    match r {
        Risk::Low => "low",
        Risk::Medium => "medium",
        Risk::High => "high",
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
    /// The record's key.
    pub key: RecordKey,
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
    let before = view
        .get(ty, key)?
        .filter(|r| !r.retired)
        .ok_or_else(|| Error::NotFound {
            missing: Missing::Record {
                ty: ty.clone(),
                key: key.clone(),
            },
        })?;
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
