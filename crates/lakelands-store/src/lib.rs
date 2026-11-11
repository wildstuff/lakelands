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
//! and [`ExprBacking`]. The modules are the store's own; those
//! the workspace's tests reach are `#[doc(hidden)]`, and a release changes
//! them without notice.

#![forbid(unsafe_code)]

mod aging;
#[doc(hidden)]
pub mod backed;
mod backing;
#[doc(hidden)]
pub mod batch;
mod build;
mod choose;
mod declare;
mod doors;
mod erase;
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
mod parts;
mod purge;
mod quarantine;
mod reads;
mod record;
mod rekey;
mod remove;
mod restate;
mod road;
mod sealing;
mod thin;
#[doc(hidden)]
pub mod writes;

pub use backing::ExprBacking;
pub use doors::{
    REACH_POLL, Standing, build_ahead, discard_ahead, import, import_through, open, room_ahead,
    standby,
};
pub use import::{Imported, check_events};
#[doc(hidden)]
pub use lakelands_core::export;
