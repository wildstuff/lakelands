//! Whether an expression may be evaluated where it is put: the check
//! the store runs on every expression a declaration or a read hands it,
//! here so that a host asks it exactly as the store does.

use std::str::FromStr;

use rust_decimal::Decimal;

use super::{Condition, Expr, Func, Literal, arity, func_name, parse};
use crate::error::Case;

/// Where an expression is evaluated, which decides the functions it may
/// use. A host names a place; it does not match on all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Place {
    /// Over one record, outside a write: a filter.
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
    /// A verb's `requires`, preconditions and set values, and an object
    /// set's `where`: like `Record`, and `exists` has meaning.
    Verb,
    /// A type's constraint: a guard's, where `preceding` has meaning as
    /// well.
    Constraint,
}

impl Condition {
    /// The typed form, checked for `place`: the text parsed, refused as
    /// [`Case::NotParsed`] where it does not parse, then [`check`]ed.
    pub fn checked(&self, place: Place) -> Result<Expr, Case> {
        let e = match self {
            Condition::Expr(e) => e.clone(),
            Condition::Text(t) => parse(t).map_err(|e| {
                let (text, why) = (t.clone(), Box::new(e.into()));
                Case::NotParsed { text, why }
            })?,
        };
        check(&e, place)?;
        Ok(e)
    }
}

/// The units `date_bucket` knows.
const BUCKET_UNITS: [&str; 5] = ["day", "week", "month", "quarter", "year"];

/// Check an expression before evaluating it: every call has a count of
/// arguments its function takes, a function that names a field is given
/// one, `setting` is given a quoted key, numbers are numbers this
/// evaluator can hold exactly, and `changed`, `previous` and the
/// aggregates appear only where they have meaning. The store checks every
/// typed expression it is handed this way and refuses it as invalid; a
/// text expression is checked the same way after parsing.
pub fn check(e: &Expr, place: Place) -> Result<(), Case> {
    match e {
        Expr::Lit {
            value: Literal::Number(n),
        } if Decimal::from_str(n).is_err() => Err(Case::NotExact { number: n.clone() }),
        Expr::Lit { .. } => Ok(()),
        Expr::Path { path } if path.is_empty() => Err(Case::PathEmpty),
        Expr::Path { .. } => Ok(()),
        Expr::Cmp { lhs, rhs, .. } | Expr::Arith { lhs, rhs, .. } => {
            check(lhs, place)?;
            check(rhs, place)
        }
        Expr::Neg { arg } | Expr::Not { arg } | Expr::IsNull { arg } => check(arg, place),
        Expr::And { args } | Expr::Or { args } => args.iter().try_for_each(|a| check(a, place)),
        Expr::In { arg, list } => {
            if list.is_empty() {
                return Err(Case::InEmpty);
            }
            check(arg, place)?;
            list.iter().try_for_each(|a| check(a, place))
        }
        Expr::Call { func, args } => {
            let name = func_name(*func).to_owned();
            let (min, max) = arity(*func);
            // A constraint is a guard, with `preceding` beside.
            let guard = matches!(place, Place::Guard | Place::Constraint);
            if args.len() < min || max.is_some_and(|m| args.len() > m) {
                return Err(Case::CallArity {
                    name,
                    given: args.len(),
                });
            }
            match func {
                Func::KnownAt | Func::SeenAt | Func::Previous | Func::Changed if !matches!(args.first(), Some(Expr::Path { path }) if path.len() == 1) =>
                {
                    return Err(Case::CallTakesField { name });
                }
                Func::Preceding
                    if !args
                        .iter()
                        .all(|a| matches!(a, Expr::Path { path } if path.len() == 1)) =>
                {
                    return Err(Case::CallTakesField { name });
                }
                Func::Preceding if place != Place::Constraint => return Err(Case::PrecedingHere),
                Func::Setting | Func::Param | Func::InSet if quoted(args.first()).is_none() => {
                    return Err(Case::CallTakesQuoted { name });
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
                        return Err(Case::BucketUnit { unit: u.clone() });
                    }
                    if (unit.is_some(), width) != (args.len() == 2, args.len() == 3) {
                        return Err(Case::BucketArguments);
                    }
                }
                Func::Previous | Func::Changed if !(guard || place == Place::Write) => {
                    return Err(Case::OnlyInWrite { name });
                }
                Func::Retired if !guard => return Err(Case::RetiredOnlyInGuard),
                Func::Created if !guard => return Err(Case::CreatedOnlyInGuard),
                Func::Exists if !(guard || place == Place::Verb) => {
                    return Err(Case::ExistsHere);
                }
                Func::Exists if !matches!(args.first(), Some(Expr::Path { path }) if path.len() == 1) =>
                {
                    return Err(Case::CallTakesField { name });
                }
                f if f.is_aggregate() && place != Place::Group => {
                    return Err(Case::AggregateHere { name });
                }
                Func::Param if !matches!(place, Place::View | Place::Group) => {
                    return Err(Case::ParamOutsideView);
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

/// The text of a quoted literal.
fn quoted(e: Option<&Expr>) -> Option<&String> {
    match e {
        Some(Expr::Lit {
            value: Literal::Text(t),
        }) => Some(t),
        _ => None,
    }
}
