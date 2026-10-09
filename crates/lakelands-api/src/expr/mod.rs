//! The one expression grammar.
//!
//! Filters (`where` on `find`, on a pattern step and on an object set),
//! trigger guards, derived fields, value-type constraints and view
//! measures are all written in this one language. A caller learns
//! `where` once. [`Expr`] is the typed core; the text form below is
//! parsed into it and printed from it, and nothing else is ever
//! evaluated.
//!
//! # The text form
//!
//! ```text
//! expr    := or
//! or      := and ('or' and)*
//! and     := not ('and' not)*
//! not     := 'not' not | cmp
//! cmp     := sum (cmpop sum | 'in' '(' list ')' | 'is' 'not'? 'null')?
//! cmpop   := '=' | '!=' | '<' | '<=' | '>' | '>='
//! sum     := product (('+' | '-') product)*
//! product := unary (('*' | '/') unary)*
//! unary   := '-' unary | atom
//! atom    := literal | path | call | '(' expr ')'
//! call    := ident '(' list? ')'
//! list    := expr (',' expr)*
//! path    := ident ('.' ident)*
//! literal := number | 'string' | 'true' | 'false' | 'null' | duration
//! duration:= number ('d' | 'h' | 'm' | 's')
//! ```
//!
//! Keywords are lower case. An identifier is ASCII: a letter or `_`,
//! then letters, digits or `_` — the same rule every declared name
//! follows, so every field can be named in an expression.
//!
//! **Paths.** A path's first segment is looked up among the fields of the
//! record in scope first. If it is one, a further segment digs into that
//! field's value — meaningful for a `json` or `geo` value, null for any
//! other. If it is not, in a pattern step the first segment names the type
//! of an earlier step and the second a field of it (`invoice.amount`). A
//! declaration whose `json` field has the name of a type is refused, so
//! the two readings never meet.
//!
//! **Literals and calls.** A duration is a number directly followed by
//! `d` (days), `h` (hours), `m` (minutes) or `s` (seconds), possibly with
//! a fraction (`1.5h`). `substr` takes two or three arguments (text, start,
//! optional length; positions count characters from 0); `round` takes one
//! or two (value, optional number of fractional digits); `coalesce` and
//! `concat` take one or more; every other function takes exactly the
//! arguments its documentation names. Calls name a [`Func`]; `known_at(f)` and `seen_at(f)`
//! read a field's provenance (`docs/decisions/010-provenance-per-field.md`),
//! `changed(f)` is true in a guard when the write changes `f`, and
//! `previous(f)` is `f`'s value before that write. `setting('key')` reads
//! the `value` of the lake's `sys.setting` record whose key is `key`, as
//! of the position being read or folded, in every place an expression is
//! evaluated; an absent key is null. `param('name')` is the value a read
//! binds to the view parameter of that name; only a view's `where` and
//! `measures` may call it, since only a view declares parameters.
//!
//! **Durations.** The difference of two instants is a duration. A
//! duration divided by a duration is a number, so the days between two
//! instants are `(paid_on - due_on) / 1d`, and the whole days
//! `floor(...)` of it; a duration times or divided by a number is a
//! duration. What an expression certainly computes is [`kind::of`].
//!
//! **Comparisons across kinds.** A comparison between values of kinds
//! that do not compare — a number against text that is not a number, a
//! boolean against anything but a boolean, a JSON value against anything
//! but another — is false for every operator, `!=` included, and never an
//! error: a filter over mixed data selects what matches and skips the
//! rest.
//! `matches(s, pattern)` takes a pattern in I-Regexp (RFC 9485), the
//! interoperable regular-expression subset, matched against the whole
//! string; it is always evaluated by the store, never pushed down to an
//! engine, so that every engine gives the same answer.
//! Aggregates (`count`, `sum`, `min`, `max`, `avg`) appear only in view
//! measures and in `find`'s aggregate, where a set of records is in
//! scope.
//! `date_bucket(x, 'unit')` serves reads (`find`'s `where` and group
//! keys, a pattern's `where`) and, from log format version 3 on, every
//! expression a declaration keeps; the event that first declares such a
//! call is written in version 3 (`docs/log-format.md` §6).
//!
//! # Determinism
//!
//! Derived fields, constraints and guards are evaluated by the fold and
//! must reproduce on replay (`docs/decisions/002-determinism-is-the-boundary.md`).
//! In them, `now()` and `today()` mean the `known_at` of the event being
//! folded, never the clock of the process replaying it. In a read, they
//! mean the time of the read.

use serde::{Deserialize, Serialize};

use crate::id::FieldName;

mod parse;

mod kept;
pub mod kind;
mod rewrite;
pub use kept::keepable;
pub use parse::{ParseError, arity, func_name, parse, print};
pub use rewrite::{expand, rewrite};

/// An expression in the one grammar, typed. A member this form does not
/// know is refused, never ignored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Expr {
    /// A constant.
    Lit {
        /// The constant's value.
        value: Literal,
    },
    /// A field of the record in scope, or `<type>.<field>` in a pattern.
    Path {
        /// The path's segments.
        path: Vec<FieldName>,
    },
    /// A comparison.
    Cmp {
        /// The comparison.
        cmp: CmpOp,
        /// Left side.
        lhs: Box<Expr>,
        /// Right side.
        rhs: Box<Expr>,
    },
    /// Arithmetic.
    Arith {
        /// The operation.
        arith: ArithOp,
        /// Left side.
        lhs: Box<Expr>,
        /// Right side.
        rhs: Box<Expr>,
    },
    /// Negation of a number.
    Neg {
        /// The operand.
        arg: Box<Expr>,
    },
    /// All of the operands hold.
    And {
        /// The operands.
        args: Vec<Expr>,
    },
    /// Any of the operands holds.
    Or {
        /// The operands.
        args: Vec<Expr>,
    },
    /// The operand does not hold.
    Not {
        /// The operand.
        arg: Box<Expr>,
    },
    /// The value is one of a list.
    In {
        /// The value tested.
        arg: Box<Expr>,
        /// The candidates.
        list: Vec<Expr>,
    },
    /// The value is absent or cleared.
    IsNull {
        /// The value tested.
        arg: Box<Expr>,
    },
    /// A function of the closed set.
    Call {
        /// The function.
        func: Func,
        /// Its arguments.
        args: Vec<Expr>,
    },
}

/// A constant in an expression.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Literal {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number, kept as written so that decimals stay exact.
    Number(String),
    /// A string.
    Text(String),
    /// A span of time, in microseconds.
    Duration(i64),
}

/// Comparisons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CmpOp {
    /// `=`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
}

/// Arithmetic.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArithOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
}

/// The closed set of functions. A new function is a change to this
/// contract, so that every engine and every evaluator knows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Func {
    /// `now()`: see the determinism rule in the module documentation.
    Now,
    /// `today()`: midnight UTC of `now()`.
    Today,
    /// `known_at(field)`: when the lake learned the field's value.
    KnownAt,
    /// `seen_at(field)`: when the source observed the field's value.
    SeenAt,
    /// `previous(field)`: in a guard, the field's value before the write
    /// (null for a record the write creates).
    Previous,
    /// `setting('key')`: the value of the lake's `sys.setting` record with
    /// that key, as it stood at the position being read or folded.
    Setting,
    /// `param('name')`: the value a read binds to the view's parameter of
    /// that name. Only a view's `where` and `measures` may call it. Format 8
    /// (`docs/log-format.md` §6).
    Param,
    /// `in_set('name')`: whether the named object set's filter holds for
    /// the record: the filter in the call's place, bound when the
    /// condition is read, never stored membership. In an object set's
    /// `where`, a trigger's `when` and a read's `where`. Format 11
    /// (`docs/log-format.md` §6).
    InSet,
    /// `changed(field)`: in a guard, whether the write changes the field.
    Changed,
    /// `retired()`: in a trigger's guard, whether the write retires the
    /// record (live before it, retired after it). Format 7
    /// (`docs/log-format.md` §6).
    Retired,
    /// `coalesce(a, b, …)`: the first operand that is not null.
    Coalesce,
    /// `if(cond, then, else)`.
    If,
    /// `lower(s)`.
    Lower,
    /// `upper(s)`.
    Upper,
    /// `trim(s)`.
    Trim,
    /// `len(s)`.
    Len,
    /// `concat(a, b, …)`.
    Concat,
    /// `substr(s, start, len)`.
    Substr,
    /// `contains(s, part)`.
    Contains,
    /// `replace(s, from, to)`: every occurrence of `from` replaced.
    Replace,
    /// `starts_with(s, prefix)`.
    StartsWith,
    /// `matches(s, pattern)`: an I-Regexp (RFC 9485) pattern matched
    /// against the whole text; evaluated in the store only.
    Matches,
    /// `abs(n)`.
    Abs,
    /// `round(n, digits)`.
    Round,
    /// `floor(n)`: the greatest whole number not above `n`, as in the
    /// whole days of `floor((now() - due_on) / 1d)`.
    Floor,
    /// `date_bucket(x, 'unit')`: the instant, in UTC, at which the bucket
    /// holding `x` starts; `unit` is `'day'`, `'week'` (ISO, from Monday),
    /// `'month'`, `'quarter'` or `'year'`. `date_bucket(x, 30d, origin)`:
    /// the start of the window of that width holding `x`, the windows
    /// counted from `origin` in both directions. Null when `x` or the
    /// origin is not an instant. What a declaration, whose expressions
    /// the log keeps, may call is the format's (`docs/log-format.md` §6,
    /// Appendix B).
    DateBucket,
    /// `count(…)`: aggregate; see the module documentation.
    Count,
    /// `sum(field)`: aggregate.
    Sum,
    /// `min(field)`: aggregate.
    Min,
    /// `max(field)`: aggregate.
    Max,
    /// `avg(field)`: aggregate.
    Avg,
}

/// The expressions directly inside `e`, left to right.
pub fn children(e: &Expr) -> impl Iterator<Item = &Expr> {
    let (pair, list): ([Option<&Expr>; 2], &[Expr]) = match e {
        Expr::Lit { .. } | Expr::Path { .. } => ([None, None], &[]),
        Expr::Cmp { lhs, rhs, .. } | Expr::Arith { lhs, rhs, .. } => ([Some(lhs), Some(rhs)], &[]),
        Expr::Neg { arg } | Expr::Not { arg } | Expr::IsNull { arg } => ([Some(arg), None], &[]),
        Expr::And { args } | Expr::Or { args } | Expr::Call { args, .. } => ([None, None], args),
        Expr::In { arg, list } => ([Some(arg), None], list),
    };
    pair.into_iter().flatten().chain(list)
}

impl Expr {
    /// Whether it calls one of `funcs`, anywhere in it.
    pub fn calls(&self, funcs: &[Func]) -> bool {
        matches!(self, Expr::Call { func, .. } if funcs.contains(func))
            || children(self).any(|c| c.calls(funcs))
    }

    /// Whether it reads the clock: calls `now()` or `today()`.
    pub fn reads_clock(&self) -> bool {
        self.calls(&[Func::Now, Func::Today])
    }

    /// The fields it reads: the first segment of every path.
    pub fn fields_read(&self) -> std::collections::BTreeSet<&FieldName> {
        let here = match self {
            Expr::Path { path } => path.first(),
            _ => None,
        };
        (here.into_iter())
            .chain(children(self).flat_map(Expr::fields_read))
            .collect()
    }

    /// The record's fields a verb's value reads: as [`Expr::fields_read`],
    /// but for `payload.<name>`, which reads the call's payload.
    pub fn record_fields_read(&self) -> std::collections::BTreeSet<&FieldName> {
        let here = match self {
            Expr::Path { path } if !matches!(&path[..], [p, _] if p.as_str() == "payload") => {
                path.first()
            }
            _ => None,
        };
        (here.into_iter())
            .chain(children(self).flat_map(Expr::record_fields_read))
            .collect()
    }
}

impl Condition {
    /// The typed form: the text parsed, the typed form as it is.
    pub fn typed(&self) -> Result<Expr, ParseError> {
        match self {
            Condition::Text(t) => parse(t),
            Condition::Expr(e) => Ok(e.clone()),
        }
    }
}

impl Func {
    /// Whether the function folds a set of records rather than reading
    /// one.
    pub fn is_aggregate(self) -> bool {
        matches!(
            self,
            Func::Count | Func::Sum | Func::Min | Func::Max | Func::Avg
        )
    }
}

/// An expression as a caller writes it: the text form, or the typed
/// form. Both reach the same evaluator.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Condition {
    /// The text form, parsed by the store.
    Text(String),
    /// The typed form.
    Expr(Expr),
}

impl From<&str> for Condition {
    fn from(s: &str) -> Self {
        Condition::Text(s.to_owned())
    }
}

impl From<Expr> for Condition {
    fn from(e: Expr) -> Self {
        Condition::Expr(e)
    }
}
