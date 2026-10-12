//! Walks over the expressions a declaration keeps: whether any of them
//! holds a node of a kind, wherever it sits.

use lakelands_api::expr::{Condition, Expr, Func};
use lakelands_api::write::Declaration;

/// Whether an expression the log keeps of a declaration holds, anywhere,
/// a node `test` accepts: every member [`Declaration::conditions`] lists.
/// A condition in text form that does not parse holds nothing.
pub fn declares(declaration: &Declaration, test: &dyn Fn(&Expr) -> bool) -> bool {
    let mut conditions = declaration.conditions().into_iter();
    conditions.any(|(_, c)| crate::describe::parsed(c).is_some_and(|e| anywhere(&e, test)))
}

/// Whether a condition holds, anywhere, a text literal `text` accepts.
pub fn names_text(c: &Condition, text: &dyn Fn(&str) -> bool) -> bool {
    use lakelands_api::expr::Literal;
    let lit = |e: &Expr| matches!(e, Expr::Lit { value: Literal::Text(t) } if text(t));
    crate::describe::parsed(c).is_some_and(|e| anywhere(&e, &lit))
}

/// Whether an expression calls, anywhere, a function `f` accepts.
pub fn calls(e: &Expr, f: &dyn Fn(Func) -> bool) -> bool {
    anywhere(e, &|e| matches!(e, Expr::Call { func, .. } if f(*func)))
}

fn anywhere(e: &Expr, test: &dyn Fn(&Expr) -> bool) -> bool {
    test(e) || super::children(e).any(|c| anywhere(c, test))
}

#[cfg(test)]
mod tests;
