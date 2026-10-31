//! One small act against a large lake: what a removal, a forget and a
//! purge of 1 000 records take as the lake grows, on the SQLite engine
//! with a key store, each delivery sealed apart.
//!
//! ```text
//! cargo build --release -p lakelands-cli --example act_bench
//! target/release/examples/act_bench --scratch <dir> [--no-remove] \
//!     --steps 100000,500000,1000000,2500000
//! target/release/examples/act_bench --scratch <dir> --set <n>,<deliveries>
//! ```
//!
//! `--steps` grows one lake through each size, and at each runs the acts
//! on a copy of it: a removal of one delivery of 1 000 records from the
//! middle of the lake, a forget after it, then on fresh copies a forget
//! and a purge of a source of 1 000 records. `--set` takes many
//! deliveries back in one act. It prints one line per step and asserts
//! nothing. Every knob is an argument. Record 099's split of an act into
//! its layers came from timers in a scratch build, not from here.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

use lakelands_api::engine::OpenMode;
use lakelands_api::principal::{Access, Grants, PrincipalKind};
use lakelands_api::record::SourceRecord;
use lakelands_api::seam::Seams;
use lakelands_api::write::{Declaration, IngestOpts, Selection};
use lakelands_api::{DeliveryId, LakeStore, Location, Principal, SourceId, WriteContext};
use serde_json::json;

fn who() -> Principal {
    Principal {
        kind: PrincipalKind::Agent,
        id: "bench".into(),
        lake: "bench".into(),
        grants: Grants {
            default: Access::Write,
            schema: true,
            ingest: true,
            restricted: true,
            ..Grants::default()
        },
    }
}

fn rec(n: usize) -> SourceRecord {
    let id = format!("dev-{:05}-{n:09}", n % 100);
    serde_json::from_value(json!({
        "type": "span", "key": id,
        "fields": {
            "id": id, "device": format!("dev-{:05}", n % 100),
            "app_class": "com.example.terminal",
            "project": format!("project-{:03}", n % 40),
            "started": format!("2026-10-12T09:{:02}:{:02}Z", (n / 60) % 60, n % 60),
            "ended": format!("2026-10-12T09:{:02}:{:02}Z", (n / 60) % 60, (n + 30) % 60),
            "seconds": 30 + n % 600,
        }
    }))
    .unwrap()
}

async fn open(dir: &Path, mode: OpenMode) -> Arc<dyn LakeStore> {
    let keys = lakelands_backings::keyring::FileKeyring::new(dir.join("keys"));
    let seams = Seams {
        engines: vec![Arc::new(lakelands_engine_sqlite::SqliteOpener)],
        keys: Some(Arc::new(keys) as _),
        ..Seams::default()
    };
    let location = Location::sqlite("bench", dir.join("lake"));
    lakelands_store::open(location, seams, mode)
        .await
        .expect("opens")
}

async fn declare(store: &Arc<dyn LakeStore>) {
    let field = |name: &str, kind: &str| json!({"name": name, "type": {"kind": kind}, "authority": "source", "erasable": name != "id"});
    let decl: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "span", "key": ["id"],
        "fields": [field("id", "text"), field("device", "text"), field("app_class", "text"),
                   field("project", "text"), field("started", "text"), field("ended", "text"),
                   field("seconds", "number")]
    }))
    .unwrap();
    store
        .catalog(WriteContext::default())
        .declare(&who(), decl)
        .await
        .expect("declares");
}

fn clone(from: &Path, to: &Path) {
    let _ = std::fs::remove_dir_all(to);
    let ok = Command::new("cp")
        .arg("-R")
        .arg(from)
        .arg(to)
        .status()
        .unwrap();
    assert!(ok.success());
}

fn du(p: &Path) -> String {
    let o = Command::new("du").args(["-sh"]).arg(p).output().unwrap();
    String::from_utf8_lossy(&o.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("?")
        .to_string()
}

fn ms(t: Instant) -> u128 {
    t.elapsed().as_millis()
}

async fn remove(base: &Path, work: &Path, n: usize, delivery: &str, also_forget: bool) {
    clone(base, work);
    let store = open(work, OpenMode::Write).await;
    let (load, sel) = (
        SourceId::from("load"),
        Selection::Delivery(DeliveryId::from(delivery)),
    );
    let t = Instant::now();
    let report = store.remove_values_preflight(&load, &sel).await.unwrap();
    println!(
        "R n={n} remove.preflight {} ms events={}",
        ms(t),
        report.events
    );
    let t = Instant::now();
    let r = store
        .remove_values(&load, &sel, false, Some(&report.report_id))
        .await
        .unwrap();
    println!(
        "R n={n} remove.act {} ms events={} reclaimed={:?} owed={:?}",
        ms(t),
        r.events,
        r.reclaimed,
        r.reclaim_owed
    );
    if also_forget {
        let t = Instant::now();
        let f = store
            .forget_source(&SourceId::from("small"), None)
            .await
            .unwrap();
        println!("R n={n} forget.after.removal {} ms way={:?}", ms(t), f.way);
    }
    store.close().await.unwrap();
    drop(store);
    let _ = std::fs::remove_dir_all(work);
}

async fn forget(base: &Path, work: &Path, n: usize) {
    clone(base, work);
    let store = open(work, OpenMode::Write).await;
    let small = SourceId::from("small");
    let t = Instant::now();
    let report = store.forget_preflight(&small).await.unwrap();
    println!("R n={n} forget.preflight {} ms way={:?}", ms(t), report.way);
    let t = Instant::now();
    let r = store
        .forget_source(&small, Some(&report.report_id))
        .await
        .unwrap();
    println!(
        "R n={n} forget.act {} ms way={:?} reclaimed={:?}",
        ms(t),
        r.way,
        r.reclaimed
    );
    store.close().await.unwrap();
    drop(store);
    let _ = std::fs::remove_dir_all(work);
}

async fn purge(base: &Path, work: &Path, n: usize) {
    clone(base, work);
    let store = open(work, OpenMode::Write).await;
    let small = SourceId::from("small");
    let t = Instant::now();
    let report = store.purge_preflight(&small, None).await.unwrap();
    println!("R n={n} purge.preflight {} ms", ms(t));
    let t = Instant::now();
    let _ = store
        .purge_source(&small, None, Some(&report.report_id))
        .await
        .unwrap();
    println!("R n={n} purge.act {} ms", ms(t));
    store.close().await.unwrap();
    drop(store);
    let _ = std::fs::remove_dir_all(work);
}

fn arg(args: &[String], name: &str) -> Option<String> {
    let at = args.iter().position(|a| a == name)?;
    args.get(at + 1).cloned()
}

fn numbers(list: &str) -> Vec<usize> {
    list.split(',')
        .map(|n| n.parse().expect("a number"))
        .collect()
}

async fn load(store: &Arc<dyn LakeStore>, from: usize, to: usize) {
    let cat = store.catalog(WriteContext::default());
    for at in (from..to).step_by(1000) {
        let recs: Vec<_> = (at..at + 1000).map(rec).collect();
        let delivery = format!("load-{:06}", at / 1000);
        let opts = IngestOpts {
            delivery: Some(delivery.as_str().into()),
            sealed_apart: true,
            ..Default::default()
        };
        lakelands_api::catalog::ingest_all_with(&*cat, &who(), &"load".into(), opts, recs)
            .await
            .unwrap();
    }
}

async fn set(dir: &Path, n: usize, many: usize) {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    let store = open(dir, OpenMode::Create).await;
    declare(&store).await;
    load(&store, 0, n).await;
    let ds = (0..many).map(|d| DeliveryId::from(format!("load-{d:06}").as_str()));
    let (sel, source) = (Selection::Deliveries(ds.collect()), SourceId::from("load"));
    let t = Instant::now();
    let report = store.remove_values_preflight(&source, &sel).await.unwrap();
    println!(
        "S n={n} many={many} preflight {} ms events={}",
        ms(t),
        report.events
    );
    let t = Instant::now();
    let r = store
        .remove_values(&source, &sel, false, Some(&report.report_id))
        .await
        .unwrap();
    println!("S n={n} many={many} act {} ms events={}", ms(t), r.events);
    store.close().await.unwrap();
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let scratch = PathBuf::from(arg(&args, "--scratch").expect("--scratch <dir>"));
    if let Some(list) = arg(&args, "--set") {
        let of = numbers(&list);
        return set(&scratch, of[0], of[1]).await;
    }
    let steps = numbers(&arg(&args, "--steps").expect("--steps <n,n,...>"));
    let also_remove = !args.iter().any(|a| a == "--no-remove");
    let (base, work) = (scratch.join("base"), scratch.join("work"));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&base).unwrap();
    let mut store = open(&base, OpenMode::Create).await;
    declare(&store).await;
    // The small source: 1 000 rows of its own, in the same type.
    {
        let cat = store.catalog(WriteContext::default());
        let recs: Vec<_> = (0..1000).map(|i| rec(100_000_000 + i)).collect();
        let opts = IngestOpts::default();
        lakelands_api::catalog::ingest_all_with(&*cat, &who(), &"small".into(), opts, recs)
            .await
            .unwrap();
    }
    let mut done = 0;
    for n in steps {
        let t = Instant::now();
        load(&store, done, n).await;
        done = n;
        store.close().await.unwrap();
        drop(store);
        println!("R n={n} built {} ms size={}", ms(t), du(&base));
        if also_remove {
            let middle = format!("load-{:06}", n / 2000);
            remove(&base, &work, n, &middle, true).await;
        }
        forget(&base, &work, n).await;
        purge(&base, &work, n).await;
        store = open(&base, OpenMode::Write).await;
    }
}
