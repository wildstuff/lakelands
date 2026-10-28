//! One MCP session: one lake, one principal, the tools that principal
//! may use, and the session's `as_of` pin.
//!
//! [`Session::handle`] takes one JSON-RPC message and answers the
//! messages to send back. Both transports use it. What the session is
//! told of without asking comes from its pump ([`crate::watch`]), which
//! stdio interleaves with the answers and streamable HTTP sends on the
//! session's event stream.
//!
//! The tools are exactly [`tools`] over what `describe` shows the
//! session's principal, generated again for every `tools/list` and every
//! `tools/call`, so a declaration is visible on the next call. A call is
//! mapped back to its door through the [`ToolSpec`] it names; no tool
//! name is written in this crate.

use std::sync::Arc;

use lakelands_api::error::{Case, Error, Place, Refusal, Said, Situation};
use lakelands_api::id::Position;
use lakelands_api::read::ReadAt;
use lakelands_api::tools::{Door, ToolSpec, tools};
use lakelands_api::{Catalog, LakeStore, Principal, Timestamp, WriteContext};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::doors::{self, Fault, args, to_json, unknown_argument};
use crate::say::Failure;
use crate::watch::Watch;

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

/// The instant a pin of the head is set at.
fn wall_clock() -> Timestamp {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
    Timestamp(now.map_or(0, |d| d.as_micros() as i64))
}

fn result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// One MCP session over one lake.
pub struct Session {
    store: Arc<dyn LakeStore>,
    catalog: Arc<dyn Catalog>,
    who: Principal,
    /// The pinned position, and the instant a pin of the head was set at,
    /// the clock its reads read (052).
    pin: Option<(Position, Option<Timestamp>)>,
    /// Whether the transport can carry notifications.
    notifies: bool,
    /// What the session is told of, shared with its pump.
    watch: Arc<Watch>,
}

impl Session {
    /// A session for `who` over one lake. `notifies` says whether the
    /// transport can carry server notifications: stdio and streamable HTTP
    /// both do.
    pub fn new(store: Arc<dyn LakeStore>, who: Principal, notifies: bool) -> Session {
        let catalog = store.catalog(WriteContext::default());
        Session {
            store,
            catalog,
            who,
            pin: None,
            notifies,
            watch: Watch::new(),
        }
    }

    /// The session's watch, for the pump its transport runs
    /// ([`crate::watch::pump`]).
    pub fn watch(&self) -> Arc<Watch> {
        self.watch.clone()
    }

    /// The pump that tells this session of changes, sending into `queue`
    /// until it is dropped; `pause` is how long a caught-up pump waits for
    /// the next append, `floor` how often at most it reads the log.
    pub fn pump(
        &self,
        queue: tokio::sync::mpsc::Sender<crate::watch::Queued>,
        pause: std::time::Duration,
        floor: std::time::Duration,
    ) -> impl std::future::Future<Output = ()> + Send + 'static {
        crate::watch::pump(
            self.store.clone(),
            self.who.clone(),
            self.watch.clone(),
            queue,
            (pause, floor),
        )
    }

    /// The position reads are pinned to, if any.
    pub fn pinned(&self) -> Option<Position> {
        self.pin.map(|(p, _)| p)
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
            "tools/list" => {
                // Counted before the tools are read: a change queued
                // meanwhile stays owed to the next stream.
                let made = self.watch.tools_made();
                match self.tools().await {
                    Ok(t) => {
                        self.watch.listed(made);
                        vec![result(
                            id,
                            json!({"tools": t.iter().map(listing).collect::<Vec<_>>()}),
                        )]
                    }
                    Err(e) => vec![error(id, code::INTERNAL, e)],
                }
            }
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
            "capabilities": {
                "tools": {"listChanged": self.notifies},
                "experimental": {"lakelands/changes": {}},
            },
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
        tools(&d.data).map_err(|problems| {
            let names = Error::unavailable(Situation::ToolNames { problems });
            Failure::from(names).to_string()
        })
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
        let outcome = match unknown_argument(&spec, arguments) {
            Some(fault) => Err(fault),
            None => self.door(&spec, arguments.clone()).await,
        };
        let answer = tool_answer(outcome);
        // A declaration's `tools/list_changed` is the pump's to send, to
        // every session, when the feed shows it.
        vec![result(id, answer)]
    }

    /// Pin reads: a read tool that names no `as_of` is read at the pin,
    /// and under its clock if it pinned the head.
    fn pinned_args(&self, mut a: Map<String, Value>) -> (Map<String, Value>, Option<Timestamp>) {
        let own = a.contains_key("cursor") || a.contains_key("as_of");
        match self.pin {
            Some((p, clock)) if !own => {
                a.insert("as_of".into(), json!(p.0));
                (a, clock)
            }
            _ => (a, None),
        }
    }

    async fn door(&mut self, spec: &ToolSpec, a: Map<String, Value>) -> Result<Value, Fault> {
        if matches!(spec.door, Door::Subscribe | Door::Unsubscribe) {
            return self.subscription(&spec.door, &spec.name, a).await;
        }
        if spec.door == Door::Cypher {
            // The tool reads: a statement that writes is refused by its
            // first updating clause before it is translated.
            let statement = a.get("statement").and_then(Value::as_str);
            let params = a.get("params").cloned().map(serde_json::from_value);
            let params = params.and_then(Result::ok).unwrap_or_default();
            let wrote = statement.and_then(|s| lakelands_cypher::writes(s, &params));
            if let Some((construct, offset)) = wrote {
                let case = Case::CypherToolWrites { construct, offset };
                return Err(Error::invalid(case).into());
            }
        }
        if spec.door != Door::Pin {
            let a = match spec.door {
                Door::Find { .. }
                | Door::Get { .. }
                | Door::Describe
                | Door::Query
                | Door::Cypher
                | Door::Search
                | Door::Proposals => self.pinned_args(a),
                _ => (a, None),
            };
            return doors::run(self.catalog.as_ref(), &self.who, &spec.door, &spec.name, a).await;
        }
        #[derive(Deserialize)]
        struct P {
            #[serde(default)]
            as_of: Option<Position>,
            #[serde(default)]
            clear: bool,
        }
        let p: P = args(&spec.name, &a)?;
        let head = self.store.head().await?;
        self.pin = match (p.clear, p.as_of) {
            (true, _) => None,
            (false, Some(at)) if at > head => {
                return Err(Fault::Contract(Error::Refused {
                    field: None,
                    reason: Refusal::OutOfRange { head },
                }));
            }
            (false, Some(at)) => Some((at, None)),
            (false, None) => Some((head, Some(wall_clock()))),
        };
        Ok(json!({"position": head.0, "pinned": self.pinned().map(|p| p.0)}))
    }
}

impl Session {
    /// `subscribe` and `unsubscribe`.
    async fn subscription(
        &mut self,
        door: &Door,
        label: &str,
        a: Map<String, Value>,
    ) -> Result<Value, Fault> {
        let head = self.store.head().await?;
        if *door == Door::Unsubscribe {
            let _: Map<String, Value> = args(label, &a)?;
            let next = self.watch.unsubscribe();
            return Ok(json!({"position": head.0, "subscribed": false, "next": next.0}));
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct S {
            #[serde(default)]
            from: Option<Position>,
            #[serde(default)]
            kinds: Option<Vec<String>>,
            #[serde(default)]
            types: Option<Vec<lakelands_api::id::TypeName>>,
            #[serde(default)]
            keys: Option<Vec<lakelands_api::id::RecordKey>>,
        }
        let s: S = args(label, &a)?;
        let from = s.from.unwrap_or(head.next());
        if from > head.next() {
            return Err(Fault::Contract(Error::Refused {
                field: Some("from".into()),
                reason: Refusal::OutOfRange { head },
            }));
        }
        if s.keys.iter().flatten().any(|k| k.as_str().is_empty()) {
            return Err(Fault::Contract(Error::Refused {
                field: Some("keys".into()),
                reason: Refusal::Invalid(Box::new(Case::FilterKeyEmpty.into())),
            }));
        }
        let kinds = crate::watch::kinds(s.kinds).map_err(|bad| {
            let case = Case::NotOneOf {
                value: Value::String(bad),
                allowed: lakelands_api::tools::CHANGE_KINDS.iter().collect(),
            };
            let at = Place::Member {
                path: "kinds".into(),
            };
            let tool = label.into();
            Fault::arguments(
                Said::from(case)
                    .within(at)
                    .within(Place::Arguments { tool }),
            )
        })?;
        self.watch.subscribe(from, kinds, s.types, s.keys);
        Ok(json!({"position": head.0, "subscribed": true, "from": from.0}))
    }
}

/// What a tool call answers: the door's JSON, or a fault as its two
/// sentences, and a contract fault with its JSON beside them.
fn tool_answer(outcome: Result<Value, Fault>) -> Value {
    match outcome {
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
        Err(Fault::Arguments(said)) => {
            let text = Failure::of(&said).to_string();
            let e = Error::invalid(*said);
            json!({"content": [{"type": "text", "text": text}, {"type": "text", "text": to_json(&e).to_string()}], "isError": true})
        }
    }
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

#[cfg(test)]
mod tests {
    use lakelands_api::error::{Error, Refusal};

    use super::*;

    /// A reaction past a trigger's `max_depth` is refused by the store,
    /// never by a door caller, whose writes carry no cause; when a tool
    /// answers it, the trigger is named in its sentence and its JSON.
    #[test]
    fn a_too_deep_refusal_names_the_trigger_in_a_tool_answer() {
        let answer = tool_answer(Err(Fault::Contract(Error::Refused {
            field: Some("once".into()),
            reason: Refusal::TooDeep { max: 1 },
        })));
        assert_eq!(answer["isError"], true, "{answer}");
        let text = answer["content"][0]["text"].as_str().unwrap_or_default();
        assert!(text.contains("`once`") && text.contains('1'), "{text}");
        let error: Value =
            serde_json::from_str(answer["content"][1]["text"].as_str().unwrap_or("null")).unwrap();
        assert_eq!(error["field"], "once", "{error}");
        assert_eq!(error["reason"]["refusal"], "too_deep", "{error}");
        assert_eq!(error["reason"]["max"], 1, "{error}");
    }
}
