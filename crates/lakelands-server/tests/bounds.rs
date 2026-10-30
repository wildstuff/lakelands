//! The HTTP door's bounds (lakelands#1523): a connection and a principal
//! may send so many requests a second, a principal have so many answered
//! at once, and a body be so large. Over a rate or the requests in
//! flight, a request is answered `429` with `Retry-After` and the
//! refusal of the catalogue, by MCP and REST alike; over the size,
//! `413`. Each principal is counted apart, a request that ends gives its
//! slot back, and a bound set `"off"` bounds nothing.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use common::Backend;
use lakelands_api::LakeStore;
use lakelands_api::location::Limits;
use lakelands_server::{Bounds, HttpConfig, Limit, serve_http};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// A wait for an answer that should come: past it, a hang is a failure.
const WAIT: Duration = Duration::from_secs(60);

fn at(n: u64) -> Limit {
    Limit::At(n.try_into().unwrap())
}

/// The fixture lake behind a door with these bounds.
async fn serve(bounds: Bounds) -> SocketAddr {
    let store = common::lake_with(Backend::Memory, Limits::default()).await;
    let open = Arc::new(
        move |_: &lakelands_api::LakeId| -> lakelands_api::Result<Arc<dyn LakeStore>> {
            Ok(store.clone())
        },
    );
    let config = HttpConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        allowed_origins: Vec::new(),
        session_idle_secs: 1800,
        feed_poll_millis: 20,
        feed_floor_millis: 25,
        workers: None,
        metrics: None,
        hosts: Vec::new(),
        bounds,
    };
    let server = serve_http(config, common::resolver(), open).await.unwrap();
    let addr = server.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = server.run().await;
    });
    addr
}

/// One answer: its status, its headers (names in lower case) and its body.
struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: Value,
}

impl Answer {
    fn header(&self, name: &str) -> Option<&str> {
        (self.headers.iter())
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// The refusal's code, as REST says it or as MCP's error `data` does.
    fn code(&self) -> &Value {
        match self.body.get("error") {
            Some(e) if e.is_object() => &e["data"]["situation"]["code"],
            _ => &self.body["situation"]["code"],
        }
    }
}

/// A request as bytes: `method` at `path` with `key`, and a JSON body.
fn request(method: &str, path: &str, key: Option<&str>, body: Option<&Value>) -> String {
    let body = body.map(Value::to_string).unwrap_or_default();
    let key = key.map_or_else(String::new, |k| format!("authorization: Bearer {k}\r\n"));
    format!(
        "{method} {path} HTTP/1.1\r\nhost: lake\r\n{key}content-type: application/json\r\n\
         accept: application/json\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )
}

/// Send `requests` one after another on one connection, and answer each.
async fn on_one(addr: SocketAddr, requests: &[String]) -> Vec<Answer> {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut answers = Vec::new();
    for r in requests {
        stream.write_all(r.as_bytes()).await.unwrap();
        answers.push(read_one(&mut stream).await);
    }
    answers
}

/// Read one answer whose body has a length.
async fn read_one(stream: &mut TcpStream) -> Answer {
    let mut raw = Vec::new();
    let mut byte = [0u8; 1];
    while !raw.ends_with(b"\r\n\r\n") {
        let n = tokio::time::timeout(WAIT, stream.read(&mut byte))
            .await
            .expect("an answer within the wait")
            .unwrap();
        assert_eq!(n, 1, "the connection closed before its answer");
        raw.push(byte[0]);
    }
    let head = String::from_utf8(raw).unwrap();
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let length: usize = (headers.iter())
        .find(|(n, _)| n == "content-length")
        .map_or(0, |(_, v)| v.parse().unwrap());
    let mut body = vec![0u8; length];
    tokio::time::timeout(WAIT, stream.read_exact(&mut body))
        .await
        .expect("a body within the wait")
        .unwrap();
    let body = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap()
    };
    Answer {
        status,
        headers,
        body,
    }
}

/// A request on a connection of its own.
async fn once(addr: SocketAddr, r: String) -> Answer {
    on_one(addr, &[r]).await.remove(0)
}

fn describe(key: &str) -> String {
    request("GET", "/lakes/finance/describe", Some(key), None)
}

fn initialize(key: &str) -> String {
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "bounds", "version": "0"}}});
    request("POST", "/lakes/finance/mcp", Some(key), Some(&init))
}

/// A refusal of a rate: `429`, a `Retry-After` of a whole second or more,
/// and `rate_bound` naming what it counts by.
fn is_rate_bound(a: &Answer, per: &str) {
    assert_eq!(a.status, 429, "{}", a.body);
    let wait: u64 = a
        .header("retry-after")
        .expect("Retry-After")
        .parse()
        .unwrap();
    assert!(wait >= 1, "Retry-After: {wait}");
    assert_eq!(a.code(), &json!("rate_bound"), "{}", a.body);
    let situation = match a.body.get("error") {
        Some(e) if e.is_object() => &e["data"]["situation"],
        _ => &a.body["situation"],
    };
    assert_eq!(situation["per"], json!(per));
}

#[tokio::test]
async fn a_connection_over_its_rate_is_answered_429_with_retry_after() {
    let addr = serve(Bounds {
        connection_rate: at(2),
        ..Bounds::OFF
    })
    .await;
    let answers = on_one(
        addr,
        &[
            describe("k-owner"),
            describe("k-owner"),
            describe("k-owner"),
        ],
    )
    .await;
    assert_eq!(answers[0].status, 200);
    assert_eq!(answers[1].status, 200);
    is_rate_bound(&answers[2], "connection");
    // Another connection has a count of its own.
    assert_eq!(once(addr, describe("k-owner")).await.status, 200);
    // MCP says it as a JSON-RPC error whose `data` is the refusal.
    let mcp = on_one(
        addr,
        &[
            initialize("k-owner"),
            initialize("k-owner"),
            initialize("k-owner"),
        ],
    )
    .await;
    assert_eq!(mcp[1].status, 200);
    is_rate_bound(&mcp[2], "connection");
}

#[tokio::test]
async fn a_principal_over_its_rate_is_refused_and_another_is_not() {
    let addr = serve(Bounds {
        principal_rate: at(2),
        ..Bounds::OFF
    })
    .await;
    // Each request on a connection of its own: the principal is counted
    // over all of them.
    let mut answers = Vec::new();
    for _ in 0..3 {
        answers.push(once(addr, describe("k-owner")).await);
    }
    assert_eq!(answers[0].status, 200);
    assert_eq!(answers[1].status, 200);
    is_rate_bound(&answers[2], "principal");
    // The clerk's flood is the clerk's alone.
    assert_eq!(once(addr, describe("k-clerk")).await.status, 200);
    is_rate_bound(&once(addr, initialize("k-owner")).await, "principal");
    assert_eq!(once(addr, initialize("k-clerk")).await.status, 200);
}

#[tokio::test]
async fn a_request_refused_before_its_principal_is_known_counts_for_none() {
    let addr = serve(Bounds {
        principal_rate: at(1),
        ..Bounds::OFF
    })
    .await;
    for _ in 0..3 {
        let a = once(addr, request("GET", "/lakes/finance/describe", None, None)).await;
        assert_eq!(a.status, 401, "{}", a.body);
        let a = once(addr, describe("k-unknown")).await;
        assert_eq!(a.status, 401, "{}", a.body);
    }
    assert_eq!(once(addr, describe("k-owner")).await.status, 200);
}

/// A `POST …/query` whose body has not come: its request is in flight
/// until it does, or until the connection closes.
async fn hold(addr: SocketAddr, key: &str) -> TcpStream {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let head = format!(
        "POST /lakes/finance/query HTTP/1.1\r\nhost: lake\r\nauthorization: Bearer {key}\r\n\
         content-type: application/json\r\ncontent-length: 1000\r\n\r\n{{"
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    stream
}

/// The next request of `key`'s that is refused for the requests in
/// flight, within the wait: the held one is counted once the door has
/// resolved it.
async fn refused_in_flight(addr: SocketAddr, key: &str) -> Answer {
    let until = tokio::time::Instant::now() + WAIT;
    loop {
        let a = once(addr, describe(key)).await;
        if a.status == 429 || tokio::time::Instant::now() > until {
            return a;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn a_principal_with_its_most_in_flight_is_refused_until_one_ends() {
    let addr = serve(Bounds {
        principal_in_flight: at(1),
        ..Bounds::OFF
    })
    .await;
    let held = hold(addr, "k-owner").await;
    let a = refused_in_flight(addr, "k-owner").await;
    assert_eq!(a.status, 429, "{}", a.body);
    assert!(a.header("retry-after").is_some(), "Retry-After");
    assert_eq!(a.code(), &json!("in_flight_bound"), "{}", a.body);
    assert_eq!(a.body["situation"]["most"], json!(1));
    // Another principal is counted apart.
    assert_eq!(once(addr, describe("k-clerk")).await.status, 200);
    // The held request's client goes away: its slot comes back.
    drop(held);
    let until = tokio::time::Instant::now() + WAIT;
    loop {
        let a = once(addr, describe("k-owner")).await;
        if a.status == 200 {
            break;
        }
        assert_eq!(a.status, 429, "{}", a.body);
        assert!(
            tokio::time::Instant::now() < until,
            "the slot never came back"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn a_body_over_the_bound_is_answered_413() {
    let addr = serve(Bounds {
        body_bytes: at(64),
        ..Bounds::OFF
    })
    .await;
    let large = json!({"query": "x".repeat(200)});
    let a = once(
        addr,
        request(
            "POST",
            "/lakes/finance/query",
            Some("k-owner"),
            Some(&large),
        ),
    )
    .await;
    assert_eq!(a.status, 413, "{}", a.body);
    assert_eq!(
        a.body["reason"]["code"],
        json!("body_too_large"),
        "{}",
        a.body
    );
    let mcp =
        json!({"jsonrpc": "2.0", "id": 1, "method": "ping", "params": {"pad": "x".repeat(200)}});
    let a = once(
        addr,
        request("POST", "/lakes/finance/mcp", Some("k-owner"), Some(&mcp)),
    )
    .await;
    assert_eq!(a.status, 413, "{}", a.body);
    let code = &a.body["error"]["data"]["reason"]["code"];
    assert_eq!(code, &json!("body_too_large"), "{}", a.body);
    // A small one is read.
    let small = json!({"query": "MATCH (i:invoice) RETURN i.key"});
    let a = once(
        addr,
        request(
            "POST",
            "/lakes/finance/query",
            Some("k-owner"),
            Some(&small),
        ),
    )
    .await;
    assert_ne!(a.status, 413, "{}", a.body);
}

#[tokio::test]
async fn bounds_set_off_bound_nothing() {
    let addr = serve(Bounds::OFF).await;
    let many: Vec<String> = (0..50).map(|_| describe("k-owner")).collect();
    for a in on_one(addr, &many).await {
        assert_eq!(a.status, 200, "{}", a.body);
    }
    let _held = hold(addr, "k-owner").await;
    let _also = hold(addr, "k-owner").await;
    assert_eq!(once(addr, describe("k-owner")).await.status, 200);
    // Larger than the default bound on a body, and still read.
    let large = json!({"query": "x".repeat(5 << 20)});
    let a = once(
        addr,
        request(
            "POST",
            "/lakes/finance/query",
            Some("k-owner"),
            Some(&large),
        ),
    )
    .await;
    assert_ne!(a.status, 413, "{}", a.body);
}

#[tokio::test]
async fn the_defaults_bound_what_no_waiting_caller_meets() {
    let addr = serve(Bounds::default()).await;
    let many: Vec<String> = (0..100).map(|_| describe("k-owner")).collect();
    for a in on_one(addr, &many).await {
        assert_eq!(a.status, 200, "{}", a.body);
    }
    let large = json!({"query": "x".repeat(5 << 20)});
    let a = once(
        addr,
        request(
            "POST",
            "/lakes/finance/query",
            Some("k-owner"),
            Some(&large),
        ),
    )
    .await;
    assert_eq!(a.status, 413, "{}", a.body);
}
