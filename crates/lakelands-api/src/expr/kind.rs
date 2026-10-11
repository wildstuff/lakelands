//! The kind of value an expression certainly computes, whatever record it
//! is evaluated over, read off the evaluator's own rules (`lakelands-core`, `expr::eval`):
//! a literal, a function's result, arithmetic over certain kinds, a field
//! by its declared kind. A text is never certain to fail, since one that
//! reads as a number or an instant is one; where the kind depends on the
//! record (a `json` field, two kinds under `if`, a setting) nothing is said.
//! A path that steps into a field that holds no members can never read.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::{ArithOp, Expr, Func, Literal};
use crate::FieldName;
use crate::error::Case;
use crate::ontology::StorageKind;

/// What an expression certainly computes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// True or false.
    Boolean,
    /// An exact number.
    Number,
    /// Text.
    Text,
    /// An instant.
    Instant,
    /// A duration.
    Duration,
}

impl Kind {
    /// The kind, for a person.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Boolean => "a boolean",
            Kind::Number => "a number",
            Kind::Text => "a text",
            Kind::Instant => "an instant",
            Kind::Duration => "a duration",
        }
    }

    /// Whether a value of this kind can land in a field of kind `field`,
    /// as the fold lands one (`eval::to_json` in `lakelands-core`).
    pub fn lands(self, field: StorageKind) -> bool {
        use StorageKind as S;
        match field {
            S::Text | S::Reference | S::Json => true,
            S::Number | S::Decimal => matches!(self, Kind::Number | Kind::Text),
            S::Boolean => self == Kind::Boolean,
            S::DateTime | S::Date => matches!(self, Kind::Instant | Kind::Text),
            S::Bytes | S::Vector | S::Geo => false,
        }
    }
}

/// The kind, for a person: "a number".
impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// What a path reads: the declared kind of the last field it reaches and
/// how many of its segments reach it, one, or more where the path steps
/// through references.
pub type Reads<'a> = &'a dyn Fn(&[FieldName]) -> Option<(StorageKind, usize)>;

/// The kind `e` certainly computes, `None` where that depends on the
/// record; or why it can never be evaluated. `field` answers what each
/// path reads.
pub fn of(e: &Expr, field: Reads<'_>) -> Result<Option<Kind>, Case> {
    use Kind::{Boolean, Duration, Instant, Number, Text};
    let expr = || super::print(e);
    Ok(match e {
        Expr::Lit { value } => match value {
            Literal::Null => None,
            Literal::Bool(_) => Some(Boolean),
            Literal::Number(_) => Some(Number),
            Literal::Text(_) => Some(Text),
            Literal::Duration(_) => Some(Duration),
        },
        Expr::Path { path } => match field(path) {
            Some((k, n)) if n == path.len() => match k {
                StorageKind::Number | StorageKind::Decimal => Some(Number),
                StorageKind::Boolean => Some(Boolean),
                StorageKind::DateTime | StorageKind::Date => Some(Instant),
                StorageKind::Text | StorageKind::Reference => Some(Text),
                _ => None,
            },
            // A longer path walks into a `json` or `geo` value's members;
            // into anything else it reads nothing, whatever the record
            // holds. `payload.<name>` is a call's payload.
            Some((k, n))
                if !matches!(k, StorageKind::Json | StorageKind::Geo)
                    && path[0].as_str() != "payload" =>
            {
                let (expr, field) = (expr(), path[n - 1].clone());
                return Err(match k {
                    StorageKind::Reference => Case::StepsIntoReference { expr, field },
                    _ => Case::StepsIntoScalar { expr, field },
                });
            }
            _ => None,
        },
        Expr::Cmp { lhs, rhs, .. } => of(lhs, field).and(of(rhs, field)).map(|_| Some(Boolean))?,
        Expr::In { arg, list: args } => {
            (std::iter::once(&**arg).chain(args)).try_for_each(|c| of(c, field).map(|_| ()))?;
            Some(Boolean)
        }
        Expr::And { args } | Expr::Or { args } => {
            args.iter().try_for_each(|c| of(c, field).map(|_| ()))?;
            Some(Boolean)
        }
        Expr::Not { arg } | Expr::IsNull { arg } => of(arg, field).map(|_| Some(Boolean))?,
        Expr::Neg { arg } => match of(arg, field)? {
            Some(k @ (Number | Duration)) => Some(k),
            Some(kind @ (Boolean | Instant)) => {
                return Err(Case::KindNegates { expr: expr(), kind });
            }
            _ => None,
        },
        Expr::Arith { arith, lhs, rhs } => match (of(lhs, field)?, of(rhs, field)?) {
            // A text may read as a number or an instant; so may what is
            // not known.
            (None | Some(Text), _) | (_, None | Some(Text)) => None,
            (Some(l), Some(r)) => Some(match (arith, l, r) {
                (_, Number, Number) => Number,
                (ArithOp::Sub, Instant, Instant) => Duration,
                (ArithOp::Add | ArithOp::Sub, Instant, Duration) => Instant,
                (ArithOp::Add, Duration, Instant) => Instant,
                (ArithOp::Add | ArithOp::Sub, Duration, Duration) => Duration,
                (ArithOp::Div, Duration, Duration) => Number,
                (ArithOp::Mul | ArithOp::Div, Duration, Number) => Duration,
                (ArithOp::Mul, Number, Duration) => Duration,
                (_, left, right) => {
                    return Err(Case::KindSides {
                        expr: expr(),
                        left,
                        right,
                    });
                }
            }),
        },
        Expr::Call { func, args } => {
            let kinds: Vec<Option<Kind>> =
                (args.iter().map(|a| of(a, field))).collect::<Result<_, _>>()?;
            let first = kinds.first().copied().flatten();
            match func {
                Func::Now | Func::Today | Func::KnownAt | Func::SeenAt | Func::DateBucket => {
                    Some(Instant)
                }
                Func::Changed | Func::Retired | Func::Contains | Func::StartsWith => Some(Boolean),
                Func::InSet => Some(Boolean),
                Func::Matches => Some(Boolean),
                Func::Lower | Func::Upper | Func::Trim | Func::Concat | Func::Substr => Some(Text),
                Func::Replace => Some(Text),
                Func::Len | Func::Count => Some(Number),
                Func::Abs | Func::Round | Func::Floor | Func::Sum | Func::Avg => match first {
                    Some(kind @ (Boolean | Instant | Duration)) => {
                        return Err(Case::KindTakesNumber { expr: expr(), kind });
                    }
                    _ => Some(Number),
                },
                Func::Previous | Func::Min | Func::Max => first,
                Func::If => kinds
                    .get(1)
                    .copied()
                    .flatten()
                    .filter(|k| kinds.get(2) == Some(&Some(*k))),
                Func::Coalesce => first.filter(|k| kinds.iter().all(|x| x == &Some(*k))),
                Func::Setting | Func::Param => None,
            }
        }
    })
}
