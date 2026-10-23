//! Record writes queued as batches on the PostgreSQL engine, held to the same
//! writes made one by one (lakelands#1332, lakelands#1313): each lake of a
//! comparison is kept in a schema of its own on the server
//! `LAKELANDS_TEST_PG_URL` names; without it the tests stand down.

mod common;

#[path = "../../lakelands-store/tests/batched/mod.rs"]
mod batched;

use std::sync::{Arc, Mutex};

use lakelands_api::seam::Seams;
use lakelands_engine_pg::PgOpener;

/// Where the lakes are kept, and the schemas they were given.
fn place(url: &str) -> (batched::Place, Arc<Mutex<Vec<String>>>) {
    let (url, given) = (url.to_owned(), Arc::new(Mutex::new(Vec::new())));
    let schemas = given.clone();
    let place: batched::Place = Arc::new(move |lake, _| {
        let schema = common::fresh_schema();
        schemas.lock().unwrap().push(schema.clone());
        let seams = Seams {
            engines: vec![Arc::new(PgOpener)],
            ..Seams::default()
        };
        (common::location(&url, lake, &schema), seams)
    });
    (place, given)
}

macro_rules! on {
    ($(#[$m:meta])* $test:ident, $lake:literal) => {
        $(#[$m])*
        #[tokio::test(flavor = "multi_thread")]
        async fn $test() {
            let Some(url) = common::url_or_skip(stringify!($test)) else {
                return;
            };
            let (place, given) = place(&url);
            batched::$test(&place, $lake).await;
            for schema in given.lock().unwrap().iter() {
                common::drop_schema(&url, schema);
            }
        }
    };
}

on!(a_batch_answers_and_logs_as_its_writes_one_by_one, "batch");
on!(
    a_failed_append_answers_every_job_and_a_rerun_is_serial,
    "faulty"
);
on!(a_panicked_batch_answers_every_job_stopped, "panicked");
on!(
    a_job_refused_after_staging_leaves_its_batch_as_it_found_it,
    "staged"
);
on!(a_batch_that_panicked_once_durable_is_caught_up, "durable");
on!(an_ingest_window_closes_a_batch, "ingest");
on!(a_soak_replays_serially, "soak");
on!(
    #[ignore = "run by hand, --release: it measures"]
    throughput,
    "throughput"
);
