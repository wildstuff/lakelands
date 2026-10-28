//! What the readers of 0.19.0 and 0.20.0 share, as both releases have it
//! in `crates/lakelands-engine-sqlite/src/log.rs`, kept fixed here: the
//! shape of an index line and the segments it reads them from.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lakelands_api::seam::Seams;
use lakelands_engine_sqlite::SqliteOpener;
use serde_json::Value;

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
