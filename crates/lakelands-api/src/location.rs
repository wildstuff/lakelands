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
            engine: EngineConfig::Sqlite {
                dir: dir.into(),
                projection_dir: None,
                tuning: SqliteTuning::default(),
            },
            ..Location::memory(lake)
        }
    }

    /// A lake kept by the PostgreSQL engine in the database `url` names,
    /// in its default schema, over TLS.
    pub fn postgres(lake: impl Into<LakeId>, url: impl Into<String>) -> Location {
        Location {
            engine: EngineConfig::Postgres {
                url: url.into(),
                schema: None,
                tls: PgTls::default(),
                tuning: PgTuning::default(),
                tablespaces: None,
            },
            ..Location::memory(lake)
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
    /// The SQLite engine: the lake's log segments live in `dir`, its
    /// projection file in `projection_dir`.
    Sqlite {
        /// The lake's directory; created on first open.
        dir: PathBuf,
        /// The projection's directory, one lake's alone; `dir` when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        projection_dir: Option<PathBuf>,
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
        /// The tablespaces the lake's tables and indexes are made in.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tablespaces: Option<PgTablespaces>,
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

/// Where a lake's parts are made, each where `default_tablespace` puts it
/// if unnamed; a part is not moved, but a rebuild remakes all but the log.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PgTablespaces {
    /// The log's and the lease's tables, with their indexes.
    pub log: Option<String>,
    /// The projection's tables but `vectors`.
    pub projection: Option<String>,
    /// Their indexes, primary keys included.
    pub indexes: Option<String>,
    /// The `vectors` table and its indexes, vector indexes included.
    pub vectors: Option<String>,
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
    /// Rows a list-shaped read returns when the caller names no limit,
    /// or `max_limit` where that is smaller.
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
    /// measure is refused `too_large`, never cut. Also the largest
    /// `limit` a measure may ask for, where it is above `max_limit`.
    pub max_graph_nodes: u32,
    /// The most rows a `find` may skip with `offset`; past it the read is
    /// refused `too_large`, never cut.
    pub max_offset: u32,
    /// The most memberships one declaration of an object set may change
    /// over the records already there; past it the declaration is refused
    /// `too_large`, naming the set, and nothing is appended
    /// (`docs/seams.md`, "Declaring a set over a large type").
    pub max_set_changes: u32,
    /// The largest `top` a search's `rerank` may ask for: the window the
    /// second stage ranks, which a cursor carries
    /// (`docs/decisions/067-a-search-may-take-a-second-stage.md`).
    pub max_rerank_top: u32,
    /// Whether the lake chooses an order index from the pages it had to
    /// sort whole, beside the fields `sort_index.<type>` names
    /// (`docs/decisions/041-a-sorted-page-reads-its-page.md`). Off, only
    /// what the host sets is indexed.
    pub choose_order_index: bool,
    /// How many whole-sorted pages a field must have within
    /// `order_window_secs` to be chosen.
    pub order_after_pages: u32,
    /// The window those pages are counted in, in seconds.
    pub order_window_secs: u32,
    /// The fewest live records a type holds for a field of it to be
    /// chosen.
    pub order_min_records: u32,
    /// The most fields the lake chooses for one type.
    pub order_max_fields: u32,
    /// How long a chosen field may go without a page ordered by it before
    /// the lake drops it, in seconds of the writer's life.
    pub order_unused_secs: u32,
    /// The records one chunk of a chosen index's build writes while it
    /// holds the writer.
    pub order_chunk: u32,
    /// Whether a forgetting always rebuilds the projection, never
    /// refolding the records its sources wrote alone
    /// (`docs/decisions/058-a-source-may-be-forgotten.md`).
    pub forget_rebuild: bool,
    /// The bytes past which the writer seals the active segment after an
    /// append, on an engine that reports its size; 0 never seals by size,
    /// and a bound below [`Limits::SEAL_FLOOR`] is read as it
    /// (`docs/decisions/074-a-segment-is-sealed-by-size.md`).
    pub seal_after_bytes: u64,
}

impl Limits {
    /// The smallest bound a segment is sealed by size at: every checkpoint
    /// lists every sealed segment, and an open reads each one's end.
    pub const SEAL_FLOOR: u64 = 1 << 20;

    /// The bound a segment is sealed by size at, none where it is off.
    pub fn seal_bound(&self) -> Option<u64> {
        (self.seal_after_bytes > 0).then(|| self.seal_after_bytes.max(Self::SEAL_FLOOR))
    }
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
            max_rerank_top: 100,
            choose_order_index: true,
            order_after_pages: 3,
            order_window_secs: 600,
            order_min_records: 2_000,
            order_max_fields: 2,
            order_unused_secs: 604_800,
            order_chunk: 10_000,
            forget_rebuild: false,
            seal_after_bytes: 256 << 20,
        }
    }
}

#[cfg(test)]
mod tests;
