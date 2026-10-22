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
use lakelands_api::event::{Event, EventBody, Procedure};
use lakelands_api::expr::Expr;
use lakelands_api::ontology::{OnInputChange, TypeDecl, ValueTypeDecl};
use lakelands_api::record::{FieldMeta, Origin, StoredRecord};
use lakelands_api::{Error, FieldName, Position, Result, TypeName, Value};
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
    /// The function that computes a field, by field: its name and
    /// version.
    procedures: BTreeMap<FieldName, Procedure>,
    /// Its constraints, parsed once (lakelands#1119).
    pub(crate) constraints: Vec<Constraint>,
}

/// A type's constraint as the write path judges it: parsed, with the
/// fields it reads and those whose write moves them, through derived
/// fields and fields cleared when their inputs change.
pub(crate) struct Constraint {
    pub(crate) decl: lakelands_api::ontology::ConstraintDecl,
    pub(crate) holds: Expr,
    pub(crate) reads: BTreeSet<FieldName>,
    pub(crate) fed: BTreeSet<FieldName>,
}

impl Plan {
    fn of(o: &Ontology, raw: &TypeDecl) -> Plan {
        let decl = resolve_functions(raw, o);
        let named = raw.fields.iter().filter_map(|f| f.ty.value_type.as_ref());
        let procedures = raw.fields.iter().filter_map(|f| {
            let name = f.function.as_ref()?;
            let function = o.functions.get(name)?;
            let procedure = Procedure {
                name: name.clone(),
                version: function.version.to_string(),
            };
            Some((f.name.clone(), procedure))
        });
        let derived = derive::order(&decl);
        let computed = derived.as_deref().unwrap_or_default();
        let constraints = (decl.constraints.iter()).filter_map(|c| {
            let holds = crate::declare::typed(&c.holds, lakelands_core::expr::Place::Guard).ok()?;
            let reads = lakelands_core::derive::reads(&holds);
            // What moves a field it reads: a derived field's inputs, and
            // those of a field the fold clears when they change, to the
            // fields a write sends (lakelands#1119).
            let feeds = (computed.iter())
                .map(|(f, e)| (f, lakelands_core::derive::reads(e)))
                .chain(
                    decl.fields
                        .iter()
                        .filter(|f| f.on_input_change == OnInputChange::Clear)
                        .map(|f| (&f.name, f.depends_on.iter().cloned().collect())),
                );
            let feeds: Vec<(&FieldName, BTreeSet<FieldName>)> = feeds.collect();
            let mut fed = reads.clone();
            while let Some(more) =
                (feeds.iter()).find(|(f, inputs)| fed.contains(*f) && !inputs.is_subset(&fed))
            {
                fed.extend(more.1.iter().cloned());
            }
            let decl = c.clone();
            Some(Constraint {
                decl,
                holds,
                reads,
                fed,
            })
        });
        let constraints = constraints.collect();
        Plan {
            entries: entries(o, &raw.name).collect(),
            value_types: named
                .filter_map(|n| o.value_types.get(n.as_str()))
                .map(|e| e.decl.clone())
                .collect(),
            derived,
            settled: backed::computed_order(o, raw),
            procedures: procedures.collect(),
            constraints,
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

/// The field's `initial`, as checked against its kind and value type.
pub(crate) fn initial_of(
    ontology: &Ontology,
    f: &lakelands_api::ontology::FieldDecl,
) -> Option<Value> {
    let v = f.initial.as_ref()?;
    let vt = f.ty.value_type.as_ref();
    let vt = vt.and_then(|n| ontology.value_types.get(n.as_str()));
    Some(value::check(v, &f.ty, vt.map(|e| &e.decl)).unwrap_or_else(|_| v.clone()))
}

/// The provenance of a field filled with its `initial` at `event`: at a
/// create, and at a migration that fills it (050).
pub(crate) fn initial_meta(event: &Event) -> FieldMeta {
    FieldMeta {
        upstream: None,
        seen_at: None,
        source: None,
        procedure: None,
        procedure_version: None,
        ..meta_of(event, Origin::Initial)
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
        procedure: event.procedure.as_ref().map(|p| p.name.clone()),
        procedure_version: event.procedure.as_ref().map(|p| p.version.clone()),
        error: None,
        erased: false,
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
        } => (ty, key, *version, patch, Door::Verb),
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
        let Some(field) = raw.field(name) else {
            continue;
        };
        if !door.admits(field, event.source.as_ref()) {
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
            if !fields.contains_key(&f.name)
                && let Some(v) = initial_of(ontology, f)
            {
                fields.insert(f.name.clone(), v);
                meta.insert(f.name.clone(), initial_meta(event));
            }
        }
    }

    // A record made again after a retirement is made at this write.
    let made = live_before.map_or(event.position, |b| b.made);
    compute(
        ontology,
        raw,
        (key, made),
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
        made,
        retired: false,
        restricted: before.is_some_and(|b| b.restricted),
    }))
}

/// The provenance of a value the fold computed: the event's, with
/// `origin: derived`, and for a field a function computes, that
/// function as the procedure.
fn derived_meta(event: &Event, plan: &Plan, field: &FieldName) -> FieldMeta {
    let mut m = meta_of(event, Origin::Derived);
    if let Some(p) = plan.procedures.get(field) {
        m.procedure = Some(p.name.clone());
        m.procedure_version = Some(p.version.clone());
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
        (&before.key, before.made),
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
        (&record.key, record.made, field),
        seam,
        input,
        output,
        event,
        &mut fields,
        &mut meta,
        outputs,
    )? {
        return Ok(None);
    }
    compute(
        ontology,
        raw,
        (&record.key, record.made),
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
    record: (&lakelands_api::RecordKey, Position),
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
        let (ty, cause) = (raw.name.clone(), Box::new(Error::invalid(e.clone())));
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
            meta,
            &derive::Write {
                before: before.as_ref(),
                written: &written,
                stamp: &|name| derived_meta(event, &plan, name),
                settings: Some(settings),
                now: event.known_at,
            },
        );
        changed.extend(touched);
        let snapshot = fields.clone();
        let settled = backed::settle_fields(
            ontology,
            &event.lake,
            raw,
            record,
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
            return Err(Error::constraint(Some(f.name.clone()), case));
        }
    }
    Ok(())
}
