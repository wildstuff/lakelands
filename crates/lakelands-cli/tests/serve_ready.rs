//! `lakelands serve` answers `/livez` and `/readyz` from the moment its
//! door is bound, before any lake is open.

use std::fs;
use std::future::Future;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

use lakelands_api::engine::OpenMode;
use lakelands_api::location::Location;
use lakelands_api::seam::Seams;
use lakelands_cli::Opening;

const WAIT: Duration = Duration::from_secs(60);

fn block_on<F: Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(f)
}

fn root(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("serve-ready")
        .join(name)
}

/// A port nothing listens on now.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// `GET path`: its status and body, or none where nothing answers.
fn get(addr: SocketAddr, path: &str) -> Option<(u16, String)> {
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).ok()?;
    write!(
        s,
        "GET {path} HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n\r\n"
    )
    .ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    let status = out.split(' ').nth(1)?.parse().ok()?;
    Some((status, out.split_once("\r\n\r\n")?.1.to_owned()))
}

/// A root with one lake and a door on `port`.
fn made(name: &str, port: u16) -> PathBuf {
    let dir = root(name);
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
    let file = dir.join("lakelands.json");
    let mut config: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    config["server"] = serde_json::json!({"bind": format!("127.0.0.1:{port}")});
    fs::write(&file, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    dir
}

/// While the primary lake's open is held up the door is up and starting;
/// an open that then fails ends the command, and the door with it.
#[test]
fn the_door_answers_while_the_primary_lake_is_opening_and_ends_with_a_failed_open() {
    const SHUT: u8 = 0;
    const RELEASED: u8 = 1;
    static GATE: AtomicU8 = AtomicU8::new(SHUT);

    let port = free_port();
    let dir = made("held", port);
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let open = |l: Location, m: OpenMode, s: Seams| -> Opening {
        Box::pin(async move {
            while GATE.load(Ordering::SeqCst) == SHUT {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            // The open fails once let go, as a damaged lake's does.
            let _ = (l, m, s);
            Err(lakelands_api::Error::unavailable(
                lakelands_api::error::Situation::Busy {
                    lake: "finance".into(),
                },
            ))
        })
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let argv = [
        "lakelands",
        "--root",
        dir.to_str().unwrap(),
        "serve",
        "--mcp",
    ];
    let probe = async {
        let started = Instant::now();
        let live = loop {
            if let Some(a) = tokio::task::spawn_blocking(move || get(addr, "/livez"))
                .await
                .unwrap()
            {
                break a;
            }
            assert!(started.elapsed() < WAIT, "the door never answered");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert_eq!(live, (200, r#"{"status":"up"}"#.into()));
        let ready = tokio::task::spawn_blocking(move || get(addr, "/readyz"))
            .await
            .unwrap();
        assert_eq!(ready, Some((503, r#"{"status":"starting"}"#.into())));
        GATE.store(RELEASED, Ordering::SeqCst);
    };
    let (code, ()) = block_on(async {
        tokio::join!(lakelands_cli::run(argv, &mut out, &mut err, &open), probe)
    });
    assert_eq!(code, 1, "{}", String::from_utf8_lossy(&err));
    // The command ended with its failed open: no door is left answering.
    assert_eq!(get(addr, "/livez"), None);
}

/// A served lake: `/livez` and `/readyz` answer 200 once every lake is open.
#[test]
fn a_served_lake_is_ready() {
    let port = free_port();
    let dir = made("ready", port);
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let mut server = Command::new(env!("CARGO_BIN_EXE_lakelands"))
        .arg("--root")
        .arg(&dir)
        .args(["serve", "--mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let started = Instant::now();
    let ready = loop {
        match get(addr, "/readyz") {
            Some((503, body)) => assert_eq!(body, r#"{"status":"starting"}"#),
            // Not bound yet.
            None => {}
            other => break other,
        }
        assert!(server.try_wait().unwrap().is_none(), "the server ended");
        assert!(started.elapsed() < WAIT, "never ready");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(ready, Some((200, r#"{"status":"ready"}"#.into())));
    assert_eq!(get(addr, "/livez").map(|a| a.0), Some(200));
    drop(server.stdin.take());
    assert!(server.wait().unwrap().success());
}
