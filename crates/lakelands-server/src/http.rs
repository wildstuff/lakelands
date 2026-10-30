//! The HTTP door: MCP over streamable HTTP at `POST /lakes/<id>/mcp`,
//! and REST under every other path of `/lakes/<id>/` ([`crate::rest`]).
//! Both take the same key, the same resolver and the same lakes.
//!
//! For MCP:
//!
//! - `initialize` resolves the caller's API key (`Authorization: Bearer
//!   <key>`) to a principal through the resolver, opens the lake, starts
//!   a session and answers its id in `Mcp-Session-Id`. Every later
//!   request names that session and presents the same key.
//! - A request is answered with one JSON response; a notification or a
//!   client response with `202 Accepted`.
//! - `GET` with the session's id opens the session's event stream: its
//!   notifications ([`crate::watch`]) as server-sent events, a change's
//!   `id` its position. The `GET` is let in like every request, and only
//!   the session's own key opens its stream. A session has one stream: a
//!   second `GET` replaces the first. `Last-Event-ID` resumes the
//!   session's changes after that position. Until a stream is open, the
//!   notifications wait in the session's queue, and a full queue stops
//!   its pump from reading further: the session lags, nothing is lost.
//! - `DELETE` with the session's id ends it, and a session unused for
//!   `session_idle_secs` ends by itself. A session is its lake's and its
//!   key's: named by anyone else, it is answered as unknown.
//! - A request whose `Origin` is not on the configured allow-list is
//!   refused with `403`, before anything else is read.
//! - A tool result's position is also sent in the `Lakelands-Position`
//!   header.
//!
//! Two unauthenticated routes answer outside any lake, at fixed paths,
//! with no data in them ([`Health`]): `GET /livez`, the process is up, and
//! `GET /readyz`, every lake it serves is open, holds its writer lease
//! (as its handle knows it) and answers its head. Whether the projection
//! has caught up with the log is not asked: the store facade has no cheap
//! answer to that, and an open does not return before its fold is done.
//!
//! Everything is configured through [`HttpConfig`], which the root's
//! `lakelands.json` carries; nothing is read from the environment.

use std::collections::HashMap;
use std::convert::Infallible;
use std::io;
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue, ORIGIN};
use hyper::{HeaderMap, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use lakelands_api::error::{Case, Result};
use lakelands_api::id::LakeId;
use lakelands_api::id::Position;
use lakelands_api::read::{CONTEXT_HEADER, POSITION_HEADER, PRINCIPAL_HEADER};
use lakelands_api::seam::{Credential, PrincipalResolver};
use lakelands_api::{LakeStore, Principal, WriteContext};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::{Notify, mpsc};

use crate::bounds::{Bounds, Connection, Gate, Held, Limit, Over};
use crate::watch::{QUEUE, Queued};

use crate::resolver::{Entry, HostEntry, same_key};
use crate::rest::Answer;
use crate::session::{PROTOCOL_VERSIONS, Session, code, error};

/// The header a session's id travels in.
pub const SESSION_HEADER: &str = "mcp-session-id";
/// The header a client names its protocol version in after `initialize`.
pub const VERSION_HEADER: &str = "mcp-protocol-version";

/// The HTTP door's configuration: the `server` section of the root's
/// `lakelands.json`.
///
/// The door speaks plain HTTP in 0.1.0, with no TLS, and the API key
/// travels in every request: bind it to loopback, or terminate TLS in
/// front of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpConfig {
    /// The address to listen on.
    pub bind: SocketAddr,
    /// Origins a browser may call from; a request that carries any other
    /// `Origin` is refused. A request without one (not from a browser)
    /// is allowed.
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    /// A session unused for this many seconds ends; the next request
    /// naming it is answered as for an unknown session.
    #[serde(default = "default_idle")]
    pub session_idle_secs: u64,
    /// How long a caught-up feed stream or MCP session waits for the
    /// lake's next append before it reads the log again, in milliseconds.
    #[serde(default = "default_feed_poll")]
    pub feed_poll_millis: u64,
    /// How often, at most, the log is read for the feed streams and MCP
    /// sessions that follow one lake with one filter, in milliseconds:
    /// they share one read (lakelands#1113), which, woken by an append,
    /// reads once this has passed since the last such read ended. An
    /// append after a quiet spell is read at once; a busy lake's appends
    /// reach the consumers in batches (lakelands#1061).
    #[serde(default = "default_feed_floor")]
    pub feed_floor_millis: u64,
    /// The threads `lakelands serve` runs its doors and sessions on
    /// ([`runtime`]). Absent, the machine's cores, at most
    /// [`WORKERS_MOST`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workers: Option<NonZeroUsize>,
    /// Where `GET /metrics` answers, apart from this door
    /// ([`crate::metrics`]). Absent, nothing is measured and nothing more
    /// is listened for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<crate::metrics::MetricsConfig>,
    /// The other hosts that may forward calls to the REST doors over the
    /// link between hosts, each with keys of its own ([`HostEntry`]).
    /// None, and no call is forwarded here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<HostEntry>,
    /// What one connection and one principal may spend of the door
    /// ([`Bounds`]). Absent, the defaults.
    #[serde(default)]
    pub bounds: Bounds,
}

impl HttpConfig {
    /// Whether the hosts' keys can stand for them: none is empty, none
    /// stands for two hosts, and none is also a key of one of
    /// `principals`, which would make a principal's key a host's. Names
    /// the host, never the key.
    pub fn check(&self, principals: &[Entry]) -> Result<(), String> {
        for (i, host) in self.hosts.iter().enumerate() {
            let id = &host.id;
            if host.keys.is_empty() || host.keys.iter().any(String::is_empty) {
                return Err(format!(
                    "server.hosts: host `{id}` has no key, or an empty one"
                ));
            }
            for key in &host.keys {
                let other =
                    (self.hosts[..i].iter()).find(|h| h.keys.iter().any(|k| same_key(k, key)));
                if let Some(h) = other {
                    return Err(format!(
                        "server.hosts: a key of host `{id}` is also host `{}`'s",
                        h.id
                    ));
                }
                let principal = principals
                    .iter()
                    .find(|p| p.api_keys.iter().any(|k| same_key(k, key)));
                if let Some(p) = principal {
                    return Err(format!(
                        "server.hosts: a key of host `{id}` is also an api key of principal `{}`",
                        p.name
                    ));
                }
            }
        }
        Ok(())
    }
}

/// The most threads [`runtime`] starts when the configuration names no
/// `workers`.
pub const WORKERS_MOST: usize = 8;

/// The runtime a server runs on: as many threads as `config` names in
/// `workers`, or the machine's cores, at most [`WORKERS_MOST`]. The
/// threads run the doors, the sessions and their pumps; no task on them
/// blocks. A lake's storage work runs on the runtime's blocking pool, not
/// on them: the store hands every engine call there, the writer's turn
/// included, and the PostgreSQL driver blocks on its own runtime only
/// from such a thread.
pub fn runtime(config: Option<&HttpConfig>) -> io::Result<tokio::runtime::Runtime> {
    let workers = config.and_then(|c| c.workers).map_or_else(
        || std::thread::available_parallelism().map_or(1, |n| n.get().min(WORKERS_MOST)),
        NonZeroUsize::get,
    );
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()
}

fn default_idle() -> u64 {
    30 * 60
}

fn default_feed_poll() -> u64 {
    250
}

fn default_feed_floor() -> u64 {
    10
}

/// Opens a lake by its id, for a new session.
pub type LakeOpener = dyn Fn(&LakeId) -> Result<Arc<dyn LakeStore>> + Send + Sync;

struct Slot {
    lake: LakeId,
    key: String,
    last_used: Mutex<Instant>,
    session: tokio::sync::Mutex<Session>,
    /// What the session's pump has queued for its stream.
    outbox: Arc<tokio::sync::Mutex<mpsc::Receiver<Queued>>>,
    /// Ends the stream that is open, when another opens.
    stream: Mutex<Arc<Notify>>,
    watch: Arc<crate::watch::Watch>,
    pump: tokio::task::JoinHandle<()>,
    /// The principal the session was initialized as, whose bounds its
    /// requests count against.
    who: Principal,
}

impl Slot {
    /// End the session's open stream and its pump: the session ended, by
    /// `DELETE`, by a new `initialize` in its place, or idle.
    fn end(&self) {
        self.stream
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .notify_one();
        self.pump.abort();
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.end();
    }
}

impl Slot {
    /// Whether this session is one the caller may use: its lake, its key.
    fn owned_by(&self, lake: &LakeId, key: &str) -> bool {
        &self.lake == lake && same_key(&self.key, key)
    }
}

type Lakes = Vec<Arc<dyn LakeStore>>;

/// What the door answers at `/readyz`: set by the host once every lake it
/// serves is open. Until then the answer is "starting"; the door may be
/// bound before the lakes are open, so liveness answers while an open
/// rebuilds a projection.
///
/// "Ready" means open and, as its handle knows it, holding the lease, with
/// a head that answers; it does not compare the head with the log's end.
/// An answer reads each lake's in-memory flags only: no fold, no scan, no
/// engine call. A lease lost to another writer shows once the handle has
/// noticed it, at its next write, rebuild or compaction.
#[derive(Clone, Default)]
pub struct Health {
    lakes: Arc<Mutex<Option<Lakes>>>,
}

/// How ready the door is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    /// The lakes are not all open yet.
    Starting,
    /// A lake is open without its lease, or its head does not answer.
    NotReady,
    /// Every lake is open, leased as its handle knows it, and answers a head.
    Ready,
}

impl Health {
    /// A door that is starting: no lakes are open yet.
    pub fn new() -> Health {
        Health::default()
    }

    /// Every lake the host serves is open: readiness now follows them.
    pub fn serving(&self, lakes: Lakes) {
        *self.lakes.lock().unwrap_or_else(|e| e.into_inner()) = Some(lakes);
    }

    /// How ready the door is now.
    pub async fn readiness(&self) -> Readiness {
        let lakes = self.lakes.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let Some(lakes) = lakes else {
            return Readiness::Starting;
        };
        for lake in &lakes {
            if !lake.is_writer() || lake.head().await.is_err() {
                return Readiness::NotReady;
            }
        }
        Readiness::Ready
    }
}

pub(crate) struct State {
    health: Health,
    idle: Duration,
    pub(crate) feed_poll: Duration,
    pub(crate) feed_floor: Duration,
    pub(crate) origins: Vec<String>,
    pub(crate) resolver: Arc<dyn PrincipalResolver>,
    pub(crate) open: Arc<LakeOpener>,
    pub(crate) hosts: Vec<HostEntry>,
    pub(crate) gate: Arc<Gate>,
    sessions: Mutex<HashMap<String, Arc<Slot>>>,
}

/// A bound HTTP door, not yet serving.
pub struct HttpServer {
    listener: TcpListener,
    state: Arc<State>,
}

/// Bind the HTTP door. [`HttpServer::run`] then serves it.
pub async fn serve_http(
    config: HttpConfig,
    resolver: Arc<dyn PrincipalResolver>,
    open: Arc<LakeOpener>,
) -> io::Result<HttpServer> {
    (config.check(&[])).map_err(|why| io::Error::new(io::ErrorKind::InvalidInput, why))?;
    let listener = TcpListener::bind(config.bind).await?;
    Ok(HttpServer {
        listener,
        state: Arc::new(State {
            health: Health::new(),
            idle: Duration::from_secs(config.session_idle_secs),
            feed_poll: Duration::from_millis(config.feed_poll_millis),
            feed_floor: Duration::from_millis(config.feed_floor_millis),
            origins: config.allowed_origins,
            resolver,
            open,
            hosts: config.hosts,
            gate: Gate::new(config.bounds),
            sessions: Mutex::new(HashMap::new()),
        }),
    })
}

impl HttpServer {
    /// The address the door listens on (useful when bound to port 0).
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// The handle the host tells when its lakes are open, for `/readyz`.
    pub fn health(&self) -> Health {
        self.state.health.clone()
    }

    /// Serve connections until the task is dropped.
    pub async fn run(self) -> io::Result<()> {
        loop {
            let (stream, _) = self.listener.accept().await?;
            let state = self.state.clone();
            tokio::spawn(async move {
                // A connection's count lives as long as it does.
                let connection = Arc::new(Connection::new());
                let service = hyper::service::service_fn(move |req| {
                    let (state, connection) = (state.clone(), connection.clone());
                    async move { Ok::<_, Infallible>(bounded(&state, &connection, req).await) }
                });
                // A connection that fails affects only itself.
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    }
}

/// A JSON reply, whole.
pub(crate) fn json_response(status: StatusCode, v: &Value) -> Response<Full<Bytes>> {
    let mut r = Response::new(Full::new(Bytes::from(
        serde_json::to_vec(v).expect("JSON prints"),
    )));
    *r.status_mut() = status;
    r.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    r
}

fn respond(status: StatusCode, body: Option<&Value>) -> Response<Full<Bytes>> {
    match body {
        Some(v) => json_response(status, v),
        None => {
            let mut r = Response::new(Full::new(Bytes::new()));
            *r.status_mut() = status;
            r
        }
    }
}

/// An HTTP refusal, with a JSON-RPC error body saying why.
fn refuse(status: StatusCode, code: i64, message: &str) -> Response<Full<Bytes>> {
    respond(status, Some(&error(Value::Null, code, message)))
}

/// A header's value, when it is text.
pub(crate) fn header(h: &HeaderMap, name: impl hyper::header::AsHeaderName) -> Option<&str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

/// Why the door turned a request away before a lake answered it. Each
/// transport says it in its own envelope: MCP as a JSON-RPC error, REST
/// as the contract's refusal with its two sentences.
pub(crate) struct Turned {
    pub(crate) status: StatusCode,
    pub(crate) why: Turn,
}

/// What kind of turning away.
pub(crate) enum Turn {
    /// The caller is not let in, an origin or a key: what happened and
    /// what to do. It names no case; a refusal of `not_granted` says none.
    NotGranted(&'static str, &'static str),
    /// The request cannot be read as it is.
    Invalid(Case),
    /// A bound of the door's is reached: answered with `Retry-After`.
    Bounded(Over),
}

impl Turned {
    fn new(status: StatusCode, why: Turn) -> Turned {
        Turned { status, why }
    }

    /// Said as MCP says it, before the request's id is known.
    fn to_mcp(&self) -> Response<Full<Bytes>> {
        self.to_mcp_as(Value::Null)
    }

    /// Said as MCP says it, as the answer to the request `id`. A request
    /// that cannot be read, or that a bound refused, is a JSON-RPC error
    /// whose `data` is the refusal as REST says it, so its code reads the
    /// same.
    fn to_mcp_as(&self, id: Value) -> Response<Full<Bytes>> {
        match &self.why {
            Turn::NotGranted(what, _) => {
                respond(self.status, Some(&error(id, code::INVALID_REQUEST, *what)))
            }
            Turn::Invalid(case) => {
                let code = match case {
                    Case::BodyNotJson { .. } => code::PARSE,
                    _ => code::INVALID_REQUEST,
                };
                let mut v = error(id, code, case.to_string());
                v["error"]["data"] = crate::rest::invalid_json(case.clone());
                respond(self.status, Some(&v))
            }
            Turn::Bounded(over) => {
                let (e, data) = crate::rest::bounded_json(over);
                let mut v = error(id, code::BOUNDED, e.to_string());
                v["error"]["data"] = data;
                retry_after(respond(self.status, Some(&v)), over)
            }
        }
    }

    /// A request over one of `gate`'s bounds.
    fn bounded(over: Over) -> Turned {
        Turned::new(StatusCode::TOO_MANY_REQUESTS, Turn::Bounded(over))
    }
}

/// An answer to a request over a bound, with the seconds to wait.
pub(crate) fn retry_after<B>(mut r: Response<B>, over: &Over) -> Response<B> {
    r.headers_mut()
        .insert("retry-after", HeaderValue::from(over.retry_after));
    r
}

/// Let a request in: its `Origin`, if any, is allowed, and it presents an
/// API key. Answers the key.
pub(crate) fn admit(state: &State, h: &HeaderMap) -> std::result::Result<String, Turned> {
    if let Some(origin) = header(h, ORIGIN)
        && !state.origins.iter().any(|o| o == origin)
    {
        return Err(Turned::new(
            StatusCode::FORBIDDEN,
            Turn::NotGranted(
                "this origin may not call the server",
                "call from an origin the server's configuration allows",
            ),
        ));
    }
    header(h, AUTHORIZATION)
        .and_then(|a| a.strip_prefix("Bearer "))
        .map(str::to_owned)
        .ok_or_else(|| {
            Turned::new(
                StatusCode::UNAUTHORIZED,
                Turn::NotGranted(
                    "present an API key as `Authorization: Bearer <key>`",
                    "send the key the lake's owner gave you",
                ),
            )
        })
}

/// The principal a key stands for in a lake, and the request counted
/// against its bounds.
pub(crate) async fn resolve(
    state: &State,
    lake: &LakeId,
    key: String,
) -> std::result::Result<(Principal, Held), Turned> {
    let who = (state.resolver.resolve(lake, &Credential::ApiKey(key)).await).map_err(|_| {
        Turned::new(
            StatusCode::UNAUTHORIZED,
            Turn::NotGranted(
                "the key is not known for this lake",
                "send the key the lake's owner gave you",
            ),
        )
    })?;
    let held = state.gate.principal(&who).map_err(Turned::bounded)?;
    Ok((who, held))
}

/// The principal a host forwards a call for, by the id it names, the
/// write context it carries, and the request counted against the
/// principal's bounds: only for a lake and a principal the host may
/// forward for, and only one the resolver knows for the lake.
pub(crate) async fn forwarded(
    state: &State,
    host: &HostEntry,
    lake: &LakeId,
    h: &HeaderMap,
) -> std::result::Result<(Principal, WriteContext, Held), Turned> {
    let bad = |case| Turned::new(StatusCode::BAD_REQUEST, Turn::Invalid(case));
    let principal = header(h, PRINCIPAL_HEADER).ok_or_else(|| bad(Case::NoForwardedPrincipal))?;
    let context = match header(h, CONTEXT_HEADER) {
        Some(c) => serde_json::from_str(c).map_err(|_| bad(Case::ForwardedContextForm))?,
        None => WriteContext::default(),
    };
    let refused = || {
        Turned::new(
            StatusCode::FORBIDDEN,
            Turn::NotGranted(
                "the host may not forward a call to this lake as this principal",
                "forward only for a principal the server's configuration lets the host act for",
            ),
        )
    };
    if !host.may(lake, principal) {
        return Err(refused());
    }
    let credential = Credential::Forwarded {
        host: host.id.clone(),
        principal: principal.to_owned(),
    };
    let who = (state.resolver.resolve(lake, &credential).await).map_err(|_| refused())?;
    let held = state.gate.principal(&who).map_err(Turned::bounded)?;
    Ok((who, context, held))
}

/// A header of the link between hosts, sent with a principal's key: it
/// is refused, so no caller sets a write context or acts as another
/// (`docs/decisions/021-the-write-context-belongs-to-the-host.md`).
pub(crate) fn no_link_headers(h: &HeaderMap) -> std::result::Result<(), Turned> {
    match [PRINCIPAL_HEADER, CONTEXT_HEADER]
        .into_iter()
        .find(|n| h.contains_key(*n))
    {
        Some(header) => Err(Turned::new(
            StatusCode::BAD_REQUEST,
            Turn::Invalid(Case::HostHeaderFromCaller {
                header: header.into(),
            }),
        )),
        None => Ok(()),
    }
}

/// The largest body the door reads whole: [`Bounds::body_bytes`], which
/// [`bounded`] gives every request.
#[derive(Clone, Copy)]
struct BodyBound(Limit);

/// A JSON body, read whole: its media type, when named, is JSON, and it
/// is no larger than its request's [`BodyBound`], or the default bound
/// where the request has none. `None` when it is empty.
pub(crate) async fn read_json(
    parts: &hyper::http::request::Parts,
    body: Incoming,
) -> std::result::Result<Option<Value>, Turned> {
    let h = &parts.headers;
    if let Some(ct) = header(h, CONTENT_TYPE)
        && !ct.starts_with("application/json")
    {
        return Err(Turned::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Turn::Invalid(Case::BodyMediaType),
        ));
    }
    let bound = parts.extensions.get::<BodyBound>().map(|b| b.0);
    let most = match bound.unwrap_or(Bounds::default().body_bytes) {
        Limit::At(n) => usize::try_from(n.get()).unwrap_or(usize::MAX),
        Limit::Off => usize::MAX,
    };
    let bytes = match Limited::new(body, most).collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => {
            return Err(Turned::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                Turn::Invalid(Case::BodyTooLarge),
            ));
        }
    };
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    serde_json::from_slice(&bytes).map(Some).map_err(|e| {
        let (fault, row, column) = ((&e).into(), e.line(), e.column());
        Turned::new(
            StatusCode::BAD_REQUEST,
            Turn::Invalid(Case::BodyNotJson { fault, row, column }),
        )
    })
}

impl State {
    /// The sessions, with every idle one purged first.
    fn sessions(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Slot>>> {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.retain(|_, s| {
            let used = *s.last_used.lock().unwrap_or_else(|e| e.into_inner());
            let live = used.elapsed() < self.idle;
            if !live {
                s.end();
            }
            live
        });
        sessions
    }

    /// The session a caller names, if it exists and is theirs, and the
    /// request counted against its principal's bounds; a refusal answers
    /// the JSON-RPC `request` id. Another key's session is answered as
    /// unknown, so its existence does not leak.
    fn session(
        &self,
        id: &str,
        lake: &LakeId,
        key: &str,
        request: Value,
    ) -> std::result::Result<(Arc<Slot>, Held), Refusal> {
        let slot = (self.sessions().get(id))
            .filter(|s| s.owned_by(lake, key))
            .cloned()
            .ok_or_else(|| Box::new(unknown_session()))?;
        let held = (self.gate.principal(&slot.who))
            .map_err(|o| Box::new(Turned::bounded(o).to_mcp_as(request)))?;
        *slot.last_used.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
        Ok((slot, held))
    }
}

/// An MCP refusal, made before anything of the request was done.
type Refusal = Box<Response<Full<Bytes>>>;

fn unknown_session() -> Response<Full<Bytes>> {
    refuse(
        StatusCode::NOT_FOUND,
        code::INVALID_REQUEST,
        "there is no such session; initialize a new one",
    )
}

fn new_session_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the operating system gives random bytes");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `GET /livez` and `GET /readyz`: a status and a one-word body, for a
/// caller that holds no key. `HEAD` is answered like `GET`.
async fn probe(state: &State, req: &Request<Incoming>, ready: bool) -> Answer {
    if !matches!(*req.method(), Method::GET | Method::HEAD) {
        let mut r = respond(StatusCode::METHOD_NOT_ALLOWED, None);
        r.headers_mut()
            .insert("allow", HeaderValue::from_static("GET, HEAD"));
        return r.map(BodyExt::boxed);
    }
    let (status, word) = if !ready {
        (StatusCode::OK, "up")
    } else {
        match state.health.readiness().await {
            Readiness::Ready => (StatusCode::OK, "ready"),
            Readiness::NotReady => (StatusCode::SERVICE_UNAVAILABLE, "not_ready"),
            Readiness::Starting => (StatusCode::SERVICE_UNAVAILABLE, "starting"),
        }
    };
    let mut r = json_response(status, &json!({ "status": word }));
    r.headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    r.map(BodyExt::boxed)
}

/// Every request, through the door's bounds
/// (`docs/decisions/095-the-http-door-bounds-what-one-caller-spends.md`):
/// counted by its connection here, before anything is read, and given the
/// largest body it may send; counted by its principal where the door
/// resolves one. A request over a bound is answered in its transport's
/// envelope. A probe is not counted: an orchestrator that polls health
/// on a busy connection must not see the process as down.
async fn bounded(state: &State, connection: &Connection, mut req: Request<Incoming>) -> Answer {
    let path = req.uri().path().to_owned();
    let mcp = is_mcp(&path);
    let probe = matches!(path.as_str(), "/livez" | "/readyz");
    if !probe && let Err(over) = state.gate.connection(connection) {
        let t = Turned::bounded(over);
        return match mcp {
            true => t.to_mcp().map(BodyExt::boxed),
            false => crate::rest::refused(t),
        };
    }
    (req.extensions_mut()).insert(BodyBound(state.gate.bounds.body_bytes));
    handle(state, req, &path, mcp).await
}

/// Whether a path is MCP's: `/lakes/<id>/mcp`, or anything not under a
/// lake that is not a probe; every other path under a lake is REST.
fn is_mcp(path: &str) -> bool {
    match path.strip_prefix("/lakes/") {
        Some(rest) => !rest.contains('/') || rest.split_once('/').is_some_and(|(_, r)| r == "mcp"),
        None => !matches!(path, "/livez" | "/readyz"),
    }
}

async fn handle(state: &State, req: Request<Incoming>, path: &str, mcp: bool) -> Answer {
    match path {
        "/livez" => probe(state, &req, false).await,
        "/readyz" => probe(state, &req, true).await,
        _ if mcp => self::mcp(state, req).await,
        _ => {
            let rest = path
                .strip_prefix("/lakes/")
                .expect("a REST path is under a lake");
            crate::rest::handle(state, req, rest).await
        }
    }
}

async fn mcp(state: &State, req: Request<Incoming>) -> Answer {
    let key = match admit(state, req.headers()) {
        Ok(k) => k,
        Err(t) => return t.to_mcp().map(BodyExt::boxed),
    };
    let lake = match req
        .uri()
        .path()
        .strip_prefix("/lakes/")
        .and_then(|p| p.strip_suffix("/mcp"))
    {
        Some(id) if !id.is_empty() && !id.contains('/') => LakeId::from(id),
        _ => {
            return refuse(
                StatusCode::NOT_FOUND,
                code::INVALID_REQUEST,
                "MCP is served at /lakes/<id>/mcp",
            )
            .map(BodyExt::boxed);
        }
    };
    let answer = match *req.method() {
        Method::GET => return stream(state, &lake, &key, &req),
        Method::POST => post(state, lake, key, req).await,
        Method::DELETE => {
            let Some(id) = header(req.headers(), SESSION_HEADER).map(str::to_owned) else {
                return refuse(
                    StatusCode::BAD_REQUEST,
                    code::INVALID_REQUEST,
                    "name the session to end in Mcp-Session-Id",
                )
                .map(BodyExt::boxed);
            };
            match state.session(&id, &lake, &key, Value::Null) {
                Ok(_) => {
                    if let Some(slot) = state.sessions().remove(&id) {
                        slot.end();
                    }
                    respond(StatusCode::NO_CONTENT, None)
                }
                Err(r) => *r,
            }
        }
        _ => {
            let mut r = refuse(
                StatusCode::METHOD_NOT_ALLOWED,
                code::INVALID_REQUEST,
                "this server takes GET, POST and DELETE",
            );
            r.headers_mut()
                .insert("allow", HeaderValue::from_static("GET, POST, DELETE"));
            r
        }
    };
    answer.map(BodyExt::boxed)
}

/// `GET`: the session's event stream.
fn stream(state: &State, lake: &LakeId, key: &str, req: &Request<Incoming>) -> Answer {
    let h = req.headers();
    if let Some(accept) = header(h, ACCEPT)
        && !(accept.contains("text/event-stream") || accept.contains("*/*"))
    {
        return refuse(
            StatusCode::NOT_ACCEPTABLE,
            code::INVALID_REQUEST,
            "the event stream is text/event-stream",
        )
        .map(BodyExt::boxed);
    }
    let Some(id) = header(h, SESSION_HEADER) else {
        return refuse(
            StatusCode::BAD_REQUEST,
            code::INVALID_REQUEST,
            "name the session whose stream to open in Mcp-Session-Id",
        )
        .map(BodyExt::boxed);
    };
    // Only the session's own key opens its stream; another key's session is
    // answered as unknown, as for every request.
    let (slot, _held) = match state.session(id, lake, key, Value::Null) {
        Ok(s) => s,
        Err(r) => return (*r).map(BodyExt::boxed),
    };
    let resume = match header(h, "last-event-id").map(str::trim) {
        None => None,
        Some(id) => match id.parse::<u64>().ok().and_then(|p| p.checked_add(1)) {
            Some(p) => Some(Position(p)),
            None => {
                return refuse(
                    StatusCode::BAD_REQUEST,
                    code::INVALID_REQUEST,
                    "`Last-Event-ID` names no position after which to resume",
                )
                .map(BodyExt::boxed);
            }
        },
    };
    // A second stream replaces the first.
    let ends = Arc::new(Notify::new());
    let before = std::mem::replace(
        &mut *slot.stream.lock().unwrap_or_else(|e| e.into_inner()),
        ends.clone(),
    );
    before.notify_one();
    let (tx, rx) = mpsc::channel::<Bytes>(16);
    // The first bytes at once: the stream is open before anything changes.
    let _ = tx.try_send(Bytes::from_static(b": open\n\n"));
    tokio::spawn(forward(slot, ends, resume, tx));
    let mut r = crate::rest::streamed("text/event-stream", rx);
    r.headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    r
}

/// Carry a session's queued notifications onto its open stream, until the
/// stream closes or another replaces it.
async fn forward(
    slot: Arc<Slot>,
    ends: Arc<Notify>,
    resume: Option<Position>,
    tx: mpsc::Sender<Bytes>,
) {
    // The queue is held by one stream at a time: the one this replaced
    // has let go of it when this gets it.
    let mut outbox = tokio::select! {
        o = slot.outbox.lock() => o,
        _ = ends.notified() => return,
    };
    // Only now does the pump start again from after the resumed
    // position, so nothing it makes for the new subscription can reach
    // the stream this replaced. Changes queued before are dropped as
    // stale by their tag.
    if let Some(from) = resume {
        slot.watch.resume(from);
    }
    // A tools change the session has not listed since comes first: the
    // stream that took it may have closed before its client read it.
    if slot.watch.tools_owed() {
        let owed = json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"});
        if tx.send(event(&owed)).await.is_err() {
            return;
        }
    }
    loop {
        let bytes = tokio::select! {
            // A stream replaced takes nothing more from the queue.
            biased;
            _ = ends.notified() => return,
            _ = tx.closed() => return,
            m = outbox.recv() => match m {
                None => return,
                // Made for a subscription since replaced or ended.
                Some(q) if !slot.watch.current(&q) => continue,
                Some(q) => event(&q.message),
            },
            _ = tokio::time::sleep(crate::rest::KEEP_ALIVE) => Bytes::from_static(b": keep-alive\n\n"),
        };
        // A change comes back after `Last-Event-ID`, a tools change while
        // it is owed.
        if tx.send(bytes).await.is_err() {
            return;
        }
        // An open stream keeps its session in use.
        *slot.last_used.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
    }
}

/// One notification as a server-sent event; a change's `id` is its
/// position.
fn event(m: &Value) -> Bytes {
    let data = serde_json::to_string(m).expect("JSON prints");
    match m["params"]["position"].as_u64() {
        Some(p) => format!("id: {p}\ndata: {data}\n\n").into(),
        None => format!("data: {data}\n\n").into(),
    }
}

async fn post(
    state: &State,
    lake: LakeId,
    key: String,
    req: Request<Incoming>,
) -> Response<Full<Bytes>> {
    if let Some(accept) = header(req.headers(), ACCEPT)
        && !(accept.contains("application/json") || accept.contains("*/*"))
    {
        return refuse(
            StatusCode::NOT_ACCEPTABLE,
            code::INVALID_REQUEST,
            "this server answers application/json",
        );
    }
    if let Some(v) = header(req.headers(), VERSION_HEADER)
        && !PROTOCOL_VERSIONS.contains(&v)
    {
        return refuse(
            StatusCode::BAD_REQUEST,
            code::INVALID_REQUEST,
            "this server does not speak that protocol version",
        );
    }
    let session_id = header(req.headers(), SESSION_HEADER).map(str::to_owned);
    let (parts, body) = req.into_parts();
    let msg = match read_json(&parts, body).await {
        Ok(Some(v)) => v,
        Ok(None) => {
            return refuse(
                StatusCode::BAD_REQUEST,
                code::PARSE,
                "the body is empty; send one JSON-RPC message",
            );
        }
        Err(t) => return t.to_mcp(),
    };
    if msg.is_array() {
        return refuse(
            StatusCode::BAD_REQUEST,
            code::INVALID_REQUEST,
            "send one message per request; batches are not taken",
        );
    }
    let id = msg.get("id").cloned();

    if msg.get("method").and_then(Value::as_str) == Some("initialize") {
        let (who, _held) = match resolve(state, &lake, key.clone()).await {
            Ok(p) => p,
            // A principal's bound answers the request; a key not known,
            // as before, no id.
            Err(
                t @ Turned {
                    why: Turn::Bounded(_),
                    ..
                },
            ) => {
                return t.to_mcp_as(id.unwrap_or(Value::Null));
            }
            Err(t) => return t.to_mcp(),
        };
        let store = match (state.open)(&lake) {
            Ok(s) => s,
            Err(e) => {
                let text = crate::say::Failure::from(e).to_string();
                return respond(
                    StatusCode::SERVICE_UNAVAILABLE,
                    Some(&error(id.unwrap_or(Value::Null), code::INTERNAL, text)),
                );
            }
        };
        let mut session = Session::new(store, who.clone(), true);
        let answers = session.handle(msg).await;
        let answer = answers.first();
        let mut r = respond(StatusCode::OK, answer);
        // Only an initialize that answered a result starts a session.
        if answer.is_some_and(|a| a.get("result").is_some()) {
            let sid = new_session_id();
            let mut sessions = state.sessions();
            // Initializing again under a session of one's own replaces it.
            if let Some(old) = &session_id
                && sessions.get(old).is_some_and(|s| s.owned_by(&lake, &key))
                && let Some(slot) = sessions.remove(old)
            {
                slot.end();
            }
            let (tx, outbox) = mpsc::channel(QUEUE);
            let pump = tokio::spawn(session.pump(tx, state.feed_poll, state.feed_floor));
            let watch = session.watch();
            sessions.insert(
                sid.clone(),
                Arc::new(Slot {
                    lake,
                    key,
                    last_used: Mutex::new(Instant::now()),
                    session: tokio::sync::Mutex::new(session),
                    outbox: Arc::new(tokio::sync::Mutex::new(outbox)),
                    stream: Mutex::new(Arc::new(Notify::new())),
                    watch,
                    pump,
                    who,
                }),
            );
            r.headers_mut().insert(
                SESSION_HEADER,
                HeaderValue::from_str(&sid).expect("hex is a header value"),
            );
        }
        return r;
    }

    let Some(sid) = session_id else {
        return refuse(
            StatusCode::BAD_REQUEST,
            code::INVALID_REQUEST,
            "name the session in Mcp-Session-Id, or initialize one",
        );
    };
    let request = id.clone().unwrap_or(Value::Null);
    let (slot, _held) = match state.session(&sid, &lake, &key, request) {
        Ok(s) => s,
        Err(r) => return *r,
    };
    let answers = slot.session.lock().await.handle(msg).await;
    // The response to this request; notifications wait for a stream.
    let response = answers
        .into_iter()
        .find(|a| id.is_some() && a.get("id") == id.as_ref());
    match response {
        Some(r) => {
            let position = r["result"]["structuredContent"]["position"].as_u64();
            let mut out = respond(StatusCode::OK, Some(&r));
            if let Some(p) = position {
                out.headers_mut()
                    .insert(POSITION_HEADER, HeaderValue::from(p));
            }
            out
        }
        None => respond(StatusCode::ACCEPTED, None),
    }
}

#[cfg(test)]
mod tests;
