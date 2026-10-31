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
//! other. If it is not, in a pattern step the first segment names a name
//! bound before the step, the start's or a sibling branch's included, and
//! the second a field of its record (`invoice.amount`), null where an
//! optional step bound none. A field or relation of the record in scope
//! that its reader may know of is read first, whatever a name says; one
//! it may not know of is as one the type does not declare.
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
//! `preceding(f, by, match…)` is `f` on the record of the same type
//! before this one in the series its `match` fields name, ordered by
//! `by`; only a constraint and a trigger's `when` may call it, since only
//! the write judging one reads the series
//! (`docs/decisions/059-a-type-declares-its-constraints.md`,
//! `docs/decisions/004-the-change-feed-is-the-log.md`).
//! `in_force(T.f, T.by <= by, T.m = m, …)` is `f` on the record of another
//! stored type `T` whose `by` is the greatest at or below this record's
//! `by` among those that hold its values in every matched field, the first
//! in key order among ties, of those its reader is answered; only a field
//! computed at the read may call it, since a stored pairing would go stale
//! when the paired record is written after the record it governs
//! (`docs/decisions/052-a-field-may-be-computed-at-the-read.md`). Its
//! `T.` paths are fields of `T`, no part of this record.
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
//! keys, a pattern's `where`) and every expression a declaration keeps.
//!
//! # Determinism
//!
//! Derived fields and guards are evaluated by the fold and must
//! reproduce on replay (`docs/decisions/002-determinism-is-the-boundary.md`).
//! In them, `now()` and `today()` mean the `known_at` of the event being
//! folded, never the clock of the process replaying it. A constraint is
//! judged by the write alone, and its clock is the write's; so is a
//! guard's `preceding`, whose firing the log keeps. In a read, they
//! mean the read's clock: the instant `valid_at` names, else as of a
//! position the `known_at` of its event, else the time of the read
//! (`docs/decisions/052-a-field-may-be-computed-at-the-read.md`).

use serde::{Deserialize, Serialize};

use crate::id::FieldName;

mod parse;

mod check;
mod kept;
pub mod kind;
mod rewrite;
pub use check::{Place, check};
pub use kept::keepable;
pub use parse::{ParseError, arity, func_by_name, func_name, parse, print};
pub use rewrite::{expand, expand_kept, rewrite};

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
    /// A field of the record in scope, or `<name>.<field>` in a pattern.
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
    /// `preceding(field, by, match, …)`: in a constraint and a trigger's
    /// guard, `field` of the live record of the type, other than the one
    /// written, that holds this record's values in every matched field
    /// and whose `by` is the greatest below this record's, the first in
    /// key order among ties; null where none is, or where this record's
    /// `by` or a matched field is null.
    Preceding,
    /// `in_force(T.f, T.by <= by, T.m = m, …)`: in a field computed at the
    /// read, `f` of the record of another stored type `T` in force at this
    /// record (the module doc); null where this `by` or a match is null.
    InForce,
    /// `setting('key')`: the value of the lake's `sys.setting` record with
    /// that key, as it stood at the position being read or folded.
    Setting,
    /// `param('name')`: the value a read binds to the view's parameter of
    /// that name. Only a view's `where` and `measures` may call it.
    Param,
    /// `exists(relation)`: in a verb's conditions and set values, whether
    /// the path that begins at the relation reaches a record: the one link
    /// a to-one relation carried by links holds from the record, or the
    /// one record a derived relation onto its target's key matches and
    /// whose `where` holds.
    Exists,
    /// `in_set('name')`: whether the named object set's filter holds for
    /// the record: the filter in the call's place, bound when the
    /// condition is read, never stored membership. In an object set's
    /// `where`, a trigger's `when` and a read's `where`.
    InSet,
    /// `changed(field)`: in a guard, whether the write changes the field.
    Changed,
    /// `retired()`: in a trigger's guard, whether the write retires the
    /// record (live before it, retired after it).
    Retired,
    /// `created()`: in a trigger's guard, whether the write found no live
    /// record under the key: a create or a revival.
    Created,
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
    /// `first(field, by)`: aggregate, the field's value on the member
    /// whose `by` comes first.
    First,
    /// `last(field, by)`: aggregate, the field's value on the member
    /// whose `by` comes last.
    Last,
}

/// The expressions directly inside `e`, left to right; of an
/// `in_force`, this record's side of each pair alone, since the other
/// type's fields are no part of this record.
pub fn children(e: &Expr) -> impl Iterator<Item = &Expr> {
    let (pair, list): ([Option<&Expr>; 2], &[Expr]) = match e {
        Expr::Lit { .. } | Expr::Path { .. } => ([None, None], &[]),
        Expr::Cmp { lhs, rhs, .. } | Expr::Arith { lhs, rhs, .. } => ([Some(lhs), Some(rhs)], &[]),
        Expr::Neg { arg } | Expr::Not { arg } | Expr::IsNull { arg } => ([Some(arg), None], &[]),
        Expr::And { args } | Expr::Or { args } | Expr::Call { args, .. } => ([None, None], args),
        Expr::In { arg, list } => ([Some(arg), None], list),
    };
    let paired = in_force(e).is_some();
    pair.into_iter()
        .flatten()
        .chain(list.iter().filter_map(move |a| match (paired, a) {
            (true, Expr::Cmp { rhs, .. }) => Some(&**rhs),
            (paired, a) => (!paired).then_some(a),
        }))
}

/// The arguments of `e` where it calls `in_force`.
fn in_force(e: &Expr) -> Option<&[Expr]> {
    match e {
        Expr::Call {
            func: Func::InForce,
            args,
        } => Some(args),
        _ => None,
    }
}

/// What an `in_force` pairs by: the other type, the field read there, the
/// order bound (its field, this record's) and each matched pair, alike.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pairing<'e> {
    /// The other type, as the paths name it.
    pub ty: &'e FieldName,
    /// The field read on the record in force.
    pub read: &'e FieldName,
    /// The order bound: `T.by`, then this record's `by`.
    pub by: (&'e FieldName, &'e FieldName),
    /// Each matched pair: `T.m`, then this record's `m`.
    pub matched: Vec<(&'e FieldName, &'e FieldName)>,
}

impl Pairing<'_> {
    /// The series it reads among the pairings of a record: `T`, the bound
    /// and the matched pairs, each `T`'s field then this record's.
    pub fn series(&self) -> Vec<FieldName> {
        let pairs = std::iter::once(self.by).chain(self.matched.iter().copied());
        (std::iter::once(self.ty).chain(pairs.flat_map(|(t, m)| [t, m])))
            .cloned()
            .collect()
    }

    /// The fields of `T` it reads: `f`, `by` and each matched one.
    pub fn theirs(&self) -> impl Iterator<Item = &FieldName> {
        let pairs = std::iter::once(self.by).chain(self.matched.iter().copied());
        std::iter::once(self.read).chain(pairs.map(|(t, _)| t))
    }

    /// The pairing of an `in_force` call's arguments; `None` where they
    /// are not of its form: a `T.f` path, then `T.by <= by`, then one
    /// `T.m = m` or more, over one `T`.
    pub fn of(args: &[Expr]) -> Option<Pairing<'_>> {
        type Side<'e> = (&'e FieldName, (&'e FieldName, &'e FieldName));
        fn side(e: &Expr, op: CmpOp) -> Option<Side<'_>> {
            if let Expr::Cmp { cmp, lhs, rhs } = e
                && let (Expr::Path { path: t }, Expr::Path { path: m }) = (&**lhs, &**rhs)
                && let ([ty, f], [m]) = (&t[..], &m[..])
                && *cmp == op
            {
                return Some((ty, (f, m)));
            }
            None
        }
        let Some(Expr::Path { path }) = args.first() else {
            return None;
        };
        let [ty, read] = &path[..] else { return None };
        let (bound, by) = side(args.get(1)?, CmpOp::Le)?;
        let matched = (args.get(2..)?.iter())
            .map(|a| side(a, CmpOp::Eq).filter(|(t, _)| *t == ty).map(|(_, p)| p))
            .collect::<Option<Vec<_>>>()?;
        let pairing = Pairing {
            ty,
            read,
            by,
            matched,
        };
        (bound == ty && !pairing.matched.is_empty()).then_some(pairing)
    }
}

/// Every path in `e`, left to right.
pub fn paths<'e>(e: &'e Expr, out: &mut Vec<&'e [FieldName]>) {
    if let Expr::Path { path } = e {
        out.push(path);
    }
    children(e).for_each(|c| paths(c, out));
}

impl Expr {
    /// Its parts as an `and` holds them, in order, an `and` among them
    /// split in turn; itself alone where it is no `and`.
    pub fn conjuncts(&self) -> Vec<&Expr> {
        match self {
            Expr::And { args } => args.iter().flat_map(Expr::conjuncts).collect(),
            e => vec![e],
        }
    }

    /// Whether it calls one of `funcs`, anywhere in it.
    pub fn calls(&self, funcs: &[Func]) -> bool {
        matches!(self, Expr::Call { func, .. } if funcs.contains(func))
            || children(self).any(|c| c.calls(funcs))
    }

    /// The keys it reads as `setting('<key>')`, without a date, anywhere in
    /// it; with `dated`, those it reads as `setting('<key>', <at>)`
    /// (`docs/decisions/090-a-setting-value-may-hold-from-a-date.md`).
    pub fn settings_read(&self, dated: bool) -> std::collections::BTreeSet<&str> {
        let mut out = std::collections::BTreeSet::new();
        if let Expr::Call {
            func: Func::Setting,
            args,
        } = self
            && let Some(Expr::Lit {
                value: Literal::Text(key),
            }) = args.first()
            && (args.len() == 2) == dated
        {
            out.insert(key.as_str());
        }
        out.extend(children(self).flat_map(|c| c.settings_read(dated)));
        out
    }

    /// What of it a walk judges: its parts as an `and` holds them, those
    /// that are the write's alone to judge left out, the clock and
    /// `preceding`; `None` where no part is left.
    pub fn walked(&self) -> Option<Expr> {
        let write = [Func::Now, Func::Today, Func::Preceding];
        let mut args: Vec<Expr> = (self.conjuncts().into_iter())
            .filter(|e| !e.calls(&write))
            .cloned()
            .collect();
        match args.len() {
            0 => None,
            1 => args.pop(),
            _ => Some(Expr::And { args }),
        }
    }

    /// The series each `preceding` in it reads, its `by` and then its
    /// matched fields, each once, with every field it reads there.
    pub fn series(
        &self,
    ) -> std::collections::BTreeMap<Vec<FieldName>, std::collections::BTreeSet<FieldName>> {
        let mut out = std::collections::BTreeMap::new();
        if let Expr::Call {
            func: Func::Preceding,
            args,
        } = self
        {
            let field = |a: &Expr| match a {
                Expr::Path { path } => path.first().cloned(),
                _ => None,
            };
            if let Some(read) = args.iter().map(field).collect::<Option<Vec<_>>>() {
                let fields: &mut std::collections::BTreeSet<_> =
                    out.entry(read[1..].to_vec()).or_default();
                fields.extend(read);
            }
        }
        for (series, fields) in children(self).flat_map(Expr::series) {
            out.entry(series).or_default().extend(fields);
        }
        out
    }

    /// Every `in_force` in it, as it pairs; `Err` with the first that is
    /// not of its form.
    pub fn pairings(&self) -> Result<Vec<Pairing<'_>>, &Expr> {
        let mut out = Vec::new();
        if let Some(args) = in_force(self) {
            out.push(Pairing::of(args).ok_or(self)?);
        }
        for c in children(self) {
            out.extend(c.pairings()?);
        }
        Ok(out)
    }

    /// Whether every `now()` and `today()` in it is a clock term, the call
    /// with or without a duration literal added or taken away, on one side
    /// of a comparison whose other side is a field, with or without a
    /// duration literal added or taken away, and nothing else.
    pub fn clock_compared(&self) -> bool {
        fn shifted(e: &Expr, base: fn(&Expr) -> bool) -> bool {
            let duration = |e: &Expr| {
                matches!(
                    e,
                    Expr::Lit {
                        value: Literal::Duration(_)
                    }
                )
            };
            match e {
                Expr::Arith {
                    arith: ArithOp::Add | ArithOp::Sub,
                    lhs,
                    rhs,
                } if duration(rhs) => base(lhs),
                Expr::Arith {
                    arith: ArithOp::Add,
                    lhs,
                    rhs,
                } if duration(lhs) => base(rhs),
                e => base(e),
            }
        }
        let term = |e: &Expr| {
            shifted(e, |e| {
                matches!(
                    e,
                    Expr::Call {
                        func: Func::Now | Func::Today,
                        ..
                    }
                )
            })
        };
        let field = |e: &Expr| shifted(e, |e| matches!(e, Expr::Path { .. }));
        match self {
            Expr::Cmp { lhs, rhs, .. }
                if (term(lhs) && field(rhs)) || (term(rhs) && field(lhs)) =>
            {
                true
            }
            Expr::Call {
                func: Func::Now | Func::Today,
                ..
            } => false,
            e => children(e).all(Expr::clock_compared),
        }
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

    /// The fields it reads outside every aggregate: of a view's measure,
    /// the measures it reads by name (018).
    pub fn fields_read_by_name(&self) -> std::collections::BTreeSet<&FieldName> {
        match self {
            Expr::Call { func, .. } if func.is_aggregate() => Default::default(),
            Expr::Path { path } => path.first().into_iter().collect(),
            _ => children(self).flat_map(Expr::fields_read_by_name).collect(),
        }
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
            Func::Count | Func::Sum | Func::Min | Func::Max | Func::Avg | Func::First | Func::Last
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
