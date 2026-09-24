//! The in-memory engine: a lake held entirely in memory, for tests and
//! for short-lived lakes. It honours the same conformance suite as every
//! other engine.
//!
//! It implements the engine seam of `lakelands-api` (`docs/seams.md`
//! §Engine) and nothing more: it keeps the log's bytes, the writer lease
//! and the states the fold hands it, and answers reads over them. It
//! never folds, never canonicalises and never decides what an event
//! means; the store does.
//!
//! # One lake, many handles
//!
//! A lake held in memory is found by its [`LakeId`] in a registry that
//! lives as long as the process. Every [`MemoryOpener::open`] of the same
//! lake returns a handle onto the same log, lease and projection, so a
//! writer and any number of readers in one process see one lake, and a
//! second writer is refused the lease exactly as it would be on an engine
//! that keeps its lake in files. A lake is gone when the process ends;
//! nothing ever removes it earlier.
//!
//! # Kept history
//!
//! Every state the fold puts is kept beside the states it supersedes,
//! each valid from a position. A view at a position reads, for every
//! record, declaration, proposal and membership, the latest state valid
//! at or before it. Nothing is replayed to answer an earlier view.
//!
//! # Filters and time
//!
//! A scan's filter is evaluated with the one
//! evaluator of `lakelands-core` over the record's own fields. The engine
//! knows neither the lake's settings nor the clock, so the store hands it
//! only filters it has already made independent of both (a `setting(…)`
//! resolved to its value, `now()` to an instant). A filter that cannot be
//! evaluated for a record does not match it.

#![forbid(unsafe_code)]

mod handle;
mod log;
mod order;
mod projection;
mod vectors;
mod view;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode};
use lakelands_api::error::{Error, Refusal, Result};
use lakelands_api::id::LakeId;
use lakelands_api::location::{EngineConfig, Location};

pub use handle::MemoryLake;

/// The name the engine answers to, in [`EngineConfig::Memory`] and in
/// [`LakeEngine::name`].
pub const NAME: &str = "memory";

/// Everything one lake holds: its log with the lease, its projection and
/// its vector index.
#[derive(Default)]
pub(crate) struct Lake {
    pub(crate) log: log::Log,
    pub(crate) projection: projection::Projection,
    pub(crate) vectors: vectors::Vectors,
}

/// The process-wide registry of lakes held in memory.
type Registry = Mutex<BTreeMap<LakeId, Arc<Mutex<Lake>>>>;

fn registry() -> &'static Registry {
    static LAKES: OnceLock<Registry> = OnceLock::new();
    LAKES.get_or_init(Registry::default)
}

/// Locks a mutex, carrying on past a panic in another holder: every
/// change to a lake is made whole or not at all under one lock, so a
/// holder that panicked left no half-made change behind.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// An error for a lake that cannot answer now.
pub(crate) fn unavailable(message: impl Into<String>) -> Error {
    Error::Unavailable {
        message: message.into(),
    }
}

/// An error for a request that does not fit the seam's contract.
pub(crate) fn invalid(message: impl Into<String>) -> Error {
    Error::Refused {
        field: None,
        reason: Refusal::Invalid {
            message: message.into(),
        },
    }
}

/// Finds the in-memory engine by name and opens lakes held in memory.
#[derive(Clone, Copy, Debug, Default)]
pub struct MemoryOpener;

impl EngineOpener for MemoryOpener {
    fn name(&self) -> &str {
        NAME
    }

    /// Opens the lake `location` names. `Create` makes it when it does not
    /// exist yet; `Read` and `Write` on a lake this process never created
    /// answer `Unavailable`, since there is nothing to open.
    fn open(&self, location: &Location, mode: OpenMode) -> Result<Box<dyn LakeEngine>> {
        if !matches!(location.engine, EngineConfig::Memory) {
            return Err(invalid(format!(
                "the {NAME} engine opens only memory locations, and lake `{}` is configured \
                 for another engine",
                location.lake
            )));
        }
        let mut lakes = lock(registry());
        let shared = match (lakes.get(&location.lake), mode) {
            (Some(lake), _) => Arc::clone(lake),
            (None, OpenMode::Create) => Arc::clone(
                lakes
                    .entry(location.lake.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(Lake::default()))),
            ),
            (None, OpenMode::Read | OpenMode::Write) => {
                return Err(unavailable(format!(
                    "there is no lake `{}` in this process's memory; open it with `create` \
                     to make it",
                    location.lake
                )));
            }
        };
        Ok(Box::new(MemoryLake::new(
            location.lake.clone(),
            mode,
            shared,
        )))
    }
}

#[cfg(test)]
mod tests;
