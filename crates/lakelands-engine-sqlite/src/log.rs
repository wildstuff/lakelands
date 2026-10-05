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
//! its last event's hash, and where its last read ended. The end of an
//! index file says the first four: the event line before the last commit
//! line names the last committed position, where its bytes lie and its
//! hash. So an open reads the last `BLOCK` bytes of each segment's index,
//! and a block further back only while it has met no commit line, which a
//! group a crash left uncommitted alone makes it do: what a lake holds
//! does not lengthen its open, and a handle's memory does not grow with
//! the lake. Afterwards a handle reads on from where its index is read,
//! in the last segment alone: one it has read while a later one existed
//! is sealed, and its index is not opened again to look for new lines.
//!
//! **A read** finds the line of the position it starts at by halving the
//! committed index by byte: every event line names its position, so the
//! first one after the middle says which half holds the line wanted. Within
//! `NEAR` bytes of it the read goes forward a line at a time, checks that
//! each event line is the position that follows, and reads the bytes the
//! line names. A read that starts where the segment's last one ended
//! begins at that line with no search, so a page of the store's sequential
//! reads costs one event line per event it answers and the commit lines
//! between. An index line that is short, unreadable or not the position
//! expected is a damaged index, never another event. It is found by the
//! read that meets it, and so by a read of the whole log; an open, which
//! reads the index's end alone, does not find it.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use lakelands_api::engine::CanonicalEvent;
use lakelands_api::error::Result;
use lakelands_api::event::{Digest, EventId, SegmentDigest};
use lakelands_api::id::Position;
use sha2::{Digest as _, Sha256};

use crate::{invalid, io_error, unavailable};

/// How much of an index file's end is read at once.
const BLOCK: u64 = 64 * 1024;

/// How much is read at once to find where one line begins.
const LINE: u64 = 512;

/// How near the line a read starts at the halving of the index ends.
const NEAR: u64 = 8 * 1024;

/// A commit line, from the line feed of the event line it follows.
const COMMITTED: &[u8] = b"\n[\"c\"]\n";

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
#[derive(Debug, Default)]
struct Segment {
    first: u64,
    /// How many events it has committed: positions `first..first + count`.
    count: u64,
    /// The byte where its committed events end in the segment file.
    end: u64,
    /// The last committed event's hash.
    last_hash: Option<String>,
    /// Bytes of the index file read through its last commit line.
    index_read: u64,
    /// Where the last read ended: the position that follows, and a byte of
    /// the index file at or before the one its line begins at.
    resume: (u64, u64),
    /// The digest, once the segment is sealed and it was computed.
    digest: Option<Digest>,
}

impl Segment {
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

/// The digest of a segment's committed bytes, hashed as they are read:
/// a segment is never held whole.
fn digest_of(dir: &Path, segment: &Segment) -> Result<Digest> {
    let path = segment_path(dir, segment.first);
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

/// One index line, an event line checked to name `expected`.
fn parse_at(path: &Path, line: &[u8], expected: u64) -> Result<Option<Entry>> {
    match parse_line(path, line)? {
        Some(e) if e.position != expected => Err(damaged(
            path,
            format!(
                "a line names position {} where position {expected} belongs",
                e.position
            ),
        )),
        entry => Ok(entry),
    }
}

/// Where `wanted` last begins in the first `end` bytes of `file`, read
/// `step` bytes at a time from the end and never whole: after a crash the
/// lines without a commit line can be most of the active segment's index.
/// What lies across two steps is passed by.
fn last_of(mut file: &File, mut end: u64, wanted: &[u8], step: u64) -> io::Result<Option<u64>> {
    loop {
        let at = end.saturating_sub(step);
        let mut block = Vec::new();
        file.seek(SeekFrom::Start(at))?;
        file.take(end - at).read_to_end(&mut block)?;
        if let Some(i) = block.windows(wanted.len()).rposition(|w| w == wanted) {
            return Ok(Some(at + i as u64));
        }
        if at == 0 {
            return Ok(None);
        }
        end = at;
    }
}

/// What the end of its index says of a segment this handle has not read:
/// the event line before a commit line names the last position committed
/// through it, where its bytes end and its hash. The commit line is the
/// last, or one before it when the last lies across two blocks: the lines
/// after it are read as a later append's are. An index that holds no
/// commit line says nothing.
fn read_end(path: &Path, mut file: &File, segment: &mut Segment) -> Result<()> {
    let reading = |e| io_error("reading", path, e);
    let len = file.metadata().map_err(reading)?.len();
    let Some(feed) = last_of(file, len, COMMITTED, BLOCK).map_err(reading)? else {
        return Ok(());
    };
    let begins = last_of(file, feed, b"\n", LINE).map_err(reading)?;
    let begins = begins.map_or(0, |at| at + 1);
    let mut line = vec![0; (feed + 1 - begins) as usize];
    file.seek(SeekFrom::Start(begins))
        .and_then(|_| file.read_exact(&mut line))
        .map_err(reading)?;
    let last = parse_line(path, &line)?
        .filter(|e| e.position >= segment.first)
        .ok_or_else(|| damaged(path, "its last commit line follows no event of its segment"))?;
    segment.count = last.position - segment.first + 1;
    segment.end = last.offset + last.len + 1;
    segment.last_hash = Some(last.hash);
    segment.index_read = feed + COMMITTED.len() as u64;
    Ok(())
}

/// An event line at or before that of position `start` in a segment's
/// committed index, as its position and the byte it begins at, the line
/// wanted no more than `NEAR` bytes after it.
fn line_near(path: &Path, mut file: &File, segment: &Segment, start: u64) -> Result<(u64, u64)> {
    // The line wanted begins at `found.1` or after it, and at `end` or
    // before it.
    let (mut found, mut end) = ((segment.first, 0), segment.index_read - 1);
    while end - found.1 > NEAR {
        let middle = found.1 + (end - found.1) / 2;
        file.seek(SeekFrom::Start(middle))
            .map_err(|e| io_error("reading", path, e))?;
        let mut lines = std::io::BufReader::new(file);
        let (mut at, mut line) = (middle, Vec::new());
        let landed = loop {
            let begins = at;
            line.clear();
            at += lines
                .read_until(b'\n', &mut line)
                .map_err(|e| io_error("reading", path, e))? as u64;
            if at == begins || begins > end {
                break None;
            }
            // The first line read is the rest of the one `middle` lies
            // in; a commit line names no position.
            if begins > middle
                && let Some(e) = parse_line(path, &line)?
            {
                break Some((e.position, begins));
            }
        };
        match landed {
            Some(line) if line.0 <= start => found = line,
            _ => end = middle,
        }
    }
    Ok(found)
}

/// One lake's log, as this handle has read it.
#[derive(Debug)]
pub(crate) struct Log {
    dir: PathBuf,
    segments: Vec<Segment>,
}

fn segment_path(dir: &Path, first: u64) -> PathBuf {
    dir.join("log").join(format!("{first:020}.jsonl"))
}

fn index_path(dir: &Path, first: u64) -> PathBuf {
    dir.join("index").join(format!("{first:020}.idx"))
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

    /// Create the files of a new, empty segment.
    fn start_segment(&self, first: u64) -> Result<()> {
        for path in [segment_path(&self.dir, first), index_path(&self.dir, first)] {
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
        // A segment read while a later one existed is sealed: its index
        // does not change, and it is not opened again.
        let sealed = self.segments.len().saturating_sub(1);
        for first in self.files()? {
            if !self.segments.iter().any(|s| s.first == first) {
                self.segments.push(Segment {
                    first,
                    resume: (first, 0),
                    ..Segment::default()
                });
            }
        }
        for segment in &mut self.segments[sealed..] {
            let path = index_path(&self.dir, segment.first);
            let mut file = match File::open(&path) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(io_error("reading", &path, e)),
            };
            if segment.index_read == 0 {
                read_end(&path, &file, segment)?;
            }
            file.seek(SeekFrom::Start(segment.index_read))
                .map_err(|e| io_error("reading", &path, e))?;
            // What follows is read a line at a time, never whole: after a
            // crash it can be most of the active segment.
            let mut tail = std::io::BufReader::new(file);
            let mut line = Vec::new();
            // What the lines since the last commit line add, kept until
            // their commit line is read: a group never committed is no
            // event.
            let mut pending = 0u64;
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
                read += line.len() as u64;
                let expected = segment.first + segment.count + pending;
                match parse_at(&path, &line, expected)? {
                    None => {
                        segment.count += pending;
                        if let Some(e) = last.take() {
                            segment.end = e.offset + e.len + 1;
                            segment.last_hash = Some(e.hash);
                        }
                        segment.index_read += read;
                        pending = 0;
                        read = 0;
                    }
                    Some(e) => {
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
            (segment_path(&self.dir, active.first), active.end),
            (index_path(&self.dir, active.first), active.index_read),
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

    /// Append `events`, whose lease the caller checked: each carrying
    /// `fence`, the lease's, or, with none, as an import does, each keeping
    /// its own fence, never below the one before it.
    pub(crate) fn append(&mut self, fence: Option<u64>, events: &[CanonicalEvent]) -> Result<()> {
        self.refresh()?;
        let first = self.tip().map_or(0, |(p, _)| p.0 + 1);
        let prev = self.tip().map_or_else(Digest::zero, |(_, h)| h);
        let floor = match (fence, first) {
            (None, 1..) => self.fence_at(first - 1)?,
            _ => 0,
        };
        lakelands_core::engines::append_fault(events, first, prev, fence, floor)?;
        if events.is_empty() {
            return Ok(());
        }
        let active = self.segments.last_mut().ok_or_else(|| {
            unavailable("the lake's log has no segment; it was not created by this engine")
        })?;
        let (segment_path, index_path) = (
            segment_path(&self.dir, active.first),
            index_path(&self.dir, active.first),
        );
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
        for event in events {
            lines.extend_from_slice(&event.bytes);
            lines.push(b'\n');
            let len = event.bytes.len() as u64;
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
        index.extend_from_slice(&COMMITTED[1..]);
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
        active.last_hash = events.last().map(|e| e.hash.0.clone());
        Ok(())
    }

    /// The fence of the event at `position`.
    fn fence_at(&mut self, position: u64) -> Result<u64> {
        self.read(Position(position), 1)?
            .first()
            .map(|e| e.fence)
            .ok_or_else(|| unavailable(format!("the log holds no event at position {position}")))
    }

    /// At most `limit` events from `from` on, as appended.
    pub(crate) fn read(&mut self, from: Position, limit: u32) -> Result<Vec<CanonicalEvent>> {
        self.refresh()?;
        let mut out = Vec::new();
        for segment in &mut self.segments {
            let Some(last) = segment.last() else {
                continue;
            };
            let start = from.0.max(segment.first);
            if start > last || out.len() >= limit as usize {
                continue;
            }
            let wanted =
                Self::entries_of(&self.dir, segment, start, limit as u64 - out.len() as u64)?;
            let path = segment_path(&self.dir, segment.first);
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
    /// `start` on: from where the segment's last read ended, when that is
    /// `start`, and else from a line near it, each line checked to be the
    /// position that follows the one before.
    fn entries_of(dir: &Path, segment: &mut Segment, start: u64, n: u64) -> Result<Vec<Entry>> {
        let path = index_path(dir, segment.first);
        let last = segment.last().unwrap_or(segment.first);
        let mut file = File::open(&path).map_err(|e| io_error("reading", &path, e))?;
        let (mut expected, mut at) = match segment.resume {
            resume if resume.0 == start => resume,
            _ => line_near(&path, &file, segment, start)?,
        };
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
            at += read as u64;
            let Some(e) = parse_at(&path, &line, expected)? else {
                continue;
            };
            expected += 1;
            if e.position >= start {
                out.push(e);
            }
        }
        segment.resume = (expected, at);
        Ok(out)
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
        let digest = digest_of(&self.dir, active)?;
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
        for segment in &mut self.segments[..sealed] {
            let Some(last) = segment.last() else {
                continue;
            };
            let digest = match &segment.digest {
                Some(d) => d.clone(),
                None => digest_of(&self.dir, segment)?,
            };
            segment.digest = Some(digest.clone());
            out.push(SegmentDigest {
                first: Position(segment.first),
                last: Position(last),
                digest,
            });
        }
        Ok(out)
    }
}
