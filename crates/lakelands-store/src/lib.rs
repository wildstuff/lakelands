//! The store: one lake over an engine, the log as the truth, the
//! projection folded from it, and the doors every caller uses.
//!
//! `open` a lake at a [`Location`](lakelands_api::Location) with the
//! host's [`Seams`](lakelands_api::seam::Seams); the answer is a
//! [`LakeStore`](lakelands_api::LakeStore) whose `catalog` gives the doors.
//! Engine work is synchronous (`docs/decisions/019-the-contract-is-object-safe.md`)
//! and runs through exactly one blocking hand-off, in [`exec`].

#![forbid(unsafe_code)]

pub mod aging;
pub mod backed;
pub mod backing;
pub mod declare;
pub mod doors;
pub mod exec;
pub mod import;
pub mod ingest;
mod kept;
pub mod lake;
pub mod log;
pub mod ontology;
pub mod purge;
pub mod reads;
pub mod record;
pub mod rekey;
pub mod writes;

pub use backing::ExprBacking;
pub use doors::{import, open};
pub use lakelands_core::export;
