//! A lake this build makes, its index lines naming `id` empty where a
//! line holds the event, is read by 0.20.0 far enough to be refused by
//! its format, not called damaged
//! (lakelands#1517, decision 086). `older` is 0.20.0's reader of the ends,
//! its rules copied from v0.20.0's `src/log.rs` (`parse_line`, `whole`,
//! `ends`) and kept fixed: an event line of eight members, `id` any
//! string, the last group counted where each line restores to its hash.

mod read_by;

use std::path::Path;

use lakelands_api::Location;
use lakelands_api::engine::OpenMode;
use lakelands_api::event::LOG_FORMAT_VERSION;
use lakelands_api::ontology::StorageKind::Text;
use lakelands_core::conformance::fixture::{Lake, log_events, source_record, sourced, type_decl};
use read_by::{Old, seams};
use serde_json::{Value, json};

/// 0.20.0's `ends` in `dir`: the first committed `v`, the last's where
/// its bytes are a line (a block's are none, which its `one_format`
/// leaves unread), and every line it counted; every way it would call the
/// log damaged is an error. Its `whole` is the rule `read_by::whole`
/// keeps for 0.22.0, which 0.20.0 has too.
fn older(dir: &Path) -> Result<(u64, Option<u64>, Vec<Old>), String> {
    let firsts = read_by::segments(dir);
    let (mut vs, mut counted) = (Vec::new(), Vec::new());
    for (i, first) in firsts.iter().enumerate() {
        let (segment, mut groups) = read_by::groups(dir, *first)?;
        let before = groups.iter().rev().nth(1).and_then(|g| g.last());
        let before = before.map(|e| e.hash.clone());
        let last = groups.last().map(Vec::as_slice).unwrap_or_default();
        if i + 1 == firsts.len() && !read_by::whole(&segment, before.as_deref(), last) {
            groups.pop();
        }
        for (n, e) in groups.iter().flatten().enumerate() {
            if e.position != first + n as u64 {
                return Err(format!("segment {first}: line {n} names {}", e.position));
            }
            let line = &segment[e.offset as usize..(e.offset + e.len) as usize];
            let v = serde_json::from_slice::<Value>(line).ok();
            vs.push(v.map(|v| v["v"].as_u64().unwrap_or(0)));
            counted.push(e.clone());
        }
    }
    let first = vs.first().copied().flatten().ok_or("no committed event")?;
    Ok((first, vs.last().copied().flatten(), counted))
}

/// Whether every event line of `lines` names its `id` empty, and its
/// `hash` empty exactly where a block holds its event and it is not the
/// block's last: a line after it points at the same bytes.
fn thinned_in_blocks(lines: &[Old]) -> bool {
    lines.iter().enumerate().all(|(i, l)| {
        let inner = lines.get(i + 1).is_some_and(|o| o.offset == l.offset);
        l.id.is_empty() && inner == l.hash.is_empty()
    })
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

/// Every index line in `dir` naming its event's id, as 0.20.0 wrote them,
/// but a block's event's, which 0.20.0 wrote none of.
fn named_as_0_20_0(dir: &Path) {
    for file in std::fs::read_dir(dir.join("index")).unwrap() {
        let path = file.unwrap().path();
        let stem = path.file_stem().unwrap().to_str().unwrap();
        let segment = std::fs::read(dir.join("log").join(format!("{stem}.jsonl"))).unwrap();
        let mut named = String::new();
        for line in std::fs::read_to_string(&path).unwrap().lines() {
            let mut e: Value = serde_json::from_str(line).unwrap();
            let (from, len) = (
                e[3].as_u64().unwrap_or(0) as usize,
                e[4].as_u64().unwrap_or(0),
            );
            let bytes = &segment[from..][..len as usize];
            if e[0] == "e" && e[5] == "" && !bytes.starts_with(b"{\"block\":") {
                let held: Value = serde_json::from_slice(bytes).unwrap();
                e[5] = held["id"].clone();
            }
            named += &format!("{e}\n");
        }
        std::fs::write(&path, named).unwrap();
    }
}

/// A lake this build made: judged by its first end's format, every `id`
/// named empty and a block's hashes but its last's; named as 0.20.0 wrote
/// them, the next appends mix lines of both, read whole.
#[tokio::test(flavor = "multi_thread")]
async fn a_lake_this_build_made_is_read_by_0_20_0_up_to_its_format() {
    let dir = read_by::scratch("0-20-0", "made");
    let mut location = Location::sqlite("lake", &dir);
    location.limits.seal_after_bytes = 1 << 20;
    written(&location, OpenMode::Create, 0).await;
    let reads = u64::from(LOG_FORMAT_VERSION);
    let (first, tip, lines) = older(&dir).unwrap();
    // The last counted event is a block's, its bytes no line: 0.20.0
    // judges the lake by its first.
    assert_eq!((first, tip), (reads, None), "the first end by {reads}");
    assert!(thinned_in_blocks(&lines), "lines thinned in blocks");
    named_as_0_20_0(&dir);
    written(&location, OpenMode::Write, 6).await;
    let (_, _, lines) = older(&dir).unwrap();
    assert!(std::fs::read_dir(dir.join("log")).unwrap().count() > 1);
    let named = lines.iter().filter(|l| !l.id.is_empty()).count();
    assert!(named > 0 && named < lines.len(), "{named} named");
    let store = lakelands_store::open(location.clone(), seams(), OpenMode::Read);
    let store = store.await.unwrap();
    let events = log_events(store.as_ref()).await.unwrap();
    // 0.20.0 counts every event but the last block's three, which it
    // takes for a crash's tail; this build reads them all.
    assert_eq!(events.len(), lines.len() + 3, "every event is read");
    for (event, line) in events.iter().zip(&lines) {
        assert_eq!(event.position.0, line.position);
        assert!(line.id.is_empty() || line.id == event.id.0, "{line:?}");
    }
    store.verify(None, None).await.unwrap();
    store.close().await.unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
