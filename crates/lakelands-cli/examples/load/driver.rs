//! A load run against a served lake, over every door it has.
//!
//! One `lakelands serve --mcp` process serves a lake from a scratch root:
//! its standard input and output are the MCP stdio door, and the port it
//! binds (port 0, the address read from its standard error) carries MCP
//! over streamable HTTP, REST and therefore `lakelands-client`. The driver
//! runs in another process, so the server's memory is its own.
//!
//! The run drives writes and reads on each door and then on all at once,
//! keeps subscriptions open while it does, streams a large ingest, reads
//! the change feed as server-sent events with reconnects, and exports the
//! lake over the wire. What it checks is exact: every write in the log
//! once, no notification lost or told twice outside the at-least-once
//! rule, the feed without a gap or a repeat, the export equal to the
//! served store's. What it measures (throughput, latency, the server's
//! memory) it reports as the bounds seen on the machine that ran it and
//! never asserts.
//!
//! Nothing waits to put things in order: every wait is for a condition,
//! under a deadline that turns a hang into a finding. Three waits are on
//! purpose, and only with `silence`: an ingest source that pauses past two
//! keep-alives, and a caught-up feed left idle for forty seconds, and a
//! session that reads nothing until the ingest is over (which needs no
//! clock, since the phases in between take their time anyway).
//!
//! The server process is killed when the run's guard drops, a panic
//! included. The scratch root is removed after a clean run and kept, its
//! path printed, after one with a finding.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::convert::Infallible;
use std::fmt::Write as _;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_core::Stream;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::body::{Frame, Incoming};
use hyper::{Method, Request, Response};
use hyper_util::rt::TokioIo;
use lakelands_api::event::EventBody;
use lakelands_api::location::{EngineConfig, Limits, PgTls, PgTlsMode, PgTuning};
use lakelands_api::principal::{Access, Grants, PrincipalKind, TypeGrants};
use lakelands_api::read::{Find, Found, Get, ReadAt};
use lakelands_api::record::SourceRecord;
use lakelands_api::seam::FeedFilter;
use lakelands_api::tools::{CHANGE_KINDS, Door, ToolSpec};
use lakelands_api::write::{Declaration, Outcome, WriteOpts};
use lakelands_api::{LakeId, LakeStore, Location, Position, Principal, WriteContext};
use lakelands_client::{ApiKey, RemoteConfig, RemoteLake};
use lakelands_server::HttpConfig;
use lakelands_server::resolver::Entry;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Notify, mpsc, oneshot};

/// The lake the run serves.
const LAKE: &str = "load";
/// A wait for something that should come: past it, the run records a
/// finding instead of hanging. It orders nothing.
const WAIT: Duration = Duration::from_secs(60);
/// The longest line an ingest body and the largest body an MCP request
/// may carry, as the server states them.
const CAP: usize = 4 << 20;
/// How the notification the server sends is named.
const CHANGED: &str = "notifications/lakelands/changed";
/// How long a source pauses, and a caught-up feed stays idle, in the
/// silence phases: past two of the server's fifteen-second keep-alives.
const SOURCE_PAUSE: Duration = Duration::from_secs(35);
const FEED_IDLE: Duration = Duration::from_secs(40);

// ------------------------------------------------------------------ plan

/// Where the lake is kept.
#[derive(Clone, Debug)]
pub enum Engine {
    /// On disk, in the scratch root.
    Sqlite,
    /// In the server's memory; the export is then compared with nothing.
    Memory,
    /// In a PostgreSQL database, in a schema of its own that the run
    /// leaves behind (its name is in the report).
    Postgres {
        /// The connection URL.
        url: String,
        /// Plaintext instead of TLS.
        plain: bool,
    },
}

/// How much the run does.
#[derive(Clone, Copy, Debug)]
pub struct Scale {
    /// Concurrent clients per door.
    pub clients: usize,
    /// Rounds each client runs; a round is about one and a half writes
    /// and a read.
    pub rounds: usize,
    /// Records in the streamed ingest.
    pub ingest: usize,
    /// Times the feed and the lagging session are dropped and resumed.
    pub reconnects: usize,
    /// Times the stdio session subscribes again while it is told.
    pub resubscribes: usize,
    /// Whether the phases that wait on purpose run.
    pub silence: bool,
}

impl Scale {
    /// The CI run: seconds, correctness only.
    pub fn short() -> Scale {
        Scale {
            clients: 2,
            rounds: 6,
            ingest: 300,
            reconnects: 2,
            resubscribes: 2,
            silence: false,
        }
    }

    /// The example's default.
    pub fn standard() -> Scale {
        Scale {
            clients: 8,
            rounds: 500,
            ingest: 200_000,
            reconnects: 10,
            resubscribes: 5,
            silence: true,
        }
    }

    /// The full run: a large ingest and a feed of millions of events.
    pub fn full() -> Scale {
        Scale {
            clients: 8,
            rounds: 2_000,
            ingest: 2_000_000,
            reconnects: 25,
            resubscribes: 10,
            silence: true,
        }
    }
}

/// One run.
#[derive(Clone, Debug)]
pub struct Plan {
    /// The `lakelands` binary.
    pub lakelands: PathBuf,
    /// The directory the scratch root is made in.
    pub scratch: PathBuf,
    /// How much.
    pub scale: Scale,
    /// Where the lake is kept.
    pub engine: Engine,
}

// ---------------------------------------------------------------- report

/// What a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Something a page or a rule promises did not hold.
    Defect,
    /// A bound the run met that a page should state.
    Limit,
}

/// One finding.
#[derive(Clone, Debug)]
pub struct Finding {
    /// Defect or limit.
    pub class: Class,
    /// The phase it was found in.
    pub phase: String,
    /// What was seen.
    pub what: String,
}

/// One line of the report: a phase.
#[derive(Clone, Debug)]
pub struct Line {
    /// The phase.
    pub phase: String,
    /// The line as printed.
    pub text: String,
    /// The same, as JSON.
    pub json: Value,
}

/// What the run found.
#[derive(Debug, Default)]
pub struct Report {
    /// One line per phase.
    pub lines: Vec<Line>,
    /// Defects and limits.
    pub findings: Vec<Finding>,
    /// The machine: cores and load before and after.
    pub machine: Value,
    /// The scratch root, kept when the run was not clean.
    pub scratch: Option<PathBuf>,
}

impl Report {
    fn defect(&mut self, phase: &str, what: impl Into<String>) {
        self.findings.push(Finding {
            class: Class::Defect,
            phase: phase.to_owned(),
            what: what.into(),
        });
    }

    fn limit(&mut self, phase: &str, what: impl Into<String>) {
        self.findings.push(Finding {
            class: Class::Limit,
            phase: phase.to_owned(),
            what: what.into(),
        });
    }

    fn line(&mut self, phase: &str, text: String, json: Value) {
        // Said as it happens, so a run that stalls shows how far it got.
        eprintln!("load: {phase}  {text}");
        self.lines.push(Line {
            phase: phase.to_owned(),
            text,
            json,
        });
    }

    /// The defects found.
    pub fn defects(&self) -> Vec<&Finding> {
        self.findings
            .iter()
            .filter(|f| f.class == Class::Defect)
            .collect()
    }

    /// The report as printed.
    pub fn text(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "machine  {}", self.machine);
        let width = self.lines.iter().map(|l| l.phase.len()).max().unwrap_or(0);
        for l in &self.lines {
            let _ = writeln!(out, "{:width$}  {}", l.phase, l.text);
        }
        for f in &self.findings {
            let class = match f.class {
                Class::Defect => "DEFECT",
                Class::Limit => "limit",
            };
            let _ = writeln!(out, "{class:6}  {}: {}", f.phase, f.what);
        }
        if let Some(dir) = &self.scratch {
            let _ = writeln!(out, "kept     {}", dir.display());
        }
        out
    }

    /// The report as JSON.
    pub fn json(&self) -> Value {
        json!({
            "machine": self.machine,
            "phases": self.lines.iter().map(|l| {
                let mut j = l.json.clone();
                j["phase"] = json!(l.phase);
                j
            }).collect::<Vec<_>>(),
            "findings": self.findings.iter().map(|f| json!({
                "class": match f.class { Class::Defect => "defect", Class::Limit => "limit" },
                "phase": f.phase,
                "what": f.what,
            })).collect::<Vec<_>>(),
            "scratch": self.scratch.as_ref().map(|d| d.display().to_string()),
        })
    }
}

// ---------------------------------------------------------- measurements

/// Latencies of one phase, by operation.
#[derive(Default)]
struct Timings(Mutex<BTreeMap<&'static str, Vec<Duration>>>);

impl Timings {
    fn add(&self, op: &'static str, d: Duration) {
        self.0.lock().unwrap().entry(op).or_default().push(d);
    }

    /// Per operation: count, rate over `wall`, and the percentiles.
    fn summary(&self, wall: Duration) -> (String, Value) {
        let mut text = String::new();
        let mut j = serde_json::Map::new();
        let mut all = 0;
        for (op, lat) in self.0.lock().unwrap().iter_mut() {
            lat.sort();
            all += lat.len();
            let p = |q: f64| lat[((lat.len() as f64 - 1.0) * q).round() as usize];
            let _ = write!(
                text,
                "  {op} {} ({:.0}/s, p50 {} p95 {} p99 {} max {})",
                lat.len(),
                lat.len() as f64 / wall.as_secs_f64(),
                ms(p(0.5)),
                ms(p(0.95)),
                ms(p(0.99)),
                ms(p(1.0)),
            );
            j.insert(
                (*op).to_owned(),
                json!({
                    "count": lat.len(),
                    "per_second": lat.len() as f64 / wall.as_secs_f64(),
                    "p50_ms": p(0.5).as_secs_f64() * 1e3,
                    "p95_ms": p(0.95).as_secs_f64() * 1e3,
                    "p99_ms": p(0.99).as_secs_f64() * 1e3,
                    "max_ms": p(1.0).as_secs_f64() * 1e3,
                }),
            );
        }
        let head = format!(
            "{all} ops in {:.1} s, {:.0} ops/s;",
            wall.as_secs_f64(),
            all as f64 / wall.as_secs_f64()
        );
        (format!("{head}{text}"), Value::Object(j))
    }
}

fn ms(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1e3)
}

fn mib(kib: u64) -> String {
    format!("{:.1} MiB", kib as f64 / 1024.0)
}

/// The server's resident memory, sampled every quarter second from
/// `/proc`; nothing on a system without it.
struct Memory {
    pid: u32,
    samples: Arc<Mutex<Vec<(Instant, u64)>>>,
    stop: Arc<AtomicBool>,
}

impl Memory {
    fn start(pid: u32) -> Memory {
        let samples = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (s, st) = (samples.clone(), stop.clone());
        std::thread::spawn(move || {
            while !st.load(Ordering::Relaxed) {
                match status(pid, "VmRSS:") {
                    Some(kib) => s.lock().unwrap().push((Instant::now(), kib)),
                    None => return,
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        });
        Memory { pid, samples, stop }
    }

    /// The server's memory over a phase: first, peak and last sample.
    fn over(&self, from: Instant, to: Instant) -> (String, Value) {
        let s = self.samples.lock().unwrap();
        let within: Vec<u64> = s
            .iter()
            .filter(|(t, _)| *t >= from && *t <= to)
            .map(|(_, k)| *k)
            .collect();
        match (within.first(), within.iter().max(), within.last()) {
            (Some(a), Some(p), Some(z)) => (
                format!("rss {} → {} (peak {})", mib(*a), mib(*z), mib(*p)),
                json!({"rss_start_kib": a, "rss_peak_kib": p, "rss_end_kib": z}),
            ),
            _ => ("rss none".into(), json!({"rss": "none"})),
        }
    }

    /// The peak the kernel recorded for the whole process.
    fn high_water(&self) -> Option<u64> {
        status(self.pid, "VmHWM:")
    }
}

impl Drop for Memory {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn status(pid: u32, field: &str) -> Option<u64> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    s.lines()
        .find_map(|l| l.strip_prefix(field))?
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()
}

fn machine() -> Value {
    let load = std::fs::read_to_string("/proc/loadavg")
        .ok()
        .map(|l| l.split_whitespace().take(3).collect::<Vec<_>>().join(" "));
    json!({
        "cores": std::thread::available_parallelism().map(|n| n.get()).ok(),
        "load": load.unwrap_or_else(|| "none".into()),
    })
}

// ----------------------------------------------------------- the server

/// The scratch root: removed when the run is clean, kept otherwise.
struct Scratch {
    dir: PathBuf,
    clean: bool,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if self.clean {
            let _ = std::fs::remove_dir_all(&self.dir);
        } else if self.dir.exists() {
            eprintln!("load: the scratch root is kept at {}", self.dir.display());
        }
    }
}

/// The served lake's process. It is killed when this drops.
struct Server {
    child: Child,
    pid: u32,
    addr: SocketAddr,
    stderr: Arc<Mutex<Vec<String>>>,
}

fn cli(plan: &Plan, root: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new(&plan.lakelands)
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| format!("{} does not run: {e}", plan.lakelands.display()))?;
    if !out.status.success() {
        return Err(format!(
            "lakelands {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn principals() -> Vec<Entry> {
    let loader = Grants {
        default: Access::Write,
        review: true,
        schema: true,
        ingest: true,
        restricted: true,
        ..Grants::default()
    };
    let watcher = Grants {
        default: Access::Read,
        types: [
            (
                "note".into(),
                TypeGrants {
                    default: None,
                    fields: [("secret".into(), Access::None)].into(),
                },
            ),
            (
                "hidden".into(),
                TypeGrants {
                    default: Some(Access::None),
                    fields: BTreeMap::new(),
                },
            ),
        ]
        .into(),
        ..Grants::default()
    };
    [("loader", loader), ("watcher", watcher)]
        .into_iter()
        .map(|(id, grants)| Entry {
            name: id.into(),
            kind: PrincipalKind::Agent,
            id: id.into(),
            lakes: Vec::new(),
            grants,
            api_keys: vec![format!("k-{id}")],
        })
        .collect()
}

/// Make the root, the lake and the configuration.
fn prepare(plan: &Plan, root: &Path) -> Result<Option<String>, String> {
    let r = root.display().to_string();
    let out = std::process::Command::new(&plan.lakelands)
        .args(["init", &r])
        .output()
        .map_err(|e| format!("{} does not run: {e}", plan.lakelands.display()))?;
    if !out.status.success() {
        return Err(format!(
            "init failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let mut schema = None;
    match &plan.engine {
        Engine::Sqlite => {
            cli(plan, root, &["lake", "new", LAKE])?;
        }
        Engine::Memory => {
            cli(plan, root, &["lake", "new", LAKE, "--engine", "memory"])?;
        }
        Engine::Postgres { url, plain } => {
            let name = format!(
                "lakelands_load_{}_{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            );
            let location = Location {
                lake: LAKE.into(),
                engine: EngineConfig::Postgres {
                    url: url.clone(),
                    schema: Some(name.clone()),
                    tls: PgTls {
                        mode: if *plain {
                            PgTlsMode::Disable
                        } else {
                            PgTlsMode::Require
                        },
                        ..PgTls::default()
                    },
                    tuning: PgTuning::default(),
                },
                key: None,
                limits: Limits::default(),
            };
            let lakes = root.join("lakes");
            std::fs::create_dir_all(&lakes).map_err(|e| e.to_string())?;
            std::fs::write(
                lakes.join(format!("{LAKE}.json")),
                serde_json::to_vec_pretty(&location).unwrap(),
            )
            .map_err(|e| e.to_string())?;
            schema = Some(name);
        }
    }
    let file = root.join("lakelands.json");
    let mut config: Value =
        serde_json::from_slice(&std::fs::read(&file).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    config["principals"] = serde_json::to_value(principals()).unwrap();
    config["server"] = serde_json::to_value(HttpConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        allowed_origins: Vec::new(),
        session_idle_secs: 1800,
        feed_poll_millis: 250,
    })
    .unwrap();
    std::fs::write(&file, serde_json::to_vec_pretty(&config).unwrap())
        .map_err(|e| e.to_string())?;
    Ok(schema)
}

impl Server {
    /// Start `serve --mcp` as the loader and wait for the address it
    /// binds.
    async fn start(
        plan: &Plan,
        root: &Path,
    ) -> Result<(Server, ChildStdin, tokio::process::ChildStdout), String> {
        let mut child = Command::new(&plan.lakelands)
            .arg("--root")
            .arg(root)
            .args(["--as", "loader", "serve", "--mcp"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("the server does not start: {e}"))?;
        let pid = child.id().ok_or("the server has no process id")?;
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let addr = tokio::time::timeout(WAIT, async {
            while let Ok(Some(l)) = lines.next_line().await {
                stderr.lock().unwrap().push(l.clone());
                if let Some(a) = l.strip_prefix("lakelands: serving HTTP at http://") {
                    return a.trim_end_matches('/').parse::<SocketAddr>().ok();
                }
            }
            None
        })
        .await
        .ok()
        .flatten()
        .ok_or_else(|| {
            format!(
                "the server named no address: {}",
                stderr.lock().unwrap().join(" | ")
            )
        })?;
        let keep = stderr.clone();
        tokio::spawn(async move {
            while let Ok(Some(l)) = lines.next_line().await {
                keep.lock().unwrap().push(l);
            }
        });
        Ok((
            Server {
                child,
                pid,
                addr,
                stderr,
            },
            stdin,
            stdout,
        ))
    }
}

// -------------------------------------------------------------- failure

/// Why a call failed: no answer within [`WAIT`], which stops a phase's
/// clients since every later call would wait as long, or an answer that
/// is wrong.
#[derive(Debug)]
enum Oops {
    Stall(String),
    Wrong(String),
}

impl Oops {
    fn stall(what: impl Into<String>) -> Oops {
        Oops::Stall(what.into())
    }
}

impl From<String> for Oops {
    fn from(what: String) -> Oops {
        Oops::Wrong(what)
    }
}

impl From<&str> for Oops {
    fn from(what: &str) -> Oops {
        Oops::Wrong(what.to_owned())
    }
}

impl std::fmt::Display for Oops {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Oops::Stall(w) => write!(f, "{w} (no answer within {} s)", WAIT.as_secs()),
            Oops::Wrong(w) => f.write_str(w),
        }
    }
}

impl From<Oops> for String {
    fn from(o: Oops) -> String {
        o.to_string()
    }
}

/// A client door's error: `unavailable` is the server not answering.
fn remote_oops(e: lakelands_api::Error) -> Oops {
    match e {
        lakelands_api::Error::Unavailable { message } => Oops::Stall(message),
        other => Oops::Wrong(format!("{other:?}")),
    }
}

// ------------------------------------------------------------- HTTP

type Body = BoxBody<Bytes, Infallible>;

fn full(b: impl Into<Bytes>) -> Body {
    Full::new(b.into()).boxed()
}

/// A body sent as it is produced.
struct Streamed(mpsc::Receiver<Bytes>);

impl hyper::body::Body for Streamed {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        self.get_mut()
            .0
            .poll_recv(cx)
            .map(|b| b.map(|b| Ok(Frame::data(b))))
    }
}

/// One HTTP/1.1 connection, kept open.
struct Conn {
    addr: SocketAddr,
    tx: hyper::client::conn::http1::SendRequest<Body>,
}

impl Conn {
    async fn open(addr: SocketAddr) -> Result<Conn, String> {
        let s = tokio::net::TcpStream::connect(addr)
            .await
            .map_err(|e| format!("connect: {e}"))?;
        let _ = s.set_nodelay(true);
        let (tx, conn) = hyper::client::conn::http1::handshake(TokioIo::new(s))
            .await
            .map_err(|e| format!("handshake: {e}"))?;
        tokio::spawn(conn);
        Ok(Conn { addr, tx })
    }

    fn request(&self, method: Method, path: &str, key: &str) -> hyper::http::request::Builder {
        Request::builder()
            .method(method)
            .uri(path)
            .header("host", self.addr.to_string())
            .header("authorization", format!("Bearer {key}"))
    }

    async fn send(&mut self, req: Request<Body>) -> Result<Response<Incoming>, Oops> {
        match tokio::time::timeout(WAIT, self.tx.ready()).await {
            Err(_) => return Err(Oops::stall("the connection was not ready")),
            // The server closed the connection; open another.
            Ok(Err(_)) => *self = Conn::open(self.addr).await?,
            Ok(Ok(())) => {}
        }
        tokio::time::timeout(WAIT, self.tx.send_request(req))
            .await
            .map_err(|_| Oops::stall("no answer"))?
            .map_err(|e| Oops::Wrong(format!("request: {e}")))
    }

    /// One exchange whose answer is JSON.
    async fn json(&mut self, req: Request<Body>) -> Result<(u16, Value), Oops> {
        let r = self.send(req).await?;
        let status = r.status().as_u16();
        let body = whole(r.into_body()).await?;
        let v = serde_json::from_slice(&body).unwrap_or(Value::Null);
        Ok((status, v))
    }
}

/// A whole answer's body, within [`WAIT`].
async fn whole(body: Incoming) -> Result<Bytes, Oops> {
    Ok(tokio::time::timeout(WAIT, body.collect())
        .await
        .map_err(|_| Oops::stall("the answer did not end"))?
        .map_err(|e| Oops::Wrong(format!("body: {e}")))?
        .to_bytes())
}

/// Server-sent events, read from a body as they come.
enum Sse {
    Event { id: Option<u64>, data: String },
    Comment(String),
}

struct SseReader {
    body: Incoming,
    buf: Vec<u8>,
    id: Option<u64>,
    data: Option<String>,
    queue: VecDeque<Sse>,
}

impl SseReader {
    fn new(body: Incoming) -> SseReader {
        SseReader {
            body,
            buf: Vec::new(),
            id: None,
            data: None,
            queue: VecDeque::new(),
        }
    }

    /// The next event or comment; `Ok(None)` when the stream ends.
    async fn next(&mut self) -> Result<Option<Sse>, String> {
        loop {
            if let Some(s) = self.queue.pop_front() {
                return Ok(Some(s));
            }
            while let Some(i) = self.buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.buf.drain(..=i).collect();
                let line = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
                let line = line.trim_end_matches('\r');
                if line.is_empty() {
                    if let Some(data) = self.data.take() {
                        self.queue.push_back(Sse::Event {
                            id: self.id.take(),
                            data,
                        });
                    }
                    self.id = None;
                } else if let Some(c) = line.strip_prefix(':') {
                    self.queue.push_back(Sse::Comment(c.trim().to_owned()));
                } else if let Some(v) = line.strip_prefix("id:") {
                    self.id = v.trim().parse().ok();
                } else if let Some(v) = line.strip_prefix("data:") {
                    let v = v.strip_prefix(' ').unwrap_or(v);
                    match &mut self.data {
                        Some(d) => {
                            d.push('\n');
                            d.push_str(v);
                        }
                        None => self.data = Some(v.to_owned()),
                    }
                }
            }
            if !self.queue.is_empty() {
                continue;
            }
            match self.body.frame().await {
                None => return Ok(None),
                Some(Err(e)) => return Err(format!("the stream broke: {e}")),
                Some(Ok(f)) => {
                    if let Ok(d) = f.into_data() {
                        self.buf.extend_from_slice(&d);
                    }
                }
            }
        }
    }
}

// ------------------------------------------------------------ notifications

/// What one session was told: each position, in the order read, with
/// the number of subscribe answers read before it.
#[derive(Default)]
struct Told {
    seen: Mutex<Vec<(u64, usize)>>,
    epoch: AtomicU64,
    /// The `from` of each subscription, by epoch.
    froms: Mutex<Vec<u64>>,
    changed: Notify,
}

impl Told {
    /// A subscription answered, from `from`: what is read after it is
    /// under it.
    fn subscribed(&self, from: u64) {
        self.froms.lock().unwrap().push(from);
        self.epoch.fetch_add(1, Ordering::SeqCst);
    }

    fn add(&self, p: u64) {
        let e = self.epoch.load(Ordering::SeqCst) as usize;
        self.seen.lock().unwrap().push((p, e));
        self.changed.notify_waiters();
    }

    fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    fn last(&self) -> Option<u64> {
        self.seen.lock().unwrap().last().map(|(p, _)| *p)
    }

    /// Wait until every expected position has been told; a wait with no
    /// progress for [`WAIT`] ends it.
    ///
    /// Each wake looks only at what was told since the last one, so the
    /// wait costs the checker once per notification, not once per
    /// notification per position.
    async fn until_told(&self, expected: &[u64]) -> Result<(), String> {
        let mut remaining: std::collections::HashSet<u64> = expected.iter().copied().collect();
        let mut read = 0;
        loop {
            let n = self.changed.notified();
            tokio::pin!(n);
            n.as_mut().enable();
            {
                let seen = self.seen.lock().unwrap();
                for (p, _) in &seen[read..] {
                    remaining.remove(p);
                }
                read = seen.len();
            }
            if remaining.is_empty() {
                return Ok(());
            }
            if tokio::time::timeout(WAIT, n).await.is_err() {
                let first = remaining.iter().min().copied();
                return Err(format!(
                    "{} change(s) never told, the first at position {first:?}",
                    remaining.len()
                ));
            }
        }
    }

    /// Check what was told against what should have been: nothing
    /// missing, nothing it should not see, and a repeat only where a new
    /// subscription asked for it again (its `from` at or below the
    /// position) or as the one change on its way when the subscription
    /// was replaced.
    fn account(&self, expected: &[u64], window: (u64, u64)) -> (Value, Vec<String>) {
        let seen = self.seen.lock().unwrap();
        let froms = self.froms.lock().unwrap();
        let mut defects = Vec::new();
        let mut first: HashMap<u64, usize> = HashMap::new();
        let mut asked_again = 0;
        let mut strays: BTreeMap<usize, usize> = BTreeMap::new();
        let mut dup = 0;
        let mut extra = 0;
        for (p, e) in seen.iter() {
            if *p < window.0 || *p > window.1 {
                continue;
            }
            if expected.binary_search(p).is_err() {
                extra += 1;
                if extra <= 3 {
                    defects.push(format!("told position {p}, which it should not have been"));
                }
                continue;
            }
            let from = e.checked_sub(1).and_then(|i| froms.get(i));
            if *e > 1 && from.is_some_and(|f| p < f) {
                *strays.entry(*e).or_default() += 1;
            }
            match first.get(p) {
                None => {
                    first.insert(*p, *e);
                }
                Some(e0) => {
                    dup += 1;
                    let again = *e > *e0 && from.is_some_and(|f| p >= f);
                    let in_flight = *e > *e0;
                    if again {
                        asked_again += 1;
                    } else if !in_flight {
                        defects.push(format!("position {p} told twice under one subscription"));
                    }
                }
            }
        }
        for (e, n) in &strays {
            if *n > 1 {
                defects.push(format!(
                    "{n} changes of the replaced subscription were told after subscription {e} answered; the rule allows one"
                ));
            }
        }
        let missing = expected.iter().filter(|p| !first.contains_key(p)).count();
        if missing > 0 {
            defects.push(format!("{missing} change(s) never told"));
        }
        (
            json!({
                "expected": expected.len(),
                "told": first.len(),
                "missing": missing,
                "repeated": dup,
                "asked_again": asked_again,
                "not_allowed": extra,
                "subscriptions": froms.len(),
            }),
            defects,
        )
    }
}

/// The MCP session on the server's standard input and output.
struct Stdio {
    to: tokio::sync::Mutex<Option<ChildStdin>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    id: AtomicU64,
    told: Arc<Told>,
    subscribes: Mutex<BTreeSet<u64>>,
}

impl Stdio {
    fn start(stdin: ChildStdin, stdout: tokio::process::ChildStdout) -> Arc<Stdio> {
        let s = Arc::new(Stdio {
            to: tokio::sync::Mutex::new(Some(stdin)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            id: AtomicU64::new(0),
            told: Arc::new(Told::default()),
            subscribes: Mutex::new(BTreeSet::new()),
        });
        let me = s.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let Ok(m) = serde_json::from_str::<Value>(&l) else {
                    continue;
                };
                if m["method"] == CHANGED {
                    if let Some(p) = m["params"]["position"].as_u64() {
                        me.told.add(p);
                    }
                } else if let Some(id) = m["id"].as_u64() {
                    // A subscribe answer read here starts a new epoch
                    // for what follows it on the stream.
                    if me.subscribes.lock().unwrap().remove(&id) {
                        let from = m["result"]["structuredContent"]["from"].as_u64();
                        me.told.subscribed(from.unwrap_or(0));
                    }
                    if let Some(tx) = me.pending.lock().unwrap().remove(&id) {
                        let _ = tx.send(m);
                    }
                }
            }
        });
        s
    }

    async fn request(&self, method: &str, params: Value, subscribe: bool) -> Result<Value, Oops> {
        let id = self.id.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        if subscribe {
            self.subscribes.lock().unwrap().insert(id);
        }
        let mut line = serde_json::to_vec(
            &json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        )
        .unwrap();
        line.push(b'\n');
        let sent = tokio::time::timeout(WAIT, async {
            let mut to = self.to.lock().await;
            let to = to.as_mut().ok_or("the session is closed")?;
            to.write_all(&line)
                .await
                .map_err(|e| format!("stdin: {e}"))?;
            to.flush().await.map_err(|e| format!("stdin: {e}"))
        })
        .await;
        let answer = match sent {
            Err(_) => Err(Oops::stall(format!("{method} could not be sent"))),
            Ok(Err(e)) => Err(Oops::Wrong(e)),
            Ok(Ok(())) => match tokio::time::timeout(WAIT, rx).await {
                Err(_) => Err(Oops::stall(format!("no answer to {method}"))),
                Ok(r) => r.map_err(|_| Oops::from("the server's output ended")),
            },
        };
        if answer.is_err() {
            // An answer that never came is not waited for again.
            self.pending.lock().unwrap().remove(&id);
            self.subscribes.lock().unwrap().remove(&id);
        }
        answer
    }

    /// Close the server's standard input: the server ends with it.
    async fn close(&self) {
        self.to.lock().await.take();
    }

    async fn call(&self, tool: &str, arguments: Value) -> Result<Value, Oops> {
        let subscribe = tool == generic("subscribe");
        let r = self
            .request(
                "tools/call",
                json!({"name": tool, "arguments": arguments}),
                subscribe,
            )
            .await?;
        tool_answer(&r)
    }
}

/// The generator's name for a generic tool.
fn generic(name: &str) -> &'static str {
    lakelands_api::tools::GENERIC_TOOLS
        .iter()
        .find(|t| **t == name)
        .unwrap_or_else(|| panic!("the generator names no generic tool `{name}`"))
}

/// A tool's answer; a refusal is `Err` with the contract's error as JSON
/// when the answer carries it.
fn tool_answer(r: &Value) -> Result<Value, Oops> {
    if r["result"]["isError"] == true {
        let error = r["result"]["content"][1]["text"]
            .as_str()
            .and_then(|t| serde_json::from_str::<Value>(t).ok())
            .unwrap_or_else(|| r["result"]["content"].clone());
        return Err(Oops::Wrong(error.to_string()));
    }
    if !r["error"].is_null() {
        return Err(Oops::Wrong(r["error"].to_string()));
    }
    Ok(r["result"]["structuredContent"].clone())
}

/// One MCP session over streamable HTTP.
struct HttpMcp {
    key: String,
    sid: String,
    id: AtomicU64,
}

/// Where the lake's MCP session lives.
fn mcp_path() -> String {
    format!("/lakes/{LAKE}/mcp")
}

impl HttpMcp {
    async fn open(conn: &mut Conn, key: &str) -> Result<HttpMcp, Oops> {
        let body = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize",
                          "params": {"protocolVersion": "2025-06-18"}});
        let req = conn
            .request(Method::POST, &mcp_path(), key)
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .body(full(body.to_string()))
            .unwrap();
        let r = conn.send(req).await?;
        let sid = r
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
            .ok_or("initialize named no session")?;
        whole(r.into_body()).await?;
        Ok(HttpMcp {
            key: key.to_owned(),
            sid,
            id: AtomicU64::new(0),
        })
    }

    /// Post one raw body; the status and the answer.
    async fn post(&self, conn: &mut Conn, body: Bytes) -> Result<(u16, Value), Oops> {
        let req = conn
            .request(Method::POST, &mcp_path(), &self.key)
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .header("mcp-session-id", &self.sid)
            .header("mcp-protocol-version", "2025-06-18")
            .body(full(body))
            .unwrap();
        conn.json(req).await
    }

    async fn call(&self, conn: &mut Conn, tool: &str, arguments: Value) -> Result<Value, Oops> {
        let id = self.id.fetch_add(1, Ordering::SeqCst) + 1;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                         "params": {"name": tool, "arguments": arguments}});
        let (status, r) = self.post(conn, Bytes::from(msg.to_string())).await?;
        if status != 200 {
            return Err(Oops::Wrong(format!("{status}: {r}")));
        }
        tool_answer(&r)
    }

    /// Read the session's event stream into `told`, resuming after the
    /// last event each time it is dropped: after every `every` changes,
    /// `reconnects` times. Runs until aborted.
    fn listen(
        self: &Arc<Self>,
        addr: SocketAddr,
        told: Arc<Told>,
        every: usize,
        mut reconnects: usize,
        faults: Arc<Mutex<Vec<String>>>,
    ) -> tokio::task::JoinHandle<()> {
        let me = self.clone();
        tokio::spawn(async move {
            let mut last: Option<u64> = None;
            loop {
                let Ok(mut conn) = Conn::open(addr).await else {
                    faults
                        .lock()
                        .unwrap()
                        .push("the event stream does not connect".into());
                    return;
                };
                let mut req = conn
                    .request(Method::GET, &mcp_path(), &me.key)
                    .header("accept", "text/event-stream")
                    .header("mcp-session-id", &me.sid)
                    .header("mcp-protocol-version", "2025-06-18");
                if let Some(p) = last {
                    req = req.header("last-event-id", p.to_string());
                }
                let r = match conn.send(req.body(full("")).unwrap()).await {
                    Ok(r) if r.status() == 200 => r,
                    Ok(r) => {
                        faults
                            .lock()
                            .unwrap()
                            .push(format!("the event stream answered {}", r.status()));
                        return;
                    }
                    Err(e) => {
                        faults
                            .lock()
                            .unwrap()
                            .push(format!("the event stream: {e}"));
                        return;
                    }
                };
                let mut sse = SseReader::new(r.into_body());
                let mut n = 0;
                loop {
                    match sse.next().await {
                        Ok(Some(Sse::Event { id, data })) => {
                            let Ok(m) = serde_json::from_str::<Value>(&data) else {
                                continue;
                            };
                            if m["method"] == CHANGED
                                && let Some(p) = m["params"]["position"].as_u64()
                            {
                                if id != Some(p) {
                                    faults.lock().unwrap().push(format!(
                                        "an event's id {id:?} is not its position {p}"
                                    ));
                                }
                                told.add(p);
                                last = Some(p);
                                n += 1;
                                if reconnects > 0 && n >= every {
                                    reconnects -= 1;
                                    break;
                                }
                            }
                        }
                        Ok(Some(Sse::Comment(_))) => {}
                        Ok(None) => {
                            faults.lock().unwrap().push("the event stream ended".into());
                            return;
                        }
                        Err(e) => {
                            faults.lock().unwrap().push(e);
                            return;
                        }
                    }
                }
            }
        })
    }
}

// ----------------------------------------------------------------- doors

/// The tool names the generator makes for this lake.
struct Names {
    upsert_note: String,
    get_note: String,
    find_note: String,
    retire_note: String,
    link_author: String,
    upsert_person: String,
    upsert_hidden: String,
}

impl Names {
    fn of(specs: &[ToolSpec]) -> Result<Names, String> {
        let name = |door: Door| {
            specs
                .iter()
                .find(|s| s.door == door)
                .map(|s| s.name.clone())
                .ok_or_else(|| format!("the generator makes no tool for {door:?}"))
        };
        Ok(Names {
            upsert_note: name(Door::Upsert { ty: "note".into() })?,
            get_note: name(Door::Get { ty: "note".into() })?,
            find_note: name(Door::Find { ty: "note".into() })?,
            retire_note: name(Door::Retire { ty: "note".into() })?,
            link_author: name(Door::Link {
                ty: "note".into(),
                relation: "author".into(),
            })?,
            upsert_person: name(Door::Upsert {
                ty: "person".into(),
            })?,
            upsert_hidden: name(Door::Upsert {
                ty: "hidden".into(),
            })?,
        })
    }

    fn upsert(&self, ty: &str) -> &str {
        match ty {
            "note" => &self.upsert_note,
            "person" => &self.upsert_person,
            _ => &self.upsert_hidden,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Via {
    Stdio,
    McpHttp,
    Rest,
    Client,
}

impl Via {
    fn name(self) -> &'static str {
        match self {
            Via::Stdio => "stdio",
            Via::McpHttp => "mcp-http",
            Via::Rest => "rest",
            Via::Client => "client",
        }
    }
}

fn loader() -> Principal {
    Principal {
        kind: PrincipalKind::Agent,
        id: "loader".into(),
        lake: LAKE.into(),
        grants: Grants::default(),
    }
}

/// One client on one door.
struct Worker {
    via: Via,
    stdio: Arc<Stdio>,
    names: Arc<Names>,
    remote: Arc<RemoteLake>,
    conn: Option<Conn>,
    mcp: Option<HttpMcp>,
}

fn applied(v: &Value) -> Result<u64, Oops> {
    if v["outcome"] != "applied" {
        return Err(Oops::Wrong(format!("not applied: {v}")));
    }
    v["position"]
        .as_u64()
        .ok_or_else(|| Oops::Wrong(format!("no position: {v}")))
}

/// A read's answer: what it carries beside its position.
fn data(v: &Value) -> Value {
    match v.get("data") {
        Some(d) => d.clone(),
        None => v.clone(),
    }
}

fn outcome(o: lakelands_api::Result<Outcome>) -> Result<u64, Oops> {
    match o.map_err(remote_oops)? {
        Outcome::Applied { position, .. } => Ok(position.0),
        other => Err(Oops::Wrong(format!("not applied: {other:?}"))),
    }
}

impl Worker {
    async fn new(
        via: Via,
        addr: SocketAddr,
        stdio: Arc<Stdio>,
        names: Arc<Names>,
        remote: Arc<RemoteLake>,
    ) -> Result<Worker, Oops> {
        let mut conn = match via {
            Via::McpHttp | Via::Rest => Some(Conn::open(addr).await?),
            _ => None,
        };
        let mcp = match (via, conn.as_mut()) {
            (Via::McpHttp, Some(c)) => Some(HttpMcp::open(c, "k-loader").await?),
            _ => None,
        };
        Ok(Worker {
            via,
            stdio,
            names,
            remote,
            conn,
            mcp,
        })
    }

    async fn tool(&mut self, tool: &str, args: Value) -> Result<Value, Oops> {
        match self.via {
            Via::Stdio => self.stdio.call(tool, args).await,
            _ => {
                let (mcp, conn) = (self.mcp.as_ref().unwrap(), self.conn.as_mut().unwrap());
                mcp.call(conn, tool, args).await
            }
        }
    }

    async fn rest(
        &mut self,
        method: Method,
        path: &str,
        headers: &[(&str, String)],
        body: Option<Value>,
    ) -> Result<Value, Oops> {
        let conn = self.conn.as_mut().unwrap();
        let mut req = conn.request(method, &format!("/lakes/{LAKE}{path}"), "k-loader");
        for (h, v) in headers {
            req = req.header(*h, v);
        }
        let req = match body {
            Some(b) => req
                .header("content-type", "application/json")
                .body(full(b.to_string())),
            None => req.body(full("")),
        }
        .unwrap();
        let (status, v) = conn.json(req).await?;
        if status >= 300 {
            return Err(Oops::Wrong(format!("{status}: {v}")));
        }
        Ok(v)
    }

    /// Write fields of one record, under an idempotency key and against
    /// an expected version when given; the position it was applied at.
    async fn upsert(
        &mut self,
        ty: &str,
        key: &str,
        fields: Value,
        idem: Option<&str>,
        expected: Option<u64>,
    ) -> Result<u64, Oops> {
        match self.via {
            Via::Stdio | Via::McpHttp => {
                let mut args = json!({"key": key, "fields": fields});
                if let Some(i) = idem {
                    args["idempotency_key"] = json!(i);
                }
                if let Some(v) = expected {
                    args["expected_version"] = json!(v);
                }
                let name = self.names.upsert(ty).to_owned();
                applied(&self.tool(&name, args).await?)
            }
            Via::Rest => {
                let mut h = Vec::new();
                if let Some(i) = idem {
                    h.push(("idempotency-key", i.to_owned()));
                }
                if let Some(v) = expected {
                    h.push(("if-match", format!("\"{v}\"")));
                }
                let path = format!("/types/{ty}/records/{key}");
                applied(&self.rest(Method::PUT, &path, &h, Some(fields)).await?)
            }
            Via::Client => {
                let patch = serde_json::from_value(fields).map_err(|e| e.to_string())?;
                let opts = WriteOpts {
                    idempotency_key: idem.map(|i| i.to_owned().into()),
                    expected_version: expected.map(lakelands_api::id::Version),
                };
                outcome(
                    self.remote
                        .catalog(WriteContext::default())
                        .upsert(&loader(), &ty.into(), &key.into(), patch, opts)
                        .await,
                )
            }
        }
    }

    /// Write fields of `key` against the version read: the answer with
    /// the versions it names, applied or `conflict`.
    async fn cas(&mut self, key: &str, fields: Value, expected: u64) -> Result<Cas, Oops> {
        let answer: Result<Value, Value> = match self.via {
            Via::Stdio | Via::McpHttp => {
                let args = json!({"key": key, "fields": fields, "expected_version": expected});
                let name = self.names.upsert_note.clone();
                match self.tool(&name, args).await {
                    Ok(v) => Ok(v),
                    Err(Oops::Wrong(w)) => match serde_json::from_str::<Value>(&w) {
                        Ok(e) if e["error"] == "conflict" => Err(e),
                        _ => return Err(Oops::Wrong(w)),
                    },
                    Err(o) => return Err(o),
                }
            }
            Via::Rest => {
                let conn = self.conn.as_mut().unwrap();
                let req = conn
                    .request(
                        Method::PUT,
                        &format!("/lakes/{LAKE}/types/note/records/{key}"),
                        "k-loader",
                    )
                    .header("if-match", format!("\"{expected}\""))
                    .header("content-type", "application/json")
                    .body(full(fields.to_string()))
                    .unwrap();
                match conn.json(req).await? {
                    (200, v) => Ok(v),
                    (412, e) => Err(e),
                    (status, v) => return Err(Oops::Wrong(format!("{status}: {v}"))),
                }
            }
            Via::Client => {
                let patch = serde_json::from_value(fields).map_err(|e| e.to_string())?;
                let opts = WriteOpts {
                    idempotency_key: None,
                    expected_version: Some(lakelands_api::id::Version(expected)),
                };
                match self
                    .remote
                    .catalog(WriteContext::default())
                    .upsert(&loader(), &"note".into(), &key.into(), patch, opts)
                    .await
                {
                    Ok(o) => Ok(serde_json::to_value(o).unwrap()),
                    Err(lakelands_api::Error::Conflict { expected, actual }) => {
                        Err(json!({"expected": expected.0, "actual": actual.0}))
                    }
                    Err(e) => return Err(remote_oops(e)),
                }
            }
        };
        let num = |v: &Value, k: &str| {
            v[k].as_u64()
                .ok_or_else(|| Oops::Wrong(format!("no `{k}` in {v}")))
        };
        match answer {
            Ok(v) => Ok(Cas::Applied {
                position: applied(&v)?,
                version: num(&v, "version")?,
            }),
            Err(e) => Ok(Cas::Conflict {
                expected: num(&e, "expected")?,
                actual: num(&e, "actual")?,
            }),
        }
    }

    /// One note's fields and version.
    async fn get(&mut self, key: &str) -> Result<(Value, u64), Oops> {
        let record = match self.via {
            Via::Stdio | Via::McpHttp => {
                let name = self.names.get_note.clone();
                data(&self.tool(&name, json!({"key": key})).await?)
            }
            Via::Rest => data(
                &self
                    .rest(
                        Method::GET,
                        &format!("/types/note/records/{key}"),
                        &[],
                        None,
                    )
                    .await?,
            ),
            Via::Client => {
                let get = Get {
                    key: key.into(),
                    with_meta: false,
                    at: ReadAt::default(),
                };
                let r = self
                    .remote
                    .catalog(WriteContext::default())
                    .get(&loader(), &"note".into(), get)
                    .await
                    .map_err(remote_oops)?;
                serde_json::to_value(r.data).unwrap()
            }
        };
        let version = record["version"]
            .as_u64()
            .ok_or_else(|| Oops::Wrong(format!("a record without a version: {record}")))?;
        Ok((record["fields"].clone(), version))
    }

    /// One page of notes: their keys and the next page's cursor.
    async fn find(
        &mut self,
        limit: u32,
        cursor: Option<String>,
    ) -> Result<(Vec<String>, Option<String>), Oops> {
        let page = match self.via {
            Via::Stdio | Via::McpHttp => {
                let mut args = json!({"limit": limit});
                if let Some(c) = &cursor {
                    args["cursor"] = json!(c);
                }
                let name = self.names.find_note.clone();
                self.tool(&name, args).await?
            }
            Via::Rest => {
                let q = match &cursor {
                    Some(c) => format!("?limit={limit}&cursor={}", encode(c)),
                    None => format!("?limit={limit}"),
                };
                self.rest(Method::GET, &format!("/types/note/records{q}"), &[], None)
                    .await?
            }
            Via::Client => {
                let find = Find {
                    limit: Some(limit),
                    cursor: cursor.map(|c| serde_json::from_value(json!(c)).unwrap()),
                    ..Find::default()
                };
                let r = self
                    .remote
                    .catalog(WriteContext::default())
                    .find(&loader(), &"note".into(), find)
                    .await
                    .map_err(remote_oops)?;
                match r.data {
                    Found::Records { rows, next_cursor } => {
                        json!({"rows": rows, "next_cursor": next_cursor})
                    }
                    Found::Groups { .. } => return Err("a find answered groups".into()),
                }
            }
        };
        let page = data(&page);
        let keys = page["rows"]
            .as_array()
            .ok_or_else(|| Oops::Wrong(format!("a page without rows: {page}")))?
            .iter()
            .filter_map(|r| r["key"].as_str().map(str::to_owned))
            .collect();
        Ok((keys, page["next_cursor"].as_str().map(str::to_owned)))
    }

    /// The pattern query for the notes titled `title`: how many rows.
    async fn query(&mut self, title: &str) -> Result<usize, Oops> {
        let pattern = json!({"start": {"type": "note", "where": format!("title = '{title}'")}});
        let answer = match self.via {
            Via::Stdio | Via::McpHttp => self.tool(generic("query"), pattern).await?,
            Via::Rest => {
                self.rest(Method::POST, "/query", &[], Some(pattern))
                    .await?
            }
            Via::Client => {
                let pattern = serde_json::from_value(pattern).map_err(|e| e.to_string())?;
                let r = self
                    .remote
                    .catalog(WriteContext::default())
                    .query(&loader(), pattern)
                    .await
                    .map_err(remote_oops)?;
                json!({"data": r.data})
            }
        };
        data(&answer)["rows"]
            .as_array()
            .map(Vec::len)
            .ok_or_else(|| Oops::Wrong(format!("a query without rows: {answer}")))
    }

    async fn link(&mut self, key: &str, to: &str) -> Result<u64, Oops> {
        match self.via {
            Via::Stdio | Via::McpHttp => {
                let name = self.names.link_author.clone();
                applied(&self.tool(&name, json!({"key": key, "to": to})).await?)
            }
            Via::Rest => {
                let path = format!("/types/note/records/{key}/links/author/{to}");
                applied(&self.rest(Method::PUT, &path, &[], Some(json!({}))).await?)
            }
            Via::Client => outcome(
                self.remote
                    .catalog(WriteContext::default())
                    .link(
                        &loader(),
                        &"note".into(),
                        &key.into(),
                        &"author".into(),
                        &to.into(),
                        BTreeMap::new(),
                        WriteOpts::default(),
                    )
                    .await,
            ),
        }
    }

    async fn retire(&mut self, key: &str) -> Result<u64, Oops> {
        match self.via {
            Via::Stdio | Via::McpHttp => {
                let name = self.names.retire_note.clone();
                applied(&self.tool(&name, json!({"key": key})).await?)
            }
            Via::Rest => {
                let path = format!("/types/note/records/{key}");
                applied(&self.rest(Method::DELETE, &path, &[], None).await?)
            }
            Via::Client => outcome(
                self.remote
                    .catalog(WriteContext::default())
                    .retire(&loader(), &"note".into(), &key.into(), WriteOpts::default())
                    .await,
            ),
        }
    }
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A write the driver saw applied: where, which record, and whether it
/// was a repeat answered from its idempotency key.
#[derive(Clone, Debug)]
struct Wrote {
    position: u64,
    ty: String,
    key: String,
}

/// The answer to a write against a version.
enum Cas {
    Applied { position: u64, version: u64 },
    Conflict { expected: u64, actual: u64 },
}

/// What every client of a phase shares.
struct Shared {
    timings: Timings,
    wrote: Mutex<Vec<Wrote>>,
    faults: Mutex<Vec<String>>,
    /// An answer did not come in time: the clients stop, since every
    /// later call would wait as long.
    stalled: AtomicBool,
    /// The record every client of the phase writes against its version.
    contended: String,
    /// Writes against its version applied, and answered `conflict`.
    cas_applied: AtomicU64,
    cas_conflicts: AtomicU64,
}

impl Shared {
    fn new(contended: String) -> Shared {
        Shared {
            timings: Timings::default(),
            wrote: Mutex::new(Vec::new()),
            faults: Mutex::new(Vec::new()),
            stalled: AtomicBool::new(false),
            contended,
            cas_applied: AtomicU64::new(0),
            cas_conflicts: AtomicU64::new(0),
        }
    }

    fn fault(&self, what: String) {
        eprintln!("load: {what}");
        self.faults.lock().unwrap().push(what);
    }

    fn failed(&self, op: &str, o: Oops) {
        if let Oops::Stall(_) = o {
            self.stalled.store(true, Ordering::SeqCst);
        }
        self.fault(format!("{op}: {}", String::from(o)));
    }

    fn wrote(&self, position: u64, ty: &str, key: &str) {
        self.wrote.lock().unwrap().push(Wrote {
            position,
            ty: ty.to_owned(),
            key: key.to_owned(),
        });
    }
}

async fn timed<T>(
    shared: &Shared,
    op: &'static str,
    f: impl std::future::Future<Output = Result<T, Oops>>,
) -> Option<T> {
    let t = Instant::now();
    match f.await {
        Ok(v) => {
            shared.timings.add(op, t.elapsed());
            Some(v)
        }
        Err(e) => {
            shared.failed(op, e);
            None
        }
    }
}

fn body_text(i: usize) -> String {
    format!("{i:0>200}")
}

/// One client's rounds. A round writes a note (with an idempotency key,
/// and every fifth round sends that write again, which must answer the
/// same position) and reads it back. Every so often it also links the
/// note to a new person, writes only its secret field, writes a record
/// of the hidden type, retires the note, reads a page, asks the pattern
/// query for it, or writes the phase's contended record against the
/// version it just read, where another client may have come first and
/// the answer is `conflict`. Each of these comes first within the first
/// six rounds, so the short scale runs every one.
async fn rounds(mut w: Worker, tag: String, n: usize, shared: Arc<Shared>, resubscribe: usize) {
    let every = if resubscribe > 0 {
        (n / (resubscribe + 1)).max(1)
    } else {
        usize::MAX
    };
    for i in 0..n {
        if shared.stalled.load(Ordering::SeqCst) {
            return;
        }
        let key = format!("{tag}-{i}");
        let idem = format!("i-{key}");
        let fields = json!({"title": key, "body": body_text(i), "n": i.to_string()});
        let write = w.upsert("note", &key, fields.clone(), Some(&idem), None);
        let Some(p) = timed(&shared, "upsert", write).await else {
            continue;
        };
        shared.wrote(p, "note", &key);
        if i % 5 == 0
            && let Some(again) = timed(
                &shared,
                "repeat",
                w.upsert("note", &key, fields, Some(&idem), None),
            )
            .await
            && again != p
        {
            shared.fault(format!(
                "a repeat of {key} under its idempotency key answered {again}, not {p}"
            ));
        }
        if let Some((f, _)) = timed(&shared, "get", w.get(&key)).await
            && f["n"] != json!(i.to_string())
        {
            shared.fault(format!("{key} read back as {f}"));
        }
        if i % 10 == 0 {
            let person = format!("p{key}");
            let write = w.upsert("person", &person, json!({"name": person}), None, None);
            if let Some(p) = timed(&shared, "upsert", write).await {
                shared.wrote(p, "person", &person);
                if let Some(p) = timed(&shared, "link", w.link(&key, &person)).await {
                    shared.wrote(p, "", &key);
                }
            }
        }
        if i % 20 == 3 {
            let write = w.upsert("note", &key, json!({"secret": "s"}), None, None);
            if let Some(p) = timed(&shared, "upsert", write).await {
                shared.wrote(p, "note", &key);
            }
        }
        if i % 25 == 2 {
            let h = format!("h{key}");
            let write = w.upsert("hidden", &h, json!({"x": "x"}), None, None);
            if let Some(p) = timed(&shared, "upsert", write).await {
                shared.wrote(p, "hidden", &h);
            }
        }
        if i % 50 == 4
            && let Some(p) = timed(&shared, "retire", w.retire(&key)).await
        {
            shared.wrote(p, "note", &key);
        }
        if i % 50 == 1 {
            timed(&shared, "find", w.find(50, None)).await;
        }
        if i % 10 == 5
            && let Some(rows) = timed(&shared, "query", w.query(&key)).await
            && rows != 1
        {
            shared.fault(format!("the pattern query for {key} answered {rows} rows"));
        }
        if i % 5 == 3 {
            let c = shared.contended.clone();
            if let Some((_, version)) = timed(&shared, "get", w.get(&c)).await {
                let t = Instant::now();
                let fields = json!({"n": format!("{tag}-{i}")});
                // Each answer names its versions: an applied write is the
                // one after the version read, a conflict found another.
                match w.cas(&c, fields, version).await {
                    Ok(Cas::Applied {
                        position,
                        version: v,
                    }) => {
                        shared.timings.add("if-match", t.elapsed());
                        shared.cas_applied.fetch_add(1, Ordering::SeqCst);
                        shared.wrote(position, "note", &c);
                        if v != version + 1 {
                            shared.fault(format!(
                                "a write of {c} against version {version} was applied as version {v}"
                            ));
                        }
                    }
                    Ok(Cas::Conflict { expected, actual }) => {
                        shared.timings.add("if-match", t.elapsed());
                        shared.cas_conflicts.fetch_add(1, Ordering::SeqCst);
                        if expected != version || actual == expected {
                            shared.fault(format!(
                                "a write of {c} against version {version} answered a conflict of expected {expected}, actual {actual}"
                            ));
                        }
                    }
                    Err(e) => shared.failed("if-match", e),
                }
            }
        }
        if i > 0 && i % every == 0 && w.via == Via::Stdio {
            // Subscribe again from after the last change read: what was on
            // its way may be told again, nothing may be lost.
            let from = w.stdio.told.last().map_or(0, |p| p + 1);
            timed(
                &shared,
                "resubscribe",
                w.stdio.call(generic("subscribe"), json!({"from": from})),
            )
            .await;
        }
    }
    // Walk every note, a page at a time: each key once.
    let mut cursor = None;
    let mut keys = BTreeSet::new();
    loop {
        let Some((page, next)) = timed(&shared, "page", w.find(500, cursor.clone())).await else {
            break;
        };
        for k in page {
            if !keys.insert(k.clone()) {
                shared.fault(format!("a walk of the pages answered {k} twice"));
            }
        }
        match next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
}

// ------------------------------------------------------------------ feed

/// What the checks read of one event of the log.
struct Seen {
    position: u64,
    kind: &'static str,
    ty: String,
    key: String,
    /// The fields its patch names, for a write.
    fields: Vec<String>,
}

impl Seen {
    fn of(e: &lakelands_api::event::Event) -> Seen {
        let (ty, key, fields) = match &e.body {
            EventBody::RecordUpserted { ty, key, patch, .. }
            | EventBody::RecordIngested { ty, key, patch, .. } => (
                ty.as_str().to_owned(),
                key.as_str().to_owned(),
                patch.keys().map(|f| f.as_str().to_owned()).collect(),
            ),
            EventBody::RecordRetired { ty, key, .. } => {
                (ty.as_str().to_owned(), key.as_str().to_owned(), Vec::new())
            }
            _ => (String::new(), String::new(), Vec::new()),
        };
        Seen {
            position: e.position.0,
            kind: e.body.kind(),
            ty,
            key,
            fields,
        }
    }

    fn is_write(&self) -> bool {
        matches!(self.kind, "record_upserted" | "record_retired")
    }
}

/// Read the log from `from` through `to` through the client's feed, a
/// page at a time, handing each event to `each` as it comes: nothing of
/// the log is held but what `each` keeps.
async fn each_event(
    remote: &RemoteLake,
    from: u64,
    to: u64,
    mut each: impl FnMut(Seen),
) -> Result<(), String> {
    let feed = remote.feed();
    let mut at = from;
    while at <= to {
        let b = feed
            .poll(Position(at), &FeedFilter::default(), 1000)
            .await
            .map_err(|e| format!("{e:?}"))?;
        if b.next.0 == at {
            return Err(format!("the feed does not move past {at}"));
        }
        for e in b.events.iter().filter(|e| e.position.0 <= to) {
            each(Seen::of(e));
        }
        at = b.next.0;
    }
    Ok(())
}

/// What a principal who reads everything, subscribed to every type it
/// may read, is told of: the changes of records.
fn told_to_loader(e: &Seen) -> bool {
    CHANGE_KINDS.contains(&e.kind)
}

/// What the watcher is told of: the changes of records it may read,
/// when a field it may read changed (it never reads `secret`, nor any
/// record of `hidden`).
fn told_to_watcher(e: &Seen) -> bool {
    match e.kind {
        "record_upserted" | "record_ingested" => {
            e.ty != "hidden" && (e.fields.is_empty() || e.fields.iter().any(|f| f != "secret"))
        }
        "record_retired" => e.ty != "hidden",
        _ => false,
    }
}

// ------------------------------------------------------------------- run

/// Run the plan; the report says what was found.
pub async fn run(plan: Plan) -> Report {
    let mut report = Report {
        machine: machine(),
        ..Report::default()
    };
    std::fs::create_dir_all(&plan.scratch).ok();
    let dir = plan.scratch.join(format!(
        "lakelands-load-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let mut scratch = Scratch {
        dir: dir.clone(),
        clean: false,
    };
    let result = drive(&plan, &dir, &mut report).await;
    if let Err(e) = result {
        report.defect("run", e);
    }
    let after = machine();
    report.machine["load_after"] = after["load"].clone();
    scratch.clean = report.defects().is_empty();
    if !scratch.clean && dir.exists() {
        report.scratch = Some(dir);
    }
    report
}

async fn drive(plan: &Plan, dir: &Path, report: &mut Report) -> Result<(), String> {
    let s = plan.scale;
    let root = dir.join("root");
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let t0 = Instant::now();
    if let Some(schema) = prepare(plan, &root)? {
        report.line(
            "setup",
            format!("postgres schema {schema} is left behind"),
            json!({"schema": schema}),
        );
    }
    let (server, stdin, stdout) = Server::start(plan, &root).await?;
    let addr = server.addr;
    let memory = Memory::start(server.pid);
    let stdio = Stdio::start(stdin, stdout);
    stdio
        .request(
            "initialize",
            json!({"protocolVersion": "2025-06-18"}),
            false,
        )
        .await?;

    let keys: BTreeMap<String, ApiKey> = [("loader", "k-loader"), ("watcher", "k-watcher")]
        .into_iter()
        .map(|(p, k)| (p.to_owned(), ApiKey::from(k)))
        .collect();
    let remote = Arc::new(
        RemoteLake::connect(RemoteConfig {
            url: format!("http://{addr}/"),
            lake: LakeId::from(LAKE),
            keys,
            feed_key: Some("loader".into()),
            ca_file: None,
            timeout_secs: 120,
            max_answer_bytes: lakelands_client::DEFAULT_MAX_ANSWER_BYTES,
            limits: Limits::default(),
        })
        .map_err(|e| e.0)?,
    );
    declare(&remote).await?;
    let description = remote
        .catalog(WriteContext::default())
        .describe(&loader(), ReadAt::default())
        .await
        .map_err(|e| format!("describe: {e:?}"))?;
    let specs = lakelands_api::tools::tools(&description.data)
        .map_err(|p| format!("the generator refused the lake: {p:?}"))?;
    let names = Arc::new(Names::of(&specs)?);

    // The subscriptions: the loader on stdio, the watcher on HTTP read as
    // it comes, and a loader session on HTTP read by nobody until the
    // ingest is over.
    let subscribe = generic("subscribe");
    stdio.call(subscribe, json!({})).await?;
    let mut c = Conn::open(addr).await?;
    let watcher = Arc::new(HttpMcp::open(&mut c, "k-watcher").await?);
    let from_w = watcher.call(&mut c, subscribe, json!({})).await?["from"]
        .as_u64()
        .unwrap_or(0);
    let lagging = Arc::new(HttpMcp::open(&mut c, "k-loader").await?);
    let from_l = lagging.call(&mut c, subscribe, json!({})).await?["from"]
        .as_u64()
        .unwrap_or(0);
    let from_s = stdio
        .told
        .froms
        .lock()
        .unwrap()
        .first()
        .copied()
        .unwrap_or(0);
    let told_w = Arc::new(Told::default());
    told_w.subscribed(from_w);
    let stream_faults = Arc::new(Mutex::new(Vec::new()));
    let watching = watcher.listen(addr, told_w.clone(), usize::MAX, 0, stream_faults.clone());
    let (m, j) = memory.over(t0, Instant::now());
    let mut j = j;
    j["addr"] = json!(addr.to_string());
    j["pid"] = json!(server.pid);
    report.line(
        "setup",
        format!(
            "served at http://{addr}/ by process {} in {:.1} s; {m}",
            server.pid,
            t0.elapsed().as_secs_f64()
        ),
        j,
    );

    // Writes and reads, door by door, then on all at once.
    let doors = [Via::Stdio, Via::McpHttp, Via::Rest, Via::Client];
    for round in 0..=doors.len() {
        let vias: Vec<Via> = if round < doors.len() {
            vec![doors[round]]
        } else {
            doors.to_vec()
        };
        let phase = if vias.len() == 1 {
            format!("doors/{}", vias[0].name())
        } else {
            "doors/all".to_owned()
        };
        // Every client of the phase writes this record against the version
        // it read.
        let contended = format!("contended-{round}");
        remote
            .catalog(WriteContext::default())
            .upsert(
                &loader(),
                &"note".into(),
                &contended.as_str().into(),
                serde_json::from_value(json!({"title": contended})).unwrap(),
                WriteOpts::default(),
            )
            .await
            .map_err(|e| format!("{contended}: {e:?}"))?;
        let shared = Arc::new(Shared::new(contended));
        let start = Instant::now();
        let head0 = remote.head().await.map_err(|e| format!("{e:?}"))?.0;
        let mut tasks = Vec::new();
        for via in &vias {
            for c in 0..s.clients {
                let w =
                    Worker::new(*via, addr, stdio.clone(), names.clone(), remote.clone()).await?;
                let tag = format!("{}{round}c{c}", via.name());
                let resub = if c == 0 && *via == Via::Stdio && round < doors.len() {
                    s.resubscribes
                } else {
                    0
                };
                tasks.push(tokio::spawn(rounds(
                    w,
                    tag,
                    s.rounds,
                    shared.clone(),
                    resub,
                )));
            }
        }
        for t in tasks {
            t.await.map_err(|e| format!("a client panicked: {e}"))?;
        }
        let wall = start.elapsed();
        let head1 = remote.head().await.map_err(|e| format!("{e:?}"))?.0;
        // Every applied write is in the log at the position it answered,
        // and the log holds no other write of these clients.
        let mut at: HashMap<u64, Seen> = HashMap::new();
        each_event(&remote, head0 + 1, head1, |e| {
            at.insert(e.position, e);
        })
        .await?;
        let wrote = shared.wrote.lock().unwrap().clone();
        for w in &wrote {
            match at.get(&w.position) {
                None => report.defect(
                    &phase,
                    format!(
                        "{} answered {} but the log has no event there",
                        w.key, w.position
                    ),
                ),
                Some(e) => {
                    if !w.ty.is_empty() && (e.ty != w.ty || e.key != w.key) {
                        report.defect(
                            &phase,
                            format!(
                                "{} {} answered {} where the log holds {} {} {}",
                                w.ty, w.key, w.position, e.kind, e.ty, e.key
                            ),
                        );
                    }
                }
            }
        }
        let writes = at.values().filter(|e| e.is_write()).count();
        let distinct: BTreeSet<u64> = wrote.iter().map(|w| w.position).collect();
        if writes != distinct.len() {
            report.defect(
                &phase,
                format!(
                    "the log holds {writes} writes, the clients saw {} applied",
                    distinct.len()
                ),
            );
        }
        // The contended record's version counts every write made
        // against it that was applied, and nothing else.
        let applied = shared.cas_applied.load(Ordering::SeqCst);
        let conflicts = shared.cas_conflicts.load(Ordering::SeqCst);
        match remote
            .catalog(WriteContext::default())
            .get(
                &loader(),
                &"note".into(),
                Get {
                    key: shared.contended.as_str().into(),
                    with_meta: false,
                    at: ReadAt::default(),
                },
            )
            .await
        {
            Ok(r) if r.data.version.0 == 1 + applied => {}
            Ok(r) => report.defect(
                &phase,
                format!(
                    "{} is at version {} after {applied} writes against its version were applied",
                    shared.contended, r.data.version.0
                ),
            ),
            Err(e) => report.defect(&phase, format!("{}: {e:?}", shared.contended)),
        }
        for f in shared.faults.lock().unwrap().iter().take(10) {
            report.defect(&phase, f.clone());
        }
        let n = shared.faults.lock().unwrap().len();
        if n > 10 {
            report.defect(&phase, format!("and {} more", n - 10));
        }
        let (t, mut j) = shared.timings.summary(wall);
        let (m, jm) = memory.over(start, Instant::now());
        j["memory"] = jm;
        j["events"] = json!(at.len());
        j["if_match"] = json!({"applied": applied, "conflicts": conflicts});
        report.line(
            &phase,
            format!("{t}; if-match {applied} applied, {conflicts} conflicts; {m}"),
            j,
        );
    }

    // The ingest, through the client: a large source, generated as sent.
    let phase = "ingest/client";
    let start = Instant::now();
    let (windows, records, dups) =
        ingest_client(&remote, "src", s.ingest, None, report, phase).await;
    let wall = start.elapsed();
    if records != s.ingest as u64 || dups != 0 {
        report.defect(
            phase,
            format!(
                "{} records sent, {records} committed and {dups} duplicates",
                s.ingest
            ),
        );
    }
    let (m, jm) = memory.over(start, Instant::now());
    report.line(
        phase,
        format!(
            "{records} records in {windows} windows in {:.1} s, {:.0} records/s; {m}",
            wall.as_secs_f64(),
            records as f64 / wall.as_secs_f64()
        ),
        json!({"records": records, "windows": windows, "seconds": wall.as_secs_f64(), "memory": jm}),
    );

    // The same source again, as raw NDJSON over REST: all duplicates.
    let phase = "ingest/rest-again";
    let start = Instant::now();
    let head1 = remote.head().await.map_err(|e| format!("{e:?}"))?.0;
    match ingest_rest(addr, s.ingest, |i| source_line("src", i), 64 << 10).await {
        Ok(lines) => {
            let (r, d, fail) = sum_lines(&lines);
            if r != 0 || d != s.ingest as u64 || fail.is_some() {
                report.defect(
                    phase,
                    format!("sent again: {r} committed, {d} duplicates, failure {fail:?}"),
                );
            }
            let head2 = remote.head().await.map_err(|e| format!("{e:?}"))?.0;
            if head2 != head1 {
                report.defect(
                    phase,
                    format!("sending the source again moved the head from {head1} to {head2}"),
                );
            }
            let wall = start.elapsed();
            let (m, jm) = memory.over(start, Instant::now());
            report.line(
                phase,
                format!("{d} duplicates in {} lines in {:.1} s, {:.0} records/s; {m}", lines.len(), wall.as_secs_f64(), d as f64 / wall.as_secs_f64()),
                json!({"duplicates": d, "lines": lines.len(), "seconds": wall.as_secs_f64(), "memory": jm}),
            );
        }
        Err(e) => report.defect(phase, e),
    }
    // A source that pauses past two keep-alives: waiting on the source
    // never counts against the client's silence bound.
    if s.silence {
        let phase = "ingest/pause";
        let start = Instant::now();
        let (w, r, d) = ingest_client(
            &remote,
            "paused",
            2000,
            Some((1000, SOURCE_PAUSE)),
            report,
            phase,
        )
        .await;
        if r + d != 2000 {
            report.defect(
                phase,
                format!(
                    "a source paused for {} s: {r} committed, {d} duplicates of 2000",
                    SOURCE_PAUSE.as_secs()
                ),
            );
        }
        report.line(
            phase,
            format!(
                "{r} records in {w} windows around a {} s pause, in {:.1} s",
                SOURCE_PAUSE.as_secs(),
                start.elapsed().as_secs_f64()
            ),
            json!({"records": r, "windows": w, "pause_seconds": SOURCE_PAUSE.as_secs()}),
        );
    }

    // The subscriptions: what each was told, against the log.
    let end = remote.head().await.map_err(|e| format!("{e:?}"))?.0;
    let phase = "notify";
    let start = Instant::now();
    // Positions come in order, so each expected set is a sorted list.
    let (mut for_stdio, mut for_watcher, mut expected_l) = (Vec::new(), Vec::new(), Vec::new());
    each_event(&remote, from_s.min(from_w).min(from_l), end, |e| {
        if told_to_loader(&e) {
            if e.position >= from_s {
                for_stdio.push(e.position);
            }
            if e.position >= from_l {
                expected_l.push(e.position);
            }
        }
        if e.position >= from_w && told_to_watcher(&e) {
            for_watcher.push(e.position);
        }
    })
    .await?;
    let told_l = Arc::new(Told::default());
    told_l.subscribed(from_l);
    let every = (expected_l.len() / (s.reconnects + 1)).max(1);
    let lag_start = Instant::now();
    let draining = lagging.listen(
        addr,
        told_l.clone(),
        every,
        s.reconnects,
        stream_faults.clone(),
    );
    for (name, told, expected, from) in [
        ("stdio", &stdio.told, &for_stdio, from_s),
        ("mcp-http watcher", &told_w, &for_watcher, from_w),
        ("mcp-http lagging", &told_l, &expected_l, from_l),
    ] {
        let t = Instant::now();
        if let Err(e) = told.until_told(expected).await {
            report.defect(phase, format!("{name}: {e}"));
        }
        let (j, defects) = told.account(expected, (from, end));
        for d in defects {
            report.defect(phase, format!("{name}: {d}"));
        }
        report.line(
            &format!("notify/{}", name.replace(' ', "-")),
            format!(
                "{} expected, {} told, {} missing, {} repeated ({} asked again), {} not allowed; waited {:.1} s",
                j["expected"], j["told"], j["missing"], j["repeated"], j["asked_again"], j["not_allowed"],
                t.elapsed().as_secs_f64()
            ),
            j,
        );
    }
    let drained = lag_start.elapsed();
    watching.abort();
    draining.abort();
    for f in stream_faults.lock().unwrap().iter() {
        report.defect(phase, f.clone());
    }
    let (m, jm) = memory.over(start, Instant::now());
    report.line(
        "notify/drain",
        format!("the lagging session drained {} changes with {} reconnects in {:.1} s ({:.0}/s); {m}", expected_l.len(), s.reconnects, drained.as_secs_f64(), expected_l.len() as f64 / drained.as_secs_f64()),
        json!({"changes": expected_l.len(), "reconnects": s.reconnects, "seconds": drained.as_secs_f64(), "memory": jm}),
    );

    // The feed as server-sent events, from the start, while writes go on.
    feed_sse(addr, &remote, s, &memory, report).await?;

    // The feed through the client.
    let phase = "feed/client";
    let start = Instant::now();
    let head = remote.head().await.map_err(|e| format!("{e:?}"))?.0;
    let (mut events, mut gaps, mut last) = (0u64, 0u64, None::<u64>);
    let read = each_event(&remote, 0, head, |e| {
        if last.map_or(e.position != 0, |l| e.position != l + 1) {
            gaps += 1;
        }
        last = Some(e.position);
        events += 1;
    })
    .await;
    match read {
        Ok(()) => {
            if gaps > 0 || events != head + 1 {
                report.defect(
                    phase,
                    format!("{events} events through {head}, {gaps} gaps"),
                );
            }
            let wall = start.elapsed();
            report.line(
                phase,
                format!(
                    "{events} events in {:.1} s, {:.0} events/s",
                    wall.as_secs_f64(),
                    events as f64 / wall.as_secs_f64()
                ),
                json!({"events": events, "seconds": wall.as_secs_f64()}),
            );
        }
        Err(e) => report.defect(phase, e),
    }

    // The export over the wire, verified, and later compared with the
    // host's own.
    let phase = "export";
    let start = Instant::now();
    let over_wire = dir.join("export-wire");
    let manifest = lakelands_core::export::write(remote.as_ref(), &over_wire).await;
    let wall = start.elapsed();
    let (m, jm) = memory.over(start, Instant::now());
    let through = match manifest {
        Ok(m) => Some(m.through.0),
        Err(e) => {
            report.defect(phase, format!("the export over the wire failed: {e:?}"));
            None
        }
    };
    if through.is_some() {
        match lakelands_core::export::verify(&over_wire) {
            Ok(v) if Some(v.through.0) == through => {}
            Ok(v) => report.defect(
                phase,
                format!(
                    "the export verifies through {}, its manifest says {through:?}",
                    v.through.0
                ),
            ),
            Err(e) => report.defect(phase, format!("the export does not verify: {e:?}")),
        }
    }
    let bytes = dir_bytes(&over_wire.join("log"));
    report.line(phase, format!("{} events, {} in {:.1} s; {m}", through.map_or(0, |t| t + 1), mib(bytes / 1024), wall.as_secs_f64()), json!({"events": through.map(|t| t + 1), "bytes": bytes, "seconds": wall.as_secs_f64(), "memory": jm}));

    // The bounds on what one request carries, last: a line of 4 MiB in
    // the log would be carried by every phase after it.
    caps(addr, report).await;

    // The server's own peak, then its end: it ends with its stdio client.
    let hwm = memory.high_water();
    drop(memory);
    let Server {
        mut child, stderr, ..
    } = server;
    stdio.close().await;
    let ended = tokio::time::timeout(WAIT, child.wait()).await;
    match ended {
        Ok(Ok(st)) if st.success() => {}
        Ok(Ok(st)) => report.defect(
            "server",
            format!(
                "the server ended with {st}: {}",
                stderr.lock().unwrap().join(" | ")
            ),
        ),
        _ => report.defect("server", "the server did not end with its stdio client"),
    }
    report.line(
        "server",
        format!("peak rss {}", hwm.map_or("none".into(), mib)),
        json!({"hwm_kib": hwm}),
    );

    if through.is_some() && !matches!(plan.engine, Engine::Memory) {
        let local = dir.join("export-local");
        match cli(
            plan,
            &root,
            &["export", "--to", &local.display().to_string()],
        ) {
            Ok(_) => {
                if let Some(d) = compare_dirs(&over_wire.join("log"), &local.join("log")) {
                    report.defect(
                        "export",
                        format!("the export over the wire is not the host's: {d}"),
                    );
                }
            }
            Err(e) => report.defect("export", e),
        }
    }
    Ok(())
}

async fn declare(remote: &RemoteLake) -> Result<(), String> {
    let c = remote.catalog(WriteContext::default());
    for d in [
        json!({"declare": "type", "name": "person", "fields": [{"name": "name", "type": {"kind": "text"}}]}),
        json!({"declare": "type", "name": "hidden", "fields": [{"name": "x", "type": {"kind": "text"}}]}),
        json!({"declare": "type", "name": "note", "fields": [
            {"name": "title", "type": {"kind": "text"}, "authority": "shared"},
            {"name": "body", "type": {"kind": "text"}, "authority": "shared"},
            {"name": "n", "type": {"kind": "text"}, "authority": "shared"},
            {"name": "secret", "type": {"kind": "text"}}],
            "relations": [{"name": "author", "target": "person", "cardinality": "many_to_one", "fields": []}]}),
    ] {
        let d: Declaration = serde_json::from_value(d).map_err(|e| e.to_string())?;
        c.declare(&loader(), d)
            .await
            .map_err(|e| format!("declare: {e:?}"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------- ingest

fn source_record(prefix: &str, i: usize) -> SourceRecord {
    serde_json::from_value(json!({
        "type": "note",
        "key": format!("{prefix}-{i}"),
        "fields": {"title": format!("{prefix} {i}"), "body": body_text(i), "n": i.to_string()},
    }))
    .unwrap()
}

fn source_line(prefix: &str, i: usize) -> String {
    let mut l = serde_json::to_string(&source_record(prefix, i)).unwrap();
    l.push('\n');
    l
}

/// A source generated as it is read, which may pause once: its clock
/// starts when the source reaches the pause.
struct Source {
    prefix: String,
    next: usize,
    end: usize,
    pause: Option<(usize, Duration)>,
    sleep: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl Stream for Source {
    type Item = SourceRecord;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<SourceRecord>> {
        let this = self.get_mut();
        if let Some((at, d)) = this.pause
            && this.next == at
        {
            let sleep = this
                .sleep
                .get_or_insert_with(|| Box::pin(tokio::time::sleep(d)));
            if sleep.as_mut().poll(cx).is_pending() {
                return Poll::Pending;
            }
            this.pause = None;
        }
        if this.next >= this.end {
            return Poll::Ready(None);
        }
        let r = source_record(&this.prefix, this.next);
        this.next += 1;
        Poll::Ready(Some(r))
    }
}

/// Ingest through the client; windows, records and duplicates.
async fn ingest_client(
    remote: &RemoteLake,
    prefix: &str,
    n: usize,
    pause: Option<(usize, Duration)>,
    report: &mut Report,
    phase: &str,
) -> (usize, u64, u64) {
    let source = Source {
        prefix: prefix.to_owned(),
        next: 0,
        end: n,
        pause,
        sleep: None,
    };
    let commits = remote
        .catalog(WriteContext::default())
        .ingest(&loader(), &prefix.into(), Box::pin(source))
        .await;
    let mut commits = match commits {
        Ok(c) => c,
        Err(e) => {
            report.defect(phase, format!("the ingest was refused: {e:?}"));
            return (0, 0, 0);
        }
    };
    let (mut windows, mut records, mut dups, mut last) = (0, 0u64, 0u64, 0u64);
    while let Some(c) = std::future::poll_fn(|cx| commits.as_mut().poll_next(cx)).await {
        match c {
            Ok(c) => {
                if c.position.0 <= last && windows > 0 {
                    report.defect(
                        phase,
                        format!("a window at {} after one at {last}", c.position.0),
                    );
                }
                last = c.position.0;
                windows += 1;
                records += u64::from(c.records);
                dups += u64::from(c.duplicates);
            }
            Err(e) => {
                report.defect(
                    phase,
                    format!("the ingest failed after {records} records: {e:?}"),
                );
                break;
            }
        }
    }
    (windows, records, dups)
}

/// POST an NDJSON body of `n` lines, made by `line`, sent in chunks of
/// `chunk` bytes as the server takes them; the answer's lines.
async fn ingest_rest(
    addr: SocketAddr,
    n: usize,
    line: impl Fn(usize) -> String + Send + 'static,
    chunk: usize,
) -> Result<Vec<Value>, String> {
    let mut conn = Conn::open(addr).await?;
    let (tx, rx) = mpsc::channel::<Bytes>(4);
    tokio::spawn(async move {
        let mut buf = Vec::with_capacity(chunk);
        for i in 0..n {
            buf.extend_from_slice(line(i).as_bytes());
            while buf.len() >= chunk {
                let rest = buf.split_off(chunk);
                if tx
                    .send(Bytes::from(std::mem::replace(&mut buf, rest)))
                    .await
                    .is_err()
                {
                    return;
                }
            }
        }
        if !buf.is_empty() {
            let _ = tx.send(Bytes::from(buf)).await;
        }
    });
    let req = conn
        .request(
            Method::POST,
            &format!("/lakes/{LAKE}/ingest/src"),
            "k-loader",
        )
        .header("content-type", "application/x-ndjson")
        .body(Streamed(rx).boxed())
        .unwrap();
    let r = conn.send(req).await?;
    if r.status() != 200 {
        let status = r.status();
        let body = whole(r.into_body()).await.unwrap_or_default();
        return Err(format!(
            "the ingest answered {status}: {}",
            String::from_utf8_lossy(&body)
        ));
    }
    let mut body = r.into_body();
    let mut buf = Vec::new();
    let mut lines = Vec::new();
    loop {
        let f = tokio::time::timeout(WAIT, body.frame())
            .await
            .map_err(|_| "the ingest's answer went quiet".to_owned())?;
        match f {
            None => break,
            Some(Err(e)) => return Err(format!("the ingest's answer broke: {e}")),
            Some(Ok(f)) => {
                if let Ok(d) = f.into_data() {
                    buf.extend_from_slice(&d);
                    while let Some(i) = buf.iter().position(|b| *b == b'\n') {
                        let l: Vec<u8> = buf.drain(..=i).collect();
                        lines.push(serde_json::from_slice(&l).unwrap_or(Value::Null));
                    }
                }
            }
        }
    }
    Ok(lines)
}

/// Records, duplicates and the failure line of an ingest's answer.
fn sum_lines(lines: &[Value]) -> (u64, u64, Option<Value>) {
    let mut r = 0;
    let mut d = 0;
    let mut fail = None;
    for l in lines {
        if l.get("error").is_some() {
            fail = Some(l.clone());
        } else {
            r += l["records"].as_u64().unwrap_or(0);
            d += l["duplicates"].as_u64().unwrap_or(0);
        }
    }
    (r, d, fail)
}

/// A source line exactly `len` bytes long before its line feed: a small
/// record padded with spaces, so a line at the cap costs the lake one
/// small event, not a 4 MiB value that every later reader carries.
fn line_of(key: &str, len: usize) -> String {
    let record = format!(r#"{{"type":"note","key":"{key}","fields":{{"title":"{key}"}}}}"#);
    format!("{record}{}\n", " ".repeat(len - record.len()))
}

async fn caps(addr: SocketAddr, report: &mut Report) {
    let phase = "caps";
    let start = Instant::now();
    // A line of exactly the cap is taken.
    match ingest_rest(addr, 1, |_| line_of("cap-exact", CAP), 64 << 10).await {
        Ok(lines) => {
            let (r, d, fail) = sum_lines(&lines);
            if r + d != 1 || fail.is_some() {
                report.defect(phase, format!("a line of {CAP} bytes: {lines:?}"));
            }
        }
        Err(e) => report.defect(phase, format!("a line of {CAP} bytes: {e}")),
    }
    // A line one byte over, and one twice the cap, in 64 KiB chunks.
    let mut seen = Vec::new();
    for (key, len) in [("cap-over", CAP + 1), ("cap-twice", 2 * CAP)] {
        match ingest_rest(addr, 1, move |_| line_of(key, len), 64 << 10).await {
            Ok(lines) => {
                let (r, d, fail) = sum_lines(&lines);
                let refused = fail.is_some() && r + d == 0;
                seen.push(json!({"bytes": len, "refused": refused, "answer": lines}));
                if !refused {
                    report.defect(phase, format!("a line of {len} bytes, over the {CAP}-byte cap, was taken when sent in 64 KiB chunks"));
                }
            }
            Err(e) => report.defect(phase, format!("a line of {len} bytes: {e}")),
        }
    }
    // An MCP request body of exactly the cap is read; one byte more is 413.
    let mut mcp_seen = Vec::new();
    if let Ok(mut conn) = Conn::open(addr).await
        && let Ok(s) = HttpMcp::open(&mut conn, "k-loader").await
    {
        for len in [CAP, CAP + 1] {
            let msg = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}})
                .to_string();
            let body = format!("{msg}{}", " ".repeat(len - msg.len()));
            match s.post(&mut conn, Bytes::from(body)).await {
                Ok((status, _)) => {
                    mcp_seen.push(json!({"bytes": len, "status": status}));
                    let want = if len == CAP { 200 } else { 413 };
                    if status != want {
                        report.defect(
                            phase,
                            format!("an MCP body of {len} bytes answered {status}, not {want}"),
                        );
                    }
                }
                Err(e) => report.defect(phase, format!("an MCP body of {len} bytes: {e}")),
            }
            conn = match Conn::open(addr).await {
                Ok(c) => c,
                Err(_) => break,
            };
        }
    }
    report.line(
        phase,
        format!(
            "ingest line {CAP} B taken; +1 B {}; ×2 {}; MCP body {CAP} B → {}, +1 B → {}; {:.1} s",
            if seen.first().is_some_and(|s| s["refused"] == true) { "refused" } else { "taken" },
            if seen.get(1).is_some_and(|s| s["refused"] == true) { "refused" } else { "taken" },
            mcp_seen.first().map_or("?".into(), |s| s["status"].to_string()),
            mcp_seen.get(1).map_or("?".into(), |s| s["status"].to_string()),
            start.elapsed().as_secs_f64(),
        ),
        json!({"ingest_lines": seen.iter().map(|s| json!({"bytes": s["bytes"], "refused": s["refused"]})).collect::<Vec<_>>(), "mcp_bodies": mcp_seen}),
    );
}

// ------------------------------------------------------------ feed (SSE)

/// A small, fixed sequence of numbers, so a run can be repeated.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % n.max(1)
    }
}

async fn feed_sse(
    addr: SocketAddr,
    remote: &Arc<RemoteLake>,
    s: Scale,
    memory: &Memory,
    report: &mut Report,
) -> Result<(), String> {
    let phase = "feed/sse";
    let start = Instant::now();
    let target = remote.head().await.map_err(|e| format!("{e:?}"))?.0;
    // Writes go on while the stream catches up.
    let stop = Arc::new(AtomicBool::new(false));
    let mut writer = Some({
        let stop = stop.clone();
        tokio::spawn(async move {
            let Ok(mut c) = Conn::open(addr).await else {
                return 0;
            };
            let mut n = 0;
            while !stop.load(Ordering::Relaxed) {
                let req = c
                    .request(
                        Method::PUT,
                        &format!("/lakes/{LAKE}/types/person/records/live-{n}"),
                        "k-loader",
                    )
                    .header("content-type", "application/json")
                    .body(full(json!({"name": "live"}).to_string()))
                    .unwrap();
                if c.json(req).await.is_err() {
                    break;
                }
                n += 1;
            }
            n
        })
    });
    let mut rng = Lcg(0x1a4e);
    let mut cuts: BTreeSet<u64> = (0..s.reconnects).map(|_| rng.below(target)).collect();
    let mut last: Option<u64> = None;
    let mut events = 0u64;
    let mut repeats = 0u64;
    let mut gaps = 0u64;
    let mut reconnects = 0;
    let mut nexts: BTreeSet<u64> = BTreeSet::new();
    let mut bad_next = 0;
    let mut live = 0;
    // Once the stream passes the head the phase began at, the writer
    // stops; the stream is then read through the head it left.
    let mut end: Option<u64> = None;
    let mut sse = open_feed(addr, None).await?;
    while end.is_none_or(|e| last.is_none_or(|l| l < e)) {
        let item = tokio::time::timeout(WAIT, sse.next()).await;
        match item {
            Err(_) => return Err("the feed went quiet".into()),
            Ok(Err(e)) => return Err(e),
            Ok(Ok(None)) => return Err("the feed ended".into()),
            Ok(Ok(Some(Sse::Comment(c)))) => {
                if let Some(n) = c.strip_prefix("next ") {
                    let n: u64 = n.trim().parse().unwrap_or(0);
                    if !nexts.insert(n)
                        || Some(n) != last.map(|l| l + 1)
                        || !polls_next(remote, n).await?
                    {
                        bad_next += 1;
                    }
                }
            }
            Ok(Ok(Some(Sse::Event { id, .. }))) => {
                let Some(p) = id else { continue };
                match last {
                    Some(l) if p <= l => repeats += 1,
                    Some(l) if p != l + 1 => gaps += 1,
                    None if p != 0 => gaps += 1,
                    _ => {}
                }
                last = Some(last.map_or(p, |l| l.max(p)));
                events += 1;
                if p >= target
                    && let Some(w) = writer.take()
                {
                    stop.store(true, Ordering::Relaxed);
                    live = w.await.unwrap_or(0);
                    end = Some(remote.head().await.map_err(|e| format!("{e:?}"))?.0);
                }
                if cuts.first().is_some_and(|c| p >= *c) {
                    cuts.pop_first();
                    reconnects += 1;
                    sse = open_feed(addr, last).await?;
                }
            }
        }
    }
    // Caught up: the stream names the next position once.
    let named = tokio::time::timeout(WAIT, async {
        loop {
            match sse.next().await {
                Ok(Some(Sse::Comment(c))) => {
                    if let Some(n) = c.strip_prefix("next ") {
                        return n.trim().parse::<u64>().ok();
                    }
                }
                Ok(Some(Sse::Event { .. })) => return None,
                _ => return None,
            }
        }
    })
    .await
    .ok()
    .flatten();
    let polled = match named {
        Some(n) => polls_next(remote, n).await?,
        None => true,
    };
    match named {
        Some(n) if Some(n) == last.map(|l| l + 1) && nexts.insert(n) && polled => {}
        // Named already, just after the last event: the stream caught up
        // on it before the phase ended.
        None if last.is_some_and(|l| nexts.contains(&(l + 1))) => {}
        other => report.defect(
            phase,
            format!("caught up after {last:?}, the stream named {other:?} as next"),
        ),
    }
    let wall = start.elapsed();
    if repeats > 0 || gaps > 0 {
        report.defect(
            phase,
            format!(
                "{repeats} repeated and {gaps} missing positions across {reconnects} reconnects"
            ),
        );
    }
    if bad_next > 0 {
        report.defect(phase, format!("{bad_next} `: next` comments named a position already named, not the one after the last event, or not the `next` a poll answers there"));
    }
    let (m, jm) = memory.over(start, Instant::now());
    report.line(
        phase,
        format!("{events} events ({live} written live) with {reconnects} reconnects in {:.1} s, {:.0} events/s; {} `: next`; {m}", wall.as_secs_f64(), events as f64 / wall.as_secs_f64(), nexts.len()),
        json!({"events": events, "live_writes": live, "reconnects": reconnects, "seconds": wall.as_secs_f64(), "nexts": nexts.len(), "memory": jm}),
    );

    if s.silence {
        // A caught-up stream left idle keeps itself open, and then tells
        // the next change.
        let phase = "feed/idle";
        let t = Instant::now();
        let mut keep_alives = 0;
        let quiet = tokio::time::timeout(FEED_IDLE, async {
            loop {
                match sse.next().await {
                    Ok(Some(Sse::Comment(c))) if c == "keep-alive" => keep_alives += 1,
                    Ok(Some(Sse::Event { id, .. })) => {
                        return Err::<(), String>(format!("an event {id:?} on an idle lake"));
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => return Err("the idle feed ended".to_owned()),
                    Err(e) => return Err(e),
                }
            }
        })
        .await;
        if let Ok(Err(e)) = quiet {
            report.defect(phase, e);
        }
        if keep_alives < 2 {
            report.defect(
                phase,
                format!("{keep_alives} keep-alives in {} s", FEED_IDLE.as_secs()),
            );
        }
        let mut c = Conn::open(addr).await?;
        let req = c
            .request(
                Method::PUT,
                &format!("/lakes/{LAKE}/types/person/records/after-idle"),
                "k-loader",
            )
            .header("content-type", "application/json")
            .body(full(json!({"name": "after"}).to_string()))
            .unwrap();
        let (_, v) = c.json(req).await?;
        let p = v["position"].as_u64();
        let told = tokio::time::timeout(WAIT, async {
            loop {
                match sse.next().await {
                    Ok(Some(Sse::Event { id, .. })) if id == p => return true,
                    Ok(Some(_)) => {}
                    _ => return false,
                }
            }
        })
        .await
        .unwrap_or(false);
        if !told {
            report.defect(phase, "the change after the idle time was not told");
        }
        report.line(
            phase,
            format!(
                "{keep_alives} keep-alives in {:.0} s idle; the next change told: {told}",
                t.elapsed().as_secs_f64()
            ),
            json!({"keep_alives": keep_alives, "told_after": told}),
        );
    }
    Ok(())
}

/// Whether `n` is the `next` a poll of the change feed answers after
/// reading the event before it, as rest.md says a `: next` comment is.
async fn polls_next(remote: &RemoteLake, n: u64) -> Result<bool, String> {
    let Some(before) = n.checked_sub(1) else {
        return Ok(false);
    };
    let b = remote
        .feed()
        .poll(Position(before), &FeedFilter::default(), 1)
        .await
        .map_err(|e| format!("{e:?}"))?;
    Ok(b.events.first().map(|e| e.position.0) == Some(before) && b.next.0 == n)
}

async fn open_feed(addr: SocketAddr, last: Option<u64>) -> Result<SseReader, String> {
    let mut c = Conn::open(addr).await?;
    let mut req = c
        .request(Method::GET, &format!("/lakes/{LAKE}/feed"), "k-loader")
        .header("accept", "text/event-stream");
    if let Some(l) = last {
        req = req.header("last-event-id", l.to_string());
    }
    let r = c.send(req.body(full("")).unwrap()).await?;
    if r.status() != 200 {
        return Err(format!("the feed answered {}", r.status()));
    }
    // The connection lives as long as its body is read.
    Ok(SseReader::new(r.into_body()))
}

// ---------------------------------------------------------------- files

fn dir_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|d| {
            d.filter_map(|e| e.ok()?.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

/// The first difference between an export over the wire and the host's
/// own, made later: the host's may go on past it (opening the lake to
/// export it appends a lease), so each file of the first must begin the
/// same file of the second, and every file but the last be equal.
fn compare_dirs(a: &Path, b: &Path) -> Option<String> {
    let names = |d: &Path| -> Result<BTreeSet<String>, String> {
        std::fs::read_dir(d)
            .map_err(|e| format!("{}: {e}", d.display()))?
            .map(|e| {
                e.map(|e| e.file_name().to_string_lossy().into_owned())
                    .map_err(|e| format!("{}: {e}", d.display()))
            })
            .collect()
    };
    let (na, nb) = match (names(a), names(b)) {
        (Ok(na), Ok(nb)) => (na, nb),
        (Err(e), _) | (_, Err(e)) => return Some(e),
    };
    if na.is_empty() {
        return Some(format!("{} holds no file", a.display()));
    }
    if !na.is_subset(&nb) {
        return Some(format!("files {na:?} and {nb:?}"));
    }
    let last = na.last().cloned();
    for n in na {
        let read = |d: &Path| std::fs::read(d.join(&n)).map_err(|e| format!("{n}: {e}"));
        let (x, y) = match (read(a), read(b)) {
            (Ok(x), Ok(y)) => (x, y),
            (Err(e), _) | (_, Err(e)) => return Some(e),
        };
        let same = if Some(&n) == last.as_ref() {
            y.starts_with(&x)
        } else {
            x == y
        };
        if !same {
            return Some(format!("{n} differs"));
        }
    }
    None
}
