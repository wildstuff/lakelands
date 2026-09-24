//! A small MCP client for trying a Lakelands server by hand: the walk's
//! "agent" without a chat product in the loop.
//!
//! ```text
//! mcp_call --server "<command>" <request>...
//! mcp_call --http <url> --key <api key> <request>...
//!
//! request := tools/list [--schemas]
//!          | tools/call <tool> '<arguments as JSON>'
//! ```
//!
//! `--server` starts the command (through `sh -c`) and speaks MCP with it
//! over its stdin and stdout; `--http` talks to a streamable HTTP door,
//! presenting the key as `Authorization: Bearer`. Either way it
//! initializes one session and makes the requests in order in that
//! session, so a `pin` holds for the calls after it.
//!
//! For `tools/list` it prints each tool's name and description, and with
//! `--schemas` its input and output JSON schemas too, so the doors'
//! schemas can be read as the server derives them from the types and the
//! caller's rights; for a
//! `tools/call`, the position line and the result's structured content as
//! pretty JSON, or the tool error's two sentences. It exits 0 when every
//! request succeeded, 1 when one failed (and stops there), and 2 when the
//! command line was not understood.
//!
//! ```sh
//! cargo run -p lakelands-server --example mcp_call -- \
//!   --server "lakelands --root walk serve --mcp" tools/list
//! cargo run -p lakelands-server --example mcp_call -- \
//!   --server "lakelands --root walk serve --mcp" tools/list --schemas
//! cargo run -p lakelands-server --example mcp_call -- \
//!   --server "lakelands --root walk serve --mcp" \
//!   tools/call pin '{}' tools/call invoice_find '{"where": "status = '"'"'open'"'"'"}'
//! ```

use std::process::{ExitCode, Stdio};

use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::service::{RoleClient, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use serde_json::{Value, json};

enum Request {
    /// `tools/list`, with the schemas when asked.
    List(bool),
    Call(String, Value),
}

const USAGE: &str = "usage: mcp_call (--server \"<command>\" | --http <url> --key <api key>) \
                     (tools/list [--schemas] | tools/call <tool> '<json arguments>')...";

fn usage(why: &str) -> ExitCode {
    eprintln!("error: {why}\n{USAGE}");
    ExitCode::from(2)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let mut args = std::env::args().skip(1).peekable();
    let (mut server, mut http, mut key) = (None, None, None);
    while let Some(flag) = args.peek().filter(|a| a.starts_with("--")).cloned() {
        args.next();
        let Some(value) = args.next() else {
            return usage(&format!("{flag} needs a value"));
        };
        match flag.as_str() {
            "--server" => server = Some(value),
            "--http" => http = Some(value),
            "--key" => key = Some(value),
            other => return usage(&format!("there is no option {other}")),
        }
    }
    let mut requests = Vec::new();
    while let Some(method) = args.next() {
        match method.as_str() {
            "tools/list" => {
                let schemas = args.next_if(|a| a == "--schemas").is_some();
                requests.push(Request::List(schemas));
            }
            "tools/call" => {
                let (Some(tool), Some(raw)) = (args.next(), args.next()) else {
                    return usage("tools/call needs a tool name and its arguments as JSON");
                };
                match serde_json::from_str::<Value>(&raw) {
                    Ok(v @ Value::Object(_)) => requests.push(Request::Call(tool, v)),
                    _ => return usage(&format!("the arguments of {tool} are not a JSON object")),
                }
            }
            other => return usage(&format!("{other} is not a request this client makes")),
        }
    }
    if requests.is_empty() {
        return usage("name at least one request");
    }

    let client = match (server, http, key) {
        (Some(command), None, None) => {
            let child = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(&command)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .kill_on_drop(true)
                .spawn();
            let mut child = match child {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("error: could not start `{command}`: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let (Some(stdout), Some(stdin)) = (child.stdout.take(), child.stdin.take()) else {
                eprintln!("error: `{command}` gave no stdin or stdout to talk over");
                return ExitCode::FAILURE;
            };
            match ().serve((stdout, stdin)).await {
                Ok(c) => (c, Some(child)),
                Err(e) => {
                    eprintln!("error: `{command}` did not answer initialize: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
        (None, Some(url), Some(key)) => {
            let transport = StreamableHttpClientTransport::from_config(
                StreamableHttpClientTransportConfig::with_uri(url.as_str()).auth_header(key),
            );
            match ().serve(transport).await {
                Ok(c) => (c, None),
                Err(e) => {
                    eprintln!("error: {url} did not answer initialize: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
        _ => return usage("name either --server, or --http with --key"),
    };
    let (client, _child) = client;
    let code = run(&client, requests).await;
    let _ = client.cancel().await;
    code
}

async fn run(client: &RunningService<RoleClient, ()>, requests: Vec<Request>) -> ExitCode {
    for request in requests {
        match request {
            Request::List(schemas) => match client.list_all_tools().await {
                Ok(tools) => {
                    let listed: Vec<Value> = tools
                        .iter()
                        .map(|t| {
                            let mut v = json!({"name": t.name, "description": t.description});
                            if schemas {
                                v["inputSchema"] = json!(t.input_schema);
                                v["outputSchema"] = json!(t.output_schema);
                            }
                            v
                        })
                        .collect();
                    println!("{}", pretty(&Value::Array(listed)));
                }
                Err(e) => {
                    eprintln!("error: tools/list failed: {e}");
                    return ExitCode::FAILURE;
                }
            },
            Request::Call(tool, arguments) => {
                let params: CallToolRequestParams =
                    match serde_json::from_value(json!({"name": tool, "arguments": arguments})) {
                        Ok(p) => p,
                        Err(e) => {
                            eprintln!("error: the call of {tool} does not read: {e}");
                            return ExitCode::FAILURE;
                        }
                    };
                let result = match client.call_tool(params).await {
                    Ok(r) => serde_json::to_value(&r).unwrap_or_default(),
                    Err(e) => {
                        eprintln!("error: {tool} failed: {e}");
                        return ExitCode::FAILURE;
                    }
                };
                if result["isError"] == Value::Bool(true) {
                    // The tool error's text: two sentences, what and what to do.
                    if let Some(text) = result["content"][0]["text"].as_str() {
                        eprint!("{text}");
                    }
                    return ExitCode::FAILURE;
                }
                let structured = &result["structuredContent"];
                println!("position {}", structured["position"]);
                println!("{}", pretty(structured));
            }
        }
    }
    ExitCode::SUCCESS
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}
