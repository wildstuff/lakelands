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
            engine: EngineConfig::Sqlite { dir: dir.into() },
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
    },
    /// The in-memory engine.
    Memory,
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
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            default_limit: 50,
            max_limit: 1000,
            max_depth: 16,
            max_reactive_depth: 8,
            commit_window_events: 32,
        }
    }
}
