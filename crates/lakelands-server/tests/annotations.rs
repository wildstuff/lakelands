//! A host's annotations at the JSON doors, over REST and over MCP, on the
//! store over both engines: a declaration carrying them
//! comes back from `describe` with them, exactly, on the type, its field,
//! its relation and the relation's link field; a field the principal may
//! not read, a link field included, is described without them; and a key
//! without a namespace, or in a reserved one, is refused, naming it, with
//! the lake unmoved.
//!
//! `schema/declare` is REST's alone; MCP declares through `declare_type`.

mod common;

use common::wire::{Mcp, rest, serve};
use common::{BACKENDS, Backend};

use std::sync::Arc;

use lakelands_api::principal::{Access, Grants, PrincipalKind, TypeGrants};
use lakelands_api::{LakeStore, Principal};
use lakelands_server::resolver::{Entry, StaticResolver};
use serde_json::{Value, json};

/// The type `memo`, annotated on the type, its field `body`, its relation
/// `about` and that relation's link field `since`.
fn memo() -> Value {
    json!({
        "name": "memo",
        "annotations": {"acme.icon": "note", "acme.package": {"name": "billing", "version": "2.1.0"}},
        "fields": [{
            "name": "body", "type": {"kind": "text"},
            "annotations": {"acme.format": "markdown", "acme.rank": 3},
        }],
        "relations": [{
            "name": "about", "target": "invoice",
            "annotations": {"acme.plural": "abouts"},
            "fields": [{
                "name": "since", "type": {"kind": "date_time"},
                "annotations": {"acme.shown": true},
            }],
        }],
    })
}

/// Reads everything, but sees `memo.body` and the link field `since`
/// masked.
fn masker_grants() -> Grants {
    let masked = |field: &str| TypeGrants {
        default: None,
        fields: [(field.into(), Access::Masked)].into(),
        ..Default::default()
    };
    Grants {
        default: Access::Read,
        types: [
            ("memo".into(), masked("body")),
            ("sys.link.memo.about".into(), masked("since")),
        ]
        .into(),
        ..Grants::default()
    }
}

fn masker() -> Principal {
    Principal {
        kind: PrincipalKind::Agent,
        id: "masker".into(),
        lake: "finance".into(),
        grants: masker_grants(),
    }
}

/// The server's principals, and the masker (key `k-masker`).
fn resolver() -> Arc<StaticResolver> {
    let mut entries = common::entries();
    entries.push(Entry {
        name: "masker".into(),
        kind: PrincipalKind::Agent,
        id: "masker".into(),
        lakes: vec!["finance".into()],
        grants: masker_grants(),
        api_keys: vec!["k-masker".into()],
    });
    Arc::new(StaticResolver::new(entries))
}

/// `memo` as the owner declared it, described to someone who reads it
/// all: every annotation comes back as it was sent.
fn described_whole(at: &str, d: &Value) {
    let m = memo();
    assert_eq!(d["annotations"], m["annotations"], "{at}: the type's: {d}");
    assert_eq!(
        d["fields"][0]["annotations"], m["fields"][0]["annotations"],
        "{at}: the field's: {d}"
    );
    let about = &d["relations"][0];
    assert_eq!(
        about["annotations"], m["relations"][0]["annotations"],
        "{at}: the relation's: {d}"
    );
    assert_eq!(
        about["fields"][0]["annotations"], m["relations"][0]["fields"][0]["annotations"],
        "{at}: the link field's: {d}"
    );
}

/// `memo` described to the masker: the type's and the relation's
/// annotations are shown; the masked field's and link field's are not.
fn described_masked(at: &str, d: &Value) {
    let m = memo();
    assert_eq!(d["annotations"], m["annotations"], "{at}: the type's: {d}");
    assert_eq!(d["fields"][0]["name"], "body", "{at}: {d}");
    assert!(
        d["fields"][0].get("annotations").is_none(),
        "{at}: a masked field is described with its annotations: {d}"
    );
    let about = &d["relations"][0];
    assert_eq!(
        about["annotations"], m["relations"][0]["annotations"],
        "{at}: the relation's: {d}"
    );
    assert!(
        about["fields"][0].get("annotations").is_none(),
        "{at}: a masked link field is described with its annotations: {d}"
    );
}

/// The keys refused, with the refusal each gets.
const REFUSED: [(&str, &str); 3] = [
    ("icon", "invalid"),
    ("sys.icon", "reserved"),
    ("lakelands.icon", "reserved"),
];

/// `memo` with one more annotation, `key`, on its field.
fn memo_with(key: &str) -> Value {
    let mut m = memo();
    m["name"] = json!("draft");
    m["fields"][0]["annotations"][key] = json!("x");
    m
}

// ------------------------------------------------------------------ REST

#[tokio::test]
async fn rest_carries_annotations_through_declare_and_describe() {
    for backend in BACKENDS {
        rest_on(backend).await;
    }
}

async fn rest_on(backend: Backend) {
    let store = common::lake_on(backend).await;
    let addr = serve(store.clone(), resolver()).await;
    let mut declaration = memo();
    declaration["declare"] = json!("type");
    let body = json!({"declaration": declaration});
    let (status, answer) = rest(addr, "k-owner", "POST", "schema/declare", Some(&body)).await;
    assert_eq!(status, 200, "{backend:?}: {answer}");

    let (status, answer) = rest(addr, "k-owner", "GET", "describe/memo", None).await;
    assert_eq!(status, 200, "{backend:?}: {answer}");
    described_whole(&format!("{backend:?}, REST, owner"), &answer["data"]);
    let (status, answer) = rest(addr, "k-masker", "GET", "describe/memo", None).await;
    assert_eq!(status, 200, "{backend:?}: {answer}");
    described_masked(&format!("{backend:?}, REST, masker"), &answer["data"]);

    let head = store.head().await.unwrap();
    for (key, refusal) in REFUSED {
        let at = format!("{backend:?}, REST, `{key}`");
        let mut declaration = memo_with(key);
        declaration["declare"] = json!("type");
        let body = json!({"declaration": declaration});
        let (status, answer) = rest(addr, "k-owner", "POST", "schema/declare", Some(&body)).await;
        let expected = if refusal == "reserved" { 403 } else { 422 };
        assert_eq!(status, expected, "{at}: {answer}");
        assert_eq!(answer["error"], "refused", "{at}: {answer}");
        assert_eq!(answer["reason"]["refusal"], refusal, "{at}: {answer}");
        let what = answer["what"].as_str().unwrap_or_default();
        assert!(
            answer["field"] == key || what.contains(&format!("`{key}`")),
            "{at}: the key is not named: {answer}"
        );
        assert_eq!(store.head().await.unwrap(), head, "{at}: the lake moved");
    }
}

// ------------------------------------------------------------------- MCP

#[tokio::test]
async fn mcp_carries_annotations_through_declare_type_and_describe() {
    for backend in BACKENDS {
        mcp_on(backend).await;
    }
}

async fn mcp_on(backend: Backend) {
    let store: Arc<dyn LakeStore> = common::lake_on(backend).await;
    let mut owner = Mcp::open(store.clone(), common::owner()).await;
    let result = owner.call("declare_type", json!({"type": memo()})).await;
    assert_eq!(result["isError"], false, "{backend:?}: {result}");

    let result = owner.call("describe", json!({"type": "memo"})).await;
    assert_eq!(result["isError"], false, "{backend:?}: {result}");
    described_whole(
        &format!("{backend:?}, MCP, owner"),
        &result["structuredContent"]["data"],
    );
    let mut masked = Mcp::open(store.clone(), masker()).await;
    let result = masked.call("describe", json!({"type": "memo"})).await;
    assert_eq!(result["isError"], false, "{backend:?}: {result}");
    described_masked(
        &format!("{backend:?}, MCP, masker"),
        &result["structuredContent"]["data"],
    );

    let head = store.head().await.unwrap();
    for (key, refusal) in REFUSED {
        let at = format!("{backend:?}, MCP, `{key}`");
        let result = owner
            .call("declare_type", json!({"type": memo_with(key)}))
            .await;
        assert_eq!(result["isError"], true, "{at}: {result}");
        let error: Value = serde_json::from_str(
            result["content"][1]["text"]
                .as_str()
                .expect("a contract fault carries its JSON"),
        )
        .unwrap();
        assert_eq!(error["reason"]["refusal"], refusal, "{at}: {error}");
        let text = result["content"][0]["text"].as_str().unwrap_or_default();
        assert!(
            error["field"] == key || text.contains(&format!("`{key}`")),
            "{at}: the key is not named: {result}"
        );
        assert_eq!(store.head().await.unwrap(), head, "{at}: the lake moved");
    }
}
