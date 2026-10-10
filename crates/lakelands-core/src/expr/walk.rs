//! Walks over the expressions a declaration keeps: whether any of them
//! holds a node of a kind, wherever it sits.

use lakelands_api::expr::{Condition, Expr, Func};
use lakelands_api::ontology::VerbEffect;
use lakelands_api::write::Declaration;

/// Whether an expression the log keeps of a declaration holds, anywhere,
/// a node `test` accepts: a field's derivation, a view's `where` and
/// measures, a value type's constraint, a verb's `requires` and
/// assignments, a trigger's `when`, an object set's filter, and an
/// expression function's body. A condition in text form that does not
/// parse holds nothing.
pub fn declares(declaration: &Declaration, test: &dyn Fn(&Expr) -> bool) -> bool {
    let mut conditions: Vec<&Condition> = Vec::new();
    match declaration {
        Declaration::Type(t) => {
            conditions.extend(t.fields.iter().filter_map(|f| f.derived.as_ref()));
            if let Some(v) = &t.view {
                conditions.extend(v.r#where.as_ref());
                conditions.extend(v.measures.iter().map(|m| &m.expr));
            }
        }
        Declaration::ValueType(v) => conditions.extend(v.constraint.as_ref()),
        Declaration::Verb(v) => {
            conditions.extend(v.requires.as_ref());
            if let VerbEffect::Write { set } = &v.effect {
                conditions.extend(set.iter().map(|a| &a.value));
            }
        }
        Declaration::Trigger(t) => conditions.extend(t.when.as_ref()),
        Declaration::ObjectSet(s) => conditions.push(&s.filter),
        Declaration::Function(f) => conditions.extend(f.expr.as_ref()),
    }
    (conditions.into_iter()).any(|c| crate::describe::parsed(c).is_some_and(|e| anywhere(&e, test)))
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
