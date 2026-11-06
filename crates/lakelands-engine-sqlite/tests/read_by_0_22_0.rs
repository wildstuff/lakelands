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
//! `crates/lakelands-engine-sqlite/src/log.rs` (`parse_line`, `ends`, and
//! the check its `refresh` asks of the active segment's last group,
//! `whole`, in `read_by`), and kept fixed here: it does not move with this
//! build, so no 0.22 build is needed to run it. A last group 0.22.x does
//! not find whole is a crash's tail to it, so a lake whose only group that
//! is would be no lake to 0.22.x, not one refused by its format.

mod read_by;

use std::ops::RangeInclusive;
use std::path::Path;

use lakelands_api::Location;
use lakelands_api::engine::OpenMode;
use lakelands_api::event::LOG_FORMAT_VERSION;
use lakelands_api::ontology::StorageKind::Text;
use lakelands_core::conformance::fixture::{Lake, source_record, sourced, type_decl};
use read_by::seams;
use serde_json::{Value, json};

/// The log formats 0.22.x reads, the oldest to the one it writes.
const READS: RangeInclusive<u64> = 14..=22;

/// The first and last committed events of the log in `dir`, as JSON
/// where their bytes read as JSON: 0.22.x reads the bytes an index line
/// points at as one line, and a block's are none, so its last event is
/// unread there, as `one_format` leaves it. Every way 0.22.x would call
/// the log damaged first, or find no event in it, is an error here.
fn ends(dir: &Path) -> Result<(Value, Option<Value>), String> {
    let (firsts, mut counted) = (read_by::segments(dir), Vec::new());
    for (i, &first) in firsts.iter().enumerate() {
        let (segment, mut groups) = read_by::groups(dir, first)?;
        // `refresh`: the active segment's last group counts where it is whole.
        let before = groups.iter().rev().nth(1).and_then(|g| g.last());
        let before = before.map(|e| e.hash.clone());
        let last = groups.last().map(Vec::as_slice).unwrap_or_default();
        if i + 1 == firsts.len() && !read_by::whole(&segment, before.as_deref(), last) {
            groups.pop();
        }
        for (n, e) in groups.into_iter().flatten().enumerate() {
            if e.position != first + n as u64 {
                return Err(format!("segment {first}: line {n} names {}", e.position));
            }
            let (from, to) = (e.offset as usize, (e.offset + e.len) as usize);
            let line = segment.get(from..to).ok_or("a line past its segment")?;
            counted.push(line.to_vec());
        }
    }
    let none = "no committed event: no lake to 0.22.x";
    let first = counted.first().ok_or(none)?;
    let first = serde_json::from_slice::<Value>(first).map_err(|e| e.to_string())?;
    let tip = counted.last().and_then(|l| serde_json::from_slice(l).ok());
    Ok((first, tip))
}

/// The format `one_format` of 0.22.x refuses the log in `dir` by, if any:
/// `None` is a lake that release goes on to read.
fn refused_by(dir: &Path) -> Result<Option<u64>, String> {
    let (first, tip) = ends(dir)?;
    let v = |line: &Value| line.get("v").and_then(Value::as_u64).unwrap_or(0);
    let made = first.pointer("/body/kind") == Some(&json!("lake_created"));
    let created = (first.pointer("/body/format").filter(|_| made)).map(|f| f.as_u64().unwrap_or(0));
    let said = [Some(v(&first)), created, tip.as_ref().map(v)];
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

/// A lake past its first append and past a seal: blocks and a second
/// segment change nothing of what 0.22.x judges.
#[tokio::test(flavor = "multi_thread")]
async fn a_lake_sealed_in_blocks_is_refused_by_0_22_by_its_format() {
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
    let firsts = read_by::segments(&dir);
    assert!(firsts.len() > 1, "the lake was sealed");
    // The active segment's last group is a block, which 0.22.x does not
    // find whole: a crash's tail to it, not counted.
    let (segment, groups) = read_by::groups(&dir, firsts[firsts.len() - 1]).unwrap();
    let before = groups.iter().rev().nth(1).and_then(|g| g.last());
    let (last, before) = (&groups[groups.len() - 1], before.map(|e| e.hash.as_str()));
    assert!(
        !read_by::whole(&segment, before, last),
        "0.22.x does not count the last group"
    );
    assert_eq!(
        refused_by(&dir),
        Ok(Some(u64::from(LOG_FORMAT_VERSION))),
        "0.22.x refuses the lake by its format"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A lake whose appends of more than one event are blocks: 0.22.x reads
/// a block's bytes as one line, which they are not, so it takes the last
/// group for a crash's tail and the event before it for no line, and
/// still judges the first event's `v`. So too where the last block is
/// torn, its commit line written and its bytes not.
#[tokio::test(flavor = "multi_thread")]
async fn a_lake_of_blocks_torn_or_whole_is_refused_by_0_22_by_its_format() {
    let dir = read_by::scratch("0-22-0", "blocks");
    let store = lakelands_store::open(Location::sqlite("lake", &dir), seams(), OpenMode::Create);
    let store = store.await.unwrap();
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("note", vec![sourced("text", Text)]))
        .await
        .unwrap();
    for i in 0..2 {
        let records =
            (0..5).map(|j| source_record("note", &format!("n{i}-{j}"), json!({ "text": "t" })));
        l.ingest("load", records.collect()).await.unwrap();
    }
    store.close().await.unwrap();
    drop((l, store));
    let firsts = read_by::segments(&dir);
    let (segment, groups) = read_by::groups(&dir, firsts[firsts.len() - 1]).unwrap();
    let last = &groups[groups.len() - 1];
    let at = last[0].offset as usize;
    assert!(
        segment[at..].starts_with(b"{\"block\":")
            && last.iter().all(|e| e.offset == last[0].offset),
        "the last append is one block"
    );
    let path = dir
        .join("log")
        .join(format!("{:020}.jsonl", firsts[firsts.len() - 1]));
    for torn in [false, true] {
        if torn {
            let len = (at + (segment.len() - at) / 2) as u64;
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_len(len)
                .unwrap();
        }
        assert_eq!(
            refused_by(&dir),
            Ok(Some(u64::from(LOG_FORMAT_VERSION))),
            "0.22.x refuses the lake by its format, its last block torn: {torn}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
