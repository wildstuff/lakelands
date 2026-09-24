//! MCP over streamable HTTP, at `POST /lakes/<id>/mcp`.
//!
//! - `initialize` resolves the caller's API key (`Authorization: Bearer
//!   <key>`) to a principal through the resolver, opens the lake, starts
//!   a session and answers its id in `Mcp-Session-Id`. Every later
//!   request names that session and presents the same key.
//! - A request is answered with one JSON response; a notification or a
//!   client response with `202 Accepted`. The server offers no SSE
//!   stream yet, so `GET` is `405`; the session already hands back every
//!   message it would stream.
//! - `DELETE` with the session's id ends it, and a session unused for
//!   `session_idle_secs` ends by itself. A session is its lake's and its
//!   key's: named by anyone else, it is answered as unknown.
//! - A request whose `Origin` is not on the configured allow-list is
//!   refused with `403`, before anything else is read.
//! - A tool result's position is also sent in the `Lakelands-Position`
//!   header.
//!
//! Everything is configured through [`HttpConfig`], which the root's
//! `lakelands.json` carries; nothing is read from the environment.

use std::collections::HashMap;
use std::convert::Infallible;
use std::io;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue, ORIGIN};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use lakelands_api::LakeStore;
use lakelands_api::error::Result;
use lakelands_api::id::LakeId;
use lakelands_api::read::POSITION_HEADER;
use lakelands_api::seam::{Credential, PrincipalResolver};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::net::TcpListener;

use crate::resolver::same_key;
use crate::session::{PROTOCOL_VERSIONS, Session, code, error};

/// The header a session's id travels in.
pub const SESSION_HEADER: &str = "mcp-session-id";
/// The header a client names its protocol version in after `initialize`.
pub const VERSION_HEADER: &str = "mcp-protocol-version";
/// The largest request body read.
const MAX_BODY: usize = 4 << 20;

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
}

fn default_idle() -> u64 {
    30 * 60
}

/// Opens a lake by its id, for a new session.
pub type LakeOpener = dyn Fn(&LakeId) -> Result<Arc<dyn LakeStore>> + Send + Sync;

struct Slot {
    lake: LakeId,
    key: String,
    last_used: Mutex<Instant>,
    session: tokio::sync::Mutex<Session>,
}

impl Slot {
    /// Whether this session is one the caller may use: its lake, its key.
    fn owned_by(&self, lake: &LakeId, key: &str) -> bool {
        &self.lake == lake && same_key(&self.key, key)
    }
}

struct State {
    idle: Duration,
    origins: Vec<String>,
    resolver: Arc<dyn PrincipalResolver>,
    open: Arc<LakeOpener>,
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
    let listener = TcpListener::bind(config.bind).await?;
    Ok(HttpServer {
        listener,
        state: Arc::new(State {
            idle: Duration::from_secs(config.session_idle_secs),
            origins: config.allowed_origins,
            resolver,
            open,
            sessions: Mutex::new(HashMap::new()),
        }),
    })
}

impl HttpServer {
    /// The address the door listens on (useful when bound to port 0).
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Serve connections until the task is dropped.
    pub async fn run(self) -> io::Result<()> {
        loop {
            let (stream, _) = self.listener.accept().await?;
            let state = self.state.clone();
            tokio::spawn(async move {
                let service = hyper::service::service_fn(move |req| {
                    let state = state.clone();
                    async move { Ok::<_, Infallible>(handle(&state, req).await) }
                });
                // A connection that fails affects only itself.
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    }
}

fn respond(status: StatusCode, body: Option<&Value>) -> Response<Full<Bytes>> {
    let mut r = Response::new(Full::new(match body {
        Some(v) => Bytes::from(serde_json::to_vec(v).expect("JSON prints")),
        None => Bytes::new(),
    }));
    *r.status_mut() = status;
    if body.is_some() {
        r.headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    }
    r
}

/// An HTTP refusal, with a JSON-RPC error body saying why.
fn refuse(status: StatusCode, code: i64, message: &str) -> Response<Full<Bytes>> {
    respond(status, Some(&error(Value::Null, code, message)))
}

fn header(req: &Request<Incoming>, name: impl hyper::header::AsHeaderName) -> Option<&str> {
    req.headers().get(name).and_then(|v| v.to_str().ok())
}

impl State {
    /// The sessions, with every idle one purged first.
    fn sessions(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<Slot>>> {
        let mut sessions = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        sessions.retain(|_, s| {
            let used = *s.last_used.lock().unwrap_or_else(|e| e.into_inner());
            used.elapsed() < self.idle
        });
        sessions
    }

    /// The session a caller names, if it exists and is theirs. Another
    /// key's session is answered as unknown, so its existence does not
    /// leak.
    fn session(&self, id: &str, lake: &LakeId, key: &str) -> Option<Arc<Slot>> {
        let slot = self
            .sessions()
            .get(id)
            .filter(|s| s.owned_by(lake, key))
            .cloned()?;
        *slot.last_used.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
        Some(slot)
    }
}

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

async fn handle(state: &State, req: Request<Incoming>) -> Response<Full<Bytes>> {
    if let Some(origin) = header(&req, ORIGIN)
        && !state.origins.iter().any(|o| o == origin)
    {
        return refuse(
            StatusCode::FORBIDDEN,
            code::INVALID_REQUEST,
            "this origin may not call the server",
        );
    }
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
            );
        }
    };
    let Some(key) = header(&req, AUTHORIZATION)
        .and_then(|a| a.strip_prefix("Bearer "))
        .map(str::to_owned)
    else {
        return refuse(
            StatusCode::UNAUTHORIZED,
            code::INVALID_REQUEST,
            "present an API key as `Authorization: Bearer <key>`",
        );
    };
    match *req.method() {
        Method::POST => post(state, lake, key, req).await,
        Method::DELETE => {
            let Some(id) = header(&req, SESSION_HEADER).map(str::to_owned) else {
                return refuse(
                    StatusCode::BAD_REQUEST,
                    code::INVALID_REQUEST,
                    "name the session to end in Mcp-Session-Id",
                );
            };
            match state.session(&id, &lake, &key) {
                Some(_) => {
                    state.sessions().remove(&id);
                    respond(StatusCode::NO_CONTENT, None)
                }
                None => unknown_session(),
            }
        }
        _ => {
            let mut r = refuse(
                StatusCode::METHOD_NOT_ALLOWED,
                code::INVALID_REQUEST,
                "this server takes POST and DELETE; it offers no event stream yet",
            );
            r.headers_mut()
                .insert("allow", HeaderValue::from_static("POST, DELETE"));
            r
        }
    }
}

async fn post(
    state: &State,
    lake: LakeId,
    key: String,
    req: Request<Incoming>,
) -> Response<Full<Bytes>> {
    if let Some(accept) = header(&req, ACCEPT)
        && !(accept.contains("application/json") || accept.contains("*/*"))
    {
        return refuse(
            StatusCode::NOT_ACCEPTABLE,
            code::INVALID_REQUEST,
            "this server answers application/json",
        );
    }
    if let Some(ct) = header(&req, CONTENT_TYPE)
        && !ct.starts_with("application/json")
    {
        return refuse(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            code::INVALID_REQUEST,
            "send the message as application/json",
        );
    }
    if let Some(v) = header(&req, VERSION_HEADER)
        && !PROTOCOL_VERSIONS.contains(&v)
    {
        return refuse(
            StatusCode::BAD_REQUEST,
            code::INVALID_REQUEST,
            "this server does not speak that protocol version",
        );
    }
    let session_id = header(&req, SESSION_HEADER).map(str::to_owned);
    let body = match Limited::new(req.into_body(), MAX_BODY).collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => {
            return refuse(
                StatusCode::PAYLOAD_TOO_LARGE,
                code::INVALID_REQUEST,
                "the message is too large or was cut off",
            );
        }
    };
    let msg: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            return refuse(
                StatusCode::BAD_REQUEST,
                code::PARSE,
                &format!("the message is not JSON: {e}"),
            );
        }
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
        let who = match state
            .resolver
            .resolve(&lake, &Credential::ApiKey(key.clone()))
            .await
        {
            Ok(p) => p,
            Err(_) => {
                return refuse(
                    StatusCode::UNAUTHORIZED,
                    code::INVALID_REQUEST,
                    "the key is not known for this lake",
                );
            }
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
        let mut session = Session::new(store, who, false);
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
            {
                sessions.remove(old);
            }
            sessions.insert(
                sid.clone(),
                Arc::new(Slot {
                    lake,
                    key,
                    last_used: Mutex::new(Instant::now()),
                    session: tokio::sync::Mutex::new(session),
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
    let Some(slot) = state.session(&sid, &lake, &key) else {
        return unknown_session();
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
