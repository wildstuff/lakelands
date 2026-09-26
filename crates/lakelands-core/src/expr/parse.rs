//! The text form of the one expression grammar: parsing into
//! [`Expr`] and printing back.
//!
//! A recursive-descent parser over the grammar in `lakelands_api::expr`,
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

use lakelands_api::FieldName;
use lakelands_api::expr::{ArithOp, CmpOp, Expr, Func, Literal};

/// Why a text expression does not parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// The character offset where parsing stopped.
    pub at: usize,
    /// What was wrong, for a person.
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at character {})", self.message, self.at)
    }
}

impl std::error::Error for ParseError {}

type Result<T> = std::result::Result<T, ParseError>;

/// Parse an expression in the text form.
pub fn parse(text: &str) -> Result<Expr> {
    let mut p = Parser {
        chars: text.chars().collect(),
        pos: 0,
    };
    let e = p.or()?;
    p.ws();
    if p.pos != p.chars.len() {
        return Err(p.err("unexpected input after the end of the expression"));
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
        Func::Setting => "setting",
        Func::Changed => "changed",
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
        Func::DateBucket => "date_bucket",
        Func::Count => "count",
        Func::Sum => "sum",
        Func::Min => "min",
        Func::Max => "max",
        Func::Avg => "avg",
    }
}

const FUNCS: [Func; 27] = [
    Func::Now,
    Func::Today,
    Func::KnownAt,
    Func::SeenAt,
    Func::Previous,
    Func::Setting,
    Func::Changed,
    Func::Coalesce,
    Func::If,
    Func::Lower,
    Func::Upper,
    Func::Trim,
    Func::Len,
    Func::Concat,
    Func::Substr,
    Func::Contains,
    Func::Replace,
    Func::StartsWith,
    Func::Matches,
    Func::Abs,
    Func::Round,
    Func::DateBucket,
    Func::Count,
    Func::Sum,
    Func::Min,
    Func::Max,
    Func::Avg,
];

fn func_by_name(name: &str) -> Option<Func> {
    FUNCS.into_iter().find(|f| func_name(*f) == name)
}

/// How many arguments a function takes: `(min, max)`, `max` unbounded when
/// `None`.
pub fn arity(f: Func) -> (usize, Option<usize>) {
    match f {
        Func::Now | Func::Today => (0, Some(0)),
        Func::KnownAt
        | Func::SeenAt
        | Func::Previous
        | Func::Setting
        | Func::Changed
        | Func::Lower
        | Func::Upper
        | Func::Trim
        | Func::Len
        | Func::Abs
        | Func::Sum
        | Func::Min
        | Func::Max
        | Func::Avg => (1, Some(1)),
        Func::Count => (0, Some(1)),
        Func::Round => (1, Some(2)),
        Func::Contains | Func::StartsWith | Func::Matches | Func::DateBucket => (2, Some(2)),
        Func::Substr => (2, Some(3)),
        Func::If | Func::Replace => (3, Some(3)),
        Func::Coalesce | Func::Concat => (1, None),
    }
}

/// Whether a function's argument must name a field rather than compute a
/// value (`known_at(amount)`, not `known_at(amount + 1)`).
fn takes_field(f: Func) -> bool {
    matches!(
        f,
        Func::KnownAt | Func::SeenAt | Func::Previous | Func::Changed
    )
}

const KEYWORDS: [&str; 8] = ["and", "or", "not", "in", "is", "null", "true", "false"];

struct Parser {
    chars: Vec<char>,
    pos: usize,
}

impl Parser {
    fn err(&self, message: impl Into<String>) -> ParseError {
        ParseError {
            at: self.pos,
            message: message.into(),
        }
    }

    fn ws(&mut self) {
        while self.chars.get(self.pos).is_some_and(|c| c.is_whitespace()) {
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
        let n = kw.chars().count();
        let matches = self.chars[self.pos..]
            .iter()
            .take(n)
            .copied()
            .eq(kw.chars());
        let boundary = !self
            .chars
            .get(self.pos + n)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_');
        if matches && boundary {
            self.pos += n;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, s: &str) -> Result<()> {
        if self.eat(s) {
            Ok(())
        } else {
            Err(self.err(format!("expected `{s}`")))
        }
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
            return Ok(Expr::Not {
                arg: Box::new(self.not()?),
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
                return Err(self.err("`in` needs at least one value"));
            }
            return Ok(Expr::In {
                arg: Box::new(lhs),
                list,
            });
        }
        if self.keyword("is") {
            let negated = self.keyword("not");
            if !self.keyword("null") {
                return Err(self.err("expected `null` after `is`"));
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
        let op = if self.eat("!=") {
            CmpOp::Ne
        } else if self.eat("<=") {
            CmpOp::Le
        } else if self.eat(">=") {
            CmpOp::Ge
        } else if self.eat("=") {
            CmpOp::Eq
        } else if self.eat("<") {
            CmpOp::Lt
        } else if self.eat(">") {
            CmpOp::Gt
        } else {
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
        let mut lhs = self.product()?;
        loop {
            let arith = if self.eat("+") {
                ArithOp::Add
            } else if self.eat("-") {
                ArithOp::Sub
            } else {
                return Ok(lhs);
            };
            let rhs = self.product()?;
            lhs = Expr::Arith {
                arith,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
    }

    fn product(&mut self) -> Result<Expr> {
        let mut lhs = self.unary()?;
        loop {
            let arith = if self.eat("*") {
                ArithOp::Mul
            } else if self.eat("/") {
                ArithOp::Div
            } else {
                return Ok(lhs);
            };
            let rhs = self.unary()?;
            lhs = Expr::Arith {
                arith,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
    }

    fn unary(&mut self) -> Result<Expr> {
        if self.eat("-") {
            return Ok(Expr::Neg {
                arg: Box::new(self.unary()?),
            });
        }
        self.atom()
    }

    fn atom(&mut self) -> Result<Expr> {
        self.ws();
        match self.peek() {
            Some('(') => {
                self.pos += 1;
                let e = self.or()?;
                self.expect(")")?;
                Ok(e)
            }
            Some('\'') => self.string(),
            Some(c) if c.is_ascii_digit() => self.number(),
            Some(c) if c.is_ascii_alphabetic() || c == '_' => self.word(),
            Some(c) => Err(self.err(format!("unexpected `{c}`"))),
            None => Err(self.err("the expression ends too early")),
        }
    }

    fn string(&mut self) -> Result<Expr> {
        self.pos += 1;
        let mut s = String::new();
        loop {
            match self.peek() {
                None => return Err(self.err("a string is not closed with `'`")),
                Some('\'') if self.chars.get(self.pos + 1) == Some(&'\'') => {
                    s.push('\'');
                    self.pos += 2;
                }
                Some('\'') => {
                    self.pos += 1;
                    return Ok(Expr::Lit {
                        value: Literal::Text(s),
                    });
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
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.peek() == Some('.')
            && self
                .chars
                .get(self.pos + 1)
                .is_some_and(char::is_ascii_digit)
        {
            self.pos += 1;
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.pos += 1;
            }
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
            let micros = duration_micros(&raw, secs).ok_or_else(|| {
                self.err(format!("`{raw}` is not a duration this grammar can hold"))
            })?;
            return Ok(Expr::Lit {
                value: Literal::Duration(micros),
            });
        }
        if self
            .peek()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        {
            return Err(self.err("a number runs into a name"));
        }
        Ok(Expr::Lit {
            value: Literal::Number(raw),
        })
    }

    fn ident(&mut self) -> String {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            self.pos += 1;
        }
        self.chars[start..self.pos].iter().collect()
    }

    fn word(&mut self) -> Result<Expr> {
        let start = self.pos;
        let first = self.ident();
        match first.as_str() {
            "true" => {
                return Ok(Expr::Lit {
                    value: Literal::Bool(true),
                });
            }
            "false" => {
                return Ok(Expr::Lit {
                    value: Literal::Bool(false),
                });
            }
            "null" => {
                return Ok(Expr::Lit {
                    value: Literal::Null,
                });
            }
            kw if KEYWORDS.contains(&kw) => {
                self.pos = start;
                return Err(self.err(format!("`{kw}` cannot start a value")));
            }
            _ => {}
        }
        self.ws();
        if self.peek() == Some('(') {
            let Some(func) = func_by_name(&first) else {
                self.pos = start;
                return Err(self.err(format!("there is no function `{first}`")));
            };
            self.pos += 1;
            let args = self.list(")")?;
            let (min, max) = arity(func);
            if args.len() < min || max.is_some_and(|m| args.len() > m) {
                self.pos = start;
                let want = match max {
                    Some(m) if m == min => format!("{min}"),
                    Some(m) => format!("{min} to {m}"),
                    None => format!("at least {min}"),
                };
                return Err(self.err(format!(
                    "`{first}` takes {want} argument(s), not {}",
                    args.len()
                )));
            }
            if takes_field(func) && !matches!(args.first(), Some(Expr::Path { .. })) {
                self.pos = start;
                return Err(self.err(format!("`{first}` takes a field name")));
            }
            if func == Func::Setting
                && !matches!(
                    args.first(),
                    Some(Expr::Lit {
                        value: Literal::Text(_)
                    })
                )
            {
                self.pos = start;
                return Err(self.err("`setting` takes a quoted key, as in setting('vat_rate')"));
            }
            return Ok(Expr::Call { func, args });
        }
        let mut path = vec![FieldName::from(first)];
        while self.peek() == Some('.') {
            self.pos += 1;
            let next = self.ident();
            if next.is_empty() {
                return Err(self.err("expected a name after `.`"));
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
            out.push(self.or()?);
            if self.eat(close) {
                return Ok(out);
            }
            self.expect(",")?;
        }
    }
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
mod tests {
    use super::*;

    fn path(p: &str) -> Expr {
        Expr::Path {
            path: p.split('.').map(FieldName::from).collect(),
        }
    }

    fn num(n: &str) -> Expr {
        Expr::Lit {
            value: Literal::Number(n.into()),
        }
    }

    #[test]
    fn precedence_and_associativity() {
        assert_eq!(
            parse("a - b - c").unwrap(),
            Expr::Arith {
                arith: ArithOp::Sub,
                lhs: Box::new(Expr::Arith {
                    arith: ArithOp::Sub,
                    lhs: Box::new(path("a")),
                    rhs: Box::new(path("b")),
                }),
                rhs: Box::new(path("c")),
            }
        );
        let e = parse("a = 1 or b = 2 and not c = 3").unwrap();
        let Expr::Or { args } = e else {
            panic!("or is loosest")
        };
        assert!(matches!(&args[1], Expr::And { args } if matches!(args[1], Expr::Not { .. })));
    }

    #[test]
    fn chains_flatten() {
        let Expr::And { args } = parse("a and b and c").unwrap() else {
            panic!()
        };
        assert_eq!(args.len(), 3);
    }

    #[test]
    fn literals() {
        assert_eq!(
            parse("'it''s'").unwrap(),
            Expr::Lit {
                value: Literal::Text("it's".into())
            }
        );
        assert_eq!(parse("12.50").unwrap(), num("12.50"));
        assert_eq!(
            parse("30d").unwrap(),
            Expr::Lit {
                value: Literal::Duration(30 * 86_400_000_000)
            }
        );
        assert_eq!(
            parse("1.5s").unwrap(),
            Expr::Lit {
                value: Literal::Duration(1_500_000)
            }
        );
        assert_eq!(
            parse("null").unwrap(),
            Expr::Lit {
                value: Literal::Null
            }
        );
        assert_eq!(
            parse("true").unwrap(),
            Expr::Lit {
                value: Literal::Bool(true)
            }
        );
    }

    #[test]
    fn in_and_null_tests() {
        assert!(
            matches!(parse("status in ('open', 'late')").unwrap(), Expr::In { list, .. } if list.len() == 2)
        );
        assert!(matches!(
            parse("note is null").unwrap(),
            Expr::IsNull { .. }
        ));
        assert!(
            matches!(parse("note is not null").unwrap(), Expr::Not { arg } if matches!(*arg, Expr::IsNull { .. }))
        );
    }

    #[test]
    fn calls_check_their_names_and_arity() {
        assert!(matches!(
            parse("due < today() - 30d").unwrap(),
            Expr::Cmp { .. }
        ));
        assert!(parse("known_at(amount) > '2026-01-01'").is_ok());
        assert!(parse("setting('vat_rate') * amount").is_ok());
        assert!(parse("changed(status) and previous(status) = 'open'").is_ok());
        for bad in [
            "bogus(x)",
            "upper(a, b)",
            "if(a, b)",
            "today(1)",
            "known_at(a + 1)",
            "setting(vat_rate)",
        ] {
            assert!(parse(bad).is_err(), "{bad} parsed");
        }
    }

    #[test]
    fn malformed_input_is_refused_where_it_goes_wrong() {
        for (bad, at) in [
            ("1 +", 3),
            ("(1 + 2", 6),
            ("a = ", 4),
            ("'open", 5),
            ("a b", 2),
            ("12x", 2),
            ("and", 0),
        ] {
            let e = parse(bad).unwrap_err();
            assert_eq!(e.at, at, "{bad}: {e}");
        }
        assert!(parse("status in ()").is_err());
    }

    #[test]
    fn keywords_do_not_swallow_longer_names() {
        assert_eq!(parse("android").unwrap(), path("android"));
        assert_eq!(parse("order.notes").unwrap(), path("order.notes"));
        assert!(matches!(parse("island = 1").unwrap(), Expr::Cmp { .. }));
    }

    #[test]
    fn printing_round_trips() {
        for text in [
            "a - (b - c)",
            "(a + b) * c",
            "-(a + b)",
            "not (a = 1 or b = 2)",
            "status = 'open' and amount > 100.00",
            "due_date < today() - 30d",
            "note is not null and tag in ('x', 'y''s')",
            "if(amount > 100, 'large', 'small')",
            "known_at(amount) >= '2026-09-01'",
            "a or b and c",
            "1.5s + 2h",
            "invoice.amount / partner.limit",
        ] {
            let e = parse(text).unwrap();
            assert_eq!(print(&e), text, "printed form of {text}");
            assert_eq!(parse(&print(&e)).unwrap(), e);
        }
    }

    #[test]
    fn printing_adds_only_needed_parentheses() {
        assert_eq!(print(&parse("((a))").unwrap()), "a");
        assert_eq!(print(&parse("(a * b) + c").unwrap()), "a * b + c");
        assert_eq!(
            print(&parse("a and (b and c)").unwrap()),
            "a and b and c".replace("b and c", "(b and c)")
        );
    }
}
