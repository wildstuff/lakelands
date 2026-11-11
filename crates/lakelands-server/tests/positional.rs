//! A struct written as a sequence is refused at the door that reads it,
//! and the same value as an object still reads (record 110). A find's
//! aggregate measure and its order, and a declaration under a proposed
//! change, are asked over MCP; a declaration on its own door is asked
//! over REST. The refusal names the members the object takes and where
//! the sequence sat.

mod common;

use common::wire::{Mcp, rest, serve};
use common::{BACKENDS, Backend};

use serde_json::{Value, json};

/// A declaration of the type `name` in the object form.
fn plain(name: &str) -> Value {
    json!({"declare": "type", "name": name, "fields": [{"name": "body", "type": {"kind": "text"}}]})
}

/// `text` names everything `names` lists, or says what it lacks.
fn names_all(text: &str, names: &[&str]) -> Result<(), String> {
    match names.iter().find(|n| !text.contains(*n)) {
        Some(n) => Err(format!("`{n}` is not named in: {text}")),
        None => Ok(()),
    }
}

#[tokio::test]
async fn mcp_refuses_a_struct_written_as_a_sequence_naming_its_members() {
    for backend in BACKENDS {
        mcp_refusals(backend).await;
        mcp_object_forms(backend).await;
    }
}

async fn mcp_refusals(backend: Backend) {
    let store = common::lake_on(backend).await;
    let mut mcp = Mcp::open(store.clone(), common::owner()).await;
    let cases: [(Value, &[&str]); 2] = [
        (
            json!({"aggregate": {"measures": [["n", "count"]]}}),
            &["aggregate.measures[0]", "`name`", "`func`"],
        ),
        (
            json!({"order_by": [["amount", "value", "asc"]]}),
            &["order_by[0]", "`field`", "`by`", "`dir`"],
        ),
    ];
    for (args, names) in cases {
        let result = mcp.call("invoice_find", args.clone()).await;
        assert_eq!(result["isError"], true, "{backend:?}, {args}: {result}");
        let text = result["content"][0]["text"].as_str().unwrap_or_default();
        names_all(text, names).unwrap_or_else(|e| panic!("{backend:?}: {e}"));
    }
    let result = mcp
        .call(
            "propose",
            json!({"write": {"write": "declare", "declarations": [["type", "memo"]]},
                   "why": "a memo type"}),
        )
        .await;
    assert_eq!(result["isError"], true, "{backend:?}: {result}");
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    names_all(text, &["write", "declarations[0]", "`declare`"])
        .unwrap_or_else(|e| panic!("{backend:?}: {e}"));
}

async fn mcp_object_forms(backend: Backend) {
    let store = common::lake_on(backend).await;
    let mut mcp = Mcp::open(store.clone(), common::owner()).await;
    for args in [
        json!({"aggregate": {"measures": [{"name": "n", "func": "count"}]}}),
        json!({"order_by": [{"field": "amount", "by": "value", "dir": "asc"}]}),
    ] {
        let result = mcp.call("invoice_find", args.clone()).await;
        assert_ne!(result["isError"], true, "{backend:?}, {args}: {result}");
    }
    let result = mcp
        .call(
            "propose",
            json!({"write": {"write": "declare", "declarations": [plain("memo")]},
                   "why": "a memo type"}),
        )
        .await;
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    assert!(
        !text.contains("not given by position"),
        "{backend:?}: the object form was refused as positional: {text}"
    );
}

#[tokio::test]
async fn rest_refuses_a_declaration_written_as_a_sequence_naming_declare() {
    for backend in BACKENDS {
        let store = common::lake_on(backend).await;
        let addr = serve(store.clone(), common::resolver()).await;
        let args = json!({"declaration": ["type", "memo"]});
        let (status, answer) = rest(addr, "k-owner", "POST", "schema/declare", Some(&args)).await;
        assert_eq!(status, 400, "{backend:?}: {answer}");
        assert_eq!(answer["error"], "refused", "{backend:?}: {answer}");
        let what = answer["what"].as_str().unwrap_or_default();
        names_all(what, &["declaration", "`declare`"])
            .unwrap_or_else(|e| panic!("{backend:?}: {e}"));
        // The object form of the same declaration reads.
        let args = json!({"declaration": plain("memo")});
        let (status, answer) = rest(addr, "k-owner", "POST", "schema/declare", Some(&args)).await;
        assert!(status < 300, "{backend:?}: {answer}");
    }
}
