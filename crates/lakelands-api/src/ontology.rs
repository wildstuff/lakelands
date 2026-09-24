//! The map of a lake: what it may hold and who may write what.
//!
//! A lake's ontology is a set of declarations: types with fields and
//! relations, value types over storage kinds, verbs with a declared
//! risk, and triggers. Declarations enter the log as events, so every
//! position has exactly one ontology in force.

use serde::{Deserialize, Serialize};

use crate::expr::Condition;
use crate::id::{FieldName, RelationName, TriggerName, TypeName, ValueTypeName, VerbName};

/// How an engine stores, compares and indexes a value. The set is closed:
/// a kind exists only when an engine must treat it differently from every
/// other kind (`docs/decisions/011-storage-kind-and-value-type.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    /// UTF-8 text.
    Text,
    /// A binary floating-point number.
    Number,
    /// An exact decimal, carried as a string of digits.
    Decimal,
    /// `true` or `false`.
    Boolean,
    /// A UTC instant.
    DateTime,
    /// Opaque bytes, base64 on the wire.
    Bytes,
    /// Any JSON value, stored whole.
    Json,
    /// The key of a record of another type.
    Reference,
    /// A fixed-length vector of numbers, for similarity search.
    Vector,
    /// A GeoJSON geometry.
    Geo,
}

impl StorageKind {
    /// Every kind, in declaration order.
    pub const ALL: [StorageKind; 10] = [
        StorageKind::Text,
        StorageKind::Number,
        StorageKind::Decimal,
        StorageKind::Boolean,
        StorageKind::DateTime,
        StorageKind::Bytes,
        StorageKind::Json,
        StorageKind::Reference,
        StorageKind::Vector,
        StorageKind::Geo,
    ];
}

/// A value type: what a value means and what it must satisfy, declared
/// in the bundle over a storage kind. The set is open; adding one needs
/// no engine change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ValueTypeDecl {
    /// The value type's name (`iban`, `money`, `email`).
    pub name: ValueTypeName,
    /// The storage kind it is stored as.
    pub kind: StorageKind,
    /// A constraint every value must satisfy; `value` names the value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraint: Option<Condition>,
    /// For text: the closed set of allowed values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub one_of: Vec<String>,
    /// A display format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// What the value type means, for people and agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A field's type: a storage kind, optionally refined by a value type
/// declared over that kind.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FieldType {
    /// How the value is stored.
    pub kind: StorageKind,
    /// What the value means, if refined.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_type: Option<ValueTypeName>,
    /// For `reference`: the type referred to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<TypeName>,
    /// For `vector`: its length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<u32>,
}

impl FieldType {
    /// A bare storage kind.
    pub fn kind(kind: StorageKind) -> FieldType {
        FieldType {
            kind,
            value_type: None,
            target: None,
            dimensions: None,
        }
    }
}

/// Which authority writes a field. It decides which door may carry the
/// field: a source feed through `ingest`, an author through `upsert` or a
/// verb, the store itself through the fold. Grants then decide which
/// principals of that authority may.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Authority {
    /// Written by authors, through `upsert` and verbs.
    #[default]
    Authored,
    /// Mirrored from a source, through `ingest` only. A re-import never
    /// overwrites an authored field.
    Source,
    /// Either door may write it; the later position wins.
    Shared,
    /// Computed by the fold from other fields; no door writes it.
    Derived,
}

/// What the fold does to a field when one of its inputs changes
/// (`docs/decisions/012-writes-touch-only-their-fields.md`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnInputChange {
    /// The value stays; its `known_at` shows it is older than its inputs.
    #[default]
    Keep,
    /// The fold clears the value in the same event that changes an input.
    Clear,
}

/// One field of a type or of a relation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldDecl {
    /// The field's name.
    pub name: FieldName,
    /// How it is stored and what it means.
    #[serde(rename = "type")]
    pub ty: FieldType,
    /// Whether every live record must carry a value.
    #[serde(default)]
    pub required: bool,
    /// Which authority writes it.
    #[serde(default)]
    pub authority: Authority,
    /// For `derived`: the expression the fold computes it from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived: Option<Condition>,
    /// The fields this one depends on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<FieldName>,
    /// What happens to this field when one of `depends_on` changes.
    #[serde(default)]
    pub on_input_change: OnInputChange,
    /// Whether values are masked for principals without read access
    /// rather than dropped.
    #[serde(default)]
    pub sensitive: bool,
    /// Whether full-text search covers this field.
    #[serde(default)]
    pub searchable: bool,
    /// What the field means, for people and agents. Not part of the
    /// schema hash (`docs/decisions/007-schema-hash-covers-shape.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A short label. Not part of the schema hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// How many records a relation links on each side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cardinality {
    /// One to one.
    OneToOne,
    /// One of this type to many of the target.
    OneToMany,
    /// Many of this type to one of the target.
    #[default]
    ManyToOne,
    /// Many to many.
    ManyToMany,
}

/// A relation from one type to another. A relation may carry fields of
/// its own (the date a partner was assigned, the share of a payment).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RelationDecl {
    /// The relation's name, unique within its type.
    pub name: RelationName,
    /// The type it points to.
    pub target: TypeName,
    /// How many on each side.
    #[serde(default)]
    pub cardinality: Cardinality,
    /// The name the relation has when walked backwards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inverse: Option<RelationName>,
    /// The field of this type that holds the target's key, if the
    /// relation is carried by a reference field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<FieldName>,
    /// Fields of the link itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<FieldDecl>,
    /// What the relation means. Not part of the schema hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A view type: records computed from other types, read with `find` like
/// any other type and never written.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ViewDecl {
    /// The type the view reads.
    pub source: TypeName,
    /// Which source records it covers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#where: Option<Condition>,
    /// Source fields whose values form one view record each.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub group_by: Vec<FieldName>,
    /// The view's fields, each an expression over its group.
    pub measures: Vec<Measure>,
}

/// One computed field of a view.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Measure {
    /// The view field it produces.
    pub name: FieldName,
    /// How it is computed.
    pub expr: Condition,
}

/// A type: the shape of one kind of record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TypeDecl {
    /// The type's name. `sys.` names are reserved.
    pub name: TypeName,
    /// The fields whose values form the record key. Empty means the key
    /// is given by the writer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub key: Vec<FieldName>,
    /// The fields.
    #[serde(default)]
    pub fields: Vec<FieldDecl>,
    /// The relations to other types.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<RelationDecl>,
    /// If set, this is a view type computed from another type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<ViewDecl>,
    /// The field that holds a record's business start time, if the type
    /// is valid-timed; `valid_to` is its end, if declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<FieldName>,
    /// The field that holds a record's business end time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<FieldName>,
    /// Whether the fold materialises per-field provenance for this type.
    /// On by default; a high-volume type may opt out
    /// (`docs/decisions/010-provenance-per-field.md`).
    #[serde(default = "yes")]
    pub field_meta: bool,
    /// What the type means. Not part of the schema hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A short label. Not part of the schema hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

fn yes() -> bool {
    true
}

/// How dangerous a verb is, as its author declares it
/// (`docs/decisions/013-verbs-run-by-declared-risk.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    /// Runs at once when its effect is writing authored fields.
    Low,
    /// Lands as a proposal.
    Medium,
    /// Lands as a proposal.
    High,
}

/// What a verb does when it runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "effect", rename_all = "snake_case")]
pub enum VerbEffect {
    /// Writes authored fields of the record it is called on: each field
    /// is set from an expression over the payload (`payload.<name>`) and
    /// the record.
    Write {
        /// The fields set, and how.
        set: Vec<Assignment>,
    },
    /// Reaches outside the lake through the host's
    /// [`VerbExecutor`](crate::seam::VerbExecutor).
    External {
        /// The executor the host binds for it.
        executor: String,
    },
}

/// One field a verb sets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Assignment {
    /// The field set.
    pub field: FieldName,
    /// Its new value.
    pub value: Condition,
}

/// A verb: a named action on a record of one type, with a typed payload.
/// Over MCP each verb is its own tool.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VerbDecl {
    /// The verb's name (`pay_invoice`).
    pub name: VerbName,
    /// The type of the record it is called on.
    pub on: TypeName,
    /// The payload's fields.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub payload: Vec<FieldDecl>,
    /// Its declared risk; the caller cannot lower it.
    pub risk: Risk,
    /// A condition the record must meet for the verb to be called.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<Condition>,
    /// What it does.
    #[serde(flatten)]
    pub effect: VerbEffect,
    /// What the verb does, for people and agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A trigger: a condition the store watches. When a write makes it
/// hold, the store appends `TriggerFired` in the same append. What
/// reacts to it is the host's business.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TriggerDecl {
    /// The trigger's name.
    pub name: TriggerName,
    /// The type whose writes it watches.
    pub on: TypeName,
    /// A guard over the written record; `changed(f)` is available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<Condition>,
    /// What the trigger is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}
