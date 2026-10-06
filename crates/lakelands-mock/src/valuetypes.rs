//! Value-type changes over the mock's state, classified as the store
//! classifies them: a loosening is declared, a tightening is migrated after
//! a preflight over every stored value, and a change of `kind` or `scale` is
//! neither.
//!
//! The preflight counts a value as a misfit by the mock's own check of a
//! write, which covers the storage kind and `one_of` and nothing more: the
//! mock evaluates no `constraint` and checks no `format`, so a tightening
//! of either finds no misfit here that the store would find.

use lakelands_api::error::{Error, Refusal, Result};
use lakelands_api::id::Position;
use lakelands_api::ontology::ValueTypeDecl;
use lakelands_api::write::{Misfit, ValueTypeReport, ValueTypeUse};

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

/// The preflight: every stored value under the value type, checked
/// against its new declaration.
pub(crate) fn preflight(st: &State, head: Position, to: &ValueTypeDecl) -> Result<ValueTypeReport> {
    let current = st
        .value_types
        .iter()
        .find(|v| v.name == to.name)
        .ok_or_else(|| invalid(format!("`{}` is not a declared value type", to.name)))?;
    match change(current, to) {
        Change::Same => return Err(invalid(format!("`{}`: nothing to migrate", to.name))),
        Change::Reshaping => {
            return Err(invalid(format!(
                "value type `{}` would change its `kind` or `scale`; that is not a value-type \
                 migration",
                to.name
            )));
        }
        _ => {}
    }
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
    };
    for (name, t) in &st.types {
        let uses: Vec<_> = t
            .decl
            .fields
            .iter()
            .filter(|f| f.ty.value_type.as_ref() == Some(&to.name))
            .collect();
        if uses.is_empty() || t.decl.view.is_some() {
            continue;
        }
        let records: Vec<_> = st.records.values().filter(|r| &r.ty == name).collect();
        report.records_scanned += records.len() as u64;
        for f in uses {
            // An `initial` the new declaration refuses is a misfit of its
            // field, counted as the store counts it.
            let initial = f.initial.as_ref();
            let unfit = initial.is_some_and(|v| value::misfit(&f.ty, v, &types).is_some());
            let mut used = ValueTypeUse {
                ty: name.clone(),
                field: f.name.clone(),
                values_scanned: 0,
                misfits: u64::from(unfit),
            };
            for r in &records {
                let Some(v) = r.fields.get(&f.name).filter(|v| !v.is_null()) else {
                    continue;
                };
                used.values_scanned += 1;
                if value::misfit(&f.ty, v, &types).is_some() {
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
    let id = serde_json::json!({
        "report": serde_json::to_value(&report).unwrap_or_default(),
        "to": serde_json::to_value(to).unwrap_or_default(),
        "head": head.0,
    });
    report.report_id = lakelands_api::canonical::sha256_hex(&id);
    Ok(report)
}
