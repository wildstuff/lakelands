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
//! log          one row per event: its canonical bytes and members, its
//!              id, prev and hash as bytes, indexed by id (log.rs)
//! declarations, records, proposals, members, outputs, routes
//!              the projection, with its kept history (projection.rs)
//! text_index, edges, full_text, vectors
//!              the head indexes that find candidates
//! ```
//!
//! Beside it, while a rebuild builds a projection, a schema
//! `lakelands_rebuild_<digest>` holds that projection's tables and `meta`
//! until they are moved into the lake's schema (scratch.rs).
//!
//! Every statement names its tables with the schema; no connection's
//! `search_path` is set. `PgLake::export` writes the log's segments as
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
mod scratch;
mod vector_index;
mod view;

use lakelands_api::engine::{CanonicalEvent, EngineOpener, LakeEngine, OpenMode};
use lakelands_api::error::{Case, Detail, Doing, Error, Part, Result, Situation};
use lakelands_api::location::{EngineConfig, Location};

pub use connect::MIN_SERVER_VERSION;
use handle::PgLake;
#[doc(hidden)]
pub use handle::{ANALYZE_AFTER, LAKE_READERS, export};
#[doc(hidden)]
pub use view::{few_batch_statements, few_holders_statements, holding_statements, read_statements};

/// The name the engine answers to, in [`LakeEngine::name`] and for
/// `EngineConfig::Postgres`.
pub(crate) const NAME: &str = "pg";

/// The events a rebuild folds a second, as measured: 20 000 events in 8.1 s, a release
/// build on the development machine (lakelands#1042).
pub(crate) const REBUILD_RATE: u64 = 2_500;

pub(crate) use lakelands_core::lock;

/// A failed statement, for the host. Of an error the server sends, only
/// its severity, SQLSTATE and primary message: its `DETAIL`, `HINT`,
/// `WHERE` and statement position can quote a record's key or values,
/// which the answer carries to a caller who may not be shown that record
/// (024). Any other failure says every cause the driver gives. The
/// server's lack of disk room is named as that, the operator's to mend.
pub(crate) fn sql(e: postgres::Error) -> Error {
    let full = e.code() == Some(&postgres::error::SqlState::DISK_FULL);
    let detail = match e.as_db_error() {
        Some(db) => {
            let (severity, code) = (db.severity(), db.code().code());
            Detail::scrubbed(format_args!("{severity} {code}: {}", db.message()))
        }
        None => {
            let mut said = e.to_string();
            let mut source = std::error::Error::source(&e);
            while let Some(s) = source {
                said.push_str(&format!(": {s}"));
                source = s.source();
            }
            Detail::scrubbed(said)
        }
    };
    if full {
        return Error::unavailable(Situation::NoRoom { part: None, detail });
    }
    let (doing, part) = (Doing::Query, Part::Projection);
    Error::unavailable(Situation::Storage {
        doing,
        part,
        detail,
    })
}

/// Finds the PostgreSQL engine by name and opens lakes kept by it.
#[derive(Clone, Copy, Debug, Default)]
pub struct PgOpener;

impl EngineOpener for PgOpener {
    fn name(&self) -> &str {
        NAME
    }

    /// Reads the log's table in the location's schema over a connection
    /// of its own, and nothing else of it; a schema that is not there is
    /// not made.
    fn log_ends(&self, location: &Location) -> Result<Option<(CanonicalEvent, CanonicalEvent)>> {
        handle::log_ends(&target(location)?)
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

    /// Opens the log in the location's schema as `Read` does, and the
    /// scratch beside its projection, a schema of its own; the
    /// projection's tables are not read.
    fn open_scratch(&self, location: &Location) -> Result<Box<dyn LakeEngine>> {
        let lake = location.lake.clone();
        Ok(Box::new(PgLake::open_scratch(lake, target(location)?)?))
    }

    /// Measures the projection's tables in the location's schema, the log's
    /// left out, from PostgreSQL's catalogue; none of them is opened to
    /// read.
    fn room(&self, location: &Location) -> Result<u64> {
        handle::room(&target(location)?)
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
        tablespaces,
    } = &location.engine
    else {
        let (engine, kind) = (NAME.to_owned(), "postgres".to_owned());
        let lake = Box::new(location.lake.clone());
        return Err(Error::invalid(Case::EngineOtherLocation {
            engine,
            location: kind,
            lake,
        }));
    };
    Ok(connect::Target {
        url: url.clone(),
        schema: schema
            .clone()
            .unwrap_or_else(|| format!("lakelands_{}", location.lake)),
        tls: tls.clone(),
        tuning: *tuning,
        tablespaces: tablespaces.clone().unwrap_or_default(),
    })
}
