//! The lake v0.23.0 made through its doors, imported as an export and read on every engine (`docs/log-format/lakes/23/`, record 054).
use crate::conformance::ImportedLake;
use crate::conformance::fixture::{doors, get_at, owner};
use crate::conformance::prelude::*;

check!(
    LAKE_23_READ = the_lake_of_format_23_is_read_as_it_answered,
    "054: the lake v0.23.0 made through its doors (`docs/log-format/lakes/23/`), in format 23, imported on this engine with the file keyring's file it ships, keeps its log, verifies, and answers as `answers.json` keeps the answers: its head and the position and kind of every event, every live record of each type it names with its fields and its stranded mark, the group `daily` kept past a thinning with its roll-up standing and its record naming a copy of the time-zone database as the one that cut it, and a person a removal left holding a source's value with another's erased, with its provenance; and a writer that opens it and rebuilds its log answers its records so again",
    given
);

async fn the_lake_of_format_23_is_read_as_it_answered(_store: Store, given: Given) -> Held {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/log-format/lakes/23");
    let said = std::fs::read(dir.join("answers.json")).map_err(no)?;
    let answers: Value = serde_json::from_slice(&said).map_err(no)?;
    let lake = lakelands_api::LakeId::from("kept23");
    let keys = super::erasable::Keys::from_file(&dir.join("keys/kept23.keys.json"), &lake)?;
    let seams = super::erasable::with(&std::sync::Arc::new(keys));
    let imported = (given.import)(dir, lake, None, seams.clone()).await;
    let ImportedLake { store, writer, .. } = imported.map_err(no)?;
    store.verify(None, None).await.must("verifying the lake")?;
    answered(store.as_ref(), &answers, "imported", true).await?;
    let store = writer(store, seams).await.map_err(no)?;
    store.rebuild().await.must("rebuilding the lake")?;
    answered(store.as_ref(), &answers, "rebuilt", false).await?;
    Ok(())
}

fn no<T: std::fmt::Display>(e: T) -> Failure {
    Failure::new(format!("reading the lake: {e}"))
}

/// Where, `then`, the lake answers otherwise than `answers` keeps, part by part: with `log`, its head and every event's position and kind too (an import appends nothing, a writer's open its `lease_taken`), and always a record a type name, a key, its fields and its stranded mark, a stranded group's `cut` held as a copy named, not which copy it is.
async fn answered(store: &dyn LakeStore, kept: &Value, then: &str, log: bool) -> Held {
    let (own, cat) = (owner(store), doors(store));
    let head = store.head().await.must("reading the head")?;
    let mut events = Vec::new();
    for e in log_events(store).await? {
        events.push(format!("{} {}", e.position, e.body.kind()));
    }
    let (mut records, mut erasable) = (Vec::new(), Vec::new());
    for ty in kept["types"].as_array().into_iter().flatten() {
        for r in all(cat.as_ref(), &own, ty.as_str().unwrap_or("")).await? {
            let named = r.cut.is_some_and(|c| !c.is_empty());
            records.push(json!([ty, r.key, r.fields, r.stranded, named]));
        }
    }
    for said in kept["erasable"].as_array().into_iter().flatten() {
        let ty = said[0].as_str().unwrap_or("");
        let key = said[1].as_str().unwrap_or("");
        let r = cat.get(&own, &ty.into(), get_at(key, None)).await;
        let r = r.must(&format!("reading `{key}`"))?.data;
        erasable.push(json!([ty, key, r.fields, r.meta]));
    }
    let now = json!({"head": head, "events": events, "records": records, "erasable": erasable});
    for part in &["head", "events", "records", "erasable"][if log { 0 } else { 2 }..] {
        ensure!(
            kept[part] == now[part],
            "{then} {part}: {}",
            json!({"now": now[part], "v0.23.0": kept[part]})
        );
    }
    Ok(())
}
