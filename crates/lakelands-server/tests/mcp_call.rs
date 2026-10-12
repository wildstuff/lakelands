//! The `mcp_call` example, run as the walk runs it: a separate process
//! that starts a stdio server and makes its requests in one session. The
//! server here is the `memory_stdio` example, the fixture lake in memory.

use std::path::PathBuf;
use std::process::Command;

/// Where cargo put this package's examples: beside the test binary's
/// `deps` directory.
fn example(name: &str) -> PathBuf {
    let exe = std::env::current_exe().expect("the test knows its own path");
    let target = exe
        .parent()
        .and_then(|deps| deps.parent())
        .expect("the test binary lives in <target>/<profile>/deps");
    target.join("examples").join(name)
}

#[test]
fn mcp_call_lists_the_tools_and_calls_one_in_one_session() {
    let server = example("memory_stdio");
    assert!(
        server.exists(),
        "build the examples first: {}",
        server.display()
    );
    let out = Command::new(example("mcp_call"))
        .args(["--server", &server.display().to_string()])
        .args(["tools/list"])
        .args(["tools/call", "pin", "{}"])
        .args(["tools/call", "invoice_find", r#"{"limit": 1}"#])
        .output()
        .expect("mcp_call runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "mcp_call failed:\n{stdout}\n{stderr}");
    for tool in ["\"describe\"", "\"invoice_find\"", "\"pin\""] {
        assert!(
            stdout.contains(tool),
            "tools/list did not list {tool}:\n{stdout}"
        );
    }
    // The fixture's four declarations follow the lake's first append
    // (`lake_created`, `lease_taken`, `sys.setting`, `sys.aging_rule`), at 4 to 7.
    assert!(
        stdout.contains("position 7\n"),
        "a call prints its position line:\n{stdout}"
    );
    assert!(
        stdout.contains("\"pinned\": 7"),
        "the pin answers the head:\n{stdout}"
    );
    assert!(
        stdout.contains("\"shape\": \"records\""),
        "the find's structured content:\n{stdout}"
    );
}

#[test]
fn tools_list_with_schemas_prints_each_doors_schemas() {
    let server = example("memory_stdio");
    let run = |list: &[&str]| {
        let out = Command::new(example("mcp_call"))
            .args(["--server", &server.display().to_string()])
            .args(list)
            .output()
            .expect("mcp_call runs");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        // One JSON document per request.
        serde_json::Deserializer::from_slice(&out.stdout)
            .into_iter::<serde_json::Value>()
            .map(Result::unwrap)
            .collect::<Vec<_>>()
    };
    let plain = &run(&["tools/list"])[0];
    assert!(plain[0].get("inputSchema").is_none(), "{plain}");
    let listed = &run(&["tools/list", "--schemas"])[0];
    let upsert = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "invoice_upsert")
        .expect("the owner may upsert invoices");
    // The schema is the type's, filtered by the caller's rights: the
    // fields an upsert may write, and an answer with its position.
    let fields = &upsert["inputSchema"]["properties"]["fields"]["properties"];
    assert!(fields.get("status").is_some(), "{upsert}");
    assert!(upsert["outputSchema"].is_object(), "{upsert}");
    // A request after it is still read as a request.
    let two = run(&["tools/list", "--schemas", "tools/list"]);
    assert_eq!(two.len(), 2);
    assert!(two[1][0].get("inputSchema").is_none());
}

#[test]
fn a_tool_error_is_shown_whole_and_the_session_goes_on_to_exit_1() {
    let server = example("memory_stdio");
    let out = Command::new(example("mcp_call"))
        .args(["--server", &server.display().to_string()])
        .args(["tools/call", "invoice_get", r#"{"key": "no-such-invoice"}"#])
        .args(["tools/call", "pin", "{}"])
        .output()
        .expect("mcp_call runs");
    assert_eq!(out.status.code(), Some(1), "a refused call fails the run");
    let stderr = String::from_utf8_lossy(&out.stderr);
    // The two sentences, then the error as JSON, as an agent reads them.
    let (text, json) = stderr
        .split_once("\n{")
        .unwrap_or_else(|| panic!("the error's JSON follows its text:\n{stderr}"));
    assert!(
        text.starts_with("error: there is no `invoice` with key `no-such-invoice`\n  what to do: "),
        "{stderr}"
    );
    let json: serde_json::Value = serde_json::from_str(&format!("{{{json}")).unwrap();
    assert_eq!(json["error"], "not_found", "{json}");
    // The call after the refusal still runs, in the same session.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("\"pinned\": 7"),
        "the pin after the refusal answers:\n{stdout}"
    );
}
