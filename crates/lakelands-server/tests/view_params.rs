//! A read's `params` on a type that declares no parameters reaches the
//! catalog through both JSON doors, so the answer is the catalog's: it
//! names the parameter. The MCP tool of such a type lists no `params`,
//! and its unknown-argument check lets a read's `params` through; REST
//! takes `params` on both read routes. Run on the mock and on the store
//! over both engines; nothing is appended.

mod common;

use common::wire::{Mcp, rest, serve};
use common::{BACKENDS, Backend};

use serde_json::json;

/// `params={"anchor":"2026-06-30"}`, percent-encoded as a query value.
const PARAMS: &str = "params=%7B%22anchor%22%3A%222026-06-30%22%7D";

#[tokio::test]
async fn mcp_answers_params_on_a_type_without_parameters_from_the_catalog() {
    for backend in BACKENDS {
        mcp_on(backend).await;
    }
}

async fn mcp_on(backend: Backend) {
    let store = common::lake_on(backend).await;
    let head = store.head().await.unwrap();
    let mut mcp = Mcp::open(store.clone(), common::owner()).await;
    let params = json!({"anchor": "2026-06-30"});
    for (tool, args) in [
        ("invoice_find", json!({"params": params})),
        ("invoice_get", json!({"key": "INV-1", "params": params})),
    ] {
        let at = format!("{backend:?}, {tool}");
        let result = mcp.call(tool, args).await;
        assert_eq!(result["isError"], true, "{at}: {result}");
        let text = result["content"][0]["text"].as_str().unwrap_or_default();
        assert!(text.contains("anchor"), "{at}: names `anchor`: {text}");
        assert!(
            !text.contains("takes no argument"),
            "{at}: the catalog answers, not the tool's schema: {text}"
        );
        assert_eq!(store.head().await.unwrap(), head, "{at}: the lake moved");
    }
}

#[tokio::test]
async fn rest_answers_params_on_a_type_without_parameters_from_the_catalog() {
    for backend in BACKENDS {
        rest_on(backend).await;
    }
}

async fn rest_on(backend: Backend) {
    let store = common::lake_on(backend).await;
    let head = store.head().await.unwrap();
    let addr = serve(store.clone(), common::resolver()).await;
    for route in [
        format!("types/invoice/records?{PARAMS}"),
        format!("types/invoice/records/INV-1?{PARAMS}"),
    ] {
        let at = format!("{backend:?}, GET {route}");
        let (status, answer) = rest(addr, "k-owner", "GET", &route, None).await;
        assert_eq!(status, 422, "{at}: {answer}");
        assert_eq!(answer["reason"]["refusal"], "invalid", "{at}: {answer}");
        let what = answer["what"].as_str().unwrap_or_default();
        assert!(what.contains("anchor"), "{at}: names `anchor`: {answer}");
        assert_eq!(store.head().await.unwrap(), head, "{at}: the lake moved");
    }
}
