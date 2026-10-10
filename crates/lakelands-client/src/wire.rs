//! One exchange with the REST door: the request a door becomes, sent
//! with the key of the principal it is made as, and its answer decoded
//! into the contract's types or the contract's error.

use std::collections::BTreeMap;
use std::error::Error as _;
use std::io;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Empty, Full, LengthLimitError, Limited};
use hyper::body::Incoming;
use hyper::header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue};
use hyper::{Method, Request, Response, Uri};
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use lakelands_api::error::{
    Case, Detail, Error, Exchange, Refusal, Remote, Result, Segment, Situation,
};
use lakelands_api::id::{LakeId, Position};
use lakelands_api::principal::Principal;
use lakelands_api::read::POSITION_HEADER;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::connect::{Connector, tls};
use crate::{ApiKey, ConfigError, RemoteConfig};

/// A request body: whole, or streamed.
pub(crate) type Body = UnsyncBoxBody<Bytes, io::Error>;

/// The connection to one lake's REST door.
pub(crate) struct Wire {
    client: Client<Connector, Body>,
    /// `<scheme>://<authority><prefix>/lakes/<lake>`.
    base: String,
    /// The host and port, for the handle's `Debug` only: a failure names
    /// the lake's REST door, never its address.
    pub(crate) place: String,
    pub(crate) lake: LakeId,
    keys: BTreeMap<String, ApiKey>,
    feed_key: Option<String>,
    pub(crate) timeout: Duration,
    /// How long a streamed answer may stay silent.
    pub(crate) silence: Duration,
    /// The most bytes one answer, one event or one line may hold.
    pub(crate) cap: usize,
}

/// One request: the route under the lake, already encoded, and what it
/// carries.
pub(crate) struct Call {
    pub(crate) method: Method,
    /// Path segments under `…/lakes/<lake>/`, each percent-encoded.
    pub(crate) path: String,
    /// The query, already encoded, or empty.
    pub(crate) query: String,
    pub(crate) headers: Vec<(&'static str, String)>,
    pub(crate) body: Option<(&'static str, Body)>,
}

impl Call {
    pub(crate) fn new(method: Method, path: String) -> Call {
        Call {
            method,
            path,
            query: String::new(),
            headers: Vec::new(),
            body: None,
        }
    }

    pub(crate) fn query(mut self, query: String) -> Call {
        self.query = query;
        self
    }

    pub(crate) fn header(mut self, name: &'static str, value: String) -> Call {
        self.headers.push((name, value));
        self
    }

    /// A JSON body.
    pub(crate) fn json(mut self, v: &Value) -> Call {
        let bytes = serde_json::to_vec(v).expect("JSON prints");
        let body = Full::new(Bytes::from(bytes))
            .map_err(|never| match never {})
            .boxed_unsync();
        self.body = Some(("application/json", body));
        self
    }

    /// A streamed body of one media type.
    pub(crate) fn stream(mut self, media: &'static str, body: Body) -> Call {
        self.body = Some((media, body));
        self
    }
}

/// Encode one path segment or query component: everything but the
/// unreserved characters.
pub(crate) fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A JSON object's members as a query: text as it is, numbers and flags
/// as written, anything else as JSON; `null` left out.
pub(crate) fn query_of(v: &Value) -> String {
    let Some(o) = v.as_object() else {
        return String::new();
    };
    o.iter()
        .filter(|(_, v)| !v.is_null())
        .map(|(k, v)| {
            let text = match v {
                Value::String(s) => s.clone(),
                Value::Number(_) | Value::Bool(_) => v.to_string(),
                _ => v.to_string(),
            };
            format!("{}={}", enc(k), enc(&text))
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// One path segment, encoded. An empty one would name another route
/// (the server drops empty segments: `get` would become `find`), so it is
/// refused before anything is sent.
pub(crate) fn seg(value: &str, part: Segment) -> Result<String> {
    if value.is_empty() {
        return Err(Error::invalid(Case::SegmentEmpty { part }));
    }
    Ok(enc(value))
}

/// Refused before anything is sent.
pub(crate) fn not_granted() -> Error {
    Error::Refused {
        field: None,
        reason: Refusal::NotGranted,
    }
}

/// The pooled client every exchange goes through, over `connector`.
fn pooled(connector: Connector) -> Client<Connector, Body> {
    Client::builder(TokioExecutor::new()).build(connector)
}

impl Wire {
    pub(crate) fn new(
        config: &RemoteConfig,
        silence: Duration,
    ) -> std::result::Result<Wire, ConfigError> {
        let bad = |what: &str| ConfigError(format!("the lake's URL {what}"));
        let uri: Uri = config
            .url
            .parse()
            .map_err(|_| bad("is not a URL: give `http://host:port/` or `https://host/`"))?;
        let secure = match uri.scheme_str() {
            Some("https") => true,
            Some("http") => false,
            _ => return Err(bad("must be `http` or `https`")),
        };
        let authority = uri
            .authority()
            .ok_or_else(|| bad("names no host"))?
            .as_str();
        // A key belongs in `keys`, never in the URL, where messages and
        // logs would carry it.
        if authority.contains('@') {
            return Err(bad(
                "holds user-info; give keys in `keys`, never in the URL",
            ));
        }
        if uri.query().is_some() || config.url.contains('#') {
            return Err(bad("holds a query or a fragment; give the base URL only"));
        }
        let host = uri.host().ok_or_else(|| bad("names no host"))?;
        let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
        let prefix = uri.path().trim_end_matches('/').to_owned();
        let tls = if secure {
            Some(tls(config.ca_file.as_deref()).map_err(ConfigError)?)
        } else {
            None
        };
        if config.lake.as_str().is_empty() || config.lake.as_str().contains('/') {
            return Err(ConfigError(
                "a lake's id is one path segment, not empty and without `/`".into(),
            ));
        }
        if config.max_answer_bytes == 0 {
            return Err(ConfigError("`max_answer_bytes` must be more than 0".into()));
        }
        // A key goes in one header; one that cannot is said here, naming
        // whose it is and never the key.
        if let Some(id) = config
            .keys
            .iter()
            .find(|(_, k)| HeaderValue::from_str(&format!("Bearer {}", k.expose())).is_err())
            .map(|(id, _)| id)
        {
            return Err(ConfigError(format!(
                "the key of `{id}` holds a character a header cannot carry"
            )));
        }
        if let Some(id) = &config.feed_key
            && !config.keys.contains_key(id)
        {
            return Err(ConfigError(format!(
                "`feed_key` names `{id}`, which has no key in `keys`"
            )));
        }
        // The bound is waited out in quarters (`doors`), and a quarter under
        // the timer's step of 1 ms would find every timer late: nothing
        // would ever be judged silent. The door's bound is 30 s; only a
        // test gives another.
        if silence < Duration::from_millis(4) {
            return Err(ConfigError("the silence bound must be 4 ms or more".into()));
        }
        let client = pooled(Connector::new(tls));
        let scheme = if secure { "https" } else { "http" };
        Ok(Wire {
            client,
            base: format!(
                "{scheme}://{authority}{prefix}/lakes/{}",
                enc(config.lake.as_str())
            ),
            place: format!("{host}:{port}"),
            lake: config.lake.clone(),
            keys: config.keys.clone(),
            feed_key: config.feed_key.clone(),
            timeout: Duration::from_secs(config.timeout_secs.max(1)),
            silence,
            cap: config.max_answer_bytes,
        })
    }

    /// This wire over a connection a test holds the other end of.
    #[cfg(test)]
    pub(crate) fn over(mut self, conn: tokio::io::DuplexStream) -> Wire {
        self.client = pooled(Connector::over(conn));
        self
    }

    /// The key `who` acts with: refused before anything is sent when
    /// there is none, or when `who` is of another lake.
    pub(crate) fn key_of(&self, who: &Principal) -> Result<&ApiKey> {
        if who.lake != self.lake {
            return Err(not_granted());
        }
        self.keys.get(&who.id).ok_or_else(not_granted)
    }

    /// The key the feed and the head are read with.
    pub(crate) fn feed_key(&self) -> Result<&ApiKey> {
        let id = self.feed_key.as_ref().ok_or_else(not_granted)?;
        self.keys.get(id).ok_or_else(not_granted)
    }

    /// A failure below the contract, naming the lake and what, never its
    /// address, a path, a key or a body.
    pub(crate) fn unavailable(&self, exchange: Exchange) -> Error {
        let lake = self.lake.to_string();
        let remote = Remote::Door { lake };
        Error::unavailable(Situation::Remote { remote, exchange })
    }

    /// Send one call and wait for the head of its answer, within the
    /// exchange's timeout.
    pub(crate) async fn send(&self, key: &ApiKey, call: Call) -> Result<Response<Incoming>> {
        let mut uri = format!("{}/{}", self.base, call.path);
        if !call.query.is_empty() {
            uri.push('?');
            uri.push_str(&call.query);
        }
        let mut req = Request::builder().method(call.method).uri(uri);
        for (name, value) in &call.headers {
            req = req.header(*name, value);
        }
        // Every key was checked at `connect`.
        let mut bearer = HeaderValue::from_str(&format!("Bearer {}", key.expose()))
            .map_err(|_| self.unavailable(Exchange::KeyNotSent))?;
        bearer.set_sensitive(true);
        req = req.header(AUTHORIZATION, bearer);
        let body = match call.body {
            Some((media, body)) => {
                req = req.header(CONTENT_TYPE, media);
                body
            }
            None => Empty::new().map_err(|never| match never {}).boxed_unsync(),
        };
        let req = req
            .body(body)
            .map_err(|_| self.unavailable(Exchange::NotSent))?;
        match tokio::time::timeout(self.timeout, self.client.request(req)).await {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(e)) => Err(self.unavailable(failed(&e))),
            Err(_) => {
                let secs = self.timeout.as_secs();
                Err(self.unavailable(Exchange::TimedOut { secs }))
            }
        }
    }

    /// Send one call and decode its whole answer: the contract's value,
    /// or the contract's error.
    pub(crate) async fn exchange<T: DeserializeOwned>(
        &self,
        key: &ApiKey,
        call: Call,
    ) -> Result<T> {
        let r = self.send(key, call).await?;
        let status = r.status();
        let bytes = self.whole(r.into_body()).await?;
        if status.is_success() {
            serde_json::from_slice(&bytes).map_err(|_| {
                let status = Some(status.as_u16());
                self.unavailable(Exchange::NotTheContract { status })
            })
        } else {
            Err(self.error_of(status, &bytes))
        }
    }

    /// A whole answer's body: at most `cap` bytes, within the exchange's
    /// timeout.
    pub(crate) async fn whole(&self, body: Incoming) -> Result<Bytes> {
        let read = Limited::new(body, self.cap).collect();
        match tokio::time::timeout(self.timeout, read).await {
            Ok(Ok(b)) => Ok(b.to_bytes()),
            Ok(Err(e)) if e.is::<LengthLimitError>() => Err(self.too_long()),
            Ok(Err(e)) => {
                let detail = Detail::scrubbed(e);
                Err(self.unavailable(Exchange::CutOff { detail }))
            }
            Err(_) => {
                let secs = self.timeout.as_secs();
                Err(self.unavailable(Exchange::NoWholeAnswer { secs }))
            }
        }
    }

    /// An answer, an event or a line longer than the cap.
    pub(crate) fn too_long(&self) -> Error {
        let cap = self.cap as u64;
        self.unavailable(Exchange::TooLarge { cap })
    }

    /// The contract's error a failed answer carries, read whole.
    pub(crate) async fn failure(&self, r: Response<Incoming>) -> Error {
        let status = r.status();
        match self.whole(r.into_body()).await {
            Ok(bytes) => self.error_of(status, &bytes),
            Err(e) => e,
        }
    }

    /// The contract's error a failed answer carries.
    pub(crate) fn error_of(&self, status: hyper::StatusCode, bytes: &[u8]) -> Error {
        serde_json::from_slice::<Error>(bytes).unwrap_or_else(|_| {
            let status = status.as_u16();
            self.unavailable(Exchange::UnexpectedStatus { status })
        })
    }

    /// The head: the position a `describe` is answered at.
    pub(crate) async fn head(&self) -> Result<Position> {
        let key = self.feed_key()?;
        let path = "describe".to_owned();
        let r = self.send(key, Call::new(Method::GET, path.clone())).await?;
        if r.status().is_success()
            && let Some(p) = position_of(&r)
        {
            return Ok(p);
        }
        Err(self.failure(r).await)
    }
}

/// The `Lakelands-Position` of an answer.
pub(crate) fn position_of<B>(r: &Response<B>) -> Option<Position> {
    r.headers()
        .get(POSITION_HEADER)?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
        .map(Position)
}

/// What failed below HTTP, in words that carry no key and no body.
fn failed(e: &hyper_util::client::legacy::Error) -> Exchange {
    let mut source = e.source();
    while let Some(s) = source {
        if let Some(io) = s.downcast_ref::<io::Error>() {
            return match io.kind() {
                io::ErrorKind::ConnectionRefused => Exchange::ConnectionRefused,
                io::ErrorKind::InvalidData => Exchange::TlsBroke {
                    detail: io.get_ref().map(Detail::scrubbed),
                },
                _ => Exchange::Unreachable {
                    detail: Some(Detail::io(io)),
                },
            };
        }
        source = s.source();
    }
    if e.is_connect() {
        Exchange::Unreachable { detail: None }
    } else {
        Exchange::Closed
    }
}
