//! Writes and what they answer: authored writes, proposals, schema
//! changes and the preflight a migration must pass.

use serde::{Deserialize, Serialize};

use crate::expr::Condition;
use crate::id::{
    Cursor, FieldName, IdempotencyKey, Position, ProposalId, RecordKey, TypeName, VerbName, Version,
};
use crate::ontology::{FieldType, TriggerDecl, TypeDecl, ValueTypeDecl, VerbDecl};

use crate::record::Patch;
use crate::time::Timestamp;

/// Options every authored write takes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteOpts {
    /// Refuse the write unless the record is at this version
    /// (compare-and-set). For a record that must not exist yet, pass
    /// `Version(0)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<Version>,
    /// Make a retry of this write a no-op: a second write with the same
    /// key from the same principal answers the first write's outcome and
    /// appends nothing. The key is recorded in the log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<IdempotencyKey>,
}

/// What a write did. A writer never has to know in advance whether its
/// write runs or waits for review; the answer says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    /// The write took effect.
    Applied {
        /// The position it is visible from.
        position: Position,
        /// The record's new version, for a record write.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<Version>,
    },
    /// The write was recorded as a proposal and waits for review.
    Proposed {
        /// The position of the `WriteProposed` event.
        position: Position,
        /// The proposal to accept or reject.
        proposal: ProposalId,
        /// Why it was not applied at once (the verb's declared risk, a
        /// policy of the lake).
        why: String,
    },
}

impl Outcome {
    /// The position the outcome was recorded at.
    pub fn position(&self) -> Position {
        match self {
            Outcome::Applied { position, .. } | Outcome::Proposed { position, .. } => *position,
        }
    }
}

/// A write that can be proposed instead of applied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "write", rename_all = "snake_case")]
pub enum ProposedWrite {
    /// An `upsert`.
    Upsert {
        /// The record's type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The record's key.
        key: RecordKey,
        /// The fields it carries.
        patch: Patch,
        /// Its options.
        #[serde(default)]
        opts: WriteOpts,
    },
    /// A `retire`.
    Retire {
        /// The record's type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The record's key.
        key: RecordKey,
        /// Its options.
        #[serde(default)]
        opts: WriteOpts,
    },
    /// A verb call.
    Call {
        /// The verb.
        verb: VerbName,
        /// The record it is called on.
        key: RecordKey,
        /// Its payload.
        payload: Patch,
        /// Its options.
        #[serde(default)]
        opts: WriteOpts,
    },
}

/// Where a proposal stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalState {
    /// Waiting for review.
    Pending,
    /// Accepted; its write was applied.
    Accepted,
    /// Rejected; nothing was written.
    Rejected,
}

/// A proposal, as the log recorded it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Proposal {
    /// Its identifier.
    pub id: ProposalId,
    /// The position of its `WriteProposed` event.
    pub position: Position,
    /// Who proposed it, as `<kind>:<id>`.
    pub proposer: String,
    /// When.
    pub proposed_at: Timestamp,
    /// What it would write.
    pub write: ProposedWrite,
    /// Why it waits.
    pub why: String,
    /// Where it stands.
    pub state: ProposalState,
    /// Who decided it, and the reason given, once decided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
    /// The reason given on rejection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Which proposals to list.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ProposalFilter {
    /// Only proposals in this state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<ProposalState>,
    /// Only proposals touching this type.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub ty: Option<TypeName>,
    /// At most this many.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Order: newest first unless stated (`proposed_at`, `position`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order_by: Vec<crate::read::Order>,
    /// Continue after an earlier page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<Cursor>,
    /// The time axes.
    #[serde(default, flatten)]
    pub at: crate::read::ReadAt,
}

/// An object set: the records of one type matching a condition. When a
/// write moves a record into or out of the set, the store appends
/// `ObjectSetChanged` in the same append.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ObjectSetDecl {
    /// The set's name.
    pub name: String,
    /// The type of its members.
    pub of: TypeName,
    /// Which records are members.
    #[serde(rename = "where")]
    pub filter: Condition,
    /// What the set is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One declaration of a lake's ontology.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "declare", rename_all = "snake_case")]
pub enum Declaration {
    /// A type, new or revised without reshaping stored records.
    Type(TypeDecl),
    /// A value type.
    ValueType(ValueTypeDecl),
    /// A verb.
    Verb(VerbDecl),
    /// A trigger.
    Trigger(TriggerDecl),
    /// An object set.
    ObjectSet(ObjectSetDecl),
}

/// A change of a type's shape that stored records must follow.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Migration {
    /// The type's new declaration.
    pub to: TypeDecl,
    /// Fields renamed: old name to new name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renames: Vec<(FieldName, FieldName)>,
}

/// The preflight of a migration: what it would do to the stored records,
/// computed before anything changes. A migration is applied only when its
/// report is lossless, and only by a call that names the report it was
/// shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationReport {
    /// The type migrated.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// The schema hash before.
    pub from_schema_hash: String,
    /// The schema hash after.
    pub to_schema_hash: String,
    /// Records examined.
    pub records_scanned: u64,
    /// Per field whose type changes: how many values convert.
    pub fields: Vec<FieldConversion>,
    /// True when every stored value converts without loss.
    pub lossless: bool,
    /// Names this report; `migrate_apply` must repeat it.
    pub report_id: String,
}

/// How the values of one field convert to its new type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldConversion {
    /// The field.
    pub field: FieldName,
    /// Its type before.
    pub from: FieldType,
    /// Its type after.
    pub to: FieldType,
    /// Values that convert exactly.
    pub convertible: u64,
    /// Values that would lose information.
    pub not_convertible: u64,
    /// Up to five keys of records that would not convert. Never values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sample_keys: Vec<RecordKey>,
}

/// The acknowledgement of a batch `ingest` committed: one per
/// group-commit window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Committed {
    /// The position of the window's last event; everything in the
    /// window is visible from here.
    pub position: Position,
    /// How many records the window committed, duplicates excluded.
    pub records: u32,
    /// How many records were already in the log and appended nothing.
    pub duplicates: u32,
}
