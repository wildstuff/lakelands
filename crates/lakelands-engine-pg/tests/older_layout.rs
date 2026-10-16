//! A projection an older build laid out (lakelands#1151): its `routes`
//! without the `source` column and its mark one layout back. A reader's
//! open is refused naming the layouts; the writer's open rebuilds it from
//! the log under its lease, saying why, and the next writer's open
//! rebuilds nothing.

mod common;

use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::error::Situation;
use lakelands_api::ontology::StorageKind::Text;
use lakelands_api::seam::Seams;
use lakelands_api::{LakeStore, Location};
use lakelands_core::conformance::fixture::{Lake, source_record, sourced, type_decl};
use lakelands_engine_pg::PgOpener;
use serde_json::json;

async fn open(location: &Location, mode: OpenMode) -> lakelands_api::Result<Arc<dyn LakeStore>> {
    let seams = Seams {
        engines: vec![Arc::new(PgOpener)],
        ..Seams::default()
    };
    lakelands_store::open(location.clone(), seams, mode).await
}

/// The first column of the first row `statement` answers.
fn one(url: &str, statement: &str) -> i64 {
    let mut client = postgres::Client::connect(url, postgres::NoTls).unwrap();
    client.query_one(statement, &[]).unwrap().get(0)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_writers_open_rebuilds_a_projection_an_older_build_laid_out() {
    let Some(url) = common::url_or_skip("older layout") else {
        return;
    };
    let schema = common::fresh_schema();
    let location = common::location(&url, "older", &schema);
    let store = open(&location, OpenMode::Create).await.unwrap();
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("note", vec![sourced("text", Text)]))
        .await
        .unwrap();
    let a = source_record("note", "a", json!({"text": "kept"}));
    l.ingest("crm", vec![a]).await.unwrap();
    store.close().await.unwrap();
    let layout = format!("SELECT n FROM \"{schema}\".meta WHERE k = 'layout'");
    let wanted = tokio::task::block_in_place(|| one(&url, &layout));
    let older = format!(
        "DROP INDEX \"{schema}\".routes_source; ALTER TABLE \"{schema}\".routes DROP COLUMN source; \
         UPDATE \"{schema}\".meta SET n = {} WHERE k = 'layout'",
        wanted - 1
    );
    tokio::task::block_in_place(|| {
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        client.batch_execute(&older).unwrap();
    });
    let refused = open(&location, OpenMode::Read).await.err().unwrap();
    assert!(
        matches!(refused.situation(), Some(Situation::ProjectionLayout { found, .. }) if *found == Some(wanted as u64 - 1)),
        "a reader's open of the older layout is refused: {refused:?}"
    );
    let store = open(&location, OpenMode::Write).await.unwrap();
    let why = store.rebuilt_at_open();
    assert!(
        why.as_ref()
            .is_some_and(|w| w.contains(&format!("layout {}", wanted - 1))),
        "the writer's open says why it rebuilt: {why:?}"
    );
    let note = Lake::of(store.as_ref()).read("note", "a").await.unwrap();
    assert_eq!(note.fields["text"], "kept");
    let sourced = format!("SELECT count(*) FROM \"{schema}\".routes WHERE source = 'crm'");
    let (sourced, now) = tokio::task::block_in_place(|| (one(&url, &sourced), one(&url, &layout)));
    assert!(
        sourced > 0 && now == wanted,
        "the rebuild indexed crm's events at layout {wanted}"
    );
    store.close().await.unwrap();
    let store = open(&location, OpenMode::Write).await.unwrap();
    assert_eq!(
        store.rebuilt_at_open(),
        None,
        "the next writer's open rebuilds nothing"
    );
    store.close().await.unwrap();
    common::drop_schema(&url, &schema);
}
