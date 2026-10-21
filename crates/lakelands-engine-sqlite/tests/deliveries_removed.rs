//! What taking back many deliveries costs on SQLite: 50 deliveries of a
//! lake of 100 000 ingested events, one `remove_values` per delivery
//! beside one act over the set, each act's walk of the log and rebuild
//! paid once per act. By hand: `cargo test --release -p
//! lakelands-engine-sqlite --test deliveries_removed -- --ignored
//! --nocapture`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use lakelands_api::engine::OpenMode;
use lakelands_api::ontology::StorageKind;
use lakelands_api::seam::Seams;
use lakelands_api::write::{IngestOpts, Selection};
use lakelands_api::{DeliveryId, LakeStore, Location, SourceId};
use lakelands_core::conformance::fixture::{Lake, source_record, sourced, type_decl};
use lakelands_engine_sqlite::SqliteOpener;
use serde_json::json;

/// How many events the lake holds, in deliveries of 1 000.
const EVENTS: u64 = 100_000;
const PER_DELIVERY: u64 = 1_000;
/// How many of those deliveries are taken back, every other one.
const TAKEN: u64 = 50;

fn delivery(d: u64) -> DeliveryId {
    DeliveryId::from(format!("load-{d:03}").as_str())
}

/// A lake of `EVENTS` tasks from `load`, each delivery its own records.
async fn lake(store: &dyn LakeStore) -> Lake {
    let l = Lake::of(store);
    l.declare(type_decl("task", vec![sourced("n", StorageKind::Number)]))
        .await
        .unwrap();
    for d in 0..EVENTS / PER_DELIVERY {
        let records = (d * PER_DELIVERY..(d + 1) * PER_DELIVERY)
            .map(|i| source_record("task", &format!("t{i:06}"), json!({"n": i})))
            .collect();
        let opts = IngestOpts {
            delivery: Some(delivery(d)),
            ..IngestOpts::default()
        };
        let load = SourceId::from("load");
        lakelands_api::ingest_all_with(l.cat.as_ref(), &l.own, &load, opts, records)
            .await
            .unwrap();
    }
    l
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "measures 50 removals over 100 000 events; run by hand with --release"]
async fn taking_back_fifty_deliveries_of_a_hundred_thousand_events() {
    for one in [false, true] {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("deliveries-removed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let seams = Seams {
            engines: vec![Arc::new(SqliteOpener)],
            ..Seams::default()
        };
        let location = Location::sqlite("removed", &dir);
        let store: Arc<dyn LakeStore> = lakelands_store::open(location, seams, OpenMode::Create)
            .await
            .unwrap();
        lake(store.as_ref()).await;
        let load = SourceId::from("load");
        let taken: Vec<DeliveryId> = (0..TAKEN).map(|d| delivery(2 * d)).collect();
        let acts = match one {
            true => vec![Selection::Deliveries(taken)],
            false => taken.into_iter().map(Selection::Delivery).collect(),
        };
        let (t, mut events) = (Instant::now(), 0);
        for selection in &acts {
            let report = store.remove_values(&load, selection, false, None).await;
            events += report.unwrap().events;
        }
        let took = t.elapsed();
        assert_eq!(
            events,
            TAKEN * PER_DELIVERY,
            "every taken event is selected once"
        );
        let how = if one {
            "one act over the set"
        } else {
            "one act each"
        };
        println!("sqlite: {EVENTS} events, {TAKEN} deliveries taken back, {how}: {took:?}");
        store.close().await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
