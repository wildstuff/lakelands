//! Aging over the mock's log: the rule check the store makes when a rule
//! is written and when it runs, and `age` itself, with the store's moves,
//! ids and report. The mock keeps no restrictions and computes no
//! triggers or object sets, so a move is the one `record_upserted`.

use std::collections::BTreeMap;

use lakelands_api::error::{Case, Error, Naming, Place, Refusal, Result, Said};
use lakelands_api::event::{EventBody, EventId, Procedure};
use lakelands_api::expr::{Condition, Expr};
use lakelands_api::id::{FieldName, Position, RecordKey, TypeName, Version};
use lakelands_api::ontology::Authority;
use lakelands_api::principal::{WriteContext, WriteOrigin};
use lakelands_api::record::Value;
use lakelands_api::time::Timestamp;
use lakelands_api::write::{AgeReport, RuleRun};
use sha2::{Digest as _, Sha256};

use crate::doors::rules;
use crate::expr::{self, Scope};
use crate::state::{self, State};
use crate::{Batch, Draft, Log, Shared};

/// The system type an aging rule is a record of.
pub(crate) const RULE_TYPE: &str = lakelands_api::seam::AGING_RULE_TYPE;

const MICROS_PER_DAY: i64 = 86_400 * 1_000_000;

pub(crate) struct Rule {
    pub(crate) on: TypeName,
    pub(crate) field: FieldName,
    pub(crate) from: Value,
    pub(crate) to: Value,
    pub(crate) when: Expr,
}

/// The rule field at fault, and why.
type Fault = (FieldName, Box<Said>);

/// Check a rule's fields as the store checks them: the rule, or its fault.
pub(crate) fn check(
    st: &State,
    fields: &BTreeMap<FieldName, Value>,
) -> std::result::Result<Rule, Fault> {
    let wrong = |member: &str, case: Case| -> Fault { (member.into(), Box::new(case.into())) };
    let text = |member: &str| match fields.get(member) {
        Some(Value::String(s)) => Ok(s.clone()),
        _ => {
            let case = Case::AgingNotText {
                member: member.into(),
            };
            Err(wrong(member, case))
        }
    };
    let name = text("name")?;
    if let Some(c) = rules::fault(Naming::AgingRule, &name) {
        return Err(wrong("name", c));
    }
    let on = TypeName::from(text("on")?.as_str());
    let ty = on.clone();
    if on.as_str().starts_with("sys.") {
        return Err(wrong("on", Case::AgingSystemType { ty }));
    }
    let Some(t) = st.types.get(&on) else {
        return Err(wrong("on", Case::Undeclared { ty }));
    };
    if t.decl.view.is_some() {
        return Err(wrong("on", Case::IsAView { ty }));
    }
    let field = FieldName::from(text("field")?.as_str());
    let Some(f) = state::field(&t.decl, &field) else {
        return Err(wrong("field", Case::UnknownField { ty, field }));
    };
    if !matches!(f.authority, Authority::Authored | Authority::Shared) {
        return Err(wrong("field", Case::AgingAuthority { ty, field }));
    }
    if t.decl.key.contains(&f.name) {
        return Err(wrong("field", Case::AgingKey { ty, field }));
    }
    let value = |member: &str| {
        let Some(v) = fields.get(member).filter(|v| !v.is_null()) else {
            let case = Case::AgingNoValue {
                member: member.into(),
            };
            return Err(wrong(member, case));
        };
        if let Some(c) = crate::value::misfit(&f.ty, v, &st.value_types) {
            let cause = Box::new(Error::Refused {
                field: Some(field.clone()),
                reason: Refusal::Constraint(Box::new(c.into())),
            });
            let (ty, field) = (on.clone(), field.clone());
            return Err(wrong(member, Case::AgingNotAValue { ty, field, cause }));
        }
        Ok(crate::value::canonical(&f.ty, v))
    };
    let (from, to) = (value("from")?, value("to")?);
    if lakelands_api::canonical::to_vec(&from) == lakelands_api::canonical::to_vec(&to) {
        return Err(wrong("to", Case::AgingSameValue));
    }
    let when = Condition::Text(text("when")?);
    let when = rules::condition(&when, rules::Eval::Record, &[]).map_err(|e| {
        let said = e.case().cloned().expect("a condition's refusal");
        (FieldName::from("when"), Box::new(said))
    })?;
    Ok(Rule {
        on,
        field,
        from,
        to,
        when,
    })
}

/// The id of a move, as `docs/log-format.md` §3 gives it.
fn aging_id(rule: &str, ty: &TypeName, key: &RecordKey, date: Timestamp) -> EventId {
    let date = date.to_string();
    let parts: [&[u8]; 5] = [
        b"lakelands:aging:v1",
        rule.as_bytes(),
        ty.as_str().as_bytes(),
        key.as_str().as_bytes(),
        date.as_bytes(),
    ];
    let mut h = Sha256::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h.update([0u8]);
        }
        h.update(p);
    }
    EventId(
        h.finalize()[..16]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}

/// Run every live rule once for `today`, in rule-name order, one append
/// per rule that moves anything.
pub(crate) fn run(shared: &Shared, log: &mut Log, today: Timestamp) -> Result<AgeReport> {
    if today.0.rem_euclid(MICROS_PER_DAY) != 0 {
        return Err(Error::Refused {
            field: Some("today".into()),
            reason: Refusal::Invalid(Box::new(Case::AgingNotADate { today }.into())),
        });
    }
    let rule_type = TypeName::from(RULE_TYPE);
    let rules: Vec<_> = State::fold(&log.events)
        .records
        .into_values()
        .filter(|r| r.ty == rule_type && !r.retired)
        .collect();
    let mut report = AgeReport {
        date: today,
        rules: Vec::new(),
        transitions: 0,
        failed_guards: 0,
        position: Position::GENESIS,
    };
    for r in rules {
        let st = State::fold(&log.events);
        let mut ran = RuleRun {
            rule: r.key.to_string(),
            version: r.version,
            moved: 0,
            failed_guards: 0,
            skipped: None,
        };
        let rule = match check(&st, &r.fields) {
            Ok(rule) => rule,
            Err((field, why)) => {
                let why = why.within(Place::Field { name: field });
                ran.skipped = Some(why.to_string());
                report.rules.push(ran);
                continue;
            }
        };
        let from = lakelands_api::canonical::to_vec(&rule.from);
        let mut batch = Batch::at(log);
        for rec in st
            .records
            .values()
            .filter(|x| x.ty == rule.on && !x.retired)
        {
            if rec
                .fields
                .get(&rule.field)
                .map(lakelands_api::canonical::to_vec)
                != Some(from.clone())
            {
                continue;
            }
            let scope = Scope {
                ty: rule.on.as_str(),
                fields: &rec.fields,
                payload: None,
                now: Some(today),
            };
            match expr::holds(&rule.when, &scope) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(_) => {
                    ran.failed_guards += 1;
                    continue;
                }
            }
            let id = aging_id(&ran.rule, &rule.on, &rec.key, today);
            if log.events.iter().any(|e| e.id == id) {
                continue;
            }
            let mut d = Draft::system(EventBody::RecordUpserted {
                ty: rule.on.clone(),
                key: rec.key.clone(),
                version: Version(rec.version.0 + 1),
                patch: [(rule.field.clone(), rule.to.clone())].into(),
                made: false,
            });
            d.procedure = Some(Procedure {
                name: format!("aging:{}", ran.rule),
                version: r.version.0.to_string(),
            });
            d.id = Some(id);
            batch.push(d);
            ran.moved += 1;
        }
        if ran.moved > 0 {
            let context = WriteContext {
                cause: None,
                origin: WriteOrigin::Scheduled,
                depth: 0,
            };
            shared.append(log, context, batch);
        }
        report.transitions += ran.moved;
        report.failed_guards += ran.failed_guards;
        report.rules.push(ran);
    }
    report.position = Shared::head(log);
    Ok(report)
}
