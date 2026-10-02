//! A declaration member Lakelands does not know, at any depth, is refused
//! at the JSON doors, over REST and over MCP, on the mock and on the store
//! over both engines: the refusal names the member, where it sits inside
//! the declaration, and where the declaration sits among the arguments,
//! and the lake does not move.
//!
//! `schema/declare` and `schema/apply` are REST's alone; `declare_type`,
//! `add_field`, `migrate` and `propose` are asked on both.

mod common;

use common::wire::{Mcp, rest, serve};
use common::{BACKENDS, Backend};

use serde_json::{Value, json};

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

#[tokio::test]
async fn rest_refuses_a_declaration_member_it_does_not_know_naming_it() {
    for backend in BACKENDS {
        rest_on(backend).await;
    }
}

async fn rest_on(backend: Backend) {
    let store = common::lake_on(backend).await;
    let head = store.head().await.unwrap();
    let addr = serve(store.clone(), common::resolver()).await;
    for case in cases() {
        let (status, answer) = rest(addr, "k-owner", "POST", case.route, Some(&case.args)).await;
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
    let mut mcp = Mcp::open(store.clone(), common::owner()).await;
    for case in cases() {
        let Some(tool) = case.tool else { continue };
        let at = format!("{backend:?}, {tool}");
        let result = mcp.call(tool, case.args).await;
        assert_eq!(result["isError"], true, "{at}: {result}");
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
