//! The tool-name generator: the one place an LLM-facing tool name is
//! made.
//!
//! [`tools`] is a pure function of a [`LakeDescription`]. Because
//! `describe` already answers as one principal may see the lake, the
//! tools it yields — their names, and the JSON schemas in and out — are
//! filtered by that principal's grants without a second rule. The mock,
//! the store's catalog and every transport take names from here; the
//! `single-catalog` gate refuses a tool name written anywhere else.
//!
//! Per type: `<type>_find`, `<type>_get`, `<type>_upsert`,
//! `<type>_retire`, each only if the principal may use it. Per relation
//! carried by link records that the principal may link:
//! `<type>_<relation>_link` and `<type>_<relation>_unlink`, named by the
//! type and the relation, since relation names are unique per type only. Per verb: the
//! verb's name. Generic: `describe`, `query`, `search`, `propose`,
//! `proposals`, `accept`, `reject`, `declare_type`, `add_field`,
//! `migrate`, each only if the principal may use it, and `pin`, the MCP
//! session's door for pinning `as_of` across a turn. A 20-type lake
//! offers about 80 tools.
//!
//! Four doors of [`Catalog`](crate::Catalog) have no tool of their own:
//! `describe_type` is `describe` with a `type`, `migrate_apply` is
//! `migrate` with a `report_id`, `ingest` is deliberately not a tool — a
//! source feeds a lake through a stream, not through a model — and neither
//! is `apply` in `0.1.0`: a bundle is applied by its author's tooling, and
//! a model declares one declaration at a time.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::describe::{FieldDescription, LakeDescription, TypeDescription, VerbDescription};
use crate::id::{RelationName, TypeName, VerbName};
use crate::ontology::{FieldType, StorageKind};

/// The door a tool leads to.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "door", rename_all = "snake_case")]
pub enum Door {
    /// `find` on one type.
    Find {
        /// The type.
        #[serde(rename = "type")]
        ty: TypeName,
    },
    /// `get` on one type.
    Get {
        /// The type.
        #[serde(rename = "type")]
        ty: TypeName,
    },
    /// `upsert` on one type.
    Upsert {
        /// The type.
        #[serde(rename = "type")]
        ty: TypeName,
    },
    /// `retire` on one type.
    Retire {
        /// The type.
        #[serde(rename = "type")]
        ty: TypeName,
    },
    /// `link` over one relation of one type.
    Link {
        /// The type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The relation.
        relation: RelationName,
    },
    /// `unlink` over one relation of one type.
    Unlink {
        /// The type.
        #[serde(rename = "type")]
        ty: TypeName,
        /// The relation.
        relation: RelationName,
    },
    /// `call` of one verb.
    Call {
        /// The verb.
        verb: VerbName,
    },
    /// `describe`.
    Describe,
    /// `pin`: a door of the MCP session, not of the catalog. It pins
    /// `as_of` for the session's later reads until cleared.
    Pin,
    /// `subscribe`: a door of the MCP session. The session is sent
    /// `notifications/lakelands/changed` for every change it may see from a
    /// position on.
    Subscribe,
    /// `unsubscribe`: ends the session's subscription.
    Unsubscribe,
    /// `query`.
    Query,
    /// `search`.
    Search,
    /// `propose`.
    Propose,
    /// `proposals`.
    Proposals,
    /// `accept`.
    Accept,
    /// `reject`.
    Reject,
    /// `declare` of a type.
    DeclareType,
    /// `declare`: any declaration. Served over REST only; the generator
    /// makes no tool of it, so agents author types with `declare_type`.
    Declare,
    /// `apply`: a set of declarations as one. Served over REST only, as
    /// `declare` is.
    Apply,
    /// `add_field`.
    AddField,
    /// `migrate`: the preflight, or with a `report_id` argument the
    /// `migrate_apply` door.
    Migrate,
}

/// One tool: its name, what it is for, its schemas, and its door.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// The tool's name; matches `^[a-z0-9_]{1,64}$`.
    pub name: String,
    /// What the tool does, for the model choosing it.
    pub description: String,
    /// JSON Schema of the arguments.
    pub input_schema: Value,
    /// JSON Schema of the answer.
    pub output_schema: Value,
    /// The door it maps to.
    pub door: Door,
}

const DESCRIBE: &str = "describe";
const PIN: &str = "pin";
const SUBSCRIBE: &str = "subscribe";
const UNSUBSCRIBE: &str = "unsubscribe";
const QUERY: &str = "query";
const SEARCH: &str = "search";
const PROPOSE: &str = "propose";
const PROPOSALS: &str = "proposals";
const ACCEPT: &str = "accept";
const REJECT: &str = "reject";
const DECLARE_TYPE: &str = "declare_type";
const ADD_FIELD: &str = "add_field";
const MIGRATE: &str = "migrate";

/// The kinds of change a session may subscribe to: the log's event kinds
/// that change a record, or say something happened to one.
pub const CHANGE_KINDS: [&str; 6] = [
    "record_upserted",
    "record_ingested",
    "record_retired",
    "output_recorded",
    "trigger_fired",
    "object_set_changed",
];

/// The names of the generic tools, which no type or verb may produce.
/// The generator emits these constants and no other generic name.
pub const GENERIC_TOOLS: [&str; 13] = [
    DESCRIBE,
    PIN,
    SUBSCRIBE,
    UNSUBSCRIBE,
    QUERY,
    SEARCH,
    PROPOSE,
    PROPOSALS,
    ACCEPT,
    REJECT,
    DECLARE_TYPE,
    ADD_FIELD,
    MIGRATE,
];

/// The longest name an MCP client is guaranteed to accept.
pub const MAX_TOOL_NAME: usize = 64;

/// A declaration's name as it appears in a tool name: lower case, every
/// character outside `[a-z0-9_]` replaced by `_` (`sys.source` becomes
/// `sys_source`).
pub fn tool_base(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

const DOOR_SUFFIXES: [&str; 4] = ["find", "get", "upsert", "retire"];

/// Why a lake's declarations cannot all become tools. The store refuses
/// a declaration that would introduce one of these, so [`tools`] never
/// meets them in a lake the store wrote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "problem", rename_all = "snake_case")]
pub enum NameProblem {
    /// Two declarations produce the same tool name.
    Collision {
        /// The tool name.
        name: String,
        /// The declarations that produce it.
        from: Vec<String>,
    },
    /// A tool name would be longer than [`MAX_TOOL_NAME`].
    TooLong {
        /// The tool name.
        name: String,
    },
    /// A declaration's name has nothing left once made a tool name.
    Empty {
        /// The declaration.
        from: String,
    },
}

impl std::fmt::Display for NameProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NameProblem::Collision { name, from } => {
                write!(
                    f,
                    "the tool name `{name}` would come from {}",
                    from.join(" and ")
                )
            }
            NameProblem::TooLong { name } => write!(
                f,
                "the tool name `{name}` is longer than {MAX_TOOL_NAME} characters"
            ),
            NameProblem::Empty { from } => write!(f, "{from} gives an empty tool name"),
        }
    }
}

/// The link doors' tool suffixes.
const LINK_SUFFIXES: [&str; 2] = ["link", "unlink"];

/// The tool name of a link door: the type's and the relation's names made
/// tool names, joined, then the door.
fn link_tool(ty: &TypeName, relation: &RelationName, door: &str) -> String {
    format!(
        "{}_{}_{door}",
        tool_base(ty.as_str()),
        tool_base(relation.as_str())
    )
}

/// Every tool name the lake's declarations would produce, for any
/// principal, with the declaration each comes from. `links` are the
/// relations carried by link records, by their type.
fn all_names(
    types: &[TypeName],
    verbs: &[VerbName],
    links: &[(TypeName, RelationName)],
) -> BTreeMap<String, Vec<String>> {
    let mut names: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for g in GENERIC_TOOLS {
        names
            .entry(g.to_owned())
            .or_default()
            .push(format!("the generic tool `{g}`"));
    }
    for ty in types {
        if tool_base(ty.as_str()).is_empty() {
            names
                .entry(String::new())
                .or_default()
                .push(format!("type `{ty}`"));
            continue;
        }
        for s in DOOR_SUFFIXES {
            names
                .entry(format!("{}_{s}", tool_base(ty.as_str())))
                .or_default()
                .push(format!("type `{ty}`"));
        }
    }
    for v in verbs {
        names
            .entry(tool_base(v.as_str()))
            .or_default()
            .push(format!("verb `{v}`"));
    }
    for (ty, relation) in links {
        for s in LINK_SUFFIXES {
            names
                .entry(link_tool(ty, relation, s))
                .or_default()
                .push(format!("relation `{relation}` of type `{ty}`"));
        }
    }
    names
}

/// The problems a set of type, verb and link-relation names would cause,
/// checked over every door any principal could be granted. Empty when
/// every name can become a tool.
pub fn name_problems(
    types: &[TypeName],
    verbs: &[VerbName],
    links: &[(TypeName, RelationName)],
) -> Vec<NameProblem> {
    let mut problems = Vec::new();
    for (name, from) in all_names(types, verbs, links) {
        if name.is_empty() {
            problems.extend(from.into_iter().map(|from| NameProblem::Empty { from }));
            continue;
        }
        if name.len() > MAX_TOOL_NAME {
            problems.push(NameProblem::TooLong { name: name.clone() });
        }
        if from.len() > 1 {
            problems.push(NameProblem::Collision { name, from });
        }
    }
    problems
}

/// The tools one principal sees for a lake: a pure function of the
/// lake's description as that principal received it, sorted by name.
///
/// Refused, with every problem found, when the described types and verbs
/// would produce a duplicate, empty or overlong name; the store refuses
/// such declarations, so a lake it wrote never meets this.
pub fn tools(lake: &LakeDescription) -> Result<Vec<ToolSpec>, Vec<NameProblem>> {
    let types: Vec<TypeName> = lake.types.iter().map(|t| t.name.clone()).collect();
    let verbs: Vec<VerbName> = lake.verbs.iter().map(|v| v.name.clone()).collect();
    let links: Vec<(TypeName, RelationName)> = lake
        .types
        .iter()
        .flat_map(|t| {
            t.relations
                .iter()
                .filter(|r| r.via.is_none())
                .map(|r| (t.name.clone(), r.name.clone()))
        })
        .collect();
    let problems = name_problems(&types, &verbs, &links);
    if !problems.is_empty() {
        return Err(problems);
    }
    let mut out = Vec::new();
    for ty in &lake.types {
        type_tools(ty, lake, &mut out);
        link_tools(ty, lake, &mut out);
    }
    for verb in &lake.verbs {
        out.push(verb_tool(verb, lake));
    }
    generic_tools(lake, &mut out);
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn obj(properties: Map<String, Value>, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn props(pairs: impl IntoIterator<Item = (&'static str, Value)>) -> Map<String, Value> {
    pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
}

fn time_axes(p: &mut Map<String, Value>) {
    p.insert(
        "as_of".into(),
        json!({"type": "integer", "minimum": 0,
               "description": "Read the lake as it was at this position."}),
    );
    p.insert(
        "valid_at".into(),
        json!({"type": "string", "format": "date-time",
               "description": "Read what was true in the world at this instant."}),
    );
}

/// The JSON Schema of one field's value.
pub fn field_schema(ty: &FieldType, lake: &LakeDescription) -> Value {
    let mut s = match ty.kind {
        StorageKind::Text => json!({"type": "string"}),
        StorageKind::Number => json!({"type": "number"}),
        StorageKind::Decimal => {
            json!({"type": "string", "pattern": "^-?[0-9]+(\\.[0-9]+)?$",
                   "description": "An exact decimal, written as a string."})
        }
        StorageKind::Boolean => json!({"type": "boolean"}),
        StorageKind::DateTime => json!({"type": "string", "format": "date-time"}),
        StorageKind::Bytes => json!({"type": "string", "contentEncoding": "base64"}),
        StorageKind::Json => json!({}),
        StorageKind::Reference => json!({"type": "string"}),
        StorageKind::Vector => json!({"type": "array", "items": {"type": "number"}}),
        StorageKind::Geo => {
            json!({"type": "object", "description": "A GeoJSON geometry.",
                   "required": ["type"]})
        }
    };
    let m = s.as_object_mut().expect("every kind schema is an object");
    if let (StorageKind::Reference, Some(target)) = (ty.kind, &ty.target) {
        m.insert(
            "description".into(),
            json!(format!("The key of a `{target}` record.")),
        );
    }
    if let (StorageKind::Vector, Some(d)) = (ty.kind, ty.dimensions) {
        m.insert("minItems".into(), json!(d));
        m.insert("maxItems".into(), json!(d));
    }
    if let Some(vt) = &ty.value_type {
        m.insert("x-value-type".into(), json!(vt));
        if let Some(decl) = lake.value_types.iter().find(|d| &d.name == vt) {
            if !decl.one_of.is_empty() {
                m.insert("enum".into(), json!(decl.one_of));
            }
            if let Some(d) = &decl.description {
                m.insert("description".into(), json!(d));
            }
        }
    }
    s
}

fn described(mut schema: Value, f: &FieldDescription) -> Value {
    if let (Some(d), Some(m)) = (&f.description, schema.as_object_mut()) {
        m.insert("description".into(), json!(d));
    }
    schema
}

/// The schema of a record as this principal reads it.
fn record_schema(ty: &TypeDescription, lake: &LakeDescription) -> Value {
    let fields: Map<String, Value> = ty
        .fields
        .iter()
        .filter(|f| f.access.reads())
        .map(|f| (f.name.to_string(), described(field_schema(&f.ty, lake), f)))
        .collect();
    json!({
        "type": "object",
        "properties": {
            "type": {"const": ty.name},
            "key": {"type": "string"},
            "version": {"type": "integer"},
            "fields": {"type": "object", "properties": fields},
            "masked": {"type": "array", "items": {"type": "string"}},
            "meta": {"type": "object",
                     "description": "Per field: origin, known_at, seen_at, position, writer, source, procedure_version; and error, for a computed field whose function failed, which then holds no value."},
        },
        "required": ["type", "key", "version", "fields"],
    })
}

/// A string schema restricted to `values`; unrestricted when there are
/// none, since strict validators refuse an empty `enum`.
fn one_of(values: impl Iterator<Item = String>) -> Value {
    let values: Vec<String> = values.collect();
    if values.is_empty() {
        json!({"type": "string"})
    } else {
        json!({"type": "string", "enum": values})
    }
}

fn order_by_schema(fields: Value) -> Value {
    json!({"type": "array", "items": {
        "type": "object",
        "description": "One sort key. `latest` orders by the position of each record's latest change and takes no field; every other `by` names its field.",
        "properties": {"field": fields,
                       "by": {"enum": ["value", "known_at", "seen_at", "latest"]},
                       "dir": {"enum": ["asc", "desc"]}},
        "additionalProperties": false}})
}

fn write_opts(p: &mut Map<String, Value>) {
    p.insert(
        "expected_version".into(),
        json!({"type": "integer", "minimum": 0,
               "description": "Refuse unless the record is at this version; 0 means it must not exist yet."}),
    );
    p.insert(
        "idempotency_key".into(),
        json!({"type": "string",
               "description": "A retry with the same key is a no-op answering the first outcome."}),
    );
}

fn outcome_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "outcome": {"enum": ["applied", "proposed"]},
            "position": {"type": "integer"},
            "version": {"type": "integer"},
            "proposal": {"type": "string"},
            "why": {"type": "string"},
        },
        "required": ["outcome", "position"],
    })
}

fn reply_schema(data: Value) -> Value {
    json!({
        "type": "object",
        "properties": {"position": {"type": "integer"}, "data": data},
        "required": ["position", "data"],
    })
}

fn label(ty: &TypeDescription) -> String {
    match &ty.description {
        Some(d) => format!("`{}` ({d})", ty.name),
        None => format!("`{}`", ty.name),
    }
}

fn type_tools(ty: &TypeDescription, lake: &LakeDescription, out: &mut Vec<ToolSpec>) {
    let base = tool_base(ty.name.as_str());
    let record = record_schema(ty, lake);
    let filterable = || {
        ty.fields
            .iter()
            .filter(|f| f.filterable)
            .map(|f| f.name.to_string())
    };

    if ty.may.find {
        let mut p = props([
            (
                "where",
                json!({"type": "string",
                       "description": "A condition in the expression grammar, e.g. status = 'open' and amount > 100."}),
            ),
            ("order_by", order_by_schema(one_of(filterable()))),
            ("limit", json!({"type": "integer", "minimum": 1})),
            ("cursor", json!({"type": "string"})),
            (
                "after",
                json!({"type": "object",
                       "description": "Resume strictly after a row: its values for the order_by keys, in order (null where it has none), and its key. Instead of a cursor.",
                       "properties": {"values": {"type": "array"},
                                      "key": {"type": "string"}},
                       "required": ["values", "key"], "additionalProperties": false}),
            ),
            (
                "aggregate",
                json!({"type": "object", "properties": {
                    "measures": {"type": "array", "items": {
                        "type": "object",
                        "properties": {"name": {"type": "string"},
                                       "func": {"enum": ["count", "sum", "min", "max", "avg"]},
                                       "field": one_of(filterable())},
                        "required": ["name", "func"], "additionalProperties": false}},
                    "group_by": {"type": "array", "items": {"anyOf": [
                        one_of(filterable()),
                        {"type": "object",
                         "properties": {"name": {"type": "string"},
                                        "expr": {"type": "string", "description": "An expression in the grammar, e.g. date_bucket(due_on, 'month')."}},
                         "required": ["name", "expr"], "additionalProperties": false}]}}},
                    "required": ["measures"], "additionalProperties": false}),
            ),
            ("with_meta", json!({"type": "boolean"})),
        ]);
        time_axes(&mut p);
        out.push(ToolSpec {
            name: format!("{base}_find"),
            description: format!(
                "Find {} records: filter with `where`, sort, page with `cursor` or resume after a row with `after`, or aggregate. Every answer carries the position it was read at.",
                label(ty)
            ),
            input_schema: obj(p, &[]),
            output_schema: reply_schema(json!({
                "type": "object",
                "properties": {"shape": {"enum": ["records", "groups"]},
                               "rows": {"type": "array"},
                               "next_cursor": {"type": "string"}},
                "required": ["shape", "rows"]})),
            door: Door::Find { ty: ty.name.clone() },
        });
    }
    if ty.may.get {
        let mut p = props([
            ("key", json!({"type": "string"})),
            ("with_meta", json!({"type": "boolean"})),
        ]);
        time_axes(&mut p);
        out.push(ToolSpec {
            name: format!("{base}_get"),
            description: format!("Get one {} record by key.", label(ty)),
            input_schema: obj(p, &["key"]),
            output_schema: reply_schema(record.clone()),
            door: Door::Get {
                ty: ty.name.clone(),
            },
        });
    }
    if ty.may.upsert {
        let writable: Map<String, Value> = ty
            .fields
            .iter()
            .filter(|f| f.authored_writable())
            .map(|f| {
                let s = described(field_schema(&f.ty, lake), f);
                (f.name.to_string(), json!({"anyOf": [s, {"type": "null"}]}))
            })
            .collect();
        let mut p = props([
            ("key", json!({"type": "string"})),
            (
                "fields",
                json!({"type": "object", "properties": writable, "additionalProperties": false,
                       "description": "Only the fields to change: an absent field is unchanged, null clears it."}),
            ),
        ]);
        write_opts(&mut p);
        out.push(ToolSpec {
            name: format!("{base}_upsert"),
            description: format!(
                "Create or change a {} record. Answers `applied` with the new position, or `proposed` if the write waits for review.",
                label(ty)
            ),
            input_schema: obj(p, &["key", "fields"]),
            output_schema: outcome_schema(),
            door: Door::Upsert { ty: ty.name.clone() },
        });
    }
    if ty.may.retire {
        let mut p = props([("key", json!({"type": "string"}))]);
        write_opts(&mut p);
        out.push(ToolSpec {
            name: format!("{base}_retire"),
            description: format!(
                "Retire a {} record. Its history stays in the lake.",
                label(ty)
            ),
            input_schema: obj(p, &["key"]),
            output_schema: outcome_schema(),
            door: Door::Retire {
                ty: ty.name.clone(),
            },
        });
    }
}

/// The link and unlink tools of one relation of `ty` the principal may
/// link.
fn link_tools(ty: &TypeDescription, lake: &LakeDescription, out: &mut Vec<ToolSpec>) {
    for relation in &ty.may.link {
        let Some(r) = ty.relations.iter().find(|r| &r.name == relation) else {
            continue;
        };
        // The link's own fields the principal may write, as the link type's
        // description gives them: the same rule as `upsert`'s.
        let link_type = format!("sys.link.{}.{relation}", ty.name);
        let fields: Map<String, Value> = lake
            .types
            .iter()
            .find(|t| t.name.as_str() == link_type)
            .map(|lt| {
                lt.fields
                    .iter()
                    .filter(|f| {
                        f.authored_writable() && r.fields.iter().any(|own| own.name == f.name)
                    })
                    .map(|f| {
                        let s = described(field_schema(&f.ty, lake), f);
                        (f.name.to_string(), json!({"anyOf": [s, {"type": "null"}]}))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let target = json!({"type": "string",
                            "description": format!("The key of the `{}` record to link to.", r.target)});
        let mut p = props([
            (
                "key",
                json!({"type": "string", "description": format!("The key of the `{}` record the link is from.", ty.name)}),
            ),
            ("to", target.clone()),
            (
                "fields",
                json!({"type": "object", "properties": fields, "additionalProperties": false,
                       "description": "The link's own fields to set: an absent field is unchanged, null clears it."}),
            ),
        ]);
        write_opts(&mut p);
        out.push(ToolSpec {
            name: link_tool(&ty.name, relation, "link"),
            description: format!(
                "Link a {} record to a `{}` record over `{relation}`, or change the link's fields. One link per pair.",
                label(ty),
                r.target
            ),
            input_schema: obj(p, &["key", "to"]),
            output_schema: outcome_schema(),
            door: Door::Link {
                ty: ty.name.clone(),
                relation: relation.clone(),
            },
        });
        let mut p = props([
            (
                "key",
                json!({"type": "string", "description": format!("The key of the `{}` record the link is from.", ty.name)}),
            ),
            ("to", target),
        ]);
        write_opts(&mut p);
        out.push(ToolSpec {
            name: link_tool(&ty.name, relation, "unlink"),
            description: format!(
                "Remove the link over `{relation}` from a {} record to a `{}` record. Its history stays in the lake.",
                label(ty),
                r.target
            ),
            input_schema: obj(p, &["key", "to"]),
            output_schema: outcome_schema(),
            door: Door::Unlink {
                ty: ty.name.clone(),
                relation: relation.clone(),
            },
        });
    }
}

fn verb_tool(verb: &VerbDescription, lake: &LakeDescription) -> ToolSpec {
    // Only the payload fields the principal may write, as for `upsert`.
    let writable = || verb.payload.iter().filter(|f| f.access.writes());
    let payload: Map<String, Value> = writable()
        .map(|f| (f.name.to_string(), described(field_schema(&f.ty, lake), f)))
        .collect();
    let required: Vec<String> = writable()
        .filter(|f| f.required)
        .map(|f| f.name.to_string())
        .collect();
    let mut p = props([
        (
            "key",
            json!({"type": "string",
                       "description": format!("The key of the `{}` record.", verb.on)}),
        ),
        (
            "payload",
            json!({"type": "object", "properties": payload,
                           "required": required, "additionalProperties": false}),
        ),
    ]);
    write_opts(&mut p);
    let what = verb
        .description
        .clone()
        .unwrap_or_else(|| format!("Call `{}` on a `{}` record.", verb.name, verb.on));
    let how = match verb.risk {
        crate::ontology::Risk::Low if !verb.external => "It runs at once.",
        _ => "It is recorded as a proposal and runs when accepted.",
    };
    ToolSpec {
        name: tool_base(verb.name.as_str()),
        description: format!("{what} {how}"),
        input_schema: obj(p, &["key", "payload"]),
        output_schema: outcome_schema(),
        door: Door::Call {
            verb: verb.name.clone(),
        },
    }
}

fn generic_tools(lake: &LakeDescription, out: &mut Vec<ToolSpec>) {
    let names = |pick: fn(&TypeDescription) -> bool| -> Value {
        one_of(
            lake.types
                .iter()
                .filter(|t| pick(t))
                .map(|t| t.name.to_string()),
        )
    };
    let findable = lake.types.iter().any(|t| t.may.find);

    let mut p = props([("type", names(|_| true))]);
    time_axes(&mut p);
    out.push(ToolSpec {
        name: DESCRIBE.into(),
        description: "Describe the lake, or one type on six axes: structure, access, rights, actions, lineage, lifecycle.".into(),
        input_schema: obj(p, &[]),
        output_schema: reply_schema(json!({"type": "object"})),
        door: Door::Describe,
    });

    out.push(ToolSpec {
        name: PIN.into(),
        description: "Pin this session's reads to one position until cleared, so every read in a turn sees the same state: `as_of` pins that position, no argument pins the current head, `clear` removes the pin. A read's own `as_of` still wins; writes are never pinned.".into(),
        input_schema: obj(
            props([
                (
                    "as_of",
                    json!({"type": "integer", "minimum": 0,
                           "description": "The position to pin; the current head when left out."}),
                ),
                (
                    "clear",
                    json!({"type": "boolean", "description": "Remove the pin."}),
                ),
            ]),
            &[],
        ),
        output_schema: json!({
            "type": "object",
            "properties": {
                "position": {"type": "integer"},
                "pinned": {"type": ["integer", "null"]},
            },
            "required": ["position", "pinned"],
        }),
        door: Door::Pin,
    });

    out.push(ToolSpec {
        name: SUBSCRIBE.into(),
        description: "Be told of changes without polling: from `from` on (by default, from now), this session is sent a `notifications/lakelands/changed` for every change of a record it may read, of the kinds, types and keys asked (by default every kind, every type it may read when the change happens, and every record). A notification names the position, the kind, the type, the key and the changed fields it may read; never a value. Subscribing again replaces the subscription; it ends with the session.".into(),
        input_schema: obj(
            props([
                (
                    "from",
                    json!({"type": "integer", "minimum": 0,
                           "description": "The first position to be told of; the one after the current head when left out."}),
                ),
                (
                    "kinds",
                    json!({"type": "array", "items": {"enum": CHANGE_KINDS},
                           "description": "Only these kinds of change; every kind when left out."}),
                ),
                (
                    "types",
                    json!({"type": "array", "items": names(|t| t.may.find),
                           "description": "Only these types, fixed now; every type readable at each change when left out."}),
                ),
                (
                    "keys",
                    json!({"type": "array", "items": {"type": "string", "minLength": 1},
                           "description": "Only the records with these keys, fixed now: of the types asked, or of any type when none are; every record when left out. One record's history is one type and one key."}),
                ),
            ]),
            &[],
        ),
        output_schema: json!({
            "type": "object",
            "properties": {
                "position": {"type": "integer"},
                "subscribed": {"type": "boolean"},
                "from": {"type": "integer"},
            },
            "required": ["position", "subscribed", "from"],
        }),
        door: Door::Subscribe,
    });

    out.push(ToolSpec {
        name: UNSUBSCRIBE.into(),
        description: "Stop being told of changes. Answers the position the session had reached, to subscribe `from` later.".into(),
        input_schema: obj(props([]), &[]),
        output_schema: json!({
            "type": "object",
            "properties": {
                "position": {"type": "integer"},
                "subscribed": {"type": "boolean"},
                "next": {"type": "integer"},
            },
            "required": ["position", "subscribed", "next"],
        }),
        door: Door::Unsubscribe,
    });

    if findable {
        let mut p = props([
            (
                "start",
                json!({"type": "object", "properties": {
                    "type": names(|t| t.may.find), "key": {"type": "string"},
                    "where": {"type": "string"}, "search": {"type": "object"}},
                    "required": ["type"]}),
            ),
            (
                "steps",
                json!({"type": "array", "items": {"type": "object", "properties": {
                    "relation": {"type": "string"}, "walk": {"enum": ["out", "in", "both"]},
                    "type": {"type": "string"}, "where": {"type": "string"},
                    "depth": {"type": "object", "properties": {
                        "min": {"type": "integer", "minimum": 0},
                        "max": {"type": "integer", "minimum": 0}}}},
                    "required": ["relation"]}}),
            ),
            (
                "to",
                json!({"type": "array", "prefixItems": [{"type": "string"}, {"type": "string"}],
                       "minItems": 2, "maxItems": 2,
                       "description": "[type, key]: return only the shortest walk to this record."}),
            ),
            (
                "project",
                json!({"type": "object", "properties": {
                "as": {"enum": ["rows", "graph", "aggregate"]},
                "fields": {"type": "array", "items": {"type": "string"}}}}),
            ),
            ("order_by", order_by_schema(json!({"type": "string"}))),
            ("limit", json!({"type": "integer", "minimum": 1})),
            ("cursor", json!({"type": "string"})),
        ]);
        time_axes(&mut p);
        out.push(ToolSpec {
            name: QUERY.into(),
            description: "Walk declared relations from a start (a record, a filter or a search) and return rows or a graph. Covers neighbours, traversal, paths and subgraphs.".into(),
            input_schema: obj(p, &["start"]),
            output_schema: reply_schema(json!({"type": "object"})),
            door: Door::Query,
        });

        let mut p = props([
            ("type", names(|t| t.may.find)),
            ("mode", json!({"enum": ["lexical", "vector", "hybrid"]})),
            ("text", json!({"type": "string"})),
            ("field", json!({"type": "string"})),
            (
                "vector",
                json!({"type": "array", "items": {"type": "number"}}),
            ),
            ("k", json!({"type": "integer", "minimum": 1})),
            ("cursor", json!({"type": "string"})),
            ("where", json!({"type": "string"})),
            ("with_meta", json!({"type": "boolean"})),
        ]);
        time_axes(&mut p);
        out.push(ToolSpec {
            name: SEARCH.into(),
            description: "Ranked search over one type, lexical, by vector or both. Hits come best first, each with a score; page with `cursor`.".into(),
            input_schema: obj(p, &["type", "mode"]),
            output_schema: reply_schema(json!({"type": "object", "properties": {
                "hits": {"type": "array"}, "ranked_by": {"type": "string"},
                "next_cursor": {"type": "string"}}})),
            door: Door::Search,
        });
    }

    if lake.types.iter().any(|t| t.may.upsert || t.may.retire) || !lake.verbs.is_empty() {
        out.push(ToolSpec {
            name: PROPOSE.into(),
            description: "Record a write as a proposal for review instead of applying it.".into(),
            input_schema: obj(
                props([
                    (
                        "write",
                        json!({"type": "object", "properties": {
                        "write": {"enum": ["upsert", "retire", "call"]}}, "required": ["write"]}),
                    ),
                    ("why", json!({"type": "string"})),
                ]),
                &["write", "why"],
            ),
            output_schema: outcome_schema(),
            door: Door::Propose,
        });
    }

    if lake.may.review {
        let mut p = props([
            (
                "state",
                json!({"enum": ["pending", "accepted", "rejected"]}),
            ),
            ("type", names(|_| true)),
            (
                "order_by",
                order_by_schema(one_of(
                    ["proposed_at", "position"].iter().map(|f| f.to_string()),
                )),
            ),
            ("limit", json!({"type": "integer", "minimum": 1})),
            ("cursor", json!({"type": "string"})),
        ]);
        time_axes(&mut p);
        out.push(ToolSpec {
            name: PROPOSALS.into(),
            description: "List proposals waiting for review, or decided ones; newest first unless ordered otherwise.".into(),
            input_schema: obj(p, &[]),
            output_schema: reply_schema(json!({"type": "array"})),
            door: Door::Proposals,
        });
        out.push(ToolSpec {
            name: ACCEPT.into(),
            description: "Accept a proposal: its write is applied.".into(),
            input_schema: obj(
                props([("proposal", json!({"type": "string"}))]),
                &["proposal"],
            ),
            output_schema: outcome_schema(),
            door: Door::Accept,
        });
        out.push(ToolSpec {
            name: REJECT.into(),
            description: "Reject a proposal: nothing is written but the rejection.".into(),
            input_schema: obj(
                props([
                    ("proposal", json!({"type": "string"})),
                    ("reason", json!({"type": "string"})),
                ]),
                &["proposal", "reason"],
            ),
            output_schema: outcome_schema(),
            door: Door::Reject,
        });
    }

    if lake.may.schema {
        out.push(ToolSpec {
            name: DECLARE_TYPE.into(),
            description: "Declare a new type, or revise one without reshaping its records.".into(),
            input_schema: obj(props([("type", json!({"type": "object"}))]), &["type"]),
            output_schema: outcome_schema(),
            door: Door::DeclareType,
        });
        out.push(ToolSpec {
            name: ADD_FIELD.into(),
            description: "Add one field to a type.".into(),
            input_schema: obj(
                props([
                    ("type", names(|_| true)),
                    ("field", json!({"type": "object"})),
                ]),
                &["type", "field"],
            ),
            output_schema: outcome_schema(),
            door: Door::AddField,
        });
        out.push(ToolSpec {
            name: MIGRATE.into(),
            description: "Change a type's shape (`to`) or tighten a value type (`value_type`), one of the two. Without `report_id` it only reports what would convert or no longer fit; with the id of that report it applies the migration.".into(),
            input_schema: obj(
                props([
                    ("to", json!({"type": "object"})),
                    ("value_type", json!({"type": "object"})),
                    (
                        "renames",
                        json!({"type": "array", "items": {
                        "type": "array", "items": {"type": "string"},
                        "minItems": 2, "maxItems": 2}}),
                    ),
                    ("report_id", json!({"type": "string"})),
                ]),
                &[],
            ),
            output_schema: json!({"type": "object"}),
            door: Door::Migrate,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::describe::{LakeRights, TypeAccess, TypeLifecycle, TypeLineage, TypeRights};
    use crate::ontology::{Authority, FieldDecl, Risk};
    use crate::principal::Access;

    fn field(
        name: &str,
        kind: StorageKind,
        authority: Authority,
        access: Access,
    ) -> FieldDescription {
        let decl = FieldDecl {
            name: name.into(),
            ty: FieldType::kind(kind),
            required: false,
            authority,
            derived: None,
            function: None,
            depends_on: vec![],
            on_input_change: Default::default(),
            sensitive: false,
            searchable: false,
            description: None,
            label: None,
            embed: None,
        };
        FieldDescription::of(&decl, access)
    }

    fn ty(name: &str, may: TypeRights, fields: Vec<FieldDescription>) -> TypeDescription {
        TypeDescription {
            name: name.into(),
            description: None,
            fields,
            relations: vec![],
            access: TypeAccess {
                key: vec![],
                valid_from: None,
                valid_to: None,
                searchable: true,
                field_meta: true,
            },
            may,
            verbs: vec![],
            lineage: TypeLineage {
                sourced: false,
                view_of: None,
            },
            lifecycle: TypeLifecycle {
                version: 1,
                schema_hash: "h".into(),
                sorted: vec![],
            },
        }
    }

    const ALL: TypeRights = TypeRights {
        find: true,
        get: true,
        upsert: true,
        retire: true,
        link: Vec::new(),
    };
    const READ: TypeRights = TypeRights {
        find: true,
        get: true,
        upsert: false,
        retire: false,
        link: Vec::new(),
    };

    fn lake(
        types: Vec<TypeDescription>,
        verbs: Vec<VerbDescription>,
        may: LakeRights,
    ) -> LakeDescription {
        LakeDescription {
            lake: "finance".into(),
            types,
            verbs,
            value_types: vec![],
            triggers: vec![],
            may,
        }
    }

    fn tools_ok(lake: &LakeDescription) -> Vec<ToolSpec> {
        tools(lake).expect("the test lake's names are valid")
    }

    fn names(tools: &[ToolSpec]) -> Vec<&str> {
        tools.iter().map(|t| t.name.as_str()).collect()
    }

    fn invoice() -> TypeDescription {
        ty(
            "invoice",
            ALL,
            vec![
                field(
                    "amount",
                    StorageKind::Decimal,
                    Authority::Source,
                    Access::Write,
                ),
                field(
                    "verdict",
                    StorageKind::Text,
                    Authority::Authored,
                    Access::Write,
                ),
                field(
                    "score",
                    StorageKind::Number,
                    Authority::Derived,
                    Access::Read,
                ),
                field("note", StorageKind::Text, Authority::Authored, Access::Read),
            ],
        )
    }

    #[test]
    fn a_type_gives_four_doors_and_the_generic_tools_follow_rights() {
        let owner = LakeRights {
            review: true,
            schema: true,
            ingest: true,
        };
        let t = tools_ok(&lake(vec![invoice()], vec![], owner));
        assert_eq!(
            names(&t),
            [
                "accept",
                "add_field",
                "declare_type",
                "describe",
                "invoice_find",
                "invoice_get",
                "invoice_retire",
                "invoice_upsert",
                "migrate",
                "pin",
                "proposals",
                "propose",
                "query",
                "reject",
                "search",
                "subscribe",
                "unsubscribe",
            ]
        );
    }

    #[test]
    fn a_reader_gets_only_reading_doors() {
        let reader = ty(
            "invoice",
            READ,
            vec![field(
                "amount",
                StorageKind::Number,
                Authority::Source,
                Access::Read,
            )],
        );
        let t = tools_ok(&lake(vec![reader], vec![], LakeRights::default()));
        assert_eq!(
            names(&t),
            [
                "describe",
                "invoice_find",
                "invoice_get",
                "pin",
                "query",
                "search",
                "subscribe",
                "unsubscribe",
            ]
        );
    }

    #[test]
    fn upsert_offers_only_authored_fields_the_principal_may_write() {
        let t = tools_ok(&lake(vec![invoice()], vec![], LakeRights::default()));
        let upsert = t.iter().find(|t| t.name == "invoice_upsert").unwrap();
        let fields = upsert.input_schema["properties"]["fields"]["properties"]
            .as_object()
            .unwrap();
        assert_eq!(fields.keys().collect::<Vec<_>>(), ["verdict"]);
    }

    #[test]
    fn a_field_the_principal_may_not_read_is_absent_from_the_record_schema() {
        let mut t = invoice();
        t.fields.push(field(
            "secret",
            StorageKind::Text,
            Authority::Authored,
            Access::None,
        ));
        let tools = tools_ok(&lake(vec![t], vec![], LakeRights::default()));
        let get = tools.iter().find(|t| t.name == "invoice_get").unwrap();
        let fields = &get.output_schema["properties"]["data"]["properties"]["fields"]["properties"];
        assert!(fields.get("amount").is_some());
        assert!(fields.get("secret").is_none());
    }

    #[test]
    fn a_verb_is_one_tool_and_says_whether_it_runs() {
        let pay = VerbDescription {
            name: "pay_invoice".into(),
            on: "invoice".into(),
            payload: vec![field(
                "amount",
                StorageKind::Decimal,
                Authority::Authored,
                Access::Write,
            )],
            risk: Risk::High,
            external: true,
            description: None,
        };
        let t = tools_ok(&lake(vec![], vec![pay], LakeRights::default()));
        let v = t.iter().find(|t| t.name == "pay_invoice").unwrap();
        assert_eq!(
            v.door,
            Door::Call {
                verb: "pay_invoice".into()
            }
        );
        assert!(v.description.contains("proposal"));
    }

    #[test]
    fn twenty_types_offer_about_eighty_tools() {
        let types = (0..20).map(|i| ty(&format!("t{i}"), ALL, vec![])).collect();
        let t = tools_ok(&lake(types, vec![], LakeRights::default()));
        // describe, pin, subscribe, unsubscribe, query, search, propose
        assert_eq!(t.len(), 80 + 7);
    }

    #[test]
    fn names_are_sanitised_sorted_and_valid_for_mcp() {
        let t = tools_ok(&lake(
            vec![
                ty("sys.source", READ, vec![]),
                ty("Invoice-Line", ALL, vec![]),
            ],
            vec![],
            LakeRights::default(),
        ));
        assert!(names(&t).contains(&"sys_source_find"));
        assert!(names(&t).contains(&"invoice_line_upsert"));
        let mut sorted = names(&t);
        sorted.sort();
        assert_eq!(sorted, names(&t));
        for tool in &t {
            assert!(!tool.name.is_empty() && tool.name.len() <= MAX_TOOL_NAME);
            assert!(
                tool.name
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
            );
        }
    }

    #[test]
    fn colliding_and_overlong_names_are_reported() {
        let problems = name_problems(
            &["a.b".into(), "a_b".into(), "x".repeat(70).into()],
            &["search".into(), "a_b_find".into()],
            &[],
        );
        let collided: Vec<&str> = problems
            .iter()
            .filter_map(|p| match p {
                NameProblem::Collision { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert!(collided.contains(&"a_b_find"));
        assert!(collided.contains(&"a_b_get"));
        assert!(collided.contains(&"search"));
        assert!(
            problems
                .iter()
                .any(|p| matches!(p, NameProblem::TooLong { .. }))
        );
        assert!(name_problems(&["invoice".into()], &["pay_invoice".into()], &[]).is_empty());
    }

    /// `invoice` with `approved_by`, a relation carried by link records
    /// with a writable `since` and a read-only `score`, and its link type as
    /// the principal is described it.
    fn linked_invoice() -> (TypeDescription, TypeDescription) {
        let mut invoice = invoice();
        invoice.relations = vec![crate::ontology::RelationDecl {
            name: "approved_by".into(),
            target: "partner".into(),
            cardinality: Default::default(),
            inverse: None,
            via: None,
            fields: vec![],
            authority: Authority::Authored,
            description: None,
        }];
        invoice.may.link = vec!["approved_by".into()];
        let link_type = ty(
            "sys.link.invoice.approved_by",
            READ,
            vec![
                field(
                    "from",
                    StorageKind::Reference,
                    Authority::Authored,
                    Access::Write,
                ),
                field(
                    "to",
                    StorageKind::Reference,
                    Authority::Authored,
                    Access::Write,
                ),
                field(
                    "since",
                    StorageKind::DateTime,
                    Authority::Authored,
                    Access::Write,
                ),
                field(
                    "score",
                    StorageKind::Number,
                    Authority::Authored,
                    Access::Read,
                ),
            ],
        );
        for f in &link_type.fields {
            if f.name.as_str() == "since" || f.name.as_str() == "score" {
                invoice.relations[0].fields.push(FieldDecl {
                    name: f.name.clone(),
                    ty: f.ty.clone(),
                    required: false,
                    authority: Authority::Authored,
                    derived: None,
                    function: None,
                    depends_on: vec![],
                    on_input_change: Default::default(),
                    sensitive: false,
                    searchable: false,
                    description: None,
                    label: None,
                    embed: None,
                });
            }
        }
        (invoice, link_type)
    }

    #[test]
    fn a_link_tool_offers_only_the_link_fields_the_principal_may_write() {
        let (invoice, link_type) = linked_invoice();
        let t = tools_ok(&lake(
            vec![invoice, link_type],
            vec![],
            LakeRights::default(),
        ));
        let link = t
            .iter()
            .find(|t| t.name == "invoice_approved_by_link")
            .unwrap();
        let fields = &link.input_schema["properties"]["fields"]["properties"];
        assert!(fields["since"].is_object(), "{fields}");
        assert!(
            fields.get("score").is_none(),
            "a read-only link field is not offered: {fields}"
        );
        assert!(fields.get("from").is_none() && fields.get("to").is_none());
    }

    #[test]
    fn a_link_relation_gets_link_and_unlink_tools_named_by_type_and_relation() {
        let mut invoice = invoice();
        invoice.relations = vec![crate::ontology::RelationDecl {
            name: "approved_by".into(),
            target: "partner".into(),
            cardinality: Default::default(),
            inverse: None,
            via: None,
            fields: vec![FieldDecl {
                name: "since".into(),
                ty: FieldType::kind(StorageKind::DateTime),
                required: false,
                authority: Authority::Authored,
                derived: None,
                function: None,
                depends_on: vec![],
                on_input_change: Default::default(),
                sensitive: false,
                searchable: false,
                description: None,
                label: None,
                embed: None,
            }],
            authority: Authority::Authored,
            description: None,
        }];
        // Offered only for the relations the principal may link.
        let without = tools_ok(&lake(vec![invoice.clone()], vec![], LakeRights::default()));
        assert!(!names(&without).iter().any(|n| n.ends_with("link")));
        invoice.may.link = vec!["approved_by".into()];
        let (_, link_type) = linked_invoice();
        let t = tools_ok(&lake(
            vec![invoice, link_type],
            vec![],
            LakeRights::default(),
        ));
        let link = t
            .iter()
            .find(|t| t.name == "invoice_approved_by_link")
            .expect("the link tool");
        assert_eq!(
            link.door,
            Door::Link {
                ty: "invoice".into(),
                relation: "approved_by".into()
            }
        );
        assert!(link.input_schema["properties"]["fields"]["properties"]["since"].is_object());
        assert_eq!(link.input_schema["required"], json!(["key", "to"]));
        let unlink = t
            .iter()
            .find(|t| t.name == "invoice_approved_by_unlink")
            .expect("the unlink tool");
        assert_eq!(
            unlink.door,
            Door::Unlink {
                ty: "invoice".into(),
                relation: "approved_by".into()
            }
        );
        // Two types' relations whose names join to one tool name collide.
        let problems = name_problems(
            &["a".into(), "a_b".into()],
            &[],
            &[("a".into(), "b_c".into()), ("a_b".into(), "c".into())],
        );
        assert!(
            problems
                .iter()
                .any(|p| matches!(p, NameProblem::Collision { name, .. } if name == "a_b_c_link")),
            "{problems:?}"
        );
    }

    #[test]
    fn the_generator_is_deterministic() {
        let l = lake(
            vec![invoice()],
            vec![],
            LakeRights {
                review: true,
                schema: true,
                ingest: true,
            },
        );
        assert_eq!(
            serde_json::to_string(&tools(&l).unwrap()).unwrap(),
            serde_json::to_string(&tools(&l).unwrap()).unwrap()
        );
    }

    const OWNER: LakeRights = LakeRights {
        review: true,
        schema: true,
        ingest: true,
    };

    #[test]
    fn the_generator_refuses_collisions_and_overlong_or_empty_names_itself() {
        let colliding = lake(
            vec![ty("a.b", READ, vec![]), ty("a_b", READ, vec![])],
            vec![],
            LakeRights::default(),
        );
        let problems = tools(&colliding).unwrap_err();
        assert!(
            problems
                .iter()
                .any(|p| matches!(p, NameProblem::Collision { name, .. } if name == "a_b_find"))
        );

        let long = lake(
            vec![ty(&"x".repeat(70), READ, vec![])],
            vec![],
            LakeRights::default(),
        );
        assert!(
            tools(&long)
                .unwrap_err()
                .iter()
                .any(|p| matches!(p, NameProblem::TooLong { .. }))
        );

        let empty = VerbDescription {
            name: "".into(),
            on: "invoice".into(),
            payload: vec![],
            risk: Risk::Low,
            external: false,
            description: None,
        };
        assert!(matches!(
            tools(&lake(vec![], vec![empty], LakeRights::default())).unwrap_err()[..],
            [NameProblem::Empty { .. }]
        ));
    }

    #[test]
    fn a_verb_payload_offers_only_fields_the_principal_may_write() {
        let verb = VerbDescription {
            name: "settle".into(),
            on: "invoice".into(),
            payload: vec![
                field(
                    "amount",
                    StorageKind::Decimal,
                    Authority::Authored,
                    Access::Write,
                ),
                field(
                    "approver",
                    StorageKind::Text,
                    Authority::Authored,
                    Access::Read,
                ),
            ],
            risk: Risk::Low,
            external: false,
            description: None,
        };
        let t = tools_ok(&lake(vec![], vec![verb], LakeRights::default()));
        let payload = &t.iter().find(|t| t.name == "settle").unwrap().input_schema["properties"]["payload"]
            ["properties"];
        assert!(payload.get("amount").is_some());
        assert!(payload.get("approver").is_none());
    }

    fn walk_for_empty_enums(v: &Value, path: &str, found: &mut Vec<String>) {
        match v {
            Value::Object(m) => {
                if m.get("enum")
                    .and_then(Value::as_array)
                    .is_some_and(|a| a.is_empty())
                {
                    found.push(path.to_owned());
                }
                for (k, v) in m {
                    walk_for_empty_enums(v, &format!("{path}.{k}"), found);
                }
            }
            Value::Array(a) => a.iter().for_each(|v| walk_for_empty_enums(v, path, found)),
            _ => {}
        }
    }

    #[test]
    fn no_schema_carries_an_empty_enum() {
        for l in [
            lake(vec![], vec![], OWNER),
            lake(vec![ty("bare", ALL, vec![])], vec![], OWNER),
        ] {
            let mut found = Vec::new();
            for t in tools_ok(&l) {
                walk_for_empty_enums(&t.input_schema, &t.name, &mut found);
                walk_for_empty_enums(&t.output_schema, &t.name, &mut found);
            }
            assert!(found.is_empty(), "empty enums at {found:?}");
        }
    }

    #[test]
    fn the_generic_tools_emitted_are_exactly_the_generic_list() {
        let (invoice, link_type) = linked_invoice();
        let t = tools_ok(&lake(vec![invoice, link_type], vec![], OWNER));
        assert!(
            names(&t).contains(&"invoice_approved_by_link"),
            "the fixture's link relation has its tools"
        );
        let mut emitted: Vec<&str> = t
            .iter()
            .filter(|t| {
                !matches!(
                    t.door,
                    Door::Find { .. }
                        | Door::Get { .. }
                        | Door::Upsert { .. }
                        | Door::Retire { .. }
                        | Door::Link { .. }
                        | Door::Unlink { .. }
                        | Door::Call { .. }
                )
            })
            .map(|t| t.name.as_str())
            .collect();
        emitted.sort();
        let mut listed = GENERIC_TOOLS.to_vec();
        listed.sort();
        assert_eq!(emitted, listed);
    }

    #[test]
    fn the_argument_schemas_reach_every_argument_the_doors_take() {
        let t = tools_ok(&lake(vec![invoice()], vec![], OWNER));
        let arg = |tool: &str, path: &[&str]| {
            let mut v = &t.iter().find(|t| t.name == tool).unwrap().input_schema["properties"];
            for p in path {
                v = &v[*p];
            }
            v.clone()
        };
        assert!(arg("invoice_find", &["order_by", "items", "properties", "by"]).is_object());
        assert!(arg("query", &["to"]).is_object());
        assert!(arg("query", &["order_by"]).is_object());
        assert!(arg("describe", &["valid_at"]).is_object());
        assert!(arg("search", &["cursor"]).is_object());
        assert!(arg("proposals", &["state"]).is_object());
    }
}
