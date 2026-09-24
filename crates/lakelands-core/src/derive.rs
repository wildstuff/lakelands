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
    match e {
        Expr::Path { path } => {
            if let Some(first) = path.first() {
                out.insert(first.clone());
            }
        }
        Expr::Lit { .. } => {}
        Expr::Cmp { lhs, rhs, .. } | Expr::Arith { lhs, rhs, .. } => {
            collect(lhs, out);
            collect(rhs, out);
        }
        Expr::Neg { arg } | Expr::Not { arg } | Expr::IsNull { arg } => collect(arg, out),
        Expr::And { args } | Expr::Or { args } | Expr::Call { args, .. } => {
            args.iter().for_each(|a| collect(a, out));
        }
        Expr::In { arg, list } => {
            collect(arg, out);
            list.iter().for_each(|a| collect(a, out));
        }
    }
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
            .and_then(value_type_scale);
        let scope = WriteScope {
            after: fields,
            before: write.before,
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

/// The decimal scale a value type declares, if any.
fn value_type_scale(vt: &ValueTypeDecl) -> Option<u32> {
    vt.scale
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ty(v: Value) -> TypeDecl {
        serde_json::from_value(v).unwrap()
    }

    fn fields(v: Value) -> BTreeMap<FieldName, Value> {
        serde_json::from_value(v).unwrap()
    }

    fn text(name: &str) -> Value {
        json!({"name": name, "type": {"kind": "text"}})
    }

    fn decimal(name: &str, derived: Option<&str>) -> Value {
        let mut f = json!({"name": name, "type": {"kind": "decimal"}});
        if let Some(d) = derived {
            f["derived"] = json!(d);
            f["authority"] = json!("derived");
        }
        f
    }

    #[test]
    fn derived_fields_are_ordered_by_what_they_read() {
        let t = ty(json!({"name": "invoice", "fields": [
            decimal("net", Some("gross - vat")),
            decimal("vat", Some("gross * 0.19")),
            decimal("gross", None),
        ]}));
        let names: Vec<String> = order(&t)
            .unwrap()
            .into_iter()
            .map(|(n, _)| n.to_string())
            .collect();
        assert_eq!(names, ["vat", "net"]);
    }

    #[test]
    fn cycles_and_self_reads_are_refused() {
        let cyc = ty(
            json!({"name": "t", "fields": [decimal("a", Some("b + 1")), decimal("b", Some("a + 1"))]}),
        );
        assert!(order(&cyc).unwrap_err().0.contains("cycle"));
        let own = ty(json!({"name": "t", "fields": [decimal("a", Some("a + 1"))]}));
        assert!(order(&own).unwrap_err().0.contains("itself"));
        let bad = ty(json!({"name": "t", "fields": [decimal("a", Some("1 +"))]}));
        assert!(order(&bad).is_err());
    }

    #[test]
    fn derived_values_recompute_in_order_and_clear_when_they_cannot() {
        let t = ty(json!({"name": "invoice", "fields": [
            decimal("gross", None),
            decimal("vat", Some("gross * 0.19")),
            decimal("net", Some("gross - vat")),
        ]}));
        let mut f = fields(json!({"gross": "100.00"}));
        let written: BTreeSet<FieldName> = ["gross".into()].into();
        let w = Write {
            before: None,
            written: &written,
            settings: None,
            now: Timestamp(0),
        };
        let touched = apply(&t, &[], &mut f, &w).unwrap();
        assert_eq!(f[&FieldName::from("vat")], json!("19.0000"));
        assert_eq!(f[&FieldName::from("net")], json!("81.0000"));
        assert_eq!(touched.len(), 2);

        f.insert("gross".into(), Value::Null);
        apply(&t, &[], &mut f, &w).unwrap();
        assert_eq!(
            f[&FieldName::from("vat")],
            Value::Null,
            "absent input clears"
        );
    }

    #[test]
    fn a_derived_decimal_is_rounded_to_its_scale_before_the_next_one_reads_it() {
        let money: ValueTypeDecl =
            serde_json::from_value(json!({"name": "money", "kind": "decimal", "scale": 2}))
                .unwrap();
        let m = |name: &str, derived: Option<&str>| {
            let mut f = decimal(name, derived);
            f["type"]["value_type"] = json!("money");
            f
        };
        let t = ty(json!({"name": "invoice", "fields": [
            m("gross", None),
            m("vat", Some("gross * 19 / 119")),
            m("net", Some("gross - vat")),
        ]}));
        let mut f = fields(json!({"gross": "100.00"}));
        let written: BTreeSet<FieldName> = ["gross".into()].into();
        let w = Write {
            before: None,
            written: &written,
            settings: None,
            now: Timestamp(0),
        };
        apply(&t, &[money], &mut f, &w).unwrap();
        assert_eq!(f[&FieldName::from("vat")], json!("15.97"));
        assert_eq!(
            f[&FieldName::from("net")],
            json!("84.03"),
            "net + vat reconciles to gross"
        );
    }

    #[test]
    fn a_derived_decimal_is_written_at_its_scale_even_when_computed_at_a_smaller_one() {
        let money: ValueTypeDecl =
            serde_json::from_value(json!({"name": "money", "kind": "decimal", "scale": 2}))
                .unwrap();
        let mut total = decimal("total", Some("quantity * 3"));
        total["type"]["value_type"] = json!("money");
        let t = ty(json!({"name": "line", "fields": [decimal("quantity", None), total]}));
        let mut f = fields(json!({"quantity": "2"}));
        let written: BTreeSet<FieldName> = ["quantity".into()].into();
        let w = Write {
            before: None,
            written: &written,
            settings: None,
            now: Timestamp(0),
        };
        apply(&t, &[money], &mut f, &w).unwrap();
        assert_eq!(f[&FieldName::from("total")], json!("6.00"));
    }

    #[test]
    fn a_dependent_is_cleared_when_its_input_changes_unless_the_write_sets_it() {
        let t = ty(json!({"name": "decl", "fields": [
            text("body"),
            {"name": "verdict", "type": {"kind": "text"}, "depends_on": ["body"], "on_input_change": "clear"},
            {"name": "since", "type": {"kind": "text"}, "depends_on": ["verdict"], "on_input_change": "clear"},
            {"name": "note", "type": {"kind": "text"}, "depends_on": ["body"], "on_input_change": "keep"},
        ]}));
        let mut f = fields(json!({"body": "v2", "verdict": "ok", "since": "2026", "note": "n"}));
        let body: BTreeSet<FieldName> = ["body".into()].into();
        let w = Write {
            before: None,
            written: &body,
            settings: None,
            now: Timestamp(0),
        };
        let touched = apply(&t, &[], &mut f, &w).unwrap();
        assert_eq!(f[&FieldName::from("verdict")], Value::Null);
        assert_eq!(
            f[&FieldName::from("since")],
            Value::Null,
            "clearing is itself a change"
        );
        assert_eq!(f[&FieldName::from("note")], json!("n"), "keep keeps");
        assert_eq!(touched, ["since".into(), "verdict".into()].into());

        let mut g = fields(json!({"body": "v3", "verdict": "fresh"}));
        let both: BTreeSet<FieldName> = ["body".into(), "verdict".into()].into();
        let w = Write {
            before: None,
            written: &both,
            settings: None,
            now: Timestamp(0),
        };
        apply(&t, &[], &mut g, &w).unwrap();
        assert_eq!(g[&FieldName::from("verdict")], json!("fresh"));
    }

    #[test]
    fn a_write_that_touches_no_input_clears_nothing() {
        let t = ty(json!({"name": "decl", "fields": [
            text("body"), text("other"),
            {"name": "verdict", "type": {"kind": "text"}, "depends_on": ["body"], "on_input_change": "clear"},
        ]}));
        let mut f = fields(json!({"body": "v", "other": "x", "verdict": "ok"}));
        let other: BTreeSet<FieldName> = ["other".into()].into();
        let w = Write {
            before: None,
            written: &other,
            settings: None,
            now: Timestamp(0),
        };
        assert!(apply(&t, &[], &mut f, &w).unwrap().is_empty());
        assert_eq!(f[&FieldName::from("verdict")], json!("ok"));
    }
}
