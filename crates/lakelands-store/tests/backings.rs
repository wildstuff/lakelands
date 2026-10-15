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

/// Tells when its call is under way, and answers only when the test
/// releases it: a write's order against the call is set by the signal,
/// never by a clock (lakelands#1051).
struct Held {
    started: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

#[async_trait]
impl FunctionBacking for Held {
    fn kind(&self) -> &str {
        "held"
    }

    async fn call(&self, call: &FunctionCall) -> Result<Value> {
        self.started.notify_one();
        self.release.notified().await;
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
    let (started, release) = (Arc::default(), Arc::default());
    let held = Held {
        started: Arc::clone(&started),
        release: Arc::clone(&release),
    };
    let seams = Seams {
        engines: vec![Arc::new(MemoryOpener::new())],
        backings: vec![Arc::new(held)],
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
        json!({"declare": "function", "name": "lookup", "backing": "held",
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
    // The write whose call is held, under way before the other starts.
    let slow = {
        let (cat, owner) = (cat.clone(), owner.clone());
        tokio::spawn(async move {
            let patch = serde_json::from_value(json!({"code": "AT"})).unwrap();
            let (ty, key) = ("place".into(), "p1".into());
            cat.upsert(&owner, &ty, &key, patch, Default::default())
                .await
                .unwrap();
        })
    };
    started.notified().await;
    // A write with nothing due answers while that call is still held. The
    // bound only keeps a write that waits behind it from hanging the test.
    let patch = serde_json::from_value(json!({"text": "hi"})).unwrap();
    let (ty, key) = ("note".into(), "n1".into());
    let quick = cat.upsert(&owner, &ty, &key, patch, Default::default());
    let answered = tokio::time::timeout(Duration::from_secs(30), quick).await;
    assert!(
        matches!(answered, Ok(Ok(_))),
        "a write with nothing due answered {answered:?} while a call was held: it waited behind it"
    );
    assert!(
        !slow.is_finished(),
        "the held call's write answered before it was released"
    );
    release.notify_one();
    slow.await.unwrap();
}
