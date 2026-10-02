//! Fields computed by a backing outside the lake, under concurrent writes:
//! each write answers a position from which its own filled field is
//! visible, even when a neighbour's drain made its call.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use lakelands_api::engine::OpenMode;
use lakelands_api::seam::{FunctionBacking, FunctionCall, Seams};
use lakelands_api::write::Declaration;
use lakelands_api::{
    Get, Grants, LakeStore, Location, Principal, PrincipalKind, ReadAt, Result, Value, WriteContext,
};
use lakelands_engine_mem::MemoryOpener;
use serde_json::json;

/// Echoes its input `code`, slowly, so a second write lands while the
/// first write's call is under way.
struct Slow;

#[async_trait]
impl FunctionBacking for Slow {
    fn kind(&self) -> &str {
        "slow"
    }

    async fn call(&self, call: &FunctionCall) -> Result<Value> {
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(call.inputs["code"].clone())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_racing_writes_each_answer_a_position_their_fill_is_visible_from() {
    let seams = Seams {
        engines: vec![Arc::new(MemoryOpener::new())],
        backings: vec![Arc::new(Slow)],
        ..Seams::default()
    };
    let store: Arc<dyn LakeStore> =
        lakelands_store::open(Location::memory("race"), seams, OpenMode::Create)
            .await
            .unwrap();
    let owner = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: "race".into(),
        grants: Grants::owner(),
    };
    let cat = store.catalog(WriteContext::default());
    let declare = |v: Value| serde_json::from_value::<Declaration>(v).unwrap();
    cat.declare(
        &owner,
        declare(
            json!({"declare": "function", "name": "lookup", "backing": "slow",
                       "inputs": [{"name": "code", "type": {"kind": "text"}}]}),
        ),
    )
    .await
    .unwrap();
    cat.declare(
        &owner,
        declare(json!({"declare": "type", "name": "place", "fields": [
            {"name": "code", "type": {"kind": "text"}},
            {"name": "name", "type": {"kind": "text"}, "authority": "derived",
             "function": "lookup", "depends_on": ["code"]}]})),
    )
    .await
    .unwrap();

    let write = |key: &'static str, code: &'static str, after: u64| {
        let (cat, owner) = (cat.clone(), owner.clone());
        async move {
            tokio::time::sleep(Duration::from_millis(after)).await;
            let patch = serde_json::from_value(json!({ "code": code })).unwrap();
            cat.upsert(
                &owner,
                &"place".into(),
                &key.into(),
                patch,
                Default::default(),
            )
            .await
            .unwrap()
        }
    };
    // The second write lands while the first write's call is under way:
    // the first writer's drain makes the second's call too.
    let (a, b) = tokio::join!(write("p1", "AT", 0), write("p2", "DE", 50));
    for (key, code, outcome) in [("p1", "AT", a), ("p2", "DE", b)] {
        let at = outcome.position();
        let r = cat
            .get(
                &owner,
                &"place".into(),
                Get {
                    key: key.into(),
                    with_meta: false,
                    at: ReadAt {
                        as_of: Some(at),
                        valid_at: None,
                    },
                    params: Default::default(),
                },
            )
            .await
            .unwrap()
            .data;
        assert_eq!(
            r.fields.get("name"),
            Some(&json!(code)),
            "`{key}`'s write answered position {at}, from which its filled `name` must be \
             visible; it reads {r:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_write_with_nothing_due_does_not_wait_behind_a_call_under_way() {
    let seams = Seams {
        engines: vec![Arc::new(MemoryOpener::new())],
        backings: vec![Arc::new(Slow)],
        ..Seams::default()
    };
    let store: Arc<dyn LakeStore> =
        lakelands_store::open(Location::memory("lane"), seams, OpenMode::Create)
            .await
            .unwrap();
    let owner = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: "lane".into(),
        grants: Grants::owner(),
    };
    let cat = store.catalog(WriteContext::default());
    let declare = |v: Value| serde_json::from_value::<Declaration>(v).unwrap();
    for d in [
        json!({"declare": "function", "name": "lookup", "backing": "slow",
               "inputs": [{"name": "code", "type": {"kind": "text"}}]}),
        json!({"declare": "type", "name": "place", "fields": [
            {"name": "code", "type": {"kind": "text"}},
            {"name": "name", "type": {"kind": "text"}, "authority": "derived",
             "function": "lookup", "depends_on": ["code"]}]}),
        json!({"declare": "type", "name": "note", "fields": [
            {"name": "text", "type": {"kind": "text"}}]}),
    ] {
        cat.declare(&owner, declare(d)).await.unwrap();
    }
    let slow = {
        let (cat, owner) = (cat.clone(), owner.clone());
        async move {
            let patch = serde_json::from_value(json!({"code": "AT"})).unwrap();
            cat.upsert(
                &owner,
                &"place".into(),
                &"p1".into(),
                patch,
                Default::default(),
            )
            .await
            .unwrap();
        }
    };
    let quick = {
        let (cat, owner) = (cat.clone(), owner.clone());
        async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let started = std::time::Instant::now();
            let patch = serde_json::from_value(json!({"text": "hi"})).unwrap();
            cat.upsert(
                &owner,
                &"note".into(),
                &"n1".into(),
                patch,
                Default::default(),
            )
            .await
            .unwrap();
            started.elapsed()
        }
    };
    let ((), took) = tokio::join!(slow, quick);
    assert!(
        took < Duration::from_millis(120),
        "a write with nothing due answered after {took:?}, behind the 200 ms call"
    );
}
