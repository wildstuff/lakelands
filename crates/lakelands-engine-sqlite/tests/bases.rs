//! The projection keeps a base once where a write shares it (lakelands#1432):
//! the records of an append of [`SHARED`] name one row of `bases`, those of
//! a smaller one keep theirs inline; forgetting a source by the scoped
//! refold leaves no row that no state names, and no state names a row that
//! is gone.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use lakelands_api::engine::OpenMode;
use lakelands_api::id::KeyId;
use lakelands_api::ontology::{FieldDecl, StorageKind::Text};
use lakelands_api::seam::{DataKey, KeyStore, Seams};
use lakelands_api::{LakeId, Location, Result, SourceId};
use lakelands_core::conformance::fixture::{Lake, source_record, sourced, type_decl};
use lakelands_core::engines::SHARED;
use lakelands_engine_sqlite::SqliteOpener;
use serde_json::json;

/// Keys kept in memory, one version a source.
#[derive(Default)]
struct Keys(Mutex<BTreeMap<String, Option<[u8; 32]>>>);

impl KeyStore for Keys {
    fn create(&self, _: &LakeId, source: &SourceId) -> Result<KeyId> {
        let key = [source.as_str().len() as u8; 32];
        self.0.lock().unwrap().insert(source.to_string(), Some(key));
        Ok(KeyId::from("k1".to_owned()))
    }
    fn key(&self, _: &LakeId, source: &SourceId, _: &KeyId) -> Result<Option<DataKey>> {
        let keys = self.0.lock().unwrap();
        Ok(keys.get(source.as_str()).copied().flatten().map(DataKey))
    }
    fn destroy(&self, _: &LakeId, source: &SourceId) -> Result<()> {
        self.0.lock().unwrap().insert(source.to_string(), None);
        Ok(())
    }
    fn ids(&self, _: &LakeId, source: &SourceId) -> Result<Vec<KeyId>> {
        let keys = self.0.lock().unwrap();
        let held = keys.get(source.as_str()).is_some_and(Option::is_some);
        Ok(held
            .then(|| KeyId::from("k1".to_owned()))
            .into_iter()
            .collect())
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_shared_base_is_one_row_a_lone_one_inline_and_a_forgetting_leaves_none_unnamed() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("bases-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let seams = Seams {
        engines: vec![Arc::new(SqliteOpener)],
        keys: Some(Arc::new(Keys::default())),
        ..Seams::default()
    };
    let location = Location::sqlite("bases", &dir);
    let store = lakelands_store::open(location, seams, OpenMode::Create)
        .await
        .unwrap();
    let l = Lake::of(store.as_ref());
    let erasable = |name: &str| FieldDecl {
        erasable: true,
        ..sourced(name, Text)
    };
    let fields = vec![erasable("title"), erasable("n")];
    l.declare(type_decl("note", fields)).await.unwrap();
    let rows = |prefix: &str, n: usize| -> Vec<_> {
        (0..n)
            .map(|i| format!("{prefix}{i}"))
            .map(|key| source_record("note", &key, json!({"title": key, "n": key})))
            .collect()
    };
    l.ingest("crm", rows("s", SHARED)).await.unwrap();
    l.ingest("erp", rows("e", 1)).await.unwrap();
    l.ingest("crm", rows("f", SHARED - 1)).await.unwrap();
    let db = rusqlite::Connection::open(dir.join("projection.db")).unwrap();
    let count = |sql: &str| -> i64 { db.query_row(sql, [], |r| r.get(0)).unwrap() };
    let base = |key: &str| -> (String, String) {
        let sql = "SELECT json_type(state, '$.base'), json_extract(state, '$.base') \
                   FROM records WHERE ty = 'note' AND key = ?1";
        db.query_row(sql, [key], |r| {
            Ok((r.get(0)?, r.get::<_, rusqlite::types::Value>(1)?))
        })
        .map(|(t, v)| (t, format!("{v:?}")))
        .unwrap()
    };
    let shared: Vec<_> = (0..SHARED).map(|i| base(&format!("s{i}"))).collect();
    assert!(
        shared.iter().all(|b| b.0 == "integer" && *b == shared[0]),
        "the records of a write of {SHARED} name one row: {shared:?}"
    );
    let alone: Vec<_> = (0..SHARED - 1)
        .map(|i| base(&format!("f{i}")))
        .chain([base("e0")])
        .collect();
    assert!(
        alone.iter().all(|b| b.0 == "object"),
        "a smaller write keeps its base inline: {alone:?}"
    );
    assert_eq!(
        count("SELECT count(*) FROM bases"),
        1,
        "one row, the shared write's"
    );
    let unnamed = "SELECT count(*) FROM bases WHERE id NOT IN \
                   (SELECT json_extract(state, '$.base') FROM records \
                   WHERE json_type(state, '$.base') = 'integer')";
    let dangling = "SELECT count(*) FROM records WHERE json_type(state, '$.base') = 'integer' \
                    AND json_extract(state, '$.base') NOT IN (SELECT id FROM bases)";
    let report = store.forget_preflight(&"crm".into()).await.unwrap();
    assert!(
        report.way.scoped,
        "the forgetting refolds in scope: {report:?}"
    );
    store.forget_source(&"crm".into(), None).await.unwrap();
    let id = shared[0]
        .1
        .trim_start_matches("Integer(")
        .trim_end_matches(')');
    let kept = format!("SELECT count(*) FROM bases WHERE id = {id}");
    assert_eq!(
        (count(&kept), count(unnamed), count(dangling)),
        (0, 0, 0),
        "crm's base goes with the states that named it; every state's base is held"
    );
    assert_eq!(base("e0").0, "object", "erp's stays inline");
    store.close().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
