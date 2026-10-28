//! The soak (lakelands#975): one lake under a small business's day for
//! hours, its server killed at random, checked after every restart and
//! every hour.
//!
//! One `lakelands serve --mcp` process serves the open book
//! (`examples/open-book/`) from a scratch root; the driver is another
//! process and speaks to it over REST, as `lakelands-client` does. A
//! simulated day, deterministic from the seed, runs every `day` of real
//! time: the accounting export (partners and invoices), the bank export
//! (accounts and movements), the desk (credit limits, reminders raised,
//! sent, escalated and resolved, a write-off proposed and accepted) and a
//! reader (the views, the late invoices, a pattern, as of an earlier
//! position and at an earlier valid time).
//!
//! **The kills.** At a random moment every `kill_every` the server is
//! killed with `SIGKILL`, whatever it is doing; once an hour it is
//! stopped cleanly instead (its stdio client closes); on PostgreSQL, once
//! an hour, every connection to the lake's database is dropped under the
//! running server, which must refuse and then recover. Between a kill and
//! the next start the driver opens the lake itself, as a host's next
//! process would: it verifies the log, restricts what the day made
//! restricted, and now and then checkpoints.
//!
//! **What is a red** (the run stops and keeps its root):
//! 1. an acknowledged write that is not in the lake, or not as written;
//! 2. a log that does not verify;
//! 3. a projection that differs from the fold of its log: an export
//!    beside the server, imported into a fresh lake, read alike at the
//!    head and as of earlier positions;
//! 4. the model's sums: `debtor` and `cash_position` against the
//!    driver's own fold, `days_overdue` against its fold;
//! 5. the feed: a gap or a repeat in its positions across the kills;
//! 6. a refusal the driver did not ask for;
//! 7. a reader with `only` on one partner and without `restricted`
//!    answered a record outside its `only`, or a restricted one.
//!
//! What it measures (the server's memory, the lake's size, each kind of
//! call's latency) it reports each hour and never asserts.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::location::{EngineConfig, Limits, PgTls, PgTlsMode, PgTuning};
use lakelands_api::principal::{Access, Grants, PrincipalKind, TypeGrants};
use lakelands_api::read::{Find, Found, ReadAt};
use lakelands_api::seam::{FeedFilter, Seams};
use lakelands_api::write::WriteOpts;
use lakelands_api::{
    Catalog, Error, LakeId, LakeStore, Location, Position, Principal, RecordKey, Refusal,
    SourceRecord, Timestamp, WriteContext,
};
use lakelands_client::{ApiKey, RemoteConfig, RemoteLake};
use lakelands_server::HttpConfig;
use lakelands_server::resolver::Entry;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};

/// A record's fields, by name.
type Fields = BTreeMap<String, Value>;

/// The lake the run serves.
const LAKE: &str = "soak";
/// The longest a call or a start may take before it is a red: a hang.
const WAIT: Duration = Duration::from_secs(120);
/// How long a call cut off by a kill waits for the server's process to
/// end before the cut is judged a red.
const EXITING: Duration = Duration::from_secs(10);
/// The partner the reader that is not trusted is limited to, and the one
/// whose records are restricted.
const OWN: &str = "p0001";
const HELD: &str = "p0002";

// ------------------------------------------------------------------ plan

/// Where the lake is kept.
#[derive(Clone, Debug)]
pub enum Engine {
    /// On disk, in the scratch root.
    Sqlite,
    /// In a PostgreSQL database that nothing else uses: the soak drops
    /// every connection to it once an hour.
    Postgres {
        /// The connection URL, naming the database.
        url: String,
        /// Plaintext instead of TLS.
        plain: bool,
    },
}

/// What the run does and for how long.
#[derive(Clone, Debug)]
pub struct Plan {
    /// The `lakelands` binary.
    pub lakelands: PathBuf,
    /// Where the lake is kept.
    pub engine: Engine,
    /// How long the run goes.
    pub length: Duration,
    /// The seed of the simulated business.
    pub seed: u64,
    /// How much real time a simulated day takes.
    pub day: Duration,
    /// The shortest and the longest wait between two kills.
    pub kill_every: (Duration, Duration),
    /// How often the deep check runs (export, verify, import, compare).
    pub deep_every: Duration,
    /// Where the scratch root goes.
    pub scratch: PathBuf,
    /// Where the report goes, if anywhere.
    pub json: Option<PathBuf>,
}

impl Plan {
    /// The defaults of a run of record: 24 hours, a day a minute, a kill
    /// every one to ten minutes, a deep check an hour.
    pub fn new(lakelands: PathBuf) -> Plan {
        Plan {
            lakelands,
            engine: Engine::Sqlite,
            length: Duration::from_secs(24 * 3600),
            seed: 1,
            day: Duration::from_secs(60),
            kill_every: (Duration::from_secs(60), Duration::from_secs(600)),
            deep_every: Duration::from_secs(3600),
            scratch: PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/soak")),
            json: None,
        }
    }

    /// The CI run: two minutes, a day a second, a kill every ten to
    /// twenty seconds, a deep check a minute.
    pub fn short(lakelands: PathBuf, scratch: PathBuf) -> Plan {
        Plan {
            length: Duration::from_secs(120),
            day: Duration::from_millis(1000),
            kill_every: (Duration::from_secs(10), Duration::from_secs(20)),
            deep_every: Duration::from_secs(60),
            scratch,
            ..Plan::new(lakelands)
        }
    }
}

// ---------------------------------------------------------------- report

/// What the run saw: a line an hour and at each event, and its first red.
#[derive(Default)]
pub struct Report {
    lines: Vec<Value>,
    red: Option<String>,
    /// The scratch root, kept after a red.
    pub kept: Option<PathBuf>,
}

impl Report {
    fn line(&mut self, what: &str, data: Value) {
        eprintln!("soak: {what}: {data}");
        self.lines.push(json!({"what": what, "data": data}));
    }

    fn fail(&mut self, what: impl Into<String>) {
        let what = what.into();
        eprintln!("soak: RED: {what}");
        self.red.get_or_insert(what);
    }

    /// The first red, if any.
    pub fn red(&self) -> Option<&String> {
        self.red.as_ref()
    }

    /// The report as written to `--json`.
    pub fn json(&self) -> Value {
        json!({"red": self.red, "kept": self.kept, "lines": self.lines})
    }
}

// --------------------------------------------------------------- business

/// A small deterministic generator: the run is the seed's.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

#[derive(Clone, Debug)]
struct Invoice {
    outbound: bool,
    partner: String,
    issued: i64,
    due: i64,
    cents: i64,
    status: &'static str,
    paid: Option<i64>,
}

#[derive(Clone, Debug)]
struct Notice {
    invoice: String,
    partner: String,
    level: u32,
    status: &'static str,
    sent: Option<i64>,
    cents: i64,
}

/// The business as the driver knows it: what it has sent, and been
/// answered, which the lake must hold.
struct Book {
    rng: Rng,
    /// The simulated day, counted from the run's first; day `d` is the
    /// date `start + d`.
    day: i64,
    start: i64,
    partners: Vec<String>,
    limits: BTreeMap<String, i64>,
    invoices: BTreeMap<String, Invoice>,
    balances: BTreeMap<String, i64>,
    movements: u64,
    notices: BTreeMap<String, Notice>,
    restricted: BTreeSet<(String, String)>,
    /// Records to restrict at the next stop: the held partner's, and
    /// every fifth invoice of the reader's own partner.
    to_restrict: Vec<(String, String)>,
}

fn money(cents: i64) -> String {
    format!(
        "{}{}.{:02}",
        if cents < 0 { "-" } else { "" },
        cents.abs() / 100,
        cents.abs() % 100
    )
}

/// The date `days` days after 1970-01-01, `YYYY-MM-DD`.
fn date(days: i64) -> String {
    Timestamp(days * 86_400_000_000).day().expect("a midnight")
}

fn today() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    (now.as_secs() / 86_400) as i64
}

impl Book {
    fn new(seed: u64, days: i64) -> Book {
        let mut book = Book {
            rng: Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1),
            day: 0,
            // The run's days end at today, so that `days_overdue`,
            // which reads today, reads the run's calendar.
            start: today() - days,
            partners: Vec::new(),
            limits: BTreeMap::new(),
            invoices: BTreeMap::new(),
            balances: BTreeMap::from([("DE0001".into(), 1_000_000), ("DE0002".into(), 250_000)]),
            movements: 0,
            notices: BTreeMap::new(),
            restricted: BTreeSet::new(),
            to_restrict: Vec::new(),
        };
        for _ in 0..20 {
            book.partners
                .push(format!("p{:04}", book.partners.len() + 1));
        }
        book
    }

    fn now(&self) -> i64 {
        (self.start + self.day).min(today())
    }
}

// ----------------------------------------------------------- the server

/// The served lake's process.
struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    pid: u32,
    addr: std::net::SocketAddr,
}

fn principals() -> Vec<Entry> {
    let desk = Grants {
        default: Access::Write,
        review: true,
        schema: true,
        ingest: true,
        restricted: true,
        ..Grants::default()
    };
    let reviewer = Grants {
        default: Access::Write,
        review: true,
        restricted: true,
        ..Grants::default()
    };
    let only = |field: &str| TypeGrants {
        only: [(field.into(), Value::String(OWN.into()))].into(),
        ..TypeGrants::default()
    };
    let doubter = Grants {
        default: Access::Read,
        types: [
            ("partner".into(), only("partner_id")),
            ("invoice".into(), only("partner")),
            ("dunning".into(), only("partner")),
        ]
        .into(),
        ..Grants::default()
    };
    [("desk", desk), ("reviewer", reviewer), ("doubter", doubter)]
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

fn who(id: &str) -> Principal {
    let grants = principals()
        .into_iter()
        .find(|e| e.id == id)
        .expect("a principal")
        .grants;
    Principal {
        kind: PrincipalKind::Agent,
        id: id.into(),
        lake: LAKE.into(),
        grants,
    }
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

/// The lake's location: on disk in the root, or a schema of its own.
fn location(plan: &Plan, root: &Path, tag: &str) -> Location {
    match &plan.engine {
        Engine::Sqlite => Location::sqlite(LAKE, root.join("lakes").join(LAKE)),
        Engine::Postgres { url, plain } => Location {
            lake: LAKE.into(),
            engine: EngineConfig::Postgres {
                url: url.clone(),
                schema: Some(format!("lakelands_soak_{}_{tag}", std::process::id())),
                tls: PgTls {
                    mode: if *plain {
                        PgTlsMode::Disable
                    } else {
                        PgTlsMode::Require
                    },
                    ..PgTls::default()
                },
                tuning: PgTuning::default(),
                tablespaces: Default::default(),
            },
            key: None,
            limits: Limits::default(),
            writers: Vec::new(),
        },
    }
}

/// Make a root with the lake at `at`, the principals and the HTTP door.
fn prepare(plan: &Plan, root: &Path, at: &Location) -> Result<(), String> {
    let out = std::process::Command::new(&plan.lakelands)
        .args(["init", &root.display().to_string()])
        .output()
        .map_err(|e| format!("{} does not run: {e}", plan.lakelands.display()))?;
    if !out.status.success() {
        return Err(format!(
            "init failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let lakes = root.join("lakes");
    std::fs::create_dir_all(&lakes).map_err(|e| e.to_string())?;
    std::fs::write(
        lakes.join(format!("{LAKE}.json")),
        serde_json::to_vec_pretty(at).unwrap(),
    )
    .map_err(|e| e.to_string())?;
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
        feed_floor_millis: 25,
        workers: None,
        metrics: None,
        hosts: Vec::new(),
    })
    .unwrap();
    std::fs::write(&file, serde_json::to_vec_pretty(&config).unwrap()).map_err(|e| e.to_string())
}

impl Server {
    /// Start `serve --mcp` as the desk and wait for the address it binds.
    async fn start(plan: &Plan, root: &Path) -> Result<Server, String> {
        let mut child = Command::new(&plan.lakelands)
            .arg("--root")
            .arg(root)
            .args(["--as", "desk", "serve", "--mcp"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("the server does not start: {e}"))?;
        let pid = child.id().ok_or("the server has no process id")?;
        let stdin = child.stdin.take();
        let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
        let mut said = Vec::new();
        let addr = tokio::time::timeout(WAIT, async {
            while let Ok(Some(l)) = lines.next_line().await {
                said.push(l.clone());
                if let Some(a) = l.strip_prefix("lakelands: serving HTTP at http://") {
                    return a.trim_end_matches('/').parse().ok();
                }
            }
            None
        })
        .await
        .ok()
        .flatten()
        .ok_or_else(|| format!("the server named no address: {}", said.join(" | ")))?;
        let log = root.join("server.log");
        tokio::spawn(async move {
            use std::io::Write as _;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log)
                .ok();
            for l in said {
                file.as_mut().map(|f| writeln!(f, "{l}"));
            }
            while let Ok(Some(l)) = lines.next_line().await {
                file.as_mut().map(|f| writeln!(f, "{l}"));
            }
        });
        Ok(Server {
            child,
            stdin,
            pid,
            addr,
        })
    }

    fn remote(&self) -> Result<Arc<RemoteLake>, String> {
        let keys: BTreeMap<String, ApiKey> = ["desk", "reviewer", "doubter"]
            .into_iter()
            .map(|p| (p.to_owned(), ApiKey::from(format!("k-{p}").as_str())))
            .collect();
        let lake = RemoteLake::connect(RemoteConfig {
            url: format!("http://{}/", self.addr),
            lake: LakeId::from(LAKE),
            keys,
            feed_key: Some("desk".into()),
            ca_file: None,
            timeout_secs: WAIT.as_secs(),
            max_answer_bytes: lakelands_client::DEFAULT_MAX_ANSWER_BYTES,
            limits: Limits::default(),
        })
        .map_err(|e| e.0)?;
        Ok(Arc::new(lake))
    }

    /// `SIGKILL`, whatever the server is doing.
    async fn kill(mut self) {
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }

    /// A clean stop: the stdio client closes, and the server ends with it.
    async fn stop(mut self) -> Result<(), String> {
        drop(self.stdin.take());
        match tokio::time::timeout(WAIT, self.child.wait()).await {
            Ok(Ok(s)) if s.success() => Ok(()),
            Ok(Ok(s)) => Err(format!("the server ended {s} when its client closed")),
            Ok(Err(e)) => Err(format!("waiting for the server: {e}")),
            Err(_) => Err("the server did not end when its client closed".into()),
        }
    }

    /// `SIGKILL` at `at`, from a task of its own, so that the kill lands
    /// whatever the server is doing then. The kill is sent without an
    /// `await` once the sleep is over, so a task aborted ([`paused`]) has
    /// either sent it or never will.
    fn kill_at(&self, at: Instant) -> tokio::task::JoinHandle<()> {
        let pid = self.pid.to_string();
        tokio::spawn(async move {
            tokio::time::sleep_until(at.into()).await;
            let _ = std::process::Command::new("kill")
                .args(["-KILL", &pid])
                .status();
        })
    }

    fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Whether the server's process ended, waiting up to [`EXITING`] for
    /// it. A `SIGKILL` closes the process's sockets before the process can
    /// be waited on, so a call the kill cut off may find it still running
    /// for a moment.
    async fn ended(&mut self) -> bool {
        tokio::time::timeout(EXITING, self.child.wait())
            .await
            .is_ok()
    }

    /// The server's resident memory in KiB: the whole, what is its own
    /// (`RssAnon`: the heap, the engine's page cache) and what maps a file
    /// (`RssFile`: the binary, a projection read through `mmap`). Read from
    /// `/proc`, so on Linux only: elsewhere each field is null.
    fn memory(&self) -> serde_json::Value {
        let status = std::fs::read_to_string(format!("/proc/{}/status", self.pid));
        let status = status.unwrap_or_default();
        let kib = |field: &str| {
            let line = status.lines().find(|l| l.starts_with(field))?;
            line.split_whitespace().nth(1)?.parse::<u64>().ok()
        };
        json!({"rss_kib": kib("VmRSS:"), "anon_kib": kib("RssAnon:"), "file_kib": kib("RssFile:")})
    }
}

/// Stops the killer before a check, so that no kill lands on the check's
/// own reads: a check judges the lake, and a read a kill cuts off is no
/// finding. Whether the server still runs; where the killer had fired,
/// it waits for the process to end and answers `false`. The killer is
/// left an idle task, so a later pause, or an abort, finds one to stop.
async fn paused(killer: &mut tokio::task::JoinHandle<()>, server: &mut Server) -> bool {
    killer.abort();
    let idle = tokio::spawn(std::future::pending());
    match std::mem::replace(killer, idle).await {
        Ok(()) => !server.ended().await,
        Err(_) => server.alive(),
    }
}

/// Opens the lake in this process, as a host's next process would.
async fn open_here(at: &Location, mode: OpenMode) -> Result<Arc<dyn LakeStore>, String> {
    let engines: Vec<Arc<dyn EngineOpener>> = vec![
        Arc::new(lakelands_engine_sqlite::SqliteOpener),
        #[cfg(feature = "pg")]
        Arc::new(lakelands_engine_pg::PgOpener),
    ];
    let seams = Seams {
        engines,
        ..Seams::default()
    };
    lakelands_store::open(at.clone(), seams, mode)
        .await
        .map_err(|e| format!("opening the lake: {e}"))
}

/// The lake's size in bytes: the files under its root on SQLite, its
/// database on PostgreSQL (the soak's own, which nothing else uses).
fn size(plan: &Plan, root: &std::path::Path) -> Option<u64> {
    fn files(dir: &std::path::Path) -> u64 {
        let entries = std::fs::read_dir(dir).into_iter().flatten().flatten();
        let size = |e: std::fs::DirEntry| match e.file_type() {
            Ok(t) if t.is_dir() => files(&e.path()),
            _ => e.metadata().map_or(0, |m| m.len()),
        };
        entries.map(size).sum()
    }
    match &plan.engine {
        Engine::Sqlite => Some(files(root)),
        Engine::Postgres { url, .. } => {
            let mut client = postgres::Client::connect(url, postgres::NoTls).ok()?;
            let sql = "SELECT pg_database_size(current_database())";
            let bytes: i64 = client.query_one(sql, &[]).ok()?.get(0);
            u64::try_from(bytes).ok()
        }
    }
}

/// Drops every connection to the PostgreSQL database `url` names but
/// this one.
fn drop_connections(url: &str) -> Result<u64, String> {
    let mut client = postgres::Client::connect(url, postgres::NoTls).map_err(|e| e.to_string())?;
    let rows = client
        .query(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
             WHERE datname = current_database() AND pid <> pg_backend_pid()",
            &[],
        )
        .map_err(|e| e.to_string())?;
    Ok(rows.len() as u64)
}

// ------------------------------------------------------------------ calls

/// Why a call did not answer as the driver wants.
enum Miss {
    /// The server is gone (a kill): the call's outcome is not known, and
    /// it is sent again after the next start.
    Gone,
    /// Anything else: a red.
    Red(String),
}

/// The latency of each kind of call, for the hour's line.
#[derive(Default)]
struct Timings(BTreeMap<&'static str, Vec<Duration>>);

impl Timings {
    fn add(&mut self, what: &'static str, d: Duration) {
        self.0.entry(what).or_default().push(d);
    }
    fn take(&mut self) -> Value {
        let mut out = serde_json::Map::new();
        for (what, mut all) in std::mem::take(&mut self.0) {
            all.sort();
            let at = |q: usize| all[(all.len() - 1) * q / 100].as_secs_f64() * 1000.0;
            out.insert(
                what.into(),
                json!({"n": all.len(), "p50_ms": at(50), "p99_ms": at(99)}),
            );
        }
        Value::Object(out)
    }
}

struct Stream<I>(I);

impl<I: Iterator<Item = SourceRecord> + Unpin> futures_core::Stream for Stream<I> {
    type Item = SourceRecord;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<SourceRecord>> {
        std::task::Poll::Ready(self.0.next())
    }
}

/// The driver's side of the run: the client, what it was answered, and
/// what is still owed after a kill.
struct Desk {
    cat: Arc<dyn Catalog>,
    store: Arc<RemoteLake>,
    /// Every record the lake must hold, by type and key, with the fields
    /// the driver last had acknowledged.
    ledger: BTreeMap<(String, String), BTreeMap<String, Value>>,
    /// Writes whose answer a kill took: sent again after the next start.
    owed: Vec<Owed>,
    /// How many writes were sent again.
    resent: u64,
    timings: Timings,
}

#[derive(Clone)]
enum Owed {
    Ingest(String, Vec<(String, String, Value)>),
    Upsert(String, String, Value, String),
    Call(String, String, Value, String),
    Accept(String),
}

impl Desk {
    fn connect(server: &Server) -> Result<Desk, String> {
        let store = server.remote()?;
        Ok(Desk {
            cat: store.catalog(WriteContext::default()),
            store,
            ledger: BTreeMap::new(),
            owed: Vec::new(),
            resent: 0,
            timings: Timings::default(),
        })
    }

    fn reconnect(&mut self, server: &Server) -> Result<(), String> {
        self.store = server.remote()?;
        self.cat = self.store.catalog(WriteContext::default());
        Ok(())
    }

    /// What an error means: a transport failure while the server is gone
    /// is a kill's; anything else is a red.
    fn miss(e: Error, alive: bool, what: &str) -> Miss {
        let transport = matches!(e, Error::Unavailable { .. });
        if transport && !alive {
            Miss::Gone
        } else {
            Miss::Red(format!("{what}: {e} ({e:?})"))
        }
    }

    async fn send(&mut self, server: &mut Server, owed: Owed) -> Result<(), Miss> {
        let t0 = Instant::now();
        let desk = who("desk");
        let answer: Result<(), Error> = match &owed {
            Owed::Ingest(source, rows) => {
                let records: Vec<SourceRecord> = rows
                    .iter()
                    .map(|(ty, key, fields)| SourceRecord {
                        source: None,
                        upstream: None,
                        ty: ty.as_str().into(),
                        key: key.as_str().into(),
                        fields: serde_json::from_value(fields.clone()).unwrap(),
                        seen_at: None,
                        resubmits: None,
                    })
                    .collect();
                let s: lakelands_api::SourceStream = Box::pin(Stream(records.into_iter()));
                match self.cat.ingest(&desk, &source.as_str().into(), s).await {
                    Err(e) => Err(e),
                    Ok(mut out) => {
                        let mut failed = None;
                        while let Some(w) =
                            std::future::poll_fn(|cx| out.as_mut().poll_next(cx)).await
                        {
                            if let Err(e) = w {
                                failed = Some(e);
                                break;
                            }
                        }
                        failed.map_or(Ok(()), Err)
                    }
                }
            }
            Owed::Upsert(ty, key, fields, idem) => {
                let opts = WriteOpts {
                    idempotency_key: Some(idem.as_str().into()),
                    ..WriteOpts::default()
                };
                let patch = serde_json::from_value(fields.clone()).unwrap();
                self.cat
                    .upsert(
                        &desk,
                        &ty.as_str().into(),
                        &key.as_str().into(),
                        patch,
                        opts,
                    )
                    .await
                    .map(|_| ())
            }
            Owed::Call(verb, key, payload, idem) => {
                let opts = WriteOpts {
                    idempotency_key: Some(idem.as_str().into()),
                    ..WriteOpts::default()
                };
                let key = RecordKey::from(key.as_str());
                let payload = serde_json::from_value(payload.clone()).unwrap();
                self.cat
                    .call(&desk, &verb.as_str().into(), Some(&key), payload, opts)
                    .await
                    .map(|_| ())
            }
            Owed::Accept(proposal) => {
                match self
                    .cat
                    .accept(&who("reviewer"), &proposal.as_str().into())
                    .await
                {
                    // Accepted before a kill took its answer.
                    Err(Error::Refused {
                        reason: Refusal::Decided,
                        ..
                    }) => Ok(()),
                    other => other.map(|_| ()),
                }
            }
        };
        let kind = match &owed {
            Owed::Ingest(..) => "ingest",
            Owed::Upsert(..) => "upsert",
            Owed::Call(..) => "call",
            Owed::Accept(..) => "accept",
        };
        self.timings.add(kind, t0.elapsed());
        match answer {
            Ok(()) => {
                self.acknowledged(&owed);
                Ok(())
            }
            Err(e) => {
                let alive = match e {
                    Error::Unavailable { .. } => !server.ended().await,
                    _ => server.alive(),
                };
                let miss = Desk::miss(e, alive, kind);
                if matches!(miss, Miss::Gone) {
                    self.owed.push(owed);
                }
                Err(miss)
            }
        }
    }

    fn acknowledged(&mut self, owed: &Owed) {
        let mut put = |ty: &str, key: &str, fields: &Value| {
            let entry = self.ledger.entry((ty.into(), key.into())).or_default();
            for (f, v) in fields.as_object().into_iter().flatten() {
                entry.insert(f.clone(), v.clone());
            }
        };
        match owed {
            Owed::Ingest(_, rows) => rows
                .iter()
                .for_each(|(ty, key, fields)| put(ty, key, fields)),
            Owed::Upsert(ty, key, fields, _) => put(ty, key, fields),
            Owed::Call(verb, key, payload, _) => {
                let (ty, set) = verb_effect(verb, payload);
                put(ty, key, &set);
            }
            Owed::Accept(_) => {}
        }
    }
}

/// The fields a verb of the open book sets, as the driver expects them.
fn verb_effect(verb: &str, payload: &Value) -> (&'static str, Value) {
    match verb {
        "set_credit_limit" => ("partner", json!({"credit_limit": payload["credit_limit"]})),
        "mark_dunning_sent" => (
            "dunning",
            json!({"status": "sent", "sent_on": payload["sent_on"]}),
        ),
        "escalate_dunning" => ("dunning", json!({"status": "escalated"})),
        "resolve_dunning" => ("dunning", json!({"status": "resolved"})),
        "write_off_dunning" => ("dunning", json!({"status": "written_off"})),
        _ => ("", json!({})),
    }
}

// ------------------------------------------------------------- the day

/// One simulated day's writes, in the order a business makes them.
fn the_day(book: &mut Book) -> Vec<Owed> {
    book.day += 1;
    let today = book.now();
    let mut writes = Vec::new();
    // A new partner now and then.
    if book.rng.chance(20) {
        book.partners
            .push(format!("p{:04}", book.partners.len() + 1));
    }
    let partners: Vec<(String, String, Value)> = book
        .partners
        .iter()
        .enumerate()
        .filter(|(i, _)| book.day == 1 || *i + 1 == book.partners.len())
        .map(|(i, p)| {
            let role = ["debtor", "debtor", "both", "creditor"][i % 4];
            ("partner".into(), p.clone(), json!({"partner_id": p, "name": format!("Partner {p}"), "role": role, "payment_terms_days": 14 + 16 * (i as i64 % 2)}))
        })
        .collect();
    // The accounting export: today's invoices, and yesterday's paid ones
    // again, as an export repeats its rows.
    let mut invoices = Vec::new();
    for _ in 0..(3 + book.rng.below(5)) {
        let id = format!("i{:07}", book.invoices.len() + 1);
        let partner = book.partners[book.rng.below(book.partners.len() as u64) as usize].clone();
        let terms = if book.rng.chance(50) { 14 } else { 30 };
        let inv = Invoice {
            outbound: book.rng.chance(80),
            partner,
            issued: today,
            due: today + terms,
            cents: 1000 + book.rng.below(500_000) as i64,
            status: "open",
            paid: None,
        };
        if inv.partner == HELD || (inv.partner == OWN && book.invoices.len().is_multiple_of(5)) {
            book.to_restrict.push(("invoice".into(), id.clone()));
        }
        invoices.push((id.clone(), inv.clone()));
        book.invoices.insert(id, inv);
    }
    // Some open invoices are paid, through the bank.
    let mut movements = Vec::new();
    let open: Vec<String> = book
        .invoices
        .iter()
        .filter(|(_, i)| i.status == "open" && i.issued < today)
        .map(|(k, _)| k.clone())
        .collect();
    for id in open {
        if !book.rng.chance(6) {
            continue;
        }
        let inv = book.invoices.get_mut(&id).unwrap();
        inv.status = "paid";
        inv.paid = Some(today);
        let (account, direction, sign) = if inv.outbound {
            ("DE0001", "credit", 1)
        } else {
            ("DE0002", "debit", -1)
        };
        *book.balances.get_mut(account).unwrap() += sign * inv.cents;
        book.movements += 1;
        let key = format!("{account}:{}:{}:{id}", date(today), money(inv.cents));
        movements.push(("transaction".to_string(), key, json!({"account": account, "value_on": date(today), "amount": money(inv.cents), "direction": direction, "reference": id})));
        invoices.push((id.clone(), inv.clone()));
    }
    let mut rows = partners;
    for (id, inv) in invoices {
        let mut fields = json!({"invoice_id": id, "direction": if inv.outbound { "outbound" } else { "inbound" }, "partner": inv.partner,
            "issued_on": date(inv.issued), "due_on": date(inv.due), "amount": money(inv.cents), "currency": "EUR", "status": inv.status});
        if let Some(p) = inv.paid {
            fields["paid_on"] = json!(date(p));
        }
        rows.push(("invoice".into(), id, fields));
    }
    writes.push(Owed::Ingest("accounting".into(), rows));
    let mut bank: Vec<(String, String, Value)> = book
        .balances
        .iter()
        .map(|(iban, cents)| ("bank_account".to_string(), iban.clone(), json!({"iban": iban, "currency": "EUR", "balance": money(*cents), "balance_on": date(today)})))
        .collect();
    bank.extend(movements);
    writes.push(Owed::Ingest("bank".into(), bank));
    // The desk: reminders of what is more than thirty days late, sent the
    // next day, escalated after fifteen more, resolved once paid.
    let late: Vec<(String, Invoice)> = book
        .invoices
        .iter()
        .filter(|(_, i)| i.outbound && i.status == "open" && today - i.due > 30)
        .map(|(k, i)| (k.clone(), i.clone()))
        .collect();
    for (id, inv) in late {
        let level = 1 + ((today - inv.due - 31) / 15).clamp(0, 2) as u32;
        let key = format!("d{id}-{level}");
        if book.notices.contains_key(&key) {
            continue;
        }
        if level > 1 {
            let before = format!("d{id}-{}", level - 1);
            if book
                .notices
                .get(&before)
                .is_some_and(|n| n.status == "sent")
            {
                book.notices.get_mut(&before).unwrap().status = "escalated";
                writes.push(Owed::Call(
                    "escalate_dunning".into(),
                    before.clone(),
                    json!({}),
                    format!("esc-{before}"),
                ));
            }
        }
        let notice = Notice {
            invoice: id.clone(),
            partner: inv.partner.clone(),
            level,
            status: "open",
            sent: None,
            cents: inv.cents,
        };
        writes.push(Owed::Upsert("dunning".into(), key.clone(),
            json!({"id": key, "invoice": id, "partner": inv.partner, "level": level, "amount": money(inv.cents), "status": "open"}), format!("raise-{key}")));
        if inv.partner == HELD {
            book.to_restrict.push(("dunning".into(), key.clone()));
        }
        book.notices.insert(key, notice);
    }
    let pending: Vec<String> = book
        .notices
        .iter()
        .filter(|(_, n)| n.status == "open" || n.status == "sent")
        .map(|(k, _)| k.clone())
        .collect();
    for key in pending {
        let paid = book
            .invoices
            .get(&book.notices[&key].invoice)
            .is_some_and(|i| i.status == "paid");
        let n = book.notices.get_mut(&key).unwrap();
        if paid {
            n.status = "resolved";
            writes.push(Owed::Call(
                "resolve_dunning".into(),
                key.clone(),
                json!({}),
                format!("res-{key}"),
            ));
        } else if n.status == "open" && n.sent.is_none() {
            n.status = "sent";
            n.sent = Some(today);
            writes.push(Owed::Call(
                "mark_dunning_sent".into(),
                key.clone(),
                json!({"sent_on": date(today)}),
                format!("sent-{key}"),
            ));
        }
    }
    // A credit limit now and then.
    if book.rng.chance(30) {
        let p = book.partners[book.rng.below(book.partners.len() as u64) as usize].clone();
        let cents = 100_000 * (1 + book.rng.below(20) as i64);
        book.limits.insert(p.clone(), cents);
        writes.push(Owed::Call(
            "set_credit_limit".into(),
            p.clone(),
            json!({"credit_limit": money(cents)}),
            format!("lim-{}-{p}", book.day),
        ));
    }
    if book.day == 1 {
        book.to_restrict.push(("partner".into(), HELD.into()));
    }
    writes
}

// ---------------------------------------------------------------- checks

/// Every record of `ty` the principal is answered, by key.
async fn all(
    cat: &dyn Catalog,
    who: &Principal,
    ty: &str,
    at: ReadAt,
) -> Result<BTreeMap<String, BTreeMap<String, Value>>, String> {
    let (mut out, mut cursor) = (BTreeMap::new(), None);
    loop {
        let find = Find {
            limit: Some(1000),
            cursor: cursor.take(),
            at,
            ..Find::default()
        };
        let Found::Records {
            rows, next_cursor, ..
        } = cat
            .find(who, &ty.into(), find)
            .await
            .map_err(|e| format!("find {ty}: {e}"))?
            .data
        else {
            return Err(format!("find {ty} answered groups"));
        };
        for r in rows {
            let fields = r
                .fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect();
            out.insert(r.key.to_string(), fields);
        }
        match next_cursor {
            Some(c) => cursor = Some(c),
            None => return Ok(out),
        }
    }
}

const TYPES: [&str; 7] = [
    "partner",
    "invoice",
    "bank_account",
    "transaction",
    "dunning",
    "debtor",
    "cash_position",
];

/// Red 1: every acknowledged record is in the lake as written.
async fn ledger_holds(desk: &Desk) -> Result<(), String> {
    let mut by_type: BTreeMap<&str, Vec<(&String, &Fields)>> = BTreeMap::new();
    for ((ty, key), fields) in &desk.ledger {
        by_type.entry(ty.as_str()).or_default().push((key, fields));
    }
    for (ty, expected) in by_type {
        let held = all(desk.cat.as_ref(), &who("desk"), ty, ReadAt::default()).await?;
        for (key, fields) in expected {
            let Some(got) = held.get(key.as_str()) else {
                return Err(format!(
                    "{ty} {key} was acknowledged and is not in the lake"
                ));
            };
            for (f, v) in fields {
                if got.get(f) != Some(v) {
                    return Err(format!(
                        "{ty} {key}.{f} was acknowledged as {v} and the lake holds {:?}",
                        got.get(f)
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Red 7: the reader with `only` on one partner and without `restricted`
/// is answered that partner's records alone, and none restricted.
async fn doubter_holds(desk: &Desk, book: &Book) -> Result<(), String> {
    for (ty, field) in [
        ("partner", "partner_id"),
        ("invoice", "partner"),
        ("dunning", "partner"),
    ] {
        let held = all(desk.cat.as_ref(), &who("doubter"), ty, ReadAt::default()).await?;
        for (key, fields) in held {
            if fields.get(field) != Some(&json!(OWN)) {
                return Err(format!(
                    "the reader limited to {OWN} was answered {ty} {key} of {:?}",
                    fields.get(field)
                ));
            }
            if book.restricted.contains(&(ty.into(), key.clone())) {
                return Err(format!(
                    "the reader without `restricted` was answered the restricted {ty} {key}"
                ));
            }
        }
    }
    Ok(())
}

/// Red 4: the views and the set against the driver's own fold.
async fn sums_hold(desk: &Desk, book: &Book) -> Result<(), String> {
    let cat = desk.cat.as_ref();
    let mut owed: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    for inv in book
        .invoices
        .values()
        .filter(|i| i.outbound && i.status == "open")
    {
        let e = owed.entry(inv.partner.clone()).or_default();
        e.0 += inv.cents;
        e.1 += 1;
    }
    let debtor = all(cat, &who("desk"), "debtor", ReadAt::default()).await?;
    let got: BTreeMap<String, (Value, Value)> = debtor
        .into_iter()
        .map(|(k, f)| {
            (
                k,
                (f["open_receivables"].clone(), f["open_invoices"].clone()),
            )
        })
        .collect();
    let want: BTreeMap<String, (Value, Value)> = owed
        .into_iter()
        .map(|(k, (c, n))| (k, (json!(money(c)), json!(n))))
        .collect();
    if got != want {
        return Err(format!(
            "`debtor` holds {} partners' sums against the {} the driver folds; first apart: {:?}",
            got.len(),
            want.len(),
            want.iter().find(|(k, v)| got.get(*k) != Some(*v))
        ));
    }
    let cash = all(cat, &who("desk"), "cash_position", ReadAt::default()).await?;
    let total: i64 = book.balances.values().sum();
    if cash
        .get("EUR")
        .map(|f| (&f["total_balance"], &f["accounts"]))
        != Some((&json!(money(total)), &json!(book.balances.len())))
    {
        return Err(format!(
            "`cash_position` holds {cash:?}, the driver's balances sum to {}",
            money(total)
        ));
    }
    let find = Find {
        filter: Some("direction = 'outbound' and status = 'open' and days_overdue > 30".into()),
        limit: Some(1000),
        ..Find::default()
    };
    let Found::Records { rows, .. } = cat
        .find(&who("desk"), &"invoice".into(), find)
        .await
        .map_err(|e| e.to_string())?
        .data
    else {
        unreachable!()
    };
    let got: BTreeSet<String> = rows.iter().map(|r| r.key.to_string()).collect();
    let want: BTreeSet<String> = book
        .invoices
        .iter()
        .filter(|(_, i)| i.outbound && i.status == "open" && i.due < today() - 30)
        .map(|(k, _)| k.clone())
        .collect();
    if got.len() < 1000 && got != want {
        return Err(format!(
            "`days_overdue > 30` finds {} invoices, the driver's fold selects {}",
            got.len(),
            want.len()
        ));
    }
    Ok(())
}

/// Red 5: the feed's positions follow each other, across the kills.
async fn feed_follows(store: &RemoteLake, next: &mut Position) -> Result<u64, String> {
    let feed = store.feed();
    let mut seen = 0;
    loop {
        let batch = feed
            .poll(*next, &FeedFilter::default(), 1000)
            .await
            .map_err(|e| format!("the feed: {e}"))?;
        for e in &batch.events {
            if e.position != *next {
                return Err(format!(
                    "the feed answered position {} where {} was next",
                    e.position.0, next.0
                ));
            }
            *next = Position(next.0 + 1);
            seen += 1;
        }
        if batch.next > batch.head || batch.events.is_empty() {
            return Ok(seen);
        }
    }
}

/// Red 2 and 3: an export beside the server verifies, and imported into a
/// fresh lake reads as the live one, at the head and as of earlier
/// positions.
async fn deep(plan: &Plan, dir: &Path, root: &Path, desk: &Desk, n: u64) -> Result<Value, String> {
    let export = dir.join(format!("export-{n}"));
    cli(
        plan,
        root,
        &["export", "--to", &export.display().to_string()],
    )?;
    let v = lakelands_core::export::verify(&export)
        .map_err(|e| format!("the export does not verify: {e}"))?;
    let other = dir.join(format!("import-{n}"));
    let at = location(plan, &other, &format!("i{n}"));
    prepare(plan, &other, &at)?;
    cli(plan, &other, &["import", &export.display().to_string()])?;
    let imported = open_here(&at, OpenMode::Read).await?;
    let head = imported.head().await.map_err(|e| e.to_string())?;
    let earlier = [head.0 / 4, head.0 / 2, head.0 * 3 / 4];
    let cat = imported.catalog(WriteContext::default());
    for as_of in std::iter::once(None).chain(earlier.iter().map(|p| Some(Position(*p)))) {
        let at = ReadAt {
            as_of,
            valid_at: None,
            clock: None,
        };
        for ty in TYPES {
            let live = all(desk.cat.as_ref(), &who("desk"), ty, at).await?;
            let again = all(cat.as_ref(), &who("desk"), ty, at).await?;
            if live != again {
                let apart = live
                    .iter()
                    .find(|(k, v)| again.get(*k) != Some(*v))
                    .map(|(k, _)| k.clone());
                return Err(format!(
                    "{ty} as of {as_of:?} reads otherwise on the lake its export makes: {} records against {}; first apart {apart:?}",
                    live.len(),
                    again.len()
                ));
            }
        }
    }
    drop(imported);
    let _ = std::fs::remove_dir_all(&export);
    let _ = std::fs::remove_dir_all(&other);
    Ok(json!({"head": head.0, "verified_through": v.through.0, "compared_at": earlier}))
}

// ------------------------------------------------------------------- run

/// The run: until its length is out or the first red.
pub async fn run(plan: Plan) -> Report {
    let mut report = Report::default();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let dir = plan
        .scratch
        .join(format!("lakelands-soak-{}-{stamp}", std::process::id()));
    let result = drive(&plan, &dir, &mut report).await;
    if let Err(e) = result {
        report.fail(e);
    }
    if report.red.is_some() {
        report.kept = Some(dir.clone());
        eprintln!("soak: the scratch root is kept at {}", dir.display());
    } else {
        let _ = std::fs::remove_dir_all(&dir);
    }
    report
}

async fn drive(plan: &Plan, dir: &Path, report: &mut Report) -> Result<(), String> {
    let root = dir.join("root");
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let at = location(plan, &root, "live");
    prepare(plan, &root, &at)?;
    let bundle = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/open-book");
    cli(plan, &root, &["apply", bundle])?;
    let t0 = Instant::now();
    let days = (plan.length.as_secs_f64() / plan.day.as_secs_f64()).ceil() as i64 + 1;
    let mut book = Book::new(plan.seed, days);
    let mut server = Server::start(plan, &root).await?;
    let mut desk = Desk::connect(&server)?;
    for view in ["debtor", "cash_position"] {
        let key = format!("view_kept.{view}");
        let owed = Owed::Upsert(
            "sys.setting".into(),
            key.clone(),
            json!({"key": key, "value": true}),
            format!("kept-{view}"),
        );
        desk.send(&mut server, owed)
            .await
            .map_err(|_| "keeping the views".to_string())?;
    }
    let mut rng = Rng(plan.seed ^ 0xDEAD_BEEF);
    let wait = |rng: &mut Rng| {
        plan.kill_every.0
            + Duration::from_secs_f64(
                (plan.kill_every.1 - plan.kill_every.0).as_secs_f64()
                    * (rng.below(1000) as f64 / 1000.0),
            )
    };
    // Once the run's time is up no kill is armed: the loop goes on until
    // nothing is owed and the server runs, and the last checks read it.
    let arm = |server: &Server, rng: &mut Rng| match t0.elapsed() < plan.length {
        true => server.kill_at(Instant::now() + wait(rng)),
        false => tokio::spawn(std::future::pending()),
    };
    let mut killer = arm(&server, &mut rng);
    let (mut last_clean, mut last_drop, mut last_deep) =
        (Instant::now(), Instant::now(), Instant::now());
    let (mut kills, mut stops, mut drops, mut deeps) = (0u64, 0u64, 0u64, 0u64);
    let mut feed_next = Position(0);
    let mut next_day = Instant::now();
    loop {
        let ending = t0.elapsed() >= plan.length;
        if ending && !killer.is_finished() {
            paused(&mut killer, &mut server).await;
        }
        // A day's writes, each sent until a kill takes one.
        if !ending && Instant::now() >= next_day {
            next_day += plan.day;
            let writes = the_day(&mut book);
            let mut gone = false;
            for (i, w) in writes.iter().enumerate() {
                if gone {
                    desk.owed.push(w.clone());
                    continue;
                }
                match desk.send(&mut server, w.clone()).await {
                    Ok(()) => {}
                    Err(Miss::Gone) => gone = i < writes.len(),
                    Err(Miss::Red(e)) => return Err(e),
                }
            }
        }
        // A dropped connection once an hour on PostgreSQL, a clean stop
        // once an hour, and the killer's `SIGKILL` at its moment.
        let hourly = Duration::from_secs(3600).min(plan.length / 2);
        let mut dead = !server.alive();
        let mut clean = !dead && !ending && last_clean.elapsed() >= hourly;
        if let (Engine::Postgres { url, .. }, false, false) = (&plan.engine, dead, ending)
            && last_drop.elapsed() >= hourly
        {
            last_drop = Instant::now();
            drops += 1;
            // No kill while the server recovers: its refusals here
            // are the drop's.
            killer.abort();
            let url = url.clone();
            let n = tokio::task::spawn_blocking(move || drop_connections(&url))
                .await
                .map_err(|e| e.to_string())??;
            report.line("dropped", json!({"connections": n}));
            // The server refuses while it reconnects, and then
            // answers again: a call that hangs past the wait is a red.
            let t = Instant::now();
            loop {
                match tokio::time::timeout(
                    WAIT,
                    desk.cat.find(
                        &who("desk"),
                        &"partner".into(),
                        Find {
                            limit: Some(1),
                            ..Find::default()
                        },
                    ),
                )
                .await
                {
                    Ok(Ok(_)) => break,
                    Ok(Err(_)) if t.elapsed() < WAIT => {
                        tokio::time::sleep(Duration::from_millis(500)).await
                    }
                    Ok(Err(e)) => {
                        let alive = server.alive();
                        return Err(format!(
                            "the server did not recover from dropped connections (its process {}): {e}",
                            if alive { "runs" } else { "ended" }
                        ));
                    }
                    Err(_) => {
                        return Err("a call hung after the connections were dropped".into());
                    }
                }
            }
            let ms = t.elapsed().as_millis();
            report.line("recovered", json!({"after_ms": ms}));
            killer = arm(&server, &mut rng);
            continue;
        }
        // A clean stop pauses the killer first. A kill whose moment came
        // as the stop began, sent or reaped after `alive` answered, is this
        // turn's kill, not a server that ended badly when its client closed.
        if clean && !paused(&mut killer, &mut server).await {
            (dead, clean) = (true, false);
        }
        if dead || clean {
            killer.abort();
            if clean {
                last_clean = Instant::now();
                stops += 1;
                server.stop().await?;
            } else {
                kills += 1;
                server.kill().await;
            }
            // Between two processes: open the lake here, verify it,
            // restrict what the day made restricted, and checkpoint now
            // and then.
            {
                let here = open_here(&at, OpenMode::Write).await?;
                here.verify(None, None)
                    .await
                    .map_err(|e| format!("the log does not verify after a stop: {e}"))?;
                for (ty, key) in std::mem::take(&mut book.to_restrict) {
                    match here
                        .restrict(&ty.as_str().into(), &key.as_str().into(), "held".into())
                        .await
                    {
                        Ok(_) => {
                            book.restricted.insert((ty, key));
                        }
                        // Not written yet: a kill took its write; restrict it at the next stop.
                        Err(Error::NotFound { .. }) => book.to_restrict.push((ty, key)),
                        Err(e) => return Err(format!("restricting {ty} {key}: {e}")),
                    }
                }
                if (kills + stops) % 7 == 0 {
                    here.checkpoint()
                        .await
                        .map_err(|e| format!("checkpoint: {e}"))?;
                }
            }
            server = Server::start(plan, &root).await?;
            killer = arm(&server, &mut rng);
            desk.reconnect(&server)?;
            for owed in std::mem::take(&mut desk.owed) {
                desk.resent += 1;
                match desk.send(&mut server, owed).await {
                    Ok(()) => {}
                    // The killer took this server too: the next turn starts another.
                    Err(Miss::Gone) => break,
                    Err(Miss::Red(e)) => return Err(format!("sent again after a stop: {e}")),
                }
            }
            if desk.owed.is_empty() && paused(&mut killer, &mut server).await {
                ledger_holds(&desk).await?;
                doubter_holds(&desk, &book).await?;
                feed_follows(&desk.store, &mut feed_next).await?;
                killer = arm(&server, &mut rng);
            }
        }
        if last_deep.elapsed() >= plan.deep_every && paused(&mut killer, &mut server).await {
            last_deep = Instant::now();
            deeps += 1;
            sums_hold(&desk, &book).await?;
            let deep = deep(plan, dir, &root, &desk, deeps).await?;
            report.line(
                "check",
                json!({"elapsed_s": t0.elapsed().as_secs(), "day": book.day, "kills": kills, "stops": stops, "drops": drops,
                    "records": desk.ledger.len(), "resent": desk.resent, "restricted": book.restricted.len(), "memory": server.memory(), "size_bytes": size(plan, &root), "deep": deep, "timings": desk.timings.take()}),
            );
            killer = arm(&server, &mut rng);
        }
        // The run ends once its time is up, nothing is owed and a pause
        // finds the server running: a kill armed before the time was up
        // lands on no last check, and one that fired is a turn more.
        let done = t0.elapsed() >= plan.length && desk.owed.is_empty();
        if done && paused(&mut killer, &mut server).await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    ledger_holds(&desk).await?;
    sums_hold(&desk, &book).await?;
    report.line("end", json!({"elapsed_s": t0.elapsed().as_secs(), "days": book.day, "kills": kills, "stops": stops, "drops": drops, "records": desk.ledger.len(), "resent": desk.resent, "restricted": book.restricted.len()}));
    killer.abort();
    if server.alive() {
        server.stop().await?;
    }
    Ok(())
}
