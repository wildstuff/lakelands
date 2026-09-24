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

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use lakelands_api::engine::{CanonicalEvent, Lease};
use lakelands_api::error::Result;
use lakelands_api::event::{Digest, EventId, SegmentDigest};
use lakelands_api::id::Position;
use sha2::{Digest as _, Sha256};

use crate::{invalid, io_error, unavailable};

/// One event as the index keeps it.
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

/// One segment: its committed events and how far its index is read.
#[derive(Debug)]
struct Segment {
    first: u64,
    entries: Vec<Entry>,
    /// Bytes of the index file read through its last commit line.
    index_read: u64,
    /// The digest, once the segment is sealed and it was computed.
    digest: Option<Digest>,
}

impl Segment {
    /// The byte where the segment's committed events end.
    fn end(&self) -> u64 {
        self.entries.last().map_or(0, |e| e.offset + e.len + 1)
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
                self.segments.push(Segment {
                    first,
                    entries: Vec::new(),
                    index_read: 0,
                    digest: None,
                });
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
            let mut tail = Vec::new();
            file.seek(SeekFrom::Start(segment.index_read))
                .and_then(|_| file.read_to_end(&mut tail))
                .map_err(|e| io_error("reading", &path, e))?;
            let mut group = Vec::new();
            let mut read = 0u64;
            for line in tail.split_inclusive(|b| *b == b'\n') {
                if line.last() != Some(&b'\n') {
                    break;
                }
                read += line.len() as u64;
                let parsed: serde_json::Value = serde_json::from_slice(line).map_err(|e| {
                    unavailable(format!("the log index {} is damaged: {e}", path.display()))
                })?;
                match parsed.as_array().map(Vec::as_slice) {
                    Some([kind]) if kind == "c" => {
                        segment.entries.append(&mut group);
                        segment.index_read += read;
                        read = 0;
                    }
                    Some([kind, rest @ ..]) if kind == "e" => {
                        let entry: (u64, u64, u64, u64, String, String, String) =
                            serde_json::from_value(serde_json::Value::Array(rest.to_vec()))
                                .map_err(|e| {
                                    unavailable(format!(
                                        "the log index {} is damaged: {e}",
                                        path.display()
                                    ))
                                })?;
                        let (position, fence, offset, len, id, prev, hash) = entry;
                        group.push(Entry {
                            position,
                            fence,
                            offset,
                            len,
                            id,
                            prev,
                            hash,
                        });
                    }
                    _ => {
                        return Err(unavailable(format!(
                            "the log index {} holds a line it does not know",
                            path.display()
                        )));
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
            (self.segment_path(active.first), active.end()),
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

    fn entries(&self) -> impl Iterator<Item = (&Segment, &Entry)> {
        self.segments
            .iter()
            .flat_map(|s| s.entries.iter().map(move |e| (s, e)))
    }

    /// The last event's position and hash.
    pub(crate) fn tip(&self) -> Option<(Position, Digest)> {
        self.entries()
            .last()
            .map(|(_, e)| (Position(e.position), Digest(e.hash.clone())))
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
        let mut offset = active.end();
        let mut lines = Vec::new();
        let mut index = Vec::new();
        let mut added = Vec::new();
        for event in events {
            lines.extend_from_slice(&event.bytes);
            lines.push(b'\n');
            let entry = Entry {
                position: event.position.0,
                fence: event.fence,
                offset,
                len: event.bytes.len() as u64,
                id: event.id.0.clone(),
                prev: event.prev.0.clone(),
                hash: event.hash.0.clone(),
            };
            let line = serde_json::json!([
                "e",
                entry.position,
                entry.fence,
                entry.offset,
                entry.len,
                entry.id,
                entry.prev,
                entry.hash
            ]);
            index.extend_from_slice(line.to_string().as_bytes());
            index.push(b'\n');
            offset += entry.len + 1;
            added.push(entry);
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
        active.entries.extend(added);
        Ok(())
    }

    /// At most `limit` events from `from` on, as appended.
    pub(crate) fn read(&mut self, from: Position, limit: u32) -> Result<Vec<CanonicalEvent>> {
        self.refresh()?;
        let mut out = Vec::new();
        for segment in &self.segments {
            let wanted: Vec<&Entry> = segment
                .entries
                .iter()
                .filter(|e| e.position >= from.0)
                .take(limit as usize - out.len())
                .collect();
            if wanted.is_empty() {
                continue;
            }
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
                    id: EventId(e.id.clone()),
                    prev: Digest(e.prev.clone()),
                    hash: Digest(e.hash.clone()),
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

    /// The digest of a segment's committed bytes.
    fn digest_of(&self, segment: &Segment) -> Result<Digest> {
        let path = self.segment_path(segment.first);
        let mut file = File::open(&path).map_err(|e| io_error("reading", &path, e))?;
        let mut bytes = vec![0; segment.end() as usize];
        file.read_exact(&mut bytes)
            .map_err(|e| io_error("reading", &path, e))?;
        Ok(Digest(lakelands_core::canonical::hex(&Sha256::digest(
            &bytes,
        ))))
    }

    /// Close the active segment, if it holds any event, and start the next.
    pub(crate) fn seal(&mut self) -> Result<Option<SegmentDigest>> {
        self.refresh()?;
        let Some(active) = self.segments.last() else {
            return Ok(None);
        };
        let (Some(first_event), Some(last_event)) = (active.entries.first(), active.entries.last())
        else {
            return Ok(None);
        };
        let digest = self.digest_of(active)?;
        let sealed = SegmentDigest {
            first: Position(first_event.position),
            last: Position(last_event.position),
            digest: digest.clone(),
        };
        let next = last_event.position + 1;
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
            let (Some(first), Some(last)) = (segment.entries.first(), segment.entries.last())
            else {
                continue;
            };
            let (first, last) = (Position(first.position), Position(last.position));
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
