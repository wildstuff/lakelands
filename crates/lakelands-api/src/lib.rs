//! The contract every other Lakelands crate builds against.
//!
//! This crate holds the types a caller or a host sees — the principal a
//! call is made as, the location a lake is configured through, the
//! position every reply is read at — together with the catalog of doors
//! an agent uses, the store facade and the seam traits an engine or a
//! host implements. It has no engine and performs no I/O.
//!
//! The one generator of LLM-facing tool names lives here, so that every
//! crate that exposes a tool takes its name from the same place.
//!
//! Empty until the contract step fills it.
