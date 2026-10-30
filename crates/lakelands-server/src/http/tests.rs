//! The liveness and readiness routes, state by state.

use std::net::SocketAddr;
use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::seam::Seams;
use lakelands_api::{LakeId, LakeStore, Location};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::{HttpConfig, serve_http};
use crate::resolver::StaticResolver;

/// A door on a free port that serves no lake; its health handle.
async fn door() -> (SocketAddr, super::Health) {
    let open = Arc::new(|_: &LakeId| -> lakelands_api::Result<Arc<dyn LakeStore>> {
        unreachable!("a probe opens no lake")
    });
    let config = HttpConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        allowed_origins: vec![],
        session_idle_secs: 1800,
        feed_poll_millis: 250,
        feed_floor_millis: 10,
        workers: None,
        metrics: None,
        hosts: Vec::new(),
        bounds: Default::default(),
    };
    let server = serve_http(config, Arc::new(StaticResolver::new(vec![])), open)
        .await
        .unwrap();
    let (addr, health) = (server.local_addr().unwrap(), server.health());
    tokio::spawn(async move {
        let _ = server.run().await;
    });
    (addr, health)
}

/// The status line and the body of `method path`, sent with no key.
async fn ask(addr: SocketAddr, method: &str, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    let req = format!("{method} {path} HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n\r\n");
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    let status = out.split(' ').nth(1).unwrap().parse().unwrap();
    let body = out.split_once("\r\n\r\n").unwrap().1.to_owned();
    (status, body)
}

async fn lake(
    name: &str,
    mode: OpenMode,
    opener: &lakelands_engine_mem::MemoryOpener,
) -> Arc<dyn LakeStore> {
    let seams = Seams {
        engines: vec![Arc::new(opener.clone())],
        ..Seams::default()
    };
    lakelands_store::open(Location::memory(name), seams, mode)
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_door_still_opening_its_lakes_is_up_and_starting() {
    let (addr, _health) = door().await;
    assert_eq!(
        ask(addr, "GET", "/livez").await,
        (200, r#"{"status":"up"}"#.into())
    );
    assert_eq!(
        ask(addr, "GET", "/readyz").await,
        (503, r#"{"status":"starting"}"#.into())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_door_whose_lakes_are_open_and_leased_is_ready() {
    let (addr, health) = door().await;
    let opener = lakelands_engine_mem::MemoryOpener::new();
    health.serving(vec![lake("a", OpenMode::Create, &opener).await]);
    assert_eq!(ask(addr, "GET", "/livez").await.0, 200);
    assert_eq!(
        ask(addr, "GET", "/readyz").await,
        (200, r#"{"status":"ready"}"#.into())
    );
    // HEAD is answered like GET, with no body.
    assert_eq!(ask(addr, "HEAD", "/readyz").await, (200, String::new()));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lake_whose_handle_gave_its_lease_up_is_not_ready_and_the_door_is_up() {
    let (addr, health) = door().await;
    let opener = lakelands_engine_mem::MemoryOpener::new();
    let mine = lake("a", OpenMode::Create, &opener).await;
    let healthy = lake("b", OpenMode::Create, &opener).await;
    health.serving(vec![healthy, mine.clone()]);
    assert_eq!(ask(addr, "GET", "/readyz").await.0, 200);
    // The handle's lease is given up and another writer takes it. A lease
    // lost without the handle noticing is not seen until its next write,
    // which the docs state as the limit.
    mine.close().await.unwrap();
    let other = lake("a", OpenMode::Write, &opener).await;
    assert!(other.is_writer());
    assert_eq!(
        ask(addr, "GET", "/readyz").await,
        (503, r#"{"status":"not_ready"}"#.into())
    );
    assert_eq!(ask(addr, "GET", "/livez").await.0, 200);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lake_opened_to_read_only_holds_no_lease_and_is_not_ready() {
    let (addr, health) = door().await;
    let opener = lakelands_engine_mem::MemoryOpener::new();
    let writer = lake("a", OpenMode::Create, &opener).await;
    health.serving(vec![lake("a", OpenMode::Read, &opener).await]);
    assert_eq!(ask(addr, "GET", "/readyz").await.0, 503);
    drop(writer);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_routes_take_no_key_carry_no_lake_and_refuse_other_methods() {
    let (addr, health) = door().await;
    let opener = lakelands_engine_mem::MemoryOpener::new();
    health.serving(vec![lake("secret-lake", OpenMode::Create, &opener).await]);
    let (_, body) = ask(addr, "GET", "/readyz").await;
    assert!(!body.contains("secret-lake"), "{body}");
    assert_eq!(ask(addr, "POST", "/livez").await.0, 405);
    assert_eq!(ask(addr, "DELETE", "/readyz").await.0, 405);
}

/// Takes the lake's lease from under the handle: released behind the
/// store's back, as a lost session does on PostgreSQL, and then taken by
/// another writer. Answers the engine holding it.
fn lease_taken(
    opener: &lakelands_engine_mem::MemoryOpener,
    name: &str,
) -> Box<dyn lakelands_api::engine::LakeEngine> {
    use lakelands_api::engine::{EngineOpener, Lease};
    use lakelands_api::error::Situation;

    let engine = opener
        .open(&Location::memory(name), OpenMode::Write)
        .unwrap();
    let held = engine.take_lease("another writer").unwrap_err();
    let Some(Situation::LeaseHeld { holder, fence }) = held.situation().cloned() else {
        panic!("the lease is held: {held:?}");
    };
    engine.release_lease(&Lease { fence, holder }).unwrap();
    engine.take_lease("another writer").unwrap();
    engine
}

/// A write through the lake: its answer.
async fn declare(store: &dyn LakeStore) -> lakelands_api::Result<()> {
    use lakelands_api::principal::{Grants, PrincipalKind};
    use lakelands_api::{Principal, WriteContext};

    let who = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: "a".into(),
        grants: Grants::owner(),
    };
    let d = serde_json::json!({"declare": "type", "name": "t", "fields": [{"name": "n", "type": {"kind": "number"}}]});
    let d: lakelands_api::write::Declaration = serde_json::from_value(d).unwrap();
    store
        .catalog(WriteContext::default())
        .declare(&who, d)
        .await
        .map(|_| ())
}

/// A lease another writer took is refused a write as stale (`LeaseStale`,
/// on the memory and SQLite engines), and the handle is no writer from
/// then on, so `/readyz` says not ready, as after a lost PostgreSQL
/// session (`LeaseNotHeld`, which `lakelands-engine-pg`'s `dropped.rs`
/// pins). The conformance suite holds the handle to it on every engine.
#[tokio::test(flavor = "multi_thread")]
async fn a_lease_another_writer_took_is_refused_and_the_lake_is_not_ready() {
    let (addr, health) = door().await;
    let opener = lakelands_engine_mem::MemoryOpener::new();
    let store = lake("a", OpenMode::Create, &opener).await;
    health.serving(vec![store.clone()]);
    let _engine = lease_taken(&opener, "a");
    assert!(declare(store.as_ref()).await.is_err());
    assert!(!store.is_writer());
    assert_eq!(
        ask(addr, "GET", "/readyz").await,
        (503, r#"{"status":"not_ready"}"#.into())
    );
}

/// A server runs on as many threads as `workers` names, and, where it
/// names none, on the machine's cores, at most [`super::WORKERS_MOST`];
/// `workers` is a count above zero.
#[test]
fn a_server_runs_on_the_threads_its_configuration_names() {
    use tokio::runtime::RuntimeFlavor;
    let config = |workers: serde_json::Value| -> serde_json::Result<HttpConfig> {
        let mut c = serde_json::json!({"bind": "127.0.0.1:0"});
        if !workers.is_null() {
            c["workers"] = workers;
        }
        serde_json::from_value(c)
    };
    let threads = |c: Option<&HttpConfig>| {
        let r = super::runtime(c).unwrap();
        assert_eq!(r.handle().runtime_flavor(), RuntimeFlavor::MultiThread);
        r.metrics().num_workers()
    };
    assert_eq!(threads(Some(&config(3.into()).unwrap())), 3);
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let named_none = config(serde_json::Value::Null).unwrap();
    assert_eq!(threads(Some(&named_none)), cores.min(super::WORKERS_MOST));
    assert_eq!(threads(None), cores.min(super::WORKERS_MOST));
    assert!(config(0.into()).is_err(), "no threads is refused");
    // Named none, it is written as none.
    assert!(
        serde_json::to_value(&named_none)
            .unwrap()
            .get("workers")
            .is_none()
    );
}
