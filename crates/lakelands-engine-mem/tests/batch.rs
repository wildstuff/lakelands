//! Record writes queued as batches on the memory engine, held to the same
//! writes made alone (lakelands#1332): each lake of a comparison is kept
//! by an opener of its own.

#[path = "../../lakelands-store/tests/batched/mod.rs"]
mod batched;

use std::sync::Arc;

use lakelands_api::Location;
use lakelands_api::seam::Seams;
use lakelands_engine_mem::MemoryOpener;

fn place() -> batched::Place {
    Arc::new(|lake, _| {
        let seams = Seams {
            engines: vec![Arc::new(MemoryOpener::new())],
            ..Seams::default()
        };
        (Location::memory(lake), seams)
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
