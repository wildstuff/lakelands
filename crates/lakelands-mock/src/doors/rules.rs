//! A declaration's names and expressions at `declare`, checked as the
//! store checks them, with its cases at its places: every name an ASCII identifier
//! (a type's, identifiers joined by `.`); every expression parsed and
//! checked for the place it is evaluated in, the functions it calls and
//! the numbers it holds; and `in_set()` only in a set's `where`, a
//! trigger's `when` and a read's `where`, where the named set's filter
//! stands in its place. The mock is a second implementation, so these are
//! copies of the store's (`lakelands_core::names`, `lakelands_core::expr`'s
//! `check` and `declare.rs`), held to it by
//! `names_and_expressions_are_declared_only_as_the_store_declares_them`.

use lakelands_api::error::{Case, Naming, Place, Result, Said};
use lakelands_api::expr::{Condition, Expr, Func, Literal, arity, children, func_name, parse};
use lakelands_api::id::{FieldName, TypeName};
use lakelands_api::ontology::{FieldDecl, VerbEffect};
use lakelands_api::write::{Declaration, ObjectSetDecl};

use super::{at, invalid, invalid_field};
use crate::state::State;

/// Where an expression is evaluated, which decides the functions it may
/// use.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Eval {
    /// Over one record, outside a write: a filter, a constraint.
    Record,
    /// Over one record in a write: a derived field.
    Write,
    /// A trigger's guard: a write, where `retired` has meaning as well.
    Guard,
    /// Over a group of records: a view measure.
    Group,
    /// Over one source record of a view: its `where`, or an aggregate's
    /// argument in a measure.
    View,
}

fn of_field(name: &FieldName) -> Place {
    let name = name.clone();
    Place::Field { name }
}

/// Refuse the names and expressions of `d` as the store refuses them, in
/// its order.
pub(super) fn declaration(st: &State, d: &Declaration) -> Result<()> {
    let set = |n: &str| st.object_sets.iter().find(|s| s.name == n);
    match d {
        Declaration::Type(t) => {
            name(Naming::Type, t.name.as_str())?;
            let owner = [Place::Type {
                name: t.name.clone(),
            }];
            for f in &t.fields {
                field(&owner, f)?;
            }
            let mut named = std::collections::BTreeSet::new();
            if let Some(r) = t.relations.iter().find(|r| !named.insert(&r.name)) {
                let (what, name) = (Naming::Relation, r.name.to_string());
                return Err(invalid(at(&owner, Case::DeclaredTwice { what, name })));
            }
            for r in &t.relations {
                name(Naming::Relation, r.name.as_str())?;
                let name = r.name.clone();
                let on = [owner[0].clone(), Place::Relation { name }];
                for f in &r.fields {
                    field(&on, f)?;
                }
            }
        }
        Declaration::ValueType(v) => {
            name(Naming::ValueType, v.name.as_str())?;
            if let Some(c) = &v.constraint {
                let name = v.name.clone();
                condition(
                    c,
                    Eval::Record,
                    &[Place::ValueType { name }, Place::Constraint],
                )?;
            }
        }
        Declaration::Verb(v) => {
            name(Naming::Verb, v.name.as_str())?;
            let verb = Place::Verb {
                name: v.name.clone(),
            };
            let payload = [verb.clone(), Place::Payload];
            for f in &v.payload {
                field(&payload, f)?;
                computed_here(&payload, f)?;
            }
            if let Some(r) = &v.requires {
                condition(r, Eval::Record, &[verb.clone(), Place::Requires])?;
            }
            match &v.effect {
                VerbEffect::Write { set } => {
                    for a in set {
                        let on = [verb.clone(), of_field(&a.field)];
                        condition(&a.value, Eval::Record, &on)?;
                    }
                }
                VerbEffect::External { executor } => name(Naming::Executor, executor)?,
            }
        }
        Declaration::Trigger(t) => {
            name(Naming::Trigger, t.name.as_str())?;
            if let Some(w) = &t.when {
                let on = [Place::Trigger {
                    name: t.name.clone(),
                }];
                filter(&set, &t.on, w, Eval::Guard, &on)?;
            }
        }
        Declaration::ObjectSet(s) => {
            name(Naming::ObjectSet, &s.name)?;
            let on = [Place::ObjectSet {
                name: s.name.clone(),
            }];
            let named = |n: &str| Some(s).filter(|s| s.name == n).or_else(|| set(n));
            filter(&named, &s.of, &s.filter, Eval::Record, &on)?;
        }
        Declaration::Function(f) => {
            name(Naming::Function, &f.name)?;
            name(Naming::Backing, &f.backing)?;
            let function = Place::Function {
                name: f.name.clone(),
            };
            let input = [function.clone(), Place::Input];
            for i in &f.inputs {
                field(&input, i)?;
                computed_here(&input, i)?;
            }
            if let ("expr", Some(e)) = (f.backing.as_str(), &f.expr) {
                condition(e, Eval::Record, &[function])?;
            }
        }
    }
    Ok(())
}

/// A field's name, and the expression it is derived from, at `owner`.
fn field(owner: &[Place], f: &FieldDecl) -> Result<()> {
    name(Naming::Field, f.name.as_str())?;
    if let Some(d) = &f.derived {
        condition(d, Eval::Write, &[owner, &[of_field(&f.name)]].concat())?;
    }
    Ok(())
}

/// As the store: `computed_at` on a field no record holds.
fn computed_here(on: &[Place], f: &FieldDecl) -> Result<()> {
    match f.computed_at {
        Some(_) => {
            let on = [on, &[of_field(&f.name)]].concat();
            Err(invalid_field(&f.name, at(&on, Case::ComputedAtNoRecord)))
        }
        None => Ok(()),
    }
}

/// Refuse a name of this kind that cannot be declared.
pub(super) fn name(what: Naming, n: &str) -> Result<()> {
    fault(what, n).map_or(Ok(()), |c| Err(invalid(c)))
}

/// Why a name of this kind cannot be declared, if it cannot.
pub(crate) fn fault(what: Naming, name: &str) -> Option<Case> {
    let identifier = |n: &str| {
        let mut chars = n.chars();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    let ok = match what {
        Naming::Type => !name.is_empty() && name.split('.').all(identifier),
        _ => identifier(name),
    };
    let name = name.escape_debug().to_string();
    match what {
        _ if ok => None,
        _ if name.is_empty() => Some(Case::NameEmpty { what }),
        Naming::Type => Some(Case::TypeNameForm { name }),
        _ => Some(Case::NameNotIdentifier { what, name }),
    }
}

/// A condition in its typed form, checked for its place, at the places
/// `on`; a set is named only where its filter is put in the name's place.
pub(crate) fn condition(c: &Condition, place: Eval, on: &[Place]) -> Result<Expr> {
    let e = typed(c, place).map_err(|c| invalid(at(on, c)))?;
    if calls_in_set(&e) {
        return Err(invalid(at(on, Case::InSetHere)));
    }
    Ok(e)
}

fn calls_in_set(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Call {
            func: Func::InSet,
            ..
        }
    ) || children(e).any(calls_in_set)
}

/// A condition parsed and checked for its place, the sets it names left
/// as it names them.
fn typed(c: &Condition, place: Eval) -> std::result::Result<Expr, Case> {
    let e = match c {
        Condition::Expr(e) => e.clone(),
        Condition::Text(t) => parse(t).map_err(|e| {
            let (text, why) = (t.clone(), Box::new(e.into()));
            Case::NotParsed { text, why }
        })?,
    };
    check(&e, place)?;
    Ok(e)
}

/// A condition over the records of `ty` that may name object sets: each
/// named set's filter stands in the call's place. `set` answers a set by
/// its name; one it does not answer is not declared, and one of another
/// type and one that names itself through others are refused.
fn filter<'s>(
    set: &dyn Fn(&str) -> Option<&'s ObjectSetDecl>,
    ty: &TypeName,
    c: &Condition,
    place: Eval,
    on: &[Place],
) -> Result<Expr> {
    let e = typed(c, place).map_err(|c| invalid(at(on, c)))?;
    let named = |name: &str| {
        let set_name = name.to_owned();
        match set(name) {
            Some(s) if &s.of == ty => typed(&s.filter, Eval::Record).map_err(Said::from),
            Some(s) => {
                let (of, ty) = (s.of.clone(), ty.clone());
                Err(Case::SetOfOtherType {
                    set: set_name,
                    of,
                    ty,
                }
                .into())
            }
            None => Err(Case::SetUndeclared { set: set_name }.into()),
        }
    };
    lakelands_api::expr::expand(&e, &named).map_err(|s| invalid(at(on, s)))
}

/// The units `date_bucket` knows.
const BUCKET_UNITS: [&str; 5] = ["day", "week", "month", "quarter", "year"];

/// An expression checked before it is taken: every call has a count of
/// arguments its function takes, a function that names a field is given
/// one, `setting`, `param` and `in_set` a quoted name, numbers are numbers
/// the store's evaluator holds exactly, and `changed`, `previous`,
/// `retired`, `param` and the aggregates appear only where they have
/// meaning.
fn check(e: &Expr, place: Eval) -> std::result::Result<(), Case> {
    match e {
        Expr::Lit {
            value: Literal::Number(n),
        } if !exact(n) => Err(Case::NotExact { number: n.clone() }),
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
            call(*func, args, place)?;
            // An aggregate's argument is evaluated per source record.
            let inner = if func.is_aggregate() {
                Eval::View
            } else {
                place
            };
            args.iter().try_for_each(|a| check(a, inner))
        }
    }
}

/// A call of `func` on `args` checked for its arguments and its place.
fn call(func: Func, args: &[Expr], place: Eval) -> std::result::Result<(), Case> {
    let name = func_name(func).to_owned();
    let (min, max) = arity(func);
    if args.len() < min || max.is_some_and(|m| args.len() > m) {
        let given = args.len();
        return Err(Case::CallArity { name, given });
    }
    let field = matches!(args.first(), Some(Expr::Path { path }) if path.len() == 1);
    match func {
        Func::KnownAt | Func::SeenAt | Func::Previous | Func::Changed if !field => {
            Err(Case::CallTakesField { name })
        }
        Func::Setting | Func::Param | Func::InSet if quoted(args.first()).is_none() => {
            Err(Case::CallTakesQuoted { name })
        }
        // A calendar unit, quoted, takes no origin; a width is a duration
        // above zero, with the origin its windows are counted from.
        Func::DateBucket => {
            let unit = quoted(args.get(1));
            let width = matches!(args.get(1),
                Some(Expr::Lit { value: Literal::Duration(w) }) if *w > 0);
            if let Some(u) = unit.filter(|u| !BUCKET_UNITS.contains(&u.as_str())) {
                return Err(Case::BucketUnit { unit: u.clone() });
            }
            if (unit.is_some(), width) != (args.len() == 2, args.len() == 3) {
                return Err(Case::BucketArguments);
            }
            Ok(())
        }
        Func::Previous | Func::Changed if !matches!(place, Eval::Write | Eval::Guard) => {
            Err(Case::OnlyInWrite { name })
        }
        Func::Retired if place != Eval::Guard => Err(Case::RetiredOnlyInGuard),
        f if f.is_aggregate() && place != Eval::Group => Err(Case::AggregateHere { name }),
        Func::Param if !matches!(place, Eval::View | Eval::Group) => Err(Case::ParamOutsideView),
        _ => Ok(()),
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

/// Whether the store's evaluator holds the numeral `n` exactly: a sign,
/// digits and a fraction, whose whole part fits in 96 bits; the store
/// rounds a fraction longer than it holds.
fn exact(n: &str) -> bool {
    const MAX: u128 = (1 << 96) - 1;
    let digits = n.strip_prefix(['-', '+']).unwrap_or(n);
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    let all_digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let whole = whole.trim_start_matches('0');
    (!digits.is_empty() && digits != ".")
        && all_digits(whole)
        && all_digits(fraction)
        && (whole.is_empty() || whole.parse::<u128>().is_ok_and(|w| w <= MAX))
}
