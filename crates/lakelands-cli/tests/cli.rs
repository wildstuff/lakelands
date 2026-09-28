//! Transcripts of the CLI: every byte of what a command prints is pinned,
//! with the root's path shown as `<root>`. Each test runs unchanged on
//! the mock and on the real store over both engines: that is how the mock
//! is proven honest.
//!
//! A refusal's sentence is not part of the contract; its kind and field
//! are. So a refusal is checked by the CLI's framing lines and the names
//! it must mention, never by the sentence the implementation words.

use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use lakelands_api::LakeStore;
use lakelands_api::engine::OpenMode;
use lakelands_api::location::{EngineConfig, Location};
use lakelands_api::seam::Seams;
use lakelands_cli::Opening;
use lakelands_mock::MockLake;

fn block_on<F: Future>(f: F) -> F::Output {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("a runtime")
        })
        .block_on(f)
}

/// Where the shell's lakes are kept.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Backend {
    /// The mock, whatever engine a location names.
    Mock,
    /// The real store, over the engine each location names.
    Store,
    /// The real store, with every lake kept in memory.
    StoreInMemory,
}

const BACKENDS: [Backend; 3] = [Backend::Mock, Backend::Store, Backend::StoreInMemory];

/// A root in a fresh directory, with lakes that live as long as the test,
/// kept on one backend.
struct Shell {
    root: PathBuf,
    backend: Backend,
    lakes: Arc<Mutex<BTreeMap<String, Arc<dyn LakeStore>>>>,
}

struct Said {
    code: u8,
    out: String,
    err: String,
}

impl Shell {
    fn on(name: &str, backend: Backend) -> Shell {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("cli")
            .join(format!("{backend:?}").to_lowercase())
            .join(name);
        let _ = fs::remove_dir_all(&root);
        Shell {
            root,
            backend,
            lakes: Arc::default(),
        }
    }

    /// Open a lake once per shell, as the binary opens it once per process.
    fn open(&self, l: Location, mode: OpenMode, host: Seams) -> Opening {
        let (lakes, backend) = (self.lakes.clone(), self.backend);
        Box::pin(async move {
            let name = l.lake.to_string();
            if let Some(store) = lakes.lock().unwrap().get(&name) {
                return Ok(store.clone());
            }
            let store: Arc<dyn LakeStore> = match backend {
                Backend::Mock => Arc::new(MockLake::open(&l)),
                Backend::Store | Backend::StoreInMemory => {
                    let mut l = l;
                    if backend == Backend::StoreInMemory {
                        l.engine = EngineConfig::Memory;
                    }
                    let seams = Seams {
                        engines: vec![
                            Arc::new(lakelands_engine_mem::MemoryOpener::new()),
                            Arc::new(lakelands_engine_sqlite::SqliteOpener),
                        ],
                        ..host
                    };
                    let mode = if mode == OpenMode::Read {
                        OpenMode::Read
                    } else {
                        OpenMode::Create
                    };
                    lakelands_store::open(l, seams, mode).await?
                }
            };
            lakes.lock().unwrap().insert(name, store.clone());
            Ok(store)
        })
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
        let open = |l: Location, m: OpenMode, s: Seams| self.open(l, m, s);
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

/// An event line with the values the lake's clock, its hash chain and its
/// lease holder make shown as placeholders.
fn clocked(line: &str) -> String {
    let mut v: serde_json::Value = serde_json::from_str(line).expect("a JSON line");
    for (member, shown) in [
        ("known_at", "{known_at}"),
        ("id", "{id}"),
        ("hash", "{hash}"),
        ("prev", "{prev}"),
    ] {
        v[member] = serde_json::Value::String(shown.into());
    }
    if v["body"]["kind"] == "lease_taken" {
        v["body"]["holder"] = serde_json::Value::String("{holder}".into());
    }
    serde_json::to_string(&v).unwrap()
}

#[track_caller]
fn ok(s: &Said, out: &str) {
    assert_eq!((s.code, s.err.as_str()), (0, ""), "stdout: {}", s.out);
    assert_eq!(s.out, out);
}

/// A refusal of `apply`: the CLI's own framing lines, and every name the
/// refusal must mention. A refusal's sentence is not part of the contract;
/// its kind and field are, so it is never pinned here.
#[track_caller]
fn refused(s: &Said, names: &[&str]) {
    assert_eq!((s.code, s.out.as_str()), (1, ""));
    assert!(s.err.starts_with("error: refused: "), "{}", s.err);
    assert!(
        s.err.ends_with(
            "  what to do: correct the bundle and apply it again; nothing was applied\n"
        ),
        "{}",
        s.err
    );
    for name in names {
        assert!(
            s.err.contains(&format!("`{name}`")) || s.err.contains(&format!(" {name}")),
            "{name}: {}",
            s.err
        );
    }
}

#[track_caller]
fn fails(s: &Said, err: &str) {
    assert_eq!((s.code, s.out.as_str()), (1, ""));
    assert_eq!(s.err, err);
}

#[test]
fn init_creates_a_root_and_refuses_to_overwrite_it() {
    for backend in BACKENDS {
        let sh = Shell::on("init", backend);
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
}

#[test]
fn init_takes_the_root_from_its_argument_or_from_root_and_the_argument_wins() {
    for backend in BACKENDS {
        let sh = Shell::on("init-root", backend);
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
}

#[test]
fn init_help_says_where_the_root_comes_from() {
    for backend in BACKENDS {
        let sh = Shell::on("init-help", backend);
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
}

#[test]
fn lake_new_writes_a_location_and_refuses_to_overwrite_it() {
    for backend in BACKENDS {
        let sh = Shell::on("lake-new", backend);
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
}

#[test]
fn apply_applies_a_bundle_as_one_set_at_one_position() {
    for backend in BACKENDS {
        let sh = Shell::on("apply", backend);
        sh.lake();
        let fixture = sh.fixture();
        // `invoice` refers to `partner`, which the loader orders after it:
        // the set is checked as one, so the forward reference holds.
        ok(
            &sh.run(&["apply", &fixture]),
            "applied 4 declaration(s), in force from position 7:\n  type `invoice`\n  type `partner`\n  verb `approve`\n  verb `pay`\n",
        );
    }
}

#[test]
fn apply_refuses_a_broken_bundle_whole_naming_every_problem() {
    for backend in BACKENDS {
        let sh = Shell::on("apply-broken", backend);
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
        ok(&sh.run(&["feed", "--from", "4"]), "");
        let file = sh.file("decls.json", "[]");
        fails(
            &sh.run(&["apply", &file]),
            "error: <root>/decls.json is not a bundle directory\n  what to do: name the directory that holds the bundle's types/, verbs/ and other lanes\n",
        );
    }
}

#[test]
fn apply_refuses_a_set_the_lake_refuses_whole_naming_every_problem() {
    for backend in BACKENDS {
        let sh = Shell::on("apply-refused", backend);
        sh.lake();
        sh.run(&["apply", &sh.fixture()]);
        // An added optional field is a declaration, not a reshape (027).
        let mut grown = FIXTURE;
        grown[0].1 =
            "name: partner\nfields:\n  name: { type: text, searchable: true }\n  city: text\n";
        let grown = sh.bundle("grown", &grown);
        let s = sh.run(&["apply", &grown]);
        assert_eq!((s.code, s.err.as_str()), (0, ""), "{}", s.out);
        // One reshaped type: its own refusal, and nothing of the set lands.
        let mut reshaped = FIXTURE;
        reshaped[0].1 = "name: partner\nfields:\n  name: number\n";
        let again = sh.bundle("reshaped", &reshaped);
        let before = sh.run(&["feed"]).out;
        refused(&sh.run(&["apply", &again]), &["partner"]);
        ok(&sh.run(&["feed"]), &before);
        // Two reshaped types: one refusal names both.
        reshaped[1].1 = "name: invoice\nfields:\n  amount: { type: decimal, authority: source }\n";
        let twice = sh.bundle("reshaped-twice", &reshaped);
        refused(&sh.run(&["apply", &twice]), &["invoice", "partner"]);
    }
}

#[test]
fn ingest_prints_one_line_per_commit_window_and_skips_what_it_has() {
    for backend in BACKENDS {
        let sh = Shell::on("ingest", backend);
        sh.lake();
        sh.run(&["apply", &sh.fixture()]);
        let rows: String = (1..=40)
        .map(|i| format!("{{\"type\": \"invoice\", \"key\": \"INV-{i}\", \"fields\": {{\"amount\": \"{i}.00\"}}}}\n"))
        .collect();
        let file = sh.file("bank.ndjson", &rows);
        ok(
            &sh.run(&["ingest", &file, "--source", "bank"]),
            "committed 32 record(s), 0 already in the lake, through position 39\n\
         committed 8 record(s), 0 already in the lake, through position 47\n",
        );
        ok(
            &sh.run(&["ingest", &file, "--source", "bank"]),
            "committed 0 record(s), 32 already in the lake, through position 47\n\
         committed 0 record(s), 8 already in the lake, through position 47\n",
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
}

#[test]
fn ingest_reads_csv_through_a_column_map() {
    for backend in BACKENDS {
        let sh = Shell::on("ingest-csv", backend);
        sh.lake();
        sh.run(&["apply", &sh.fixture()]);
        let csv = sh.file(
        "bank.csv",
        "Invoice,Amount,Seen\r\nINV-1,120.50,2026-09-24\r\n\"INV-2\",\"99.00\",\nINV-3,1.00,2026-09-25T10:00:00Z\n",
    );
        let map = sh.file(
        "bank.map.json",
        r#"{"type": "invoice", "key": "Invoice", "fields": {"Amount": "amount"}, "seen_at": "Seen"}"#,
    );
        ok(
            &sh.run(&["ingest", &csv, "--source", "bank", "--map", &map]),
            "committed 3 record(s), 0 already in the lake, through position 10\n",
        );
        let feed = sh.run(&["feed", "--from", "7", "--kind", "record_ingested"]);
        let events: Vec<serde_json::Value> = feed
            .out
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0]["body"]["patch"]["amount"], "120.50");
        assert_eq!(events[0]["seen_at"], "2026-09-24T00:00:00.000000Z");
        assert_eq!(events[1]["seen_at"], serde_json::Value::Null);
        assert_eq!(events[2]["body"]["patch"]["amount"], "1.00");
        // An empty cell is absent, so a row of empty cells writes nothing, and
        // an empty patch is refused.
        let empty = sh.file("empty.csv", "Invoice,Amount,Seen\nINV-4,,\n");
        // Refused as invalid: the CLI's framing, never the sentence.
        let s = sh.run(&["ingest", &empty, "--source", "bank", "--map", &map]);
        assert_eq!((s.code, s.out.as_str()), (1, ""));
        assert!(
            s.err.starts_with("error: refused: ")
                && s.err.ends_with(
                    "  what to do: correct the input and run the command again; nothing was written\n"
                ),
            "{}",
            s.err
        );
        let wrong = sh.file(
            "wrong.map.json",
            r#"{"type": "invoice", "key": "Number", "fields": {}}"#,
        );
        fails(
            &sh.run(&["ingest", &csv, "--source", "bank", "--map", &wrong]),
            "error: <root>/bank.csv has no column `Number`, which the map names\n  what to do: correct the file and run the command again; nothing was ingested\n",
        );
    }
}

#[test]
fn feed_prints_the_log_in_canonical_form_from_a_position() {
    for backend in BACKENDS {
        let sh = Shell::on("feed", backend);
        sh.lake();
        // A lake begins with its creation, the creator's lease, the settings
        // type and the aging rule type. Every line is an event's canonical bytes; what the lake's clock
        // and the hash chain make (`known_at`, `id`, `hash`, `prev`) is shown
        // as a placeholder, since the mock keeps a clock of its own.
        let s = sh.run(&["feed"]);
        assert_eq!((s.code, s.err.as_str()), (0, ""));
        let lines: Vec<String> = s.out.lines().map(clocked).collect();
        assert_eq!(
            lines[..2],
            [
                "{\"body\":{\"format\":1,\"kind\":\"lake_created\"},\"context\":{\"depth\":0,\"origin\":\"direct\"},\"fence\":0,\"hash\":\"{hash}\",\"id\":\"{id}\",\"known_at\":\"{known_at}\",\"lake\":\"finance\",\"origin\":\"system\",\"position\":0,\"prev\":\"{prev}\",\"v\":1,\"writer\":\"system:lakelands\"}",
                "{\"body\":{\"fence\":1,\"holder\":\"{holder}\",\"kind\":\"lease_taken\"},\"context\":{\"depth\":0,\"origin\":\"direct\"},\"fence\":1,\"hash\":\"{hash}\",\"id\":\"{id}\",\"known_at\":\"{known_at}\",\"lake\":\"finance\",\"origin\":\"system\",\"position\":1,\"prev\":\"{prev}\",\"v\":1,\"writer\":\"system:lakelands\"}",
            ],
            "{}",
            s.out
        );
        assert!(
            lines[2].contains("\"name\":\"sys.setting\"") && lines[2].contains("\"position\":2")
        );
        assert!(
            lines[3].contains("\"name\":\"sys.aging_rule\"") && lines[3].contains("\"position\":3")
        );
        sh.run(&["apply", &sh.fixture()]);
        let s = sh.run(&["feed", "--from", "4"]);
        assert_eq!(s.code, 0);
        let positions: Vec<u64> = s
            .out
            .lines()
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                v["position"].as_u64().unwrap()
            })
            .collect();
        assert_eq!(positions, [4, 5, 6, 7]);
        ok(&sh.run(&["feed", "--from", "12"]), "");
    }
}

#[test]
fn apply_writes_aging_rules_after_the_declarations_and_age_runs_them() {
    const STALE: (&str, &str) = (
        "aging_rules/stale.yaml",
        "name: stale\non: invoice\nfield: status\nfrom: approved\nto: stale\nwhen: \"true\"\n",
    );
    for backend in BACKENDS {
        let sh = Shell::on("age", backend);
        sh.lake();
        let mut files = FIXTURE.to_vec();
        files.push(STALE);
        let dir = sh.bundle("aged", &files);
        ok(
            &sh.run(&["apply", &dir]),
            "applied 4 declaration(s), in force from position 7:\n  type `invoice`\n  type `partner`\n  verb `approve`\n  verb `pay`\n  aging rule `stale` written at position 8\n",
        );
        // The rule as stored equals the file's: nothing is written.
        let rules = sh.bundle("rules", &[STALE]);
        ok(
            &sh.run(&["apply", &rules]),
            "  aging rule `stale` unchanged\n",
        );
        // Compared as the lake stores them: `1.0` is the stored `1`.
        let level = (
            "aging_rules/level.yaml",
            "name: level\non: reminder\nfield: level\nfrom: 1.0\nto: 2.0\nwhen: \"true\"\n",
        );
        let reminder = (
            "types/reminder.yaml",
            "name: reminder\nfields:\n  level: number\n",
        );
        let first = sh.run(&["apply", &sh.bundle("levels", &[reminder, level])]);
        assert_eq!(first.code, 0, "{}", first.err);
        assert!(
            first.out.contains("aging rule `level` written"),
            "{}",
            first.out
        );
        ok(
            &sh.run(&["apply", &sh.bundle("level-again", &[level])]),
            "  aging rule `level` unchanged\n",
        );
        ok(
            &sh.run(&["age", "--today", "2026-03-01"]),
            "level: moved 0, 0 guard(s) failed\nstale: moved 0, 0 guard(s) failed\naged 2026-03-01 through position 10\n",
        );
        fails(
            &sh.run(&["age", "--today", "+026-03-01"]),
            "error: `+026-03-01` is not a date\n  what to do: give --today as a date, YYYY-MM-DD\n",
        );
        fails(
            &sh.run(&["age", "--today", "2026-3-1"]),
            "error: `2026-3-1` is not a date\n  what to do: give --today as a date, YYYY-MM-DD\n",
        );
    }
}

/// What `apply`, `age` and `purge` print that the store's checks cannot
/// hold: a rule field the file dropped is cleared, a skipped rule's line,
/// and a type the purge did not search (lakelands#116, items 4, 8, 12).
#[test]
fn apply_clears_a_dropped_rule_field_and_age_and_purge_print_what_they_left() {
    const TICKET: (&str, &str) = (
        "types/ticket.yaml",
        "name: ticket\nfields:\n  state: text\n",
    );
    const NOTED: (&str, &str) = (
        "aging_rules/stale.yaml",
        "name: stale\non: ticket\nfield: state\nfrom: new\nto: stale\nwhen: \"true\"\ndescription: tickets left new\n",
    );
    const PLAIN: (&str, &str) = (
        "aging_rules/stale.yaml",
        "name: stale\non: ticket\nfield: state\nfrom: new\nto: stale\nwhen: \"true\"\n",
    );
    for backend in BACKENDS {
        let sh = Shell::on("rule-fields", backend);
        sh.lake();
        let first = sh.run(&["apply", &sh.bundle("noted", &[TICKET, NOTED])]);
        assert_eq!(first.code, 0, "{backend:?}: {}", first.err);
        // The file no longer carries `description`: the stored rule loses it.
        let again = sh.run(&["apply", &sh.bundle("plain", &[PLAIN])]);
        assert!(
            again.code == 0 && again.out.contains("aging rule `stale` written"),
            "{backend:?}: {} {}",
            again.out,
            again.err
        );
        let feed = sh.run(&["feed", "--kind", "record_upserted"]);
        let last: serde_json::Value = serde_json::from_str(
            feed.out
                .lines()
                .last()
                .expect("the rule's writes are in the feed"),
        )
        .unwrap();
        assert_eq!(
            last["body"]["patch"]["description"],
            serde_json::Value::Null,
            "{backend:?}: re-applying without `description` clears it: {last}"
        );
        assert!(
            last["body"]["patch"]
                .as_object()
                .is_some_and(|p| p.contains_key("description")),
            "{backend:?}: the clearing write names `description`: {last}"
        );

        // `state` comes from a source now: the rule no longer fits.
        let sourced = (
            "types/ticket.yaml",
            "name: ticket\nfields:\n  state: { type: text, authority: source }\n",
        );
        let redeclared = sh.run(&["apply", &sh.bundle("sourced", &[sourced])]);
        assert_eq!(redeclared.code, 0, "{backend:?}: {}", redeclared.err);
        let aged = sh.run(&["age", "--today", "2026-03-01"]);
        assert!(
            aged.code == 0
                && aged.out.lines().any(|l| l.starts_with("stale: skipped, "))
                && aged
                    .out
                    .lines()
                    .last()
                    .is_some_and(|l| l.starts_with("aged 2026-03-01 through position ")),
            "{backend:?}: a skipped rule prints `<rule>: skipped, <why>`: {} {}",
            aged.out,
            aged.err
        );

        // A type that keeps no per-field provenance is named, not searched.
        let ledger = (
            "types/ledger.yaml",
            "name: ledger\nfield_meta: false\nfields:\n  sum: { type: number, authority: source }\n",
        );
        let applied = sh.run(&["apply", &sh.bundle("ledger", &[ledger])]);
        assert_eq!(applied.code, 0, "{backend:?}: {}", applied.err);
        let rows = sh.file(
            "ledger.ndjson",
            "{\"type\": \"ledger\", \"key\": \"l1\", \"fields\": {\"sum\": 1}}\n",
        );
        let fed = sh.run(&["ingest", &rows, "--source", "books"]);
        assert_eq!(fed.code, 0, "{backend:?}: {}", fed.err);
        ok(
            &sh.run(&["purge", "--source", "books"]),
            "ledger: not searched, it keeps no per-field provenance\nnothing to retire for books\n",
        );
    }
}

#[test]
fn purge_retires_what_a_source_brought_in_and_prints_its_position() {
    for backend in BACKENDS {
        let sh = Shell::on("purge", backend);
        sh.lake();
        sh.run(&["apply", &sh.fixture()]);
        let csv = sh.file("bank.csv", "Invoice,Amount\nINV-1,1.00\nINV-2,2.00\n");
        let map = sh.file(
            "bank.map.json",
            r#"{"type": "invoice", "key": "Invoice", "fields": {"Amount": "amount"}}"#,
        );
        sh.run(&["ingest", &csv, "--source", "bank", "--map", &map]);
        ok(
            &sh.run(&["purge", "--source", "bank"]),
            "invoice: retired 2\npurged bank through position 11\n",
        );
        // Nothing left to retire: nothing is appended, and it says so.
        ok(
            &sh.run(&["purge", "--source", "bank"]),
            "nothing to retire for bank\n",
        );
        let empty = sh.run(&["purge", "--source", ""]);
        assert_eq!((empty.code, empty.out.as_str()), (2, ""), "{}", empty.err);
    }
}

#[test]
fn rebuild_prints_the_position_it_rebuilt_through() {
    for backend in BACKENDS {
        let sh = Shell::on("rebuild", backend);
        sh.lake();
        sh.run(&["apply", &sh.fixture()]);
        ok(
            &sh.run(&["rebuild"]),
            "rebuilt the projection from the log through position 7\n",
        );
    }
}

#[test]
fn compact_prints_what_the_engine_measured_or_that_it_keeps_nothing() {
    for backend in [Backend::Store, Backend::StoreInMemory] {
        let sh = Shell::on("compact", backend);
        sh.lake();
        sh.run(&["apply", &sh.fixture()]);
        sh.run(&["rebuild"]);
        let run = sh.run(&["compact"]);
        assert_eq!(run.code, 0, "{}", run.err);
        if backend == Backend::StoreInMemory {
            assert_eq!(
                run.out,
                "nothing to reclaim in finance: its engine keeps no storage to give back\n"
            );
            continue;
        }
        // The sizes are SQLite's own measure; the line and the head are
        // what is pinned.
        let line = run
            .out
            .strip_prefix("compacted finance: projection ")
            .and_then(|l| l.strip_suffix(" bytes at position 7\n"))
            .unwrap_or_else(|| panic!("{}", run.out));
        let (before, after) = line.split_once(" → ").expect("before → after");
        let (before, after): (u64, u64) = (before.parse().unwrap(), after.parse().unwrap());
        assert!(after <= before, "{}", run.out);
    }
}

#[test]
fn export_prints_its_files_and_the_head_it_read_and_refuses_a_directory_in_use() {
    for backend in BACKENDS {
        let sh = Shell::on("export", backend);
        sh.lake();
        sh.run(&["apply", &sh.fixture()]);
        let to = sh.root.join("out").display().to_string();
        ok(
            &sh.run(&["export", "--to", &to]),
            "log/00000000000000000000.jsonl: positions 0..7\n\
             exported finance through position 7 to <root>/out\n",
        );
        // The mock hashes no event's canonical bytes, so only the store's
        // export is a chain a verifier can follow.
        if !matches!(backend, Backend::Mock) {
            let files = lakelands_core::export::verify(&sh.root.join("out")).unwrap();
            assert_eq!(files.through, lakelands_api::Position(7));
        }
        fails(
            &sh.run(&["export", "--to", &to]),
            "error: refused: <root>/out is not empty; an export writes only into an empty or new \
             directory\n  what to do: export into an empty or new directory\n",
        );
        let empty = sh.run(&["export", "--to", ""]);
        assert_eq!((empty.code, empty.out.as_str()), (2, ""), "{}", empty.err);
    }
}

#[test]
fn serve_needs_a_protocol_named() {
    for backend in BACKENDS {
        let sh = Shell::on("serve", backend);
        sh.lake();
        fails(
            &sh.run(&["serve"]),
            "error: nothing to serve was named\n  what to do: pass --mcp to serve the lake over the Model Context Protocol\n",
        );
    }
}

#[test]
fn a_principal_from_the_configuration_acts_under_its_grants() {
    for backend in BACKENDS {
        let sh = Shell::on("as", backend);
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
            "error: refused: you hold no grant for this\n  what to do: act as a principal whose grants allow it, or ask the lake's owner for a grant\n",
        );
    }
}

#[test]
fn the_root_configuration_carries_the_http_door() {
    let config: lakelands_cli::Config = serde_json::from_str(
        r#"{"lakelands": 1, "owner": "owner", "principals": [],
            "server": {"bind": "127.0.0.1:7700", "allowed_origins": ["http://localhost:3000"]}}"#,
    )
    .unwrap();
    let server = config.server.expect("the server section reads");
    assert_eq!(server.bind.port(), 7700);
    assert_eq!(server.allowed_origins, ["http://localhost:3000"]);
    assert_eq!(
        server.session_idle_secs, 1800,
        "sessions end after half an hour idle"
    );
    let unknown = serde_json::from_str::<lakelands_cli::Config>(
        r#"{"lakelands": 1, "owner": "owner", "server": {"bind": "127.0.0.1:7700", "port": 1}}"#,
    );
    assert!(
        unknown.is_err(),
        "an unknown key in the server section is refused"
    );
}

#[test]
fn the_root_configuration_carries_the_embedding_client_and_refuses_a_key_it_cannot_read() {
    let config: lakelands_cli::Config = serde_json::from_str(
        r#"{"lakelands": 1, "owner": "owner",
            "embed": {"endpoint": "http://127.0.0.1:9/v1/embeddings", "model": "tiny",
                      "dimensions": 2, "api_key_file": "secrets/embed.key"}}"#,
    )
    .unwrap();
    let embed = config.embed.expect("the embed section reads");
    assert_eq!((embed.model.as_str(), embed.dimensions), ("tiny", 2));
    assert_eq!((embed.max_batch, embed.timeout_secs), (64, 30));
    assert!(!embed.send_dimensions);
    let unknown = serde_json::from_str::<lakelands_cli::Config>(
        r#"{"lakelands": 1, "owner": "owner", "embed": {"endpoint": "http://127.0.0.1:9/",
            "model": "tiny", "dimensions": 2, "api_key": "sk-inline"}}"#,
    );
    assert!(
        unknown.is_err(),
        "an unknown key in the embed section is refused"
    );

    let sh = Shell::on("embed-key", Backend::Store);
    sh.lake();
    let config = r#"{"lakelands": 1, "owner": "owner",
        "embed": {"endpoint": "http://127.0.0.1:9/v1/embeddings", "model": "tiny",
                  "dimensions": 2, "api_key_file": "secrets/embed.key"}}"#;
    fs::write(sh.root.join("lakelands.json"), config).unwrap();
    let s = sh.run(&["feed"]);
    assert_eq!(s.code, 1, "{}", s.out);
    // The shell shows the root as `<root>`.
    assert!(
        s.err.starts_with("error: <root>/lakelands.json: ")
            && s.err.contains(
                "the embedding client's key file <root>/secrets/embed.key cannot be read"
            )
            && s.err
                .contains("\n  what to do: fix the embed section of the configuration"),
        "{}",
        s.err
    );
    // A relative key file is the root's.
    fs::create_dir_all(sh.root.join("secrets")).unwrap();
    sh.file("secrets/embed.key", "sk-test\n");
    let s = sh.run(&["feed"]);
    assert_eq!(s.code, 0, "{}", s.err);
}

#[test]
fn the_root_configuration_carries_the_deliverer_and_refuses_an_endpoint_it_should_not_reach() {
    let config: lakelands_cli::Config = serde_json::from_str(
        r#"{"lakelands": 1, "owner": "owner",
            "deliver": {"endpoints": [{"name": "billing", "url": "https://hooks.example/x",
                                       "lakes": ["finance"]}]}}"#,
    )
    .unwrap();
    let deliver = config.deliver.expect("the deliver section reads");
    assert_eq!(
        (
            deliver.poll_millis,
            deliver.retry_first_secs,
            deliver.retry_max_secs
        ),
        (1000, 1, 300)
    );
    assert_eq!(
        deliver.endpoints[0].kinds,
        ["trigger_fired", "object_set_changed"]
    );

    let sh = Shell::on("deliver-config", Backend::Store);
    sh.lake();
    let config = r#"{"lakelands": 1, "owner": "owner",
        "deliver": {"endpoints": [{"name": "billing", "url": "https://u:hunter2@hooks.example/x",
                                   "lakes": ["finance"]}]}}"#;
    fs::write(sh.root.join("lakelands.json"), config).unwrap();
    let s = sh.run(&["feed"]);
    assert_eq!(s.code, 1, "{}", s.out);
    assert!(
        s.err.starts_with("error: <root>/lakelands.json: ")
            && s.err
                .contains("\n  what to do: fix the deliver section of the configuration")
            && !s.err.contains("hunter2"),
        "{}",
        s.err
    );

    // An endpoint naming a lake the root does not hold stops every
    // command, except the one that creates that lake.
    let config = r#"{"lakelands": 1, "owner": "owner",
        "deliver": {"endpoints": [{"name": "billing", "url": "https://hooks.example/x",
                                   "lakes": ["finance", "payroll"]}]}}"#;
    fs::write(sh.root.join("lakelands.json"), config).unwrap();
    let s = sh.run(&["feed", "--lake", "finance"]);
    assert_eq!(s.code, 1, "{}", s.out);
    assert!(
        s.err
            .contains("names the lake `payroll`, which this root does not hold")
            && s.err.contains("\n  what to do: fix the deliver section"),
        "{}",
        s.err
    );
    let s = sh.run(&["lake", "new", "payroll"]);
    assert_eq!(s.code, 0, "{}", s.err);
    let s = sh.run(&["feed", "--lake", "finance"]);
    assert_eq!(s.code, 0, "{}", s.err);
}

#[test]
fn the_root_configuration_carries_the_ticker_and_refuses_an_offset_that_is_none() {
    let config: lakelands_cli::Config = serde_json::from_str(
        r#"{"lakelands": 1, "owner": "owner", "aging": {"utc_offset": "+01:00"}}"#,
    )
    .unwrap();
    let aging = config.aging.expect("the aging section reads");
    assert_eq!(
        (aging.utc_offset.as_str(), aging.check_secs),
        ("+01:00", 60)
    );
    let unknown = serde_json::from_str::<lakelands_cli::Config>(
        r#"{"lakelands": 1, "owner": "owner", "aging": {"zone": "Europe/Vienna"}}"#,
    );
    assert!(
        unknown.is_err(),
        "an unknown key in the aging section is refused"
    );

    let sh = Shell::on("aging-config", Backend::Store);
    sh.lake();
    let config = r#"{"lakelands": 1, "owner": "owner", "aging": {"utc_offset": "CET"}}"#;
    fs::write(sh.root.join("lakelands.json"), config).unwrap();
    let s = sh.run(&["feed"]);
    assert_eq!(s.code, 1, "{}", s.out);
    assert!(
        s.err.starts_with("error: <root>/lakelands.json: ")
            && s.err
                .contains("\n  what to do: fix the aging section of the configuration"),
        "{}",
        s.err
    );
}

/// A root that allows an `exec` command computes a field with it: the
/// lake declares a function naming the command, a record is ingested,
/// and the store calls the command and records its answer. The mock
/// computes no function fields, so this runs on the real store only.
#[cfg(unix)]
#[test]
fn a_command_the_root_allows_computes_a_field_through_the_exec_backing() {
    for backend in [Backend::Store, Backend::StoreInMemory] {
        let sh = Shell::on("exec-backing", backend);
        sh.lake();
        let config = r#"{"lakelands": 1, "owner": "owner",
            "backings": {"exec": {"commands": {"echo": {"program": "/bin/cat"}}}}}"#;
        fs::write(sh.root.join("lakelands.json"), config).unwrap();
        let bundle = sh.bundle(
            "places",
            &[
                (
                    "functions/lookup.yaml",
                    "name: lookup\nbacking: exec\ninputs:\n  code: text\nconfig:\n  command: echo\n",
                ),
                (
                    "types/place.yaml",
                    "name: place\nfields:\n  code: { type: text, authority: source }\n  answer:\n    type: json\n    authority: derived\n    function: lookup\n    depends_on: code\n",
                ),
            ],
        );
        let applied = sh.run(&["apply", &bundle]);
        assert_eq!(applied.code, 0, "{}", applied.err);
        let file = sh.file(
            "places.ndjson",
            "{\"type\": \"place\", \"key\": \"p1\", \"fields\": {\"code\": \"AT\"}}\n",
        );
        let ingested = sh.run(&["ingest", &file, "--source", "registry"]);
        assert_eq!(ingested.code, 0, "{}", ingested.err);
        let s = sh.run(&["feed", "--kind", "output_recorded"]);
        assert_eq!(s.code, 0, "{}", s.err);
        let events: Vec<serde_json::Value> = s
            .out
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(events.len(), 1, "{}", s.out);
        let body = &events[0]["body"];
        assert_eq!(body["seam"], "function:exec:lookup");
        assert_eq!(body["field"], serde_json::json!(["place", "p1", "answer"]));
        // `/bin/cat` answers the call it was sent.
        assert_eq!(
            body["output"],
            serde_json::json!({"function": "lookup", "inputs": {"code": "AT"},
                               "config": {"command": "echo"}})
        );
        assert!(
            ingested
                .out
                .contains(&format!("through position {}", events[0]["position"])),
            "the ingest answers the position the filled field is visible from: {}",
            ingested.out
        );
    }
}

/// The built binary, end to end: a root, a lake on disk, a bundle
/// applied, and `serve --mcp` answering an MCP client on its standard
/// input and output with the lake's tools, until the client hangs up.
#[test]
fn the_built_binary_serves_a_lake_over_mcp_on_stdio() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};

    let sh = Shell::on("binary", Backend::Store);
    let root = sh.root.display().to_string();
    let lakelands = env!("CARGO_BIN_EXE_lakelands");
    let run = |args: &[&str]| {
        let out = Command::new(lakelands)
            .args(["--root", root.as_str()])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run(&["init"]);
    run(&["lake", "new", "finance"]);
    run(&["apply", &sh.fixture()]);

    let mut server = Command::new(lakelands)
        .args(["--root", root.as_str(), "serve", "--mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut to = server.stdin.take().unwrap();
    for line in [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    ] {
        writeln!(to, "{line}").unwrap();
    }
    drop(to);
    let answers: Vec<serde_json::Value> = BufReader::new(server.stdout.take().unwrap())
        .lines()
        .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
        .collect();
    let status = server.wait().unwrap();
    assert!(status.success(), "serve --mcp exited with {status}");
    assert_eq!(answers.len(), 2, "{answers:?}");
    assert_eq!(answers[0]["result"]["serverInfo"]["name"], "lakelands");
    let tools: Vec<&str> = answers[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        tools.contains(&"invoice_find") && tools.contains(&"approve"),
        "{tools:?}"
    );
}

/// With a `server` section, `serve --mcp` still ends when its stdio client
/// closes: the HTTP door beside it does not keep the process alive, and
/// the lake's writer lease is free for the next command.
#[test]
fn serve_mcp_ends_with_its_stdio_client_even_with_a_server_section() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let sh = Shell::on("binary-server", Backend::Store);
    let root = sh.root.display().to_string();
    let lakelands = env!("CARGO_BIN_EXE_lakelands");
    let run = |args: &[&str]| {
        Command::new(lakelands)
            .args(["--root", root.as_str()])
            .args(args)
            .output()
            .unwrap()
    };
    assert!(run(&["init"]).status.success());
    assert!(run(&["lake", "new", "finance"]).status.success());
    let file = sh.root.join("lakelands.json");
    let mut config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    config["server"] = serde_json::json!({"bind": "127.0.0.1:0"});
    fs::write(&file, serde_json::to_string_pretty(&config).unwrap()).unwrap();

    let mut server = Command::new(lakelands)
        .args(["--root", root.as_str(), "serve", "--mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The client goes away at once. The bound only turns a child that
    // never ends into a failure; a loaded machine takes seconds to start
    // one that opens a lake and binds a port, so it is the load driver's
    // WAIT, not ten seconds (lakelands#158).
    const WAIT: Duration = Duration::from_secs(60);
    drop(server.stdin.take());
    let started = Instant::now();
    let status = loop {
        if let Some(status) = server.try_wait().unwrap() {
            break status;
        }
        if started.elapsed() > WAIT {
            let _ = server.kill();
            panic!("serve --mcp with a server section did not end with its client");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "serve --mcp exited with {status}");
    // The lease is free: a write goes through.
    let out = run(&["apply", &sh.fixture()]);
    assert!(
        out.status.success(),
        "the next write was refused: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn help_is_plain_and_a_wrong_command_line_exits_2() {
    for backend in BACKENDS {
        let sh = Shell::on("help", backend);
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
}

/// Customers linked to partners by `source` links, for `import`.
const LINKED: [(&str, &str); 2] = [
    (
        "types/partner.yaml",
        "name: partner\nkey: name\nfields:\n  name: { type: text, authority: source, required: true }\n",
    ),
    (
        "types/customer.yaml",
        "name: customer
key: code
fields:
  code: { type: text, authority: source, required: true }
  name: { type: text, authority: source }
  note: text
relations:
  knows: { target: partner, cardinality: many_to_many, authority: source }
",
    ),
];

/// A SQLite lake with records, links, a retirement and an authored write,
/// exported into `<root>/export`.
fn exported(sh: &Shell) -> String {
    sh.lake();
    assert_eq!(sh.run(&["apply", &sh.bundle("linked", &LINKED)]).code, 0);
    let rows = sh.file(
        "crm.ndjson",
        "{\"type\": \"partner\", \"key\": \"acme\", \"fields\": {\"name\": \"acme\"}}\n\
         {\"type\": \"customer\", \"key\": \"c1\", \"fields\": {\"code\": \"c1\", \"name\": \"Ada\"}}\n\
         {\"type\": \"sys.link.customer.knows\", \"key\": \"c1:acme\", \"fields\": {\"from\": \"c1\", \"to\": \"acme\"}}\n",
    );
    assert_eq!(sh.run(&["ingest", &rows, "--source", "crm"]).code, 0);
    let old = sh.file(
        "old.ndjson",
        "{\"type\": \"customer\", \"key\": \"c9\", \"fields\": {\"code\": \"c9\"}}\n",
    );
    assert_eq!(sh.run(&["ingest", &old, "--source", "old"]).code, 0);
    assert_eq!(sh.run(&["purge", "--source", "old"]).code, 0);
    let store = sh.lakes.lock().unwrap()["finance"].clone();
    let owner = lakelands_api::Principal {
        kind: lakelands_api::PrincipalKind::Human,
        id: "owner".into(),
        lake: store.lake().clone(),
        grants: lakelands_api::Grants::owner(),
    };
    block_on(
        store
            .catalog(lakelands_api::WriteContext::default())
            .upsert(
                &owner,
                &"customer".into(),
                &"c1".into(),
                [("note".into(), serde_json::json!("vip"))].into(),
                lakelands_api::WriteOpts::default(),
            ),
    )
    .unwrap();
    let dir = sh.root.join("export").display().to_string();
    assert_eq!(sh.run(&["export", "--to", &dir]).code, 0);
    dir
}

/// The last event's position and hash, as `feed` prints it.
fn last_event(feed: &str) -> (u64, String) {
    let last: serde_json::Value = serde_json::from_str(feed.lines().last().unwrap()).unwrap();
    (
        last["position"].as_u64().unwrap(),
        last["hash"].as_str().unwrap().to_owned(),
    )
}

/// `import` goes through the store over the binary's own engines, so it
/// is checked on the real store only: the mock keeps no files to import
/// into.
#[test]
fn import_makes_the_exported_lake_in_an_empty_lake_of_its_name() {
    let from = Shell::on("import-from", Backend::Store);
    let dir = exported(&from);
    let source = from.run(&["feed"]).out;
    let (head, hash) = last_event(&source);
    // The manifest is never trusted: the lake is the first event's.
    let manifest = PathBuf::from(&dir).join("export.json");
    let text = fs::read_to_string(&manifest).unwrap();
    fs::write(&manifest, text.replace("\"finance\"", "\"elsewhere\"")).unwrap();

    let to = Shell::on("import-to", Backend::Store);
    to.lake();
    let events = head + 1;
    ok(
        &to.run(&["import", &dir]),
        &format!(
            "imported lake `finance`: {events} event(s) from 1 file(s), head {head}, hash {hash}\n"
        ),
    );
    assert_eq!(
        to.run(&["feed"]).out,
        source,
        "the same log, event for event"
    );
    // Exported again, the files begin with the same bytes: the lake's
    // first writer appends its own lease after them.
    let again = to.root.join("again").display().to_string();
    assert_eq!(to.run(&["export", "--to", &again]).code, 0);
    let first = "log/00000000000000000000.jsonl";
    let was = fs::read(PathBuf::from(&dir).join(first)).unwrap();
    let now = fs::read(PathBuf::from(&again).join(first)).unwrap();
    assert!(
        now.starts_with(&was),
        "the imported lake exports the source's bytes"
    );
}

#[test]
fn import_refuses_a_lake_that_holds_events_another_lake_memory_and_a_damaged_export() {
    let from = Shell::on("import-refusals-from", Backend::Store);
    let dir = exported(&from);
    let (head, _) = last_event(&from.run(&["feed"]).out);

    let to = Shell::on("import-refusals-to", Backend::Store);
    to.lake();
    assert_eq!(to.run(&["import", &dir]).code, 0);
    let again = to.run(&["import", &dir]);
    assert_eq!((again.code, again.out.as_str()), (1, ""));
    assert!(
        again
            .err
            .contains(&format!("already holds events through position {head}")),
        "{}",
        again.err
    );
    assert!(again.err.contains("  what to do: "), "{}", again.err);

    assert_eq!(to.run(&["lake", "new", "other"]).code, 0);
    let other = to.run(&["import", &dir, "--lake", "other"]);
    assert_eq!((other.code, other.out.as_str()), (1, ""));
    assert!(
        other.err.contains("`finance`") && other.err.contains("`other`"),
        "{}",
        other.err
    );

    let memory = Shell::on("import-refusals-memory", Backend::Store);
    let root = memory.root.display().to_string();
    assert_eq!(memory.raw(&["lakelands", "init", &root]).code, 0);
    assert_eq!(
        memory
            .run(&["lake", "new", "finance", "--engine", "memory"])
            .code,
        0
    );
    let kept = memory.run(&["import", &dir]);
    assert_eq!((kept.code, kept.out.as_str()), (1, ""));
    assert!(kept.err.contains("kept in memory"), "{}", kept.err);

    let damaged = from.root.join("damaged");
    fs::create_dir_all(damaged.join("log")).unwrap();
    let segment = "log/00000000000000000000.jsonl";
    let mut bytes = fs::read(PathBuf::from(&dir).join(segment)).unwrap();
    let at = bytes.len() / 2;
    bytes[at] = if bytes[at] == b'a' { b'b' } else { b'a' };
    fs::write(damaged.join(segment), bytes).unwrap();
    let fresh = Shell::on("import-refusals-damaged", Backend::Store);
    fresh.lake();
    let s = fresh.run(&[
        "import",
        &damaged.display().to_string(),
        "--lake",
        "finance",
    ]);
    assert_eq!((s.code, s.out.as_str()), (1, ""));
    assert!(s.err.contains("the export is damaged"), "{}", s.err);
    assert!(
        fresh.run(&["import", &dir]).code == 0,
        "a damaged export imported nothing"
    );
}

/// The built binary moves a lake between two roots, each command in a
/// process of its own, as the walk types it: the command's engines are the
/// binary's, not a test's opener.
#[test]
fn the_built_binary_moves_a_lake_by_export_and_import() {
    use std::process::Command;

    let lakelands = env!("CARGO_BIN_EXE_lakelands");
    let run = |root: &PathBuf, args: &[&str]| -> String {
        let out = Command::new(lakelands)
            .arg("--root")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    let from = Shell::on("import-binary-from", Backend::Store);
    let to = Shell::on("import-binary-to", Backend::Store);
    run(&from.root, &["init"]);
    run(&from.root, &["lake", "new", "finance"]);
    run(&from.root, &["apply", &from.bundle("linked", &LINKED)]);
    let rows = from.file(
        "crm.ndjson",
        "{\"type\": \"partner\", \"key\": \"acme\", \"fields\": {\"name\": \"acme\"}}\n",
    );
    run(&from.root, &["ingest", &rows, "--source", "crm"]);
    let dir = from.root.join("export").display().to_string();
    run(&from.root, &["export", "--to", &dir]);
    let source = run(&from.root, &["feed"]);
    run(&to.root, &["init"]);
    run(&to.root, &["lake", "new", "finance"]);
    let said = run(&to.root, &["import", &dir]);
    assert!(said.starts_with("imported lake `finance`: "), "{said}");
    assert_eq!(run(&to.root, &["feed"]), source);
}
