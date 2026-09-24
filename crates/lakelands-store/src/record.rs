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
//!
//! A record written again after it was retired starts from no fields, and
//! its version continues from the retired one.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::event::{Event, EventBody};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::record::{FieldMeta, Origin, StoredRecord};
use lakelands_api::{Error, FieldName, Result, Value};
use lakelands_core::derive;
use lakelands_core::rights::Door;

use crate::ontology::Ontology;

/// The provenance an event gives the fields it writes.
fn meta_of(event: &Event, origin: Origin) -> FieldMeta {
    FieldMeta {
        origin,
        known_at: event.known_at,
        seen_at: event.seen_at,
        position: event.position,
        writer: event.writer.clone(),
        source: event.source.clone(),
        procedure_version: event.procedure.as_ref().map(|p| p.version.clone()),
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
) -> Result<Option<StoredRecord>> {
    let (ty, key, version, patch, door) = match &event.body {
        EventBody::RecordUpserted {
            ty,
            key,
            version,
            patch,
        } => (ty, key, *version, patch, Door::Authored),
        EventBody::RecordIngested {
            ty,
            key,
            version,
            patch,
        } => (ty, key, *version, patch, Door::Source),
        EventBody::RecordRetired { ty, key, version }
        | EventBody::RecordRestricted {
            ty, key, version, ..
        }
        | EventBody::RecordUnrestricted { ty, key, version } => {
            let Some(mut r) = before.cloned() else {
                return Err(Error::Unavailable {
                    message: format!(
                        "the log changes `{ty}` `{key}` at position {} before any event wrote it",
                        event.position
                    ),
                });
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
    let decl = &resolve_functions(ontology.ty(ty)?, ontology);
    let live_before = before.filter(|b| !b.retired);
    let mut fields = live_before.map(|b| b.fields.clone()).unwrap_or_default();
    let mut meta = live_before.map(|b| b.meta.clone()).unwrap_or_default();

    let mut written = BTreeSet::new();
    for (name, value) in patch {
        let Some(field) = decl.fields.iter().find(|f| &f.name == name) else {
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

    let raw = ontology.ty(ty)?;
    let value_types = ontology.value_type_list();
    let touched = derive::apply(
        decl,
        &value_types,
        &mut fields,
        &derive::Write {
            before: live_before.map(|b| &b.fields),
            written: &written,
            settings: Some(settings),
            now: event.known_at,
        },
    )
    .map_err(|e| Error::Unavailable {
        message: format!("the fold cannot compute the derived fields of `{ty}`: {e}"),
    })?;
    for name in touched {
        let m = derived_meta(event, raw, ontology, &name);
        meta.insert(name, m);
    }
    fields.retain(|_, v| !v.is_null());
    if !decl.field_meta {
        meta.clear();
    }
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
fn derived_meta(
    event: &Event,
    raw: &TypeDecl,
    ontology: &Ontology,
    field: &FieldName,
) -> FieldMeta {
    let mut m = meta_of(event, Origin::Derived);
    if let Some(func) = raw
        .fields
        .iter()
        .find(|f| &f.name == field)
        .and_then(|f| f.function.as_ref())
        .and_then(|name| ontology.functions.get(name))
    {
        m.procedure_version = Some(func.version.to_string());
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
) -> Result<Option<StoredRecord>> {
    if before.retired {
        return Ok(None);
    }
    let raw = ontology.ty(&before.ty)?;
    let decl = resolve_functions(raw, ontology);
    let mut fields = before.fields.clone();
    let written = BTreeSet::new();
    let touched = derive::apply(
        &decl,
        &ontology.value_type_list(),
        &mut fields,
        &derive::Write {
            before: Some(&before.fields),
            written: &written,
            settings: Some(settings),
            now: event.known_at,
        },
    )
    .map_err(|e| Error::Unavailable {
        message: format!(
            "the fold cannot compute the derived fields of `{}`: {e}",
            before.ty
        ),
    })?;
    if touched.is_empty() {
        return Ok(None);
    }
    let mut meta = before.meta.clone();
    for name in &touched {
        meta.insert(name.clone(), derived_meta(event, raw, ontology, name));
    }
    fields.retain(|_, v| !v.is_null());
    if !decl.field_meta {
        meta.clear();
    }
    Ok(Some(StoredRecord {
        fields,
        meta,
        position: event.position,
        ..before.clone()
    }))
}

/// A type with every field computed by a declared function given that
/// function's expression, so the derived pass treats both alike. Only
/// `expr` functions are declared in 0.1.0; any other backing is refused
/// when the field is declared.
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
            return Err(Error::Refused {
                field: Some(f.name.clone()),
                reason: lakelands_api::Refusal::Constraint {
                    message: format!("`{}` requires a value for `{}`", decl.name, f.name),
                },
            });
        }
    }
    Ok(())
}
