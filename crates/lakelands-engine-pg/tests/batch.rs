//! Record writes queued as batches on the PostgreSQL engine, held to the
//! same writes made alone (lakelands#1332): each lake of a comparison is
//! kept in a schema of its own on the server `LAKELANDS_TEST_PG_URL`
//! names; without it the tests stand down.

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

#[tokio::test(flavor = "multi_thread")]
async fn a_batch_answers_and_logs_as_its_writes_alone() {
    let Some(url) = common::url_or_skip("a_batch_answers_and_logs_as_its_writes_alone") else {
        return;
    };
    let (place, given) = place(&url);
    batched::a_batch_answers_and_logs_as_its_writes_alone(&place, "batch").await;
    for schema in given.lock().unwrap().iter() {
        common::drop_schema(&url, schema);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_batch_answers_every_job_and_the_lake_writes_again() {
    let test = "a_failed_batch_answers_every_job_and_the_lake_writes_again";
    let Some(url) = common::url_or_skip(test) else {
        return;
    };
    let (place, given) = place(&url);
    batched::a_failed_batch_answers_every_job_and_the_lake_writes_again(&place, "faulty").await;
    for schema in given.lock().unwrap().iter() {
        common::drop_schema(&url, schema);
    }
}
