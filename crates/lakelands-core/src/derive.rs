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
use lakelands_api::expr::{Expr, Func};
use lakelands_api::ontology::{Authority, FieldDecl, OnInputChange, TypeDecl, ValueTypeDecl};
use lakelands_api::record::StoredRecord;
use lakelands_api::{FieldMeta, FieldName, Timestamp, Value};

use crate::expr::{WriteScope, eval, eval::to_json, parse};

/// The fields an expression reads: the first segment of every path, and
/// the field named by `known_at`, `seen_at`, `previous` and `changed`.
pub fn reads(e: &Expr) -> BTreeSet<FieldName> {
    e.fields_read().into_iter().cloned().collect()
}

/// The settings `e` reads, each by the key its `setting('…')` quotes.
pub fn settings(e: &Expr, named: &mut BTreeSet<String>) {
    if let Expr::Call {
        func: Func::Setting,
        args,
    } = e
        && let Some(Expr::Lit {
            value: lakelands_api::expr::Literal::Text(key),
        }) = args.first()
    {
        named.insert(key.clone());
    }
    lakelands_api::expr::children(e).for_each(|c| settings(c, named));
}

/// The member of the call's payload a path in a verb's value reads:
/// `payload.<name>`, and no other path. A bare `payload` and a longer path
/// are the record's field of that name (028, extended).
#[doc(hidden)]
pub fn payload_member(path: &[FieldName]) -> Option<&FieldName> {
    match path {
        [payload, name] if payload.as_str() == "payload" => Some(name),
        _ => None,
    }
}

/// What a verb's value reads: the record's fields, as [`reads`] counts
/// them, and the members of the call's payload.
#[doc(hidden)]
pub fn verb_reads(e: &Expr) -> (BTreeSet<FieldName>, BTreeSet<FieldName>) {
    let mut all = Vec::new();
    lakelands_api::expr::paths(e, &mut all);
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
#[doc(hidden)]
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

/// What the record does not decide: the lake's settings, the clock, and
/// when a value was learned or seen. [`bare`] evaluates without them.
const OUTSIDE: &[Func] = &[
    Func::Now,
    Func::Today,
    Func::KnownAt,
    Func::SeenAt,
    Func::Setting,
    Func::Param,
];

/// Whether the record alone decides `e`: neither it nor a derived field it
/// reads, followed through the derived ones, reads what [`OUTSIDE`] names,
/// or a field computed outside the lake, an embedding or a backed
/// function's, which a write may fill from an output recorded before.
fn decided(ty: &TypeDecl, e: &Expr) -> bool {
    let computed = order(ty).unwrap_or_default();
    let outside = |n: &FieldName| {
        let f = ty.field(n);
        f.is_some_and(|f| f.embed.is_some() || (f.function.is_some() && f.derived.is_none()))
    };
    let mut seen = BTreeSet::new();
    let mut pending = vec![e];
    while let Some(e) = pending.pop() {
        if e.calls(OUTSIDE) || reads(e).iter().any(outside) {
            return false;
        }
        let more = computed.iter().filter(|(n, _)| reads(e).contains(n));
        pending.extend(more.filter(|(n, _)| seen.insert(n.clone())).map(|(_, e)| e));
    }
    true
}

/// The fields of `ty` that no create can send: those only verbs write, and
/// those computed from nothing but such fields, followed through the
/// derived ones. `ty` has each `expr` function's expression in place of its
/// name.
#[doc(hidden)]
pub fn fixed(ty: &TypeDecl) -> BTreeSet<FieldName> {
    let mut fixed: BTreeSet<FieldName> = (ty.fields.iter())
        .filter(|f| f.authority == Authority::Verb)
        .map(|f| f.name.clone())
        .collect();
    let derived: Vec<(FieldName, Expr)> = order(ty).unwrap_or_default();
    loop {
        let more = derived.iter().find(|(n, e)| {
            let computed = ty.fields.iter().any(|f| &f.name == n && !f.at_read());
            computed && !fixed.contains(n) && reads(e).is_subset(&fixed)
        });
        match more {
            Some((n, _)) => fixed.insert(n.clone()),
            None => return fixed,
        };
    }
}

/// The record a create that sends nothing but its key leaves: each
/// field's `initial`, and each derived field computed from them. `ty` is
/// as for [`fixed`].
#[doc(hidden)]
pub fn bare(ty: &TypeDecl, value_types: &[ValueTypeDecl]) -> BTreeMap<FieldName, Value> {
    sent(ty, value_types, &BTreeMap::new())
}

/// The record a create that sends `sent` leaves: [`bare`] with those
/// values in place of the `initial`s.
fn sent(
    ty: &TypeDecl,
    value_types: &[ValueTypeDecl],
    sent: &BTreeMap<FieldName, Value>,
) -> BTreeMap<FieldName, Value> {
    let mut fields: BTreeMap<FieldName, Value> = (ty.fields.iter())
        .filter_map(|f| Some((f.name.clone(), f.initial.clone()?)))
        .collect();
    fields.extend(sent.iter().map(|(n, v)| (n.clone(), v.clone())));
    for (name, expr) in order(ty).unwrap_or_default() {
        let Some(decl) = ty.fields.iter().find(|f| f.name == name && !f.at_read()) else {
            continue;
        };
        let scope = WriteScope {
            after: &fields,
            before: None,
            retiring: false,
            meta: None,
            settings: None,
            now: Timestamp(0),
        };
        match computed(decl, &expr, value_types, &scope) {
            Value::Null => fields.remove(&name),
            v => fields.insert(name, v),
        };
    }
    fields
}

/// The fields that feed `read`, followed through the derived fields of
/// `ty` to those no expression of it computes. `ty` is as for [`fixed`].
#[doc(hidden)]
pub fn leaves(ty: &TypeDecl, read: &BTreeSet<FieldName>) -> BTreeSet<FieldName> {
    let computed = order(ty).unwrap_or_default();
    let mut leaves = BTreeSet::new();
    let mut pending: Vec<FieldName> = read.iter().cloned().collect();
    while let Some(n) = pending.pop() {
        match computed.iter().find(|(c, _)| *c == n) {
            Some((_, e)) => pending.extend(reads(e)),
            None => {
                leaves.insert(n);
            }
        }
    }
    leaves
}

/// Whether no create that sends only fields `sendable` names can make `e`
/// hold, as far as its form shows it: no field it reads is fed by one of
/// them, or one of the conditions it joins by `and` is, read only from
/// fields none of them feeds, broken by the record a create leaves
/// ([`bare`]) and decided by the record alone. Anything else is a create it
/// cannot rule out. `ty` is as for [`fixed`].
#[doc(hidden)]
pub fn stuck(
    ty: &TypeDecl,
    value_types: &[ValueTypeDecl],
    e: &Expr,
    sendable: &dyn Fn(&FieldName) -> bool,
) -> bool {
    let fed = |e: &Expr| leaves(ty, &reads(e)).iter().any(sendable);
    if !fed(e) {
        return true;
    }
    let Expr::And { args } = e else {
        return false;
    };
    let fields = bare(ty, value_types);
    let scope = WriteScope {
        after: &fields,
        before: None,
        retiring: false,
        meta: None,
        settings: None,
        now: Timestamp(0),
    };
    let broken = |a: &Expr| decided(ty, a) && !crate::expr::holds(a, &scope).unwrap_or(false);
    args.iter().any(|a| !fed(a) && broken(a))
}

/// The constraints of `ty` that the record [`bare`] breaks, with the
/// fields of the part that breaks. A create fills the key's fields from
/// its key, a value of its own: so a constraint counts only where it breaks
/// whatever the key holds, as one that reads no key field, through the
/// derived ones, or one that joins by `and` a part that reads none and
/// breaks, whose fields are then that part's. A part a setting or the clock
/// decides is not judged here: the create's own write judges it. `ty` is
/// as for [`fixed`].
#[doc(hidden)]
pub fn breaks<'t>(
    ty: &'t TypeDecl,
    value_types: &[ValueTypeDecl],
) -> Vec<(
    &'t lakelands_api::ontology::ConstraintDecl,
    BTreeSet<FieldName>,
)> {
    let fields = bare(ty, value_types);
    let scope = WriteScope {
        after: &fields,
        before: None,
        retiring: false,
        meta: None,
        settings: None,
        now: Timestamp(0),
    };
    let key: BTreeSet<FieldName> = ty.key.iter().cloned().collect();
    let keyless = |e: &Expr| leaves(ty, &reads(e)).is_disjoint(&key);
    let breaks = |e: &Expr| decided(ty, e) && !crate::expr::holds(e, &scope).unwrap_or(false);
    let broken = ty.constraints.iter().filter_map(|c| {
        let e = c.holds.typed().ok()?;
        if keyless(&e) && decided(ty, &e) {
            return breaks(&e).then(|| (c, reads(&e)));
        }
        let Expr::And { args } = &e else {
            return None;
        };
        let part: Vec<&Expr> = args.iter().filter(|a| keyless(a) && breaks(a)).collect();
        (!part.is_empty()).then(|| (c, part.into_iter().flat_map(reads).collect()))
    });
    broken.collect()
}

/// The context of one write, for the fold.
#[doc(hidden)]
pub struct Write<'a> {
    /// The record's fields before the write; `None` for a create.
    pub before: Option<&'a BTreeMap<FieldName, Value>>,
    /// The fields the write itself sets or clears.
    pub written: &'a BTreeSet<FieldName>,
    /// The provenance a field the fold changes takes.
    pub stamp: &'a dyn Fn(&FieldName) -> FieldMeta,
    /// The lake's settings, by key.
    pub settings: Option<&'a BTreeMap<String, Value>>,
    /// The `known_at` of the event.
    pub now: Timestamp,
}

/// Apply the fold's own rules to a record after a write's patch was
/// merged into `fields`, its derived fields computed in `order`, as
/// [`order`] answers it for `ty`. Answers the fields the fold changed
/// (cleared or recomputed). Each takes the event's provenance with
/// `origin: derived` in `meta` as it changes, so a field computed after it
/// reads that; a type that keeps none reads none.
#[doc(hidden)]
pub fn apply(
    ty: &TypeDecl,
    order: &[(FieldName, Expr)],
    value_types: &[ValueTypeDecl],
    fields: &mut BTreeMap<FieldName, Value>,
    meta: &mut BTreeMap<FieldName, FieldMeta>,
    write: &Write<'_>,
) -> BTreeSet<FieldName> {
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
                meta.insert(f.name.clone(), (write.stamp)(&f.name));
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
    for (name, expr) in order {
        let decl = ty
            .fields
            .iter()
            .find(|f| &f.name == name)
            .expect("ordered from the type");
        let scope = WriteScope {
            after: fields,
            before: write.before,
            retiring: false,
            meta: ty.field_meta.then_some(&*meta),
            settings: write.settings,
            now: write.now,
        };
        let value = match decl.at_read() {
            true => Value::Null,
            false => computed(decl, expr, value_types, &scope),
        };
        if fields.get(name).unwrap_or(&Value::Null) != &value {
            fields.insert(name.clone(), value);
            meta.insert(name.clone(), (write.stamp)(name));
            touched.insert(name.clone());
        }
    }
    touched
}

/// A record as one read answers it: each field computed at the read
/// computed from its fields as stored, their provenance, the lake's
/// settings and the read's clock, in the order the fold computes derived
/// fields; a value that cannot be computed is none. A type whose derived
/// fields cannot be ordered was refused when declared, and has none here.
#[doc(hidden)]
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
