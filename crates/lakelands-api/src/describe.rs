//! `describe`: the only metadata read. `describe(lake)` lists what is
//! there; each type is described on six axes — structure, access,
//! rights, actions, lineage and lifecycle — as one principal may see it.
//! The tool catalog is generated from this answer and nothing else.

use serde::{Deserialize, Serialize};

use crate::expr::Condition;
use crate::id::{FieldName, LakeId, RelationName, TypeName, VerbName};
use crate::ontology::{
    Authority, EmbedDecl, FieldDecl, FieldType, OnInputChange, RelationDecl, Risk, TriggerDecl,
    ValueTypeDecl, ViewDecl,
};
use crate::principal::Access;
use crate::write::{FunctionDecl, ObjectSetDecl};

/// What a lake holds, as one principal may see it. Types, fields and
/// verbs the principal holds no grant for are absent, not listed as
/// forbidden. Functions, triggers and object sets are answered as
/// declared, each listed only to a principal who reads every field its
/// declaration names or holds the `schema` grant: a condition or an
/// input list says what the fields it names are compared with, which is
/// no business of a principal who may not read them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LakeDescription {
    /// The lake.
    pub lake: LakeId,
    /// The types.
    pub types: Vec<TypeDescription>,
    /// The verbs the principal may call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verbs: Vec<VerbDescription>,
    /// The value types in use.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub value_types: Vec<ValueTypeDecl>,
    /// The functions in force.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<FunctionDecl>,
    /// The triggers in force, on types the principal sees.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<TriggerDecl>,
    /// The object sets in force, of types the principal sees.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub object_sets: Vec<ObjectSetDecl>,
    /// What the principal may do beyond the per-type doors.
    pub may: LakeRights,
}

/// Rights of the principal beyond any one type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LakeRights {
    /// Review proposals (`accept`, `reject`).
    pub review: bool,
    /// Change the ontology (`declare_type`, `add_field`, `migrate`).
    pub schema: bool,
    /// Feed the lake through `ingest`.
    pub ingest: bool,
}

/// One type, on six axes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TypeDescription {
    /// The type's name.
    pub name: TypeName,
    /// What it means.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A short label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The language of its searchable text, as a BCP-47 tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// For a view type: the view as declared, to a principal who reads
    /// every source field it names or holds the `schema` grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<ViewDecl>,
    /// Structure: the fields this principal may see, and the relations.
    pub fields: Vec<FieldDescription>,
    /// Structure: relations to other types.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relations: Vec<RelationDecl>,
    /// Access: how records are addressed and read.
    pub access: TypeAccess,
    /// Rights: which doors the principal may use on this type.
    pub may: TypeRights,
    /// Actions: the verbs callable on records of this type.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verbs: Vec<VerbName>,
    /// Lineage: the authority records of this type are written by, and
    /// for a view the type it is computed from.
    pub lineage: TypeLineage,
    /// Lifecycle: version and shape.
    pub lifecycle: TypeLifecycle,
}

/// One field, as the principal may see it: the declaration whole for a
/// field it reads, the shape only for a masked one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldDescription {
    /// The field's name.
    pub name: FieldName,
    /// Its type.
    #[serde(rename = "type")]
    pub ty: FieldType,
    /// Whether it is required.
    pub required: bool,
    /// Which authority writes it.
    pub authority: Authority,
    /// This principal's access to it.
    pub access: Access,
    /// For a derived field: its expression.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived: Option<Condition>,
    /// For a derived field: the declared function that computes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    /// The fields it depends on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<FieldName>,
    /// What happens to it when an input changes.
    pub on_input_change: OnInputChange,
    /// Whether `where` and `order_by` may use it.
    pub filterable: bool,
    /// Whether full-text search covers it.
    pub searchable: bool,
    /// Whether its value is masked, not dropped, for a principal who may
    /// not read it.
    pub sensitive: bool,
    /// What it means.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A short label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// For an embedding: the text fields it is made from, and the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embed: Option<EmbedDecl>,
}

impl FieldDescription {
    /// The description of a declared field for a principal with `access`.
    /// A masked field answers its shape only — name, type, whether it is
    /// required and sensitive, its authority and what an input change does
    /// to it — never what it is computed from or what it means
    /// (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`).
    pub fn of(decl: &FieldDecl, access: Access) -> FieldDescription {
        use crate::ontology::StorageKind::{Bytes, Json, Vector};
        if !access.reads() {
            return FieldDescription {
                name: decl.name.clone(),
                ty: decl.ty.clone(),
                required: decl.required,
                authority: decl.authority,
                access,
                derived: None,
                function: None,
                depends_on: vec![],
                on_input_change: decl.on_input_change,
                filterable: false,
                searchable: false,
                sensitive: decl.sensitive,
                description: None,
                label: None,
                embed: None,
            };
        }
        FieldDescription {
            name: decl.name.clone(),
            ty: decl.ty.clone(),
            required: decl.required,
            authority: decl.authority,
            access,
            derived: decl.derived.clone(),
            function: decl.function.clone(),
            depends_on: decl.depends_on.clone(),
            on_input_change: decl.on_input_change,
            filterable: !matches!(decl.ty.kind, Bytes | Json | Vector),
            searchable: decl.searchable,
            sensitive: decl.sensitive,
            description: decl.description.clone(),
            label: decl.label.clone(),
            embed: decl.embed.clone(),
        }
    }

    /// Whether the principal may write the field through an authored
    /// door (`upsert`, a verb).
    pub fn authored_writable(&self) -> bool {
        self.access.writes() && matches!(self.authority, Authority::Authored | Authority::Shared)
    }
}

/// How records of a type are addressed and read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TypeAccess {
    /// The fields that form the key; empty when the writer gives it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub key: Vec<FieldName>,
    /// The field holding business start time, if valid-timed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<FieldName>,
    /// The field holding business end time, if declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<FieldName>,
    /// Whether `search` covers the type lexically.
    pub searchable: bool,
    /// Whether per-field provenance is kept (`with: meta`).
    pub field_meta: bool,
}

/// The doors one principal may use on one type.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeRights {
    /// `find` and `search`.
    pub find: bool,
    /// `get`.
    pub get: bool,
    /// `upsert`: at least one authored field is writable.
    pub upsert: bool,
    /// `retire`: every field of the type is writable, whatever its
    /// authority, as the principal's grants resolve it. A principal who
    /// may write only some fields of a record may not remove all of them.
    pub retire: bool,
    /// `link` and `unlink`: the relations carried by link records whose
    /// links the principal may write through those doors.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub link: Vec<RelationName>,
}

/// Where a type's records come from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TypeLineage {
    /// Whether any field is fed by a source.
    pub sourced: bool,
    /// For a view type: the type it is computed from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view_of: Option<TypeName>,
}

/// A type's version and shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeLifecycle {
    /// The declaration's version: one more for every change to it.
    pub version: u32,
    /// Identifies the shape (`docs/decisions/007-schema-hash-covers-shape.md`).
    pub schema_hash: String,
    /// The fields the lake keeps an order index for, by its
    /// `sort_index.<type>` setting: a `find` ordered first by one of them
    /// reads its page, not the type
    /// (`docs/decisions/041-a-sorted-page-reads-its-page.md`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sorted: Vec<FieldName>,
    /// The vector fields the lake asks an index for, by its
    /// `vector_index.<type>` setting: a search over one of them answers
    /// the index's set and says `ranked_by: index`; on an engine that keeps
    /// no such index that set is the exact ranking.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indexed: Vec<FieldName>,
}

/// One verb, as a caller sees it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VerbDescription {
    /// The verb.
    pub name: VerbName,
    /// The type of record it is called on.
    pub on: TypeName,
    /// Its payload's fields.
    pub payload: Vec<FieldDescription>,
    /// Its declared risk: `low` runs, anything else is proposed.
    pub risk: Risk,
    /// Whether it reaches outside the lake.
    pub external: bool,
    /// What it does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}
