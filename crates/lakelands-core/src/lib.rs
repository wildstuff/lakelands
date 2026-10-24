//! The deterministic heart of a lake.
//!
//! The ontology that governs a lake, the one expression grammar it is
//! written in, the canonical bytes every hash is taken over, and the rules
//! that turn declarations and writes into state. Everything here must
//! reproduce byte-identically from the log; whatever needs the outside
//! world lives behind a seam declared in `lakelands-api`
//! (`docs/decisions/002-determinism-is-the-boundary.md`).
//!
//! A host builds on `value::check`, `derive::reads`, `expr::{check, parse,
//! Place}`, `log::record_key` and `names::is_identifier`, and on nothing
//! else here. Every other public item is `#[doc(hidden)]`: the workspace's
//! other crates and the conformance suite share it, and a release changes
//! it without notice.

#![forbid(unsafe_code)]

#[doc(hidden)]
pub use lakelands_api::canonical;

#[doc(hidden)]
pub mod bundle;
#[doc(hidden)]
pub mod conformance;
pub mod derive;
#[doc(hidden)]
pub mod describe;
#[doc(hidden)]
pub mod engines;
#[doc(hidden)]
pub mod export;
pub mod expr;
#[doc(hidden)]
pub mod feed;
#[doc(hidden)]
pub mod graph;
#[doc(hidden)]
pub mod links;
pub mod log;
pub mod names;
pub(crate) mod rights;
#[doc(hidden)]
pub mod shape;
#[doc(hidden)]
pub mod sight;
mod stem;
#[doc(hidden)]
pub mod valid_time;
pub mod value;

/// The rules of [`rights`] that judge no principal, which a door applies
/// beside its sight ([`sight::Sight`]): a system type's name, a view where a
/// record is written, a source's write over another hand's. Every other
/// rule is asked of a sight.
#[doc(hidden)]
pub use rights::{check_admitted, is_system, not_a_view};

use lakelands_api::error::{Detail, Doing, Flaw, Part, Situation};
use lakelands_api::{Error, Position};

/// Stored or exported events, or the files that hold them, are damaged:
/// `part`, at `position` if one, by `flaw`.
#[doc(hidden)]
pub fn damaged(part: Part, position: Option<Position>, flaw: Flaw) -> Error {
    Error::unavailable(Situation::Damaged {
        part,
        position,
        flaw,
    })
}

/// The storage failed `doing` to `part`, in the operating system's words.
#[doc(hidden)]
pub fn io_failed(doing: Doing, part: Part, e: &std::io::Error) -> Error {
    let detail = Detail::io(e);
    Error::unavailable(Situation::Storage {
        doing,
        part,
        detail,
    })
}

/// Locks a mutex, carrying on past a panic in another holder: a caller
/// locks only state every holder changes whole or not at all.
#[doc(hidden)]
pub fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
