//! What a record event does to a record: the per-field merge.
//!
//! One function, [`apply`], computes a record's state after an event. The
//! fold calls it to write the projection; the write path calls it first,
//! on the same inputs, to see the record it is about to create — so a
//! trigger or an object set is judged on exactly the state the fold will
//! then write, never on a second computation that could disagree.
//!
//! The merge, per field the event's patch carries:
//!
//! - **Authority.** A field is written only when the door the event came
//!   through admits the field's authority: `record_upserted` writes
//!   `authored` and `shared` fields, `record_ingested` writes `source` and
//!   `shared` fields, and nothing writes a `derived` one. The write path
//!   refuses a patch that breaks this before anything is appended; a field
//!   that reaches the fold anyway (a damaged log) is left unwritten rather
//!   than merged. So a re-import never overwrites an authored verdict, and
//!   an author never overwrites a figure the source is the authority on.
//! - **Sparse.** A field the patch does not carry keeps its value and its
//!   provenance; `null` clears it.
//! - **Order.** Two writes to one field resolve by log position: the later
//!   event's value stands. No clock and no source time decides.
//! - **Provenance.** Every field the event writes takes the event's
//!   `origin`, `known_at`, `seen_at`, `position`, `writer`, `source` and
//!   procedure version (`docs/decisions/010-provenance-per-field.md`), unless
//!   the type opted out of per-field provenance.
//! - **The fold's own fields.** Dependents are cleared and derived fields
//!   recomputed in the same event (`lakelands_core::derive`); what the fold
//!   writes this way takes the event's provenance with `origin: derived`.
//!   A field whose function has a backing outside the store is filled
//!   from a recorded output or cleared until one is recorded
//!   ([`crate::backed`]).
//!
//! - **Initial values.** The event that makes a record (none was live
//!   under the key) fills each field that declares `initial` and holds no
//!   value after the patch, with `origin: initial` and the event's
//!   position, time and writer
//!   (`docs/decisions/050-a-field-declares-its-initial-value.md`).
//!
//! A record written again after it was retired starts from no fields, and
//! its version continues from the retired one.

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use lakelands_api::error::{Case, Flaw, Part, Said};
use lakelands_api::event::{Event, EventBody};
use lakelands_api::expr::Expr;
use lakelands_api::ontology::{TypeDecl, ValueTypeDecl};
use lakelands_api::record::{FieldMeta, Origin, StoredRecord};
use lakelands_api::{FieldName, Result, TypeName, Value};
use lakelands_core::rights::Door;
use lakelands_core::{damaged, derive, value};

use crate::backed::{self, Outputs};
use crate::ontology::Ontology;

/// What computing a type's records reads of the ontology, prepared once
/// for every record: none of it depends on the record. An ordering that
/// fails is kept as it failed and answered only when a live record is
/// computed, at the same record and with the same flaw as without a plan.
pub struct Plan {
    /// The entries it was made from ([`entries`]): held, not their
    /// addresses, so that none is freed while the plan stands and another
    /// allocated in its place.
    entries: Vec<Held>,
    /// The type with its `expr` functions' bodies put in
    /// ([`resolve_functions`]).
    decl: TypeDecl,
    /// The value types its fields name.
    value_types: Vec<ValueTypeDecl>,
    /// Its derived fields in the order the fold computes them
    /// ([`derive::order`]).
    derived: std::result::Result<Vec<(FieldName, Expr)>, Said>,
    /// Its computed fields in the order the fold settles them
    /// ([`backed::computed_order`]).
    pub(crate) settled: std::result::Result<Vec<(FieldName, BTreeSet<FieldName>)>, Case>,
    /// The version of the function that computes a field, by field.
    versions: BTreeMap<FieldName, String>,
}

impl Plan {
    fn of(o: &Ontology, raw: &TypeDecl) -> Plan {
        let decl = resolve_functions(raw, o);
        let named = raw.fields.iter().filter_map(|f| f.ty.value_type.as_ref());
        let versions = raw.fields.iter().filter_map(|f| {
            let function = o.functions.get(f.function.as_ref()?)?;
            Some((f.name.clone(), function.version.to_string()))
        });
        Plan {
            entries: entries(o, &raw.name).collect(),
            value_types: named
                .filter_map(|n| o.value_types.get(n.as_str()))
                .map(|e| e.decl.clone())
                .collect(),
            derived: derive::order(&decl),
            settled: backed::computed_order(o, raw),
            versions: versions.collect(),
            decl,
        }
    }
}

/// An entry of the ontology, of whichever kind.
type Held = Arc<dyn Any + Send + Sync>;

/// The entries of an ontology a type's plan is made from: the type's, and
/// those of the functions and the value types its fields name.
fn entries<'o>(o: &'o Ontology, ty: &TypeName) -> impl Iterator<Item = Held> + 'o {
    let t = o.types.get(ty);
    let named = t.into_iter().flat_map(|t| &t.decl.fields).flat_map(|f| {
        let function = f.function.as_ref().and_then(|n| o.functions.get(n));
        let value_type = (f.ty.value_type.as_ref()).and_then(|n| o.value_types.get(n.as_str()));
        let function = function.map(|e| e.clone() as Held);
        function
            .into_iter()
            .chain(value_type.map(|e| e.clone() as _))
    });
    t.map(|t| t.clone() as Held).into_iter().chain(named)
}

/// The plans the fold, a step's walk and the write path compute by, one
/// per type. A plan stands while the ontology it is asked of holds the
/// very entries it was made from, compared by pointer: a declaration put
/// in force makes a new entry, and a function declared again with the
/// same inputs keeps its type's entry but not its own.
#[derive(Default)]
pub struct Plans(Mutex<BTreeMap<TypeName, Arc<Plan>>>);

impl Plans {
    /// The plan of `raw`, a type of `o`, made again when an entry it was
    /// made from is no longer the one in `o`.
    pub fn get(&self, o: &Ontology, raw: &TypeDecl) -> Arc<Plan> {
        if !o.types.contains_key(&raw.name) {
            return Arc::new(Plan::of(o, raw));
        }
        let mut plans = lakelands_core::lock(&self.0);
        let ptr = |e: &Held| Arc::as_ptr(e).cast::<()>();
        if let Some(plan) = plans.get(&raw.name)
            && (plan.entries.iter().map(ptr)).eq(entries(o, &raw.name).map(|e| ptr(&e)))
        {
            return plan.clone();
        }
        let plan = Arc::new(Plan::of(o, raw));
        plans.insert(raw.name.clone(), plan.clone());
        plan
    }
}

/// The provenance an event gives the fields it writes.
pub(crate) fn meta_of(event: &Event, origin: Origin) -> FieldMeta {
    FieldMeta {
        upstream: event.upstream.clone(),
        origin,
        known_at: event.known_at,
        seen_at: event.seen_at,
        position: event.position,
        writer: event.writer.clone(),
        source: event.source.clone(),
        procedure_version: event.procedure.as_ref().map(|p| p.version.clone()),
        error: None,
    }
}

/// A record's state after a record event, from its state before (`None`
/// for a record never written). `settings` are the lake's settings as the
/// event folds them. `None` for an event that is not about a record.
pub fn apply(
    ontology: &Ontology,
    before: Option<&StoredRecord>,
    event: &Event,
    settings: &BTreeMap<String, Value>,
    outputs: &mut Outputs<'_>,
) -> Result<Option<StoredRecord>> {
    let (ty, key, version, patch, door) = match &event.body {
        EventBody::RecordUpserted {
            ty,
            key,
            version,
            patch,
            ..
        } => (ty, key, *version, patch, Door::Authored),
        EventBody::RecordIngested {
            ty,
            key,
            version,
            patch,
            ..
        } => (ty, key, *version, patch, Door::Source),
        EventBody::RecordRetired { ty, key, version }
        | EventBody::RecordRestricted {
            ty, key, version, ..
        }
        | EventBody::RecordUnrestricted { ty, key, version } => {
            let Some(mut r) = before.cloned() else {
                let (ty, key) = (ty.clone(), key.clone());
                let flaw = Flaw::ChangedUnwritten { ty, key };
                return Err(damaged(Part::Log, Some(event.position), flaw));
            };
            match &event.body {
                EventBody::RecordRetired { .. } => r.retired = true,
                EventBody::RecordRestricted { .. } => r.restricted = true,
                _ => r.restricted = false,
            }
            r.version = *version;
            r.position = event.position;
            return Ok(Some(r));
        }
        _ => return Ok(None),
    };
    let raw = ontology.ty(ty)?;
    let live_before = before.filter(|b| !b.retired);
    let mut fields = live_before.map(|b| b.fields.clone()).unwrap_or_default();
    let mut meta = live_before.map(|b| b.meta.clone()).unwrap_or_default();

    let mut written = BTreeSet::new();
    for (name, value) in patch {
        let Some(field) = raw.fields.iter().find(|f| &f.name == name) else {
            continue;
        };
        if !door.admits(field.authority) {
            continue;
        }
        if value.is_null() {
            fields.remove(name);
        } else {
            fields.insert(name.clone(), value.clone());
        }
        written.insert(name.clone());
        meta.insert(name.clone(), meta_of(event, event.origin));
    }
    // A record that comes to exist takes each declared `initial` value the
    // creating event left out: the door judges the record with it, and the
    // fold computes from it, as it computes every field at a create (050).
    if live_before.is_none() {
        for f in raw.fields.iter().filter(|f| f.initial.is_some()) {
            if fields.contains_key(&f.name) {
                continue;
            }
            let (v, vt) = (
                f.initial.as_ref().expect("initial"),
                f.ty.value_type.as_ref(),
            );
            let vt = vt.and_then(|n| ontology.value_types.get(n.as_str()));
            let v = value::check(v, &f.ty, vt.map(|e| &e.decl)).unwrap_or_else(|_| v.clone());
            fields.insert(f.name.clone(), v);
            let of_the_event = FieldMeta {
                upstream: None,
                seen_at: None,
                source: None,
                procedure_version: None,
                ..meta_of(event, Origin::Initial)
            };
            meta.insert(f.name.clone(), of_the_event);
        }
    }

    compute(
        ontology,
        raw,
        key,
        live_before.map(|b| &b.fields),
        written,
        &mut fields,
        &mut meta,
        event,
        settings,
        false,
        outputs,
    )?;
    Ok(Some(StoredRecord {
        ty: ty.clone(),
        key: key.clone(),
        version,
        fields,
        meta,
        position: event.position,
        retired: false,
        restricted: before.is_some_and(|b| b.restricted),
    }))
}

/// The provenance of a value the fold computed: the event's, with
/// `origin: derived`, and for a field a function computes, that
/// function's version as the procedure's.
fn derived_meta(event: &Event, plan: &Plan, field: &FieldName) -> FieldMeta {
    let mut m = meta_of(event, Origin::Derived);
    if let Some(version) = plan.versions.get(field) {
        m.procedure_version = Some(version.clone());
    }
    m
}

/// A live record's derived fields computed again, when what computes them
/// changed: a function it names was redeclared, or its type's derived
/// expressions were. `None` when nothing changes. The values take the
/// event's provenance; the record's version stays, since no write touched
/// it.
pub fn rederive(
    ontology: &Ontology,
    before: &StoredRecord,
    event: &Event,
    settings: &BTreeMap<String, Value>,
    outputs: &mut Outputs<'_>,
) -> Result<Option<StoredRecord>> {
    if before.retired {
        return Ok(None);
    }
    let raw = ontology.ty(&before.ty)?;
    let mut fields = before.fields.clone();
    let mut meta = before.meta.clone();
    let changed = compute(
        ontology,
        raw,
        &before.key,
        Some(&before.fields),
        BTreeSet::new(),
        &mut fields,
        &mut meta,
        event,
        settings,
        true,
        outputs,
    )?;
    if changed.is_empty() {
        return Ok(None);
    }
    Ok(Some(StoredRecord {
        fields,
        meta,
        position: event.position,
        ..before.clone()
    }))
}

/// A record's state after an `output_recorded` that names one of its
/// fields: the field takes the output (its value, or no value and the
/// reason), and the fold's own rules follow as for a write of that one
/// field: derived fields recomputed, dependents cleared, and backed fields
/// whose inputs it changed filled from a recorded output or made due. The
/// record's version stays, since no write touched it. `None` when the
/// output fills nothing: the record is retired, the field is no longer
/// computed by the function the seam names, the record's call has moved
/// on to other inputs, or the field already holds what the output says.
#[allow(clippy::too_many_arguments)]
pub fn fill(
    ontology: &Ontology,
    record: &StoredRecord,
    field: &FieldName,
    seam: &str,
    input: &lakelands_api::event::Digest,
    output: &Value,
    event: &Event,
    settings: &BTreeMap<String, Value>,
    outputs: &mut Outputs<'_>,
) -> Result<Option<StoredRecord>> {
    if record.retired {
        return Ok(None);
    }
    let raw = ontology.ty(&record.ty)?;
    let mut fields = record.fields.clone();
    let mut meta = record.meta.clone();
    if !backed::put_output(
        ontology,
        raw,
        field,
        seam,
        input,
        output,
        event,
        &mut fields,
        &mut meta,
    ) {
        return Ok(None);
    }
    compute(
        ontology,
        raw,
        &record.key,
        Some(&record.fields),
        [field.clone()].into(),
        &mut fields,
        &mut meta,
        event,
        settings,
        false,
        outputs,
    )?;
    Ok(Some(StoredRecord {
        fields,
        meta,
        position: event.position,
        ..record.clone()
    }))
}

/// The fold's own fields after an event changed `written` (or, with
/// `force`, after what computes them was declared again): derived fields
/// recomputed and dependents cleared, then backed fields settled in the
/// order of the one reads-graph, and again, with what settling changed as
/// the new `written`, until nothing changes. It ends, since the graph has
/// no cycle. Every field it changes takes the event's provenance with
/// `origin: derived`. Null fields are dropped, and so is the provenance of
/// a type that keeps none. Answers the fields it changed.
#[allow(clippy::too_many_arguments)]
fn compute(
    ontology: &Ontology,
    raw: &TypeDecl,
    key: &lakelands_api::RecordKey,
    before: Option<&BTreeMap<FieldName, Value>>,
    mut written: BTreeSet<FieldName>,
    fields: &mut BTreeMap<FieldName, Value>,
    meta: &mut BTreeMap<FieldName, FieldMeta>,
    event: &Event,
    settings: &BTreeMap<String, Value>,
    mut force: bool,
    outputs: &mut Outputs<'_>,
) -> Result<BTreeSet<FieldName>> {
    let plan = outputs.plans.get(ontology, raw);
    let order = plan.derived.as_ref().map_err(|e| {
        let (ty, cause) = (
            raw.name.clone(),
            Box::new(lakelands_core::invalid(e.clone())),
        );
        damaged(Part::Projection, None, Flaw::CannotDerive { ty, cause })
    })?;
    let mut changed = BTreeSet::new();
    let mut before: Option<BTreeMap<FieldName, Value>> = before.cloned();
    // One round per computed field at most: the graph has no cycle.
    for _ in 0..=raw.fields.len() {
        let touched = derive::apply(
            &plan.decl,
            order,
            &plan.value_types,
            fields,
            &derive::Write {
                before: before.as_ref(),
                written: &written,
                settings: Some(settings),
                now: event.known_at,
            },
        );
        for name in touched {
            meta.insert(name.clone(), derived_meta(event, &plan, &name));
            changed.insert(name);
        }
        let snapshot = fields.clone();
        let settled = backed::settle_fields(
            ontology,
            &event.lake,
            raw,
            key,
            before.as_ref(),
            fields,
            meta,
            event,
            force,
            outputs,
        )?;
        force = false;
        if settled.is_empty() {
            break;
        }
        changed.extend(settled.iter().cloned());
        written = settled.into_iter().collect();
        before = Some(snapshot);
    }
    fields.retain(|_, v| !v.is_null());
    if !raw.field_meta {
        meta.clear();
    }
    Ok(changed)
}

/// A type with every field computed by a declared `expr` function given
/// that function's expression, so the derived pass treats both alike. A
/// field whose function has another backing is left to [`backed`].
pub fn resolve_functions(decl: &TypeDecl, ontology: &Ontology) -> TypeDecl {
    let mut out = decl.clone();
    for f in &mut out.fields {
        if let Some(name) = &f.function
            && let Some(func) = ontology.functions.get(name)
        {
            f.derived = func.decl.expr.clone();
        }
    }
    out
}

/// Refuse a record that lacks a value for a required field.
pub fn check_required(decl: &TypeDecl, fields: &BTreeMap<FieldName, Value>) -> Result<()> {
    for f in decl.fields.iter().filter(|f| f.required) {
        if fields.get(&f.name).is_none_or(Value::is_null) {
            let (ty, field) = (decl.name.clone(), f.name.clone());
            let case = lakelands_api::error::Case::Required { ty, field };
            return Err(crate::writes::constraint(Some(f.name.clone()), case));
        }
    }
    Ok(())
}
