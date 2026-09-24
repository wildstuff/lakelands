//! The official Rust MCP SDK, as a client, talks to this server over
//! stdio and over streamable HTTP. The SDK is a dev-dependency only;
//! what the server answers byte for byte is pinned in the transcripts.

mod common;

use common::{BACKENDS, Backend};

use std::sync::Arc;

use lakelands_api::LakeStore;
use lakelands_server::{HttpConfig, serve_http, serve_io};
use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use serde_json::{Value, json};
use tokio::io::BufReader;

fn params(name: &str, arguments: Value) -> CallToolRequestParams {
    serde_json::from_value(json!({"name": name, "arguments": arguments})).unwrap()
}

fn structured(r: &CallToolResult) -> Value {
    serde_json::to_value(r).unwrap()["structuredContent"].clone()
}

/// What any client should see of the fixture lake, as the owner.
async fn exercise(client: &RunningService<RoleClient, ()>) {
    let info = client.peer_info().expect("the server introduced itself");
    let info = serde_json::to_value(info).unwrap();
    assert_eq!(info["serverInfo"]["name"], "lakelands");

    let tools = client.list_all_tools().await.unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    for expected in lakelands_api::tools::GENERIC_TOOLS {
        assert!(
            names.contains(&expected),
            "{expected} missing from {names:?}"
        );
    }

    let got = client
        .call_tool(params("invoice_get", json!({"key": "INV-1"})))
        .await
        .unwrap();
    assert_eq!(got.is_error, Some(false));
    let s = structured(&got);
    assert_eq!(s["position"], 8);
    assert_eq!(s["data"]["fields"]["status"], "open");

    let pinned = client.call_tool(params("pin", json!({}))).await.unwrap();
    assert_eq!(structured(&pinned)["pinned"], 8);
    let wrote = client
        .call_tool(params(
            "invoice_upsert",
            json!({"key": "INV-1", "fields": {"status": "held"}}),
        ))
        .await
        .unwrap();
    assert_eq!(structured(&wrote)["position"], 9);
    let seen = client
        .call_tool(params("invoice_get", json!({"key": "INV-1"})))
        .await
        .unwrap();
    assert_eq!(
        structured(&seen)["data"]["fields"]["status"],
        "open",
        "the pin holds"
    );

    let refused = client
        .call_tool(params(
            "invoice_upsert",
            json!({"key": "INV-1", "fields": {"amount": "1.00"}}),
        ))
        .await
        .unwrap();
    assert_eq!(refused.is_error, Some(true));
    let text = serde_json::to_value(&refused).unwrap()["content"][0]["text"].clone();
    assert!(
        text.as_str()
            .unwrap()
            .starts_with("error: refused on `amount`"),
        "{text}"
    );

    let unknown = client
        .call_tool(params("invoice_teleport", json!({})))
        .await;
    assert!(unknown.is_err(), "an unknown tool is a protocol error");
}

#[tokio::test]
async fn the_sdk_client_works_over_stdio() {
    for backend in BACKENDS {
        the_sdk_client_works_over_stdio_on(backend).await;
    }
}

async fn the_sdk_client_works_over_stdio_on(backend: Backend) {
    let (client_io, server_io) = tokio::io::duplex(1 << 20);
    let (server_read, server_write) = tokio::io::split(server_io);
    let server = tokio::spawn(serve_io(
        common::lake_on(backend).await,
        common::owner(),
        BufReader::new(server_read),
        server_write,
    ));
    let client = ().serve(tokio::io::split(client_io)).await.unwrap();
    exercise(&client).await;
    client.cancel().await.unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test]
async fn the_sdk_client_works_over_streamable_http() {
    for backend in BACKENDS {
        the_sdk_client_works_over_streamable_http_on(backend).await;
    }
}

async fn the_sdk_client_works_over_streamable_http_on(backend: Backend) {
    let store = common::lake_on(backend).await;
    let open = Arc::new(
        move |_: &lakelands_api::LakeId| -> lakelands_api::Result<Arc<dyn LakeStore>> {
            Ok(store.clone())
        },
    );
    let config = HttpConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        allowed_origins: Vec::new(),
        session_idle_secs: 1800,
        feed_poll_millis: 250,
    };
    let server = serve_http(config, common::resolver(), open).await.unwrap();
    let url = format!("http://{}/lakes/finance/mcp", server.local_addr().unwrap());
    let task = tokio::spawn(server.run());
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(url).auth_header("k-owner"),
    );
    let client = ().serve(transport).await.unwrap();
    exercise(&client).await;
    client.cancel().await.unwrap();
    task.abort();
}
