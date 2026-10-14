//! An expression rewritten: one node put in another's place.

use super::{Expr, Func, Literal};
use crate::error::{Case, Said};

/// `e` with every `in_set('name')` replaced by that set's filter, itself
/// expanded: a set is its filter, bound when the condition is read. `set`
/// answers a set's filter, or why the set cannot be named here. A set that
/// names itself through others is refused, naming them.
pub fn expand(e: &Expr, set: &dyn Fn(&str) -> Result<Expr, Said>) -> Result<Expr, Said> {
    expand_kept(e, set, &|_| false)
}

/// [`expand`], but each set `kept` names stays `in_set('name')`, answered
/// by its kept membership where the condition is evaluated: a read's
/// `in_set` of a set whose `where` reads across an edge
/// (`docs/decisions/055-a-path-steps-through-a-reference.md`).
pub fn expand_kept(
    e: &Expr,
    set: &dyn Fn(&str) -> Result<Expr, Said>,
    kept: &dyn Fn(&str) -> bool,
) -> Result<Expr, Said> {
    type Named<'a> = &'a dyn Fn(&str) -> Result<Expr, Said>;
    type Kept<'a> = &'a dyn Fn(&str) -> bool;
    fn within(e: &Expr, (set, kept): (Named<'_>, Kept<'_>), path: &[&str]) -> Result<Expr, Said> {
        let failed = std::cell::RefCell::new(None);
        let out = rewrite(e, &|x| {
            let Expr::Call {
                func: Func::InSet,
                args,
            } = x
            else {
                return None;
            };
            let Some(Expr::Lit {
                value: Literal::Text(name),
            }) = args.first()
            else {
                return None;
            };
            if kept(name) {
                return Some(x.clone());
            }
            let inner = if path.contains(&name.as_str()) {
                let through = path.iter().collect();
                let set = name.clone();
                Err(Case::SetCycle { set, through }.into())
            } else {
                let path = [path, &[name.as_str()]].concat();
                set(name).and_then(|f| within(&f, (set, kept), &path))
            };
            Some(inner.unwrap_or_else(|why| {
                failed.replace(Some(why));
                x.clone()
            }))
        });
        failed.into_inner().map_or(Ok(out), Err)
    }
    within(e, (set, kept), &[])
}

/// `e` with each node `put` answers for replaced by its answer, and every
/// other node rebuilt over its rewritten children.
pub fn rewrite(e: &Expr, put: &dyn Fn(&Expr) -> Option<Expr>) -> Expr {
    if let Some(x) = put(e) {
        return x;
    }
    let b = |x: &Expr| Box::new(rewrite(x, put));
    let all = |xs: &[Expr]| xs.iter().map(|x| rewrite(x, put)).collect();
    match e {
        Expr::Call { func, args } => Expr::Call {
            func: *func,
            args: all(args),
        },
        Expr::Cmp { cmp, lhs, rhs } => Expr::Cmp {
            cmp: *cmp,
            lhs: b(lhs),
            rhs: b(rhs),
        },
        Expr::Arith { arith, lhs, rhs } => Expr::Arith {
            arith: *arith,
            lhs: b(lhs),
            rhs: b(rhs),
        },
        Expr::Neg { arg } => Expr::Neg { arg: b(arg) },
        Expr::Not { arg } => Expr::Not { arg: b(arg) },
        Expr::IsNull { arg } => Expr::IsNull { arg: b(arg) },
        Expr::And { args } => Expr::And { args: all(args) },
        Expr::Or { args } => Expr::Or { args: all(args) },
        Expr::In { arg, list } => Expr::In {
            arg: b(arg),
            list: all(list),
        },
        Expr::Lit { .. } | Expr::Path { .. } => e.clone(),
    }
}
