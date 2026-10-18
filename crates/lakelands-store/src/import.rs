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
//! it, the one format), then read once more for what only an import needs:
//! every event belongs to the lake the location names, and every fence is
//! the one its writer held (`docs/log-format.md` §4.1: 0 at position 0,
//! each `lease_taken` carrying the fence it names, every other event the
//! fence of the `lease_taken` before it). A fault in the files refuses the
//! import before the new lake holds anything.
//!
//! **The target must be empty,** and a lake that is there is not opened.
//! The opener is asked for the ends of the target's log first, as an open
//! asks it
//! (`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`): a
//! lake written in another format is refused for its format, and a log
//! that holds anything refuses the import, naming its head. Neither opens
//! the engine, so an import onto a lake in use takes no lease and changes
//! nothing of it: an import is a lake's birth, not a merge. Where there is
//! no lake the engine is opened `Create` and the lease taken; a
//! projection found there with no log under it is another lake's, or
//! what is left of one, and refuses the import as damaged before an
//! event is carried.
//!
//! **Then the files, as they are.** The first file's events go to
//! `LakeEngine::import` in batches of at most [`BATCH`] bytes. Each later file starts with
//! its `checkpoint`: its first batch is handed to
//! `LakeEngine::seal_segment`, which closes the segment before it and
//! begins the next with that batch in one step, once the digest the engine
//! takes of that segment is the file's; the rest follows through `import`.
//! The projection is folded under the import's lease as a rebuild folds
//! it, and nothing is appended: the head is the export's. The lease is
//! given up, and the host opens the lake as always; that open takes a
//! lease greater than every fence in the log and continues it.
//!
//! **A failure is loud.** Once the first event is written, what can still
//! fail is the target's storage. The error names the position the import
//! reached and the file it was reading; the lake then holds a true prefix
//! of the export, which is not the source, and refuses another import,
//! naming its head. The host removes it (its file or schema) and imports
//! again.

use std::collections::BTreeSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lakelands_api::engine::{CanonicalEvent, Lease, OpenMode};
use lakelands_api::error::{Case, Detail, Doing, Flaw, Part, Said, Situation};
use lakelands_api::event::{Digest, LOG_FORMAT_VERSION};
use lakelands_api::location::Location;
use lakelands_api::seam::Seams;
use lakelands_api::{Error, LakeId, Position, Result, SourceId};
use sha2::{Digest as _, Sha256};

use crate::lake::Lake;

/// How many bytes of events an import hands the engine at a time, each
/// event counted by its line and the id and hashes it carries beside it:
/// each batch is one durable append, so the batch is far larger than a
/// write's commit window, and small enough that what it holds does not
/// matter beside a page of the fold. It is bounded in bytes, not events:
/// a driver keeps the buffers it grew for the largest statement it sent
/// until its connection closes, so what an import holds follows its
/// largest batch, and events grow larger as a lake does. On PostgreSQL a
/// batch of 2 048 events held 5.4 MiB after the first segment and
/// 10.8 MiB after the second, whose first batch was 13% larger, through
/// the fold that follows (lakelands#1241).
const BATCH: usize = 512 << 10;

/// What `event` counts against a [`BATCH`].
fn weight(event: &CanonicalEvent) -> usize {
    event.bytes.len() + event.id.0.len() + event.prev.0.len() + event.hash.0.len()
}

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
fn precondition(said: impl Into<Said>) -> Error {
    let said = Box::new(said.into());
    crate::writes::refused(lakelands_api::Refusal::Precondition(said))
}

/// The export's files failed `doing`; the path is the operator's, not
/// the answer's.
fn io(doing: Doing, _: &Path, e: std::io::Error) -> Error {
    lakelands_core::io_failed(doing, Part::Export, &e)
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
    /// The event's source, for a sealed ingest.
    #[serde(default)]
    source: Option<SourceId>,
    /// What a source's key sealed in it (058).
    #[serde(default)]
    sealed: Option<Sealed>,
}

/// Of an event's body, its kind, the fence a `lease_taken` names, the
/// source a `source_forgotten` forgot or a `values_removed` took from, and
/// the versions of its key the latter destroyed.
#[derive(serde::Deserialize)]
struct Body {
    kind: String,
    #[serde(default)]
    fence: Option<u64>,
    #[serde(default)]
    source: Option<SourceId>,
    #[serde(default)]
    keys: Vec<lakelands_api::id::KeyId>,
}

/// Of an event's `sealed`, the version of the key and the source of a
/// recorded output.
#[derive(serde::Deserialize)]
struct Sealed {
    key: lakelands_api::id::KeyId,
    #[serde(default)]
    source: Option<SourceId>,
}

/// Each line of `path` as the event the store would append, with its
/// envelope, in order.
fn events(path: &Path) -> Result<impl Iterator<Item = Result<(CanonicalEvent, Envelope)>> + '_> {
    let file = fs::File::open(path).map_err(|e| io(Doing::Open, path, e))?;
    Ok(BufReader::new(file).split(b'\n').map(move |line| {
        let line = line.map_err(|e| io(Doing::Read, path, e))?;
        // The parser's own words, which name the member it does not know:
        // an import's caller is the operator, and the export is its own.
        let e: Envelope = serde_json::from_slice(&line).map_err(|err| {
            let detail = Detail::scrubbed(err);
            lakelands_core::damaged(Part::Export, None, Flaw::NotAnEvent { detail })
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
/// written: every event is `lake`'s, and every fence is its writer's; and
/// every key a sealed value names, of a source the export does not forget,
/// is one the bound key store gives. With none bound such a value is
/// refused as unbound, and a key the store does not give as missing: the
/// value would be lost (058). A forgotten source's values arrive erased,
/// and so do those under a version an act of removal destroyed (065).
/// Answers each file's digest.
fn check(
    files: &[PathBuf],
    lake: &LakeId,
    keys: Option<&dyn lakelands_api::seam::KeyStore>,
) -> Result<Vec<Digest>> {
    let (mut sealed, mut forgot, mut shredded) =
        (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    let mut fence = 0;
    let mut digests = Vec::with_capacity(files.len());
    for path in files {
        // Hashed as it is read, so a segment is never held whole.
        let mut hasher = Sha256::new();
        let mut file = fs::File::open(path).map_err(|e| io(Doing::Open, path, e))?;
        std::io::copy(&mut file, &mut hasher).map_err(|e| io(Doing::Read, path, e))?;
        digests.push(Digest(lakelands_core::canonical::hex(&hasher.finalize())));
        for event in events(path)? {
            let (_, event) = event?;
            if event.lake != *lake {
                let (position, held) = (event.position, Box::new(event.lake.clone()));
                let lake = Box::new(lake.clone());
                let case = Case::ImportOtherLake {
                    position,
                    held,
                    lake,
                };
                return Err(precondition(case));
            }
            if event.body.kind == "lease_taken"
                && let Some(taken) = event.body.fence
            {
                fence = taken;
            }
            if let Some(s) = &event.sealed
                && let Some(source) = s.source.clone().or_else(|| event.source.clone())
            {
                sealed.insert((source, s.key.clone()));
            }
            if event.body.kind == "source_forgotten" {
                forgot.extend(event.body.source.clone());
            }
            // A version an act of removal destroyed arrives erased (065).
            if event.body.kind == "values_removed"
                && let Some(source) = &event.body.source
            {
                let taken = event.body.keys.iter().map(|k| (source.clone(), k.clone()));
                shredded.extend(taken);
            }
            if event.fence != fence {
                let (carried, lease) = (event.fence, fence);
                let flaw = Flaw::FenceMismatch { carried, lease };
                return Err(lakelands_core::damaged(
                    Part::Export,
                    Some(event.position),
                    flaw,
                ));
            }
        }
    }
    let kept = |(s, k): &(SourceId, lakelands_api::id::KeyId)| {
        !forgot.contains(s) && !shredded.contains(&(s.clone(), k.clone()))
    };
    for (source, key) in sealed.into_iter().filter(kept) {
        let Some(keys) = keys else {
            return Err(Error::unbound(lakelands_api::error::Seam::KeyStore));
        };
        if keys.key(lake, &source, &key)?.is_none() {
            return Err(Error::unavailable(Situation::KeyMissing { source, key }));
        }
    }
    Ok(digests)
}

/// Import the export in `dir` as a new lake at `location` (module doc).
pub fn import(location: &Location, seams: Seams, dir: &Path) -> Result<Imported> {
    let verified = lakelands_core::export::verify(dir)?;
    let files = lakelands_core::export::segment_files(dir)?;
    let digests = check(&files, &location.lake, seams.keys.as_deref())?;
    let opener = crate::lake::opener(location, &seams)?;
    if let Some(ends) = opener.log_ends(location)? {
        crate::log::one_format(&location.lake, Some(&ends))?;
        let (lake, head) = (Box::new(location.lake.clone()), ends.1.position);
        return Err(precondition(Case::ImportNotEmpty { lake, head }));
    }
    let engine: Arc<dyn lakelands_api::engine::LakeEngine> =
        Arc::from(opener.open(location, OpenMode::Create)?);
    // A projection with no log under it is not a place with no lake. It
    // is asked before the lease is taken, as an open asks it: taking the
    // lease may cut the log.
    crate::lake::ends(engine.as_ref())?;
    let lease = engine.take_lease(&format!("lakelands import {}", std::process::id()))?;
    let written = (|| {
        // A lake another writer made since the ends were asked for.
        if let (_, Some((head, _))) = crate::lake::ends(engine.as_ref())? {
            let lake = Box::new(location.lake.clone());
            return Err(precondition(Case::ImportNotEmpty { lake, head }));
        }
        carry(engine.as_ref(), &lease, &files, &digests, location)?;
        let lake = Lake::assemble(location, seams.clone(), engine.clone(), Some(lease.clone()));
        lake.catch_up().map_err(|e| {
            let cause = Box::new(e);
            Error::unavailable(Situation::ImportNotFolded { cause })
        })?;
        // A forgetting the export holds is finished here, as a writer's
        // open would: a key store restored from a backup has the source's
        // keys destroyed again, and no read of the new lake answers what
        // they sealed (058).
        lake.finish_forgetting(&mut lake.writer()).map(drop)
    })();
    let released = engine.release_lease(&lease);
    written?;
    released?;
    drop(engine);
    // An export of an older format is upgraded as any lake is: by a
    // writer's open, whose lease is above every fence the export holds
    // (`docs/log-format.md` §6). Its events stay as they were written.
    let ends = crate::lake::opener(location, &seams)?.log_ends(location)?;
    if crate::log::one_format(&location.lake, ends.as_ref())? < LOG_FORMAT_VERSION {
        Lake::open(location, seams, OpenMode::Write)?.close()?;
    }
    Ok(Imported {
        lake: location.lake.clone(),
        through: verified.through,
        hash: verified.hash,
        events: verified.through.0 + 1,
        files: files.len(),
    })
}

/// Hand every file's events to the engine a batch at a time; the first
/// batch of each file after the first enters with the seal of the file
/// before it.
fn carry(
    engine: &dyn lakelands_api::engine::LakeEngine,
    lease: &Lease,
    files: &[PathBuf],
    digests: &[Digest],
    location: &Location,
) -> Result<()> {
    let mut reached = Position::GENESIS;
    let stopped = |_: &Path, reached: Position, e: Error| {
        let (lake, cause) = (location.lake.to_string(), Box::new(e));
        Error::unavailable(Situation::ImportStopped {
            reached,
            lake,
            cause,
        })
    };
    for (i, path) in files.iter().enumerate() {
        // A file after the first begins a segment: its first events enter
        // with the seal of the one before, whose digest is checked against
        // that file's before anything is sealed.
        let mut seals = i > 0;
        let mut hand = |batch: &[CanonicalEvent]| -> Result<()> {
            if !std::mem::take(&mut seals) {
                return engine.import(lease, batch);
            }
            let before = &digests[i - 1];
            let sealed = engine.seal_segment(lease, true, &mut |sealed| {
                if &sealed.digest == before {
                    return Ok(batch.to_vec());
                }
                let (engine, file) = (sealed.digest.0.clone(), before.0.clone());
                let flaw = Flaw::DigestDiffers { engine, file };
                Err(lakelands_core::damaged(Part::Export, None, flaw))
            })?;
            sealed
                .map(drop)
                .ok_or_else(|| lakelands_core::damaged(Part::Log, None, Flaw::NothingToSeal))
        };
        let (mut batch, mut weighed) = (Vec::new(), 0);
        for event in events(path)? {
            let event = event?.0;
            weighed += weight(&event);
            batch.push(event);
            if weighed >= BATCH {
                hand(&batch).map_err(|e| stopped(path, reached, e))?;
                reached = batch.last().expect("a full window").position.next();
                batch.clear();
                weighed = 0;
            }
        }
        if !batch.is_empty() {
            hand(&batch).map_err(|e| stopped(path, reached, e))?;
            reached = batch.last().expect("not empty").position.next();
        }
    }
    Ok(())
}
