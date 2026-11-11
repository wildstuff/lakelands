//! The kept lakes (record 054): the lake v0.24.0 made through its doors (`docs/log-format/lakes/24/`), imported as an export and read on every engine, and the lake v0.23.0 made (`docs/log-format/lakes/23/`), kept as bytes and refused on every engine: no format before 24 is read (record 115).
use crate::conformance::ImportedLake;
use crate::conformance::fixture::{doors, get_at, owner};
use crate::conformance::prelude::*;
use crate::export;

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

/// Where, `then`, the lake answers otherwise than `answers` keeps, part by part: with `log`, its head and every event's position and kind too (an import appends nothing, a writer's open its `lease_taken`), and always a record as a type name, a key, its fields and its stranded mark, and each `erasable` record named with its fields and provenance.
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
            records.push(json!([ty, r.key, r.fields, r.stranded]));
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
            json!({"now": now[part], "v0.24.0": kept[part]})
        );
    }
    Ok(())
}

check!(
    LAKE_23_REFUSED = the_lake_of_format_23_is_refused,
    "054, 115: the lake v0.23.0 made through its doors (`docs/log-format/lakes/23/`), in format 23, kept as its bytes, is refused by a verifier of its export and by an import on this engine as `log_format`, naming the lake, format 23 and the one format this release reads and writes; nothing of it is read and nothing upgraded",
    given
);

async fn the_lake_of_format_23_is_refused(_store: Store, given: Given) -> Held {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/log-format/lakes/23");
    let reads = u64::from(lakelands_api::event::LOG_FORMAT_VERSION);
    let lake = lakelands_api::LakeId::from("kept23");
    let keys = super::erasable::Keys::from_file(&dir.join("keys/kept23.keys.json"), &lake)?;
    let seams = super::erasable::with(&std::sync::Arc::new(keys));
    let verified = export::verify(&dir).err();
    let imported = (given.import)(dir, lake, None, seams).await.err();
    for (door, e) in [("a verifier", verified), ("an import", imported)] {
        // A refusal that is not of the format is the call's own failure.
        if let Some(e) = e
            .as_ref()
            .filter(|e| !matches!(e.situation(), Some(Situation::LogFormat { .. })))
        {
            ensure!(
                !matches!(e, Error::Unavailable { .. }),
                "{door} was unavailable: {e}"
            );
        }
        ensure!(
            matches!(e.as_ref().and_then(Error::situation),
                Some(Situation::LogFormat { lake: l, found: 23, reads: r, oldest })
                    if l == "kept23" && *r == reads && *oldest == Some(reads)),
            "{door} of the lake of format 23 refuses it as `log_format`, naming `kept23`, \
             format 23 and {reads}; it answered {e:?}"
        );
    }
    Ok(())
}
