//! A lake of format 21, its index lines naming `id` empty, is read by
//! 0.20.0 far enough that it is refused by its format, not called damaged
//! (lakelands#1517,
//! `docs/decisions/086-an-engine-keeps-an-events-id-once-and-its-hashes-as-bytes.md`).
//!
//! 0.20.0 judges a lake's format by the first and the last event of its
//! log, and finds them by reading each segment's index as 0.19.0 did
//! (`read_by_0_19_0.rs`), with two changes: an event line names `prev`
//! empty after a segment's first, and a frame's line is found by the gap
//! it leaves. It parses an event line as eight members, the `id` among
//! them a string, and calls any other shape damage; it reads the ends
//! and the last group by `v` and by `hash`, never by `id`.
//!
//! `older` below is that reader, its rules copied as v0.20.0 has them in
//! `crates/lakelands-engine-sqlite/src/log.rs` (`parse_line`, `whole`,
//! `ends`) and kept fixed here. It restores a line under a frame as the
//! log format says, which this build does not change. Like the 0.19.0
//! reader beside it, it parses every committed line of every index.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lakelands_api::Location;
use lakelands_api::engine::OpenMode;
use lakelands_api::event::LOG_FORMAT_VERSION;
use lakelands_api::ontology::StorageKind::Text;
use lakelands_api::seam::Seams;
use lakelands_core::conformance::fixture::{Lake, log_events, source_record, sourced, type_decl};
use lakelands_engine_sqlite::SqliteOpener;
use serde_json::{Value, json};

fn seams() -> Seams {
    Seams {
        engines: vec![Arc::new(SqliteOpener)],
        ..Seams::default()
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("read-by-0-20-0-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// An event line as 0.20.0 reads one.
#[derive(Clone, Debug)]
struct Old {
    position: u64,
    offset: u64,
    len: u64,
    id: String,
    hash: String,
}

/// 0.20.0's `parse_line`: `["c"]` is a commit line, `["e", position,
/// fence, offset, len, id, prev, hash]` an event line, with exactly these
/// seven members of these kinds, `id` and `prev` any string; anything
/// else is a damaged index.
fn parse_line(line: &[u8]) -> Result<Option<Old>, String> {
    let shown = || String::from_utf8_lossy(line).into_owned();
    let parsed: Value =
        serde_json::from_slice(line).map_err(|_| format!("no JSON: {}", shown()))?;
    match parsed.as_array().map(Vec::as_slice) {
        Some([kind]) if kind == "c" => Ok(None),
        Some([kind, rest @ ..]) if kind == "e" => {
            let members: (u64, u64, u64, u64, String, String, String) =
                serde_json::from_value(Value::Array(rest.to_vec()))
                    .map_err(|_| format!("members that do not fit: {}", shown()))?;
            let (position, _, offset, len, id, _, hash) = members;
            Ok(Some(Old {
                position,
                offset,
                len,
                id,
                hash,
            }))
        }
        _ => Err(format!("a line it does not know: {}", shown())),
    }
}

/// Each event of a segment file restored, in order.
fn restored(segment: &[u8]) -> Vec<Vec<u8>> {
    let mut lines = lakelands_core::log::Lines::default();
    let read = segment.split(|b| *b == b'\n').filter(|l| !l.is_empty());
    read.filter_map(|l| lines.read(l).unwrap()).collect()
}

/// 0.20.0's `whole`: each line of the group is in the segment file, a
/// line feed after it, and the event it restores to hashes to the hash
/// its index line names.
fn whole(segment: &[u8], first: u64, group: &[Old]) -> bool {
    let events = restored(segment);
    group.iter().all(|e| {
        let to = (e.offset + e.len) as usize;
        if segment.get(to) != Some(&b'\n') {
            return false;
        }
        let Some(Ok(Value::Object(mut event))) = events
            .get((e.position - first) as usize)
            .map(|b| serde_json::from_slice::<Value>(b))
        else {
            return false;
        };
        event.remove("hash");
        lakelands_core::canonical::sha256_hex(&Value::Object(event)) == e.hash
    })
}

/// What 0.20.0's `ends` reads of the log in `dir` before it judges the
/// format: the first committed event and the last, restored, and every
/// index line it parsed. Every way it would call the log damaged first is
/// an error here.
fn older(dir: &Path) -> Result<(Vec<u8>, Vec<u8>, Vec<Old>), String> {
    let mut firsts: Vec<u64> = std::fs::read_dir(dir.join("log"))
        .unwrap()
        .filter_map(|e| {
            let name = e.unwrap().file_name().into_string().unwrap();
            let stem = name.strip_suffix(".jsonl")?.to_owned();
            (stem.len() == 20).then(|| stem.parse().unwrap())
        })
        .collect();
    firsts.sort_unstable();
    let (mut ends, mut parsed) = (Vec::new(), Vec::new());
    for (i, first) in firsts.iter().enumerate() {
        let segment = std::fs::read(dir.join("log").join(format!("{first:020}.jsonl"))).unwrap();
        let index = std::fs::read(dir.join("index").join(format!("{first:020}.idx"))).unwrap();
        let (mut groups, mut group) = (Vec::new(), Vec::new());
        for line in index.split_inclusive(|b| *b == b'\n') {
            let Some(line) = line.strip_suffix(b"\n") else {
                break;
            };
            match parse_line(line).map_err(|e| format!("segment {first}: {e}"))? {
                Some(e) => group.push(e),
                None => groups.push(std::mem::take(&mut group)),
            }
        }
        if i + 1 == firsts.len() && groups.last().is_some_and(|g| !whole(&segment, *first, g)) {
            groups.pop();
        }
        let counted: Vec<&Old> = groups.iter().flatten().collect();
        for (n, e) in counted.iter().enumerate() {
            if e.position != first + n as u64 {
                return Err(format!("segment {first}: line {n} names {}", e.position));
            }
        }
        let events = restored(&segment);
        let at = |e: &Old| events[(e.position - first) as usize].clone();
        if let Some(e) = counted.last() {
            ends.push((counted.first().map(|f| at(f)), at(e)));
        }
        parsed.extend(counted.into_iter().cloned());
    }
    let first = ends.first().and_then(|(f, _)| f.clone());
    let tip = ends.last().map(|(_, t)| t.clone());
    let (first, tip) = first.zip(tip).ok_or("no committed event")?;
    Ok((first, tip, parsed))
}

/// The `v` an event says.
fn v(event: &[u8]) -> u64 {
    let event: Value = serde_json::from_slice(event).unwrap();
    event.get("v").and_then(Value::as_u64).unwrap_or(0)
}

/// A lake with appends of three records each around notes that take the
/// first segment past its bound, so that a seal begins the next.
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
        let text = if i % 3 == 0 {
            "n".repeat(300 << 10)
        } else {
            "a note".to_owned()
        };
        let records =
            (0..3).map(|j| source_record("note", &format!("n{i}-{j}"), json!({ "text": text })));
        l.ingest("load", records.collect()).await.unwrap();
    }
    store.close().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lake_this_build_made_is_read_by_0_20_0_up_to_its_format() {
    let dir = scratch("made");
    let mut location = Location::sqlite("lake", &dir);
    location.limits.seal_after_bytes = 1 << 20;
    written(&location, OpenMode::Create, 0).await;
    written(&location, OpenMode::Write, 6).await;
    let segments = std::fs::read_dir(dir.join("log")).unwrap().count();
    assert!(
        segments >= 2,
        "the lake was sealed: {segments} segment files"
    );
    let (first, tip, lines) = older(&dir).unwrap();
    let reads = u64::from(LOG_FORMAT_VERSION);
    assert_eq!(
        (v(&first), v(&tip)),
        (reads, reads),
        "0.20.0 judges both ends by format {reads}"
    );
    assert!(
        lines.iter().all(|l| l.id.is_empty()),
        "every line this build writes names `id` empty"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lake_of_20_upgraded_is_read_by_0_20_0_up_to_its_format() {
    let vectors =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/log-format/vectors/20");
    let dir = scratch("upgraded");
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
        assert_eq!(
            (v(&first), v(&tip)),
            (20, u64::from(LOG_FORMAT_VERSION)),
            "0.20.0 judges the upgraded lake's tip by 21 after {opens} writer's open(s) more"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The index lines of every segment in `dir` naming the id each event's
/// line holds, as 0.20.0 wrote them.
fn named_as_0_20_0(dir: &Path) {
    for file in std::fs::read_dir(dir.join("index")).unwrap() {
        let path = file.unwrap().path();
        let stem = path.file_stem().unwrap().to_str().unwrap();
        let segment = std::fs::read(dir.join("log").join(format!("{stem}.jsonl"))).unwrap();
        let index = std::fs::read_to_string(&path).unwrap();
        let mut named = String::new();
        for line in index.lines() {
            let mut parsed: Value = serde_json::from_str(line).unwrap();
            if let Some(Value::Array(e)) = Some(&mut parsed).filter(|p| p[0] == "e") {
                let (offset, len) = (e[3].as_u64().unwrap(), e[4].as_u64().unwrap());
                let held = &segment[offset as usize..(offset + len) as usize];
                let held: Value = serde_json::from_slice(held).unwrap();
                e[5] = held["id"].clone();
            }
            named += &serde_json::to_string(&parsed).unwrap();
            named.push('\n');
        }
        std::fs::write(&path, named).unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_segment_of_lines_of_20_and_of_21_reads_back_whole() {
    let dir = scratch("mixed");
    let mut location = Location::sqlite("lake", &dir);
    location.limits.seal_after_bytes = 1 << 20;
    written(&location, OpenMode::Create, 0).await;
    named_as_0_20_0(&dir);
    written(&location, OpenMode::Write, 6).await;
    let (_, _, lines) = older(&dir).unwrap();
    let (named, empty) = lines.iter().partition::<Vec<_>, _>(|l| !l.id.is_empty());
    assert!(
        !named.is_empty() && !empty.is_empty(),
        "the index holds lines of both: {} named, {} empty",
        named.len(),
        empty.len()
    );
    let store = lakelands_store::open(location.clone(), seams(), OpenMode::Read);
    let store = store.await.unwrap();
    let events = log_events(store.as_ref()).await.unwrap();
    assert_eq!(events.len(), lines.len(), "every event is read");
    for (event, line) in events.iter().zip(&lines) {
        assert_eq!(event.position.0, line.position);
        assert!(
            line.id.is_empty() || line.id == event.id.0,
            "an id a line names is the event's"
        );
    }
    store.verify(None, None).await.unwrap();
    store.close().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
