//! The PostgreSQL engine: a lake kept in one schema of a database, its log
//! as the format's lines, its writer lease as a row and an advisory lock,
//! its projection with its kept history
//! (`docs/decisions/038-a-lake-in-postgresql.md`).
//!
//! It implements the engine seam of `lakelands-api` (`docs/seams.md`
//! §Engine) and nothing more: it keeps the log's bytes, the writer lease
//! and the states the fold hands it, and answers reads over them. It never
//! folds, never canonicalises and never decides what an event means; the
//! store does. Where the seam asks it to select, order, link, score or
//! rank, it applies `lakelands_core::engines`, the rules every engine
//! shares, and uses its own indexes only to find candidates. So a lake
//! answers the same here as on SQLite or in memory.
//!
//! # A lake's schema
//!
//! ```text
//! meta         the lake's name, and the position the projection reflects
//! lease        the fencing token and its holder (lease.rs)
//! segments     the log's segments, and each sealed one's digest (log.rs)
//! log          one row per event: its canonical bytes and members
//! declarations, records, proposals, members, outputs, applied
//!              the projection, with its kept history (projection.rs)
//! text_index, edges, full_text, vectors
//!              the head indexes that find candidates
//! ```
//!
//! Every statement names its tables with the schema; no connection's
//! `search_path` is set. [`PgLake::export`] writes the log's segments as
//! the files `docs/log-format.md` §4.2 describes.
//!
//! # Connections
//!
//! Everything arrives through the location: the URL, the schema, the TLS
//! and the timeouts (`EngineConfig::Postgres`). Nothing is read from the
//! environment. The engine talks to the server through one door,
//! [`connect::Db`], which also keeps the driver's own runtime out of the
//! caller's (see [`connect::driver`]).

#![forbid(unsafe_code)]

mod connect;
mod handle;
mod lease;
mod log;
mod projection;
mod view;

use std::sync::{Mutex, MutexGuard};

use lakelands_api::engine::{EngineOpener, LakeEngine, OpenMode};
use lakelands_api::error::{Error, Refusal, Result};
use lakelands_api::location::{EngineConfig, Location};

pub use connect::MIN_SERVER_VERSION;
pub use handle::{PgLake, export};

/// The name the engine answers to, in [`LakeEngine::name`] and for
/// `EngineConfig::Postgres`.
pub const NAME: &str = "pg";

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

/// A failed statement, for the host, with every cause the driver gives.
pub(crate) fn sql(e: postgres::Error) -> Error {
    let mut message = format!("the database failed: {e}");
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        message.push_str(&format!(": {s}"));
        source = s.source();
    }
    unavailable(message)
}

/// Finds the PostgreSQL engine by name and opens lakes kept by it.
#[derive(Clone, Copy, Debug, Default)]
pub struct PgOpener;

impl EngineOpener for PgOpener {
    fn name(&self) -> &str {
        NAME
    }

    /// Opens the lake in the location's schema. `Create` makes the schema
    /// and the lake when there is none; `Read` and `Write` on a schema
    /// holding no lake answer `Unavailable`.
    fn open(&self, location: &Location, mode: OpenMode) -> Result<Box<dyn LakeEngine>> {
        Ok(Box::new(PgLake::open(
            location.lake.clone(),
            target(location)?,
            mode,
        )?))
    }
}

/// Where a location keeps its lake: the connection, the schema (by
/// default `lakelands_<lake>`), the TLS and the timeouts.
pub(crate) fn target(location: &Location) -> Result<connect::Target> {
    let EngineConfig::Postgres {
        url,
        schema,
        tls,
        tuning,
    } = &location.engine
    else {
        return Err(invalid(format!(
            "the {NAME} engine opens only postgres locations, and lake `{}` is configured for \
             another engine",
            location.lake
        )));
    };
    Ok(connect::Target {
        url: url.clone(),
        schema: schema
            .clone()
            .unwrap_or_else(|| format!("lakelands_{}", location.lake)),
        tls: tls.clone(),
        tuning: *tuning,
    })
}
