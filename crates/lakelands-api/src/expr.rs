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
//! Keywords are lower case. A `path` names a field of the record in
//! scope; in a pattern step, `<type>.<field>` names a field of an
//! earlier step. Calls name a [`Func`]; `known_at(f)` and `seen_at(f)`
//! read a field's provenance (`docs/decisions/010-provenance-per-field.md`),
//! and `changed(f)` is true in a guard when the write changes `f`.
//! `matches(s, pattern)` takes a pattern in I-Regexp (RFC 9485), the
//! interoperable regular-expression subset, matched against the whole
//! string; it is always evaluated by the store, never pushed down to an
//! engine, so that every engine gives the same answer.
//! Aggregates (`count`, `sum`, `min`, `max`, `avg`) appear only in view
//! measures and in `find`'s aggregate, where a set of records is in
//! scope.
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

/// An expression in the one grammar, typed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
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
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
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
    /// `changed(field)`: in a guard, whether the write changes the field.
    Changed,
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
    /// `starts_with(s, prefix)`.
    StartsWith,
    /// `matches(s, pattern)`: an I-Regexp (RFC 9485) pattern matched
    /// against the whole text; evaluated in the store only.
    Matches,
    /// `abs(n)`.
    Abs,
    /// `round(n, digits)`.
    Round,
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
