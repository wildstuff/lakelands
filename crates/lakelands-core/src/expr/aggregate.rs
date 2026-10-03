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
use lakelands_api::read::{AggFunc, AggMeasure, Aggregate, Group, GroupKey};
use lakelands_api::{FieldName, Timestamp, Value};
use rust_decimal::Decimal;

use super::eval::{EvalError, RecordScope, Result, Scope, Val, compare, err, eval};

/// The scope of one group: its folds, its grouping values, and the
/// measures computed before the one being evaluated.
pub struct GroupScope<'a> {
    /// The group's aggregate calls, folded over its records.
    pub folds: &'a Folds,
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
        Some(self.folds.value(func, args.first()))
    }
}

/// One aggregate call over a group, folded a record at a time: the count,
/// the exact sum and the best value, kept instead of the records. It
/// answers what folding the records at once would, errors included: an
/// argument that fails on any record comes before a value that cannot be
/// added or compared, and either before the result.
#[derive(Clone, Debug, Default)]
struct Fold {
    count: usize,
    sum: Decimal,
    best: Option<Val>,
    failed: Option<EvalError>,
    wrong: Option<EvalError>,
}

impl Fold {
    fn add(&mut self, func: Func, arg: Option<&Expr>, scope: &RecordScope<'_>) {
        let Some(arg) = arg else {
            self.count += 1;
            return;
        };
        if self.failed.is_some() {
            return;
        }
        let v = match eval(arg, scope) {
            Ok(Val::Null) => return,
            Ok(v) => v,
            Err(e) => {
                self.failed = Some(e);
                return;
            }
        };
        self.count += 1;
        if self.wrong.is_some() {
            return;
        }
        let folded = match func {
            Func::Sum | Func::Avg => exact_add(self.sum, &v).map(|sum| self.sum = sum),
            Func::Min | Func::Max => {
                let want = if func == Func::Min {
                    CmpOp::Lt
                } else {
                    CmpOp::Gt
                };
                match self.best.as_ref().map(|b| compare(want, &v, b)) {
                    Some(Ok(false)) => Ok(()),
                    Some(Err(e)) => Err(e),
                    _ => {
                        self.best = Some(v);
                        Ok(())
                    }
                }
            }
            _ => Ok(()),
        };
        self.wrong = folded.err();
    }

    fn value(&self, func: Func, arg: Option<&Expr>) -> Result<Val> {
        if arg.is_none() && func != Func::Count {
            return err(format!("{}() needs a field", super::func_name(func)));
        }
        if let Some(e) = self.failed.as_ref().or(self.wrong.as_ref()) {
            return Err(e.clone());
        }
        let count = Decimal::from(self.count);
        match func {
            Func::Count => Ok(Val::Num(count)),
            Func::Sum => Ok(Val::Num(self.sum)),
            Func::Avg if self.count == 0 => Ok(Val::Null),
            Func::Avg => (self.sum.checked_div(count).map(Val::Num))
                .ok_or_else(|| EvalError("the average is too large for exact arithmetic".into())),
            Func::Min | Func::Max => Ok(self.best.clone().unwrap_or(Val::Null)),
            _ => err(format!("{}() is not an aggregate", super::func_name(func))),
        }
    }
}

/// `sum` plus one value, exactly: a value that is not a number is an
/// error, never skipped.
fn exact_add(sum: Decimal, v: &Val) -> Result<Decimal> {
    let n = match v {
        Val::Num(d) => *d,
        Val::Text(s) => s
            .parse::<Decimal>()
            .map_err(|_| EvalError(format!("sum() met the text '{s}', which is not a number")))?,
        other => return err(format!("sum() met {other:?}, which is not a number")),
    };
    sum.checked_add(n)
        .ok_or_else(|| EvalError("the sum is too large for exact arithmetic".into()))
}

/// The aggregate calls a group's measures make, each once, folded over
/// its records as they come: what the group keeps instead of them. A call
/// no record was folded into answers as over no records.
#[derive(Clone, Debug, Default)]
pub struct Folds(Vec<(Func, Option<Expr>, Fold)>);

impl Folds {
    fn of(measures: &[(FieldName, Expr)]) -> Folds {
        fn calls(e: &Expr, out: &mut Folds) {
            match e {
                Expr::Call { func, args } if func.is_aggregate() => {
                    let arg = args.first().cloned();
                    if !out.0.iter().any(|(f, a, _)| f == func && *a == arg) {
                        out.0.push((*func, arg, Fold::default()));
                    }
                }
                e => super::children(e).for_each(|c| calls(c, out)),
            }
        }
        let mut out = Folds::default();
        measures.iter().for_each(|(_, e)| calls(e, &mut out));
        out
    }

    fn add(&mut self, row: &Row, settings: Option<&BTreeMap<String, Value>>, now: Timestamp) {
        let scope = RecordScope {
            fields: row,
            settings,
            now,
        };
        for (func, arg, fold) in &mut self.0 {
            fold.add(*func, arg.as_ref(), &scope);
        }
    }

    fn value(&self, func: Func, arg: Option<&Expr>) -> Result<Val> {
        let none = Fold::default();
        let held = (self.0.iter()).find(|(f, a, _)| *f == func && a.as_ref() == arg);
        held.map_or(&none, |(_, _, fold)| fold).value(func, arg)
    }
}

/// Records split into groups as they come, each group keeping its
/// grouping values and its folds, never its records.
#[derive(Debug)]
pub struct Grouping {
    keys: Vec<(FieldName, Option<Expr>)>,
    folds: Folds,
    now: Timestamp,
    groups: BTreeMap<Vec<u8>, (Row, Folds)>,
}

impl Grouping {
    /// Groups by resolved `keys`, each folding what `measures` aggregate.
    pub fn new(
        keys: Vec<(FieldName, Option<Expr>)>,
        measures: &[(FieldName, Expr)],
        now: Timestamp,
    ) -> Grouping {
        Grouping {
            keys,
            folds: Folds::of(measures),
            now,
            groups: BTreeMap::new(),
        }
    }

    /// Fold in one record, into its group: a key is a field's value, or
    /// an expression's. A value the expression cannot compute for the
    /// record is null, the missing group.
    pub fn add(&mut self, row: &Row, settings: Option<&BTreeMap<String, Value>>) {
        let value = |(name, expr): &(FieldName, Option<Expr>)| {
            let v = match expr {
                None => row.get(name).cloned().unwrap_or(Value::Null),
                Some(e) => {
                    let scope = RecordScope {
                        fields: row,
                        settings: None,
                        now: self.now,
                    };
                    eval(e, &scope).map_or(Value::Null, |v| to_value(&v))
                }
            };
            (name.clone(), v)
        };
        let key: Row = self.keys.iter().map(value).collect();
        let group = (self.groups.entry(crate::canonical::of(&key)))
            .or_insert_with(|| (key, self.folds.clone()));
        group.1.add(row, settings, self.now);
    }

    /// The groups, in canonical key order.
    pub fn groups(self) -> Vec<(Row, Folds)> {
        self.groups.into_values().collect()
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

/// `find`'s aggregate over a set of records. No records and no grouping
/// give one group, so `count` answers 0 rather than nothing.
pub fn aggregate(rows: &[&Row], agg: &Aggregate, now: Timestamp) -> Result<Vec<Group>> {
    let mut grouping = grouping(agg, now)?;
    rows.iter().for_each(|r| grouping.add(r, None));
    answer(grouping, agg)
}

/// The grouping `find`'s aggregate folds its records into as they come.
pub fn grouping(agg: &Aggregate, now: Timestamp) -> Result<Grouping> {
    let calls: Vec<_> = agg.measures.iter().map(call).collect();
    Ok(Grouping::new(group_keys(&agg.group_by)?, &calls, now))
}

/// [`aggregate`]'s answer over the records `grouping` folded.
pub fn answer(grouping: Grouping, agg: &Aggregate) -> Result<Vec<Group>> {
    let (calls, now): (Vec<_>, _) = (agg.measures.iter().map(call).collect(), grouping.now);
    let mut grouped = grouping.groups();
    if grouped.is_empty() && agg.group_by.is_empty() {
        grouped.push(Default::default());
    }
    let group = |(by, folds): (Row, Folds)| {
        // A measure of `find` names a field; no setting is read.
        let values = measures(&folds, &by, &calls, Row::new(), None, now)?;
        Ok(Group { by, values })
    };
    grouped.into_iter().map(group).collect()
}

/// A measure of `find` as the aggregate call it makes, under its name.
fn call(m: &AggMeasure) -> (FieldName, Expr) {
    let func = match m.func {
        AggFunc::Count => Func::Count,
        AggFunc::Sum => Func::Sum,
        AggFunc::Min => Func::Min,
        AggFunc::Max => Func::Max,
        AggFunc::Avg => Func::Avg,
    };
    let args = m.field.iter().map(|f| Expr::Path {
        path: vec![f.clone()],
    });
    let args = args.collect();
    (m.name.clone(), Expr::Call { func, args })
}

/// One group's measures over its folds, each computed in declaration
/// order and visible to the measures after it, as are the values `given`
/// (a merged source's measures), which the answer holds too.
pub fn measures(
    folds: &Folds,
    by: &Row,
    measures: &[(FieldName, Expr)],
    given: Row,
    settings: Option<&BTreeMap<String, Value>>,
    now: Timestamp,
) -> Result<Row> {
    let mut earlier = given;
    for (name, expr) in measures {
        let scope = GroupScope {
            folds,
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
