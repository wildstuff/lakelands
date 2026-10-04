//! A set in force whose filter steps through a reference stays in force,
//! and is refused when it is declared again
//! (`docs/decisions/028-authoring-is-strict-and-whole.md`, extended,
//! lakelands#552).
//!
//! A store before the check took `partner.name = 'x'` as a set's filter;
//! the path read null and the set held no record. The store refuses to
//! declare one now, so the test writes the log through the engine seam as
//! such a store left it. It runs on the memory engine, the store's one
//! engine among its test dependencies.

use std::sync::Arc;

use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::event::{Digest, EventBody, LOG_FORMAT_BASE};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::seam::{Seams, setting_type};
use lakelands_api::write::Declaration;
use lakelands_api::{
    Error, Grants, LakeStore, Location, Position, Principal, PrincipalKind, Refusal, Timestamp,
    WriteContext, WriteOpts,
};
use lakelands_engine_mem::MemoryOpener;
use lakelands_store::log::{Draft, seal};
use serde_json::json;

const LAKE: &str = "set-before";

fn set(filter: &str) -> Declaration {
    let d = json!({"declare": "object_set", "name": "of_x", "of": "invoice", "where": filter});
    serde_json::from_value(d).expect("a set")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_set_in_force_through_a_reference_stays_and_is_judged_when_declared_again() {
    let opener = MemoryOpener::new();
    let location = Location::memory(LAKE);
    let partner =
        json!({"name": "partner", "fields": [{"name": "name", "type": {"kind": "text"}}]});
    let invoice = json!({"name": "invoice", "fields": [
        {"name": "partner", "type": {"kind": "reference", "target": "partner"}}]});
    let ty = |t: serde_json::Value| serde_json::from_value::<TypeDecl>(t).expect("a type");
    let declared = |declaration: Declaration, schema_hash| EventBody::Declared {
        schema_hash,
        declaration,
        version: 1,
    };
    let of = |t: TypeDecl| {
        let hash = lakelands_core::shape::schema_hash(&t).ok();
        declared(Declaration::Type(t), hash)
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
            of(setting_type()),
            of(ty(partner)),
            of(ty(invoice)),
            declared(set("partner.name = 'x'"), None),
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
    let cat = store.catalog(WriteContext::default());
    let (invoices, partners) = ("invoice".into(), "partner".into());
    let patch = |v: serde_json::Value| serde_json::from_value(v).expect("a patch");
    let named = patch(json!({"name": "x"}));
    cat.upsert(&owner, &partners, &"p1".into(), named, WriteOpts::default())
        .await
        .expect("a partner");
    let billed = patch(json!({"partner": "p1"}));
    let key = "i1".into();
    let wrote = cat.upsert(&owner, &invoices, &key, billed, WriteOpts::default());
    let wrote = wrote
        .await
        .expect("a write to the set's type lands as it did");
    assert!(
        matches!(&wrote, lakelands_api::Outcome::Applied { set_off, .. } if set_off.joined.is_empty()),
        "the set holds no record, as it held none: {wrote:?}"
    );

    // Declared again, with whatever filter through the reference, it is
    // refused; with one over the reference's key it is taken.
    let refused = cat.declare(&owner, set("partner.name = 'y'")).await;
    assert!(
        matches!(&refused, Err(Error::Refused { reason: Refusal::Invalid { message }, .. })
            if message.contains("steps into `partner`")),
        "{refused:?}"
    );
    cat.declare(&owner, set("partner = 'p1'"))
        .await
        .expect("a set over the reference's key");
}
