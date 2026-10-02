//! A lake's log as its segment files (`docs/log-format.md` §4.2), for any
//! lake, and the check that reads them back without Lakelands' engines
//! (`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`).
//!
//! [`write`] reads the log through the lake's change feed, the log as the
//! host reads it: every event, retired and restricted records' included,
//! unmasked. An event's canonical bytes are what it re-serialises to (a
//! reader refuses one that does not), and every `checkpoint` names where a
//! sealed segment ends, so the files come out the same for every engine:
//!
//! ```text
//! <dir>/log/<first position, 20 digits>.jsonl   one canonical line per event
//! <dir>/export.json                              a manifest, never trusted
//! ```
//!
//! A lake that was never checkpointed exports one file,
//! `00000000000000000000.jsonl`. Each event keeps the format version it was
//! written in, so a lake that re-keyed a type exports version-1 lines up
//! to its first re-key and version-2 lines from there. The export reads
//! through the head as it stood when it began, a page of the feed at a
//! time, writing each line as it comes; it writes only into an empty or
//! absent directory, and an export that fails leaves nothing behind.
//!
//! [`verify`] reads only `log/`, a line at a time: canonical lines,
//! consecutive positions, the hash chain from 64 zeros, every checkpoint
//! against the files before it, and versions that are known and never go
//! down.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write as _};
use std::path::{Path, PathBuf};

use lakelands_api::catalog::Verification;
use lakelands_api::event::{Digest, Event, EventBody, LOG_FORMAT_BASE, LOG_FORMAT_VERSION};
use lakelands_api::seam::FeedFilter;
use lakelands_api::{Error, LakeStore, Position, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::canonical;

/// How many events the export reads from the feed at a time.
const PAGE: u32 = 1_000;

/// One segment file of an export.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegmentFile {
    /// Its path under the export, `log/<first>.jsonl`.
    pub file: String,
    /// Its first position.
    pub first: Position,
    /// Its last position.
    pub last: Position,
    /// SHA-256 over its bytes.
    pub digest: Digest,
}

/// What an export wrote: the manifest, `export.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The lake.
    pub lake: lakelands_api::LakeId,
    /// The head the export read through.
    pub through: Position,
    /// The hash of the event at `through`.
    pub hash: Digest,
    /// The lowest and the highest format version among its events.
    pub versions: [u32; 2],
    /// The segment files, in order.
    pub segments: Vec<SegmentFile>,
}

fn io(what: &str, path: &Path, e: std::io::Error) -> Error {
    crate::unavailable(format!("cannot {what} {}: {e}", path.display()))
}

fn damaged(position: impl std::fmt::Display, why: impl std::fmt::Display) -> Error {
    Error::Unavailable {
        message: format!("the export is damaged at position {position}: {why}"),
    }
}

/// The name of the file a segment starting at `first` is kept in.
pub fn file_name(first: Position) -> String {
    format!("log/{:020}.jsonl", first.0)
}

/// Write `store`'s log as its segment files, and the manifest, into `dir`,
/// which must be absent or empty. Answers the manifest. On any failure
/// what the export wrote is removed again, so no partial export is left to
/// pass for a shorter log.
pub async fn write(store: &dyn LakeStore, dir: &Path) -> Result<Manifest> {
    let existed = match fs::read_dir(dir) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                return Err(Error::Refused {
                    field: None,
                    reason: lakelands_api::Refusal::Precondition {
                        message: format!(
                            "{} is not empty; an export writes only into an empty or new directory",
                            dir.display()
                        ),
                    },
                });
            }
            true
        }
        Err(_) => false,
    };
    let written = write_into(store, dir).await;
    if written.is_err() {
        let _ = fs::remove_dir_all(dir.join("log"));
        let _ = fs::remove_file(dir.join("export.json"));
        if !existed {
            let _ = fs::remove_dir(dir);
        }
    }
    written
}

/// The segment file being written: its lines go out as they come, and
/// its digest is taken over the same bytes.
struct Segment {
    file: String,
    path: PathBuf,
    first: Position,
    last: Position,
    out: BufWriter<File>,
    digest: Sha256,
}

impl Segment {
    fn start(dir: &Path, first: Position) -> Result<Segment> {
        let file = file_name(first);
        let path = dir.join(&file);
        let out = File::create(&path).map_err(|e| io("create", &path, e))?;
        Ok(Segment {
            file,
            path,
            first,
            last: first,
            out: BufWriter::new(out),
            digest: Sha256::new(),
        })
    }

    fn line(&mut self, event: &Event) -> Result<()> {
        let mut bytes = canonical::of(event);
        bytes.push(b'\n');
        self.digest.update(&bytes);
        self.last = event.position;
        self.out
            .write_all(&bytes)
            .map_err(|e| io("write", &self.path, e))
    }

    fn finish(self) -> Result<SegmentFile> {
        let file = self
            .out
            .into_inner()
            .map_err(|e| io("write", &self.path, e.into_error()))?;
        file.sync_all().map_err(|e| io("write", &self.path, e))?;
        Ok(SegmentFile {
            file: self.file,
            first: self.first,
            last: self.last,
            digest: Digest(canonical::hex(&self.digest.finalize())),
        })
    }
}

async fn write_into(store: &dyn LakeStore, dir: &Path) -> Result<Manifest> {
    let through = store.head().await?;
    let log = dir.join("log");
    fs::create_dir_all(&log).map_err(|e| io("create", &log, e))?;
    let feed = store.feed();
    let mut segments = Vec::new();
    let mut open: Option<Segment> = None;
    let mut next = Position::GENESIS;
    let mut versions: Option<[u32; 2]> = None;
    let mut hash = Digest::zero();
    'feed: while next <= through {
        let batch = feed.poll(next, &FeedFilter::default(), PAGE).await?;
        if batch.events.is_empty() {
            break;
        }
        for event in batch.events {
            if event.position > through {
                break 'feed;
            }
            if event.position != next {
                return Err(Error::Unavailable {
                    message: format!(
                        "the feed answered position {} where {next} was next; nothing is exported",
                        event.position
                    ),
                });
            }
            // A checkpoint starts the segment after the one it seals.
            if open.is_none() || matches!(event.body, EventBody::Checkpoint { .. }) {
                if let Some(done) = open.take() {
                    segments.push(done.finish()?);
                }
                open = Some(Segment::start(dir, event.position)?);
            }
            if let Some(segment) = open.as_mut() {
                segment.line(&event)?;
            }
            versions = Some(match versions {
                None => [event.v, event.v],
                Some([lo, hi]) => [lo.min(event.v), hi.max(event.v)],
            });
            hash = event.hash;
            next = next.next();
        }
    }
    let (Some(segment), Some(versions)) = (open, versions) else {
        return Err(Error::Unavailable {
            message: "the feed answered no event; nothing is exported".to_owned(),
        });
    };
    segments.push(segment.finish()?);
    if next != through.next() {
        return Err(Error::Unavailable {
            message: format!(
                "the feed ended at position {next}, before the head {through} it was read to; \
                 nothing is exported"
            ),
        });
    }
    let manifest = Manifest {
        lake: store.lake().clone(),
        through,
        hash,
        versions,
        segments,
    };
    let path = dir.join("export.json");
    let text = serde_json::to_string_pretty(&manifest).expect("a manifest serialises");
    fs::write(&path, text + "\n").map_err(|e| io("write", &path, e))?;
    Ok(manifest)
}

/// Where a verification stands: the next position, the hash before it,
/// the last format version seen (0 before the first line), the last
/// `known_at` (none before the first line), and the files sealed so far.
struct Chain {
    next: Position,
    prev: Digest,
    version: u32,
    known_at: Option<lakelands_api::Timestamp>,
    sealed: Vec<(Position, Position, String)>,
}

impl Chain {
    /// Check one line of the file `index` (from 0), which starts at `first`.
    fn line(&mut self, line: &[u8], index: usize, first: Position) -> Result<()> {
        let at = self.next;
        let value: Value = serde_json::from_slice(line)
            .map_err(|e| damaged(at, format!("a line is not JSON: {e}")))?;
        if canonical::to_vec(&value) != line {
            return Err(damaged(at, "a line is not canonical"));
        }
        let v = value.get("v").and_then(Value::as_u64).unwrap_or(0);
        if v < u64::from(LOG_FORMAT_BASE) || v > u64::from(LOG_FORMAT_VERSION) {
            return Err(damaged(
                at,
                format!(
                    "written in log format {v}, and the formats are {LOG_FORMAT_BASE} to \
                     {LOG_FORMAT_VERSION}"
                ),
            ));
        }
        let v = v as u32;
        if v < self.version {
            return Err(damaged(
                at,
                format!(
                    "written in format {v} after an event in format {}",
                    self.version
                ),
            ));
        }
        let event: Event = serde_json::from_value(value.clone())
            .map_err(|e| damaged(at, format!("not an event: {e}")))?;
        if canonical::of(&event) != line {
            return Err(damaged(at, "a member is not in its one written form"));
        }
        if event.position != at {
            return Err(damaged(
                at,
                format!("the line is position {}", event.position),
            ));
        }
        // `known_at` never goes back (log-format §1.3).
        if let Some(before) = self.known_at
            && event.known_at < before
        {
            return Err(damaged(
                at,
                format!(
                    "its known_at {} is before the {before} of the event before it",
                    event.known_at
                ),
            ));
        }
        self.known_at = Some(event.known_at);
        if event.prev != self.prev {
            return Err(damaged(
                at,
                "it does not name the hash of the event before it",
            ));
        }
        let mut unhashed = value;
        if let Some(members) = unhashed.as_object_mut() {
            members.remove("hash");
        }
        if canonical::sha256_hex(&unhashed) != event.hash.0 {
            return Err(damaged(at, "the hash does not match the event"));
        }
        let later_file_start = at == first && index > 0;
        match &event.body {
            EventBody::Checkpoint {
                through,
                hash,
                segments,
            } => {
                if !later_file_start {
                    return Err(damaged(
                        at,
                        "a checkpoint is not the first event of a later file",
                    ));
                }
                if through.next() != at || hash != &event.prev {
                    return Err(damaged(
                        at,
                        "the checkpoint does not name the position and hash before it",
                    ));
                }
                let listed: Vec<(Position, Position, String)> = segments
                    .iter()
                    .map(|s| (s.first, s.last, s.digest.0.clone()))
                    .collect();
                if listed != self.sealed {
                    return Err(damaged(
                        at,
                        "the checkpoint does not list the files before it",
                    ));
                }
            }
            _ if later_file_start => {
                return Err(damaged(at, "a later file does not start with a checkpoint"));
            }
            _ => {}
        }
        self.version = v;
        self.prev = event.hash;
        self.next = at.next();
        Ok(())
    }
}

/// The first position a segment file's name gives: 20 decimal digits and
/// `.jsonl`, nothing else.
fn first_of(path: &Path) -> Option<u64> {
    let stem = path.file_stem()?.to_str()?;
    if stem.len() == 20 && stem.bytes().all(|b| b.is_ascii_digit()) {
        stem.parse().ok()
    } else {
        None
    }
}

/// An export's segment files, in order.
pub fn segment_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let log = dir.join("log");
    let mut files: Vec<PathBuf> = fs::read_dir(&log)
        .map_err(|e| io("read", &log, e))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .collect();
    files.sort();
    Ok(files)
}

/// Verify an export from its segment files alone, a line at a time: the
/// chain from position 0 through the last line, and its hash.
pub fn verify(dir: &Path) -> Result<Verification> {
    let files = segment_files(dir)?;
    let mut chain = Chain {
        next: Position::GENESIS,
        prev: Digest::zero(),
        version: 0,
        known_at: None,
        sealed: Vec::new(),
    };
    for (index, path) in files.iter().enumerate() {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let first = chain.next;
        match first_of(path) {
            Some(n) if n == first.0 => {}
            Some(n) => {
                return Err(damaged(
                    first,
                    format!("the next file is {name}, starting at {n}"),
                ));
            }
            None => {
                return Err(damaged(
                    first,
                    format!("{name} is not named by a first position in 20 digits"),
                ));
            }
        }
        let mut reader = BufReader::new(File::open(path).map_err(|e| io("read", path, e))?);
        let mut digest = Sha256::new();
        let mut line = Vec::new();
        loop {
            line.clear();
            let n = reader
                .read_until(b'\n', &mut line)
                .map_err(|e| io("read", path, e))?;
            if n == 0 {
                break;
            }
            digest.update(&line);
            if line.pop() != Some(b'\n') {
                return Err(damaged(
                    chain.next,
                    format!("{name} does not end in a line feed"),
                ));
            }
            chain.line(&line, index, first)?;
        }
        if chain.next == first {
            return Err(damaged(first, format!("{name} holds no event")));
        }
        let last = Position(chain.next.0 - 1);
        chain
            .sealed
            .push((first, last, canonical::hex(&digest.finalize())));
    }
    if chain.next == Position::GENESIS {
        return Err(damaged(0, "there is no segment file under log/"));
    }
    Ok(Verification {
        from: Position::GENESIS,
        through: Position(chain.next.0 - 1),
        hash: chain.prev,
    })
}
