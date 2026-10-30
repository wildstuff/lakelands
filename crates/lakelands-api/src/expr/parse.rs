//! The text form of the one expression grammar: parsing into
//! [`Expr`] and printing back.
//!
//! A recursive-descent parser over the grammar in [`crate::expr`],
//! with left-associative binary operators and the precedence, loosest
//! first: `or`, `and`, `not`, comparisons (`=`, `!=`, `<`, `<=`, `>`,
//! `>=`, `in`, `is [not] null`), `+ -`, `* /`, unary `-`. `and` and `or`
//! chains are flattened, so `a and b and c` is one `And` of three.
//!
//! Strings are single-quoted; a quote inside one is written twice
//! (`'it''s'`). There are no other escapes. A number is kept as written,
//! so a decimal stays exact; a number followed directly by `d`, `h`, `m`
//! or `s` is a duration. Printing is the inverse: `parse(print(e)) == e`
//! for every expression the parser can produce, and the printer adds
//! parentheses only where precedence needs them.

use std::fmt;

use super::{ArithOp, CmpOp, Expr, Func, Literal};
use crate::FieldName;
use crate::error::Case;

/// Why a text expression does not parse: its case, which names the
/// character offset where parsing stopped, and that offset.
#[derive(Clone, Debug, PartialEq)]
pub struct ParseError {
    /// The character offset where parsing stopped.
    pub at: usize,
    /// What was wrong.
    pub case: Case,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.case.fmt(f)
    }
}

impl From<ParseError> for Case {
    fn from(e: ParseError) -> Case {
        e.case
    }
}

impl std::error::Error for ParseError {}

type Result<T> = std::result::Result<T, ParseError>;

/// Parse an expression in the text form.
pub fn parse(text: &str) -> Result<Expr> {
    let mut p = Parser {
        chars: text.chars().collect(),
        pos: 0,
        depth: 0,
    };
    let e = p.or()?;
    p.ws();
    if p.pos != p.chars.len() {
        return Err(p.err(|offset| Case::ParseTrailing { offset }));
    }
    Ok(e)
}

/// The name a function has in the text form.
pub fn func_name(f: Func) -> &'static str {
    match f {
        Func::Now => "now",
        Func::Today => "today",
        Func::KnownAt => "known_at",
        Func::SeenAt => "seen_at",
        Func::Previous => "previous",
        Func::Preceding => "preceding",
        Func::Setting => "setting",
        Func::Param => "param",
        Func::InSet => "in_set",
        Func::Exists => "exists",
        Func::Changed => "changed",
        Func::Retired => "retired",
        Func::Created => "created",
        Func::Coalesce => "coalesce",
        Func::If => "if",
        Func::Lower => "lower",
        Func::Upper => "upper",
        Func::Trim => "trim",
        Func::Len => "len",
        Func::Concat => "concat",
        Func::Substr => "substr",
        Func::Contains => "contains",
        Func::Replace => "replace",
        Func::StartsWith => "starts_with",
        Func::Matches => "matches",
        Func::Abs => "abs",
        Func::Round => "round",
        Func::Floor => "floor",
        Func::DateBucket => "date_bucket",
        Func::Count => "count",
        Func::Sum => "sum",
        Func::Min => "min",
        Func::Max => "max",
        Func::Avg => "avg",
    }
}

/// A function by its text name, which is its serde name (`func_name`
/// spells the same names out).
pub fn func_by_name(name: &str) -> Option<Func> {
    serde_json::from_value(serde_json::Value::from(name)).ok()
}

/// How many arguments a function takes: `(min, max)`, `max` unbounded when
/// `None`.
pub fn arity(f: Func) -> (usize, Option<usize>) {
    match f {
        Func::Now | Func::Today | Func::Retired | Func::Created => (0, Some(0)),
        Func::KnownAt
        | Func::SeenAt
        | Func::Previous
        | Func::Param
        | Func::InSet
        | Func::Exists
        | Func::Changed
        | Func::Lower
        | Func::Upper
        | Func::Trim
        | Func::Len
        | Func::Abs
        | Func::Floor
        | Func::Sum
        | Func::Min
        | Func::Max
        | Func::Avg => (1, Some(1)),
        Func::Count => (0, Some(1)),
        Func::Round | Func::Setting => (1, Some(2)),
        Func::Contains | Func::StartsWith | Func::Matches => (2, Some(2)),
        Func::DateBucket => (2, Some(3)),
        Func::Substr => (2, Some(3)),
        Func::If | Func::Replace => (3, Some(3)),
        Func::Preceding => (3, None),
        Func::Coalesce | Func::Concat => (1, None),
    }
}

/// Whether a function's argument must name a field rather than compute a
/// value (`known_at(amount)`, not `known_at(amount + 1)`).
fn takes_field(f: Func) -> bool {
    matches!(
        f,
        Func::KnownAt | Func::SeenAt | Func::Previous | Func::Preceding | Func::Changed
    )
}

const KEYWORDS: [&str; 8] = ["and", "or", "not", "in", "is", "null", "true", "false"];

/// How deep an expression nests: every way back into `or` (a
/// parenthesis, a call argument, a list item) and every `not` and unary
/// `-` goes one deeper. A deeper expression is refused before the parser
/// recurses further, so no text runs it out of stack.
const NESTING: usize = 32;

struct Parser {
    chars: Vec<char>,
    pos: usize,
    depth: usize,
}

impl Parser {
    /// The case `said` makes of the offset where parsing stopped.
    fn err(&self, said: impl FnOnce(usize) -> Case) -> ParseError {
        let at = self.pos;
        let case = said(at);
        ParseError { at, case }
    }

    fn ws(&mut self) {
        self.skip(char::is_whitespace);
    }

    fn skip(&mut self, mut keep: impl FnMut(char) -> bool) {
        while self.peek().is_some_and(&mut keep) {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn eat(&mut self, s: &str) -> bool {
        self.ws();
        let n = s.chars().count();
        if self.chars[self.pos..].iter().take(n).copied().eq(s.chars()) {
            self.pos += n;
            true
        } else {
            false
        }
    }

    /// A keyword, only when not the prefix of a longer identifier.
    fn keyword(&mut self, kw: &str) -> bool {
        self.ws();
        let next = self.chars.get(self.pos + kw.chars().count());
        !next.is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_') && self.eat(kw)
    }

    fn expect(&mut self, s: &str) -> Result<()> {
        if self.eat(s) {
            Ok(())
        } else {
            let token = s.to_owned();
            Err(self.err(|offset| Case::ParseExpected { token, offset }))
        }
    }

    /// `f`, one level deeper; a level too many is refused at `begins`,
    /// the character where it starts.
    fn nested(&mut self, begins: usize, f: fn(&mut Self) -> Result<Expr>) -> Result<Expr> {
        if self.depth == NESTING {
            let case = Case::ExprTooDeep {
                max: NESTING,
                offset: begins,
            };
            return Err(ParseError { at: begins, case });
        }
        self.depth += 1;
        let e = f(self);
        self.depth -= 1;
        e
    }

    fn or(&mut self) -> Result<Expr> {
        let mut args = vec![self.and()?];
        while self.keyword("or") {
            args.push(self.and()?);
        }
        Ok(if args.len() == 1 {
            args.pop().expect("one operand")
        } else {
            Expr::Or { args }
        })
    }

    fn and(&mut self) -> Result<Expr> {
        let mut args = vec![self.not()?];
        while self.keyword("and") {
            args.push(self.not()?);
        }
        Ok(if args.len() == 1 {
            args.pop().expect("one operand")
        } else {
            Expr::And { args }
        })
    }

    fn not(&mut self) -> Result<Expr> {
        if self.keyword("not") {
            let begins = self.pos - "not".len();
            return self.nested(begins, |p| {
                Ok(Expr::Not {
                    arg: Box::new(p.not()?),
                })
            });
        }
        self.cmp()
    }

    fn cmp(&mut self) -> Result<Expr> {
        let lhs = self.sum()?;
        if self.keyword("in") {
            self.expect("(")?;
            let list = self.list(")")?;
            if list.is_empty() {
                return Err(self.err(|offset| Case::ParseEmptyIn { offset }));
            }
            return Ok(Expr::In {
                arg: Box::new(lhs),
                list,
            });
        }
        if self.keyword("is") {
            let negated = self.keyword("not");
            if !self.keyword("null") {
                return Err(self.err(|offset| Case::ParseIsNull { offset }));
            }
            let test = Expr::IsNull { arg: Box::new(lhs) };
            return Ok(if negated {
                Expr::Not {
                    arg: Box::new(test),
                }
            } else {
                test
            });
        }
        self.ws();
        // Longest first: `<=` before `<`.
        let ops = [
            ("!=", CmpOp::Ne),
            ("<=", CmpOp::Le),
            (">=", CmpOp::Ge),
            ("=", CmpOp::Eq),
            ("<", CmpOp::Lt),
            (">", CmpOp::Gt),
        ];
        let Some(&(_, op)) = ops.iter().find(|(s, _)| self.eat(s)) else {
            return Ok(lhs);
        };
        let rhs = self.sum()?;
        Ok(Expr::Cmp {
            cmp: op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        })
    }

    fn sum(&mut self) -> Result<Expr> {
        self.arith([("+", ArithOp::Add), ("-", ArithOp::Sub)], Self::product)
    }

    fn product(&mut self) -> Result<Expr> {
        self.arith([("*", ArithOp::Mul), ("/", ArithOp::Div)], Self::unary)
    }

    /// Operands joined by either of two operators, left-associative.
    fn arith(
        &mut self,
        ops: [(&str, ArithOp); 2],
        operand: fn(&mut Self) -> Result<Expr>,
    ) -> Result<Expr> {
        let mut lhs = operand(self)?;
        while let Some(&(_, arith)) = ops.iter().find(|(op, _)| self.eat(op)) {
            let rhs = operand(self)?;
            lhs = Expr::Arith {
                arith,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> Result<Expr> {
        if self.eat("-") {
            let begins = self.pos - 1;
            return self.nested(begins, |p| {
                Ok(Expr::Neg {
                    arg: Box::new(p.unary()?),
                })
            });
        }
        self.atom()
    }

    fn atom(&mut self) -> Result<Expr> {
        self.ws();
        match self.peek() {
            Some('(') => {
                let begins = self.pos;
                self.pos += 1;
                self.nested(begins, |p| {
                    let e = p.or()?;
                    p.expect(")")?;
                    Ok(e)
                })
            }
            Some('\'') => self.string(),
            Some(c) if c.is_ascii_digit() => self.number(),
            Some(c) if c.is_ascii_alphabetic() || c == '_' => self.word(),
            Some(found) => Err(self.err(|offset| Case::ParseUnexpected { found, offset })),
            None => Err(self.err(|offset| Case::ParseEndsEarly { offset })),
        }
    }

    fn string(&mut self) -> Result<Expr> {
        self.pos += 1;
        let mut s = String::new();
        loop {
            match self.peek() {
                None => return Err(self.err(|offset| Case::ParseUnclosed { offset })),
                Some('\'') if self.chars.get(self.pos + 1) == Some(&'\'') => {
                    s.push('\'');
                    self.pos += 2;
                }
                Some('\'') => {
                    self.pos += 1;
                    return Ok(lit(Literal::Text(s)));
                }
                Some(c) => {
                    s.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    fn number(&mut self) -> Result<Expr> {
        let start = self.pos;
        self.skip(|c| c.is_ascii_digit());
        if self.peek() == Some('.')
            && self
                .chars
                .get(self.pos + 1)
                .is_some_and(char::is_ascii_digit)
        {
            self.pos += 1;
            self.skip(|c| c.is_ascii_digit());
        }
        let raw: String = self.chars[start..self.pos].iter().collect();
        let unit = match self.peek() {
            Some('d') => Some(86_400),
            Some('h') => Some(3_600),
            Some('m') => Some(60),
            Some('s') => Some(1),
            _ => None,
        };
        let after_unit = self.chars.get(self.pos + 1);
        if let (Some(secs), false) = (
            unit,
            after_unit.is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_'),
        ) {
            self.pos += 1;
            let Some(micros) = duration_micros(&raw, secs) else {
                return Err(self.err(|offset| Case::ParseNoDuration { raw, offset }));
            };
            return Ok(lit(Literal::Duration(micros)));
        }
        if self
            .peek()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            return Err(self.err(|offset| Case::ParseNumberIntoName { offset }));
        }
        Ok(lit(Literal::Number(raw)))
    }

    fn ident(&mut self) -> String {
        let start = self.pos;
        self.skip(|c| c.is_ascii_alphanumeric() || c == '_');
        self.chars[start..self.pos].iter().collect()
    }

    fn word(&mut self) -> Result<Expr> {
        let start = self.pos;
        let first = self.ident();
        match first.as_str() {
            "true" => return Ok(lit(Literal::Bool(true))),
            "false" => return Ok(lit(Literal::Bool(false))),
            "null" => return Ok(lit(Literal::Null)),
            kw if KEYWORDS.contains(&kw) => {
                self.pos = start;
                let keyword = kw.to_owned();
                return Err(self.err(|offset| Case::ParseKeyword { keyword, offset }));
            }
            _ => {}
        }
        self.ws();
        if self.peek() == Some('(') {
            let Some(func) = func_by_name(&first) else {
                self.pos = start;
                return Err(self.err(|offset| Case::ParseNoFunction {
                    name: first,
                    offset,
                }));
            };
            self.pos += 1;
            let args = self.list(")")?;
            let (min, max) = arity(func);
            if args.len() < min || max.is_some_and(|m| args.len() > m) {
                self.pos = start;
                let (name, given) = (first, args.len());
                return Err(self.err(|offset| match max {
                    Some(takes) if takes == min => Case::ParseArityExact {
                        name,
                        takes,
                        given,
                        offset,
                    },
                    Some(max) => Case::ParseArityRange {
                        name,
                        min,
                        max,
                        given,
                        offset,
                    },
                    None => Case::ParseArityAtLeast {
                        name,
                        min,
                        given,
                        offset,
                    },
                }));
            }
            // `preceding` names a field in every argument, the others in
            // their first.
            let named = if func == Func::Preceding {
                args.len()
            } else {
                1
            };
            let path = |a: &Expr| matches!(a, Expr::Path { .. });
            if takes_field(func) && !args.iter().take(named).all(path) {
                self.pos = start;
                return Err(self.err(|offset| Case::ParseTakesField {
                    name: first,
                    offset,
                }));
            }
            if matches!(func, Func::Setting | Func::Param)
                && !matches!(
                    args.first(),
                    Some(Expr::Lit {
                        value: Literal::Text(_)
                    })
                )
            {
                self.pos = start;
                return Err(self.err(|offset| match func {
                    Func::Param => Case::ParseParamName { offset },
                    _ => Case::ParseSettingKey { offset },
                }));
            }
            return Ok(Expr::Call { func, args });
        }
        let mut path = vec![FieldName::from(first)];
        while self.peek() == Some('.') {
            self.pos += 1;
            let next = self.ident();
            if next.is_empty() {
                return Err(self.err(|offset| Case::ParseNameAfterDot { offset }));
            }
            path.push(FieldName::from(next));
        }
        Ok(Expr::Path { path })
    }

    fn list(&mut self, close: &str) -> Result<Vec<Expr>> {
        let mut out = Vec::new();
        if self.eat(close) {
            return Ok(out);
        }
        loop {
            self.ws();
            out.push(self.nested(self.pos, Self::or)?);
            if self.eat(close) {
                return Ok(out);
            }
            self.expect(",")?;
        }
    }
}

fn lit(value: Literal) -> Expr {
    Expr::Lit { value }
}

/// A duration literal in microseconds, if it fits and is a whole number of
/// microseconds.
fn duration_micros(raw: &str, unit_secs: i64) -> Option<i64> {
    let (int, frac) = raw.split_once('.').unwrap_or((raw, ""));
    if frac.len() > 12 {
        return None;
    }
    let scale = 10i128.pow(frac.len() as u32);
    let whole: i128 = format!("{int}{frac}").parse().ok()?;
    let micros = whole * i128::from(unit_secs) * 1_000_000;
    if micros % scale != 0 {
        return None;
    }
    i64::try_from(micros / scale).ok()
}

/// Print an expression in the text form.
pub fn print(e: &Expr) -> String {
    let mut s = String::new();
    write_expr(e, 0, &mut s);
    s
}

// Binding strength: higher binds tighter.
const OR: u8 = 1;
const AND: u8 = 2;
const NOT: u8 = 3;
const CMP: u8 = 4;
const SUM: u8 = 5;
const PRODUCT: u8 = 6;
const UNARY: u8 = 7;
const ATOM: u8 = 8;

fn strength(e: &Expr) -> u8 {
    match e {
        Expr::Or { args } if args.len() > 1 => OR,
        Expr::And { args } if args.len() > 1 => AND,
        Expr::Not { arg } if matches!(**arg, Expr::IsNull { .. }) => CMP,
        Expr::Not { .. } => NOT,
        Expr::Cmp { .. } | Expr::In { .. } | Expr::IsNull { .. } => CMP,
        Expr::Arith {
            arith: ArithOp::Add | ArithOp::Sub,
            ..
        } => SUM,
        Expr::Arith { .. } => PRODUCT,
        Expr::Neg { .. } => UNARY,
        _ => ATOM,
    }
}

fn write_expr(e: &Expr, min: u8, out: &mut String) {
    let own = strength(e);
    let paren = own < min;
    if paren {
        out.push('(');
    }
    match e {
        Expr::Lit { value } => write_literal(value, out),
        Expr::Path { path } => {
            let names: Vec<&str> = path.iter().map(FieldName::as_str).collect();
            out.push_str(&names.join("."));
        }
        Expr::Cmp { cmp, lhs, rhs } => {
            write_expr(lhs, SUM, out);
            out.push_str(match cmp {
                CmpOp::Eq => " = ",
                CmpOp::Ne => " != ",
                CmpOp::Lt => " < ",
                CmpOp::Le => " <= ",
                CmpOp::Gt => " > ",
                CmpOp::Ge => " >= ",
            });
            write_expr(rhs, SUM, out);
        }
        Expr::Arith { arith, lhs, rhs } => {
            let (sym, level) = match arith {
                ArithOp::Add => (" + ", SUM),
                ArithOp::Sub => (" - ", SUM),
                ArithOp::Mul => (" * ", PRODUCT),
                ArithOp::Div => (" / ", PRODUCT),
            };
            // Left-associative: the right operand needs parentheses at the
            // same level, the left one does not.
            write_expr(lhs, level, out);
            out.push_str(sym);
            write_expr(rhs, level + 1, out);
        }
        Expr::Neg { arg } => {
            out.push('-');
            write_expr(arg, UNARY, out);
        }
        Expr::And { args } | Expr::Or { args } if args.is_empty() => {
            // An empty `and` holds; an empty `or` does not.
            out.push_str(if matches!(e, Expr::And { .. }) {
                "true"
            } else {
                "false"
            });
        }
        Expr::And { args } | Expr::Or { args } if args.len() == 1 => write_expr(&args[0], min, out),
        Expr::And { args } => join(args, " and ", AND + 1, out),
        Expr::Or { args } => join(args, " or ", OR + 1, out),
        Expr::Not { arg } => match &**arg {
            Expr::IsNull { arg } => {
                write_expr(arg, SUM, out);
                out.push_str(" is not null");
            }
            arg => {
                out.push_str("not ");
                write_expr(arg, NOT, out);
            }
        },
        Expr::In { arg, list } => {
            write_expr(arg, SUM, out);
            out.push_str(" in (");
            join(list, ", ", OR, out);
            out.push(')');
        }
        Expr::IsNull { arg } => {
            write_expr(arg, SUM, out);
            out.push_str(" is null");
        }
        Expr::Call { func, args } => {
            out.push_str(func_name(*func));
            out.push('(');
            join(args, ", ", OR, out);
            out.push(')');
        }
    }
    if paren {
        out.push(')');
    }
}

fn join(args: &[Expr], sep: &str, level: u8, out: &mut String) {
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            out.push_str(sep);
        }
        write_expr(a, level, out);
    }
}

fn write_literal(l: &Literal, out: &mut String) {
    match l {
        Literal::Null => out.push_str("null"),
        Literal::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Literal::Number(n) => out.push_str(n),
        Literal::Text(t) => {
            out.push('\'');
            out.push_str(&t.replace('\'', "''"));
            out.push('\'');
        }
        Literal::Duration(micros) => {
            for (unit, secs) in [('d', 86_400i64), ('h', 3_600), ('m', 60)] {
                let per = secs * 1_000_000;
                if *micros % per == 0 {
                    out.push_str(&format!("{}{unit}", micros / per));
                    return;
                }
            }
            let (whole, frac) = (micros / 1_000_000, (micros % 1_000_000).abs());
            if frac == 0 {
                out.push_str(&format!("{whole}s"));
            } else {
                let frac = format!("{frac:06}");
                out.push_str(&format!("{whole}.{}s", frac.trim_end_matches('0')));
            }
        }
    }
}

#[cfg(test)]
mod tests;
