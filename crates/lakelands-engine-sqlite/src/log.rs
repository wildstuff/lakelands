//! The log as segment files, and the index beside them.
//!
//! **Segments.** The log is kept as `docs/log-format.md` §4.2 names it:
//! `log/<first position in 20 digits>.jsonl`, its stored form (§1.2), each
//! line followed by one line feed, nothing else: each event's line as the
//! store handed it, and an append's frame line before the first event it
//! covers. The last file is the active segment and takes appends; every
//! other file is sealed and never changes. A lake directory can therefore
//! be verified by anyone holding the format, with nothing of this engine.
//! A segment's digest is over its events as the store restores them, not
//! over the file's bytes, which are those only where every line is its
//! whole event.
//!
//! **A segment after the first comes to exist with its first events in
//! it.** A seal writes the next segment's index whole, its event lines
//! and their commit line, and syncs it and its name; then the segment's
//! lines, under a name no handle and no verifier lists
//! (`<first>.jsonl.partial`), synced; and the rename to the segment's
//! name is the step. Whoever lists `log/` before it reads the log as it
//! was; whoever lists it after finds the segment with its events
//! committed, so no later segment is ever empty. A seal that fails before
//! the rename takes its two files away and answers its error. A crash
//! there leaves them, read by nobody: a seal at that position writes over
//! both, and the next writer removes them as it takes the lease. After
//! the rename the seal stands, and an error says so: where the sync of
//! the segment's name fails the outcome is not known, as below for an
//! append; where the process ends before that sync nothing syncs the name
//! again, and a crash of the machine may still take the segment with it.
//!
//! **The index.** An append also hands the engine each event's position,
//! id, `prev`, `hash` and fence beside its bytes, and its frame, and a
//! read must give them back unchanged. They are kept in
//! `index/<first>.idx`, one JSON line per event — its members and where
//! its bytes lie — and one commit line closing each append. An event
//! line names its `id` empty: the event's line holds it on every line, a
//! frame never, right after its top-level `hash`, whose value the index
//! line holds, and a read takes it from there
//! (`docs/decisions/086-an-engine-keeps-an-events-id-once-and-its-hashes-as-bytes.md`).
//! An event line names `prev` only at a segment's first event: every other
//! one names it empty, and it is the hash on the line before it. A reader
//! takes an event line that names `id` or `prev`, as an index of an
//! earlier release wrote every one, and a segment may hold lines of both. A frame has no line of its own: an
//! event's bytes begin where the line before it ends, and where they
//! begin later, the bytes between are the frame line that covers it, and
//! each event line after it in its append that names `prev` empty. An
//! event that follows one a frame covers and is covered by none names its
//! `prev`, so the engine counts a frame's events without reading its
//! bytes.
//!
//! **An index line has the shapes 0.19.0 and 0.20.0 read.** Both read
//! a lake's index far enough to find its first and last events, and then
//! judge their format; a line they do not know stops them first, and
//! they call the lake damaged
//! (`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`).
//! A frame found by its gap and an `id` and a `prev` named empty keep
//! every line in the eight members they read, so they reach the judgment
//! and name the format. 0.19.0 checks the active segment's last group
//! against its hashes as lines of its own format, and a line of 20 that
//! shares a frame fails that check, so it takes such a group for a
//! crash's tail. The store writes the append that makes a lake whole
//! (`docs/log-format.md` §1.2): where a lake's only events are that
//! append, still its last,
//! the group passes and it names the format. 0.20.0 judges the ends by
//! `v` and the last group by `hash`, not by `id`; an event whose `id` it
//! would take empty is of 21, which it refuses by its format before its
//! store reads one.
//!
//! **All or none.** An append writes its lines to the segment, then its
//! index lines and the commit line, and syncs the two files at once. Only
//! events before the last commit line exist, and a handle checks the
//! active segment's last group against its hashes as it first reads it;
//! what a crash left past it, or a group the check refuses, the next
//! writer cuts off. How a failed sync, a crash during the syncs and the
//! lease's rewrite of the last group go is
//! `docs/decisions/060-an-append-syncs-its-two-files-at-once.md`.
//!
//! **Nothing is written behind bytes that are no event.** A write can
//! fail after it wrote, as on a full disk, and the handle goes on under
//! its lease. So an append that fails reads what is committed and cuts
//! both files back to it before it answers its error, and every append
//! and every seal begins the same way, whoever left bytes there: an
//! offset the index names is the byte its event begins at, and a sealed
//! file holds its events' lines and nothing else. The cut takes the
//! index first, so the index never commits bytes the segment has lost,
//! and a file that ends before what is committed in it is damage: it is
//! refused, and nothing is written behind it.
//!
//! **A commit line that is whole is never cut,** but for a last group
//! the check refuses: another handle may have read its group, so where a
//! sync after it fails the append answers that its outcome is not known.
//!
//! **A segment without its index is damage.** The order above tells a
//! crash from a loss. A crash leaves an index shorter than its segment,
//! its last group ahead of the segment, or a lake's first segment with no
//! bytes and no index yet. No crash
//! leaves a segment with lines and no index file, or a later segment
//! with none: every handle refuses such a log, naming the segment,
//! before anything is read or cut, where it was once read as empty and
//! the next writer cut it
//! (`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`).
//! What nothing in the files tells apart reads as a crash's tail: an
//! index file that is there and was shortened by hand, a byte changed by
//! hand in the last group's lines, and a commit line before the last one
//! made no JSON, after which the writer cuts the last two groups.
//!
//! **The segments follow each other.** The first begins at position 0,
//! each later one at the position after the last of the one before it,
//! and its first event names that one's hash: a seal leaves them so. A
//! log that is otherwise lost a sealed segment or holds another log's,
//! and every handle refuses it as it reads the segments.
//!
//! Every handle reads the files, so a reader opened beside the writer
//! sees each append once it is committed.
//!
//! **What a handle holds.** Not the index: per segment, its first position
//! and how many events it has committed (positions within a segment follow
//! each other), where its committed bytes end, how far its index is read,
//! its last event's hash, the hash its first event names as the one
//! before it, and where its last read ended. The end of an index file
//! says the first four: the event line before the last commit line names
//! the last committed position, where its bytes lie and its hash. So an
//! open reads the last `BLOCK` bytes of each segment's index and the
//! first line of each after the first, and a block further back only
//! while it has met no commit line, which a group a crash left
//! uncommitted alone makes it do: what a lake holds does not lengthen
//! its open, and a handle's memory does not grow with the lake.
//! Afterwards a handle reads on from where its index is read, in the
//! last segment alone: one it has read to its end while a later one
//! existed is sealed, and its index is not opened again to look for new
//! lines.
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
//! reads an index's end, its first line and the active segment's last
//! group alone, finds there a line that names another position. A line
//! that is no JSON there, or past what is committed with no commit line
//! after it, is a crash's tear.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use std::sync::Arc;

use lakelands_api::engine::{CanonicalEvent, Frame};
use lakelands_api::error::Result;
use lakelands_api::event::{Digest, EventId, SegmentDigest};
use lakelands_api::id::Position;
use lakelands_core::log::{Block, Fault, Restoring, block_of, read_unit};
use sha2::{Digest as _, Sha256};

use lakelands_api::error::{Case, Detail, Doing, Error, Flaw, Left, Part, Situation};

use crate::{damaged as damaged_at, io_error};

/// How much of an index file's end is read at once.
const BLOCK: u64 = 64 * 1024;

/// How much is read at once to find where one line begins.
const LINE: u64 = 512;

/// How near the line a read starts at the halving of the index ends.
const NEAR: u64 = 8 * 1024;

/// A commit line, from the line feed of the event line it follows.
const COMMITTED: &[u8] = b"\n[\"c\"]\n";

/// What a segment's file is named instead of `jsonl` until its seal's
/// rename.
const PARTIAL: &str = "jsonl.partial";

/// One event's index line, as read from the file.
#[derive(Clone, Debug)]
struct Entry {
    position: u64,
    fence: u64,
    offset: u64,
    len: u64,
    id: String,
    /// The `prev` it names: a segment's first event line names it, and
    /// any other's is the hash of the event line before it.
    prev: Option<String>,
    hash: String,
}

/// One index line.
#[derive(Clone, Debug)]
enum Line {
    /// An event's.
    Event(Entry),
    /// A commit line, closing an append.
    Commit,
}

/// Where a read goes on from in a segment's index: the position the next
/// event line names, the byte its line begins at, and the hash of the
/// event before it.
#[derive(Clone, Debug, Default)]
struct Resume {
    expected: u64,
    at: u64,
    hash: Option<String>,
}

impl Resume {
    /// Reads on over `line`: an event line, its `prev` found, is answered.
    /// A short event line with no event line before it names no `prev`,
    /// and is damage.
    fn take(&mut self, path: &Path, line: Line) -> Result<Option<Entry>> {
        let Line::Event(mut e) = line else {
            return Ok(None);
        };
        let prev = e.prev.take().or_else(|| self.hash.clone());
        e.prev = Some(prev.ok_or_else(|| damaged(path, Flaw::UnknownLine))?);
        self.expected = e.position + 1;
        self.hash = Some(e.hash.clone());
        Ok(Some(e))
    }
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
    /// The hash its first event line names as the one before it, for a
    /// segment after the first.
    prev: Option<String>,
    /// Bytes of the index file read through its last commit line.
    index_read: u64,
    /// Where the last read ended.
    resume: Resume,
    /// The digest, once the segment is sealed and it was computed.
    digest: Option<Digest>,
    /// Whether the groups a refresh reads are checked before they count:
    /// the active segment's last, as a handle first reads it, until one
    /// passes (module doc).
    unproven: bool,
    /// Where its committed bytes and its index ended before its last
    /// group: what a writer's lease writes again.
    checked: (u64, u64),
    /// The bytes its committed events restore to, through the byte of the
    /// file it is measured to: each line's and each block's `raw`. The
    /// writer measures it as it recovers, and an append adds its own.
    raw: Option<(u64, u64)>,
}

impl Segment {
    /// The last committed position, if any.
    fn last(&self) -> Option<u64> {
        (self.count > 0).then(|| self.first + self.count - 1)
    }
}

/// An index line that does not say what it must: the index is damaged.
/// The path is the operator's, not the answer's.
fn damaged(_: &Path, flaw: Flaw) -> lakelands_api::Error {
    damaged_at(Part::LogIndex, None, flaw)
}

/// One index line: an event's, naming its `prev` or naming it empty, or
/// a commit line.
fn parse_line(path: &Path, line: &[u8]) -> Result<Line> {
    let not_json = |e| {
        damaged(
            path,
            Flaw::NotJson {
                detail: Detail::json(&e),
            },
        )
    };
    let parsed: serde_json::Value = serde_json::from_slice(line).map_err(not_json)?;
    match parsed.as_array().map(Vec::as_slice) {
        Some([kind]) if kind == "c" => Ok(Line::Commit),
        Some([kind, rest @ ..]) if kind == "e" => {
            let (position, fence, offset, len, id, prev, hash): (_, _, _, _, _, String, _) =
                serde_json::from_value(serde_json::Value::Array(rest.to_vec()))
                    .map_err(not_json)?;
            Ok(Line::Event(Entry {
                position,
                fence,
                offset,
                len,
                id,
                prev: (!prev.is_empty()).then_some(prev),
                hash,
            }))
        }
        _ => Err(damaged(path, Flaw::UnknownLine)),
    }
}

/// The digest of a segment's committed events as they restore from its
/// lines and blocks (`docs/log-format.md` §4.2), read a unit at a time: a
/// segment is never held whole. A unit that does not restore is damage at
/// the position it holds.
fn digest_of(dir: &Path, segment: &Segment) -> Result<Digest> {
    let path = segment_path(dir, segment.first);
    let reading = |e| io_error(Doing::Read, Part::Log, e);
    let file = File::open(&path).map_err(reading)?;
    let mut units = Restoring::new(std::io::BufReader::new(file.take(segment.end)), false);
    let mut hash = Sha256::new();
    let mut next = segment.first;
    loop {
        let flawed = |flaw| damaged_at(Part::Segment, Some(Position(next)), flaw);
        match units.next_event() {
            Ok(Some(event)) => {
                hash.update(&event.bytes);
                hash.update(b"\n");
                next += 1;
            }
            Ok(None) => break,
            Err(Fault::Io(e)) => return Err(reading(e)),
            Err(Fault::Short) => return Err(short(&path)),
            Err(Fault::Flaw(flaw)) => return Err(flawed(flaw)),
        }
    }
    if units.read() != segment.end {
        return Err(short(&path));
    }
    let flawed = |flaw| damaged_at(Part::Segment, Some(Position(next)), flaw);
    units.end().map_err(flawed)?;
    Ok(Digest(lakelands_core::canonical::hex(&hash.finalize())))
}

/// The bytes the units of a segment file from byte `from` to `end`
/// restore to: each line's and its line feed, each block's `raw`, its
/// stored bytes not inflated.
fn raw_of(path: &Path, from: u64, end: u64) -> Result<u64> {
    let reading = |e| io_error(Doing::Read, Part::Log, e);
    let mut file = File::open(path).map_err(reading)?;
    file.seek(SeekFrom::Start(from)).map_err(reading)?;
    let mut units = std::io::BufReader::new(file.take(end - from));
    let mut raw = 0;
    loop {
        match read_unit(&mut units) {
            Ok(Some(unit)) => raw += unit.raw(),
            Ok(None) => return Ok(raw),
            Err(Fault::Io(e)) => return Err(reading(e)),
            Err(Fault::Short) => return Err(short(path)),
            Err(Fault::Flaw(flaw)) => return Err(damaged_at(Part::Segment, None, flaw)),
        }
    }
}

/// A file of the log that ends before what is committed in it.
fn short(_: &Path) -> lakelands_api::Error {
    damaged_at(Part::Segment, None, Flaw::Truncated)
}

/// Whether the segment beginning at `first` holds each of `group`'s lines
/// and blocks whole: their bytes, the line feed after them, a block's
/// inflating to its lines, and the event each restores to hashing to the
/// hash its index line names. A crash can leave a commit line ahead of
/// the bytes it commits (module doc).
fn whole(dir: &Path, first: u64, group: Vec<Entry>) -> Result<bool> {
    let reading = |e| io_error(Doing::Read, Part::Log, e);
    let file = File::open(segment_path(dir, first)).map_err(reading)?;
    let held = file.metadata().map_err(reading)?.len();
    for e in &group {
        if e.offset.saturating_add(e.len) >= held
            || bytes_at(&file, e.offset + e.len, 1).map_err(reading)? != b"\n"
        {
            return Ok(false);
        }
    }
    let events = match events_at((&file, first), group, &mut None) {
        Err(e) if matches!(e.situation(), Some(Situation::Damaged { .. })) => return Ok(false),
        read => read?,
    };
    for event in events {
        let restored = lakelands_core::log::restore_json(&event);
        if !restored.is_ok_and(|(r, _)| lakelands_core::engines::hash_of(r) == event.hash.0) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// A block read and inflated: where its bytes begin in its segment, its
/// frame as the seam holds it, its lines, and where each event's line
/// begins among them, the one past the last too.
#[derive(Debug)]
pub(crate) struct Inflated {
    at: (u64, u64),
    frame: Arc<Frame>,
    raw: Vec<u8>,
    starts: Vec<usize>,
}

impl Inflated {
    /// The block whose bytes, `bytes`, begin at byte `offset` of the
    /// segment beginning at `segment`, inflated.
    fn of(at: (u64, u64), bytes: Vec<u8>) -> std::result::Result<Inflated, Flaw> {
        let feed = bytes.iter().position(|b| *b == b'\n').ok_or(Flaw::NotABlock)?;
        let head = serde_json::from_slice(&bytes[..feed]).map_err(|e| Flaw::NotJson {
            detail: Detail::json(&e),
        })?;
        let (block, _) = Block::of(&head)?;
        let raw = lakelands_core::log::inflate(&block, &bytes[feed + 1..])?;
        let mut starts = vec![0];
        starts.extend(raw.iter().enumerate().filter(|(_, b)| **b == b'\n').map(|(i, _)| i + 1));
        let framed = raw.starts_with(b"{\"events\":");
        if framed {
            starts.remove(0);
        }
        if starts.len() as u64 != block.events + 1 {
            return Err(Flaw::BlockBytes);
        }
        let events = u32::try_from(block.events).map_err(|_| Flaw::NotABlock)?;
        let frame = Arc::new(Frame {
            first: block.first,
            events,
            bytes,
        });
        Ok(Inflated {
            at,
            frame,
            raw,
            starts,
        })
    }

    /// The line of the event at `position`, if the block holds it.
    fn line(&self, position: u64) -> Option<Vec<u8>> {
        let i = usize::try_from(position.checked_sub(self.frame.first.0)?).ok()?;
        let (from, to) = (*self.starts.get(i)?, *self.starts.get(i + 1)?);
        Some(self.raw[from..to - 1].to_vec())
    }
}

/// The events `entries` of the segment beginning at `first` name, their
/// `prev` found, read from the segment file `file`: each line's bytes,
/// and each block's once, inflated, the last one kept in `held` for the
/// read after.
fn events_at(
    (file, first): (&File, u64),
    entries: Vec<Entry>,
    held: &mut Option<Inflated>,
) -> Result<Vec<CanonicalEvent>> {
    let mut out = Vec::with_capacity(entries.len());
    for e in entries {
        let position = Position(e.position);
        let flawed = |flaw| damaged_at(Part::Segment, Some(position), flaw);
        let read = |from, len| {
            bytes_at(file, from, len).map_err(|err| match err.kind() {
                io::ErrorKind::UnexpectedEof => flawed(Flaw::Truncated),
                _ => io_error(Doing::Read, Part::Log, err),
            })
        };
        let at = (first, e.offset);
        let (bytes, frame) = match held.as_ref().filter(|h| h.at == at) {
            Some(h) => (h.line(e.position), Some(h.frame.clone())),
            None => {
                let bytes = read(e.offset, e.len)?;
                match bytes.starts_with(b"{\"block\":") {
                    false => (Some(bytes), None),
                    true => {
                        let h = held.insert(Inflated::of(at, bytes).map_err(flawed)?);
                        (h.line(e.position), Some(h.frame.clone()))
                    }
                }
            }
        };
        let bytes = bytes.ok_or_else(|| flawed(Flaw::Disagrees))?;
        let id = match e.id.is_empty() {
            true => id_in(&bytes, &e.hash),
            false => e.id,
        };
        out.push(CanonicalEvent {
            position,
            id: EventId(id),
            prev: Digest(e.prev.unwrap_or_default()),
            hash: Digest(e.hash),
            fence: e.fence,
            bytes,
            frame,
        });
    }
    Ok(out)
}

/// The `id` an event's line holds, where its index line names it empty.
/// Canonical order puts it right after the top-level `hash`, whose value
/// the index line holds; a line where it is not found so, or holds an
/// escape, is parsed. A line that holds none answers it empty, which the
/// store's check of the event it restores refuses.
fn id_in(line: &[u8], hash: &str) -> String {
    let after = format!("\"hash\":\"{hash}\",\"id\":\"");
    let at = line
        .windows(after.len())
        .position(|w| w == after.as_bytes());
    let rest = at.map(|at| &line[at + after.len()..]);
    let id = rest.and_then(|r| r.iter().position(|b| *b == b'"').map(|end| &r[..end]));
    match id.filter(|id| !id.contains(&b'\\')) {
        Some(id) => String::from_utf8_lossy(id).into_owned(),
        None => serde_json::from_slice::<serde_json::Value>(line)
            .ok()
            .and_then(|v| v.get("id")?.as_str().map(str::to_owned))
            .unwrap_or_default(),
    }
}

/// The `len` bytes of `file` from `from` on.
fn bytes_at(mut file: &File, from: u64, len: u64) -> io::Result<Vec<u8>> {
    let mut bytes = vec![0; len as usize];
    file.seek(SeekFrom::Start(from))?;
    file.read_exact(&mut bytes).map(|()| bytes)
}

/// One index line, an event line checked to name `expected`.
fn parse_at(path: &Path, line: &[u8], expected: u64) -> Result<Line> {
    let line = parse_line(path, line)?;
    let found = match &line {
        Line::Event(e) => e.position,
        Line::Commit => return Ok(line),
    };
    if found != expected {
        let (expected, found) = (Position(expected), Some(Position(found)));
        return Err(damaged(path, Flaw::Position { expected, found }));
    }
    Ok(line)
}

/// Where `wanted` last begins in the first `end` bytes of `file`, read
/// `step` bytes at a time from the end and never whole: after a crash the
/// lines without a commit line can be most of the active segment's index.
/// The steps overlap by `wanted` less a byte, so what lies across two is
/// found.
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
        end = at + wanted.len() as u64 - 1;
    }
}

/// What the end of its index says of a segment this handle has not read:
/// the event line before a commit line names the last position committed
/// through it, where its bytes end and its hash. The commit line is the
/// last, or the one before it where the segment's last group is to be
/// checked: the lines after it are read as a later append's are. An index
/// that holds no such commit line says nothing.
fn read_end(path: &Path, mut file: &File, segment: &mut Segment) -> Result<()> {
    let reading = |e| io_error(Doing::Read, Part::Log, e);
    let mut len = file.metadata().map_err(reading)?.len();
    if segment.unproven {
        len = last_of(file, len, COMMITTED, BLOCK)
            .map_err(reading)?
            .unwrap_or(0);
    }
    let Some(feed) = last_of(file, len, COMMITTED, BLOCK).map_err(reading)? else {
        return Ok(());
    };
    let begins = last_of(file, feed, b"\n", LINE).map_err(reading)?;
    let begins = begins.map_or(0, |at| at + 1);
    let mut line = vec![0; (feed + 1 - begins) as usize];
    file.seek(SeekFrom::Start(begins))
        .and_then(|_| file.read_exact(&mut line))
        .map_err(reading)?;
    let last = match parse_line(path, &line)? {
        Line::Event(e) if e.position >= segment.first => e,
        _ => return Err(damaged(path, Flaw::CommitWithoutEvent)),
    };
    segment.count = last.position - segment.first + 1;
    segment.end = last.offset + last.len + 1;
    segment.last_hash = Some(last.hash);
    segment.index_read = feed + COMMITTED.len() as u64;
    segment.checked = (segment.end, segment.index_read);
    Ok(())
}

/// The hash the first event line of a segment's index names as the one
/// before it: the last of the segment before, which [`Log::follows`]
/// holds it to. An index with no whole first line names none.
fn first_prev(path: &Path, mut file: &File, first: u64) -> Result<Option<String>> {
    let reading = |e| io_error(Doing::Read, Part::Log, e);
    file.seek(SeekFrom::Start(0)).map_err(reading)?;
    let mut line = Vec::new();
    let mut lines = std::io::BufReader::new(file);
    lines.read_until(b'\n', &mut line).map_err(reading)?;
    if line.last() != Some(&b'\n') {
        return Ok(None);
    }
    match parse_at(path, &line, first)? {
        Line::Event(e) => Ok(e.prev),
        Line::Commit => Ok(None),
    }
}

/// Where a read of position `start` in a segment's committed index goes
/// on from: the segment's first line, or the line after a commit line
/// whose event line before it names a position before `start`, as the
/// position that follows, the byte the line begins at, and that event
/// line's hash, the line wanted no more than `NEAR` bytes and an append's
/// lines after it.
fn line_near(path: &Path, mut file: &File, segment: &Segment, start: u64) -> Result<Resume> {
    // The line wanted begins at `found.at` or after it, and at `end` or
    // before it.
    let (mut found, mut end) = (
        Resume {
            expected: segment.first,
            ..Resume::default()
        },
        segment.index_read - 1,
    );
    while end.saturating_sub(found.at) > NEAR {
        let middle = found.at + (end - found.at) / 2;
        file.seek(SeekFrom::Start(middle))
            .map_err(|e| io_error(Doing::Read, Part::Log, e))?;
        let mut lines = std::io::BufReader::new(file);
        let (mut at, mut line, mut last) = (middle, Vec::new(), None::<Entry>);
        let landed = loop {
            let begins = at;
            line.clear();
            at += lines
                .read_until(b'\n', &mut line)
                .map_err(|e| io_error(Doing::Read, Part::Log, e))? as u64;
            if at == begins || begins > end {
                break None;
            }
            // The first line read is the rest of the one `middle` lies in.
            if begins == middle {
                continue;
            }
            match parse_line(path, &line)? {
                Line::Commit => {
                    if let Some(e) = last.take() {
                        break Some(Resume {
                            expected: e.position + 1,
                            at,
                            hash: Some(e.hash),
                        });
                    }
                }
                Line::Event(e) => last = Some(e),
            }
        };
        match landed {
            Some(line) if line.expected <= start => found = line,
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
    /// How many segments a refresh that did not fail has read to their end
    /// while a later one was listed: they are sealed, their indexes do not
    /// change, and they are not opened again to look for new lines.
    sealed: usize,
    /// How a file of the log is synced: `sync_data`, a fault in a test.
    sync: fn(&Path, &File) -> io::Result<()>,
    /// Whether a sync failed on this handle and writing it again failed
    /// too: it writes nothing more (module doc).
    unsynced: bool,
    /// The block the last read inflated, which the next read most often
    /// goes on in.
    held: Option<Inflated>,
}

fn segment_path(dir: &Path, first: u64) -> PathBuf {
    dir.join("log").join(format!("{first:020}.jsonl"))
}

fn index_path(dir: &Path, first: u64) -> PathBuf {
    dir.join("index").join(format!("{first:020}.idx"))
}

/// The names in `dir`. An entry that cannot be read fails the listing: a
/// segment passed over would be read as absent, and its index removed.
fn names(dir: &Path) -> Result<Vec<String>> {
    let read = fs::read_dir(dir).and_then(Iterator::collect::<io::Result<Vec<_>>>);
    let entries = read.map_err(|e| io_error(Doing::Read, Part::Log, e))?;
    let names = entries.iter().map(|e| e.file_name().into_string().ok());
    Ok(names.flatten().collect())
}

/// Sync a directory, so a file created in it survives a crash.
pub(crate) fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Whether `dir` holds a segment file. A directory that holds a
/// projection and none is refused: the projection is made after the
/// first segment, so no crash leaves the two so, and what is there is a
/// lake that lost its log. The projection is looked for first and the
/// segments second, the reverse of the order a making writes them in:
/// beside a lake being made, whoever finds the projection finds its
/// segment. Asked before anything is read or made in the directory.
pub(crate) fn kept(dir: &Path) -> Result<bool> {
    let projection = dir.join("projection.db").is_file();
    let log = dir.join("log");
    let segments = log.is_dir() && names(&log)?.iter().any(|n| n.ends_with(".jsonl"));
    if projection && !segments {
        return Err(damaged_at(Part::Log, None, Flaw::NoSegmentFile));
    }
    Ok(segments)
}

/// The first and the last committed event of the log in `dir`, read
/// from its files alone: no file is made or changed, and a directory
/// with no log answers none.
pub(crate) fn ends(dir: &Path) -> Result<Option<(CanonicalEvent, CanonicalEvent)>> {
    if !kept(dir)? {
        return Ok(None);
    }
    let mut log = Log::open(dir, false)?;
    let Some((tip, _)) = log.tip() else {
        return Ok(None);
    };
    let mut at = |position| log.read(position, 1).map(|mut e| e.pop());
    Ok(at(Position(0))?.zip(at(tip)?))
}

/// The lines `events` add to a segment file from byte `offset` on, each
/// block's bytes once, before its first event's index line, and their
/// index lines, closed by one commit line. An event a line holds names
/// its `id` empty in the index, which the line holds (module doc); one a
/// block holds names it, and its offset and length are the block's. An
/// index line names `prev` where the event is its segment's first, at
/// `first`; any other's is the hash of the event line before it.
fn lines_of(events: &[CanonicalEvent], mut offset: u64, first: u64) -> (Vec<u8>, Vec<u8>) {
    let (mut lines, mut index, mut block) = (Vec::new(), Vec::new(), (offset, 0));
    for event in events {
        let (at, len, id) = match &event.frame {
            Some(frame) => {
                if frame.first == event.position {
                    lines.extend_from_slice(&frame.bytes);
                    lines.push(b'\n');
                    block = (offset, frame.bytes.len() as u64);
                    offset += block.1 + 1;
                }
                (block.0, block.1, &event.id.0[..])
            }
            None => {
                lines.extend_from_slice(&event.bytes);
                lines.push(b'\n');
                let len = event.bytes.len() as u64;
                offset += len + 1;
                (offset - len - 1, len, "")
            }
        };
        let (position, fence, hash) = (event.position.0, event.fence, &event.hash.0);
        let prev = if position == first { &event.prev.0[..] } else { "" };
        let line = serde_json::json!(["e", position, fence, at, len, id, prev, hash]);
        index.extend_from_slice(line.to_string().as_bytes());
        index.push(b'\n');
    }
    index.extend_from_slice(&COMMITTED[1..]);
    (lines, index)
}

impl Log {
    /// The log in `dir`. `make` makes its two directories and the first,
    /// empty segment: it is asked only where `dir` holds no segment file
    /// ([`kept`]), so nothing is made beside a log that is there.
    pub(crate) fn open(dir: &Path, make: bool) -> Result<Log> {
        let mut log = Log {
            dir: dir.to_owned(),
            segments: Vec::new(),
            sealed: 0,
            sync: |_, file| file.sync_data(),
            unsynced: false,
            held: None,
        };
        if make {
            for sub in ["log", "index"] {
                let d = dir.join(sub);
                fs::create_dir_all(&d).map_err(|e| io_error(Doing::Create, Part::Log, e))?;
            }
            log.start_segment(0)?;
        }
        log.refresh()?;
        Ok(log)
    }

    /// Create the files of a new lake's first segment, empty.
    fn start_segment(&self, first: u64) -> Result<()> {
        for path in [segment_path(&self.dir, first), index_path(&self.dir, first)] {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|f| f.sync_all())
                .map_err(|e| io_error(Doing::Create, Part::Log, e))?;
        }
        ["log", "index"].iter().try_for_each(|sub| {
            let dir = self.dir.join(sub);
            sync_dir(&dir).map_err(|e| io_error(Doing::Sync, Part::Log, e))
        })
    }

    /// The first positions of the segment files, in order.
    fn files(&self) -> Result<Vec<u64>> {
        let mut firsts: Vec<u64> = names(&self.dir.join("log"))?
            .iter()
            .filter_map(|name| {
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
                self.segments.push(Segment {
                    first,
                    resume: Resume {
                        expected: first,
                        ..Resume::default()
                    },
                    ..Segment::default()
                });
            }
        }
        let count = self.segments.len();
        for (at, segment) in (self.sealed..).zip(&mut self.segments[self.sealed..]) {
            let path = index_path(&self.dir, segment.first);
            let mut file = match File::open(&path) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Only a lake's first segment, still empty, has none
                    // (module doc): any other lost its index. A making
                    // writes the index before the segment's first line,
                    // so the segment's length is read first and the index
                    // looked for again after it: beside a lake being
                    // made, whoever finds lines finds their index.
                    let lines = segment_path(&self.dir, segment.first);
                    let held = fs::metadata(&lines)
                        .map_err(|e| io_error(Doing::Read, Part::Log, e))?
                        .len();
                    if segment.first == 0 && held == 0 {
                        continue;
                    }
                    match File::open(&path) {
                        Ok(f) => f,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            let name = lines.file_name().unwrap_or_default();
                            let name = name.to_string_lossy().into_owned();
                            return Err(damaged(&path, Flaw::IndexMissing { name }));
                        }
                        Err(e) => return Err(io_error(Doing::Read, Part::Log, e)),
                    }
                }
                Err(e) => return Err(io_error(Doing::Read, Part::Log, e)),
            };
            if segment.first > 0 && segment.prev.is_none() {
                segment.prev = first_prev(&path, &file, segment.first)?;
            }
            if segment.index_read == 0 {
                // The active segment's last group is read below, as a later
                // append's is, and checked (module doc).
                segment.unproven = at + 1 == count;
                read_end(&path, &file, segment)?;
            }
            file.seek(SeekFrom::Start(segment.index_read))
                .map_err(|e| io_error(Doing::Read, Part::Log, e))?;
            // What follows is read a line at a time, never whole: after a
            // crash it can be most of the active segment.
            let mut tail = std::io::BufReader::new(file);
            let mut line = Vec::new();
            // What the lines since the last commit line add, kept until
            // their commit line is read: a group never committed is no
            // event.
            let mut pending = 0u64;
            let mut last: Option<Entry> = None;
            // The group's lines, kept only while they are to be checked.
            let mut group = Vec::new();
            let mut walk = Resume {
                hash: segment.last_hash.clone(),
                ..Resume::default()
            };
            // A line that is no JSON past what is committed: damage where a
            // commit line follows it, a crash's tear where none does.
            let mut torn_at = None;
            let mut read = 0u64;
            loop {
                line.clear();
                let n = tail
                    .read_until(b'\n', &mut line)
                    .map_err(|e| io_error(Doing::Read, Part::Log, e))?;
                if n == 0 || line.last() != Some(&b'\n') {
                    break;
                }
                read += line.len() as u64;
                if torn_at.is_some() {
                    match line == COMMITTED[1..] {
                        true => return Err(torn_at.take().expect("checked above")),
                        false => continue,
                    }
                }
                let expected = segment.first + segment.count + pending;
                let parsed = parse_at(&path, &line, expected);
                // A line that is no JSON is one a crash tore: under the check
                // its group was never answered. A line that names another
                // position stays damage.
                let torn = |e: &lakelands_api::Error| {
                    matches!(
                        e.situation(),
                        Some(Situation::Damaged {
                            flaw: Flaw::NotJson { .. } | Flaw::UnknownLine,
                            ..
                        })
                    )
                };
                if parsed.as_ref().is_err_and(torn) {
                    if segment.unproven {
                        break;
                    }
                    torn_at = parsed.err();
                    continue;
                }
                match parsed? {
                    Line::Commit => {
                        if segment.unproven {
                            if !whole(&self.dir, segment.first, std::mem::take(&mut group))? {
                                break;
                            }
                            (segment.unproven, group) = (false, Vec::new());
                        }
                        segment.checked = (segment.end, segment.index_read);
                        segment.count += pending;
                        if let Some(e) = last.take() {
                            segment.end = e.offset + e.len + 1;
                            segment.last_hash = Some(e.hash);
                        }
                        segment.index_read += read;
                        pending = 0;
                        read = 0;
                    }
                    line => {
                        pending += u64::from(matches!(line, Line::Event(_)));
                        if segment.unproven
                            && let Some(e) = walk.take(&path, line.clone())?
                        {
                            group.push(e);
                        }
                        if let Line::Event(e) = line {
                            last = Some(e);
                        }
                    }
                }
            }
        }
        self.follows()?;
        self.sealed = self.segments.len().saturating_sub(1);
        Ok(())
    }

    /// Refuses a log whose segments do not follow each other: the first
    /// begins at position 0, each later one at the position after the
    /// last of the one before it, and its first event names that one's
    /// hash. A seal leaves them so, so a log that is otherwise lost a
    /// sealed segment or holds another log's. Asked of the segments a
    /// refresh read, from what a handle holds of each.
    fn follows(&self) -> Result<()> {
        let gap = |expected: u64, found: u64| {
            let (expected, found) = (Position(expected), Some(Position(found)));
            damaged_at(Part::Log, None, Flaw::Position { expected, found })
        };
        match self.segments.first() {
            Some(s) if s.first != 0 => return Err(gap(0, s.first)),
            _ => {}
        }
        for pair in self.segments[self.sealed..].windows(2) {
            let (before, next) = (&pair[0], &pair[1]);
            if before.first + before.count != next.first {
                return Err(gap(before.first + before.count, next.first));
            }
            if next.prev.is_some() && next.prev != before.last_hash {
                let at = Some(Position(next.first));
                return Err(damaged_at(Part::Log, at, Flaw::PrevHash));
            }
        }
        Ok(())
    }

    /// Read what is committed, then cut off what lies past it in the
    /// active segment. Only the writer does this, holding the lease, and
    /// not on a handle that writes nothing more: one whose append's sync
    /// failed and whose second write of that group then failed in any
    /// way, before its sync or at it
    /// (`docs/decisions/060-an-append-syncs-its-two-files-at-once.md`).
    pub(crate) fn recover(&mut self) -> Result<()> {
        if self.unsynced {
            return Err(Error::unavailable(Situation::Unsynced));
        }
        self.refresh()?;
        self.cut()?;
        let Some(active) = self.segments.last_mut() else {
            return Ok(());
        };
        let (through, raw) = active.raw.filter(|(t, _)| *t <= active.end).unwrap_or((0, 0));
        if through < active.end {
            let path = segment_path(&self.dir, active.first);
            let more = raw_of(&path, through, active.end)?;
            active.raw = Some((active.end, raw + more));
        }
        Ok(())
    }

    /// Cut the active segment's two files back to their committed lengths,
    /// as this handle has read them: the index first, which commits
    /// nothing the segment then loses. A file shorter than that is
    /// refused as damaged, a segment before either file is cut.
    fn cut(&self) -> Result<()> {
        let Some(active) = self.segments.last() else {
            return Ok(());
        };
        let lines = segment_path(&self.dir, active.first);
        let held = fs::metadata(&lines).ok().filter(fs::Metadata::is_file);
        if held.is_some_and(|m| m.len() < active.end) {
            return Err(short(&lines));
        }
        for (path, len) in [
            (index_path(&self.dir, active.first), active.index_read),
            (segment_path(&self.dir, active.first), active.end),
        ] {
            // A crash between creating a segment and its index leaves no
            // index; the writer creates it empty.
            let file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .map_err(|e| io_error(Doing::Open, Part::Log, e))?;
            let now = file
                .metadata()
                .map_err(|e| io_error(Doing::Read, Part::Log, e))?
                .len();
            if now < len {
                return Err(short(&path));
            }
            if now > len {
                file.set_len(len)
                    .and_then(|()| file.sync_all())
                    .map_err(|e| io_error(Doing::Truncate, Part::Log, e))?;
            }
        }
        Ok(())
    }

    /// The bytes the active segment's committed events restore to, as the
    /// writer last measured them: each line's and each block's `raw`
    /// (`docs/log-format.md` §1.5); where it has not, the bytes its file
    /// holds.
    pub(crate) fn active_raw(&self) -> Option<u64> {
        let active = self.segments.last()?;
        Some(match active.raw {
            Some((through, raw)) if through == active.end => raw,
            _ => active.end,
        })
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
        self.recover()?;
        self.continues(fence, events)?;
        if events.is_empty() {
            return Ok(());
        }
        let active = self.segments.last().ok_or_else(|| {
            Error::unavailable(Situation::Unmade {
                part: Part::Segment,
            })
        })?;
        let (segment_path, index_path) = (
            segment_path(&self.dir, active.first),
            index_path(&self.dir, active.first),
        );
        // An active segment holding nothing is named by the position its
        // first event takes; one named otherwise is not this log's.
        if active.count == 0 && events[0].position.0 != active.first {
            let (named, position) = (Position(active.first), events[0].position);
            return Err(Error::invalid(Case::ActiveSegmentNamed { named, position }));
        }
        let (lines, index) = lines_of(events, active.end, active.first);
        let parts = [
            (segment_path.as_path(), &lines[..], active.end),
            (index_path.as_path(), &index[..], active.index_read),
        ];
        // A failed sync is written again and synced once more; where that
        // second write fails in any way, the handle writes nothing more
        // (`docs/decisions/060-an-append-syncs-its-two-files-at-once.md`).
        let mut written = self.put(parts);
        let again = matches!(written, Err((_, true)));
        if again {
            written = self.put(parts);
        }
        if let Err((e, _)) = written {
            let e = self.failed(events, &e, again);
            self.unsynced = again;
            return Err(e);
        }
        let active = self.segments.last_mut().expect("checked above");
        // A group this handle wrote needs no check.
        (active.checked, active.unproven) = ((active.end, active.index_read), false);
        active.index_read += index.len() as u64;
        active.count += events.len() as u64;
        active.raw = active.raw.filter(|(t, _)| *t == active.end).map(|(t, raw)| {
            let added: u64 = events.iter().map(|e| match &e.frame {
                Some(f) if f.first == e.position => block_of(f).and_then(Result::ok).map_or(0, |b| b.raw),
                Some(_) => 0,
                None => e.bytes.len() as u64 + 1,
            }).sum();
            (t + lines.len() as u64, raw + added)
        });
        active.end += lines.len() as u64;
        active.last_hash = events.last().map(|e| e.hash.0.clone());
        Ok(())
    }

    /// Write each of `parts`, a file of the log, its bytes and the offset
    /// they begin at, the segment first, then sync the files at once. An
    /// error says whether it was a sync that failed.
    fn put(&self, parts: [(&Path, &[u8], u64); 2]) -> std::result::Result<(), (io::Error, bool)> {
        let mut files = Vec::new();
        for (path, bytes, at) in parts {
            let fail = |e| (e, false);
            let mut file = OpenOptions::new().write(true).open(path).map_err(fail)?;
            file.seek(SeekFrom::Start(at))
                .and_then(|_| file.write_all(bytes))
                .map_err(fail)?;
            files.push(file);
        }
        let sync = self.sync;
        let (synced, indexed) = std::thread::scope(|s| {
            // Where no thread can be made, the segment is synced here.
            let segment = || sync(parts[0].0, &files[0]);
            let synced = std::thread::Builder::new().spawn_scoped(s, segment);
            let indexed = sync(parts[1].0, &files[1]);
            let panicked = |_| Err(io::Error::other("the sync's thread panicked"));
            let synced = match synced {
                Ok(thread) => thread.join().unwrap_or_else(panicked),
                Err(_) => segment(),
            };
            (synced, indexed)
        });
        synced.and(indexed).map_err(|e| (e, true))
    }

    /// Write the active segment's last group again and sync it, as a
    /// writer takes the lease: bytes whose sync failed may be held in
    /// memory as written, where the check found them (module doc).
    pub(crate) fn settle(&self) -> Result<()> {
        let Some(active) = self.segments.last() else {
            return Ok(());
        };
        let (from, indexed) = active.checked;
        if active.end <= from {
            return Ok(());
        }
        let (lines, index) = (
            segment_path(&self.dir, active.first),
            index_path(&self.dir, active.first),
        );
        let read = |path: &Path, from: u64, to: u64| bytes_at(&File::open(path)?, from, to - from);
        let reading = |e| io_error(Doing::Read, Part::Log, e);
        let bytes = read(&lines, from, active.end).map_err(reading)?;
        let entries = read(&index, indexed, active.index_read).map_err(reading)?;
        let parts = [
            (lines.as_path(), &bytes[..], from),
            (index.as_path(), &entries[..], indexed),
        ];
        self.put(parts)
            .map_err(|(e, _)| io_error(Doing::Sync, Part::Log, e))
    }

    /// Refuse `events` unless they continue the log as this handle has
    /// read it, under `fence` as [`Self::append`] takes it.
    fn continues(&mut self, fence: Option<u64>, events: &[CanonicalEvent]) -> Result<()> {
        let first = self.tip().map_or(0, |(p, _)| p.0 + 1);
        let prev = self.tip().map_or_else(Digest::zero, |(_, h)| h);
        let floor = match (fence, first) {
            (None, 1..) => self.fence_at(first - 1)?,
            _ => 0,
        };
        lakelands_core::engines::append_fault(events, first, prev, fence, floor)?;
        lakelands_core::log::frame_fault(events)
    }

    /// The error of an append of `events` whose write to a file of the
    /// log failed with `e`. What it left is read, then cut, before the
    /// error is answered: a group whose commit line is whole stays, and
    /// its outcome is not known. Where the cut fails, the next append or
    /// seal cuts before it writes, or is refused. One that found no room
    /// and left nothing is named as that, unless it wrote `again` after a
    /// failed sync: the handle then writes nothing more.
    fn failed(
        &mut self,
        events: &[CanonicalEvent],
        e: &io::Error,
        again: bool,
    ) -> lakelands_api::Error {
        let last = events.last().map(|e| (e.position, e.hash.clone()));
        let (left, cut) = match self.recover() {
            Ok(()) if self.tip() == last => (Left::Unsure, None),
            Ok(()) => (Left::Nothing, None),
            Err(cut) => (Left::Uncut, Some(Box::new(cut))),
        };
        let detail = Detail::io(e);
        if !again && left == Left::Nothing && lakelands_core::no_room(e) {
            let part = Some(Part::Log);
            return Error::unavailable(Situation::NoRoom { part, detail });
        }
        Error::unavailable(Situation::AppendFailed { detail, left, cut })
    }

    /// The fence of the event at `position`.
    fn fence_at(&mut self, position: u64) -> Result<u64> {
        self.read(Position(position), 1)?
            .first()
            .map(|e| e.fence)
            .ok_or_else(|| {
                let flaw = Flaw::NoEvent {
                    position: Position(position),
                };
                damaged_at(Part::Log, None, flaw)
            })
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
            let file = File::open(&path).map_err(|e| io_error(Doing::Read, Part::Log, e))?;
            out.extend(events_at((&file, segment.first), wanted, &mut self.held)?);
            if out.len() >= limit as usize {
                break;
            }
        }
        Ok(out)
    }

    /// At most `n` of a segment's committed event lines from position
    /// `start` on, each with its `prev`: from where the segment's last read
    /// ended, when that is `start`, and else from a line near it, each line
    /// checked to be the position that follows the one before.
    fn entries_of(dir: &Path, segment: &mut Segment, start: u64, n: u64) -> Result<Vec<Entry>> {
        let path = index_path(dir, segment.first);
        let last = segment.last().unwrap_or(segment.first);
        let mut file = File::open(&path).map_err(|e| io_error(Doing::Read, Part::Log, e))?;
        let mut resume = match &segment.resume {
            resume if resume.expected == start => resume.clone(),
            _ => line_near(&path, &file, segment, start)?,
        };
        file.seek(SeekFrom::Start(resume.at))
            .map_err(|e| io_error(Doing::Read, Part::Log, e))?;
        let mut lines = std::io::BufReader::new(file);
        let mut line = Vec::new();
        let (mut out, wanted) = (Vec::<Entry>::new(), n as usize);
        while resume.expected <= last && out.len() < wanted {
            line.clear();
            let read = lines
                .read_until(b'\n', &mut line)
                .map_err(|e| io_error(Doing::Read, Part::Log, e))?;
            if read == 0 || line.last() != Some(&b'\n') {
                let position = Position(resume.expected);
                return Err(damaged(&path, Flaw::EndsBefore { position }));
            }
            resume.at += read as u64;
            let parsed = parse_at(&path, &line, resume.expected)?;
            if let Some(e) = resume.take(&path, parsed)?
                && e.position >= start
            {
                out.push(e);
            }
        }
        segment.resume = resume;
        Ok(out)
    }

    /// Close the active segment, if it holds any event, and begin the next
    /// with the events `first` answers for it, under `fence` as
    /// [`Self::append`] takes it, in one step: the rename that gives the
    /// next segment its name (module doc). An error before it leaves the
    /// log as it was; an error after it says that the seal stands.
    pub(crate) fn seal(
        &mut self,
        fence: Option<u64>,
        first: &mut dyn FnMut(&SegmentDigest) -> Result<Vec<CanonicalEvent>>,
    ) -> Result<Option<SegmentDigest>> {
        self.recover()?;
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
        let events = lakelands_core::engines::first_of_next(first, &sealed)?;
        self.continues(fence, &events)?;
        let (lines, index) = lines_of(&events, 0, last + 1);
        let (segment, index_path) = (
            segment_path(&self.dir, last + 1),
            index_path(&self.dir, last + 1),
        );
        let partial = segment.with_extension(PARTIAL);
        let write = |path: &Path, part: Part, bytes: &[u8]| {
            File::create(path)
                .and_then(|mut file| file.write_all(bytes).and_then(|()| file.sync_all()))
                .map_err(|e| io_error(Doing::Write, part, e))
        };
        let (log, beside) = (self.dir.join("log"), self.dir.join("index"));
        // The index and its name are durable before the segment's name
        // exists: a segment that is listed has its events committed.
        let written = write(&index_path, Part::LogIndex, &index)
            .and_then(|()| sync_dir(&beside).map_err(|e| io_error(Doing::Sync, Part::LogIndex, e)))
            .and_then(|()| write(&partial, Part::Segment, &lines))
            .and_then(|()| {
                fs::rename(&partial, &segment).map_err(|e| io_error(Doing::Rename, Part::Log, e))
            });
        if let Err(e) = written {
            let _ = fs::remove_file(&partial);
            let _ = fs::remove_file(&index_path);
            return Err(e);
        }
        self.segments.last_mut().expect("checked above").digest = Some(digest);
        // The rename was the step: an error from here on says so.
        sync_dir(&log).map_err(|e| {
            let detail = Detail::io(&e);
            Error::unavailable(Situation::SealUnsure { detail })
        })?;
        self.refresh().map_err(|e| {
            let (last, cause) = (Position(last), Box::new(e));
            Error::unavailable(Situation::SealedNotRead { last, cause })
        })?;
        Ok(Some(sealed))
    }

    /// Take away what a seal that never reached its rename left behind a
    /// crash: a segment's lines under their other name, and an index whose
    /// segment does not exist. No handle reads either, so one that cannot
    /// be removed is passed over and refuses no lease. Only the writer
    /// does this, as it takes the lease.
    pub(crate) fn sweep(&self) -> Result<()> {
        let firsts = self.files()?;
        for sub in ["log", "index"] {
            let dir = self.dir.join(sub);
            for name in names(&dir)? {
                let of_no_segment = |first: &str| first.parse().is_ok_and(|f| !firsts.contains(&f));
                if name.ends_with(PARTIAL) || name.strip_suffix(".idx").is_some_and(of_no_segment) {
                    let _ = fs::remove_file(dir.join(&name));
                }
            }
        }
        Ok(())
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

#[cfg(test)]
mod tests;
