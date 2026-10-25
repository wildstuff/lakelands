//! `lakelands serve` with `server.metrics` set binds a meter to every lake
//! it serves, the primary one and the others alike, and answers a scrape
//! with each lake's writes on a listener of its own.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

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

/// The address after `prefix` on a line of `serve`'s standard error.
fn address(line: &str, prefix: &str) -> Option<SocketAddr> {
    let rest = line.strip_prefix(prefix)?;
    rest.split('/').next()?.parse().ok()
}

#[test]
fn serve_measures_the_primary_lake_and_every_other_it_serves() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("serve-metrics");
    let _ = fs::remove_dir_all(&dir);
    let bin = env!("CARGO_BIN_EXE_lakelands");
    let run = |args: &[&str]| {
        let out = Command::new(bin)
            .arg("--root")
            .arg(&dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run(&["init"]);
    run(&["lake", "new", "finance"]);
    run(&["lake", "new", "ledger"]);
    let file = dir.join("lakelands.json");
    let mut config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    config["server"] = json!({"bind": "127.0.0.1:0", "metrics": {"bind": "127.0.0.1:0"}});
    config["principals"] = json!([{"name": "writer", "kind": "agent", "id": "writer",
        "grants": {"default": "write", "schema": true, "restricted": true}, "api_keys": ["k-writer"]}]);
    fs::write(&file, serde_json::to_string_pretty(&config).unwrap()).unwrap();

    let mut server = Command::new(bin)
        .arg("--root")
        .arg(&dir)
        .args(["serve", "--lake", "finance", "--mcp"])
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
    let (mut scrape, mut door) = (None, None);
    while door.is_none() {
        let line = rx.recv_timeout(WAIT).expect("serve never said it serves");
        scrape = scrape.or(address(&line, "lakelands: metrics at http://"));
        door = address(&line, "lakelands: serving HTTP at http://");
    }
    let (scrape, door) = (
        scrape.expect("serve never said where its metrics are"),
        door.unwrap(),
    );

    let item = json!({"declaration": {"declare": "type", "name": "item",
        "fields": [{"name": "n", "type": {"kind": "number"}}]}});
    for (lake, writes) in [("finance", 1), ("ledger", 2)] {
        let declared = ask(
            door,
            "POST",
            &format!("/lakes/{lake}/schema/declare"),
            Some(item.clone()),
        );
        assert_eq!(declared.0, 200, "{lake}: {}", declared.1);
        for n in 0..writes {
            let path = format!("/lakes/{lake}/types/item/records/i{n}");
            let put = ask(door, "PUT", &path, Some(json!({"n": n})));
            assert_eq!(put.0, 200, "{lake}: {}", put.1);
        }
    }
    let (status, text) = ask(scrape, "GET", "/metrics", None);
    assert_eq!(status, 200, "{text}");
    for (lake, writes) in [("finance", 1), ("ledger", 2)] {
        let counted = text.lines().any(|l| {
            l.starts_with(&format!(
                "lakelands_writes_total{{lake=\"{lake}\",engine=\""
            )) && l.ends_with(&format!("}} {writes}"))
        });
        assert!(
            counted,
            "`{lake}` is not shown with {writes} writes:\n{text}"
        );
        let held = format!("lakelands_lease_held{{lake=\"{lake}\",");
        assert!(
            text.lines()
                .any(|l| l.starts_with(&held) && l.ends_with(" 1")),
            "`{lake}` is not shown holding its lease:\n{text}"
        );
    }
    drop(server.stdin.take());
    assert!(server.wait().unwrap().success());
}
