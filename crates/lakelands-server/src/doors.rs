//! One door, run from its arguments: what MCP and REST share.
//!
//! Both transports turn a request into a [`Door`] and a JSON object of
//! arguments — an MCP tool call carries them as they are, a REST route
//! assembles them from its path, query, headers and body under the same
//! names — and [`run`] calls the catalog. So the two can never read an
//! argument differently, and a door's answer is the same JSON on both.

use std::fmt;

use lakelands_api::error::{Case, Error, Naming, Place, Refusal, Said};
use lakelands_api::id::{ProposalId, RecordKey, RelationName, TypeName};
use lakelands_api::ontology::{FieldDecl, TriggerDecl, TypeDecl, ValueTypeDecl, VerbDecl};
use lakelands_api::query::{Pattern, Search};
use lakelands_api::read::{Find, Get, ReadAt};
use lakelands_api::record::Patch;
use lakelands_api::tools::{Door, ToolSpec};
use lakelands_api::write::{
    Declaration, FunctionDecl, Migration, ObjectSetDecl, ProposalFilter, ProposedWrite,
    ValueTypeMigration, WriteOpts,
};
use lakelands_api::{Catalog, Principal};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde::de::value::StringDeserializer;
use serde::de::{
    DeserializeSeed, Deserializer, EnumAccess, IntoDeserializer, MapAccess, SeqAccess,
    VariantAccess, Visitor,
};
use serde_json::{Map, Value};

/// Why a door failed: the contract's closed set, or arguments that do
/// not fit the door, refused before it runs.
pub(crate) enum Fault {
    Contract(Error),
    Arguments(Box<Said>),
}

impl From<Error> for Fault {
    fn from(e: Error) -> Fault {
        Fault::Contract(e)
    }
}

impl Fault {
    /// Arguments that do not fit, refused with this case.
    pub(crate) fn arguments(said: impl Into<Said>) -> Fault {
        Fault::Arguments(Box::new(said.into()))
    }
}

/// The arguments, read as `T`; `door` names the door in the refusal, and
/// the refusal names where among the arguments it failed, such as
/// `declarations[2]`, and inside a declaration where in it.
pub(crate) fn args<T: DeserializeOwned>(door: &str, a: &Map<String, Value>) -> Result<T, Fault> {
    let v = Value::Object(a.clone());
    serde_path_to_error::deserialize(v.clone()).map_err(|e| {
        let at = path(&e);
        let mut said = misfit::<T>(v.clone());
        if said.is_none()
            && let Some((within, inner)) =
                located(&v, &e).and_then(|d| in_declaration(d, String::new()))
        {
            let at = join(at.as_deref().unwrap_or(""), &within);
            said = Some(((!at.is_empty()).then_some(at), inner));
        }
        let (at, said) = said.unwrap_or_else(|| {
            let fault = e.inner().into();
            (at, Case::ArgumentsUnfit { fault }.into())
        });
        let said = match at {
            Some(path) => said.within(Place::Member { path }),
            None => said,
        };
        Fault::arguments(said.within(Place::Arguments { tool: door.into() }))
    })
}

/// Where a read failed, as its path says it; `None` at the root.
fn path(e: &serde_path_to_error::Error<serde_json::Error>) -> Option<String> {
    let path = e.path().to_string();
    (path != ".").then_some(path)
}

/// `inner` under `outer`, as a path says it; `""` is the root.
fn join(outer: &str, inner: &str) -> String {
    match (outer, inner) {
        ("", inner) => inner.to_owned(),
        (outer, "") => outer.to_owned(),
        (outer, inner) if inner.starts_with('[') => [outer, inner].concat(),
        (outer, inner) => [outer, ".", inner].concat(),
    }
}

/// Why `v` does not read as `T`, when it is a member taken or needed or a
/// variant: read once more by [`Probe`], whose errors say so as data. The
/// path is where it sits.
fn misfit<T: DeserializeOwned>(v: Value) -> Option<(Option<String>, Said)> {
    let probed = T::deserialize(Probe(v, String::new())).err()?;
    let at = probed.at.clone().filter(|a| !a.is_empty());
    Some((at, probed.case()?.into()))
}

/// The value at the place a read failed.
fn located<'v>(
    v: &'v Value,
    e: &serde_path_to_error::Error<serde_json::Error>,
) -> Option<&'v Value> {
    use serde_path_to_error::Segment;
    e.path().iter().try_fold(v, |v, s| match s {
        Segment::Seq { index } => v.get(index),
        Segment::Map { key } | Segment::Enum { variant: key } => v.get(key.as_str()),
        Segment::Unknown => None,
    })
}

/// A declaration at or under `v` that does not read, at `at` below where
/// `v` sits: a declaration is read by hand, by its kind, so a failure
/// inside it is found by reading it again as that kind.
fn in_declaration(v: &Value, at: String) -> Option<(String, Said)> {
    match v {
        Value::Object(m) => {
            if let Some(Value::String(kind)) = m.get("declare") {
                let mut members = m.clone();
                members.remove("declare");
                let name = m.get("name").and_then(Value::as_str).map(str::to_owned);
                let (what, said) = declared(kind, Value::Object(members))?;
                return Some((at, said.within(Place::Declaration { what, name })));
            }
            m.iter().find_map(|(k, v)| in_declaration(v, join(&at, k)))
        }
        Value::Array(items) => items.iter().enumerate().find_map(|(i, v)| {
            let mut at = at.clone();
            at.push('[');
            at.push_str(&i.to_string());
            at.push(']');
            in_declaration(v, at)
        }),
        _ => None,
    }
}

/// Why the members of a declaration of `kind` do not read, if they do not.
fn declared(kind: &str, members: Value) -> Option<(Naming, Said)> {
    fn one<T: DeserializeOwned>(v: Value) -> Option<Said> {
        let e = serde_path_to_error::deserialize::<_, T>(v.clone()).err()?;
        let (at, said) = misfit::<T>(v).unwrap_or_else(|| {
            let fault = e.inner().into();
            (path(&e), Case::ArgumentsUnfit { fault }.into())
        });
        Some(match at {
            Some(path) => said.within(Place::Member { path }),
            None => said,
        })
    }
    let (what, said) = match kind {
        "type" => (Naming::Type, one::<TypeDecl>(members)),
        "value_type" => (Naming::ValueType, one::<ValueTypeDecl>(members)),
        "verb" => (Naming::Verb, one::<VerbDecl>(members)),
        "trigger" => (Naming::Trigger, one::<TriggerDecl>(members)),
        "object_set" => (Naming::ObjectSet, one::<ObjectSetDecl>(members)),
        "function" => (Naming::Function, one::<FunctionDecl>(members)),
        _ => return None,
    };
    Some((what, said?))
}

// ------------------------------------------------------------- the probe

/// A JSON value read as a parser reads it, whose errors are data: a
/// member a struct does not take, one it needs, a variant an enum does
/// not have, and where. It is a second reading of what the parser
/// refused, only to say why without its words.
struct Probe(Value, String);

/// Why the probe failed, and where, as the path of the value (`""` the
/// root).
#[derive(Debug)]
struct Probed {
    at: Option<String>,
    why: Why,
}

#[derive(Debug)]
enum Why {
    NoMember(String, &'static [&'static str]),
    Missing(&'static str),
    Variant(String, &'static [&'static str]),
    Other,
}

impl Probed {
    fn of(why: Why) -> Probed {
        Probed { at: None, why }
    }

    /// The same, at `at` unless it was said deeper.
    fn at(mut self, at: &str) -> Probed {
        self.at.get_or_insert_with(|| at.to_owned());
        self
    }

    fn case(self) -> Option<Case> {
        Some(match self.why {
            Why::NoMember(member, takes) => {
                let takes = (!takes.is_empty()).then(|| takes.iter().collect());
                Case::NoMember { member, takes }
            }
            Why::Missing(member) => Case::MemberMissing {
                member: member.into(),
            },
            Why::Variant(value, takes) => Case::NotOneOf {
                value: Value::String(value),
                allowed: takes.iter().collect(),
            },
            Why::Other => return None,
        })
    }
}

impl fmt::Display for Probed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the arguments do not read")
    }
}

impl std::error::Error for Probed {}

impl serde::de::Error for Probed {
    fn custom<T: fmt::Display>(_: T) -> Probed {
        Probed::of(Why::Other)
    }

    fn unknown_field(field: &str, expected: &'static [&'static str]) -> Probed {
        Probed::of(Why::NoMember(field.into(), expected))
    }

    fn missing_field(field: &'static str) -> Probed {
        Probed::of(Why::Missing(field))
    }

    fn unknown_variant(variant: &str, expected: &'static [&'static str]) -> Probed {
        Probed::of(Why::Variant(variant.into(), expected))
    }
}

fn text(s: String) -> StringDeserializer<Probed> {
    s.into_deserializer()
}

impl<'de> Deserializer<'de> for Probe {
    type Error = Probed;

    fn deserialize_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Probed> {
        let Probe(value, at) = self;
        match value {
            Value::Null => v.visit_unit(),
            Value::Bool(b) => v.visit_bool(b),
            Value::Number(n) => match (n.as_u64(), n.as_i64()) {
                (Some(u), _) => v.visit_u64(u),
                (None, Some(i)) => v.visit_i64(i),
                _ => v.visit_f64(n.as_f64().unwrap_or_default()),
            },
            Value::String(s) => v.visit_string(s),
            Value::Array(a) => v.visit_seq(Items(a.into_iter().enumerate(), at)),
            Value::Object(m) => v.visit_map(Members(m.into_iter(), None, at)),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Probed> {
        match self.0 {
            Value::Null => v.visit_none(),
            _ => v.visit_some(self),
        }
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _: &'static str,
        _: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, Probed> {
        let Probe(value, at) = self;
        match value {
            Value::String(name) => v.visit_enum(Variant(name, None, at)),
            Value::Object(m) if m.len() == 1 => {
                let (name, value) = m.into_iter().next().expect("one member");
                let at = join(&at, &name);
                v.visit_enum(Variant(name, Some(value), at))
            }
            value => Probe(value, at).deserialize_any(v),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        v: V,
    ) -> Result<V::Value, Probed> {
        v.visit_newtype_struct(self)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct seq tuple tuple_struct map struct
        identifier ignored_any
    }
}

struct Items(std::iter::Enumerate<std::vec::IntoIter<Value>>, String);

impl<'de> SeqAccess<'de> for Items {
    type Error = Probed;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Probed> {
        let Some((i, v)) = self.0.next() else {
            return Ok(None);
        };
        let mut at = self.1.clone();
        at.push('[');
        at.push_str(&i.to_string());
        at.push(']');
        let read = seed.deserialize(Probe(v, at.clone()));
        read.map(Some).map_err(|e| e.at(&at))
    }
}

struct Members(serde_json::map::IntoIter, Option<(String, Value)>, String);

impl<'de> MapAccess<'de> for Members {
    type Error = Probed;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Probed> {
        let Some((k, v)) = self.0.next() else {
            return Ok(None);
        };
        // A member refused by its name is refused where its struct sits.
        let read = seed
            .deserialize(text(k.clone()))
            .map_err(|e| e.at(&self.2))?;
        self.1 = Some((k, v));
        Ok(Some(read))
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Probed> {
        let (k, v) = self.1.take().ok_or_else(|| Probed::of(Why::Other))?;
        let at = join(&self.2, &k);
        seed.deserialize(Probe(v, at.clone()))
            .map_err(|e| e.at(&at))
    }
}

/// An enum's variant: its name, its value (none for a unit variant
/// written as a string) and where the value sits.
struct Variant(String, Option<Value>, String);

impl<'de> EnumAccess<'de> for Variant {
    type Error = Probed;
    type Variant = Variant;

    fn variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<(T::Value, Variant), Probed> {
        let name = seed.deserialize(text(self.0.clone()))?;
        Ok((name, self))
    }
}

impl<'de> VariantAccess<'de> for Variant {
    type Error = Probed;

    fn unit_variant(self) -> Result<(), Probed> {
        Ok(())
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, Probed> {
        seed.deserialize(Probe(self.1.unwrap_or_default(), self.2))
    }

    fn tuple_variant<V: Visitor<'de>>(self, _: usize, v: V) -> Result<V::Value, Probed> {
        Probe(self.1.unwrap_or_default(), self.2).deserialize_any(v)
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        _: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, Probed> {
        Probe(self.1.unwrap_or_default(), self.2).deserialize_any(v)
    }
}

/// An argument the catalog checks itself, before it reads anything, even
/// where the tool's schema does not list it, so that its refusal says
/// why: a read's `params` on a type that is not a view with parameters is
/// refused saying the type declares none, and a `key` to a verb on its
/// type naming the verb.
fn catalog_answers(door: &Door, argument: &str) -> bool {
    matches!(
        (door, argument),
        (Door::Find { .. } | Door::Get { .. }, "params") | (Door::Call { .. }, "key")
    )
}

pub(crate) fn to_json<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).expect("contract types serialise")
}

/// Refuse an argument the door's input schema does not list, before
/// anything runs: a misspelled `expected_version` must not turn a
/// compare-and-set into an unconditional write. The exceptions are the
/// catalog's to answer ([`catalog_answers`]).
pub(crate) fn unknown_argument(spec: &ToolSpec, arguments: &Map<String, Value>) -> Option<Fault> {
    let listed = spec.input_schema["properties"].as_object();
    let key = arguments.keys().find(|k| {
        !catalog_answers(&spec.door, k) && !listed.is_some_and(|p| p.contains_key(k.as_str()))
    })?;
    Some(Fault::arguments(Case::NoArgument {
        tool: spec.name.clone(),
        argument: key.clone(),
    }))
}

/// Run one catalog door with its arguments, as `who`. `label` names the
/// door in a refusal of the arguments. [`Door::Pin`], [`Door::Subscribe`] and
/// [`Door::Unsubscribe`] belong to the MCP session, which answers them
/// itself; here they are refused.
pub(crate) async fn run(
    c: &dyn Catalog,
    who: &Principal,
    door: &Door,
    label: &str,
    a: Map<String, Value>,
) -> Result<Value, Fault> {
    #[derive(Deserialize)]
    struct Write {
        key: RecordKey,
        #[serde(default)]
        fields: Patch,
        #[serde(flatten)]
        opts: WriteOpts,
    }
    // A call's key is optional: a verb on its type takes none.
    #[derive(Deserialize)]
    struct Called {
        #[serde(default)]
        key: Option<RecordKey>,
        #[serde(default)]
        payload: Patch,
        #[serde(flatten)]
        opts: WriteOpts,
    }
    #[derive(Deserialize)]
    struct Link {
        key: RecordKey,
        to: RecordKey,
        #[serde(default)]
        fields: Patch,
        #[serde(flatten)]
        opts: WriteOpts,
    }
    Ok(match door {
        Door::Find { ty } => to_json(&c.find(who, ty, args::<Find>(label, &a)?).await?),
        Door::Get { ty } => to_json(&c.get(who, ty, args::<Get>(label, &a)?).await?),
        Door::Upsert { ty } => {
            let w: Write = args(label, &a)?;
            to_json(&c.upsert(who, ty, &w.key, w.fields, w.opts).await?)
        }
        Door::Retire { ty } => {
            let w: Write = args(label, &a)?;
            to_json(&c.retire(who, ty, &w.key, w.opts).await?)
        }
        Door::Link { ty, relation } => {
            let l: Link = args(label, &a)?;
            to_json(
                &c.link(who, ty, &l.key, relation, &l.to, l.fields, l.opts)
                    .await?,
            )
        }
        Door::Unlink { ty, relation } => {
            let l: Link = args(label, &a)?;
            to_json(&c.unlink(who, ty, &l.key, relation, &l.to, l.opts).await?)
        }
        Door::Call { verb } => {
            let w: Called = args(label, &a)?;
            to_json(&c.call(who, verb, w.key.as_ref(), w.payload, w.opts).await?)
        }
        Door::Describe => {
            #[derive(Deserialize)]
            struct D {
                #[serde(default, rename = "type")]
                ty: Option<TypeName>,
                #[serde(flatten)]
                at: ReadAt,
            }
            let d: D = args(label, &a)?;
            match d.ty {
                Some(ty) => to_json(&c.describe_type(who, &ty, d.at).await?),
                None => to_json(&c.describe(who, d.at).await?),
            }
        }
        Door::Pin => {
            let tool = label.into();
            return Err(Fault::arguments(Case::NoSessionToPin { tool }));
        }
        Door::Subscribe | Door::Unsubscribe => {
            let tool = label.into();
            return Err(Fault::arguments(Case::NoSession { tool }));
        }
        Door::Query => to_json(&c.query(who, args::<Pattern>(label, &a)?).await?),
        Door::Search => to_json(&c.search(who, args::<Search>(label, &a)?).await?),
        Door::Propose => {
            #[derive(Deserialize)]
            struct P {
                write: ProposedWrite,
                why: String,
            }
            let p: P = args(label, &a)?;
            to_json(&c.propose(who, p.write, p.why).await?)
        }
        Door::Proposals => to_json(&c.proposals(who, args::<ProposalFilter>(label, &a)?).await?),
        Door::Accept | Door::Reject => {
            #[derive(Deserialize)]
            struct D {
                proposal: ProposalId,
                #[serde(default)]
                reason: String,
            }
            let d: D = args(label, &a)?;
            if *door == Door::Accept {
                to_json(&c.accept(who, &d.proposal).await?)
            } else {
                to_json(&c.reject(who, &d.proposal, d.reason).await?)
            }
        }
        Door::DeclareType => {
            #[derive(Deserialize)]
            struct D {
                #[serde(rename = "type")]
                ty: TypeDecl,
            }
            let d: D = args(label, &a)?;
            to_json(&c.declare(who, Declaration::Type(d.ty)).await?)
        }
        Door::Declare => {
            // No tool's schema checks these routes' members, so the
            // arguments refuse one they do not take.
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct D {
                declaration: Declaration,
            }
            let d: D = args(label, &a)?;
            to_json(&c.declare(who, d.declaration).await?)
        }
        Door::Apply => {
            // No tool's schema checks these routes' members, so the
            // arguments refuse one they do not take.
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct D {
                declarations: Vec<Declaration>,
            }
            let d: D = args(label, &a)?;
            to_json(&c.apply(who, d.declarations).await?)
        }
        Door::Recompute => {
            let what: lakelands_api::write::Recompute = args(label, &a)?;
            to_json(&c.recompute(who, what).await?)
        }
        Door::AddField => {
            #[derive(Deserialize)]
            struct D {
                #[serde(rename = "type")]
                ty: TypeName,
                field: FieldDecl,
            }
            let d: D = args(label, &a)?;
            to_json(&c.add_field(who, &d.ty, d.field).await?)
        }
        Door::Migrate => {
            #[derive(Deserialize)]
            struct M {
                #[serde(default)]
                to: Option<TypeDecl>,
                #[serde(default)]
                value_type: Option<ValueTypeDecl>,
                // A type's fields, or a value type's values.
                #[serde(default)]
                renames: Vec<(String, String)>,
                #[serde(default)]
                relation_renames: Vec<(RelationName, RelationName)>,
                #[serde(default)]
                report_id: Option<String>,
            }
            let m: M = args(label, &a)?;
            let wrong = |field: &str, case: Case| {
                Fault::Contract(Error::Refused {
                    field: Some(field.into()),
                    reason: Refusal::Invalid(Box::new(case.into())),
                })
            };
            match (m.to, m.value_type) {
                (Some(to), None) => {
                    let renames = m.renames.into_iter();
                    let migration = Migration {
                        to,
                        renames: renames.map(|(a, b)| (a.into(), b.into())).collect(),
                        relation_renames: m.relation_renames,
                    };
                    match m.report_id {
                        Some(r) => to_json(&c.migrate_apply(who, migration, &r).await?),
                        None => to_json(&c.migrate(who, migration).await?),
                    }
                }
                (None, Some(vt)) => {
                    if !m.relation_renames.is_empty() {
                        return Err(wrong("relation_renames", Case::ValueTypeNoRelations));
                    }
                    let vm = ValueTypeMigration {
                        to: vt,
                        renames: m.renames,
                    };
                    match m.report_id {
                        Some(r) => to_json(&c.migrate_value_type_apply(who, vm, &r).await?),
                        None => to_json(&c.migrate_value_type(who, vm).await?),
                    }
                }
                (Some(_), Some(_)) => {
                    return Err(wrong("value_type", Case::MigrationBoth));
                }
                (None, None) => {
                    return Err(wrong("to", Case::MigrationNeither));
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use lakelands_api::engine::OpenMode;
    use lakelands_api::seam::Seams;
    use lakelands_api::{Grants, LakeStore, Location, PrincipalKind, WriteContext};
    use serde_json::json;

    use super::*;

    fn owner(lake: &str) -> Principal {
        Principal {
            kind: PrincipalKind::Human,
            id: "owner".into(),
            lake: lake.into(),
            grants: Grants::owner(),
        }
    }

    /// The mock, and the store over the memory and SQLite engines.
    async fn lakes() -> Vec<(&'static str, Arc<dyn LakeStore>)> {
        let dir =
            std::env::temp_dir().join(format!("lakelands-migrate-door-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let seams = || Seams {
            engines: vec![
                Arc::new(lakelands_engine_mem::MemoryOpener::new()),
                Arc::new(lakelands_engine_sqlite::SqliteOpener),
            ],
            ..Seams::default()
        };
        vec![
            (
                "mock",
                Arc::new(lakelands_mock::MockLake::new("doors")) as Arc<dyn LakeStore>,
            ),
            (
                "memory",
                lakelands_store::open(Location::memory("doors"), seams(), OpenMode::Create)
                    .await
                    .unwrap(),
            ),
            (
                "sqlite",
                lakelands_store::open(Location::sqlite("doors", &dir), seams(), OpenMode::Create)
                    .await
                    .unwrap(),
            ),
        ]
    }

    /// The `migrate` tool names a type (`to`) or a value type
    /// (`value_type`), one of the two, and a value type renames values,
    /// never relations: anything else is refused as invalid, naming the
    /// argument, on the mock and on the store alike.
    #[tokio::test(flavor = "multi_thread")]
    async fn migrate_takes_a_type_or_a_value_type_never_both_and_never_neither() {
        let vt = json!({"name": "status_code", "kind": "text", "one_of": ["open", "late"]});
        let ty = json!({"name": "memo", "fields": [{"name": "text", "type": {"kind": "text"}}]});
        for (name, store) in lakes().await {
            let cat = store.catalog(WriteContext::default());
            let who = owner("doors");
            cat.declare(&who, serde_json::from_value(json!({"declare": "value_type", "name": "status_code", "kind": "text", "one_of": ["open", "late", "void"]})).unwrap())
                .await
                .unwrap();
            let call = |a: Value| {
                let (cat, who) = (cat.clone(), who.clone());
                async move {
                    let a = a.as_object().cloned().unwrap_or_default();
                    run(cat.as_ref(), &who, &Door::Migrate, "migrate", a).await
                }
            };
            for (args, field) in [
                (json!({"to": ty, "value_type": vt}), "value_type"),
                (json!({}), "to"),
                (json!({"report_id": "r"}), "to"),
                (
                    json!({"value_type": vt, "relation_renames": [["a", "b"]]}),
                    "relation_renames",
                ),
            ] {
                let answer = call(args.clone()).await;
                assert!(
                    matches!(
                        &answer,
                        Err(Fault::Contract(Error::Refused { field: Some(f), reason: Refusal::Invalid(_) }))
                            if f.as_str() == field
                    ),
                    "on {name}, {args} was not refused as invalid naming `{field}`"
                );
            }
            let report = call(json!({"value_type": vt})).await;
            assert!(
                matches!(&report, Ok(v) if v["data"]["fits"] == json!(true)),
                "on {name}, the value type's preflight did not answer"
            );
            let renamed = call(json!({"value_type": vt, "renames": [["void", "late"]]})).await;
            assert!(
                matches!(&renamed, Ok(v) if v["data"]["fits"] == json!(true)),
                "on {name}, the value type's preflight with renames did not answer"
            );
        }
    }
}
