//! The YAML documents of a bundle, one struct per lane, exactly as an
//! author writes them.
//!
//! Every struct refuses keys it does not know, so a misspelt or retired
//! key is an error naming the key, never a setting silently left at its
//! default. The author conveniences (a name as a mapping key, a field
//! type as a short string, a one-element list as a bare string) are
//! read here; turning them into contract types is `lower`'s work.

use std::fmt;
use std::marker::PhantomData;

use serde::Deserialize;
use serde::de::value::{MapAccessDeserializer, SeqAccessDeserializer};
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};

use lakelands_api::expr::{Condition, Expr};
use lakelands_api::ontology::{Authority, Cardinality, OnInputChange, Risk, StorageKind};

/// `value_types/<name>.yaml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ValueTypeDoc {
    pub name: String,
    pub kind: StorageKind,
    #[serde(default)]
    pub constraint: Option<Cond>,
    #[serde(default)]
    pub one_of: Vec<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub scale: Option<u32>,
    #[serde(default)]
    pub description: Option<String>,
}

/// `types/<name>.yaml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TypeDoc {
    pub name: String,
    #[serde(default)]
    pub key: OneOrMany,
    #[serde(default)]
    pub fields: Named<FieldDoc>,
    #[serde(default)]
    pub relations: Named<RelationDoc>,
    #[serde(default)]
    pub view: Option<ViewDoc>,
    #[serde(default)]
    pub valid_from: Option<String>,
    #[serde(default)]
    pub valid_to: Option<String>,
    #[serde(default = "yes")]
    pub field_meta: bool,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
}

fn yes() -> bool {
    true
}

/// A field: a bare type string, or the full form.
pub(super) type FieldDoc = StrOr<FieldBody>;

/// The full form of a field.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FieldBody {
    #[serde(rename = "type")]
    pub ty: TypeSpec,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub authority: Authority,
    #[serde(default)]
    pub derived: Option<Cond>,
    #[serde(default)]
    pub function: Option<String>,
    #[serde(default)]
    pub depends_on: OneOrMany,
    #[serde(default)]
    pub on_input_change: OnInputChange,
    #[serde(default)]
    pub sensitive: bool,
    #[serde(default)]
    pub searchable: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub embed: Option<EmbedDoc>,
}

/// What a vector field is the embedding of.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EmbedDoc {
    pub from: OneOrMany,
    pub model: String,
}

/// A field's type: the short string (`decimal`, `reference(partner)`,
/// `vector(1536)`, `iban`), or the contract's object form.
pub(super) type TypeSpec = StrOr<FieldTypeDoc>;

/// The object form of a field's type.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FieldTypeDoc {
    pub kind: StorageKind,
    #[serde(default)]
    pub value_type: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub dimensions: Option<u32>,
}

/// A relation, named by its key in the type's `relations` mapping.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RelationDoc {
    pub target: String,
    #[serde(default)]
    pub cardinality: Cardinality,
    #[serde(default)]
    pub inverse: Option<String>,
    #[serde(default)]
    pub via: Option<String>,
    #[serde(default)]
    pub fields: Named<FieldDoc>,
    /// Who writes the relation's links, when it has no `via`.
    #[serde(default)]
    pub authority: Authority,
    #[serde(default)]
    pub description: Option<String>,
}

/// A view type's definition.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ViewDoc {
    pub source: String,
    #[serde(default, rename = "where")]
    pub filter: Option<Cond>,
    #[serde(default)]
    pub group_by: OneOrMany,
    pub measures: Named<Cond>,
}

/// `verbs/<name>.yaml`. Exactly one of `set` (the verb writes fields of
/// its record) and `executor` (it reaches outside the lake).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct VerbDoc {
    pub name: String,
    pub on: String,
    pub risk: Risk,
    #[serde(default)]
    pub payload: Named<FieldDoc>,
    #[serde(default)]
    pub requires: Option<Cond>,
    #[serde(default)]
    pub set: Option<Named<Cond>>,
    #[serde(default)]
    pub executor: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// `triggers/<name>.yaml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TriggerDoc {
    pub name: String,
    pub on: String,
    #[serde(default)]
    pub when: Option<Cond>,
    #[serde(default)]
    pub description: Option<String>,
}

/// `object_sets/<name>.yaml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ObjectSetDoc {
    pub name: String,
    pub of: String,
    #[serde(rename = "where")]
    pub filter: Cond,
    #[serde(default)]
    pub description: Option<String>,
}

/// `functions/<name>.yaml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FunctionDoc {
    pub name: String,
    #[serde(default)]
    pub backing: Option<String>,
    #[serde(default)]
    pub inputs: Named<FieldDoc>,
    #[serde(default)]
    pub expr: Option<Cond>,
    #[serde(default)]
    pub config: Option<serde_json::Value>,
    #[serde(default)]
    pub description: Option<String>,
}

/// A string, or a mapping read as `T`. Errors inside the mapping keep
/// their own message (an unknown key is named), which an untagged enum
/// would lose.
#[derive(Debug)]
pub(super) enum StrOr<T> {
    /// The short form.
    Str(String),
    /// The mapping form.
    Map(T),
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for StrOr<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for V<T> {
            type Value = StrOr<T>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string or a mapping")
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<Self::Value, E> {
                Ok(StrOr::Str(s.to_owned()))
            }
            fn visit_map<A: MapAccess<'de>>(self, m: A) -> Result<Self::Value, A::Error> {
                T::deserialize(MapAccessDeserializer::new(m)).map(StrOr::Map)
            }
        }
        d.deserialize_any(V(PhantomData))
    }
}

/// An expression: the text form as a string, or the typed form as a
/// mapping.
#[derive(Debug)]
pub(super) struct Cond(pub Condition);

impl<'de> Deserialize<'de> for Cond {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Cond(match StrOr::<Expr>::deserialize(d)? {
            StrOr::Str(s) => Condition::Text(s),
            StrOr::Map(e) => Condition::Expr(e),
        }))
    }
}

/// A list of names, or one name written bare (`key: number`).
#[derive(Debug, Default)]
pub(super) struct OneOrMany(pub Vec<String>);

impl<'de> Deserialize<'de> for OneOrMany {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = OneOrMany;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a name or a list of names")
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<Self::Value, E> {
                Ok(OneOrMany(vec![s.to_owned()]))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, s: A) -> Result<Self::Value, A::Error> {
                Vec::deserialize(SeqAccessDeserializer::new(s)).map(OneOrMany)
            }
        }
        d.deserialize_any(V)
    }
}

/// Named entries written as a mapping from name to entry, kept in the
/// order the author wrote them. A name written twice is refused.
#[derive(Debug)]
pub(super) struct Named<T>(pub Vec<(String, T)>);

impl<T> Default for Named<T> {
    fn default() -> Self {
        Named(Vec::new())
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Named<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V<T>(PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for V<T> {
            type Value = Named<T>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a mapping from name to entry")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Self::Value, A::Error> {
                let mut out: Vec<(String, T)> = Vec::new();
                while let Some(name) = m.next_key::<String>()? {
                    if out.iter().any(|(n, _)| *n == name) {
                        return Err(de::Error::custom(format!("`{name}` appears twice")));
                    }
                    let entry = m.next_value::<T>()?;
                    out.push((name, entry));
                }
                Ok(Named(out))
            }
        }
        d.deserialize_map(V(PhantomData))
    }
}
