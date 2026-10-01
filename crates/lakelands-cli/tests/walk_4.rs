//! Walk 4's materials, kept from rotting: the commands
//! `docs/walks/walk-4.md` types over `examples/shop/` give the page's
//! answers at the page's positions.
//!
//! - An order arrives with a customer number, and a derived relation
//!   links it to its customer with no link record.
//! - Paying it fires a trigger guarded by the transition from `open`.
//! - The host (`walk_4_host`) reacts: it calls `create_payment`, a verb
//!   on the type `payment`, naming the firing as its cause, and writes the
//!   payment, naming the call.
//! - A second run of the host appends nothing, and its retry answers the
//!   call's first position with the ledger's recorded answer.
//! - The lineage reads back from the payment to the order's change.
//! - Retiring the order fires nothing.
//! - A view with a parameter answers one window per date the reader
//!   binds, and refuses a read that binds none.
//!
//! Each command runs the built `lakelands` binary in a process of its
//! own, as the walker runs it, since each process that writes takes the
//! lake's lease once and the positions count it. The host is the built
//! `walk_4_host` example, which `cargo test` builds beside the tests.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn example() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/shop")
}

fn show(p: &Path) -> String {
    p.display().to_string()
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

/// The `walk_4_host` example beside this test's own binary: the test runs
/// from `<target>/<profile>/deps/`, the example is built into
/// `<target>/<profile>/examples/`.
fn host_program() -> PathBuf {
    let exe = std::env::current_exe().expect("the test knows its own path");
    let profile = exe
        .parent()
        .and_then(|deps| deps.parent())
        .expect("the test runs from <target>/<profile>/deps");
    let program = profile.join("examples").join("walk_4_host");
    assert!(
        program.is_file(),
        "{} is missing: build it with `cargo build -p lakelands-cli --example walk_4_host`",
        program.display()
    );
    program
}

/// One MCP session with `lakelands <args> serve --mcp`, as the page's
/// client makes it: initialize, then each `tools/call` in order; the
/// structured answer of every call, each a success.
fn mcp(args: &[&str], calls: &[(&str, Value)]) -> Vec<Value> {
    mcp_results(args, calls)
        .into_iter()
        .enumerate()
        .map(|(i, result)| {
            assert_ne!(result["isError"], true, "call {} failed: {result}", i + 1);
            result["structuredContent"].clone()
        })
        .collect()
}

/// The same session, answering every call's whole `result`.
fn mcp_results(args: &[&str], calls: &[(&str, Value)]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lakelands"))
        .args(args)
        .args(["serve", "--mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the server starts");
    let mut lines = vec![
        json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
               "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                          "clientInfo": {"name": "walk_4", "version": "0"}}}),
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
            answers
                .iter()
                .find(|a| a["id"] == json!(id))
                .unwrap_or_else(|| panic!("call {id} was answered: {answers:?}"))["result"]
                .clone()
        })
        .collect()
}

/// The lake's feed: every event, as `lakelands feed` prints it.
fn feed(root: &str) -> Vec<Value> {
    ok(&["--root", root, "feed"])
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// The event at `position`, as its kind, the body members named, and its
/// cause.
fn at(feed: &[Value], position: usize, members: &[&str]) -> (String, Vec<Value>, Value) {
    let e = &feed[position];
    assert_eq!(e["position"], json!(position));
    let body = &e["body"];
    (
        body["kind"].as_str().unwrap().to_owned(),
        members.iter().map(|m| body[*m].clone()).collect(),
        e["cause"].clone(),
    )
}

#[test]
fn the_walks_commands_run_a_procedure_from_an_order_to_its_payment() {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("walk-4");
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    let walk = base.join("walk");
    let w = show(&walk);
    let shop = show(&example());
    let data = |name: &str| show(&example().join("data").join(name));
    let root = ["--root", w.as_str()];

    // 1. The lake, its ontology and its data.
    ok(&["init", &w]);
    ok(&["--root", &w, "lake", "new", "shop"]);
    assert_eq!(
        ok(&["--root", &w, "apply", &shop]),
        "applied 7 declaration(s), in force from position 10:\n  type `customer`\n  \
         type `order`\n  type `payment`\n  type `revenue_7d`\n  verb `create_payment`\n  \
         trigger `order_paid`\n  trigger `order_settled`\n"
    );
    let events = feed(&w);
    for e in &events[4..=10] {
        assert_eq!(e["v"], json!(8), "the apply's append is in format 8");
    }
    for (file, through, n) in [("customers.ndjson", 13, 2), ("orders.ndjson", 17, 3)] {
        assert_eq!(
            ok(&["--root", &w, "ingest", &data(file), "--source", "shop"]).trim_end(),
            format!("committed {n} record(s), 0 already in the lake, through position {through}")
        );
    }

    // 2. The derived relation: from the order to its customer, by number.
    let answers = mcp(
        &root,
        &[
            (
                "query",
                json!({"start": {"type": "order", "key": "O-1"},
                       "steps": [{"relation": "placed_by"}]}),
            ),
            ("describe", json!({})),
        ],
    );
    assert_eq!(answers[0]["position"], json!(18));
    let rows = answers[0]["data"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["customer.name"], json!("Harbor Supplies"));
    let types = answers[1]["data"]["types"].as_array().unwrap();
    assert!(
        !types
            .iter()
            .any(|t| t["name"].as_str().unwrap().starts_with("sys.link.")),
        "a derived relation has no link type"
    );
    let order = types.iter().find(|t| t["name"] == json!("order")).unwrap();
    assert_eq!(
        order["relations"][0]["derived"],
        json!({"match": [{"from": "customer_no", "to": "number"}]})
    );

    // 3. Two orders paid; a later write of a paid order's date only.
    let answers = mcp(
        &root,
        &[
            (
                "order_upsert",
                json!({"key": "O-1", "fields": {"status": "paid", "paid_on": "2026-09-28"}}),
            ),
            (
                "order_upsert",
                json!({"key": "O-2", "fields": {"status": "paid", "paid_on": "2026-10-03"}}),
            ),
            (
                "order_upsert",
                json!({"key": "O-1", "fields": {"paid_on": "2026-09-29"}}),
            ),
        ],
    );
    let written: Vec<Value> = answers.iter().map(|a| a["position"].clone()).collect();
    assert_eq!(written, [json!(20), json!(23), json!(26)]);
    let events = feed(&w);
    for (p, trigger, key, cause) in [
        (21, "order_paid", "O-1", 20),
        (22, "order_settled", "O-1", 20),
        (24, "order_paid", "O-2", 23),
        (25, "order_settled", "O-2", 23),
        (27, "order_settled", "O-1", 26),
    ] {
        assert_eq!(
            at(&events, p, &["trigger", "key"]),
            (
                "trigger_fired".to_owned(),
                vec![json!(trigger), json!(key)],
                json!(cause)
            ),
            "at {p}"
        );
    }
    assert_eq!(events.len(), 28, "`paid_on` alone is no transition");

    // 4. The host reacts to every `order_paid` from 20 on.
    let host = || {
        let out = Command::new(host_program())
            .args(["--root", &w, "--lake", "shop", "--from", "20"])
            .output()
            .expect("the host runs");
        assert!(
            out.status.success(),
            "walk_4_host failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    let reactions = "order `O-1`: fired at 21, create_payment at 29 answered \
                     {\"payment\":\"PAY-O-1\"}, payment `PAY-O-1` at 31\n\
                     order `O-2`: fired at 24, create_payment at 32 answered \
                     {\"payment\":\"PAY-O-2\"}, payment `PAY-O-2` at 34\n";
    assert_eq!(host(), reactions);
    let events = feed(&w);
    assert_eq!(at(&events, 28, &[]).0, "lease_taken");
    for (call, fired, order, payment) in [(29, 21, "O-1", "PAY-O-1"), (32, 24, "O-2", "PAY-O-2")] {
        let e = &events[call];
        assert_eq!(e["body"]["kind"], json!("verb_called"));
        assert!(e["body"].get("key").is_none(), "a verb on its type: no key");
        assert_eq!(e["cause"], json!(fired), "the call names the firing");
        assert_eq!(e["context"], json!({"depth": 1, "origin": "reactive"}));
        assert_eq!(e["writer"], json!("service:walk_4_host"));
        assert_eq!(
            at(&events, call + 1, &["output"]),
            (
                "output_recorded".to_owned(),
                vec![json!({"payment": payment})],
                json!(call)
            )
        );
        assert_eq!(
            at(&events, call + 2, &["type", "key"]),
            (
                "record_upserted".to_owned(),
                vec![json!("payment"), json!(payment)],
                json!(call)
            ),
            "the payment names the call"
        );
        assert_eq!(events[call + 2]["body"]["patch"]["order_id"], json!(order));
        assert_eq!(
            events[call + 2]["context"],
            json!({"depth": 2, "origin": "reactive"})
        );
    }

    // 5. The host again: every reaction answers its first outcome, the
    // call at its own position with the ledger's recorded answer, and
    // nothing is appended but the host's lease.
    assert_eq!(host(), reactions);
    let events = feed(&w);
    assert_eq!(events.len(), 36);
    assert_eq!(at(&events, 35, &[]).0, "lease_taken");

    // 6. The lineage, read back from the payment.
    let answers = mcp(
        &root,
        &[("payment_get", json!({"key": "PAY-O-1", "with_meta": true}))],
    );
    assert_eq!(answers[0]["position"], json!(36));
    assert_eq!(answers[0]["data"]["meta"]["amount"]["position"], json!(31));
    let cause = |p: usize| events[p]["cause"].as_u64().unwrap();
    assert_eq!(cause(31), 29, "the payment names the call");
    assert_eq!(cause(29), 21, "the call names the firing");
    assert_eq!(cause(21), 20, "the firing names the order's change");
    assert_eq!(at(&events, 20, &["key"]).1, vec![json!("O-1")]);

    // 7. Retiring the paid order fires nothing.
    let answers = mcp(&root, &[("order_retire", json!({"key": "O-1"}))]);
    assert_eq!(answers[0]["position"], json!(38));
    let events = feed(&w);
    assert_eq!(at(&events, 38, &["key"]).0, "record_retired");
    assert_eq!(events.len(), 39, "nothing after the retirement");

    // 8. The view at two business dates, and a read that binds none.
    let window = |anchor: &str| ("revenue_7d_find", json!({"params": {"anchor": anchor}}));
    let answers = mcp(&root, &[window("2026-09-30"), window("2026-10-07")]);
    let rows = |a: &Value| {
        assert_eq!(a["position"], json!(39));
        a["data"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r["key"].clone(), r["fields"].clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        rows(&answers[0]),
        [(
            json!("C-1"),
            json!({"customer_no": "C-1", "payments": 1, "total": "120.00"})
        )]
    );
    assert_eq!(
        rows(&answers[1]),
        [(
            json!("C-2"),
            json!({"customer_no": "C-2", "payments": 1, "total": "80.00"})
        )]
    );
    let refused = mcp_results(&root, &[("revenue_7d_find", json!({}))]);
    assert_eq!(refused[0]["isError"], true, "{}", refused[0]);
    let said = refused[0]["content"][0]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(
        said.contains("view `revenue_7d` needs parameter `anchor` (date_time)"),
        "{said}"
    );
}
