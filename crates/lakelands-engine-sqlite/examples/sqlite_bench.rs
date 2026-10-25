//! One lake of `<records>` records on the SQLite engine, `<runs>` times
//! over: batch append rate, a full-text search, a `where` over a JSON
//! field, a sorted page, a count, the exact vector scan with and without
//! a filter, and a rebuild; each run prints its medians, the last lines
//! the median over the runs (lakelands#1378).
//!
//! The runs behind lakelands#1378 built the example once per SQLite, and
//! ran the two binaries interleaved, five times each, one lake per run:
//!
//! ```text
//! cargo build --release -p lakelands-engine-sqlite --example sqlite_bench
//! cp $CARGO_TARGET_DIR/release/examples/sqlite_bench bench-<before|after>
//! for i in 1 2 3 4 5; do for v in before after; do
//!   bench-$v <scratch dir> 100000 1 > out-$v-$i.txt
//! done; done
//! ```
//!
//! The first build was on `origin/develop` (rusqlite 0.40, SQLite 3.53.2),
//! the second on this change (rusqlite 0.41, SQLite 3.54.0). It asserts
//! nothing.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use lakelands_api::engine::OpenMode;
use lakelands_api::principal::{Access, Grants, PrincipalKind};
use lakelands_api::record::SourceRecord;
use lakelands_api::seam::Seams;
use lakelands_api::write::Declaration;
use lakelands_api::{LakeStore, Location, Principal, WriteContext};
use serde_json::{Value, json};

const DIMS: usize = 128;

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

fn vecof(i: usize) -> Vec<f32> {
    let mut x = (i as u64)
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    (0..DIMS)
        .map(|_| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((x >> 33) as f32 / (1u64 << 31) as f32) - 0.5
        })
        .collect()
}

fn rec(i: usize) -> SourceRecord {
    serde_json::from_value(json!({
        "type": "item", "key": format!("k{i:06}"),
        "fields": {
            "title": format!("word{} alpha{} common", i % 1000, i % 37),
            "body": format!("body text number {i} with topic{} and filler words", i % 211),
            "meta": {"group": format!("g{}", i % 100), "n": i},
            "score": (i * 7919) % 10007,
            "vec": vecof(i),
        }
    }))
    .unwrap()
}

fn median(v: &mut Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

async fn rep<F, Fut>(n: usize, mut f: F) -> f64
where
    F: FnMut(usize) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let mut t = Vec::new();
    for i in 0..n {
        let s = Instant::now();
        f(i).await;
        t.push(s.elapsed().as_secs_f64() * 1e3);
    }
    median(&mut t)
}

async fn open(dir: &Path, mode: OpenMode) -> Arc<dyn LakeStore> {
    let seams = Seams {
        engines: vec![Arc::new(lakelands_engine_sqlite::SqliteOpener)],
        ..Seams::default()
    };
    lakelands_store::open(Location::sqlite("bench", dir), seams, mode)
        .await
        .expect("opens")
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let scratch = PathBuf::from(&a[0]);
    let n: usize = a[1].parse().unwrap();
    let runs: usize = a[2].parse().unwrap();
    let mut out: std::collections::BTreeMap<&str, Vec<f64>> = Default::default();
    for run in 0..runs {
        let dir = scratch.join(format!("run{run}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = open(&dir, OpenMode::Create).await;
        let cat = store.catalog(WriteContext::default());
        let decl: Declaration = serde_json::from_value(json!({"declare": "type", "name": "item", "fields": [
            {"name": "title", "type": {"kind": "text"}, "searchable": true, "authority": "source"},
            {"name": "body", "type": {"kind": "text"}, "searchable": true, "authority": "source"},
            {"name": "meta", "type": {"kind": "json"}, "authority": "source"},
            {"name": "score", "type": {"kind": "number"}, "authority": "source"},
            {"name": "vec", "type": {"kind": "vector", "dimensions": DIMS}, "authority": "source"}]})).unwrap();
        cat.declare(&who(), decl).await.expect("declares");
        // batch append: windows of 1000 records
        let s = Instant::now();
        let mut done = 0;
        while done < n {
            let recs: Vec<_> = (done..(done + 1000).min(n)).map(rec).collect();
            done += recs.len();
            lakelands_api::catalog::ingest_all(&*cat, &who(), &"bench".into(), recs)
                .await
                .expect("ingests");
        }
        out.entry("append records/s")
            .or_default()
            .push(n as f64 / s.elapsed().as_secs_f64());
        let find = |v: Value| serde_json::from_value::<lakelands_api::read::Find>(v).unwrap();
        let search = |v: Value| serde_json::from_value::<lakelands_api::query::Search>(v).unwrap();
        let ty = "item".into();
        // sanity
        let f = cat
            .find(
                &who(),
                &ty,
                find(json!({"where": "meta.group = 'g7'", "limit": 10})),
            )
            .await
            .unwrap();
        let sr = cat
            .search(
                &who(),
                search(json!({"type": "item", "mode": "lexical", "text": "word5 topic3", "k": 10})),
            )
            .await
            .unwrap();
        let vs = cat.search(&who(), search(json!({"type": "item", "mode": "vector", "field": "vec", "vector": vecof(999_999), "k": 10}))).await.unwrap();
        let vf = cat.search(&who(), search(json!({"type": "item", "mode": "vector", "field": "vec", "vector": vecof(999_999), "k": 10, "where": "meta.group = 'g7'"}))).await.unwrap();
        let ct = cat.find(&who(), &ty, find(json!({"where": "score < 5000", "aggregate": {"measures": [{"name": "n", "func": "count"}]}}))).await.unwrap();
        if run == 0 {
            for (l, x) in [
                ("find", format!("{f:?}")),
                ("search", format!("{sr:?}")),
                ("vec", format!("{vs:?}")),
                ("vecf", format!("{vf:?}")),
                ("count", format!("{ct:?}")),
            ] {
                eprintln!("{l}: {}", &x[..x.len().min(300)]);
            }
        }
        let w = 50;
        let v = rep(w, |i| { let cat = cat.clone(); async move {
            cat.search(&who(), search(json!({"type": "item", "mode": "lexical", "text": format!("word{} topic{}", i % 1000, i % 211), "k": 10}))).await.unwrap(); } }).await;
        out.entry("full-text search ms").or_default().push(v);
        let v = rep(w, |i| {
            let cat = cat.clone();
            async move {
                cat.find(
                    &who(),
                    &"item".into(),
                    find(json!({"where": format!("meta.group = 'g{}'", i % 100), "limit": 10})),
                )
                .await
                .unwrap();
            }
        })
        .await;
        out.entry("where json field ms").or_default().push(v);
        let v = rep(w, |i| {
            let cat = cat.clone();
            async move {
                let _ = i;
                cat.find(
                    &who(),
                    &"item".into(),
                    find(json!({"order_by": [{"field": "score", "dir": "desc"}], "limit": 10})),
                )
                .await
                .unwrap();
            }
        })
        .await;
        out.entry("sorted page ms").or_default().push(v);
        let v = rep(w, |i| { let cat = cat.clone(); async move {
            cat.find(&who(), &"item".into(), find(json!({"where": format!("score < {}", 1000 + i * 100), "aggregate": {"measures": [{"name": "n", "func": "count"}]}}))).await.unwrap(); } }).await;
        out.entry("count ms").or_default().push(v);
        let v = rep(10, |i| { let cat = cat.clone(); async move {
            cat.search(&who(), search(json!({"type": "item", "mode": "vector", "field": "vec", "vector": vecof(900_000 + i), "k": 10}))).await.unwrap(); } }).await;
        out.entry("nearest, no filter ms").or_default().push(v);
        let v = rep(10, |i| { let cat = cat.clone(); async move {
            cat.search(&who(), search(json!({"type": "item", "mode": "vector", "field": "vec", "vector": vecof(900_000 + i), "k": 10, "where": format!("meta.group = 'g{}'", i % 100)}))).await.unwrap(); } }).await;
        out.entry("nearest, filtered ms").or_default().push(v);
        let s = Instant::now();
        store.rebuild().await.expect("rebuilds");
        out.entry("rebuild s per 100k records")
            .or_default()
            .push(s.elapsed().as_secs_f64() * 100_000.0 / n as f64);
        store.close().await.ok();
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
        eprintln!("run {run} done");
    }
    for (k, v) in &mut out {
        let all = format!("{:.2?}", v);
        println!("{k}: median {:.2} of {} runs {all}", median(v), runs);
    }
}
