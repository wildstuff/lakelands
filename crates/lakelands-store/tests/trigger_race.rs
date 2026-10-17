//! A trigger's ceiling against a firing appended concurrently
//! (`docs/decisions/021-the-write-context-belongs-to-the-host.md`,
//! extended, lakelands#375): a reaction naming the position a concurrent
//! write's firing is about to take is refused, as past the head or as
//! past the trigger's ceiling, and never written. The conformance suite
//! holds the same order sequentially; only a race shows the store reads
//! the cause under the writer.

use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::ontology::{FieldDecl, StorageKind, TriggerDecl, TypeDecl};
use lakelands_api::principal::{WriteContext, WriteOrigin};
use lakelands_api::seam::Seams;
use lakelands_api::write::{Declaration, WriteOpts};
use lakelands_api::{Grants, LakeStore, Location, Patch, Principal, PrincipalKind, Refusal};
use lakelands_engine_mem::MemoryOpener;
use serde_json::json;

/// How many races are run.
const RACES: u32 = 500;

fn owner() -> Principal {
    Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: "trigger-race".into(),
        grants: Grants::owner(),
    }
}

fn job(n: u32) -> Patch {
    [("n".into(), json!(n))].into()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reaction_naming_a_firing_appended_concurrently_never_passes_its_ceiling() {
    let seams = Seams {
        engines: vec![Arc::new(MemoryOpener::new())],
        ..Seams::default()
    };
    let store: Arc<dyn LakeStore> =
        lakelands_store::open(Location::memory("trigger-race"), seams, OpenMode::Create)
            .await
            .expect("the lake opens");
    let own = owner();
    let cat = store.catalog(WriteContext::default());
    let ty = TypeDecl {
        fields: vec![FieldDecl::of("n", StorageKind::Number)],
        ..TypeDecl::named("job")
    };
    cat.declare(&own, Declaration::Type(ty))
        .await
        .expect("declaring `job`");
    let once = TriggerDecl {
        name: "once".into(),
        on: "job".into(),
        when: None,
        max_depth: Some(1),
        description: None,
        when_strict: false,
        on_changes: None,
    };
    cat.declare(&own, Declaration::Trigger(once))
        .await
        .expect("declaring `once`");
    let (mut past, mut too_deep, mut written) = (0, 0, Vec::new());
    for i in 0..RACES {
        // A write puts its record at head + 1 and `once`'s firing at
        // head + 2; the reaction names head + 2 while that write races it,
        // two deeper, past the ceiling of a firing at depth 0.
        let head = store.head().await.expect("the head");
        let firing = head.next().next();
        let reaction = store.catalog(WriteContext {
            origin: WriteOrigin::Reactive,
            depth: 2,
            cause: Some(firing),
        });
        let (direct, react) = (cat.clone(), own.clone());
        let a = tokio::spawn(async move {
            direct
                .upsert(
                    &react,
                    &"job".into(),
                    &format!("a{i}").into(),
                    job(i),
                    WriteOpts::default(),
                )
                .await
        });
        let react = own.clone();
        let b = tokio::spawn(async move {
            reaction
                .upsert(
                    &react,
                    &"job".into(),
                    &format!("b{i}").into(),
                    job(i),
                    WriteOpts::default(),
                )
                .await
        });
        a.await
            .expect("the direct write runs")
            .expect("the direct write applies");
        match b.await.expect("the reaction runs") {
            Ok(o) => written.push(o.position()),
            Err(lakelands_api::Error::Refused {
                reason: Refusal::TooDeep { .. },
                ..
            }) => too_deep += 1,
            Err(lakelands_api::Error::Refused {
                reason: Refusal::Invalid { .. },
                ..
            }) => past += 1,
            Err(e) => panic!("race {i}: the reaction failed otherwise: {e}"),
        }
    }
    assert!(
        written.is_empty(),
        "{} of {RACES} reactions naming a firing of `once` (max_depth 1) were written at depth \
         2, first at {:?}; refused too_deep {too_deep}, past the head {past}",
        written.len(),
        written.first()
    );
}
