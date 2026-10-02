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

use lakelands_api::expr::Expr;
use lakelands_api::ontology::{FieldDecl, OnInputChange, TypeDecl, ValueTypeDecl};
use lakelands_api::{FieldName, Timestamp, Value};

use crate::expr::{WriteScope, eval, eval::to_json, parse};

/// Why a type's derived fields cannot be ordered or parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeriveError(pub String);

impl std::fmt::Display for DeriveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DeriveError {}

/// The fields an expression reads: the first segment of every path, and
/// the field named by `known_at`, `seen_at`, `previous` and `changed`.
pub fn reads(e: &Expr) -> BTreeSet<FieldName> {
    let mut out = BTreeSet::new();
    collect(e, &mut out);
    out
}

fn collect(e: &Expr, out: &mut BTreeSet<FieldName>) {
    if let Expr::Path { path } = e
        && let Some(first) = path.first()
    {
        out.insert(first.clone());
    }
    crate::expr::children(e).for_each(|c| collect(c, out));
}

fn typed(field: &FieldDecl) -> Result<Option<Expr>, DeriveError> {
    match &field.derived {
        None => Ok(None),
        Some(lakelands_api::expr::Condition::Expr(e)) => Ok(Some(e.clone())),
        Some(lakelands_api::expr::Condition::Text(t)) => parse(t).map(Some).map_err(|e| {
            DeriveError(format!(
                "`{}` is derived from `{t}`, which does not parse: {e}",
                field.name
            ))
        }),
    }
}

/// The derived fields of a type in the order the fold computes them: each
/// after every derived field it reads; among fields free to go at the same
/// point, declaration order. A cycle is an error naming its fields.
pub fn order(ty: &TypeDecl) -> Result<Vec<(FieldName, Expr)>, DeriveError> {
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
        return Err(DeriveError(format!("derived field `{n}` reads itself")));
    }
    let mut done: BTreeSet<FieldName> = BTreeSet::new();
    let mut out = Vec::new();
    while !waiting.is_empty() {
        let Some(i) = waiting
            .iter()
            .position(|(_, _, deps)| deps.is_subset(&done))
        else {
            let cycle: Vec<String> = waiting.iter().map(|(n, _, _)| n.to_string()).collect();
            return Err(DeriveError(format!(
                "derived fields read each other in a cycle: {}",
                cycle.join(", ")
            )));
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
) -> Result<BTreeSet<FieldName>, DeriveError> {
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

    // 2. Recompute derived fields in dependency order.
    for (name, expr) in order(ty)? {
        let decl = ty
            .fields
            .iter()
            .find(|f| f.name == name)
            .expect("ordered from the type");
        let scale = decl
            .ty
            .value_type
            .as_ref()
            .and_then(|vt| value_types.iter().find(|d| &d.name == vt))
            .and_then(|vt| vt.scale);
        let scope = WriteScope {
            after: fields,
            before: write.before,
            retiring: false,
            meta: None,
            settings: write.settings,
            now: write.now,
        };
        let value = eval(&expr, &scope)
            .ok()
            .and_then(|v| to_json(&v, decl.ty.kind, scale))
            .unwrap_or(Value::Null);
        let old = fields.get(&name).cloned().unwrap_or(Value::Null);
        if old != value {
            fields.insert(name.clone(), value);
            touched.insert(name);
        }
    }
    Ok(touched)
}

#[cfg(test)]
mod tests;
