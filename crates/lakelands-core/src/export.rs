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
//! `00000000000000000000.jsonl`. Every line is in the release's one format
//! version. The export reads through the head as it stood when it began,
//! a page of the feed at a
//! time, writing each line as it comes; it writes only into an empty or
//! absent directory, and an export that fails leaves nothing behind.
//!
//! [`verify`] reads only `log/`, a line at a time: canonical lines,
//! consecutive positions, the hash chain from 64 zeros, every checkpoint
//! against the files before it, every line in the one format version
//! this release reads, and no member where a reader of the lake refuses
//! it (`crate::log::misplaced`), so an import refuses it before it writes.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write as _};
use std::path::{Path, PathBuf};

use lakelands_api::catalog::Verification;
use lakelands_api::error::{Case, Detail, Doing, Flaw, Part};
use lakelands_api::event::{Digest, Event, EventBody, LOG_FORMAT_VERSION};
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
    /// The one format version its events are written in.
    pub format: u32,
    /// The segment files, in order.
    pub segments: Vec<SegmentFile>,
}

/// The export's files failed `doing`; the path is the operator's, not
/// the answer's.
fn io(doing: Doing, _: &Path, e: std::io::Error) -> Error {
    crate::io_failed(doing, Part::Export, &e)
}

fn damaged<T>(position: impl Into<Position>, flaw: Flaw) -> Result<T> {
    Err(crate::damaged(Part::Export, Some(position.into()), flaw))
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
                let said = Box::new(Case::ExportNotEmpty.into());
                return Err(Error::Refused {
                    field: None,
                    reason: lakelands_api::Refusal::Precondition(said),
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
        let out = File::create(&path).map_err(|e| io(Doing::Create, &path, e))?;
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
            .map_err(|e| io(Doing::Write, &self.path, e))
    }

    fn finish(self) -> Result<SegmentFile> {
        let file = self
            .out
            .into_inner()
            .map_err(|e| io(Doing::Write, &self.path, e.into_error()))?;
        file.sync_all()
            .map_err(|e| io(Doing::Write, &self.path, e))?;
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
    fs::create_dir_all(&log).map_err(|e| io(Doing::Create, &log, e))?;
    let feed = store.feed();
    let mut segments = Vec::new();
    let mut open: Option<Segment> = None;
    let mut next = Position::GENESIS;
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
                let (expected, found) = (next, Some(event.position));
                let flaw = Flaw::Position { expected, found };
                return Err(crate::damaged(Part::Feed, Some(next), flaw));
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
            hash = event.hash;
            next = next.next();
        }
    }
    let Some(segment) = open else {
        let flaw = Flaw::NoEvent { position: next };
        return Err(crate::damaged(Part::Feed, None, flaw));
    };
    segments.push(segment.finish()?);
    if next != through.next() {
        let flaw = Flaw::EndsBefore { position: through };
        return Err(crate::damaged(Part::Feed, Some(next), flaw));
    }
    let manifest = Manifest {
        lake: store.lake().clone(),
        through,
        hash,
        format: LOG_FORMAT_VERSION,
        segments,
    };
    let path = dir.join("export.json");
    let text = serde_json::to_string_pretty(&manifest).expect("a manifest serialises");
    fs::write(&path, text + "\n").map_err(|e| io(Doing::Write, &path, e))?;
    Ok(manifest)
}

/// Where a verification stands: the next position, the hash before it,
/// the last `known_at` (none before the first line), and the files sealed
/// so far.
struct Chain {
    next: Position,
    prev: Digest,
    known_at: Option<lakelands_api::Timestamp>,
    formats: crate::log::FormatSteps,
    fence: u64,
    sealed: Vec<(Position, Position, String)>,
}

impl Chain {
    /// Check one line of the file `index` (from 0), which starts at `first`.
    fn line(&mut self, line: &[u8], index: usize, first: Position) -> Result<()> {
        let at = self.next;
        let value: Value = serde_json::from_slice(line).or_else(|e| {
            damaged(
                at,
                Flaw::NotJson {
                    detail: Detail::json(&e),
                },
            )
        })?;
        if canonical::to_vec(&value) != line {
            return damaged(at, Flaw::NotCanonical);
        }
        let v = value.get("v").and_then(Value::as_u64).unwrap_or(0);
        // An export of a format this release does not read is not read in
        // part; an older one it reads is read through its converter.
        if !crate::log::reads_format(v) {
            let reads = LOG_FORMAT_VERSION.into();
            return damaged(at, Flaw::FormatOther { v, reads });
        }
        let read = crate::log::converted(v, value.clone());
        let converted = canonical::to_vec(&read);
        // The parser's own words, which name the member it does not know:
        // an export is verified for its operator, from its own files.
        let event: Event = serde_json::from_value(read).or_else(|e| {
            damaged(
                at,
                Flaw::NotAnEvent {
                    detail: Detail::scrubbed(e),
                },
            )
        })?;
        if canonical::of(&event) != converted {
            return damaged(at, Flaw::NotOneForm);
        }
        // No number written as an integer beyond ±(2^53 − 1) (log-format
        // §1.2): a reader with doubles would hash another event.
        if let Some(n) = canonical::integer_too_large(&value) {
            return damaged(at, Flaw::IntegerTooLarge { n: n.clone() });
        }
        if event.position != at {
            let (expected, found) = (at, Some(event.position));
            return damaged(at, Flaw::Position { expected, found });
        }
        let flaw = crate::log::misplaced(&event);
        if let Some(flaw) = flaw.or_else(|| crate::log::created_in_another(&event)) {
            return damaged(at, flaw);
        }
        if let Some(flaw) = self.formats.next(&event) {
            return damaged(at, flaw);
        }
        // Fences never fall (log-format §4).
        if event.fence < self.fence {
            let (fence, floor) = (event.fence, self.fence);
            return damaged(at, Flaw::FenceBelow { fence, floor });
        }
        self.fence = event.fence;
        // `known_at` never goes back (log-format §1.3).
        if let Some(before) = self.known_at
            && event.known_at < before
        {
            let known_at = event.known_at;
            return damaged(at, Flaw::KnownAtBack { known_at, before });
        }
        self.known_at = Some(event.known_at);
        if event.prev != self.prev {
            return damaged(at, Flaw::PrevHash);
        }
        if crate::engines::hash_of(value) != event.hash.0 {
            return damaged(at, Flaw::HashMismatch);
        }
        let later_file_start = at == first && index > 0;
        match &event.body {
            EventBody::Checkpoint {
                through,
                hash,
                segments,
            } => {
                if !later_file_start {
                    return damaged(at, Flaw::CheckpointNotFirst);
                }
                if through.next() != at || hash != &event.prev {
                    return damaged(at, Flaw::CheckpointPrev);
                }
                let listed: Vec<(Position, Position, String)> = segments
                    .iter()
                    .map(|s| (s.first, s.last, s.digest.0.clone()))
                    .collect();
                if listed != self.sealed {
                    return damaged(at, Flaw::CheckpointFiles);
                }
            }
            _ if later_file_start => {
                return damaged(at, Flaw::LaterWithoutCheckpoint);
            }
            _ => {}
        }
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
        .map_err(|e| io(Doing::Read, &log, e))?
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
        known_at: None,
        formats: crate::log::FormatSteps::default(),
        fence: 0,
        sealed: Vec::new(),
    };
    for (index, path) in files.iter().enumerate() {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let first = chain.next;
        match first_of(path) {
            Some(n) if n == first.0 => {}
            Some(n) => {
                let name = name.to_string();
                return damaged(first, Flaw::NextFile { name, first: n });
            }
            None => {
                let name = name.to_string();
                return damaged(first, Flaw::FileName { name });
            }
        }
        let mut reader = BufReader::new(File::open(path).map_err(|e| io(Doing::Read, path, e))?);
        let mut digest = Sha256::new();
        let mut line = Vec::new();
        loop {
            line.clear();
            let n = reader
                .read_until(b'\n', &mut line)
                .map_err(|e| io(Doing::Read, path, e))?;
            if n == 0 {
                break;
            }
            digest.update(&line);
            if line.pop() != Some(b'\n') {
                let name = name.to_string();
                return damaged(chain.next, Flaw::NoLineFeed { name });
            }
            chain.line(&line, index, first)?;
        }
        if chain.next == first {
            let name = name.to_string();
            return damaged(first, Flaw::NoEventIn { name });
        }
        let last = Position(chain.next.0 - 1);
        chain
            .sealed
            .push((first, last, canonical::hex(&digest.finalize())));
    }
    if chain.next == Position::GENESIS {
        return damaged(Position::GENESIS, Flaw::NoSegmentFile);
    }
    // A writer's `lease_taken` that raised the format ends the log with no
    // `lake_upgraded` after it.
    if let Some(flaw) = chain.formats.unmarked() {
        return damaged(Position(chain.next.0 - 1), flaw);
    }
    Ok(Verification {
        from: Position::GENESIS,
        through: Position(chain.next.0 - 1),
        hash: chain.prev,
    })
}
