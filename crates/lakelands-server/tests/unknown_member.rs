//! A declaration member Lakelands does not know, at any depth, is refused
//! at the JSON doors, over REST and over MCP, on the mock and on the store
//! over both engines: the refusal names the member, where it sits inside
//! the declaration, and where the declaration sits among the arguments,
//! and the lake does not move.
//!
//! `schema/declare` and `schema/apply` are REST's alone; `declare_type`,
//! `add_field`, `migrate` and `propose` are asked on both.

mod common;

use common::{BACKENDS, Backend};

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use lakelands_api::LakeStore;
use lakelands_server::{HttpConfig, serve_http, serve_io};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// A wait for an answer that should come: past it, a hang is a failure.
const WAIT: Duration = Duration::from_secs(60);

/// A field of the type `memo` with `format`, which no field has.
fn field_with_format() -> Value {
    json!({"name": "body", "type": {"kind": "text"}, "format": "iban"})
}

fn memo() -> Value {
    json!({"name": "memo", "fields": [field_with_format()]})
}

fn plain(name: &str) -> Value {
    json!({"declare": "type", "name": name, "fields": [{"name": "body", "type": {"kind": "text"}}]})
}

/// One door asked with an unknown member: its REST route, its MCP tool
/// (none for REST's own routes), its arguments, and what the refusal
/// must name — the member, then the paths outside and inside the
/// declaration.
struct Case {
    route: &'static str,
    tool: Option<&'static str>,
    args: Value,
    names: &'static [&'static str],
}

fn cases() -> Vec<Case> {
    let mut memo_declared = memo();
    memo_declared["declare"] = json!("type");
    vec![
        Case {
            route: "schema/declare",
            tool: None,
            args: json!({"declaration": memo_declared.clone()}),
            names: &["format", "declaration", "fields[0]", "memo"],
        },
        Case {
            route: "schema/apply",
            tool: None,
            args: json!({"declarations": [plain("note"), plain("letter"), memo_declared.clone()]}),
            names: &["format", "declarations[2]", "fields[0]", "memo"],
        },
        Case {
            route: "schema/declare_type",
            tool: Some("declare_type"),
            args: json!({"type": memo()}),
            names: &["format", "type", "fields[0]"],
        },
        Case {
            route: "schema/add_field",
            tool: Some("add_field"),
            args: json!({"type": "invoice", "field": field_with_format()}),
            names: &["format", "field"],
        },
        Case {
            route: "schema/migrate",
            tool: Some("migrate"),
            args: json!({"to": {
                "name": "invoice",
                "fields": [{"name": "status", "type": {"kind": "text"}}],
                "relations": [{"name": "billed_to", "target": "partner", "colour": "red"}],
            }}),
            names: &["colour", "to.relations[0]"],
        },
        Case {
            route: "proposals",
            tool: Some("propose"),
            args: json!({
                "write": {"write": "declare", "declarations": [plain("note"), memo_declared]},
                "why": "a memo type",
            }),
            names: &["format", "write", "fields[0]", "memo"],
        },
    ]
}

/// `text` names everything `names` lists, or says what it lacks.
fn names_all(text: &str, names: &[&str]) -> Result<(), String> {
    match names.iter().find(|n| !text.contains(*n)) {
        Some(n) => Err(format!("`{n}` is not named in: {text}")),
        None => Ok(()),
    }
}

// ------------------------------------------------------------------ REST

async fn serve(store: Arc<dyn LakeStore>) -> SocketAddr {
    let open = Arc::new(
        move |_: &lakelands_api::LakeId| -> lakelands_api::Result<Arc<dyn LakeStore>> {
            Ok(store.clone())
        },
    );
    let config = HttpConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        allowed_origins: Vec::new(),
        session_idle_secs: 1800,
        feed_poll_millis: 20,
    };
    let server = serve_http(config, common::resolver(), open).await.unwrap();
    let addr = server.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = server.run().await;
    });
    addr
}

/// POST `body` to the lake's `route` as the owner: the status and the body.
async fn post(addr: SocketAddr, route: &str, body: &Value) -> (u16, Value) {
    let body = body.to_string();
    let request = format!(
        "POST /lakes/finance/{route} HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n\
         authorization: Bearer k-owner\r\ncontent-type: application/json\r\n\
         content-length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    tokio::time::timeout(WAIT, stream.read_to_end(&mut raw))
        .await
        .expect("an answer within the wait")
        .unwrap();
    let raw = String::from_utf8(raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, serde_json::from_str(body).unwrap())
}

#[tokio::test]
async fn rest_refuses_a_declaration_member_it_does_not_know_naming_it() {
    for backend in BACKENDS {
        rest_on(backend).await;
    }
}

async fn rest_on(backend: Backend) {
    let store = common::lake_on(backend).await;
    let head = store.head().await.unwrap();
    let addr = serve(store.clone()).await;
    for case in cases() {
        let (status, answer) = post(addr, case.route, &case.args).await;
        let at = format!("{backend:?}, POST {}", case.route);
        assert_eq!(status, 400, "{at}: {answer}");
        assert_eq!(answer["error"], "refused", "{at}: {answer}");
        assert_eq!(answer["reason"]["refusal"], "invalid", "{at}: {answer}");
        let what = answer["what"].as_str().unwrap_or_default();
        if let Err(e) = names_all(what, case.names) {
            panic!("{at}: {e}");
        }
        assert_eq!(store.head().await.unwrap(), head, "{at}: the lake moved");
    }
}

// ------------------------------------------------------------------- MCP

#[tokio::test]
async fn mcp_refuses_a_declaration_member_it_does_not_know_naming_it() {
    for backend in BACKENDS {
        mcp_on(backend).await;
    }
}

async fn mcp_on(backend: Backend) {
    let store = common::lake_on(backend).await;
    let head = store.head().await.unwrap();
    let (client, server) = tokio::io::duplex(1 << 20);
    let (sr, sw) = tokio::io::split(server);
    tokio::spawn(serve_io(
        store.clone(),
        common::owner(),
        BufReader::new(sr),
        sw,
    ));
    let (cr, mut to) = tokio::io::split(client);
    let mut from = BufReader::new(cr);
    let mut id = 0;
    let mut request = async |method: &str, params: Value| -> Value {
        id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let mut bytes = serde_json::to_vec(&msg).unwrap();
        bytes.push(b'\n');
        to.write_all(&bytes).await.unwrap();
        loop {
            let mut l = String::new();
            tokio::time::timeout(WAIT, from.read_line(&mut l))
                .await
                .expect("an answer within the wait")
                .unwrap();
            let m: Value = serde_json::from_str(&l).unwrap();
            if m["id"] == json!(id) {
                return m;
            }
        }
    };
    request("initialize", json!({"protocolVersion": "2025-06-18"})).await;
    for case in cases() {
        let Some(tool) = case.tool else { continue };
        let at = format!("{backend:?}, {tool}");
        let r = request("tools/call", json!({"name": tool, "arguments": case.args})).await;
        let result = &r["result"];
        assert_eq!(result["isError"], true, "{at}: {r}");
        let text = result["content"][0]["text"].as_str().unwrap_or_default();
        if let Err(e) = names_all(text, case.names) {
            panic!("{at}: {e}");
        }
        assert!(
            text.contains("nothing was declared"),
            "{at}: the todo does not say nothing was declared: {text}"
        );
        assert_eq!(store.head().await.unwrap(), head, "{at}: the lake moved");
    }
}
