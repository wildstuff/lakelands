//! What a meter costs a write: one lake after another with no meter, with
//! one that does nothing, and with the meter `serve` binds where
//! `server.metrics` is set, writing a record of a plain type at a time.
//!
//! ```text
//! cargo build --release -p lakelands-cli --example meter_bench
//! target/release/examples/meter_bench --engine memory|sqlite [--writes <n>] \
//!     [--rounds <n>] [--callers <n>] --scratch <dir>
//! ```
//!
//! Each round opens a fresh lake for each binding in turn, warms it with a
//! tenth of the writes and times the rest, `--callers` of them at once;
//! it prints each binding's median microseconds a write, then what a
//! timed span and a count cost on each meter alone. It asserts nothing.
//! Every knob is an argument.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::principal::{Access, Grants, PrincipalKind};
use lakelands_api::seam::{Count, Level, Meter, Metered, Seams, Spread};
use lakelands_api::write::{Declaration, WriteOpts};
use lakelands_api::{LakeId, LakeStore, Location, Principal, WriteContext};
use lakelands_server::Metrics;
use serde_json::json;

/// A meter that is told everything and keeps nothing.
struct Nothing;

impl Meter for Nothing {
    fn count(&self, at: Metered<'_>, n: Count, by: u64) {
        std::hint::black_box((at, n, by));
    }
    fn level(&self, at: Metered<'_>, n: Level, value: u64) {
        std::hint::black_box((at, n, value));
    }
    fn observe(&self, at: Metered<'_>, n: Spread, value: u64) {
        std::hint::black_box((at, n, value));
    }
}

/// The bindings compared, in the order each round opens them.
const BINDINGS: [&str; 3] = ["none", "nothing", "serve"];

fn meter(binding: &str, lake: &LakeId) -> Option<Arc<dyn Meter>> {
    match binding {
        "nothing" => Some(Arc::new(Nothing)),
        "serve" => Some(Metrics::new().lake(lake)),
        _ => None,
    }
}

fn writer() -> Principal {
    Principal {
        kind: PrincipalKind::Agent,
        id: "bench".into(),
        lake: "bench".into(),
        grants: Grants {
            default: Access::Write,
            schema: true,
            restricted: true,
            ..Grants::default()
        },
    }
}

async fn lake(engine: &str, dir: &Path, meter: Option<Arc<dyn Meter>>) -> Arc<dyn LakeStore> {
    let (location, opener): (Location, Arc<dyn EngineOpener>) = match engine {
        "memory" => (
            Location::memory("bench"),
            Arc::new(lakelands_engine_mem::MemoryOpener::new()),
        ),
        _ => (
            Location::sqlite("bench", dir),
            Arc::new(lakelands_engine_sqlite::SqliteOpener),
        ),
    };
    let seams = Seams {
        engines: vec![opener],
        meter,
        ..Seams::default()
    };
    let store = lakelands_store::open(location, seams, OpenMode::Create)
        .await
        .expect("the lake opens");
    let plain = json!({"declare": "type", "name": "plain",
        "fields": [{"name": "n", "type": {"kind": "number"}}]});
    let plain: Declaration = serde_json::from_value(plain).expect("the type reads");
    let cat = store.catalog(WriteContext::default());
    cat.declare(&writer(), plain)
        .await
        .expect("the type declares");
    store
}

/// Microseconds a write, `writes` of them by `callers` at once.
async fn timed(store: &Arc<dyn LakeStore>, writes: usize, callers: usize, tag: &str) -> f64 {
    let began = Instant::now();
    let mut tasks = Vec::new();
    for c in 0..callers {
        let (store, tag) = (store.clone(), tag.to_owned());
        tasks.push(tokio::spawn(async move {
            let cat = store.catalog(WriteContext::default());
            for i in 0..writes / callers {
                let patch = serde_json::from_value(json!({ "n": i })).expect("the patch reads");
                let key = format!("{tag}-{c}-{i}").as_str().into();
                cat.upsert(
                    &writer(),
                    &"plain".into(),
                    &key,
                    patch,
                    WriteOpts::default(),
                )
                .await
                .expect("the write lands");
            }
        }));
    }
    for t in tasks {
        t.await.expect("the caller ends");
    }
    began.elapsed().as_secs_f64() * 1e6 / writes as f64
}

/// Nanoseconds one timed span and one count take on `meter`.
fn hooks(meter: Option<Arc<dyn Meter>>, lake: &LakeId) -> f64 {
    let meter = std::hint::black_box(meter);
    let n = 10_000_000u64;
    let began = Instant::now();
    for i in 0..n {
        let t = meter.as_ref().map(|_| Instant::now());
        if let (Some(m), Some(t)) = (&meter, t) {
            let at = Metered {
                lake,
                engine: "memory",
            };
            m.observe(at, Spread::Append, t.elapsed().as_micros() as u64);
            m.count(at, Count::Events, i);
        }
    }
    began.elapsed().as_nanos() as f64 / n as f64
}

fn arg<T: std::str::FromStr>(args: &[String], name: &str, default: Option<T>) -> T {
    let given = args
        .iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1));
    match given {
        Some(v) => v.parse().unwrap_or_else(|_| panic!("{name} does not read")),
        None => default.unwrap_or_else(|| panic!("{name} is needed")),
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let engine: String = arg(&args, "--engine", Some("memory".into()));
    let writes: usize = arg(&args, "--writes", Some(20_000));
    let rounds: usize = arg(&args, "--rounds", Some(5));
    let callers: usize = arg(&args, "--callers", Some(1));
    let scratch: PathBuf = arg(&args, "--scratch", None);
    let id = LakeId::from("bench");
    let mut each: Vec<Vec<f64>> = BINDINGS.iter().map(|_| Vec::new()).collect();
    for round in 0..rounds {
        for (b, binding) in BINDINGS.iter().enumerate() {
            let dir = scratch.join(format!("{engine}-{round}-{binding}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("the scratch directory is made");
            let store = lake(&engine, &dir, meter(binding, &id)).await;
            timed(&store, writes / 10, callers, "warm").await;
            each[b].push(timed(&store, writes, callers, "timed").await);
            drop(store);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
    for (binding, us) in BINDINGS.iter().zip(&mut each) {
        us.sort_by(f64::total_cmp);
        let median = us[us.len() / 2];
        println!("{engine} callers={callers} {binding}: {median:.2} µs a write {us:.2?}");
    }
    for binding in BINDINGS {
        let ns = hooks(meter(binding, &id), &id);
        println!("hooks {binding}: {ns:.2} ns a span and a count");
    }
}
