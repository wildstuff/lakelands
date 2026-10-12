//! What an expression means: the one evaluator every filter, guard,
//! derived field, constraint and view measure goes through.
//!
//! # Values
//!
//! An expression computes a [`Val`]: null, a boolean, an exact decimal
//! number, text, an instant, a duration, or a JSON value it cannot look
//! into. Record values enter by their JSON form (`docs/log-format.md` §5):
//! a JSON number and a decimal string are both numbers when arithmetic or
//! a comparison needs one, and text that is a timestamp is an instant when
//! compared with an instant.
//!
//! # The rules
//!
//! - **Absent never matches.** A field with no value is null. A comparison
//!   with a null operand is false — `!=` included — so a filter never
//!   selects a record for what it does not hold; `is null` is the way to
//!   ask. Arithmetic and functions on null give null; `coalesce` picks the
//!   first operand that is not null.
//! - **Numbers compare as numbers.** `1 = 1.0`, and `'10' > 9`: whenever
//!   both sides read as numbers, they compare exactly as decimals.
//!   Otherwise instants compare as instants, and everything else compares
//!   as text. A boolean equals only a boolean and has no order.
//! - **Arithmetic is exact and checked.** Decimal arithmetic never passes
//!   through a binary float; overflow and division by zero are errors,
//!   never a wrapped or infinite value. An instant plus or minus a duration
//!   is an instant; the difference of two instants is a duration. A
//!   duration divided by a duration is a number; a duration times or
//!   divided by a number is a duration, to the microsecond.
//! - **`and`, `or` and `if` are lazy.** Only the operands that decide the
//!   answer are evaluated, so `if(b != 0, a / b, 0)` never divides by zero.
//!   An operand that is not a boolean counts as false.
//! - **Text functions count characters,** not bytes. `replace` with an
//!   empty pattern changes nothing. `matches` takes an I-Regexp (RFC 9485)
//!   and matches the whole text.
//! - **`round` rounds half away from zero,** the rule money is kept by.
//!   `floor` is the greatest whole number not above its operand.
//! - **Time is the caller's.** `now()` and `today()` read the scope's
//!   clock: the fold binds it to the `known_at` of the event it folds, so a
//!   replay computes the same value; a read binds it to the time of the
//!   read.
//!
//! An expression that cannot be evaluated — a type misuse, an overflow, a
//! function used where it has no meaning — is a [`Case`]. What an
//! error means is the caller's posture, stated where each caller lives: a
//! record its filter fails for does not match, a group key that fails is
//! null, a trigger guard does not fire, a derived field is cleared. One
//! record's value never fails a read of the others. What can never be
//! evaluated, whatever the record holds, is refused before any of them
//! runs (`lakelands_api::expr::kind`).

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::str::FromStr;

use lakelands_api::error::{Case, Operand};
use lakelands_api::expr::{ArithOp, CmpOp, Expr, Func, Literal};
use lakelands_api::{FieldMeta, FieldName, Timestamp};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::Value;

/// One value an expression computes.
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    /// Absent, or cleared.
    Null,
    /// A boolean.
    Bool(bool),
    /// An exact decimal number.
    Num(Decimal),
    /// Text.
    Text(String),
    /// A UTC instant.
    Time(Timestamp),
    /// A span of time, in microseconds.
    Duration(i64),
    /// A JSON array or object, compared whole and never looked into.
    Json(Value),
}

/// The result of an evaluation: a value, or the case of why it has none.
pub type Result<T> = std::result::Result<T, Case>;

pub(crate) fn err<T>(case: Case) -> Result<T> {
    Err(case)
}

/// What an expression can see: the record in scope and the context of the
/// evaluation.
pub trait Scope {
    /// The value at a path: a field of the record in scope, or
    /// `<name>.<field>` of a record a pattern's row binds. `None` when
    /// absent.
    fn value(&self, path: &[FieldName]) -> Option<Value>;

    /// In a guard: the field's value before the write. `None` outside a
    /// write, for a record the write creates, or for an absent field.
    fn previous(&self, _field: &FieldName) -> Option<Value> {
        None
    }

    /// Whether the evaluation is a guard over a write, where `changed` and
    /// `previous` have meaning.
    fn in_write(&self) -> bool {
        false
    }

    /// In a guard: whether the write retires the record.
    fn retiring(&self) -> bool {
        false
    }

    /// The provenance of a field, when the scope carries it.
    fn meta(&self, _field: &FieldName) -> Option<FieldMeta> {
        None
    }

    /// The value of the lake's `sys.setting` record with this key.
    fn setting(&self, _key: &str) -> Option<Value> {
        None
    }

    /// The value a read binds to the view parameter of this name. `None`
    /// outside a view's `where` and `measures`.
    fn param(&self, _name: &str) -> Option<Value> {
        None
    }

    /// The clock `now()` and `today()` read.
    fn now(&self) -> Timestamp;

    /// In a verb: whether the path that begins at `relation` reaches a
    /// record. `None` outside a verb.
    fn exists(&self, _relation: &FieldName) -> Option<bool> {
        None
    }

    /// An aggregate function over the set of records in scope. `None` where
    /// no set is in scope: over one record an aggregate has no meaning.
    fn aggregate(&self, _func: Func, _args: &[Expr]) -> Option<Result<Val>> {
        None
    }
}

/// The plainest scope: one record's fields and their provenance, the
/// lake's settings as of the position read, and a clock.
#[derive(Clone, Copy, Debug)]
pub struct RecordScope<'a> {
    /// The record's fields.
    pub fields: &'a BTreeMap<FieldName, Value>,
    /// Per-field provenance; none where no record stands.
    pub meta: Option<&'a BTreeMap<FieldName, FieldMeta>>,
    /// The lake's settings by key, as of the position read.
    pub settings: Option<&'a BTreeMap<String, Value>>,
    /// The clock.
    pub now: Timestamp,
}

impl Scope for RecordScope<'_> {
    fn value(&self, path: &[FieldName]) -> Option<Value> {
        lookup(self.fields, path)
    }

    fn meta(&self, field: &FieldName) -> Option<FieldMeta> {
        self.meta.and_then(|m| m.get(field)).cloned()
    }

    fn setting(&self, key: &str) -> Option<Value> {
        self.settings.and_then(|s| s.get(key)).cloned()
    }

    fn now(&self) -> Timestamp {
        self.now
    }
}

/// A guard over a write: the record after the write, the record before it
/// (`None` for a create), its per-field provenance, the lake's settings and
/// the event's time.
pub struct WriteScope<'a> {
    /// The record's fields after the write.
    pub after: &'a BTreeMap<FieldName, Value>,
    /// The record's fields before the write; `None` for a create.
    pub before: Option<&'a BTreeMap<FieldName, Value>>,
    /// Whether the write retires the record: live before, retired after.
    /// A retirement keeps the fields, so only this tells it from an
    /// update.
    pub retiring: bool,
    /// Per-field provenance after the write, if kept.
    pub meta: Option<&'a BTreeMap<FieldName, FieldMeta>>,
    /// The lake's settings, by key.
    pub settings: Option<&'a BTreeMap<String, Value>>,
    /// The `known_at` of the event being folded.
    pub now: Timestamp,
}

impl Scope for WriteScope<'_> {
    fn value(&self, path: &[FieldName]) -> Option<Value> {
        lookup(self.after, path)
    }

    fn previous(&self, field: &FieldName) -> Option<Value> {
        self.before.and_then(|b| b.get(field)).cloned()
    }

    fn in_write(&self) -> bool {
        true
    }

    fn retiring(&self) -> bool {
        self.retiring
    }

    fn meta(&self, field: &FieldName) -> Option<FieldMeta> {
        self.meta.and_then(|m| m.get(field)).cloned()
    }

    fn setting(&self, key: &str) -> Option<Value> {
        self.settings.and_then(|s| s.get(key)).cloned()
    }

    fn now(&self) -> Timestamp {
        self.now
    }
}

/// A one-segment path reads a field; a longer path reads into a JSON
/// object stored in that field.
pub fn lookup(fields: &BTreeMap<FieldName, Value>, path: &[FieldName]) -> Option<Value> {
    let (first, rest) = path.split_first()?;
    let mut v = fields.get(first)?;
    for seg in rest {
        v = v.get(seg.as_str())?;
    }
    (!v.is_null()).then(|| v.clone())
}

/// A record value as a [`Val`].
pub fn from_json(v: &Value) -> Val {
    match v {
        Value::Null => Val::Null,
        Value::Bool(b) => Val::Bool(*b),
        Value::Number(n) => decimal_of_number(n).map_or(Val::Json(v.clone()), Val::Num),
        Value::String(s) => Val::Text(s.clone()),
        Value::Array(_) | Value::Object(_) => Val::Json(v.clone()),
    }
}

fn decimal_of_number(n: &serde_json::Number) -> Option<Decimal> {
    if let Some(i) = n.as_i64() {
        return Some(Decimal::from(i));
    }
    if let Some(u) = n.as_u64() {
        return Some(Decimal::from(u));
    }
    // A float enters by its shortest decimal form, never by its binary
    // expansion: 0.1 is 0.1.
    let shortest = crate::canonical::number(n.as_f64()?);
    Decimal::from_str(&shortest)
        .or_else(|_| Decimal::from_scientific(&shortest))
        .ok()
}

/// Evaluate an expression in a scope.
pub fn eval(e: &Expr, scope: &dyn Scope) -> Result<Val> {
    match e {
        Expr::Lit { value } => Ok(literal(value)?),
        Expr::Path { path } => Ok(scope.value(path).as_ref().map_or(Val::Null, from_json)),
        Expr::Cmp { cmp, lhs, rhs } => {
            let (l, r) = (eval(lhs, scope)?, eval(rhs, scope)?);
            Ok(Val::Bool(compare(*cmp, &l, &r)?))
        }
        Expr::Arith { arith, lhs, rhs } => arith_op(*arith, eval(lhs, scope)?, eval(rhs, scope)?),
        Expr::Neg { arg } => match eval(arg, scope)? {
            Val::Null => Ok(Val::Null),
            Val::Duration(d) => Ok(Val::Duration(d.checked_neg().ok_or_else(overflow)?)),
            v => Ok(Val::Num(-number(&v)?)),
        },
        // The first operand that decides stops the evaluation.
        Expr::And { args } | Expr::Or { args } => {
            let or = matches!(e, Expr::Or { .. });
            for a in args {
                if truthy(&eval(a, scope)?) == or {
                    return Ok(Val::Bool(or));
                }
            }
            Ok(Val::Bool(!or))
        }
        Expr::Not { arg } => Ok(Val::Bool(!truthy(&eval(arg, scope)?))),
        Expr::In { arg, list } => {
            let v = eval(arg, scope)?;
            for item in list {
                if compare(CmpOp::Eq, &v, &eval(item, scope)?)? {
                    return Ok(Val::Bool(true));
                }
            }
            Ok(Val::Bool(false))
        }
        Expr::IsNull { arg } => Ok(Val::Bool(eval(arg, scope)? == Val::Null)),
        Expr::Call { func, args } => call(*func, args, scope),
    }
}

/// Where an expression is evaluated, which decides the functions it may
/// use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// Over one record, outside a write: a filter, a constraint.
    Record,
    /// Over one record in a write: a derived field, where `changed` and
    /// `previous` have meaning.
    Write,
    /// A trigger's guard: a write, where `retired` has meaning as well.
    Guard,
    /// Over a group of records: a view measure, where aggregates have
    /// meaning.
    Group,
    /// Over one source record of a view: its `where`, or an aggregate's
    /// argument in a measure. Like `Record`, and `param` has meaning.
    View,
    /// A verb's `requires` and set values: like `Record`, and `exists`
    /// has meaning.
    Verb,
}

/// The units `date_bucket` knows.
const BUCKET_UNITS: [&str; 5] = ["day", "week", "month", "quarter", "year"];

/// The instant, in UTC, at which the bucket of `unit` holding `t` starts:
/// midnight of the day, Monday 00:00 of the ISO week, or the first of the
/// month, of the quarter's first month, or of January.
fn bucket_start(t: Timestamp, unit: &str) -> Result<Timestamp> {
    const DAY: i64 = 86_400_000_000;
    let days = t.0.div_euclid(DAY);
    let (y, m, _) = t.date();
    let first = |month: u32| Timestamp::from_date(y, month, 1).ok_or_else(overflow);
    match unit {
        "day" => Ok(Timestamp(days * DAY)),
        // 1970-01-01 was a Thursday: Monday is three days before it.
        "week" => Ok(Timestamp((days - (days + 3).rem_euclid(7)) * DAY)),
        "month" => first(m),
        "quarter" => first((m - 1) / 3 * 3 + 1),
        "year" => first(1),
        other => err(Case::BucketUnit { unit: other.into() }),
    }
}

/// Check an expression before evaluating it: every call has a count of
/// arguments its function takes, a function that names a field is given
/// one, `setting` is given a quoted key, numbers are numbers this
/// evaluator can hold exactly, and `changed`, `previous` and the
/// aggregates appear only where they have meaning. The store checks every
/// typed expression it is handed this way and refuses it as invalid; a
/// text expression is checked the same way after parsing.
pub fn check(e: &Expr, place: Place) -> Result<()> {
    match e {
        Expr::Lit { value } => literal(value).map(|_| ()),
        Expr::Path { path } if path.is_empty() => err(Case::PathEmpty),
        Expr::Path { .. } => Ok(()),
        Expr::Cmp { lhs, rhs, .. } | Expr::Arith { lhs, rhs, .. } => {
            check(lhs, place)?;
            check(rhs, place)
        }
        Expr::Neg { arg } | Expr::Not { arg } | Expr::IsNull { arg } => check(arg, place),
        Expr::And { args } | Expr::Or { args } => args.iter().try_for_each(|a| check(a, place)),
        Expr::In { arg, list } => {
            if list.is_empty() {
                return err(Case::InEmpty);
            }
            check(arg, place)?;
            list.iter().try_for_each(|a| check(a, place))
        }
        Expr::Call { func, args } => {
            let name = super::func_name(*func).to_owned();
            let (min, max) = super::arity(*func);
            if args.len() < min || max.is_some_and(|m| args.len() > m) {
                return err(Case::CallArity {
                    name,
                    given: args.len(),
                });
            }
            match func {
                Func::KnownAt | Func::SeenAt | Func::Previous | Func::Changed if !matches!(args.first(), Some(Expr::Path { path }) if path.len() == 1) =>
                {
                    return err(Case::CallTakesField { name });
                }
                Func::Setting | Func::Param | Func::InSet if quoted(args.first()).is_none() => {
                    return err(Case::CallTakesQuoted { name });
                }
                // A calendar unit, quoted, starts where the calendar says and
                // takes no origin; a width is a duration above zero, with
                // the origin its windows are counted from.
                Func::DateBucket => {
                    let unit = quoted(args.get(1));
                    let width = matches!(args.get(1),
                        Some(Expr::Lit { value: Literal::Duration(w) }) if *w > 0);
                    let known = |u: &&String| BUCKET_UNITS.contains(&u.as_str());
                    if let Some(u) = unit.filter(|u| !known(u)) {
                        return err(Case::BucketUnit { unit: u.clone() });
                    }
                    if (unit.is_some(), width) != (args.len() == 2, args.len() == 3) {
                        return err(Case::BucketArguments);
                    }
                }
                Func::Previous | Func::Changed if !matches!(place, Place::Write | Place::Guard) => {
                    return err(Case::OnlyInWrite { name });
                }
                Func::Retired if place != Place::Guard => return err(Case::RetiredOnlyInGuard),
                Func::Exists if place != Place::Verb => return err(Case::ExistsHere),
                Func::Exists if !matches!(args.first(), Some(Expr::Path { path }) if path.len() == 1) =>
                {
                    return err(Case::CallTakesField { name });
                }
                f if f.is_aggregate() && place != Place::Group => {
                    return err(Case::AggregateHere { name });
                }
                Func::Param if !matches!(place, Place::View | Place::Group) => {
                    return err(Case::ParamOutsideView);
                }
                _ => {}
            }
            // An aggregate's argument is evaluated per source record.
            let inner = if func.is_aggregate() {
                Place::View
            } else {
                place
            };
            args.iter().try_for_each(|a| check(a, inner))
        }
    }
}

/// `e` with every `param('name')` replaced by the literal `values` holds
/// for that name; a name it does not hold is left as the call, which reads
/// null.
pub fn bind_params(e: &Expr, values: &BTreeMap<String, Literal>) -> Expr {
    lakelands_api::expr::rewrite(e, &|x| match x {
        Expr::Call {
            func: Func::Param,
            args,
        } => {
            let value = quoted(args.first()).and_then(|name| values.get(name));
            value.map(|v| Expr::Lit { value: v.clone() })
        }
        _ => None,
    })
}

/// The names every `param('name')` in `e` reads, added to `out`.
pub fn params_read(e: &Expr, out: &mut std::collections::BTreeSet<String>) {
    if let Expr::Call {
        func: Func::Param,
        args,
    } = e
    {
        if let Some(name) = quoted(args.first()) {
            out.insert(name.clone());
        }
    } else {
        super::children(e).for_each(|c| params_read(c, out));
    }
}

/// The text of a quoted literal.
fn quoted(e: Option<&Expr>) -> Option<&String> {
    match e {
        Some(Expr::Lit {
            value: Literal::Text(t),
        }) => Some(t),
        _ => None,
    }
}

/// Whether a condition holds: true only when it evaluates to `true`. A
/// null or a non-boolean does not hold.
pub fn holds(e: &Expr, scope: &dyn Scope) -> Result<bool> {
    Ok(truthy(&eval(e, scope)?))
}

fn truthy(v: &Val) -> bool {
    *v == Val::Bool(true)
}

fn overflow() -> Case {
    Case::Overflow
}

fn literal(l: &Literal) -> Result<Val> {
    Ok(match l {
        Literal::Null => Val::Null,
        Literal::Bool(b) => Val::Bool(*b),
        Literal::Number(n) => {
            Val::Num(Decimal::from_str(n).or_else(|_| err(Case::NotExact { number: n.clone() }))?)
        }
        Literal::Text(t) => Val::Text(t.clone()),
        Literal::Duration(d) => Val::Duration(*d),
    })
}

/// A value as an exact number, if it reads as one.
pub(crate) fn as_number(v: &Val) -> Option<Decimal> {
    match v {
        Val::Num(d) => Some(*d),
        Val::Text(s) => Decimal::from_str(s).ok(),
        _ => None,
    }
}

fn number(v: &Val) -> Result<Decimal> {
    as_number(v).ok_or_else(|| Case::NotANumber { value: operand(v) })
}

/// A value as the instant it denotes, if it reads as one. A text is read
/// without the bound of the years 0000 to 9999: here it is compared or
/// computed with, and `'9999-12-31T23:30:00-01:00'` is after every
/// instant a field holds, not a text to compare letter by letter. What
/// is written is judged where it lands.
pub(crate) fn as_time(v: &Val) -> Option<Timestamp> {
    match v {
        Val::Time(t) => Some(*t),
        Val::Text(s) => Timestamp::denoted(s).ok(),
        _ => None,
    }
}

/// A value as a case names it.
pub(crate) fn operand(v: &Val) -> Box<Operand> {
    Box::new(match v {
        Val::Null => Operand::Null,
        Val::Bool(value) => Operand::Boolean { value: *value },
        Val::Num(d) => Operand::Number {
            value: d.to_string(),
        },
        Val::Text(value) => Operand::Text {
            value: value.clone(),
        },
        Val::Time(value) => Operand::Instant { value: *value },
        Val::Duration(micros) => Operand::Duration { micros: *micros },
        Val::Json(_) => Operand::Json,
    })
}

/// The order of two non-null values, when they have one: numbers (and
/// text that reads as a number) numerically, instants as instants,
/// durations as durations, other text as text. Values of kinds that do not
/// compare — a number against text that is not one, a boolean against
/// anything but a boolean, a JSON value against anything — have none.
fn order(l: &Val, r: &Val) -> Option<Ordering> {
    if let (Some(a), Some(b)) = (as_number(l), as_number(r)) {
        return Some(a.cmp(&b));
    }
    match (l, r) {
        (Val::Duration(a), Val::Duration(b)) => Some(a.cmp(b)),
        (Val::Time(_), _) | (_, Val::Time(_)) => Some(as_time(l)?.cmp(&as_time(r)?)),
        (Val::Text(a), Val::Text(b)) => Some(match (as_time(l), as_time(r)) {
            (Some(x), Some(y)) => x.cmp(&y),
            _ => a.cmp(b),
        }),
        _ => None,
    }
}

/// A comparison. It never fails: null on either side, or two values of
/// kinds that do not compare, make every operator false — `!=` included —
/// so a filter over mixed data selects what matches and skips the rest
/// rather than failing the whole read. Booleans and JSON values are equal
/// or not, and have no order.
pub fn compare(op: CmpOp, l: &Val, r: &Val) -> Result<bool> {
    if *l == Val::Null || *r == Val::Null {
        return Ok(false);
    }
    let same = match (l, r) {
        (Val::Bool(a), Val::Bool(b)) => Some(a == b),
        (Val::Json(a), Val::Json(b)) => Some(a == b),
        (Val::Bool(_) | Val::Json(_), _) | (_, Val::Bool(_) | Val::Json(_)) => None,
        _ => order(l, r).map(Ordering::is_eq),
    };
    Ok(match op {
        CmpOp::Eq => same == Some(true),
        CmpOp::Ne => same == Some(false),
        CmpOp::Lt => order(l, r).is_some_and(Ordering::is_lt),
        CmpOp::Le => order(l, r).is_some_and(Ordering::is_le),
        CmpOp::Gt => order(l, r).is_some_and(Ordering::is_gt),
        CmpOp::Ge => order(l, r).is_some_and(Ordering::is_ge),
    })
}

fn arith_op(op: ArithOp, l: Val, r: Val) -> Result<Val> {
    if l == Val::Null || r == Val::Null {
        return Ok(Val::Null);
    }
    let time_or_duration = |v: &Val| matches!(v, Val::Time(_) | Val::Duration(_));
    let two_instants_as_text = op == ArithOp::Sub
        && as_number(&l).is_none()
        && as_number(&r).is_none()
        && as_time(&l).is_some()
        && as_time(&r).is_some();
    if time_or_duration(&l) || time_or_duration(&r) || two_instants_as_text {
        return time_arith(op, &l, &r);
    }
    let (a, b) = (number(&l)?, number(&r)?);
    let out = match op {
        ArithOp::Add => a.checked_add(b),
        ArithOp::Sub => a.checked_sub(b),
        ArithOp::Mul => a.checked_mul(b),
        ArithOp::Div => {
            if b.is_zero() {
                return err(Case::DivisionByZero);
            }
            a.checked_div(b)
        }
    };
    out.map(Val::Num).ok_or_else(overflow)
}

fn time_arith(op: ArithOp, l: &Val, r: &Val) -> Result<Val> {
    let shift = |t: Timestamp, d: i64, sign: i64| {
        d.checked_mul(sign)
            .and_then(|d| t.0.checked_add(d))
            .map(|m| Val::Time(Timestamp(m)))
            .ok_or_else(overflow)
    };
    match (op, l, r) {
        (ArithOp::Add, Val::Duration(a), Val::Duration(b)) => {
            a.checked_add(*b).map(Val::Duration).ok_or_else(overflow)
        }
        (ArithOp::Sub, Val::Duration(a), Val::Duration(b)) => {
            a.checked_sub(*b).map(Val::Duration).ok_or_else(overflow)
        }
        (ArithOp::Add, t, Val::Duration(d))
        | (ArithOp::Add, Val::Duration(d), t)
        | (ArithOp::Sub, t, Val::Duration(d)) => match as_time(t) {
            Some(t) => shift(t, *d, if op == ArithOp::Sub { -1 } else { 1 }),
            None => err(Case::NotShiftable { value: operand(t) }),
        },
        (ArithOp::Sub, a, b) => match (as_time(a), as_time(b)) {
            (Some(x), Some(y)) => x.0.checked_sub(y.0).map(Val::Duration).ok_or_else(overflow),
            _ => err(Case::MinusNoMeaning {
                left: operand(a),
                right: operand(b),
            }),
        },
        // A duration by a duration is how many of the one the other holds;
        // a duration by or times a number is a duration, to the microsecond.
        (ArithOp::Div, Val::Duration(a), Val::Duration(b)) if *b != 0 => {
            let n = Decimal::from(*a).checked_div(Decimal::from(*b));
            n.map(Val::Num).ok_or_else(overflow)
        }
        (ArithOp::Div, Val::Duration(_), Val::Duration(_)) => err(Case::DivisionByZero),
        (ArithOp::Mul | ArithOp::Div, Val::Duration(d), n)
        | (ArithOp::Mul, n, Val::Duration(d))
            if as_number(n).is_some() =>
        {
            let n = number(n)?;
            let scaled = match op {
                ArithOp::Div if n.is_zero() => return err(Case::DivisionByZero),
                ArithOp::Div => Decimal::from(*d).checked_div(n),
                _ => Decimal::from(*d).checked_mul(n),
            };
            let micros = scaled.and_then(|m| m.trunc().to_i64());
            micros.map(Val::Duration).ok_or_else(overflow)
        }
        (_, a, b) => err(Case::NotMultiplied {
            left: operand(a),
            right: operand(b),
        }),
    }
}

/// A value as text, for text functions: numbers in their exact form,
/// booleans as `true` / `false`, instants in the canonical form.
fn text(v: &Val) -> Option<String> {
    Some(match v {
        Val::Null => return None,
        Val::Bool(b) => b.to_string(),
        Val::Num(d) => d.normalize().to_string(),
        Val::Text(s) => s.clone(),
        Val::Time(t) => t.to_string(),
        Val::Duration(d) => format!("{d}"),
        Val::Json(j) => j.to_string(),
    })
}

fn usize_of(v: &Val) -> Result<usize> {
    let d = number(v)?;
    if d.is_sign_negative() || !d.fract().is_zero() {
        return err(Case::NotPosition {
            number: d.to_string(),
        });
    }
    d.to_usize().ok_or_else(overflow)
}

fn field_arg<'a>(name: &str, args: &'a [Expr]) -> Result<&'a FieldName> {
    match args.first() {
        Some(Expr::Path { path }) if path.len() == 1 => Ok(&path[0]),
        _ => err(Case::CallTakesField { name: name.into() }),
    }
}

fn call(func: Func, args: &[Expr], scope: &dyn Scope) -> Result<Val> {
    // A typed expression reaches here without the parser's checks; the
    // argument count is checked again so no malformed call can panic.
    let (min, max) = super::arity(func);
    let name = super::func_name(func);
    if args.len() < min || max.is_some_and(|m| args.len() > m) {
        let (name, given) = (name.into(), args.len());
        return err(Case::CallArity { name, given });
    }
    // The functions that decide which arguments to evaluate, or read a
    // field's name rather than its value.
    match func {
        Func::If => {
            return if holds(&args[0], scope)? {
                eval(&args[1], scope)
            } else {
                eval(&args[2], scope)
            };
        }
        Func::Coalesce => {
            for a in args {
                let v = eval(a, scope)?;
                if v != Val::Null {
                    return Ok(v);
                }
            }
            return Ok(Val::Null);
        }
        Func::KnownAt | Func::SeenAt => {
            let meta = scope.meta(field_arg(name, args)?);
            return Ok(match (func, meta) {
                (Func::KnownAt, Some(m)) => Val::Time(m.known_at),
                (Func::SeenAt, Some(m)) => m.seen_at.map_or(Val::Null, Val::Time),
                _ => Val::Null,
            });
        }
        Func::Previous | Func::Changed => {
            if !scope.in_write() {
                return err(Case::OnlyInWrite { name: name.into() });
            }
            let field = field_arg(name, args)?;
            let before = scope.previous(field).map_or(Val::Null, |v| from_json(&v));
            if func == Func::Previous {
                return Ok(before);
            }
            let after = scope
                .value(std::slice::from_ref(field))
                .map_or(Val::Null, |v| from_json(&v));
            let same = match (&before, &after) {
                (Val::Null, Val::Null) => true,
                (Val::Null, _) | (_, Val::Null) => false,
                (a, b) => compare(CmpOp::Eq, a, b)?,
            };
            return Ok(Val::Bool(!same));
        }
        Func::Retired => {
            if !scope.in_write() {
                return err(Case::RetiredOnlyInGuard);
            }
            return Ok(Val::Bool(scope.retiring()));
        }
        Func::Exists => {
            let relation = field_arg(name, args)?;
            return scope
                .exists(relation)
                .map_or(err(Case::ExistsHere), |b| Ok(Val::Bool(b)));
        }
        Func::Count | Func::Sum | Func::Min | Func::Max | Func::Avg => {
            if let Some(v) = scope.aggregate(func, args) {
                return v;
            }
            return err(Case::AggregateHere { name: name.into() });
        }
        _ => {}
    }
    let vals = args
        .iter()
        .map(|a| eval(a, scope))
        .collect::<Result<Vec<_>>>()?;
    if vals.contains(&Val::Null) && func != Func::Concat {
        return Ok(Val::Null);
    }
    let s = |i: usize| text(&vals[i]).unwrap_or_default();
    Ok(match func {
        Func::Now => Val::Time(scope.now()),
        Func::Today => {
            let day = 86_400_000_000;
            Val::Time(Timestamp(scope.now().0.div_euclid(day) * day))
        }
        Func::Setting => match &vals[0] {
            Val::Text(key) => scope.setting(key).map_or(Val::Null, |v| from_json(&v)),
            other => {
                return err(Case::SettingTakesKey {
                    value: operand(other),
                });
            }
        },
        // A set's filter stands in the call's place before a condition is
        // evaluated ([`expand`]); a call that is left names no place for it.
        Func::InSet => {
            return err(Case::InSetHere);
        }
        // Answered above, from the scope.
        Func::Exists => return err(Case::ExistsHere),
        Func::Param => match &vals[0] {
            Val::Text(name) => scope.param(name).map_or(Val::Null, |v| from_json(&v)),
            other => {
                return err(Case::ParamTakesName {
                    value: operand(other),
                });
            }
        },
        Func::Lower => Val::Text(s(0).to_lowercase()),
        Func::Upper => Val::Text(s(0).to_uppercase()),
        Func::Trim => Val::Text(s(0).trim().to_owned()),
        Func::Len => Val::Num(Decimal::from(s(0).chars().count())),
        Func::Concat => Val::Text(vals.iter().filter_map(text).collect()),
        Func::Substr => {
            let chars: Vec<char> = s(0).chars().collect();
            let start = usize_of(&vals[1])?.min(chars.len());
            let end = match vals.get(2) {
                Some(len) => start.saturating_add(usize_of(len)?).min(chars.len()),
                None => chars.len(),
            };
            Val::Text(chars[start..end].iter().collect())
        }
        Func::Contains => Val::Bool(s(0).contains(&s(1))),
        Func::StartsWith => Val::Bool(s(0).starts_with(&s(1))),
        Func::Replace => {
            let from = s(1);
            Val::Text(if from.is_empty() {
                s(0)
            } else {
                s(0).replace(&from, &s(2))
            })
        }
        Func::Matches => Val::Bool(i_regexp(&s(1))?.is_match(&s(0))),
        Func::Abs => Val::Num(number(&vals[0])?.abs()),
        Func::Floor => Val::Num(number(&vals[0])?.floor()),
        Func::DateBucket => match (as_time(&vals[0]), &vals[1]) {
            (Some(t), Val::Text(unit)) => Val::Time(bucket_start(t, unit)?),
            // The window of that width `t` falls in, counted from the origin
            // in both directions: an instant before it is in a window too.
            (Some(t), Val::Duration(w)) if *w > 0 => match vals.get(2).and_then(as_time) {
                Some(o) => {
                    let windows = t.0.checked_sub(o.0).map(|d| d.div_euclid(*w));
                    let start = windows.and_then(|n| n.checked_mul(*w));
                    let start = start.and_then(|d| o.0.checked_add(d));
                    Val::Time(Timestamp(start.ok_or_else(overflow)?))
                }
                None => Val::Null,
            },
            (None, _) => Val::Null,
            (_, other) => {
                return err(Case::BucketUnitText {
                    value: operand(other),
                });
            }
        },
        Func::Round => {
            let digits = match vals.get(1) {
                Some(d) => u32::try_from(usize_of(d)?).map_err(|_| overflow())?,
                None => 0,
            };
            Val::Num(
                number(&vals[0])?
                    .round_dp_with_strategy(digits, RoundingStrategy::MidpointAwayFromZero),
            )
        }
        Func::If
        | Func::Coalesce
        | Func::KnownAt
        | Func::SeenAt
        | Func::Previous
        | Func::Changed
        | Func::Retired
        | Func::Count
        | Func::Sum
        | Func::Min
        | Func::Max
        | Func::Avg => unreachable!("handled above"),
    })
}

/// An I-Regexp pattern (RFC 9485), matched against the whole text; see
/// `iregexp` for what is refused and how it is translated.
fn i_regexp(pattern: &str) -> Result<regex::Regex> {
    let p = || pattern.to_owned();
    let not = |why| Case::NotIRegexp {
        pattern: p(),
        why: Box::new(why),
    };
    let translated = super::iregexp::translate(pattern).map_err(not)?;
    regex::Regex::new(&translated).map_err(|e| {
        let limit = match e {
            regex::Error::CompiledTooBig(limit) => Some(limit),
            _ => None,
        };
        Case::RegexUnmatchable {
            pattern: p(),
            limit,
        }
    })
}

/// Land a computed value in a field's JSON form (`docs/log-format.md` §5):
/// a decimal as its exact digits, a number as a JSON number, an instant in
/// the canonical form. `scale` rounds a decimal half away from zero first.
/// `None` when the value does not fit the kind; the caller clears the field.
pub fn to_json(
    v: &Val,
    kind: lakelands_api::ontology::StorageKind,
    scale: Option<u32>,
) -> Option<Value> {
    use lakelands_api::ontology::StorageKind as K;
    // A value at a declared scale is rounded half away from zero and then
    // written with exactly that many fractional digits, as a written value
    // of the type is.
    let rounded = |d: Decimal| scale.map_or(d, |s| crate::value::at_scale(d, s));
    match (v, kind) {
        (Val::Null, _) => Some(Value::Null),
        (v, K::Decimal) => as_number(v).map(|d| Value::String(rounded(d).to_string())),
        (v, K::Number) => as_number(v).and_then(|d| {
            let d = rounded(d);
            // A whole number past 64 bits lands as its double, for the
            // judgement of what is written to refuse by its size.
            let whole = d.fract().is_zero().then(|| d.to_i64()).flatten();
            let double = || d.to_f64().and_then(serde_json::Number::from_f64);
            whole
                .map(Value::from)
                .or_else(|| double().map(Value::Number))
        }),
        (Val::Bool(b), K::Boolean) => Some(Value::Bool(*b)),
        (v, K::DateTime) => as_time(v).map(|t| Value::String(t.to_string())),
        (v, K::Date) => as_time(v).and_then(Timestamp::day).map(Value::String),
        (v, K::Text | K::Reference) => text(v).map(Value::String),
        (Val::Json(j), K::Json | K::Geo | K::Vector) => Some(j.clone()),
        (v, K::Json) => text(v).map(Value::String),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
