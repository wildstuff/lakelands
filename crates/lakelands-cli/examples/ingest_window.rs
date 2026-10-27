//! How a delivery's size, the lake's `commit_window_events` and sealing
//! move the ingest rate on the SQLite engine. Asserts nothing.
//!
//! ```text
//! cargo run --release -p lakelands-cli --example ingest_window -- <scratch dir> [rows [mode commit]]
//! ```
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use lakelands_api::engine::OpenMode;
use lakelands_api::principal::{Access, Grants, PrincipalKind};
use lakelands_api::record::SourceRecord;
use lakelands_api::seam::Seams;
use lakelands_api::write::Declaration;
use lakelands_api::{LakeStore, Location, Principal, WriteContext};
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

/// One closed window span of one device, about 200 bytes of JSON.
fn rec(n: usize) -> SourceRecord {
    let id = format!("dev-{:05}-{n:09}", n % 100);
    serde_json::from_value(json!({
        "type": "span",
        "key": id,
        "fields": {
            "id": id,
            "device": format!("dev-{:05}", n % 100),
            "app_class": "com.mitchellh.ghostty",
            "project": format!("project-{:03}", n % 40),
            "started": format!("2026-10-12T09:{:02}:{:02}Z", (n / 60) % 60, n % 60),
            "ended": format!("2026-10-12T09:{:02}:{:02}Z", (n / 60) % 60, (n + 30) % 60),
            "seconds": 30 + n % 600,
        }
    }))
    .unwrap()
}

async fn open(dir: &Path, commit: u32, sealed: bool) -> Arc<dyn LakeStore> {
    let seams = Seams {
        engines: vec![Arc::new(lakelands_engine_sqlite::SqliteOpener)],
        keys: sealed.then(|| {
            Arc::new(lakelands_backings::keyring::FileKeyring::new(
                dir.join("keys"),
            )) as _
        }),
        ..Seams::default()
    };
    let mut location = Location::sqlite("bench", dir.join("lake"));
    location.limits.commit_window_events = commit;
    let store = lakelands_store::open(location, seams, OpenMode::Create)
        .await
        .expect("opens");
    let field = |name: &str, kind: &str| {
        json!({"name": name, "type": {"kind": kind}, "authority": "source",
               "erasable": sealed && name != "id"})
    };
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
    store
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let scratch = Path::new(&a[0]).to_path_buf();
    let rows: usize = a.get(1).map_or(20_000, |n| n.parse().unwrap());
    // Optional: only this mode and commit window, for a profiler.
    let only: Option<(&str, u32)> = a.get(2).map(|m| (m.as_str(), a[3].parse().unwrap()));
    // plain: no key store; sealed: under the source's key; apart: also
    // under a key of the delivery's own, as a host that takes back one
    // delivery seals it.
    for mode in ["apart", "sealed", "plain"] {
        let sealed = mode != "plain";
        for delivery in [1usize, 32, 500, 1_000] {
            for commit in [32u32, 256, 1_024] {
                if delivery == 1 && commit != 32 {
                    continue;
                }
                if only.is_some_and(|(m, c)| m != mode || c != commit || delivery != 1_000) {
                    continue;
                }
                let total = if delivery == 1 { rows / 20 } else { rows };
                let dir = scratch.join(format!(
                    "{mode}-d{delivery}-c{commit}-{}",
                    std::process::id()
                ));
                std::fs::create_dir_all(&dir).unwrap();
                let store = open(&dir, commit, sealed).await;
                let cat = store.catalog(WriteContext::default());
                let (mut done, mut slowest) = (0, 0f64);
                let started = Instant::now();
                while done < total {
                    let recs: Vec<_> = (done..(done + delivery).min(total)).map(rec).collect();
                    done += recs.len();
                    let t = Instant::now();
                    let opts = lakelands_api::write::IngestOpts {
                        delivery: (mode == "apart").then(|| format!("batch-{done}").into()),
                        sealed_apart: mode == "apart",
                        ..Default::default()
                    };
                    lakelands_api::catalog::ingest_all_with(
                        &*cat,
                        &who(),
                        &"feed".into(),
                        opts,
                        recs,
                    )
                    .await
                    .expect("ingests");
                    slowest = slowest.max(t.elapsed().as_secs_f64() * 1e3);
                }
                let took = started.elapsed().as_secs_f64();
                println!(
                    "{mode:<6} delivery={delivery:>5} commit_window={commit:>5}: {total:>6} rows {:>8.0} rows/s, {:>7.2} ms/delivery, slowest {slowest:.1} ms",
                    total as f64 / took,
                    took * 1e3 / total.div_ceil(delivery) as f64,
                );
                drop(cat);
                drop(store);
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }
}
