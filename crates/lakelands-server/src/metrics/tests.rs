//! The metrics door: what a scrape prints, and what it answers over the
//! wire.

use std::net::SocketAddr;
use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::seam::{Count, Level, Metered, Seams, Spread};
use lakelands_api::write::Declaration;
use lakelands_api::{Grants, LakeId, LakeStore, Location, Principal, PrincipalKind, WriteContext};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::{Metrics, MetricsConfig, TEXT_FORMAT, serve_metrics};

/// The status line, the headers and the body of `method path`.
async fn ask(addr: SocketAddr, method: &str, path: &str) -> (u16, String, String) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let req = format!("{method} {path} HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    let status = out.split(' ').nth(1).unwrap().parse().unwrap();
    let (head, body) = out.split_once("\r\n\r\n").unwrap();
    (status, head.to_lowercase(), body.to_owned())
}

#[test]
fn a_lake_told_nothing_is_not_shown_and_one_told_is_labelled() {
    let metrics = Metrics::new();
    let quiet = LakeId::from("quiet");
    let _ = metrics.lake(&quiet);
    let lake = LakeId::from("a\"b\\c\nd");
    let m = metrics.lake(&lake);
    let at = || Metered {
        lake: &lake,
        engine: "memory",
    };
    m.count(at(), Count::Writes, 3);
    m.count(at(), Count::Writes, 2);
    m.level(at(), Level::Tip, 41);
    m.observe(at(), Spread::Append, 300);
    m.observe(at(), Spread::Append, 20_000_000);
    m.observe(at(), Spread::BatchWrites, 3);
    let text = metrics.render();
    let labels = r#"lake="a\"b\\c\nd",engine="memory""#;
    for line in [
        "# TYPE lakelands_writes_total counter".to_owned(),
        format!("lakelands_writes_total{{{labels}}} 5"),
        "# TYPE lakelands_tip gauge".to_owned(),
        format!("lakelands_tip{{{labels}}} 41"),
        "# TYPE lakelands_append_seconds histogram".to_owned(),
        format!("lakelands_append_seconds_bucket{{{labels},le=\"0.00025\"}} 0"),
        format!("lakelands_append_seconds_bucket{{{labels},le=\"0.0005\"}} 1"),
        format!("lakelands_append_seconds_bucket{{{labels},le=\"10\"}} 1"),
        format!("lakelands_append_seconds_bucket{{{labels},le=\"+Inf\"}} 2"),
        format!("lakelands_append_seconds_sum{{{labels}}} 20.0003"),
        format!("lakelands_append_seconds_count{{{labels}}} 2"),
        format!("lakelands_batch_writes_bucket{{{labels},le=\"2\"}} 0"),
        format!("lakelands_batch_writes_bucket{{{labels},le=\"4\"}} 1"),
        format!("lakelands_batch_writes_sum{{{labels}}} 3"),
    ] {
        assert!(
            text.lines().any(|l| l == line),
            "`{line}` is not in:\n{text}"
        );
    }
    assert!(
        !text.contains("quiet"),
        "a lake told nothing is shown:\n{text}"
    );
    assert!(
        !text.contains("lakelands_lease_held{"),
        "a level never told is shown:\n{text}"
    );
    let again = metrics.lake(&lake);
    again.count(at(), Count::Writes, 1);
    assert!(
        metrics
            .render()
            .contains(&format!("lakelands_writes_total{{{labels}}} 6"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_scrape_over_the_wire_shows_a_lakes_writes_and_nothing_it_holds() {
    let metrics = Metrics::new();
    let id = LakeId::from("served");
    let seams = Seams {
        engines: vec![Arc::new(lakelands_engine_mem::MemoryOpener::new())],
        meter: Some(metrics.lake(&id)),
        ..Seams::default()
    };
    let store: Arc<dyn LakeStore> =
        lakelands_store::open(Location::memory("served"), seams, OpenMode::Create)
            .await
            .unwrap();
    let owner = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: id.clone(),
        grants: Grants::owner(),
    };
    let cat = store.catalog(WriteContext::default());
    let ty = serde_json::json!({"declare": "type", "name": "secret", "fields": [
        {"name": "word", "type": {"kind": "text"}}]});
    let ty: Declaration = serde_json::from_value(ty).unwrap();
    cat.declare(&owner, ty).await.unwrap();
    let patch = serde_json::from_value(serde_json::json!({"word": "swordfish"})).unwrap();
    let o = cat
        .upsert(
            &owner,
            &"secret".into(),
            &"s1".into(),
            patch,
            Default::default(),
        )
        .await
        .unwrap();
    let config = MetricsConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
    };
    let mut server = serve_metrics(&config, metrics).await.unwrap();
    server.scrape = std::time::Duration::from_millis(300);
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.run());
    let (status, head, body) = ask(addr, "GET", "/metrics").await;
    assert_eq!(status, 200, "{body}");
    assert!(
        head.contains(&format!("content-type: {TEXT_FORMAT}")),
        "{head}"
    );
    let labels = r#"lake="served",engine="memory""#;
    assert!(
        body.contains(&format!("lakelands_writes_total{{{labels}}} 1")),
        "{body}"
    );
    let tip = format!("lakelands_tip{{{labels}}} {}", o.position().0);
    assert!(body.lines().any(|l| l == tip), "`{tip}` is not in:\n{body}");
    for held in ["swordfish", "secret", "s1", "word"] {
        assert!(!body.contains(held), "a scrape tells `{held}`:\n{body}");
    }
    assert_eq!(ask(addr, "GET", "/other").await.0, 404);
    let (status, head, _) = ask(addr, "POST", "/metrics").await;
    assert_eq!(status, 405);
    assert!(head.contains("allow: get, head"), "{head}");
    let (status, head, body) = ask(addr, "HEAD", "/metrics").await;
    assert_eq!(status, 200);
    assert!(head.contains(TEXT_FORMAT), "{head}");
    assert!(body.is_empty(), "a HEAD is answered with a body: {body}");
    let mut idle = TcpStream::connect(addr).await.unwrap();
    let mut rest = Vec::new();
    let closed = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        idle.read_to_end(&mut rest),
    );
    assert!(
        closed.await.is_ok(),
        "a connection that asks nothing is kept open past a scrape's time"
    );
}
