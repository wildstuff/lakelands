//! Import: a lake born from an export
//! (`docs/decisions/030-a-lake-directory-verifies-without-its-engine.md`).
//!
//! An export is a lake's log as segment files, and it verifies without
//! Lakelands. An import opens a new lake from one, on any engine, with
//! every event unchanged and at its position, so the lake it makes is the
//! exported lake: the same chain, the same head, the same answers, and an
//! export of it writes the same files.
//!
//! **Everything is read before anything is written.** The export is
//! verified whole (`lakelands_core::export::verify`: canonical lines, consecutive
//! positions, the hash chain, every checkpoint against the files before
//! it, versions), then read once more for what only an import needs:
//! every event belongs to the lake the location names, and every fence is
//! the one its writer held (`docs/log-format.md` §4.1: 0 at position 0,
//! each `lease_taken` carrying the fence it names, every other event the
//! fence of the `lease_taken` before it). A fault in the files refuses the
//! import before the new lake holds anything.
//!
//! **The target must be empty.** The engine is opened `Create` and the
//! lease taken; a log that holds anything refuses the import, naming its
//! head: an import is a lake's birth, not a merge.
//!
//! **Then the files, as they are.** Each file's events go to
//! `LakeEngine::import` [`BATCH`] at a time; before each file after the
//! first, which starts with its `checkpoint`, the engine seals the segment
//! before it, and the digest it answers must be the file's. The
//! projection is folded under the import's lease as a rebuild folds it,
//! and nothing is appended: the head is the export's. The lease is given
//! up, and the host opens the lake as always; that open takes a lease
//! greater than every fence in the log and continues it.
//!
//! **A failure is loud.** Once the first event is written, what can still
//! fail is the target's storage. The error names the position the import
//! reached and the file it was reading; the lake then holds a true prefix
//! of the export, which is not the source, and refuses another import,
//! naming its head. The host removes it (its file or schema) and imports
//! again.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lakelands_api::engine::{CanonicalEvent, Lease, OpenMode};
use lakelands_api::event::Digest;
use lakelands_api::location::Location;
use lakelands_api::seam::Seams;
use lakelands_api::{Error, LakeId, Position, Result};
use lakelands_core::unavailable;
use sha2::{Digest as _, Sha256};

use crate::lake::Lake;

/// How many events an import hands the engine at a time: each batch is
/// one durable append, so the batch is far larger than a write's commit
/// window, and small enough that what it holds does not matter beside a
/// page of the fold (some 600 bytes an event).
const BATCH: usize = 2_048;

/// What an import made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Imported {
    /// The lake.
    pub lake: LakeId,
    /// Its head: the export's last position.
    pub through: Position,
    /// The hash of the event at `through`.
    pub hash: Digest,
    /// How many events it holds.
    pub events: u64,
    /// How many segment files it came from.
    pub files: usize,
}

/// A refusal of the import as asked: the export or the target is not
/// one it can make a lake from, and asking again changes nothing.
fn precondition(message: String) -> Error {
    Error::Refused {
        field: None,
        reason: lakelands_api::Refusal::Precondition { message },
    }
}

fn io(what: &str, path: &Path, e: std::io::Error) -> Error {
    unavailable(format!("cannot {what} {}: {e}", path.display()))
}

/// What an import reads of a line beyond its bytes; the whole export was
/// verified before (canonical lines, the chain, every hash), so the rest
/// of an event need not be opened again.
#[derive(serde::Deserialize)]
struct Envelope {
    position: Position,
    id: lakelands_api::event::EventId,
    prev: Digest,
    hash: Digest,
    fence: u64,
    lake: LakeId,
    body: Body,
}

/// Of an event's body, its kind and the fence a `lease_taken` names.
#[derive(serde::Deserialize)]
struct Body {
    kind: String,
    #[serde(default)]
    fence: Option<u64>,
}

/// Each line of `path` as the event the store would append, with its
/// envelope, in order.
fn events(path: &Path) -> Result<impl Iterator<Item = Result<(CanonicalEvent, Envelope)>> + '_> {
    let file = fs::File::open(path).map_err(|e| io("open", path, e))?;
    Ok(BufReader::new(file).split(b'\n').map(move |line| {
        let line = line.map_err(|e| io("read", path, e))?;
        let e: Envelope = serde_json::from_slice(&line).map_err(|err| {
            unavailable(format!(
                "{} holds a line that is not an event: {err}",
                path.display()
            ))
        })?;
        let stored = CanonicalEvent {
            position: e.position,
            id: e.id.clone(),
            prev: e.prev.clone(),
            hash: e.hash.clone(),
            fence: e.fence,
            bytes: line,
        };
        Ok((stored, e))
    }))
}

/// What only an import checks, over the whole export, before anything is
/// written: every event is `lake`'s, and every fence is its writer's.
/// Answers each file's digest.
fn check(files: &[PathBuf], lake: &LakeId) -> Result<Vec<Digest>> {
    let mut fence = 0;
    let mut digests = Vec::with_capacity(files.len());
    for path in files {
        // Hashed as it is read, so a segment is never held whole.
        let mut hasher = Sha256::new();
        let mut file = fs::File::open(path).map_err(|e| io("open", path, e))?;
        std::io::copy(&mut file, &mut hasher).map_err(|e| io("read", path, e))?;
        digests.push(Digest(lakelands_core::canonical::hex(&hasher.finalize())));
        for event in events(path)? {
            let (_, event) = event?;
            if event.lake != *lake {
                return Err(precondition(format!(
                    "{} holds at position {} an event of the lake `{}`, and the import makes \
                     `{lake}`; a lake's name is inside every hash, so an export opens only as \
                     the lake it came from; nothing was imported",
                    path.display(),
                    event.position,
                    event.lake
                )));
            }
            if event.body.kind == "lease_taken"
                && let Some(taken) = event.body.fence
            {
                fence = taken;
            }
            if event.fence != fence {
                return Err(unavailable(format!(
                    "the export is damaged in {} at position {}: the event carries fence {}, and \
                     its writer's lease had fence {fence}; nothing was imported",
                    path.display(),
                    event.position,
                    event.fence
                )));
            }
        }
    }
    Ok(digests)
}

/// Import the export in `dir` as a new lake at `location` (module doc).
pub fn import(location: &Location, seams: Seams, dir: &Path) -> Result<Imported> {
    let verified = lakelands_core::export::verify(dir)?;
    let files = lakelands_core::export::segment_files(dir)?;
    let digests = check(&files, &location.lake)?;
    let opener = crate::lake::opener(location, &seams)?;
    let engine: Arc<dyn lakelands_api::engine::LakeEngine> =
        Arc::from(opener.open(location, OpenMode::Create)?);
    let lease = engine.take_lease(&format!("lakelands import {}", std::process::id()))?;
    let written = (|| {
        if let Some((head, _)) = engine.tip()? {
            return Err(precondition(format!(
                "the lake `{}` already holds events through position {head}; an import makes a \
                 new lake and goes only into one that holds nothing",
                location.lake
            )));
        }
        carry(engine.as_ref(), &lease, &files, &digests, location)?;
        let lake = Lake::assemble(location, seams, engine.clone(), Some(lease.clone()));
        lake.catch_up().map_err(|e| {
            unavailable(format!(
                "every event was imported, and folding the projection failed: {e}; a writer's \
                 open folds it again"
            ))
        })
    })();
    let released = engine.release_lease(&lease);
    written?;
    released?;
    Ok(Imported {
        lake: location.lake.clone(),
        through: verified.through,
        hash: verified.hash,
        events: verified.through.0 + 1,
        files: files.len(),
    })
}

/// Hand every file's events to the engine a batch at a time, sealing
/// before each file after the first.
fn carry(
    engine: &dyn lakelands_api::engine::LakeEngine,
    lease: &Lease,
    files: &[PathBuf],
    digests: &[Digest],
    location: &Location,
) -> Result<()> {
    let window = BATCH;
    let mut reached = Position::GENESIS;
    let stopped = |path: &Path, reached: Position, e: Error| {
        unavailable(format!(
            "the import stopped in {} at position {reached}: {e}; the lake `{}` holds the \
             export's events before that position, a true prefix that is not the exported lake, \
             and refuses another import; remove it (its file or schema) and import again",
            path.display(),
            location.lake
        ))
    };
    for (i, path) in files.iter().enumerate() {
        if i > 0 {
            let sealed = engine
                .seal_segment(lease)
                .map_err(|e| stopped(path, reached, e))?;
            let before = &digests[i - 1];
            if sealed.as_ref().map(|s| &s.digest) != Some(before) {
                return Err(stopped(
                    path,
                    reached,
                    unavailable(format!(
                        "the engine sealed the segment before it with the digest {:?}, and \
                             the file's is `{}`",
                        sealed.map(|s| s.digest.0),
                        before.0
                    )),
                ));
            }
        }
        let mut batch = Vec::with_capacity(window);
        for event in events(path)? {
            batch.push(event?.0);
            if batch.len() == window {
                engine
                    .import(lease, &batch)
                    .map_err(|e| stopped(path, reached, e))?;
                reached = batch.last().expect("a full window").position.next();
                batch.clear();
            }
        }
        if !batch.is_empty() {
            engine
                .import(lease, &batch)
                .map_err(|e| stopped(path, reached, e))?;
            reached = batch.last().expect("not empty").position.next();
        }
    }
    Ok(())
}
