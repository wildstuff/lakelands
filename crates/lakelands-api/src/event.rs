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

/// The log format version this release writes. Every event carries the
/// version it was written in as `v`, and `lake_created.format` says the
/// one the lake was made in. A lake written in a format from
/// [`OLDEST_LOG_FORMAT`] up to this one is read, each older event through
/// its format's converter, and a writer's open upgrades it; one in another
/// format is refused by name, with both numbers, and nothing of it is read
/// (`docs/log-format.md` §6,
/// `docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`).
pub const LOG_FORMAT_VERSION: u32 = 18;

/// The oldest log format this release reads: 14, the format of the first
/// release with real installations (054).
pub const OLDEST_LOG_FORMAT: u32 = 14;

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
    /// For values read from another lake's log: that lake and position.
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
    /// The delivery a source's record came in, on a `record_ingested`
    /// from format 17 on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<crate::id::DeliveryId>,
    /// Which members of the body's `patch`, or its `output`, are sealed
    /// under the source's key, from format 16 on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed: Option<Sealed>,
    /// The kind and its payload.
    pub body: EventBody,
    /// SHA-256 over the event's canonical bytes without this field.
    pub hash: Digest,
}

/// The members of an event a source's key seals: the patch members it
/// names, or the recorded output where `fields` is empty. Each holds the
/// base64 of AES-256-GCM-SIV's output over the value's canonical bytes
/// (`docs/log-format.md` §1.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sealed {
    /// The version of the source's key that sealed them.
    pub key: crate::id::KeyId,
    /// The source whose key it is, for an event whose envelope names none:
    /// a recorded output computed from that source's values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<crate::id::SourceId>,
    /// The patch members sealed, in name order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<FieldName>,
    /// The keys of the source outside its versions that seal them too,
    /// in id order, each a delivery's sealed apart: the members are
    /// sealed under one key derived from `key` and these, from format 18
    /// on (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apart: Vec<crate::id::KeyId>,
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
    /// A writer's open upgraded a lake written in an older format: from
    /// this event on the lake is written in `to`. The events before it
    /// stay as they were written, and are read through their format's
    /// converter.
    LakeUpgraded {
        /// The format the lake was written in before.
        from: u32,
        /// The format it is written in from here on.
        to: u32,
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
        /// at its last declaration.
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
        /// re-keys; empty when no key changes.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        rekeyed: Vec<crate::write::Rekey>,
    },
    /// A value type's migration that renames values: its declaration is
    /// put in force and every stored record that holds an old value takes
    /// the new one in the fold.
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
        /// Whether the write found no live record under the key: with
        /// `version` 1 the first, above it made again.
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
        /// Whether the write found no live record under the key: with
        /// `version` 1 the first, above it made again.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        made: bool,
    },
    /// A host forgot a source: its keys are destroyed, and every value it
    /// sealed reads as erased, at every position
    /// (`docs/decisions/058-a-source-may-be-forgotten.md`).
    SourceForgotten {
        /// The source.
        source: SourceId,
        /// The versions of its key the act destroys.
        keys: Vec<crate::id::KeyId>,
    },
    /// A host took back part of a source's values: each one a selected
    /// `record_ingested` of the source before this event carries reads as
    /// erased, at every position
    /// (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`).
    ValuesRemoved {
        /// The source.
        source: SourceId,
        /// The selection, as asked.
        selection: crate::write::Selection,
        /// For a time, the first position it does not reach.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<Position>,
        /// The versions of the source's key it destroys, oldest first: the
        /// longest prefix whose values it all selects.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        keys: Vec<crate::id::KeyId>,
        /// The source's keys of deliveries sealed apart it destroys, in id
        /// order: each whose values it all selects, from format 18 on.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        apart: Vec<crate::id::KeyId>,
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
        /// type.
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
        /// type changed after it.
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
        /// What kind of change fired it: the write at its `cause`. The
        /// store writes it on every firing from format 17 on; a firing of
        /// a format before carries none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        change: Option<ChangeKind>,
        /// Whether that write made the record: it found no live record
        /// under the key. Carried with `change`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created: Option<bool>,
    },
    /// A write would have made a trigger's condition hold, at a depth where
    /// a reaction to it could write nothing: the write's own depth is at
    /// or above the bound that applies, so no `trigger_fired` is appended
    /// (`docs/decisions/021-the-write-context-belongs-to-the-host.md`).
    /// Added in format 17.
    TriggerDropped {
        /// The trigger.
        trigger: TriggerName,
        /// The record's type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The record's key.
        key: RecordKey,
        /// The depth of the write that would have fired it.
        depth: u32,
        /// The bound that applied.
        max_depth: u32,
        /// Whose bound it is.
        bound: DepthBound,
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
        /// The output; `None` where `output_at` names it. An output of
        /// `null` is `Some(Null)`, and read back so.
        #[serde(
            default,
            deserialize_with = "crate::write::present",
            skip_serializing_if = "Option::is_none"
        )]
        output: Option<Value>,
        /// The position of the call's latest output, which a fill of an
        /// output the lake holds names instead of carrying it again
        /// (`docs/decisions/053-a-recorded-output-is-recorded-once.md`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_at: Option<Position>,
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
    LakeUpgraded => "lake_upgraded",
    LeaseTaken => "lease_taken",
    Declared => "declared",
    Migrated => "migrated",
    ValueTypeMigrated => "value_type_migrated",
    RecordUpserted => "record_upserted",
    RecordIngested => "record_ingested",
    SourceForgotten => "source_forgotten",
    ValuesRemoved => "values_removed",
    RecordRetired => "record_retired",
    RecordRestricted => "record_restricted",
    RecordUnrestricted => "record_unrestricted",
    VerbCalled => "verb_called",
    WriteProposed => "write_proposed",
    WriteAccepted => "write_accepted",
    WriteRejected => "write_rejected",
    TriggerFired => "trigger_fired",
    TriggerDropped => "trigger_dropped",
    ObjectSetChanged => "object_set_changed",
    OutputRecorded => "output_recorded",
    Checkpoint => "checkpoint",
}

/// What kind of change fired a trigger (lakelands#1217): the write at the
/// firing's `cause`, by the door it came through.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// An authored write: an upsert, a verb's write, an accepted one, an
    /// aging move.
    Upserted,
    /// A source's record, ingested.
    Ingested,
    /// A retirement, asked for or accepted, or a migration's of a link.
    Retired,
    /// A retirement of a record a forgotten source alone brought in.
    Forgotten,
    /// A retirement of a record a purged source brought in.
    Purged,
    /// A field filled from a recorded output, computed outside the lake.
    Recomputed,
}

/// Whose depth bound dropped a trigger's firing: the trigger's own
/// `max_depth`, or the lake's `max_reactive_depth`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DepthBound {
    /// The trigger's `max_depth`.
    Trigger,
    /// The lake's `max_reactive_depth`.
    Lake,
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
