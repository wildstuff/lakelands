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

/// A view's parameters as one required object of the read's input, each
/// parameter required and of its kind's schema. Answers whether the view
/// declares any.
fn view_params(ty: &TypeDescription, lake: &LakeDescription, p: &mut Map<String, Value>) -> bool {
    let Some(params) = ty
        .view
        .as_ref()
        .map(|v| &v.params)
        .filter(|ps| !ps.is_empty())
    else {
        return false;
    };
    let properties: Map<String, Value> = params
        .iter()
        .map(|q| {
            (
                q.name.to_string(),
                field_schema(&FieldType::kind(q.kind), lake),
            )
        })
        .collect();
    let names: Vec<&str> = params.iter().map(|q| q.name.as_str()).collect();
    p.insert(
        "params".into(),
        json!({"type": "object",
               "description": "The view's parameters, every one bound; a date alone means midnight UTC.",
               "properties": properties, "required": names, "additionalProperties": false}),
    );
    true
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
                     "description": "Per field: origin, known_at, seen_at, position, writer, source, procedure_version, upstream (the lake and position a value read from another lake came from); and error, for a computed field whose function failed, which then holds no value."},
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
        let params = view_params(ty, lake, &mut p);
        out.push(ToolSpec {
            name: format!("{base}_find"),
            description: format!(
                "Find {} records: filter with `where`, sort, page with `cursor` or resume after a row with `after`, or aggregate. Every answer carries the position it was read at.",
                label(ty)
            ),
            input_schema: obj(p, if params { &["params"] } else { &[] }),
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
        let params = view_params(ty, lake, &mut p);
        out.push(ToolSpec {
            name: format!("{base}_get"),
            description: format!("Get one {} record by key.", label(ty)),
            input_schema: obj(p, if params { &["key", "params"] } else { &["key"] }),
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
    let mut p = props([(
        "payload",
        json!({"type": "object", "properties": payload,
                       "required": required, "additionalProperties": false}),
    )]);
    write_opts(&mut p);
    // A verb on its type takes no key, and has no record version to
    // compare: it takes neither.
    let on_record = verb.scope.is_record();
    if on_record {
        p.insert(
            "key".into(),
            json!({"type": "string",
                   "description": format!("The key of the `{}` record.", verb.on)}),
        );
    } else {
        p.remove("expected_version");
    }
    let what = verb.description.clone().unwrap_or_else(|| {
        if on_record {
            format!("Call `{}` on a `{}` record.", verb.name, verb.on)
        } else {
            format!("Call `{}` on the type `{}`.", verb.name, verb.on)
        }
    });
    // By the risk alone: a `low` verb runs at once, an external one too.
    let how = match verb.risk {
        crate::ontology::Risk::Low => "It runs at once.",
        _ => "It is recorded as a proposal and runs when accepted.",
    };
    ToolSpec {
        name: tool_base(verb.name.as_str()),
        description: format!("{what} {how}"),
        input_schema: obj(
            p,
            if on_record {
                &["key", "payload"]
            } else {
                &["payload"]
            },
        ),
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
        description: "Describe the lake: its types, verbs, value types, functions, triggers and object sets as declared; or one type on six axes: structure, access, rights, actions, lineage, lifecycle.".into(),
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
                    "where": {"type": "string"}, "search": {"type": "object"},
                    "params": {"type": "object",
                               "description": "The parameters of a view the pattern starts on, by name."}},
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
                "as": {"enum": ["rows", "graph", "aggregate", "degree", "components"],
                       "description": "`degree` answers rows of `type`, `key` and `degree` (links over the steps' relations), highest first; `components` answers rows of `component`, `size`, `type` and `key`, largest component first. Both take steps without `depth` or `where`; `components` walks each step `both`."},
                "fields": {"type": "array", "items": {"type": "string"}}}}),
            ),
            ("order_by", order_by_schema(json!({"type": "string"}))),
            ("limit", json!({"type": "integer", "minimum": 1})),
            ("cursor", json!({"type": "string"})),
        ]);
        time_axes(&mut p);
        out.push(ToolSpec {
            name: QUERY.into(),
            description: "Walk declared relations from a start (a record, a filter or a search) and return rows or a graph. Covers neighbours, traversal, paths and subgraphs, and two whole-graph measures: degree centrality and connected components.".into(),
            input_schema: obj(p, &["start"]),
            output_schema: reply_schema(json!({"type": "object"})),
            door: Door::Query,
        });

        let mut p = props([
            ("type", names(|t| t.may.find)),
            ("mode", json!({"enum": ["lexical", "vector", "hybrid"]})),
            ("text", json!({"type": "string"})),
            (
                "fields",
                json!({"type": "array", "items": {"type": "string"}, "minItems": 1,
                "description": "lexical and hybrid: match the text only in these searchable fields."}),
            ),
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
        // Beside the record writes it may make, a writer may propose
        // declarations and migrations, which a reviewer holding the
        // `schema` grant accepts.
        let kinds = ["upsert", "retire", "call", "declare", "migrate"];
        out.push(ToolSpec {
            name: PROPOSE.into(),
            description: "Record a write as a proposal for review instead of applying it: a \
                record write, a verb call, declarations (as apply takes them) or a migration \
                (as migrate takes it); a declaration or migration is accepted by a reviewer \
                holding the schema grant."
                .into(),
            input_schema: obj(
                props([
                    (
                        "write",
                        json!({"type": "object", "properties": {
                        "write": {"enum": kinds},
                        "declarations": {"type": "array", "items": {"type": "object"}},
                        "migration": {"type": "object"}}, "required": ["write"]}),
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
                "kind",
                json!({"enum": ["upsert", "retire", "call", "declare", "migrate"]}),
            ),
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
mod tests;
