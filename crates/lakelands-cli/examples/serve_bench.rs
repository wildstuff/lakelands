//! Writes a second and their p99 through `lakelands serve`, at 1, 4 and
//! 16 callers: what the server's runtime costs a caller, on the engine
//! the lake is kept on.
//!
//! ```text
//! cargo build --release -p lakelands-cli --bin lakelands --example serve_bench
//! target/release/examples/serve_bench --lakelands target/release/lakelands \
//!     [--memory | --pg-url <url> [--pg-plain]] [--secs <n>] [--runs <n>] \
//!     [--callers 1,4,16] [--workers <n>] --scratch <dir>
//! ```
//!
//! One `serve --mcp` process serves a fresh lake from a scratch root; the
//! callers are `lakelands-client` handles in this process, each writing a
//! record of a plain type after another over REST for `--secs`, after a
//! second's warm-up. `--workers` sets the server configuration's
//! `workers`; without it the configuration names none. It prints one line
//! per run and asserts nothing. Every knob is an argument.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use lakelands_api::location::{EngineConfig, Limits, PgTls, PgTlsMode, PgTuning};
use lakelands_api::principal::{Access, Grants, PrincipalKind};
use lakelands_api::write::{Declaration, WriteOpts};
use lakelands_api::{LakeId, LakeStore, Location, Principal, WriteContext};
use lakelands_client::{ApiKey, RemoteConfig, RemoteLake};
use lakelands_server::resolver::Entry;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

const LAKE: &str = "bench";

fn cli(bin: &Path, args: &[&str]) {
    let out = std::process::Command::new(bin)
        .args(args)
        .output()
        .expect("lakelands runs");
    assert!(
        out.status.success(),
        "lakelands {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn writer() -> Principal {
    Principal {
        kind: PrincipalKind::Agent,
        id: "writer".into(),
        lake: LAKE.into(),
        grants: Grants::default(),
    }
}

/// A root with one lake and a server bound to port 0.
fn prepare(bin: &Path, root: &Path, pg: Option<(&str, bool)>, workers: Option<usize>) {
    let r = root.display().to_string();
    cli(bin, &["init", &r]);
    match pg {
        None => cli(bin, &["--root", &r, "lake", "new", LAKE]),
        Some((url, plain)) => {
            let schema = format!(
                "lakelands_bench_{}_{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis()
            );
            let location = Location {
                lake: LAKE.into(),
                engine: EngineConfig::Postgres {
                    url: url.into(),
                    schema: Some(schema),
                    tls: PgTls {
                        mode: if plain {
                            PgTlsMode::Disable
                        } else {
                            PgTlsMode::Require
                        },
                        ..PgTls::default()
                    },
                    tuning: PgTuning::default(),
                    tablespaces: None,
                },
                key: None,
                limits: Limits::default(),
                writers: Vec::new(),
            };
            std::fs::create_dir_all(root.join("lakes")).unwrap();
            std::fs::write(
                root.join("lakes").join(format!("{LAKE}.json")),
                serde_json::to_vec_pretty(&location).unwrap(),
            )
            .unwrap();
        }
    }
    let file = root.join("lakelands.json");
    let mut config: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    config["principals"] = serde_json::to_value(vec![Entry {
        name: "writer".into(),
        kind: PrincipalKind::Agent,
        id: "writer".into(),
        lakes: Vec::new(),
        grants: Grants {
            default: Access::Write,
            schema: true,
            restricted: true,
            ..Grants::default()
        },
        api_keys: vec!["k-writer".into()],
    }])
    .unwrap();
    config["server"] = json!({"bind": "127.0.0.1:0"});
    if let Some(w) = workers {
        config["server"]["workers"] = json!(w);
    }
    std::fs::write(&file, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
}

/// One run at `callers`: writes a second and the p50 and p99.
async fn run(
    remote: &Arc<RemoteLake>,
    callers: usize,
    secs: f64,
    tag: &str,
) -> (f64, Duration, Duration) {
    let warm = Duration::from_secs(1);
    let stop = Arc::new(AtomicBool::new(false));
    let began = Instant::now();
    let mut running = Vec::new();
    for c in 0..callers {
        let (remote, stop, tag) = (remote.clone(), stop.clone(), tag.to_owned());
        running.push(tokio::spawn(async move {
            let cat = remote.catalog(WriteContext::default());
            let mut took = Vec::new();
            let mut i = 0u64;
            while !stop.load(Ordering::Relaxed) {
                let at = Instant::now();
                let p = serde_json::from_value(json!({"n": i})).unwrap();
                let key = format!("{tag}-{c}-{i}");
                cat.upsert(
                    &writer(),
                    &"plain".into(),
                    &key.into(),
                    p,
                    WriteOpts::default(),
                )
                .await
                .expect("it applies");
                if at >= began + warm {
                    took.push(at.elapsed());
                }
                i += 1;
            }
            took
        }));
    }
    tokio::time::sleep(warm + Duration::from_secs_f64(secs)).await;
    stop.store(true, Ordering::Relaxed);
    let mut took = Vec::new();
    for r in running {
        took.extend(r.await.unwrap());
    }
    took.sort();
    let q = |f: f64| took[((took.len() as f64 * f) as usize).min(took.len() - 1)];
    (took.len() as f64 / secs, q(0.5), q(0.99))
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    let (mut bin, mut scratch, mut pg_url, mut plain, mut memory) =
        (None, None, None, false, false);
    let (mut secs, mut runs, mut workers) = (5.0, 3usize, None);
    let mut callers = vec![1usize, 4, 16];
    while let Some(a) = args.next() {
        let mut v = || args.next().expect("a value");
        match a.as_str() {
            "--lakelands" => bin = Some(PathBuf::from(v())),
            "--scratch" => scratch = Some(PathBuf::from(v())),
            "--pg-url" => pg_url = Some(v()),
            "--pg-plain" => plain = true,
            "--memory" => memory = true,
            "--secs" => secs = v().parse().unwrap(),
            "--runs" => runs = v().parse().unwrap(),
            "--workers" => workers = Some(v().parse().unwrap()),
            "--callers" => callers = v().split(',').map(|c| c.parse().unwrap()).collect(),
            other => panic!("unknown argument {other}"),
        }
    }
    let bin = bin.expect("--lakelands");
    let root = scratch
        .expect("--scratch")
        .join(format!("root-{}", std::process::id()));
    prepare(&bin, &root, pg_url.as_deref().map(|u| (u, plain)), workers);
    if memory {
        let r = root.display().to_string();
        std::fs::remove_file(root.join("lakes").join(format!("{LAKE}.json"))).unwrap();
        cli(
            &bin,
            &["--root", &r, "lake", "new", LAKE, "--engine", "memory"],
        );
    }
    let mut child = Command::new(&bin)
        .arg("--root")
        .arg(&root)
        .args(["--as", "writer", "serve", "--mcp"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("serve starts");
    let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
    let addr = loop {
        let l = lines
            .next_line()
            .await
            .unwrap()
            .expect("serve names its address");
        if let Some(a) = l.strip_prefix("lakelands: serving HTTP at http://") {
            break a.trim_end_matches('/').to_owned();
        }
    };
    let remote = Arc::new(
        RemoteLake::connect(RemoteConfig {
            url: format!("http://{addr}/"),
            lake: LakeId::from(LAKE),
            keys: BTreeMap::from([("writer".to_owned(), ApiKey::from("k-writer"))]),
            feed_key: None,
            ca_file: None,
            timeout_secs: 120,
            max_answer_bytes: lakelands_client::DEFAULT_MAX_ANSWER_BYTES,
            limits: Limits::default(),
        })
        .unwrap(),
    );
    let plain_type: Declaration =
        serde_json::from_value(json!({"declare": "type", "name": "plain",
        "fields": [{"name": "n", "type": {"kind": "number"}}]}))
        .unwrap();
    remote
        .catalog(WriteContext::default())
        .declare(&writer(), plain_type)
        .await
        .expect("declared");
    for &c in &callers {
        for r in 0..runs {
            let (rate, p50, p99) = run(&remote, c, secs, &format!("r{c}-{r}")).await;
            println!(
                "{c:>3} callers run {r}: {rate:>7.0} writes/s, p50 {:.2} ms, p99 {:.2} ms",
                p50.as_secs_f64() * 1e3,
                p99.as_secs_f64() * 1e3
            );
        }
    }
    drop(child);
    let _ = std::fs::remove_dir_all(&root);
}
