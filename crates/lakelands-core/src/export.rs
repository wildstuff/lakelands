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
//! A lake never checkpointed exports one file, `00000000000000000000.jsonl`.
//! Every line is in the release's one format version. The export reads
//! through the head as it stood when it began, a page of the feed at a
//! time, writing each line as it comes; it writes only into an empty or
//! absent directory, and a failed export leaves nothing behind.
//!
//! [`backup`] writes the same files and no manifest, and run again into
//! its directory writes again the run its last line ends, then what
//! follows; it writes into
//! no directory holding anything beside `log/` and `export.json`
//! (`docs/decisions/077-a-backup-is-an-export-that-grows.md`).
//!
//! Each file holds its events in a copy's stored form (§4.2): each append
//! of two or more events, as the feed tells its end, in the blocks the
//! store writes it in (§1.5), which come out the same for the same events,
//! and every event alone on a line holding its `position`, and `prev` only
//! on the file's first. A copy imported and exported again is the same
//! files.
//!
//! [`verify`] reads only `log/`, a line or a block at a time, each event
//! restored from its line or its block: canonical lines, consecutive positions, the hash chain from 64 zeros, every checkpoint
//! against the files before it, one format version this release reads, and
//! no member a reader of the lake refuses (`crate::log::misplaced`), so an
//! import refuses it before it writes.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead as _, BufReader, BufWriter, Write as _};
use std::path::{Path, PathBuf};

use lakelands_api::catalog::Verification;
use lakelands_api::engine::CanonicalEvent;
use lakelands_api::error::{Case, Detail, Doing, Flaw, Part};
use lakelands_api::event::{Digest, Event, EventBody, LOG_FORMAT_VERSION};
use lakelands_api::seam::{ChangeFeed, FeedFilter};
use lakelands_api::{Error, LakeId, LakeStore, Position, Refusal, Result};
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
    /// SHA-256 over its events' canonical bytes, each followed by a line
    /// feed: its digest (§4.2).
    pub digest: Digest,
}

/// What an export wrote: the manifest, `export.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The lake.
    pub lake: LakeId,
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

/// Write `store`'s log as segment files and the manifest into `dir`, which
/// must be absent or empty. On failure what was written is removed, so no
/// partial export passes for a shorter log.
pub async fn write(store: &dyn LakeStore, dir: &Path) -> Result<Manifest> {
    write_from(store, &*store.feed(), dir).await
}

async fn write_from(store: &dyn LakeStore, feed: &dyn ChangeFeed, dir: &Path) -> Result<Manifest> {
    let existed = match fs::read_dir(dir) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                let said = Box::new(Case::ExportNotEmpty.into());
                return Err(Error::refused(None, Refusal::Precondition(said)));
            }
            true
        }
        Err(_) => false,
    };
    let written = write_into(store, feed, dir).await;
    if written.is_err() {
        let _ = fs::remove_dir_all(dir.join("log"));
        let _ = fs::remove_file(dir.join("export.json"));
        if !existed {
            let _ = fs::remove_dir(dir);
        }
    }
    written
}

/// The segment file being written: its lines go out a run at a time
/// (`docs/log-format.md` §4.2), and its digest is taken over its events'
/// canonical bytes.
struct Segment {
    file: String,
    path: PathBuf,
    first: Position,
    last: Position,
    out: BufWriter<File>,
    digest: Sha256,
    /// The run not yet written: the events of one append, and the last
    /// position of that append.
    run: Vec<Value>,
    end: Option<Position>,
}

impl Segment {
    fn start(dir: &Path, first: Position) -> Result<Segment> {
        let file = file_name(first);
        let path = dir.join(&file);
        // Appended to, so a backup writes on after its last file's end.
        let out = OpenOptions::new().create(true).append(true).open(&path);
        let out = out.map_err(|e| io(Doing::Create, &path, e))?;
        Ok(Segment {
            file,
            path,
            first,
            last: first,
            out: BufWriter::new(out),
            digest: Sha256::new(),
            run: Vec::new(),
            end: None,
        })
    }

    /// Takes `event`, of the append that ends at `end`.
    fn line(&mut self, event: &Event, end: Position) -> Result<()> {
        let bytes = canonical::of(event);
        self.digest.update(&bytes);
        self.digest.update(b"\n");
        self.last = event.position;
        if self.end != Some(end) {
            self.flush()?;
        }
        self.end = Some(end);
        let value = serde_json::from_slice(&bytes).expect("an event's canonical bytes are JSON");
        self.run.push(value);
        Ok(())
    }

    /// Writes the run held: as the store's writer writes an append of two
    /// or more events, in blocks, and else each event on its line, its
    /// `prev` only on the segment's first.
    fn flush(&mut self) -> Result<()> {
        let first = self.first;
        let mut stored: Vec<CanonicalEvent> = self.run.iter().map(stored_of).collect();
        crate::log::frame_append(&mut stored);
        let units: Vec<Vec<u8>> = match stored.iter().any(|e| e.frame.is_some()) {
            true => stored
                .iter()
                .filter_map(|e| e.frame.as_ref().filter(|f| f.first == e.position))
                .map(|f| f.bytes.clone())
                .collect(),
            false => {
                let holds = |_: usize, e: &Value| (true, e["position"].as_u64() == Some(first.0));
                crate::log::stored_form(&self.run, holds).1
            }
        };
        self.run.clear();
        for unit in units {
            self.out
                .write_all(&unit)
                .and_then(|()| self.out.write_all(b"\n"))
                .map_err(|e| io(Doing::Write, &self.path, e))?;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<SegmentFile> {
        self.flush()?;
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

/// The event at the engine seam an event's canonical value is, as the
/// store hands it to the writer's form.
pub(crate) fn stored_of(event: &Value) -> CanonicalEvent {
    let text = |name: &str| event[name].as_str().unwrap_or_default().to_owned();
    CanonicalEvent {
        position: Position(event["position"].as_u64().unwrap_or_default()),
        id: lakelands_api::event::EventId(text("id")),
        prev: Digest(text("prev")),
        hash: Digest(text("hash")),
        fence: event["fence"].as_u64().unwrap_or_default(),
        bytes: canonical::to_vec(event),
        frame: None,
    }
}

async fn write_into(store: &dyn LakeStore, feed: &dyn ChangeFeed, dir: &Path) -> Result<Manifest> {
    let through = store.head().await?;
    let log = dir.join("log");
    fs::create_dir_all(&log).map_err(|e| io(Doing::Create, &log, e))?;
    let start = (None, Position::GENESIS, Digest::zero());
    let (segments, hash) = copy(feed, dir, start, through).await?;
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

/// Write the feed from `next`, whose event before has `hash`, through
/// `through` into `dir`, after the lines `open` holds: the files written
/// and the hash at `through`, `log/` synced after them.
async fn copy(
    feed: &dyn ChangeFeed,
    dir: &Path,
    (mut open, mut next, mut hash): (Option<Segment>, Position, Digest),
    through: Position,
) -> Result<(Vec<SegmentFile>, Digest)> {
    let mut segments = Vec::new();
    'feed: while next <= through {
        let batch = feed.poll(next, &FeedFilter::default(), PAGE).await?;
        if batch.events.is_empty() {
            break;
        }
        let ends: Vec<Position> = batch
            .events
            .iter()
            .map(|e| batch.append_of(e.position).unwrap_or(e.position))
            .collect();
        for (event, end) in batch.events.into_iter().zip(ends) {
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
                segment.line(&event, end)?;
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
    let log = dir.join("log");
    File::open(&log)
        .and_then(|d| d.sync_all())
        .map_err(|e| io(Doing::Write, &log, e))?;
    Ok((segments, hash))
}

/// What a backup run wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backup {
    /// The lake.
    pub lake: LakeId,
    /// The cut: the head the run read through.
    pub through: Position,
    /// The hash of the event at `through`.
    pub hash: Digest,
    /// The last position the directory held before the run; `None` for a
    /// new backup.
    pub since: Option<Position>,
    /// The events after `since`: the new ones, not those written again.
    pub copied: u64,
    /// Where the run began writing again the last append the directory
    /// held, when that reaches back before `since`.
    pub again: Option<Position>,
    /// The files it wrote into, the one it extended first.
    pub files: Vec<String>,
}

/// Back `store`'s log up into `dir`, reading it through `feed`, the
/// store's own or one in front of it: an empty or absent directory takes
/// the whole log, as [`write`] writes it; one holding a backup takes the
/// events after its last line, which must be the lake's own at its
/// position, through the head as it stood once that line was found. A
/// refused run changes nothing: what a stopped run left after the last
/// line is cut only once that line is the lake's.
pub async fn backup(store: &dyn LakeStore, feed: &dyn ChangeFeed, dir: &Path) -> Result<Backup> {
    let refused = |case: Case| Error::refused(None, Refusal::Precondition(Box::new(case.into())));
    let names: std::io::Result<Vec<_>> = match fs::read_dir(dir) {
        Ok(entries) => entries.map(|e| e.map(|e| e.file_name())).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    };
    let names = names.map_err(|e| io(Doing::Read, dir, e))?;
    if names.iter().any(|n| n != "log" && n != "export.json") {
        return Err(refused(Case::ExportNotEmpty));
    }
    let (mut files, mut torn) = (Vec::new(), Vec::new());
    if names.iter().any(|n| n == "log") {
        files = segment_files(dir)?;
    }
    let mut first = None;
    let (since, again, segments, hash, through) = loop {
        let Some(path) = files.pop() else {
            removed(&torn)?;
            let _ = fs::remove_dir(dir.join("log"));
            let _ = fs::remove_file(dir.join("export.json"));
            let m = write_from(store, feed, dir).await?;
            break (None, None, m.segments, m.hash, m.through);
        };
        let Some(tail) = tail(&path)? else {
            torn.push(path);
            continue;
        };
        let held = tail.position;
        let old = *first.get_or_insert(held);
        let at = |position| async move {
            let found = feed.poll(position, &FeedFilter::default(), 1).await?;
            Ok::<_, Error>(found.events.into_iter().find(|e| e.position == position))
        };
        let derived = |e: &Event| {
            let restored = crate::log::restored(tail.line.clone(), None, Some((held, &e.prev)));
            restored.is_ok_and(|(r, _)| canonical::to_vec(&r) == canonical::of(e))
        };
        if !at(held).await?.as_ref().is_some_and(derived) {
            return Err(refused(Case::BackupOtherLog { held }));
        }
        // The run that holds the last line is written again, so that it
        // is as long as an export's would be (§4.2).
        let Some(start) = at(tail.run).await? else {
            return Err(refused(Case::BackupOtherLog { held }));
        };
        removed(&torn)?;
        torn.clear();
        let cut = OpenOptions::new().write(true).open(&path);
        let cut = cut.and_then(|f| f.set_len(tail.cut).and_then(|()| f.sync_all()));
        cut.map_err(|e| io(Doing::Write, &path, e))?;
        // A frame an earlier release wrote may begin inside an append: its
        // run goes too, and the file's end is read again, so that the
        // append is written whole.
        if let Some(before) = tail.run.0.checked_sub(1).map(Position) {
            let two = feed.poll(before, &FeedFilter::default(), 2).await?;
            let end = |p: Position| two.append_of(p).unwrap_or(p);
            if end(before) == end(tail.run) {
                files.push(path);
                continue;
            }
        }
        let through = store.head().await?;
        let open = match tail.cut {
            0 => None,
            _ => Some(Segment::start(
                dir,
                Position(first_of(&path).unwrap_or(held.0)),
            )?),
        };
        let (segments, hash) = copy(feed, dir, (open, tail.run, start.prev), through).await?;
        let again = (tail.run.0 < old.0).then_some(tail.run);
        break (Some(old), again, segments, hash, through);
    };
    let _ = fs::remove_file(dir.join("export.json"));
    Ok(Backup {
        lake: store.lake().clone(),
        through,
        hash,
        since,
        copied: through.0 + 1 - since.map_or(0, |p| p.0 + 1),
        again,
        files: segments.into_iter().map(|s| s.file).collect(),
    })
}

/// Remove the files a stopped run left holding no whole line.
fn removed(paths: &[PathBuf]) -> Result<()> {
    let gone = |p: &PathBuf| fs::remove_file(p).map_err(|e| io(Doing::Write, p, e));
    paths.iter().try_for_each(gone)
}

/// The end of a backup's file: its last event as its line holds it, or
/// restored where a block holds it, the position it names, the first
/// position of the append that holds it, and the byte that append begins
/// at. What follows the last whole line or block is a stopped run's.
struct Tail {
    line: Value,
    position: Position,
    run: Position,
    cut: u64,
}

/// The end of the file at `path`, read forward a line or a block at a
/// time, its blocks' bytes inflated only for the last; `None` where it
/// holds no whole event.
fn tail(path: &Path) -> Result<Option<Tail>> {
    let failed = |e: std::io::Error| io(Doing::Read, path, e);
    let mut file = BufReader::new(File::open(path).map_err(failed)?);
    // The last whole unit and the byte it begins at, and the first
    // position and byte of the append that holds it.
    let (mut last, mut run, mut at) = (None, (Position(0), 0), 0u64);
    loop {
        let unit = match crate::log::read_unit(&mut file) {
            Ok(Some(unit)) => unit,
            Ok(None) | Err(crate::log::Fault::Short | crate::log::Fault::Flaw(_)) => break,
            Err(crate::log::Fault::Io(e)) => return Err(failed(e)),
        };
        let (first, goes_on) = match &unit {
            crate::log::Unit::Line(line) => {
                let line = serde_json::from_slice::<Value>(line).ok();
                let position = line.as_ref().and_then(|l| l.get("position")?.as_u64());
                (position.map(Position), false)
            }
            crate::log::Unit::Block(block, ..) => {
                let held = |l: &Option<(crate::log::Unit, u64)>| match l {
                    Some((crate::log::Unit::Block(b, ..), _)) => {
                        b.end == block.end && b.last().next() == block.first
                    }
                    _ => false,
                };
                (Some(block.first), held(&last))
            }
        };
        let Some(first) = first else {
            break;
        };
        if !goes_on {
            run = (first, at);
        }
        let len = match &unit {
            crate::log::Unit::Line(line) => line.len() as u64 + 1,
            crate::log::Unit::Block(block, _, line, _) => line.len() as u64 + block.stored + 2,
        };
        last = Some((unit, at));
        at += len;
    }
    let Some((unit, _)) = last else {
        return Ok(None);
    };
    let line = match unit {
        crate::log::Unit::Line(line) => serde_json::from_slice(&line).unwrap_or_default(),
        crate::log::Unit::Block(_, _, line, z) => {
            let mut bytes = line;
            bytes.push(b'\n');
            bytes.extend_from_slice(&z);
            bytes.push(b'\n');
            let mut restoring = crate::log::Restoring::new(&bytes[..], false);
            let mut event = Value::Null;
            while let Ok(Some(restored)) = restoring.next_event() {
                event = restored.event;
            }
            event
        }
    };
    let position = Position(line["position"].as_u64().unwrap_or(u64::MAX));
    Ok(Some(Tail {
        line,
        position,
        run: run.0,
        cut: run.1,
    }))
}

/// Where a verification stands: the next position, the hash before it,
/// the last `known_at` (none before the first line), and the files sealed
/// so far.
struct Chain {
    next: Position,
    prev: Digest,
    known_at: Option<lakelands_api::Timestamp>,
    fence: u64,
    sealed: Vec<(Position, Position, String)>,
}

impl Chain {
    /// Check one event of the file `index` (from 0), which starts at
    /// `first`, as its line or its block restores it, every line read
    /// canonical and in a format this release reads: an export of another
    /// is not read in part. It answers the event's canonical bytes.
    fn event(&mut self, value: Value, index: usize, first: Position) -> Result<Vec<u8>> {
        let at = self.next;
        let bytes = canonical::to_vec(&value);
        // The parser's own words, which name the member it does not know:
        // an export is verified for its operator, from its own files.
        let event: Event = serde_json::from_value(value.clone()).or_else(|e| {
            damaged(
                at,
                Flaw::NotAnEvent {
                    detail: Detail::scrubbed(e),
                },
            )
        })?;
        if canonical::of(&event) != bytes {
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
        let restored = canonical::to_vec(&value);
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
        Ok(restored)
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

/// The lake an export holds, as its first event names it, or the frame
/// before it where that holds it: its first line alone, and never the
/// manifest, which is not trusted. `None` when the first line cannot be
/// read; the import's own verification then says why.
pub fn lake_of(dir: &Path) -> Option<String> {
    let first = dir.join(file_name(Position::GENESIS));
    let file = File::open(first).ok()?;
    let mut lines = BufReader::new(file).lines();
    let line: serde_json::Value = serde_json::from_str(&lines.next()?.ok()?).ok()?;
    if let Some(framed) = line.get("frame") {
        let event: serde_json::Value = serde_json::from_str(&lines.next()?.ok()?).ok()?;
        return [&framed["lake"], &event["lake"]]
            .iter()
            .find_map(|l| l.as_str().map(str::to_owned));
    }
    line["lake"].as_str().map(str::to_owned)
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

/// The first and the last event of a segment file, restored from the line
/// or the block each stands on (log-format §1.2, §1.5): an append of two
/// events or more is stored as a block, whose events are not lines, and
/// an archive of another format may hold them as this release's writer
/// holds its own. The last is returned with the position it stands at.
/// `None` where the file does not restore whole or holds no event; the
/// flaw the verifier already holds is then damage.
fn segment_ends(path: &Path) -> Option<(serde_json::Value, serde_json::Value, Position)> {
    let file = File::open(path).ok()?;
    let mut units = BufReader::new(file);
    let mut first = None;
    let mut last = None;
    loop {
        match crate::log::read_unit(&mut units) {
            Ok(Some(crate::log::Unit::Line(line))) => {
                let event: serde_json::Value = serde_json::from_slice(&line).ok()?;
                // A frame's own line holds no event: the lines it covers
                // follow and each keeps its own `v` and `position`.
                if crate::log::is_frame(&event) {
                    continue;
                }
                let at = Position(event.get("position")?.as_u64()?);
                first.get_or_insert(event.clone());
                last = Some((event, at));
            }
            Ok(Some(crate::log::Unit::Block(block, _, line, stored))) => {
                let events = block_events(&line, &stored, block.events)?;
                let tip = events.last()?;
                let at = Position(tip.get("position")?.as_u64()?);
                first.get_or_insert(events.first()?.clone());
                last = Some((tip.clone(), at));
            }
            Ok(None) => break,
            Err(_) => return None,
        }
    }
    let (tip, at) = last?;
    Some((first?, tip, at))
}

/// The events a block holds, restored from the block's own bytes — its
/// first line and its stored bytes, read by a reader that starts at it,
/// with nothing before it to check against — so that a block is read by
/// what it says and not by the chain around it. `None` unless every event
/// it says it holds restores; the flaw the verifier already holds is then
/// damage.
fn block_events(line: &[u8], stored: &[u8], events: u64) -> Option<Vec<serde_json::Value>> {
    let mut bytes = line.to_vec();
    bytes.push(b'\n');
    bytes.extend_from_slice(stored);
    bytes.push(b'\n');
    let mut restoring = crate::log::Restoring::new(&bytes[..], false);
    let mut out = Vec::new();
    loop {
        match restoring.next_event() {
            Ok(Some(restored)) => out.push(restored.event),
            Ok(None) => break,
            Err(_) => return None,
        }
    }
    (out.len() as u64 == events).then_some(out)
}

/// An export's two ends as the judgement over a lake's ends reads them:
/// the first event of its first segment, the last event of its last, each
/// restored from its line or its block, and the position that last stands
/// at. `None` where a segment does not restore whole; the flaw the
/// verifier already holds is then damage.
fn ends_of(dir: &Path) -> Option<(serde_json::Value, serde_json::Value, Position)> {
    let files = segment_files(dir).ok()?;
    let (first, _, _) = segment_ends(files.first()?)?;
    let (_, tip, tip_at) = segment_ends(files.last()?)?;
    Some((first, tip, tip_at))
}

/// Verify an export from its segment files alone, a line at a time: the
/// chain from position 0 through the last line, and its hash.
pub fn verify(dir: &Path) -> Result<Verification> {
    let files = segment_files(dir)?;
    let mut chain = Chain {
        next: Position::GENESIS,
        prev: Digest::zero(),
        known_at: None,
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
        let reader = BufReader::new(File::open(path).map_err(|e| io(Doing::Read, path, e))?);
        let mut events = crate::log::Restoring::new(reader, true);
        let mut digest = Sha256::new();
        loop {
            let event = match events.next_event() {
                Ok(Some(event)) => event,
                Ok(None) => break,
                Err(crate::log::Fault::Io(e)) => return Err(io(Doing::Read, path, e)),
                Err(crate::log::Fault::Short) => {
                    let name = name.to_string();
                    return damaged(chain.next, Flaw::NoLineFeed { name });
                }
                Err(crate::log::Fault::Flaw(flaw)) => {
                    // An archive whose first event is written in a format
                    // this release does not read is judged as the open of
                    // that lake judges it, over the archive's two ends and
                    // by the same judgement (`crate::log::one_format`): a
                    // lake that is another release's is refused as
                    // `log_format`, naming the lake, its format and the
                    // one this release reads and writes, and two ends
                    // that contradict each other are damage, named where
                    // the open names it. A line of another format where
                    // the chain has already passed an event is damage,
                    // and says so.
                    if chain.next == Position::GENESIS
                        && matches!(flaw, Flaw::FormatOther { .. })
                        && let Some(lake) = lake_of(dir)
                        && let Some((first, tip, tip_at)) = ends_of(dir)
                        && let Err(judged) = crate::log::one_format(
                            &lake,
                            (Position::GENESIS, tip_at),
                            Some(&first),
                            Some(&tip),
                        )
                    {
                        return Err(judged);
                    }
                    return damaged(chain.next, flaw);
                }
            };
            let event = chain.event(event.event, index, first)?;
            digest.update(&event);
            digest.update(b"\n");
        }
        events.end().or_else(|flaw| damaged(chain.next, flaw))?;
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
    Ok(Verification {
        from: Position::GENESIS,
        through: Position(chain.next.0 - 1),
        hash: chain.prev,
    })
}

#[cfg(test)]
mod tests;
