//! What the fold computes itself: derived fields, and the clearing of
//! fields whose inputs changed (`docs/decisions/012-writes-touch-only-their-fields.md`).
//!
//! For every write that changes fields of a record, in the same event:
//!
//! 1. **Dependents are cleared.** A field that declares `depends_on` with
//!    `on_input_change: clear` is cleared when the write changes one of its
//!    inputs — unless the write itself sets that field. Clearing counts as
//!    a change, so a field that depends on a cleared field is cleared too.
//! 2. **Derived fields are recomputed,** each from its expression over the
//!    record as it now stands, in dependency order: a derived field is
//!    computed after every derived field it reads, whatever the order they
//!    were declared in. A cycle among derived fields is refused when the
//!    type is declared, so the fold never meets one.
//!
//! A derived value lands in its field's storage kind; a decimal whose value
//! type declares a scale is rounded half away from zero first, so a chain
//! like `net = gross - vat` reads the already-rounded `vat` and reconciles
//! to the cent. A derived value that cannot be computed for this record —
//! an absent input, a division by zero, a value that does not fit the
//! field — is cleared rather than left stale.
//!
//! All of it is a pure function of the record, the write, the lake's
//! settings and the event's time, so a replay recomputes it exactly.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::error::{Case, Place, Said};
use lakelands_api::expr::Expr;
use lakelands_api::ontology::{FieldDecl, OnInputChange, TypeDecl, ValueTypeDecl};
use lakelands_api::record::StoredRecord;
use lakelands_api::{FieldName, Timestamp, Value};

use crate::expr::{WriteScope, eval, eval::to_json, parse};

/// The fields an expression reads: the first segment of every path, and
/// the field named by `known_at`, `seen_at`, `previous` and `changed`.
pub fn reads(e: &Expr) -> BTreeSet<FieldName> {
    e.fields_read().into_iter().cloned().collect()
}

fn paths<'e>(e: &'e Expr, out: &mut Vec<&'e [FieldName]>) {
    if let Expr::Path { path } = e {
        out.push(path);
    }
    crate::expr::children(e).for_each(|c| paths(c, out));
}

/// The member of the call's payload a path in a verb's value reads:
/// `payload.<name>`, and no other path. A bare `payload` and a longer path
/// are the record's field of that name (028, extended).
pub fn payload_member(path: &[FieldName]) -> Option<&FieldName> {
    match path {
        [payload, name] if payload.as_str() == "payload" => Some(name),
        _ => None,
    }
}

/// What a verb's value reads: the record's fields, as [`reads`] counts
/// them, and the members of the call's payload.
pub fn verb_reads(e: &Expr) -> (BTreeSet<FieldName>, BTreeSet<FieldName>) {
    let mut all = Vec::new();
    paths(e, &mut all);
    let record = all.iter().filter(|p| payload_member(p).is_none());
    (
        record.filter_map(|p| p.first().cloned()).collect(),
        all.iter()
            .filter_map(|p| payload_member(p).cloned())
            .collect(),
    )
}

fn typed(field: &FieldDecl) -> Result<Option<Expr>, Said> {
    match &field.derived {
        None => Ok(None),
        Some(lakelands_api::expr::Condition::Expr(e)) => Ok(Some(e.clone())),
        Some(lakelands_api::expr::Condition::Text(t)) => parse(t).map(Some).map_err(|e| {
            let name = field.name.clone();
            Said::from(e.case).within(Place::Field { name })
        }),
    }
}

/// The derived fields of a type in the order the fold computes them: each
/// after every derived field it reads; among fields free to go at the same
/// point, declaration order. A cycle is an error naming its fields.
pub fn order(ty: &TypeDecl) -> Result<Vec<(FieldName, Expr)>, Said> {
    let mut derived: Vec<(FieldName, Expr)> = Vec::new();
    for f in &ty.fields {
        if let Some(e) = typed(f)? {
            derived.push((f.name.clone(), e));
        }
    }
    let names: BTreeSet<FieldName> = derived.iter().map(|(n, _)| n.clone()).collect();
    let mut waiting: Vec<(FieldName, Expr, BTreeSet<FieldName>)> = derived
        .into_iter()
        .map(|(n, e)| {
            let deps = reads(&e)
                .intersection(&names)
                .filter(|d| **d != n)
                .cloned()
                .collect();
            (n, e, deps)
        })
        .collect();
    if let Some((n, _, _)) = waiting.iter().find(|(n, e, _)| reads(e).contains(n)) {
        return Err(Case::DerivedReadsItself { field: n.clone() }.into());
    }
    let mut done: BTreeSet<FieldName> = BTreeSet::new();
    let mut out = Vec::new();
    while !waiting.is_empty() {
        let Some(i) = waiting
            .iter()
            .position(|(_, _, deps)| deps.is_subset(&done))
        else {
            let fields = waiting.iter().map(|(n, _, _)| n).collect();
            return Err(Case::DerivedCycle { fields }.into());
        };
        let (n, e, _) = waiting.remove(i);
        done.insert(n.clone());
        out.push((n, e));
    }
    Ok(out)
}

/// The context of one write, for the fold.
pub struct Write<'a> {
    /// The record's fields before the write; `None` for a create.
    pub before: Option<&'a BTreeMap<FieldName, Value>>,
    /// The fields the write itself sets or clears.
    pub written: &'a BTreeSet<FieldName>,
    /// The lake's settings, by key.
    pub settings: Option<&'a BTreeMap<String, Value>>,
    /// The `known_at` of the event.
    pub now: Timestamp,
}

/// Apply the fold's own rules to a record after a write's patch was
/// merged into `fields`. Answers the fields the fold changed (cleared or
/// recomputed), which take the event's provenance with `origin: derived`.
pub fn apply(
    ty: &TypeDecl,
    value_types: &[ValueTypeDecl],
    fields: &mut BTreeMap<FieldName, Value>,
    write: &Write<'_>,
) -> Result<BTreeSet<FieldName>, Said> {
    let mut changed: BTreeSet<FieldName> = write.written.clone();
    let mut touched = BTreeSet::new();

    // 1. Clear dependents, to a fixed point, in declaration order.
    loop {
        let mut again = false;
        for f in &ty.fields {
            let clears = f.on_input_change == OnInputChange::Clear
                && !write.written.contains(&f.name)
                && f.depends_on.iter().any(|d| changed.contains(d));
            if clears && fields.get(&f.name).is_some_and(|v| !v.is_null()) {
                fields.insert(f.name.clone(), Value::Null);
                changed.insert(f.name.clone());
                touched.insert(f.name.clone());
                again = true;
            }
        }
        if !again {
            break;
        }
    }

    // 2. Recompute derived fields in dependency order; one computed at the
    //    read holds no value.
    for (name, expr) in order(ty)? {
        let decl = ty
            .fields
            .iter()
            .find(|f| f.name == name)
            .expect("ordered from the type");
        let scope = WriteScope {
            after: fields,
            before: write.before,
            retiring: false,
            meta: None,
            settings: write.settings,
            now: write.now,
        };
        let value = match decl.at_read() {
            true => Value::Null,
            false => computed(decl, &expr, value_types, &scope),
        };
        if fields.get(&name).unwrap_or(&Value::Null) != &value {
            fields.insert(name.clone(), value);
            touched.insert(name);
        }
    }
    Ok(touched)
}

/// A record as one read answers it: each field computed at the read
/// computed from its fields as stored, their provenance, the lake's
/// settings and the read's clock, in the order the fold computes derived
/// fields; a value that cannot be computed is none. A type whose derived
/// fields cannot be ordered was refused when declared, and has none here.
pub fn at_read(
    ty: &TypeDecl,
    value_types: &[ValueTypeDecl],
    record: &mut StoredRecord,
    settings: &BTreeMap<String, Value>,
    now: Timestamp,
) {
    if !ty.fields.iter().any(FieldDecl::at_read) {
        return;
    }
    // Provenance one had when it was computed at the write is not its own.
    (record.meta).retain(|n, _| !ty.fields.iter().any(|f| &f.name == n && f.at_read()));
    for (name, expr) in order(ty).unwrap_or_default() {
        let Some(decl) = ty.fields.iter().find(|f| f.name == name && f.at_read()) else {
            continue;
        };
        let scope = WriteScope {
            after: &record.fields,
            before: None,
            retiring: false,
            meta: Some(&record.meta),
            settings: Some(settings),
            now,
        };
        match computed(decl, &expr, value_types, &scope) {
            Value::Null => record.fields.remove(&name),
            v => record.fields.insert(name, v),
        };
    }
}

/// What `expr` computes for the field `decl` over `scope`, landed in its
/// kind at its value type's scale; null when it cannot be, or when the
/// field's type would not take it: a JSON value is not checked when it is
/// written, so a vector or a geometry computed from one is checked here.
fn computed(
    decl: &FieldDecl,
    expr: &Expr,
    value_types: &[ValueTypeDecl],
    scope: &WriteScope<'_>,
) -> Value {
    let scale = (decl.ty.value_type.as_ref())
        .and_then(|vt| value_types.iter().find(|d| &d.name == vt))
        .and_then(|vt| vt.scale);
    eval(expr, scope)
        .ok()
        .and_then(|v| to_json(&v, decl.ty.kind, scale))
        .filter(|v| v.is_null() || crate::shape::fits(&decl.ty, v))
        .unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests;
