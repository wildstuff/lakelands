//! The store: one lake over an engine, the log as the truth, the
//! projection folded from it, and the doors every caller uses.
//!
//! `open` a lake at a [`Location`](lakelands_api::Location) with the
//! host's [`Seams`](lakelands_api::seam::Seams); the answer is a
//! [`LakeStore`](lakelands_api::LakeStore) whose `catalog` gives the doors.
//! Engine work is synchronous (`docs/decisions/019-the-contract-is-object-safe.md`)
//! and runs through exactly one blocking hand-off, in `exec`.
//!
//! A host builds on `open`, `import` and the [`Imported`] it answers,
//! `written_in` and [`ExprBacking`]. The modules are the store's own; those
//! the workspace's tests reach are `#[doc(hidden)]`, and a release changes
//! them without notice.

#![forbid(unsafe_code)]

mod aging;
#[doc(hidden)]
pub mod backed;
mod backing;
mod declare;
mod doors;
mod exec;
mod forget;
mod import;
mod ingest;
mod kept;
#[doc(hidden)]
pub mod lake;
#[doc(hidden)]
pub mod log;
mod ontology;
mod purge;
mod reads;
mod record;
mod rekey;
mod sealing;
#[doc(hidden)]
pub mod writes;

pub use backing::ExprBacking;
pub use doors::{import, open, written_in};
pub use import::Imported;
#[doc(hidden)]
pub use lakelands_core::export;
