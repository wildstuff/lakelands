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
    CanonicalEvent, Declared, LakeEngine, Lease, OpenMode, ProjectionStore, ProjectionView,
    Reclaimed, VectorIndex,
};
use lakelands_api::error::Result;
use lakelands_api::event::{Digest, EventId, SegmentDigest};
use lakelands_api::id::{LakeId, Position, RecordKey, TypeName};
use lakelands_api::location::SqliteTuning;
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::write::{Declaration, Proposal};
use lakelands_core::engines::{self, vector_bytes, vector_of};
use rusqlite::{Connection, OpenFlags, params};

use crate::lease::{Held, LeaseFile};
use crate::log::Log;
use crate::projection::{self, MEMBERS, OUTPUTS, PROPOSALS};
use crate::view::SqliteView;
use crate::{NAME, invalid, io_error, lock, sql, unavailable};

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
        if !dir.join("log").is_dir() {
            if !create {
                return Err(unavailable(format!(
                    "there is no lake in {}; open it with `create` to make it",
                    dir.display()
                )));
            }
            std::fs::create_dir_all(&dir).map_err(|e| io_error("creating", &dir, e))?;
        }
        check_name(&dir, &lake, create)?;
        let log = Log::open(&dir, create)?;
        let path = dir.join("projection.db");
        // A handle opened read-only creates and changes nothing in the
        // directory: it needs the projection a writer made.
        let writer = match mode {
            OpenMode::Read => {
                if !path.is_file() {
                    return Err(unavailable(format!(
                        "the lake in {} has no projection yet; open it for writing first",
                        dir.display()
                    )));
                }
                None
            }
            OpenMode::Write | OpenMode::Create => {
                let conn = connect(&path, &tuning, true)?;
                conn.execute_batch(projection::SCHEMA).map_err(sql)?;
                // An order index a writer did not keep (a binary that lacks
                // it folded past its mark or wrote its setting) is rebuilt
                // from the head, once.
                projection::catch_up_sorted(&conn)?;
                Some(conn)
            }
        };
        let reader = connect(&path, &tuning, false)?;
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
        })
    }

    /// Run `f` on the writer connection; a handle opened read-only has
    /// none, and writes nothing.
    fn write<R>(&self, f: impl FnOnce(&Connection) -> Result<R>) -> Result<R> {
        let writer = lock(&self.writer);
        match writer.as_ref() {
            Some(conn) => f(conn),
            None => Err(unavailable(format!(
                "lake `{}` is open read-only here; its projection is written by the handle \
                 that holds the lease",
                self.lake
            ))),
        }
    }

    /// Refuses a write to the log through a handle opened read-only.
    fn writable(&self) -> Result<()> {
        match self.mode {
            OpenMode::Read => Err(unavailable(format!(
                "lake `{}` is open read-only here; open it for writing to take the lease or \
                 append",
                self.lake
            ))),
            OpenMode::Write | OpenMode::Create => Ok(()),
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
        Err(unavailable(if holder.is_empty() {
            format!(
                "the lease with fence {} is stale: no one holds the lake's writer lease",
                lease.fence
            )
        } else {
            format!(
                "the lease with fence {} is stale: `{holder}` holds the lake's writer lease with \
                 fence {fence}",
                lease.fence
            )
        }))
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
                Err(e) => Err(io_error("measuring", p, e)),
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
        Ok(name) => Err(invalid(format!(
            "{} holds the lake `{}`, not `{lake}`",
            dir.display(),
            name.trim_end()
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
            std::fs::write(&path, format!("{lake}\n")).map_err(|e| io_error("writing", &path, e))
        }
        Err(e) => Err(io_error("reading", &path, e)),
    }
}

impl Drop for SqliteLake {
    /// A handle that is dropped gives up the lease it holds, as a process
    /// that ends does.
    fn drop(&mut self) {
        if let Some(held) = lock(&self.held).take() {
            let _ = self.lease_file.release(held);
        }
    }
}

impl LakeEngine for SqliteLake {
    fn name(&self) -> &str {
        NAME
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
        // before this one appends.
        lock(&self.log).recover()?;
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
        let mut held = self.check_lease(lease)?;
        let held = held
            .as_mut()
            .ok_or_else(|| unavailable("the lease is not held by this handle"))?;
        let mut log = lock(&self.log);
        log.refresh()?;
        if let Some((tip, _)) = log.tip()
            && self.lease_file.imported(held)? != Some(tip.0)
        {
            return Err(invalid(format!(
                "the lake's log holds events up to position {tip} that were not imported; an \
                 import goes only into a lake that holds nothing else, and nothing was \
                 appended"
            )));
        }
        log.append(None, events)?;
        match events.last() {
            Some(last) => self
                .lease_file
                .record_import(held, last.position.0, last.fence),
            None => Ok(()),
        }
    }

    fn read(&self, from: Position, limit: u32) -> Result<Vec<CanonicalEvent>> {
        lock(&self.log).read(from, limit)
    }

    fn seal_segment(&self, lease: &Lease) -> Result<Option<SegmentDigest>> {
        let _held = self.check_lease(lease)?;
        lock(&self.log).seal()
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

    fn vectors(&self) -> Option<&dyn VectorIndex> {
        Some(self)
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
            .ok_or_else(|| invalid("there is no transaction to commit: call `begin` first"))?;
        self.write(|conn| {
            let folded = projection::folded(conn)?;
            if let Some(refused) = engines::commit_refusal(folded, t.latest, through) {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(refused);
            }
            for (ty, at) in &t.redeclared {
                projection::index_older(conn, ty, *at)?;
            }
            projection::set_folded(conn, through)?;
            projection::mark_sorted(conn, through)?;
            conn.execute_batch("COMMIT").map_err(sql)
        })
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
            let redeclared = projection::put_declaration(conn, &d)?;
            t.redeclared.extend(redeclared.map(|ty| (ty, at)));
            Ok(())
        })
    }

    fn put_record(&self, record: &StoredRecord) -> Result<()> {
        self.stage(record.position, |conn, t| {
            projection::put_record(conn, &mut t.kept, record)
        })
    }

    fn put_proposal(&self, proposal: &Proposal, at: Position) -> Result<()> {
        let state = serde_json::to_string(proposal).expect("a proposal serialises");
        self.stage(at, |conn, _| {
            PROPOSALS.put(conn, &[&proposal.id.as_str()], at, &state)
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
            MEMBERS.put(conn, &[&set, &ty.as_str(), &key.as_str()], at, &member)
        })
    }

    fn put_output(&self, seam: &str, input: &Digest, output: &Value, at: Position) -> Result<()> {
        let state = serde_json::to_string(output).expect("a value serialises");
        self.stage(at, |conn, _| {
            OUTPUTS.put(conn, &[&seam, &input.0.as_str()], at, &state)
        })
    }

    fn mark_applied(&self, id: &EventId, at: Position) -> Result<()> {
        self.stage(at, |conn, _| {
            conn.prepare_cached("INSERT OR IGNORE INTO applied (id, at) VALUES (?1, ?2)")
                .and_then(|mut s| s.execute(params![id.0, at.0 as i64]))
                .map(|_| ())
                .map_err(sql)
        })
    }

    /// Drops the projection, its kept history and the vector index; the
    /// log and the lease stay. An open transaction is abandoned.
    fn clear(&self) -> Result<()> {
        *lock(&self.transaction) = None;
        self.write(|conn| {
            abandon(conn)?;
            let statements: String = projection::TABLES
                .iter()
                .map(|t| format!("DELETE FROM {t};"))
                .collect();
            conn.execute_batch(&format!("BEGIN IMMEDIATE; {statements} COMMIT;"))
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
            return Err(invalid(
                "the projection is compacted between appends: a transaction is open",
            ));
        }
        self.write(|conn| {
            // A transaction a failed commit left open is abandoned, as
            // `begin` and `clear` abandon it.
            abandon(conn)?;
            let before = self.size()?;
            if !truncate_journal(conn)? {
                return Err(unavailable(format!(
                    "lake `{}` could not be compacted: a reader holds a snapshot of its \
                     projection; nothing a read answers has changed, so compact again when the \
                     reader is done",
                    self.lake
                )));
            }
            conn.execute_batch("VACUUM").map_err(|e| {
                unavailable(format!(
                    "lake `{}` could not be compacted, and its projection is as it was: {e}",
                    self.lake
                ))
            })?;
            // The rewrite is committed. A reader that holds a snapshot now
            // keeps the journal from being truncated: the compact is done
            // all the same, and `after` counts the journal it left.
            truncate_journal(conn)?;
            Ok(Reclaimed {
                before: Some(before),
                after: Some(self.size()?),
            })
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

/// The vector index: each field's vectors in one table, searched by
/// brute force with the one cosine every engine uses
/// (`lakelands_api::query::cosine`), so its answers are those of every
/// other engine to the last bit. A query steps through every vector of
/// the field and keeps only the best `k`
/// (`lakelands_core::engines::TopK`), so its cost grows linearly with them
/// and what it holds does not: the ceiling `docs/seams.md` states.
/// Vectors are indexed at the head only; filtering the candidates by what
/// an earlier view could see is the store's.
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

    fn nearest(
        &self,
        ty: &TypeName,
        field: &str,
        query: &[f32],
        k: u32,
    ) -> Result<Vec<(RecordKey, f64)>> {
        let conn = lock(&self.reader);
        let mut stmt = conn
            .prepare_cached(
                "SELECT key, data FROM vectors WHERE ty = ?1 AND field = ?2 ORDER BY key",
            )
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
                return Err(engines::dimensions(query.len(), v.len(), ty, field, &key));
            }
            top.push(
                &RecordKey::from(key),
                lakelands_api::query::cosine(query, &v),
            );
        }
        Ok(top.into_ranked())
    }

    fn count(&self, ty: &TypeName, field: &str) -> Result<u64> {
        lock(&self.reader)
            .prepare_cached("SELECT count(*) FROM vectors WHERE ty = ?1 AND field = ?2")
            .and_then(|mut s| s.query_row(params![ty.as_str(), field], |r| r.get::<_, i64>(0)))
            .map(|n| n as u64)
            .map_err(sql)
    }
}
