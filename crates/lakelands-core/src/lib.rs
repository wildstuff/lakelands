//! The deterministic heart of a lake.
//!
//! The ontology that governs a lake, the one expression grammar it is
//! written in, the canonical bytes every hash is taken over, and the rules
//! that turn declarations and writes into state. Everything here must
//! reproduce byte-identically from the log; whatever needs the outside
//! world lives behind a seam declared in `lakelands-api`
//! (`docs/decisions/002-determinism-is-the-boundary.md`).

#![forbid(unsafe_code)]

pub use lakelands_api::canonical;

pub mod bundle;
pub mod conformance;
pub mod derive;
pub mod describe;
pub mod engines;
pub mod export;
pub mod expr;
pub mod feed;
pub mod graph;
pub mod links;
pub mod log;
pub mod names;
pub mod rights;
pub mod shape;
pub mod sight;
pub mod stem;
pub mod valid_time;
pub mod value;

use lakelands_api::error::{Detail, Doing, Flaw, Part, Said, Situation};
use lakelands_api::{Error, Position};

/// A refusal of what the caller asked for as not well-formed, naming no
/// field.
pub fn invalid(said: impl Into<Said>) -> Error {
    Error::invalid(said)
}

/// Stored or exported events, or the files that hold them, are damaged:
/// `part`, at `position` if one, by `flaw`.
pub fn damaged(part: Part, position: Option<Position>, flaw: Flaw) -> Error {
    Error::unavailable(Situation::Damaged {
        part,
        position,
        flaw,
    })
}

/// The storage failed `doing` to `part`, in the operating system's words.
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
pub fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
