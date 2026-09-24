//! A handle onto one lake directory: the log and lease, the projection's
//! write side, the vector index.
//!
//! Handles share nothing but the directory: any number of them, in this
//! process or others, see one lake. Each keeps its own read of the log,
//! its own connections to the projection, its open transaction and the
//! lease it took, which it gives up when it is dropped. A handle opened
//! read-only refuses the lease, every append and every seal.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use lakelands_api::engine::{
    CanonicalEvent, Declared, LakeEngine, Lease, OpenMode, ProjectionStore, ProjectionView,
    VectorIndex,
};
use lakelands_api::error::{Error, Refusal, Result};
use lakelands_api::event::{Digest, EventId, SegmentDigest};
use lakelands_api::id::{LakeId, Position, RecordKey, TypeName};
use lakelands_api::location::SqliteTuning;
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::write::{Declaration, Proposal};
use rusqlite::{Connection, OpenFlags, params};

use crate::lease::{Held, LeaseFile};
use crate::log::Log;
use crate::projection::{self, MEMBERS, OUTPUTS, PROPOSALS};
use crate::view::SqliteView;
use crate::{NAME, invalid, lock, sql, unavailable};

/// The open transaction of the fold: the latest position a put in it
/// names, which the commit must reach.
#[derive(Default)]
struct Transaction {
    latest: Option<Position>,
}

/// One handle onto a lake kept by the SQLite engine.
pub struct SqliteLake {
    lake: LakeId,
    mode: OpenMode,
    log: Mutex<Log>,
    lease_file: LeaseFile,
    held: Mutex<Option<Held>>,
    writer: Mutex<Option<Connection>>,
    reader: Mutex<Connection>,
    transaction: Mutex<Option<Transaction>>,
}

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
    conn.execute_batch(&format!(
        "PRAGMA busy_timeout = 10000;
         PRAGMA cache_size = -{cache};
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
            std::fs::create_dir_all(&dir).map_err(|e| crate::io_error("creating", &dir, e))?;
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
                Some(conn)
            }
        };
        let reader = connect(&path, &tuning, false)?;
        Ok(SqliteLake {
            lease_file: LeaseFile::new(&dir),
            lake,
            mode,
            log: Mutex::new(log),
            held: Mutex::new(None),
            writer: Mutex::new(writer),
            reader: Mutex::new(reader),
            transaction: Mutex::new(None),
        })
    }

    /// The reader connection.
    pub(crate) fn reader(&self) -> Result<MutexGuard<'_, Connection>> {
        Ok(lock(&self.reader))
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

    /// Refuses a lease that is not the one this handle holds, or whose
    /// fencing token the lease file no longer records: the lock can be lost
    /// without the process knowing, and the token is what says so.
    fn check_lease(&self, lease: &Lease) -> Result<()> {
        let (fence, holder) = self.lease_file.current()?;
        if lock(&self.held).as_ref().is_some_and(|h| h.lease == *lease) && fence == lease.fence {
            return Ok(());
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

    /// Hold a put for the open transaction.
    fn stage(&self, position: Position, f: impl FnOnce(&Connection) -> Result<()>) -> Result<()> {
        let mut transaction = lock(&self.transaction);
        let Some(t) = transaction.as_mut() else {
            return Err(invalid(
                "the projection is written inside a transaction: call `begin` first",
            ));
        };
        t.latest = t.latest.max(Some(position));
        self.write(f)
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
            std::fs::write(&path, format!("{lake}\n"))
                .map_err(|e| crate::io_error("writing", &path, e))
        }
        Err(e) => Err(crate::io_error("reading", &path, e)),
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
            return Err(unavailable(format!(
                "the lake's writer lease is held by `{}` (fence {}); it must be released before \
                 another writer can take it",
                h.lease.holder, h.lease.fence
            )));
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
        self.writable()?;
        self.check_lease(lease)?;
        lock(&self.log).append(lease, events)
    }

    fn read(&self, from: Position, limit: u32) -> Result<Vec<CanonicalEvent>> {
        lock(&self.log).read(from, limit)
    }

    fn seal_segment(&self, lease: &Lease) -> Result<Option<SegmentDigest>> {
        self.writable()?;
        self.check_lease(lease)?;
        lock(&self.log).seal()
    }

    fn segments(&self) -> Result<Vec<SegmentDigest>> {
        lock(&self.log).segments()
    }

    fn projection(&self) -> &dyn ProjectionStore {
        self
    }

    fn view(&self, as_of: Option<Position>) -> Result<Box<dyn ProjectionView + '_>> {
        let folded = projection::folded(&*self.reader()?)?;
        let at = match (as_of, folded) {
            (None, folded) => folded,
            (Some(p), Some(head)) if p <= head => Some(p),
            (Some(_), head) => {
                return Err(Error::Refused {
                    field: None,
                    reason: Refusal::OutOfRange {
                        head: head.unwrap_or_default(),
                    },
                });
            }
        };
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
            if !conn.is_autocommit() {
                conn.execute_batch("ROLLBACK").map_err(sql)?;
            }
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
            let refuse = |message: String| {
                let _ = conn.execute_batch("ROLLBACK");
                Err(invalid(message))
            };
            if let Some(folded) = projection::folded(conn)?.filter(|f| *f > through) {
                return refuse(format!(
                    "the projection reflects the log through position {folded}; a transaction \
                     cannot commit through the earlier position {through}"
                ));
            }
            if let Some(late) = t.latest.filter(|p| *p > through) {
                return refuse(format!(
                    "a state valid from position {late} cannot be committed through the earlier \
                     position {through}"
                ));
            }
            projection::set_folded(conn, through)?;
            conn.execute_batch("COMMIT").map_err(sql)
        })
    }

    fn rollback(&self) {
        *lock(&self.transaction) = None;
        let _ = self.write(|conn| {
            if !conn.is_autocommit() {
                conn.execute_batch("ROLLBACK").map_err(sql)?;
            }
            Ok(())
        });
    }

    fn folded_through(&self) -> Result<Option<Position>> {
        projection::folded(&*self.reader()?)
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
        self.stage(at, |conn| projection::put_declaration(conn, &d))
    }

    fn put_record(&self, record: &StoredRecord) -> Result<()> {
        self.stage(record.position, |conn| projection::put_record(conn, record))
    }

    fn put_proposal(&self, proposal: &Proposal, at: Position) -> Result<()> {
        let state = serde_json::to_string(proposal).expect("a proposal serialises");
        self.stage(at, |conn| {
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
        self.stage(at, |conn| {
            MEMBERS.put(conn, &[&set, &ty.as_str(), &key.as_str()], at, &member)
        })
    }

    fn put_output(&self, seam: &str, input: &Digest, output: &Value, at: Position) -> Result<()> {
        let state = serde_json::to_string(output).expect("a value serialises");
        self.stage(at, |conn| {
            OUTPUTS.put(conn, &[&seam, &input.0.as_str()], at, &state)
        })
    }

    fn mark_applied(&self, id: &EventId, at: Position) -> Result<()> {
        self.stage(at, |conn| {
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
            if !conn.is_autocommit() {
                conn.execute_batch("ROLLBACK").map_err(sql)?;
            }
            let statements: String = projection::TABLES
                .iter()
                .map(|t| format!("DELETE FROM {t};"))
                .collect();
            conn.execute_batch(&format!("BEGIN IMMEDIATE; {statements} COMMIT;"))
                .map_err(sql)
        })
    }
}

/// A vector as the index stores it: its components as little-endian
/// `f32`s.
fn vector_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn vector_of(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

/// The vector index: each field's vectors in one table, searched by
/// brute force with the one cosine every engine uses
/// (`lakelands_core::engines::cosine`), so its answers are those of every
/// other engine to the last bit. A query reads every vector of the field,
/// so its cost grows linearly with them: the ceiling `docs/seams.md`
/// states. Vectors are indexed at the head only; filtering the candidates
/// by what an earlier view could see is the store's.
impl VectorIndex for SqliteLake {
    fn put(&self, ty: &TypeName, key: &RecordKey, field: &str, vector: &[f32]) -> Result<()> {
        self.write(|conn| {
            conn.prepare_cached(
                "INSERT INTO vectors (ty, field, key, data) VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT (ty, field, key) DO UPDATE SET data = ?4",
            )
            .and_then(|mut s| {
                s.execute(params![
                    ty.as_str(),
                    field,
                    key.as_str(),
                    vector_bytes(vector)
                ])
            })
            .map(|_| ())
            .map_err(sql)
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
        let conn = self.reader()?;
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
        let mut hits = Vec::new();
        for row in rows {
            let (key, data) = row.map_err(sql)?;
            let v = vector_of(&data);
            if v.len() != query.len() {
                return Err(invalid(format!(
                    "the query vector has {} dimensions, but `{ty}.{field}` holds vectors of {} \
                     (record `{key}`)",
                    query.len(),
                    v.len()
                )));
            }
            hits.push((
                RecordKey::from(key),
                lakelands_core::engines::cosine(query, &v),
            ));
        }
        lakelands_core::engines::rank(&mut hits, k);
        Ok(hits)
    }

    fn count(&self, ty: &TypeName, field: &str) -> Result<u64> {
        self.reader()?
            .prepare_cached("SELECT count(*) FROM vectors WHERE ty = ?1 AND field = ?2")
            .and_then(|mut s| s.query_row(params![ty.as_str(), field], |r| r.get::<_, i64>(0)))
            .map(|n| n as u64)
            .map_err(sql)
    }
}
