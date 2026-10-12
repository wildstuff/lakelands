//! A lake's list of renames is what its fold recorded, and every open
//! reads it there: read-only, for writing, and for a rebuild, which makes
//! it again from the log (`docs/decisions/006-grants-not-roles.md`,
//! extended). No open looks for renames anywhere else, so a grant holds a
//! field through them only because the fold recorded each one.
//!
//! The suite holds the same through a reopen for writing, a rebuild and an
//! import on every engine (`renamed_grants`); the read-only open is here,
//! since the suite's harness opens a lake for writing. It runs on the
//! memory engine, the store's one engine among its test dependencies.

use std::sync::Arc;

use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::event::Digest;
use lakelands_api::principal::TypeGrants;
use lakelands_api::seam::Seams;
use lakelands_api::write::{Declaration, Migration, WriteOpts};
use lakelands_api::{Access, Grants, LakeStore, Location, Principal, PrincipalKind, WriteContext};
use lakelands_engine_mem::MemoryOpener;
use serde_json::{Value, json};

const LAKE: &str = "renames-at-an-open";

fn account(fields: &[&str]) -> Value {
    let fields: Vec<_> = (fields.iter())
        .map(|n| json!({"name": n, "type": {"kind": "text"}}))
        .collect();
    json!({"name": "account", "fields": fields})
}

/// A principal that reads everything but `account`'s field `kept_from`.
fn kept_from(field: &str) -> Principal {
    let mut grants = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    let fields = [(field.into(), Access::None)].into();
    let held = TypeGrants {
        default: None,
        fields,
        ..Default::default()
    };
    grants.types.insert("account".into(), held);
    Principal {
        kind: PrincipalKind::Agent,
        id: "kept".into(),
        lake: LAKE.into(),
        grants,
    }
}

/// The fields of `account`'s record `1` that `who` is answered.
async fn answered(store: &dyn LakeStore, who: &Principal) -> Vec<String> {
    let get = serde_json::from_value(json!({"key": "1"})).expect("a get");
    let cat = store.catalog(WriteContext::default());
    let got = cat.get(who, &"account".into(), get).await;
    let got = got.expect("reading the record").data;
    got.fields.keys().map(ToString::to_string).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_renaming_migration_is_read_back_at_an_open_in_every_mode() {
    let opener = Arc::new(MemoryOpener::new());
    let location = Location::memory(LAKE);
    let seams = || Seams {
        engines: vec![opener.clone()],
        ..Seams::default()
    };
    let owner = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: LAKE.into(),
        grants: Grants::owner(),
    };

    // `account` is declared with `holder`, written, and migrated twice:
    // `holder` to `keeper`, `keeper` to `owner`.
    let store = lakelands_store::open(location.clone(), seams(), OpenMode::Create).await;
    let store = store.expect("the store creates the lake");
    let cat = store.catalog(WriteContext::default());
    let first = serde_json::from_value(account(&["a", "holder"])).expect("a type");
    let first = Declaration::Type(first);
    cat.declare(&owner, first)
        .await
        .expect("declaring `account`");
    let patch = serde_json::from_value(json!({"a": "1", "holder": "secret"})).expect("a patch");
    cat.upsert(
        &owner,
        &"account".into(),
        &"1".into(),
        patch,
        WriteOpts::default(),
    )
    .await
    .expect("writing the record");
    for (from, to) in [("holder", "keeper"), ("keeper", "owner")] {
        let m = Migration {
            to: serde_json::from_value(account(&["a", to])).expect("a type"),
            renames: vec![(from.into(), to.into())],
            relation_renames: Vec::new(),
        };
        let report = cat.migrate(&owner, m.clone()).await;
        let report = report.expect("the migration's preflight").data;
        cat.migrate_apply(&owner, m, &report.report_id)
            .await
            .expect("applying the migration");
    }
    store
        .close()
        .await
        .expect("closing the lake's first writer");
    drop(cat);
    drop(store);

    // The list as the lake's one output holds it: each entry's type and
    // pairs, in the log's order.
    let recorded = || {
        let engine = opener.open(&location, OpenMode::Read).expect("the engine");
        let view = engine.view(None).expect("a view");
        let list = view.output("sys:renames", &Digest::zero());
        let list = list.expect("an output").map_or(json!([]), |(_, list)| list);
        let entries = list.as_array().cloned().unwrap_or_default();
        let pairs = |e: &Value| (e["type"].clone(), e["renames"].clone());
        entries.iter().map(pairs).collect::<Vec<_>>()
    };
    let list = vec![
        (json!("account"), json!([["holder", "keeper"]])),
        (json!("account"), json!([["keeper", "owner"]])),
    ];
    let (stayed, followed) = (kept_from("holder"), kept_from("owner"));
    for mode in [OpenMode::Read, OpenMode::Write, OpenMode::Rebuild] {
        let store = lakelands_store::open(location.clone(), seams(), mode).await;
        let store = store.unwrap_or_else(|e| panic!("opening the lake ({mode:?}): {e}"));
        assert_eq!(
            answered(store.as_ref(), &owner).await,
            ["a", "owner"],
            "{mode:?}: the owner reads the renamed field"
        );
        for who in [&stayed, &followed] {
            assert_eq!(
                answered(store.as_ref(), who).await,
                ["a"],
                "{mode:?}: `{:?}` is held on `owner`",
                who.grants.types
            );
        }
        assert_eq!(recorded(), list, "{mode:?}: the list is the fold's");
        store.close().await.expect("closing");
    }
}
