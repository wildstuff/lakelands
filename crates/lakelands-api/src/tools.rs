//! The tool-name generator: the one place an LLM-facing tool name is
//! made.
//!
//! [`tools`] is a pure function of a [`LakeDescription`]. Because
//! `describe` already answers as one principal may see the lake, the
//! tools it yields — their names, and the JSON schemas in and out — are
//! filtered by that principal's grants without a second rule. The
//! store's catalog and every transport take names from here; the
//! `single-catalog` gate refuses a tool name written anywhere else.
//!
//! Per type: `<type>_find`, `<type>_get`, `<type>_upsert`,
//! `<type>_retire`, each only if the principal may use it. Per relation
//! carried by link records that the principal may link:
//! `<type>_<relation>_link` and `<type>_<relation>_unlink`, named by the
//! type and the relation, since relation names are unique per type only. Per verb: the
//! verb's name. Generic: `describe`, `query`, `search`, `propose`,
//! `proposals`, `accept`, `reject`, `declare_type`, `add_field`, `recompute`,
//! `migrate`, each only if the principal may use it, and the MCP session's
//! doors: `pin`, for `as_of` across a turn, and `subscribe` and `unsubscribe`,
//! for being told of changes. A 20-type lake offers about 80 tools.
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
use crate::error::{catalog, closed};
use crate::id::{RelationName, TypeName, VerbName};
use crate::key::LINK_PREFIX;
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
    /// `change` on one type: a change to the records a condition selects,
    /// planned first (`docs/decisions/081-a-change-to-the-records-a-condition-selects-is-planned-before-it-acts.md`).
    Change {
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
    /// `cypher`: a statement of the openCypher subset, translated into one
    /// pattern and run as `query` runs it; a read tool, listed where
    /// `query` is.
    Cypher,
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
    /// `recompute`.
    Recompute,
    /// `migrate`: the preflight, or with a `report_id` argument the
    /// `migrate_apply` door.
    Migrate,
    /// `plan`: a change's plan. Served over REST only; the tools that make
    /// a change take `plan` and `plan_id` instead
    /// (`docs/decisions/062-a-change-plans-before-it-acts.md`).
    Plan,
    /// `enact`: a change made against its plan. Served over REST only, as
    /// `plan` is.
    Enact,
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
const CYPHER: &str = "cypher";
const SEARCH: &str = "search";

/// The second stage a search or a pattern's start asks for.
fn rerank_schema() -> Value {
    json!({"type": "object", "properties": {
        "top": {"type": "integer", "minimum": 1,
                "description": "How many of the first hits the lake's reranker ranks again, at most the lake's `limits.max_rerank_top`; the answer ends at this window."},
        "text": {"type": "string",
                 "description": "What the window is ranked against; the search's own text when left out, so a vector search must give it."}},
        "required": ["top"]})
}
const PROPOSE: &str = "propose";
const PROPOSALS: &str = "proposals";
const ACCEPT: &str = "accept";
const REJECT: &str = "reject";
const DECLARE_TYPE: &str = "declare_type";
const ADD_FIELD: &str = "add_field";
const RECOMPUTE: &str = "recompute";
const MIGRATE: &str = "migrate";

/// The kinds of change a session may subscribe to: the log's event kinds
/// that change a record, or say something happened to one.
pub const CHANGE_KINDS: [&str; 7] = [
    "record_upserted",
    "record_ingested",
    "record_retired",
    "output_recorded",
    "trigger_fired",
    "trigger_dropped",
    "object_set_changed",
];

/// The names of the generic tools, which no type or verb may produce.
/// The generator emits these constants and no other generic name.
pub const GENERIC_TOOLS: [&str; 15] = [
    DESCRIBE,
    PIN,
    SUBSCRIBE,
    UNSUBSCRIBE,
    QUERY,
    CYPHER,
    SEARCH,
    PROPOSE,
    PROPOSALS,
    ACCEPT,
    REJECT,
    DECLARE_TYPE,
    ADD_FIELD,
    RECOMPUTE,
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

const DOOR_SUFFIXES: [&str; 5] = ["find", "get", "upsert", "retire", "change"];

catalog! {
    join = " and ";
    /// A declaration a tool name comes from.
    pub enum NameSource {
        /// A generic tool, by its name.
        Generic { tool: String } => "the generic tool `{tool}`",
        /// A type, whose doors are tools.
        Type {
            #[serde(rename = "type")]
            ty: TypeName,
        } => "type `{ty}`",
        /// A verb.
        Verb { verb: VerbName } => "verb `{verb}`",
        /// A relation of a type, whose link doors are tools.
        Link {
            relation: RelationName,
            #[serde(rename = "type")]
            ty: TypeName,
        } => "relation `{relation}` of type `{ty}`",
    }
}

catalog! {
    tag = "problem";
    /// Why a lake's declarations cannot all become tools. The store refuses a declaration that would introduce one of these, so [`tools`] never meets them in a lake the store wrote.
    pub enum NameProblem {
        /// Two declarations produce the same tool name.
        Collision {
            name: String,
            #[serde(with = "closed::list")]
            from: Vec<NameSource>,
        } => "the tool name `{name}` would come from {from}",
        /// A tool name would be longer than [`MAX_TOOL_NAME`]; one problem for each declaration it comes from.
        TooLong {
            name: String,
            #[serde(default, skip_serializing_if = "Option::is_none", with = "closed::option")]
            from: Option<NameSource>,
        } => "the tool name `{name}`[ of {from}] is longer than 64 characters",
        /// A declaration's name has nothing left once made a tool name.
        Empty {
            #[serde(with = "closed")]
            from: NameSource,
        } => "{from} gives an empty tool name",
    }
}

// `NameProblem::TooLong`'s template says the limit.
const _: () = assert!(MAX_TOOL_NAME == 64);

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
pub fn all_names(
    types: &[TypeName],
    verbs: &[VerbName],
    links: &[(TypeName, RelationName)],
) -> BTreeMap<String, Vec<NameSource>> {
    let mut names: BTreeMap<String, Vec<NameSource>> = BTreeMap::new();
    let mut add = |name: String, from: NameSource| names.entry(name).or_default().push(from);
    for g in GENERIC_TOOLS {
        let tool = g.to_owned();
        add(g.to_owned(), NameSource::Generic { tool });
    }
    for ty in types {
        let base = tool_base(ty.as_str());
        let ty = ty.clone();
        if base.is_empty() {
            add(base, NameSource::Type { ty });
            continue;
        }
        for s in DOOR_SUFFIXES {
            let ty = ty.clone();
            add(format!("{base}_{s}"), NameSource::Type { ty });
        }
    }
    for v in verbs {
        let verb = v.clone();
        add(tool_base(v.as_str()), NameSource::Verb { verb });
    }
    for (ty, relation) in links {
        for s in LINK_SUFFIXES {
            let (relation, ty) = (relation.clone(), ty.clone());
            add(
                link_tool(&ty, &relation, s),
                NameSource::Link { relation, ty },
            );
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
            problems.extend(from.iter().map(|from| NameProblem::TooLong {
                name: name.clone(),
                from: Some(from.clone()),
            }));
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
                .filter(|r| r.carried_by_links())
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
    let params = ty.view.as_ref().map_or(&[][..], |v| &v.params);
    if params.is_empty() {
        return false;
    }
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
               "description": "Read the lake as it was at this position; now() is the time it was written, unless valid_at names one."}),
    );
    p.insert(
        "valid_at".into(),
        json!({"type": "string", "format": "date-time",
               "description": "Read what was true in the world at this instant; now() and today() are this instant."}),
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
        StorageKind::Date => json!({"type": "string", "format": "date"}),
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
                     "description": "Per field: origin, known_at, seen_at, position, writer, source, procedure (the verb, function or embedding model that produced the value) and procedure_version, upstream (the lake and position a value read from another lake came from); and error, for a computed field whose function failed, which then holds no value."},
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

/// The sort keys of records that each have an event and a provenance:
/// a stored type's.
fn order_by_schema(fields: Value) -> Value {
    json!({"type": "array", "items": {
        "type": "object",
        "description": "One sort key. `latest` orders by the position of each record's latest change and takes no field; every other `by` names its field.",
        "properties": {"field": fields,
                       "by": {"enum": ["value", "known_at", "seen_at", "latest"]},
                       "dir": {"enum": ["asc", "desc"]}},
        "additionalProperties": false}})
}

/// The sort keys of what has no event or provenance of its own to order
/// by, a view's records, a query's rows, a proposal: each by its value,
/// so no `by` is offered that its door refuses.
fn value_order_schema(fields: Value, what: &str) -> Value {
    json!({"type": "array", "items": {"type": "object",
        "description": format!("One sort key of {what}, by its value."),
        "properties": {"field": fields, "dir": {"enum": ["asc", "desc"]}},
        "required": ["field"],
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
               "description": "A retry with the same key appends nothing and answers the first outcome; for a verb that reaches outside the lake, its executor's recorded answer, or the refusal `unanswered` while none is recorded."}),
    );
}

fn outcome_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "outcome": {"enum": ["applied", "proposed"]},
            "position": {"type": "integer"},
            "version": {"type": "integer"},
            "output": {"description": "An external verb's recorded output."},
            "through": {"type": "integer",
                        "description": "The last position of what the write set off, when after `position`."},
            "fired": {"type": "array", "items": {"type": "string"},
                      "description": "The triggers the write fired."},
            "joined": {"type": "array", "items": {"type": "string"},
                       "description": "The object sets the record joined."},
            "left": {"type": "array", "items": {"type": "string"},
                     "description": "The object sets the record left."},
            "renamed": {"type": "array",
                        "items": {"type": "array", "items": {"type": "string"},
                                  "minItems": 2, "maxItems": 2},
                        "description": "For a migration: each field it renamed, old name and new."},
            "plan": {"type": "object",
                      "description": "For a change to the ontology: the plan that applied (see `plan`)."},
            "declared": {"type": "array",
                         "items": {"type": "object",
                                   "properties": {
                                       "kind": {"enum": crate::write::DeclarationKind::ALL
                                           .map(crate::write::DeclarationKind::as_str)},
                                       "name": {"type": "string"},
                                       "version": {"type": "integer"},
                                       "since": {"type": "integer"},
                                       "appended": {"type": "boolean"},
                                   },
                                   "required": ["kind", "name", "version", "since", "appended"]},
                         "description": "For `declare`, `apply`, a `propose` of declarations all in force and the `accept` of proposed declarations: each declaration sent, as it stands in force after the call, and whether this call appended it."},
            "proposal": {"type": "string"},
            "why": {"type": "string"},
        },
        "required": ["outcome", "position"],
    })
}

/// What a change's plan says, as `plan: true` answers it in a reply's
/// `data` (062).
fn plan_schema() -> Value {
    let count = |what: &str| {
        json!({"type": "array", "description": what, "items": {"type": "object",
            "properties": {"field": {"type": "string"}, "records": {"type": "integer"},
                           "sample_keys": {"type": "array", "items": {"type": "string"}}}}})
    };
    reply_schema(json!({
        "type": "object",
        "properties": {
            "report_id": {"type": "string",
                          "description": "Send it back as `plan_id` to make exactly this change."},
            "at": {"type": "integer"},
            "types": {"type": "array", "items": {"type": "object", "properties": {
                "type": {"type": "string"},
                "recomputed": {"type": "integer",
                               "description": "Live records computed again."},
                "unlive": count("Per reference field: the live records left naming no live record."),
                "breaches": {"type": "array", "items": {"type": "object"},
                             "description": "Per constraint: the live records that would break it."},
            }}},
            "value_types": {"type": "array", "items": {"type": "object"}},
            "records": {"type": "object",
                        "description": "For a change to records (081): what it selects, changes and is refused.",
                        "properties": {
                            "type": {"type": "string"},
                            "selected": {"type": "integer",
                                         "description": "How many records the condition selects."},
                            "sample_keys": {"type": "array", "items": {"type": "string"}},
                            "fields": count("Per field set: how many selected records it would change."),
                            "refused": {"type": "array", "items": {"type": "object", "properties": {
                                "refusal": {"type": "object"}, "records": {"type": "integer"},
                                "sample_keys": {"type": "array", "items": {"type": "string"}}}},
                                "description": "Each refusal a selected record would meet; one is enough that the change cannot be enacted."}},
                        "required": ["type", "selected", "sample_keys"]},
            "lossless": {"type": "boolean"},
        },
        "required": ["report_id", "at", "lossless"],
    }))
}

/// The arguments and the answer of a tool that makes a change to the
/// ontology: planned first with `plan`, made against its plan with
/// `plan_id`, or made at once (062).
fn planned(mut input: Value) -> (Value, Value) {
    let props = &mut input["properties"];
    props["plan"] = json!({"type": "boolean",
        "description": "Answer the change's plan and change nothing."});
    props["plan_id"] = json!({"type": "string",
        "description": "Make the change only if its plan is still this one (the plan's `report_id`)."});
    (input, json!({"anyOf": [outcome_schema(), plan_schema()]}))
}

/// The sentence a tool that makes a change to the ontology ends with.
const PLAN_FIRST: &str = " Plan it first: with `plan: true` it answers what the change would do, the records it computes again, those it would leave naming no live record and those that would break a constraint, and changes nothing; then call it again with that plan's `report_id` as `plan_id` to make exactly that change, refused `plan_stale` if the lake moved.";

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

/// The `key` argument of a type's tools, described so that a caller
/// knows what to pass: the key fields' values for a keyed type, `all` for
/// a view without key fields, and for a type that declares no key, the
/// caller's own choice when it writes, and the key it wrote under after.
fn key_schema(ty: &TypeDescription, writes: bool) -> Value {
    let name = &ty.name;
    let fields: Vec<String> = ty.access.key.iter().map(|f| format!("`{f}`")).collect();
    let description = match fields.as_slice() {
        [] if ty.lineage.view_of.is_some() => format!(
            "`{}`: `{name}` is a view without key fields, so it holds one record, under this key.",
            crate::key::UNGROUPED
        ),
        [] if writes => format!(
            "`{name}` declares no key, so the key is yours to choose. It names this record from now on, and a later write with the same key changes it."
        ),
        [] => format!("The key the `{name}` record was written under."),
        [one] => format!("The key of the `{name}` record: the value of its {one}."),
        many => format!(
            "The key of the `{name}` record: the values of its {} joined by `:`, with `%` written `%25` and `:` written `%3A`.",
            many.join(", ")
        ),
    };
    json!({"type": "string", "description": description})
}

/// The schemas of the fields a principal may write on a type.
fn writable_fields(ty: &TypeDescription, lake: &LakeDescription) -> Map<String, Value> {
    ty.fields
        .iter()
        .filter(|f| f.authored_writable())
        .map(|f| {
            let s = described(field_schema(&f.ty, lake), f);
            (f.name.to_string(), json!({"anyOf": [s, {"type": "null"}]}))
        })
        .collect()
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
                       "description": "A condition in the expression grammar, e.g. status = 'open' and amount > 100; a list is written in parentheses, status in ('open', 'paid'), not in brackets as a `cypher` statement writes it; a text is matched by a function, contains(name, 'Kest') or starts_with(name, 'Kest'), not by a word between its operands."}),
            ),
            (
                "order_by",
                match ty.lineage.view_of {
                    Some(_) => {
                        value_order_schema(one_of(filterable()), "the view's records: a field")
                    }
                    None => order_by_schema(one_of(filterable())),
                },
            ),
            ("limit", json!({"type": "integer", "minimum": 1})),
            (
                "cursor",
                json!({"type": "string",
                       "description": "The `next_cursor` of an earlier answer, for the next page. Send it with the same `where` and `order_by` as the page it came from."}),
            ),
            (
                "offset",
                json!({"type": "integer", "minimum": 0,
                       "description": "Skip this many rows of the order before the page, to jump to a page of a table. The rows skipped are read, so a large offset costs more; continue from there with the cursor. Not with cursor, after or aggregate."}),
            ),
            (
                "total",
                json!({"type": "boolean",
                       "description": "Also answer how many records match, at the answer's position. Counted only when asked."}),
            ),
            (
                "retired",
                json!({"type": "boolean",
                       "description": "Also answer how many retired records match, at the same position. Counted only when asked; not with aggregate."}),
            ),
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
                "Find {} records: filter with `where`, sort, page with `cursor`, resume after a row with `after` or jump with `offset`, ask for the `total`, or aggregate. Every answer carries the position it was read at.",
                label(ty)
            ),
            input_schema: obj(p, if params { &["params"] } else { &[] }),
            output_schema: reply_schema(json!({
                "type": "object",
                "properties": {"shape": {"enum": ["records", "groups"]},
                               "rows": {"type": "array"},
                               "next_cursor": {"type": "string"},
                               "total": {"type": "integer"},
                               "retired": {"type": "integer"}},
                "required": ["shape", "rows"]})),
            door: Door::Find { ty: ty.name.clone() },
        });
    }
    if ty.may.get {
        let mut p = props([
            ("key", key_schema(ty, false)),
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
        let writable = writable_fields(ty, lake);
        // A create must send what a constraint needs of it (lakelands#1169);
        // where none it can send is taken, the tool changes records only
        // (lakelands#1188).
        let needed: Vec<&str> = (ty.fields.iter())
            .filter(|f| f.required_at_create && f.authored_writable())
            .map(|f| f.name.as_str())
            .collect();
        let mut fields_note =
            "Only the fields to change: an absent field is unchanged, null clears it.".to_owned();
        if ty.may.create && !needed.is_empty() {
            fields_note.push_str(&format!(
                " To create the record, send also: {}.",
                needed.join(", ")
            ));
        }
        let mut p = props([
            ("key", key_schema(ty, true)),
            (
                "fields",
                json!({"type": "object", "properties": writable, "additionalProperties": false,
                       "description": fields_note}),
            ),
        ]);
        write_opts(&mut p);
        let what = match &ty.may.create_why {
            Some(why) if !ty.may.create => {
                format!(
                    "Change a record of {}; it cannot create one: {why}.",
                    label(ty)
                )
            }
            _ => format!("Create or change a record of {}.", label(ty)),
        };
        out.push(ToolSpec {
            name: format!("{base}_upsert"),
            description: format!(
                "{what} Answers `applied` with the new position, or `proposed` if the write waits for review."
            ),
            input_schema: obj(p, &["key", "fields"]),
            output_schema: outcome_schema(),
            door: Door::Upsert { ty: ty.name.clone() },
        });
    }
    if ty.may.find && (ty.may.upsert || ty.may.retire) {
        // A link type's ends name the link: a change sets its own fields or
        // unlinks, and never names an end.
        let link = ty.name.as_str().starts_with(LINK_PREFIX);
        let mut set_fields = writable_fields(ty, lake);
        if link {
            set_fields.retain(|name, _| name != "from" && name != "to");
        }
        let mut p = props([(
            "where",
            json!({"type": "string",
                   "description": "The records to change: a condition in the expression grammar, read as `find` reads it."}),
        )]);
        if ty.may.upsert {
            p.insert(
                "set".into(),
                json!({"type": "object", "properties": set_fields,
                       "additionalProperties": false,
                       "description": "The values to set, the same on every record selected. Send this or `retire`."}),
            );
        }
        if ty.may.retire {
            p.insert(
                "retire".into(),
                json!({"const": true, "description": if link {
                    "Unlink every link selected. Send this or `set`."
                } else {
                    "Retire every record selected. Send this or `set`."
                }}),
            );
        }
        let (mut input, _) = planned(obj(p, &["where"]));
        input["properties"]["plan"]["description"] = json!(
            "Answer the change's plan and change nothing. Also what a call without `plan` or `plan_id` does."
        );
        out.push(ToolSpec {
            name: format!("{base}_change"),
            description: format!(
                "{} every {} record a condition selects. It is planned first and enacted only against that plan: `plan: true`, or a call with neither `plan` nor `plan_id`, answers how many records it selects, the first keys, the records each field would change and each refusal a record would meet, and changes nothing; call it again with that plan's `report_id` as `plan_id` to make exactly that change in one append, refused `plan_stale` if a selected record changed or the selection moved, and `too_large` past the lake's `max_change_records`, never cut. A refusal of one record refuses the whole change.",
                match (ty.may.upsert, ty.may.retire, link) {
                    (true, true, false) => "Set values on, or retire,",
                    (true, true, true) => "Set the link's own fields on, or unlink (`retire`),",
                    (true, false, false) => "Set values on",
                    (true, false, true) => "Set the link's own fields on",
                    (false, _, false) => "Retire",
                    (false, _, true) => "Unlink (`retire`)",
                },
                label(ty)
            ),
            input_schema: input,
            output_schema: json!({"anyOf": [outcome_schema(), plan_schema()]}),
            door: Door::Change { ty: ty.name.clone() },
        });
    }
    if ty.may.retire {
        let mut p = props([("key", key_schema(ty, false))]);
        write_opts(&mut p);
        out.push(ToolSpec {
            name: format!("{base}_retire"),
            description: format!(
                "Retire a record of {}. Its history stays in the lake.",
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
        let key = json!({"type": "string",
                         "description": format!("The key of the `{}` record the link is from.", ty.name)});
        let target = json!({"type": "string",
                            "description": format!("The key of the `{}` record to link to.", r.target)});
        let mut p = props([
            ("key", key.clone()),
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
        let mut p = props([("key", key), ("to", target)]);
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
            format!("Call `{}` on a record of `{}`.", verb.name, verb.on)
        } else {
            format!("Call `{}` on the type `{}`.", verb.name, verb.on)
        }
    });
    // By the risk: a `low` verb runs at once, an external one too. A verb
    // that reaches outside the lake runs through an executor its host
    // binds; the tool says whether it is bound here, and says what a call
    // does without one when the answer does not tell (an older server).
    let (how, refused) = if verb.risk == crate::ontology::Risk::Low {
        ("It runs at once", "a call is refused")
    } else {
        (
            "It is recorded as a proposal and runs when accepted",
            "a call or a proposal is refused",
        )
    };
    let through = match &verb.executor {
        Some(e) if e.bound => format!(
            ", through its executor `{}`, which this lake's host binds.",
            e.name
        ),
        Some(e) => format!(
            ", through its executor `{}`; that executor is not bound in this lake, so {refused} and writes nothing.",
            e.name
        ),
        None if verb.external => ", through an executor the lake's host binds; where none is bound, a call is refused and writes nothing.".into(),
        None if verb.retires => ", and retires the record; its history stays in the lake.".into(),
        None => ".".into(),
    };
    ToolSpec {
        name: tool_base(verb.name.as_str()),
        description: format!("{what} {how}{through}"),
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

/// What a pattern's `as` says to the model that writes one.
const BINDING: &str = "The name a row binds this record under, `<name>.<field>`; its type's name when left out. Two records of one type in a row need a name each.";

/// [`BINDING`] for a step, which may reach several types.
const STEP_BINDING: &str = "The name a row binds this step's records under, `<name>.<field>`, with `<name>.key`, `<name>._type` and `<name>._depth`; its type's name when left out, or its relation's where it reaches several types. Name a step that may reach several types: left out, its name follows the types its reader is shown.";

/// What a step's `from`, `join`, `optional` and `absent` say: a pattern
/// of rows is a tree (`docs/decisions/080-a-pattern-may-branch-and-join.md`).
const STEP_FROM: &str = "The name this step walks from: the start's, or one an earlier step binds; the one the step before it binds when left out. Two steps from one name branch, and a row is a match of every branch. Rows and an aggregate only.";

/// [`STEP_FROM`] for a join.
const STEP_JOIN: &str = "A name an earlier step binds: the step holds only where it reaches that name's record, closing a cycle. It binds no name and takes no `as`, `type` or `where`.";

/// [`STEP_FROM`] for an optional step.
const STEP_OPTIONAL: &str = "Keep the row where the step has no match: its name's `key`, `_type` and `_depth` are null. A step from or joining it says `optional` too, and they match whole or are null whole.";

/// [`STEP_FROM`] for an absent step.
const STEP_ABSENT: &str = "Keep the row only where the step has no match from its `from`; with `join`, only where no walk reaches the joined record. It binds no name and takes no `as`.";

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
        description: "Pin this session's reads to one position until cleared, so every read in a turn sees the same state: `as_of` pins that position, no argument pins the current head, `clear` removes the pin. A read's own `as_of` still wins; writes are never pinned. A read pinned to the head reads `now()` at the instant the pin was set; one pinned to a named position, at the time its event was written; `valid_at` comes first.".into(),
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
                    "rerank": rerank_schema(),
                    "params": {"type": "object",
                               "description": "The parameters of a view the pattern starts on, by name."},
                    "as": {"type": "string", "description": BINDING}}}),
            ),
            (
                "steps",
                json!({"type": "array", "items": {"type": "object", "properties": {
                    "relation": {"type": "string"}, "walk": {"enum": ["out", "in", "both"]},
                    "type": {"type": "string"}, "where": {"type": "string"},
                    "depth": {"type": "object", "properties": {
                        "min": {"type": "integer", "minimum": 0,
                                "description": "The fewest hops; 0 keeps the record the step starts from."},
                        "max": {"type": "integer", "minimum": 0,
                                "description": "The most hops, at most the lake's `max_depth`."}},
                        "required": ["min", "max"],
                        "description": "How many hops the step takes, `min` to `max`; one hop when left out. Give both members, as {\"min\": 0, \"max\": 6}. A graph or a path (`to`) whose steps all have `min` 0 is one walk over their relations, in any order; a step with `min` 1 or more makes it a chain, each step walking on from where the one before it stopped."},
                    "as": {"type": "string", "description": STEP_BINDING},
                    "from": {"type": "string", "description": STEP_FROM},
                    "join": {"type": "string", "description": STEP_JOIN},
                    "optional": {"type": "boolean", "description": STEP_OPTIONAL},
                    "absent": {"type": "boolean", "description": STEP_ABSENT}},
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
                "as": {"enum": ["rows", "graph", "aggregate", "degree", "components", "reach", "page_rank", "cheapest", "communities"],
                       "description": "`degree` answers rows of `type`, `key` and `degree` (links over the steps' relations), highest first; `components` answers rows of `component`, `size`, `type` and `key`, largest component first; with no start `type` it grows from every linked record, in every type. Both take steps without `depth`, `where` or `type`; `components` walks each step `both`. `reach` answers rows of `step`, `depth`, `type` and `key`: what each step reaches from what the step before it kept, dead ends included, each record at the first step that reaches it. `page_rank` answers rows of `type`, `key` and `page_rank` over the graph the start reaches, each step in its own direction, highest first, the same on every engine; like `components` it takes steps without `depth`, `where` or `type`, and may leave the start `type` out. `cheapest` needs `to` and one start: rows of `hop`, `type`, `key`, `relation`, `walk` and `cost` along the walk that costs least, a link costing its `weight` field, then the fewest hops. `communities` answers rows of `community`, `size`, `type` and `key`, as `components` does, each component cut into groups more linked within than between, the same on every engine; it takes the steps `components` takes."},
                "fields": {"type": "array", "items": {"type": "string"},
                           "description": "For `rows`: the paths a row holds; every bound name's fields when left out."},
                "group_by": {"type": "array", "items": {"type": "string"},
                             "description": "For `aggregate`: the paths a group is told apart by, as a row holds them (`customer.country`)."},
                "measures": {"type": "array", "items": {
                    "type": "object",
                    "properties": {"name": {"type": "string"},
                                   "func": {"enum": ["count", "sum", "min", "max", "avg"]},
                                   "field": {"type": "string"}},
                    "required": ["name", "func"], "additionalProperties": false},
                             "description": "For `aggregate`: what each group answers, named."},
                "weight": {"type": "string", "description": "For `cheapest`: the number or decimal field of every step's link type that a link costs."}}}),
            ),
            (
                "order_by",
                value_order_schema(json!({"type": "string"}), "the rows: a path they hold"),
            ),
            ("limit", json!({"type": "integer", "minimum": 1})),
            (
                "cursor",
                json!({"type": "string",
                       "description": "The `next_cursor` of an earlier answer, for the next page. Send it with the same pattern as the page it came from."}),
            ),
        ]);
        time_axes(&mut p);
        out.push(ToolSpec {
            name: QUERY.into(),
            description: "Walk declared relations from a start (a record, a filter or a search) and return rows or a graph. Covers neighbours, traversal, paths and subgraphs, whole-graph measures (degree centrality, connected components, PageRank and communities) and the cheapest walk. A graph whose steps all start at depth 0 is one walk over their relations, each in its own step's direction and depth; any other is a chain, as rows walk it; a graph cut by `limit` pages with `next_cursor`. Rows and an aggregate may branch and join: a step's `from` names an earlier name, `join` closes a cycle, `optional` binds null where nothing matches and `absent` keeps a row only where nothing does. A path (`to`) has one start and answers a graph, or with `cheapest` rows. A measure is computed whole for every page: read it in one page, with `limit` up to the lake's `max_graph_nodes`.".into(),
            input_schema: obj(p, &["start"]),
            output_schema: reply_schema(json!({"type": "object"})),
            door: Door::Query,
        });

        // The subset read beside `query`, never in its place: what it
        // cannot spell, `query` can.
        let mut p = props([
            (
                "statement",
                json!({"type": "string", "minLength": 1,
                       "description": "One statement of the openCypher subset, as in `MATCH (c:customer {name: $name})<-[:placed_by]-(o:order) RETURN o.id, o.total ORDER BY o.total DESC LIMIT 20`."}),
            ),
            (
                "params",
                json!({"type": "object",
                       "description": "The value of every `$name` the statement reads, by name; a value given and not read is refused."}),
            ),
            (
                "cursor",
                json!({"type": "string",
                       "description": "The `next_cursor` of the page before, given with the same statement and parameters, to read the next page."}),
            ),
        ]);
        time_axes(&mut p);
        out.push(ToolSpec {
            name: CYPHER.into(),
            description: "Read the lake with one statement of the openCypher subset: `MATCH`, `OPTIONAL MATCH`, `WHERE` and `RETURN`, with `ORDER BY` and `LIMIT`. It is translated into one pattern of `query` and read as `query` reads it, under the same rights; it never writes. The first node names its type, as in `(o:order)`, and a node may carry a map of field values, as in `(c:customer {name: $name})`. A relationship names a declared relation and is walked in its direction, `(o:order)-[:placed_by]->(c:customer)`, or against it, `(c)<-[:placed_by]-(o)`; `-[:r*1..3]->` walks it 1 to 3 hops, its upper bound written. A further part, after a comma or in a further `MATCH`, begins at a name bound before it. `WHERE` takes the lake's expression grammar with Cypher's operators (`=`, `<>`, `<`, `AND`, `OR`, `NOT`, `IN [...]`, `STARTS WITH`, `CONTAINS`, `IS NULL`), and `NOT (p)<-[:r]-(:t)` keeps a row only where no such link is. `RETURN` names fields of bound names (`o.total`), or `*` for every bound name. An aggregate (`count(*)`, `count(x)`, `sum(x.f)`, `min(x.f)`, `max(x.f)`, `avg(x.f)`, each named with `AS`) answers one group per value of the fields returned beside it, the groups in one fixed order by those fields, so `ORDER BY` beside an aggregate is refused: leave it out. Without one, `ORDER BY` names fields of bound names (`o.total DESC`), never an alias or an expression. `LIMIT` takes a whole number and caps the rows of a page; `SKIP` past 0 is refused: to read the next page, pass the answer's `next_cursor` as `cursor` with the same statement and parameters. An aggregate answers every group in one page with no cursor, ignoring `LIMIT`: for the largest few, read every group and pick them. Give values as `$name` with `params`. Refused, naming the construct at its character offset: `WITH`, `DISTINCT`, `UNWIND`, `UNION`, a named path, a function the grammar lacks, and every write (`CREATE`, `MERGE`, `SET`, `DELETE`), which goes through the typed write tools. The subset cannot spell a shortest or cheapest walk, a path to a record, degree, components, reach, PageRank or communities: `query` reads those. A statement that begins with `EXPLAIN` answers the `query` pattern it translates to and reads nothing.".into(),
            input_schema: obj(p, &["statement"]),
            output_schema: reply_schema(json!({"type": "object"})),
            door: Door::Cypher,
        });

        let mut p = props([
            ("type", names(|t| t.may.find)),
            (
                "mode",
                json!({"enum": ["lexical", "vector", "hybrid"],
                "description": "lexical ranks `text` in the searchable fields; vector ranks `vector` against `field`; hybrid fuses both and needs `text` and `field`, its `vector` optional."}),
            ),
            (
                "text",
                json!({"type": "string",
                "description": "lexical and hybrid: the words ranked; a word matches a field's word as written, its case and accents aside and stemmed where the type declares a `language`, so a misspelled word matches nothing: for a name half known, a find's `where` with starts_with or contains."}),
            ),
            (
                "fields",
                json!({"type": "array", "items": {"type": "string"}, "minItems": 1,
                "description": "lexical and hybrid: match the text only in these searchable fields."}),
            ),
            (
                "text_fields",
                json!({"type": "object", "minProperties": 1,
                "additionalProperties": {"type": "number", "exclusiveMinimum": 0, "maximum": 1000000},
                "description": "lexical and hybrid: a weight for a match of the text in each searchable field named, over the type's `search_weights` setting; a field it leaves out takes the setting's, else 1."}),
            ),
            (
                "field",
                json!({"type": "string",
                "description": "vector and hybrid, which both need it: the vector field searched, one the type declares."}),
            ),
            (
                "vector",
                json!({"type": "array", "items": {"type": "number"},
                "description": "vector, and hybrid where the caller has one: the query vector."}),
            ),
            (
                "model",
                json!({"type": "string",
                "description": "The model that made `vector`, named only beside it; over a field that is a model's embedding, it must be that model."}),
            ),
            (
                "weight",
                json!({"type": "number", "exclusiveMinimum": 0, "maximum": 1000000,
                "description": "The weight of `field`'s ranking against the text's (`text_weight`) or the fields' in `also`."}),
            ),
            (
                "text_weight",
                json!({"type": "number", "exclusiveMinimum": 0, "maximum": 1000000}),
            ),
            (
                "also",
                json!({"type": "array", "items": {"type": "object", "required": ["field", "vector"]},
                "description": "vector: further vector fields, each {field, vector, weight, model}, fused with `field` by weighted rank."}),
            ),
            ("k", json!({"type": "integer", "minimum": 1})),
            ("cursor", json!({"type": "string"})),
            ("where", json!({"type": "string"})),
            ("rerank", rerank_schema()),
            ("with_meta", json!({"type": "boolean"})),
            (
                "matches",
                json!({"type": "boolean",
                "description": format!("Answer on each hit the searched fields its text matched, each with the char spans of its first {} matched tokens and their count; needs a text.", crate::query::MATCH_SPANS)}),
            ),
        ]);
        time_axes(&mut p);
        out.push(ToolSpec {
            name: SEARCH.into(),
            description: "Ranked search over one type, lexical, by vector or both. Hits come best first, each with a score; page with `cursor`. With `rerank` the lake's reranker ranks the first `top` hits again and the answer ends at that window.".into(),
            input_schema: obj(p, &["type", "mode"]),
            output_schema: reply_schema(json!({"type": "object", "properties": {
                "hits": {"type": "array"}, "ranked_by": {"type": "string"},
                "truncated": {"type": "boolean", "description": "A reranked search only: the first stage holds more hits than the window, unranked and not answered."},
                "axes": {"type": "array"}, "next_cursor": {"type": "string"}}})),
            door: Door::Search,
        });
    }

    if lake.types.iter().any(|t| t.may.upsert || t.may.retire) || !lake.verbs.is_empty() {
        // Beside the record writes it may make, a writer may propose
        // declarations and migrations, which a reviewer holding the
        // `schema` grant accepts. Each kind of write has its own members,
        // so a caller sees the shape the store reads.
        let opts = json!({"type": "object", "properties": {
            "expected_version": {"type": "integer", "minimum": 0},
            "idempotency_key": {"type": "string"}}});
        let kind = |write: &str, members: Value, required: &[&str]| {
            let mut members = members;
            members["write"] = json!({"const": write});
            let mut required = required.to_vec();
            required.insert(0, "write");
            json!({"type": "object", "properties": members, "required": required})
        };
        let verbs = one_of(lake.verbs.iter().map(|v| v.name.to_string()));
        let write = json!({"oneOf": [
            kind("upsert", json!({"type": names(|t| t.may.upsert), "key": {"type": "string"},
                                  "patch": {"type": "object",
                                            "description": "The fields to write, as `<type>_upsert` takes them under `fields`."},
                                  "opts": opts}), &["type", "key", "patch"]),
            kind("retire", json!({"type": names(|t| t.may.retire), "key": {"type": "string"},
                                  "opts": opts}), &["type", "key"]),
            kind("records", json!({"type": names(|t| t.may.find && (t.may.upsert || t.may.retire)),
                                   "where": {"type": "string", "description": "The condition, as `<type>_change` takes it."},
                                   "to": {"description": "`{\"set\": {fields}}` or `\"retire\"`, as `<type>_change` takes `set` and `retire`."}}),
                 &["type", "where", "to"]),
            kind("call", json!({"verb": verbs, "key": {"type": "string",
                                "description": "The record's key; left out for a verb called on its type."},
                                "payload": {"type": "object"}, "opts": opts}), &["verb", "payload"]),
            kind("declare", json!({"declarations": {"type": "array", "items": {"type": "object"}}}),
                 &["declarations"]),
            kind("migrate", json!({"migration": {"type": "object"}}), &["migration"]),
            kind("migrate_value_type", json!({"to": {"type": "object",
                 "description": "The value type's new declaration."},
                 "renames": {"type": "array", "description": "Old value to new value.",
                             "items": {"type": "array", "items": {"type": "string"},
                                       "minItems": 2, "maxItems": 2}}}), &["to"]),
        ]});
        out.push(ToolSpec {
            name: PROPOSE.into(),
            description: "Record a write as a proposal for review instead of applying it: a \
                record write, a verb call, declarations (as apply takes them) or a migration \
                (as migrate takes it), of a type or of a value type that is to accept less or \
                rename values. A record write or a verb call needs the grants it would need \
                applied: a proposal waits for review, it does not lift a grant. Declarations \
                and a migration need no `schema` grant to propose; a reviewer holding it \
                accepts them. Declarations all in force, and a migration to the type in \
                force that renames and fills nothing, are not filed: they are answered \
                applied at the head with `declared` naming what stands, and nothing is \
                appended."
                .into(),
            input_schema: obj(
                props([("write", write), ("why", json!({"type": "string"}))]),
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
                json!({"enum": ["upsert", "retire", "call", "declare", "migrate", "migrate_value_type", "compute", "records"]}),
            ),
            (
                "order_by",
                value_order_schema(
                    one_of(["proposed_at", "position"].iter().map(|f| f.to_string())),
                    "the proposals: a member",
                ),
            ),
            ("limit", json!({"type": "integer", "minimum": 1})),
            ("cursor", json!({"type": "string"})),
        ]);
        time_axes(&mut p);
        out.push(ToolSpec {
            name: PROPOSALS.into(),
            description: "List proposals waiting for review, or decided ones; newest first unless ordered otherwise. Each shows its write as far as you may see it, and `more: true` where it holds more than you are shown.".into(),
            input_schema: obj(p, &[]),
            output_schema: reply_schema(json!({"type": "array"})),
            door: Door::Proposals,
        });
        out.push(ToolSpec {
            name: ACCEPT.into(),
            description: "Accept a proposal: its write is applied. For a change to the ontology, `plan_id` accepts it only if the plan of its change made now has that id: call the change's own tool with `plan: true` for it. A change to records is accepted only with `plan_id`, the `report_id` of the plan `<type>_change` answers now, and is refused `plan_stale` otherwise, the proposal left pending. The plan `proposals` lists was made when it was proposed, and its id is not one to send.".into(),
            input_schema: obj(
                props([
                    ("proposal", json!({"type": "string"})),
                    ("plan_id", json!({"type": "string"})),
                ]),
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
        let (input_schema, output_schema) =
            planned(obj(props([("type", json!({"type": "object"}))]), &["type"]));
        out.push(ToolSpec {
            name: DECLARE_TYPE.into(),
            description: format!(
                "Declare a new type, or revise one without reshaping its records.{PLAN_FIRST}"
            ),
            input_schema,
            output_schema,
            door: Door::DeclareType,
        });
        let (input_schema, output_schema) = planned(obj(
            props([
                ("type", names(|_| true)),
                ("field", json!({"type": "object"})),
            ]),
            &["type", "field"],
        ));
        out.push(ToolSpec {
            name: ADD_FIELD.into(),
            description: format!("Add one field to a type.{PLAN_FIRST}"),
            input_schema,
            output_schema,
            door: Door::AddField,
        });
        let (input_schema, output_schema) = planned(obj(
            props([
                ("function", json!({"type": "string"})),
                ("type", names(|_| true)),
            ]),
            &[],
        ));
        out.push(ToolSpec {
            name: RECOMPUTE.into(),
            description: format!("Compute again what a declaration in force computes: name a `function`, or a `type` with a field computed outside the lake. Retries a failed call and makes a field that waits.{PLAN_FIRST}"),
            input_schema,
            output_schema,
            door: Door::Recompute,
        });
        // Old name and new name, for a field or a relation.
        let pairs = json!({"type": "array", "items": {
            "type": "array", "items": {"type": "string"}, "minItems": 2, "maxItems": 2}});
        out.push(ToolSpec {
            name: MIGRATE.into(),
            description: "Change a type's shape (`to`) or a value type (`value_type`), one of the two. `renames` pairs old with new: a type's field names, or a text value type's values, which every stored record holding the old value takes, retired ones included; a condition that names an old value is yours to change, and the report lists those it can read (`conditions`). Without `report_id` it only reports what would convert, what would no longer fit and what it drops (`dropped`, each with the records that hold a value); with the id of that report it applies the migration, which discards the values `dropped` counts. `lossless` speaks of what the migration keeps, not of what it drops: sum `dropped`'s counts to know what goes.".into(),
            input_schema: obj(
                props([
                    ("to", json!({"type": "object"})),
                    ("value_type", json!({"type": "object"})),
                    ("renames", pairs.clone()),
                    ("relation_renames", pairs),
                    ("fill", json!({"type": "array", "items": {"type": "string"},
                        "description": "Fields of `to` whose `initial` each live record that holds no value takes; the report counts them (`filled`)."})),
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
