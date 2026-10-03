//! Where a lake lives and how it is configured.
//!
//! Everything that configures a lake arrives here, explicitly, at the
//! call site; Lakelands reads no environment
//! (`docs/decisions/005-no-environment-reads.md`). `Location` is
//! configuration, not a seam: every seam is configured through it.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::id::LakeId;

/// A lake's address and configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Location {
    /// The lake this location holds.
    pub lake: LakeId,
    /// The engine and where it keeps the lake.
    pub engine: EngineConfig,
    /// A handle naming the key a lake is encrypted with, resolved by the
    /// host. The store never sees key material in configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<KeyHandle>,
    /// Limits that bound the work one call may cause.
    #[serde(default)]
    pub limits: Limits,
}

impl Location {
    /// A lake kept by the SQLite engine in a directory of its own.
    pub fn sqlite(lake: impl Into<LakeId>, dir: impl Into<PathBuf>) -> Location {
        Location {
            lake: lake.into(),
            engine: EngineConfig::Sqlite {
                dir: dir.into(),
                tuning: SqliteTuning::default(),
            },
            key: None,
            limits: Limits::default(),
        }
    }

    /// A lake kept by the PostgreSQL engine in the database `url` names,
    /// in its default schema, over TLS.
    pub fn postgres(lake: impl Into<LakeId>, url: impl Into<String>) -> Location {
        Location {
            lake: lake.into(),
            engine: EngineConfig::Postgres {
                url: url.into(),
                schema: None,
                tls: PgTls::default(),
                tuning: PgTuning::default(),
            },
            key: None,
            limits: Limits::default(),
        }
    }

    /// A lake held in memory, gone when the process ends.
    pub fn memory(lake: impl Into<LakeId>) -> Location {
        Location {
            lake: lake.into(),
            engine: EngineConfig::Memory,
            key: None,
            limits: Limits::default(),
        }
    }
}

/// Which engine keeps a lake, with that engine's own settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "engine", rename_all = "snake_case")]
#[non_exhaustive]
pub enum EngineConfig {
    /// The SQLite engine: the lake's log segments and its projection
    /// file live in `dir`.
    Sqlite {
        /// The lake's directory; created on first open.
        dir: PathBuf,
        /// How the engine sizes its caches and checkpoints its projection.
        #[serde(default)]
        tuning: SqliteTuning,
    },
    /// The in-memory engine.
    Memory,
    /// The PostgreSQL engine: the lake kept in one schema of the database
    /// `url` names (`docs/decisions/038-a-lake-in-postgresql.md`).
    Postgres {
        /// The connection, as a `postgres://` URL. Its `sslmode` is not
        /// read: TLS is `tls`.
        url: String,
        /// The schema that holds the lake; `lakelands_<lake>` when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        schema: Option<String>,
        /// How the connection is secured.
        #[serde(default)]
        tls: PgTls,
        /// Timeouts the engine sets on its connections.
        #[serde(default)]
        tuning: PgTuning,
    },
    /// An engine this crate does not name, selected by `name` and
    /// configured by `settings` (a host's own engine, for instance).
    Other {
        /// The engine's name, as it registers itself.
        name: String,
        /// Settings the engine interprets.
        #[serde(default)]
        settings: BTreeMap<String, serde_json::Value>,
    },
}

/// How the PostgreSQL engine secures its connections.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PgTls {
    /// `require` (the default) or `disable`.
    pub mode: PgTlsMode,
    /// The certificate authorities the server's certificate is verified
    /// against, as a PEM bundle; the first system bundle found at a
    /// well-known path when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ca_file: Option<PathBuf>,
    /// A client certificate to present (PEM), with `client_key`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_cert: Option<PathBuf>,
    /// The client certificate's private key (PEM).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_key: Option<PathBuf>,
}

/// Whether the PostgreSQL engine's connections use TLS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PgTlsMode {
    /// TLS, with the server's certificate and name verified; no plaintext.
    #[default]
    Require,
    /// Plaintext, for a server on the same machine or a network the
    /// operator secures otherwise.
    Disable,
}

/// The PostgreSQL engine's timeouts. None of them can change an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PgTuning {
    /// How long opening a connection may take, in seconds.
    pub connect_timeout_secs: u32,
    /// How long one statement may run, in seconds; 0 for no limit.
    pub statement_timeout_secs: u32,
}

impl Default for PgTuning {
    fn default() -> PgTuning {
        PgTuning {
            connect_timeout_secs: 10,
            statement_timeout_secs: 0,
        }
    }
}

/// The SQLite engine's performance levers, per lake. Only levers that
/// cannot change an answer or weaken durability are here: the
/// projection's journal (write-ahead) and its synchronisation are fixed,
/// because the fold's crash safety rests on them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SqliteTuning {
    /// The writer connection's page cache, in KiB. A lake has one writer,
    /// so this does not multiply.
    pub writer_cache_kb: u32,
    /// Each reader connection's page cache, in KiB. It multiplies by the
    /// readers open at once, so keep it below the writer's.
    pub reader_cache_kb: u32,
    /// Bytes of the projection file mapped into memory per connection, in
    /// MiB; 0 turns memory-mapped reads off.
    pub mmap_mb: u32,
    /// How many pages the write-ahead journal grows before it is folded
    /// back into the projection file. Higher amortises the checkpoint over
    /// more writes and grows the journal.
    pub wal_autocheckpoint_pages: u32,
}

impl SqliteTuning {
    /// The defaults: a 64 MiB writer cache, 16 MiB per reader, 256 MiB
    /// mapped, a checkpoint every 20,000 pages.
    pub const DEFAULT: SqliteTuning = SqliteTuning {
        writer_cache_kb: 64 * 1024,
        reader_cache_kb: 16 * 1024,
        mmap_mb: 256,
        wal_autocheckpoint_pages: 20_000,
    };

    /// Tuning sized to a host's memory, for a host that wants its caches
    /// to follow the machine: smaller below 17 GiB, the defaults up to
    /// 48 GiB, larger above. The reader cache grows slowest because it
    /// multiplies by the readers open; mapped pages are the operating
    /// system's to reclaim, so they grow a little faster.
    pub fn for_total_memory(bytes: u64) -> SqliteTuning {
        let (writer_mb, reader_mb, mmap_mb) = match bytes >> 30 {
            0..=8 => (16, 4, 64),
            9..=16 => (32, 8, 128),
            17..=48 => return SqliteTuning::DEFAULT,
            49..=96 => (96, 24, 384),
            _ => (128, 32, 512),
        };
        SqliteTuning {
            writer_cache_kb: writer_mb * 1024,
            reader_cache_kb: reader_mb * 1024,
            mmap_mb,
            ..SqliteTuning::DEFAULT
        }
    }
}

impl Default for SqliteTuning {
    fn default() -> Self {
        SqliteTuning::DEFAULT
    }
}

/// A name for an encryption key, resolved by the host. Never the key.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct KeyHandle(pub String);

/// Bounds on the work one call may cause.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    /// Rows a list-shaped read returns when the caller names no limit.
    pub default_limit: u32,
    /// The largest `limit` a caller may ask for.
    pub max_limit: u32,
    /// The deepest a pattern step may walk.
    pub max_depth: u32,
    /// The deepest a chain of reactive writes may go.
    pub max_reactive_depth: u32,
    /// The most events one group-commit window holds.
    pub commit_window_events: u32,
    /// The most records a whole-graph measure may reach; past it the
    /// measure is refused `too_large`, never cut.
    pub max_graph_nodes: u32,
    /// The most rows a `find` may skip with `offset`; past it the read is
    /// refused `too_large`, never cut.
    pub max_offset: u32,
    /// The most memberships one declaration of an object set may change
    /// over the records already there; past it the declaration is refused
    /// `too_large`, naming the set, and nothing is appended
    /// (`docs/seams.md`, "Declaring a set over a large type").
    pub max_set_changes: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            default_limit: 50,
            max_limit: 1000,
            max_depth: 16,
            max_reactive_depth: 8,
            commit_window_events: 32,
            max_graph_nodes: 100_000,
            max_offset: 100_000,
            max_set_changes: 20_000,
        }
    }
}

#[cfg(test)]
mod tests;
