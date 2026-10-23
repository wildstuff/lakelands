//! Record writes queued as batches on the SQLite engine, held to the same
//! writes made one by one (lakelands#1332, lakelands#1313): each lake of a
//! comparison is kept in a directory of its own.

#[path = "../../lakelands-store/tests/batched/mod.rs"]
mod batched;

use std::path::PathBuf;
use std::sync::Arc;

use lakelands_api::Location;
use lakelands_api::seam::Seams;
use lakelands_engine_sqlite::SqliteOpener;

fn place() -> batched::Place {
    let root =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("batch-{}", std::process::id()));
    Arc::new(move |lake, twin| {
        let seams = Seams {
            engines: vec![Arc::new(SqliteOpener)],
            ..Seams::default()
        };
        (
            Location::sqlite(lake, root.join(format!("{lake}-{twin}"))),
            seams,
        )
    })
}

macro_rules! on {
    ($(#[$m:meta])* $test:ident, $lake:literal) => {
        $(#[$m])*
        #[tokio::test(flavor = "multi_thread")]
        async fn $test() {
            batched::$test(&place(), $lake).await;
        }
    };
}

on!(a_batch_answers_and_logs_as_its_writes_one_by_one, "batch");
on!(
    a_failed_append_answers_every_job_and_a_rerun_is_serial,
    "faulty"
);
on!(a_panicked_batch_answers_every_job_stopped, "panicked");
on!(a_batch_that_panicked_once_durable_is_caught_up, "durable");
on!(an_ingest_window_closes_a_batch, "ingest");
on!(a_soak_replays_serially, "soak");
on!(
    #[ignore = "run by hand, --release: it measures"]
    throughput,
    "throughput"
);
