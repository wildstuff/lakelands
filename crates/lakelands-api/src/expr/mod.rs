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
//! evaluated; an absent key is null.
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

pub use parse::{ParseError, arity, func_name, parse, print};

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
    /// `date_bucket(x, 'unit')`: the instant, in UTC, at which the bucket
    /// holding `x` starts; `unit` is `'day'`, `'week'` (ISO, from Monday),
    /// `'month'`, `'quarter'` or `'year'`. Null when `x` is not an
    /// instant. A read's function only: it is refused in a declaration,
    /// whose expressions the log keeps, until a format version admits it
    /// (`docs/log-format.md` §6, Appendix B).
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
