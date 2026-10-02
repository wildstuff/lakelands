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
pub mod graph;
pub mod links;
pub mod log;
pub mod names;
pub mod rights;
pub mod shape;
pub mod stem;
pub mod valid_time;
pub mod value;
pub mod versions;

use lakelands_api::{Error, Refusal};

/// A refusal of what the caller asked for as not well-formed, naming no
/// field.
pub fn invalid(message: impl Into<String>) -> Error {
    Error::Refused {
        field: None,
        reason: Refusal::Invalid {
            message: message.into(),
        },
    }
}

/// The lake cannot answer now; `message` says why.
pub fn unavailable(message: impl Into<String>) -> Error {
    Error::Unavailable {
        message: message.into(),
    }
}
