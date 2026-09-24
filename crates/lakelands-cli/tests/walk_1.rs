//! Walk 1's materials, kept from rotting: the liquidity bundle in
//! `examples/liquidity/` loads, and the commands `docs/walks/walk-1.md`
//! types — `init`, `lake new`, `apply`, and the three `ingest --map` runs
//! — succeed on the in-memory lake, in the script's order.
//!
//! The answers the walk expects are checked on the real store, in
//! `lakelands-store`'s `walk_1` test; the mock computes no derived fields
//! and does not aggregate.

use std::collections::BTreeMap;
use std::fs;
use std::future::Future;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use lakelands_api::location::Location;
use lakelands_api::write::Declaration;
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

/// A root in a fresh directory whose lakes are in-memory mocks.
struct Shell {
    root: PathBuf,
    lakes: Mutex<BTreeMap<String, MockLake>>,
}

impl Shell {
    fn open(&self, l: &Location) -> Result<Arc<dyn LakeStore>> {
        let mut lakes = self.lakes.lock().unwrap();
        let lake = lakes
            .entry(l.lake.to_string())
            .or_insert_with(|| MockLake::open(l));
        Ok(Arc::new(lake.clone()))
    }

    /// `lakelands <args>`, answering the exit status and what it printed.
    fn run(&self, args: &[&str]) -> (u8, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let open = |l: &Location| self.open(l);
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
fn the_walks_commands_run_in_order_on_the_in_memory_lake() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("walk-1");
    let _ = fs::remove_dir_all(&root);
    let sh = Shell {
        root,
        lakes: Mutex::new(BTreeMap::new()),
    };
    let root = sh.root.display().to_string();
    let data = example().join("data");
    let path = |name: &str| data.join(name).display().to_string();

    sh.ok(&["init", &root]);
    sh.ok(&["--root", &root, "lake", "new", "liquidity"]);
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
    let (records, duplicates) = counted(&september);
    assert_eq!(
        records + duplicates,
        12,
        "the September export has twelve rows: {september}"
    );
}
