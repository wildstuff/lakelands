//! The one expression grammar: what an expression means (`eval`). The
//! typed form and its text form (`parse`, `print`) live in
//! `lakelands_api::expr`, so a host parses a condition exactly as the
//! store does; text and typed forms reach the same evaluator.

#[doc(hidden)]
pub mod aggregate;
#[doc(hidden)]
pub mod eval;
#[doc(hidden)]
pub mod iregexp;
#[doc(hidden)]
pub mod order;
#[doc(hidden)]
pub mod walk;

pub use eval::{Place, check};
#[doc(hidden)]
pub use eval::{RecordScope, Scope, Val, WriteScope, bind_params, eval, holds, params_read};
pub use lakelands_api::expr::parse;
#[doc(hidden)]
pub use lakelands_api::expr::{ParseError, arity, children, func_name, print};
#[doc(hidden)]
pub use walk::{calls, declares, names_text};
