//! A lake this build makes is refused by a binary of 0.22.x by its format,
//! not called damaged and not read (record 098).
//!
//! 0.22.x reads log formats 14 to 22. It asks a log's ends before it opens
//! the engine, and judges three numbers: the first event's `v`, the format
//! that event's `lake_created` says, and the last event's `v`. The store
//! writes the append that makes a lake whole, so the first line is plain
//! however the later ones are stored, and every line holds its `v`.
//!
//! `refused_by` below is that reader, its rules copied as v0.22.0 has them
//! in `crates/lakelands-store/src/log.rs` (`one_format`) and
//! `crates/lakelands-engine-sqlite/src/log.rs` (`parse_line`, `ends`), and
//! kept fixed here: it does not move with this build, so no 0.22 build is
//! needed to run it.

mod read_by;

use std::ops::RangeInclusive;
use std::path::Path;

use lakelands_api::Location;
use lakelands_api::engine::OpenMode;
use lakelands_api::event::LOG_FORMAT_VERSION;
use lakelands_api::ontology::StorageKind::Text;
use lakelands_core::conformance::fixture::{Lake, source_record, sourced, type_decl};
use read_by::{Old, seams};
use serde_json::{Value, json};

/// The log formats 0.22.x reads, the oldest to the one it writes.
const READS: RangeInclusive<u64> = 14..=22;

/// The first and last committed lines of the log in `dir`, as JSON. Every
/// way 0.22.x would call the log damaged first is an error here.
fn ends(dir: &Path) -> Result<(Value, Value), String> {
    let mut lines = Vec::new();
    for first in read_by::segments(dir) {
        let (segment, groups) = read_by::groups(dir, first)?;
        let counted: Vec<Old> = groups.into_iter().flatten().collect();
        for (n, e) in counted.iter().enumerate() {
            if e.position != first + n as u64 {
                return Err(format!("segment {first}: line {n} names {}", e.position));
            }
            let (from, to) = (e.offset as usize, (e.offset + e.len) as usize);
            let line = segment.get(from..to).ok_or("a line past its segment")?;
            lines.push(serde_json::from_slice::<Value>(line).map_err(|e| e.to_string())?);
        }
    }
    let first = lines.first().cloned().ok_or("no committed event")?;
    Ok((first, lines.pop().ok_or("no committed event")?))
}

/// The format `one_format` of 0.22.x refuses the log in `dir` by, if any:
/// `None` is a lake that release goes on to read.
fn refused_by(dir: &Path) -> Result<Option<u64>, String> {
    let (first, tip) = ends(dir)?;
    let v = |line: &Value| line.get("v").and_then(Value::as_u64).unwrap_or(0);
    let made = first.pointer("/body/kind") == Some(&json!("lake_created"));
    let created = (first.pointer("/body/format").filter(|_| made)).map(|f| f.as_u64().unwrap_or(0));
    let said = [Some(v(&first)), created, Some(v(&tip))];
    Ok(said.into_iter().flatten().find(|f| !READS.contains(f)))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lake_just_made_is_refused_by_0_22_by_its_format() {
    let dir = read_by::scratch("0-22-0", "just-made");
    let location = Location::sqlite("lake", &dir);
    let store = lakelands_store::open(location, seams(), OpenMode::Create);
    store.await.unwrap().close().await.unwrap();
    assert_eq!(
        refused_by(&dir),
        Ok(Some(u64::from(LOG_FORMAT_VERSION))),
        "0.22.x refuses a lake just made by its format"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A lake past its first append and past a seal: framed lines and a
/// second segment change nothing of what 0.22.x judges.
#[tokio::test(flavor = "multi_thread")]
async fn a_lake_sealed_and_framed_is_refused_by_0_22_by_its_format() {
    let dir = read_by::scratch("0-22-0", "sealed");
    let mut location = Location::sqlite("lake", &dir);
    location.limits.seal_after_bytes = 1 << 20;
    let store = lakelands_store::open(location, seams(), OpenMode::Create);
    let store = store.await.unwrap();
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("note", vec![sourced("text", Text)]))
        .await
        .unwrap();
    for i in 0..12 {
        let text = "n".repeat(if i % 3 == 0 { 300 << 10 } else { 6 });
        let records =
            (0..3).map(|j| source_record("note", &format!("n{i}-{j}"), json!({ "text": text })));
        l.ingest("load", records.collect()).await.unwrap();
    }
    store.close().await.unwrap();
    drop((l, store));
    assert!(read_by::segments(&dir).len() > 1, "the lake was sealed");
    assert_eq!(
        refused_by(&dir),
        Ok(Some(u64::from(LOG_FORMAT_VERSION))),
        "0.22.x refuses the lake by its format"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
