//! Transcripts of the CLI: every byte of what a command prints is pinned,
//! with the root's path shown as `<root>`. Each test runs unchanged on
//! the real store, over the engine each location names and with every
//! lake kept in memory.
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
use lakelands_core::conformance::fixture;

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
    /// The real store, over the engine each location names.
    Store,
    /// The real store, with every lake kept in memory.
    StoreInMemory,
}

const BACKENDS: [Backend; 2] = [Backend::Store, Backend::StoreInMemory];

/// A root in a fresh directory, with lakes that live as long as the test,
/// kept on one backend.
struct Shell {
    root: PathBuf,
    backend: Backend,
    lakes: Arc<Mutex<BTreeMap<String, Arc<dyn LakeStore>>>>,
    /// How many of the engine's next reclaims fail (the suite's harness).
    fails: lakelands_core::conformance::checks::Fails,
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
            fails: Arc::default(),
        }
    }

    /// Open a lake once per shell, as the binary opens it once per process.
    fn open(&self, l: Location, mode: OpenMode, host: Seams) -> Opening {
        let (lakes, backend) = (self.lakes.clone(), self.backend);
        let failing = lakelands_core::conformance::checks::failing(&self.fails);
        Box::pin(async move {
            let name = l.lake.to_string();
            if let Some(store) = lakes.lock().unwrap().get(&name) {
                return Ok(store.clone());
            }
            let mut l = l;
            if backend == Backend::StoreInMemory {
                l.engine = EngineConfig::Memory;
            }
            let seams = Seams {
                engines: vec![
                    failing(Arc::new(lakelands_engine_mem::MemoryOpener::new())),
                    failing(Arc::new(lakelands_engine_sqlite::SqliteOpener)),
                ],
                ..host
            };
            let mode = if mode == OpenMode::Read {
                OpenMode::Read
            } else {
                OpenMode::Create
            };
            let store: Arc<dyn LakeStore> = lakelands_store::open(l, seams, mode).await?;
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

    /// The door checks' fixture ontology, as a bundle.
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

Usage: lakelands init [DIR] [--owner <ID>] [--example [<NAME>]]
       lakelands init --root <DIR> [--owner <ID>] [--example [<NAME>]]

Arguments:
  [DIR]  The directory to hold the root; --root when left out

Options:
      --owner <ID>        The id recorded as the writer when you act as the owner [default: owner]
      --root <DIR>        The Lakelands root to work in [default: .]
      --as <NAME>         Act as this principal from the root's configuration instead of the owner
      --example [<NAME>]  Make a lake from an example this binary carries, with an MCP configuration for it; without a name, list them
      --lake <ID>         The lake to work on; may be left out when the root holds only one
  -h, --help              Print help (see more with '--help')
",
        );
    }
}

#[cfg(feature = "pg")]
#[test]
fn lake_new_writes_a_postgres_location_as_hosting_shows_it() {
    let sh = Shell::on("lake-new-pg", Backend::Store);
    let root = sh.root.display().to_string();
    assert_eq!(sh.raw(&["lakelands", "init", &root]).code, 0);
    fails(
        &sh.run(&["lake", "new", "books", "--engine", "postgres"]),
        "error: a PostgreSQL lake needs `--url`\n  what to do: name the database, as `--url postgres://user@host/database`; the location file holds the URL, password included, as plain text\n",
    );
    assert!(!sh.root.join("lakes/books.json").exists());
    let url = "postgres://lakelands:secret@db.example:5432/lakes";
    ok(
        &sh.run(&["lake", "new", "books", "--engine", "postgres", "--url", url]),
        "created lake `books` (engine postgres) in lakes/books.json\n",
    );
    let written = fs::read_to_string(sh.root.join("lakes/books.json")).unwrap();
    let location: Location = serde_json::from_str(&written).unwrap();
    assert_eq!(location, Location::postgres("books", url));
    #[cfg(unix)]
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(
            &fs::metadata(sh.root.join("lakes/books.json"))
                .unwrap()
                .permissions()
        ) & 0o777,
        0o600,
        "a location that may hold a password is its operator's alone"
    );
    fails(
        &sh.run(&["lake", "new", "x", "--url", url]),
        "error: `--url`, `--schema`, `--no-tls` and `--ca-file` are for `postgres`, not `sqlite`\n  what to do: leave them out, or choose `--engine postgres`\n",
    );
    fails(
        &sh.run(&[
            "lake",
            "new",
            "x",
            "--engine",
            "postgres",
            "--url",
            url,
            "--no-tls",
            "--ca-file",
            "/c.pem",
        ]),
        "error: `--no-tls` and `--ca-file` contradict each other\n  what to do: a CA file verifies a TLS connection: leave one of them out\n",
    );
    assert!(!sh.root.join("lakes/x.json").exists());
    ok(
        &sh.run(&[
            "lake",
            "new",
            "ledger",
            "--engine",
            "postgres",
            "--url",
            url,
            "--schema",
            "ledger_s",
            "--ca-file",
            "/etc/ssl/ca.pem",
        ]),
        "created lake `ledger` (engine postgres) in lakes/ledger.json\n",
    );
    let written = fs::read_to_string(sh.root.join("lakes/ledger.json")).unwrap();
    let written: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(written["engine"]["schema"], "ledger_s");
    assert_eq!(written["engine"]["tls"]["ca_file"], "/etc/ssl/ca.pem");
    ok(
        &sh.run(&[
            "lake", "new", "local", "--engine", "postgres", "--url", url, "--no-tls",
        ]),
        "created lake `local` (engine postgres) in lakes/local.json\n",
    );
    let written = fs::read_to_string(sh.root.join("lakes/local.json")).unwrap();
    let written: serde_json::Value = serde_json::from_str(&written).unwrap();
    assert_eq!(written["engine"]["tls"]["mode"], "disable");
    fails(
        &sh.run(&["lake", "new", "books", "--engine", "postgres", "--url", url]),
        "error: lake `books` exists already in <root>; nothing was changed\n  what to do: choose another name, or keep working with the lake that is there\n",
    );
}

#[cfg(not(feature = "pg"))]
#[test]
fn lake_new_refuses_postgres_in_a_build_without_it() {
    let sh = Shell::on("lake-new-no-pg", Backend::Store);
    let root = sh.root.display().to_string();
    assert_eq!(sh.raw(&["lakelands", "init", &root]).code, 0);
    fails(
        &sh.run(&[
            "lake",
            "new",
            "books",
            "--engine",
            "postgres",
            "--url",
            "postgres://h/d",
        ]),
        "error: this build of lakelands has no PostgreSQL engine\n  what to do: build it with the `pg` feature, or choose `sqlite` or `memory`\n",
    );
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
        // Applied again, it says each is in force already and since when,
        // and that nothing was appended (lakelands#1106).
        ok(
            &sh.run(&["apply", &fixture]),
            "4 declaration(s) in force already: nothing appended\n  \
             type `invoice`, in force since position 4\n  \
             type `partner`, in force since position 5\n  \
             verb `approve`, in force since position 6\n  \
             verb `pay`, in force since position 7\n",
        );
        // A set of one in force and one new: the new one is appended, and
        // the other says since when.
        let note = "name: note\nfields:\n  text: { type: text }\n";
        let mixed = sh.bundle(
            "mixed",
            &[("types/partner.yaml", PARTNER), ("types/note.yaml", note)],
        );
        let said = sh.run(&["apply", &mixed]);
        assert_eq!(
            (said.code, said.err.as_str()),
            (0, ""),
            "stdout: {}",
            said.out
        );
        assert!(
            said.out
                .starts_with("applied 2 declaration(s), in force from position 8:\n")
                && said.out.contains("  type `note`\n")
                && said
                    .out
                    .contains("  type `partner`, in force since position 5\n"),
            "{backend:?}: {}",
            said.out
        );
    }
}

#[test]
fn recompute_names_a_function_or_a_type_and_says_where_it_holds() {
    for backend in BACKENDS {
        let sh = Shell::on("recompute", backend);
        sh.lake();
        let rated = sh.bundle(
            "rated",
            &[("functions/twice.yaml", "name: twice\nexpr: \"1\"\n")],
        );
        let applied = sh.run(&["apply", &rated]);
        assert_eq!((applied.code, applied.err.as_str()), (0, ""));
        let done = sh.run(&["recompute", "--function", "twice"]);
        assert_eq!((done.code, done.err.as_str()), (0, ""), "{}", done.out);
        assert!(
            done.out
                .starts_with("computed function `twice` again, from position "),
            "{}",
            done.out
        );
        // What it cannot name is refused, saying why; neither name is one.
        for (args, names) in [
            (&["recompute", "--function", "ghost"][..], "ghost"),
            (&["recompute"][..], "--function"),
        ] {
            let s = sh.run(args);
            assert_eq!((s.code, s.out.as_str()), (1, ""), "{}", s.err);
            assert!(s.err.contains(names), "{}", s.err);
        }
    }
}

/// `change` prints the plan of a change to records as one JSON document
/// and changes nothing; with `--plan` it makes the change against that
/// plan, and a plan the lake moved from is refused `plan_stale` (081).
#[test]
fn change_prints_the_plan_and_makes_the_change_only_against_it() {
    for backend in BACKENDS {
        let sh = Shell::on("change", backend);
        sh.lake();
        assert_eq!(sh.run(&["apply", &sh.fixture()]).code, 0);
        let store = sh.lakes.lock().unwrap()["finance"].clone();
        let owner = lakelands_api::Principal {
            kind: lakelands_api::PrincipalKind::Human,
            id: "owner".into(),
            lake: store.lake().clone(),
            grants: lakelands_api::Grants::owner(),
        };
        let put = |key: &str, status: &str| {
            let patch = [("status".into(), serde_json::json!(status))].into();
            let catalog = store.catalog(lakelands_api::WriteContext::default());
            let at = (&"invoice".into(), &key.into());
            let done = catalog.upsert(&owner, at.0, at.1, patch, Default::default());
            block_on(done).unwrap();
        };
        put("INV-1", "open");
        put("INV-2", "open");
        let head = block_on(store.head()).unwrap();
        let args = ["change", "invoice", "--where", "status = 'open'"];
        let planned = sh.run(&[&args[..], &["--set", "status=\"late\""]].concat());
        assert_eq!((planned.code, planned.err.as_str()), (0, ""));
        let plan: serde_json::Value = serde_json::from_str(&planned.out).unwrap();
        assert_eq!(plan["records"]["selected"], 2, "{}", planned.out);
        assert_eq!(
            plan["said"][0],
            "The condition selects 2 records of `invoice`: `INV-1`, `INV-2`."
        );
        assert_eq!(block_on(store.head()).unwrap(), head, "a plan appended");
        let id = plan["report_id"].as_str().unwrap();

        put("INV-2", "paid");
        let at = [&args[..], &["--set", "status=\"late\"", "--plan", id]].concat();
        let stale = sh.run(&at);
        assert_eq!((stale.code, stale.out.as_str()), (1, ""), "{}", stale.err);
        assert!(stale.err.contains("plan"), "{}", stale.err);

        let plan = sh.run(&[&args[..], &["--set", "status=\"late\""]].concat());
        let plan: serde_json::Value = serde_json::from_str(&plan.out).unwrap();
        let id = plan["report_id"].as_str().unwrap();
        let at = [&args[..], &["--set", "status=\"late\"", "--plan", id]].concat();
        let done = sh.run(&at);
        assert_eq!((done.code, done.err.as_str()), (0, ""));
        let outcome: serde_json::Value = serde_json::from_str(&done.out).unwrap();
        assert_eq!(outcome["outcome"], "applied", "{}", done.out);

        // Neither `--set` nor `--retire` is a usage error; a value that
        // is no JSON is refused, saying which.
        let neither = sh.run(&args);
        assert_eq!(
            (neither.code, neither.out.as_str()),
            (2, ""),
            "{}",
            neither.err
        );
        let bad = sh.run(&[&args[..], &["--set", "status=late"]].concat());
        assert_eq!((bad.code, bad.out.as_str()), (1, ""), "{}", bad.err);
        assert!(bad.err.contains("status=late"), "{}", bad.err);

        // `--retire` is planned, changes nothing until enacted against its
        // plan, and then retires what the condition selected.
        put("INV-5", "scrap");
        let scrap = [
            "change",
            "invoice",
            "--where",
            "status = 'scrap'",
            "--retire",
        ];
        let head = block_on(store.head()).unwrap();
        let planned = sh.run(&scrap);
        assert_eq!((planned.code, planned.err.as_str()), (0, ""));
        let plan: serde_json::Value = serde_json::from_str(&planned.out).unwrap();
        assert_eq!(plan["records"]["selected"], 1, "{}", planned.out);
        assert_eq!(block_on(store.head()).unwrap(), head, "a plan appended");
        let id = plan["report_id"].as_str().unwrap();
        let done = sh.run(&[&scrap[..], &["--plan", id]].concat());
        assert_eq!((done.code, done.err.as_str()), (0, ""));
        let outcome: serde_json::Value = serde_json::from_str(&done.out).unwrap();
        assert_eq!(outcome["outcome"], "applied", "{}", done.out);
        let again = sh.run(&scrap);
        let again: serde_json::Value = serde_json::from_str(&again.out).unwrap();
        assert_eq!(again["records"]["selected"], 0, "{again}");
        let both = sh.run(&[&scrap[..], &["--set", "status=\"x\""]].concat());
        assert_eq!((both.code, both.out.as_str()), (2, ""), "{}", both.err);
    }
}

/// `query --cypher` reads the lake with an openCypher statement and its
/// `--param`s and prints the answer as one JSON document, a page further
/// on by `--cursor` and an earlier lake by `--as-of`; `EXPLAIN` prints
/// the pattern the statement translates to and reads nothing. A statement
/// the lake refuses, and a `--param` that is no JSON, are said on stderr.
#[test]
fn query_runs_an_open_cypher_statement_with_its_params() {
    for backend in BACKENDS {
        let sh = Shell::on("query", backend);
        sh.lake();
        assert_eq!(sh.run(&["apply", &sh.fixture()]).code, 0);
        let store = sh.lakes.lock().unwrap()["finance"].clone();
        let owner = lakelands_api::Principal {
            kind: lakelands_api::PrincipalKind::Human,
            id: "owner".into(),
            lake: store.lake().clone(),
            grants: lakelands_api::Grants::owner(),
        };
        let mut before = None;
        for (key, status) in [("INV-1", "open"), ("INV-2", "paid"), ("INV-3", "open")] {
            before = before.or((key == "INV-3").then(|| block_on(store.head()).unwrap()));
            let patch = [("status".into(), serde_json::json!(status))].into();
            let catalog = store.catalog(lakelands_api::WriteContext::default());
            let at = (&"invoice".into(), &key.into());
            let done = catalog.upsert(&owner, at.0, at.1, patch, Default::default());
            block_on(done).unwrap();
        }
        let statement = "MATCH (i:invoice) WHERE i.status = $status \
                         RETURN i.key ORDER BY i.key";
        let status = ["--param", "status=\"open\""];
        let read = sh.run(&[&["query", "--cypher", statement][..], &status].concat());
        assert_eq!((read.code, read.err.as_str()), (0, ""));
        let answer: serde_json::Value = serde_json::from_str(&read.out).unwrap();
        assert_eq!(
            answer["data"]["rows"],
            serde_json::json!([{ "i.key": "INV-1" }, { "i.key": "INV-3" }]),
            "{}",
            read.out
        );

        // A page continues by `--cursor`; `--as-of` reads an earlier lake.
        let paged = format!("{statement} LIMIT 1");
        let first = sh.run(&[&["query", "--cypher", &paged][..], &status].concat());
        let first: serde_json::Value = serde_json::from_str(&first.out).unwrap();
        assert_eq!(
            first["data"]["rows"],
            serde_json::json!([{ "i.key": "INV-1" }])
        );
        let cursor = first["data"]["next_cursor"].as_str().unwrap();
        let on = [
            &["query", "--cypher", &paged, "--cursor", cursor][..],
            &status,
        ]
        .concat();
        let next = sh.run(&on);
        assert_eq!((next.code, next.err.as_str()), (0, ""));
        let next: serde_json::Value = serde_json::from_str(&next.out).unwrap();
        assert_eq!(
            next["data"]["rows"],
            serde_json::json!([{ "i.key": "INV-3" }])
        );
        let before = before.unwrap().0.to_string();
        let earlier = [
            &["query", "--cypher", statement, "--as-of", &before][..],
            &status,
        ];
        let earlier = sh.run(&earlier.concat());
        assert_eq!((earlier.code, earlier.err.as_str()), (0, ""));
        let earlier: serde_json::Value = serde_json::from_str(&earlier.out).unwrap();
        assert_eq!(
            earlier["data"]["rows"],
            serde_json::json!([{ "i.key": "INV-1" }])
        );

        let explain = format!("EXPLAIN {statement}");
        let head = block_on(store.head()).unwrap();
        let shown = sh.run(&[&["query", "--cypher", &explain][..], &status].concat());
        assert_eq!((shown.code, shown.err.as_str()), (0, ""));
        let shown: serde_json::Value = serde_json::from_str(&shown.out).unwrap();
        let start = shown["pattern"]["start"].to_string();
        assert!(start.contains("\"invoice\""), "{shown}");
        assert_eq!(shown.get("data"), None, "{shown}");
        assert_eq!(block_on(store.head()).unwrap(), head);

        let missing = sh.run(&["query", "--cypher", statement]);
        assert_eq!((missing.code, missing.out.as_str()), (1, ""));
        assert!(missing.err.contains("status"), "{}", missing.err);
        let bad = sh.run(&["query", "--cypher", statement, "--param", "status=open"]);
        assert_eq!((bad.code, bad.out.as_str()), (1, ""), "{}", bad.err);
        assert!(bad.err.contains("status=open"), "{}", bad.err);
    }
}

/// `query --cypher` with a write statement: a `MERGE` is written at once,
/// and a `MATCH … SET` prints its plan with what it would change said,
/// changes nothing, and is enacted by `--plan` with the plan's id.
#[test]
fn query_writes_with_an_open_cypher_statement_and_enacts_a_plan() {
    for backend in BACKENDS {
        let sh = Shell::on("query-write", backend);
        sh.lake();
        assert_eq!(sh.run(&["apply", &sh.fixture()]).code, 0);
        let store = sh.lakes.lock().unwrap()["finance"].clone();
        let merged = "MERGE (i:invoice {key: $key}) SET i.status = $status";
        let args = ["--param", "key=\"INV-1\"", "--param", "status=\"open\""];
        let put = sh.run(&[&["query", "--cypher", merged][..], &args].concat());
        assert_eq!((put.code, put.err.as_str()), (0, ""));
        let put: serde_json::Value = serde_json::from_str(&put.out).unwrap();
        assert_eq!(put["outcome"], "applied", "{put}");

        let late = "MATCH (i:invoice) WHERE i.status = $status SET i.status = $to";
        let p = ["--param", "status=\"open\"", "--param", "to=\"late\""];
        let head = block_on(store.head()).unwrap();
        let planned = sh.run(&[&["query", "--cypher", late][..], &p].concat());
        assert_eq!((planned.code, planned.err.as_str()), (0, ""));
        let plan: serde_json::Value = serde_json::from_str(&planned.out).unwrap();
        assert_eq!(plan["data"]["records"]["selected"], 1, "{plan}");
        let said = plan["said"].as_array().expect("said");
        assert!(
            said.iter()
                .any(|s| s.as_str().unwrap().contains("`status`")),
            "{plan}"
        );
        assert_eq!(block_on(store.head()).unwrap(), head, "a plan appended");

        let id = plan["data"]["report_id"].as_str().unwrap();
        let enact = [&["query", "--cypher", late][..], &p, &["--plan", id]].concat();
        let done = sh.run(&enact);
        assert_eq!((done.code, done.err.as_str()), (0, ""));
        let done: serde_json::Value = serde_json::from_str(&done.out).unwrap();
        assert_eq!(done["outcome"], "applied", "{done}");
        let read = sh.run(&[
            "query",
            "--cypher",
            "MATCH (i:invoice) RETURN i.key, i.status",
        ]);
        let read: serde_json::Value = serde_json::from_str(&read.out).unwrap();
        let want = serde_json::json!([{ "i.key": "INV-1", "i.status": "late" }]);
        assert_eq!(read["data"]["rows"], want, "{read}");
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

/// `search` ranks a type's records by `--text`; as of a position, with
/// `--text-as-of`, by the text the records `--where` selects held there,
/// so of two pages a second ingest swapped it finds the one that held the
/// words then. Without `--where` it is refused, naming `text_as_of`.
#[test]
fn search_ranks_the_text_then_with_text_as_of() {
    for backend in BACKENDS {
        let sh = Shell::on("search", backend);
        sh.lake();
        let page = "name: page\nfields:\n  doc: { type: text, authority: source }\n  text: { type: text, searchable: true, authority: source }\n";
        assert_eq!(
            sh.run(&["apply", &sh.bundle("pages", &[("types/page.yaml", page)])])
                .code,
            0
        );
        let rows = |one: &str, two: &str| {
            let row = |key, text| {
                format!(
                    "{{\"type\": \"page\", \"key\": \"{key}\", \"fields\": {{\"doc\": \"d1\", \"text\": \"{text}\"}}}}\n"
                )
            };
            row("p1", one) + &row("p2", two)
        };
        let (cover, invoice) = ("Cover letter", "Invoice 4711, total");
        let first = sh.run(&[
            "ingest",
            &sh.file("first.ndjson", &rows(cover, invoice)),
            "--source",
            "scan",
        ]);
        let before = first.out.trim_end().rsplit(' ').next().unwrap().to_owned();
        let second = sh.file("second.ndjson", &rows(invoice, cover));
        let said = sh.run(&["ingest", &second, "--source", "scan"]);
        assert_eq!(said.code, 0, "{}", said.err);
        let search = [
            "search", "--type", "page", "--text", "4711", "--as-of", &before,
        ];
        let hits = |more: &[&str]| {
            let said = sh.run(&[&search[..], more].concat());
            assert_eq!((said.code, said.err.as_str()), (0, ""), "{}", said.out);
            let said: serde_json::Value = serde_json::from_str(&said.out).unwrap();
            let hit = |h: &serde_json::Value| (h["record"]["key"].clone(), h["matches"].clone());
            said["data"]["hits"]
                .as_array()
                .unwrap()
                .iter()
                .map(hit)
                .collect::<Vec<_>>()
        };
        let then = hits(&["--where", "doc = 'd1'", "--text-as-of", "--matches"]);
        let spans = serde_json::json!({"text": {"spans": [{"start": 8, "end": 12}], "count": 1}});
        assert_eq!(then, [("p2".into(), spans)]);
        let head = hits(&["--where", "doc = 'd1'"]);
        assert_eq!(head, [("p1".into(), serde_json::Value::Null)]);
        let refused = sh.run(&[&search[..], &["--text-as-of"]].concat());
        assert_eq!((refused.code, refused.out.as_str()), (1, ""));
        assert!(refused.err.contains("text_as_of"), "{}", refused.err);
    }
}

#[test]
fn ingest_prints_one_line_per_commit_window_and_skips_what_it_has() {
    for backend in BACKENDS {
        let sh = Shell::on("ingest", backend);
        sh.lake();
        sh.run(&["apply", &sh.fixture()]);
        let rows: String = (1..=300)
        .map(|i| format!("{{\"type\": \"invoice\", \"key\": \"INV-{i}\", \"fields\": {{\"amount\": \"{i}.00\"}}}}\n"))
        .collect();
        let file = sh.file("bank.ndjson", &rows);
        ok(
            &sh.run(&["ingest", &file, "--source", "bank"]),
            "committed 256 record(s), 0 already in the lake, through position 263\n\
         committed 44 record(s), 0 already in the lake, through position 307\n",
        );
        ok(
            &sh.run(&["ingest", &file, "--source", "bank"]),
            "committed 0 record(s), 256 already in the lake, through position 307\n\
         committed 0 record(s), 44 already in the lake, through position 307\n",
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
        // The record is named, and where to send the stream again from.
        assert!(
            s.err
                .starts_with("error: record 0 of the stream (`invoice` `INV-4`): refused: ")
                && s.err
                    .ends_with("nothing was written; then send the stream again from record 0\n"),
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
        // as a placeholder.
        let s = sh.run(&["feed"]);
        assert_eq!((s.code, s.err.as_str()), (0, ""));
        let lines: Vec<String> = s.out.lines().map(clocked).collect();
        // Every event is in the release's one log format.
        let v = lakelands_api::event::LOG_FORMAT_VERSION.to_string();
        assert_eq!(
            lines[..2],
            [
                "{\"body\":{\"format\":{v},\"kind\":\"lake_created\"},\"context\":{\"depth\":0,\"origin\":\"direct\"},\"fence\":0,\"hash\":\"{hash}\",\"id\":\"{id}\",\"known_at\":\"{known_at}\",\"lake\":\"finance\",\"origin\":\"system\",\"position\":0,\"prev\":\"{prev}\",\"v\":{v},\"writer\":\"system:lakelands\"}",
                "{\"body\":{\"fence\":1,\"holder\":\"{holder}\",\"kind\":\"lease_taken\"},\"context\":{\"depth\":0,\"origin\":\"direct\"},\"fence\":1,\"hash\":\"{hash}\",\"id\":\"{id}\",\"known_at\":\"{known_at}\",\"lake\":\"finance\",\"origin\":\"system\",\"position\":1,\"prev\":\"{prev}\",\"v\":{v},\"writer\":\"system:lakelands\"}",
            ]
            .map(|line| line.replace("{v}", &v)),
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
        // A preflight says what the purge would do and under which id; a
        // purge under another id is refused, and under that one it purges.
        let seen = sh.run(&["purge", "--source", "bank", "--preflight"]);
        let said = "invoice: would retire 2\nbank: preflight at position 9; purge with --report ";
        let id = seen.out.strip_prefix(said).map(str::trim);
        let id = id.unwrap_or_else(|| panic!("{backend:?}: {} {}", seen.out, seen.err));
        let stale = sh.run(&["purge", "--source", "bank", "--report", "another"]);
        assert!(
            stale.code != 0 && stale.err.contains("preflight again"),
            "{}",
            stale.err
        );
        ok(
            &sh.run(&["purge", "--source", "bank", "--report", id]),
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

/// `--type` narrows a purge to one type: each closing line names the type,
/// the preflight's id is of that purge alone, and the source's records in
/// its other types stay.
#[test]
fn purge_of_one_type_names_the_type_in_its_lines() {
    const TYPES: [(&str, &str); 2] = [
        (
            "types/payer.yaml",
            "name: payer\nfields:\n  name: { type: text, authority: source }\n",
        ),
        (
            "types/payee.yaml",
            "name: payee\nfields:\n  name: { type: text, authority: source }\n",
        ),
    ];
    for backend in BACKENDS {
        let sh = Shell::on("purge-type", backend);
        sh.lake();
        let applied = sh.run(&["apply", &sh.bundle("types", &TYPES)]);
        assert_eq!(applied.code, 0, "{backend:?}: {}", applied.err);
        let rows = sh.file(
            "bank.ndjson",
            "{\"type\": \"payer\", \"key\": \"p1\", \"fields\": {\"name\": \"Ada\"}}\n\
             {\"type\": \"payee\", \"key\": \"q1\", \"fields\": {\"name\": \"Bo\"}}\n",
        );
        let fed = sh.run(&["ingest", &rows, "--source", "bank"]);
        assert_eq!(fed.code, 0, "{backend:?}: {}", fed.err);
        let id = |seen: Said, said: &str| match seen.out.strip_prefix(said) {
            Some(id) => id.trim().to_owned(),
            None => panic!("{backend:?}: {} {}", seen.out, seen.err),
        };
        let whole = id(
            sh.run(&["purge", "--source", "bank", "--preflight"]),
            "payee: would retire 1\npayer: would retire 1\nbank: preflight at position 7; purge with --report ",
        );
        let of = ["purge", "--source", "bank", "--type", "payer"];
        let typed = id(
            sh.run(&[&of[..], &["--preflight"]].concat()),
            "payer: would retire 1\nbank in payer: preflight at position 7; purge with --report ",
        );
        // The report of the source whole is not of this purge.
        let other = sh.run(&[&of[..], &["--report", &whole]].concat());
        assert!(
            other.code != 0 && other.err.contains("not of this purge"),
            "{backend:?}: {} {}",
            other.out,
            other.err
        );
        ok(
            &sh.run(&[&of[..], &["--report", &typed]].concat()),
            "payer: retired 1\npurged bank from payer through position 8\n",
        );
        ok(&sh.run(&of), "nothing to retire for bank in payer\n");
        ok(
            &sh.run(&["purge", "--source", "bank"]),
            "payee: retired 1\npurged bank through position 9\n",
        );
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

/// `upgrade` reads the log's ends: a lake in the format this release
/// writes needs none, with `--check` or without. A lake of an older
/// format is upgraded by the open it makes, which
/// `crates/lakelands-store/tests/lake.rs` holds.
#[test]
fn upgrade_says_a_lake_of_this_release_s_format_needs_none() {
    let v = lakelands_api::event::LOG_FORMAT_VERSION;
    let sh = Shell::on("upgrade", Backend::Store);
    sh.lake();
    sh.run(&["apply", &sh.fixture()]);
    let said = format!("finance is written in log format {v}: nothing to upgrade\n");
    ok(&sh.run(&["upgrade", "--check"]), &said);
    ok(&sh.run(&["upgrade"]), &said);
}

/// `upgrade` on a lake the release before wrote whole, in format 14:
/// `--check` says what an open for writing does and opens none; the
/// upgrade says the step it took; run again it has none to take.
#[test]
fn upgrade_takes_a_lake_of_the_format_before_one_step_once() {
    use lakelands_api::engine::EngineOpener;
    use lakelands_api::event::{EventBody, LOG_FORMAT_VERSION as NEW, OLDEST_LOG_FORMAT as OLD};
    use lakelands_api::{Position, Timestamp};
    let sh = Shell::on("upgrade-14", Backend::Store);
    sh.lake();
    // The lake's location, as the root keeps it, its directory under it.
    let file = sh.root.join("lakes").join("finance.json");
    let mut at: lakelands_api::Location =
        serde_json::from_str(&fs::read_to_string(file).unwrap()).unwrap();
    if let lakelands_api::location::EngineConfig::Sqlite { dir, .. } = &mut at.engine
        && dir.is_relative()
    {
        *dir = sh.root.join(&*dir);
    }
    // The release before's writer: `lake_created` and its lease, in 14.
    let engine = lakelands_engine_sqlite::SqliteOpener
        .open(&at, OpenMode::Create)
        .unwrap();
    let lease = engine.take_lease("the release before").unwrap();
    let older = |e: lakelands_api::engine::CanonicalEvent, format: bool| {
        let mut v: serde_json::Value = serde_json::from_slice(&e.bytes).unwrap();
        v["v"] = serde_json::json!(OLD);
        if format {
            v["body"]["format"] = serde_json::json!(OLD);
        }
        v.as_object_mut().unwrap().remove("hash");
        let hash = lakelands_api::canonical::sha256_hex(&v);
        v["hash"] = serde_json::json!(hash);
        lakelands_api::engine::CanonicalEvent {
            hash: lakelands_api::event::Digest(hash),
            bytes: lakelands_api::canonical::to_vec(&v),
            ..e
        }
    };
    let id = &"finance".into();
    let made =
        lakelands_store::log::Draft::system(Timestamp(1), EventBody::LakeCreated { format: OLD });
    let (_, made) = lakelands_store::log::seal(
        id,
        Position(0),
        &lakelands_api::event::Digest::zero(),
        0,
        made,
    );
    let made = older(made, true);
    let (fence, holder) = (lease.fence, "the release before".to_owned());
    let taken =
        lakelands_store::log::Draft::system(Timestamp(2), EventBody::LeaseTaken { fence, holder });
    let (_, taken) = lakelands_store::log::seal(id, Position(1), &made.hash, fence, taken);
    engine.append(&lease, &[made, older(taken, false)]).unwrap();
    engine.release_lease(&lease).unwrap();
    drop(engine);
    // The refusal names a command that runs, and advises it.
    let owed = sh.run(&["verify"]);
    let named = "lakelands upgrade --lake finance";
    assert!(owed.err.contains(&format!("`{named}`")), "{}", owed.err);
    assert!(
        owed.err.contains("what to do: run `lakelands upgrade`"),
        "{}",
        owed.err
    );
    ok(
        &sh.run(&["upgrade", "--check"]),
        &format!(
            "finance is written in log format {OLD}: an open for writing appends `lake_upgraded` \
             from {OLD} to {NEW}, and rebuilds the projection where an older build laid it \
             out; nothing was opened for writing\n"
        ),
    );
    let run = sh.run(&named.split_whitespace().skip(1).collect::<Vec<_>>());
    let said = format!("upgraded finance from log format {OLD} to {NEW} through position ");
    assert!(
        run.code == 0 && run.out.starts_with(&said),
        "{} {}{}",
        run.code,
        run.out,
        run.err
    );
    ok(
        &sh.run(&["upgrade"]),
        &format!("finance is written in log format {NEW}: nothing to upgrade\n"),
    );
}

/// `upgrade` on a lake declared and not yet written says what `verify`
/// says, and exits as it does.
#[test]
fn upgrade_says_a_lake_no_command_has_written_is_not_there_yet() {
    let sh = Shell::on("upgrade-unwritten", Backend::Store);
    sh.lake();
    for args in [&["upgrade", "--check"][..], &["upgrade"]] {
        let run = sh.run(args);
        assert_eq!((run.code, run.out.as_str()), (3, ""));
        assert!(
            run.err
                .starts_with("error: there is no lake `finance` to upgrade yet\n"),
            "{}",
            run.err
        );
        assert!(
            run.err
                .contains("what to do: a lake is made by the first command that writes"),
            "{}",
            run.err
        );
    }
    assert_eq!(sh.run(&["verify"]).code, 3);
}

/// `upgrade` on a lake of this release's format whose projection an
/// older build laid out (lakelands#1151): `--check` says the open for
/// writing rebuilds it and opens none; the upgrade rebuilds it once.
#[test]
fn upgrade_rebuilds_a_projection_an_older_build_laid_out() {
    let sh = Shell::on("upgrade-layout", Backend::Store);
    sh.lake();
    let file = sh.root.join("lakes").join("finance.json");
    let mut at: lakelands_api::Location =
        serde_json::from_str(&fs::read_to_string(file).unwrap()).unwrap();
    let lakelands_api::location::EngineConfig::Sqlite { dir, .. } = &mut at.engine else {
        panic!("a store lake is SQLite's")
    };
    *dir = sh.root.join(&*dir);
    let path = dir.join("projection.db");
    // Made outside the shell, whose process keeps every store it opened.
    let seams = Seams {
        engines: vec![Arc::new(lakelands_engine_sqlite::SqliteOpener)],
        ..Seams::default()
    };
    block_on(async {
        let store = lakelands_store::open(at, seams, OpenMode::Create)
            .await
            .unwrap();
        store.close().await.unwrap();
    });
    let db = rusqlite::Connection::open(&path).unwrap();
    let wanted: i64 =
        (db.query_row("SELECT v FROM meta WHERE k = 'layout'", [], |r| r.get(0))).unwrap();
    let older = wanted - 1;
    db.execute_batch(&format!(
        "DROP INDEX routes_source; ALTER TABLE routes DROP COLUMN source;
         UPDATE meta SET v = {older} WHERE k = 'layout';"
    ))
    .unwrap();
    drop(db);
    let v = lakelands_api::event::LOG_FORMAT_VERSION;
    ok(
        &sh.run(&["upgrade", "--check"]),
        &format!(
            "finance is written in log format {v}, its projection in layout {older}: an open \
             for writing rebuilds it; nothing was opened for writing\n"
        ),
    );
    let run = sh.run(&["upgrade"]);
    let said = format!("rebuilt the projection of finance from layout {older} in ");
    assert!(
        run.code == 0 && run.out.starts_with(&said),
        "{} {}{}",
        run.code,
        run.out,
        run.err
    );
    // The disk, not the shell's store: the projection is laid out anew.
    let db = rusqlite::Connection::open(&path).unwrap();
    let now: i64 =
        (db.query_row("SELECT v FROM meta WHERE k = 'layout'", [], |r| r.get(0))).unwrap();
    let source: i64 = (db.query_row(
        "SELECT count(*) FROM pragma_table_info('routes') WHERE name = 'source'",
        [],
        |r| r.get(0),
    ))
    .unwrap();
    assert_eq!(
        (now, source),
        (wanted, 1),
        "the projection is at layout {wanted} again"
    );
}

/// `build-ahead` beside the shell's writer, which holds the lake
/// (lakelands#1322): it says the room it takes before it starts and opens
/// the log and its own scratch only, so the projection, unreadable while
/// it runs, is not opened and the writer keeps its lease and writes on;
/// `upgrade` of the projection marked a layout back then folds only what
/// was written after the build. `--discard` removes a build ahead, and
/// says so where none is kept.
#[test]
fn build_ahead_builds_beside_the_writer_and_the_upgrade_goes_on_from_it() {
    use std::os::unix::fs::PermissionsExt;
    let sh = Shell::on("build-ahead", Backend::Store);
    sh.lake();
    assert_eq!(sh.run(&["apply", &sh.fixture()]).code, 0);
    let file = sh.root.join("lakes").join("finance.json");
    let at: Location = serde_json::from_str(&fs::read_to_string(file).unwrap()).unwrap();
    let EngineConfig::Sqlite { dir, .. } = &at.engine else {
        panic!("a store lake is SQLite's")
    };
    let path = sh.root.join(dir).join("projection.db");
    let mode = |m: u32| fs::set_permissions(&path, fs::Permissions::from_mode(m)).unwrap();
    mode(0o000);
    let run = sh.run(&["build-ahead"]);
    mode(0o644);
    let said: Vec<&str> = run.out.lines().collect();
    assert!(
        run.code == 0
            && said.len() == 2
            && said[0].starts_with("building ahead takes about ")
            && said[1].starts_with("built finance's projection ahead through position "),
        "{} {}{}",
        run.code,
        run.out,
        run.err
    );
    let bank = sh.file(
        "bank.ndjson",
        "{\"type\": \"invoice\", \"key\": \"INV-9\", \"fields\": {\"amount\": \"9.00\"}}\n",
    );
    let wrote = sh.run(&["ingest", &bank, "--source", "bank"]);
    assert_eq!(wrote.code, 0, "the writer writes on: {}", wrote.err);
    // The release before is stopped; this one finds its layout older.
    sh.lakes.lock().unwrap().clear();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("UPDATE meta SET v = v - 1 WHERE k = 'layout'", [])
        .unwrap();
    drop(db);
    let run = sh.run(&["upgrade"]);
    assert!(
        run.code == 0
            && run
                .out
                .starts_with("rebuilt the projection of finance from layout ")
            && run
                .out
                .ends_with(", folding only what was written after the build ahead\n"),
        "{} {}{}",
        run.code,
        run.out,
        run.err
    );
    let none = "finance kept no projection built ahead: nothing was removed\n";
    ok(&sh.run(&["build-ahead", "--discard"]), none);
    assert_eq!(sh.run(&["build-ahead"]).code, 0);
    ok(
        &sh.run(&["build-ahead", "--discard"]),
        "removed the projection built ahead beside finance\n",
    );
    ok(&sh.run(&["build-ahead", "--discard"]), none);
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
        // The export is a chain a verifier can follow.
        let files = lakelands_core::export::verify(&sh.root.join("out")).unwrap();
        assert_eq!(files.through, lakelands_api::Position(7));
        fails(
            &sh.run(&["export", "--to", &to]),
            "error: refused: the export's directory is not empty; an export writes only into an \
             empty or new directory\n  what to do: export into an empty or new directory\n",
        );
        let empty = sh.run(&["export", "--to", ""]);
        assert_eq!((empty.code, empty.out.as_str()), (2, ""), "{}", empty.err);
    }
}

/// A root's `lakes_dir` is where `lake new` puts a SQLite lake, and its
/// `exports_dir` where `export` writes with no `--to`: a directory of
/// its own, named by the lake and the moment.
#[test]
fn lake_new_and_export_place_by_the_root_s_directories() {
    let sh = Shell::on("placed", Backend::Store);
    let root = sh.root.display().to_string();
    assert_eq!(sh.raw(&["lakelands", "init", &root]).code, 0);
    let file = sh.root.join("lakelands.json");
    let mut config: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    config["lakes_dir"] = "stores".into();
    config["exports_dir"] = "out".into();
    fs::write(&file, config.to_string()).unwrap();
    assert_eq!(sh.run(&["lake", "new", "finance"]).code, 0);
    let location = fs::read_to_string(sh.root.join("lakes/finance.json")).unwrap();
    let location: Location = serde_json::from_str(&location).unwrap();
    assert_eq!(location, Location::sqlite("finance", "stores/finance"));
    sh.run(&["apply", &sh.fixture()]);
    let said = sh.run(&["export"]);
    assert_eq!(said.code, 0, "{}", said.err);
    let made: Vec<_> = fs::read_dir(sh.root.join("out"))
        .unwrap()
        .flatten()
        .collect();
    let name = made[0].file_name().into_string().unwrap();
    assert_eq!(
        (made.len(), name.len()),
        (1, "finance-20261024T120000Z".len()),
        "{name}"
    );
    assert!(
        name.starts_with("finance-2") && name.ends_with('Z'),
        "{name}"
    );
    let files = lakelands_core::export::verify(&made[0].path()).unwrap();
    assert_eq!(files.through, lakelands_api::Position(7));
    assert!(sh.root.join("stores/finance/log").is_dir());
}

/// `feed` and `export` open a place that holds no lake yet to create
/// it. A lake that is refused as it stands is not such a place: a lake
/// whose projection is ahead of its log is asked for once, to read, and
/// no file of its log is cut (lakelands#865).
#[test]
fn feed_and_export_do_not_open_a_damaged_lake_to_create_it() {
    let sh = Shell::on("export-damaged", Backend::Store);
    sh.lake();
    sh.run(&["apply", &sh.fixture()]);
    let held: Vec<_> = sh.lakes.lock().unwrap().values().cloned().collect();
    for store in held {
        block_on(store.close()).unwrap();
    }
    // The lake's directory, and its index emptied: the log reads as
    // holding nothing, under a projection folded through its events.
    fn find(dir: &std::path::Path, name: &str) -> Option<PathBuf> {
        fs::read_dir(dir).ok()?.flatten().find_map(|e| {
            let path = e.path();
            if path.file_name().is_some_and(|n| n == name) {
                return Some(path);
            }
            path.is_dir().then(|| find(&path, name))?
        })
    }
    let index = find(&sh.root, "00000000000000000000.idx").expect("the lake's index");
    let segment = find(&sh.root, "00000000000000000000.jsonl").expect("the lake's segment");
    fs::write(&index, b"").unwrap();
    let lines = fs::read(&segment).unwrap();
    assert!(!lines.is_empty());
    let to = sh.root.join("out").display().to_string();
    for args in [vec!["export", "--to", &to], vec!["feed"]] {
        let asked = Mutex::new(Vec::new());
        let open = |l: Location, mode: OpenMode, host: Seams| -> Opening {
            asked.lock().unwrap().push(mode);
            let engines: Vec<Arc<dyn lakelands_api::engine::EngineOpener>> =
                vec![Arc::new(lakelands_engine_sqlite::SqliteOpener)];
            Box::pin(lakelands_store::open(l, Seams { engines, ..host }, mode))
        };
        let root = sh.root.display().to_string();
        let mut argv = vec!["lakelands", "--root", root.as_str()];
        argv.extend_from_slice(&args);
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = block_on(lakelands_cli::run(argv, &mut out, &mut err, &open));
        let err = String::from_utf8(err).unwrap();
        assert_eq!((code, out.as_slice()), (1, &b""[..]), "{args:?}: {err}");
        assert!(err.contains("damaged"), "{args:?}: {err}");
        assert_eq!(*asked.lock().unwrap(), [OpenMode::Read], "{args:?}");
        assert_eq!(fs::read(&segment).unwrap(), lines, "{args:?} cut the log");
        assert_eq!(fs::read(&index).unwrap(), b"", "{args:?}");
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

/// A host's keys are its own: an empty key, a key two hosts share and a
/// key that is also a principal's are refused when the root is opened,
/// naming the host and never the key.
#[test]
fn the_server_section_refuses_a_host_key_that_is_not_its_own() {
    let sh = Shell::on("host-keys", Backend::Store);
    sh.lake();
    let writer = r#"[{"name": "writer", "kind": "agent", "id": "writer",
        "grants": {"default": "write"}, "api_keys": ["k-writer"]}]"#;
    let hosts = [
        (
            r#"[{"id": "edge", "keys": [""]}]"#,
            "host `edge` has no key, or an empty one",
        ),
        (
            r#"[{"id": "edge", "keys": []}]"#,
            "host `edge` has no key, or an empty one",
        ),
        (
            r#"[{"id": "edge", "keys": ["k-h"]}, {"id": "near", "keys": ["k-n", "k-h"]}]"#,
            "a key of host `near` is also host `edge`'s",
        ),
        (
            r#"[{"id": "edge", "keys": ["k-writer"]}]"#,
            "a key of host `edge` is also an api key of principal `writer`",
        ),
    ];
    for (hosts, why) in hosts {
        let config = format!(
            r#"{{"lakelands": 1, "owner": "owner", "principals": {writer},
                "server": {{"bind": "127.0.0.1:0", "hosts": {hosts}}}}}"#
        );
        fs::write(sh.root.join("lakelands.json"), config).unwrap();
        let s = sh.run(&["feed"]);
        assert_eq!(s.code, 1, "{hosts}: {}", s.out);
        assert!(
            s.err
                .starts_with("error: <root>/lakelands.json: server.hosts: ")
                && s.err.contains(why)
                && !s.err.contains("k-")
                && s.err.contains("\n  what to do: fix the server section"),
            "{hosts}: {}",
            s.err
        );
    }
    let config = format!(
        r#"{{"lakelands": 1, "owner": "owner", "principals": {writer},
            "server": {{"bind": "127.0.0.1:0", "hosts": [{{"id": "edge", "keys": ["k-h"]}}]}}}}"#
    );
    fs::write(sh.root.join("lakelands.json"), config).unwrap();
    let s = sh.run(&["feed"]);
    assert_eq!(s.code, 0, "keys of its own: {}", s.err);
}

/// A root that allows an `exec` command computes a field with it: the
/// lake declares a function naming the command, a record is ingested,
/// and the store calls the command and records its answer.
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

/// `import` goes through the store over the binary's own engines, which
/// keep the files it imports into.
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

/// `backup` brings its directory up to the head, writing only what is new,
/// and copies the keys apart from it; `restore --check` verifies it, and
/// `restore` puts the keys back where the keyring holds none, then imports.
#[test]
fn backup_grows_its_directory_and_restore_makes_the_lake_with_its_keys() {
    let keyring = r#"{"lakelands": 1, "owner": "owner", "keys": {"keyring": "keys"}}"#;
    let from = Shell::on("backup-from", Backend::Store);
    from.lake();
    fs::write(from.root.join("lakelands.json"), keyring).unwrap();
    from.run(&["apply", &from.fixture()]);
    // A source's erasable value, sealed under a key of the keyring.
    let contact =
        "name: contact\nfields:\n  email: { type: text, authority: source, erasable: true }\n";
    let applied = from.run(&[
        "apply",
        &from.bundle("contact", &[("types/contact.yaml", contact)]),
    ]);
    assert_eq!(applied.code, 0, "{}", applied.err);
    let row =
        "{\"type\": \"contact\", \"key\": \"c1\", \"fields\": {\"email\": \"ada@example.org\"}}\n";
    let fed = from.run(&["ingest", &from.file("crm.ndjson", row), "--source", "crm"]);
    assert_eq!(fed.code, 0, "{}", fed.err);
    let (dir, keys) = (from.root.join("backup"), from.root.join("backup-keys"));
    let (dir, keys) = (dir.display().to_string(), keys.display().to_string());
    let (head, _) = last_event(&from.run(&["feed"]).out);
    ok(
        &from.run(&["backup", "--to", &dir, "--keys-to", &keys]),
        &format!(
            "backed up finance through position {head} ({} new event(s), new) to <root>/backup\n\
             copied the lake's keys to <root>/backup-keys/finance.keys.json\n",
            head + 1
        ),
    );
    ok(
        &from.run(&["backup", "--to", &dir]),
        &format!(
            "backed up finance through position {head} (0 new event(s), since {head}) to <root>/backup\n"
        ),
    );
    from.run(&[
        "apply",
        &from.bundle(
            "more",
            &[
                ("types/partner.yaml", PARTNER),
                ("types/note.yaml", "name: note\nfields:\n  text: text\n"),
            ],
        ),
    ]);
    let (now, hash) = last_event(&from.run(&["feed"]).out);
    assert!(now > head);
    let grown = from.run(&["backup", "--to", &dir]);
    assert!(
        grown
            .out
            .contains(&format!("({} new event(s), since {head})", now - head)),
        "{}",
        grown.err
    );
    // The keys are never kept with the backup.
    let inside = format!("{dir}/keys");
    let mixed = from.run(&["backup", "--to", &dir, "--keys-to", &inside]);
    assert!(
        mixed.code == 1 && mixed.err.contains("one inside the other"),
        "{}",
        mixed.err
    );
    // Nor is a backup written into a lake's directory.
    let lake = from.root.join("lakes/finance").display().to_string();
    let live = from.run(&["backup", "--to", &lake]);
    assert!(
        live.code == 1 && live.err.contains("inside the lake `finance`'s directory"),
        "{}",
        live.err
    );

    let check = from.run(&["restore", &dir, "--check"]);
    assert_eq!(check.code, 0, "{}", check.err);
    assert!(
        check
            .out
            .contains(&format!("through {now} (last hash {hash})")),
        "{}",
        check.out
    );

    let to = Shell::on("backup-to", Backend::Store);
    to.lake();
    fs::write(to.root.join("lakelands.json"), keyring).unwrap();
    // A refused restore takes the keys it put back out again.
    let empty = to.root.join("empty");
    fs::create_dir_all(&empty).unwrap();
    let refused = to.run(&[
        "restore",
        &empty.display().to_string(),
        "--keys-from",
        &keys,
    ]);
    assert!(
        refused.code != 0 && !to.root.join("keys/finance.keys.json").exists(),
        "{}",
        refused.err
    );
    // An output closed after its first line does not take the keys of a
    // lake it imported out again.
    struct Closed(Vec<u8>);
    impl std::io::Write for Closed {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            if self.0.contains(&b'\n') {
                return Err(std::io::ErrorKind::BrokenPipe.into());
            }
            self.0.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let (root, mut out, mut err) = (
        to.root.display().to_string(),
        Closed(Vec::new()),
        Vec::new(),
    );
    let argv = [
        "lakelands",
        "--root",
        &root,
        "restore",
        &dir,
        "--keys-from",
        &keys,
    ];
    let open = |l: Location, m: OpenMode, s: Seams| to.open(l, m, s);
    let code = block_on(lakelands_cli::run(argv, &mut out, &mut err, &open));
    assert!(
        code != 0
            && out.0 == format!("put the keys of lake `finance` back from {keys}\n").as_bytes()
            && to.root.join("keys/finance.keys.json").exists(),
        "{}",
        String::from_utf8_lossy(&err)
    );
    assert_eq!(to.run(&["feed"]).out, from.run(&["feed"]).out);
    // The erasable value reads back under the keys put back.
    let store = to.lakes.lock().unwrap()["finance"].clone();
    let get = serde_json::from_value(serde_json::json!({"key": "c1"})).unwrap();
    let (doors, owner) = (fixture::doors(&*store), fixture::owner(&*store));
    let got = block_on(doors.get(&owner, &lakelands_api::id::TypeName::from("contact"), get));
    assert!(format!("{got:?}").contains("ada@example.org"), "{got:?}");
    // The keyring's own file is never replaced by a copy.
    let twice = to.run(&["restore", &dir, "--keys-from", &keys]);
    assert!(
        twice.code == 1 && twice.err.contains("already holds a file"),
        "{}",
        twice.err
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

/// `export` only reads, each command in a process of its own as an
/// operator runs them: it appends nothing to the lake it exports, and it
/// runs while a `serve` holds the lake's writer.
#[test]
fn the_built_binary_exports_without_a_writer_beside_a_running_serve() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Command, Stdio};

    let sh = Shell::on("export-reads", Backend::Store);
    let lakelands = env!("CARGO_BIN_EXE_lakelands");
    let run = |args: &[&str]| {
        Command::new(lakelands)
            .arg("--root")
            .arg(&sh.root)
            .args(args)
            .output()
            .unwrap()
    };
    let ok = |args: &[&str]| {
        let out = run(args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    ok(&["init"]);
    ok(&["lake", "new", "finance"]);
    ok(&["apply", &sh.fixture()]);
    let head = |feed: &str| last_event(feed).0;
    let before = head(&ok(&["feed"]));
    let first = sh.root.join("first").display().to_string();
    let said = ok(&["export", "--to", &first]);
    assert!(
        said.ends_with(&format!(
            "exported finance through position {before} to {first}\n"
        )),
        "{said}"
    );
    assert_eq!(head(&ok(&["feed"])), before, "the export appended nothing");

    // A `serve` holds the writer while its client is connected.
    let mut server = Command::new(lakelands)
        .arg("--root")
        .arg(&sh.root)
        .args(["serve", "--mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut to = server.stdin.take().unwrap();
    writeln!(
        to,
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-11-25","capabilities":{{}},"clientInfo":{{"name":"t","version":"1"}}}}}}"#
    )
    .unwrap();
    let mut answer = String::new();
    BufReader::new(server.stdout.take().unwrap())
        .read_line(&mut answer)
        .unwrap();
    assert!(answer.contains("serverInfo"), "{answer}");
    let served = head(&ok(&["feed"]));
    assert!(served > before, "serve took the writer's lease");
    let second = sh.root.join("second").display().to_string();
    let beside = run(&["export", "--to", &second]);
    assert!(
        beside.status.success(),
        "an export runs beside a serve: {}",
        String::from_utf8_lossy(&beside.stderr)
    );
    assert_eq!(head(&ok(&["feed"])), served, "the export appended nothing");
    drop(to);
    assert!(server.wait().unwrap().success());
}

/// `forget` (058): with the root's keyring, a source's erasable values
/// enter the log sealed; the preflight says what the act erases and
/// retires and how long it holds the writer, the act under its id erases
/// them and says through which position, and the source then writes
/// nothing more.
#[test]
fn forget_erases_a_sources_values_and_prints_what_it_did() {
    const TYPES: [(&str, &str); 1] = [(
        "types/contact.yaml",
        "name: contact\nfields:\n  email: { type: text, authority: source, erasable: true }\n",
    )];
    for backend in BACKENDS {
        let sh = Shell::on("forget", backend);
        sh.lake();
        let config = r#"{"lakelands": 1, "owner": "owner", "keys": {"keyring": "keys"}}"#;
        fs::write(sh.root.join("lakelands.json"), config).unwrap();
        let applied = sh.run(&["apply", &sh.bundle("types", &TYPES)]);
        assert_eq!(applied.code, 0, "{backend:?}: {}", applied.err);
        let rows = sh.file(
            "crm.ndjson",
            "{\"type\": \"contact\", \"key\": \"c1\", \"fields\": {\"email\": \"ada@example.org\"}}\n",
        );
        let fed = sh.run(&["ingest", &rows, "--source", "crm"]);
        assert_eq!(fed.code, 0, "{backend:?}: {}", fed.err);
        // Without --report it acts not: it answers the preflight.
        let bare = sh.run(&["forget", "--source", "crm"]);
        assert!(
            bare.out.contains("forget with --report "),
            "{backend:?}: {} {}",
            bare.out,
            bare.err
        );
        let seen = sh.run(&["forget", "--source", "crm", "--preflight"]);
        let said = "contact.email: would erase 1 value(s)\ncontact: retires 1 record(s) crm made and alone brought\ncrm: preflight at position ";
        assert!(
            seen.out.starts_with(said),
            "{backend:?}: {} {}",
            seen.out,
            seen.err
        );
        let id = seen
            .out
            .rsplit("--report ")
            .next()
            .map(str::trim)
            .unwrap_or_default();
        assert!(
            seen.out.contains("the act holds the writer about"),
            "{}",
            seen.out
        );
        let done = sh.run(&["forget", "--source", "crm", "--report", id]);
        assert!(
            done.code == 0
                && done.out.starts_with("contact.email: erased 1 value(s)\ncontact: retires 1 record(s) crm made and alone brought\nforgot crm through position "),
            "{backend:?}: {} {}",
            done.out,
            done.err
        );
        let again = sh.run(&["ingest", &rows, "--source", "crm"]);
        assert!(
            again.code != 0 && again.err.contains("was forgotten"),
            "{backend:?}: {}",
            again.err
        );
    }
}

/// `values remove` (065): without --report, as with --preflight, it says
/// the report's sentences and the id, appending nothing; a stale id is
/// refused in the store's words, and so is `--require-shredded` while a
/// value of an erasable field would be redacted only, naming the count;
/// `--before`, `--delivery` and `--seen-before` are one selection, given
/// once, `--delivery` repeated a set of them (lakelands#1290), and
/// `--seen-before` counts the events that state no `seen_at`
/// apart (lakelands#1166); the act
/// under the preflight's id takes the values back and says through which
/// position, and, where its reclaim fails, that the reclaim is owed and
/// why, as the preflights after it say (lakelands#1179).
#[test]
fn values_remove_takes_back_part_of_a_source_and_prints_what_it_did() {
    const TYPES: [(&str, &str); 1] = [(
        "types/visit.yaml",
        "name: visit\nfields:\n  place: { type: text, authority: source }\n  email: { type: text, authority: source, erasable: true }\n  note: { type: text }\n",
    )];
    for backend in BACKENDS {
        let sh = Shell::on("values-remove", backend);
        sh.lake();
        let config = r#"{"lakelands": 1, "owner": "owner", "keys": {"keyring": "keys"}}"#;
        fs::write(sh.root.join("lakelands.json"), config).unwrap();
        let applied = sh.run(&["apply", &sh.bundle("types", &TYPES)]);
        assert_eq!(applied.code, 0, "{backend:?}: {}", applied.err);
        // Two deliveries under one version of `tracker`'s key, the cut
        // between them: the version also seals what the cut leaves.
        let mut cut = String::new();
        for (n, at) in [("v1", "Rome"), ("v2", "Oslo")] {
            let row = format!(
                "{{\"type\": \"visit\", \"key\": \"{n}\", \"fields\": {{\"place\": \"{at}\", \"email\": \"{n}@example.org\"}}}}\n"
            );
            let fed = sh.run(&[
                "ingest",
                &sh.file(&format!("{n}.ndjson"), &row),
                "--source",
                "tracker",
            ]);
            assert_eq!(fed.code, 0, "{backend:?}: {}", fed.err);
            if cut.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(20));
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH);
                let micros = now.unwrap().as_micros() as i64;
                cut = lakelands_api::Timestamp(micros).to_string();
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        let remove = |more: &[&str]| {
            let mut args = vec!["values", "remove", "--source", "tracker"];
            args.extend_from_slice(more);
            sh.run(&args)
        };
        let all = ["--before", cut.as_str()];
        let feed = sh.run(&["feed"]).out;
        let bare = remove(&all);
        let seen = remove(&[&all[..], &["--preflight"]].concat());
        let said = format!("1 event of `tracker` written before {cut}");
        assert!(
            [&bare, &seen].iter().all(|r| r.code == 0
                && r.out.starts_with(&said)
                && !r.out.contains("no time seen")
                && r.out.contains("remove with --report ")),
            "{backend:?}: {} {} / {} {}",
            bare.out,
            bare.err,
            seen.out,
            seen.err
        );
        assert_eq!(
            sh.run(&["feed"]).out,
            feed,
            "{backend:?}: the preflight appends nothing"
        );
        let id = seen
            .out
            .rsplit("--report ")
            .next()
            .map(str::trim)
            .unwrap_or_default();
        let day = lakelands_api::Timestamp::parse("2026-01-01").unwrap();
        let unseen = remove(&["--seen-before", "2026-01-01"]);
        assert!(
            unseen.code == 0
                && unseen.out.starts_with(&format!(
                    "0 events of `tracker` seen before {day} are selected."
                ))
                && unseen
                    .out
                    .contains("\n2 events state no time seen and stay.\n"),
            "{backend:?}: {} {}",
            unseen.out,
            unseen.err
        );
        let set = remove(&["--delivery", "a", "--delivery", "b", "--preflight"]);
        assert!(
            set.code == 0
                && set
                    .out
                    .starts_with("0 events of `tracker` of 2 deliveries (`a`, `b`) are selected."),
            "{backend:?}: {} {}",
            set.out,
            set.err
        );
        for usage in [
            remove(&[]),
            remove(&[&all[..], &["--delivery", "d1"]].concat()),
            remove(&[&all[..], &["--seen-before", "2026-01-01"]].concat()),
            remove(&[&all[..], &["--preflight", "--report", id]].concat()),
        ] {
            assert!(usage.code == 2, "{backend:?}: a usage error: {}", usage.err);
        }
        let stale = remove(&[&all[..], &["--report", "0"]].concat());
        assert!(
            stale.code != 0
                && stale
                    .err
                    .contains("that report is not of this removal at this head"),
            "{backend:?}: {}",
            stale.err
        );
        let shredded = remove(&[&all[..], &["--require-shredded", "--report", id]].concat());
        assert!(
            shredded.code != 0
                && shredded
                    .err
                    .contains("1 value(s) of erasable fields of `tracker`"),
            "{backend:?}: {} {}",
            shredded.out,
            shredded.err
        );
        assert_eq!(
            sh.run(&["feed"]).out,
            feed,
            "{backend:?}: the refusals append nothing"
        );
        // The act's reclaim fails: it answers, owing it, and so do the
        // preflights after it.
        sh.fails[0].store(1, std::sync::atomic::Ordering::SeqCst);
        let done = remove(&[&all[..], &["--report", id]].concat());
        let owed = "an act's reclaim is still owed, tried again at the next open or compact: lake ";
        assert!(
            done.code == 0
                && done.out.starts_with(&said)
                && done
                    .out
                    .contains("\ntook back from tracker through position ")
                && done.out.contains(&format!("; {owed}")),
            "{backend:?}: {} {}",
            done.out,
            done.err
        );
        for pre in [
            remove(&all),
            sh.run(&["forget", "--source", "tracker", "--preflight"]),
        ] {
            assert!(
                pre.out.contains(&format!("tracker: {owed}")),
                "{backend:?}: {} {}",
                pre.out,
                pre.err
            );
        }
    }
}

/// A keyring inside a lake's directory, or its projection's, would travel
/// with the lake's files, a destroyed key with it: the root refuses it (058).
#[test]
fn a_keyring_inside_a_lakes_directory_is_refused() {
    let sh = Shell::on("keyring-inside", Backend::Store);
    sh.lake();
    let file = sh.root.join("lakes/finance.json");
    let mut location: serde_json::Value =
        serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    location["engine"]["projection_dir"] = "projections/finance".into();
    fs::write(&file, location.to_string()).unwrap();
    // Named straight, through `..`, and through a link, which is followed.
    #[cfg(unix)]
    std::os::unix::fs::symlink(sh.root.join("lakes"), sh.root.join("shelf")).unwrap();
    let mut named = vec![
        "lakes/finance/keys",
        "elsewhere/../lakes/finance/keys",
        "projections/finance/keys",
    ];
    if cfg!(unix) {
        named.push("shelf/finance/keys");
    }
    for keyring in named {
        let config =
            format!(r#"{{"lakelands": 1, "owner": "owner", "keys": {{"keyring": "{keyring}"}}}}"#);
        fs::write(sh.root.join("lakelands.json"), config).unwrap();
        let refused = sh.run(&["feed"]);
        assert!(
            refused.code != 0
                && refused
                    .err
                    .contains("is inside the lake `finance`'s directory"),
            "{keyring}: {}",
            refused.err
        );
    }
}

/// The members a struct declares, as its derived `Deserialize` names
/// them to `deserialize_struct`: read from the type, so a member added is
/// seen whether or not a configuration holds it.
fn members_of<'de, T: serde::Deserialize<'de>>() -> &'static [&'static str] {
    use serde::de::{Error, Visitor, value};
    struct Names<'a>(&'a mut &'static [&'static str]);
    impl<'de> serde::Deserializer<'de> for Names<'_> {
        type Error = value::Error;
        fn deserialize_any<V: Visitor<'de>>(self, _: V) -> Result<V::Value, value::Error> {
            Err(value::Error::custom("not a struct"))
        }
        fn deserialize_struct<V: Visitor<'de>>(
            self,
            _: &'static str,
            fields: &'static [&'static str],
            _: V,
        ) -> Result<V::Value, value::Error> {
            *self.0 = fields;
            Err(value::Error::custom("only its members are read"))
        }
        serde::forward_to_deserialize_any! {
            bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes
            byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct map enum
            identifier ignored_any
        }
    }
    let mut names: &'static [&'static str] = &[];
    let _ = T::deserialize(Names(&mut names));
    names
}

/// Every member of the root's `lakelands.json`, read from `Config` and
/// each struct of its sections, is a root setting or binding of the list,
/// and each one of the list is such a member: a section or a member added
/// to the configuration and not listed is red, as is one listed that no
/// struct holds. `lakelands`, `owner` and `principals` are neither (063).
/// A setting's default is the section's.
#[test]
fn the_root_s_settings_are_the_list_s() {
    use lakelands_api::settings::{BINDINGS, Layer, SETTINGS};
    use std::collections::BTreeSet;
    let sections: [(&str, &[&str]); 12] = [
        ("", members_of::<lakelands_cli::Config>()),
        ("server", members_of::<lakelands_server::HttpConfig>()),
        (
            "server.metrics",
            members_of::<lakelands_server::MetricsConfig>(),
        ),
        ("aging", members_of::<lakelands_cli::ticker::AgingConfig>()),
        ("deliver", members_of::<lakelands_backings::DeliverConfig>()),
        ("embed", members_of::<lakelands_backings::EmbedConfig>()),
        ("backings", members_of::<lakelands_backings::Config>()),
        (
            "backings.http",
            members_of::<lakelands_backings::HttpConfig>(),
        ),
        (
            "backings.exec",
            members_of::<lakelands_backings::ExecConfig>(),
        ),
        (
            "mirror",
            members_of::<lakelands_cli::mirror::MirrorConfig>(),
        ),
        ("keys", members_of::<lakelands_cli::KeysConfig>()),
        ("standby", members_of::<lakelands_cli::StandbyConfig>()),
    ];
    let neither = ["lakelands", "owner", "principals"];
    let path = |s: &str, m: &str| match s {
        "" => m.to_owned(),
        s => format!("{s}.{m}"),
    };
    let every: BTreeSet<String> = (sections.iter())
        .flat_map(|(s, ms)| ms.iter().map(|m| path(s, m)))
        .collect();
    for n in neither {
        assert!(every.contains(n), "`{n}` is a member of the root");
    }
    for (s, ms) in &sections[1..] {
        assert!(
            !ms.is_empty() && every.contains(*s),
            "`{s}` is a section of the root"
        );
    }
    let declared: BTreeSet<String> = (every.iter())
        .filter(|k| !neither.contains(&k.as_str()) && !sections.iter().any(|(s, _)| s == k))
        .cloned()
        .collect();
    let listed: BTreeSet<String> = (SETTINGS.iter().map(|s| (s.key, s.layer)))
        .chain(BINDINGS.iter().map(|b| (b.key, b.layer)))
        .filter(|(_, layer)| *layer == Layer::Root)
        .map(|(k, _)| k.to_owned())
        .collect();
    assert_eq!(declared, listed, "the root's members and the list's");
    let config: lakelands_cli::Config = serde_json::from_value(serde_json::json!({
        "lakelands": 1, "owner": "owner",
        "server": {"bind": "127.0.0.1:7700"},
        "aging": {},
        "deliver": {"endpoints": [], "ca_file": "ca.pem"},
        "embed": {"endpoint": "http://127.0.0.1:9/", "model": "m", "dimensions": 2,
            "api_key_file": "k", "ca_file": "ca.pem"},
        "backings": {"http": {"hosts": [], "ca_file": "ca.pem"}, "exec": {"commands": {}}},
        "mirror": {"connections": []},
        "keys": {"keyring": "keys"},
        "standby": {},
    }))
    .unwrap();
    let held = serde_json::to_value(&config).unwrap();
    for s in SETTINGS.iter().filter(|s| s.layer == Layer::Root) {
        let at = format!("/{}", s.key.replace('.', "/"));
        if !s.default_value().is_null() {
            assert_eq!(held.pointer(&at), Some(&s.default_value()), "`{}`", s.key);
        }
    }
}

/// `settings` lists every setting with its value for the lake; `set` and
/// `unset` write a lake setting through the store's judge, and a setting
/// kept in a file is refused with exit 2, naming the file.
#[test]
fn settings_lists_every_setting_and_sets_only_the_lake_s() {
    for backend in BACKENDS {
        let sh = Shell::on("settings", backend);
        sh.lake();
        sh.run(&["apply", &sh.fixture()]);
        let set = sh.run(&[
            "settings",
            "set",
            "sort_index",
            r#"["amount"]"#,
            "--type",
            "invoice",
        ]);
        ok(
            &set,
            "set `sort_index.invoice` to [\"amount\"] at position 8\n",
        );
        ok(&sh.run(&["settings"]), SETTINGS_SET);

        let bad = sh.run(&[
            "settings",
            "set",
            "sort_index",
            r#"["nope"]"#,
            "--type",
            "invoice",
        ]);
        assert_eq!((bad.code, bad.out.as_str()), (1, ""));
        assert!(
            bad.err.contains("`sort_index.invoice`") && bad.err.contains("`nope`"),
            "{}",
            bad.err
        );
        let unlisted = sh.run(&["settings", "set", "sort_index", r#"["amount"]"#]);
        assert!(
            unlisted.code == 1 && unlisted.err.contains("`sort_index.`"),
            "{}",
            unlisted.err
        );

        let kept = sh.run(&["settings", "set", "limits.max_limit", "5"]);
        assert_eq!((kept.code, kept.out.as_str()), (2, ""));
        assert_eq!(
            kept.err,
            "error: `limits.max_limit` is not a lake setting: it is kept in lakes/finance.json\n  \
             what to do: edit `limits.max_limit` in lakes/finance.json and open the lake again\n"
        );
        // Its last part alone would be a host's key that nothing reads.
        let bare = sh.run(&["settings", "set", "max_limit", "5"]);
        assert_eq!((bare.code, bare.out.as_str()), (2, ""));
        assert!(
            bare.err.starts_with(
                "error: `max_limit` is not a lake setting: `limits.max_limit` is kept in lakes/finance.json\n"
            ),
            "{}",
            bare.err
        );
        let shared = sh.run(&["settings", "set", "timeout_secs", "5"]);
        assert!(
            shared.code == 2
                && shared
                    .err
                    .contains("`deliver.timeout_secs` in lakelands.json")
                && shared
                    .err
                    .contains("`embed.timeout_secs` in lakelands.json"),
            "{}",
            shared.err
        );
        let served = sh.run(&["settings", "unset", "server.bind"]);
        assert_eq!((served.code, served.out.as_str()), (2, ""));
        assert!(
            served.err.contains("it is kept in lakelands.json"),
            "{}",
            served.err
        );
        // So would its last parts, and a section's name.
        let suffix = sh.run(&["settings", "set", "tuning.mmap_mb", "5"]);
        assert!(
            suffix.code == 2
                && suffix.err.contains(
                    "`tuning.mmap_mb` is not a lake setting: `engine.tuning.mmap_mb` is kept in lakes/finance.json"
                ),
            "{}",
            suffix.err
        );
        let inner = sh.run(&["settings", "set", "tuning", "{}"]);
        assert!(
            inner.code == 2
                && inner.err.contains(
                    "`tuning` is not a lake setting: it is a section of lakes/finance.json"
                ),
            "{}",
            inner.err
        );
        let section = sh.run(&["settings", "set", "deliver", "{}"]);
        assert_eq!(
            (section.code, section.err.as_str()),
            (
                2,
                "error: `deliver` is not a lake setting: it is a section of lakelands.json\n  \
                 what to do: edit its members in lakelands.json and open the root again\n"
            )
        );

        let unset = sh.run(&["settings", "unset", "sort_index", "--type", "invoice"]);
        ok(&unset, "retired `sort_index.invoice` at position 9\n");
        let listed = sh.run(&["settings"]);
        assert!(!listed.out.contains("sort_index.invoice"), "{}", listed.out);

        // A configured section is read member by member, at each one's
        // whole path.
        let config = r#"{"lakelands": 1, "owner": "owner",
            "backings": {"http": {"hosts": ["geo.example"], "timeout_secs": 3}},
            "mirror": {"connections": [], "batch": 7}}"#;
        fs::write(sh.root.join("lakelands.json"), config).unwrap();
        let listed = sh.run(&["settings"]);
        let rows: Vec<String> = (listed.out.lines())
            .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect();
        for want in [
            "backings.http.timeout_secs root 3 (default 10)",
            "backings.http.max_reply_bytes root 1048576 (default)",
            "backings.exec.timeout_secs root not configured: lakelands.json has no `backings.exec`",
            "mirror.poll_millis root 1000 (default)",
            "mirror.batch root 7 (default 100)",
        ] {
            assert!(
                rows.iter().any(|r| r == want),
                "`{want}` in {}{}",
                listed.out,
                listed.err
            );
        }
    }
}

const SETTINGS_SET: &str = "\
sort_index.<type>                       lake      null (default)
sort_index.invoice                      lake      [\"amount\"] (default null)
vector_index.<type>                     lake      null (default)
lookup_index.<type>                     lake      null (default)
search_weights.<type>                   lake      null (default)
view_kept.<type>                        lake      false (default)
key_period                              lake      null (default)
limits.default_limit                    location  50 (default)
limits.max_limit                        location  1000 (default)
limits.max_depth                        location  16 (default)
limits.max_reactive_depth               location  8 (default)
limits.commit_window_events             location  256 (default)
limits.commit_window_max                location  4096 (default)
limits.max_graph_nodes                  location  100000 (default)
limits.max_offset                       location  100000 (default)
limits.max_set_changes                  location  20000 (default)
limits.max_change_records               location  10000 (default)
limits.max_rerank_top                   location  100 (default)
limits.max_text_as_of                   location  10000 (default)
limits.choose_order_index               location  true (default)
limits.order_after_pages                location  3 (default)
limits.order_window_secs                location  600 (default)
limits.order_min_records                location  2000 (default)
limits.order_max_fields                 location  2 (default)
limits.order_unused_secs                location  604800 (default)
limits.order_chunk                      location  10000 (default)
limits.forget_rebuild                   location  false (default)
limits.seal_after_bytes                 location  268435456 (default)
limits.quarantined_max                  location  10000 (default)
engine.tuning.writer_cache_kb           location  65536 (default)
engine.tuning.reader_cache_kb           location  16384 (default)
engine.tuning.mmap_mb                   location  256 (default)
engine.tuning.wal_autocheckpoint_pages  location  20000 (default)
server.bind                             root      not configured: lakelands.json has no `server`
server.allowed_origins                  root      not configured: lakelands.json has no `server`
server.session_idle_secs                root      not configured: lakelands.json has no `server`
server.feed_poll_millis                 root      not configured: lakelands.json has no `server`
server.feed_floor_millis                root      not configured: lakelands.json has no `server`
server.workers                          root      not configured: lakelands.json has no `server`
server.metrics.bind                     root      not configured: lakelands.json has no `server.metrics`
aging.utc_offset                        root      not configured: lakelands.json has no `aging`
aging.check_secs                        root      not configured: lakelands.json has no `aging`
deliver.poll_millis                     root      not configured: lakelands.json has no `deliver`
deliver.batch                           root      not configured: lakelands.json has no `deliver`
deliver.retry_first_secs                root      not configured: lakelands.json has no `deliver`
deliver.retry_max_secs                  root      not configured: lakelands.json has no `deliver`
deliver.timeout_secs                    root      not configured: lakelands.json has no `deliver`
deliver.max_reply_bytes                 root      not configured: lakelands.json has no `deliver`
embed.model                             root      not configured: lakelands.json has no `embed`
embed.dimensions                        root      not configured: lakelands.json has no `embed`
embed.send_dimensions                   root      not configured: lakelands.json has no `embed`
embed.max_batch                         root      not configured: lakelands.json has no `embed`
embed.timeout_secs                      root      not configured: lakelands.json has no `embed`
embed.max_reply_bytes                   root      not configured: lakelands.json has no `embed`
backings.http.timeout_secs              root      not configured: lakelands.json has no `backings.http`
backings.http.max_reply_bytes           root      not configured: lakelands.json has no `backings.http`
backings.exec.timeout_secs              root      not configured: lakelands.json has no `backings.exec`
backings.exec.max_output_bytes          root      not configured: lakelands.json has no `backings.exec`
mirror.poll_millis                      root      not configured: lakelands.json has no `mirror`
mirror.batch                            root      not configured: lakelands.json has no `mirror`
standby.try_millis                      root      not configured: lakelands.json has no `standby`
";

/// A bundle declares a host's setting and gives its first value in one
/// `apply`; applied again, it keeps the value written since. `settings`
/// lists it with its group, kind and unit, and `set` judges it by its kind.
#[test]
fn a_bundle_declares_a_setting_and_gives_its_first_value() {
    for backend in BACKENDS {
        let sh = Shell::on("host-setting", backend);
        sh.lake();
        let vat = "key: vat_rate\ntype: { kind: decimal }\nunit: fraction\ngroup: operator\nvalue: \"0.19\"\n";
        let bundle = sh.bundle("vat", &[("settings/vat_rate.yaml", vat)]);
        let first = sh.run(&["apply", &bundle]);
        assert_eq!(first.code, 0, "{}", first.err);
        assert!(
            first
                .out
                .contains("setting `vat_rate` given its first value"),
            "{}",
            first.out
        );
        ok(
            &sh.run(&["settings", "set", "vat_rate", "\"0.2\""]),
            "set `vat_rate` to \"0.2\" at position 6\n",
        );
        let again = sh.run(&["apply", &bundle]);
        assert!(
            again
                .out
                .contains("setting `vat_rate` holds a value already, kept"),
            "{}",
            again.out
        );
        let bad = sh.run(&["settings", "set", "vat_rate", "19%"]);
        assert!(bad.code == 1 && bad.err.contains("`value`"), "{}", bad.err);
        let listed = sh.run(&["settings"]);
        assert!(
            listed.out.lines().any(|l| l.starts_with("vat_rate ")
                && l.contains(" host ")
                && l.ends_with("\"0.2\" (group operator, decimal, in fraction)")),
            "{}",
            listed.out
        );
        // Unset since, it is not given its first value again.
        ok(
            &sh.run(&["settings", "unset", "vat_rate"]),
            "retired `vat_rate` at position 7\n",
        );
        let third = sh.run(&["apply", &bundle]);
        assert!(
            third
                .out
                .contains("setting `vat_rate` was unset since, kept unset"),
            "{}",
            third.out
        );
    }
}
