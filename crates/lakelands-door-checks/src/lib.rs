//! The door checks: what a door can observe of a lake, written once.
//!
//! [`checks`] holds the door-facing behaviours of the contract as async
//! functions over an `Arc<dyn LakeStore>`, with the fixture ontology and
//! its cast, and [`door_checks!`] expands to one `#[test]` per check for
//! whatever lake a caller makes. This crate runs them on the store over
//! memory and SQLite (`tests/checks_on_the_store.rs`); the client runs
//! them again over a server, so every check is held in process and
//! across the network door. [`stream`] is the two stream helpers a
//! caller of `ingest` needs.
//!
//! It is a crate of its own and a dev-dependency of the door crates,
//! never a cargo feature: features are one per engine. It holds no lake.
//! A door crate's tests open the store on the memory engine.
//!
//! A refusal's sentence is its case's, from the one catalog: a check
//! asserts a refusal's kind, its field and the case it says, never its
//! words.

#![forbid(unsafe_code)]

pub mod checks;
pub mod stream;
