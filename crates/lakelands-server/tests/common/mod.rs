//! The lake and the principals every server test uses.

#![allow(dead_code)]

use std::sync::Arc;

use lakelands_api::principal::{Access, Grants, PrincipalKind, TypeGrants};
use lakelands_api::write::WriteOpts;
use lakelands_api::{LakeStore, Principal, WriteContext};
use lakelands_mock::MockLake;
use lakelands_server::resolver::{Entry, StaticResolver};
use serde_json::json;

/// The fixture lake with one invoice written, at a known position.
pub async fn lake() -> Arc<dyn LakeStore> {
    let store: Arc<dyn LakeStore> = Arc::new(MockLake::new("finance"));
    let f = lakelands_mock::checks::setup(store.clone()).await;
    let c = store.catalog(WriteContext::default());
    c.upsert(
        &f.cast.owner,
        &"invoice".into(),
        &"INV-1".into(),
        lakelands_mock::checks::patch(&[("status", json!("open"))]),
        WriteOpts::default(),
    )
    .await
    .unwrap();
    store
}

fn clerk_grants() -> Grants {
    Grants {
        default: Access::Read,
        types: [(
            "invoice".into(),
            TypeGrants {
                default: None,
                fields: [
                    ("status".into(), Access::Write),
                    ("secret".into(), Access::None),
                ]
                .into(),
            },
        )]
        .into(),
        ..Grants::default()
    }
}

/// The owner (key `k-owner`) and a clerk (key `k-clerk`).
pub fn entries() -> Vec<Entry> {
    vec![
        Entry {
            name: "owner".into(),
            kind: PrincipalKind::Human,
            id: "owner".into(),
            lakes: vec!["finance".into()],
            grants: Grants::owner(),
            api_keys: vec!["k-owner".into()],
        },
        Entry {
            name: "clerk".into(),
            kind: PrincipalKind::Agent,
            id: "clerk".into(),
            lakes: vec!["finance".into()],
            grants: clerk_grants(),
            api_keys: vec!["k-clerk".into()],
        },
    ]
}

pub fn resolver() -> Arc<StaticResolver> {
    Arc::new(StaticResolver::new(entries()))
}

/// The lake's owner, as a stdio server is started for.
pub fn owner() -> Principal {
    Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: "finance".into(),
        grants: Grants::owner(),
    }
}
