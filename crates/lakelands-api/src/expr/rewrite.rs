//! An expression rewritten: one node put in another's place.

use super::{Expr, Func, Literal};

/// `e` with every `in_set('name')` replaced by that set's filter, itself
/// expanded: a set is its filter, bound when the condition is read. `set`
/// answers a set's filter, or why the set cannot be named here. A set that
/// names itself through others is refused, naming them.
pub fn expand(e: &Expr, set: &dyn Fn(&str) -> Result<Expr, String>) -> Result<Expr, String> {
    type Named<'a> = &'a dyn Fn(&str) -> Result<Expr, String>;
    fn within(e: &Expr, set: Named<'_>, path: &[&str]) -> Result<Expr, String> {
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
            let inner = if path.contains(&name.as_str()) {
                Err(format!(
                    "in_set('{name}') names a set that names it in turn, through {}",
                    path.join(", ")
                ))
            } else {
                set(name).and_then(|f| within(&f, set, &[path, &[name.as_str()]].concat()))
            };
            Some(inner.unwrap_or_else(|why| {
                failed.replace(Some(why));
                x.clone()
            }))
        });
        failed.into_inner().map_or(Ok(out), Err)
    }
    within(e, set, &[])
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
