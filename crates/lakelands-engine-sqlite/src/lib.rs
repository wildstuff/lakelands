//! The SQLite engine: a lake as a directory holding its log as segment
//! files, its writer lease as a file, and its projection as one SQLite
//! database. It honours the same conformance suite as every other engine,
//! and decides nothing the in-memory engine decides differently: both call
//! the same rulebook.
//!
//! It implements the engine seam of `lakelands-api` (`docs/seams.md`
//! §Engine) and nothing more: it keeps the log's bytes, the writer lease
//! and the states the fold hands it, and answers reads over them. It
//! never folds, never canonicalises and never decides what an event
//! means; the store does. Where the seam asks it to select, order, link,
//! score or rank, it applies `lakelands_core::engines`, the rules every
//! engine shares, and uses its own indexes only to find candidates.
//!
//! # A lake directory
//!
//! ```text
//! <dir>/lake                  the lake's name
//! <dir>/lease                 the fencing token and its holder (lease.rs)
//! <dir>/log/<first>.jsonl     the log's segments (docs/log-format.md §4.2)
//! <dir>/index/<first>.idx     each event's members beside its bytes (log.rs)
//! <dir>/projection.db         the projection, with its kept history
//! ```
//!
//! The log is the truth: the projection and the index hold nothing the log
//! does not, and a lake's segments verify with nothing but the format.
//!
//! # Tuning
//!
//! Cache sizes, memory-mapped reads and the journal's checkpoint interval
//! come from the location's [`SqliteTuning`]; the engine reads no
//! environment.

#![forbid(unsafe_code)]

mod handle;
mod lease;
mod log;
mod projection;
mod view;

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode};
use lakelands_api::error::{Error, Refusal, Result};
use lakelands_api::location::{EngineConfig, Location, SqliteTuning};

pub use handle::SqliteLake;

/// The name the engine answers to, in [`EngineConfig::Sqlite`] and in
/// [`LakeEngine::name`].
pub const NAME: &str = "sqlite";

/// Locks a mutex, carrying on past a panic in another holder: every
/// change a holder makes is written whole or not at all.
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

/// A failed file operation, for the host.
pub(crate) fn io_error(doing: &str, path: &Path, e: std::io::Error) -> Error {
    unavailable(format!("{doing} {} failed: {e}", path.display()))
}

/// A failed projection operation, for the host.
pub(crate) fn sql(e: rusqlite::Error) -> Error {
    unavailable(format!("the projection failed: {e}"))
}

/// Finds the SQLite engine by name and opens lakes kept by it.
#[derive(Clone, Copy, Debug, Default)]
pub struct SqliteOpener;

impl EngineOpener for SqliteOpener {
    fn name(&self) -> &str {
        NAME
    }

    /// Opens the lake in the location's directory. `Create` makes the
    /// directory and the lake when there is none; `Read` and `Write` on a
    /// directory holding no lake answer `Unavailable`.
    fn open(&self, location: &Location, mode: OpenMode) -> Result<Box<dyn LakeEngine>> {
        let EngineConfig::Sqlite { dir, tuning } = &location.engine else {
            return Err(invalid(format!(
                "the {NAME} engine opens only sqlite locations, and lake `{}` is configured for \
                 another engine",
                location.lake
            )));
        };
        Ok(Box::new(SqliteLake::open(
            location.lake.clone(),
            dir.clone(),
            *tuning,
            mode,
        )?))
    }
}

/// The tuning a location names, for a host that builds one.
pub fn tuning(location: &Location) -> Option<SqliteTuning> {
    match &location.engine {
        EngineConfig::Sqlite { tuning, .. } => Some(*tuning),
        _ => None,
    }
}
