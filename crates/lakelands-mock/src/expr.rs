//! Enough of the one expression grammar for the doors' tests: the text
//! form parsed into the typed core, and the core evaluated over one
//! record.
//!
//! The parser takes the whole grammar of `lakelands_api::expr`. The
//! evaluator knows comparisons, `and`, `or`, `not`, `in`, `is null`,
//! arithmetic and the string functions `lower`, `upper`, `trim`, `len`,
//! `contains`, `starts_with`, `concat` and `coalesce`, a date-time shifted
//! by a duration, and `now()` and `today()` where the scope has a clock
//! (an aging run's date); any other function, `setting()` among them, is
//! refused by name. As on the store, a function over a null input is null,
//! known or not, but `concat`, which skips it, and `coalesce`; a text
//! function reads a number or a boolean as its text.
//! Comparisons with `null` do not hold; `not` holds of anything but `true`.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use lakelands_api::error::{Error, Refusal};
use lakelands_api::expr::{ArithOp, CmpOp, Condition, Expr, Func, Literal};
use lakelands_api::id::FieldName;
use lakelands_api::record::Value;
use lakelands_api::time::Timestamp;

fn invalid(message: impl Into<String>) -> Error {
    Error::Refused {
        field: None,
        reason: Refusal::Invalid {
            message: message.into(),
        },
    }
}

/// The typed form of a condition, parsing the text form.
pub(crate) fn typed(c: &Condition) -> Result<Expr, Error> {
    match c {
        Condition::Expr(e) => Ok(e.clone()),
        Condition::Text(s) => Parser::new(s)?.whole(),
    }
}

/// As the store: the member of the call's payload a path in a verb's value
/// reads, `payload.<name>` and no other path.
pub(crate) fn payload_member(path: &[FieldName]) -> Option<&FieldName> {
    match path {
        [payload, name] if payload.as_str() == "payload" => Some(name),
        _ => None,
    }
}

/// Every path an expression reads.
pub(crate) fn paths(e: &Expr, out: &mut Vec<Vec<FieldName>>) {
    match e {
        Expr::Lit { .. } => {}
        Expr::Path { path } => out.push(path.clone()),
        Expr::Cmp { lhs, rhs, .. } | Expr::Arith { lhs, rhs, .. } => {
            paths(lhs, out);
            paths(rhs, out);
        }
        Expr::Neg { arg } | Expr::Not { arg } | Expr::IsNull { arg } => paths(arg, out),
        Expr::And { args } | Expr::Or { args } | Expr::Call { args, .. } => {
            args.iter().for_each(|a| paths(a, out))
        }
        Expr::In { arg, list } => {
            paths(arg, out);
            list.iter().for_each(|a| paths(a, out));
        }
    }
}

/// The fields a view's measure reads from its source: the paths inside an
/// aggregate's arguments. A name outside an aggregate is a measure or a
/// grouping field, as the store reads it.
pub(crate) fn aggregated(e: &Expr, inside: bool, out: &mut Vec<FieldName>) {
    match e {
        Expr::Path { path } if inside => out.extend(path.first().cloned()),
        Expr::Lit { .. } | Expr::Path { .. } => {}
        Expr::Call { func, args } => args
            .iter()
            .for_each(|a| aggregated(a, inside || func.is_aggregate(), out)),
        Expr::Cmp { lhs, rhs, .. } | Expr::Arith { lhs, rhs, .. } => {
            aggregated(lhs, inside, out);
            aggregated(rhs, inside, out);
        }
        Expr::Neg { arg } | Expr::Not { arg } | Expr::IsNull { arg } => {
            aggregated(arg, inside, out)
        }
        Expr::And { args } | Expr::Or { args } => {
            args.iter().for_each(|a| aggregated(a, inside, out))
        }
        Expr::In { arg, list } => {
            aggregated(arg, inside, out);
            list.iter().for_each(|a| aggregated(a, inside, out));
        }
    }
}

/// What a path may name while an expression is evaluated.
pub(crate) struct Scope<'a> {
    /// The record's type name, so that `<type>.<field>` resolves.
    pub ty: &'a str,
    /// The record's fields.
    pub fields: &'a BTreeMap<FieldName, Value>,
    /// A verb's payload, as `payload.<field>`.
    pub payload: Option<&'a BTreeMap<FieldName, Value>>,
    /// What `now()` means here, and `today()` its date at midnight; `None`
    /// where the mock keeps no clock, and the two are refused.
    pub now: Option<Timestamp>,
}

impl Scope<'_> {
    fn path(&self, p: &[FieldName]) -> Value {
        let found = match (p, payload_member(p)) {
            (_, Some(f)) => self.payload.and_then(|m| m.get(f)),
            ([f], _) => self.fields.get(f),
            ([head, f], _) if head.as_str() == self.ty => self.fields.get(f),
            _ => None,
        };
        found.cloned().unwrap_or(Value::Null)
    }
}

/// Whether a condition holds for a record.
pub(crate) fn holds(e: &Expr, scope: &Scope<'_>) -> Result<bool, Error> {
    Ok(eval(e, scope)? == Value::Bool(true))
}

/// The value of an expression over a record.
pub(crate) fn eval(e: &Expr, s: &Scope<'_>) -> Result<Value, Error> {
    Ok(match e {
        Expr::Lit { value } => literal(value),
        Expr::Path { path } => s.path(path),
        Expr::Cmp { cmp, lhs, rhs } => {
            let (a, b) = (eval(lhs, s)?, eval(rhs, s)?);
            let o = compare(&a, &b);
            Value::Bool(match cmp {
                CmpOp::Eq => o == Some(Ordering::Equal),
                CmpOp::Ne => o.is_some_and(|o| o != Ordering::Equal),
                CmpOp::Lt => o == Some(Ordering::Less),
                CmpOp::Le => matches!(o, Some(Ordering::Less | Ordering::Equal)),
                CmpOp::Gt => o == Some(Ordering::Greater),
                CmpOp::Ge => matches!(o, Some(Ordering::Greater | Ordering::Equal)),
            })
        }
        Expr::Arith { arith, lhs, rhs } => {
            let (a, b) = (eval(lhs, s)?, eval(rhs, s)?);
            // The difference of two instants is a duration, a number of
            // microseconds here.
            if let (Value::String(a), Value::String(b), ArithOp::Sub) = (&a, &b, arith)
                && let (Ok(a), Ok(b)) = (Timestamp::parse(a), Timestamp::parse(b))
            {
                return Ok(num((a.0 - b.0) as f64));
            }
            // A date-time shifted by a duration (a number of microseconds).
            if let (Value::String(t), Value::Number(d), ArithOp::Add | ArithOp::Sub) =
                (&a, &b, arith)
                && let (Ok(t), Some(d)) = (Timestamp::parse(t), d.as_i64())
            {
                let d = if matches!(arith, ArithOp::Sub) { -d } else { d };
                return Ok(Value::String(Timestamp(t.0 + d).to_string()));
            }
            match (number(&a), number(&b)) {
                (Some(a), Some(b)) => num(match arith {
                    ArithOp::Add => a + b,
                    ArithOp::Sub => a - b,
                    ArithOp::Mul => a * b,
                    ArithOp::Div => a / b,
                }),
                _ => Value::Null,
            }
        }
        Expr::Neg { arg } => number(&eval(arg, s)?).map_or(Value::Null, |n| num(-n)),
        Expr::And { args } => {
            for a in args {
                if !holds(a, s)? {
                    return Ok(Value::Bool(false));
                }
            }
            Value::Bool(true)
        }
        Expr::Or { args } => {
            for a in args {
                if holds(a, s)? {
                    return Ok(Value::Bool(true));
                }
            }
            Value::Bool(false)
        }
        // As the store: `not` holds of anything but `true`, null included.
        Expr::Not { arg } => Value::Bool(eval(arg, s)? != Value::Bool(true)),
        Expr::In { arg, list } => {
            let v = eval(arg, s)?;
            let mut found = false;
            for c in list {
                found |= compare(&v, &eval(c, s)?) == Some(Ordering::Equal);
            }
            Value::Bool(found)
        }
        Expr::IsNull { arg } => Value::Bool(eval(arg, s)?.is_null()),
        Expr::Call {
            func: func @ (Func::Now | Func::Today),
            args,
        } if args.is_empty() => {
            let now = s
                .now
                .ok_or_else(|| invalid("the in-memory mock keeps no clock here"))?;
            let day = 86_400 * 1_000_000;
            let t = match func {
                Func::Today => Timestamp(now.0 - now.0.rem_euclid(day)),
                _ => now,
            };
            Value::String(t.to_string())
        }
        Expr::Call { func, args } => {
            let args = args
                .iter()
                .map(|a| eval(a, s))
                .collect::<Result<Vec<_>, _>>()?;
            call(*func, &args)?
        }
    })
}

fn call(func: Func, args: &[Value]) -> Result<Value, Error> {
    // As the store: a function over a null input is null, but `concat`,
    // which skips it, and `coalesce`, which chooses.
    if !matches!(func, Func::Concat | Func::Coalesce) && args.iter().any(Value::is_null) {
        return Ok(Value::Null);
    }
    let text = |i: usize| args.get(i).map(as_text).unwrap_or_default();
    let string = |s: String| Value::String(s);
    Ok(match func {
        Func::Lower => string(text(0).to_lowercase()),
        Func::Upper => string(text(0).to_uppercase()),
        Func::Trim => string(text(0).trim().to_owned()),
        Func::Len => Value::from(text(0).chars().count()),
        Func::Contains => Value::Bool(text(0).contains(&text(1))),
        Func::StartsWith => Value::Bool(text(0).starts_with(&text(1))),
        Func::Concat => string(args.iter().map(as_text).collect()),
        Func::Coalesce => args
            .iter()
            .find(|a| !a.is_null())
            .cloned()
            .unwrap_or(Value::Null),
        Func::Floor => (args.first().and_then(number)).map_or(Value::Null, |n| num(n.floor())),
        other => {
            return Err(invalid(format!(
                "the in-memory mock does not evaluate `{}`",
                serde_json::to_value(other)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default()
            )));
        }
    })
}

/// A value as a text function reads it, as the store does: a number or a
/// boolean as its text, null as nothing.
fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn literal(l: &Literal) -> Value {
    match l {
        Literal::Null => Value::Null,
        Literal::Bool(b) => Value::Bool(*b),
        Literal::Number(n) => serde_json::from_str(n).unwrap_or(Value::Null),
        Literal::Text(t) => Value::String(t.clone()),
        Literal::Duration(d) => Value::from(*d),
    }
}

fn num(n: f64) -> Value {
    serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number)
}

/// A value as a number: numbers, and decimals written as strings.
fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// How two values order; `None` when they do not compare (either is
/// null, or their kinds differ).
pub(crate) fn compare(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::Null, _) | (_, Value::Null) => None,
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        (Value::String(x), Value::String(y)) => match (Timestamp::parse(x), Timestamp::parse(y)) {
            (Ok(x), Ok(y)) => Some(x.cmp(&y)),
            _ => match (x.parse::<f64>(), y.parse::<f64>()) {
                (Ok(x), Ok(y)) => x.partial_cmp(&y),
                _ => Some(x.cmp(y)),
            },
        },
        (Value::Number(_) | Value::String(_), Value::Number(_) | Value::String(_)) => {
            number(a)?.partial_cmp(&number(b)?)
        }
        _ => (a == b).then_some(Ordering::Equal),
    }
}

// ---- the text form ----

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Num(String),
    Dur(i64),
    Str(String),
    Sym(&'static str),
}

struct Parser {
    toks: Vec<Tok>,
    at: usize,
}

const SYMBOLS: [&str; 13] = [
    "!=", "<=", ">=", "=", "<", ">", "+", "-", "*", "/", "(", ")", ",",
];

impl Parser {
    fn new(src: &str) -> Result<Parser, Error> {
        let b = src.as_bytes();
        let mut toks = Vec::new();
        let mut i = 0;
        while i < b.len() {
            let c = b[i];
            if c.is_ascii_whitespace() {
                i += 1;
            } else if c == b'\'' {
                let mut s = String::new();
                i += 1;
                loop {
                    match b.get(i) {
                        None => return Err(invalid(format!("unterminated string in `{src}`"))),
                        Some(b'\'') if b.get(i + 1) == Some(&b'\'') => {
                            s.push('\'');
                            i += 2;
                        }
                        Some(b'\'') => {
                            i += 1;
                            break;
                        }
                        Some(_) => {
                            let ch = src[i..].chars().next().unwrap_or_default();
                            s.push(ch);
                            i += ch.len_utf8();
                        }
                    }
                }
                toks.push(Tok::Str(s));
            } else if c.is_ascii_digit() {
                let start = i;
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                    i += 1;
                }
                let n = &src[start..i];
                let unit = b.get(i).copied();
                let ident_follows = b.get(i + 1).is_some_and(|c| c.is_ascii_alphanumeric());
                match unit {
                    Some(u @ (b'd' | b'h' | b'm' | b's')) if !ident_follows => {
                        let per: i64 = match u {
                            b'd' => 86_400_000_000,
                            b'h' => 3_600_000_000,
                            b'm' => 60_000_000,
                            _ => 1_000_000,
                        };
                        let n: f64 = n
                            .parse()
                            .map_err(|_| invalid(format!("bad number `{n}`")))?;
                        toks.push(Tok::Dur((n * per as f64) as i64));
                        i += 1;
                    }
                    _ => toks.push(Tok::Num(n.to_owned())),
                }
            } else if c.is_ascii_alphabetic() || c == b'_' {
                let start = i;
                while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.')
                {
                    i += 1;
                }
                toks.push(Tok::Ident(src[start..i].to_owned()));
            } else if let Some(sym) = SYMBOLS.iter().find(|s| src[i..].starts_with(**s)) {
                toks.push(Tok::Sym(sym));
                i += sym.len();
            } else {
                return Err(invalid(format!(
                    "unexpected `{}` in `{src}`",
                    src[i..].chars().next().unwrap_or_default()
                )));
            }
        }
        Ok(Parser { toks, at: 0 })
    }

    fn whole(mut self) -> Result<Expr, Error> {
        let e = self.or()?;
        match self.toks.get(self.at) {
            None => Ok(e),
            Some(t) => Err(invalid(format!(
                "unexpected {t:?} after a complete expression"
            ))),
        }
    }

    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.at)
    }

    fn keyword(&mut self, k: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Ident(i)) if i == k) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn sym(&mut self, s: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Sym(x)) if *x == s) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, s: &str) -> Result<(), Error> {
        if self.sym(s) {
            Ok(())
        } else {
            Err(invalid(format!("expected `{s}`")))
        }
    }

    fn or(&mut self) -> Result<Expr, Error> {
        let mut args = vec![self.and()?];
        while self.keyword("or") {
            args.push(self.and()?);
        }
        Ok(if args.len() == 1 {
            args.remove(0)
        } else {
            Expr::Or { args }
        })
    }

    fn and(&mut self) -> Result<Expr, Error> {
        let mut args = vec![self.not()?];
        while self.keyword("and") {
            args.push(self.not()?);
        }
        Ok(if args.len() == 1 {
            args.remove(0)
        } else {
            Expr::And { args }
        })
    }

    fn not(&mut self) -> Result<Expr, Error> {
        if self.keyword("not") {
            return Ok(Expr::Not {
                arg: Box::new(self.not()?),
            });
        }
        self.cmp()
    }

    fn cmp(&mut self) -> Result<Expr, Error> {
        let lhs = self.sum()?;
        let op = match self.peek() {
            Some(Tok::Sym("=")) => CmpOp::Eq,
            Some(Tok::Sym("!=")) => CmpOp::Ne,
            Some(Tok::Sym("<")) => CmpOp::Lt,
            Some(Tok::Sym("<=")) => CmpOp::Le,
            Some(Tok::Sym(">")) => CmpOp::Gt,
            Some(Tok::Sym(">=")) => CmpOp::Ge,
            _ => {
                if self.keyword("in") {
                    self.expect("(")?;
                    let list = self.list()?;
                    self.expect(")")?;
                    return Ok(Expr::In {
                        arg: Box::new(lhs),
                        list,
                    });
                }
                if self.keyword("is") {
                    let negated = self.keyword("not");
                    if !self.keyword("null") {
                        return Err(invalid("expected `null` after `is`"));
                    }
                    let e = Expr::IsNull { arg: Box::new(lhs) };
                    return Ok(if negated {
                        Expr::Not { arg: Box::new(e) }
                    } else {
                        e
                    });
                }
                return Ok(lhs);
            }
        };
        self.at += 1;
        Ok(Expr::Cmp {
            cmp: op,
            lhs: Box::new(lhs),
            rhs: Box::new(self.sum()?),
        })
    }

    fn sum(&mut self) -> Result<Expr, Error> {
        let mut lhs = self.product()?;
        loop {
            let arith = if self.sym("+") {
                ArithOp::Add
            } else if self.sym("-") {
                ArithOp::Sub
            } else {
                return Ok(lhs);
            };
            lhs = Expr::Arith {
                arith,
                lhs: Box::new(lhs),
                rhs: Box::new(self.product()?),
            };
        }
    }

    fn product(&mut self) -> Result<Expr, Error> {
        let mut lhs = self.unary()?;
        loop {
            let arith = if self.sym("*") {
                ArithOp::Mul
            } else if self.sym("/") {
                ArithOp::Div
            } else {
                return Ok(lhs);
            };
            lhs = Expr::Arith {
                arith,
                lhs: Box::new(lhs),
                rhs: Box::new(self.unary()?),
            };
        }
    }

    fn unary(&mut self) -> Result<Expr, Error> {
        if self.sym("-") {
            return Ok(Expr::Neg {
                arg: Box::new(self.unary()?),
            });
        }
        self.atom()
    }

    fn list(&mut self) -> Result<Vec<Expr>, Error> {
        let mut out = vec![self.or()?];
        while self.sym(",") {
            out.push(self.or()?);
        }
        Ok(out)
    }

    fn atom(&mut self) -> Result<Expr, Error> {
        let lit = |value| Ok(Expr::Lit { value });
        let tok = self
            .toks
            .get(self.at)
            .cloned()
            .ok_or_else(|| invalid("the expression ends too early"))?;
        self.at += 1;
        match tok {
            Tok::Num(n) => lit(Literal::Number(n)),
            Tok::Dur(d) => lit(Literal::Duration(d)),
            Tok::Str(s) => lit(Literal::Text(s)),
            Tok::Sym("(") => {
                let e = self.or()?;
                self.expect(")")?;
                Ok(e)
            }
            Tok::Ident(i) if i == "true" => lit(Literal::Bool(true)),
            Tok::Ident(i) if i == "false" => lit(Literal::Bool(false)),
            Tok::Ident(i) if i == "null" => lit(Literal::Null),
            Tok::Ident(i) if self.sym("(") => {
                let func: Func = serde_json::from_value(Value::String(i.clone()))
                    .map_err(|_| invalid(format!("there is no function `{i}`")))?;
                let args = if self.sym(")") {
                    Vec::new()
                } else {
                    let a = self.list()?;
                    self.expect(")")?;
                    a
                };
                Ok(Expr::Call { func, args })
            }
            Tok::Ident(i) => Ok(Expr::Path {
                path: i.split('.').map(FieldName::from).collect(),
            }),
            Tok::Sym(s) => Err(invalid(format!("unexpected `{s}`"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(src: &str, fields: &[(&str, Value)]) -> bool {
        let fields = fields
            .iter()
            .map(|(k, v)| (FieldName::from(*k), v.clone()))
            .collect();
        let scope = Scope {
            ty: "invoice",
            fields: &fields,
            payload: None,
            now: None,
        };
        holds(&typed(&Condition::from(src)).unwrap(), &scope).unwrap()
    }

    #[test]
    fn the_text_form_parses_and_evaluates() {
        let r = [
            ("status", Value::from("open")),
            ("amount", Value::from("120.50")),
        ];
        assert!(check("status = 'open' and amount > 100", &r));
        assert!(check("invoice.amount >= 120.5", &r));
        assert!(!check("status = 'open' and not amount > 100", &r));
        assert!(check("status in ('paid', 'open') or amount < 0", &r));
        assert!(check("note is null and status is not null", &r));
        assert!(check("lower(upper(status)) = 'open'", &r));
        assert!(check("amount * 2 - 1 = 240", &r));
    }

    #[test]
    fn a_comparison_with_null_does_not_hold() {
        assert!(!check("note = 'x'", &[]));
        assert!(!check("note != 'x'", &[]));
    }

    #[test]
    fn broken_text_is_refused_as_invalid() {
        for src in ["status =", "status = 'open", "(status", "nope(1)", "a # b"] {
            let e = typed(&Condition::from(src)).unwrap_err();
            assert!(
                matches!(
                    e,
                    Error::Refused {
                        reason: Refusal::Invalid { .. },
                        ..
                    }
                ),
                "{src}"
            );
        }
    }
}
