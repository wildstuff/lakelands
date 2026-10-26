//! A search that needs a vector field and names none is refused naming
//! the member and the mode that needs it, at the MCP door's `search` tool
//! (REST's route reads the same arguments; the server's tests ask both).

use std::sync::Arc;

use lakelands_api::LakeStore;
use lakelands_api::read::ReadAt;
use lakelands_api::tools::{Door, tools};
use serde_json::{Value, json};

use crate::checks::setup;
use crate::statements::Tools;

/// Expands to one `#[test]` per check of the `search` tool.
///
/// `$new` is called once per check and gives a fresh, empty lake and the
/// MCP door's tools over it, as `(Arc<dyn LakeStore>, Arc<dyn Tools>)`.
#[macro_export]
macro_rules! search_field_checks {
    ($new:expr, $block_on:path) => {
        $crate::search_field_checks!(@each $new, $block_on;
            a_vector_or_hybrid_search_without_its_field_is_refused_naming_it);
    };
    (@each $new:expr, $block_on:path; $($name:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                let (lake, tools) = ($new)();
                $block_on($crate::search_field::$name(lake, tools));
            }
        )*
    };
}

/// A hybrid and a vector search without `field` are each refused as a
/// member missing: `field`, and the mode that needs it.
pub async fn a_vector_or_hybrid_search_without_its_field_is_refused_naming_it(
    lake: Arc<dyn LakeStore>,
    tools_at: Arc<dyn Tools>,
) {
    let f = setup(lake).await;
    let owner = &f.cast.owner;
    let d = f.catalog.describe(owner, ReadAt::default()).await;
    let d = d.expect("the owner is described").data;
    let offered = tools(&d).expect("the fixture's names are valid");
    let name = offered.into_iter().find(|t| t.door == Door::Search);
    let name = name.expect("the owner is offered `search`").name;
    for (mode, rest) in [
        ("hybrid", json!({"text": "seawater pump", "k": 8})),
        ("vector", json!({"vector": [1.0, 0.0]})),
    ] {
        let mut args = rest;
        args["type"] = json!("invoice");
        args["mode"] = json!(mode);
        let refused = tools_call(&tools_at, owner, &name, args).await;
        let wire = serde_json::to_value(&refused).expect("an error is JSON");
        let reason = &wire["reason"];
        let got = json!([reason["code"], reason["mode"], reason["member"]]);
        assert_eq!(got, json!(["mode_needs_member", mode, "field"]), "{wire}");
        let said = refused.to_string();
        assert!(said.contains("`field`") && said.contains(mode), "{said}");
    }
}

async fn tools_call(
    tools_at: &Arc<dyn Tools>,
    who: &lakelands_api::Principal,
    name: &str,
    args: Value,
) -> lakelands_api::Error {
    let refused = tools_at.call(who, name, args).await;
    refused.expect_err("a search without its field is refused")
}
