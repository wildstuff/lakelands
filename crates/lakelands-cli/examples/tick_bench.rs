//! What one market tick costs a lake: writes a second, the p99 of a
//! write, the bytes a record keeps, and what a kept roll-up and a
//! trigger's guard add to a write. Asserts nothing.
//!
//! ```text
//! cargo build --release -p lakelands-cli --bin lakelands --example tick_bench
//! target/release/examples/tick_bench --lakelands target/release/lakelands \
//!     [--pg-url <url> --pg-plain] [--records <n>] [--singles <n>] [--series <n>] \
//!     [--callers <n>] [--batch <n>] [--rollups 0,1,3] [--triggers none,preceding] \
//!     [--reads <n>] --scratch <dir>
//! ```
//!
//! One cell is one fresh lake served by one `serve --mcp` process. The
//! tick type is `symbol`, `at` (under an order index), `price`, `size`
//! and `seq`, keyed by `symbol`, `at` and `seq`; `--series` symbols of
//! uneven size (a symbol of rank `r` has weight `1 / (r + 1)^0.9`) get
//! their ticks in time order, 50 ms of the lake's time apart, whatever
//! the wall clock says. The first `--singles` records are written by
//! `--callers` callers an `upsert` each; the rest by one caller in
//! `ingest` calls of `--batch` records. A roll-up is a kept view of the
//! ticks grouped by `[symbol, date_bucket(at, …)]` with `count`, `sum`,
//! `min`, `max`, `first` and `last`: `--rollups 1` keeps the hour,
//! `3` the minute, the hour and the day. A trigger `preceding` is judged
//! on every write: its bound is a constant, so it never fires.
//!
//! A row prints per cell: writes a second and the p99 of a write for the
//! callers' phase, records a second and the p99 of a call for the
//! ingest's, and bytes a record kept in the log (the log's own files, or
//! PostgreSQL's `log`, `frames` and `segments` with their indexes), in
//! the projection (its tables, indexes left out) and in indexes (the
//! projection's). Then the reads: one series' last hour and last day,
//! raw and from the roll-up, the median of `--reads` reads. The bytes of
//! a SQLite lake are read after the server has stopped; PostgreSQL's need
//! `--pg-plain`. Every knob is an argument.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use lakelands_api::location::{EngineConfig, Limits, PgTls, PgTlsMode, PgTuning};
use lakelands_api::principal::{Access, Grants, PrincipalKind};
use lakelands_api::read::{Direction, Find, Found, Order};
use lakelands_api::record::SourceRecord;
use lakelands_api::write::{Declaration, IngestOpts, WriteOpts};
use lakelands_api::{LakeId, LakeStore, Location, Principal, WriteContext};
use lakelands_client::{ApiKey, RemoteConfig, RemoteLake};
use lakelands_server::resolver::Entry;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

const LAKE: &str = "bench";
/// The lake's time between two ticks of the whole feed, in milliseconds.
const STEP_MS: u64 = 50;
/// 2026-10-01T00:00:00Z, in milliseconds since the epoch.
const BASE_MS: u64 = 1_790_812_800_000;

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

/// A root with one lake and a server bound to port 0; the PostgreSQL
/// schema the lake is in, where it is.
fn prepare(bin: &Path, root: &Path, pg: Option<(&str, bool)>) -> Option<String> {
    let r = root.display().to_string();
    cli(bin, &["init", &r]);
    let mut schema_out = None;
    match pg {
        None => cli(bin, &["--root", &r, "lake", "new", LAKE]),
        Some((url, plain)) => {
            let schema = format!(
                "lakelands_tick_{}_{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis()
            );
            schema_out = Some(schema.clone());
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
            ingest: true,
            restricted: true,
            ..Grants::default()
        },
        api_keys: vec!["k-writer".into()],
    }])
    .unwrap();
    config["server"] = json!({"bind": "127.0.0.1:0"});
    std::fs::write(&file, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    schema_out
}

/// The feed: which symbol a tick is of, and the numbers of the tick.
struct Feed {
    /// The cumulative weight of the symbols, ending at 1.
    cumulative: Vec<f64>,
    /// How many ticks each symbol has had.
    seqs: Vec<AtomicU64>,
    /// Which tick is next.
    next: AtomicU64,
}

impl Feed {
    fn new(series: usize) -> Feed {
        let weights: Vec<f64> = (0..series)
            .map(|r| 1.0 / (r as f64 + 1.0).powf(0.9))
            .collect();
        let total: f64 = weights.iter().sum();
        let mut sum = 0.0;
        let cumulative = weights
            .iter()
            .map(|w| {
                sum += w / total;
                sum
            })
            .collect();
        Feed {
            cumulative,
            seqs: (0..series).map(|_| AtomicU64::new(0)).collect(),
            next: AtomicU64::new(0),
        }
    }

    /// The symbol of tick `i`: a fixed function of `i`.
    fn rank(&self, i: u64) -> usize {
        let mut x = i.wrapping_add(0x9E37_79B9_7F4A_7C15);
        x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^= x >> 31;
        let u = (x >> 11) as f64 / (1u64 << 53) as f64;
        self.cumulative
            .partition_point(|c| *c < u)
            .min(self.cumulative.len() - 1)
    }

    /// The next tick's fields.
    fn tick(&self) -> (String, Value) {
        let i = self.next.fetch_add(1, Ordering::Relaxed);
        let rank = self.rank(i);
        let seq = self.seqs[rank].fetch_add(1, Ordering::Relaxed);
        let symbol = format!("S{rank:04}");
        let at = instant(BASE_MS + i * STEP_MS);
        let price = format!("{}.{:02}", 100 + (i / 7) % 50, i % 100);
        let fields = json!({"symbol": symbol, "at": at, "price": price,
                            "size": 1 + i % 500, "seq": seq});
        (key(&symbol, &at, seq), fields)
    }
}

fn key(symbol: &str, at: &str, seq: u64) -> String {
    format!("{symbol}:{}:{seq}", at.replace(':', "%3A"))
}

/// An instant, as the lake reads it.
fn instant(ms: u64) -> String {
    let (days, rest) = (ms / 86_400_000, ms % 86_400_000);
    // Days since 1970-01-01 to a civil date (Hinnant).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}000Z",
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1_000 % 60,
        rest % 1_000
    )
}

fn declarations(rollups: usize, trigger: &str) -> Vec<Value> {
    let origin = "'2026-01-01T00:00:00Z'";
    let field = |name: &str, kind: &str| json!({"name": name, "type": {"kind": kind}, "required": true, "admits": ["feed"]});
    let bucket = |name: &str, width: &str| {
        json!({"name": name, "type": {"kind": "date_time"}, "authority": "derived",
               "derived": format!("date_bucket(at, {width}, {origin})")})
    };
    let mut out = vec![
        json!({"declare": "type", "name": "tick", "key": ["symbol", "at", "seq"],
        "fields": [field("symbol", "text"), field("at", "date_time"), field("price", "decimal"),
                   field("size", "number"), field("seq", "number"),
                   bucket("m1", "1m"), bucket("h1", "1h"), bucket("d1", "1d")]}),
    ];
    let kept: &[(&str, &str)] = match rollups {
        0 => &[],
        1 => &[("h1", "tick_h1")],
        _ => &[("m1", "tick_m1"), ("h1", "tick_h1"), ("d1", "tick_d1")],
    };
    for (by, name) in kept {
        out.push(
            json!({"declare": "type", "name": name, "key": ["symbol", by],
            "fields": [{"name": "symbol", "type": {"kind": "text"}},
                       {"name": by, "type": {"kind": "date_time"}},
                       {"name": "n", "type": {"kind": "number"}},
                       {"name": "vol", "type": {"kind": "number"}},
                       {"name": "lo", "type": {"kind": "decimal"}},
                       {"name": "hi", "type": {"kind": "decimal"}},
                       {"name": "open", "type": {"kind": "decimal"}},
                       {"name": "close", "type": {"kind": "decimal"}}],
            "view": {"source": "tick", "group_by": ["symbol", by],
                "measures": [{"name": "n", "expr": "count()"},
                             {"name": "vol", "expr": "sum(size)"},
                             {"name": "lo", "expr": "min(price)"},
                             {"name": "hi", "expr": "max(price)"},
                             {"name": "open", "expr": "first(price, at)"},
                             {"name": "close", "expr": "last(price, at)"}]}}),
        );
    }
    if trigger == "preceding" {
        out.push(json!({"declare": "trigger", "name": "tick_guard", "on": "tick",
            "when": "preceding(price, at, symbol) is not null and price > preceding(price, at, symbol) + 1000000"}));
    }
    out
}

struct Cell {
    singles: (f64, Duration, Duration),
    ingest: (f64, Duration, Duration),
    /// Rows per series, largest first.
    largest: usize,
}

fn quantile(took: &mut [Duration], f: f64) -> Duration {
    took.sort();
    if took.is_empty() {
        return Duration::ZERO;
    }
    took[((took.len() as f64 * f) as usize).min(took.len() - 1)]
}

async fn load(
    remote: &Arc<RemoteLake>,
    feed: &Arc<Feed>,
    records: u64,
    singles: u64,
    callers: usize,
    batch: usize,
) -> Cell {
    let singles = singles.min(records);
    let began = Instant::now();
    let mut running = Vec::new();
    for _ in 0..callers {
        let (remote, feed) = (remote.clone(), feed.clone());
        running.push(tokio::spawn(async move {
            let cat = remote.catalog(WriteContext::default());
            let mut took = Vec::new();
            while feed.next.load(Ordering::Relaxed) < singles {
                let (k, fields) = feed.tick();
                let patch = serde_json::from_value(fields).unwrap();
                let at = Instant::now();
                cat.upsert(
                    &writer(),
                    &"tick".into(),
                    &k.into(),
                    patch,
                    WriteOpts::default(),
                )
                .await
                .expect("it applies");
                took.push(at.elapsed());
            }
            took
        }));
    }
    let mut took = Vec::new();
    for r in running {
        took.extend(r.await.unwrap());
    }
    let secs = began.elapsed().as_secs_f64();
    let singles_done = took.len() as f64;
    let singles_out = (
        singles_done / secs,
        quantile(&mut took.clone(), 0.5),
        quantile(&mut took, 0.99),
    );
    // The rest, by one caller in calls of `batch`.
    let cat = remote.catalog(WriteContext::default());
    let (mut calls, mut done) = (Vec::new(), 0u64);
    let began = Instant::now();
    while feed.next.load(Ordering::Relaxed) < records {
        let want = (records - feed.next.load(Ordering::Relaxed)).min(batch as u64);
        let recs: Vec<SourceRecord> = (0..want)
            .map(|_| {
                let (k, fields) = feed.tick();
                serde_json::from_value(json!({"type": "tick", "key": k, "fields": fields})).unwrap()
            })
            .collect();
        done += recs.len() as u64;
        let at = Instant::now();
        lakelands_api::catalog::ingest_all_with(
            &*cat,
            &writer(),
            &"feed".into(),
            IngestOpts::default(),
            recs,
        )
        .await
        .expect("ingests");
        calls.push(at.elapsed());
    }
    let secs = began.elapsed().as_secs_f64();
    let ingest_out = (
        done as f64 / secs.max(1e-9),
        quantile(&mut calls.clone(), 0.5),
        quantile(&mut calls, 0.99),
    );
    let largest = feed.seqs[0].load(Ordering::Relaxed) as usize;
    Cell {
        singles: singles_out,
        ingest: ingest_out,
        largest,
    }
}

/// The rows of a find, walked to the end; how many, and how long.
async fn walk(
    cat: &Arc<dyn lakelands_api::Catalog>,
    ty: &str,
    filter: &str,
    order: &str,
) -> (usize, Duration) {
    let at = Instant::now();
    let (mut rows, mut cursor) = (0, None);
    loop {
        let find = Find {
            filter: Some(filter.into()),
            order_by: vec![Order {
                field: order.into(),
                by: Default::default(),
                dir: Direction::Asc,
            }],
            limit: Some(1_000),
            cursor: cursor.take(),
            ..Find::default()
        };
        let got = cat.find(&writer(), &ty.into(), find).await.expect("finds");
        match got.data {
            Found::Records {
                rows: page,
                next_cursor,
                ..
            } => {
                rows += page.len();
                match next_cursor {
                    Some(c) => cursor = Some(c),
                    None => break,
                }
            }
            Found::Groups { .. } => unreachable!("a listing"),
        }
    }
    (rows, at.elapsed())
}

async fn median(
    reads: usize,
    cat: &Arc<dyn lakelands_api::Catalog>,
    ty: &str,
    filter: &str,
    order: &str,
) -> (usize, f64) {
    let (mut took, mut rows) = (Vec::new(), 0);
    for _ in 0..reads {
        let (r, t) = walk(cat, ty, filter, order).await;
        rows = r;
        took.push(t);
    }
    (rows, quantile(&mut took, 0.5).as_secs_f64() * 1e3)
}

fn dir_bytes(dir: &Path, keep: &dyn Fn(&str) -> bool) -> u64 {
    let mut total = 0;
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            total += dir_bytes(&p, keep);
        } else if keep(&e.file_name().to_string_lossy()) {
            total += e.metadata().map_or(0, |m| m.len());
        }
    }
    total
}

/// Bytes in the log, the projection's tables and its indexes.
fn sizes_sqlite(root: &Path) -> Option<(u64, u64, u64)> {
    let dir = root.join("lakes").join(LAKE);
    let log = dir_bytes(&dir, &|n| !n.starts_with("projection.db"));
    let db = dir.join("projection.db");
    // The `sqlite3` shell, whose `dbstat` is the pages of each b-tree.
    let out = std::process::Command::new("sqlite3")
        .arg("-readonly")
        .arg(&db)
        .arg(
            "SELECT COALESCE(SUM(pgsize), 0) FROM dbstat WHERE name IN \
             (SELECT name FROM sqlite_master WHERE type = 'index') \
             OR name LIKE 'sqlite_autoindex%'",
        )
        .output()
        .ok()?;
    let index: i64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    let all = dir_bytes(&dir, &|n| n.starts_with("projection.db"));
    Some((log, all.saturating_sub(index as u64), index as u64))
}

fn sizes_pg(url: &str, schema: &str) -> Option<(u64, u64, u64)> {
    let mut c = postgres::Client::connect(url, postgres::NoTls).ok()?;
    let rows = c
        .query(
            "SELECT c.relname, pg_table_size(c.oid), pg_indexes_size(c.oid) \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = $1 AND c.relkind = 'r'",
            &[&schema],
        )
        .ok()?;
    let (mut log, mut proj, mut index) = (0u64, 0u64, 0u64);
    for r in rows {
        let (name, table, idx): (String, i64, i64) = (r.get(0), r.get(1), r.get(2));
        if matches!(name.as_str(), "log" | "frames" | "segments") {
            log += (table + idx) as u64;
        } else {
            proj += table as u64;
            index += idx as u64;
        }
    }
    Some((log, proj, index))
}

#[allow(clippy::too_many_arguments)]
async fn cell(
    bin: &Path,
    scratch: &Path,
    pg: Option<(&str, bool)>,
    rollups: usize,
    trigger: &str,
    records: u64,
    a: &Args,
) {
    let root = scratch.join(format!("root-{}-{rollups}-{trigger}", std::process::id()));
    let schema = prepare(bin, &root, pg);
    let mut child = Command::new(bin)
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
        if let Some(x) = l.strip_prefix("lakelands: serving HTTP at http://") {
            break x.trim_end_matches('/').to_owned();
        }
    };
    let remote = Arc::new(
        RemoteLake::connect(RemoteConfig {
            url: format!("http://{addr}/"),
            lake: LakeId::from(LAKE),
            keys: BTreeMap::from([("writer".to_owned(), ApiKey::from("k-writer"))]),
            feed_key: None,
            ca_file: None,
            timeout_secs: 600,
            max_answer_bytes: lakelands_client::DEFAULT_MAX_ANSWER_BYTES,
            limits: Limits::default(),
        })
        .unwrap(),
    );
    let cat = remote.catalog(WriteContext::default());
    for d in declarations(rollups, trigger) {
        let decl: Declaration = serde_json::from_value(d.clone()).unwrap();
        cat.declare(&writer(), decl).await.expect("declared");
        if d["name"] == "tick" && d["declare"] == "type" {
            cat.upsert(
                &writer(),
                &lakelands_api::seam::SETTING_TYPE.into(),
                &"sort_index.tick".into(),
                serde_json::from_value(json!({"key": "sort_index.tick", "value": ["at"]})).unwrap(),
                WriteOpts::default(),
            )
            .await
            .expect("an order index on `at`");
        }
        if let Some(name) = d["name"]
            .as_str()
            .filter(|n| n.starts_with("tick_") && d["view"].is_object())
        {
            let key = format!("view_kept.{name}");
            cat.upsert(
                &writer(),
                &lakelands_api::seam::SETTING_TYPE.into(),
                &key.clone().into(),
                serde_json::from_value(json!({"key": key, "value": true})).unwrap(),
                WriteOpts::default(),
            )
            .await
            .expect("kept");
        }
    }
    let feed = Arc::new(Feed::new(a.series));
    let c = load(&remote, &feed, records, a.singles, a.callers, a.batch).await;
    // The reads, at the end of the feed: the largest series' last hour
    // and last day.
    let end = BASE_MS + records * STEP_MS;
    let (hour, day) = (instant(end - 3_600_000), instant(end - 86_400_000));
    let sym = "symbol = 'S0000'";
    let mut reads = Vec::new();
    for (label, from) in [("hour", &hour), ("day", &day)] {
        let (rows, ms) = median(
            a.reads,
            &cat,
            "tick",
            &format!("{sym} and at >= '{from}'"),
            "at",
        )
        .await;
        reads.push(format!("raw {label} {rows} rows {ms:.1} ms"));
    }
    if rollups > 0 {
        let finest = if rollups == 1 { "h1" } else { "m1" };
        for (label, view, by, from) in [
            ("hour", format!("tick_{finest}"), finest, &hour),
            ("day", "tick_h1".to_owned(), "h1", &day),
        ] {
            let (rows, ms) = median(
                a.reads,
                &cat,
                &view,
                &format!("{sym} and {by} >= '{from}'"),
                by,
            )
            .await;
            reads.push(format!("roll-up {label} {rows} rows {ms:.1} ms"));
        }
    }
    drop(cat);
    drop(remote);
    let _ = child.start_kill();
    let _ = child.wait().await;
    let bytes = match (&schema, pg) {
        (Some(s), Some((url, true))) => sizes_pg(url, s),
        (Some(_), _) => None,
        (None, _) => sizes_sqlite(&root),
    };
    let per = |b: u64| format!("{:.0}", b as f64 / records as f64);
    let (log, proj, index) = bytes
        .map_or(("n/a".into(), "n/a".into(), "n/a".into()), |(l, p, i)| {
            (per(l), per(p), per(i))
        });
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    println!(
        "| {} | {rollups} | {trigger} | {records} | {:.0} | {:.1} | {:.0} | {:.1} | {log} | {proj} | {index} |",
        if pg.is_some() { "postgres" } else { "sqlite" },
        c.singles.0,
        ms(c.singles.2),
        c.ingest.0,
        ms(c.ingest.2),
    );
    println!(
        "|   reads (largest series, {} ticks) | {} |",
        c.largest,
        reads.join(" | ")
    );
    if let Some((url, _)) = pg
        && let Some(s) = schema
        && let Ok(mut cl) = postgres::Client::connect(url, postgres::NoTls)
    {
        let _ = cl.batch_execute(&format!("DROP SCHEMA IF EXISTS {s} CASCADE"));
    }
    let _ = std::fs::remove_dir_all(&root);
}

struct Args {
    series: usize,
    singles: u64,
    callers: usize,
    batch: usize,
    reads: usize,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let mut args = std::env::args().skip(1);
    let (mut bin, mut scratch, mut pg_url, mut plain) = (None, None, None, false);
    let mut records = vec![100_000u64];
    let mut a = Args {
        series: 5_000,
        singles: 20_000,
        callers: 16,
        batch: 1_000,
        reads: 5,
    };
    let (mut rollups, mut triggers) = (
        vec![0usize, 1, 3],
        vec!["none".to_owned(), "preceding".to_owned()],
    );
    while let Some(arg) = args.next() {
        let mut v = || args.next().expect("a value");
        match arg.as_str() {
            "--lakelands" => bin = Some(PathBuf::from(v())),
            "--scratch" => scratch = Some(PathBuf::from(v())),
            "--pg-url" => pg_url = Some(v()),
            "--pg-plain" => plain = true,
            "--records" => records = v().split(',').map(|c| c.parse().unwrap()).collect(),
            "--singles" => a.singles = v().parse().unwrap(),
            "--series" => a.series = v().parse().unwrap(),
            "--callers" => a.callers = v().parse().unwrap(),
            "--batch" => a.batch = v().parse().unwrap(),
            "--reads" => a.reads = v().parse().unwrap(),
            "--rollups" => rollups = v().split(',').map(|c| c.parse().unwrap()).collect(),
            "--triggers" => triggers = v().split(',').map(str::to_owned).collect(),
            other => panic!("unknown argument {other}"),
        }
    }
    let bin = bin.expect("--lakelands");
    let scratch = scratch.expect("--scratch");
    println!(
        "| engine | roll-ups | trigger | records | callers: writes/s | p99 ms | ingest: records/s | p99 ms a call of {} | log B/rec | projection B/rec | index B/rec |",
        a.batch
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    for &n in &records {
        for &r in &rollups {
            for t in &triggers {
                cell(
                    &bin,
                    &scratch,
                    pg_url.as_deref().map(|u| (u, plain)),
                    r,
                    t,
                    n,
                    &a,
                )
                .await;
            }
        }
    }
}
