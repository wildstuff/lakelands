//! The one expression grammar: what an expression means (`eval`). The
//! typed form and its text form (`parse`, `print`) live in
//! `lakelands_api::expr`, so a host parses a condition exactly as the
//! store does; text and typed forms reach the same evaluator.

pub mod aggregate;
pub mod eval;
pub mod iregexp;
pub mod order;

pub use eval::{
    EvalError, Place, RecordScope, Scope, Val, WriteScope, bind_params, check, eval, holds,
    params_read,
};
pub use lakelands_api::expr::{ParseError, arity, func_name, parse, print};

use lakelands_api::expr::Expr;

/// The expressions directly inside `e`, left to right.
pub fn children(e: &Expr) -> impl Iterator<Item = &Expr> {
    let (pair, list): ([Option<&Expr>; 2], &[Expr]) = match e {
        Expr::Lit { .. } | Expr::Path { .. } => ([None, None], &[]),
        Expr::Cmp { lhs, rhs, .. } | Expr::Arith { lhs, rhs, .. } => ([Some(lhs), Some(rhs)], &[]),
        Expr::Neg { arg } | Expr::Not { arg } | Expr::IsNull { arg } => ([Some(arg), None], &[]),
        Expr::And { args } | Expr::Or { args } | Expr::Call { args, .. } => ([None, None], args),
        Expr::In { arg, list } => ([Some(arg), None], list),
    };
    pair.into_iter().flatten().chain(list)
}
