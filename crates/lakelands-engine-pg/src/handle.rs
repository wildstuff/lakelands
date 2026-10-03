//! A handle onto one lake's schema: the log and lease, the projection's
//! write side, the vector index.
//!
//! Handles share nothing but the database: any number of them, in this
//! process or others, see one lake. A handle keeps its own connections:
//! - a reader, for views and log reads, which only reads;
//! - for a handle opened for writing, a lease connection, which holds the
//!   lease's lock, appends and seals;
//! - and a projection connection, the only one that writes the projection.
//!
//! It gives up the lease it took when it is dropped. A handle opened
//! read-only refuses the lease, every append, every seal and every write
//! of the projection.
//!
//! **Statistics after a load.** The planner plans a view's statements
//! from the tables' statistics, and PostgreSQL's own analysis of freshly
//! loaded tables runs only some time after the load: until then a page
//! read through an index can be planned as a walk over the type, hundreds
//! of milliseconds instead of one or two. So the writer counts the records
//! it puts, and once [`ANALYZE_AFTER`] have gone in since its last
//! analysis, it runs `ANALYZE` over `records` and the head indexes after
//! the commit has answered, outside the fold's transaction. A rebuild is
//! a refold after `clear`, which starts the count again, so it is
//! analysed as a fresh load is: after its first `ANALYZE_AFTER` records,
//! and a lake smaller than that is left to PostgreSQL's own. Analysis
//! changes no answer, only a plan; a failed one leaves the commit as it
//! is.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use lakelands_api::engine::{
    CanonicalEvent, Declared, LakeEngine, Lease, OpenMode, ProjectionStore, ProjectionView,
    Reclaimed, VectorIndex,
};
use lakelands_api::error::Result;
use lakelands_api::event::{Digest, EventId, SegmentDigest};
use lakelands_api::id::{LakeId, Position, RecordKey, TypeName};
use lakelands_api::query::cosine;
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::write::{Declaration, Proposal};
use lakelands_core::engines::{self, vector_bytes, vector_of};

use crate::connect::{self, Db, Target};
use crate::projection::{self, Buffer, HeadTypes, Names, PROJECTION_TABLES};
use crate::vector_index::{self, Pgvector};
use crate::view::PgView;
use crate::{NAME, invalid, lease, lock, log, unavailable};

/// The open transaction of the fold: the latest position a put in it
/// names, which the commit must reach, the head declarations it read, and
/// the puts it holds until the commit writes them.
#[derive(Default)]
struct Transaction {
    latest: Option<Position>,
    types: HeadTypes,
    buffer: Buffer,
    /// The types declared in it, each with the position of its latest
    /// declaration: the commit indexes their older records under it.
    redeclared: BTreeMap<TypeName, Position>,
}

/// The records a writer puts between two analyses of its tables.
pub const ANALYZE_AFTER: u64 = 5_000;

/// The connections a handle opened for writing keeps, and the records
/// put since the tables were last analysed.
struct Writing {
    lease: Mutex<Db>,
    projection: Mutex<Db>,
    unanalysed: AtomicU64,
}

/// One handle onto a lake kept by the PostgreSQL engine.
pub struct PgLake {
    lake: LakeId,
    target: Target,
    names: Names,
    reader: Mutex<Db>,
    writing: Option<Writing>,
    held: Mutex<Option<Lease>>,
    transaction: Mutex<Option<Transaction>>,
    /// pgvector, when the server has it: the vector index a lake asks for
    /// lives there (`crate::vector_index`).
    pgvector: Option<Pgvector>,
}

impl PgLake {
    pub(crate) fn open(lake: LakeId, target: Target, mode: OpenMode) -> Result<PgLake> {
        let names = Names::new(target.quoted_schema()?);
        let mut reader = connect::connect(&target)?;
        let exists = matches!(shape(&mut reader, &target.schema)?, Shape::Lake);
        if !exists && mode != OpenMode::Create {
            return Err(unavailable(format!(
                "there is no lake in the schema `{}`; open it with `create` to make it",
                target.schema
            )));
        }
        let (writing, pgvector) = match mode {
            OpenMode::Read => (None, vector_index::find(&mut reader, false).ok()),
            OpenMode::Write | OpenMode::Create => {
                let lease = connect::session_keeping_state(&target)?;
                let mut projection = connect::connect(&target)?;
                prepare(&mut projection, &names, &target.schema, &lake, mode)?;
                projection::catch_up_sorted(&mut projection, &names)?;
                let found = vector_index::find(&mut projection, true);
                if let Ok(v) = &found {
                    vector_index::prepare(&mut projection, &names, v)?;
                }
                vector_index::catch_up(
                    &mut projection,
                    &names,
                    found.as_ref().map_err(String::as_str),
                    lake.as_str(),
                )?;
                let writing = Writing {
                    lease: Mutex::new(lease),
                    projection: Mutex::new(projection),
                    unanalysed: AtomicU64::new(0),
                };
                (Some(writing), found.ok())
            }
        };
        check_name(&mut reader, &names, &target.schema, &lake)?;
        Ok(PgLake {
            lake,
            target,
            names,
            reader: Mutex::new(reader),
            writing,
            held: Mutex::new(None),
            transaction: Mutex::new(None),
            pgvector,
        })
    }

    /// The reader connection.
    pub(crate) fn reader(&self) -> MutexGuard<'_, Db> {
        lock(&self.reader)
    }

    /// The lake's tables.
    pub(crate) fn names(&self) -> &Names {
        &self.names
    }

    fn writing(&self) -> Result<&Writing> {
        self.writing.as_ref().ok_or_else(|| {
            unavailable(format!(
                "lake `{}` is open read-only here; open it for writing to take the lease, append \
                 or write its projection",
                self.lake
            ))
        })
    }

    /// The lease connection, when this handle holds `lease`; otherwise
    /// nothing was `what`.
    fn holding(&self, lease: &Lease, what: &str) -> Result<MutexGuard<'_, Db>> {
        let writing = self.writing()?;
        if lock(&self.held).as_ref() != Some(lease) {
            return Err(unavailable(format!(
                "the lease with fence {} is not held by this handle; nothing was {what}",
                lease.fence
            )));
        }
        Ok(lock(&writing.lease))
    }

    /// Take a put into the open transaction: `f` holds it in the buffer,
    /// which the commit writes.
    fn stage(&self, position: Position, f: impl FnOnce(&mut Buffer)) -> Result<()> {
        self.writing()?;
        let mut transaction = lock(&self.transaction);
        let t = transaction.as_mut().ok_or_else(engines::unstaged)?;
        t.latest = t.latest.max(Some(position));
        f(&mut t.buffer);
        Ok(())
    }
}

/// What a schema holds, as the engine sees it at open.
enum Shape {
    /// No schema, or one holding no table.
    Empty,
    /// A lake's tables (some may be added by a later version).
    Lake,
}

/// Look at the schema: empty, a lake's, or something else, which is
/// refused. One schema holds one lake and nothing beside it.
fn shape(db: &mut Db, schema: &str) -> Result<Shape> {
    let tables: Vec<String> = db.column(
        "SELECT tablename FROM pg_catalog.pg_tables WHERE schemaname = $1 ORDER BY tablename",
        &[&schema],
    )?;
    if tables.is_empty() {
        return Ok(Shape::Empty);
    }
    let foreign: Vec<&String> = tables
        .iter()
        .filter(|t| !projection::LAKE_TABLES.contains(&t.as_str()))
        .collect();
    if !tables.iter().any(|t| t == "meta") || !foreign.is_empty() {
        let names: Vec<String> = foreign.iter().map(|t| format!("`{t}`")).collect();
        return Err(invalid(format!(
            "the schema `{schema}` holds tables that are not a lake's ({}); a lake needs a \
             schema of its own",
            if names.is_empty() {
                "it has no `meta`".to_owned()
            } else {
                names.join(", ")
            }
        )));
    }
    // The columns the engine reads must be those it wrote.
    let columns: i64 = db
        .query_one(
            "SELECT count(*) FROM information_schema.columns WHERE table_schema = $1 AND \
             ((table_name = 'meta' AND column_name IN ('k', 'n', 't')) OR \
              (table_name = 'lease' AND column_name IN ('one', 'fence', 'holder')))",
            &[&schema],
        )?
        .get(0);
    let lease_table = tables.iter().any(|t| t == "lease");
    if columns != if lease_table { 6 } else { 3 } {
        return Err(invalid(format!(
            "the schema `{schema}` holds a `meta` or `lease` table that is not a lake's"
        )));
    }
    Ok(Shape::Lake)
}

/// Create the lake's tables where they are missing, and on `Create` the
/// lake itself, under a lock that keeps two openers from racing.
fn prepare(db: &mut Db, names: &Names, schema: &str, lake: &LakeId, mode: OpenMode) -> Result<()> {
    db.transaction("BEGIN ISOLATION LEVEL READ COMMITTED", |db| {
        db.query_one(
            "SELECT pg_advisory_xact_lock(hashtextextended('lakelands create ' || $1, 0))",
            &[&schema],
        )?;
        db.batch(&names.schema_sql())?;
        if mode == OpenMode::Create {
            db.execute(
                &format!(
                    "INSERT INTO {} (k, t) VALUES ('lake', $1) ON CONFLICT DO NOTHING",
                    names.t("meta")
                ),
                &[&lake.as_str()],
            )?;
            log::create(db, names)?;
        }
        Ok(())
    })
}

/// The lake a schema holds is named in its `lake` row; a schema opened
/// under another name is refused.
fn check_name(db: &mut Db, names: &Names, schema: &str, lake: &LakeId) -> Result<()> {
    let held: Option<String> = db
        .query_opt(
            &format!("SELECT t FROM {} WHERE k = 'lake'", names.t("meta")),
            &[],
        )?
        .and_then(|r| r.get(0));
    match held {
        Some(name) if name == lake.as_str() => Ok(()),
        Some(name) => Err(invalid(format!(
            "the schema `{schema}` holds the lake `{name}`, not `{lake}`"
        ))),
        None => Err(unavailable(format!(
            "the schema `{schema}` holds no lake's name; it was not created by this engine"
        ))),
    }
}

impl Drop for PgLake {
    /// A handle that is dropped gives up the lease it holds, as a session
    /// that ends does.
    fn drop(&mut self) {
        let held = lock(&self.held).clone();
        if let Some(lease) = held {
            let _ = self.release_lease(&lease);
        }
    }
}

impl LakeEngine for PgLake {
    fn name(&self) -> &str {
        NAME
    }

    fn lake(&self) -> &LakeId {
        &self.lake
    }

    fn take_lease(&self, holder: &str) -> Result<Lease> {
        let writing = self.writing()?;
        let mut held = lock(&self.held);
        if let Some(h) = held.as_ref() {
            return Err(engines::lease_held(&h.holder, h.fence));
        }
        let lease = lease::take(
            &mut lock(&writing.lease),
            &self.names,
            &self.target.schema,
            holder,
        )?;
        *held = Some(lease.clone());
        Ok(lease)
    }

    fn release_lease(&self, lease: &Lease) -> Result<()> {
        let mut held = lock(&self.held);
        if held.as_ref() == Some(lease) {
            let writing = self.writing()?;
            held.take();
            lease::release(
                &mut lock(&writing.lease),
                &self.names,
                &self.target.schema,
                lease,
            )?;
        }
        Ok(())
    }

    fn tip(&self) -> Result<Option<(Position, Digest)>> {
        log::tip(&mut self.reader(), &self.names)
    }

    fn append(&self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        let mut db = self.holding(lease, "appended")?;
        log::append(&mut db, &self.names, lease, events, false)
    }

    fn import(&self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        let mut db = self.holding(lease, "appended")?;
        log::append(&mut db, &self.names, lease, events, true)
    }

    fn read(&self, from: Position, limit: u32) -> Result<Vec<CanonicalEvent>> {
        log::read(&mut self.reader(), &self.names, from, limit)
    }

    fn seal_segment(&self, lease: &Lease) -> Result<Option<SegmentDigest>> {
        let mut db = self.holding(lease, "sealed")?;
        log::seal(&mut db, &self.names, lease)
    }

    fn segments(&self) -> Result<Vec<SegmentDigest>> {
        log::segments(&mut self.reader(), &self.names)
    }

    fn projection(&self) -> &dyn ProjectionStore {
        self
    }

    fn view(&self, as_of: Option<Position>) -> Result<Box<dyn ProjectionView + '_>> {
        let folded = projection::folded(&mut self.reader(), &self.names)?;
        let at = engines::view_at(as_of, folded)?;
        Ok(Box::new(PgView { lake: self, at }))
    }

    fn vectors(&self) -> Option<&dyn VectorIndex> {
        Some(self)
    }
}

impl ProjectionStore for PgLake {
    /// Opens a transaction. Beginning while one is open abandons the open
    /// one, as a rollback would.
    fn begin(&self) -> Result<()> {
        let writing = self.writing()?;
        let mut transaction = lock(&self.transaction);
        let mut db = lock(&writing.projection);
        if transaction.take().is_some() {
            db.batch("ROLLBACK")?;
        }
        db.batch("BEGIN ISOLATION LEVEL READ COMMITTED")?;
        *transaction = Some(Transaction::default());
        Ok(())
    }

    fn commit(&self, through: Position) -> Result<()> {
        let writing = self.writing()?;
        let mut transaction = lock(&self.transaction);
        let t = transaction
            .take()
            .ok_or_else(|| invalid("there is no transaction to commit: call `begin` first"))?;
        let mut db = lock(&writing.projection);
        let folded = projection::folded(&mut db, &self.names)?;
        if let Some(refused) = engines::commit_refusal(folded, t.latest, through) {
            return db.end(Err(refused));
        }
        let Transaction {
            mut types,
            buffer,
            redeclared,
            ..
        } = t;
        let put = buffer.records_put() as u64;
        let written = buffer
            .write(&mut db, &self.names, &mut types, self.pgvector.as_ref())
            .and_then(|()| {
                let older = |(ty, at): (&TypeName, &Position)| {
                    projection::index_older(&mut db, &self.names, &mut types, ty, *at)
                };
                redeclared.iter().try_for_each(older)
            })
            .and_then(|()| projection::set_folded(&mut db, &self.names, through));
        db.end(written)?;
        // Once enough records went in since the last analysis, the planner
        // learns of them, after the commit and outside it (module doc).
        if writing.unanalysed.fetch_add(put, Ordering::Relaxed) + put >= ANALYZE_AFTER {
            writing.unanalysed.store(0, Ordering::Relaxed);
            let _ = projection::analyze(&mut db, &self.names);
        }
        Ok(())
    }

    fn rollback(&self) {
        *lock(&self.transaction) = None;
        if let Some(writing) = self.writing.as_ref() {
            let _ = lock(&writing.projection).batch("ROLLBACK");
        }
    }

    fn folded_through(&self) -> Result<Option<Position>> {
        projection::folded(&mut self.reader(), &self.names)
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
        let writing = self.writing()?;
        let mut transaction = lock(&self.transaction);
        let t = transaction.as_mut().ok_or_else(engines::unstaged)?;
        t.latest = t.latest.max(Some(at));
        let mut db = lock(&writing.projection);
        if let Some(ty) = projection::put_declaration(&mut db, &self.names, &mut t.types, &d)? {
            t.redeclared.insert(ty, at);
        }
        // A redeclaration may change a field's dimensions or drop it: the
        // type's vector indexes are built again under it.
        if let Declaration::Type(ty) = declaration {
            vector_index::redeclared(&mut db, &self.names, self.pgvector.as_ref(), &ty.name, at)?;
        }
        Ok(())
    }

    fn put_record(&self, record: &StoredRecord) -> Result<()> {
        self.stage(record.position, |b| b.record(record))
    }

    fn put_proposal(&self, proposal: &Proposal, at: Position) -> Result<()> {
        let state = serde_json::to_string(proposal).expect("a proposal serialises");
        self.stage(at, |b| b.proposal(proposal.id.as_str(), at, state))
    }

    fn put_membership(
        &self,
        set: &str,
        ty: &TypeName,
        key: &RecordKey,
        member: bool,
        at: Position,
    ) -> Result<()> {
        self.stage(at, |b| b.member(set, ty.as_str(), key.as_str(), at, member))
    }

    fn put_output(&self, seam: &str, input: &Digest, output: &Value, at: Position) -> Result<()> {
        let state = serde_json::to_string(output).expect("a value serialises");
        self.stage(at, |b| b.output(seam, input.0.as_str(), at, state))
    }

    fn mark_applied(&self, id: &EventId, at: Position) -> Result<()> {
        self.stage(at, |b| b.applied(&id.0, at))
    }

    /// Drops the projection, its kept history and the vector index; the
    /// log, the lease and the lake's name stay. An open transaction is
    /// abandoned.
    fn clear(&self) -> Result<()> {
        let writing = self.writing()?;
        *lock(&self.transaction) = None;
        // The tables start empty again, and so does the count (module doc).
        writing.unanalysed.store(0, Ordering::Relaxed);
        let mut db = lock(&writing.projection);
        let _ = db.batch("ROLLBACK");
        let statements: String = PROJECTION_TABLES
            .iter()
            .map(|t| format!("DELETE FROM {};", self.names.t(t)))
            .collect();
        db.batch(&format!(
            "BEGIN; {statements} DELETE FROM {m} WHERE k = 'folded' OR k LIKE 'vindex.%'; \
             COMMIT;",
            m = self.names.t("meta")
        ))
    }

    /// `VACUUM (ANALYZE)` over the projection's tables and its `meta`,
    /// outside a transaction; never `VACUUM FULL`, which would lock every
    /// reader out while it rewrote. Readers go on reading. A `VACUUM` that
    /// fails cannot be rolled back, but it changes no row a view reads at
    /// any point. The log's tables are not touched.
    fn compact(&self) -> Result<Reclaimed> {
        let writing = self.writing()?;
        let transaction = lock(&self.transaction);
        if transaction.is_some() {
            return Err(invalid(
                "the projection is compacted between appends: a transaction is open",
            ));
        }
        let tables: Vec<&str> = PROJECTION_TABLES.iter().copied().chain(["meta"]).collect();
        let mut db = lock(&writing.projection);
        // A transaction a failed commit left open is abandoned, as `clear`
        // abandons it: `VACUUM` cannot run inside one.
        let _ = db.batch("ROLLBACK");
        let before = size(&mut db, &self.target.schema, &tables)?;
        let listed: Vec<String> = tables.iter().map(|t| self.names.t(t)).collect();
        db.batch(&format!("VACUUM (ANALYZE) {}", listed.join(", ")))?;
        let after = size(&mut db, &self.target.schema, &tables)?;
        Ok(Reclaimed {
            before: Some(before),
            after: Some(after),
        })
    }
}

/// The summed size in bytes of some of a schema's tables, their indexes
/// and TOAST included.
fn size(db: &mut Db, schema: &str, tables: &[&str]) -> Result<u64> {
    let row = db.query_one(
        "SELECT coalesce(sum(pg_catalog.pg_total_relation_size(c.oid)), 0)::int8 \
         FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = $1 AND c.relname = ANY($2) AND c.relkind = 'r'",
        &[&schema, &tables],
    )?;
    Ok(row.get::<_, i64>(0) as u64)
}

/// The vector index: each field's vectors in one table, searched by brute
/// force with the one cosine every engine uses
/// (`lakelands_api::query::cosine`), so its answers are those of every
/// other engine to the last bit. A query streams every vector of the
/// field from the server a row at a time and keeps only the best `k`
/// (`lakelands_core::engines::TopK`), so its cost grows linearly with them
/// and what it holds does not: the ceiling `docs/seams.md` states.
/// Vectors are indexed at the head only, in the fold's
/// transaction; filtering the candidates by what an earlier view could see
/// is the store's.
impl VectorIndex for PgLake {
    fn put(&self, ty: &TypeName, key: &RecordKey, field: &str, vector: &[f32]) -> Result<()> {
        let writing = self.writing()?;
        let mut db = lock(&writing.projection);
        match &self.pgvector {
            // The unit vector goes in with the vector when the lake asks
            // for the field's index (`crate::vector_index`).
            Some(v) => db.execute(
                &vector_index::put_sql(&self.names, v),
                &[
                    &ty.as_str(),
                    &field,
                    &key.as_str(),
                    &vector_bytes(vector),
                    &vector_index::mark(ty, field),
                    &vector_index::unit_text(vector),
                ],
            ),
            None => db.execute(
                &format!(
                    "INSERT INTO {} (ty, field, key, data) VALUES ($1, $2, $3, $4) \
                     ON CONFLICT (ty, field, key) DO UPDATE SET data = $4",
                    self.names.t("vectors")
                ),
                &[&ty.as_str(), &field, &key.as_str(), &vector_bytes(vector)],
            ),
        }
    }

    fn remove(&self, ty: &TypeName, key: &RecordKey, field: &str) -> Result<()> {
        let writing = self.writing()?;
        lock(&writing.projection).execute(
            &format!(
                "DELETE FROM {} WHERE ty = $1 AND field = $2 AND key = $3",
                self.names.t("vectors")
            ),
            &[&ty.as_str(), &field, &key.as_str()],
        )
    }

    fn nearest(
        &self,
        ty: &TypeName,
        field: &str,
        query: &[f32],
        k: u32,
    ) -> Result<Vec<(RecordKey, f64)>> {
        let mut top = lakelands_core::engines::TopK::new(k);
        // The exact scan checks every vector's dimensions; the index holds
        // only vectors of the query's.
        let mut rank = |row: &postgres::Row, exact: bool| {
            let key: String = row.get(0);
            let v = vector_of(&row.get::<_, Vec<u8>>(1));
            if exact && v.len() != query.len() {
                return Err(engines::dimensions(query.len(), v.len(), ty, field, &key));
            }
            top.push(&RecordKey::from(key), cosine(query, &v));
            Ok(())
        };
        let mut db = self.reader();
        // A field the lake asks an index for, whose index describes the
        // head, answers the index's `k` candidates, ranked by the one
        // cosine (`crate::vector_index`); every other search scans.
        if let (Some(v), Some(unit)) = (&self.pgvector, vector_index::unit_text(query))
            && let Some(n) = vector_index::usable(&mut db, &self.names, ty, field)?
            && n as usize == query.len()
        {
            let sql = vector_index::candidates_sql(&self.names, v, ty, field, n);
            db.transaction("BEGIN ISOLATION LEVEL READ COMMITTED READ ONLY", |db| {
                db.batch(&vector_index::search_settings(k))?;
                db.query_each(&sql, &[&unit, &i64::from(k)], |row| rank(row, false))
            })?;
            return Ok(top.into_ranked());
        }
        db.query_each(
            &format!(
                "SELECT key, data FROM {} WHERE ty = $1 AND field = $2 ORDER BY key COLLATE \"C\"",
                self.names.t("vectors")
            ),
            &[&ty.as_str(), &field],
            |row| rank(row, true),
        )?;
        Ok(top.into_ranked())
    }

    fn count(&self, ty: &TypeName, field: &str) -> Result<u64> {
        let n: i64 = self
            .reader()
            .query_one(
                &format!(
                    "SELECT count(*) FROM {} WHERE ty = $1 AND field = $2",
                    self.names.t("vectors")
                ),
                &[&ty.as_str(), &field],
            )?
            .get(0);
        Ok(n as u64)
    }
}

/// Write a lake's log as the segment files `docs/log-format.md` §4.2
/// describes, `<dir>/log/<first>.jsonl`, through a handle that only reads.
pub fn export(location: &lakelands_api::Location, dir: &std::path::Path) -> Result<()> {
    let lake = PgLake::open(
        location.lake.clone(),
        crate::target(location)?,
        OpenMode::Read,
    )?;
    log::export(&mut lake.reader(), &lake.names, dir)
}
