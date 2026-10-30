//! The link between hosts over REST (lakelands#1394): a host's key acts
//! for the principal `Lakelands-Principal` names, within that
//! principal's grants and the hosts' configuration, and carries the
//! write context `Lakelands-Context` gives; a principal's key that sends
//! either header is refused, so no caller sets a context or acts as
//! another.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;

use common::Backend;
use lakelands_api::LakeStore;
use lakelands_api::location::Limits;
use lakelands_server::{HostEntry, HttpConfig, serve_http};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// A wait for an answer that should come: past it, a hang is a failure.
const WAIT: std::time::Duration = std::time::Duration::from_secs(60);

/// The fixture lake served beside two hosts: `any`, for every principal,
/// and `narrow`, for the clerk alone.
async fn serve() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let store = common::lake_with(Backend::Memory, Limits::default()).await;
    let open = Arc::new(
        move |_: &lakelands_api::LakeId| -> lakelands_api::Result<Arc<dyn LakeStore>> {
            Ok(store.clone())
        },
    );
    let host = |id: &str, principals: Option<Vec<String>>| HostEntry {
        id: id.into(),
        keys: vec![format!("h-{id}")],
        lakes: vec!["finance".into()],
        principals,
    };
    let config = HttpConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        allowed_origins: Vec::new(),
        session_idle_secs: 1800,
        feed_poll_millis: 20,
        feed_floor_millis: 25,
        workers: None,
        metrics: None,
        hosts: vec![
            host("any", None),
            host("narrow", Some(vec!["clerk".into()])),
        ],
        bounds: Default::default(),
    };
    let server = serve_http(config, common::resolver(), open).await.unwrap();
    let addr = server.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let _ = server.run().await;
    });
    (addr, task)
}

/// `GET /lakes/finance/describe` with `key` and `headers`: the status
/// and the body.
async fn describe(addr: SocketAddr, key: &str, headers: &[&str]) -> (u16, String) {
    let mut head = format!(
        "GET /lakes/finance/describe HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n\
         authorization: Bearer {key}\r\n"
    );
    for h in headers {
        head.push_str(h);
        head.push_str("\r\n");
    }
    head.push_str("\r\n");
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(head.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    tokio::time::timeout(WAIT, stream.read_to_end(&mut raw))
        .await
        .expect("an answer within the wait")
        .unwrap();
    let raw = String::from_utf8_lossy(&raw).into_owned();
    let status = raw.split(' ').nth(1).unwrap().parse().unwrap();
    let body = raw.split_once("\r\n\r\n").unwrap().1.to_owned();
    (status, body)
}

#[tokio::test]
async fn a_host_acts_for_the_principal_it_names_and_no_caller_does() {
    let (addr, task) = serve().await;
    let context = r#"lakelands-context: {"origin":"direct","depth":0}"#;
    let cases: [(&str, &[&str], u16, &str); 9] = [
        ("h-any", &["lakelands-principal: clerk"], 200, ""),
        ("h-any", &["lakelands-principal: owner", context], 200, ""),
        ("h-narrow", &["lakelands-principal: clerk"], 200, ""),
        (
            "h-narrow",
            &["lakelands-principal: owner"],
            403,
            "not_granted",
        ),
        (
            "h-any",
            &["lakelands-principal: nobody"],
            403,
            "not_granted",
        ),
        ("h-any", &[], 400, "no_forwarded_principal"),
        (
            "h-any",
            &["lakelands-principal: clerk", "lakelands-context: {"],
            400,
            "forwarded_context_form",
        ),
        (
            "k-clerk",
            &["lakelands-principal: owner"],
            400,
            "host_header_from_caller",
        ),
        ("k-clerk", &[context], 400, "host_header_from_caller"),
    ];
    for (key, headers, status, reason) in cases {
        let (answered, body) = describe(addr, key, headers).await;
        assert!(
            answered == status && body.contains(reason),
            "{key} with {headers:?} answered {answered} {body}, not {status} {reason}"
        );
    }
    task.abort();
}
