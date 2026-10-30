//! lakelands#1575 measurement: `describe_type` at the head after every
//! record write, on SQLite. One type; `<rate>` record writes a second for
//! `<secs>` seconds spread over `<lanes>` tasks, each lane writing a record
//! and then describing the type at the head. Prints describes a second,
//! p50 / p99 of the describe, and the shares of its time (counters from
//! `lakelands_core::probe`, a throwaway patch).
//!
//! ```text
//! cargo run --release -p lakelands-engine-sqlite --example describe_bench -- <scratch dir> 1000 10 1
//! ```
use std::sync::Arc;
use std::time::{Duration, Instant};

use lakelands_api::engine::OpenMode;
use lakelands_api::principal::{Access, Grants, PrincipalKind};
use lakelands_api::seam::Seams;
use lakelands_api::write::{Declaration, WriteOpts};
use lakelands_api::{Location, Principal, ReadAt, WriteContext};
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

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(&a[0]).join(format!("d{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (rate, secs, lanes): (u64, u64, u64) =
        (a[1].parse().unwrap(), a[2].parse().unwrap(), a[3].parse().unwrap());
    let seams = Seams {
        engines: vec![Arc::new(lakelands_engine_sqlite::SqliteOpener)],
        ..Seams::default()
    };
    let store = lakelands_store::open(Location::sqlite("bench", &dir), seams, OpenMode::Create)
        .await
        .expect("opens");
    let cat = store.catalog(WriteContext::default());
    let decl: Declaration = serde_json::from_value(json!({"declare": "type", "name": "item", "fields": [
        {"name": "title", "type": {"kind": "text"}},
        {"name": "n", "type": {"kind": "number"}}]}))
    .unwrap();
    cat.declare(&who(), decl).await.expect("declares");
    let ty: lakelands_api::TypeName = "item".into();
    // warm
    for i in 0..50 {
        let patch = serde_json::from_value(json!({"title": format!("w{i}"), "n": i})).unwrap();
        cat.upsert(&who(), &ty, &format!("w{i}").into(), patch, WriteOpts::default())
            .await
            .unwrap();
        cat.describe_type(&who(), &ty, ReadAt::default()).await.unwrap();
    }
    lakelands_core::probe::reset();
    let start = Instant::now();
    let end = start + Duration::from_secs(secs);
    let every = Duration::from_nanos(1_000_000_000 * lanes / rate);
    let mut tasks = Vec::new();
    for lane in 0..lanes {
        let (cat, ty) = (cat.clone(), ty.clone());
        tasks.push(tokio::spawn(async move {
            let mut lat = Vec::new();
            let mut next = start + every * lane as u32 / lanes as u32;
            let mut i = 0u64;
            while Instant::now() < end {
                tokio::time::sleep_until(next.into()).await;
                next += every;
                let patch = serde_json::from_value(json!({"title": format!("t{i}"), "n": i})).unwrap();
                cat.upsert(&who(), &ty, &format!("l{lane}-{i}").into(), patch, WriteOpts::default())
                    .await
                    .unwrap();
                let s = Instant::now();
                cat.describe_type(&who(), &ty, ReadAt::default()).await.unwrap();
                lat.push(s.elapsed());
                i += 1;
            }
            lat
        }));
    }
    let mut lat = Vec::new();
    for t in tasks {
        lat.extend(t.await.unwrap());
    }
    let wall = start.elapsed().as_secs_f64();
    lat.sort();
    let n = lat.len();
    let p = |q: f64| lat[((n as f64 * q) as usize).min(n - 1)].as_secs_f64() * 1e3;
    println!("lanes {lanes} target {rate}/s for {secs}s");
    println!("describes {n} = {:.0}/s (writes the same)", n as f64 / wall);
    println!("describe p50 {:.3} ms  p99 {:.3} ms  max {:.3} ms", p(0.5), p(0.99), p(1.0));
    let c: std::collections::HashMap<_, _> = lakelands_core::probe::snapshot().into_iter().collect();
    let per = |k: &str| c[k] as f64 / n as f64 / 1e3;
    let total: f64 = lat.iter().map(|d| d.as_secs_f64() * 1e6).sum::<f64>() / n as f64;
    println!("per describe, mean us: client latency {total:.0}");
    println!("  reading() setup (view, shape, sight, open): {:.0}", per("READING_NS"));
    println!("  Reader::describe_type closure:               {:.0}", per("DESCRIBE_NS"));
    println!("    name_sorted:                                {:.0}", per("NAME_SORTED_NS"));
    println!("      view.chosen (engine read):                {:.0}", per("CHOSEN_NS"));
    println!("      tally lock+clone:                         {:.0}", per("TALLY_NS"));
    let hand = total - per("READING_NS") - per("DESCRIBE_NS");
    println!("  hand-off (spawn_blocking, scheduling, rest):  {hand:.0}");
    println!(
        "reader pool: takes {} waited {} total take time {:.1} ms (mean {:.1} us per take)",
        c["POOL_TAKES"], c["POOL_WAITS"], c["POOL_WAIT_NS"] as f64 / 1e6,
        c["POOL_WAIT_NS"] as f64 / c["POOL_TAKES"].max(1) as f64 / 1e3
    );
}
