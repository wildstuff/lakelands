//! An in-memory `Catalog` for the doors' tests.
//!
//! The CLI and the server are tested against this crate before the real
//! store exists, and keep being tested against it afterwards, so that a
//! door's behaviour is pinned to the contract rather than to one store.
//! It is a crate of its own and a dev-dependency of the door crates —
//! never a cargo feature: features are one per engine.
//!
//! Empty until the contract exists.
