//! The event log, as types. `docs/log-format.md` is the specification:
//! the envelope, the canonical bytes, the hash chain, every kind, the
//! segments and the checkpoints. These types are its Rust form; where the
//! two disagree, the document is right and this module is a bug.
//!
//! The log is the truth of a lake (`docs/decisions/001-the-log-is-the-truth.md`).
//! Engines store events and give them back byte-identically; only the
//! store's core writes them.

use serde::{Deserialize, Serialize};

use crate::id::{
    FieldName, IdempotencyKey, LakeId, Position, ProposalId, RecordKey, SourceId, TriggerName,
    TypeName, VerbName, Version,
};
use crate::principal::WriteContext;
use crate::record::{Origin, Patch, Value};
use crate::time::Timestamp;
use crate::write::{Declaration, Migration, ProposedWrite};

/// The newest log format version this contract reads and writes. Every
/// event carries the version it was written under; a reader refuses a
/// version newer than the one it knows, by name and with a message, never
/// by guessing. Version 2 adds `migrated.rekeyed`; version 3 the member
/// `upstream` and `date_bucket` in a declaration; version 4 a proposed
/// declaration or migration; version 5 a declaration's `annotations`;
/// version 6 a derived relation; version 7 a call of `retired` in a
/// declaration; version 8 a view's `params` and a call of `param`, and a
/// verb on its type; version 9 a view's `merge`; version 10 a proposed
/// tightening of a value type; version 11 a call of `floor` in a
/// declaration; version 12 a declaration `with_the_set`; version 13 a
/// proposal `pinned` to the record it writes, and the kind `date`
/// (`docs/log-format.md` §6).
pub const LOG_FORMAT_VERSION: u32 = 13;

/// The version a lake is created with and writes until it needs a newer
/// one: a lake that never re-keys a type stays at version 1, byte for
/// byte.
pub const LOG_FORMAT_BASE: u32 = 1;

/// A SHA-256 digest, as 64 lower-case hexadecimal digits.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Digest(pub String);

impl Digest {
    /// The predecessor of a lake's first event: 64 zeros.
    pub fn zero() -> Digest {
        Digest("0".repeat(64))
    }
}

/// An event's identifier: 32 lower-case hexadecimal digits (128 bits).
/// Content-derived for ingest and for authored writes with an idempotency
/// key, so a redelivery is recognised; derived from the lake and the
/// position otherwise (`docs/log-format.md` §3).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventId(pub String);

/// A procedure that produced a value: a verb, a function backing, a
/// derivation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Procedure {
    /// Its name.
    pub name: String,
    /// Its version.
    pub version: String,
}

/// One event: the envelope every kind shares, and the kind's body. A
/// member this format version does not know is refused, never skipped.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    /// The log format version it was written under.
    pub v: u32,
    /// The lake it belongs to.
    pub lake: LakeId,
    /// Its coordinate: 0 for the first event, no gaps.
    pub position: Position,
    /// Its identifier.
    pub id: EventId,
    /// The hash of the event at `position - 1`; [`Digest::zero`] at 0.
    pub prev: Digest,
    /// The lake's wall time when the event was appended.
    pub known_at: Timestamp,
    /// The principal it was written as, `<kind>:<id>`.
    pub writer: String,
    /// Which authority wrote it.
    pub origin: Origin,
    /// The source, for an event from a source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceId>,
    /// When the source observed the values, for an event from a source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen_at: Option<Timestamp>,
    /// For values read from another lake's log: that lake and position
    /// (format version 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<crate::record::Upstream>,
    /// The procedure that produced the values, if one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub procedure: Option<Procedure>,
    /// The host's context for the write. Always present.
    pub context: WriteContext,
    /// The event this one is a consequence of, in the same or an earlier
    /// append (a verb's write, a fired trigger, an accepted proposal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<Position>,
    /// The writer lease's fencing token the event was appended under.
    pub fence: u64,
    /// The caller's idempotency key, for an authored write that gave one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<IdempotencyKey>,
    /// The kind and its payload.
    pub body: EventBody,
    /// SHA-256 over the event's canonical bytes without this field.
    pub hash: Digest,
}

/// Every kind of event, and its payload. The set is closed per format
/// version; a new kind is a format change (`docs/log-format.md`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EventBody {
    /// The first event of every lake, at position 0.
    LakeCreated {
        /// The log format version the lake was created with.
        format: u32,
    },
    /// A writer took the lake's lease.
    LeaseTaken {
        /// The new fencing token; greater than every earlier one.
        fence: u64,
        /// Who holds it: a process description for the operator.
        holder: String,
    },
    /// A declaration entered the ontology.
    Declared {
        /// The declaration.
        declaration: Declaration,
        /// For a type: its schema hash after this event.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        schema_hash: Option<String>,
        /// The declaration's version after this event (1 for new).
        version: u32,
        /// Whether another declaration of the same set follows in this
        /// append: the records the set computes again are computed once,
        /// at its last declaration (format version 12).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        with_the_set: bool,
    },
    /// A type changed shape; its stored records follow in the fold.
    Migrated {
        /// The migration.
        migration: Migration,
        /// The schema hash after.
        schema_hash: String,
        /// The type's version after.
        version: u32,
        /// The preflight report it was applied against.
        report_id: String,
        /// Every record key the migration moves, over every type it
        /// re-keys (format version 2); empty when no key changes.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        rekeyed: Vec<crate::write::Rekey>,
    },
    /// A value type's migration that renames values: its declaration is
    /// put in force and every stored record that holds an old value takes
    /// the new one in the fold (format version 12).
    ValueTypeMigrated {
        /// The value type's new declaration.
        declaration: crate::ontology::ValueTypeDecl,
        /// Its version after this event.
        version: u32,
        /// The values renamed: old value to new value.
        renames: Vec<(String, String)>,
        /// The preflight report it was applied against.
        report_id: String,
        /// The types whose fields use the value type, which the fold
        /// rewrites: every one the ontology names, in name order.
        types: Vec<TypeName>,
    },
    /// An authored write: `upsert`, a verb's effect, an accepted
    /// proposal.
    RecordUpserted {
        /// The record's type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The record's key.
        key: RecordKey,
        /// The record's version after this event.
        version: Version,
        /// The sparse patch.
        patch: Patch,
        /// Whether the write found no live record under the key (format
        /// version 11): with `version` 1 the first, above it made again.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        made: bool,
    },
    /// A write from a source, through `ingest`.
    RecordIngested {
        /// The record's type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The record's key.
        key: RecordKey,
        /// The record's version after this event.
        version: Version,
        /// The sparse patch.
        patch: Patch,
        /// Whether the write found no live record under the key (format
        /// version 11): with `version` 1 the first, above it made again.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        made: bool,
    },
    /// A record was retired.
    RecordRetired {
        /// The record's type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The record's key.
        key: RecordKey,
        /// The record's version after this event.
        version: Version,
    },
    /// A record was withheld from ordinary reads (a retention hold).
    RecordRestricted {
        /// The record's type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The record's key.
        key: RecordKey,
        /// The record's version after this event.
        version: Version,
        /// Why, as the host gave it.
        reason: String,
    },
    /// A hold was lifted.
    RecordUnrestricted {
        /// The record's type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The record's key.
        key: RecordKey,
        /// The record's version after this event.
        version: Version,
    },
    /// A verb was called and ran. Its writes follow, caused by this
    /// event.
    VerbCalled {
        /// The verb.
        verb: VerbName,
        /// The record it was called on; absent for a verb called on its
        /// type (log format 8).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<RecordKey>,
        /// Its payload.
        payload: Patch,
    },
    /// A write was recorded as a proposal instead of being applied.
    WriteProposed {
        /// The proposal's identifier.
        proposal: ProposalId,
        /// What it would write.
        write: ProposedWrite,
        /// Why it waits.
        why: String,
        /// Whether its proposer's grant admits only some records of the
        /// type it writes: its accept is refused once the record or the
        /// type changed after it (format version 13).
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        pinned: bool,
    },
    /// A proposal was accepted. Its write follows, caused by this event.
    WriteAccepted {
        /// The proposal.
        proposal: ProposalId,
    },
    /// A proposal was rejected. Nothing was written.
    WriteRejected {
        /// The proposal.
        proposal: ProposalId,
        /// The reason given.
        reason: String,
    },
    /// A write made a trigger's condition hold. What reacts is the
    /// host's business; this event is the durable fact that it is due.
    TriggerFired {
        /// The trigger.
        trigger: TriggerName,
        /// The record's type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The record's key.
        key: RecordKey,
    },
    /// A write moved a record into or out of an object set.
    ObjectSetChanged {
        /// The set.
        set: String,
        /// The record's type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The record's key.
        key: RecordKey,
        /// True if it joined, false if it left.
        joined: bool,
    },
    /// The recorded output of a seam the store called (a function
    /// backing, an embedding), read back on replay instead of calling
    /// the seam again (`docs/decisions/002-determinism-is-the-boundary.md`).
    OutputRecorded {
        /// The seam and the name it was called under.
        seam: String,
        /// SHA-256 of the canonical bytes of the input.
        input: Digest,
        /// The output.
        output: Value,
        /// The field it fills, if one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        field: Option<(TypeName, RecordKey, FieldName)>,
    },
    /// A checkpoint: the chain's state at a segment boundary, so that a
    /// reader can verify later segments without the earlier ones, and a
    /// missing earlier segment is detected rather than skipped.
    Checkpoint {
        /// The last position of the sealed segment.
        through: Position,
        /// The hash of the event at `through`.
        hash: Digest,
        /// Every sealed segment so far, oldest first.
        segments: Vec<SegmentDigest>,
    },
}

/// The kinds of one format version: `EventBody::kind` and [`KINDS`] come
/// from this one list, so they cannot drift, and a variant not listed
/// here does not compile.
macro_rules! kinds {
    ($($variant:ident => $name:literal,)*) => {
        /// Every event kind of the current format version, as the log and
        /// the change feed spell them, in the order `EventBody` declares
        /// them. `FeedFilter::kinds` takes free strings, so a relay may name
        /// a kind a newer format adds; this is the closed set of this one.
        pub const KINDS: &[&str] = &[$($name),*];

        impl EventBody {
            /// The kind's name as the log and the change feed spell it.
            pub fn kind(&self) -> &'static str {
                match self {
                    $(EventBody::$variant { .. } => $name,)*
                }
            }
        }
    };
}

kinds! {
    LakeCreated => "lake_created",
    LeaseTaken => "lease_taken",
    Declared => "declared",
    Migrated => "migrated",
    ValueTypeMigrated => "value_type_migrated",
    RecordUpserted => "record_upserted",
    RecordIngested => "record_ingested",
    RecordRetired => "record_retired",
    RecordRestricted => "record_restricted",
    RecordUnrestricted => "record_unrestricted",
    VerbCalled => "verb_called",
    WriteProposed => "write_proposed",
    WriteAccepted => "write_accepted",
    WriteRejected => "write_rejected",
    TriggerFired => "trigger_fired",
    ObjectSetChanged => "object_set_changed",
    OutputRecorded => "output_recorded",
    Checkpoint => "checkpoint",
}

/// A sealed segment, as a checkpoint lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentDigest {
    /// Its first position.
    pub first: Position,
    /// Its last position.
    pub last: Position,
    /// SHA-256 of the segment file's bytes.
    pub digest: Digest,
}

#[cfg(test)]
mod tests;
