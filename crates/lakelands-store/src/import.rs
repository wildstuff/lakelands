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
//! it, the one format, no member where a reader refuses it: an ingest
//! sealed under another source's key among them), then read once more
//! for what only an import needs: every event belongs to the lake the
//! location names, its `id` is of the form `docs/log-format.md` §3 gives
//! it, which a reader does not judge and an engine keeps as its bytes,
//! and every fence is the one its writer held
//! (`docs/log-format.md` §4.1: 0 at position 0, each `lease_taken`
//! carrying the fence it names, every other event the fence of the
//! `lease_taken` before it). A fault in the files refuses the
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
//! **Then the events, as they are.** Each file's events are restored from
//! its lines and frames (`docs/log-format.md` §1.2) and appended as the
//! lake it came from appended them: each frame of the export is one
//! append, and each event no frame covers one of its own, framed as the
//! store frames one, so the imported lake's feed tells the same appends
//! (083) and the stored form is the new lake's own. The first file's
//! events go to `LakeEngine::import` in batches of at most [`BATCH`]
//! bytes, each append whole, one larger than that alone, so what an
//! import holds follows the copy's largest append. Each later file starts with its `checkpoint`:
//! its first batch is handed to `LakeEngine::seal_segment`, which closes
//! the segment before it and begins the next with that batch in one step,
//! once the digest the engine takes of that segment, over its restored
//! events, is the file's; the
//! rest follows through `import`.
//! The projection is folded under the import's lease as a rebuild folds
//! it, and nothing is appended: the head is the export's. No segment is
//! sealed by size, the re-open that upgrades an older export included.
//! The lease is given up, and the host opens the lake as always; that
//! open takes a lease greater than every fence in the log and continues
//! it, and may seal a segment past its bound
//! (`docs/decisions/074-a-segment-is-sealed-by-size.md`).
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
use serde::Deserialize as _;
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

/// Hands `appends` on in batches of at most `limit` bytes by [`weight`],
/// each append whole: a batch is handed before the append that would take
/// it over, so only an append heavier than `limit` makes a larger one,
/// and goes alone.
fn batched(
    appends: impl Iterator<Item = Result<Vec<CanonicalEvent>>>,
    limit: usize,
    hand: &mut dyn FnMut(&[CanonicalEvent]) -> Result<()>,
) -> Result<()> {
    let (mut batch, mut weighed) = (Vec::new(), 0);
    for append in appends {
        let append = append?;
        let weighs: usize = append.iter().map(weight).sum();
        if !batch.is_empty() && weighed + weighs > limit {
            hand(&batch)?;
            batch.clear();
            weighed = 0;
        }
        weighed += weighs;
        batch.extend(append);
    }
    if batch.is_empty() {
        return Ok(());
    }
    hand(&batch)
}

/// The appends of a file's `events`, each the events one frame of the
/// export covers or one event no frame covers, framed as the store's
/// writer frames an append: an import keeps the appends the lake it came
/// from wrote, as its feed tells them (083).
fn appends(
    mut events: impl Iterator<Item = Result<(CanonicalEvent, Position)>>,
) -> impl Iterator<Item = Result<Vec<CanonicalEvent>>> {
    std::iter::from_fn(move || {
        let mut append = Vec::new();
        for event in events.by_ref() {
            let (event, end) = match event {
                Ok(read) => read,
                Err(e) => return Some(Err(e)),
            };
            let last = event.position >= end;
            append.push(event);
            if last {
                break;
            }
        }
        if append.is_empty() {
            return None;
        }
        lakelands_core::log::frame_append(&mut append);
        Some(Ok(append))
    })
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
    Error::refused(None, lakelands_api::Refusal::Precondition(said))
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
/// the versions of its key and the keys sealed apart the latter destroyed.
#[derive(serde::Deserialize)]
struct Body {
    kind: String,
    #[serde(default)]
    fence: Option<u64>,
    #[serde(default)]
    source: Option<SourceId>,
    #[serde(default)]
    keys: Vec<lakelands_api::id::KeyId>,
    #[serde(default)]
    apart: Vec<lakelands_api::id::KeyId>,
}

/// Of an event's `sealed`, the version of the key and the source of a
/// recorded output.
#[derive(serde::Deserialize)]
struct Sealed {
    key: lakelands_api::id::KeyId,
    #[serde(default)]
    source: Option<SourceId>,
    /// The keys of a delivery sealed apart it is sealed under too (065).
    #[serde(default)]
    apart: Vec<lakelands_api::id::KeyId>,
}

/// An event of an export as [`events`] reads it.
type Read = (CanonicalEvent, Envelope, Position);

/// Each event of `path`, restored from its line and its frame
/// (`docs/log-format.md` §1.2), as the event the store would append, its
/// bytes the event's canonical bytes and framed by none, with its
/// envelope and the last position its frame covers, its own where none
/// covers it, in order.
fn events(path: &Path) -> Result<impl Iterator<Item = Result<Read>> + '_> {
    let file = fs::File::open(path).map_err(|e| io(Doing::Open, path, e))?;
    let (mut lines, mut before) = (
        lakelands_core::log::Lines::default(),
        None::<(Position, Digest)>,
    );
    let read = BufReader::new(file).split(b'\n').map(move |line| {
        let line = line.map_err(|e| io(Doing::Read, path, e))?;
        let damaged = |flaw| lakelands_core::damaged(Part::Export, None, flaw);
        let value = lakelands_core::log::line_of(&line).map_err(damaged)?;
        let after = before.as_ref().map(|(p, h)| (*p, h));
        let Some((value, _)) = lines.line(value, after).map_err(damaged)? else {
            return Ok(None);
        };
        // The parser's own words, which name the member it does not know:
        // an import's caller is the operator, and the export is its own.
        let e = Envelope::deserialize(&value).map_err(|err| {
            let detail = Detail::scrubbed(err);
            damaged(Flaw::NotAnEvent { detail })
        })?;
        before = Some((e.position, e.hash.clone()));
        let stored = CanonicalEvent {
            position: e.position,
            id: e.id.clone(),
            prev: e.prev.clone(),
            hash: e.hash.clone(),
            fence: e.fence,
            bytes: lakelands_core::canonical::to_vec(&value),
            frame: None,
        };
        let end = Position(e.position.0 + lines.left());
        Ok(Some((stored, e, end)))
    });
    Ok(read.filter_map(Result::transpose))
}

/// What only an import checks, over the whole export, before anything is
/// written: every event is `lake`'s, and every fence is its writer's; and
/// every key a sealed value names, of a source the export does not forget,
/// is one the bound key store gives. With none bound such a value is
/// refused as unbound, and a key the store does not give as missing: the
/// value would be lost (058). A forgotten source's values arrive erased,
/// and so do those under a key an act of removal destroyed, a version or
/// one of a delivery sealed apart, though their other keys are gone too
/// (065).
/// Answers each file's digest, over the events it restores.
fn check(
    files: &[PathBuf],
    lake: &LakeId,
    keys: Option<&dyn lakelands_api::seam::KeyStore>,
) -> Result<Vec<Digest>> {
    let (digests, needed) = scan(files, lake)?;
    for (source, key) in needed {
        let Some(keys) = keys else {
            return Err(Error::unbound(lakelands_api::error::Seam::KeyStore));
        };
        if keys.key(lake, &source, &key)?.is_none() {
            return Err(Error::unavailable(Situation::KeyMissing { source, key }));
        }
    }
    Ok(digests)
}

/// Every event in the export in `dir` checked as an import checks it, for
/// `lake`, without a key store: what `restore --check` runs after `verify`.
pub fn check_events(dir: &Path, lake: &LakeId) -> Result<()> {
    scan(&lakelands_core::export::segment_files(dir)?, lake).map(drop)
}

type Needed = BTreeSet<(SourceId, lakelands_api::id::KeyId)>;

/// Each file's digest, and the keys the export's sealed values need.
fn scan(files: &[PathBuf], lake: &LakeId) -> Result<(Vec<Digest>, Needed)> {
    let (mut sealed, mut forgot, mut shredded) =
        (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    let mut fence = 0;
    let mut digests = Vec::with_capacity(files.len());
    for path in files {
        // Over the events it restores, as an engine digests a segment
        // (§4.2), hashed as they are read, so a segment is never held whole.
        let mut hasher = Sha256::new();
        for event in events(path)? {
            let (stored, event, _) = event?;
            hasher.update(&stored.bytes);
            hasher.update(b"\n");
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
                let keys: Vec<_> = std::iter::once(&s.key).chain(&s.apart).cloned().collect();
                sealed.insert((source, keys));
            }
            if event.body.kind == "source_forgotten" {
                forgot.extend(event.body.source.clone());
            }
            // A version an act of removal destroyed arrives erased (065).
            if event.body.kind == "values_removed"
                && let Some(source) = &event.body.source
            {
                let taken = (event.body.keys.iter()).chain(&event.body.apart);
                shredded.extend(taken.map(|k| (source.clone(), k.clone())));
            }
            // PostgreSQL keeps an id as its 16 bytes (086); a log another
            // writer made may hold one of another form.
            if !event.id.of_form() {
                let flaw = Flaw::IdNotOfForm;
                let at = Some(event.position);
                return Err(lakelands_core::damaged(Part::Export, at, flaw));
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
        digests.push(Digest(lakelands_core::canonical::hex(&hasher.finalize())));
    }
    let kept = |(s, ks): &(SourceId, Vec<lakelands_api::id::KeyId>)| {
        !forgot.contains(s)
            && !ks
                .iter()
                .any(|k| shredded.contains(&(s.clone(), k.clone())))
    };
    let needed: BTreeSet<_> = (sealed.into_iter().filter(kept))
        .flat_map(|(s, ks)| ks.into_iter().map(move |k| (s.clone(), k)))
        .collect();
    Ok((digests, needed))
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
        let mut lake = Lake::assemble(location, seams.clone(), engine.clone(), Some(lease.clone()));
        let lake = lake.turn();
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
    // (`docs/log-format.md` §6). Its events stay as they were written,
    // and it seals nothing by size: the head is the export's (074).
    let ends = crate::lake::opener(location, &seams)?.log_ends(location)?;
    if crate::log::one_format(&location.lake, ends.as_ref())? < LOG_FORMAT_VERSION {
        let mut location = location.clone();
        location.limits.seal_after_bytes = 0;
        Lake::open(&location, seams, OpenMode::Write)?
            .turn()
            .close()?;
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
        // Each batch holds whole appends, each framed as the store's
        // writer frames one.
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
        let read = events(path)?.map(|event| event.map(|(event, _, end)| (event, end)));
        batched(appends(read), BATCH, &mut |batch| {
            hand(batch).map_err(|e| stopped(path, reached, e))?;
            reached = batch.last().expect("not empty").position.next();
            Ok(())
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
