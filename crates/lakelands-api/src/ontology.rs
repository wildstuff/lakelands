//! The map of a lake: what it may hold and who may write what.
//!
//! A lake's ontology is a set of declarations: types with fields and
//! relations, value types over storage kinds, verbs with a declared
//! risk, and triggers. Declarations enter the log as events, so every
//! position has exactly one ontology in force.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::expr::Condition;
use crate::id::{
    FieldName, ParamName, RelationName, TriggerName, TypeName, ValueTypeName, VerbName,
};

/// A host's own marks on a declaration: a map from a namespaced key
/// (`acme.icon`) to a JSON value. Lakelands stores, replays and describes
/// them and never reads them: an annotation grants nothing and restricts
/// nothing, and is meaning, never shape
/// (`docs/decisions/007-schema-hash-covers-shape.md`, extended).
pub type Annotations = BTreeMap<String, serde_json::Value>;

/// The most canonical bytes all the annotations of one declaration may
/// hold together. A constant of the contract, not a lake's setting, so a
/// declaration one lake takes every lake takes.
pub const ANNOTATIONS_MAX_BYTES: usize = 16 * 1024;

/// The longest an annotation key may be, in bytes.
pub const ANNOTATION_KEY_MAX_BYTES: usize = 128;

/// The namespaces no host may use for an annotation key.
pub const RESERVED_ANNOTATION_NAMESPACES: &[&str] = &["sys", "lakelands"];

/// How an engine stores, compares and indexes a value. The set is closed:
/// a kind exists only when an engine must treat it differently from every
/// other kind (`docs/decisions/011-storage-kind-and-value-type.md`).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum StorageKind {
    /// UTF-8 text.
    #[default]
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
#[serde(deny_unknown_fields)]
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
    /// A named format every value must satisfy (`email`, `iban`,
    /// `domain`, `url`). A name the store does not know is refused when
    /// the value type is declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// For decimal: the number of fractional digits every value is kept
    /// at. A written value with more digits is rounded half away from
    /// zero (the rule money is kept by); a derived value is rounded before
    /// the next derived field reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<u32>,
    /// What the value type means, for people and agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A field's type: a storage kind, optionally refined by a value type
/// declared over that kind. A `reference` must name its `target`, a
/// `vector` its `dimensions` (greater than zero); neither member may be
/// given for another kind. Its default, as `StorageKind`'s, is text.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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

impl Authority {
    /// Whether this is the default, `authored`.
    pub fn is_authored(&self) -> bool {
        *self == Authority::Authored
    }
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
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// For `derived`, instead of `derived`: the declared function that
    /// computes it. The function's inputs are read from the record's fields
    /// of the same names; an `expr` function is recomputed on replay like
    /// any derived expression.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
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
    /// For a `derived` vector: the text fields it is the embedding of, and
    /// the model that makes it. Not part of the schema hash: it says how
    /// the value is computed, not what the field is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embed: Option<EmbedDecl>,
    /// The host's own marks; never read by Lakelands.
    #[serde(default, skip_serializing_if = "Annotations::is_empty")]
    pub annotations: Annotations,
}

impl FieldDecl {
    /// A field of one storage kind, every other member at its default:
    /// build one as `FieldDecl { searchable: true, ..FieldDecl::of(name, kind) }`,
    /// so a member added later does not break the literal.
    pub fn of(name: impl Into<FieldName>, kind: StorageKind) -> FieldDecl {
        FieldDecl {
            name: name.into(),
            ty: FieldType::kind(kind),
            ..FieldDecl::default()
        }
    }
}

/// What a vector field is the embedding of
/// (`docs/decisions/022-functions-are-declarations.md`, extended).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmbedDecl {
    /// The text fields of the same record whose values, joined in this
    /// order with one blank line, are embedded.
    pub from: Vec<FieldName>,
    /// The model the vectors come from; the bound embedding backend must
    /// name it.
    pub model: String,
}

impl TypeDecl {
    /// A type with no field yet, every other member at its default:
    /// build one as `TypeDecl { fields: …, ..TypeDecl::named(name) }`, so a
    /// member added later does not break the literal.
    pub fn named(name: impl Into<TypeName>) -> TypeDecl {
        TypeDecl {
            name: name.into(),
            ..TypeDecl::default()
        }
    }

    /// The first of this type's embeddings declared against the rules,
    /// with why: an embedding is a `derived` vector with dimensions,
    /// neither derived from an expression nor computed by a function,
    /// with no `depends_on` beside its `from`; `from` names, each once,
    /// text fields of the type other than itself, sensitive only where
    /// the embedding is; and it names its model. `None` when every
    /// embedding keeps them. Every door checks this before it asks
    /// whether a backend is bound.
    pub fn embedding_problem(&self) -> Option<(FieldName, String)> {
        self.fields.iter().find_map(|f| {
            let why = embedding_problem(self, f)?;
            Some((
                f.name.clone(),
                format!("type `{}`, field `{}`: {why}", self.name, f.name),
            ))
        })
    }
}

fn embedding_problem(t: &TypeDecl, f: &FieldDecl) -> Option<String> {
    let e = f.embed.as_ref()?;
    if f.ty.kind != StorageKind::Vector || f.ty.dimensions.is_none() {
        return Some("only a vector with dimensions is the embedding of text".into());
    }
    if f.derived.is_some() || f.function.is_some() {
        return Some(
            "a field is an embedding, derived from an expression or computed by a function, \
             only one"
                .into(),
        );
    }
    if f.authority != Authority::Derived {
        return Some("an embedding has the authority `derived`".into());
    }
    if !f.depends_on.is_empty() {
        return Some(
            "an embedding depends on its `from` fields; `depends_on` is not written beside it"
                .into(),
        );
    }
    if e.from.is_empty() {
        return Some("an embedding names the fields it is made `from`".into());
    }
    if e.model.trim().is_empty() {
        return Some("an embedding names its model".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for name in &e.from {
        if !seen.insert(name) {
            return Some(format!("it names `{name}` in `from` twice"));
        }
        if name == &f.name {
            return Some("it cannot be made from itself".into());
        }
        let Some(source) = t.fields.iter().find(|d| &d.name == name) else {
            return Some(format!(
                "it is made from `{name}`, which is not one of its fields"
            ));
        };
        if source.ty.kind != StorageKind::Text {
            return Some(format!("it is made from `{name}`, which is not text"));
        }
        // A vector gives its text away to whoever may read it.
        if source.sensitive && !f.sensitive {
            return Some(format!(
                "it is made from the sensitive field `{name}` and is not sensitive itself"
            ));
        }
    }
    None
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
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// Fields of the link itself, for a relation carried by link records
    /// (no `via`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<FieldDecl>,
    /// Who writes the links of a relation carried by link records:
    /// `authored` (the default) through the link doors, `source` by
    /// ingest (`docs/decisions/035-a-link-is-a-record.md`). Not part of the
    /// schema hash.
    #[serde(default, skip_serializing_if = "Authority::is_authored")]
    pub authority: Authority,
    /// What the relation means. Not part of the schema hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The host's own marks; never read by Lakelands.
    #[serde(default, skip_serializing_if = "Annotations::is_empty")]
    pub annotations: Annotations,
    /// Instead of a carrier: the fields whose values connect the two
    /// types. The store resolves it when a walk reads it and stores no
    /// edge (`docs/decisions/044-a-relation-may-be-derived-from-matching-values.md`).
    /// Not part of the schema hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived: Option<Derived>,
}

/// What connects the records of a derived relation: every pair of fields
/// holds the same non-null value on both sides.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Derived {
    /// The pairs, one or more.
    #[serde(rename = "match")]
    pub pairs: Vec<MatchPair>,
}

/// One pair of a derived relation's match.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchPair {
    /// A field of the owning type.
    pub from: FieldName,
    /// A field of the target type, of the same storage kind.
    pub to: FieldName,
}

impl RelationDecl {
    /// A relation to `target`, every other member at its default: build
    /// one as `RelationDecl { inverse: …, ..RelationDecl::to(name, target) }`,
    /// so a member added later does not break the literal.
    pub fn to(name: impl Into<RelationName>, target: impl Into<TypeName>) -> RelationDecl {
        RelationDecl {
            name: name.into(),
            target: target.into(),
            ..RelationDecl::default()
        }
    }
}

/// A view type: records computed from other types, read with `find` like
/// any other type and never written.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewDecl {
    /// The type the view reads.
    pub source: TypeName,
    /// The values a read binds, which `where` and `measures` read with
    /// `param('name')`, in declared order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<ViewParam>,
    /// Which source records it covers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#where: Option<Condition>,
    /// Source fields whose values form one view record each. Without
    /// them the view is one record, keyed [`crate::key::UNGROUPED`], over
    /// no source record too.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub group_by: Vec<FieldName>,
    /// The view's fields, each an expression over its group. They are
    /// computed after the merged sources' measures, which they may read by
    /// name.
    pub measures: Vec<Measure>,
    /// Further sources the view merges on its key, each grouped by fields
    /// of its own that line up with `group_by` (018).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merge: Vec<ViewSource>,
}

/// A further source of a view: its records, filtered by its `where`,
/// grouped by its `group_by`, whose fields line up with the view's
/// `group_by` by position, and measured by its own aggregates. Its groups
/// meet the view's other sources' where their keys are identical.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewSource {
    /// The type it reads.
    pub source: TypeName,
    /// Which of its records it covers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#where: Option<Condition>,
    /// Its fields whose values form the view's key, in the view's order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub group_by: Vec<FieldName>,
    /// Its measures, aggregates over its group, named uniquely in the view.
    pub measures: Vec<Measure>,
}

/// One parameter of a view: a value of a scalar kind that every read of
/// the view binds.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewParam {
    /// Its name.
    pub name: ParamName,
    /// Its kind: `text`, `number`, `decimal`, `boolean` or `date_time`.
    pub kind: StorageKind,
}

/// One computed field of a view.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Measure {
    /// The view field it produces.
    pub name: FieldName,
    /// How it is computed.
    pub expr: Condition,
}

/// A type: the shape of one kind of record.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// If set, this is a view type computed from another type. Boxed, as
    /// few types are views and a view is the largest member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<Box<ViewDecl>>,
    /// The field that holds a record's business start time, if the type
    /// is valid-timed; `valid_to` is its end, if declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<FieldName>,
    /// The field that holds a record's business end time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<FieldName>,
    /// The language of the type's searchable text, as a BCP-47 tag
    /// (`de`, `en`, `de-CH`). Full-text search stems by it; without it,
    /// text is only split and lower-cased (`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
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
    /// The host's own marks; never read by Lakelands.
    #[serde(default, skip_serializing_if = "Annotations::is_empty")]
    pub annotations: Annotations,
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

/// What a verb is called on: one record of its type, or its type
/// (`docs/decisions/013-verbs-run-by-declared-risk.md`, extended). A verb
/// on its type takes no key, and its effect reaches outside the lake.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerbScope {
    /// One record, named by its key.
    #[default]
    Record,
    /// The type, with no record: an effect that creates something
    /// outside the lake, for which no record exists yet.
    Type,
}

impl VerbScope {
    /// Whether this is the default, which a declaration never writes.
    pub fn is_record(&self) -> bool {
        *self == VerbScope::Record
    }
}

/// One field a verb sets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    /// The field set.
    pub field: FieldName,
    /// Its new value.
    pub value: Condition,
}

/// A verb: a named action on a record of one type, or on the type itself,
/// with a typed payload. Over MCP each verb is its own tool.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "VerbWire")]
pub struct VerbDecl {
    /// The verb's name (`pay_invoice`).
    pub name: VerbName,
    /// Its type: the type of the record it is called on, or the type it
    /// is called on.
    pub on: TypeName,
    /// Whether it is called on a record of `on` (the default, never
    /// written) or on `on` itself (log format 8).
    #[serde(default, skip_serializing_if = "VerbScope::is_record")]
    pub scope: VerbScope,
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

/// A verb as it is read: the effect's members beside the verb's own, each
/// refused where the effect does not take it. serde cannot refuse an
/// unknown member of a struct that flattens another, so the verb is read
/// through this and then built.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VerbWire {
    name: VerbName,
    on: TypeName,
    #[serde(default)]
    scope: VerbScope,
    #[serde(default)]
    payload: Vec<FieldDecl>,
    risk: Risk,
    #[serde(default)]
    requires: Option<Condition>,
    effect: EffectKind,
    #[serde(default)]
    set: Option<Vec<Assignment>>,
    #[serde(default)]
    executor: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum EffectKind {
    Write,
    External,
}

impl TryFrom<VerbWire> for VerbDecl {
    type Error = String;

    fn try_from(w: VerbWire) -> Result<VerbDecl, String> {
        let effect = match (w.effect, w.set, w.executor) {
            (EffectKind::Write, Some(set), None) => VerbEffect::Write { set },
            (EffectKind::External, None, Some(executor)) => VerbEffect::External { executor },
            (EffectKind::Write, _, Some(_)) => {
                return Err("a verb whose `effect` is `write` has no member `executor`".into());
            }
            (EffectKind::External, Some(_), _) => {
                return Err("a verb whose `effect` is `external` has no member `set`".into());
            }
            (EffectKind::Write, None, None) => return Err("missing field `set`".into()),
            (EffectKind::External, None, None) => return Err("missing field `executor`".into()),
        };
        Ok(VerbDecl {
            name: w.name,
            on: w.on,
            scope: w.scope,
            payload: w.payload,
            risk: w.risk,
            requires: w.requires,
            effect,
            description: w.description,
        })
    }
}

/// A trigger: a condition the store watches. When a write makes it
/// hold, the store appends `TriggerFired` in the same append. What
/// reacts to it is the host's business.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerDecl {
    /// The trigger's name.
    pub name: TriggerName,
    /// The type whose writes it watches.
    pub on: TypeName,
    /// A guard over the written record; `changed(f)` and `previous(f)`
    /// (the value before the write, null for a record the write creates)
    /// are available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<Condition>,
    /// What the trigger is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// How deep a reaction to this trigger's firing may be: a reactive
    /// write whose `cause` names a firing of it, deeper than this, is
    /// refused `too_deep`. At most the lake's `max_reactive_depth`
    /// (`docs/decisions/021-the-write-context-belongs-to-the-host.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<u32>,
}
