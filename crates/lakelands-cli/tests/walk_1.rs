//! Walk 1's materials, kept from rotting: the liquidity bundle in
//! `examples/liquidity/` loads, and the commands `docs/walks/walk-1.md`
//! types — `init`, `lake new`, `apply`, and the three `ingest --map` runs
//! — succeed in the script's order, on the mock and on the binary's own
//! `open_store`, the real store over SQLite, with the same counts.
//!
//! The answers the walk expects are checked on the real store, in
//! `lakelands-store`'s `walk_1` test; the mock computes no derived fields
//! and does not aggregate.

use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use lakelands_api::LakeStore;
use lakelands_api::location::Location;
use lakelands_api::write::Declaration;
use lakelands_cli::Opening;
use lakelands_mock::MockLake;

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
            "object_set overdue",
        ]
    );
}

/// A root in a fresh directory whose lakes are mocks, or opened as the
/// binary opens them.
struct Shell {
    root: PathBuf,
    mock: bool,
    lakes: Mutex<BTreeMap<String, MockLake>>,
}

impl Shell {
    fn open(
        &self,
        l: Location,
        mode: lakelands_api::engine::OpenMode,
        host: lakelands_api::seam::Seams,
    ) -> Opening {
        if !self.mock {
            return lakelands_cli::open_store(l, mode, host);
        }
        let mut lakes = self.lakes.lock().unwrap();
        let lake = lakes
            .entry(l.lake.to_string())
            .or_insert_with(|| MockLake::open(&l))
            .clone();
        Box::pin(async move { Ok(Arc::new(lake) as Arc<dyn LakeStore>) })
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
fn the_walks_commands_run_in_order_on_the_mock() {
    walk(true, "sqlite");
}

#[test]
fn the_walks_commands_run_in_order_on_the_real_store_over_sqlite() {
    walk(false, "sqlite");
}

/// A lake in memory lives as long as its process; this test's commands
/// all run in one, as a host that embeds the CLI's library runs them.
#[test]
fn the_walks_commands_run_in_order_on_the_real_store_in_memory() {
    walk(false, "memory");
}

fn walk(mock: bool, engine: &str) {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "walk-1-{}-{engine}",
        if mock { "mock" } else { "store" }
    ));
    let _ = fs::remove_dir_all(&root);
    let sh = Shell {
        root,
        mock,
        lakes: Mutex::new(BTreeMap::new()),
    };
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
    assert!(
        applied.starts_with("applied 8 declaration(s), in force from position ")
            && applied.lines().filter(|l| l.starts_with("  ")).count() == 8,
        "apply applies the bundle's eight declarations as one set:\n{applied}"
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
