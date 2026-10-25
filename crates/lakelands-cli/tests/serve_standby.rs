//! `lakelands serve` with a `standby` section opens its lakes beside their
//! writer: its door serves what the writer wrote, refuses a write and is
//! not ready while the writer's process holds the lease, and once that
//! process is gone it takes the lease, is ready and serves writes
//! (`docs/decisions/076-a-lake-keeps-one-writer-and-a-dataset-that-outgrows-it-becomes-several-lakes.md`).

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::json;

const WAIT: Duration = Duration::from_secs(60);

/// `method path` as the writer, with `body`: its status and body.
fn ask(
    addr: SocketAddr,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> (u16, String) {
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).unwrap();
    s.set_read_timeout(Some(WAIT)).unwrap();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n\
         authorization: Bearer k-writer\r\ncontent-type: application/json\r\n\
         content-length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let status = out.split(' ').nth(1).unwrap().parse().unwrap();
    (status, out.split_once("\r\n\r\n").unwrap().1.to_owned())
}

/// A root at `dir` that serves on a port of its own and stands by.
fn root(dir: &Path) {
    let _ = fs::remove_dir_all(dir);
    let out = Command::new(env!("CARGO_BIN_EXE_lakelands"))
        .arg("--root")
        .arg(dir)
        .arg("init")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let file = dir.join("lakelands.json");
    let mut config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    config["server"] = json!({"bind": "127.0.0.1:0"});
    config["standby"] = json!({"try_millis": 50});
    config["principals"] = json!([{"name": "writer", "kind": "agent", "id": "writer",
        "grants": {"default": "write", "schema": true, "restricted": true}, "api_keys": ["k-writer"]}]);
    fs::write(&file, serde_json::to_string_pretty(&config).unwrap()).unwrap();
}

/// `serve` on the root at `dir`, and the address its door serves at.
fn serve(dir: &Path) -> (Child, SocketAddr) {
    let mut server = Command::new(env!("CARGO_BIN_EXE_lakelands"))
        .arg("--root")
        .arg(dir)
        .args(["serve", "--mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let (tx, rx) = mpsc::channel();
    let stderr = server.stderr.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let _ = tx.send(line.unwrap_or_default());
        }
    });
    let door = loop {
        let line = rx.recv_timeout(WAIT).expect("serve never said it serves");
        let rest = line.strip_prefix("lakelands: serving HTTP at http://");
        if let Some(addr) = rest.and_then(|r| r.split('/').next()?.parse().ok()) {
            break addr;
        }
    };
    (server, door)
}

/// Until `/readyz` at `door` answers `status`.
fn ready(door: SocketAddr, status: u16) {
    let started = Instant::now();
    while ask(door, "GET", "/readyz", None).0 != status {
        assert!(started.elapsed() < WAIT, "/readyz never answered {status}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_standby_serve_takes_the_lease_once_the_writers_process_is_gone() {
    let at = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("serve-standby");
    let (first, second) = (at.join("writer"), at.join("standby"));
    root(&first);
    root(&second);
    let made = Command::new(env!("CARGO_BIN_EXE_lakelands"))
        .arg("--root")
        .arg(&first)
        .args(["lake", "new", "finance"])
        .output()
        .unwrap();
    assert!(made.status.success());
    // The second root names the first root's lake where it lies.
    let mut location: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(first.join("lakes/finance.json")).unwrap())
            .unwrap();
    location["engine"]["dir"] = json!(first.join("lakes/finance"));
    fs::create_dir_all(second.join("lakes")).unwrap();
    fs::write(
        second.join("lakes/finance.json"),
        serde_json::to_string_pretty(&location).unwrap(),
    )
    .unwrap();

    // The lake is made by the first `serve`, which is its writer at once.
    let (mut writer, w) = serve(&first);
    ready(w, 200);
    let item = json!({"declaration": {"declare": "type", "name": "item",
        "fields": [{"name": "n", "type": {"kind": "number"}}]}});
    let declared = ask(w, "POST", "/lakes/finance/schema/declare", Some(item));
    assert_eq!(declared.0, 200, "{}", declared.1);
    let put = ask(
        w,
        "PUT",
        "/lakes/finance/types/item/records/i0",
        Some(json!({"n": 0})),
    );
    assert_eq!(put.0, 200, "{}", put.1);

    let (mut standby, s) = serve(&second);
    let read = ask(s, "GET", "/lakes/finance/types/item/records/i0", None);
    assert_eq!(
        read.0, 200,
        "the standby must serve the writer's write: {}",
        read.1
    );
    let refused = ask(
        s,
        "PUT",
        "/lakes/finance/types/item/records/i1",
        Some(json!({"n": 1})),
    );
    assert_ne!(
        refused.0, 200,
        "the standby must refuse a write while the writer holds the lease"
    );
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        ask(s, "GET", "/readyz", None).0,
        503,
        "the standby must not be ready while the writer holds the lease"
    );

    writer.kill().unwrap();
    writer.wait().unwrap();
    ready(s, 200);
    let put = ask(
        s,
        "PUT",
        "/lakes/finance/types/item/records/i1",
        Some(json!({"n": 1})),
    );
    assert_eq!(
        put.0, 200,
        "the standby must serve a write once it holds the lease: {}",
        put.1
    );
    drop(standby.stdin.take());
    assert!(standby.wait().unwrap().success());
}
