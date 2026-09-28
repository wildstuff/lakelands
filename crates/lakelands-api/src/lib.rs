//! The contract every other Lakelands crate builds against.
//!
//! This crate holds what a caller or a host sees: the [`Principal`] a
//! call is made as, the [`Location`] a lake is configured through, the
//! [`Position`] every reply is read at; the doors ([`Catalog`], and the
//! [`LakeStore`] facade a host holds); the log's types; and the seven
//! seams through which Lakelands reaches anything outside its
//! deterministic core. It performs no I/O and depends on no engine.
//!
//! The one generator of LLM-facing tool names, [`tools::tools`], lives
//! here, so that every crate exposing a tool takes its names from the
//! same place.
//!
//! Three words never vary (`docs/glossary.md`): `position`, the log
//! coordinate; `version`, a record's compare-and-set token; `valid_at`,
//! business time.

#![forbid(unsafe_code)]

pub mod canonical;
pub mod catalog;
pub mod describe;
pub mod engine;
pub mod error;
pub mod event;
pub mod expr;
pub mod id;
pub mod key;
pub mod location;
pub mod ontology;
pub mod principal;
pub mod query;
pub mod read;
pub mod record;
pub mod seam;
pub mod shape;
pub mod time;
pub mod tools;
pub mod write;

pub use catalog::{Catalog, CommitStream, LakeStore, SourceStream, Verification, ingest_all};
pub use error::{Error, Missing, Refusal, Result};
pub use id::{
    Cursor, FieldName, IdempotencyKey, LakeId, Position, ProposalId, RecordKey, RelationName,
    SourceId, TriggerName, TypeName, ValueTypeName, VerbName, Version,
};
pub use location::{EngineConfig, Location, SqliteTuning};
pub use principal::{Access, Grants, Principal, PrincipalKind, WriteContext, WriteOrigin};
pub use read::{After, Find, Found, Get, ReadAt, Reply};
pub use record::{FieldMeta, Origin, Patch, Record, SourceRecord, Value};
pub use time::Timestamp;
pub use write::{Committed, Outcome, WriteOpts};
