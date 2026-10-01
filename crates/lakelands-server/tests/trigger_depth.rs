//! A trigger's `max_depth` at the JSON doors, on the mock and on the store
//! over both engines: declared through REST, it comes back from `describe`
//! over REST and MCP; above the lake's `max_reactive_depth` it is refused
//! as invalid, naming the trigger and both numbers, with the lake
//! unmoved, while 0 and the ceiling itself are taken.
//!
//! The ceiling binds reactions, which a host writes naming a firing as
//! their cause. A door's caller is never one: its writes carry no cause,
//! so the `too_deep` refusal is the store's to check, and the doors only
//! say it (the unit tests beside `tool_answer` and `failure_json`).

mod common;

use common::wire::{Mcp, rest, serve};
use common::{BACKENDS, Backend};

use lakelands_api::location::Limits;
use serde_json::{Value, json};

/// A trigger on invoices, its reactions capped at `max_depth`.
fn declaration(name: &str, max_depth: u32) -> Value {
    json!({"declaration": {
        "declare": "trigger", "name": name, "on": "invoice", "max_depth": max_depth,
    }})
}

/// The trigger `name` in a lake description, if it lists it.
fn trigger<'a>(description: &'a Value, name: &str) -> Option<&'a Value> {
    description["triggers"]
        .as_array()?
        .iter()
        .find(|t| t["name"] == name)
}

#[tokio::test]
async fn a_triggers_max_depth_is_declared_described_and_bound_by_the_lakes_ceiling() {
    for backend in BACKENDS {
        on(backend).await;
    }
}

async fn on(backend: Backend) {
    let store = common::lake_on(backend).await;
    let addr = serve(store.clone(), common::resolver()).await;
    let at = |what: &str| format!("{backend:?}, {what}");

    let (status, answer) = rest(
        addr,
        "k-owner",
        "POST",
        "schema/declare",
        Some(&declaration("once", 1)),
    )
    .await;
    assert_eq!(status, 200, "{}: {answer}", at("declare"));
    let (status, answer) = rest(addr, "k-owner", "GET", "describe", None).await;
    assert_eq!(status, 200, "{}: {answer}", at("describe"));
    let once = trigger(&answer["data"], "once");
    assert_eq!(
        once.map(|t| &t["max_depth"]),
        Some(&json!(1)),
        "{}: {answer}",
        at("REST describe")
    );
    let mut mcp = Mcp::open(store.clone(), common::owner()).await;
    let result = mcp.call("describe", json!({})).await;
    let described = &result["structuredContent"]["data"];
    assert_eq!(
        trigger(described, "once").map(|t| &t["max_depth"]),
        Some(&json!(1)),
        "{}: {result}",
        at("MCP describe")
    );

    let ceiling = Limits::default().max_reactive_depth;
    let head = store.head().await.unwrap();
    let (status, answer) = rest(
        addr,
        "k-owner",
        "POST",
        "schema/declare",
        Some(&declaration("deep", ceiling + 1)),
    )
    .await;
    let what = format!("{}: {answer}", at("max_depth above the ceiling"));
    assert_eq!(status, 422, "{what}");
    assert_eq!(answer["reason"]["refusal"], "invalid", "{what}");
    let said = answer["what"].as_str().unwrap_or_default();
    for name in ["deep", &(ceiling + 1).to_string(), &ceiling.to_string()] {
        assert!(said.contains(name), "`{name}` is not named: {what}");
    }
    assert_eq!(store.head().await.unwrap(), head, "{what}: the lake moved");
    for (name, depth) in [("none", 0), ("whole", ceiling)] {
        let (status, answer) = rest(
            addr,
            "k-owner",
            "POST",
            "schema/declare",
            Some(&declaration(name, depth)),
        )
        .await;
        assert_eq!(
            status,
            200,
            "{}: {answer}",
            at(&format!("max_depth {depth}"))
        );
    }
}
