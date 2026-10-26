//! An expression in openCypher's spelling, parsed straight into the one
//! expression grammar's typed [`Expr`], never into its text.
//!
//! The precedence is openCypher's, loosest first: `OR`, `XOR`, `AND`,
//! `NOT`, a comparison or a predicate (`=`, `<>`, `<`, `<=`, `>`, `>=`,
//! `IN`, `IS [NOT] NULL`, `STARTS WITH`, `CONTAINS`, `=~`), `+ -`,
//! `* /`, unary `-`. Of these, `XOR`, `%`, `^` and `ENDS WITH` have no
//! counterpart in the grammar and are refused. A parameter becomes a
//! literal where it stands, so no value can change what the statement
//! says.

use lakelands_api::FieldName;
use lakelands_api::Value;
use lakelands_api::error::{Case, Construct};
use lakelands_api::expr::{ArithOp, CmpOp, Expr, Func, Literal, arity, func_by_name, func_name};

use crate::lex::Tok;
use crate::parse::{Absent, Name, Parser, Result, absent_marker, outside, reserved};

fn boxed(e: Expr) -> Box<Expr> {
    Box::new(e)
}

fn lit(value: Literal) -> Expr {
    Expr::Lit { value }
}

/// One operand alone, or the `and` or `or` of several.
fn joined(mut args: Vec<Expr>, or: bool) -> Expr {
    match (args.len(), or) {
        (1, _) => args.pop().expect("one operand"),
        (_, true) => Expr::Or { args },
        (_, false) => Expr::And { args },
    }
}

/// How deep a condition nests: every parenthesis, call, list and
/// condition enters at `or`, and `NOT` and a unary `-` go one deeper
/// each. A deeper statement is refused, so no statement runs the parser
/// out of stack and every pattern stays within what a JSON reader takes.
const NESTING: usize = 32;

impl Parser<'_> {
    /// `f`, one level deeper.
    fn nested(&mut self, f: fn(&mut Self) -> Result<Expr>) -> Result<Expr> {
        if self.depth == NESTING {
            let (max, offset) = (NESTING, self.at());
            return Err(Case::CypherTooDeep { max, offset });
        }
        self.depth += 1;
        let e = f(self);
        self.depth -= 1;
        e
    }

    pub fn or(&mut self) -> Result<Expr> {
        self.nested(|p| {
            let mut args = vec![p.xor()?];
            while p.kw("OR") {
                args.push(p.xor()?);
            }
            Ok(joined(args, true))
        })
    }

    fn xor(&mut self) -> Result<Expr> {
        let e = self.and()?;
        if self.is_kw("XOR") {
            return Err(outside(Construct::Xor, self.at()));
        }
        Ok(e)
    }

    fn and(&mut self) -> Result<Expr> {
        let mut args = vec![self.not()?];
        while self.kw("AND") {
            args.push(self.not()?);
        }
        Ok(joined(args, false))
    }

    fn not(&mut self) -> Result<Expr> {
        if !self.is_kw("NOT") {
            return self.cmp();
        }
        let at = self.at();
        self.bump();
        let exists = self.is_kw("EXISTS") && matches!(self.ahead(1), Tok::Punct("{"));
        if exists || (self.is("(") && self.pattern_ahead()) {
            return self.absent(at, exists);
        }
        Ok(Expr::Not {
            arg: boxed(self.nested(Self::not)?),
        })
    }

    /// A part that must not match, after its `NOT`: `EXISTS { … }` or a
    /// bare part. It stands in the condition as its marker.
    fn absent(&mut self, at: usize, braced: bool) -> Result<Expr> {
        let names = std::mem::take(&mut self.names);
        let absent = std::mem::take(&mut self.absent);
        let found = self.absent_part(braced);
        self.names = names;
        self.absent = absent;
        let (part, filter) = found?;
        self.absent.push(Absent { at, part, filter });
        Ok(absent_marker(self.absent.len() - 1))
    }

    fn absent_part(
        &mut self,
        braced: bool,
    ) -> Result<(crate::parse::Part, Option<crate::parse::Where>)> {
        if !braced {
            return Ok((self.part()?, None));
        }
        self.bump();
        self.bump();
        self.kw("MATCH");
        let part = self.part()?;
        let filter = if self.kw("WHERE") {
            let w = self.condition()?;
            if let Some(a) = w.absent.first() {
                return Err(outside(Construct::Exists, a.at));
            }
            Some(w)
        } else {
            None
        };
        self.expect("}")?;
        Ok((part, filter))
    }

    /// Whether the `(` here begins a part rather than a parenthesis.
    fn pattern_ahead(&self) -> bool {
        let var =
            |t: &Tok| matches!(t, Tok::Word(w) if !reserved(w)) || matches!(t, Tok::Quoted(_));
        match (self.ahead(1), self.ahead(2)) {
            (Tok::Punct(")" | ":"), _) => true,
            (v, Tok::Punct(":" | "{")) if var(v) => true,
            (v, Tok::Punct(")")) if var(v) => matches!(
                (self.ahead(3), self.ahead(4)),
                (Tok::Punct("-"), Tok::Punct("[" | "-")) | (Tok::Punct("<"), Tok::Punct("-"))
            ),
            _ => false,
        }
    }

    fn cmp(&mut self) -> Result<Expr> {
        let lhs = self.sum()?;
        if self.kw("IS") {
            let negated = self.kw("NOT");
            if !self.kw("NULL") {
                return Err(Case::ParseIsNull { offset: self.at() });
            }
            let test = Expr::IsNull { arg: boxed(lhs) };
            return Ok(match negated {
                true => Expr::Not { arg: boxed(test) },
                false => test,
            });
        }
        if self.kw("IN") {
            let list = self.in_list()?;
            return Ok(Expr::In {
                arg: boxed(lhs),
                list,
            });
        }
        if self.is_kw("ENDS") {
            return Err(outside(Construct::EndsWith, self.at()));
        }
        let func = if self.kw("STARTS") {
            self.expect_kw("WITH")?;
            Some(Func::StartsWith)
        } else if self.kw("CONTAINS") {
            Some(Func::Contains)
        } else if self.eat("=~") {
            Some(Func::Matches)
        } else {
            None
        };
        if let Some(func) = func {
            let rhs = self.sum()?;
            return Ok(Expr::Call {
                func,
                args: vec![lhs, rhs],
            });
        }
        let ops = [
            ("=", CmpOp::Eq),
            ("<>", CmpOp::Ne),
            ("<", CmpOp::Lt),
            ("<=", CmpOp::Le),
            (">", CmpOp::Gt),
            (">=", CmpOp::Ge),
        ];
        let Some(&(_, cmp)) = ops.iter().find(|(p, _)| self.is(p)) else {
            return Ok(lhs);
        };
        self.bump();
        let rhs = self.sum()?;
        Ok(Expr::Cmp {
            cmp,
            lhs: boxed(lhs),
            rhs: boxed(rhs),
        })
    }

    /// The list after `IN`: written in brackets, or a parameter that
    /// holds one.
    fn in_list(&mut self) -> Result<Vec<Expr>> {
        if let Tok::Param(p) = self.tok().clone() {
            let name = lakelands_api::ParamName::from(p);
            let Value::Array(values) = self.param(&name)? else {
                return Err(Case::CypherParamValue { name });
            };
            self.bump();
            let each = values.into_iter().map(|v| scalar(&name, v).map(lit));
            return each.collect();
        }
        if !self.is("[") {
            return Err(self.expected("["));
        }
        if matches!(self.ahead(1), Tok::Word(_)) && self.is_kw_at(2, "IN") {
            return Err(outside(Construct::ListComprehension, self.at()));
        }
        self.bump();
        let mut list = Vec::new();
        if self.eat("]") {
            return Ok(list);
        }
        loop {
            list.push(self.or()?);
            if self.eat("]") {
                return Ok(list);
            }
            self.expect(",")?;
        }
    }

    fn sum(&mut self) -> Result<Expr> {
        self.arith([("+", ArithOp::Add), ("-", ArithOp::Sub)], Self::product)
    }

    fn product(&mut self) -> Result<Expr> {
        self.arith([("*", ArithOp::Mul), ("/", ArithOp::Div)], Self::power)
    }

    fn arith(
        &mut self,
        ops: [(&str, ArithOp); 2],
        operand: fn(&mut Self) -> Result<Expr>,
    ) -> Result<Expr> {
        let mut lhs = operand(self)?;
        loop {
            if self.is("%") {
                return Err(outside(Construct::Modulo, self.at()));
            }
            let Some(&(_, arith)) = ops.iter().find(|(p, _)| self.is(p)) else {
                return Ok(lhs);
            };
            self.bump();
            let rhs = operand(self)?;
            lhs = Expr::Arith {
                arith,
                lhs: boxed(lhs),
                rhs: boxed(rhs),
            };
        }
    }

    fn power(&mut self) -> Result<Expr> {
        let e = self.unary()?;
        if self.is("^") {
            return Err(outside(Construct::Power, self.at()));
        }
        Ok(e)
    }

    fn unary(&mut self) -> Result<Expr> {
        if self.eat("-") {
            return Ok(Expr::Neg {
                arg: boxed(self.nested(Self::unary)?),
            });
        }
        self.eat("+");
        self.atom()
    }

    fn atom(&mut self) -> Result<Expr> {
        let at = self.at();
        match self.tok().clone() {
            Tok::Num(n) => {
                self.bump();
                Ok(lit(Literal::Number(n)))
            }
            Tok::Str(s) => {
                self.bump();
                Ok(lit(Literal::Text(s)))
            }
            Tok::Param(p) => {
                let name = lakelands_api::ParamName::from(p);
                let value = self.param(&name)?;
                self.bump();
                Ok(lit(scalar(&name, value)?))
            }
            Tok::Punct("(") if self.pattern_ahead() => Err(outside(Construct::Exists, at)),
            Tok::Punct("(") => {
                self.bump();
                let e = self.or()?;
                self.expect(")")?;
                Ok(e)
            }
            Tok::Punct("[") => Err(outside(
                match (self.ahead(1), self.ahead(2)) {
                    (Tok::Word(_), _) if self.is_kw_at(2, "IN") => Construct::ListComprehension,
                    (Tok::Punct("("), _) => Construct::PatternComprehension,
                    _ => Construct::ListLiteral,
                },
                at,
            )),
            Tok::Punct("{") => Err(outside(Construct::MapLiteral, at)),
            Tok::Word(w) => self.word(w, at),
            Tok::Quoted(_) => self.path(),
            Tok::End => Err(Case::ParseEndsEarly { offset: at }),
            Tok::Punct(p) => {
                let found = p.chars().next().unwrap_or(' ');
                Err(Case::ParseUnexpected { found, offset: at })
            }
        }
    }

    fn word(&mut self, w: String, at: usize) -> Result<Expr> {
        let value = match w.to_ascii_lowercase().as_str() {
            "true" => Some(Literal::Bool(true)),
            "false" => Some(Literal::Bool(false)),
            "null" => Some(Literal::Null),
            _ => None,
        };
        if let Some(value) = value {
            self.bump();
            return Ok(lit(value));
        }
        let next = self.ahead(1).clone();
        let construct = match w.to_ascii_uppercase().as_str() {
            "CASE" => Some(Construct::Case),
            "EXISTS" => Some(Construct::Exists),
            "COUNT" if next == Tok::Punct("{") => Some(Construct::CountSubquery),
            _ => None,
        };
        if let Some(c) = construct {
            return Err(outside(c, at));
        }
        if next == Tok::Punct("(") {
            return self.call(w, at);
        }
        if reserved(&w) {
            let keyword = w.to_ascii_uppercase();
            return Err(Case::ParseKeyword {
                keyword,
                offset: at,
            });
        }
        if next == Tok::Punct("{") {
            return Err(outside(Construct::MapProjection, self.toks[self.i + 1].at));
        }
        // A dotted name before `(` is a namespaced function, as
        // `apoc.coll.sum(…)`: none is in the grammar.
        let mut k = 1;
        while matches!(self.ahead(k), Tok::Punct(".")) && matches!(self.ahead(k + 1), Tok::Word(_))
        {
            k += 2;
        }
        if k > 1 && matches!(self.ahead(k), Tok::Punct("(")) {
            let name = (self.toks[self.i..self.i + k].iter())
                .map(|t| match &t.tok {
                    Tok::Word(w) => w.as_str(),
                    _ => ".",
                })
                .collect();
            return Err(Case::CypherFunction { name, offset: at });
        }
        self.path()
    }

    /// A variable and the properties read through it.
    fn path(&mut self) -> Result<Expr> {
        let at = self.at();
        let var = self.name(false)?;
        self.names.push(Name {
            name: var.clone(),
            at,
        });
        let mut path = vec![FieldName::from(var)];
        while self.eat(".") {
            path.push(FieldName::from(self.name(true)?));
        }
        Ok(Expr::Path { path })
    }

    /// A call of `name`, at its `(`: a function of the grammar, by its
    /// name (`func_name`) in any case, or `duration` of a fixed length.
    fn call(&mut self, name: String, at: usize) -> Result<Expr> {
        if name.eq_ignore_ascii_case("duration") {
            return self.duration(at);
        }
        let lower = name.to_ascii_lowercase();
        let Some(func) = func_by_name(&lower) else {
            if lower == "shortestpath" || lower == "allshortestpaths" {
                return Err(outside(Construct::ShortestPath, at));
            }
            return Err(Case::CypherFunction { name, offset: at });
        };
        self.bump();
        self.bump();
        if self.is_kw("DISTINCT") {
            return Err(outside(Construct::Distinct, self.at()));
        }
        let mut args = Vec::new();
        if func == Func::Count && self.is("*") {
            self.bump();
            self.expect(")")?;
        } else if !self.eat(")") {
            loop {
                args.push(self.or()?);
                if self.eat(")") {
                    break;
                }
                self.expect(",")?;
            }
        }
        let (min, max) = arity(func);
        let given = args.len();
        if given < min || max.is_some_and(|m| given > m) {
            let (name, offset) = (func_name(func).to_owned(), at);
            return Err(match max {
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
            });
        }
        if func.is_aggregate() {
            self.aggregates.push(at);
        }
        Ok(Expr::Call { func, args })
    }

    /// `duration('P3DT4H')`: a duration literal, where its length is
    /// fixed. Months and years are not, and are refused.
    fn duration(&mut self, at: usize) -> Result<Expr> {
        self.bump();
        self.bump();
        let raw = match self.tok().clone() {
            Tok::Str(s) => s,
            Tok::Param(p) => {
                let name = lakelands_api::ParamName::from(p);
                match self.param(&name)? {
                    Value::String(s) => s,
                    _ => return Err(Case::CypherParamValue { name }),
                }
            }
            _ => return Err(self.expected("'")),
        };
        self.bump();
        self.expect(")")?;
        match iso_micros(&raw) {
            Ok(micros) => Ok(lit(Literal::Duration(micros))),
            Err(true) => Err(outside(Construct::CalendarDuration, at)),
            Err(false) => Err(Case::ParseNoDuration { raw, offset: at }),
        }
    }
}

/// A parameter's value where one value stands.
fn scalar(name: &lakelands_api::ParamName, v: Value) -> Result<Literal> {
    Ok(match v {
        Value::Null => Literal::Null,
        Value::Bool(b) => Literal::Bool(b),
        Value::Number(n) => Literal::Number(n.to_string()),
        Value::String(s) => Literal::Text(s),
        Value::Array(_) | Value::Object(_) => {
            let name = name.clone();
            return Err(Case::CypherParamValue { name });
        }
    })
}

/// An ISO 8601 duration of weeks, days, hours, minutes and seconds in
/// microseconds; `Err(true)` where it names months or years, which have
/// no fixed length, `Err(false)` where it is no duration or does not fit.
fn iso_micros(raw: &str) -> std::result::Result<i64, bool> {
    let upper = raw.to_ascii_uppercase();
    let rest = upper.strip_prefix('P').ok_or(false)?;
    let (date, time) = rest.split_once('T').unwrap_or((rest, ""));
    if (rest.contains('T') && time.is_empty()) || rest.is_empty() {
        return Err(false);
    }
    let mut total: i128 = 0;
    let units: [(&str, &[(char, i128)]); 2] = [
        (date, &[('Y', -1), ('M', -1), ('W', 604_800), ('D', 86_400)]),
        (time, &[('H', 3_600), ('M', 60), ('S', 1)]),
    ];
    for (part, allowed) in units {
        let mut number = String::new();
        let mut next = 0;
        for c in part.chars() {
            if c.is_ascii_digit() || c == '.' {
                number.push(c);
                continue;
            }
            let k = (allowed[next..].iter().position(|(u, _)| *u == c)).ok_or(false)?;
            let secs = allowed[next + k].1;
            next += k + 1;
            if secs < 0 {
                return Err(true);
            }
            total += scaled(&number, secs).ok_or(false)?;
            number.clear();
        }
        if !number.is_empty() {
            return Err(false);
        }
    }
    i64::try_from(total).map_err(|_| false)
}

/// `number` seconds of `secs` each, in whole microseconds.
fn scaled(number: &str, secs: i128) -> Option<i128> {
    let (int, frac) = number.split_once('.').unwrap_or((number, ""));
    if int.is_empty() || frac.len() > 12 || frac.contains('.') {
        return None;
    }
    let digits: i128 = [int, frac].concat().parse().ok()?;
    let micros = digits.checked_mul(secs * 1_000_000)?;
    let scale = 10i128.pow(frac.len() as u32);
    (micros % scale == 0).then_some(micros / scale)
}
