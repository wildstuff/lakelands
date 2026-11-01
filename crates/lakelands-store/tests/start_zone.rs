//! The zone a location starts a lake in is recorded by the writer's first
//! open as the lake's `zone` setting, where the lake never recorded one;
//! from then on the log alone says the zone, and `LakeStore::zone`
//! answers it at the head
//! (`docs/decisions/101-a-lake-keeps-its-calendar-in-one-named-zone.md`).

use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::seam::{SETTING_TYPE, Seams};
use lakelands_api::{LakeStore, Location, RecordKey, TypeName};
use lakelands_core::conformance::fixture::Lake;
use lakelands_engine_mem::MemoryOpener;
use serde_json::json;

/// The lake `name` opened in `mode` on `opener`, its location starting
/// it in `zone`.
async fn open(
    opener: &MemoryOpener,
    name: &str,
    zone: Option<&str>,
    mode: OpenMode,
) -> lakelands_api::Result<Arc<dyn LakeStore>> {
    let location = Location {
        zone: zone.map(str::to_owned),
        ..Location::memory(name)
    };
    let seams = Seams {
        engines: vec![Arc::new(opener.clone())],
        ..Seams::default()
    };
    lakelands_store::open(location, seams, mode).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_first_open_records_the_location_s_zone_and_the_log_says_it_after() {
    let opener = MemoryOpener::new();
    let lake = open(&opener, "zoned", Some("Europe/Berlin"), OpenMode::Create)
        .await
        .expect("a lake starting in Berlin");
    assert_eq!(lake.zone().await.unwrap(), "Europe/Berlin");
    let l = Lake::of(lake.as_ref());
    let held = l.read(SETTING_TYPE, "zone").await.expect("the setting");
    assert_eq!(held.fields.get("value"), Some(&json!("Europe/Berlin")));

    // A zone set since stands: the location is not read again.
    l.set("zone", json!("Asia/Tokyo")).await.unwrap();
    lake.close().await.unwrap();
    let lake = open(&opener, "zoned", Some("Europe/Berlin"), OpenMode::Write)
        .await
        .unwrap();
    assert_eq!(lake.zone().await.unwrap(), "Asia/Tokyo");

    // Nor is one retired: the lake reads UTC, as it chose.
    let l = Lake::of(lake.as_ref());
    let (ty, key) = (TypeName::from(SETTING_TYPE), RecordKey::from("zone"));
    l.cat
        .retire(&l.own, &ty, &key, Default::default())
        .await
        .unwrap();
    lake.close().await.unwrap();
    let lake = open(&opener, "zoned", Some("Europe/Berlin"), OpenMode::Write)
        .await
        .unwrap();
    assert_eq!(lake.zone().await.unwrap(), "UTC");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lake_its_location_starts_in_no_zone_reads_utc_and_a_reader_records_none() {
    let opener = MemoryOpener::new();
    let lake = open(&opener, "plain", None, OpenMode::Create)
        .await
        .unwrap();
    assert_eq!(lake.zone().await.unwrap(), "UTC");
    // A reader opened with a zone writes nothing.
    let reader = open(&opener, "plain", Some("Europe/Berlin"), OpenMode::Read)
        .await
        .unwrap();
    assert_eq!(reader.zone().await.unwrap(), "UTC");
    assert_eq!(lake.zone().await.unwrap(), "UTC");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_zone_the_database_does_not_name_refuses_the_open() {
    let opener = MemoryOpener::new();
    let opened = open(&opener, "mars", Some("Mars/Olympus_Mons"), OpenMode::Create).await;
    let e = opened.err().expect("refused");
    assert!(e.to_string().contains("time zone"), "{e}");
}
