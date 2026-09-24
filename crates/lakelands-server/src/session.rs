//! One MCP session: one lake, one principal, the tools that principal
//! may use, and the session's `as_of` pin.
//!
//! [`Session::handle`] takes one JSON-RPC message and answers the
//! messages to send back: the response to a request first, then any
//! notification it caused. Both transports use it. stdio writes every
//! message; streamable HTTP answers the response as JSON today, and the
//! same list is what an SSE stream will carry when notifications go over
//! HTTP too.
//!
//! The tools are exactly [`tools`] over what `describe` shows the
//! session's principal, generated again for every `tools/list` and every
//! `tools/call`, so a declaration is visible on the next call. A call is
//! mapped back to its door through the [`ToolSpec`] it names; no tool
//! name is written in this crate.

use std::sync::Arc;

use lakelands_api::error::{Error, Refusal};
use lakelands_api::id::{FieldName, Position, ProposalId, RecordKey, TypeName};
use lakelands_api::ontology::{FieldDecl, TypeDecl};
use lakelands_api::query::{Pattern, Search};
use lakelands_api::read::{Find, Get, ReadAt};
use lakelands_api::record::Patch;
use lakelands_api::tools::{Door, ToolSpec, tools};
use lakelands_api::write::{Declaration, Migration, ProposalFilter, ProposedWrite, WriteOpts};
use lakelands_api::{Catalog, LakeStore, Principal, WriteContext};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

use crate::say::Failure;

/// The protocol versions this server speaks, newest first.
pub const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// JSON-RPC error codes the server answers with.
pub(crate) mod code {
    /// The message is not JSON.
    pub const PARSE: i64 = -32700;
    /// The message is not a JSON-RPC request.
    pub const INVALID_REQUEST: i64 = -32600;
    /// No such method.
    pub const METHOD_NOT_FOUND: i64 = -32601;
    /// The parameters do not fit the method, or the tool is unknown.
    pub const INVALID_PARAMS: i64 = -32602;
    /// The server failed.
    pub const INTERNAL: i64 = -32603;
}

/// A JSON-RPC error response.
pub(crate) fn error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

fn result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// Why a tool call failed: the contract's closed set, or arguments that
/// do not fit the tool.
enum Fault {
    Contract(Error),
    Arguments(String, String),
}

impl From<Error> for Fault {
    fn from(e: Error) -> Fault {
        Fault::Contract(e)
    }
}

fn args<T: DeserializeOwned>(tool: &str, a: &Map<String, Value>) -> Result<T, Fault> {
    serde_json::from_value(Value::Object(a.clone())).map_err(|e| {
        Fault::Arguments(
            format!("the arguments do not fit `{tool}`: {e}"),
            "correct them against the tool's input schema and call it again".into(),
        )
    })
}

fn to_json<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).expect("contract types serialise")
}

/// One MCP session over one lake.
pub struct Session {
    store: Arc<dyn LakeStore>,
    catalog: Arc<dyn Catalog>,
    who: Principal,
    pin: Option<Position>,
    /// Whether the transport can carry `notifications/tools/list_changed`.
    notifies: bool,
}

impl Session {
    /// A session for `who` over one lake. `notifies` says whether the
    /// transport can carry server notifications (stdio can; HTTP cannot
    /// until it streams).
    pub fn new(store: Arc<dyn LakeStore>, who: Principal, notifies: bool) -> Session {
        let catalog = store.catalog(WriteContext::default());
        Session {
            store,
            catalog,
            who,
            pin: None,
            notifies,
        }
    }

    /// The position reads are pinned to, if any.
    pub fn pinned(&self) -> Option<Position> {
        self.pin
    }

    /// Answer one JSON-RPC message: the response to a request, then any
    /// notification it caused. A notification or a response from the
    /// client answers nothing.
    pub async fn handle(&mut self, msg: Value) -> Vec<Value> {
        let Some(obj) = msg.as_object() else {
            return vec![error(
                Value::Null,
                code::INVALID_REQUEST,
                "a message must be a JSON object",
            )];
        };
        let id = obj.get("id").cloned();
        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            // A response to something the server never asked: nothing to say.
            return match id {
                Some(_) if obj.contains_key("result") || obj.contains_key("error") => Vec::new(),
                _ => vec![error(
                    Value::Null,
                    code::INVALID_REQUEST,
                    "a request must name a method",
                )],
            };
        };
        if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return vec![error(
                id.unwrap_or(Value::Null),
                code::INVALID_REQUEST,
                "only JSON-RPC 2.0 is spoken",
            )];
        }
        let Some(id) = id else {
            // Notifications (`notifications/initialized`, a cancellation)
            // need no answer.
            return Vec::new();
        };
        let empty = Map::new();
        let params = obj
            .get("params")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        match method {
            "initialize" => vec![result(id, self.initialize(params))],
            "ping" => vec![result(id, json!({}))],
            "tools/list" => match self.tools().await {
                Ok(t) => vec![result(
                    id,
                    json!({"tools": t.iter().map(listing).collect::<Vec<_>>()}),
                )],
                Err(e) => vec![error(id, code::INTERNAL, e)],
            },
            "tools/call" => self.call(id, params).await,
            other => vec![error(
                id,
                code::METHOD_NOT_FOUND,
                format!("this server has no method `{other}`"),
            )],
        }
    }

    /// The answer to `initialize`: the protocol version (the client's if
    /// spoken here, else the newest), and what the server offers.
    pub fn initialize(&self, params: &Map<String, Value>) -> Value {
        let asked = params.get("protocolVersion").and_then(Value::as_str);
        let version = asked
            .filter(|v| PROTOCOL_VERSIONS.contains(v))
            .unwrap_or(PROTOCOL_VERSIONS[0]);
        json!({
            "protocolVersion": version,
            "capabilities": {"tools": {"listChanged": self.notifies}},
            "serverInfo": {"name": "lakelands", "version": env!("CARGO_PKG_VERSION")},
            "instructions": format!(
                "This session serves one lake, `{}`. Every answer carries the position it was read or written at. Call `describe` to see what the lake holds.",
                self.store.lake()
            ),
        })
    }

    async fn tools(&self) -> Result<Vec<ToolSpec>, String> {
        let d = self
            .catalog
            .describe(&self.who, ReadAt::default())
            .await
            .map_err(|e| Failure::from(e).to_string())?;
        tools(&d.data)
            .map_err(|p| format!("the lake's names cannot all become tools: {}", to_json(&p)))
    }

    async fn call(&mut self, id: Value, params: &Map<String, Value>) -> Vec<Value> {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return vec![error(
                id,
                code::INVALID_PARAMS,
                "`tools/call` needs the tool's `name`",
            )];
        };
        let empty = Map::new();
        let arguments = match params.get("arguments") {
            None | Some(Value::Null) => &empty,
            Some(Value::Object(m)) => m,
            Some(_) => {
                return vec![error(
                    id,
                    code::INVALID_PARAMS,
                    "`arguments` must be an object",
                )];
            }
        };
        let spec = match self.tools().await {
            Ok(t) => t.into_iter().find(|t| t.name == name),
            Err(e) => return vec![error(id, code::INTERNAL, e)],
        };
        let Some(spec) = spec else {
            return vec![error(
                id,
                code::INVALID_PARAMS,
                format!("there is no tool `{name}` for you in this lake"),
            )];
        };
        let reshapes = matches!(spec.door, Door::DeclareType | Door::AddField)
            || (spec.door == Door::Migrate && arguments.contains_key("report_id"));
        let outcome = match unknown_argument(&spec, arguments) {
            Some(fault) => Err(fault),
            None => self.door(&spec, arguments.clone()).await,
        };
        let answer = match outcome {
            Ok(v) => {
                let position = v.get("position").cloned().unwrap_or(Value::Null);
                let text = format!(
                    "position {position}\n{}",
                    serde_json::to_string_pretty(&v).expect("JSON prints")
                );
                json!({"content": [{"type": "text", "text": text}], "structuredContent": v, "isError": false})
            }
            Err(Fault::Contract(e)) => {
                let text = Failure::from(e.clone()).to_string();
                json!({"content": [{"type": "text", "text": text}, {"type": "text", "text": to_json(&e).to_string()}], "isError": true})
            }
            Err(Fault::Arguments(what, todo)) => {
                let text = Failure::new(what, todo).to_string();
                json!({"content": [{"type": "text", "text": text}], "isError": true})
            }
        };
        let ok = answer["isError"] == false;
        let mut out = vec![result(id, answer)];
        if ok && reshapes && self.notifies {
            out.push(json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}));
        }
        out
    }

    /// Pin reads: a read tool that names no `as_of` is read at the pin.
    fn pinned_args(&self, mut a: Map<String, Value>) -> Map<String, Value> {
        if let (Some(p), false) = (self.pin, a.contains_key("cursor")) {
            a.entry("as_of").or_insert(json!(p.0));
        }
        a
    }

    async fn door(&mut self, spec: &ToolSpec, a: Map<String, Value>) -> Result<Value, Fault> {
        let (c, who, tool) = (self.catalog.clone(), self.who.clone(), spec.name.as_str());
        #[derive(Deserialize)]
        struct Write {
            key: RecordKey,
            #[serde(default)]
            fields: Patch,
            #[serde(default)]
            payload: Patch,
            #[serde(flatten)]
            opts: WriteOpts,
        }
        Ok(match &spec.door {
            Door::Find { ty } => to_json(
                &c.find(&who, ty, args::<Find>(tool, &self.pinned_args(a))?)
                    .await?,
            ),
            Door::Get { ty } => to_json(
                &c.get(&who, ty, args::<Get>(tool, &self.pinned_args(a))?)
                    .await?,
            ),
            Door::Upsert { ty } => {
                let w: Write = args(tool, &a)?;
                to_json(&c.upsert(&who, ty, &w.key, w.fields, w.opts).await?)
            }
            Door::Retire { ty } => {
                let w: Write = args(tool, &a)?;
                to_json(&c.retire(&who, ty, &w.key, w.opts).await?)
            }
            Door::Call { verb } => {
                let w: Write = args(tool, &a)?;
                to_json(&c.call(&who, verb, &w.key, w.payload, w.opts).await?)
            }
            Door::Describe => {
                #[derive(Deserialize)]
                struct D {
                    #[serde(default, rename = "type")]
                    ty: Option<TypeName>,
                    #[serde(flatten)]
                    at: ReadAt,
                }
                let d: D = args(tool, &self.pinned_args(a))?;
                match d.ty {
                    Some(ty) => to_json(&c.describe_type(&who, &ty, d.at).await?),
                    None => to_json(&c.describe(&who, d.at).await?),
                }
            }
            Door::Pin => {
                #[derive(Deserialize)]
                struct P {
                    #[serde(default)]
                    as_of: Option<Position>,
                    #[serde(default)]
                    clear: bool,
                }
                let p: P = args(tool, &a)?;
                let head = self.store.head().await?;
                self.pin = match (p.clear, p.as_of) {
                    (true, _) => None,
                    (false, Some(at)) if at > head => {
                        return Err(Fault::Contract(Error::Refused {
                            field: None,
                            reason: Refusal::OutOfRange { head },
                        }));
                    }
                    (false, at) => Some(at.unwrap_or(head)),
                };
                json!({"position": head.0, "pinned": self.pin.map(|p| p.0)})
            }
            Door::Query => to_json(
                &c.query(&who, args::<Pattern>(tool, &self.pinned_args(a))?)
                    .await?,
            ),
            Door::Search => to_json(
                &c.search(&who, args::<Search>(tool, &self.pinned_args(a))?)
                    .await?,
            ),
            Door::Propose => {
                #[derive(Deserialize)]
                struct P {
                    write: ProposedWrite,
                    why: String,
                }
                let p: P = args(tool, &a)?;
                to_json(&c.propose(&who, p.write, p.why).await?)
            }
            Door::Proposals => to_json(
                &c.proposals(&who, args::<ProposalFilter>(tool, &self.pinned_args(a))?)
                    .await?,
            ),
            Door::Accept | Door::Reject => {
                #[derive(Deserialize)]
                struct D {
                    proposal: ProposalId,
                    #[serde(default)]
                    reason: String,
                }
                let d: D = args(tool, &a)?;
                if spec.door == Door::Accept {
                    to_json(&c.accept(&who, &d.proposal).await?)
                } else {
                    to_json(&c.reject(&who, &d.proposal, d.reason).await?)
                }
            }
            Door::DeclareType => {
                #[derive(Deserialize)]
                struct D {
                    #[serde(rename = "type")]
                    ty: TypeDecl,
                }
                let d: D = args(tool, &a)?;
                to_json(&c.declare(&who, Declaration::Type(d.ty)).await?)
            }
            Door::AddField => {
                #[derive(Deserialize)]
                struct D {
                    #[serde(rename = "type")]
                    ty: TypeName,
                    field: FieldDecl,
                }
                let d: D = args(tool, &a)?;
                to_json(&c.add_field(&who, &d.ty, d.field).await?)
            }
            Door::Migrate => {
                #[derive(Deserialize)]
                struct M {
                    to: TypeDecl,
                    #[serde(default)]
                    renames: Vec<(FieldName, FieldName)>,
                    #[serde(default)]
                    report_id: Option<String>,
                }
                let m: M = args(tool, &a)?;
                let migration = Migration {
                    to: m.to,
                    renames: m.renames,
                };
                match m.report_id {
                    Some(r) => to_json(&c.migrate_apply(&who, migration, &r).await?),
                    None => to_json(&c.migrate(&who, migration).await?),
                }
            }
        })
    }
}

/// Refuse an argument the tool's input schema does not list, before
/// anything runs: a misspelled `expected_version` must not turn a
/// compare-and-set into an unconditional write.
fn unknown_argument(spec: &ToolSpec, arguments: &Map<String, Value>) -> Option<Fault> {
    let listed = spec.input_schema["properties"].as_object();
    let key = arguments
        .keys()
        .find(|k| !listed.is_some_and(|p| p.contains_key(k.as_str())))?;
    Some(Fault::Arguments(
        format!(
            "`{}` takes no argument `{key}`; nothing was done",
            spec.name
        ),
        "use only the arguments the tool's input schema lists, then call it again".into(),
    ))
}

/// A tool as `tools/list` lists it.
fn listing(t: &ToolSpec) -> Value {
    json!({
        "name": t.name,
        "description": t.description,
        "inputSchema": t.input_schema,
        "outputSchema": t.output_schema,
    })
}
