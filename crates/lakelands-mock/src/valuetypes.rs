//! Value-type changes over the mock's state, classified as the store
//! classifies them: a loosening is declared, a tightening is migrated after
//! a preflight over every stored value, and a change of `kind` or `scale` is
//! neither.
//!
//! The preflight counts a value as a misfit by the mock's own check of a
//! write, which covers the storage kind and `one_of` and nothing more: the
//! mock evaluates no `constraint` and checks no `format`, so a tightening
//! of either finds no misfit here that the store would find.
//!
//! A migration that renames values renames them as the store does, in
//! every field of the value type but one a backed function computes, and
//! refuses what the store refuses: a renamed value in a key field, and one
//! a derived field's expression holds. Its report lists no conditions: the
//! mock reads no expression for a literal.

use lakelands_api::error::{Error, Refusal, Result};
use lakelands_api::id::{FieldName, Position, TypeName, ValueTypeName};
use lakelands_api::ontology::{FieldType, TypeDecl, ValueTypeDecl};
use lakelands_api::write::{
    Misfit, ValueRenamed, ValueTypeMigration, ValueTypeReport, ValueTypeUse,
};

use crate::state::State;
use crate::value;

fn invalid(message: String) -> Error {
    Error::Refused {
        field: None,
        reason: Refusal::Invalid { message },
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    Same,
    Loosening,
    Tightening,
    Reshaping,
}

/// How a value type's new declaration relates to the one in force.
pub(crate) fn change(current: &ValueTypeDecl, new: &ValueTypeDecl) -> Change {
    if current.kind != new.kind || current.scale != new.scale {
        return Change::Reshaping;
    }
    if current.one_of == new.one_of
        && current.constraint == new.constraint
        && current.format == new.format
    {
        return Change::Same;
    }
    let one_of = new.one_of.is_empty()
        || (!current.one_of.is_empty() && current.one_of.iter().all(|v| new.one_of.contains(v)));
    let constraint = new.constraint.is_none() || new.constraint == current.constraint;
    let format = new.format.is_none() || new.format == current.format;
    if one_of && constraint && format {
        Change::Loosening
    } else {
        Change::Tightening
    }
}

/// Refuse a redeclaration `declare` may not make.
pub(crate) fn check_declared(st: &State, new: &ValueTypeDecl) -> Result<()> {
    let Some(current) = st.value_types.iter().find(|v| v.name == new.name) else {
        return Ok(());
    };
    match change(current, new) {
        Change::Same | Change::Loosening => Ok(()),
        Change::Tightening => Err(invalid(format!(
            "value type `{}` would accept less than it does; tighten it through `migrate`",
            new.name
        ))),
        Change::Reshaping => Err(invalid(format!(
            "value type `{}` would change its `kind` or `scale`; that is not a value-type \
             migration",
            new.name
        ))),
    }
}

/// The types that use a value type, as the store names them: not a view,
/// with a field of it, in name order.
pub(crate) fn using_types(st: &State, vt: &ValueTypeName) -> Vec<TypeName> {
    let uses = |t: &TypeDecl| (t.fields.iter()).any(|f| f.ty.value_type.as_ref() == Some(vt));
    let types = st.types.values().map(|t| &t.decl);
    types
        .filter(|t| t.view.is_none() && uses(t))
        .map(|t| t.name.clone())
        .collect()
}

/// The fields of `t` a value type's renames reach: every one of the value
/// type but one a backed function computes.
pub(crate) fn renamed_fields(st: &State, t: &TypeDecl, vt: &ValueTypeName) -> Vec<FieldName> {
    let backed = |n: &str| {
        st.functions
            .iter()
            .any(|f| f.name.as_str() == n && f.backing != "expr")
    };
    let of = |f: &&lakelands_api::ontology::FieldDecl| {
        f.ty.value_type.as_ref() == Some(vt) && !f.function.as_deref().is_some_and(backed)
    };
    t.fields.iter().filter(of).map(|f| f.name.clone()).collect()
}

/// The using types whose fields' `initial` names an old value, declared
/// with the new one.
pub(crate) fn initials(st: &State, m: &ValueTypeMigration) -> Vec<TypeDecl> {
    let mut out = Vec::new();
    for ty in using_types(st, &m.to.name) {
        let mut t = st.types[&ty].decl.clone();
        let reached = renamed_fields(st, &t, &m.to.name);
        let mut changed = false;
        for f in t.fields.iter_mut().filter(|f| reached.contains(&f.name)) {
            if let Some(v) = f.initial.as_ref().and_then(|v| v.as_str())
                && m.renamed(v) != v
            {
                f.initial = Some(m.renamed(v).into());
                changed = true;
            }
        }
        if changed {
            out.push(t);
        }
    }
    out
}

/// The old value a stored one is, if the migration renames it.
fn old<'a>(m: &ValueTypeMigration, v: &'a serde_json::Value) -> Option<&'a str> {
    v.as_str().filter(|s| m.renamed(s) != *s)
}

/// Whether an expression holds, anywhere, an old value as a literal.
fn holds_old(e: &lakelands_api::expr::Expr, m: &ValueTypeMigration) -> bool {
    use lakelands_api::expr::{Expr, Literal, children};
    matches!(e, Expr::Lit { value: Literal::Text(t) } if m.renamed(t) != t)
        || children(e).any(|c| holds_old(c, m))
}

/// Whether a value type's declaration takes a text value, by the mock's
/// own check.
fn accepts(vt: &ValueTypeDecl, v: &str) -> bool {
    let ty = FieldType {
        kind: vt.kind,
        value_type: Some(vt.name.clone()),
        ..FieldType::default()
    };
    value::misfit(&ty, &v.into(), std::slice::from_ref(vt)).is_none()
}

/// The preflight: every stored value under the value type, renamed as
/// the migration renames it, checked against its new declaration.
pub(crate) fn preflight(
    st: &State,
    head: Position,
    m: &ValueTypeMigration,
) -> Result<ValueTypeReport> {
    let to = &m.to;
    let current = st
        .value_types
        .iter()
        .find(|v| v.name == to.name)
        .ok_or_else(|| invalid(format!("`{}` is not a declared value type", to.name)))?;
    match change(current, to) {
        Change::Same if m.renames.is_empty() => {
            return Err(invalid(format!("`{}`: nothing to migrate", to.name)));
        }
        Change::Reshaping => {
            return Err(invalid(format!(
                "value type `{}` would change its `kind` or `scale`; that is not a value-type \
                 migration",
                to.name
            )));
        }
        _ => {}
    }
    m.check_renames(current, accepts).map_err(invalid)?;
    let mut types: Vec<ValueTypeDecl> = st
        .value_types
        .iter()
        .filter(|v| v.name != to.name)
        .cloned()
        .collect();
    types.push(to.clone());
    let from_version = st
        .decl_versions
        .get(&format!("value_type:{}", to.name))
        .copied()
        .unwrap_or(1);
    let mut report = ValueTypeReport {
        value_type: to.name.clone(),
        from_version,
        records_scanned: 0,
        fields: Vec::new(),
        misfits: 0,
        first: Vec::new(),
        fits: true,
        report_id: String::new(),
        conditions: Vec::new(),
        conditions_unshown: false,
    };
    let old = |v| old(m, v);
    for name in using_types(st, &to.name) {
        let t = &st.types[&name];
        let uses: Vec<_> = t
            .decl
            .fields
            .iter()
            .filter(|f| f.ty.value_type.as_ref() == Some(&to.name))
            .collect();
        let reached = renamed_fields(st, &t.decl, &to.name);
        for f in uses.iter().filter(|f| reached.contains(&f.name)) {
            let derived = f.derived.as_ref().and_then(|c| crate::expr::typed(c).ok());
            if derived.is_some_and(|e| holds_old(&e, m)) {
                return Err(invalid(format!(
                    "`{name}`.`{}` is computed from an expression that holds a value this \
                     migration renames; change the expression first",
                    f.name
                )));
            }
        }
        let records: Vec<_> = st.records.values().filter(|r| r.ty == name).collect();
        report.records_scanned += records.len() as u64;
        for f in uses {
            let reaches = reached.contains(&f.name);
            // An `initial` the new declaration refuses is a misfit of its
            // field, counted as the store counts it.
            let initial = (f.initial.as_ref()).map(|v| match old(v).filter(|_| reaches) {
                Some(o) => m.renamed(o).into(),
                None => v.clone(),
            });
            let unfit = initial.is_some_and(|v| value::misfit(&f.ty, &v, &types).is_some());
            let mut used = ValueTypeUse {
                ty: name.clone(),
                field: f.name.clone(),
                values_scanned: 0,
                misfits: u64::from(unfit),
                renamed: (m.renames.iter())
                    .map(|(from, to)| ValueRenamed {
                        from: from.clone(),
                        to: to.clone(),
                        records: 0,
                    })
                    .collect(),
            };
            for r in &records {
                let Some(v) = r.fields.get(&f.name).filter(|v| !v.is_null()) else {
                    continue;
                };
                used.values_scanned += 1;
                let misfit = match (old(v), reaches) {
                    (Some(o), true) => {
                        if t.decl.key.contains(&f.name) {
                            return Err(invalid(format!(
                                "`{name}`.`{}` is part of the key and holds a value this \
                                 migration renames, which would leave records whose key no \
                                 longer matches their fields; re-key `{name}` through its \
                                 migration first",
                                f.name
                            )));
                        }
                        let pair = used.renamed.iter_mut().find(|p| p.from == o);
                        pair.into_iter().for_each(|p| p.records += 1);
                        value::misfit(&f.ty, &m.renamed(o).into(), &types).is_some()
                    }
                    (Some(_), false) => true,
                    (None, _) => value::misfit(&f.ty, v, &types).is_some(),
                };
                if misfit {
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
    let mut id = serde_json::json!({
        "report": serde_json::to_value(&report).unwrap_or_default(),
        "to": serde_json::to_value(to).unwrap_or_default(),
        "head": head.0,
    });
    if !m.renames.is_empty() {
        id["renames"] = serde_json::json!(m.renames);
    }
    report.report_id = lakelands_api::canonical::sha256_hex(&id);
    Ok(report)
}
