//! Walk 5's record, kept from rotting: the calls a blind LLM agent made
//! through MCP alone, as the principal `clerk`, give the answers
//! `docs/walks/walk-5.md` quotes, at its positions.
//!
//! - Run 1: the agent works the scenario on its own. It reads `describe`
//!   first, writes within its grants, meets the medium-risk proposal and
//!   an executor that is not bound.
//! - Run 2: each task starts with a colleague's faulty call, given to the
//!   agent; the agent's own next calls follow. Unknown field, missing key,
//!   no grant, a proposal shaped wrong, a stale version, a key on a
//!   type-scoped verb, a missing view parameter.
//! - Run 3: the same for a value of the wrong kind.
//!
//! Each run was one MCP session, so each replay is one `serve --mcp`
//! process, which takes the lake's lease once. The lake is walk 4's shop
//! with `examples/walk-5/`'s verb added and its principal in the root's
//! configuration.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn examples() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples")
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

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// One MCP session with `lakelands <args> serve --mcp`: initialize, then
/// each `tools/call` in order; every call's whole `result`.
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
                          "clientInfo": {"name": "walk_5", "version": "0"}}}),
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

/// A call's structured answer; the call succeeded.
fn answer(result: &Value) -> &Value {
    assert_ne!(result["isError"], true, "{result}");
    &result["structuredContent"]
}

/// A refused call's two parts, as the agent read them: the text (what and
/// what to do) and the error as JSON, which an argument fault has not.
fn refused(result: &Value) -> (&str, Option<Value>) {
    assert_eq!(result["isError"], true, "{result}");
    let content = result["content"].as_array().unwrap();
    let text = content[0]["text"].as_str().unwrap();
    let json = content
        .get(1)
        .map(|c| serde_json::from_str(c["text"].as_str().unwrap()).unwrap());
    (text, json)
}

/// The lake's feed: every event, as `lakelands feed` prints it.
fn feed(root: &str) -> Vec<Value> {
    ok(&["--root", root, "feed"])
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

// Since #349, the next step tells a caller that nothing was written and
// who can make it run; the walk's run read "bind it in the host's
// configuration".
const UNBOUND: &str = "error: nothing is bound for the verb executor `ledger`\n  \
                       what to do: nothing was written; this cannot run until the lake's \
                       operator binds it in the host's configuration\n";
const NOT_GRANTED: &str = "error: refused on `paid_on`: you hold no grant for this\n  \
                           what to do: act as a principal whose grants allow it, or ask \
                           the lake's owner for a grant\n";
const CORRECT: &str =
    "what to do: correct the input and run the command again; nothing was written\n";
const FIT: &str = "what to do: correct them against the tool's input schema and call it again\n";

#[test]
fn a_blind_agent_works_the_shop_through_mcp_alone() {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("walk-5");
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    let walk = base.join("walk");
    let w = show(&walk);

    // The lake: the shop with walk 5's verb, and the clerk.
    ok(&["init", &w]);
    ok(&["--root", &w, "lake", "new", "shop"]);
    let shop = walk.join("shop");
    copy_dir(&examples().join("shop"), &shop);
    fs::copy(
        examples().join("walk-5/verbs/cancel_order.yaml"),
        shop.join("verbs/cancel_order.yaml"),
    )
    .unwrap();
    assert_eq!(
        ok(&["--root", &w, "apply", &show(&shop)]),
        "applied 8 declaration(s), in force from position 11:\n  type `customer`\n  \
         type `order`\n  type `payment`\n  type `revenue_7d`\n  verb `cancel_order`\n  \
         verb `create_payment`\n  trigger `order_paid`\n  trigger `order_settled`\n"
    );
    for (file, through, n) in [("customers.ndjson", 14, 2), ("orders.ndjson", 18, 3)] {
        let data = show(&shop.join("data").join(file));
        assert_eq!(
            ok(&["--root", &w, "ingest", &data, "--source", "shop"]).trim_end(),
            format!("committed {n} record(s), 0 already in the lake, through position {through}")
        );
    }
    let config = walk.join("lakelands.json");
    let mut root: Value = serde_json::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
    let clerk: Value = serde_json::from_str(
        &fs::read_to_string(examples().join("walk-5/principal.json")).unwrap(),
    )
    .unwrap();
    root["principals"].as_array_mut().unwrap().push(clerk);
    fs::write(&config, serde_json::to_string_pretty(&root).unwrap()).unwrap();
    let clerk = ["--root", w.as_str(), "--as", "clerk"];

    // Run 1: the scenario, the agent on its own; its lease at 19.
    let r = mcp_results(
        &clerk,
        &[
            ("describe", json!({})),
            ("sys_aging_rule_find", json!({})),
            ("sys_setting_find", json!({})),
            ("customer_find", json!({"where": "name = 'Delta Metals'"})),
            ("order_find", json!({"with_meta": true})),
            (
                "order_upsert",
                json!({"key": "O-2", "fields": {"status": "shipped"}, "expected_version": 1,
                       "idempotency_key": "o2-shipped-20261007"}),
            ),
            (
                "order_upsert",
                json!({"key": "O-2", "fields": {"status": "paid"}, "expected_version": 2,
                       "idempotency_key": "o2-paid-20261007"}),
            ),
            ("payment_find", json!({"with_meta": true})),
            ("order_get", json!({"key": "O-2", "with_meta": true})),
            (
                "create_payment",
                json!({"payload": {"order_id": "O-2"}, "idempotency_key": "payment-O-2"}),
            ),
            (
                "cancel_order",
                json!({"key": "O-3", "payload": {}, "expected_version": 1,
                       "idempotency_key": "cancel-O-3"}),
            ),
            (
                "create_payment",
                json!({"payload": {"order_id": "O-1"}, "idempotency_key": "payment-O-1"}),
            ),
            (
                "revenue_7d_find",
                json!({"params": {"anchor": "2026-10-07T00:00:00Z"}}),
            ),
        ],
    );
    // What `describe` told the agent it may do.
    let d = answer(&r[0]);
    assert_eq!(d["position"], json!(19));
    assert_eq!(
        d["data"]["may"],
        json!({"ingest": false, "review": false, "schema": false})
    );
    let order = &d["data"]["types"][1];
    assert_eq!(order["name"], json!("order"));
    let access = |name: &str| {
        order["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == json!(name))
            .unwrap()["access"]
            .clone()
    };
    assert_eq!(access("status"), json!("write"));
    assert_eq!(access("paid_on"), json!("read"));
    assert_eq!(d["data"]["verbs"][0]["name"], json!("cancel_order"));
    assert_eq!(d["data"]["verbs"][0]["risk"], json!("medium"));
    assert_eq!(d["data"]["verbs"][1]["scope"], json!("type"));
    for i in [1, 2] {
        assert_eq!(
            answer(&r[i]),
            &json!({"data": {"rows": [], "shape": "records"}, "position": 19})
        );
    }
    assert_eq!(answer(&r[3])["data"]["rows"][0]["key"], json!("C-2"));
    let orders = &answer(&r[4])["data"]["rows"];
    assert_eq!(orders[1]["key"], json!("O-2"));
    assert_eq!(orders[1]["fields"]["customer_no"], json!("C-2"));
    assert_eq!(
        answer(&r[5]),
        &json!({"outcome": "applied", "position": 20, "version": 2})
    );
    // The write at 21 fired `order_settled` at 22, in its own append, and
    // its answer says so; the read answers at 22.
    assert_eq!(
        answer(&r[6]),
        &json!({"outcome": "applied", "position": 21, "version": 3, "through": 22,
                "fired": ["order_settled"]})
    );
    assert_eq!(
        answer(&r[7]),
        &json!({"data": {"rows": [], "shape": "records"}, "position": 22})
    );
    let o2 = answer(&r[8]);
    assert_eq!(o2["position"], json!(22));
    assert_eq!(o2["data"]["fields"]["status"], json!("paid"));
    assert_eq!(o2["data"]["meta"]["status"]["writer"], json!("agent:clerk"));
    assert_eq!(o2["data"]["meta"]["status"]["position"], json!(21));
    for i in [9, 11] {
        assert_eq!(
            refused(&r[i]),
            (
                UNBOUND,
                Some(json!({"error": "unbound", "seam": "the verb executor `ledger`"}))
            )
        );
    }
    let proposed = answer(&r[10]);
    assert_eq!(proposed["outcome"], json!("proposed"));
    assert_eq!(proposed["position"], json!(23));
    assert_eq!(
        proposed["why"],
        json!("`cancel_order` is declared medium risk; it runs when accepted")
    );
    assert_eq!(
        answer(&r[12]),
        &json!({"data": {"rows": [], "shape": "records"}, "position": 23})
    );
    let events = feed(&w);
    let fired = &events[22];
    assert_eq!(fired["cause"], json!(21));
    assert_eq!(fired["body"]["kind"], json!("trigger_fired"));
    assert_eq!(fired["body"]["trigger"], json!("order_settled"));
    assert_eq!(fired["known_at"], events[21]["known_at"], "one append");
    assert_eq!(events[23]["body"]["kind"], json!("write_proposed"));

    // Run 2: a colleague's faulty call first, then the agent's; lease at 24.
    let why = "Clerk asked to record that order O-2 was paid on 2026-10-06; direct write \
               refused (no grant on order.paid_on).";
    let r = mcp_results(
        &clerk,
        &[
            (
                "order_upsert",
                json!({"key": "O-1", "fields": {"state": "shipped"}}),
            ),
            (
                "order_upsert",
                json!({"key": "O-1", "fields": {"status": "shipped"}}),
            ),
            (
                "payment_upsert",
                json!({"fields": {"order_id": "O-2", "customer_no": "C-2", "amount": 80,
                                  "paid_on": "2026-10-06"}}),
            ),
            ("payment_find", json!({"limit": 20, "with_meta": true})),
            ("describe", json!({"type": "payment"})),
            (
                "payment_upsert",
                json!({"key": "O-2", "fields": {"order_id": "O-2", "customer_no": "C-2",
                                                "amount": "80.00",
                                                "paid_on": "2026-10-06T00:00:00Z"},
                       "expected_version": 0}),
            ),
            (
                "order_upsert",
                json!({"key": "O-2", "fields": {"paid_on": "2026-10-06"}}),
            ),
            (
                "propose",
                json!({"write": {"write": "upsert", "type": "order", "key": "O-2",
                                 "fields": {"paid_on": "2026-10-06T00:00:00Z"}},
                       "why": why}),
            ),
            (
                "propose",
                json!({"write": {"write": "upsert", "type": "order", "key": "O-2",
                                 "patch": {"paid_on": "2026-10-06T00:00:00Z"}},
                       "why": why}),
            ),
            (
                "order_upsert",
                json!({"key": "O-1", "fields": {"status": "on hold"}, "expected_version": 1}),
            ),
            ("order_get", json!({"key": "O-1", "with_meta": true})),
            (
                "order_upsert",
                json!({"key": "O-1", "fields": {"status": "on hold"}, "expected_version": 2}),
            ),
            (
                "create_payment",
                json!({"key": "O-1", "payload": {"order_id": "O-1"}}),
            ),
            (
                "create_payment",
                json!({"payload": {"order_id": "O-1"},
                       "idempotency_key": "create-payment-O-1-2026-10-07"}),
            ),
            ("revenue_7d_find", json!({})),
            (
                "revenue_7d_find",
                json!({"params": {"anchor": "2026-10-07"}}),
            ),
        ],
    );
    let invalid = |message: &str| json!({"error": "refused", "reason": {"message": message, "refusal": "invalid"}});
    assert_eq!(
        refused(&r[0]),
        (
            format!("error: refused on `state`: `order` has no field `state`\n  {CORRECT}")
                .as_str(),
            Some(json!({"error": "refused", "field": "state",
                        "reason": {"message": "`order` has no field `state`", "refusal": "invalid"}}))
        )
    );
    assert_eq!(
        answer(&r[1]),
        &json!({"outcome": "applied", "position": 25, "version": 2})
    );
    assert_eq!(
        refused(&r[2]),
        (
            format!(
                "error: the arguments do not fit `payment_upsert`: missing field `key`\n  {FIT}"
            )
            .as_str(),
            None
        )
    );
    assert_eq!(
        answer(&r[3]),
        &json!({"data": {"rows": [], "shape": "records"}, "position": 25})
    );
    let payment = &answer(&r[4])["data"];
    assert_eq!(payment["name"], json!("payment"));
    assert_eq!(
        payment["access"],
        json!({"field_meta": true, "searchable": false}),
        "a type without a key"
    );
    assert_eq!(
        answer(&r[5]),
        &json!({"outcome": "applied", "position": 26, "version": 1})
    );
    let not_granted = Some(json!({"error": "refused", "field": "paid_on",
                                  "reason": {"refusal": "not_granted"}}));
    assert_eq!(refused(&r[6]), (NOT_GRANTED, not_granted.clone()));
    assert_eq!(
        refused(&r[7]),
        (
            format!("error: the arguments do not fit `propose` at `write`: missing field `patch`\n  {FIT}")
                .as_str(),
            None
        )
    );
    assert_eq!(refused(&r[8]), (NOT_GRANTED, not_granted));
    assert_eq!(
        refused(&r[9]),
        (
            "error: the record changed: expected version 1, found 2\n  \
             what to do: read the record again and retry against its current version\n",
            Some(json!({"error": "conflict", "expected": 1, "actual": 2}))
        )
    );
    let o1 = answer(&r[10]);
    assert_eq!(o1["data"]["version"], json!(2));
    assert_eq!(o1["data"]["meta"]["status"]["position"], json!(25));
    assert_eq!(
        answer(&r[11]),
        &json!({"outcome": "applied", "position": 27, "version": 3})
    );
    let no_key =
        "verb `create_payment` is called on the type `payment`, not on a record: it takes no key";
    assert_eq!(
        refused(&r[12]),
        (
            format!("error: refused: {no_key}\n  {CORRECT}").as_str(),
            Some(invalid(no_key))
        )
    );
    assert_eq!(
        refused(&r[13]),
        (
            UNBOUND,
            Some(json!({"error": "unbound", "seam": "the verb executor `ledger`"}))
        )
    );
    let needs = "view `revenue_7d` needs parameter `anchor` (date_time)";
    assert_eq!(
        refused(&r[14]),
        (
            format!("error: refused: {needs}\n  {CORRECT}").as_str(),
            Some(invalid(needs))
        )
    );
    assert_eq!(
        answer(&r[15]),
        &json!({"data": {"rows": [{"fields": {"customer_no": "C-2", "payments": 1, "total": "80.00"},
                                   "key": "C-2", "type": "revenue_7d", "version": 1}],
                         "shape": "records"},
                "position": 27})
    );

    // Run 3: a value of the wrong kind; lease at 28.
    let r = mcp_results(
        &clerk,
        &[
            (
                "payment_upsert",
                json!({"key": "P-O-1", "fields": {"order_id": "O-1", "customer_no": "C-1",
                                                  "amount": 120, "paid_on": "2026-10-07"}}),
            ),
            (
                "payment_upsert",
                json!({"key": "P-O-1", "fields": {"order_id": "O-1", "customer_no": "C-1",
                                                  "amount": "120.00", "paid_on": "2026-10-07"}}),
            ),
            ("payment_get", json!({"key": "P-O-1"})),
        ],
    );
    let kind = "a decimal value must be a string of digits";
    assert_eq!(
        refused(&r[0]),
        (
            format!("error: refused on `amount`: {kind}\n  {CORRECT}").as_str(),
            Some(json!({"error": "refused", "field": "amount",
                        "reason": {"message": kind, "refusal": "constraint"}}))
        )
    );
    assert_eq!(
        answer(&r[1]),
        &json!({"outcome": "applied", "position": 29, "version": 1})
    );
    assert_eq!(
        answer(&r[2]),
        &json!({"data": {"fields": {"amount": "120.00", "customer_no": "C-1", "order_id": "O-1",
                                    "paid_on": "2026-10-07T00:00:00.000000Z"},
                         "key": "P-O-1", "type": "payment", "version": 1},
                "position": 29})
    );
}
