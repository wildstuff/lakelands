//! A module whose evaluator is hidden and re-exports two of its items.

#[doc(hidden)]
pub mod eval;

pub use eval::{Place, check};
#[doc(hidden)]
pub use eval::eval;
