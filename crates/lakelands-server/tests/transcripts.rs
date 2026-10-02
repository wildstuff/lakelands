//! Recorded JSON-RPC transcripts: the exact bytes this server answers.
//!
//! Each file in `tests/transcripts/` holds requests (`>`) and the answers
//! expected (`<`). The server's wire format is pinned here, independently
//! of any client library, so a change in a client can never move it.
//!
//! Notifications are not in these transcripts: they come when a session's
//! pump reads the feed, not in step with the requests, and
//! `tests/notify.rs` checks them.
//!
//! A transcript with requests but no answers is recorded: the test
//! writes the answers into the file and fails, asking for the file to be
//! read and committed.

mod common;

use common::{BACKENDS, Backend, check, path};

use std::fmt::Write as _;
use std::sync::Arc;

use lakelands_api::LakeStore;
use lakelands_server::{HttpConfig, serve_http, serve_io};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

#[tokio::test]
async fn stdio_answers_byte_for_byte() {
    for backend in BACKENDS {
        stdio_answers_byte_for_byte_on(backend).await;
    }
}

async fn stdio_answers_byte_for_byte_on(backend: Backend) {
    let file = std::fs::read_to_string(path("stdio.txt")).unwrap();
    let requests: Vec<&str> = file.lines().filter_map(|l| l.strip_prefix("> ")).collect();
    let requests_only = !file.lines().any(|l| l.starts_with("< "));
    let mut recorded = String::new();
    // One request at a time, so every answer is written under its request.
    let (input, mut to_server) = tokio::io::duplex(1 << 20);
    let (mut from_server, output) = tokio::io::duplex(1 << 20);
    let server = tokio::spawn(serve_io(
        common::lake_on(backend).await,
        common::owner(),
        BufReader::new(input),
        output,
    ));
    let mut reader = BufReader::new(&mut from_server);
    for line in file.lines() {
        if line.starts_with("# ") || line.is_empty() {
            writeln!(recorded, "{line}").unwrap();
            continue;
        }
        let Some(req) = line.strip_prefix("> ") else {
            continue;
        };
        writeln!(recorded, "> {req}").unwrap();
        to_server
            .write_all(format!("{req}\n").as_bytes())
            .await
            .unwrap();
        to_server
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":\"fence\",\"method\":\"ping\"}\n")
            .await
            .unwrap();
        loop {
            let mut out = String::new();
            tokio::io::AsyncBufReadExt::read_line(&mut reader, &mut out)
                .await
                .unwrap();
            if out.contains("\"id\":\"fence\"") {
                break;
            }
            // Notifications come when the session's pump reads the feed, not
            // in step with the requests: `tests/notify.rs` holds them.
            if serde_json::from_str::<serde_json::Value>(&out).is_ok_and(|m| m.get("id").is_none())
            {
                continue;
            }
            write!(recorded, "< {out}").unwrap();
        }
    }
    drop(to_server);
    server.await.unwrap().unwrap();
    let _ = requests;
    check(backend, "stdio.txt", requests_only, recorded, file);
}

/// Send one raw HTTP/1.1 request; answer the status, the pinned headers
/// and the body.
async fn exchange(
    addr: std::net::SocketAddr,
    block: &[&str],
    session: &str,
) -> (String, Vec<String>, String) {
    let mut lines = block.iter().map(|l| l.replace("{session}", session));
    let start = lines.next().unwrap();
    let mut head = format!("{start} HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n");
    let mut body = String::new();
    for l in lines {
        if l.starts_with('{') || l.starts_with('[') {
            body = l;
        } else {
            write!(head, "{l}\r\n").unwrap();
        }
    }
    if !body.is_empty() {
        write!(
            head,
            "content-type: application/json\r\ncontent-length: {}\r\n",
            body.len()
        )
        .unwrap();
    }
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(format!("{head}\r\n{body}").as_bytes())
        .await
        .unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await.unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let mut head = head.lines();
    let status = head.next().unwrap().split(' ').nth(1).unwrap().to_owned();
    let mut pinned: Vec<String> = head
        .map(|h| match h.split_once(':') {
            Some((name, value)) => format!("{}:{value}", name.to_ascii_lowercase()),
            None => h.to_owned(),
        })
        .filter(|h| {
            [
                "content-type:",
                "mcp-session-id:",
                "lakelands-position:",
                "allow:",
            ]
            .iter()
            .any(|p| h.starts_with(p))
        })
        .collect();
    pinned.sort();
    (status, pinned, body.to_owned())
}

#[tokio::test]
async fn streamable_http_answers_byte_for_byte() {
    for backend in BACKENDS {
        streamable_http_answers_byte_for_byte_on(backend).await;
    }
}

async fn streamable_http_answers_byte_for_byte_on(backend: Backend) {
    let file = std::fs::read_to_string(path("http.txt")).unwrap();
    let requests_only = !file.lines().any(|l| l.starts_with("< "));
    let store = common::lake_on(backend).await;
    let open = Arc::new(
        move |_: &lakelands_api::LakeId| -> lakelands_api::Result<Arc<dyn LakeStore>> {
            Ok(store.clone())
        },
    );
    let config = HttpConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        allowed_origins: vec!["http://localhost:3000".into()],
        session_idle_secs: 1800,
        feed_poll_millis: 250,
    };
    let server = serve_http(config, common::resolver(), open).await.unwrap();
    let addr = server.local_addr().unwrap();
    let task = tokio::spawn(server.run());

    let mut recorded = String::new();
    let mut session = String::new();
    let mut block: Vec<&str> = Vec::new();
    let mut lines = file.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some(req) = line.strip_prefix("> ") {
            block.push(req);
            writeln!(recorded, "{line}").unwrap();
            if lines.peek().is_some_and(|n| n.starts_with("> ")) {
                continue;
            }
            let (status, headers, body) = exchange(addr, &block, &session).await;
            block.clear();
            for h in &headers {
                if let Some(id) = h.strip_prefix("mcp-session-id: ") {
                    session = id.to_owned();
                }
            }
            writeln!(recorded, "< {status}").unwrap();
            let hide = |t: &str| {
                if session.is_empty() {
                    t.to_owned()
                } else {
                    t.replace(&session, "{session}")
                }
            };
            for h in headers {
                writeln!(recorded, "< {}", hide(&h)).unwrap();
            }
            if !body.is_empty() {
                writeln!(recorded, "< {body}").unwrap();
            }
        } else if !line.starts_with("< ") {
            writeln!(recorded, "{line}").unwrap();
        }
    }
    task.abort();
    check(backend, "http.txt", requests_only, recorded, file);
}

/// Sessions end when idle, are replaced when their key initializes
/// again, and exist only when initialize answered a result.
#[tokio::test]
async fn sessions_end_idle_are_replaced_and_never_orphaned() {
    for backend in BACKENDS {
        sessions_end_idle_are_replaced_and_never_orphaned_on(backend).await;
    }
}

async fn sessions_end_idle_are_replaced_and_never_orphaned_on(backend: Backend) {
    let store = common::lake_on(backend).await;
    let open = Arc::new(
        move |_: &lakelands_api::LakeId| -> lakelands_api::Result<Arc<dyn LakeStore>> {
            Ok(store.clone())
        },
    );
    let start = |idle: u64| {
        let (open, resolver) = (open.clone(), common::resolver());
        async move {
            let config = HttpConfig {
                bind: "127.0.0.1:0".parse().unwrap(),
                allowed_origins: Vec::new(),
                session_idle_secs: idle,
                feed_poll_millis: 250,
            };
            let server = serve_http(config, resolver, open).await.unwrap();
            let addr = server.local_addr().unwrap();
            (addr, tokio::spawn(server.run()))
        }
    };
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#;
    let ping = r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#;
    let session_of = |headers: &[String]| {
        headers
            .iter()
            .find_map(|h| h.strip_prefix("mcp-session-id: ").map(str::to_owned))
    };

    // Idle for longer than the limit (here: none at all), a session is gone.
    let (addr, task) = start(0).await;
    let block = [
        "POST /lakes/finance/mcp",
        "authorization: Bearer k-clerk",
        init,
    ];
    let (_, headers, _) = exchange(addr, &block, "").await;
    let sid = session_of(&headers).expect("initialize names a session");
    let block = [
        "POST /lakes/finance/mcp",
        "authorization: Bearer k-clerk",
        "mcp-session-id: {session}",
        ping,
    ];
    assert_eq!(exchange(addr, &block, &sid).await.0, "404");
    task.abort();

    let (addr, task) = start(1800).await;
    // An initialize that answers an error starts no session.
    let bad = r#"{"jsonrpc":"1.0","id":1,"method":"initialize"}"#;
    let block = [
        "POST /lakes/finance/mcp",
        "authorization: Bearer k-clerk",
        bad,
    ];
    let (status, headers, body) = exchange(addr, &block, "").await;
    assert_eq!(status, "200");
    assert!(body.contains("\"error\""), "{body}");
    assert_eq!(session_of(&headers), None);

    // Initializing again under one's own session replaces it.
    let block = [
        "POST /lakes/finance/mcp",
        "authorization: Bearer k-clerk",
        init,
    ];
    let (_, headers, _) = exchange(addr, &block, "").await;
    let first = session_of(&headers).unwrap();
    let block = [
        "POST /lakes/finance/mcp",
        "authorization: Bearer k-clerk",
        "mcp-session-id: {session}",
        init,
    ];
    let (_, headers, _) = exchange(addr, &block, &first).await;
    let second = session_of(&headers).unwrap();
    assert_ne!(first, second);
    let block = [
        "POST /lakes/finance/mcp",
        "authorization: Bearer k-clerk",
        "mcp-session-id: {session}",
        ping,
    ];
    assert_eq!(
        exchange(addr, &block, &first).await.0,
        "404",
        "the old session ended"
    );
    assert_eq!(exchange(addr, &block, &second).await.0, "200");
    task.abort();
}
