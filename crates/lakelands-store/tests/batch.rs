//! Record writes queued as batches on the memory engine probed: a batch
//! whose fold fails (lakelands#1332).

#[path = "batched/mod.rs"]
mod batched;
mod probed;

use std::sync::Arc;

use lakelands_api::Location;
use lakelands_api::seam::Seams;
use lakelands_engine_mem::MemoryOpener;
use probed::{Probe, ProbedOpener};

#[tokio::test(flavor = "multi_thread")]
async fn a_batch_whose_fold_fails_answers_each_job_at_its_own_position() {
    let probe = Arc::new(Probe::default());
    let opener = ProbedOpener(MemoryOpener::new(), probe.clone());
    let opener = Arc::new(opener);
    let place: batched::Place = Arc::new(move |lake, _| {
        let seams = Seams {
            engines: vec![opener.clone()],
            ..Seams::default()
        };
        (Location::memory(lake), seams)
    });
    let refuse = || *probe.refuse_commit.lock().unwrap() = Some(1);
    batched::a_batch_whose_fold_fails_answers_each_job_at_its_own_position(
        &place, "folds", &refuse,
    )
    .await;
}
