//! Aggregates: expressions over a set of records rather than one.
//!
//! `count`, `sum`, `min`, `max` and `avg` fold a group of records. They
//! appear in `find`'s aggregate and in a view's measures; a measure may
//! also compute with the group's own values and with measures declared
//! before it (`margin = sum(revenue) - sum(cost)`, then
//! `rate = margin / sum(revenue)`).
//!
//! - `count()` counts the records; `count(x)` counts those where `x` is
//!   not null.
//! - `sum(x)` adds exactly, as decimals; it skips nulls, and a value that
//!   is not a number is an error, never silently skipped. The sum of no
//!   values is 0.
//! - `min(x)` and `max(x)` order values as a comparison does
//!   (`crate::expr::eval`); nulls are skipped, no values give null.
//! - `avg(x)` is `sum(x) / count(x)`, exactly, and null when nothing is
//!   counted.
//!
//! Groups are formed by the values of the grouping fields (a null is a
//! value like any other here) and returned in the order of their key's
//! canonical bytes, so every engine answers in the same order.

use std::collections::BTreeMap;

use lakelands_api::expr::{CmpOp, Condition, Expr, Func};
use lakelands_api::read::{AggFunc, Aggregate, Group, GroupKey};
use lakelands_api::{FieldName, Timestamp, Value};
use rust_decimal::Decimal;

use super::eval::{EvalError, RecordScope, Result, Scope, Val, compare, eval};

/// The scope of one group: its records, its grouping values, and the
/// measures computed before the one being evaluated.
pub struct GroupScope<'a> {
    /// The group's records.
    pub rows: &'a [&'a BTreeMap<FieldName, Value>],
    /// The grouping fields' values.
    pub by: &'a BTreeMap<FieldName, Value>,
    /// Measures already computed, by name.
    pub earlier: &'a BTreeMap<FieldName, Value>,
    /// The lake's settings by key, as of the position read.
    pub settings: Option<&'a BTreeMap<String, Value>>,
    /// The clock.
    pub now: Timestamp,
}

impl Scope for GroupScope<'_> {
    fn value(&self, path: &[FieldName]) -> Option<Value> {
        let [name] = path else { return None };
        self.earlier
            .get(name)
            .or_else(|| self.by.get(name))
            .filter(|v| !v.is_null())
            .cloned()
    }

    fn now(&self) -> Timestamp {
        self.now
    }

    fn setting(&self, key: &str) -> Option<Value> {
        self.settings.and_then(|s| s.get(key)).cloned()
    }

    fn aggregate(&self, func: Func, args: &[Expr]) -> Option<Result<Val>> {
        Some(fold(func, args.first(), self.rows, self.settings, self.now))
    }
}

fn row_values(
    arg: &Expr,
    rows: &[&BTreeMap<FieldName, Value>],
    settings: Option<&BTreeMap<String, Value>>,
    now: Timestamp,
) -> Result<Vec<Val>> {
    let mut out = Vec::new();
    for row in rows {
        let v = eval(
            arg,
            &RecordScope {
                fields: row,
                settings,
                now,
            },
        )?;
        if v != Val::Null {
            out.push(v);
        }
    }
    Ok(out)
}

fn exact_sum(values: &[Val]) -> Result<Decimal> {
    values.iter().try_fold(Decimal::ZERO, |acc, v| {
        let n = match v {
            Val::Num(d) => *d,
            Val::Text(s) => s.parse::<Decimal>().map_err(|_| {
                EvalError(format!("sum() met the text '{s}', which is not a number"))
            })?,
            other => {
                return Err(EvalError(format!(
                    "sum() met {other:?}, which is not a number"
                )));
            }
        };
        acc.checked_add(n)
            .ok_or_else(|| EvalError("the sum is too large for exact arithmetic".into()))
    })
}

fn fold(
    func: Func,
    arg: Option<&Expr>,
    rows: &[&BTreeMap<FieldName, Value>],
    settings: Option<&BTreeMap<String, Value>>,
    now: Timestamp,
) -> Result<Val> {
    let Some(arg) = arg else {
        return match func {
            Func::Count => Ok(Val::Num(Decimal::from(rows.len()))),
            _ => Err(EvalError(format!(
                "{}() needs a field",
                super::func_name(func)
            ))),
        };
    };
    let values = row_values(arg, rows, settings, now)?;
    match func {
        Func::Count => Ok(Val::Num(Decimal::from(values.len()))),
        Func::Sum => exact_sum(&values).map(Val::Num),
        Func::Avg if values.is_empty() => Ok(Val::Null),
        Func::Avg => {
            let sum = exact_sum(&values)?;
            sum.checked_div(Decimal::from(values.len()))
                .map(Val::Num)
                .ok_or_else(|| EvalError("the average is too large for exact arithmetic".into()))
        }
        Func::Min | Func::Max => {
            let want = if func == Func::Min {
                CmpOp::Lt
            } else {
                CmpOp::Gt
            };
            let mut best: Option<Val> = None;
            for v in values {
                best = match best {
                    Some(b) if !compare(want, &v, &b)? => Some(b),
                    _ => Some(v),
                };
            }
            Ok(best.unwrap_or(Val::Null))
        }
        _ => Err(EvalError(format!(
            "{}() is not an aggregate",
            super::func_name(func)
        ))),
    }
}

/// A computed value in its JSON form, for a group's answer: numbers as
/// exact decimal strings, instants in the canonical form.
pub fn to_value(v: &Val) -> Value {
    match v {
        Val::Null => Value::Null,
        Val::Bool(b) => Value::Bool(*b),
        // Exact, and at the scale the inputs carried: a sum of amounts in
        // cents reads in cents.
        Val::Num(d) => Value::String(d.to_string()),
        Val::Text(s) => Value::String(s.clone()),
        Val::Time(t) => Value::String(t.to_string()),
        Val::Duration(d) => Value::from(*d),
        Val::Json(j) => j.clone(),
    }
}

/// One record's fields.
pub type Row = BTreeMap<FieldName, Value>;

/// One group: its grouping values and its records.
pub type Grouped<'a> = (Row, Vec<&'a Row>);

/// Split records into groups by the values of `by`, in canonical key order.
pub fn groups<'a>(rows: &[&'a Row], by: &[FieldName]) -> Vec<Grouped<'a>> {
    let mut map: BTreeMap<Vec<u8>, Grouped<'a>> = BTreeMap::new();
    for row in rows {
        let key: BTreeMap<FieldName, Value> = by
            .iter()
            .map(|f| (f.clone(), row.get(f).cloned().unwrap_or(Value::Null)))
            .collect();
        let bytes = crate::canonical::of(&key);
        map.entry(bytes)
            .or_insert_with(|| (key, Vec::new()))
            .1
            .push(row);
    }
    map.into_values().collect()
}

/// An aggregate's group keys, resolved: each key's name and, for a named
/// expression, the expression, parsed and checked as a `where` is. Names
/// follow the rule for field names and are distinct.
pub fn group_keys(keys: &[GroupKey]) -> Result<Vec<(FieldName, Option<Expr>)>> {
    let mut out: Vec<(FieldName, Option<Expr>)> = Vec::with_capacity(keys.len());
    for key in keys {
        let name = key.name();
        if let Some(fault) = crate::names::fault("group", name.as_str()) {
            return Err(EvalError(fault));
        }
        if out.iter().any(|(n, _)| n == name) {
            return Err(EvalError(format!(
                "two group keys are named `{name}`; each needs a name of its own"
            )));
        }
        let expr = match key {
            GroupKey::Field(_) => None,
            GroupKey::Named { expr, .. } => {
                let e = match expr {
                    Condition::Expr(e) => e.clone(),
                    Condition::Text(t) => super::parse(t).map_err(|e| {
                        EvalError(format!("group `{name}`: `{t}` does not parse: {e}"))
                    })?,
                };
                super::check(&e, super::Place::Record)
                    .map_err(|e| EvalError(format!("group `{name}`: {e}")))?;
                Some(e)
            }
        };
        out.push((name.clone(), expr));
    }
    Ok(out)
}

/// Split records into groups by resolved keys, in canonical key order: a
/// field's value, or an expression's per record. A value the expression
/// cannot compute for a record is null, the missing group.
fn keyed_groups<'a>(
    rows: &[&'a Row],
    keys: &[(FieldName, Option<Expr>)],
    now: Timestamp,
) -> Vec<Grouped<'a>> {
    let mut map: BTreeMap<Vec<u8>, Grouped<'a>> = BTreeMap::new();
    for row in rows {
        let key: BTreeMap<FieldName, Value> = keys
            .iter()
            .map(|(name, expr)| {
                let v = match expr {
                    None => row.get(name).cloned().unwrap_or(Value::Null),
                    Some(e) => {
                        let scope = RecordScope {
                            fields: row,
                            settings: None,
                            now,
                        };
                        eval(e, &scope).map_or(Value::Null, |v| to_value(&v))
                    }
                };
                (name.clone(), v)
            })
            .collect();
        let bytes = crate::canonical::of(&key);
        map.entry(bytes)
            .or_insert_with(|| (key, Vec::new()))
            .1
            .push(row);
    }
    map.into_values().collect()
}

/// `find`'s aggregate over a set of records. No records and no grouping
/// give one group, so `count` answers 0 rather than nothing.
pub fn aggregate(
    rows: &[&BTreeMap<FieldName, Value>],
    agg: &Aggregate,
    now: Timestamp,
) -> Result<Vec<Group>> {
    let keys = group_keys(&agg.group_by)?;
    let mut grouped = keyed_groups(rows, &keys, now);
    if grouped.is_empty() && agg.group_by.is_empty() {
        grouped.push((BTreeMap::new(), Vec::new()));
    }
    grouped
        .into_iter()
        .map(|(by, members)| {
            let mut values = BTreeMap::new();
            for m in &agg.measures {
                let func = match m.func {
                    AggFunc::Count => Func::Count,
                    AggFunc::Sum => Func::Sum,
                    AggFunc::Min => Func::Min,
                    AggFunc::Max => Func::Max,
                    AggFunc::Avg => Func::Avg,
                };
                let arg = m.field.clone().map(|f| Expr::Path { path: vec![f] });
                // A measure of `find` names a field; no setting is read.
                let v = fold(func, arg.as_ref(), &members, None, now)?;
                values.insert(m.name.clone(), to_value(&v));
            }
            Ok(Group { by, values })
        })
        .collect()
}

/// One group's measures, each computed in declaration order and visible to
/// the measures after it.
pub fn measures(
    members: &[&BTreeMap<FieldName, Value>],
    by: &BTreeMap<FieldName, Value>,
    measures: &[(FieldName, Expr)],
    settings: Option<&BTreeMap<String, Value>>,
    now: Timestamp,
) -> Result<BTreeMap<FieldName, Value>> {
    let mut earlier = BTreeMap::new();
    for (name, expr) in measures {
        let scope = GroupScope {
            rows: members,
            by,
            earlier: &earlier,
            settings,
            now,
        };
        let v = eval(expr, &scope)?;
        earlier.insert(name.clone(), to_value(&v));
    }
    Ok(earlier)
}

#[cfg(test)]
mod tests;
