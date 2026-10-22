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
//! A lake held in memory is found by its [`LakeId`] in the registry of the
//! [`MemoryOpener`] that made it, and of its clones. Every open of the
//! same lake through that opener returns a handle onto the same log, lease
//! and projection, so a writer and any number of readers see one lake, and
//! a second writer is refused the lease exactly as it would be on an engine
//! that keeps its lake in files. A lake lives as long as its opener; there
//! is no state beyond the openers, so two openers never share a lake, and
//! every test with an opener of its own is isolated from every other.
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
mod projection;
mod vectors;
mod view;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use lakelands_api::engine::{CanonicalEvent, EngineOpener, LakeEngine, OpenMode};
use lakelands_api::error::{Case, Error, Result, Situation};
use lakelands_api::id::LakeId;
use lakelands_api::location::{EngineConfig, Location};

use handle::MemoryLake;

/// The name the engine answers to, in [`EngineConfig::Memory`] and in
/// [`LakeEngine::name`].
pub(crate) const NAME: &str = "memory";

/// The events a rebuild folds a second, as measured: 50 000 events in 4.2 s, a release
/// build on the development machine (lakelands#1042).
pub(crate) const REBUILD_RATE: u64 = 12_000;

/// Everything one lake holds: its log with the lease, its projection and
/// its vector index.
#[derive(Default)]
pub(crate) struct Lake {
    pub(crate) log: log::Log,
    pub(crate) projection: projection::Projection,
    pub(crate) vectors: vectors::Vectors,
}

/// The lakes an opener holds in memory, by name.
type Registry = Mutex<BTreeMap<LakeId, Arc<Mutex<Lake>>>>;

/// Locks a mutex, carrying on past a panic in another holder: every
/// change to a lake is made whole or not at all under one lock, so a
/// holder that panicked left no half-made change behind.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Finds the in-memory engine by name and opens lakes held in memory. The
/// lakes are this opener's: its clones share them, another opener holds
/// its own.
#[derive(Clone, Default)]
pub struct MemoryOpener {
    lakes: Arc<Registry>,
}

impl MemoryOpener {
    /// An opener holding no lake yet.
    pub fn new() -> MemoryOpener {
        MemoryOpener::default()
    }
}

impl std::fmt::Debug for MemoryOpener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryOpener")
            .field("lakes", &lock(&self.lakes).len())
            .finish()
    }
}

impl EngineOpener for MemoryOpener {
    fn name(&self) -> &str {
        NAME
    }

    fn log_ends(&self, location: &Location) -> Result<Option<(CanonicalEvent, CanonicalEvent)>> {
        let lakes = lock(&self.lakes);
        Ok((lakes.get(&location.lake)).and_then(|lake| lock(lake).log.ends()))
    }

    /// Opens the lake `location` names. `Create` makes it when it does not
    /// exist yet; `Read` and `Write` on a lake this process never created
    /// answer `Unavailable`, since there is nothing to open.
    fn open(&self, location: &Location, mode: OpenMode) -> Result<Box<dyn LakeEngine>> {
        if !matches!(location.engine, EngineConfig::Memory) {
            let lake = Box::new(location.lake.clone());
            let (engine, location) = (NAME.to_owned(), "memory".to_owned());
            return Err(Error::invalid(Case::EngineOtherLocation {
                engine,
                location,
                lake,
            }));
        }
        let mut lakes = lock(&self.lakes);
        let shared = match (lakes.get(&location.lake), mode) {
            (Some(lake), _) => Arc::clone(lake),
            (None, OpenMode::Create) => Arc::clone(
                lakes
                    .entry(location.lake.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(Lake::default()))),
            ),
            (None, OpenMode::Read | OpenMode::Write | OpenMode::Rebuild) => {
                let lake = location.lake.to_string();
                let schema = None;
                return Err(Error::unavailable(Situation::NoLake { lake, schema }));
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
