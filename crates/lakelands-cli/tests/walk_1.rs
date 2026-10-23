//! Walk 1's materials, kept from rotting: the liquidity bundle in
//! `examples/liquidity/` loads, and the commands `docs/walks/walk-1.md`
//! types — `init`, `lake new`, `apply`, and the three `ingest --map` runs
//! — succeed in the script's order on the binary's own `open_store`, the
//! real store over SQLite and in memory, with the same counts.
//!
//! The answers the walk expects are checked in `lakelands-store`'s
//! `walk_1` test, and here on the lake `init --example liquidity` makes
//! in one command from the files the binary carries.

use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};

use lakelands_api::engine::OpenMode;
use lakelands_api::location::Location;
use lakelands_api::read::{AggFunc, AggMeasure, Aggregate, Find, Found};
use lakelands_api::write::Declaration;
use lakelands_api::{Get, Grants, Position, Principal, PrincipalKind, ReadAt, WriteContext};
use lakelands_cli::Opening;
use lakelands_cli::example::EXAMPLES;

fn block_on<F: Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(f)
}

fn example() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/liquidity")
}

#[test]
fn the_liquidity_bundle_loads_with_every_lane_the_walk_needs() {
    let bundle = lakelands_core::bundle::load_dir(&example()).unwrap_or_else(|problems| {
        let lines: Vec<String> = problems.iter().map(ToString::to_string).collect();
        panic!("the liquidity bundle does not load:\n{}", lines.join("\n"))
    });
    let names: Vec<String> = bundle
        .declarations
        .iter()
        .map(|d| match d {
            Declaration::ValueType(v) => format!("value_type {}", v.name),
            Declaration::Type(t) => format!("type {}", t.name),
            Declaration::Function(f) => format!("function {}", f.name),
            Declaration::Verb(v) => format!("verb {}", v.name),
            Declaration::Trigger(t) => format!("trigger {}", t.name),
            Declaration::ObjectSet(o) => format!("object_set {}", o.name),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        names,
        [
            "value_type invoice_status",
            "value_type money",
            "type invoice",
            "type partner",
            "type transaction",
            "verb pay_invoice",
            "trigger invoice_overdue",
        ]
    );
}

/// A root in a fresh directory whose lakes are opened as the binary opens
/// them.
struct Shell {
    root: PathBuf,
}

impl Shell {
    fn open(
        &self,
        l: Location,
        mode: lakelands_api::engine::OpenMode,
        host: lakelands_api::seam::Seams,
    ) -> Opening {
        lakelands_cli::open_store(l, mode, host)
    }

    /// `lakelands <args>`, answering the exit status and what it printed.
    fn run(&self, args: &[&str]) -> (u8, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let open = |l: Location, m: lakelands_api::engine::OpenMode, s| self.open(l, m, s);
        let code = block_on(lakelands_cli::run(
            std::iter::once("lakelands").chain(args.iter().copied()),
            &mut out,
            &mut err,
            &open,
        ));
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn ok(&self, args: &[&str]) -> String {
        let (code, out, err) = self.run(args);
        assert_eq!(code, 0, "`lakelands {}` failed:\n{err}", args.join(" "));
        out
    }
}

#[test]
fn the_walks_commands_run_in_order_on_the_real_store_over_sqlite() {
    walk("sqlite");
}

/// A lake in memory lives as long as its process; this test's commands
/// all run in one, as a host that embeds the CLI's library runs them.
#[test]
fn the_walks_commands_run_in_order_on_the_real_store_in_memory() {
    walk("memory");
}

fn walk(engine: &str) {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("walk-1-store-{engine}"));
    let _ = fs::remove_dir_all(&root);
    let sh = Shell { root };
    let root = sh.root.display().to_string();
    let data = example().join("data");
    let path = |name: &str| data.join(name).display().to_string();

    sh.ok(&["init", &root]);
    sh.ok(&[
        "--root",
        &root,
        "lake",
        "new",
        "liquidity",
        "--engine",
        engine,
    ]);
    let applied = sh.ok(&["--root", &root, "apply", &example().display().to_string()]);
    let rule = |l: &&str| l.starts_with("  aging rule ");
    assert!(
        applied.starts_with("applied 7 declaration(s), in force from position ")
            && applied
                .lines()
                .filter(|l| l.starts_with("  ") && !rule(l))
                .count()
                == 7,
        "apply applies the bundle's seven declarations as one set:\n{applied}"
    );
    assert_eq!(
        applied.lines().filter(rule).collect::<Vec<_>>(),
        ["  aging rule `invoice_overdue` written at position 12"],
        "apply writes the bundle's aging rule after its declarations:\n{applied}"
    );

    let ingest = |file: &str, source: &str, map: &str| {
        sh.ok(&[
            "--root",
            &root,
            "ingest",
            &path(file),
            "--source",
            source,
            "--map",
            &path(map),
        ])
    };
    let counted = |out: &str| -> (u32, u32) {
        out.lines().fold((0, 0), |(r, d), line| {
            let n: Vec<u32> = line.split(' ').filter_map(|w| w.parse().ok()).collect();
            (r + n[0], d + n[1])
        })
    };
    let partners = ingest("partners.csv", "crm", "partners.map.json");
    assert_eq!(counted(&partners), (4, 0), "{partners}");
    let august = ingest("invoices-august.csv", "erp", "invoices.map.json");
    assert_eq!(counted(&august), (10, 0), "{august}");
    let september = ingest("invoices-september.csv", "erp", "invoices.map.json");
    assert_eq!(
        counted(&september),
        (3, 9),
        "the September export has twelve rows, three of them changed: {september}"
    );
}

/// A fresh directory for a test's root.
fn fresh(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    dir
}

/// Every file below `dir`, by its `/`-separated path in it, with its
/// contents.
fn files_below(dir: &Path, prefix: &str, out: &mut Vec<(String, String)>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            files_below(&path, &format!("{prefix}{name}/"), out);
        } else {
            out.push((
                format!("{prefix}{name}"),
                fs::read_to_string(&path).unwrap(),
            ));
        }
    }
}

/// The binary carries each example whole: its table lists exactly the
/// files of its directory under `examples/`, with their contents, so a
/// file added there and not to the table is caught here.
#[test]
fn every_example_carried_is_the_files_of_its_directory() {
    let examples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples");
    for example in EXAMPLES {
        let mut on_disk = Vec::new();
        files_below(&examples.join(example.dir), "", &mut on_disk);
        on_disk.sort();
        let mut carried: Vec<(String, String)> = (example.files.iter())
            .map(|(p, t)| ((*p).to_owned(), (*t).to_owned()))
            .collect();
        carried.sort();
        let names = |f: &[(String, String)]| f.iter().map(|f| f.0.clone()).collect::<Vec<_>>();
        assert_eq!(
            names(&carried),
            names(&on_disk),
            "example `{}`",
            example.name
        );
        assert!(
            carried == on_disk,
            "example `{}`: a file's contents differ",
            example.name
        );
    }
}

/// The walk's question at one position: each debtor's overdue gross
/// against its credit limit, as `invoice_find` with an aggregate and
/// `partner_get` answer them, and whether it is over.
async fn over(
    location: Location,
    as_of: Option<Position>,
) -> BTreeMap<String, (String, String, bool)> {
    let store = lakelands_cli::open_store(location, OpenMode::Read, Default::default())
        .await
        .unwrap();
    let cat = store.catalog(WriteContext::default());
    let who = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: "liquidity".into(),
        grants: Grants::owner(),
    };
    let at = ReadAt {
        as_of,
        valid_at: None,
        clock: None,
    };
    let find = Find {
        filter: Some("status = 'open' and due_on < now() - 30d".into()),
        aggregate: Some(Aggregate {
            measures: vec![AggMeasure {
                name: "overdue_gross".into(),
                func: AggFunc::Sum,
                field: Some("gross".into()),
            }],
            group_by: vec!["partner".into()],
        }),
        at,
        ..Find::default()
    };
    let found = cat.find(&who, &"invoice".into(), find).await.unwrap().data;
    let Found::Groups { rows } = found else {
        panic!("an aggregate find answers groups, not {found:?}");
    };
    let mut out = BTreeMap::new();
    for g in rows {
        let partner = g.by["partner"].as_str().unwrap().to_owned();
        let owed = g.values["overdue_gross"].as_str().unwrap().to_owned();
        let get = Get {
            key: partner.as_str().into(),
            with_meta: false,
            at,
            params: Default::default(),
        };
        let p = cat.get(&who, &"partner".into(), get).await.unwrap().data;
        let limit = p.fields["credit_limit"].as_str().unwrap().to_owned();
        let cents = |s: &str| (s.replace('.', "")).parse::<i64>().unwrap();
        let is_over = cents(&owed) > cents(&limit);
        out.insert(partner, (owed, limit, is_over));
    }
    out
}

/// `init DIR --example liquidity` makes walk 1's lake in one process,
/// from the files the binary carries: the eight declarations and the
/// aging rule, the three ingests with the walk's counts, at the positions
/// one writer's lease gives them; the MCP configuration names this binary
/// and the lake; and the walk's question has the walk's answers, at the
/// head and at the August ingest's position the command printed.
#[test]
fn init_with_an_example_makes_the_walks_lake_and_its_mcp_configuration() {
    let dir = fresh("walk-1-example");
    let sh = Shell { root: dir.clone() };
    let out = sh.ok(&["init", &dir.display().to_string(), "--example", "liquidity"]);
    assert!(
        out.contains("applied 7 declaration(s), in force from position 11:"),
        "{out}"
    );
    assert!(
        out.contains("  aging rule `invoice_overdue` written at position 12"),
        "{out}"
    );
    let committed: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with("committed "))
        .collect();
    assert_eq!(
        committed,
        [
            "committed 4 record(s), 0 already in the lake, through position 16",
            "committed 10 record(s), 0 already in the lake, through position 33",
            "committed 3 record(s), 9 already in the lake, through position 37",
        ],
        "{out}"
    );

    let root = dir.canonicalize().unwrap();
    let exe = std::env::current_exe().unwrap().canonicalize().unwrap();
    let config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join(".mcp.json")).unwrap()).unwrap();
    let args = [
        "--root",
        &root.display().to_string(),
        "--lake",
        "liquidity",
        "serve",
        "--mcp",
    ];
    assert_eq!(
        config,
        serde_json::json!({"mcpServers": {"lakelands": {
            "command": exe.display().to_string(), "args": args}}})
    );
    assert!(
        out.contains(&format!(
            "  for any other MCP client: {} {}",
            exe.display(),
            args.join(" ")
        )),
        "{out}"
    );
    let (repository, version) = (env!("CARGO_PKG_REPOSITORY"), env!("CARGO_PKG_VERSION"));
    let walk = format!("{repository}/blob/v{version}/docs/walks/walk-1.md");
    assert!(out.contains(&walk), "{out}");
    assert!(
        out.contains(
            "as the lake stood at position 33 (pin it): P-100, P-200 and P-300 Cedar Logistics"
        ),
        "{out}"
    );

    let location = Location::sqlite("liquidity", root.join("lakes/liquidity"));
    let owed = |owed: &str, limit: &str, over: bool| (owed.to_owned(), limit.to_owned(), over);
    let head = block_on(over(location.clone(), None));
    assert_eq!(
        head,
        BTreeMap::from([
            ("P-100".to_owned(), owed("10710.00", "10000.00", true)),
            ("P-200".to_owned(), owed("5950.00", "5000.00", true)),
            ("P-300".to_owned(), owed("4760.00", "8000.00", false)),
            ("P-400".to_owned(), owed("10710.00", "20000.00", false)),
        ]),
        "at the head"
    );
    let then = block_on(over(location, Some(Position(33))));
    assert_eq!(
        then,
        BTreeMap::from([
            ("P-100".to_owned(), owed("10710.00", "10000.00", true)),
            ("P-200".to_owned(), owed("5950.00", "5000.00", true)),
            ("P-300".to_owned(), owed("8330.00", "8000.00", true)),
            ("P-400".to_owned(), owed("7140.00", "20000.00", false)),
        ]),
        "as of the August ingest"
    );
}

/// A `.mcp.json` that is there already is the user's: it is left byte
/// for byte. Without a `lakelands` server the command prints the entry to
/// add; with this one, it says so; with one that names another command or
/// root, it says that and prints the entry to put in its place.
#[test]
fn init_with_an_example_leaves_an_existing_mcp_configuration_as_it_was() {
    let this = fresh("walk-1-example-this-server");
    fs::create_dir_all(&this).unwrap();
    let exe = std::env::current_exe().unwrap().canonicalize().unwrap();
    let root = this.canonicalize().unwrap().display().to_string();
    let args = ["--root", &root, "--lake", "liquidity", "serve", "--mcp"];
    let server = serde_json::json!({ "command": exe.display().to_string(), "args": args });
    let this_held = serde_json::json!({ "mcpServers": { "lakelands": server } }).to_string();
    for (name, held, said) in [
        (
            "walk-1-example-other-server",
            "{\"mcpServers\": {\"other\": {\"command\": \"x\"}}}\n",
            "is there already and is left as it is; add this server under `mcpServers`:",
        ),
        (
            "walk-1-example-this-server",
            this_held.as_str(),
            "holds this server `lakelands` already and is left as it is",
        ),
        (
            "walk-1-example-another-server",
            "{\"mcpServers\": {\"lakelands\": {\"command\": \"y\"}}}\n",
            "holds a server `lakelands` that names another command or root",
        ),
        (
            "walk-1-example-no-json",
            "not json\n",
            "add this server under `mcpServers`:",
        ),
    ] {
        let dir = fresh(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(".mcp.json"), held).unwrap();
        let sh = Shell { root: dir.clone() };
        let out = sh.ok(&["init", &dir.display().to_string(), "--example", "liquidity"]);
        assert_eq!(
            fs::read_to_string(dir.join(".mcp.json")).unwrap(),
            held,
            "{name}"
        );
        assert!(out.contains(said), "{name}:\n{out}");
        if !said.starts_with("holds this") {
            assert!(
                out.contains("\"lakelands\": {"),
                "{name}: the entry to add:\n{out}"
            );
        }
    }
}

/// Without a name, `--example` lists what the binary carries and makes
/// nothing; a name it does not carry is refused before anything is made;
/// a directory that holds a root already is refused and left as it was.
#[test]
fn init_with_an_example_lists_and_refuses_before_it_makes_anything() {
    let dir = fresh("walk-1-example-refused");
    let sh = Shell { root: dir.clone() };
    let d = dir.display().to_string();
    let listed = sh.ok(&["init", &d, "--example"]);
    assert!(listed.contains("  liquidity: "), "{listed}");
    assert!(!dir.exists(), "listing made the directory");

    let (code, _, err) = sh.run(&["init", &d, "--example", "nope"]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("this binary carries no example `nope`"),
        "{err}"
    );
    assert!(!dir.exists(), "a refused example made the directory");

    sh.ok(&["init", &d]);
    let (code, _, err) = sh.run(&["init", &d, "--example", "liquidity"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("already holds a Lakelands root"), "{err}");
    assert!(
        dir.join("lakelands.json").exists(),
        "the root that was there is gone"
    );
    assert!(!dir.join(".mcp.json").exists() && !dir.join("lakes/liquidity.json").exists());
}

/// Anything at the lake's location file or store is refused before the
/// first file is written: no root is left behind, and an old store is
/// never ingested into again.
#[test]
fn init_with_an_example_refuses_what_is_there_before_it_writes() {
    let file = |p: &Path| fs::write(p, "{}\n").unwrap();
    let dir = |p: &Path| fs::create_dir_all(p).unwrap();
    for (name, there, make) in [
        (
            "walk-1-example-left-store",
            "lakes/liquidity",
            &dir as &dyn Fn(&Path),
        ),
        ("walk-1-example-left-file", "lakes/liquidity", &file),
        (
            "walk-1-example-left-location",
            "lakes/liquidity.json",
            &file,
        ),
    ] {
        let root = fresh(name);
        fs::create_dir_all(root.join("lakes")).unwrap();
        make(&root.join(there));
        let sh = Shell { root: root.clone() };
        let d = root.display().to_string();
        let (code, _, err) = sh.run(&["init", &d, "--example", "liquidity"]);
        assert_eq!(code, 1, "{name}: {err}");
        assert!(
            err.contains("is there already; nothing was changed"),
            "{name}: {err}"
        );
        assert!(
            !root.join("lakelands.json").exists(),
            "{name}: a root was left behind"
        );
        assert!(!root.join(".mcp.json").exists(), "{name}");
    }
}

/// A run that fails after it made the root removes what it made, and the
/// same command then runs. The fault is a file-size limit (448 KiB, with
/// the signal it raises ignored, so a write fails as a full disk does): the
/// partners ingest commits and the August ingest fails. A directory and a
/// `lakes/` that were there before stay: no directory, an empty one, and
/// one with an empty `lakes/`.
#[test]
fn init_with_an_example_removes_what_it_made_when_a_later_step_fails() {
    for (n, there) in [&[][..], &[""], &["lakes"]].into_iter().enumerate() {
        fault(n, there);
    }
}

fn fault(n: usize, there: &[&str]) {
    let dir = fresh(&format!("walk-1-example-fault-{n}"));
    for path in there {
        fs::create_dir_all(dir.join(path)).unwrap();
    }
    let d = dir.display().to_string();
    let run = |limit: &str| {
        std::process::Command::new("bash")
            .args([
                "-c",
                "trap '' XFSZ; ulimit -f \"$1\"; shift; exec \"$@\"",
                "-",
            ])
            .args([limit, env!("CARGO_BIN_EXE_lakelands"), "init", &d])
            .args(["--example", "liquidity"])
            .output()
            .unwrap()
    };
    let failed = run("448");
    let (out, err) = (
        String::from_utf8_lossy(&failed.stdout),
        String::from_utf8_lossy(&failed.stderr),
    );
    assert_eq!(failed.status.code(), Some(1), "{out}\n{err}");
    assert!(
        out.contains("through position 16") && !out.contains("through position 33"),
        "the fault is to land in the August ingest:\n{out}\n{err}"
    );
    assert!(err.contains("nothing this run made is left in"), "{err}");
    let left: Vec<String> = fs::read_dir(&dir)
        .map(|d| {
            d.map(|e| e.unwrap().file_name().display().to_string())
                .collect()
        })
        .unwrap_or_default();
    match there {
        [] => assert!(!dir.exists(), "the failed run left {}", dir.display()),
        [""] => assert!(
            dir.is_dir() && left.is_empty(),
            "the failed run left {left:?}"
        ),
        _ => {
            assert_eq!(left, ["lakes"], "the failed run left {left:?}");
            assert_eq!(fs::read_dir(dir.join("lakes")).unwrap().count(), 0);
        }
    }

    let again = run("unlimited");
    let out = String::from_utf8_lossy(&again.stdout);
    assert!(again.status.success(), "{out}");
    assert!(out.contains("through position 37"), "{out}");
}
