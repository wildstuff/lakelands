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
//! <dir>/lake                  the lake's name; every handle onto the
//!                             projection holds it locked shared (handle.rs)
//! <dir>/lease                 the fencing token and its holder (lease.rs)
//! <dir>/log/<first>.jsonl     the log's segments (docs/log-format.md §4.2)
//! <dir>/index/<first>.idx     each event's members beside its bytes (log.rs)
//! <dir>/projection.db         the projection, with its kept history
//! <dir>/projection.db.rebuild the projection a rebuild builds beside it (scratch.rs)
//! <dir>/projection.db.rebuild.mark  its lock, and its mark once built; emptied, kept
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
mod scratch;
mod view;

use lakelands_api::engine::{CanonicalEvent, EngineOpener, LakeEngine, OpenMode};
use lakelands_api::error::{Case, Detail, Doing, Error, Part, Result, Situation};
use lakelands_api::location::{EngineConfig, Location, SqliteTuning};

use handle::SqliteLake;
#[doc(hidden)]
pub use view::{few_holders_statements, first_page_statements, read_statements, seek_statements};

/// The name the engine answers to, in [`EngineConfig::Sqlite`] and in
/// [`LakeEngine::name`].
pub(crate) const NAME: &str = "sqlite";

/// The events a rebuild folds a second, as measured: 50 000 events in 2.9 s, a release
/// build on the development machine (lakelands#1042).
pub(crate) const REBUILD_RATE: u64 = 17_000;

pub(crate) use lakelands_core::{damaged, lock};

/// A failed file operation, for the host.
pub(crate) fn io_error(doing: Doing, part: Part, e: std::io::Error) -> Error {
    lakelands_core::io_failed(doing, part, &e)
}

/// A failed projection operation, for the host, as [`told`] says it.
pub(crate) fn sql(e: rusqlite::Error) -> Error {
    let detail = Detail::scrubbed(told(&e));
    let (doing, part) = (Doing::Query, Part::Projection);
    Error::unavailable(Situation::Storage {
        doing,
        part,
        detail,
    })
}

/// What a SQLite error says to the host: of a failure SQLite reports, its
/// extended code and message, which name a failed constraint's columns or
/// the schema's own check, never a row's values; of any other error, only
/// its kind. The driver's own errors quote the statement's text
/// (`SqlInputError`) or a stored value (`IntegralValueOutOfRange`, a
/// failed conversion), which the answer would carry to a caller who may
/// not be shown them (024).
pub(crate) fn told(e: &rusqlite::Error) -> String {
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

    /// Reads the log's files in the location's directory and nothing
    /// else of it; a directory that is not there is not made. A segment
    /// without its index file, or a projection with no segment file, is
    /// damage and not a directory with no lake.
    fn log_ends(&self, location: &Location) -> Result<Option<(CanonicalEvent, CanonicalEvent)>> {
        log::ends(kept_in(location)?.0)
    }

    /// Opens the lake in the location's directory. `Create` makes the
    /// directory and the lake when there is none; `Read` and `Write` on a
    /// directory holding no lake answer `Unavailable`.
    fn open(&self, location: &Location, mode: OpenMode) -> Result<Box<dyn LakeEngine>> {
        let (dir, tuning) = kept_in(location)?;
        Ok(Box::new(SqliteLake::open(
            location.lake.clone(),
            dir.clone(),
            *tuning,
            mode,
        )?))
    }

    /// Opens the log in the location's directory as `Read` does, and the
    /// scratch beside its projection, `projection.db.rebuild`; the
    /// projection itself is not opened.
    fn open_scratch(&self, location: &Location) -> Result<Box<dyn LakeEngine>> {
        let (dir, tuning) = kept_in(location)?;
        let lake = location.lake.clone();
        Ok(Box::new(SqliteLake::open_scratch(
            lake,
            dir.clone(),
            *tuning,
        )?))
    }
}

/// The directory and the tuning of a location this engine keeps.
fn kept_in(location: &Location) -> Result<(&std::path::PathBuf, &SqliteTuning)> {
    let EngineConfig::Sqlite { dir, tuning } = &location.engine else {
        let lake = Box::new(location.lake.clone());
        let (engine, location) = (NAME.to_owned(), "sqlite".to_owned());
        return Err(Error::invalid(Case::EngineOtherLocation {
            engine,
            location,
            lake,
        }));
    };
    Ok((dir, tuning))
}

#[cfg(test)]
mod tests;
