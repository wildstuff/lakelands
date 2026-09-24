//! The deterministic heart of a lake.
//!
//! The ontology that governs a lake, the one expression grammar it is
//! written in, the canonical bytes every hash is taken over, and the rules
//! that turn declarations and writes into state. Everything here must
//! reproduce byte-identically from the log; whatever needs the outside
//! world lives behind a seam declared in `lakelands-api`
//! (`docs/decisions/002-determinism-is-the-boundary.md`).

#![forbid(unsafe_code)]

pub mod canonical;

pub mod bundle;
pub mod conformance;
pub mod derive;
pub mod describe;
pub mod engines;
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
