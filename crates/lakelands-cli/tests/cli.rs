//! Transcripts of the CLI against the in-memory lake: every byte of
//! what a command prints is pinned, with the root's path shown as
//! `<root>`.

use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use lakelands_api::location::Location;
use lakelands_api::{LakeStore, Result};
use lakelands_mock::MockLake;

fn block_on<F: Future>(f: F) -> F::Output {
    let mut f = pin!(f);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(out) = f.as_mut().poll(&mut cx) {
            return out;
        }
    }
}

/// A root in a fresh directory, with in-memory lakes that live as long
/// as the test.
struct Shell {
    root: PathBuf,
    lakes: Mutex<BTreeMap<String, MockLake>>,
}

struct Said {
    code: u8,
    out: String,
    err: String,
}

impl Shell {
    fn new(name: &str) -> Shell {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("cli")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        Shell {
            root,
            lakes: Mutex::new(BTreeMap::new()),
        }
    }

    fn open(&self, l: &Location) -> Result<Arc<dyn LakeStore>> {
        let mut lakes = self.lakes.lock().unwrap();
        let lake = lakes
            .entry(l.lake.to_string())
            .or_insert_with(|| MockLake::open(l));
        Ok(Arc::new(lake.clone()))
    }

    fn file(&self, name: &str, text: &str) -> String {
        fs::create_dir_all(&self.root).unwrap();
        let p = self.root.join(name);
        fs::write(&p, text).unwrap();
        p.display().to_string()
    }

    /// Run `lakelands --root <root> <args>`.
    fn run(&self, args: &[&str]) -> Said {
        let root = self.root.display().to_string();
        let mut argv = vec!["lakelands", "--root", root.as_str()];
        argv.extend_from_slice(args);
        self.raw(&argv)
    }

    fn raw(&self, argv: &[&str]) -> Said {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let open = |l: &Location| self.open(l);
        let code = block_on(lakelands_cli::run(
            argv.iter().copied(),
            &mut out,
            &mut err,
            &open,
        ));
        let root = self.root.display().to_string();
        let clean = |b: Vec<u8>| String::from_utf8(b).unwrap().replace(&root, "<root>");
        Said {
            code,
            out: clean(out),
            err: clean(err),
        }
    }

    fn lake(&self) {
        let root = self.root.display().to_string();
        assert_eq!(self.raw(&["lakelands", "init", &root]).code, 0);
        assert_eq!(self.run(&["lake", "new", "finance"]).code, 0);
    }

    /// Write a bundle directory from `(path, yaml)` pairs.
    fn bundle(&self, name: &str, files: &[(&str, &str)]) -> String {
        let dir = self.root.join(name);
        let _ = fs::remove_dir_all(&dir);
        for (path, yaml) in files {
            let p = dir.join(path);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, yaml).unwrap();
        }
        dir.display().to_string()
    }

    /// The mock's fixture ontology, as a bundle.
    fn fixture(&self) -> String {
        self.bundle("fixture", &FIXTURE)
    }
}

const PARTNER: &str = "name: partner\nfields:\n  name: { type: text, searchable: true }\n";

const FIXTURE: [(&str, &str); 4] = [
    ("types/partner.yaml", PARTNER),
    (
        "types/invoice.yaml",
        "name: invoice
description: A bill received.
fields:
  amount: { type: decimal, authority: source }
  status: { type: text, searchable: true }
  verdict: text
  reason: { type: text, depends_on: verdict, on_input_change: clear }
  note: { type: text, sensitive: true }
  secret: text
  partner: reference(partner)
relations:
  billed_by: { target: partner, via: partner, inverse: bills }
",
    ),
    (
        "verbs/approve.yaml",
        "name: approve\non: invoice\nrisk: low\nset:\n  status: \"'approved'\"\n",
    ),
    (
        "verbs/pay.yaml",
        "name: pay
on: invoice
risk: high
requires: \"status = 'approved'\"
payload:
  reference: text
set:
  status: \"'paid'\"
",
    ),
];

#[track_caller]
fn ok(s: &Said, out: &str) {
    assert_eq!((s.code, s.err.as_str()), (0, ""), "stdout: {}", s.out);
    assert_eq!(s.out, out);
}

#[track_caller]
fn fails(s: &Said, err: &str) {
    assert_eq!((s.code, s.out.as_str()), (1, ""));
    assert_eq!(s.err, err);
}

#[test]
fn init_creates_a_root_and_refuses_to_overwrite_it() {
    let sh = Shell::new("init");
    let root = sh.root.display().to_string();
    ok(
        &sh.raw(&["lakelands", "init", &root]),
        "created a Lakelands root in <root>\n  next: lakelands --root <root> lake new <id>\n",
    );
    let config = fs::read_to_string(sh.root.join("lakelands.json")).unwrap();
    assert_eq!(
        config,
        "{\n  \"lakelands\": 1,\n  \"owner\": \"owner\",\n  \"principals\": []\n}\n"
    );
    fails(
        &sh.raw(&["lakelands", "init", &root]),
        "error: <root> already holds a Lakelands root; nothing was changed\n  what to do: to start another root, name a directory without a lakelands.json\n",
    );
}

#[test]
fn init_takes_the_root_from_its_argument_or_from_root_and_the_argument_wins() {
    let sh = Shell::new("init-root");
    ok(
        &sh.run(&["init"]),
        "created a Lakelands root in <root>\n  next: lakelands --root <root> lake new <id>\n",
    );
    let other = sh.root.join("other").display().to_string();
    ok(
        &sh.run(&["init", &other]),
        "created a Lakelands root in <root>/other\n  next: lakelands --root <root>/other lake new <id>\n",
    );
    assert!(sh.root.join("other/lakelands.json").is_file());
}

#[test]
fn init_help_says_where_the_root_comes_from() {
    let sh = Shell::new("init-help");
    ok(
        &sh.raw(&["lakelands", "init", "-h"]),
        "Create a Lakelands root in a directory

Usage: lakelands init [DIR] [--owner <ID>]
       lakelands init --root <DIR> [--owner <ID>]

Arguments:
  [DIR]  The directory to hold the root; --root when left out

Options:
      --owner <ID>  The id recorded as the writer when you act as the owner [default: owner]
      --root <DIR>  The Lakelands root to work in [default: .]
      --as <NAME>   Act as this principal from the root's configuration instead of the owner
      --lake <ID>   The lake to work on; may be left out when the root holds only one
  -h, --help        Print help (see more with '--help')
",
    );
}

#[test]
fn lake_new_writes_a_location_and_refuses_to_overwrite_it() {
    let sh = Shell::new("lake-new");
    fails(
        &sh.run(&["lake", "new", "finance"]),
        "error: <root> is not a Lakelands root (it has no lakelands.json)\n  what to do: run `lakelands init <root>` first, or name the root with --root\n",
    );
    sh.lake();
    let location = fs::read_to_string(sh.root.join("lakes/finance.json")).unwrap();
    let location: Location = serde_json::from_str(&location).unwrap();
    assert_eq!(location, Location::sqlite("finance", "lakes/finance"));
    fails(
        &sh.run(&["lake", "new", "finance"]),
        "error: lake `finance` exists already in <root>; nothing was changed\n  what to do: choose another name, or keep working with the lake that is there\n",
    );
    fails(
        &sh.run(&["lake", "new", "Fin ance"]),
        "error: `Fin ance` cannot name a lake\n  what to do: use up to 63 lower-case letters, digits, `-` and `_`, starting with a letter or a digit\n",
    );
    ok(
        &sh.run(&["lake", "new", "scratch", "--engine", "memory"]),
        "created lake `scratch` (engine memory) in lakes/scratch.json\n",
    );
    fails(
        &sh.run(&["rebuild"]),
        "error: <root> holds several lakes\n  what to do: name one with --lake: finance, scratch\n",
    );
}

#[test]
fn apply_declares_a_bundle_in_the_loader_s_order_at_each_position() {
    let sh = Shell::new("apply");
    sh.lake();
    let fixture = sh.fixture();
    ok(
        &sh.run(&["apply", &fixture]),
        "declared type `invoice` at position 1\n\
         declared type `partner` at position 2\n\
         declared verb `approve` at position 3\n\
         declared verb `pay` at position 4\n",
    );
}

#[test]
fn apply_refuses_a_broken_bundle_whole_naming_every_problem() {
    let sh = Shell::new("apply-broken");
    sh.lake();
    let broken = sh.bundle(
        "broken",
        &[
            (
                "types/partner.yaml",
                "name: partner\nfields:\n  name: txt\n",
            ),
            (
                "verbs/pay.yaml",
                "name: pay\non: invoice\nrisk: high\nset: {}\n",
            ),
        ],
    );
    let s = sh.run(&["apply", &broken]);
    assert_eq!((s.code, s.out.as_str()), (1, ""));
    let lines: Vec<&str> = s.err.lines().collect();
    assert_eq!(lines.len(), 3, "{}", s.err);
    assert!(
        lines[0].starts_with("error: types/partner.yaml: "),
        "{}",
        s.err
    );
    assert!(lines[1].starts_with("error: verbs/pay.yaml: "), "{}", s.err);
    assert_eq!(
        lines[2],
        "  what to do: correct the bundle and apply it again; nothing was applied"
    );
    ok(&sh.run(&["feed", "--from", "1"]), "");
    let file = sh.file("decls.json", "[]");
    fails(
        &sh.run(&["apply", &file]),
        "error: <root>/decls.json is not a bundle directory\n  what to do: name the directory that holds the bundle's types/, verbs/ and other lanes\n",
    );
}

#[test]
fn apply_stops_at_the_first_refused_declaration_and_says_what_stays() {
    let sh = Shell::new("apply-refused");
    sh.lake();
    sh.run(&["apply", &sh.fixture()]);
    let mut reshaped = FIXTURE;
    reshaped[0].1 = "name: partner\nfields:\n  name: text\n  city: text\n";
    let again = sh.bundle("reshaped", &reshaped);
    let s = sh.run(&["apply", &again]);
    assert_eq!(s.out, "declared type `invoice` at position 5\n");
    assert_eq!(
        s.err,
        "error: declaration 2 of 4 (type `partner`): refused: this declaration reshapes the records of `partner`; change it with `migrate`\n  what to do: correct the input and run the command again; the 1 declaration(s) before it stay applied\n"
    );
}

#[test]
fn ingest_prints_one_line_per_commit_window_and_skips_what_it_has() {
    let sh = Shell::new("ingest");
    sh.lake();
    sh.run(&["apply", &sh.fixture()]);
    let rows: String = (1..=40)
        .map(|i| format!("{{\"type\": \"invoice\", \"key\": \"INV-{i}\", \"fields\": {{\"amount\": \"{i}.00\"}}}}\n"))
        .collect();
    let file = sh.file("bank.ndjson", &rows);
    ok(
        &sh.run(&["ingest", &file, "--source", "bank"]),
        "committed 32 record(s), 0 already in the lake, through position 36\n\
         committed 8 record(s), 0 already in the lake, through position 44\n",
    );
    ok(
        &sh.run(&["ingest", &file, "--source", "bank"]),
        "committed 0 record(s), 32 already in the lake, through position 44\n\
         committed 0 record(s), 8 already in the lake, through position 44\n",
    );
    let broken = sh.file(
        "broken.ndjson",
        "{\"type\": \"invoice\", \"key\": \"A\", \"fields\": {}}\nnot json\n",
    );
    let s = sh.run(&["ingest", &broken, "--source", "bank"]);
    assert_eq!(s.code, 1);
    assert!(
        s.err
            .starts_with("error: line 2 of <root>/broken.ndjson is not a source record: "),
        "{}",
        s.err
    );
    fails(
        &sh.run(&["ingest", &file, "--source", "bank", "--as", "clerk"]),
        "error: no principal named `clerk` may address lake `finance`\n  what to do: add an entry for it to lakelands.json, or leave out --as to act as the owner\n",
    );
}

#[test]
fn ingest_reads_csv_through_a_column_map() {
    let sh = Shell::new("ingest-csv");
    sh.lake();
    sh.run(&["apply", &sh.fixture()]);
    let csv = sh.file(
        "bank.csv",
        "Invoice,Amount,Seen\r\nINV-1,120.50,2026-09-24\r\n\"INV-2\",\"99.00\",\nINV-3,,2026-09-25T10:00:00Z\n",
    );
    let map = sh.file(
        "bank.map.json",
        r#"{"type": "invoice", "key": "Invoice", "fields": {"Amount": "amount"}, "seen_at": "Seen"}"#,
    );
    ok(
        &sh.run(&["ingest", &csv, "--source", "bank", "--map", &map]),
        "committed 3 record(s), 0 already in the lake, through position 7\n",
    );
    let feed = sh.run(&["feed", "--from", "5", "--kind", "record_ingested"]);
    let events: Vec<serde_json::Value> = feed
        .out
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["body"]["patch"]["amount"], "120.50");
    assert_eq!(events[0]["seen_at"], "2026-09-24T00:00:00.000000Z");
    assert_eq!(events[1]["seen_at"], serde_json::Value::Null);
    assert_eq!(events[2]["body"]["patch"], serde_json::json!({}));
    let wrong = sh.file(
        "wrong.map.json",
        r#"{"type": "invoice", "key": "Number", "fields": {}}"#,
    );
    fails(
        &sh.run(&["ingest", &csv, "--source", "bank", "--map", &wrong]),
        "error: <root>/bank.csv has no column `Number`, which the map names\n  what to do: correct the file and run the command again; nothing was ingested\n",
    );
}

#[test]
fn feed_prints_the_log_in_canonical_form_from_a_position() {
    let sh = Shell::new("feed");
    sh.lake();
    ok(
        &sh.run(&["feed"]),
        "{\"body\":{\"format\":1,\"kind\":\"lake_created\"},\"context\":{\"depth\":0,\"origin\":\"direct\"},\"fence\":1,\"hash\":\"0123234c0bc2bbb6102e9d9cfc2f3f57c478a40bf5c3337ce80b7e9414d9bf8d\",\"id\":\"00000000000000000000000000000000\",\"known_at\":\"2026-01-01T00:00:00.000000Z\",\"lake\":\"finance\",\"origin\":\"system\",\"position\":0,\"prev\":\"0000000000000000000000000000000000000000000000000000000000000000\",\"v\":1,\"writer\":\"system:lakelands\"}\n",
    );
    sh.run(&["apply", &sh.fixture()]);
    let s = sh.run(&["feed", "--from", "3"]);
    assert_eq!(s.code, 0);
    let positions: Vec<u64> = s
        .out
        .lines()
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            v["position"].as_u64().unwrap()
        })
        .collect();
    assert_eq!(positions, [3, 4]);
    ok(&sh.run(&["feed", "--from", "9"]), "");
}

#[test]
fn rebuild_prints_the_position_it_rebuilt_through() {
    let sh = Shell::new("rebuild");
    sh.lake();
    sh.run(&["apply", &sh.fixture()]);
    ok(
        &sh.run(&["rebuild"]),
        "rebuilt the projection from the log through position 4\n",
    );
}

#[test]
fn serve_says_there_is_no_server_yet() {
    let sh = Shell::new("serve");
    sh.lake();
    fails(
        &sh.run(&["serve", "--mcp"]),
        "error: this build of lakelands has no MCP server yet; nothing was started\n  what to do: use the library in-process until the server door is built\n",
    );
    fails(
        &sh.run(&["serve"]),
        "error: nothing to serve was named\n  what to do: pass --mcp to serve the lake over the Model Context Protocol\n",
    );
}

#[test]
fn a_principal_from_the_configuration_acts_under_its_grants() {
    let sh = Shell::new("as");
    sh.lake();
    sh.run(&["apply", &sh.fixture()]);
    let config = r#"{"lakelands": 1, "owner": "owner", "principals": [
        {"name": "reader", "kind": "agent", "id": "reader", "grants": {"default": "read"}}]}"#;
    fs::write(sh.root.join("lakelands.json"), config).unwrap();
    let file = sh.file(
        "one.ndjson",
        "{\"type\": \"invoice\", \"key\": \"A\", \"fields\": {\"amount\": \"1.00\"}}\n",
    );
    fails(
        &sh.run(&["ingest", &file, "--source", "bank", "--as", "reader"]),
        "error: refused: you hold no grant for this\n  what to do: act as a principal whose grants allow it (--as), or ask the lake's owner for a grant\n",
    );
}

#[test]
fn the_built_binary_opens_no_lake_until_the_store_exists() {
    let sh = Shell::new("no-store");
    sh.lake();
    let root = sh.root.display().to_string();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = block_on(lakelands_cli::run(
        ["lakelands", "--root", root.as_str(), "rebuild"],
        &mut out,
        &mut err,
        &lakelands_cli::open_store,
    ));
    assert_eq!(code, 1);
    assert_eq!(
        String::from_utf8(err).unwrap(),
        "error: the lake is unavailable: this build of lakelands has no storage engine yet, so lake `finance` cannot be opened\n  what to do: wait and retry, or check that no other process holds the lake\n"
    );
}

#[test]
fn help_is_plain_and_a_wrong_command_line_exits_2() {
    let sh = Shell::new("help");
    let s = sh.raw(&["lakelands", "--help"]);
    assert_eq!(s.code, 0);
    for line in [
        "init ", "lake ", "apply ", "ingest ", "feed ", "rebuild ", "serve ",
    ] {
        assert!(s.out.contains(line), "{line} missing from:\n{}", s.out);
    }
    let s = sh.raw(&["lakelands", "ingest"]);
    assert_eq!(s.code, 2);
    assert!(s.err.contains("--source"), "{}", s.err);
}
