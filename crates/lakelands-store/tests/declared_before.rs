//! A declaration a log already holds is folded as it was written: the
//! fold does not judge it (`docs/decisions/028-authoring-is-strict-and-whole.md`,
//! extended, lakelands#549).
//!
//! A derived `number` field whose expression is a duration was taken by a
//! store before the check and never filled. The store refuses to declare
//! one now, so the test writes the log through the engine seam as such a
//! store left it. The type is judged when it is next declared. It runs on the memory engine, the store's one engine
//! among its test dependencies.

use std::sync::Arc;

use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::event::{Digest, EventBody, LOG_FORMAT_BASE};
use lakelands_api::ontology::{FieldDecl, TypeDecl};
use lakelands_api::seam::{Seams, setting_type};
use lakelands_api::write::Declaration;
use lakelands_api::{
    Error, Grants, LakeStore, Location, Position, Principal, PrincipalKind, Refusal, Timestamp,
    WriteContext, WriteOpts,
};
use lakelands_engine_mem::MemoryOpener;
use lakelands_store::log::{Draft, seal};
use serde_json::json;

const LAKE: &str = "declared-before";

fn late() -> serde_json::Value {
    json!({"name": "late", "type": {"kind": "number"}, "authority": "derived",
           "derived": "paid_on - due_on", "depends_on": ["paid_on", "due_on"]})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_declaration_the_log_holds_is_folded_as_before_and_judged_when_declared_again() {
    let opener = MemoryOpener::new();
    let location = Location::memory(LAKE);
    let when = |name: &str| json!({"name": name, "type": {"kind": "date_time"}});
    let invoice = json!({"name": "invoice", "fields": [when("due_on"), when("paid_on"), late()]});
    let invoice: TypeDecl = serde_json::from_value(invoice).expect("a type");
    let declared = |t: TypeDecl| EventBody::Declared {
        schema_hash: lakelands_core::shape::schema_hash(&t).ok(),
        declaration: Declaration::Type(t),
        version: 1,
        with_the_set: false,
    };
    {
        let engine = opener
            .open(&location, OpenMode::Create)
            .expect("the engine creates the lake");
        let lease = engine.take_lease("an older writer").expect("the lease");
        let now = Timestamp(1_790_000_000_000_000);
        let bodies = vec![
            EventBody::LakeCreated {
                format: LOG_FORMAT_BASE,
            },
            EventBody::LeaseTaken {
                fence: lease.fence,
                holder: lease.holder.clone(),
            },
            declared(setting_type()),
            declared(invoice),
        ];
        let (mut prev, mut stored) = (Digest::zero(), Vec::new());
        for (i, body) in bodies.into_iter().enumerate() {
            let fence = if i == 0 { 0 } else { lease.fence };
            let draft = Draft::system(now, body);
            let (event, canon) = seal(&location.lake, Position(i as u64), &prev, fence, draft);
            prev = event.hash.clone();
            stored.push(canon);
        }
        engine.append(&lease, &stored).expect("the older log");
        engine
            .release_lease(&lease)
            .expect("the older writer lets go");
    }

    let seams = Seams {
        engines: vec![Arc::new(opener)],
        ..Seams::default()
    };
    let store: Arc<dyn LakeStore> = lakelands_store::open(location, seams, OpenMode::Write)
        .await
        .expect("the store opens a lake whose log holds the declaration");
    let owner = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: LAKE.into(),
        grants: Grants::owner(),
    };
    let (cat, ty) = (store.catalog(WriteContext::default()), "invoice".into());
    let patch = json!({"due_on": "2026-08-01T00:00:00Z", "paid_on": "2026-08-03T00:00:00Z"});
    let patch = serde_json::from_value(patch).expect("a patch");
    cat.upsert(&owner, &ty, &"i1".into(), patch, WriteOpts::default())
        .await
        .expect("a write to the type lands as it did");
    let get = serde_json::from_value(json!({"key": "i1"})).expect("a get");
    let got = cat.get(&owner, &ty, get).await.expect("the record").data;
    let fields: Vec<&str> = got.fields.keys().map(|f| f.as_str()).collect();
    assert_eq!(
        fields,
        ["due_on", "paid_on"],
        "the field is cleared, as it was before the check"
    );

    // The type is held to the rule when it is declared again: a field
    // more, itself fine, is refused on the one that is never filled.
    let note: FieldDecl =
        serde_json::from_value(json!({"name": "note", "type": {"kind": "text"}})).expect("a field");
    let refused = cat.add_field(&owner, &ty, note).await;
    assert!(
        matches!(&refused, Err(Error::Refused { field: Some(f), reason: Refusal::Invalid { .. } })
            if f.as_str() == "late"),
        "{refused:?}"
    );
}
