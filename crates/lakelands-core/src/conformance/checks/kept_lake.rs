//! The lake v0.23.0 made through its doors, imported as an export and read on every engine (`docs/log-format/lakes/23/`, record 054).
use crate::conformance::ImportedLake;
use crate::conformance::fixture::owner;
use crate::conformance::prelude::*;
use lakelands_api::LakeStore;

check!(
    LAKE_23_READ = the_lake_of_format_23_is_read_as_it_answered,
    "054: the lake v0.23.0 made through its doors (`docs/log-format/lakes/23/`), in format 23, imported on this engine with the file keyring's file it ships, keeps its log, verifies, and answers as `answers.json` keeps the answers: every live record of each type it names with its fields and its stranded mark, the group `daily` kept past a thinning with its roll-up standing and the copy of the time-zone database that cut it, and a person a removal left holding a source's value with another's erased, with its provenance; and it answers so again rebuilt",
    given
);

async fn the_lake_of_format_23_is_read_as_it_answered(store: Store, given: Given) -> Held {
    store.head().await.must("reading the head")?;
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/log-format/lakes/23");
    let said = std::fs::read(dir.join("answers.json")).map_err(no)?;
    let answers: Value = serde_json::from_slice(&said).map_err(no)?;
    let lake = lakelands_api::LakeId::from("kept23");
    let keys = super::erasable::Keys::from_file(&dir.join("keys/kept23.keys.json"), &lake)?;
    let seams = super::erasable::with(&std::sync::Arc::new(keys));
    let imported = (given.import)(dir, lake, None, seams.clone()).await;
    let ImportedLake { store, writer, .. } = imported.map_err(no)?;
    store.verify(None, None).await.must("verifying the lake")?;
    let red = answered(store.as_ref(), &answers).await?;
    ensure!(red.is_empty(), "at the head: {}", red.join("; "));
    let store = writer(store, seams).await.map_err(no)?;
    store.rebuild().await.must("rebuilding the lake")?;
    let red = answered(store.as_ref(), &answers).await?;
    ensure!(red.is_empty(), "rebuilt: {}", red.join("; "));
    Ok(())
}

fn no<T: std::fmt::Debug>(e: T) -> Failure {
    Failure::new(format!("reading the lake: {e:?}"))
}

/// Where the lake answers otherwise than `answers` keeps, part by part: a record a type name, a key, its fields and its stranded mark, a stranded group's `cut` held as a copy named, not which copy it is.
async fn answered(store: &dyn LakeStore, answers: &Value) -> Result<Vec<String>, Failure> {
    let (own, cat) = (owner(store), crate::conformance::fixture::doors(store));
    let (mut records, mut erasable) = (Vec::new(), Vec::new());
    let types = answers["types"].as_array().cloned().unwrap_or_default();
    for ty in types.iter().filter_map(Value::as_str) {
        for r in all(cat.as_ref(), &own, ty).await? {
            records.push(json!([ty, r.key, r.fields, r.stranded, r.cut.is_some()]));
        }
    }
    for said in answers["erasable"].as_array().into_iter().flatten() {
        let ty = said[0].as_str().unwrap_or("");
        let key = said[1].as_str().unwrap_or("");
        let get = crate::conformance::fixture::get_at(key, None);
        let r = cat.get(&own, &ty.into(), get).await;
        let r = r.must(&format!("reading `{key}`"))?.data;
        erasable.push(json!([ty, key, r.fields, r.meta]));
    }
    let now = json!({"records": records, "erasable": erasable});
    let mut red = Vec::new();
    for part in ["records", "erasable"] {
        if answers[part] != now[part] {
            let (want, got) = (answers[part].to_string(), now[part].to_string());
            red.push(format!("{part}: v0.23.0 {want}, this release {got}"));
        }
    }
    Ok(red)
}
