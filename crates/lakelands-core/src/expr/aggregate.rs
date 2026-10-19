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
//!   values is 0. A sum reads at the largest scale among its values, a
//!   zero's included, whatever order they are added in ([`Scales`]).
//! - `min(x)` and `max(x)` order values as a comparison does
//!   (`crate::expr::eval`); nulls are skipped, no values give null.
//! - `avg(x)` is `sum(x) / count(x)`, exactly, and null when nothing is
//!   counted.
//!
//! Groups are formed by the values of the grouping fields (a null is a
//! value like any other here) and returned in the order of their key's
//! canonical bytes, so every engine answers in the same order.

use std::collections::BTreeMap;

use lakelands_api::error::{Case, Naming, Place, Said};
use lakelands_api::expr::{CmpOp, Condition, Expr, Func};
use lakelands_api::read::{AggFunc, AggMeasure, Aggregate, Group, GroupKey};
use lakelands_api::{FieldName, Timestamp, Value};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::eval::{RecordScope, Result, Scope, Val, compare, err, eval, operand};

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
    scales: Scales,
    best: Option<Val>,
    failed: Option<Case>,
    wrong: Option<Case>,
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
            Func::Sum | Func::Avg => self.exact_add(&v),
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
            let name = super::func_name(func).into();
            return err(Case::AggregateNeedsField { name });
        }
        if let Some(e) = self.failed.as_ref().or(self.wrong.as_ref()) {
            return Err(e.clone());
        }
        let count = Decimal::from(self.count);
        // Where the sum is exact it is read off its values; where it is
        // not, it is what adding them in the order they came gave.
        let sum = self.scales.exact().unwrap_or(self.sum);
        match func {
            Func::Count => Ok(Val::Num(count)),
            Func::Sum => Ok(Val::Num(sum)),
            Func::Avg if self.count == 0 => Ok(Val::Null),
            Func::Avg => (sum.checked_div(count).map(Val::Num)).ok_or(Case::Overflow),
            Func::Min | Func::Max => Ok(self.best.clone().unwrap_or(Val::Null)),
            _ => err(Case::NotAggregate {
                name: super::func_name(func).into(),
            }),
        }
    }

    /// The sum plus one value, exactly: a value that is not a number is an
    /// error, never skipped.
    fn exact_add(&mut self, v: &Val) -> Result<()> {
        let not = || Case::SumNotNumber { value: operand(v) };
        let n = match v {
            Val::Num(d) => *d,
            Val::Text(s) => s.parse::<Decimal>().map_err(|_| not())?,
            _ => return Err(not()),
        };
        self.scales.fold(n, true);
        self.sum = self.sum.checked_add(n).ok_or(Case::Overflow)?;
        Ok(())
    }
}

/// The numbers of a sum by the scale they carry: for each scale how many,
/// their sum and the sum of their magnitudes, as integers. Where the
/// magnitudes, written at the largest scale among the values, stay below
/// 2^96, no order of additions rounds or fails, and the sum is exact at
/// that scale: a function of its values, whatever order they came in, and
/// one a number can be taken out of again. Adding them one to the next is
/// not: a zero hands the other number back as it stands, so
/// `1.10 - 1.10 + 5` read `5` or `5.00` by the order of the keys.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Scales(BTreeMap<u32, (u64, Wide, Wide)>);

/// An integer wider than a JSON number, written as its decimal text.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Wide(i128);

impl Serialize for Wide {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Wide {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Wide, D::Error> {
        let text = String::deserialize(d)?;
        text.parse().map(Wide).map_err(serde::de::Error::custom)
    }
}

impl Scales {
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Put a number in, or take one out. A scale no number carries has no
    /// entry, so the numbers held say the same whatever was put and taken
    /// on the way. Magnitudes past an integer's width, and a number taken
    /// that was not put, leave the scale's magnitude at the width's end:
    /// the sum is then not exact, and stays so.
    fn fold(&mut self, n: Decimal, put: bool) {
        let (scale, m) = (n.scale(), n.mantissa());
        let (count, sum, size) = self.0.entry(scale).or_default();
        if put {
            *count = count.saturating_add(1);
            sum.0 = sum.0.saturating_add(m);
            size.0 = size.0.saturating_add(m.abs());
        } else if *count == 0 || size.0 == i128::MAX || size.0 < m.abs() {
            size.0 = i128::MAX;
        } else {
            (*count, sum.0, size.0) = (*count - 1, sum.0 - m, size.0 - m.abs());
            match (*count, size.0) {
                (0, 0) => drop(self.0.remove(&scale)),
                (0, _) => size.0 = i128::MAX,
                _ => {}
            }
        }
    }

    /// The sum at the largest scale among the values, a zero's included
    /// and the zero positive, where it is exact; no values sum to `0`.
    fn exact(&self) -> Option<Decimal> {
        let top = self.0.keys().next_back().copied().unwrap_or(0);
        let (mut sum, mut size) = (0i128, 0i128);
        for (scale, (_, s, m)) in &self.0 {
            let up = 10i128.checked_pow(top - scale)?;
            sum = sum.checked_add(s.0.checked_mul(up)?)?;
            size = size.checked_add(m.0.checked_mul(up)?)?;
        }
        // A scale read back from a written state may be one no decimal has.
        let read = Decimal::try_from_i128_with_scale(sum, top).ok();
        read.filter(|_| size < 1 << 96)
    }
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

    fn add(&mut self, (row, meta): Member<'_>, settings: Settings<'_>, now: Timestamp) {
        let scope = RecordScope {
            fields: row,
            meta,
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
    pub fn add(&mut self, (row, meta): Member<'_>, settings: Settings<'_>) {
        let value = |(name, expr): &(FieldName, Option<Expr>)| {
            let v = match expr {
                None => row.get(name).cloned().unwrap_or(Value::Null),
                Some(e) => {
                    let scope = RecordScope {
                        fields: row,
                        meta,
                        settings,
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
        group.1.add((row, meta), settings, self.now);
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

/// What an aggregate folds in: a record's fields and, where a record
/// stands, its provenance; a row of a pattern or of a view has none.
pub type Member<'a> = (
    &'a Row,
    Option<&'a BTreeMap<FieldName, lakelands_api::FieldMeta>>,
);

/// The lake's settings by key, which an argument may read.
pub type Settings<'a> = Option<&'a BTreeMap<String, Value>>;

/// An aggregate's answer, or the case of why it has none, said where it
/// is.
pub type Answer<T> = std::result::Result<T, Said>;

/// An aggregate's group keys, resolved: each key's name and, for a named
/// expression, the expression, parsed and checked as a `where` is. A named
/// key's name follows the rule for field names, a field key names its
/// field, and the names are distinct.
pub fn group_keys(keys: &[GroupKey]) -> Answer<Vec<(FieldName, Option<Expr>)>> {
    let mut out: Vec<(FieldName, Option<Expr>)> = Vec::with_capacity(keys.len());
    for key in keys {
        let name = key.name();
        if let GroupKey::Named { .. } = key
            && let Some(fault) = crate::names::fault(Naming::Group, name.as_str())
        {
            return Err(fault.into());
        }
        if out.iter().any(|(n, _)| n == name) {
            return Err(Case::GroupKeyTwice { name: name.clone() }.into());
        }
        let within = |case: Case| Said::from(case).within(Place::Group { name: name.clone() });
        let expr = match key {
            GroupKey::Field(_) => None,
            GroupKey::Named { expr, .. } => {
                let e = match expr {
                    Condition::Expr(e) => e.clone(),
                    Condition::Text(t) => super::parse(t).map_err(|e| within(e.case))?,
                };
                super::check(&e, super::Place::Record).map_err(within)?;
                Some(e)
            }
        };
        out.push((name.clone(), expr));
    }
    Ok(out)
}

/// A path a pattern's rows hold, named whole, as an order, a projected
/// field, a field key or a measure's field names one: the longest name
/// `bound` that starts it, then one field of its record or `key`. A path
/// no bound name starts, or one that goes on past the field, is refused.
/// Answers the name and the field.
pub fn row_path(name: &str, bound: &dyn Fn(&str) -> bool) -> Answer<(String, FieldName)> {
    let path: Vec<&str> = name.split('.').collect();
    match (1..path.len()).rev().find(|&k| bound(&path[..k].join("."))) {
        Some(k) if k + 1 == path.len() => Ok((path[..k].join("."), FieldName::from(path[k]))),
        _ => Err(Case::PatternNoPath { path: name.into() }.into()),
    }
}

/// A pattern's aggregate read over its rows, which hold each value under
/// its path, `<name>.<field>` or `<name>.key`, for each name `bound`
/// (018, extended). A measure's field and a field key name a path whole.
/// Each path of a named key reads the longest name bound, a field of its
/// record, then members, and is rewritten to the row's one key. A path no
/// bound name starts is refused. Answers the aggregate so read and each
/// name and field it reads.
pub fn over_rows(
    agg: &Aggregate,
    bound: &dyn Fn(&str) -> bool,
) -> Answer<(Aggregate, Vec<(String, FieldName)>)> {
    let read = std::cell::RefCell::new(Vec::new());
    let joined = |path: &[FieldName]| {
        path.iter()
            .map(FieldName::as_str)
            .collect::<Vec<_>>()
            .join(".")
    };
    let unbound = |path: String| Said::from(Case::PatternNoPath { path });
    // Where a path's field stands: after the longest name `bound`.
    let at = |path: &[FieldName]| {
        let k = (1..path.len()).rev().find(|&k| bound(&joined(&path[..k])));
        let k = k.ok_or_else(|| unbound(joined(path)))?;
        read.borrow_mut()
            .push((joined(&path[..k]), path[k].clone()));
        Ok(k)
    };
    let whole = |name: &FieldName| {
        read.borrow_mut().push(row_path(name.as_str(), bound)?);
        Ok::<_, Said>(())
    };
    agg.measures
        .iter()
        .filter_map(|m| m.field.as_ref())
        .try_for_each(whole)?;
    let mut out = agg.clone();
    for (key, (_, e)) in out.group_by.iter_mut().zip(group_keys(&agg.group_by)?) {
        let (GroupKey::Named { expr, .. }, Some(e)) = (&mut *key, e) else {
            if let GroupKey::Field(f) = key {
                whole(f)?;
            }
            continue;
        };
        let failed = std::cell::RefCell::new(None);
        let e = lakelands_api::expr::rewrite(&e, &|x| {
            let Expr::Path { path } = x else {
                return None;
            };
            let k = at(path).map_err(|e| failed.replace(Some(e))).ok()?;
            let head = std::iter::once(FieldName::from(joined(&path[..=k])));
            let path = head.chain(path[k + 1..].iter().cloned()).collect();
            Some(Expr::Path { path })
        });
        if let Some(e) = failed.into_inner() {
            return Err(e);
        }
        *expr = Condition::Expr(e);
    }
    Ok((out, read.into_inner()))
}

/// A pattern's aggregate over its rows, its keys reading the lake's
/// `settings`. No rows and no grouping give one group, so `count`
/// answers 0 rather than nothing.
pub fn aggregate(
    rows: &[&Row],
    agg: &Aggregate,
    settings: &BTreeMap<String, Value>,
    now: Timestamp,
) -> Answer<Vec<Group>> {
    let mut grouping = grouping(agg, now)?;
    rows.iter()
        .for_each(|r| grouping.add((r, None), Some(settings)));
    answer(grouping, agg)
}

/// The grouping `find`'s aggregate folds its records into as they come.
pub fn grouping(agg: &Aggregate, now: Timestamp) -> Answer<Grouping> {
    let calls: Vec<_> = agg.measures.iter().map(call).collect();
    Ok(Grouping::new(group_keys(&agg.group_by)?, &calls, now))
}

/// [`aggregate`]'s answer over the records `grouping` folded.
pub fn answer(grouping: Grouping, agg: &Aggregate) -> Answer<Vec<Group>> {
    let (calls, now): (Vec<_>, _) = (agg.measures.iter().map(call).collect(), grouping.now);
    let mut grouped = grouping.groups();
    if grouped.is_empty() && agg.group_by.is_empty() {
        grouped.push(Default::default());
    }
    let group = |(by, folds): (Row, Folds)| -> Answer<Group> {
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
    named_measures(folds, by, measures, given, settings, now).map_err(|(_, e)| e)
}

/// [`measures`], its error naming the measure that failed.
pub fn named_measures(
    folds: &Folds,
    by: &Row,
    measures: &[(FieldName, Expr)],
    given: Row,
    settings: Option<&BTreeMap<String, Value>>,
    now: Timestamp,
) -> std::result::Result<Row, (FieldName, Case)> {
    let mut earlier = given;
    for (name, expr) in measures {
        let scope = GroupScope {
            folds,
            by,
            earlier: &earlier,
            settings,
            now,
        };
        let v = eval(expr, &scope).map_err(|e| (name.clone(), e))?;
        earlier.insert(name.clone(), to_value(&v));
    }
    Ok(earlier)
}

mod kept;
pub use kept::{Entry, Keeping, KeptFolds, Placed};

#[cfg(test)]
mod tests;
