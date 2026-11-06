//! A meter that panics is the host's fault, never the lake's: every
//! append is still folded, and every write answers as it would unmetered.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use lakelands_api::engine::OpenMode;
use lakelands_api::seam::{Count, Level, Meter, Metered, Seams, Spread};
use lakelands_api::write::Declaration;
use lakelands_api::{
    Get, Grants, LakeStore, Location, Principal, PrincipalKind, ReadAt, WriteContext,
};
use lakelands_engine_mem::MemoryOpener;
use serde_json::json;

/// Panics on every number it is told, after counting it.
#[derive(Default)]
struct Panics(AtomicU64);

impl Panics {
    fn told(&self) -> ! {
        self.0.fetch_add(1, Ordering::Relaxed);
        panic!("the meter fails");
    }
}

impl Meter for Panics {
    fn count(&self, _: Metered<'_>, _: Count, _: u64) {
        self.told()
    }
    fn level(&self, _: Metered<'_>, _: Level, _: u64) {
        self.told()
    }
    fn observe(&self, _: Metered<'_>, _: Spread, _: u64) {
        self.told()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_panicking_meter_leaves_every_append_folded() {
    let meter = Arc::new(Panics::default());
    let seams = Seams {
        engines: vec![Arc::new(MemoryOpener::new())],
        meter: Some(meter.clone()),
        ..Seams::default()
    };
    let store: Arc<dyn LakeStore> =
        lakelands_store::open(Location::memory("metered"), seams, OpenMode::Create)
            .await
            .unwrap();
    let owner = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: "metered".into(),
        grants: Grants::owner(),
    };
    let cat = store.catalog(WriteContext::default());
    let ty = json!({"declare": "type", "name": "item", "fields": [
        {"name": "n", "type": {"kind": "number"}}]});
    let ty: Declaration = serde_json::from_value(ty).unwrap();
    cat.declare(&owner, ty).await.unwrap();
    let write = |key: String, n: u64| {
        let (cat, owner) = (cat.clone(), owner.clone());
        async move {
            let patch = serde_json::from_value(json!({ "n": n })).unwrap();
            let (ty, k) = ("item".into(), key.as_str().into());
            let o = cat.upsert(&owner, &ty, &k, patch, Default::default()).await;
            (key, o.unwrap().position())
        }
    };
    let mut written = Vec::new();
    for n in 0..3 {
        written.push(write(format!("a{n}"), n).await);
    }
    let at_once: Vec<_> = (0..16)
        .map(|n| tokio::spawn(write(format!("b{n}"), n)))
        .collect();
    for w in at_once {
        written.push(w.await.unwrap());
    }
    let last = written.iter().map(|w| w.1).max().unwrap();
    let head = store.head().await.unwrap();
    assert!(
        meter.0.load(Ordering::Relaxed) > 0,
        "the meter was never told a number"
    );
    assert_eq!(head, last, "the head stays behind the last append, {last}");
    for (key, at) in written {
        let get = Get {
            key: key.as_str().into(),
            with_meta: false,
            at: ReadAt::default(),
            params: Default::default(),
        };
        let r = cat.get(&owner, &"item".into(), get).await.unwrap();
        assert!(
            r.position >= at,
            "`{key}`, written at {at}, reads at {}",
            r.position
        );
    }
}
