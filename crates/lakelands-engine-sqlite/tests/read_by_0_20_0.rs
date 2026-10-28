//! A lake of format 21, its index lines naming `id` empty, is read by
//! 0.20.0 far enough to be refused by its format, not called damaged
//! (lakelands#1517, decision 086). `older` is 0.20.0's reader of the ends,
//! its rules copied from v0.20.0's `src/log.rs` (`parse_line`, `whole`,
//! `ends`) and kept fixed: an event line of eight members, `id` any
//! string, the last group counted where each line restores to its hash.

mod read_by;

use std::path::{Path, PathBuf};

use lakelands_api::Location;
use lakelands_api::engine::OpenMode;
use lakelands_api::event::LOG_FORMAT_VERSION;
use lakelands_api::ontology::StorageKind::Text;
use lakelands_core::conformance::fixture::{Lake, log_events, source_record, sourced, type_decl};
use read_by::{Old, seams};
use serde_json::{Value, json};

/// 0.20.0's `ends` in `dir`: the first and last committed `v`, and every
/// line it counted; every way it would call the log damaged is an error.
fn older(dir: &Path) -> Result<(u64, u64, Vec<Old>), String> {
    let firsts = read_by::segments(dir);
    let (mut vs, mut counted) = (Vec::new(), Vec::new());
    for (i, first) in firsts.iter().enumerate() {
        let (segment, mut groups) = read_by::groups(dir, *first)?;
        let mut lines = lakelands_core::log::Lines::default();
        let events: Vec<Value> = (segment.split(|b| *b == b'\n').filter(|l| !l.is_empty()))
            .filter_map(|l| lines.read(l).unwrap())
            .map(|e| serde_json::from_slice(&e).unwrap())
            .collect();
        // `whole`: each line ends in a line feed and restores to its hash.
        let whole = |g: &Vec<Old>| {
            g.iter().all(
                |Old {
                     position,
                     offset,
                     len,
                     hash,
                     ..
                 }| {
                    let mut event = events.get((position - first) as usize).cloned();
                    let event = event.as_mut().and_then(Value::as_object_mut).map(|e| {
                        e.remove("hash");
                        lakelands_core::canonical::sha256_hex(&Value::Object(e.clone()))
                    });
                    segment.get((offset + len) as usize) == Some(&b'\n')
                        && event.as_ref() == Some(hash)
                },
            )
        };
        if i + 1 == firsts.len() && groups.last().is_some_and(|g| !whole(g)) {
            groups.pop();
        }
        for (n, e) in groups.iter().flatten().enumerate() {
            if e.position != first + n as u64 {
                return Err(format!("segment {first}: line {n} names {}", e.position));
            }
            vs.push(events[n]["v"].as_u64().unwrap_or(0));
            counted.push(e.clone());
        }
    }
    let ends = vs.first().zip(vs.last()).ok_or("no committed event")?;
    Ok((*ends.0, *ends.1, counted))
}

/// Appends of three records each, around notes that take a segment past
/// its bound, so that a seal begins the next.
async fn written(location: &Location, mode: OpenMode, from: usize) {
    let store = lakelands_store::open(location.clone(), seams(), mode);
    let store = store.await.unwrap();
    let l = Lake::of(store.as_ref());
    if mode == OpenMode::Create {
        l.declare(type_decl("note", vec![sourced("text", Text)]))
            .await
            .unwrap();
    }
    for i in from..from + 6 {
        let text = "n".repeat(if i % 3 == 0 { 300 << 10 } else { 6 });
        let records =
            (0..3).map(|j| source_record("note", &format!("n{i}-{j}"), json!({ "text": text })));
        l.ingest("load", records.collect()).await.unwrap();
    }
    store.close().await.unwrap();
}

/// Every index line in `dir` naming its event's id, as 0.20.0 wrote them.
fn named_as_0_20_0(dir: &Path) {
    for file in std::fs::read_dir(dir.join("index")).unwrap() {
        let path = file.unwrap().path();
        let stem = path.file_stem().unwrap().to_str().unwrap();
        let segment = std::fs::read(dir.join("log").join(format!("{stem}.jsonl"))).unwrap();
        let mut named = String::new();
        for line in std::fs::read_to_string(&path).unwrap().lines() {
            let mut e: Value = serde_json::from_str(line).unwrap();
            if e[0] == "e" {
                let (from, len) = (e[3].as_u64().unwrap() as usize, e[4].as_u64().unwrap());
                let held: Value = serde_json::from_slice(&segment[from..][..len as usize]).unwrap();
                e[5] = held["id"].clone();
            }
            named += &format!("{e}\n");
        }
        std::fs::write(&path, named).unwrap();
    }
}

/// A lake this build made: both ends by 21, every `id` named empty; named
/// as 0.20.0 wrote them, the next appends mix lines of both, read whole.
#[tokio::test(flavor = "multi_thread")]
async fn a_lake_this_build_made_is_read_by_0_20_0_up_to_its_format() {
    let dir = read_by::scratch("0-20-0", "made");
    let mut location = Location::sqlite("lake", &dir);
    location.limits.seal_after_bytes = 1 << 20;
    written(&location, OpenMode::Create, 0).await;
    let reads = u64::from(LOG_FORMAT_VERSION);
    let (first, tip, lines) = older(&dir).unwrap();
    assert_eq!((first, tip), (reads, reads), "both ends by {reads}");
    assert!(lines.iter().all(|l| l.id.is_empty()), "ids named empty");
    named_as_0_20_0(&dir);
    written(&location, OpenMode::Write, 6).await;
    let (_, _, lines) = older(&dir).unwrap();
    assert!(std::fs::read_dir(dir.join("log")).unwrap().count() > 1);
    let named = lines.iter().filter(|l| !l.id.is_empty()).count();
    assert!(named > 0 && named < lines.len(), "{named} named");
    let store = lakelands_store::open(location.clone(), seams(), OpenMode::Read);
    let store = store.await.unwrap();
    let events = log_events(store.as_ref()).await.unwrap();
    assert_eq!(events.len(), lines.len(), "every event is read");
    for (event, line) in events.iter().zip(&lines) {
        assert_eq!(event.position.0, line.position);
        assert!(line.id.is_empty() || line.id == event.id.0, "{line:?}");
    }
    store.verify(None, None).await.unwrap();
    store.close().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lake_of_20_upgraded_is_read_by_0_20_0_up_to_its_format() {
    let vectors =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/log-format/vectors/20");
    let dir = read_by::scratch("0-20-0", "upgraded");
    let location = Location::sqlite("vectors", &dir);
    lakelands_store::import(location.clone(), seams(), vectors)
        .await
        .unwrap();
    for opens in 0..2 {
        if opens > 0 {
            let store = lakelands_store::open(location.clone(), seams(), OpenMode::Write);
            store.await.unwrap().close().await.unwrap();
        }
        let (first, tip, _) = older(&dir).unwrap();
        let reads = u64::from(LOG_FORMAT_VERSION);
        assert_eq!((first, tip), (20, reads), "after {opens} opens");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
