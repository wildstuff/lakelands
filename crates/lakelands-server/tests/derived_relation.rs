//! A derived relation has no link door, so it takes no tool name
//! (lakelands#902): a lake with one whose link doors would be named too
//! long, and a verb named as one's link door would be, keeps its tools at
//! both JSON doors. Before, the store declared both and the generator
//! then refused the lake's whole catalog. Run on the store over both
//! engines.

mod common;

use common::wire::{Mcp, rest, serve};
use common::{BACKENDS, Backend};

use std::sync::Arc;

use lakelands_api::{LakeStore, WriteContext};
use serde_json::json;

/// The fixture lake with `inw`, whose derived relation's unlink door
/// would be named too long, and `inv` beside a verb named as the link
/// door of its derived relation would be.
async fn lake(backend: Backend) -> Arc<dyn LakeStore> {
    let store = common::lake_on(backend).await;
    let catalog = store.catalog(WriteContext::default());
    let derived = |name: &str, relation: &str| {
        json!({"declare": "type", "name": name,
               "fields": [{"name": "ref", "type": {"kind": "text"}}],
               "relations": [{"name": relation, "target": "payroll",
                              "derived": {"match": [{"from": "ref", "to": "code"}]}}]})
    };
    for d in [
        json!({"declare": "type", "name": "payroll",
               "fields": [{"name": "code", "type": {"kind": "text"}}]}),
        derived("inw", &"m".repeat(54)),
        derived("inv", "matched"),
        json!({"declare": "verb", "name": "inv_matched_link", "on": "payroll", "risk": "low",
               "effect": "write", "set": [{"field": "code", "value": "'x'"}]}),
    ] {
        let what = d.to_string();
        let d = serde_json::from_value(d).expect("a declaration");
        let declared = catalog.declare(&common::owner(), d).await;
        declared.unwrap_or_else(|e| panic!("{backend:?}, declaring {what}: {e}"));
    }
    store
}

#[tokio::test]
async fn a_session_lists_the_tools_of_a_lake_with_a_derived_relation() {
    for backend in BACKENDS {
        let mut mcp = Mcp::open(lake(backend).await, common::owner()).await;
        let listed = mcp.request("tools/list", json!({})).await;
        let names: Vec<&str> = listed["result"]["tools"]
            .as_array()
            .map(|t| t.iter().filter_map(|t| t["name"].as_str()).collect())
            .unwrap_or_default();
        assert!(
            names.contains(&"inv_matched_link") && names.contains(&"inw_find"),
            "{backend:?}: the session's tools: {listed}"
        );
    }
}

#[tokio::test]
async fn rest_checks_the_arguments_of_a_lake_with_a_derived_relation() {
    for backend in BACKENDS {
        let addr = serve(lake(backend).await, common::resolver()).await;
        let body = json!({"start": {"type": "inw"}});
        let (status, answer) = rest(addr, "k-owner", "POST", "query", Some(&body)).await;
        assert_eq!(status, 200, "{backend:?}, POST query: {answer}");
    }
}
