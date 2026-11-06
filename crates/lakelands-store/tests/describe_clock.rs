//! A host setting's value in force is told on the answer's clock, by a
//! description the writer answers from what it holds as by one it reads
//! (lakelands#1575). Its own test binary: the clock it fixes is the
//! process's.

mod probed;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use lakelands_api::engine::{EngineOpener, OpenMode};
use lakelands_api::seam::{SETTING_TYPE, Seams};
use lakelands_api::{LakeStore, Location, ReadAt, Timestamp};
use lakelands_core::conformance::fixture::Lake;
use lakelands_engine_mem::MemoryOpener;
use lakelands_store::batch;
use probed::{Probe, ProbedOpener};
use serde_json::{Value, json};

/// 2029-06-01, before the period from 2030 begins, and 2030-06-01, after.
const BEFORE: Timestamp = Timestamp(1_874_966_400_000_000);
const AFTER: Timestamp = Timestamp(1_906_502_400_000_000);

/// `vat_rate`'s value as the lake's description at the head tells it on
/// the clock `at`, and the views that asked.
async fn told(l: &Lake, probe: &Probe, at: Timestamp) -> (Option<Value>, usize) {
    batch::fix_clock(Some(at));
    let was = probe.views.load(Ordering::SeqCst);
    let d = l.cat.describe(&l.own, ReadAt::default()).await.unwrap();
    let vat = d.data.settings.into_iter().find(|s| s.key == "vat_rate");
    let views = probe.views.load(Ordering::SeqCst) - was;
    (vat.and_then(|s| s.value), views)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kept_description_tells_the_value_in_force_on_its_clock() {
    let probe = Arc::new(Probe::default());
    let opener: Arc<dyn EngineOpener> = Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()));
    let seams = Seams {
        engines: vec![opener],
        ..Seams::default()
    };
    let store: Arc<dyn LakeStore> =
        lakelands_store::open(Location::memory("clock"), seams, OpenMode::Create)
            .await
            .expect("a handle on the lake");
    let l = Lake::of(store.as_ref());
    batch::fix_clock(Some(BEFORE));
    let vat = json!({"declare": "setting", "key": "vat_rate", "type": {"kind": "decimal"},
        "group": "finance"});
    l.declare_json(vat).await.unwrap();
    l.set("vat_rate", json!("0.16")).await.unwrap();
    let from = json!({"key": "vat_rate", "from": "2030-01-01", "value": "0.25"});
    l.upsert(SETTING_TYPE, "vat_rate", from).await.unwrap();
    let mut seen = Vec::new();
    for at in [BEFORE, BEFORE, AFTER, BEFORE] {
        seen.push(told(&l, &probe, at).await);
    }
    batch::fix_clock(None);
    let (old, new) = (Some(json!("0.16")), Some(json!("0.25")));
    assert_eq!(
        seen,
        [(old.clone(), 1), (old.clone(), 0), (new, 0), (old, 0)],
        "read once, then answered from what the writer holds, each on its clock"
    );
}
