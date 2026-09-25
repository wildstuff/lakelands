//! The log as segment files, and the index beside them.
//!
//! **Segments.** The log is kept as `docs/log-format.md` §4.2 names it:
//! `log/<first position in 20 digits>.jsonl`, each event's canonical bytes
//! followed by one line feed, nothing else. The last file is the active
//! segment and takes appends; every other file is sealed and never
//! changes. Sealing starts the next file, empty, named by the position
//! its first event will take. A lake directory can therefore be verified
//! by anyone holding the format, with nothing of this engine.
//!
//! **The index.** An append also hands the engine each event's position,
//! id, `prev`, `hash` and fence beside its bytes, and a read must give
//! them back unchanged without parsing the bytes. They are kept in
//! `index/<first>.idx`, one JSON line per event — its members and where
//! its bytes lie — and one commit line closing each append.
//!
//! **All or none.** An append writes its lines to the segment and syncs
//! them, then writes its index lines and the commit line and syncs those.
//! Only events before the last commit line exist: a crash before it
//! leaves lines past the committed end of the segment, which no read
//! returns and which the next writer cuts off when it takes the lease. So
//! an append is durable and whole when it returns, and after a crash it
//! is either all there or not at all.
//!
//! Every handle reads the files, so a reader opened beside the writer
//! sees each append once it is committed.
//!
//! **What a handle holds.** Not the index: per segment, its first position
//! and how many events it has committed (positions within a segment follow
//! each other), where its committed bytes end, how far its index is read,
//! its last event's hash, and every `MARK_EVERY`th event's place in the
//! index file. A read seeks to the mark at or before the position it wants,
//! reads index lines forward, checks that each event line is the position
//! it expects, and reads the bytes the line names. Before the first event
//! it answers it passes fewer than `MARK_EVERY` event lines, and the
//! commit lines between them; then it reads one event line per event it
//! answers, and the commit lines between. A page of the store's
//! sequential reads therefore costs what it did when the index was held,
//! plus one seek and that skip. So a handle's memory does
//! not grow with the lake beyond a mark per `MARK_EVERY` events, and a
//! handle opened read-only in another process refreshes and holds the
//! same. An index line that is short, unreadable or not the position
//! expected is a damaged index, never another event.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use lakelands_api::engine::{CanonicalEvent, Lease};
use lakelands_api::error::Result;
use lakelands_api::event::{Digest, EventId, SegmentDigest};
use lakelands_api::id::Position;
use sha2::{Digest as _, Sha256};

use crate::{invalid, io_error, unavailable};

/// Every how many events a segment keeps its place in the index file.
const MARK_EVERY: u64 = 256;

/// One event's index line, as read from the file.
#[derive(Clone, Debug)]
struct Entry {
    position: u64,
    fence: u64,
    offset: u64,
    len: u64,
    id: String,
    prev: String,
    hash: String,
}

/// One segment, as a handle keeps it: its span and where to find its
/// events, not the events.
#[derive(Debug)]
struct Segment {
    first: u64,
    /// How many events it has committed: positions `first..first + count`.
    count: u64,
    /// The byte where its committed events end in the segment file.
    end: u64,
    /// The last committed event's hash.
    last_hash: Option<String>,
    /// Where in the index file the line of every `MARK_EVERY`th committed
    /// event begins: `marks[k]` is that of position `first + k * MARK_EVERY`.
    marks: Vec<u64>,
    /// Bytes of the index file read through its last commit line.
    index_read: u64,
    /// The digest, once the segment is sealed and it was computed.
    digest: Option<Digest>,
}

impl Segment {
    fn new(first: u64) -> Segment {
        Segment {
            first,
            count: 0,
            end: 0,
            last_hash: None,
            marks: Vec::new(),
            index_read: 0,
            digest: None,
        }
    }

    /// The last committed position, if any.
    fn last(&self) -> Option<u64> {
        (self.count > 0).then(|| self.first + self.count - 1)
    }
}

/// An index line that does not say what it must: the index is damaged.
fn damaged(path: &Path, what: impl std::fmt::Display) -> lakelands_api::Error {
    unavailable(format!(
        "the log index {} is damaged: {what}",
        path.display()
    ))
}

/// One index line: an event (`Some`) or a commit line (`None`).
fn parse_line(path: &Path, line: &[u8]) -> Result<Option<Entry>> {
    let parsed: serde_json::Value = serde_json::from_slice(line).map_err(|e| damaged(path, e))?;
    match parsed.as_array().map(Vec::as_slice) {
        Some([kind]) if kind == "c" => Ok(None),
        Some([kind, rest @ ..]) if kind == "e" => {
            let (position, fence, offset, len, id, prev, hash): (
                u64,
                u64,
                u64,
                u64,
                String,
                String,
                String,
            ) = serde_json::from_value(serde_json::Value::Array(rest.to_vec()))
                .map_err(|e| damaged(path, e))?;
            Ok(Some(Entry {
                position,
                fence,
                offset,
                len,
                id,
                prev,
                hash,
            }))
        }
        _ => Err(damaged(path, "it holds a line it does not know")),
    }
}

/// One lake's log, as this handle has read it.
#[derive(Debug)]
pub(crate) struct Log {
    dir: PathBuf,
    segments: Vec<Segment>,
}

fn segment_name(first: u64) -> String {
    format!("{first:020}")
}

/// Sync a directory, so a file created in it survives a crash.
fn sync_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| io_error("syncing", dir, e))?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

impl Log {
    /// The log in `dir`. `create` makes the first, empty segment.
    pub(crate) fn open(dir: &Path, create: bool) -> Result<Log> {
        let mut log = Log {
            dir: dir.to_owned(),
            segments: Vec::new(),
        };
        if create {
            for sub in ["log", "index"] {
                let d = dir.join(sub);
                fs::create_dir_all(&d).map_err(|e| io_error("creating", &d, e))?;
            }
            if log.files()?.is_empty() {
                log.start_segment(0)?;
            }
        }
        log.refresh()?;
        Ok(log)
    }

    fn segment_path(&self, first: u64) -> PathBuf {
        self.dir
            .join("log")
            .join(format!("{}.jsonl", segment_name(first)))
    }

    fn index_path(&self, first: u64) -> PathBuf {
        self.dir
            .join("index")
            .join(format!("{}.idx", segment_name(first)))
    }

    /// Create the files of a new, empty segment.
    fn start_segment(&self, first: u64) -> Result<()> {
        for path in [self.segment_path(first), self.index_path(first)] {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|f| f.sync_all())
                .map_err(|e| io_error("creating", &path, e))?;
        }
        sync_dir(&self.dir.join("log"))?;
        sync_dir(&self.dir.join("index"))
    }

    /// The first positions of the segment files, in order.
    fn files(&self) -> Result<Vec<u64>> {
        let dir = self.dir.join("log");
        let mut firsts: Vec<u64> = fs::read_dir(&dir)
            .map_err(|e| io_error("reading", &dir, e))?
            .filter_map(|entry| {
                let name = entry.ok()?.file_name().into_string().ok()?;
                let stem = name.strip_suffix(".jsonl")?;
                (stem.len() == 20).then(|| stem.parse().ok())?
            })
            .collect();
        firsts.sort_unstable();
        Ok(firsts)
    }

    /// Read what other handles committed since this one last looked.
    pub(crate) fn refresh(&mut self) -> Result<()> {
        for first in self.files()? {
            if !self.segments.iter().any(|s| s.first == first) {
                self.segments.push(Segment::new(first));
            }
        }
        let paths: Vec<(usize, PathBuf)> = self
            .segments
            .iter()
            .enumerate()
            .map(|(i, s)| (i, self.index_path(s.first)))
            .collect();
        for (i, path) in paths {
            let segment = &mut self.segments[i];
            let mut file = match File::open(&path) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(io_error("reading", &path, e)),
            };
            file.seek(SeekFrom::Start(segment.index_read))
                .map_err(|e| io_error("reading", &path, e))?;
            // The tail not indexed yet is read a line at a time, never
            // whole: after a crash it can be most of the active segment.
            let mut tail = std::io::BufReader::new(file);
            let mut line = Vec::new();
            // What the lines since the last commit line add, kept until
            // their commit line is read: a group never committed is no
            // event.
            let mut pending = 0u64;
            let mut marks = Vec::new();
            let mut last: Option<Entry> = None;
            let mut read = 0u64;
            loop {
                line.clear();
                let n = tail
                    .read_until(b'\n', &mut line)
                    .map_err(|e| io_error("reading", &path, e))?;
                if n == 0 || line.last() != Some(&b'\n') {
                    break;
                }
                let at = segment.index_read + read;
                read += line.len() as u64;
                match parse_line(&path, &line)? {
                    None => {
                        segment.count += pending;
                        segment.marks.append(&mut marks);
                        if let Some(e) = last.take() {
                            segment.end = e.offset + e.len + 1;
                            segment.last_hash = Some(e.hash);
                        }
                        segment.index_read += read;
                        pending = 0;
                        read = 0;
                    }
                    Some(e) => {
                        let expected = segment.first + segment.count + pending;
                        if e.position != expected {
                            return Err(damaged(
                                &path,
                                format!(
                                    "a line names position {} where position {expected} belongs",
                                    e.position
                                ),
                            ));
                        }
                        if (expected - segment.first).is_multiple_of(MARK_EVERY) {
                            marks.push(at);
                        }
                        pending += 1;
                        last = Some(e);
                    }
                }
            }
        }
        Ok(())
    }

    /// Cut off what the last append left uncommitted in the active
    /// segment. Only the writer does this, holding the lease.
    pub(crate) fn recover(&mut self) -> Result<()> {
        self.refresh()?;
        let Some(active) = self.segments.last() else {
            return Ok(());
        };
        for (path, len) in [
            (self.segment_path(active.first), active.end),
            (self.index_path(active.first), active.index_read),
        ] {
            // A crash between creating a segment and its index leaves no
            // index; the writer creates it empty.
            let file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .map_err(|e| io_error("opening", &path, e))?;
            let now = file
                .metadata()
                .map_err(|e| io_error("reading", &path, e))?
                .len();
            if now > len {
                file.set_len(len)
                    .and_then(|()| file.sync_all())
                    .map_err(|e| io_error("truncating", &path, e))?;
            }
        }
        Ok(())
    }

    /// The last event's position and hash.
    pub(crate) fn tip(&self) -> Option<(Position, Digest)> {
        self.segments
            .iter()
            .rev()
            .find_map(|s| Some((Position(s.last()?), Digest(s.last_hash.clone()?))))
    }

    /// Append `events` under `lease`, whose currency the caller checked.
    pub(crate) fn append(&mut self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        self.refresh()?;
        let first = self.tip().map_or(0, |(p, _)| p.0 + 1);
        let mut prev = self.tip().map_or_else(Digest::zero, |(_, h)| h);
        for (expected, event) in (first..).zip(events) {
            // The event at position 0 is written before any lease exists
            // in the lake, under fence 0 (`docs/log-format.md` §4.1).
            let wanted = if event.position.0 == 0 {
                0
            } else {
                lease.fence
            };
            if event.fence != wanted {
                return Err(invalid(format!(
                    "the event at position {} carries fence {}, and it must carry fence {}; \
                     nothing was appended",
                    event.position, event.fence, wanted
                )));
            }
            if event.position.0 != expected {
                return Err(invalid(format!(
                    "the event at position {} does not continue the log, whose next position \
                     is {expected}; nothing was appended",
                    event.position
                )));
            }
            if event.prev != prev {
                return Err(invalid(format!(
                    "the event at position {} names `{}` as the hash before it, but that hash \
                     is `{}`; nothing was appended",
                    event.position, event.prev.0, prev.0
                )));
            }
            if event.bytes.contains(&b'\n') {
                return Err(invalid(format!(
                    "the event at position {} holds a line feed in its bytes, which canonical \
                     bytes never do; nothing was appended",
                    event.position
                )));
            }
            prev = event.hash.clone();
        }
        if events.is_empty() {
            return Ok(());
        }
        let active = self.segments.last_mut().ok_or_else(|| {
            unavailable("the lake's log has no segment; it was not created by this engine")
        })?;
        let first = active.first;
        let (segment_path, index_path) = (self.segment_path(first), self.index_path(first));
        let active = self.segments.last_mut().expect("checked above");
        // An active segment holding nothing is named by the position its
        // first event takes; one named otherwise is not this log's.
        if active.count == 0 && events[0].position.0 != active.first {
            return Err(invalid(format!(
                "the active segment is named for position {}, and the event to append is at \
                 position {}; nothing was appended",
                active.first, events[0].position
            )));
        }
        let mut offset = active.end;
        let mut lines = Vec::new();
        let mut index = Vec::new();
        let mut marks = Vec::new();
        for event in events {
            lines.extend_from_slice(&event.bytes);
            lines.push(b'\n');
            let len = event.bytes.len() as u64;
            if (event.position.0 - active.first).is_multiple_of(MARK_EVERY) {
                marks.push(active.index_read + index.len() as u64);
            }
            let line = serde_json::json!([
                "e",
                event.position.0,
                event.fence,
                offset,
                len,
                event.id.0,
                event.prev.0,
                event.hash.0
            ]);
            index.extend_from_slice(line.to_string().as_bytes());
            index.push(b'\n');
            offset += len + 1;
        }
        index.extend_from_slice(b"[\"c\"]\n");
        for (path, bytes) in [(&segment_path, &lines), (&index_path, &index)] {
            let mut file = OpenOptions::new()
                .append(true)
                .open(path)
                .map_err(|e| io_error("opening", path, e))?;
            file.write_all(bytes)
                .and_then(|()| file.sync_data())
                .map_err(|e| io_error("appending to", path, e))?;
        }
        active.index_read += index.len() as u64;
        active.count += events.len() as u64;
        active.end = offset;
        active.marks.extend(marks);
        active.last_hash = events.last().map(|e| e.hash.0.clone());
        Ok(())
    }

    /// At most `limit` events from `from` on, as appended.
    pub(crate) fn read(&mut self, from: Position, limit: u32) -> Result<Vec<CanonicalEvent>> {
        self.refresh()?;
        let mut out = Vec::new();
        for segment in &self.segments {
            let Some(last) = segment.last() else {
                continue;
            };
            let start = from.0.max(segment.first);
            if start > last || out.len() >= limit as usize {
                continue;
            }
            let wanted = self.entries_of(segment, start, limit as u64 - out.len() as u64)?;
            let path = self.segment_path(segment.first);
            let mut file = File::open(&path).map_err(|e| io_error("reading", &path, e))?;
            for e in wanted {
                let mut bytes = vec![0; e.len as usize];
                file.seek(SeekFrom::Start(e.offset))
                    .and_then(|_| file.read_exact(&mut bytes))
                    .map_err(|err| match err.kind() {
                        std::io::ErrorKind::UnexpectedEof => unavailable(format!(
                            "{} ends before the committed event at position {}; the log is \
                             damaged",
                            path.display(),
                            e.position
                        )),
                        _ => io_error("reading", &path, err),
                    })?;
                out.push(CanonicalEvent {
                    position: Position(e.position),
                    id: EventId(e.id),
                    prev: Digest(e.prev),
                    hash: Digest(e.hash),
                    fence: e.fence,
                    bytes,
                });
            }
            if out.len() >= limit as usize {
                break;
            }
        }
        Ok(out)
    }

    /// At most `n` of a segment's committed index lines from position
    /// `start` on: from the mark at or before it, each line checked to be
    /// the position that follows the one before.
    fn entries_of(&self, segment: &Segment, start: u64, n: u64) -> Result<Vec<Entry>> {
        let path = self.index_path(segment.first);
        let last = segment.last().unwrap_or(segment.first);
        let k = (start - segment.first) / MARK_EVERY;
        let (mut expected, at) = match segment.marks.get(k as usize) {
            Some(at) => (segment.first + k * MARK_EVERY, *at),
            None => return Err(damaged(&path, format!("no mark for position {start}"))),
        };
        let mut file = File::open(&path).map_err(|e| io_error("reading", &path, e))?;
        file.seek(SeekFrom::Start(at))
            .map_err(|e| io_error("reading", &path, e))?;
        let mut lines = std::io::BufReader::new(file);
        let mut line = Vec::new();
        let mut out = Vec::new();
        while expected <= last && (out.len() as u64) < n {
            line.clear();
            let read = lines
                .read_until(b'\n', &mut line)
                .map_err(|e| io_error("reading", &path, e))?;
            if read == 0 || line.last() != Some(&b'\n') {
                return Err(damaged(
                    &path,
                    format!("it ends before the committed position {expected}"),
                ));
            }
            let Some(e) = parse_line(&path, &line)? else {
                continue;
            };
            if e.position != expected {
                return Err(damaged(
                    &path,
                    format!(
                        "a line names position {} where position {expected} belongs",
                        e.position
                    ),
                ));
            }
            expected += 1;
            if e.position >= start {
                out.push(e);
            }
        }
        Ok(out)
    }

    /// The digest of a segment's committed bytes, hashed as they are read:
    /// a segment is never held whole.
    fn digest_of(&self, segment: &Segment) -> Result<Digest> {
        let path = self.segment_path(segment.first);
        let file = File::open(&path).map_err(|e| io_error("reading", &path, e))?;
        let mut hash = Sha256::new();
        let read = std::io::copy(&mut file.take(segment.end), &mut hash)
            .map_err(|e| io_error("reading", &path, e))?;
        if read != segment.end {
            return Err(unavailable(format!(
                "{} ends before its committed events do; the log is damaged",
                path.display()
            )));
        }
        Ok(Digest(lakelands_core::canonical::hex(&hash.finalize())))
    }

    /// Close the active segment, if it holds any event, and start the next.
    pub(crate) fn seal(&mut self) -> Result<Option<SegmentDigest>> {
        self.refresh()?;
        let Some(active) = self.segments.last() else {
            return Ok(None);
        };
        let Some(last) = active.last() else {
            return Ok(None);
        };
        let digest = self.digest_of(active)?;
        let sealed = SegmentDigest {
            first: Position(active.first),
            last: Position(last),
            digest: digest.clone(),
        };
        let next = last + 1;
        self.start_segment(next)?;
        self.segments.last_mut().expect("checked above").digest = Some(digest);
        self.refresh()?;
        Ok(Some(sealed))
    }

    /// Every sealed segment, oldest first.
    pub(crate) fn segments(&mut self) -> Result<Vec<SegmentDigest>> {
        self.refresh()?;
        let sealed = self.segments.len().saturating_sub(1);
        let mut out = Vec::with_capacity(sealed);
        for i in 0..sealed {
            let segment = &self.segments[i];
            let Some(last) = segment.last() else {
                continue;
            };
            let (first, last) = (Position(segment.first), Position(last));
            let digest = match &segment.digest {
                Some(d) => d.clone(),
                None => {
                    let d = self.digest_of(segment)?;
                    self.segments[i].digest = Some(d.clone());
                    d
                }
            };
            out.push(SegmentDigest {
                first,
                last,
                digest,
            });
        }
        Ok(out)
    }
}
