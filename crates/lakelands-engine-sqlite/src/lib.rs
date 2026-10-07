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
//! come from the location's [`SqliteTuning`](lakelands_api::SqliteTuning);
//! the engine reads no environment.

#![forbid(unsafe_code)]

mod handle;
mod lease;
mod log;
mod projection;
mod view;

use std::path::Path;

use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode};
use lakelands_api::error::{Error, Result};
use lakelands_api::location::{EngineConfig, Location};

pub use handle::SqliteLake;
#[doc(hidden)]
pub use view::{first_page_statements, read_statements, seek_statements};

/// The name the engine answers to, in [`EngineConfig::Sqlite`] and in
/// [`LakeEngine::name`].
pub const NAME: &str = "sqlite";

pub(crate) use lakelands_core::{invalid, lock, unavailable};

/// A failed file operation, for the host.
pub(crate) fn io_error(doing: &str, path: &Path, e: std::io::Error) -> Error {
    unavailable(format!("{doing} {} failed: {e}", path.display()))
}

/// A failed projection operation, for the host, as [`told`] says it.
pub(crate) fn sql(e: rusqlite::Error) -> Error {
    unavailable(format!("the projection failed: {}", told(&e)))
}

/// What a SQLite error says to the host: of a failure SQLite reports, its
/// extended code and message, which name a failed constraint's columns or
/// the schema's own check, never a row's values; of any other error, only
/// its kind. The driver's own errors quote the statement's text
/// (`SqlInputError`) or a stored value (`IntegralValueOutOfRange`, a
/// failed conversion), which the answer would carry to a caller who may
/// not be shown them (024).
fn told(e: &rusqlite::Error) -> String {
    if let rusqlite::Error::SqliteFailure(f, message) = e {
        let message = message.clone().unwrap_or_else(|| f.to_string());
        return format!("SQLite {}: {message}", f.extended_code);
    }
    let kind = format!("{e:?}");
    let kind = kind.split(|c: char| !c.is_alphanumeric()).next();
    kind.unwrap_or_default().to_owned()
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

#[cfg(test)]
mod tests;
