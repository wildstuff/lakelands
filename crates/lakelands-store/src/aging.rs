//! Aging: on a date, move a field of the records of a type from one value
//! to another where a condition holds
//! (`docs/decisions/036-aging-runs-by-date-once.md`).
//!
//! A rule is a `sys.aging_rule` record, checked by [`check`] when it is
//! written and again when it runs: the type or its field may have changed
//! since, and a rule that no longer fits is skipped with the reason, never
//! run half. Its author's rights are checked once, when it is written
//! ([`check_authority`]); the run carries them. [`run`] holds the writer's lock for the whole date and reads
//! every live record of a rule's type at the head, restricted ones
//! included: aging is the host's maintenance, not a principal's read.
//!
//! Each move is an ordinary record write, planned as `upsert` plans one,
//! written by the system with the rule as its procedure and the id of
//! `docs/log-format.md` §3; triggers and object sets react in the same
//! append. A move whose id is already in the lake is not made again, so
//! aging a date twice writes it once. A condition that cannot be evaluated
//! for a record, or a move the store would refuse for it, leaves the record
//! as it is and is counted as a failed guard.

use std::collections::BTreeMap;

use lakelands_api::event::Procedure;
use lakelands_api::expr::{Condition, Expr};
use lakelands_api::ontology::Authority;
use lakelands_api::principal::{WriteContext, WriteOrigin};
use lakelands_api::record::Origin;
use lakelands_api::write::{AgeReport, RuleRun};
use lakelands_api::{FieldName, Patch, Position, Principal, Result, Timestamp, TypeName, Value};
use lakelands_core::derive;
use lakelands_core::expr::{self, Place, RecordScope};
use lakelands_core::rights::{self, Door};
use lakelands_core::{canonical, names};

use crate::declare;
use crate::lake::{AGING_RULE_TYPE, Lake, every};
use crate::log;
use crate::ontology::Ontology;
use crate::writes::{self, Call, Pending};

const MICROS_PER_DAY: i64 = 86_400 * 1_000_000;

/// A rule that fits the lake's shape: what it moves, and when.
pub struct Rule {
    /// The type it ages.
    pub on: TypeName,
    /// The field it moves.
    pub field: FieldName,
    /// The value that ages, as the field holds it.
    pub from: Value,
    /// The value it becomes.
    pub to: Value,
    /// The condition, over the record's fields.
    pub when: Expr,
}

/// Check a rule's fields against the ontology: the rule, or the rule field
/// at fault and why.
pub fn check(
    ontology: &Ontology,
    fields: &BTreeMap<FieldName, Value>,
) -> std::result::Result<Rule, (FieldName, String)> {
    let text = |name: &str| -> std::result::Result<String, (FieldName, String)> {
        match fields.get(name) {
            Some(Value::String(s)) => Ok(s.clone()),
            _ => Err((name.into(), format!("an aging rule's `{name}` is text"))),
        }
    };
    let name = text("name")?;
    if let Some(m) = names::fault("aging rule", &name) {
        return Err(("name".into(), m));
    }
    let on = TypeName::from(text("on")?.as_str());
    if on.as_str().starts_with("sys.") {
        return Err((
            "on".into(),
            format!("`{on}` is a system type; aging never moves it"),
        ));
    }
    let Ok(decl) = ontology.ty(&on) else {
        return Err(("on".into(), format!("`{on}` is not a declared type")));
    };
    if decl.view.is_some() {
        return Err((
            "on".into(),
            format!("`{on}` is a view; it is never written"),
        ));
    }
    let field = FieldName::from(text("field")?.as_str());
    let Some(f) = decl.fields.iter().find(|f| f.name == field) else {
        return Err(("field".into(), format!("`{on}` has no field `{field}`")));
    };
    if !matches!(f.authority, Authority::Authored | Authority::Shared) {
        return Err((
            "field".into(),
            format!(
                "`{on}`.`{field}` is not `authored` or `shared`; aging writes it as `upsert` does"
            ),
        ));
    }
    if decl.key.contains(&field) {
        return Err((
            "field".into(),
            format!("`{on}`.`{field}` is part of the key; aging never moves a key"),
        ));
    }
    let value = |name: &str| -> std::result::Result<Value, (FieldName, String)> {
        let v = fields.get(name).filter(|v| !v.is_null()).ok_or_else(|| {
            (
                name.into(),
                format!("an aging rule's `{name}` holds a value"),
            )
        })?;
        let patch: Patch = [(field.clone(), v.clone())].into();
        let checked = writes::check_values(ontology, decl, &patch, &decl.fields)
            .map_err(|e| (name.into(), format!("not a value of `{on}`.`{field}`: {e}")))?;
        Ok(checked[&field].clone())
    };
    let from = value("from")?;
    let to = value("to")?;
    if canonical::to_vec(&from) == canonical::to_vec(&to) {
        return Err((
            "to".into(),
            "`to` equals `from`; a rule moves a value to another".into(),
        ));
    }
    let when = text("when")?;
    let when = declare::condition(
        &Condition::Text(when),
        Place::Record,
        "an aging rule's `when`",
    )
    .map_err(|e| ("when".into(), crate::backed::why(&e)))?;
    Ok(Rule {
        on,
        field,
        from,
        to,
        when,
    })
}

/// Refuse a rule its author could not have made by hand: the run moves
/// `field` as the system, so the author must hold write on it; and `when`
/// decides, record by record, whether a field is moved, so every field it
/// reads is probed and needs the author's right to read it
/// (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`).
/// Refused as not granted, naming the field of `on`.
pub fn check_authority(who: &Principal, ontology: &Ontology, rule: &Rule) -> Result<()> {
    let decl = ontology.ty(&rule.on)?;
    let patch: Patch = [(rule.field.clone(), rule.to.clone())].into();
    rights::check_write(who, decl, Door::Authored, &patch)?;
    rights::check_probe(who, decl, derive::reads(&rule.when).iter())
}

/// Run every live rule once for `today`, in rule-name order, one append
/// per rule that moves anything.
pub fn run(lake: &Lake, today: Timestamp) -> Result<AgeReport> {
    // `today` is a date: a time other than UTC midnight is refused.
    if today.0.rem_euclid(MICROS_PER_DAY) != 0 {
        return Err(declare::invalid_field(
            &"today".into(),
            format!("`today` is a date, at UTC midnight; `{today}` is not"),
        ));
    }
    let system = lake.system("lakelands");
    let call = Call {
        lake,
        who: &system,
        context: WriteContext {
            cause: None,
            origin: WriteOrigin::Scheduled,
            depth: 0,
        },
        followed: None,
    };
    let all = every(false);
    let mut w = lake.for_write()?;
    let mut rules = lake
        .engine
        .view(None)?
        .scan(&TypeName::from(AGING_RULE_TYPE), &all)?;
    rules.sort_by(|a, b| a.key.as_str().cmp(b.key.as_str()));
    let mut report = AgeReport {
        date: today,
        rules: Vec::new(),
        transitions: 0,
        failed_guards: 0,
        position: Position::GENESIS,
    };
    for r in rules {
        let mut ran = RuleRun {
            rule: r.key.to_string(),
            version: r.version,
            moved: 0,
            failed_guards: 0,
            skipped: None,
        };
        let rule = match check(&w.ontology, &r.fields) {
            Ok(rule) => rule,
            Err((field, why)) => {
                ran.skipped = Some(format!("`{field}`: {why}"));
                report.rules.push(ran);
                continue;
            }
        };
        let procedure = Procedure {
            name: format!("aging:{}", ran.rule),
            version: r.version.0.to_string(),
        };
        let view = lake.engine.view(None)?;
        let now = lake.clock(&mut w);
        let from = canonical::to_vec(&rule.from);
        let mut pending = Pending::default();
        let mut drafts = Vec::new();
        for record in view.scan(&rule.on, &all)? {
            if record.fields.get(&rule.field).map(canonical::to_vec) != Some(from.clone()) {
                continue;
            }
            let scope = RecordScope {
                fields: &record.fields,
                settings: Some(&w.settings),
                now: today,
            };
            match expr::holds(&rule.when, &scope) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(_) => {
                    ran.failed_guards += 1;
                    continue;
                }
            }
            let id = log::aging_id(&ran.rule, &rule.on, &record.key, today);
            if view.applied(&id)?.is_some() {
                continue;
            }
            let stamp = |d: &mut log::Draft| {
                d.origin = Origin::System;
                d.procedure = Some(procedure.clone());
                d.id = Some(id.clone());
            };
            let patch: Patch = [(rule.field.clone(), rule.to.clone())].into();
            let n = drafts.len();
            let plan = match call.plan_record(
                &w,
                view.as_ref(),
                &pending,
                &rule.on,
                &record.key,
                &patch,
                Door::Authored,
                None,
                now,
                n,
                &system,
                &stamp,
            ) {
                Ok(plan) => plan,
                Err(_) => {
                    ran.failed_guards += 1;
                    continue;
                }
            };
            let at = writes::position_after(&w, n);
            drafts.extend(call.planned(&w, view.as_ref(), &mut pending, plan, at, now)?);
            ran.moved += 1;
        }
        drop(view);
        if !drafts.is_empty() {
            lake.append(&mut w, drafts)?;
        }
        report.transitions += ran.moved;
        report.failed_guards += ran.failed_guards;
        report.rules.push(ran);
    }
    report.position = w.tip.as_ref().map_or(Position::GENESIS, |t| t.0);
    Ok(report)
}
