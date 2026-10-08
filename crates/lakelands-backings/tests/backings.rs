//! The `http` and `exec` backings against a local responder and local
//! programs: what they send, what they answer, and what they refuse.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::sync::Arc;

use lakelands_api::Error;
use lakelands_api::error::{Case, Place, Said};
use lakelands_api::seam::{FunctionBacking, FunctionCall};
use lakelands_backings::{Command, Config, ExecConfig, HttpConfig, backings};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn call(config: Value) -> FunctionCall {
    FunctionCall {
        lake: "l".into(),
        function: "lookup".into(),
        inputs: json!({"code": "AT"}),
        config,
    }
}

/// What a call was refused as invalid for, said at its function.
fn refused(r: &lakelands_api::Result<Value>) -> Said {
    let said = match r {
        Err(e @ Error::Refused { .. }) => e.case().cloned(),
        _ => None,
    };
    let said = said.unwrap_or_else(|| panic!("expected the call to be refused, got {r:?}"));
    let name = "lookup".to_owned();
    assert_eq!(said.at.first(), Some(&Place::Function { name }), "{said}");
    said
}

fn failure(r: lakelands_api::Result<Value>) -> String {
    match r {
        Err(Error::Unavailable { situation }) => situation.to_string(),
        other => panic!("expected the call to fail, got {other:?}"),
    }
}

// ---- exec ----

fn exec(commands: &[(&str, &str, &[&str])], timeout_secs: u64) -> Arc<dyn FunctionBacking> {
    let commands: BTreeMap<String, Command> = commands
        .iter()
        .map(|(name, program, args)| {
            (
                (*name).to_owned(),
                Command {
                    program: program.into(),
                    args: args.iter().map(|a| (*a).to_owned()).collect(),
                },
            )
        })
        .collect();
    let config = Config {
        exec: Some(ExecConfig {
            commands,
            timeout_secs,
            max_output_bytes: 1024,
        }),
        http: None,
    };
    backings(&config).pop().expect("the exec backing")
}

#[tokio::test]
async fn exec_sends_the_call_on_stdin_and_reads_the_output_from_stdout() {
    let b = exec(&[("echo", "/bin/cat", &[])], 5);
    assert_eq!(b.kind(), "exec");
    let got = b.call(&call(json!({"command": "echo"}))).await.unwrap();
    assert_eq!(
        got,
        json!({"function": "lookup", "inputs": {"code": "AT"}, "config": {"command": "echo"}})
    );
}

#[tokio::test]
async fn exec_runs_only_the_commands_the_operator_named() {
    let b = exec(&[("echo", "/bin/cat", &[])], 5);
    let command = "rm".to_owned();
    let said = refused(&b.call(&call(json!({"command": "rm"}))).await);
    assert_eq!(said.case, Case::CommandNotAllowed { command });
    let said = refused(&b.call(&call(json!({"program": "/bin/cat"}))).await);
    assert_eq!(said.case, Case::ExecNoCommand);
}

#[tokio::test]
async fn exec_gives_the_command_an_empty_environment() {
    let b = exec(&[("env", "/usr/bin/env", &[])], 5);
    let why = failure(b.call(&call(json!({"command": "env"}))).await);
    // `env` with an empty environment prints nothing, which is no JSON.
    assert!(why.contains("not JSON"), "{why}");
}

#[tokio::test]
async fn exec_fails_on_a_failure_status_bad_output_or_the_timeout() {
    let b = exec(
        &[
            // `false` is not at one path on every Unix; `/bin/sh` is.
            ("false", "/bin/sh", &["-c", "exit 1"]),
            ("words", "/bin/echo", &["not json"]),
            ("slow", "/bin/sleep", &["5"]),
        ],
        1,
    );
    let why = failure(b.call(&call(json!({"command": "false"}))).await);
    assert!(why.contains("failed with exit status 1"), "{why}");
    let why = failure(b.call(&call(json!({"command": "words"}))).await);
    assert!(why.contains("not JSON"), "{why}");
    let started = std::time::Instant::now();
    let why = failure(b.call(&call(json!({"command": "slow"}))).await);
    assert!(why.contains("longer than 1s"), "{why}");
    assert!(
        started.elapsed().as_secs() < 4,
        "the slow command was stopped"
    );
}

// ---- http ----

/// A responder that answers every connection with `reply`, and sends
/// back each request it read.
async fn responder(reply: &'static str) -> (u16, tokio::sync::mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            let mut got = Vec::new();
            let mut buf = [0u8; 4096];
            // Headers, then as much body as Content-Length says.
            loop {
                let n = s.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                got.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&got).to_string();
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if got.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let _ = tx.send(String::from_utf8_lossy(&got).to_string()).await;
            if !reply.is_empty() {
                let _ = s.write_all(reply.as_bytes()).await;
            } else {
                // Never answer.
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            }
        }
    });
    (port, rx)
}

fn http(hosts: &[&str], timeout_secs: u64) -> Arc<dyn FunctionBacking> {
    let config = Config {
        http: Some(HttpConfig {
            hosts: hosts.iter().map(|h| (*h).to_owned()).collect(),
            timeout_secs,
            max_reply_bytes: 1024,
            ca_file: None,
        }),
        exec: None,
    };
    backings(&config).pop().expect("the http backing")
}

#[tokio::test]
async fn http_posts_the_call_and_reads_a_2xx_reply_as_the_output() {
    let (port, mut requests) = responder(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 18\r\nconnection: close\r\n\r\n{\"name\":\"Austria\"}",
    )
    .await;
    let b = http(&["127.0.0.1"], 5);
    assert_eq!(b.kind(), "http");
    let url = format!("http://127.0.0.1:{port}/lookup?v=1");
    let got = b.call(&call(json!({"url": url}))).await.unwrap();
    assert_eq!(got, json!({"name": "Austria"}));
    let request = requests.recv().await.unwrap();
    assert!(
        request.starts_with("POST /lookup?v=1 HTTP/1.1\r\n"),
        "{request}"
    );
    let body = &request[request.find("\r\n\r\n").unwrap() + 4..];
    assert_eq!(
        serde_json::from_str::<Value>(body).unwrap(),
        json!({"function": "lookup", "inputs": {"code": "AT"}, "config": {"url": url}})
    );
}

#[tokio::test]
async fn http_reaches_only_the_hosts_the_operator_named() {
    let (port, mut requests) = responder("HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}").await;
    // Allowed on another port only: refused before connecting.
    let b = http(&["127.0.0.1:1"], 5);
    let url = format!("http://127.0.0.1:{port}/");
    let not_allowed = Case::HostNotAllowed {
        host: "127.0.0.1".into(),
        port,
    };
    let said = refused(&b.call(&call(json!({"url": url}))).await);
    assert_eq!(said.case, not_allowed);
    let b = http(&["geo.example"], 5);
    let said = refused(&b.call(&call(json!({"url": url}))).await);
    assert_eq!(said.case, not_allowed);
    let said = refused(&b.call(&call(json!({"url": "ftp://geo.example/"}))).await);
    let key = "url".to_owned();
    assert_eq!(said.at.get(1), Some(&Place::FunctionConfig { key }));
    assert_eq!(said.case, Case::UrlScheme);
    let said = refused(&b.call(&call(json!({}))).await);
    assert_eq!(said.case, Case::HttpNoUrl);
    assert!(
        requests.try_recv().is_err(),
        "no request reached the responder"
    );
    // The port-exact entry allows exactly that port.
    let b = http(&[&format!("127.0.0.1:{port}")], 5);
    assert_eq!(b.call(&call(json!({"url": url}))).await.unwrap(), json!({}));
}

#[tokio::test]
async fn http_follows_no_redirect_and_fails_on_other_statuses() {
    let (port, _) = responder(
        "HTTP/1.1 302 Found\r\nlocation: http://elsewhere.example/\r\ncontent-length: 0\r\n\r\n",
    )
    .await;
    let b = http(&["127.0.0.1"], 5);
    let why = failure(
        b.call(&call(json!({"url": format!("http://127.0.0.1:{port}/")})))
            .await,
    );
    assert!(why.contains("redirect"), "{why}");
    let (port, _) =
        responder("HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\n\r\n").await;
    let why = failure(
        b.call(&call(json!({"url": format!("http://127.0.0.1:{port}/")})))
            .await,
    );
    assert!(why.contains("500"), "{why}");
}

#[tokio::test]
async fn http_fails_when_the_reply_takes_longer_than_the_timeout() {
    let (port, _) = responder("").await;
    let b = http(&["127.0.0.1"], 1);
    let started = std::time::Instant::now();
    let why = failure(
        b.call(&call(json!({"url": format!("http://127.0.0.1:{port}/")})))
            .await,
    );
    assert!(why.contains("within 1 seconds"), "{why}");
    assert!(started.elapsed().as_secs() < 4);
}

#[tokio::test]
async fn https_without_a_readable_certificate_bundle_fails_the_call() {
    let (port, _) = responder("HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}").await;
    let config = Config {
        http: Some(HttpConfig {
            hosts: vec!["localhost".into()],
            timeout_secs: 5,
            max_reply_bytes: 1024,
            ca_file: Some("/nonexistent/bundle.pem".into()),
        }),
        exec: None,
    };
    let b = backings(&config).pop().unwrap();
    let why = failure(
        b.call(&call(json!({"url": format!("https://localhost:{port}/")})))
            .await,
    );
    assert!(why.contains("certificate bundle"), "{why}");
}

#[tokio::test]
async fn exec_stops_a_command_at_its_output_limit_and_keeps_its_stderr_out_of_the_answer() {
    let b = exec(
        &[
            ("endless", "/usr/bin/yes", &[]),
            ("loud", "/bin/sh", &["-c", "echo secret-detail >&2; exit 3"]),
        ],
        5,
    );
    let started = std::time::Instant::now();
    let why = failure(b.call(&call(json!({"command": "endless"}))).await);
    assert!(why.contains("more than 1024 bytes"), "{why}");
    assert!(
        started.elapsed().as_secs() < 3,
        "stopped at the limit, not at the timeout"
    );
    let why = failure(b.call(&call(json!({"command": "loud"}))).await);
    assert!(why.contains("exit status 3"), "{why}");
    assert!(!why.contains("secret-detail"), "{why}");
}

#[test]
fn a_command_named_by_a_relative_path_is_refused_when_the_configuration_loads() {
    let config: Config = serde_json::from_value(json!({
        "exec": {"commands": {"geo": {"program": "geocode"}}}
    }))
    .unwrap();
    let why = config.check().unwrap_err();
    assert!(why.contains("not an absolute path"), "{why}");
    let config: Config = serde_json::from_value(json!({"http": {"hosts": []}})).unwrap();
    assert!(config.check().is_err());
}

#[tokio::test]
async fn http_sends_the_host_and_port_as_host_and_never_user_information() {
    let (port, mut requests) = responder("HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}").await;
    let b = http(&["127.0.0.1"], 5);
    let url = format!("http://user:secret@127.0.0.1:{port}/");
    let answer = b.call(&call(json!({"url": url}))).await;
    assert!(answer.is_ok(), "{answer:?}");
    let request = requests.recv().await.unwrap();
    assert!(
        request.contains(&format!("host: 127.0.0.1:{port}\r\n")),
        "{request}"
    );
    // The body carries the function's configuration, URL included; the
    // headers never carry the user information.
    let headers = &request[..request.find("\r\n\r\n").unwrap()];
    assert!(!headers.contains("secret"), "{headers}");
}

#[tokio::test]
async fn an_http_failure_names_the_function_never_the_url() {
    let (port, _) =
        responder("HTTP/1.1 500 Internal Server Error\r\ncontent-length: 0\r\n\r\n").await;
    let b = http(&["127.0.0.1"], 5);
    let url = format!("http://user:secret@127.0.0.1:{port}/lookup?token=hidden");
    let why = failure(b.call(&call(json!({"url": url}))).await);
    assert!(why.contains("500"), "{why}");
    let (listener_port, _) = responder("").await;
    let b = http(&["127.0.0.1"], 1);
    let url = format!("http://user:secret@127.0.0.1:{listener_port}/lookup?token=hidden");
    let why = failure(b.call(&call(json!({"url": url}))).await);
    assert!(why.contains("function `lookup`"), "{why}");
    assert!(!why.contains(&listener_port.to_string()), "{why}");
    assert!(!why.contains("secret") && !why.contains("hidden"), "{why}");
}
