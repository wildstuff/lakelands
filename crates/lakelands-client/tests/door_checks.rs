//! The door checks, run unchanged on a lake reached over the REST door:
//! a fresh in-memory store per check, served by `serve_http` on
//! loopback, with a resolver whose entries are the checks' cast, each
//! principal with its grants and the key `k-<id>`, and a `RemoteLake`
//! keyed by those ids, and the statement checks through its `cypher`
//! over the route; the `cypher` tool's checks hold the MCP door's
//! session over the same store against that route. The checks that call a host operation, and those
//! that read where one in-process feed poll stops (`in_process_feed`),
//! are marked so in `door_checks!` and are ignored here, each saying why.

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use lakelands_api::id::Cursor;
use lakelands_api::location::Limits;
use lakelands_api::{Error, LakeStore, Params, Principal, ReadAt};
use lakelands_client::RemoteLake;
use lakelands_door_checks::statements::{Statements, Tools};
use lakelands_server::Session;
use serde_json::{Value, json};

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("a runtime")
    })
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    runtime().block_on(f)
}

/// A fresh lake name: every check gets its own lake.
fn fresh() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "finance-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn served() -> Arc<RemoteLake> {
    let store = block_on(common::memory_lake(&fresh(), Limits::default()));
    let (addr, keys) = block_on(common::serve(store.clone()));
    Arc::new(common::remote(addr, store.lake(), keys))
}

fn remote() -> Arc<dyn LakeStore> {
    served()
}

/// The statement door over REST: the client's `cypher`, answered as JSON.
struct OverRest(Arc<RemoteLake>);

#[async_trait]
impl Statements for OverRest {
    async fn cypher(
        &self,
        who: &Principal,
        statement: &str,
        params: &Params,
        at: ReadAt,
        cursor: Option<&Cursor>,
    ) -> Result<Value, Error> {
        let answer = self.0.cypher(who, statement, params, at, cursor).await?;
        Ok(serde_json::to_value(answer).expect("an answer is JSON"))
    }

    async fn enact(
        &self,
        who: &Principal,
        statement: &str,
        params: &Params,
        plan_id: &str,
    ) -> Result<Value, Error> {
        let answer = self.0.cypher_enact(who, statement, params, plan_id).await?;
        Ok(serde_json::to_value(answer).expect("an answer is JSON"))
    }
}

fn remote_with_statements() -> (Arc<dyn LakeStore>, Arc<dyn Statements>) {
    let lake = served();
    (lake.clone(), Arc::new(OverRest(lake)))
}

/// The MCP door's tools: a fresh session of the principal over the
/// store the route serves, for every request.
struct OverMcp(Arc<dyn LakeStore>);

impl OverMcp {
    async fn result(&self, who: &Principal, method: &str, params: Value) -> Value {
        let mut session = Session::new(self.0.clone(), who.clone(), false);
        let request = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        let answer = session.handle(request).await.remove(0);
        let result = answer.get("result").cloned();
        result.unwrap_or_else(|| panic!("`{method}` is answered: {answer}"))
    }
}

#[async_trait]
impl Tools for OverMcp {
    async fn list(&self, who: &Principal) -> Vec<Value> {
        let listed = self.result(who, "tools/list", json!({})).await;
        serde_json::from_value(listed["tools"].clone()).expect("the tools")
    }

    async fn call(&self, who: &Principal, name: &str, arguments: Value) -> Result<Value, Error> {
        let params = json!({"name": name, "arguments": arguments});
        let answer = self.result(who, "tools/call", params).await;
        if answer["isError"] != json!(true) {
            return Ok(answer["structuredContent"].clone());
        }
        let error = answer["content"][1]["text"].as_str().unwrap_or_default();
        let error = serde_json::from_str(error);
        Err(error.unwrap_or_else(|e| panic!("a refused call carries its error ({e}): {answer}")))
    }
}

fn remote_with_tools() -> (Arc<dyn LakeStore>, Arc<dyn Statements>, Arc<dyn Tools>) {
    let store = block_on(common::memory_lake(&fresh(), Limits::default()));
    let (addr, keys) = block_on(common::serve(store.clone()));
    let lake = Arc::new(common::remote(addr, store.lake(), keys));
    (
        lake.clone(),
        Arc::new(OverRest(lake)),
        Arc::new(OverMcp(store)),
    )
}

lakelands_door_checks::door_checks!(remote, block_on, remote);
lakelands_door_checks::statement_checks!(remote_with_statements, block_on);
lakelands_door_checks::cypher_tool_checks!(remote_with_tools, block_on);

fn remote_with_search_tool() -> (Arc<dyn LakeStore>, Arc<dyn Tools>) {
    let (lake, _, tools) = remote_with_tools();
    (lake, tools)
}

lakelands_door_checks::search_field_checks!(remote_with_search_tool, block_on);
