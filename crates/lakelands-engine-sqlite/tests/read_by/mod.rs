//! What the readers of 0.19.0, 0.20.0 and 0.22.0 share, as those releases
//! have it in `crates/lakelands-engine-sqlite/src/log.rs`, kept fixed
//! here: the shape of an index line and the segments it reads them from;
//! and 0.22.0's check of the active segment's last group, [`whole`].

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lakelands_api::seam::Seams;
use lakelands_engine_sqlite::SqliteOpener;
use serde_json::{Map, Value};

pub fn seams() -> Seams {
    Seams {
        engines: vec![Arc::new(SqliteOpener)],
        ..Seams::default()
    }
}

pub fn scratch(release: &str, name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("read-by-{release}-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// An event line as an older release reads one.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct Old {
    pub position: u64,
    _fence: u64,
    pub offset: u64,
    pub len: u64,
    pub id: String,
    pub prev: String,
    pub hash: String,
}

/// `parse_line`: `["c"]` is a commit line, `["e", position, fence,
/// offset, len, id, prev, hash]` an event line, with exactly these seven
/// members of these kinds; anything else it refuses, a line that is no
/// JSON or whose members do not fit as `NotJson`, any other shape as
/// `UnknownLine`: either is a damaged index.
fn parse_line(line: &[u8]) -> Result<Option<Old>, String> {
    let shown = || String::from_utf8_lossy(line).into_owned();
    let parsed: Value =
        serde_json::from_slice(line).map_err(|_| format!("no JSON: {}", shown()))?;
    match parsed.as_array().map(Vec::as_slice) {
        Some([kind]) if kind == "c" => Ok(None),
        Some([kind, rest @ ..]) if kind == "e" => serde_json::from_value(rest.to_vec().into())
            .map(Some)
            .map_err(|_| format!("members that do not fit: {}", shown())),
        _ => Err(format!("a line it does not know: {}", shown())),
    }
}

/// The first position of every segment of the log in `dir`, in order.
pub fn segments(dir: &Path) -> Vec<u64> {
    let mut firsts: Vec<u64> = std::fs::read_dir(dir.join("log"))
        .unwrap()
        .filter_map(|e| {
            let name = e.unwrap().file_name().into_string().unwrap();
            let stem = name.strip_suffix(".jsonl")?.to_owned();
            (stem.len() == 20).then(|| stem.parse().unwrap())
        })
        .collect();
    firsts.sort_unstable();
    firsts
}

/// The segment at `first` in `dir`, and its index's committed groups.
pub fn groups(dir: &Path, first: u64) -> Result<(Vec<u8>, Vec<Vec<Old>>), String> {
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
    Ok((segment, groups))
}

/// 0.22.0's `whole`, which its `refresh` asks of the active segment's last
/// committed group before it counts (`segment.unproven`): each line of
/// `group` ends in a line feed within `segment`, and the event it restores
/// to hashes to the hash its index line names. A group that is not whole
/// is a crash's tail to 0.22.0: it does not count, and a lake whose only
/// group it is holds no event. `before` is the hash of the event before
/// the group's first, where its index line names its `prev` empty.
///
/// The restoring is 0.22.0's `restore_json` (`crates/lakelands-core/src/
/// log.rs`): the line read as JSON alone, in no format judged; from format
/// 20 on its frame's members added, those under `body` and `sealed` one
/// level deep, a frame of another format or a member held twice refused;
/// then `position` and `prev` added where the line holds none. The hash is
/// `hash_of`, the canonical SHA-256 of the event without `hash`.
pub fn whole(segment: &[u8], before: Option<&str>, group: &[Old]) -> bool {
    let frames = frames(segment);
    let mut prev = before.map(str::to_owned);
    group.iter().all(|e| {
        let derived = (!e.prev.is_empty()).then(|| e.prev.clone()).or(prev.take());
        prev = Some(e.hash.clone());
        let end = (e.offset + e.len) as usize;
        let line = segment.get(e.offset as usize..end);
        let line = line.filter(|_| segment.get(end) == Some(&b'\n'));
        let Some(Value::Object(mut event)) = line.and_then(|l| serde_json::from_slice(l).ok())
        else {
            return false;
        };
        let v = event.get("v").and_then(Value::as_u64).unwrap_or(0);
        if v >= 20 {
            if let Some((fv, members)) = frames.get(&e.offset)
                && (*fv != v || !framed_into(&mut event, members))
            {
                return false;
            }
            event.entry("position").or_insert(e.position.into());
            event
                .entry("prev")
                .or_insert(derived.unwrap_or_default().into());
        }
        event.remove("hash");
        lakelands_core::canonical::sha256_hex(&Value::Object(event)) == e.hash
    })
}

/// The frame each event line of `segment` is covered by, by the line's
/// offset: a frame line (one holding `events` or `frame`) covers as many
/// event lines after it as its `events` says, with its `v` and its `frame`.
fn frames(segment: &[u8]) -> HashMap<u64, (u64, Map<String, Value>)> {
    let (mut covered, mut frame, mut left, mut at) = (HashMap::new(), None, 0, 0);
    for line in segment.split_inclusive(|b| *b == b'\n') {
        let read: Option<Value> = serde_json::from_slice(line.trim_ascii_end()).ok();
        let read = read.unwrap_or_default();
        if read.get("events").is_some() || read.get("frame").is_some() {
            let v = read.get("v").and_then(Value::as_u64).unwrap_or(0);
            let members = read.get("frame").and_then(Value::as_object).cloned();
            frame = Some((v, members.unwrap_or_default()));
            left = read.get("events").and_then(Value::as_u64).unwrap_or(0);
        } else if left > 0 {
            covered.insert(at, frame.clone().unwrap_or_default());
            left -= 1;
        }
        at += line.len() as u64;
    }
    covered
}

/// A frame's members added to `event`, those under `body` and `sealed`
/// merged one level deep: false where `event` holds one of them already.
fn framed_into(event: &mut Map<String, Value>, members: &Map<String, Value>) -> bool {
    members.iter().all(|(name, value)| match value {
        Value::Object(framed) if ["body", "sealed"].contains(&name.as_str()) => {
            let held = event
                .entry(name.clone())
                .or_insert_with(|| Map::new().into());
            held.as_object_mut().is_some_and(|held| {
                framed
                    .iter()
                    .all(|(k, v)| held.insert(k.clone(), v.clone()).is_none())
            })
        }
        _ => event.insert(name.clone(), value.clone()).is_none(),
    })
}
