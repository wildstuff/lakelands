//! One door, run from its arguments: what MCP and REST share.
//!
//! Both transports turn a request into a [`Door`] and a JSON object of
//! arguments — an MCP tool call carries them as they are, a REST route
//! assembles them from its path, query, headers and body under the same
//! names — and [`run`] calls the catalog. So the two can never read an
//! argument differently, and a door's answer is the same JSON on both.

use std::fmt;

use lakelands_api::error::{Case, Error, Naming, Place, Refusal, Said};
use lakelands_api::expr::Condition;
use lakelands_api::id::{Cursor, Position, ProposalId, RecordKey, RelationName, TypeName};
use lakelands_api::ontology::{FieldDecl, TriggerDecl, TypeDecl, ValueTypeDecl, VerbDecl};
use lakelands_api::query::{Pattern, Search};
use lakelands_api::read::{Find, Get, ReadAt};
use lakelands_api::record::Patch;
use lakelands_api::tools::{Door, ToolSpec};
use lakelands_api::write::{
    Change, ChangeTo, Declaration, FunctionDecl, Migration, ObjectSetDecl, ProposalFilter,
    ProposedWrite, SettingDecl, ValueTypeMigration, Withdrawal, WriteOpts,
};
use lakelands_api::{Catalog, Params, Principal, Timestamp};
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
                // A setting is named by its key.
                let named = if kind == "setting" { "key" } else { "name" };
                let name = m.get(named).and_then(Value::as_str).map(str::to_owned);
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
        "setting" => (Naming::Setting, one::<SettingDecl>(members)),
        "withdraw" => (Naming::Withdrawal, one::<Withdrawal>(members)),
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

/// A plan, or an outcome that carries one, with what it says beside it
/// as `said`, for a person who decides on it (062).
fn said(answer: &mut Value) {
    // Each proposal of a listing that keeps its plan.
    if let Some(listed) = answer.get_mut("data").and_then(Value::as_array_mut) {
        for p in listed
            .iter_mut()
            .filter(|p| p["write"].get("plan").is_some())
        {
            let plan = serde_json::from_value(p["write"]["plan"].clone());
            if let Ok(plan) = plan {
                p["said"] = Value::from(crate::say::plan(&plan));
            }
        }
        return;
    }
    let plan = match answer.get("plan") {
        Some(plan) => plan,
        None if answer
            .get("data")
            .is_some_and(|d| d.get("at").is_some() && d.get("lossless").is_some()) =>
        {
            &answer["data"]
        }
        None => return,
    };
    if let Ok(plan) = serde_json::from_value::<lakelands_api::write::Plan>(plan.clone()) {
        answer["said"] = Value::from(crate::say::plan(&plan));
    }
}

/// A change tool's `plan` and `plan_id`
/// (`docs/decisions/062-a-change-plans-before-it-acts.md`).
#[derive(Deserialize)]
struct Planned {
    #[serde(default)]
    plan: bool,
    #[serde(default)]
    plan_id: Option<String>,
}

impl Planned {
    /// The change planned, or made against its plan; given back where the
    /// tool makes it at once.
    fn change(self, change: Change) -> Result<Planning, Change> {
        match (self.plan, self.plan_id) {
            (true, _) => Ok(Planning::Plan(change)),
            (false, Some(id)) => Ok(Planning::Enact(change, id)),
            (false, None) => Err(change),
        }
    }
}

/// A change a tool plans, or makes against its plan.
enum Planning {
    Plan(Change),
    Enact(Change, String),
}

impl Planning {
    async fn run(self, c: &dyn Catalog, who: &Principal) -> Result<Value, Fault> {
        Ok(match self {
            Planning::Plan(change) => to_json(&c.plan(who, change).await?),
            Planning::Enact(change, id) => to_json(&c.enact(who, change, &id).await?),
        })
    }
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
/// itself; here they are refused. A read is clocked by `clock` where the
/// session keeps one (`ReadAt::clock`, a pin of the head).
pub(crate) async fn run(
    c: &dyn Catalog,
    who: &Principal,
    door: &Door,
    label: &str,
    (a, clock): (Map<String, Value>, Option<Timestamp>),
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
    let mut answer = match door {
        Door::Find { ty } => {
            let mut f: Find = args(label, &a)?;
            f.at.clock = clock;
            to_json(&c.find(who, ty, f).await?)
        }
        Door::Get { ty } => {
            let mut g: Get = args(label, &a)?;
            g.at.clock = clock;
            to_json(&c.get(who, ty, g).await?)
        }
        Door::Upsert { ty } => {
            let w: Write = args(label, &a)?;
            to_json(&c.upsert(who, ty, &w.key, w.fields, w.opts).await?)
        }
        Door::Retire { ty } => {
            let w: Write = args(label, &a)?;
            to_json(&c.retire(who, ty, &w.key, w.opts).await?)
        }
        Door::Change { ty } => {
            #[derive(Deserialize)]
            struct Raw {
                #[serde(rename = "where")]
                filter: Condition,
                #[serde(default)]
                set: Option<Patch>,
                #[serde(default)]
                retire: bool,
                #[serde(flatten)]
                planned: Planned,
            }
            #[derive(Deserialize)]
            #[serde(try_from = "Raw")]
            struct D(Condition, ChangeTo, Planned);
            impl TryFrom<Raw> for D {
                type Error = &'static str;
                fn try_from(r: Raw) -> Result<D, &'static str> {
                    let to = match (r.set, r.retire) {
                        (Some(set), false) => ChangeTo::Set(set),
                        (None, true) => ChangeTo::Retire,
                        _ => return Err("send `set` or `retire`, one of the two"),
                    };
                    Ok(D(r.filter, to, r.planned))
                }
            }
            let D(filter, to, planned) = args(label, &a)?;
            let change = Change::Records {
                ty: ty.clone(),
                filter: Box::new(filter),
                to,
                only: Default::default(),
                skip_restricted: false,
            };
            // A call with neither `plan` nor `plan_id` answers the plan (081).
            planned
                .change(change)
                .unwrap_or_else(Planning::Plan)
                .run(c, who)
                .await?
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
            let mut d: D = args(label, &a)?;
            d.at.clock = clock;
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
        Door::Query => {
            let mut p: Pattern = args(label, &a)?;
            p.at.clock = clock;
            to_json(&c.query(who, p).await?)
        }
        Door::Cypher => {
            // The axes are members of their own: a flattened `ReadAt`
            // would hide which member a refusal names.
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct S {
                statement: String,
                #[serde(default)]
                params: Params,
                #[serde(default)]
                cursor: Option<Cursor>,
                #[serde(default)]
                as_of: Option<Position>,
                #[serde(default)]
                valid_at: Option<Timestamp>,
            }
            let s: S = args(label, &a)?;
            let (as_of, valid_at) = (s.as_of, s.valid_at);
            let at = ReadAt {
                as_of,
                valid_at,
                clock,
            };
            let answer = lakelands_cypher::run(c, who, &s.statement, &s.params, at, s.cursor);
            to_json(&answer.await?)
        }
        Door::Search => {
            let mut s: Search = args(label, &a)?;
            s.at.clock = clock;
            to_json(&c.search(who, s).await?)
        }
        Door::Propose => {
            #[derive(Deserialize)]
            struct P {
                write: ProposedWrite,
                why: String,
            }
            let p: P = args(label, &a)?;
            to_json(&c.propose(who, p.write, p.why).await?)
        }
        Door::Proposals => {
            let mut f: ProposalFilter = args(label, &a)?;
            f.at.clock = clock;
            to_json(&c.proposals(who, f).await?)
        }
        Door::Accept | Door::Reject => {
            #[derive(Deserialize)]
            struct D {
                proposal: ProposalId,
                #[serde(default)]
                reason: String,
                #[serde(default)]
                plan_id: Option<String>,
            }
            let d: D = args(label, &a)?;
            // The route that accepts against a plan takes no accept without
            // one (062).
            if label.ends_with("accept_planned") && d.plan_id.is_none() {
                #[derive(Deserialize)]
                struct Planned {
                    #[allow(dead_code)]
                    plan_id: String,
                }
                args::<Planned>(label, &a)?;
            }
            match (*door == Door::Accept, d.plan_id) {
                (true, Some(id)) => to_json(&c.accept_planned(who, &d.proposal, &id).await?),
                (true, None) => to_json(&c.accept(who, &d.proposal).await?),
                (false, _) => to_json(&c.reject(who, &d.proposal, d.reason).await?),
            }
        }
        Door::DeclareType => {
            #[derive(Deserialize)]
            struct D {
                #[serde(rename = "type")]
                ty: TypeDecl,
                #[serde(flatten)]
                planned: Planned,
            }
            let d: D = args(label, &a)?;
            let declarations = vec![Declaration::Type(d.ty)];
            match d.planned.change(Change::Declare { declarations }) {
                Ok(planning) => planning.run(c, who).await?,
                Err(Change::Declare { mut declarations }) => {
                    let declaration = declarations.remove(0);
                    to_json(&c.declare(who, declaration).await?)
                }
                Err(_) => unreachable!("declare_type plans a declaration"),
            }
        }
        Door::Plan => to_json(&c.plan(who, args::<Change>(label, &a)?).await?),
        Door::Enact => {
            #[derive(Deserialize)]
            struct I {
                plan_id: String,
            }
            let (mut change, mut id) = (a.clone(), Map::new());
            id.extend(change.remove("plan_id").map(|v| ("plan_id".to_owned(), v)));
            let i: I = args(label, &id)?;
            to_json(
                &c.enact(who, args::<Change>(label, &change)?, &i.plan_id)
                    .await?,
            )
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
            #[derive(Deserialize)]
            struct D {
                #[serde(flatten)]
                what: lakelands_api::write::Recompute,
                #[serde(flatten)]
                planned: Planned,
            }
            let d: D = args(label, &a)?;
            match d.planned.change(Change::Recompute { what: d.what }) {
                Ok(planning) => planning.run(c, who).await?,
                Err(Change::Recompute { what }) => to_json(&c.recompute(who, what).await?),
                Err(_) => unreachable!("recompute plans a recompute"),
            }
        }
        Door::AddField => {
            #[derive(Deserialize)]
            struct D {
                #[serde(rename = "type")]
                ty: TypeName,
                field: FieldDecl,
                #[serde(flatten)]
                planned: Planned,
            }
            let d: D = args(label, &a)?;
            let field = Box::new(d.field);
            match d.planned.change(Change::AddField { ty: d.ty, field }) {
                Ok(planning) => planning.run(c, who).await?,
                Err(Change::AddField { ty, field }) => {
                    to_json(&c.add_field(who, &ty, *field).await?)
                }
                Err(_) => unreachable!("add_field plans a field added"),
            }
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
                fill: Vec<lakelands_api::FieldName>,
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
                        renames: renames.map(|(a, b)| (a.into(), b.into())).collect(),
                        relation_renames: m.relation_renames,
                        fill: m.fill,
                        ..Migration::to(to)
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
                    if let Some(field) = m.fill.first().cloned() {
                        return Err(wrong("fill", Case::FillUnknown { field }));
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
    };
    said(&mut answer);
    Ok(answer)
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

    /// The store over the memory and SQLite engines.
    async fn lakes(test: &str) -> Vec<(&'static str, Arc<dyn LakeStore>)> {
        let dir =
            std::env::temp_dir().join(format!("lakelands-{test}-door-{}", std::process::id()));
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
    /// argument, on both engines alike.
    #[tokio::test(flavor = "multi_thread")]
    async fn migrate_takes_a_type_or_a_value_type_never_both_and_never_neither() {
        let vt = json!({"name": "status_code", "kind": "text", "one_of": ["open", "late"]});
        let ty = json!({"name": "memo", "fields": [{"name": "text", "type": {"kind": "text"}}]});
        for (name, store) in lakes("first").await {
            let cat = store.catalog(WriteContext::default());
            let who = owner("doors");
            cat.declare(&who, serde_json::from_value(json!({"declare": "value_type", "name": "status_code", "kind": "text", "one_of": ["open", "late", "void"]})).unwrap())
                .await
                .unwrap();
            let call = |a: Value| {
                let (cat, who) = (cat.clone(), who.clone());
                async move {
                    let a = a.as_object().cloned().unwrap_or_default();
                    run(cat.as_ref(), &who, &Door::Migrate, "migrate", (a, None)).await
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

    /// `<type>_change` answers the plan when it is sent neither `plan` nor
    /// `plan_id` and changes nothing, makes the change against the plan's
    /// `report_id`, and takes `set` or `retire`, one of the two (081).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_change_tool_answers_the_plan_first_and_enacts_only_against_it() {
        let ty = json!({"declare": "type", "name": "memo", "fields": [{"name": "text", "type": {"kind": "text"}}]});
        for (name, store) in lakes("second").await {
            let cat = store.catalog(WriteContext::default());
            let who = owner("doors");
            cat.declare(&who, serde_json::from_value(ty.clone()).unwrap())
                .await
                .unwrap();
            for k in ["a", "b"] {
                let patch = [("text".into(), json!("old"))].into();
                let at = (&"memo".into(), &k.into());
                let put = cat.upsert(&who, at.0, at.1, patch, Default::default());
                put.await.unwrap();
            }
            let door = Door::Change { ty: "memo".into() };
            let call = |a: Value| {
                let (cat, who, door) = (cat.clone(), who.clone(), door.clone());
                async move {
                    let a = a.as_object().cloned().unwrap_or_default();
                    run(cat.as_ref(), &who, &door, "the change door", (a, None)).await
                }
            };
            let head = store.head().await.unwrap();
            let args = json!({"where": "text = 'old'", "set": {"text": "new"}});
            let plan = call(args.clone()).await.ok().unwrap();
            assert_eq!(plan["data"]["records"]["selected"], json!(2), "on {name}");
            assert!(plan["said"].is_array(), "on {name}: {plan}");
            assert_eq!(
                store.head().await.unwrap(),
                head,
                "on {name}, a plan appended"
            );
            let mut enact = args.clone();
            enact["plan_id"] = plan["data"]["report_id"].clone();
            let done = call(enact).await.ok().unwrap();
            assert_eq!(done["outcome"], json!("applied"), "on {name}");
            for bad in [
                json!({"where": "true"}),
                json!({"where": "true", "set": {"text": "x"}, "retire": true}),
            ] {
                let answer = call(bad.clone()).await;
                assert!(
                    matches!(&answer, Err(Fault::Arguments(_))),
                    "on {name}, {bad} was not refused as unfit"
                );
            }
        }
    }

    /// A change to records proposed through `propose` is listed as kind
    /// `records`, refused by a plain `accept` and by `accept` with a stale
    /// `plan_id` (`plan_stale`, nothing appended), and made by `accept`
    /// with the id of the plan the reviewer was shown (081, 062).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_proposed_change_to_records_is_accepted_only_with_its_plan_id() {
        let ty = json!({"declare": "type", "name": "memo", "fields": [{"name": "text", "type": {"kind": "text"}}]});
        for (name, store) in lakes("third").await {
            let cat = store.catalog(WriteContext::default());
            let who = owner("doors");
            cat.declare(&who, serde_json::from_value(ty.clone()).unwrap())
                .await
                .unwrap();
            let put = |k: &'static str, text: &'static str| {
                let (cat, who) = (cat.clone(), who.clone());
                async move {
                    let patch = [("text".into(), json!(text))].into();
                    let at = (&"memo".into(), &k.into());
                    let put = cat.upsert(&who, at.0, at.1, patch, Default::default());
                    put.await.unwrap();
                }
            };
            put("a", "old").await;
            put("b", "old").await;
            let call = |door: Door, a: Value| {
                let (cat, who) = (cat.clone(), who.clone());
                async move {
                    let a = a.as_object().cloned().unwrap_or_default();
                    run(cat.as_ref(), &who, &door, "the door", (a, None)).await
                }
            };
            let to = json!({"set": {"text": "new"}});
            let filed = call(
                Door::Propose,
                json!({"write": {"write": "records", "type": "memo", "where": "text = 'old'", "to": to},
                       "why": "renew them"}),
            )
            .await
            .ok()
            .unwrap();
            let proposal = filed["proposal"].clone();
            assert!(proposal.is_string(), "on {name}: {filed}");

            let listed = call(Door::Proposals, json!({"kind": "records"})).await.ok();
            let listed = listed.unwrap();
            let held = listed["data"].as_array().unwrap();
            assert!(
                held.iter().any(|p| p["id"] == proposal
                    && p["write"]["write"] == json!("records")
                    && p["said"].as_array().is_some_and(|s| !s.is_empty())),
                "on {name}: {listed}"
            );

            let head = store.head().await.unwrap();
            let refused = call(Door::Accept, json!({"proposal": proposal})).await;
            assert!(
                matches!(&refused, Err(Fault::Contract(_))),
                "on {name}, a plain accept was not refused"
            );
            let shown = cat
                .plan(
                    &who,
                    lakelands_api::write::Change::Records {
                        ty: "memo".into(),
                        filter: Box::new("text = 'old'".into()),
                        to: serde_json::from_value(to.clone()).unwrap(),
                        only: Default::default(),
                        skip_restricted: false,
                    },
                )
                .await
                .unwrap()
                .data;
            put("b", "other").await;
            let head = head.max(store.head().await.unwrap());
            let stale = call(
                Door::Accept,
                json!({"proposal": proposal, "plan_id": shown.report_id}),
            )
            .await;
            let Err(Fault::Contract(stale)) = stale else {
                panic!("on {name}, a stale plan_id was accepted");
            };
            assert!(stale.to_string().contains("plan"), "on {name}: {stale}");
            assert_eq!(
                store.head().await.unwrap(),
                head,
                "on {name}, a refusal appended"
            );

            put("b", "old").await;
            let fresh = cat
                .plan(
                    &who,
                    lakelands_api::write::Change::Records {
                        ty: "memo".into(),
                        filter: Box::new("text = 'old'".into()),
                        to: serde_json::from_value(to.clone()).unwrap(),
                        only: Default::default(),
                        skip_restricted: false,
                    },
                )
                .await
                .unwrap()
                .data;
            let done = call(
                Door::Accept,
                json!({"proposal": proposal, "plan_id": fresh.report_id}),
            )
            .await
            .ok()
            .unwrap();
            assert_eq!(done["outcome"], json!("applied"), "on {name}: {done}");
        }
    }
}
