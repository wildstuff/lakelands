//! REST over the same catalog, as recorded transcripts on the mock and
//! on the store over both engines, and the change feed as server-sent
//! events.
//!
//! `tests/transcripts/rest.txt` holds requests (`>`: the request line,
//! then headers, then body lines) and the answers expected (`<`: the
//! status, the pinned headers, then the body, one line per NDJSON line).
//! Proposal ids and schema hashes are the log format's own values on the
//! mock and the store alike, and are compared as they are. A migration
//! report's id and a page cursor are opaque and minted differently, so
//! the last ones answered are shown as `{report_id}` and `{cursor}`, and
//! a request may name them so; the last proposal answered may be named
//! as `{proposal}` in a request. A request without a `content-type`
//! line is sent as `application/json`; `content-type: (none)` sends its
//! body with no `Content-Type` at all.
//!
//! The lake here commits ingests in windows of one record, so window
//! boundaries are in the transcript, and declares one verb whose executor
//! no host binds. The wire shows one `Committed` line per window whatever
//! it holds; how a window fills is the store's, held in process by the
//! suite. A window of one closes at its record, so the transcript does not
//! depend on how fast a body's next line arrives (lakelands#807).

mod common;

use common::{BACKENDS, Backend, check, path};

use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;

use lakelands_api::id::Position;
use lakelands_api::location::Limits;
use lakelands_api::ontology::{Risk, VerbDecl, VerbEffect};
use lakelands_api::seam::FeedFilter;
use lakelands_api::write::{Declaration, WriteOpts};
use lakelands_api::{LakeStore, WriteContext};
use lakelands_server::{HttpConfig, serve_http};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// A wait for an answer that should come: past it, a hang is a failure.
/// It orders nothing, and a loaded machine may take seconds to schedule
/// the server, so it is the load driver's WAIT (lakelands#158).
const WAIT: std::time::Duration = std::time::Duration::from_secs(60);

/// The headers a transcript pins.
const PINNED: [&str; 5] = [
    "allow:",
    "content-type:",
    "etag:",
    "lakelands-position:",
    "cache-control:",
];

/// The fixture lake as REST serves it here: ingest windows of one
/// record, and an external verb `notify` with no executor bound.
async fn rest_lake(backend: Backend) -> Arc<dyn LakeStore> {
    let limits = Limits {
        commit_window_events: 1,
        ..Limits::default()
    };
    let store = common::lake_with(backend, limits).await;
    store
        .catalog(WriteContext::default())
        .declare(
            &common::owner(),
            Declaration::Verb(VerbDecl {
                name: "notify".into(),
                on: "invoice".into(),
                scope: Default::default(),
                payload: Vec::new(),
                risk: Risk::Low,
                requires: None,
                effect: VerbEffect::External {
                    executor: "mailer".into(),
                },
                description: None,
            }),
        )
        .await
        .expect("the owner declares the verb");
    store
}

async fn serve(store: Arc<dyn LakeStore>) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let open = Arc::new(
        move |_: &lakelands_api::LakeId| -> lakelands_api::Result<Arc<dyn LakeStore>> {
            Ok(store.clone())
        },
    );
    let config = HttpConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        allowed_origins: vec!["http://localhost:3000".into()],
        session_idle_secs: 1800,
        feed_poll_millis: 20,
    };
    let server = serve_http(config, common::resolver(), open).await.unwrap();
    let addr = server.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let _ = server.run().await;
    });
    (addr, task)
}

/// The request a transcript block states, as bytes.
fn request(addr: SocketAddr, block: &[&str]) -> Vec<u8> {
    let (start, rest) = block.split_first().unwrap();
    let mut head = format!("{start} HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n");
    let mut body = Vec::new();
    let mut typed = false;
    for l in rest {
        match l.split_once(": ") {
            Some((name, _))
                if body.is_empty()
                    && !name.is_empty()
                    && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') =>
            {
                typed |= name == "content-type";
                // `content-type: (none)` sends a body with no media type.
                if *l != "content-type: (none)" {
                    write!(head, "{l}\r\n").unwrap();
                }
            }
            _ => body.push(*l),
        }
    }
    let body = match body.len() {
        0 => String::new(),
        1 => body[0].to_owned(),
        _ => body.join("\n") + "\n",
    };
    if !body.is_empty() && !typed {
        head.push_str("content-type: application/json\r\n");
    }
    if !body.is_empty() {
        write!(head, "content-length: {}\r\n", body.len()).unwrap();
    }
    format!("{head}\r\n{body}").into_bytes()
}

/// A chunked body, joined, as far as it has arrived.
fn dechunk(mut raw: &str) -> String {
    let mut out = String::new();
    while let Some((size, rest)) = raw.split_once("\r\n") {
        let n = usize::from_str_radix(size.trim(), 16).unwrap();
        if n == 0 || rest.len() < n + 2 {
            break;
        }
        out.push_str(&rest[..n]);
        raw = &rest[n + 2..];
    }
    out
}

/// Send one request; answer the status, the pinned headers and the body.
/// An event stream never ends by itself: it is read until the comment
/// that says it caught up.
async fn exchange(addr: SocketAddr, block: &[&str]) -> (String, Vec<String>, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(&request(addr, block)).await.unwrap();
    let mut bytes = Vec::new();
    let mut buf = [0u8; 4096];
    let raw = loop {
        let n = tokio::time::timeout(WAIT, stream.read(&mut buf))
            .await
            .expect("an answer within the wait")
            .unwrap();
        let raw = String::from_utf8_lossy(&bytes).into_owned();
        if n == 0 {
            break raw;
        }
        bytes.extend_from_slice(&buf[..n]);
        let raw = String::from_utf8_lossy(&bytes).into_owned();
        if let Some((head, body)) = raw.split_once("\r\n\r\n")
            && head
                .to_ascii_lowercase()
                .contains("content-type: text/event-stream")
            && dechunk(body)
                .split("\n\n")
                .any(|e| e.starts_with(": next "))
        {
            break raw;
        }
    };
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let mut lines = head.lines();
    let status = lines.next().unwrap().split(' ').nth(1).unwrap().to_owned();
    let headers: Vec<String> = lines
        .map(|h| match h.split_once(':') {
            Some((name, value)) => format!("{}:{value}", name.to_ascii_lowercase()),
            None => h.to_owned(),
        })
        .collect();
    let body = if headers.iter().any(|h| h == "transfer-encoding: chunked") {
        dechunk(body)
    } else {
        body.to_owned()
    };
    let mut pinned: Vec<String> = headers
        .into_iter()
        .filter(|h| PINNED.iter().any(|p| h.starts_with(p)))
        .collect();
    pinned.sort();
    (status, pinned, body)
}

#[tokio::test]
async fn rest_answers_byte_for_byte() {
    for backend in BACKENDS {
        rest_answers_byte_for_byte_on(backend).await;
    }
}

/// The string value of the first `"member":"…"` in a text.
fn member<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let start = text.find(&format!("\"{name}\":\""))? + name.len() + 4;
    let len = text[start..].find('"')?;
    Some(&text[start..start + len])
}

async fn rest_answers_byte_for_byte_on(backend: Backend) {
    let file = std::fs::read_to_string(path("rest.txt")).unwrap();
    let requests_only = !file.lines().any(|l| l.starts_with("< "));
    let (addr, task) = serve(rest_lake(backend).await).await;
    let mut recorded = String::new();
    let mut block: Vec<String> = Vec::new();
    // Opaque values answered so far, newest last: (member, shown as,
    // values). A request names the newest; an answer shows every one.
    let mut proposal = String::new();
    let mut opaque: Vec<(&str, &str, Vec<String>)> = vec![
        ("report_id", "{report_id}", Vec::new()),
        ("next_cursor", "{cursor}", Vec::new()),
    ];
    let mut lines = file.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some(req) = line.strip_prefix("> ") {
            let mut req = req.replace("{proposal}", &proposal);
            for (_, shown, values) in &opaque {
                if let Some(v) = values.last() {
                    req = req.replace(shown, v);
                }
            }
            block.push(req);
            writeln!(recorded, "{line}").unwrap();
            if lines.peek().is_some_and(|n| n.starts_with("> ")) {
                continue;
            }
            let block_: Vec<&str> = block.iter().map(String::as_str).collect();
            let (status, headers, body) = exchange(addr, &block_).await;
            block.clear();
            if let Some(p) = member(&body, "proposal") {
                proposal = p.to_owned();
            }
            for (name, _, values) in &mut opaque {
                if let Some(v) = member(&body, name) {
                    values.push(v.to_owned());
                }
            }
            writeln!(recorded, "< {status}").unwrap();
            for h in headers {
                writeln!(recorded, "< {h}").unwrap();
            }
            let mut body = body;
            for (_, shown, values) in &opaque {
                for v in values {
                    body = body.replace(v.as_str(), shown);
                }
            }
            for l in body.lines() {
                writeln!(recorded, "< {l}").unwrap();
            }
        } else if !line.starts_with("< ") {
            writeln!(recorded, "{line}").unwrap();
        }
    }
    task.abort();
    check(backend, "rest.txt", requests_only, recorded, file);
}

/// One server-sent event: its id, its kind, its data, and its bytes.
type Sse = (u64, String, serde_json::Value, String);

/// A reader of one open event stream.
struct Events {
    reader: BufReader<TcpStream>,
    pending: String,
}

impl Events {
    /// Open the feed; answer the status, the head of the reply, and the
    /// stream when it opened.
    async fn open(addr: SocketAddr, block: &[&str]) -> (String, String, Option<Events>) {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.write_all(&request(addr, block)).await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut head = String::new();
        loop {
            let mut l = String::new();
            reader.read_line(&mut l).await.unwrap();
            if l == "\r\n" {
                break;
            }
            head.push_str(&l.to_ascii_lowercase());
        }
        let status = head.split(' ').nth(1).unwrap().to_owned();
        let events = (status == "200").then(|| Events {
            reader,
            pending: String::new(),
        });
        (status, head, events)
    }

    /// The next event, skipping comments.
    async fn next(&mut self) -> Sse {
        loop {
            if let Some(i) = self.pending.find("\n\n") {
                let event: String = self.pending.drain(..i + 2).collect();
                let (mut id, mut kind, mut data) = (None, String::new(), None);
                for l in event.lines() {
                    if let Some(v) = l.strip_prefix("id: ") {
                        id = v.parse().ok();
                    } else if let Some(v) = l.strip_prefix("event: ") {
                        kind = v.to_owned();
                    } else if let Some(v) = l.strip_prefix("data: ") {
                        data = serde_json::from_str(v).ok();
                    }
                }
                if let (Some(id), Some(data)) = (id, data) {
                    return (id, kind, data, event);
                }
                continue;
            }
            // One chunk: its size, its bytes, its line end.
            let mut size = String::new();
            self.reader.read_line(&mut size).await.unwrap();
            let n = usize::from_str_radix(size.trim(), 16).unwrap();
            assert!(n > 0, "the feed ended");
            let mut chunk = vec![0; n + 2];
            self.reader.read_exact(&mut chunk).await.unwrap();
            self.pending
                .push_str(std::str::from_utf8(&chunk[..n]).unwrap());
        }
    }
}

/// The feed sends the log from a position, waits at the head for what is
/// written next, and resumes after `Last-Event-ID`. Only a principal that
/// reads everything opens it.
#[tokio::test]
async fn the_feed_streams_the_log_and_resumes() {
    for backend in BACKENDS {
        the_feed_streams_the_log_and_resumes_on(backend).await;
    }
}

async fn the_feed_streams_the_log_and_resumes_on(backend: Backend) {
    let store = common::lake_on(backend).await;
    let head = store.head().await.unwrap().0;
    let (addr, task) = serve(store.clone()).await;

    let (status, _, _) = Events::open(
        addr,
        &["GET /lakes/finance/feed", "authorization: Bearer k-clerk"],
    )
    .await;
    assert_eq!(status, "403", "{backend:?}: the clerk reads masked fields");

    let (status, reply, events) = Events::open(
        addr,
        &[
            "GET /lakes/finance/feed?from=0",
            "authorization: Bearer k-owner",
        ],
    )
    .await;
    assert_eq!(status, "200", "{backend:?}");
    assert!(reply.contains("content-type: text/event-stream"), "{reply}");
    assert!(
        reply.contains(&format!("lakelands-position: {head}")),
        "{reply}"
    );
    let mut events = events.unwrap();
    // Each event's bytes: its position, its kind, and the event as the
    // lake's own feed holds it, as JSON.
    let log = store
        .feed()
        .poll(Position(0), &FeedFilter::default(), 1000)
        .await
        .unwrap()
        .events;
    let mut seen = Vec::new();
    while seen.last() != Some(&head) {
        let (id, _, _, raw) = events.next().await;
        let e = &log[id as usize];
        let expected = format!(
            "id: {id}\nevent: {}\ndata: {}\n\n",
            e.body.kind(),
            serde_json::to_value(e).unwrap()
        );
        assert_eq!(raw, expected, "{backend:?}: event {id}, byte for byte");
        seen.push(id);
    }
    assert_eq!(
        seen,
        (0..=head).collect::<Vec<_>>(),
        "{backend:?}: in order, none missed"
    );

    // At the head, the stream waits for the next write.
    let c = store.catalog(WriteContext::default());
    c.upsert(
        &common::owner(),
        &"invoice".into(),
        &"INV-1".into(),
        lakelands_mock::checks::patch(&[("status", json!("held"))]),
        WriteOpts::default(),
    )
    .await
    .unwrap();
    let (id, kind, data, _) = events.next().await;
    assert!(id > head, "{backend:?}");
    assert_eq!(
        data["body"]["kind"],
        json!(kind),
        "{backend:?}: the event is its kind"
    );
    assert_eq!(data["body"]["key"], json!("INV-1"), "{backend:?}: {data}");
    drop(events);

    // A reconnecting client names the last id it received.
    let (status, _, events) = Events::open(
        addr,
        &[
            "GET /lakes/finance/feed?from=0",
            "authorization: Bearer k-owner",
            &format!("last-event-id: {head}"),
        ],
    )
    .await;
    assert_eq!(status, "200");
    let (next, _, _, _) = events.unwrap().next().await;
    assert_eq!(
        next,
        head + 1,
        "{backend:?}: resumed after the last id, not from `from`"
    );

    task.abort();
}

/// `aggregate` reaches find as the JSON its tool takes. The mock does not
/// aggregate (it says so as `invalid`); the store does, on both engines.
#[tokio::test]
async fn find_aggregates_on_the_store() {
    for backend in BACKENDS {
        let (addr, task) = serve(rest_lake(backend).await).await;
        let (status, _, body) = exchange(
            addr,
            &[
                "GET /lakes/finance/types/invoice/records?aggregate=%7B%22measures%22%3A%5B%7B%22name%22%3A%22n%22%2C%22func%22%3A%22count%22%7D%5D%7D",
                "authorization: Bearer k-clerk",
            ],
        )
        .await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        match backend {
            Backend::Mock => {
                assert_eq!(status, "422", "{body}");
                assert_eq!(v["reason"]["refusal"], "invalid", "{body}");
            }
            Backend::Memory | Backend::Sqlite => {
                assert_eq!(status, "200", "{backend:?}: {body}");
                assert_eq!(v["data"]["shape"], "groups", "{body}");
                assert_eq!(v["data"]["rows"][0]["values"]["n"], json!("1"), "{body}");
            }
        }
        task.abort();
    }
}

/// A JSON body over the server's limit of 4 MiB is refused as too large
/// (413) before any of it reaches a door, and the lake is unchanged.
#[tokio::test]
async fn a_body_over_four_mebibytes_is_413() {
    let store = rest_lake(Backend::Mock).await;
    let head = store.head().await.unwrap();
    let (addr, task) = serve(store.clone()).await;
    let note = "x".repeat(4 << 20);
    let body = format!("{{\"note\":\"{note}\"}}");
    let (status, pinned, answer) = exchange(
        addr,
        &[
            "PUT /lakes/finance/types/invoice/records/INV-1",
            "authorization: Bearer k-owner",
            "content-type: application/json",
            &body,
        ],
    )
    .await;
    task.abort();
    assert_eq!(status, "413", "{answer}");
    let answer: serde_json::Value = serde_json::from_str(&answer).unwrap();
    assert_eq!(
        (&answer["error"], &answer["reason"]["refusal"]),
        (&json!("refused"), &json!("invalid")),
        "{answer}"
    );
    assert!(
        pinned.contains(&format!("lakelands-position: {}", head.0)),
        "{pinned:?}"
    );
    assert_eq!(store.head().await.unwrap(), head, "nothing was written");
}

/// POST one NDJSON line of `len` bytes (a record padded with spaces)
/// and its line feed as an ingest, in chunks of `chunk` bytes, each
/// written on its own, so the line feed comes in the last one; answer
/// the lines of the answer.
async fn ingest_one_line(addr: SocketAddr, len: usize, chunk: usize) -> Vec<serde_json::Value> {
    let record = r#"{"type":"invoice","key":"INV-L","fields":{"amount":"1.00"}}"#;
    let mut body = format!("{record}{}", " ".repeat(len - record.len())).into_bytes();
    body.push(b'\n');
    let (mut reader, mut stream) = TcpStream::connect(addr).await.unwrap().into_split();
    // The answer is read as it comes, while the body is still being sent:
    // a server that refuses and closes with request bytes unread may be
    // answered by a reset (macOS does), and the reset ends the reading.
    // A reset after the answer's last chunk is its end; before it, a
    // failure.
    let answer = tokio::spawn(async move {
        let mut raw = Vec::new();
        let mut buf = [0u8; 64 << 10];
        let complete = |raw: &[u8]| {
            String::from_utf8_lossy(raw)
                .split_once("\r\n\r\n")
                .is_some_and(|(_, body)| body.ends_with("0\r\n\r\n"))
        };
        loop {
            match reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    raw.extend_from_slice(&buf[..n]);
                    if complete(&raw) {
                        break;
                    }
                }
                Err(_) if complete(&raw) => break,
                Err(e) => panic!(
                    "the answer broke off: {e}: {:?}",
                    String::from_utf8_lossy(&raw)
                ),
            }
        }
        raw
    });
    let head = format!(
        "POST /lakes/finance/ingest/bank HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n\
         authorization: Bearer k-owner\r\ncontent-type: application/x-ndjson\r\n\
         transfer-encoding: chunked\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    for part in body.chunks(chunk) {
        let mut framed = format!("{:x}\r\n", part.len()).into_bytes();
        framed.extend_from_slice(part);
        framed.extend_from_slice(b"\r\n");
        // The server may stop reading once it refuses; the answer says so.
        if answer.is_finished() || stream.write_all(&framed).await.is_err() {
            break;
        }
        if stream.flush().await.is_err() {
            break;
        }
    }
    let _ = stream.write_all(b"0\r\n\r\n").await;
    let raw = tokio::time::timeout(std::time::Duration::from_secs(30), answer)
        .await
        .expect("an answer within thirty seconds")
        .unwrap();
    let raw = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    dechunk(body)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// An ingest line is at most 4 MiB, to the byte, however the body is cut:
/// a line one byte over, whose line feed arrives in the same read as its
/// last byte, is refused as a line twice the cap is, and nothing of it
/// is committed (lakelands#135).
#[tokio::test]
async fn an_ingest_line_over_four_mebibytes_is_refused_to_the_byte() {
    const CAP: usize = 4 << 20;
    const CHUNK: usize = 64 << 10;
    let store = rest_lake(Backend::Mock).await;
    let (addr, task) = serve(store.clone()).await;

    let taken = ingest_one_line(addr, CAP, CHUNK).await;
    assert_eq!(taken.len(), 1, "{taken:?}");
    assert_eq!(taken[0]["records"], json!(1), "a line of 4 MiB is taken");

    let head = store.head().await.unwrap();
    let over = ingest_one_line(addr, CAP + 1, CHUNK).await;
    let twice = ingest_one_line(addr, 2 * CAP, CHUNK).await;
    task.abort();
    assert_eq!(twice.len(), 1, "{twice:?}");
    assert_eq!(
        (&twice[0]["error"], &twice[0]["reason"]["refusal"]),
        (&json!("refused"), &json!("invalid")),
        "{twice:?}"
    );
    assert_eq!(over, twice, "one byte over is refused as twice the cap is");
    assert_eq!(store.head().await.unwrap(), head, "nothing was committed");
}

/// A refused ingest is answered while the body is still arriving: after
/// the refusing line the server reads the rest of the body and drops it,
/// so a client that keeps sending, many times the line cap, finishes
/// sending and reads the whole answer, where a close with the body unread
/// would answer it with a reset (lakelands#156).
#[tokio::test]
async fn a_refused_ingest_is_answered_while_the_body_keeps_arriving() {
    const REST: usize = 32 << 20;
    let store = rest_lake(Backend::Mock).await;
    let (addr, task) = serve(store.clone()).await;
    let head = store.head().await.unwrap();

    let refusing = b"not a record\n";
    let filler = [b' '; 64 << 10];
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "POST /lakes/finance/ingest/bank HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n\
         authorization: Bearer k-owner\r\ncontent-type: application/x-ndjson\r\n\
         content-length: {}\r\n\r\n",
        refusing.len() + REST
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.write_all(refusing).await.unwrap();
    // No pause for the answer: the whole body goes out, then the answer
    // is read to its end.
    for _ in 0..REST / filler.len() {
        stream
            .write_all(&filler)
            .await
            .expect("the server reads the body to its end");
    }
    let mut raw = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        stream.read_to_end(&mut raw),
    )
    .await
    .expect("an answer within thirty seconds")
    .expect("the answer is read to its end, without a reset");
    task.abort();
    let raw = String::from_utf8_lossy(&raw).into_owned();
    let (status, body) = raw.split_once("\r\n\r\n").unwrap();
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    let answer: Vec<serde_json::Value> = dechunk(body)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(answer.len(), 1, "{answer:?}");
    assert_eq!(
        (&answer[0]["error"], &answer[0]["reason"]["refusal"]),
        (&json!("refused"), &json!("invalid")),
        "{answer:?}"
    );
    assert_eq!(store.head().await.unwrap(), head, "nothing was committed");
}

/// A window that takes longer than the keep-alive to commit is answered
/// with an empty line meanwhile, so a client holding the answer to a
/// silence bound hears the server works; then its line comes, and the
/// ingest ends as it would have (lakelands#166).
#[tokio::test]
async fn a_window_slower_than_the_keep_alive_is_answered_with_an_empty_line() {
    use std::sync::atomic::Ordering;
    let faults = Arc::new(common::flaky::Faults::default());
    // Past one keep-alive of fifteen seconds, short of two.
    faults.first_window_millis.store(17_000, Ordering::SeqCst);
    let store: Arc<dyn LakeStore> = Arc::new(common::flaky::Store {
        inner: rest_lake(Backend::Mock).await,
        faults,
    });
    let (addr, task) = serve(store.clone()).await;

    let body = "{\"type\":\"invoice\",\"key\":\"INV-K\",\"fields\":{\"amount\":\"1.00\"}}\n";
    let request = format!(
        "POST /lakes/finance/ingest/bank HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n\
         authorization: Bearer k-owner\r\ncontent-type: application/x-ndjson\r\n\
         content-length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    tokio::time::timeout(WAIT, stream.read_to_end(&mut raw))
        .await
        .expect("an answer within the wait")
        .unwrap();
    task.abort();
    let raw = String::from_utf8_lossy(&raw).into_owned();
    let (status, body) = raw.split_once("\r\n\r\n").unwrap();
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    let answer = dechunk(body);
    let (quiet, lines) = answer.split_at(answer.len() - answer.trim_start().len());
    assert_eq!(
        quiet, "\n",
        "one empty line while the window commits: {answer:?}"
    );
    let lines: Vec<serde_json::Value> = lines
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 1, "{answer:?}");
    assert_eq!(lines[0]["records"], json!(1), "{answer:?}");
}

/// A lake the server cannot open answers `unavailable` (503), whatever
/// the route, and carries no position: there is no log it was read at.
#[tokio::test]
async fn a_lake_that_cannot_be_opened_is_503() {
    let open = Arc::new(
        |lake: &lakelands_api::LakeId| -> lakelands_api::Result<Arc<dyn LakeStore>> {
            let (lake, schema) = (lake.to_string(), None);
            Err(lakelands_api::Error::unavailable(
                lakelands_api::error::Situation::NoLake { lake, schema },
            ))
        },
    );
    let config = HttpConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        allowed_origins: vec![],
        session_idle_secs: 1800,
        feed_poll_millis: 20,
    };
    let server = serve_http(config, common::resolver(), open).await.unwrap();
    let addr = server.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let _ = server.run().await;
    });
    let (status, pinned, answer) = exchange(
        addr,
        &[
            "GET /lakes/finance/describe",
            "authorization: Bearer k-owner",
        ],
    )
    .await;
    task.abort();
    assert_eq!(status, "503", "{answer}");
    let answer: serde_json::Value = serde_json::from_str(&answer).unwrap();
    assert_eq!(answer["error"], json!("unavailable"), "{answer}");
    assert!(
        !pinned.iter().any(|h| h.starts_with("lakelands-position:")),
        "{pinned:?}"
    );
}

/// A caught-up feed says where it caught up once, then sends a comment
/// every fifteen seconds, so a proxy
/// keeps it open, and nothing before that. Slow by nature: it waits for
/// one keep-alive.
#[tokio::test]
async fn a_quiet_feed_sends_a_keep_alive_every_fifteen_seconds() {
    let store = common::lake_on(Backend::Mock).await;
    let next = store.head().await.unwrap().0 + 1;
    let (addr, task) = serve(store).await;
    let from = format!("GET /lakes/finance/feed?from={next}");
    let (status, _, events) = Events::open(addr, &[&from, "authorization: Bearer k-owner"]).await;
    assert_eq!(status, "200");
    let mut events = events.unwrap();
    let opened = std::time::Instant::now();
    let mut seen = String::new();
    let mut buf = [0u8; 256];
    while !seen.contains(": keep-alive") {
        let n = tokio::time::timeout(
            std::time::Duration::from_secs(25),
            events.reader.read(&mut buf),
        )
        .await
        .expect("a keep-alive within 25 seconds")
        .unwrap();
        assert!(n > 0, "the stream ended: {seen:?}");
        seen.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    let waited = opened.elapsed().as_secs_f64();
    task.abort();
    assert!(
        (14.0..20.0).contains(&waited),
        "the first keep-alive came after {waited:.1} seconds; a quiet feed sends one every \
         fifteen"
    );
    assert!(
        !seen.contains("id: "),
        "nothing was written, so the feed sent no event: {seen:?}"
    );
    // It said where it caught up once, at the start, and not again while
    // the position stood still.
    assert_eq!(
        seen.matches(": next ").collect::<Vec<_>>(),
        [": next "],
        "{seen:?}"
    );
}

/// The server's configuration: a caught-up feed polls the lake every 250
/// milliseconds unless the configuration says otherwise.
#[test]
fn the_feed_polls_every_250_milliseconds_unless_configured() {
    let config: HttpConfig = serde_json::from_value(json!({"bind": "127.0.0.1:7700"})).unwrap();
    assert_eq!(config.feed_poll_millis, 250);
}
