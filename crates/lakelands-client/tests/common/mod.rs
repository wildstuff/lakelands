//! A lake served over the REST door on loopback, and a client for it.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::location::Limits;
use lakelands_api::seam::Seams;
use lakelands_api::{LakeId, LakeStore, Location, Principal};
use lakelands_client::{ApiKey, RemoteConfig, RemoteLake};
use lakelands_mock::checks::Cast;
use lakelands_server::resolver::{Entry, StaticResolver};
use lakelands_server::{HttpConfig, serve_http};

/// A fresh, empty in-memory store.
pub async fn memory_lake(lake: &str, limits: Limits) -> Arc<dyn LakeStore> {
    let seams = Seams {
        engines: vec![Arc::new(lakelands_engine_mem::MemoryOpener::new())],
        ..Seams::default()
    };
    let location = Location {
        limits,
        ..Location::memory(lake)
    };
    lakelands_store::open(location, seams, OpenMode::Create)
        .await
        .expect("the lake opens")
}

/// The checks' cast, as a list.
pub fn cast(lake: &LakeId) -> Vec<Principal> {
    let c = Cast::of(lake);
    vec![
        c.owner,
        c.clerk,
        c.architect,
        c.auditor,
        c.reviewer,
        c.importer,
        c.stranger,
        c.keeper,
        c.tuner,
    ]
}

/// Serve `store` on loopback, resolving the cast by the keys `k-<id>`;
/// answer the address and the keys by principal id.
pub async fn serve(store: Arc<dyn LakeStore>) -> (SocketAddr, BTreeMap<String, ApiKey>) {
    let cast = cast(store.lake());
    let entries = cast
        .iter()
        .map(|p| Entry {
            name: p.id.clone(),
            kind: p.kind,
            id: p.id.clone(),
            lakes: Vec::new(),
            grants: p.grants.clone(),
            api_keys: vec![format!("k-{}", p.id)],
        })
        .collect();
    let keys = cast
        .iter()
        .map(|p| (p.id.clone(), ApiKey::from(format!("k-{}", p.id))))
        .collect();
    let open = Arc::new(
        move |_: &LakeId| -> lakelands_api::Result<Arc<dyn LakeStore>> { Ok(store.clone()) },
    );
    let config = HttpConfig {
        bind: "127.0.0.1:0".parse().unwrap(),
        allowed_origins: Vec::new(),
        session_idle_secs: 1800,
        feed_poll_millis: 20,
    };
    let server = serve_http(config, Arc::new(StaticResolver::new(entries)), open)
        .await
        .expect("the door serves");
    let addr = server.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = server.run().await;
    });
    (addr, keys)
}

/// The configuration of a client for a lake at `addr`.
pub fn config(addr: SocketAddr, lake: &LakeId, keys: BTreeMap<String, ApiKey>) -> RemoteConfig {
    RemoteConfig {
        url: format!("http://{addr}/"),
        lake: lake.clone(),
        keys,
        feed_key: Some("owner".into()),
        ca_file: None,
        timeout_secs: 30,
        max_answer_bytes: lakelands_client::DEFAULT_MAX_ANSWER_BYTES,
        limits: Limits::default(),
    }
}

/// A client for a lake at `addr`, reading the feed as its owner.
pub fn remote(addr: SocketAddr, lake: &LakeId, keys: BTreeMap<String, ApiKey>) -> RemoteLake {
    RemoteLake::connect(config(addr, lake, keys)).expect("the client is configured")
}
