//! A checkpoint whose event the server refuses has sealed nothing
//! (lakelands#623): the seal and the next segment's first events are one
//! transaction. The refusal is a real one, a trigger on the log's table
//! that raises for a checkpoint's row. By hand, with
//! `LAKELANDS_TEST_PG_URL` set.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use lakelands_api::engine::OpenMode;
use lakelands_api::seam::Seams;
use lakelands_api::{Grants, LakeStore, Principal, PrincipalKind, WriteContext};
use lakelands_engine_pg::PgOpener;
use serde_json::json;

fn seams() -> Seams {
    Seams {
        engines: vec![Arc::new(PgOpener)],
        ..Seams::default()
    }
}

/// Run `statements` on a connection of their own, and answer how many
/// segments the lake in `schema` keeps and how many of them are sealed.
fn admin(url: &str, schema: &str, statements: &str) -> (i64, i64) {
    let (url, statements) = (url.to_owned(), statements.to_owned());
    let count = format!("SELECT count(*), count(digest) FROM \"{schema}\".segments");
    std::thread::spawn(move || {
        let mut client = postgres::Client::connect(&url, postgres::NoTls).unwrap();
        client.batch_execute(&statements).unwrap();
        let row = client.query_one(&count, &[]).unwrap();
        (row.get(0), row.get(1))
    })
    .join()
    .unwrap()
}

async fn put(store: &dyn LakeStore, owner: &Principal, key: &str) {
    let patch = serde_json::from_value(json!({ "note": key })).unwrap();
    let cat = store.catalog(WriteContext::default());
    cat.upsert(
        owner,
        &"entry".into(),
        &key.into(),
        patch,
        Default::default(),
    )
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_checkpoint_whose_event_the_server_refuses_has_sealed_nothing() {
    let Some(url) = common::url_or_skip("the refused checkpoint") else {
        return;
    };
    let (schema, target) = (common::fresh_schema(), common::fresh_schema());
    let location = common::location(&url, "ledger", &schema);
    let store = lakelands_store::open(location, seams(), OpenMode::Create)
        .await
        .unwrap();
    let owner = Principal {
        kind: PrincipalKind::Human,
        id: "owner".into(),
        lake: "ledger".into(),
        grants: Grants::owner(),
    };
    let declared = json!({"declare": "type", "name": "entry",
        "fields": [{"name": "note", "type": {"kind": "text"}}]});
    let cat = store.catalog(WriteContext::default());
    cat.declare(&owner, serde_json::from_value(declared).unwrap())
        .await
        .unwrap();
    put(store.as_ref(), &owner, "a").await;
    let head = store.head().await.unwrap();

    let refuse = format!(
        "CREATE FUNCTION \"{schema}\".refuse() RETURNS trigger LANGUAGE plpgsql AS \
         $$ BEGIN RAISE EXCEPTION 'a checkpoint''s row is refused'; END $$; \
         CREATE TRIGGER refuse BEFORE INSERT ON \"{schema}\".log FOR EACH ROW \
         WHEN (position('\"kind\":\"checkpoint\"' in convert_from(NEW.bytes, 'UTF8')) > 0) \
         EXECUTE FUNCTION \"{schema}\".refuse();"
    );
    admin(&url, &schema, &refuse);
    let failed = store.checkpoint().await.unwrap_err().to_string();
    assert!(failed.contains("a checkpoint's row is refused"), "{failed}");
    let kept = admin(
        &url,
        &schema,
        &format!("DROP TRIGGER refuse ON \"{schema}\".log"),
    );
    assert_eq!(kept, (1, 0), "segments kept, and sealed: {failed}");
    assert_eq!(store.head().await.unwrap(), head);

    // The next write lands in the first segment, and the checkpoint sent
    // again seals it: the lake verifies, exports and imports.
    put(store.as_ref(), &owner, "b").await;
    let sealed = store.checkpoint().await.unwrap();
    assert_eq!(sealed.0, head.0 + 2);
    put(store.as_ref(), &owner, "c").await;
    assert_eq!(admin(&url, &schema, ""), (2, 1));
    let said = store.verify(None, None).await.unwrap();
    assert_eq!(said.through.0, sealed.0 + 1);
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("pg-one-step-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let manifest = lakelands_core::export::write(store.as_ref(), &out).await;
    let files: Vec<(u64, u64)> = (manifest.unwrap().segments.iter())
        .map(|s| (s.first.0, s.last.0))
        .collect();
    assert_eq!(files, [(0, sealed.0 - 1), (sealed.0, sealed.0 + 1)]);
    let read = lakelands_core::export::verify(&out).unwrap();
    assert_eq!(read.through.0, sealed.0 + 1);
    let to = common::location(&url, "ledger", &target);
    let imported = lakelands_store::import(to, seams(), out.clone()).await;
    assert_eq!(imported.unwrap().files, 2);
    store.close().await.unwrap();
    let _ = std::fs::remove_dir_all(&out);
    common::drop_schemas(&url);
}
