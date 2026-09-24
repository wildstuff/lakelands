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
//! `<type>_retire`, each only if the principal may use it. Per verb: the
//! verb's name. Generic: `describe`, `query`, `search`, `propose`,
//! `proposals`, `accept`, `reject`, `declare_type`, `add_field`,
//! `migrate`, each only if the principal may use it. A 20-type lake
//! offers about 80 tools.
//!
//! Three doors of [`Catalog`](crate::Catalog) have no tool of their own:
//! `describe_type` is `describe` with a `type`, `migrate_apply` is
//! `migrate` with a `report_id`, and `ingest` is deliberately not a tool —
//! a source feeds a lake through a stream, not through a model.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::describe::{FieldDescription, LakeDescription, TypeDescription, VerbDescription};
use crate::id::{TypeName, VerbName};
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
    /// `call` of one verb.
    Call {
        /// The verb.
        verb: VerbName,
    },
    /// `describe`.
    Describe,
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
const QUERY: &str = "query";
const SEARCH: &str = "search";
const PROPOSE: &str = "propose";
const PROPOSALS: &str = "proposals";
const ACCEPT: &str = "accept";
const REJECT: &str = "reject";
const DECLARE_TYPE: &str = "declare_type";
const ADD_FIELD: &str = "add_field";
const MIGRATE: &str = "migrate";

/// The names of the generic tools, which no type or verb may produce.
/// The generator emits these constants and no other generic name.
pub const GENERIC_TOOLS: [&str; 10] = [
    DESCRIBE,
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

/// Every tool name the lake's declarations would produce, for any
/// principal, with the declaration each comes from.
fn all_names(types: &[TypeName], verbs: &[VerbName]) -> BTreeMap<String, Vec<String>> {
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
    names
}

/// The problems a set of type and verb names would cause, checked over
/// every door any principal could be granted. Empty when every name can
/// become a tool.
pub fn name_problems(types: &[TypeName], verbs: &[VerbName]) -> Vec<NameProblem> {
    let mut problems = Vec::new();
    for (name, from) in all_names(types, verbs) {
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
    let problems = name_problems(&types, &verbs);
    if !problems.is_empty() {
        return Err(problems);
    }
    let mut out = Vec::new();
    for ty in &lake.types {
        type_tools(ty, lake, &mut out);
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
                     "description": "Per field: origin, known_at, seen_at, position, writer, source, procedure_version."},
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
        "properties": {"field": fields,
                       "by": {"enum": ["value", "known_at", "seen_at"]},
                       "dir": {"enum": ["asc", "desc"]}},
        "required": ["field"], "additionalProperties": false}})
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
                "aggregate",
                json!({"type": "object", "properties": {
                    "measures": {"type": "array", "items": {
                        "type": "object",
                        "properties": {"name": {"type": "string"},
                                       "func": {"enum": ["count", "sum", "min", "max", "avg"]},
                                       "field": one_of(filterable())},
                        "required": ["name", "func"], "additionalProperties": false}},
                    "group_by": {"type": "array", "items": one_of(filterable())}},
                    "required": ["measures"], "additionalProperties": false}),
            ),
            ("with_meta", json!({"type": "boolean"})),
        ]);
        time_axes(&mut p);
        out.push(ToolSpec {
            name: format!("{base}_find"),
            description: format!(
                "Find {} records: filter with `where`, sort, page with `cursor`, or aggregate. Every answer carries the position it was read at.",
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
            description: "Change a type's shape. Without `report_id` it only reports what would convert; with the id of that report it applies a lossless migration.".into(),
            input_schema: obj(
                props([
                    ("to", json!({"type": "object"})),
                    (
                        "renames",
                        json!({"type": "array", "items": {
                        "type": "array", "items": {"type": "string"},
                        "minItems": 2, "maxItems": 2}}),
                    ),
                    ("report_id", json!({"type": "string"})),
                ]),
                &["to"],
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
            depends_on: vec![],
            on_input_change: Default::default(),
            sensitive: false,
            searchable: false,
            description: None,
            label: None,
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
            },
        }
    }

    const ALL: TypeRights = TypeRights {
        find: true,
        get: true,
        upsert: true,
        retire: true,
    };
    const READ: TypeRights = TypeRights {
        find: true,
        get: true,
        upsert: false,
        retire: false,
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
                "proposals",
                "propose",
                "query",
                "reject",
                "search",
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
            ["describe", "invoice_find", "invoice_get", "query", "search"]
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
        assert_eq!(t.len(), 80 + 4); // describe, query, search, propose
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
        assert!(name_problems(&["invoice".into()], &["pay_invoice".into()]).is_empty());
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
        let t = tools_ok(&lake(vec![invoice()], vec![], OWNER));
        let mut emitted: Vec<&str> = t
            .iter()
            .filter(|t| {
                !matches!(
                    t.door,
                    Door::Find { .. }
                        | Door::Get { .. }
                        | Door::Upsert { .. }
                        | Door::Retire { .. }
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
