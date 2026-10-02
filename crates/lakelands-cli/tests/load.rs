//! The load run's short mode: every door of a served lake, a few
//! subscriptions, an ingest, the line and body caps, the feed with
//! reconnects and an export, at a scale that runs in CI. It checks counts
//! and correctness only; how long it takes is reported, never asserted.
//! The full run is the `load` example.

#[path = "../examples/load/driver.rs"]
mod driver;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_short_load_over_every_door_runs_clean() {
    let plan = driver::Plan {
        lakelands: env!("CARGO_BIN_EXE_lakelands").into(),
        scratch: env!("CARGO_TARGET_TMPDIR").into(),
        scale: driver::Scale::short(),
        engine: driver::Engine::Sqlite,
    };
    let report = driver::run(plan).await;
    println!("{}", report.text());
    let defects = report.defects();
    assert!(defects.is_empty(), "{}", report.text());
}
