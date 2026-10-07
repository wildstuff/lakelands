//! A declaration's names and expressions at `declare`, checked as the
//! store checks them, with its sentences: every name an ASCII identifier
//! (a type's, identifiers joined by `.`); every expression parsed and
//! checked for the place it is evaluated in, the functions it calls and
//! the numbers it holds; and `in_set()` only in a set's `where`, a
//! trigger's `when` and a read's `where`, where the named set's filter
//! stands in its place. The mock is a second implementation, so these are
//! copies of the store's (`lakelands_core::names`, `lakelands_core::expr`'s
//! `check` and `declare.rs`), held to it by
//! `names_and_expressions_are_declared_only_as_the_store_declares_them`.

use lakelands_api::error::Result;
use lakelands_api::expr::{Condition, Expr, Func, Literal, arity, children, func_name, parse};
use lakelands_api::id::TypeName;
use lakelands_api::ontology::{FieldDecl, VerbEffect};
use lakelands_api::write::{Declaration, ObjectSetDecl};

use super::invalid;
use crate::state::State;

/// Where an expression is evaluated, which decides the functions it may
/// use.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Place {
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

/// Refuse the names and expressions of `d` as the store refuses them, in
/// its order.
pub(super) fn declaration(st: &State, d: &Declaration) -> Result<()> {
    let set = |n: &str| st.object_sets.iter().find(|s| s.name == n);
    match d {
        Declaration::Type(t) => {
            name("type", t.name.as_str())?;
            let owner = format!("type `{}`", t.name);
            for f in &t.fields {
                field(&owner, f)?;
            }
            for r in &t.relations {
                name("relation", r.name.as_str())?;
                for f in &r.fields {
                    field(&format!("{owner}, relation `{}`", r.name), f)?;
                }
            }
        }
        Declaration::ValueType(v) => {
            name("value type", v.name.as_str())?;
            if let Some(c) = &v.constraint {
                let at = format!("value type `{}`, constraint", v.name);
                condition(c, Place::Record, &at)?;
            }
        }
        Declaration::Verb(v) => {
            name("verb", v.name.as_str())?;
            for f in &v.payload {
                let at = format!("verb `{}`, payload", v.name);
                field(&at, f)?;
                computed_here(&at, f)?;
            }
            if let Some(r) = &v.requires {
                condition(r, Place::Record, &format!("verb `{}`, requires", v.name))?;
            }
            match &v.effect {
                VerbEffect::Write { set } => {
                    for a in set {
                        let at = format!("verb `{}`, `{}`", v.name, a.field);
                        condition(&a.value, Place::Record, &at)?;
                    }
                }
                VerbEffect::External { executor } => name("executor", executor)?,
            }
        }
        Declaration::Trigger(t) => {
            name("trigger", t.name.as_str())?;
            if let Some(w) = &t.when {
                let at = format!("trigger `{}`", t.name);
                filter(&set, &t.on, w, Place::Guard, &at)?;
            }
        }
        Declaration::ObjectSet(s) => {
            name("object set", &s.name)?;
            let at = format!("object set `{}`", s.name);
            let named = |n: &str| Some(s).filter(|s| s.name == n).or_else(|| set(n));
            filter(&named, &s.of, &s.filter, Place::Record, &at)?;
        }
        Declaration::Function(f) => {
            name("function", &f.name)?;
            name("backing", &f.backing)?;
            for input in &f.inputs {
                let at = format!("function `{}`, input", f.name);
                field(&at, input)?;
                computed_here(&at, input)?;
            }
            if let ("expr", Some(e)) = (f.backing.as_str(), &f.expr) {
                condition(e, Place::Record, &format!("function `{}`", f.name))?;
            }
        }
    }
    Ok(())
}

/// A field's name, and the expression it is derived from, at `owner`.
fn field(owner: &str, f: &FieldDecl) -> Result<()> {
    name("field", f.name.as_str())?;
    if let Some(d) = &f.derived {
        condition(d, Place::Write, &format!("{owner}, field `{}`", f.name))?;
    }
    Ok(())
}

/// As the store: `computed_at` on a field no record holds.
fn computed_here(at: &str, f: &FieldDecl) -> Result<()> {
    match f.computed_at {
        Some(_) => {
            let message = format!(
                "{at}, field `{}`: only a field of a record says `computed_at`",
                f.name
            );
            Err(super::on(
                &f.name,
                lakelands_api::Refusal::Invalid { message },
            ))
        }
        None => Ok(()),
    }
}

/// Refuse a name of this kind that cannot be declared.
pub(super) fn name(what: &str, n: &str) -> Result<()> {
    fault(what, n).map_or(Ok(()), |m| Err(invalid(m)))
}

/// Why a name of this kind cannot be declared, if it cannot. `what` is
/// the kind of thing named (`type`, `field`, …), for the message.
pub(crate) fn fault(what: &str, name: &str) -> Option<String> {
    let identifier = |n: &str| {
        let mut chars = n.chars();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    };
    let (ok, rule) = if what == "type" {
        let joined = !name.is_empty() && name.split('.').all(identifier);
        (joined, "ASCII identifiers joined by `.`")
    } else {
        let rule = "an ASCII identifier: a letter or `_`, then letters, digits or `_`";
        (identifier(name), rule)
    };
    if ok {
        return None;
    }
    Some(if name.is_empty() {
        format!("a {what} name must not be empty")
    } else {
        format!("{what} name `{}` is not {rule}", name.escape_debug())
    })
}

/// A condition in its typed form, checked for its place; a set is named
/// only where its filter is put in the name's place.
pub(super) fn condition(c: &Condition, place: Place, at: &str) -> Result<Expr> {
    let e = typed(c, place, at)?;
    if calls_in_set(&e) {
        return Err(invalid(format!(
            "{at}: in_set() has meaning in an object set's `where`, a trigger's `when` and a \
             read's `where`"
        )));
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
fn typed(c: &Condition, place: Place, at: &str) -> Result<Expr> {
    let e = match c {
        Condition::Expr(e) => e.clone(),
        Condition::Text(t) => {
            parse(t).map_err(|e| invalid(format!("{at}: `{t}` does not parse: {e}")))?
        }
    };
    check(&e, place).map_err(|e| invalid(format!("{at}: {e}")))?;
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
    place: Place,
    at: &str,
) -> Result<Expr> {
    let e = typed(c, place, at)?;
    let named = |name: &str| match set(name) {
        Some(s) if &s.of == ty => typed(&s.filter, Place::Record, at).map_err(|e| e.to_string()),
        Some(s) => Err(format!(
            "in_set('{name}') names a set of `{}`, not of `{ty}`",
            s.of
        )),
        None => Err(format!(
            "in_set('{name}') names an object set that is not declared"
        )),
    };
    lakelands_api::expr::expand(&e, &named).map_err(|e| invalid(format!("{at}: {e}")))
}

/// The units `date_bucket` knows.
const BUCKET_UNITS: [&str; 5] = ["day", "week", "month", "quarter", "year"];

/// An expression checked before it is taken: every call has a count of
/// arguments its function takes, a function that names a field is given
/// one, `setting`, `param` and `in_set` a quoted name, numbers are numbers
/// the store's evaluator holds exactly, and `changed`, `previous`,
/// `retired`, `param` and the aggregates appear only where they have
/// meaning.
fn check(e: &Expr, place: Place) -> std::result::Result<(), String> {
    match e {
        Expr::Lit {
            value: Literal::Number(n),
        } if !exact(n) => Err(format!(
            "`{n}` is not a number this evaluator can hold exactly"
        )),
        Expr::Lit { .. } => Ok(()),
        Expr::Path { path } if path.is_empty() => Err("a path names at least one field".into()),
        Expr::Path { .. } => Ok(()),
        Expr::Cmp { lhs, rhs, .. } | Expr::Arith { lhs, rhs, .. } => {
            check(lhs, place)?;
            check(rhs, place)
        }
        Expr::Neg { arg } | Expr::Not { arg } | Expr::IsNull { arg } => check(arg, place),
        Expr::And { args } | Expr::Or { args } => args.iter().try_for_each(|a| check(a, place)),
        Expr::In { arg, list } => {
            if list.is_empty() {
                return Err("`in` needs at least one value".into());
            }
            check(arg, place)?;
            list.iter().try_for_each(|a| check(a, place))
        }
        Expr::Call { func, args } => {
            call(*func, args, place)?;
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

/// A call of `func` on `args` checked for its arguments and its place.
fn call(func: Func, args: &[Expr], place: Place) -> std::result::Result<(), String> {
    let name = func_name(func);
    let (min, max) = arity(func);
    if args.len() < min || max.is_some_and(|m| args.len() > m) {
        return Err(format!("{name}() does not take {} argument(s)", args.len()));
    }
    let field = matches!(args.first(), Some(Expr::Path { path }) if path.len() == 1);
    match func {
        Func::KnownAt | Func::SeenAt | Func::Previous | Func::Changed if !field => {
            Err(format!("{name}() takes one field name"))
        }
        Func::Setting | Func::Param | Func::InSet if quoted(args.first()).is_none() => {
            Err(format!("{name}() takes a quoted name"))
        }
        // A calendar unit, quoted, takes no origin; a width is a duration
        // above zero, with the origin its windows are counted from.
        Func::DateBucket => {
            let unit = quoted(args.get(1));
            let width = matches!(args.get(1),
                Some(Expr::Lit { value: Literal::Duration(w) }) if *w > 0);
            if let Some(u) = unit.filter(|u| !BUCKET_UNITS.contains(&u.as_str())) {
                return Err(format!(
                    "date_bucket() knows no unit '{u}'; the units are 'day', 'week', 'month', \
                     'quarter' and 'year', and a width is written unquoted, as in \
                     date_bucket(t, 30d, origin)"
                ));
            }
            if (unit.is_some(), width) != (args.len() == 2, args.len() == 3) {
                return Err(
                    "date_bucket() takes a calendar unit quoted ('day', 'week', \
                     'month', 'quarter' or 'year') and no origin, or a width above zero and \
                     the origin its windows are counted from, as in date_bucket(t, 30d, origin)"
                        .into(),
                );
            }
            Ok(())
        }
        Func::Previous | Func::Changed if !matches!(place, Place::Write | Place::Guard) => {
            Err(format!("{name}() has meaning only in a guard over a write"))
        }
        Func::Retired if place != Place::Guard => {
            Err("retired() has meaning only in a trigger's guard".into())
        }
        f if f.is_aggregate() && place != Place::Group => Err(format!(
            "{name}() folds a set of records and has no meaning here"
        )),
        Func::Param if !matches!(place, Place::View | Place::Group) => Err(
            "param() is read only in a view's `where` and `measures`, where the view declares \
             its parameters"
                .into(),
        ),
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
