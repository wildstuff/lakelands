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
/// by guessing. Version 2 adds `migrated.rekeyed` (`docs/log-format.md`
/// §6).
pub const LOG_FORMAT_VERSION: u32 = 2;

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
        /// The record it was called on.
        key: RecordKey,
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

impl EventBody {
    /// The kind's name as the log and the change feed spell it.
    pub fn kind(&self) -> &'static str {
        match self {
            EventBody::LakeCreated { .. } => "lake_created",
            EventBody::LeaseTaken { .. } => "lease_taken",
            EventBody::Declared { .. } => "declared",
            EventBody::Migrated { .. } => "migrated",
            EventBody::RecordUpserted { .. } => "record_upserted",
            EventBody::RecordIngested { .. } => "record_ingested",
            EventBody::RecordRetired { .. } => "record_retired",
            EventBody::RecordRestricted { .. } => "record_restricted",
            EventBody::RecordUnrestricted { .. } => "record_unrestricted",
            EventBody::VerbCalled { .. } => "verb_called",
            EventBody::WriteProposed { .. } => "write_proposed",
            EventBody::WriteAccepted { .. } => "write_accepted",
            EventBody::WriteRejected { .. } => "write_rejected",
            EventBody::TriggerFired { .. } => "trigger_fired",
            EventBody::ObjectSetChanged { .. } => "object_set_changed",
            EventBody::OutputRecorded { .. } => "output_recorded",
            EventBody::Checkpoint { .. } => "checkpoint",
        }
    }
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
mod tests {
    use super::*;

    fn event() -> serde_json::Value {
        serde_json::json!({
            "v": 1, "lake": "finance", "position": 0, "id": "0".repeat(32),
            "prev": Digest::zero(), "known_at": "2026-09-24T00:00:00.000000Z",
            "writer": "system:lakelands", "origin": "system",
            "context": {"origin": "direct", "depth": 0}, "fence": 0,
            "body": {"kind": "lake_created", "format": 1}, "hash": Digest::zero(),
        })
    }

    #[test]
    fn a_complete_event_reads() {
        let e: Event = serde_json::from_value(event()).unwrap();
        assert_eq!(e.body, EventBody::LakeCreated { format: 1 });
    }

    #[test]
    fn unknown_members_and_kinds_are_refused_and_context_is_required() {
        let mut extra = event();
        extra["colour"] = serde_json::json!("blue");
        assert!(serde_json::from_value::<Event>(extra).is_err());

        let mut body = event();
        body["body"]["extra"] = serde_json::json!(1);
        assert!(serde_json::from_value::<Event>(body).is_err());

        let mut kind = event();
        kind["body"] = serde_json::json!({"kind": "record_teleported"});
        assert!(serde_json::from_value::<Event>(kind).is_err());

        let mut no_context = event();
        no_context.as_object_mut().unwrap().remove("context");
        assert!(serde_json::from_value::<Event>(no_context).is_err());
    }

    #[test]
    fn every_kind_serialises_under_the_name_it_reports() {
        let bodies = [
            EventBody::LakeCreated { format: 1 },
            EventBody::WriteAccepted {
                proposal: ProposalId::from("p1"),
            },
            EventBody::RecordRetired {
                ty: "invoice".into(),
                key: "INV-1".into(),
                version: Version(3),
            },
        ];
        for body in bodies {
            let json = serde_json::to_value(&body).unwrap();
            assert_eq!(json["kind"], body.kind());
        }
    }
}
