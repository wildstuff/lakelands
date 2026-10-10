//! The map of a lake: what it may hold and who may write what.
//!
//! A lake's ontology is a set of declarations: types with fields and
//! relations, value types over storage kinds, verbs with a declared
//! risk, and triggers. Declarations enter the log as events, so every
//! position has exactly one ontology in force.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{Case, Place, Said};
use crate::expr::{Condition, Func};
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
    /// A calendar date, `YYYY-MM-DD`: an instant at a UTC midnight, kept
    /// in the form a person writes it.
    Date,
}

impl StorageKind {
    /// Every kind, in declaration order.
    pub const ALL: [StorageKind; 11] = [
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
        StorageKind::Date,
    ];
}

/// Whether `v` is a component of a vector value: a number whose `f32`
/// is finite. A search reads a vector as `f32`s, so a number beyond their
/// range would be infinite there and score every hit `NaN`. Every vector
/// that enters a lake goes through this, written or computed.
pub fn is_vector_component(v: &serde_json::Value) -> bool {
    v.as_f64().is_some_and(|x| (x as f32).is_finite())
}

/// The kind as a declaration names it: `text`, `date_time`.
impl std::fmt::Display for StorageKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            StorageKind::Text => "text",
            StorageKind::Number => "number",
            StorageKind::Decimal => "decimal",
            StorageKind::Boolean => "boolean",
            StorageKind::DateTime => "date_time",
            StorageKind::Bytes => "bytes",
            StorageKind::Json => "json",
            StorageKind::Reference => "reference",
            StorageKind::Vector => "vector",
            StorageKind::Geo => "geo",
            StorageKind::Date => "date",
        })
    }
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
            ..FieldType::default()
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

/// When a computed field is computed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComputedAt {
    /// By the fold, at each write that changes its inputs: the value is
    /// stored, and the clock it reads is the write's.
    Write,
    /// At every read, from the record as stored and the read's clock: the
    /// value is never stored, so a log, a feed and an export hold none.
    Read,
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
    /// For a computed field: when it is computed. Left out, at the write,
    /// and an expression that reads the clock is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub computed_at: Option<ComputedAt>,
    /// The fields this one depends on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<FieldName>,
    /// What happens to this field when one of `depends_on` changes.
    #[serde(default)]
    pub on_input_change: OnInputChange,
    /// The value the field takes when its record comes to exist and the
    /// creating write gives it none. On an authored field of a type's own
    /// alone (`docs/decisions/050-a-field-declares-its-initial-value.md`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial: Option<serde_json::Value>,
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

    /// Whether it is computed at every read ([`ComputedAt::Read`]).
    pub fn at_read(&self) -> bool {
        self.computed_at == Some(ComputedAt::Read)
    }
}

/// The members of a field's declaration that say what it is computed
/// from, what it starts as, what it means or how the host marked it,
/// listed once: one who may know of a field and not read it is shown none
/// of them, wherever a declaration is answered
/// (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`).
macro_rules! unread {
    ($($member:ident),*) => {
        impl FieldDecl {
            /// The members shown only to one who may read the field, as
            /// they serialise.
            pub const UNREAD: &[&str] = &[$(stringify!($member)),*];

            /// Leaves the declaration its shape alone: its name, type,
            /// authority, whether it is required and sensitive and what
            /// an input change does to it.
            pub fn shape_alone(&mut self) {
                $(self.$member = Default::default();)*
            }
        }
    };
}
unread! { derived, function, computed_at, depends_on, initial, searchable, description, label, embed, annotations }

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
    /// The fields a record of the type is keyed by: its `key`, or, of a
    /// view, its `group_by`, none where it groups by nothing. A view's
    /// `key` names the same or is left out; one that came in past
    /// `declare` and says otherwise keys nothing (lakelands#782,
    /// lakelands#788).
    pub fn key_fields(&self) -> &[FieldName] {
        match &self.view {
            Some(v) => &v.group_by,
            None => &self.key,
        }
    }

    /// Of a view, where its key's values come from: its source and the
    /// fields it groups by, then each merge source and the fields it
    /// groups by, which line up with them position by position (018).
    /// None of a stored type.
    pub fn key_sources(&self) -> Vec<(&TypeName, &[FieldName])> {
        let Some(v) = &self.view else {
            return Vec::new();
        };
        let merged = v.merge.iter().map(|m| (&m.source, m.group_by.as_slice()));
        std::iter::once((&v.source, v.group_by.as_slice()))
            .chain(merged)
            .collect()
    }

    /// Of a view, the view fields the measure making `field` reads by
    /// name, its own or a merged source's: a field is built from them as a
    /// derived field from its `depends_on`, so one built from a measure its
    /// reader may not know of is not known either (024, lakelands#782).
    /// None of a stored type.
    pub fn measure_reads(&self, field: &FieldName) -> Vec<FieldName> {
        let Some(v) = &self.view else {
            return Vec::new();
        };
        let mut measures = (v.measures.iter()).chain(v.merge.iter().flat_map(|s| &s.measures));
        let expr = measures.find(|m| &m.name == field).map(|m| m.expr.typed());
        let Some(Ok(expr)) = expr else {
            return Vec::new();
        };
        let reads = expr.fields_read_by_name().into_iter();
        reads.filter(|n| *n != field).cloned().collect()
    }

    /// The types this declared shape names against the one in force,
    /// `held` (lakelands#762): itself and, of a view, its sources; and the
    /// target of each relation and reference field it adds, changes or
    /// drops. Both are compared completed, as a lake keeps a type (each
    /// computed field's `depends_on` filled), so a relation or a field
    /// restated as it stands names nothing the type in force does not. The
    /// store judges a proposal's types by it.
    pub fn names_against(&self, held: Option<&TypeDecl>) -> Vec<TypeName> {
        let (rels, fields) = held.map_or((&[][..], &[][..]), |h| (&h.relations[..], &h.fields[..]));
        let moved = |a: &[RelationDecl], b: &[RelationDecl]| {
            let moved = a.iter().filter(|r| !b.contains(r));
            moved.map(|r| r.target.clone()).collect::<Vec<_>>()
        };
        let refs = |a: &[FieldDecl], b: &[FieldDecl]| {
            let moved = a.iter().filter(|f| !b.contains(f));
            moved
                .filter_map(|f| f.ty.target.clone())
                .collect::<Vec<_>>()
        };
        let view = self
            .view
            .iter()
            .flat_map(|v| std::iter::once(&v.source).chain(v.merge.iter().map(|m| &m.source)));
        std::iter::once(self.name.clone())
            .chain(view.cloned())
            .chain(moved(&self.relations, rels))
            .chain(moved(rels, &self.relations))
            .chain(refs(&self.fields, fields))
            .chain(refs(fields, &self.fields))
            .collect()
    }

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
    /// with why, said at the type and the field: an embedding is a
    /// `derived` vector with dimensions, neither derived from an
    /// expression nor computed by a function, with no `depends_on` beside
    /// its `from`; `from` names, each once, text fields of the type other
    /// than itself, sensitive only where the embedding is; and it names
    /// its model. `None` when every embedding keeps them. Every door
    /// checks this before it asks whether a backend is bound.
    pub fn embedding_problem(&self) -> Option<(FieldName, Said)> {
        self.fields.iter().find_map(|f| {
            let case = embedding_problem(self, f)?;
            let field = Place::Field {
                name: f.name.clone(),
            };
            let ty = Place::Type {
                name: self.name.clone(),
            };
            Some((f.name.clone(), Said::from(case).within(field).within(ty)))
        })
    }

    /// The first of this type's fields that says when it is computed
    /// against the rules, with why, said at the type, the relation if a
    /// link's, and the field. A computed field whose expression, or
    /// `expr` function, reads the clock says when. One computed at the
    /// read is computed at every read from the record as stored, so no
    /// write gives it and nothing that reads stored records names it: it
    /// has the authority `derived`, an expression computes it, and it is
    /// not required, sensitive by itself, cleared or searched; it is no
    /// key, validity bound, `via` or derived relation's match, no field of
    /// a view or of a link; and no field computed at the write reads it.
    /// `None` when every field keeps them. Every door checks this.
    pub fn computed_at_problem<'f>(
        &self,
        function: impl Fn(&str) -> Option<&'f crate::write::FunctionDecl>,
    ) -> Option<(FieldName, Said)> {
        let ty = || Place::Type {
            name: self.name.clone(),
        };
        let said = |at: &[Place], f: &FieldDecl, case: Case| {
            let mut said = Said::from(case).within(Place::Field {
                name: f.name.clone(),
            });
            for place in at.iter().rev() {
                said = said.within(place.clone());
            }
            Some((f.name.clone(), said))
        };
        let view = self
            .view
            .as_ref()
            .and(self.fields.iter().find(|f| f.computed_at.is_some()));
        if let Some(f) = view {
            return said(&[ty()], f, Case::ComputedAtInView);
        }
        for r in &self.relations {
            let relation = Place::Relation {
                name: r.name.clone(),
            };
            let at = [ty(), relation];
            for f in &r.fields {
                if f.at_read() {
                    return said(&at, f, Case::LinkFieldAtRead);
                }
                if let Some(case) = computed_at_problem(&r.fields, f, &function) {
                    return said(&at, f, case);
                }
            }
        }
        let pairs = self
            .relations
            .iter()
            .flat_map(|r| r.derived.iter().flat_map(|d| &d.pairs));
        let named = (self.key.iter().map(|n| (n, Case::AtReadKey)))
            .chain(self.valid_from.iter().map(|n| (n, Case::AtReadValidFrom)))
            .chain(self.valid_to.iter().map(|n| (n, Case::AtReadValidTo)))
            .chain(
                self.relations
                    .iter()
                    .filter_map(|r| Some((r.via.as_ref()?, Case::AtReadVia))),
            )
            .chain(pairs.map(|p| (&p.from, Case::AtReadMatch)));
        for (n, case) in named {
            if let Some(f) = self.fields.iter().find(|f| &f.name == n && f.at_read()) {
                return said(&[ty()], f, case);
            }
        }
        self.fields.iter().find_map(|f| {
            let case = computed_at_problem(&self.fields, f, &function)?;
            said(&[ty()], f, case)
        })
    }
}

/// What of `computed_at_problem` one field of `fields` breaks by itself.
fn computed_at_problem<'f>(
    fields: &[FieldDecl],
    f: &FieldDecl,
    function: &dyn Fn(&str) -> Option<&'f crate::write::FunctionDecl>,
) -> Option<Case> {
    let called = f.function.as_deref().and_then(function);
    let backed = called.is_some_and(|func| func.backing != "expr");
    let expr = match (&f.derived, called) {
        (Some(c), _) => c.typed().ok(),
        (None, Some(func)) => func.expr.as_ref().and_then(|c| c.typed().ok()),
        _ => None,
    };
    let reads: Vec<&FieldName> = match (&expr, called, &f.embed) {
        (Some(e), _, _) => e.fields_read().into_iter().collect(),
        (None, Some(func), _) => func.inputs.iter().map(|i| &i.name).collect(),
        (_, _, Some(embed)) => embed.from.iter().collect(),
        _ => Vec::new(),
    };
    let at_read = |n: &&FieldName| fields.iter().any(|d| &d.name == *n && d.at_read());
    match f.computed_at {
        Some(_) if f.derived.is_none() && f.function.is_none() => {
            return Some(Case::ComputedAtUncomputed);
        }
        None if expr.as_ref().is_some_and(crate::expr::Expr::reads_clock) => {
            return Some(Case::ClockUnsaid);
        }
        Some(ComputedAt::Read) => {}
        _ => {
            let field = reads.into_iter().find(at_read)?.clone();
            return Some(Case::AtWriteReadsAtRead { field });
        }
    }
    let write = expr.is_some_and(|e| e.calls(&[Func::Previous, Func::Changed]));
    Some(if f.authority != Authority::Derived {
        Case::AtReadAuthority
    } else if backed {
        Case::AtReadBacked
    } else if write {
        Case::AtReadPrevious
    } else if f.required {
        Case::AtReadRequired
    } else if f.sensitive {
        Case::AtReadSensitive
    } else if f.on_input_change == OnInputChange::Clear {
        Case::AtReadCleared
    } else if f.searchable {
        Case::AtReadSearchable
    } else {
        return None;
    })
}

fn embedding_problem(t: &TypeDecl, f: &FieldDecl) -> Option<Case> {
    let e = f.embed.as_ref()?;
    if f.ty.kind != StorageKind::Vector || f.ty.dimensions.is_none() {
        return Some(Case::EmbedNotVector);
    }
    if f.derived.is_some() || f.function.is_some() {
        return Some(Case::EmbedAlsoComputed);
    }
    if f.authority != Authority::Derived {
        return Some(Case::EmbedNotDerived);
    }
    if !f.depends_on.is_empty() {
        return Some(Case::EmbedDependsOn);
    }
    if e.from.is_empty() {
        return Some(Case::EmbedNoFrom);
    }
    if e.model.trim().is_empty() {
        return Some(Case::EmbedNoModel);
    }
    let mut seen = std::collections::BTreeSet::new();
    for name in &e.from {
        let field = name.clone();
        if !seen.insert(name) {
            return Some(Case::EmbedFromTwice { field });
        }
        if name == &f.name {
            return Some(Case::EmbedFromItself);
        }
        // Not one of the type's fields: as one it does not have.
        let Some(source) = t.fields.iter().find(|d| &d.name == name) else {
            let ty = t.name.clone();
            return Some(Case::UnknownField { ty, field });
        };
        if source.ty.kind != StorageKind::Text {
            return Some(Case::EmbedFromNotText { field });
        }
        // A vector gives its text away to whoever may read it.
        if source.sensitive && !f.sensitive {
            return Some(Case::EmbedFromSensitive { field });
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
    /// The name the relation has when walked backwards: a step naming it
    /// walks the relation the other way, unless the type it stands on
    /// declares a relation of that name.
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

impl ViewDecl {
    /// The definition without each measure, its own or a merged source's,
    /// whose view field `known` does not answer: shown beside a type's
    /// fields, it names none that they leave out (lakelands#782).
    pub fn keep_measures(&mut self, known: impl Fn(&FieldName) -> bool) {
        self.measures.retain(|m| known(&m.name));
        for s in &mut self.merge {
            s.measures.retain(|m| known(&m.name));
        }
    }
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
    /// Its kind: `text`, `number`, `decimal`, `boolean`, `date_time` or
    /// `date`.
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
    /// The `date_time` or `date` field that holds a record's business
    /// start time, if the type is valid-timed; `valid_to` is its end, if
    /// declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<FieldName>,
    /// The `date_time` or `date` field that holds a record's business end
    /// time.
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
    /// written) or on `on` itself.
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

impl VerbDecl {
    /// The record's fields the verb reads: those its `requires` reads, and
    /// those its set values read but for `payload.<name>`, a member of the
    /// call's payload. `None` where one of them does not parse.
    pub fn fields_read(&self) -> Option<std::collections::BTreeSet<FieldName>> {
        let set = match &self.effect {
            VerbEffect::Write { set } => set.as_slice(),
            VerbEffect::External { .. } => &[],
        };
        let mut read = std::collections::BTreeSet::new();
        if let Some(r) = &self.requires {
            read.extend(r.typed().ok()?.fields_read().into_iter().cloned());
        }
        for a in set {
            read.extend(
                a.value
                    .typed()
                    .ok()?
                    .record_fields_read()
                    .into_iter()
                    .cloned(),
            );
        }
        Some(read)
    }
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
