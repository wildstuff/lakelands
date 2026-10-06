//! A lake made before the fold recorded a migration's renames
//! (lakelands#527) holds a grant through them at its next open: the list
//! is found from what the projection keeps, without reading the log
//! (`docs/decisions/006-grants-not-roles.md`, extended).
//!
//! The store cannot make such a lake any more, since its fold records the
//! list, so the test writes one through the engine seam as the store left
//! lakes then: the log, and a projection that holds each declaration at
//! the position of its event and no list. It runs on the memory engine,
//! the store's one engine among its test dependencies.

use std::sync::Arc;

use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::event::{Digest, EventBody, LOG_FORMAT_VERSION};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::principal::TypeGrants;
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::{Seams, setting_type};
use lakelands_api::write::{Declaration, Migration};
use lakelands_api::{
    Access, Grants, LakeStore, Location, Position, Principal, PrincipalKind, Timestamp, Value,
    Version, WriteContext,
};
use lakelands_engine_mem::MemoryOpener;
use lakelands_store::log::{Draft, seal};
use serde_json::json;

const LAKE: &str = "made-before-renames";

fn ty(name: &str, fields: &[&str]) -> TypeDecl {
    let fields: Vec<_> = (fields.iter())
        .map(|n| json!({"name": n, "type": {"kind": "text"}}))
        .collect();
    serde_json::from_value(json!({"name": name, "fields": fields})).expect("a type")
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
async fn a_lake_made_before_renames_were_recorded_holds_a_grant_through_them() {
    let opener = Arc::new(MemoryOpener::new());
    let location = Location::memory(LAKE);
    // `account` is declared, migrated (`holder` to `keeper`), declared
    // again with one more field, and migrated again (`keeper` to `owner`);
    // `plain` is declared once. Each event's position is its index.
    let renamed = |to: TypeDecl, from: &str, new: &str, version: u32| EventBody::Migrated {
        schema_hash: lakelands_core::shape::schema_hash(&to).expect("a schema hash"),
        migration: Migration {
            to,
            renames: vec![(from.into(), new.into())],
            relation_renames: Vec::new(),
        },
        version,
        report_id: "r".into(),
        rekeyed: Vec::new(),
    };
    let declared = |t: TypeDecl, version: u32| EventBody::Declared {
        schema_hash: lakelands_core::shape::schema_hash(&t).ok(),
        declaration: Declaration::Type(t),
        version,
    };
    let now = Timestamp(1_790_000_000_000_000);
    let made = {
        let engine = opener
            .open(&location, OpenMode::Create)
            .expect("the engine creates the lake");
        let lease = engine.take_lease("an older writer").expect("the lease");
        let bodies = vec![
            EventBody::LakeCreated {
                format: LOG_FORMAT_VERSION,
            },
            EventBody::LeaseTaken {
                fence: lease.fence,
                holder: lease.holder.clone(),
            },
            declared(setting_type(), 1),
            declared(ty("account", &["a", "holder"]), 1),
            renamed(ty("account", &["a", "keeper"]), "holder", "keeper", 2),
            declared(ty("account", &["a", "keeper", "more"]), 3),
            renamed(ty("account", &["a", "owner", "more"]), "keeper", "owner", 4),
            declared(ty("plain", &["a"]), 1),
        ];
        let (mut prev, mut stored) = (Digest::zero(), Vec::new());
        let p = engine.projection();
        p.begin().expect("the projection's transaction");
        for (i, body) in bodies.into_iter().enumerate() {
            let position = Position(i as u64);
            // What the fold put for each event then: the declaration.
            match &body {
                EventBody::Declared {
                    declaration,
                    schema_hash,
                    version,
                } => p.put_declaration(declaration, *version, schema_hash.as_deref(), position),
                EventBody::Migrated {
                    migration,
                    schema_hash,
                    version,
                    ..
                } => {
                    let d = Declaration::Type(migration.to.clone());
                    p.put_declaration(&d, *version, Some(schema_hash), position)
                }
                _ => Ok(()),
            }
            .expect("the projection takes the declaration");
            let fence = if i == 0 { 0 } else { lease.fence };
            let draft = Draft::system(now, body);
            let (event, canon) = seal(&location.lake, position, &prev, fence, draft);
            prev = event.hash.clone();
            stored.push(canon);
        }
        let fields = [("a", "1"), ("owner", "secret"), ("more", "m")];
        let fields = fields.map(|(f, v)| (f.into(), Value::from(v)));
        p.put_record(&StoredRecord {
            ty: "account".into(),
            key: "1".into(),
            version: Version(1),
            fields: fields.into(),
            meta: Default::default(),
            position: Position(7),
            retired: false,
            restricted: false,
        })
        .expect("the projection takes the record");
        p.commit(Position(7)).expect("the projection commits");
        engine
            .append(&lease, &stored)
            .expect("the older lake's log");
        engine
            .release_lease(&lease)
            .expect("the older writer lets go");
        stored.len()
    };
    let seams = || Seams {
        engines: vec![opener.clone()],
        ..Seams::default()
    };
    // The list the fold makes for this log, as the lake's one output holds it.
    let list = json!([
        {"at": 4, "type": "account", "renames": [["holder", "keeper"]]},
        {"at": 6, "type": "account", "renames": [["keeper", "owner"]]},
    ]);
    let recorded = || {
        let engine = opener.open(&location, OpenMode::Read).expect("the engine");
        let view = engine.view(None).expect("a view");
        view.output("sys:renames", &Digest::zero())
            .expect("an output")
    };
    let (stayed, followed) = (kept_from("holder"), kept_from("owner"));

    // Read-only, before any writer: closed, and nothing recorded.
    let read = lakelands_store::open(location.clone(), seams(), OpenMode::Read).await;
    let read = read.expect("the store opens the older lake read-only");
    let shown = answered(read.as_ref(), &stayed).await;
    assert_eq!(
        shown,
        ["a", "more"],
        "a read-only handle holds `holder: none` on `owner`"
    );
    assert_eq!(recorded(), None, "a read-only handle records nothing");
    read.close().await.expect("closing the read-only handle");

    // For writing: closed, and the list recorded; the log gained only what
    // a writer's open adds.
    for round in ["opened for writing", "opened again"] {
        let store = lakelands_store::open(location.clone(), seams(), OpenMode::Write).await;
        let store = store.expect("the store opens the older lake for writing");
        for who in [&stayed, &followed] {
            let shown = answered(store.as_ref(), who).await;
            assert_eq!(
                shown,
                ["a", "more"],
                "{round}: `{:?}` is held on `owner`",
                who.grants.types
            );
        }
        assert_eq!(
            recorded(),
            Some(list.clone()),
            "{round}: the list is recorded"
        );
        if round == "opened again" {
            // A rebuild makes the same list from the log.
            store.rebuild().await.expect("rebuilding");
            assert_eq!(
                recorded(),
                Some(list.clone()),
                "the fold makes the same list"
            );
            let feed = store
                .feed()
                .poll(Position(0), &Default::default(), 100)
                .await;
            let log = feed.expect("reading the log").events;
            let kinds: Vec<&str> = log[made..].iter().map(|e| e.body.kind()).collect();
            let opens = kinds.iter().filter(|k| **k == "lease_taken").count();
            assert!(
                opens == 2
                    && kinds
                        .iter()
                        .all(|k| ["lease_taken", "declared"].contains(k)),
                "the opens append their leases and the system types a newer store adds, and \
                 nothing else: {kinds:?}"
            );
        }
        store.close().await.expect("closing");
    }
}
