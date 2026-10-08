//! Walk 3's materials, kept from rotting: the commands
//! `docs/walks/walk-3.md` types over `examples/network/` give the page's
//! answers at the page's positions. A CRM lake is exported and imported
//! into a second root with the same feed; a ledger lake in the first root
//! mirrors the CRM's partners and says which state of the CRM each copy
//! reflects; the CRM answers degree and components over its links; and
//! the imported lake, under a ceiling of five records, refuses the
//! components of the whole type as too large.
//!
//! Each command runs the built `lakelands` binary in a process of its
//! own, as the walker runs it, since each process that writes takes the
//! lake's lease once and the positions count it.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

fn example() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/network")
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

/// One MCP session with `lakelands <args> serve --mcp`, as the page's
/// client makes it: initialize, then each `tools/call` in order; the
/// result of every answer.
fn mcp(args: &[&str], calls: &[(&str, Value)]) -> Vec<Value> {
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
                          "clientInfo": {"name": "walk_3", "version": "0"}}}),
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

/// The query over `supplies`, both ways, projected as `as`, from `start`.
fn measure(start: Value, r#as: &str) -> Value {
    json!({"start": start, "steps": [{"relation": "supplies", "walk": "both"}],
           "project": {"as": r#as}})
}

/// The rows of a measure's answer, each as its listed columns.
fn rows(answer: &Value, columns: &[&str]) -> Vec<Vec<Value>> {
    answer["structuredContent"]["data"]["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("an answer with rows: {answer}"))
        .iter()
        .map(|r| columns.iter().map(|c| r[*c].clone()).collect())
        .collect()
}

#[test]
fn the_walks_commands_move_mirror_and_measure_the_network() {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("walk-3");
    let _ = fs::remove_dir_all(&base);
    fs::create_dir_all(&base).unwrap();
    let walk = base.join("walk");
    let moved = base.join("moved");
    let export = base.join("crm-export");
    let (w, m) = (show(&walk), show(&moved));
    let crm = show(&example().join("crm"));
    let data = |name: &str| show(&example().join("crm/data").join(name));

    ok(&["init", &w]);
    ok(&["--root", &w, "lake", "new", "crm"]);
    assert_eq!(
        ok(&["--root", &w, "apply", &crm]),
        "applied 1 declaration(s), in force from position 5:\n  type `partner`\n"
    );
    assert_eq!(
        ok(&[
            "--root",
            &w,
            "ingest",
            &data("partners.ndjson"),
            "--source",
            "crm"
        ])
        .trim_end(),
        "committed 7 record(s), 0 already in the lake, through position 13"
    );
    assert_eq!(
        ok(&[
            "--root",
            &w,
            "ingest",
            &data("supplies.ndjson"),
            "--source",
            "crm"
        ])
        .trim_end(),
        "committed 4 record(s), 0 already in the lake, through position 18"
    );

    // 1. The lake moves: the export reads through 18 and appends nothing,
    // and the import makes the same lake, feed for feed.
    let exported = ok(&["--root", &w, "export", "--to", &show(&export)]);
    assert_eq!(
        exported,
        format!(
            "log/00000000000000000000.jsonl: positions 0..18\n\
             exported crm through position 18 to {}\n",
            show(&export)
        )
    );
    ok(&["init", &m]);
    ok(&["--root", &m, "lake", "new", "crm"]);
    let imported = ok(&["--root", &m, "import", &show(&export)]);
    let feed = ok(&["--root", &w, "feed"]);
    assert_eq!(
        ok(&["--root", &m, "feed"]),
        feed,
        "the imported lake's feed is the exported lake's"
    );
    let last: Value = serde_json::from_str(feed.lines().last().unwrap()).unwrap();
    assert_eq!(
        imported.trim_end(),
        format!(
            "imported lake `crm`: 19 event(s) from 1 file(s), head 18, hash {}",
            last["hash"].as_str().unwrap()
        ),
        "the import names the head and the hash of the feed's last line"
    );

    // 2. Two lakes in one root: the ledger mirrors the CRM's partners.
    ok(&["--root", &w, "lake", "new", "ledger"]);
    fs::copy(
        example().join("lakelands.json"),
        walk.join("lakelands.json"),
    )
    .unwrap();
    assert_eq!(
        ok(&[
            "--root",
            &w,
            "--lake",
            "ledger",
            "apply",
            &show(&example().join("ledger"))
        ]),
        "applied 1 declaration(s), in force from position 5:\n  type `partner`\n"
    );
    // The page holds `serve` up for two seconds; the test waits for the
    // snapshot's line in the mirror's journal instead, up to thirty.
    let mut serve = Command::new(env!("CARGO_BIN_EXE_lakelands"))
        .args(["--root", &w, "--lake", "ledger", "serve", "--mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the server starts");
    let journal = walk.join("mirrors/ledger/crm.ndjson");
    let until = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !fs::read_to_string(&journal).is_ok_and(|j| j.contains("\"snapshot\""))
        && std::time::Instant::now() < until
    {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    drop(serve.stdin.take());
    serve.wait().expect("the server ends with its input");
    let ledger: Vec<(u64, String, String)> =
        ok(&["--root", &w, "--lake", "ledger", "feed", "--from", "6"])
            .lines()
            .map(|l| {
                let e: Value = serde_json::from_str(l).unwrap();
                let b = &e["body"];
                (
                    e["position"].as_u64().unwrap(),
                    b["kind"].as_str().unwrap().to_owned(),
                    b["key"].as_str().unwrap_or_default().to_owned(),
                )
            })
            .collect();
    let mut expected = vec![(6, "lease_taken".to_owned(), String::new())];
    for (at, n) in (7..).zip(1..=7) {
        expected.push((at, "record_ingested".to_owned(), format!("P-{n}00")));
    }
    expected.push((14, "record_upserted".to_owned(), "crm:partner".to_owned()));
    assert_eq!(
        ledger, expected,
        "the serve's lease, the seven copies, the mirror's record"
    );

    let reads = mcp(
        &["--root", &w, "--lake", "ledger"],
        &[
            ("partner_get", json!({"key": "P-100", "with_meta": true})),
            ("sys_mirror_find", json!({})),
        ],
    );
    let copy = &reads[0]["structuredContent"];
    assert_eq!(copy["position"], json!(15));
    assert_eq!(
        copy["data"]["fields"],
        json!({"id": "P-100", "name": "Harbor Supplies"}),
        "the city stays home"
    );
    for field in ["id", "name"] {
        let meta = &copy["data"]["meta"][field];
        assert_eq!(
            [
                &meta["origin"],
                &meta["source"],
                &meta["writer"],
                &meta["position"],
                &meta["upstream"]
            ],
            [
                &json!("source"),
                &json!("mirror:crm:partner"),
                &json!("service:mirror"),
                &json!(7),
                &json!({"lake": "crm", "position": 18}),
            ],
            "the metadata of `{field}`"
        );
    }
    assert_eq!(
        reads[1]["structuredContent"]["data"]["rows"],
        json!([{"type": "sys.mirror", "key": "crm:partner", "version": 1,
                "fields": {"connection": "crm", "next": 19, "source_lake": "crm",
                           "source_type": "partner", "target_type": "partner"}}])
    );

    // 3. The measures, in the CRM, at its session's lease 19.
    let all = json!({"type": "partner"});
    let measured = mcp(
        &["--root", &w, "--lake", "crm"],
        &[
            ("query", measure(all.clone(), "degree")),
            ("query", measure(all.clone(), "components")),
        ],
    );
    assert_eq!(measured[0]["structuredContent"]["position"], json!(19));
    assert_eq!(
        rows(&measured[0], &["key", "degree"]),
        [
            ("P-100", 3),
            ("P-200", 1),
            ("P-300", 1),
            ("P-400", 1),
            ("P-500", 1),
            ("P-600", 1),
            ("P-700", 0)
        ]
        .map(|(k, d)| vec![json!(k), json!(d)])
    );
    assert_eq!(
        rows(&measured[1], &["component", "size", "key"]),
        [
            (0, 4, "P-100"),
            (0, 4, "P-200"),
            (0, 4, "P-300"),
            (0, 4, "P-400"),
            (1, 2, "P-500"),
            (1, 2, "P-600"),
            (2, 1, "P-700"),
        ]
        .map(|(c, s, k)| vec![json!(c), json!(s), json!(k)])
    );

    // 4. The same lake under a ceiling of five, in the second root.
    let location = moved.join("lakes/crm.json");
    let text = fs::read_to_string(&location).unwrap();
    assert!(text.contains("\"max_graph_nodes\": 100000"), "{text}");
    fs::write(
        &location,
        text.replace("\"max_graph_nodes\": 100000", "\"max_graph_nodes\": 5"),
    )
    .unwrap();
    let capped = mcp(
        &["--root", &m],
        &[
            (
                "query",
                measure(json!({"type": "partner", "key": "P-500"}), "components"),
            ),
            ("query", measure(all, "components")),
        ],
    );
    assert_eq!(capped[0]["structuredContent"]["position"], json!(19));
    assert_eq!(
        rows(&capped[0], &["component", "size", "key"]),
        [(0, 2, "P-500"), (0, 2, "P-600")].map(|(c, s, k)| vec![json!(c), json!(s), json!(k)])
    );
    assert_eq!(capped[1]["isError"], json!(true), "{}", capped[1]);
    assert_eq!(
        capped[1]["content"][0]["text"],
        json!(
            "error: refused: a graph measure may reach 5 records, no more\n  \
             what to do: start from fewer records, or raise the lake's `max_graph_nodes` in its location\n"
        )
    );
    assert_eq!(
        serde_json::from_str::<Value>(capped[1]["content"][1]["text"].as_str().unwrap()).unwrap(),
        json!({"error": "refused", "reason": {"max": 5, "refusal": "too_large"},
            "message": "refused: a graph measure may reach 5 records, no more"})
    );
}
