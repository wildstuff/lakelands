//! Records as a caller reads them, with the provenance of every field.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::id::{FieldName, Position, RecordKey, SourceId, TypeName, Version};
use crate::time::Timestamp;

/// A field's value. Every storage kind has one JSON form: text, numbers,
/// booleans and JSON as themselves; decimals, date-times, bytes (base64)
/// and references as strings; vectors as arrays of numbers; geo as a
/// GeoJSON object.
pub type Value = serde_json::Value;

/// Which authority wrote a value: the `origin` of its provenance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// A source feed, through `ingest`.
    Source,
    /// An author, through `upsert`, a verb or an accepted proposal.
    Authored,
    /// The fold, from other fields.
    Derived,
    /// The store itself: declarations, leases, checkpoints, triggers.
    System,
}

/// The provenance of one field of one record
/// (`docs/decisions/010-provenance-per-field.md`). The fold materialises
/// it from the event that last wrote the field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldMeta {
    /// Which authority wrote the value.
    pub origin: Origin,
    /// When the lake learned it: the writing event's wall time.
    pub known_at: Timestamp,
    /// When its source observed it, for a value from a source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen_at: Option<Timestamp>,
    /// The writing event's exact coordinate.
    pub position: Position,
    /// The principal that wrote it, as `<kind>:<id>`.
    pub writer: String,
    /// The source it came from, for a value from a source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceId>,
    /// For a value a procedure produced: which version of it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub procedure_version: Option<String>,
    /// For a field a function computes that holds no value because its
    /// backing failed: why (`docs/decisions/010-provenance-per-field.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One record, as one principal may see it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    /// The record's type.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// The record's key.
    pub key: RecordKey,
    /// Its compare-and-set token.
    pub version: Version,
    /// The values this principal may read. A field with no value is
    /// absent.
    pub fields: BTreeMap<FieldName, Value>,
    /// Fields that hold a value this principal may not read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub masked: Vec<FieldName>,
    /// Per-field provenance, filled when the read asked `with: meta`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub meta: BTreeMap<FieldName, FieldMeta>,
}

/// A record as an engine stores it: every field, every flag, no masking.
/// Only the store and the engines see this form.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredRecord {
    /// The record's type.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// The record's key.
    pub key: RecordKey,
    /// Its compare-and-set token.
    pub version: Version,
    /// Every value.
    pub fields: BTreeMap<FieldName, Value>,
    /// Every field's provenance (empty if the type opted out).
    #[serde(default)]
    pub meta: BTreeMap<FieldName, FieldMeta>,
    /// The position of the last event that changed the record.
    pub position: Position,
    /// Retired: invisible to reads at or after `position`.
    #[serde(default)]
    pub retired: bool,
    /// Withheld from principals without the `restricted` grant.
    #[serde(default)]
    pub restricted: bool,
}

impl StoredRecord {
    /// The state of a key a re-key moved away from: retired, at version 0,
    /// with no field (`docs/log-format.md` §2.3). It ends the key's
    /// history, so a later record at the key starts at version 1; it is
    /// no record, and every scan of stored history skips it.
    pub fn ended(ty: TypeName, key: RecordKey, position: Position) -> StoredRecord {
        StoredRecord {
            ty,
            key,
            version: Version(0),
            fields: BTreeMap::new(),
            meta: BTreeMap::new(),
            position,
            retired: true,
            restricted: false,
        }
    }

    /// Whether this is an ended key rather than a record.
    pub fn is_ended(&self) -> bool {
        self.version.0 == 0
    }
}

/// A sparse patch: the fields a write carries. A field absent from the
/// patch is unchanged; a field set to `null` is cleared
/// (`docs/decisions/012-writes-touch-only-their-fields.md`).
pub type Patch = BTreeMap<FieldName, Value>;

/// One record from a source, as `ingest` receives it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceRecord {
    /// The record's type.
    #[serde(rename = "type")]
    pub ty: TypeName,
    /// The record's key.
    pub key: RecordKey,
    /// The source's fields, as a sparse patch.
    pub fields: Patch,
    /// When the source observed these values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen_at: Option<Timestamp>,
}
