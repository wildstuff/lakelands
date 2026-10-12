//! The soak's short mode: the open book served for two minutes on SQLite,
//! its server killed every ten to twenty seconds and stopped cleanly once,
//! every acknowledged write, the reader limited to one partner and the
//! feed checked after each start, and the export verified, imported and
//! read alike once a minute. It holds that the soak still builds and its
//! checks still pass; the 24-hour runs are the `soak` example's.

#[path = "../examples/soak/driver.rs"]
mod driver;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_short_soak_with_kills_runs_clean() {
    let plan = driver::Plan::short(
        env!("CARGO_BIN_EXE_lakelands").into(),
        env!("CARGO_TARGET_TMPDIR").into(),
    );
    let report = driver::run(plan).await;
    assert!(report.red().is_none(), "{}", report.json());
}
