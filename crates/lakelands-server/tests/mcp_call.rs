//! The `mcp_call` example, run as the walk runs it: a separate process
//! that starts a stdio server and makes its requests in one session. The
//! server here is the `mock_stdio` example, the fixture lake in memory.

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
    let server = example("mock_stdio");
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
    // (`lake_created`, `lease_taken`, `sys.setting`), at 3 to 6.
    assert!(
        stdout.contains("position 6\n"),
        "a call prints its position line:\n{stdout}"
    );
    assert!(
        stdout.contains("\"pinned\": 6"),
        "the pin answers the head:\n{stdout}"
    );
    assert!(
        stdout.contains("\"shape\": \"records\""),
        "the find's structured content:\n{stdout}"
    );
}

#[test]
fn a_tool_error_is_its_two_sentences_and_exit_1() {
    let server = example("mock_stdio");
    let out = Command::new(example("mcp_call"))
        .args(["--server", &server.display().to_string()])
        .args(["tools/call", "invoice_get", r#"{"key": "no-such-invoice"}"#])
        .output()
        .expect("mcp_call runs");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr
            .starts_with("error: there is no `invoice` with key `no-such-invoice`\n  what to do: "),
        "{stderr}"
    );
}
