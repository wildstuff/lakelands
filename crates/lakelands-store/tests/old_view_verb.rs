//! A lake declared before 0.8.0 may hold a verb on a record of a view:
//! declaring one is refused since (lakelands#370), but replay does not
//! judge a declaration again. A call of such a verb is refused at the door
//! as `invalid`, naming the view, as every door refuses a view.
//!
//! The contract cannot declare such a verb any more, so the test writes the
//! log through the engine seam, as the store wrote it then, and opens it.
//! It runs on the memory engine, the store's one engine among its test
//! dependencies.

use std::sync::Arc;

use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::error::{Case, Said};
use lakelands_api::event::{Digest, EventBody, LOG_FORMAT_VERSION};
use lakelands_api::ontology::StorageKind;
use lakelands_api::seam::{Seams, setting_type};
use lakelands_api::write::{Declaration, WriteOpts};
use lakelands_api::{
    Error, Grants, LakeStore, Location, Patch, Position, Principal, PrincipalKind, Refusal,
    Timestamp, WriteContext,
};
use lakelands_core::conformance::fixture::{field, partner_totals, type_decl};
use lakelands_engine_mem::MemoryOpener;
use lakelands_store::log::{Draft, seal};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_verb_on_a_record_of_a_view_from_an_older_lake_is_refused_at_the_call() {
    let opener = MemoryOpener::new();
    let lake = "older-view-verb";
    let location = Location::memory(lake);
    {
        let engine = opener
            .open(&location, OpenMode::Create)
            .expect("the engine creates the lake");
        let lease = engine.take_lease("an older writer").expect("the lease");
        let now = Timestamp(1_790_000_000_000_000);
        let typed = |t: lakelands_api::ontology::TypeDecl| {
            let hash = lakelands_core::shape::schema_hash(&t).expect("a schema hash");
            EventBody::Declared {
                declaration: Declaration::Type(t),
                schema_hash: Some(hash),
                version: 1,
                with_the_set: false,
            }
        };
        let invoice = type_decl(
            "invoice",
            vec![
                field("partner", StorageKind::Text),
                field("amount", StorageKind::Decimal),
            ],
        );
        let poke: Declaration = serde_json::from_value(serde_json::json!({
            "declare": "verb", "name": "poke", "on": "pt", "risk": "low",
            "effect": "write", "set": [{"field": "total", "value": "1"}]
        }))
        .expect("a verb declaration");
        let bodies = vec![
            EventBody::LakeCreated {
                format: LOG_FORMAT_VERSION,
            },
            EventBody::LeaseTaken {
                fence: lease.fence,
                holder: lease.holder.clone(),
            },
            typed(setting_type()),
            typed(invoice),
            typed(partner_totals("pt", StorageKind::Decimal)),
            EventBody::Declared {
                declaration: poke,
                schema_hash: None,
                version: 1,
                with_the_set: false,
            },
        ];
        let mut prev = Digest::zero();
        let mut stored = Vec::new();
        for (i, body) in bodies.into_iter().enumerate() {
            let fence = if i == 0 { 0 } else { lease.fence };
            let d = Draft::system(now, body);
            let (event, canon) = seal(&location.lake, Position(i as u64), &prev, fence, d);
            prev = event.hash.clone();
            stored.push(canon);
        }
        engine
            .append(&lease, &stored)
            .expect("the older lake's log");
        engine
            .release_lease(&lease)
            .expect("the older writer lets go");
    }

    let store: Arc<dyn LakeStore> = lakelands_store::open(
        location,
        Seams {
            engines: vec![Arc::new(opener)],
            ..Seams::default()
        },
        OpenMode::Write,
    )
    .await
    .expect("the older lake opens");
    let owner = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: lake.into(),
        grants: Grants::owner(),
    };
    let head = store.head().await.expect("the head");
    let got = store
        .catalog(WriteContext::default())
        .call(
            &owner,
            &"poke".into(),
            Some(&"p1".into()),
            Patch::new(),
            WriteOpts::default(),
        )
        .await;
    let view = Said::from(Case::IsAView { ty: "pt".into() });
    assert!(
        matches!(&got, Err(Error::Refused { reason: Refusal::Invalid(said), .. })
            if **said == view),
        "a call of `poke`, declared on a record of the view `pt` before 0.8.0, is refused as \
         invalid naming `pt`; it answered {got:?}"
    );
    assert_eq!(
        store.head().await.expect("the head"),
        head,
        "nothing is appended"
    );
}
