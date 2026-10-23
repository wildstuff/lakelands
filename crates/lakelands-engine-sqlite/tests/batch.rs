//! Record writes queued as batches on the SQLite engine, held to the same
//! writes made alone (lakelands#1332): each lake of a comparison is kept
//! in a directory of its own.

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

#[tokio::test(flavor = "multi_thread")]
async fn a_batch_answers_and_logs_as_its_writes_alone() {
    batched::a_batch_answers_and_logs_as_its_writes_alone(&place(), "batch").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_batch_answers_every_job_and_the_lake_writes_again() {
    batched::a_failed_batch_answers_every_job_and_the_lake_writes_again(&place(), "faulty").await;
}
