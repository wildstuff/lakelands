//! Aging over the mock's log: the rule check the store makes when a rule
//! is written and when it runs, and `age` itself, with the store's moves,
//! ids and report. The mock keeps no restrictions and computes no
//! triggers or object sets, so a move is the one `record_upserted`.

use std::collections::BTreeMap;

use lakelands_api::error::{Error, Refusal, Result};
use lakelands_api::event::{EventBody, EventId, Procedure};
use lakelands_api::expr::{Condition, Expr, Func};
use lakelands_api::id::{FieldName, Position, RecordKey, TypeName, Version};
use lakelands_api::ontology::Authority;
use lakelands_api::principal::{WriteContext, WriteOrigin};
use lakelands_api::record::Value;
use lakelands_api::time::Timestamp;
use lakelands_api::write::{AgeReport, RuleRun};
use sha2::{Digest as _, Sha256};

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

/// Check a rule's fields: the rule, or the rule field at fault and why.
pub(crate) fn check(
    st: &State,
    fields: &BTreeMap<FieldName, Value>,
) -> std::result::Result<Rule, (FieldName, String)> {
    let text = |name: &str| match fields.get(name) {
        Some(Value::String(s)) => Ok(s.clone()),
        _ => Err((FieldName::from(name), format!("`{name}` is text"))),
    };
    text("name")?;
    let on = TypeName::from(text("on")?.as_str());
    if on.as_str().starts_with("sys.") {
        return Err(("on".into(), format!("`{on}` is a system type")));
    }
    let t = st
        .types
        .get(&on)
        .filter(|t| t.decl.view.is_none())
        .ok_or_else(|| (FieldName::from("on"), format!("`{on}` is no written type")))?;
    let field = FieldName::from(text("field")?.as_str());
    let f = state::field(&t.decl, &field)
        .filter(|f| matches!(f.authority, Authority::Authored | Authority::Shared))
        .filter(|f| !t.decl.key.contains(&f.name))
        .ok_or_else(|| {
            (
                FieldName::from("field"),
                format!("`{field}` is no authored or shared field of `{on}` outside its key"),
            )
        })?;
    let value = |name: &str| {
        let v = fields
            .get(name)
            .filter(|v| !v.is_null())
            .ok_or_else(|| (FieldName::from(name), format!("`{name}` holds a value")))?;
        if let Some(m) = crate::value::misfit(&f.ty, v, &st.value_types) {
            return Err((FieldName::from(name), m));
        }
        Ok(crate::value::canonical(&f.ty, v))
    };
    let (from, to) = (value("from")?, value("to")?);
    if crate::value::canonical_bytes(&from) == crate::value::canonical_bytes(&to) {
        return Err(("to".into(), "`to` equals `from`".into()));
    }
    let when = expr::typed(&Condition::Text(text("when")?))
        .ok()
        .filter(|e| !writes_only(e))
        .ok_or_else(|| {
            (
                FieldName::from("when"),
                "`when` is a condition over the record, without `changed` or `previous`".into(),
            )
        })?;
    Ok(Rule {
        on,
        field,
        from,
        to,
        when,
    })
}

/// Whether an expression reads `changed` or `previous`, which have
/// meaning only in a write.
fn writes_only(e: &Expr) -> bool {
    match e {
        Expr::Lit { .. } | Expr::Path { .. } => false,
        Expr::Cmp { lhs, rhs, .. } | Expr::Arith { lhs, rhs, .. } => {
            writes_only(lhs) || writes_only(rhs)
        }
        Expr::Neg { arg } | Expr::Not { arg } | Expr::IsNull { arg } => writes_only(arg),
        Expr::Call { func, args } => {
            matches!(func, Func::Changed | Func::Previous) || args.iter().any(writes_only)
        }
        Expr::And { args } | Expr::Or { args } => args.iter().any(writes_only),
        Expr::In { arg, list } => writes_only(arg) || list.iter().any(writes_only),
    }
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
            reason: Refusal::Invalid {
                message: "`today` is a date, at UTC midnight".into(),
            },
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
                ran.skipped = Some(format!("`{field}`: {why}"));
                report.rules.push(ran);
                continue;
            }
        };
        let from = crate::value::canonical_bytes(&rule.from);
        let mut batch = Batch::at(log);
        for rec in st
            .records
            .values()
            .filter(|x| x.ty == rule.on && !x.retired)
        {
            if rec
                .fields
                .get(&rule.field)
                .map(crate::value::canonical_bytes)
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
