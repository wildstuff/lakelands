//! A lake of format 20 is read by 0.19.0 far enough that it is refused by
//! its format, not called damaged (lakelands#1468,
//! `docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`).
//!
//! 0.19.0 judges a lake's format by the first and the last event of its
//! log, and finds them by reading each segment's index: the event line
//! before the last commit line, the first line of a later segment, and
//! the lines a read of position 0 and of the tip meets. An index line of
//! another shape stops it there and it calls the lake damaged. In the
//! active segment it checks the last group's lines against their hashes,
//! and reads a group that fails the check as a crash's tail, not counted.
//! The store writes the append that creates a lake, and the one that
//! upgrades it, whole, so that group passes the check while it is the
//! lake's only one of 20.
//!
//! `older` below is that reader, its rules copied as v0.19.0 has them in
//! `crates/lakelands-engine-sqlite/src/log.rs` (`parse_line`, `whole`,
//! `read_end`, `first_prev`, `follows`) and kept fixed here: it does not
//! move with this build. It is stricter than that release in one way,
//! which only makes it harder to pass: every committed line of every
//! index is parsed, as a read's halving may land on any of them.

mod read_by;

use std::path::{Path, PathBuf};

use lakelands_api::Location;
use lakelands_api::engine::OpenMode;
use lakelands_api::event::LOG_FORMAT_VERSION;
use lakelands_api::ontology::StorageKind::Text;
use lakelands_core::conformance::fixture::{Lake, source_record, sourced, type_decl};
use read_by::{Old, seams};
use serde_json::{Value, json};

/// 0.19.0's `whole`: each line of the group is in the segment file, a
/// line feed after it, and the line, read as an event, hashes to the hash
/// its index line names: SHA-256 over its canonical bytes less `hash`.
fn whole(segment: &[u8], group: &[Old]) -> bool {
    group.iter().all(|e| {
        let (from, to) = (e.offset as usize, (e.offset + e.len) as usize);
        let Some(line) = segment
            .get(from..to)
            .filter(|_| segment.get(to) == Some(&b'\n'))
        else {
            return false;
        };
        let Ok(Value::Object(mut event)) = serde_json::from_slice::<Value>(line) else {
            return false;
        };
        event.remove("hash");
        lakelands_core::canonical::sha256_hex(&Value::Object(event)) == e.hash
    })
}

/// What 0.19.0's open reads of the log in `dir` before it judges the
/// format: the first committed event's line and the last's, as their
/// bytes. Every way it would call the log damaged first is an error here.
fn older(dir: &Path) -> Result<(Vec<u8>, Vec<u8>), String> {
    let firsts = read_by::segments(dir);
    let (mut ends, mut last_hash) = (Vec::new(), None::<String>);
    for (i, first) in firsts.iter().enumerate() {
        let (segment, mut groups) = read_by::groups(dir, *first)?;
        let events: Vec<&Old> = groups.iter().flatten().collect();
        // Its first event line names its own position, and a later
        // segment's names the last hash of the one before (`follows`).
        if let Some(e) = events.first() {
            let follows = i == 0 || last_hash.as_ref() == Some(&e.prev);
            if e.position != *first || !follows {
                return Err(format!("segment {first} does not follow the one before"));
            }
        }
        // The active segment's last group counts only where it is whole.
        if i + 1 == firsts.len() && groups.last().is_some_and(|g| !whole(&segment, g)) {
            groups.pop();
        }
        let line = |e: &Old| segment[e.offset as usize..(e.offset + e.len) as usize].to_vec();
        let counted: Vec<&Old> = groups.iter().flatten().collect();
        for (n, e) in counted.iter().enumerate() {
            if e.position != first + n as u64 {
                return Err(format!("segment {first}: line {n} names {}", e.position));
            }
        }
        if let Some(e) = counted.last() {
            last_hash = Some(e.hash.clone());
            ends.push((counted.first().map(|f| line(f)), line(e)));
        }
    }
    let first = ends.first().and_then(|(f, _)| f.clone());
    let tip = ends.last().map(|(_, t)| t.clone());
    first
        .zip(tip)
        .ok_or_else(|| "no committed event".to_owned())
}

/// The `v` a line says, as 0.19.0's format check reads it.
fn v(line: &[u8]) -> u64 {
    let line: Value = serde_json::from_slice(line).unwrap();
    line.get("v").and_then(Value::as_u64).unwrap_or(0)
}

/// Whether a segment file holds a frame line.
fn framed(dir: &Path) -> bool {
    let files = std::fs::read_dir(dir.join("log")).unwrap();
    files
        .map(|f| std::fs::read(f.unwrap().path()).unwrap())
        .any(|bytes| {
            let lines = bytes.split(|b| *b == b'\n');
            lines
                .filter_map(|l| serde_json::from_slice::<Value>(l).ok())
                .any(|l| l.get("frame").is_some())
        })
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lake_this_build_made_is_read_by_0_19_0_up_to_its_format() {
    let dir = read_by::scratch("0-19-0", "made");
    let mut location = Location::sqlite("lake", &dir);
    location.limits.seal_after_bytes = 1 << 20;
    let store = lakelands_store::open(location.clone(), seams(), OpenMode::Create);
    let store = store.await.unwrap();
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("note", vec![sourced("text", Text)]))
        .await
        .unwrap();
    // Appends of three records, each framed, around notes that take the
    // first segment past its bound, so that a seal begins the next.
    for i in 0..12 {
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
    drop((l, store));
    let segments = std::fs::read_dir(dir.join("log")).unwrap().count();
    assert!(
        segments >= 2,
        "the lake was sealed: {segments} segment files"
    );
    assert!(framed(&dir), "the lake holds a frame line");

    let (first, tip) = older(&dir).unwrap();
    let reads = u64::from(LOG_FORMAT_VERSION);
    assert_eq!(
        (v(&first), v(&tip)),
        (reads, reads),
        "0.19.0 judges both ends by format {reads}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lake_of_19_upgraded_is_read_by_0_19_0_up_to_its_format() {
    let vectors =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/log-format/vectors/19");
    let dir = read_by::scratch("0-19-0", "upgraded");
    let location = Location::sqlite("vectors", &dir);
    lakelands_store::import(location.clone(), seams(), vectors.clone())
        .await
        .unwrap();
    // As written: every line of format 19 is in its segment file byte for
    // byte.
    for name in ["00000000000000000000.jsonl", "00000000000000000007.jsonl"] {
        let kept = std::fs::read(vectors.join("log").join(name)).unwrap();
        let held = std::fs::read(dir.join("log").join(name)).unwrap();
        assert!(
            held.starts_with(&kept),
            "{name} holds the lines of format 19 as written"
        );
    }
    // The import's own writer's open upgraded it, and a later one appends
    // its own `lease_taken`. The upgrade's append is whole, so 0.19.0
    // counts it even while it is the last and finds 20 from the import on.
    for opens in 0..2 {
        if opens > 0 {
            let store = lakelands_store::open(location.clone(), seams(), OpenMode::Write);
            store.await.unwrap().close().await.unwrap();
        }
        let (first, tip) = older(&dir).unwrap();
        assert_eq!(
            (v(&first), v(&tip)),
            (19, u64::from(LOG_FORMAT_VERSION)),
            "0.19.0 judges the upgraded lake's tip by 20 after {opens} writer's open(s) more"
        );
        assert!(!framed(&dir), "the upgrade's append is whole");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lake_just_made_is_read_by_0_19_0_up_to_its_format() {
    let dir = read_by::scratch("0-19-0", "just-made");
    let location = Location::sqlite("lake", &dir);
    let store = lakelands_store::open(location, seams(), OpenMode::Create);
    store.await.unwrap().close().await.unwrap();
    // Its creation is its only append, and it is whole.
    let (first, tip) = older(&dir).unwrap();
    let reads = u64::from(LOG_FORMAT_VERSION);
    assert_eq!(
        (v(&first), v(&tip)),
        (reads, reads),
        "0.19.0 judges both ends of a lake just made by format {reads}"
    );
    assert!(!framed(&dir), "the creation's append is whole");
    let _ = std::fs::remove_dir_all(&dir);
}
