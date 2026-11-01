//! The block (`docs/log-format.md` §1.5): an append of two or more events,
//! its lines compressed by zlib under one first line of canonical JSON.
//!
//! A block's first line holds its format, so a release that does not read
//! the format finds the number there, and what a reader needs before it
//! inflates: the first position and the `prev` the block's first event
//! names, its last event's hash, how many events it holds, the last
//! position of the append it belongs to (`end`), and its sizes inflated
//! (`raw`) and stored (`stored`). Its lines are the stored form of §1.2
//! inside it: a frame line first where it holds two or more events, then
//! each event's line, leaving out what the block decides: `v`,
//! `position`, `prev`, `hash`, and an `id` the event's own members give
//! ([`derived_id`]). Each event restores to the canonical bytes, hash and
//! id the store made; a changed byte anywhere fails the inflate, a line,
//! or the chain at the block's last hash.
//!
//! An append is whole or absent: a reader of a file holds the blocks of
//! an append until the one that reaches its `end`, and a file that ends
//! before it holds an append cut short ([`Flaw::AppendShort`]).

use std::io::{self, BufRead, Read as _, Write as _};
use std::ops::Range;
use std::sync::Arc;

use flate2::Compression;
use flate2::bufread::ZlibDecoder;
use flate2::write::ZlibEncoder;
use lakelands_api::Position;
use lakelands_api::engine::{CanonicalEvent, Frame};
use lakelands_api::error::Flaw;
use lakelands_api::event::{Digest, EventId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Framed, Lines, format_of, is_frame, json_of, line_of, restored};
use crate::canonical;

/// The most events a block holds.
pub const BLOCK_EVENTS: usize = 1_000;

/// The most bytes of its events' canonical lines a block holds, but for
/// one event larger alone.
pub const BLOCK_BYTES: usize = 1 << 20;

/// What a block's first line says of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Block {
    /// The last position of the append the block belongs to.
    pub end: Position,
    /// How many events it holds.
    pub events: u64,
    /// Its first event's position.
    pub first: Position,
    /// Its last event's hash.
    pub hash: Digest,
    /// The hash its first event names as the one before it.
    pub prev: Digest,
    /// Its lines' bytes inflated, each line feed counted.
    pub raw: u64,
    /// Its bytes as stored, after its first line's line feed and before
    /// the one that ends them.
    pub stored: u64,
}

impl Block {
    /// Its last event's position.
    pub fn last(&self) -> Position {
        Position(self.first.0.saturating_add(self.events.saturating_sub(1)))
    }

    /// The block a first line holds, read as JSON, and its format. A line
    /// that is no block's first line is [`Flaw::NotABlock`].
    pub fn of(line: &Value) -> Result<(Block, u64), Flaw> {
        let members = line.as_object().ok_or(Flaw::NotABlock)?;
        let block = members.get("block").cloned();
        let block = block.and_then(|b| serde_json::from_value::<Block>(b).ok());
        match block {
            Some(b)
                if members.len() == 2
                    && members.contains_key("v")
                    && (1..=u64::from(u32::MAX)).contains(&b.events)
                    && b.first.0.checked_add(b.events - 1).is_some()
                    && b.end >= b.last() =>
            {
                Ok((b, format_of(line)))
            }
            _ => Err(Flaw::NotABlock),
        }
    }

    /// Its first line's canonical bytes, in format `v`.
    pub fn line(&self, v: u64) -> Vec<u8> {
        canonical::to_vec(&serde_json::json!({ "block": self, "v": v }))
    }
}

/// Whether a line, read, is a block's first line: an event holds no
/// `block`.
pub fn is_block(line: &Value) -> bool {
    line.get("block").is_some()
}

/// Whether `bytes`, a frame's at the engine seam, are a block's: its first
/// line, a line feed and its stored bytes.
fn is_block_bytes(bytes: &[u8]) -> bool {
    bytes.starts_with(b"{\"block\":")
}

/// A block's bytes as the seam holds them, split into its first line and
/// its stored bytes.
fn split(bytes: &[u8]) -> Result<(&[u8], &[u8]), Flaw> {
    let at = bytes.iter().position(|b| *b == b'\n');
    at.map(|at| (&bytes[..at], &bytes[at + 1..]))
        .ok_or(Flaw::NotABlock)
}

/// The block whose bytes `frame` holds at the engine seam, if it holds a
/// block, read as JSON alone.
pub fn block_of(frame: &Frame) -> Option<Result<Block, Flaw>> {
    is_block_bytes(&frame.bytes).then(|| {
        let (head, _) = split(&frame.bytes)?;
        Block::of(&json_of(head)?).map(|(b, _)| b)
    })
}

/// The id an event's own members decide (`docs/log-format.md` §3), from
/// its canonical value: an ingest's, over its source, type, key, version
/// before, patch and box; a quarantined record's version's, over its id,
/// version before, cells, box and refusal. Any other event's is none.
pub fn derived_id(event: &Value) -> Option<EventId> {
    let body = event.get("body")?;
    fn text<'a>(v: &'a Value, name: &str) -> Option<&'a [u8]> {
        v.get(name)?.as_str().map(str::as_bytes)
    }
    let before = body.get("version")?.as_u64()?.checked_sub(1)?;
    match body.get("kind")?.as_str()? {
        "record_ingested" => {
            let parts = [
                text(event, "source")?,
                text(body, "type")?,
                text(body, "key")?,
            ];
            let patch = canonical::to_vec(body.get("patch")?);
            let boxed = event
                .get("sealed")
                .map(|s| text(s, "box").unwrap_or_default());
            Some(super::ingest_of(parts, before, &patch, boxed))
        }
        "record_quarantined" => {
            let id = body.get("id")?;
            let at = (
                [text(id, "source")?, text(id, "delivery")?],
                id.get("index")?.as_u64()?,
            );
            let cells = canonical::to_vec(body.get("cells")?);
            let refusal = canonical::to_vec(body.get("refusal")?);
            let boxed = body
                .get("sealed")
                .and_then(|s| text(s, "box"))
                .unwrap_or_default();
            Some(super::quarantined_of(at, before, &cells, boxed, &refusal))
        }
        _ => None,
    }
}

/// Whether the `id` an event holds is the one its members decide.
fn id_derived(event: &Value) -> bool {
    let held = event.get("id").and_then(Value::as_str);
    derived_id(event).is_some_and(|id| held == Some(id.0.as_str()))
}

/// The ranges of one append's events its blocks hold, from the length of
/// each event's canonical bytes: at most [`BLOCK_EVENTS`] events and
/// [`BLOCK_BYTES`] of their lines each, and at least one.
fn ranges(sizes: impl ExactSizeIterator<Item = usize>) -> Vec<Range<usize>> {
    let (n, mut out, mut start, mut bytes) = (sizes.len(), Vec::new(), 0, 0);
    for (i, size) in sizes.enumerate() {
        let full = i - start == BLOCK_EVENTS || bytes + size + 1 > BLOCK_BYTES;
        if i > start && full {
            out.push(start..i);
            (start, bytes) = (i, 0);
        }
        bytes += size + 1;
    }
    if n > start {
        out.push(start..n);
    }
    out
}

/// zlib at level 6 of `raw`.
fn deflate(raw: &[u8]) -> Vec<u8> {
    let mut out = ZlibEncoder::new(Vec::new(), Compression::new(6));
    out.write_all(raw)
        .and_then(|()| out.finish())
        .expect("compressing into memory does not fail")
}

/// Stores `events`, one append's of two or more, their canonical values
/// `values`, as blocks: each event's line and the frame its block's bytes
/// are, `end` in each the append's last position.
pub(super) fn into_blocks(events: &mut [CanonicalEvent], values: &[Value]) {
    let v = values.first().map_or(0, format_of);
    let end = events.last().map_or(Position(0), |e| e.position);
    let sizes = events.iter().map(|e| e.bytes.len());
    for range in ranges(sizes) {
        let leave = |_: usize, e: &Value| {
            let mut leave = vec!["hash", "position", "prev", "v"];
            if id_derived(e) {
                leave.push("id");
            }
            leave
        };
        let (frame, lines) = super::form(&values[range.clone()], leave);
        let mut raw = Vec::new();
        for line in frame.iter().chain(&lines) {
            raw.extend_from_slice(line);
            raw.push(b'\n');
        }
        let z = deflate(&raw);
        let (first, last) = (&events[range.start], &events[range.end - 1]);
        let block = Block {
            end,
            events: range.len() as u64,
            first: first.position,
            hash: last.hash.clone(),
            prev: first.prev.clone(),
            raw: raw.len() as u64,
            stored: z.len() as u64,
        };
        let mut bytes = block.line(v);
        bytes.push(b'\n');
        bytes.extend_from_slice(&z);
        let frame = Arc::new(Frame {
            first: first.position,
            events: range.len() as u32,
            bytes,
        });
        for (event, line) in events[range].iter_mut().zip(lines) {
            (event.bytes, event.frame) = (line, Some(frame.clone()));
        }
    }
}

/// The lines a block's stored bytes inflate to, the `raw` bytes its first
/// line names, each ended by a line feed; anything else, and bytes after
/// the zlib stream, is [`Flaw::BlockBytes`].
pub fn inflate(block: &Block, z: &[u8]) -> Result<Vec<u8>, Flaw> {
    let mut out = Vec::with_capacity(block.raw.min(4 * BLOCK_BYTES as u64) as usize);
    let mut stream = ZlibDecoder::new(z);
    let read = (&mut stream).take(block.raw + 1).read_to_end(&mut out);
    let whole = read.is_ok()
        && out.len() as u64 == block.raw
        && stream.total_in() == z.len() as u64
        && z.len() as u64 == block.stored
        && out.last() == Some(&b'\n');
    whole.then_some(out).ok_or(Flaw::BlockBytes)
}

/// The first line a block's stored bytes inflate to, inflating no further.
fn first_line(z: &[u8]) -> Result<Vec<u8>, Flaw> {
    let mut line = Vec::new();
    let mut lines = io::BufReader::new(ZlibDecoder::new(z)).take(4 * BLOCK_BYTES as u64);
    lines
        .read_until(b'\n', &mut line)
        .map_err(|_| Flaw::BlockBytes)?;
    match line.pop() {
        Some(b'\n') => Ok(line),
        _ => Err(Flaw::BlockBytes),
    }
}

/// One event's line in a block restored (§1.5): `v` the block's, the
/// members `frame` holds, the `position` and `prev` given, the `id` its
/// members decide where it holds none, and `hash` given, or computed
/// where none is. A line that holds what the block decides is refused.
fn in_block(
    line: Value,
    v: u64,
    frame: Option<&Framed>,
    (position, prev): (Position, &Digest),
    hash: Option<&Digest>,
) -> Result<Value, Flaw> {
    if is_frame(&line) || is_block(&line) {
        return Err(Flaw::FrameInFrame);
    }
    let Value::Object(mut members) = line else {
        return Ok(line);
    };
    if let Some(member) = ["hash", "position", "prev", "v"]
        .iter()
        .find(|m| members.contains_key(**m))
    {
        return Err(Flaw::FramedTwice {
            member: (*member).to_owned(),
        });
    }
    members.insert("v".into(), v.into());
    let (mut event, _) = restored(Value::Object(members), frame, Some((position, prev)))?;
    if event.get("id").is_none()
        && let Some(id) = derived_id(&event)
    {
        event["id"] = id.0.into();
    }
    event["hash"] = match hash {
        Some(h) => h.0.clone().into(),
        None => crate::engines::hash_of(event.clone()).into(),
    };
    Ok(event)
}

/// A line read as JSON, and, `strict`, refused unless canonical.
fn read_line(bytes: &[u8], strict: bool) -> Result<Value, Flaw> {
    let line = json_of(bytes)?;
    match strict && canonical::to_vec(&line) != bytes {
        true => Err(Flaw::NotCanonical),
        false => Ok(line),
    }
}

/// The event at the engine seam whose frame holds a block's bytes,
/// restored from its line and the block: the block's first line read as
/// [`line_of`] reads one, `strict`, else as JSON alone, and its frame line
/// inflated alone. A block that does not say what the frame does, or does
/// not hold the event, is [`Flaw::Disagrees`]. The hash is the one the
/// engine keeps, which the reader checks.
pub(super) fn restore_one(
    stored: &CanonicalEvent,
    frame: &Frame,
    strict: bool,
) -> Option<Result<(Value, bool), Flaw>> {
    if !is_block_bytes(&frame.bytes) {
        return None;
    }
    let restore = || {
        let (head, z) = split(&frame.bytes)?;
        let (block, v) = Block::of(&if strict {
            line_of(head)?
        } else {
            json_of(head)?
        })?;
        let within = stored.position.0.checked_sub(block.first.0);
        if block.first != frame.first
            || block.events != u64::from(frame.events)
            || block.stored != z.len() as u64
            || !within.is_some_and(|i| i < block.events)
        {
            return Err(Flaw::Disagrees);
        }
        let first = read_line(&first_line(z)?, strict)?;
        let framed = match is_frame(&first) {
            true => Some(Framed::of(first)?),
            false => None,
        };
        let line = read_line(&stored.bytes, strict)?;
        let derived = (stored.position, &stored.prev);
        let event = in_block(line, v, framed.as_ref(), derived, Some(&stored.hash))?;
        Ok((event, false))
    };
    Some(restore())
}

/// An event restored from a block: its value and its canonical bytes.
type Held = (Value, Vec<u8>);

/// Each event a block restores (§1.5) after the event `before`, none at a
/// segment's first, each its value and its canonical bytes: the block
/// follows `before`, its bytes inflate whole, every line is read, canonical
/// where `strict`, it holds the events it says, and the last one's hash
/// is the block's. A line that does not restore is the flaw of its own
/// event: the events before it are answered with it, so that the break is
/// named at the position of the event that does not restore.
fn restore_block(
    (block, v, z): (&Block, u64, &[u8]),
    before: Option<(Position, &Digest)>,
    strict: bool,
) -> Result<(Vec<Held>, Option<Flaw>), Flaw> {
    if let Some((position, hash)) = before {
        if block.first != position.next() {
            let (expected, found) = (position.next(), Some(block.first));
            return Err(Flaw::Position { expected, found });
        }
        if &block.prev != hash {
            return Err(Flaw::PrevHash);
        }
    }
    let raw = inflate(block, z)?;
    let (mut frame, mut out) = (None, Vec::with_capacity(block.events as usize));
    let mut prev = block.prev.clone();
    for (i, bytes) in raw[..raw.len() - 1].split(|b| *b == b'\n').enumerate() {
        if out.len() as u64 == block.events {
            return Err(Flaw::BlockBytes);
        }
        let mut restore = || -> Result<(), Flaw> {
            let line = read_line(bytes, strict)?;
            if i == 0 && is_frame(&line) {
                frame = Some(Framed::of(line)?);
                return Ok(());
            }
            let position = Position(block.first.0 + out.len() as u64);
            let event = in_block(line, v, frame.as_ref(), (position, &prev), None)?;
            prev = Digest(event["hash"].as_str().unwrap_or_default().to_owned());
            let bytes = canonical::to_vec(&event);
            out.push((event, bytes));
            Ok(())
        };
        if let Err(flaw) = restore() {
            return Ok((out, Some(flaw)));
        }
    }
    if out.len() as u64 != block.events || frame.as_ref().is_some_and(|f| f.events != block.events)
    {
        return Err(Flaw::BlockBytes);
    }
    if prev != block.hash {
        return Err(Flaw::HashMismatch);
    }
    Ok((out, None))
}

/// One unit of a segment file's bytes: a line, or a block.
#[derive(Clone, Debug)]
pub enum Unit {
    /// A line, without its line feed.
    Line(Vec<u8>),
    /// A block: what its first line says, its format, that line's bytes
    /// and its stored bytes.
    Block(Block, u64, Vec<u8>, Vec<u8>),
}

impl Unit {
    /// Its bytes as restored count them: a line's and its line feed, a
    /// block's `raw`.
    pub fn raw(&self) -> u64 {
        match self {
            Unit::Line(line) => line.len() as u64 + 1,
            Unit::Block(block, ..) => block.raw,
        }
    }
}

/// Why a file's next unit was not read whole.
#[derive(Debug)]
pub enum Fault {
    /// The file could not be read.
    Io(io::Error),
    /// The file ends inside it: a crash's tear, or a file cut short.
    Short,
    /// It is read, and it is not what the format holds.
    Flaw(Flaw),
}

/// Reads the next unit of a segment file from `input`, none at its end:
/// a line through its line feed, and where that line is a block's first,
/// the `stored` bytes it names and the line feed after them.
pub fn read_unit(input: &mut impl BufRead) -> Result<Option<Unit>, Fault> {
    let mut line = Vec::new();
    if input.read_until(b'\n', &mut line).map_err(Fault::Io)? == 0 {
        return Ok(None);
    }
    if line.pop() != Some(b'\n') {
        return Err(Fault::Short);
    }
    if !is_block_bytes(&line) {
        return Ok(Some(Unit::Line(line)));
    }
    let (block, v) = json_of(&line)
        .and_then(|l| Block::of(&l))
        .map_err(Fault::Flaw)?;
    let mut z = Vec::new();
    let read = input.take(block.stored).read_to_end(&mut z);
    let mut feed = [0];
    if read.map_err(Fault::Io)? as u64 != block.stored
        || input.read(&mut feed).map_err(Fault::Io)? == 0
    {
        return Err(Fault::Short);
    }
    match feed {
        [b'\n'] => Ok(Some(Unit::Block(block, v, line, z))),
        _ => Err(Fault::Flaw(Flaw::BlockBytes)),
    }
}

/// An event a segment file restores: its value, its canonical bytes, the
/// last position of its append, and the byte its unit begins at.
#[derive(Clone, Debug)]
pub struct Restored {
    /// Its value, restored.
    pub event: Value,
    /// Its canonical bytes; a line that is its whole event as kept.
    pub bytes: Vec<u8>,
    /// The last position of the append it belongs to.
    pub end: Position,
    /// The byte of the file its line or its block begins at.
    pub unit: u64,
}

/// A segment file's events restored in order (§1.2, §1.5), its bytes read
/// a unit at a time and never held whole: what a digest, a verifier, a
/// backup's end and an import read a file with. `strict`, every line is
/// read as [`line_of`] reads one, and canonical inside a block; else as
/// JSON alone, as an engine, which keeps events of any format, reads them.
#[derive(Debug)]
pub struct Restoring<R> {
    input: R,
    strict: bool,
    lines: Lines,
    before: Option<(Position, Digest)>,
    held: std::vec::IntoIter<Held>,
    end: Position,
    open: Option<Position>,
    read: u64,
    unit: u64,
    broken: Option<Flaw>,
}

impl<R: BufRead> Restoring<R> {
    /// The events of the file `input` reads, from its start.
    pub fn new(input: R, strict: bool) -> Restoring<R> {
        Restoring {
            input,
            strict,
            lines: Lines::default(),
            before: None,
            held: Vec::new().into_iter(),
            end: Position(0),
            open: None,
            read: 0,
            unit: 0,
            broken: None,
        }
    }

    /// The position and hash of the last event restored, none before the
    /// first.
    pub fn before(&self) -> Option<&(Position, Digest)> {
        self.before.as_ref()
    }

    /// How many bytes of the file it has read: through the last unit read.
    pub fn read(&self) -> u64 {
        self.read
    }

    /// The next event, none at the file's end. A unit that is not read
    /// whole, or does not restore, is the fault.
    pub fn next_event(&mut self) -> Result<Option<Restored>, Fault> {
        loop {
            if let Some((event, bytes)) = self.held.next() {
                let (end, unit) = (self.end, self.unit);
                return Ok(Some(Restored {
                    event,
                    bytes,
                    end,
                    unit,
                }));
            }
            if let Some(flaw) = self.broken.take() {
                return Err(Fault::Flaw(flaw));
            }
            let Some(unit) = read_unit(&mut self.input)? else {
                return Ok(None);
            };
            self.unit = self.read;
            let flaw = Fault::Flaw;
            let before = self.before.as_ref().map(|(p, h)| (*p, h));
            match unit {
                Unit::Line(bytes) => {
                    self.read += bytes.len() as u64 + 1;
                    if let Some(end) = self.open {
                        return Err(flaw(Flaw::AppendShort { end }));
                    }
                    let value = match self.strict {
                        true => line_of(&bytes),
                        false => json_of(&bytes),
                    };
                    let Some((event, whole)) = self
                        .lines
                        .line(value.map_err(flaw)?, before)
                        .map_err(flaw)?
                    else {
                        continue;
                    };
                    let position = event.get("position").and_then(Value::as_u64).unwrap_or(0);
                    let hash = event
                        .get("hash")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    self.before = Some((Position(position), Digest(hash.to_owned())));
                    self.end = Position(position + self.lines.left());
                    let bytes = if whole {
                        bytes
                    } else {
                        canonical::to_vec(&event)
                    };
                    self.held = vec![(event, bytes)].into_iter();
                }
                Unit::Block(block, v, line, z) => {
                    self.read += line.len() as u64 + block.stored + 2;
                    if self.strict {
                        line_of(&line).map_err(flaw)?;
                    }
                    if self.lines.left() > 0 {
                        return Err(flaw(Flaw::FrameInFrame));
                    }
                    if let Some(end) = self.open.filter(|end| *end != block.end) {
                        return Err(flaw(Flaw::AppendShort { end }));
                    }
                    if before.is_none() && self.open.is_some() {
                        return Err(flaw(Flaw::StartUnheld));
                    }
                    let (events, broken) =
                        restore_block((&block, v, &z), before, self.strict).map_err(flaw)?;
                    self.broken = broken;
                    self.before = Some((block.last(), block.hash.clone()));
                    self.open = (block.end > block.last()).then_some(block.end);
                    self.end = block.end;
                    self.held = events.into_iter();
                }
            }
        }
    }

    /// Refuses a file that ends inside a frame's lines or inside an
    /// append's blocks.
    pub fn end(&self) -> Result<(), Flaw> {
        self.lines.end()?;
        match self.open {
            Some(end) => Err(Flaw::AppendShort { end }),
            None => Ok(()),
        }
    }
}
