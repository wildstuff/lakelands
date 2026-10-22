//! A handle onto one lake directory: the log and lease, the projection's
//! write side, the vector index.
//!
//! Handles share nothing but the directory: any number of them, in this
//! process or others, see one lake. Each keeps its own read of the log,
//! its own connections to the projection, its open transaction and the
//! lease it took, which it gives up when it is dropped. A handle opened
//! read-only refuses the lease, every append and every seal.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use lakelands_api::engine::{
    Applied, CanonicalEvent, Choose, Declared, LakeEngine, Lease, OpenMode, ProjectionStore,
    ProjectionView, Ranked, Rankings, Reclaimed, StatesOf, VectorIndex,
};
use lakelands_api::error::Result;
use lakelands_api::event::{Digest, SegmentDigest};
use lakelands_api::id::{LakeId, Position, RecordKey, SourceId, TypeName};
use lakelands_api::location::SqliteTuning;
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::write::{Declaration, Proposal};
use lakelands_core::engines::{self, vector_bytes, vector_of};
use rusqlite::{Connection, OpenFlags, params};

use crate::lease::{Held, LeaseFile};
use crate::log::Log;
use crate::projection::{self, MEMBERS, OUTPUTS, PROPOSALS};
use crate::scratch::{self, Building, Mark};
use crate::view::SqliteView;
use lakelands_api::error::{Case, Detail, Doing, Error, Part, Situation};

use crate::{NAME, REBUILD_RATE, io_error, lock, sql};

/// The open transaction of the fold: the latest position a put in it
/// names, which the commit must reach.
#[derive(Default)]
struct Transaction {
    latest: Option<Position>,
    /// The types declared in it, each with the position of its latest
    /// declaration: the commit indexes their older records under it.
    redeclared: BTreeMap<TypeName, Position>,
    /// The types `put_record` indexed a record under: each read once in
    /// the transaction, and all forgotten when a declaration or a
    /// `sort_index.<type>` setting is put, which may change one.
    kept: projection::Kept,
    /// The declarations and the `lookup_index.<type>` settings it put: the
    /// commit decides the exact-text index's fields again for what they move.
    relook: engines::Relook,
}

/// One handle onto a lake kept by the SQLite engine.
pub struct SqliteLake {
    lake: LakeId,
    mode: OpenMode,
    path: PathBuf,
    log: Mutex<Log>,
    lease_file: LeaseFile,
    held: Mutex<Option<Held>>,
    writer: Mutex<Option<Connection>>,
    pub(crate) reader: Mutex<Connection>,
    transaction: Mutex<Option<Transaction>>,
    /// The head's declarations as the last commit left them, and the
    /// position it folded through, for the next one's relook.
    heads: Mutex<Option<(Position, engines::Heads)>>,
    /// The lake's directory and the tuning its connections are made with,
    /// for a swap that connects again.
    dir: PathBuf,
    tuning: SqliteTuning,
    /// On a handle onto the scratch, its lock and mark (`scratch.rs`).
    building: Mutex<Option<Building>>,
}

/// How long a statement waits for another connection's lock.
const BUSY: Duration = Duration::from_secs(10);

/// Open a connection to the projection with the tuning for its role. A
/// reader's connection is read-only: it never creates the file, never
/// changes its journal and never writes a page.
fn connect(path: &std::path::Path, tuning: &SqliteTuning, writer: bool) -> Result<Connection> {
    let flags = if writer {
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let conn =
        Connection::open_with_flags(path, flags | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(sql)?;
    let cache = if writer {
        tuning.writer_cache_kb
    } else {
        tuning.reader_cache_kb
    };
    conn.busy_timeout(BUSY).map_err(sql)?;
    conn.execute_batch(&format!(
        "PRAGMA cache_size = -{cache};
         PRAGMA mmap_size = {};",
        u64::from(tuning.mmap_mb) * 1024 * 1024,
    ))
    .map_err(sql)?;
    if writer {
        // The journal and its synchronisation are fixed, not tuning: the
        // projection is rebuilt from the log, and the fold's crash safety
        // rests on a write-ahead journal committed whole.
        conn.execute_batch(&format!(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA wal_autocheckpoint = {};",
            tuning.wal_autocheckpoint_pages
        ))
        .map_err(sql)?;
    }
    Ok(conn)
}

impl SqliteLake {
    pub(crate) fn open(
        lake: LakeId,
        dir: PathBuf,
        tuning: SqliteTuning,
        mode: OpenMode,
    ) -> Result<SqliteLake> {
        let create = mode == OpenMode::Create;
        // A log that is there is judged before anything is made beside
        // it, and a directory with no segment file holds no lake: only an
        // open to create makes anything in it.
        let mut log = if crate::log::kept(&dir)? {
            let log = Log::open(&dir, false)?;
            check_name(&dir, &lake, create)?;
            log
        } else if create {
            std::fs::create_dir_all(&dir).map_err(|e| io_error(Doing::Create, Part::File, e))?;
            check_name(&dir, &lake, create)?;
            Log::open(&dir, true)?
        } else {
            let (lake, schema) = (lake.to_string(), None);
            return Err(Error::unavailable(Situation::NoLake { lake, schema }));
        };
        // A scratch an earlier builder left unfit to swap in is removed by
        // the next writer, before it opens the projection.
        if mode != OpenMode::Read {
            scratch::sweep(&dir, &lake, &mut log)?;
        }
        let path = dir.join("projection.db");
        let no_projection = || {
            let lake = lake.to_string();
            Error::unavailable(Situation::NoProjection { lake })
        };
        // A handle opened read-only creates and changes nothing in the
        // directory: it needs the projection a writer made.
        let writer = match mode {
            OpenMode::Read if !path.is_file() => return Err(no_projection()),
            OpenMode::Read => None,
            OpenMode::Write | OpenMode::Create => {
                let conn = connect(&path, &tuning, true)?;
                // A projection is read only by the layout it was made
                // with; one that holds no table yet is made with this build's.
                if projection::made(&conn)? {
                    engines::layout(&lake, projection::layout(&conn)?, projection::LAYOUT)?;
                }
                let (schema, mark) = (projection::SCHEMA, projection::mark());
                conn.execute_batch(&format!("BEGIN IMMEDIATE; {schema} {mark} COMMIT;"))
                    .map_err(sql)?;
                // An order index a writer did not keep (a binary that lacks
                // it folded past its mark or wrote its setting) is rebuilt
                // from the head, once.
                projection::catch_up_sorted(&conn)?;
                Some(conn)
            }
            // Whatever layout made the projection, nothing is read from it
            // or made in it before `clear` makes it anew.
            OpenMode::Rebuild => Some(connect(&path, &tuning, true)?),
        };
        let reader = connect(&path, &tuning, false)?;
        // Between a maker's connect and its tables a reader finds no
        // table: no projection yet, as the moment before.
        if mode == OpenMode::Read && !projection::made(&reader)? {
            return Err(no_projection());
        }
        if mode != OpenMode::Rebuild {
            engines::layout(&lake, projection::layout(&reader)?, projection::LAYOUT)?;
        }
        Ok(SqliteLake {
            lease_file: LeaseFile::new(&dir),
            lake,
            mode,
            path,
            log: Mutex::new(log),
            held: Mutex::new(None),
            writer: Mutex::new(writer),
            reader: Mutex::new(reader),
            transaction: Mutex::new(None),
            heads: Mutex::new(None),
            dir,
            tuning,
            building: Mutex::new(None),
        })
    }

    /// A handle onto the scratch of the lake in `dir`: it reads the log as
    /// a reader does, and folds into the scratch, never the projection.
    /// A scratch unfit to keep is removed and made anew.
    pub(crate) fn open_scratch(
        lake: LakeId,
        dir: PathBuf,
        tuning: SqliteTuning,
    ) -> Result<SqliteLake> {
        if !crate::log::kept(&dir)? {
            let (lake, schema) = (lake.to_string(), None);
            return Err(Error::unavailable(Situation::NoLake { lake, schema }));
        }
        let mut log = Log::open(&dir, false)?;
        check_name(&dir, &lake, false)?;
        let mut building = scratch::lock(&dir)?.ok_or_else(|| scratch::rebuilding(&lake))?;
        if building.fit(&lake, &mut log, false)?.is_none() {
            building.remove()?;
        }
        let path = building.path();
        let conn = connect(&path, &tuning, true)?;
        let (schema, mark) = (projection::SCHEMA, projection::mark());
        conn.execute_batch(&format!("BEGIN IMMEDIATE; {schema} {mark} COMMIT;"))
            .map_err(sql)?;
        projection::catch_up_sorted(&conn)?;
        let reader = connect(&path, &tuning, false)?;
        Ok(SqliteLake {
            lease_file: LeaseFile::new(&dir),
            lake,
            mode: OpenMode::Read,
            path,
            log: Mutex::new(log),
            held: Mutex::new(None),
            writer: Mutex::new(Some(conn)),
            reader: Mutex::new(reader),
            transaction: Mutex::new(None),
            heads: Mutex::new(None),
            dir,
            tuning,
            building: Mutex::new(Some(building)),
        })
    }

    /// Close this handle's connections to its projection, so that its file
    /// can be replaced or removed; the reader is left on an empty database
    /// until [`reconnect`](Self::reconnect).
    fn disconnect(&self) -> Result<()> {
        *lock(&self.transaction) = None;
        *lock(&self.heads) = None;
        drop(lock(&self.writer).take());
        *lock(&self.reader) = Connection::open_in_memory().map_err(sql)?;
        Ok(())
    }

    /// Connect this handle to its projection again, after
    /// [`disconnect`](Self::disconnect).
    fn reconnect(&self) -> Result<()> {
        *lock(&self.writer) = Some(connect(&self.path, &self.tuning, true)?);
        *lock(&self.reader) = connect(&self.path, &self.tuning, false)?;
        Ok(())
    }

    /// Put the scratch in the projection's place, checked as
    /// [`LakeEngine::swap`] says. The projection is taken out of its
    /// write-ahead journal first, which SQLite allows only to the one
    /// connection that has it open: so no other handle reads it when it is
    /// replaced. The rename is one step of the directory, synced.
    fn swapping(&self, lease: &Lease) -> Result<Option<Position>> {
        let _held = self.check_lease(lease)?;
        let mut building =
            scratch::lock(&self.dir)?.ok_or_else(|| scratch::rebuilding(&self.lake))?;
        let no_scratch = || {
            Error::unavailable(Situation::NoScratch {
                lake: self.lake.to_string(),
            })
        };
        let mut log = lock(&self.log);
        log.refresh()?;
        let mark = building
            .fit(&self.lake, &mut log, false)?
            .ok_or_else(no_scratch)?;
        let tip = log.tip();
        drop(log);
        if Mark::of_tip(&self.lake, tip.as_ref()) != mark {
            return Err(Error::unavailable(Situation::ScratchBehind {
                lake: self.lake.to_string(),
                marked: mark.position.map(Position),
                tip: tip.map(|(p, _)| p),
            }));
        }
        building.settle(&self.lake)?;
        self.disconnect()?;
        let replaced = (|| {
            let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
            let live = Connection::open_with_flags(&self.path, flags).map_err(sql)?;
            live.busy_timeout(Duration::ZERO).map_err(sql)?;
            let mode = live.query_row("PRAGMA journal_mode = DELETE", [], |r| {
                r.get::<_, String>(0)
            });
            if mode.ok().as_deref() != Some("delete")
                || live.execute_batch("BEGIN EXCLUSIVE").is_err()
            {
                let lake = self.lake.to_string();
                return Err(Error::unavailable(Situation::SwapBusy { lake }));
            }
            for end in ["-wal", "-shm"] {
                let _ = std::fs::remove_file(self.dir.join(format!("projection.db{end}")));
            }
            std::fs::rename(building.path(), &self.path)
                .and_then(|()| crate::log::sync_dir(&self.dir))
                .map_err(|e| io_error(Doing::Rename, Part::Projection, e))?;
            drop(live);
            building.set(None)
        })();
        self.reconnect()?;
        replaced?;
        drop(building);
        projection::folded(&lock(&self.reader))
    }

    /// Run `f` on the writer connection; a handle opened read-only has
    /// none, and writes nothing.
    fn write<R>(&self, f: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
        let writer = lock(&self.writer);
        match writer.as_ref() {
            Some(conn) => f(conn),
            None => Err(self.read_only()),
        }
    }

    /// The refusal of a write through a handle opened read-only.
    fn read_only(&self) -> Error {
        let (lake, leased_elsewhere) = (Some(self.lake.to_string()), false);
        Error::unavailable(Situation::ReadOnly {
            lake,
            leased_elsewhere,
        })
    }

    /// Refuses a write to the log through a handle opened read-only.
    fn writable(&self) -> Result<()> {
        match self.mode {
            OpenMode::Read => Err(self.read_only()),
            OpenMode::Write | OpenMode::Create | OpenMode::Rebuild => Ok(()),
        }
    }

    /// Refuses a handle opened read-only, and a lease that is not the one
    /// this handle holds, or whose fencing token the lease file no longer
    /// records: the lock can be lost without the process knowing, and the
    /// token is what says so.
    ///
    /// The answer is the lease this handle holds, locked: while the caller
    /// keeps it, this handle cannot give the lease up, so the check and
    /// what the caller then cuts and writes are one step. The lease
    /// file's lock, which the lease held keeps from `take_lease` to its
    /// release, lets no other handle take the lease meanwhile.
    fn check_lease(&self, lease: &Lease) -> Result<MutexGuard<'_, Option<Held>>> {
        self.writable()?;
        let held = lock(&self.held);
        let (fence, holder) = self.lease_file.current()?;
        if held.as_ref().is_some_and(|h| h.lease == *lease) && fence == lease.fence {
            return Ok(held);
        }
        let named = !holder.is_empty();
        Err(Error::unavailable(Situation::LeaseStale {
            fence: lease.fence,
            holder: named.then_some(holder),
            current: named.then_some(fence),
            undone: None,
        }))
    }

    /// Run `write`, which adds imported events to the log and answers the
    /// last one's position and fence: refused unless the log holds nothing
    /// but imported events, and what it added is recorded in the lease
    /// file.
    fn importing<R>(
        &self,
        lease: &Lease,
        write: impl FnOnce(&mut Log) -> Result<(R, Option<(u64, u64)>)>,
    ) -> Result<R> {
        let mut held = self.check_lease(lease)?;
        let held = held.as_mut().ok_or_else(|| {
            let (fence, undone) = (None, None);
            Error::unavailable(Situation::LeaseNotHeld { fence, undone })
        })?;
        let mut log = lock(&self.log);
        log.refresh()?;
        if let Some((tip, _)) = log.tip()
            && self.lease_file.imported(held)? != Some(tip.0)
        {
            return Err(Error::invalid(Case::ImportIntoHeld { tip }));
        }
        let (answer, last) = write(&mut log)?;
        if let Some((position, fence)) = last {
            self.lease_file.record_import(held, position, fence)?;
        }
        Ok(answer)
    }

    /// The projection's storage in bytes: `projection.db` and its
    /// write-ahead journal.
    fn size(&self) -> Result<u64> {
        let wal = self.path.with_extension("db-wal");
        [&self.path, &wal]
            .into_iter()
            .try_fold(0, |sum, p| match std::fs::metadata(p) {
                Ok(m) => Ok(sum + m.len()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(sum),
                Err(e) => Err(io_error(Doing::Measure, Part::File, e)),
            })
    }

    /// Hold a put for the open transaction.
    fn stage(
        &self,
        position: Position,
        f: impl FnOnce(&Connection, &mut Transaction) -> Result<()>,
    ) -> Result<()> {
        let mut transaction = lock(&self.transaction);
        let t = transaction.as_mut().ok_or_else(engines::unstaged)?;
        t.latest = t.latest.max(Some(position));
        self.write(|conn| f(conn, t))
    }
}

/// The lake a directory holds is named in its `lake` file; a directory
/// opened under another name is refused.
fn check_name(dir: &std::path::Path, lake: &LakeId, create: bool) -> Result<()> {
    let path = dir.join("lake");
    match std::fs::read_to_string(&path) {
        Ok(name) if name.trim_end() == lake.as_str() => Ok(()),
        Ok(name) => {
            let (schema, held, lake) = (None, name.trim_end().to_owned(), Box::new(lake.clone()));
            Err(Error::invalid(Case::LakeOther { schema, held, lake }))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
            std::fs::write(&path, format!("{lake}\n"))
                .map_err(|e| io_error(Doing::Write, Part::File, e))
        }
        Err(e) => Err(io_error(Doing::Read, Part::File, e)),
    }
}

impl Drop for SqliteLake {
    /// A handle that is dropped gives up the lease it holds, as a process
    /// that ends does, and a handle onto the scratch removes it unless it
    /// is marked built through the position it is folded through.
    fn drop(&mut self) {
        if let Some(held) = lock(&self.held).take() {
            let _ = self.lease_file.release(held);
        }
        if let Some(mut building) = lock(&self.building).take() {
            let _ = self.disconnect();
            let log = self
                .log
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !matches!(building.fit(&self.lake, log, false), Ok(Some(_))) {
                let _ = building.remove();
            }
        }
    }
}

impl LakeEngine for SqliteLake {
    fn name(&self) -> &str {
        NAME
    }

    fn rebuild_rate(&self) -> u64 {
        REBUILD_RATE
    }

    fn lake(&self) -> &LakeId {
        &self.lake
    }

    fn take_lease(&self, holder: &str) -> Result<Lease> {
        self.writable()?;
        let mut held = lock(&self.held);
        if let Some(h) = held.as_ref() {
            return Err(engines::lease_held(&h.lease.holder, h.lease.fence));
        }
        let taken = self.lease_file.take(holder)?;
        // What a writer that ended mid-append left uncommitted is cut off
        // before this one appends, and its last group is written again.
        let mut log = lock(&self.log);
        log.recover()?;
        log.settle()?;
        log.sweep()?;
        let lease = taken.lease.clone();
        *held = Some(taken);
        Ok(lease)
    }

    fn release_lease(&self, lease: &Lease) -> Result<()> {
        let mut held = lock(&self.held);
        if held.as_ref().is_some_and(|h| h.lease == *lease) {
            // The writer leaves an empty journal behind. It does not wait
            // for a reader that holds a snapshot of it, and a journal it
            // could not truncate is no failure of the release: the next
            // release, or `compact`, truncates it.
            let _ = self.write(|conn| {
                conn.busy_timeout(Duration::ZERO).map_err(sql)?;
                let truncated = truncate_journal(conn);
                conn.busy_timeout(BUSY).map_err(sql)?;
                truncated
            });
            self.lease_file
                .release(held.take().expect("checked above"))?;
        }
        Ok(())
    }

    fn tip(&self) -> Result<Option<(Position, Digest)>> {
        let mut log = lock(&self.log);
        log.refresh()?;
        Ok(log.tip())
    }

    fn append(&self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        let _held = self.check_lease(lease)?;
        lock(&self.log).append(Some(lease.fence), events)
    }

    fn import(&self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        self.importing(lease, |log| {
            log.append(None, events)?;
            Ok(((), events.last().map(|e| (e.position.0, e.fence))))
        })
    }

    fn read(&self, from: Position, limit: u32) -> Result<Vec<CanonicalEvent>> {
        lock(&self.log).read(from, limit)
    }

    fn seal_segment(
        &self,
        lease: &Lease,
        imported: bool,
        first: &mut dyn FnMut(&SegmentDigest) -> Result<Vec<CanonicalEvent>>,
    ) -> Result<Option<SegmentDigest>> {
        if !imported {
            let _held = self.check_lease(lease)?;
            return lock(&self.log).seal(Some(lease.fence), first);
        }
        self.importing(lease, |log| {
            let mut last = None;
            let sealed = log.seal(None, &mut |sealed| {
                let events = first(sealed)?;
                last = events.last().map(|e| (e.position.0, e.fence));
                Ok(events)
            })?;
            Ok((sealed, last))
        })
    }

    fn segments(&self) -> Result<Vec<SegmentDigest>> {
        lock(&self.log).segments()
    }

    fn projection(&self) -> &dyn ProjectionStore {
        self
    }

    fn view(&self, as_of: Option<Position>) -> Result<Box<dyn ProjectionView + '_>> {
        let folded = projection::folded(&lock(&self.reader))?;
        let at = engines::view_at(as_of, folded)?;
        Ok(Box::new(SqliteView { lake: self, at }))
    }

    /// One read transaction holds the position and every list.
    fn ranked(&self, ty: &TypeName, asked: &Rankings, k: u32) -> Result<Ranked> {
        let conn = lock(&self.reader);
        conn.execute_batch("BEGIN").map_err(sql)?;
        let ranked = projection::folded(&conn).and_then(|position| {
            let mut lists = Vec::new();
            if let Some((text, fields)) = &asked.text {
                let (fields, weights) = (fields.as_deref(), &asked.text_weights);
                lists.push(crate::view::lexical(&conn, ty, text, fields, weights, k)?);
            }
            for (field, query) in &asked.vectors {
                lists.push(nearest(&conn, ty, field, query, k)?);
            }
            Ok(Ranked { position, lists })
        });
        let _ = conn.execute_batch("COMMIT");
        ranked
    }

    fn vectors(&self) -> Option<&dyn VectorIndex> {
        Some(self)
    }

    /// Checkpoints the scratch's journal into its file, then writes the
    /// mark: a mark that is there names a scratch whole through it.
    fn mark_built(&self) -> Result<Option<Position>> {
        let mut building = lock(&self.building);
        let Some(building) = building.as_mut() else {
            let lake = self.lake.to_string();
            return Err(Error::unavailable(Situation::NoScratch { lake }));
        };
        *lock(&self.transaction) = None;
        let folded = self.write(|conn| {
            abandon(conn)?;
            if !truncate_journal(conn)? {
                let lake = self.lake.to_string();
                return Err(Error::unavailable(Situation::Busy { lake }));
            }
            projection::folded(conn)
        })?;
        let at = match folded {
            Some(p) => {
                let at = lock(&self.log)
                    .read(p, 1)?
                    .pop()
                    .filter(|e| e.position == p);
                let tip = || lock(&self.log).tip().map(|(p, _)| p);
                let lake = self.lake.to_string();
                Some(at.ok_or_else(|| {
                    Error::unavailable(Situation::ScratchBehind {
                        lake,
                        marked: folded,
                        tip: tip(),
                    })
                })?)
            }
            None => None,
        };
        building.set(Some(&Mark::of(&self.lake, at.as_ref())))?;
        Ok(folded)
    }

    fn swap(&self, lease: &Lease) -> Result<Option<Position>> {
        self.swapping(lease)
    }
}

impl ProjectionStore for SqliteLake {
    /// Opens a transaction. Beginning while one is open abandons the open
    /// one, as a rollback would.
    fn begin(&self) -> Result<()> {
        let mut transaction = lock(&self.transaction);
        self.write(|conn| {
            abandon(conn)?;
            conn.execute_batch("BEGIN IMMEDIATE").map_err(sql)
        })?;
        *transaction = Some(Transaction::default());
        Ok(())
    }

    fn commit(&self, through: Position) -> Result<()> {
        let mut transaction = lock(&self.transaction);
        let t = transaction
            .take()
            .ok_or_else(|| Error::invalid(Case::CommitUnbegun))?;
        let kept = lock(&self.heads).take();
        let heads = self.write(|conn| {
            let folded = projection::folded(conn)?;
            if let Some(refused) = engines::commit_refusal(folded, t.latest, through) {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(refused);
            }
            let held = |(at, _): &(Position, _)| Some(*at) == folded;
            let mut heads = kept.filter(held).map(|(_, heads)| heads);
            if t.relook.any() {
                let read = || projection::head_declarations(conn);
                let (after, moved) = t.relook.heads(heads, read)?;
                projection::relook(conn, &after, moved.as_ref())?;
                heads = Some(after);
            }
            for (ty, at) in &t.redeclared {
                projection::index_older(conn, ty, *at)?;
            }
            projection::set_folded(conn, through)?;
            projection::mark_sorted(conn, folded, through)?;
            conn.execute_batch("COMMIT").map_err(sql)?;
            Ok(heads)
        })?;
        *lock(&self.heads) = heads.map(|heads| (through, heads));
        Ok(())
    }

    fn rollback(&self) {
        *lock(&self.transaction) = None;
        let _ = self.write(abandon);
    }

    fn folded_through(&self) -> Result<Option<Position>> {
        projection::folded(&lock(&self.reader))
    }

    fn put_declaration(
        &self,
        declaration: &Declaration,
        version: u32,
        schema_hash: Option<&str>,
        at: Position,
    ) -> Result<()> {
        let d = Declared {
            declaration: declaration.clone(),
            version,
            schema_hash: schema_hash.map(str::to_owned),
            since: at,
        };
        self.stage(at, |conn, t| {
            t.kept.clear();
            t.relook.declared(&d.declaration);
            let redeclared = projection::put_declaration(conn, &d)?;
            t.redeclared.extend(redeclared.map(|ty| (ty, at)));
            Ok(())
        })
    }

    fn put_record(&self, record: &StoredRecord) -> Result<()> {
        self.stage(record.position, |conn, t| {
            t.relook.record(record);
            projection::put_record(conn, &mut t.kept, record)
        })
    }

    fn end_head_only(&self, name: &TypeName, at: Position) -> Result<()> {
        engines::head_only(name)?;
        self.stage(at, |conn, _| {
            let gone = conn.execute("DELETE FROM records WHERE ty = ?1", [name.as_str()]);
            gone.map(drop).map_err(sql)
        })
    }

    fn drop_states(&self, of: StatesOf<'_>) -> Result<()> {
        self.stage(Position::GENESIS, |conn, _| match of {
            StatesOf::Record(ty, key) => projection::drop_record(conn, ty, key),
            StatesOf::Output(seam, input) => projection::drop_output(conn, seam, &input.0),
        })
    }

    fn put_proposal(&self, proposal: &Proposal, at: Position) -> Result<()> {
        let state = serde_json::to_string(proposal).expect("a proposal serialises");
        self.stage(at, |conn, _| {
            PROPOSALS
                .put(conn, &[&proposal.id.as_str()], at, &state)
                .map(drop)
        })
    }

    fn put_membership(
        &self,
        set: &str,
        ty: &TypeName,
        key: &RecordKey,
        member: bool,
        at: Position,
    ) -> Result<()> {
        self.stage(at, |conn, _| {
            MEMBERS
                .put(conn, &[&set, &ty.as_str(), &key.as_str()], at, &member)
                .map(drop)
        })
    }

    fn put_output(&self, seam: &str, input: &Digest, output: &Value, at: Position) -> Result<()> {
        let state = serde_json::to_string(output).expect("a value serialises");
        self.stage(at, |conn, _| {
            OUTPUTS
                .put(conn, &[&seam, &input.0.as_str()], at, &state)
                .map(drop)
        })
    }

    fn mark_applied(&self, applied: &Applied) -> Result<()> {
        let at = applied.at.0 as i64;
        self.stage(applied.at, |conn, _| {
            conn.prepare_cached("INSERT OR IGNORE INTO applied (id, at) VALUES (?1, ?2)")
                .and_then(|mut s| s.execute(params![applied.id.0, at]))
                .map_err(sql)?;
            let mut route = conn
                .prepare_cached(
                    "INSERT INTO routes (at, kind, ty, key, source) VALUES (?1, ?2, ?3, ?4, ?5)",
                )
                .map_err(sql)?;
            let source = applied.source.as_ref().map(SourceId::as_str);
            for a in &applied.about {
                let (ty, key) = (a.ty.as_ref().map(TypeName::as_str), a.key.as_ref());
                let key = key.map(RecordKey::as_str);
                route
                    .execute(params![at, applied.kind, ty, key, source])
                    .map_err(sql)?;
            }
            Ok(())
        })
    }

    /// Drops the projection, its kept history and the vector index, and
    /// makes the projection's tables again at this build's layout, marked
    /// with it; the log and the lease stay. An open transaction is
    /// abandoned.
    fn clear(&self) -> Result<()> {
        *lock(&self.transaction) = None;
        self.write(|conn| {
            abandon(conn)?;
            let dropped: String = projection::TABLES
                .iter()
                .map(|t| format!("DROP TABLE IF EXISTS {t};"))
                .collect();
            let (schema, mark) = (projection::SCHEMA, projection::mark());
            conn.execute_batch(&format!(
                "BEGIN IMMEDIATE; {dropped} {schema} {mark} COMMIT;"
            ))
            .map_err(sql)
        })
    }

    /// Truncates the write-ahead journal, rewrites `projection.db` without
    /// its free pages (`VACUUM`), and truncates the journal again. The log
    /// is not touched. `VACUUM` is one transaction: when it fails, the
    /// projection is as it was.
    fn compact(&self) -> Result<Reclaimed> {
        let transaction = lock(&self.transaction);
        if transaction.is_some() {
            return Err(Error::invalid(Case::CompactInTransaction));
        }
        self.write(|conn| {
            // A transaction a failed commit left open is abandoned, as
            // `begin` and `clear` abandon it.
            abandon(conn)?;
            let before = self.size()?;
            if !truncate_journal(conn)? {
                let lake = self.lake.to_string();
                return Err(Error::unavailable(Situation::Busy { lake }));
            }
            conn.execute_batch("VACUUM").map_err(|e| {
                let (lake, detail) = (self.lake.to_string(), Detail::scrubbed(crate::told(&e)));
                Error::unavailable(Situation::CompactFailed { lake, detail })
            })?;
            // The rewrite is committed. A reader that holds a snapshot now
            // keeps the journal from being truncated, and the journal still
            // holds the pages the rewrite freed: the compact answers busy,
            // so what is owed of it (058) stays owed, and the next one
            // truncates. No answer has changed.
            if !truncate_journal(conn)? {
                let lake = self.lake.to_string();
                return Err(Error::unavailable(Situation::Busy { lake }));
            }
            Ok(Reclaimed {
                before: Some(before),
                after: Some(self.size()?),
            })
        })
    }

    /// One step of a chosen order index, in a transaction of its own
    /// (`projection::choose_order`), outside the fold's.
    fn choose_order(&self, ty: &TypeName, step: &Choose) -> Result<Option<RecordKey>> {
        let transaction = lock(&self.transaction);
        if transaction.is_some() {
            return Err(Error::invalid(Case::CompactInTransaction));
        }
        self.write(|conn| {
            abandon(conn)?;
            projection::choose_order(conn, ty, step)
        })
    }
}

/// Roll back the transaction `conn` holds open, if any.
fn abandon(conn: &Connection) -> Result<()> {
    if !conn.is_autocommit() {
        conn.execute_batch("ROLLBACK").map_err(sql)?;
    }
    Ok(())
}

/// Checkpoint the write-ahead journal into `projection.db` and truncate
/// it; `false` when a reader holding a snapshot past the busy timeout kept
/// it from being truncated. Either way no answer has changed.
fn truncate_journal(conn: &Connection) -> Result<bool> {
    let busy: i64 = conn
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))
        .map_err(sql)?;
    Ok(busy == 0)
}

/// The `k` vectors of one field nearest to `query`, read through `conn`,
/// by brute force with the one cosine every engine uses
/// (`lakelands_api::query::cosine`), so its answers are those of every
/// other engine to the last bit. It steps through every vector of the
/// field and keeps only the best `k` (`lakelands_core::engines::TopK`), so
/// its cost grows linearly with them and what it holds does not: the
/// ceiling `docs/seams.md` states.
fn nearest(
    conn: &Connection,
    ty: &TypeName,
    field: &str,
    query: &[f32],
    k: u32,
) -> Result<Vec<(RecordKey, f64)>> {
    let mut stmt = conn
        .prepare_cached("SELECT key, data FROM vectors WHERE ty = ?1 AND field = ?2 ORDER BY key")
        .map_err(sql)?;
    let rows = stmt
        .query_map(params![ty.as_str(), field], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        })
        .map_err(sql)?;
    let mut top = lakelands_core::engines::TopK::new(k);
    for row in rows {
        let (key, data) = row.map_err(sql)?;
        let v = vector_of(&data);
        if v.len() != query.len() {
            return Err(engines::dimensions(query.len(), v.len(), ty, field));
        }
        top.push(
            &RecordKey::from(key),
            lakelands_api::query::cosine(query, &v),
        );
    }
    Ok(top.into_ranked())
}

/// The vector index's write side and its size: each field's vectors in
/// one table, indexed at the head only, in the fold's transaction.
impl VectorIndex for SqliteLake {
    fn put(&self, ty: &TypeName, key: &RecordKey, field: &str, vector: &[f32]) -> Result<()> {
        self.write(|conn| {
            projection::run(
                conn,
                "INSERT INTO vectors (ty, field, key, data) VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT (ty, field, key) DO UPDATE SET data = ?4",
                params![ty.as_str(), field, key.as_str(), vector_bytes(vector)],
            )
        })
    }

    fn remove(&self, ty: &TypeName, key: &RecordKey, field: &str) -> Result<()> {
        self.write(|conn| {
            conn.prepare_cached("DELETE FROM vectors WHERE ty = ?1 AND field = ?2 AND key = ?3")
                .and_then(|mut s| s.execute(params![ty.as_str(), field, key.as_str()]))
                .map(|_| ())
                .map_err(sql)
        })
    }

    fn count(&self, ty: &TypeName, field: &str) -> Result<u64> {
        lock(&self.reader)
            .prepare_cached("SELECT count(*) FROM vectors WHERE ty = ?1 AND field = ?2")
            .and_then(|mut s| s.query_row(params![ty.as_str(), field], |r| r.get::<_, i64>(0)))
            .map(|n| n as u64)
            .map_err(sql)
    }
}
