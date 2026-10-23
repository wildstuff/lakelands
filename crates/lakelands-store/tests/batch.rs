//! Record writes queued as batches on the memory engine probed: a batch
//! whose fold fails, and a write a backing computes whose fold fails with
//! writes queued behind it (lakelands#1332, lakelands#1313).

#[path = "batched/mod.rs"]
mod batched;
mod probed;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use lakelands_api::Location;
use lakelands_api::seam::{FunctionBacking, FunctionCall, Seams};
use lakelands_engine_mem::MemoryOpener;
use probed::{Probe, ProbedOpener};
use serde_json::{Value, json};

/// A backing that counts its calls.
#[derive(Default)]
struct Counting(AtomicUsize);

#[async_trait]
impl FunctionBacking for Counting {
    fn kind(&self) -> &str {
        "counting"
    }

    async fn call(&self, _: &FunctionCall) -> lakelands_api::Result<Value> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(json!(7))
    }
}

/// Lakes on the probed memory engine, `counting` bound as their backing,
/// and what makes the next fold fail.
fn place(counting: Arc<Counting>) -> (batched::Place, impl Fn()) {
    let probe = Arc::new(Probe::default());
    let opener = Arc::new(ProbedOpener(MemoryOpener::new(), probe.clone()));
    let place: batched::Place = Arc::new(move |lake, _| {
        let seams = Seams {
            engines: vec![opener.clone()],
            backings: vec![counting.clone()],
            ..Seams::default()
        };
        (Location::memory(lake), seams)
    });
    (place, move || {
        *probe.refuse_commit.lock().unwrap() = Some(1)
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_batch_whose_fold_fails_answers_each_job_at_its_own_position() {
    let (place, refuse) = place(Arc::default());
    batched::a_batch_whose_fold_fails_answers_each_job_at_its_own_position(
        &place, "folds", &refuse,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_backed_write_whose_fold_failed_is_caught_up_alone() {
    let counting = Arc::new(Counting::default());
    let (place, refuse) = place(counting.clone());
    let handed = || counting.0.load(Ordering::SeqCst);
    batched::a_backed_write_whose_fold_failed_is_caught_up_alone(
        &place, "backed", &refuse, &handed,
    )
    .await;
}
