//! Walk 2's materials, kept from rotting: the collections bundle in
//! `examples/collections/` loads, and the commands `docs/walks/walk-2.md`
//! types (`init`, `lake new`, `apply`, the two `ingest --map` runs and
//! the agent's two links through `serve --mcp`) give the page's answers
//! at the page's positions. Each command runs the built `lakelands`
//! binary in a process of its own, as the walker runs it, since each
//! process takes the lake's lease once and the positions count it.
//!
//! The embedding endpoint runs inside the test, from the fake's own
//! source. The `exec` program is the built `walk_2_fake` example, which
//! `cargo test` builds beside the tests. The mock is not walked: it
//! refuses a function of the `exec` backing and an embedded field as
//! unbound when they are declared.

use std::fs;
use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use lakelands_api::write::Declaration;
use serde_json::{Value, json};

#[path = "../examples/walk_2_fake.rs"]
#[allow(dead_code)]
mod fake;

fn example() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/collections")
}

/// The `walk_2_fake` example beside this test's own binary: the test runs
/// from `<target>/<profile>/deps/`, the example is built into
/// `<target>/<profile>/examples/`.
fn fake_program() -> PathBuf {
    let exe = std::env::current_exe().expect("the test knows its own path");
    let profile = exe
        .parent()
        .and_then(|deps| deps.parent())
        .expect("the test runs from <target>/<profile>/deps");
    let program = profile.join("examples").join("walk_2_fake");
    assert!(
        program.is_file(),
        "{} is missing: build it with `cargo build -p lakelands-cli --example walk_2_fake`",
        program.display()
    );
    program
}

#[test]
fn the_collections_bundle_loads_with_every_lane_the_walk_needs() {
    let bundle = lakelands_core::bundle::load_dir(&example()).unwrap_or_else(|problems| {
        let lines: Vec<String> = problems.iter().map(ToString::to_string).collect();
        panic!(
            "the collections bundle does not load:\n{}",
            lines.join("\n")
        )
    });
    let names: Vec<String> = bundle
        .declarations
        .iter()
        .map(|d| match d {
            Declaration::ValueType(v) => format!("value_type {}", v.name),
            Declaration::Type(t) => format!("type {}", t.name),
            Declaration::Function(f) => format!("function {} ({})", f.name, f.backing),
            Declaration::Verb(v) => format!("verb {}", v.name),
            Declaration::Trigger(t) => format!("trigger {}", t.name),
            Declaration::ObjectSet(o) => format!("object_set {}", o.name),
        })
        .collect();
    assert_eq!(
        names,
        [
            "value_type invoice_status",
            "value_type money",
            "type invoice",
            "type partner",
            "function band (exec)",
            "trigger went_overdue",
        ]
    );
}

/// `lakelands <args>` in a process of its own, answering what it printed.
fn ok(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_lakelands"))
        .args(args)
        .output()
        .expect("the binary runs");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "`lakelands {}` failed:\n{err}",
        args.join(" ")
    );
    String::from_utf8(out.stdout).unwrap()
}

/// One MCP session with `lakelands serve --mcp`, as the page's client
/// makes it: initialize, then each `tools/call` in order; the structured
/// content of every answer.
fn mcp(root: &str, calls: &[(&str, Value)]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lakelands"))
        .args(["--root", root, "serve", "--mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the server starts");
    let mut lines = vec![
        json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
               "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                          "clientInfo": {"name": "walk_2", "version": "0"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    ];
    for (id, (name, arguments)) in calls.iter().enumerate() {
        lines.push(
            json!({"jsonrpc": "2.0", "id": id + 1, "method": "tools/call",
                          "params": {"name": name, "arguments": arguments}}),
        );
    }
    let mut stdin = child.stdin.take().unwrap();
    for line in &lines {
        writeln!(stdin, "{line}").unwrap();
    }
    drop(stdin);
    let out = child
        .wait_with_output()
        .expect("the server ends with its input");
    let answers: Vec<Value> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    (1..=calls.len())
        .map(|id| {
            let a = answers
                .iter()
                .find(|a| a["id"] == json!(id))
                .unwrap_or_else(|| panic!("call {id} was answered: {answers:?}"));
            a["result"]["structuredContent"].clone()
        })
        .collect()
}

#[cfg(unix)]
#[test]
fn the_walks_commands_fill_band_and_summary_vec_through_the_fakes() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || fake::serve(listener, |_, body| fake::embed(body)));
    // The deliverer of every `serve` posts here; nothing fires in this test.
    let hooks = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let hook_addr = hooks.local_addr().unwrap();
    std::thread::spawn(move || fake::serve(hooks, |_, _| json!({})));

    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("walk-2-store-sqlite");
    let _ = fs::remove_dir_all(&root);
    let r = root.display().to_string();
    ok(&["init", &r]);
    ok(&["--root", &r, "lake", "new", "collections"]);

    // The page's configuration, with the fake's path and this test's ports.
    let config = fs::read_to_string(example().join("lakelands.json"))
        .unwrap()
        .replace("<fake>", &fake_program().display().to_string())
        .replace("127.0.0.1:8701", &addr.to_string())
        .replace("127.0.0.1:8702", &hook_addr.to_string());
    fs::write(root.join("lakelands.json"), config).unwrap();

    // The positions are the page's: every command that writes takes the
    // lease first, so a run of the walk lands on the same numbers.
    let applied = ok(&["--root", &r, "apply", &example().display().to_string()]);
    assert!(
        applied.starts_with("applied 6 declaration(s), in force from position 10:\n")
            && applied.lines().filter(|l| l.starts_with("  ")).count() == 6,
        "apply applies the bundle's six declarations as one set, from position 10:\n{applied}"
    );
    let data = example().join("data");
    let path = |name: &str| data.join(name).display().to_string();
    let ingest = |file: &str, source: &str, map: &str| {
        ok(&[
            "--root",
            &r,
            "ingest",
            &path(file),
            "--source",
            source,
            "--map",
            &path(map),
        ])
    };
    let partners = ingest("partners.csv", "crm", "partners.map.json");
    assert_eq!(
        partners.trim_end(),
        "committed 3 record(s), 0 already in the lake, through position 14"
    );
    let invoices = ingest("invoices.csv", "erp", "invoices.map.json");
    assert_eq!(
        invoices.trim_end(),
        "committed 6 record(s), 0 already in the lake, through position 33",
        "the ingest answers the position its calls' outputs are visible from"
    );

    // Twelve outputs, 22 to 33, in the page's order: INV-5001's band, the
    // six embeddings, then the other five bands.
    let feed = ok(&["--root", &r, "feed", "--kind", "output_recorded"]);
    let outputs: Vec<(u64, String, String, Value)> = feed
        .lines()
        .map(|l| {
            let e: Value = serde_json::from_str(l).unwrap();
            let b = &e["body"];
            (
                e["position"].as_u64().unwrap(),
                b["field"][1].as_str().unwrap().to_owned(),
                b["field"][2].as_str().unwrap().to_owned(),
                b["output"].clone(),
            )
        })
        .collect();
    let band = |i: &str, v: &str| (i.to_owned(), "band".to_owned(), json!(v));
    let vec = |i: &str, v: [u8; 4]| (i.to_owned(), "summary_vec".to_owned(), json!(v));
    let expected = [
        band("INV-5001", "mid"),
        vec("INV-5001", [1, 0, 0, 0]),
        vec("INV-5002", [2, 0, 0, 0]),
        vec("INV-5003", [0, 0, 1, 0]),
        vec("INV-5004", [0, 2, 0, 0]),
        vec("INV-5005", [0, 0, 0, 1]),
        vec("INV-5006", [1, 0, 0, 1]),
        band("INV-5002", "low"),
        band("INV-5003", "high"),
        band("INV-5004", "low"),
        band("INV-5005", "mid"),
        band("INV-5006", "mid"),
    ];
    assert_eq!(outputs.len(), expected.len(), "{feed}");
    for ((position, invoice, field, output), (at, (i, f, v))) in
        outputs.into_iter().zip((22..).zip(expected))
    {
        assert_eq!(
            (position, invoice, field, output),
            (at, i, f, v),
            "output {at} of the page"
        );
    }

    // The agent's two links, in one session: the server takes the lease
    // at 34, the links land at 35 and 36.
    let links = mcp(
        &r,
        &[
            (
                "invoice_approved_by_link",
                json!({"key": "INV-5001", "to": "P-300",
                       "fields": {"since": "2026-09-01T00:00:00Z"}}),
            ),
            (
                "invoice_approved_by_link",
                json!({"key": "INV-5003", "to": "P-100",
                       "fields": {"since": "2026-10-15T00:00:00Z"}}),
            ),
        ],
    );
    assert_eq!(
        links,
        [
            json!({"outcome": "applied", "position": 35, "version": 1}),
            json!({"outcome": "applied", "position": 36, "version": 1}),
        ]
    );
}
