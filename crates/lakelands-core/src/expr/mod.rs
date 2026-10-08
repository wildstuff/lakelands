//! The one expression grammar: what an expression means (`eval`). The
//! typed form and its text form (`parse`, `print`) live in
//! `lakelands_api::expr`, so a host parses a condition exactly as the
//! store does; text and typed forms reach the same evaluator.

pub mod aggregate;
pub mod eval;
pub mod iregexp;
pub mod order;

pub use eval::{
    Place, RecordScope, Scope, Val, WriteScope, bind_params, check, eval, holds, params_read,
};
pub use lakelands_api::expr::{ParseError, arity, children, func_name, parse, print};
