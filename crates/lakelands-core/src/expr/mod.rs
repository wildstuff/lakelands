//! The one expression grammar: its text form (`parse`, `print`) and what an
//! expression means (`eval`). The typed form is `lakelands_api::expr::Expr`;
//! text and typed forms reach the same evaluator.

pub mod aggregate;
pub mod eval;
pub mod iregexp;
pub mod order;
mod parse;

pub use eval::{EvalError, Place, RecordScope, Scope, Val, WriteScope, check, eval, holds};
pub use parse::{ParseError, arity, func_name, parse, print};
