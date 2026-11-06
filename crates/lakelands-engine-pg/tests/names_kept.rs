//! A flush names the types and sources of its events once: a name the
//! projection already holds takes no id, so the ids of `names` stay as
//! many as its rows however many flushes run.

mod common;

use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::seam::Seams;
use lakelands_api::{LakeStore, Location};
use lakelands_core::conformance::fixture::{Lake, source_record, sourced, type_decl};
use lakelands_engine_pg::PgOpener;
use serde_json::json;

async fn open(location: &Location) -> lakelands_api::Result<Arc<dyn LakeStore>> {
    let seams = Seams {
        engines: vec![Arc::new(PgOpener)],
        ..Seams::default()
    };
    lakelands_store::open(location.clone(), seams, OpenMode::Create).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flushes_over_known_names_burn_no_ids() {
    let Some(url) = common::url_or_skip("names kept") else {
        return;
    };
    let schema = common::fresh_schema();
    let store = open(&common::location(&url, "names", &schema))
        .await
        .unwrap();
    let l = Lake::of(store.as_ref());
    l.declare(type_decl(
        "note",
        vec![sourced("n", lakelands_api::ontology::StorageKind::Decimal)],
    ))
    .await
    .unwrap();
    for i in 0..5 {
        let r = source_record("note", &format!("k{i}"), json!({"n": "1"}));
        l.ingest("crm", vec![r]).await.unwrap();
    }
    // The next new name is the one a burned id would show in.
    let r = source_record("note", "z", json!({"n": "1"}));
    l.ingest("erp", vec![r]).await.unwrap();
    store.close().await.unwrap();
    let ids = format!("SELECT max(id)::int8 * 1000 + count(*) FROM \"{schema}\".names");
    let packed: i64 = tokio::task::block_in_place(|| {
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        client.query_one(&ids, &[]).unwrap().get(0)
    });
    let (max, count) = (packed / 1000, packed % 1000);
    assert!(count >= 3, "the type and both sources are named");
    assert_eq!(max, count, "no flush took an id for a name it held");
    common::drop_schema(&url, &schema);
}
