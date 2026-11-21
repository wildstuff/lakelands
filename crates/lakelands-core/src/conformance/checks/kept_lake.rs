//! The kept lakes (record 054): the lake v0.24.0 made through its doors (`docs/log-format/lakes/24/`) and the lake v0.23.0 made (`docs/log-format/lakes/23/`), each imported as an export and read on every engine; the lake of format 23 is carried up to format 24 by the writer's open that appends its first event of format 24 (lakelands#1824).
use crate::conformance::ImportedLake;
use crate::conformance::fixture::{doors, get_at, owner};
use crate::conformance::prelude::*;

check!(
    LAKE_24_READ = the_lake_of_format_24_is_read_as_it_answered,
    "054, 115: the lake v0.24.0 made through its doors (`docs/log-format/lakes/24/`), in format 24, imported on this engine with the file keyring's file it ships, keeps its log, verifies, and answers as `answers.json` keeps the answers: its head and the position and kind of every event, every live record of each type it names with its fields, a person whose values an erasure took whole, a person with one field erased, and an invoice whose state-move fields a verb moved, each with its provenance; and a writer that opens it and rebuilds its log answers its records so again",
    given
);

async fn the_lake_of_format_24_is_read_as_it_answered(_store: Store, given: Given) -> Held {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/log-format/lakes/24");
    let said = std::fs::read(dir.join("answers.json")).map_err(no)?;
    let answers: Value = serde_json::from_slice(&said).map_err(no)?;
    let lake = lakelands_api::LakeId::from("kept24");
    let keys = super::erasable::Keys::from_file(&dir.join("keys/kept24.keys.json"), &lake)?;
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

/// Where, `then`, the lake answers otherwise than `answers` keeps, part by part: with `log`, its head and every event's position and kind too (an import appends nothing, a writer's open its `lease_taken`), and always a record as a type name, a key, its fields and its stranded mark (with, in a lake v0.23.0 made, whether a copy is named as its `cut`), and each `erasable` record named with its fields and provenance.
async fn answered(store: &dyn LakeStore, kept: &Value, then: &str, log: bool) -> Held {
    let (own, cat) = (owner(store), doors(store));
    let head = store.head().await.must("reading the head")?;
    let mut events = Vec::new();
    for e in log_events(store).await? {
        events.push(format!("{} {}", e.position, e.body.kind()));
    }
    let (mut records, mut erasable) = (Vec::new(), Vec::new());
    let kept_records = kept["records"].as_array().into_iter().flatten();
    let cut_named = kept_records
        .filter_map(Value::as_array)
        .any(|r| r.len() == 5);
    for ty in kept["types"].as_array().into_iter().flatten() {
        for r in all(cat.as_ref(), &own, ty.as_str().unwrap_or("")).await? {
            // A lake of format 23 kept a stranded group's `cut` as a copy
            // named, not which copy it is: its answers hold that as a
            // record's fifth part.
            match cut_named {
                true => records.push(json!([
                    ty,
                    r.key,
                    r.fields,
                    r.stranded,
                    r.cut.is_some_and(|c| !c.is_empty())
                ])),
                false => records.push(json!([ty, r.key, r.fields, r.stranded])),
            }
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
            json!({"now": now[part], "kept": kept[part]})
        );
    }
    Ok(())
}

check!(
    LAKE_23_READ = the_lake_of_format_23_is_read_and_carried_up,
    "054, 115: the lake v0.23.0 made through its doors (`docs/log-format/lakes/23/`), in format 23, imported on this engine with the file keyring's file it ships, keeps its log, verifies, and answers as `answers.json` keeps the answers: its head and the position and kind of every event, every live record of each type it names with its fields, its stranded mark and its cut named, and a person a removal left holding a source's value with another's erased, with its provenance; a writer's open carries it up to format 24 and says it carried it up from 23: the `lease_taken` it appends is the lake's first event of format 24, every event before it keeps format 23, the lake verifies, and it describes itself to its owner as it did imported, every type with its doc and fields, every declaration and every setting with its value; a second writer's open carries nothing up; and a rebuild of its log answers its records and describes the lake so again",
    given
);

async fn the_lake_of_format_23_is_read_and_carried_up(_store: Store, given: Given) -> Held {
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
    let before = described(store.as_ref()).await?;
    let store = writer.clone()(store, seams.clone()).await.map_err(no)?;
    let (from, head) = (
        store.upgraded_at_open(),
        store.head().await.must("the head")?,
    );
    ensure!(
        from == Some(23),
        "the writer's open says it carried the lake up from {from:?}, not 23"
    );
    let formats: Vec<_> = log_events(store.as_ref())
        .await?
        .iter()
        .map(|e| (e.v, e.body.kind()))
        .collect();
    let tip = formats.last().cloned();
    ensure!(
        tip == Some((24, "lease_taken"))
            && formats[..formats.len() - 1].iter().all(|(v, _)| *v == 23),
        "after the writer's open, through {head}, the log is of formats {formats:?}"
    );
    store
        .verify(None, None)
        .await
        .must("verifying the lake carried up")?;
    same(&before, described(store.as_ref()).await?, "carried up")?;
    let store = writer(store, seams).await.map_err(no)?;
    let again = store.upgraded_at_open();
    ensure!(
        again.is_none(),
        "a second writer's open carried the lake up again, from {again:?}"
    );
    store.rebuild().await.must("rebuilding the lake")?;
    answered(store.as_ref(), &answers, "rebuilt", false).await?;
    same(&before, described(store.as_ref()).await?, "rebuilt")?;
    Ok(())
}

/// What the lake tells its owner it holds: every type with its doc and
/// fields, its verbs, value types, functions, triggers, object sets and
/// every setting with its value. A series' `state` is left out: it says
/// how far the projection has built the series' index, which an open goes
/// on building, not what the lake holds.
async fn described(store: &dyn LakeStore) -> std::result::Result<Value, Failure> {
    let (own, cat) = (owner(store), doors(store));
    let d = cat.describe(&own, Default::default()).await;
    let mut d = serde_json::to_value(d.must("describing the lake")?.data).map_err(no)?;
    for t in d["types"].as_array_mut().into_iter().flatten() {
        for s in t["series"].as_array_mut().into_iter().flatten() {
            s.as_object_mut().map(|s| s.remove("state"));
        }
    }
    Ok(d)
}

/// Where the lake, `then`, describes itself otherwise than it did imported.
fn same(before: &Value, now: Value, then: &str) -> Held {
    ensure!(
        *before == now,
        "{then}, the lake describes itself otherwise: {}",
        json!({"now": now, "imported": before})
    );
    Ok(())
}
