//! The metrics door: `GET /metrics` in the Prometheus text format, on a
//! listener of its own, apart from the lakes' door
//! (`docs/decisions/079-a-lake-is-measured-through-a-meter-the-host-binds.md`).
//!
//! [`Metrics`] is the meter `lakelands serve` binds to every lake it opens:
//! each lake's numbers are kept in atomics, labelled with the lake and its
//! engine, and printed whole at a scrape. A scrape tells what lakes the
//! server holds, how large they are and how fast they move, never anything
//! of what they hold, and asks no key: the listener is bound where only
//! the operator reaches it. Nothing is listened for unless `server.metrics`
//! names an address.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::fmt::Write as _;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use bytes::Bytes;
use http_body_util::Full;
use hyper::header::{ALLOW, CONTENT_TYPE, HeaderValue};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use lakelands_api::id::LakeId;
use lakelands_api::seam::{Count, Level, Meter, Metered, Spread};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;

/// The metrics door's configuration: `server.metrics` in the root's
/// `lakelands.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricsConfig {
    /// The address `GET /metrics` answers on; loopback or a private
    /// interface, since a scrape asks no key.
    pub bind: SocketAddr,
}

/// The content type of the Prometheus text format.
pub const TEXT_FORMAT: &str = "text/plain; version=0.0.4; charset=utf-8";

/// The upper bounds of a time's buckets, in microseconds: 100 µs to 10 s.
const MICROS: &[u64] = &[
    100, 250, 500, 1_000, 2_500, 5_000, 10_000, 25_000, 50_000, 100_000, 250_000, 500_000,
    1_000_000, 2_500_000, 5_000_000, 10_000_000,
];

/// The upper bounds of a batch's buckets, in writes.
const WRITES: &[u64] = &[1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024];

/// Every lake's numbers, as the meter of each.
#[derive(Default)]
pub struct Metrics {
    lakes: Mutex<BTreeMap<String, Arc<LakeMeter>>>,
}

impl Metrics {
    /// A registry with no lake yet.
    pub fn new() -> Arc<Metrics> {
        Arc::default()
    }

    /// The meter of `lake`, the same however often it is asked for, so a
    /// lake opened again goes on from its numbers.
    pub fn lake(&self, lake: &LakeId) -> Arc<dyn Meter> {
        let mut lakes = self.lakes.lock().unwrap_or_else(|p| p.into_inner());
        lakes.entry(lake.to_string()).or_default().clone()
    }

    /// Every lake's numbers in the Prometheus text format. A lake told
    /// nothing yet is not shown, and a level never told is not either.
    pub fn render(&self) -> String {
        let lakes: Vec<_> = {
            let lakes = self.lakes.lock().unwrap_or_else(|p| p.into_inner());
            lakes.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };
        let told: Vec<_> = lakes
            .iter()
            .filter_map(|(lake, m)| m.engine.get().map(|e| (labels(lake, e), m)))
            .collect();
        let mut out = String::new();
        for (i, n) in Count::ALL.iter().enumerate() {
            let _ = writeln!(out, "# TYPE {} counter", n.name());
            for (labels, m) in &told {
                let v = m.counts[i].load(Ordering::Relaxed);
                let _ = writeln!(out, "{}{{{labels}}} {v}", n.name());
            }
        }
        for (i, n) in Level::ALL.iter().enumerate() {
            let _ = writeln!(out, "# TYPE {} gauge", n.name());
            for (labels, m) in &told {
                let (set, v) = &m.levels[i];
                if set.load(Ordering::Relaxed) {
                    let v = v.load(Ordering::Relaxed);
                    let _ = writeln!(out, "{}{{{labels}}} {v}", n.name());
                }
            }
        }
        for (i, n) in Spread::ALL.iter().enumerate() {
            let _ = writeln!(out, "# TYPE {} histogram", n.name());
            for (labels, m) in &told {
                m.spreads[i].render(&mut out, n.name(), labels, n.micros());
            }
        }
        out
    }
}

/// `lake` and `engine` as a metric's labels, escaped.
fn labels(lake: &str, engine: &str) -> String {
    format!("lake=\"{}\",engine=\"{}\"", escape(lake), escape(engine))
}

/// A label's value as the text format writes it.
fn escape(v: &str) -> String {
    v.replace('\\', r"\\")
        .replace('"', "\\\"")
        .replace('\n', r"\n")
}

/// One lake's numbers.
struct LakeMeter {
    engine: OnceLock<String>,
    counts: Vec<AtomicU64>,
    levels: Vec<(AtomicBool, AtomicU64)>,
    spreads: Vec<Histogram>,
}

impl Default for LakeMeter {
    fn default() -> LakeMeter {
        let bounds = |n: &Spread| if n.micros() { MICROS } else { WRITES };
        LakeMeter {
            engine: OnceLock::new(),
            counts: Count::ALL.iter().map(|_| AtomicU64::new(0)).collect(),
            levels: Level::ALL.iter().map(|_| Default::default()).collect(),
            spreads: Spread::ALL
                .iter()
                .map(|n| Histogram::new(bounds(n)))
                .collect(),
        }
    }
}

/// Where `n` stands among `all`; the kinds are closed, so it is found.
fn at<T: PartialEq>(all: &[T], n: &T) -> usize {
    all.iter().position(|m| m == n).unwrap_or_default()
}

impl Meter for LakeMeter {
    fn count(&self, at_: Metered<'_>, n: Count, by: u64) {
        self.engine.get_or_init(|| at_.engine.to_owned());
        self.counts[at(Count::ALL, &n)].fetch_add(by, Ordering::Relaxed);
    }

    fn level(&self, at_: Metered<'_>, n: Level, value: u64) {
        self.engine.get_or_init(|| at_.engine.to_owned());
        let (set, v) = &self.levels[at(Level::ALL, &n)];
        v.store(value, Ordering::Relaxed);
        set.store(true, Ordering::Relaxed);
    }

    fn observe(&self, at_: Metered<'_>, n: Spread, value: u64) {
        self.engine.get_or_init(|| at_.engine.to_owned());
        self.spreads[at(Spread::ALL, &n)].observe(value);
    }
}

/// A spread of values in fixed buckets, each held apart and summed up at
/// a scrape.
struct Histogram {
    bounds: &'static [u64],
    /// One more than the bounds: the last holds what lies above them all.
    buckets: Vec<AtomicU64>,
    sum: AtomicU64,
}

impl Histogram {
    fn new(bounds: &'static [u64]) -> Histogram {
        let buckets = (0..=bounds.len()).map(|_| AtomicU64::new(0)).collect();
        Histogram {
            bounds,
            buckets,
            sum: AtomicU64::new(0),
        }
    }

    fn observe(&self, value: u64) {
        let i = self.bounds.partition_point(|b| *b < value);
        self.buckets[i].fetch_add(1, Ordering::Relaxed);
        self.sum.fetch_add(value, Ordering::Relaxed);
    }

    /// Its lines under `name`, in seconds where it holds microseconds.
    fn render(&self, out: &mut String, name: &str, labels: &str, micros: bool) {
        let scaled = |v: u64| match micros {
            true => (v as f64 / 1e6).to_string(),
            false => v.to_string(),
        };
        let mut seen = 0;
        for (b, n) in self.bounds.iter().zip(&self.buckets) {
            seen += n.load(Ordering::Relaxed);
            let le = scaled(*b);
            let _ = writeln!(out, "{name}_bucket{{{labels},le=\"{le}\"}} {seen}");
        }
        seen += self.buckets[self.bounds.len()].load(Ordering::Relaxed);
        let _ = writeln!(out, "{name}_bucket{{{labels},le=\"+Inf\"}} {seen}");
        let sum = scaled(self.sum.load(Ordering::Relaxed));
        let _ = writeln!(out, "{name}_sum{{{labels}}} {sum}");
        let _ = writeln!(out, "{name}_count{{{labels}}} {seen}");
    }
}

/// A bound metrics door, not yet serving.
pub struct MetricsServer {
    listener: TcpListener,
    metrics: Arc<Metrics>,
}

/// Bind the metrics door at `config.bind`. [`MetricsServer::run`] then
/// serves `metrics` there.
pub async fn serve_metrics(
    config: &MetricsConfig,
    metrics: Arc<Metrics>,
) -> io::Result<MetricsServer> {
    let listener = TcpListener::bind(config.bind).await?;
    Ok(MetricsServer { listener, metrics })
}

impl MetricsServer {
    /// The address the door listens on (useful when bound to port 0).
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Serve scrapes until the task is dropped.
    pub async fn run(self) -> io::Result<()> {
        loop {
            let (stream, _) = self.listener.accept().await?;
            let metrics = self.metrics.clone();
            tokio::spawn(async move {
                let service = hyper::service::service_fn(move |req| {
                    let metrics = metrics.clone();
                    async move { Ok::<_, Infallible>(scrape(&metrics, &req)) }
                });
                // A connection that fails affects only itself.
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    }
}

/// The answer to one request of the metrics door.
fn scrape<B>(metrics: &Metrics, req: &Request<B>) -> Response<Full<Bytes>> {
    let (status, body) = match (req.uri().path(), req.method()) {
        ("/metrics", &Method::GET) => (StatusCode::OK, metrics.render()),
        ("/metrics", _) => (StatusCode::METHOD_NOT_ALLOWED, String::new()),
        _ => (StatusCode::NOT_FOUND, String::new()),
    };
    let mut r = Response::new(Full::new(Bytes::from(body)));
    *r.status_mut() = status;
    match status {
        StatusCode::OK => {
            let text = HeaderValue::from_static(TEXT_FORMAT);
            r.headers_mut().insert(CONTENT_TYPE, text);
        }
        StatusCode::METHOD_NOT_ALLOWED => {
            r.headers_mut()
                .insert(ALLOW, HeaderValue::from_static("GET"));
        }
        _ => {}
    }
    r
}

#[cfg(test)]
mod tests;
