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
//! it puts, and the order index rows a `sort_index.<type>` setting's write
//! rebuilds, a type's at once; once [`ANALYZE_AFTER`] have gone in since
//! its last analysis, it runs `ANALYZE` over `records` and the head
//! indexes after the commit has answered, outside the fold's transaction.
//! A rebuild is a refold after `clear`, which starts the count again, so
//! it is analysed as a fresh load is: after its first `ANALYZE_AFTER`
//! records, and a lake smaller than that is left to PostgreSQL's own.
//! Analysis changes no answer, only a plan; a failed one leaves the commit
//! as it is.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};

use lakelands_api::engine::{
    Applied, CanonicalEvent, Declared, LakeEngine, Lease, OpenMode, ProjectionStore,
    ProjectionView, Ranked, Rankings, Reclaimed, VectorIndex,
};
use lakelands_api::error::{Act, Case, Error, Part, Result, Situation};
use lakelands_api::event::{Digest, SegmentDigest};
use lakelands_api::id::{LakeId, Position, RecordKey, TypeName};
use lakelands_api::query::cosine;
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::write::{Declaration, Proposal};
use lakelands_core::engines::{self, vector_bytes, vector_of};

use crate::connect::{self, Db, Target};
use crate::projection::{self, Buffer, HeadTypes, Names, PROJECTION_TABLES};
use crate::vector_index::{self, Pgvector};
use crate::view::PgView;
use crate::{NAME, REBUILD_RATE, invalid, lease, lock, log};

/// The open transaction of the fold: the latest position a put in it
/// names, which the commit must reach, the head declarations it read, and
/// the puts it holds until a full page of them, or the commit, is written.
#[derive(Default)]
struct Transaction {
    latest: Option<Position>,
    types: HeadTypes,
    buffer: Buffer,
    /// The types declared in it, each with the position of its latest
    /// declaration: the commit indexes their older records under it.
    redeclared: BTreeMap<TypeName, Position>,
    /// The types whose `vector_index.<type>` setting a page wrote, each with
    /// the setting's position: the commit builds their indexes, once.
    asked: BTreeMap<TypeName, Position>,
    /// The declarations and the `lookup_index.<type>` settings it put: the
    /// commit decides the exact-text index's fields again for what they move.
    relook: engines::Relook,
    /// The record states staged in it, and the order index rows a page's
    /// settings rebuilt, for the count that tells the planner: counted as
    /// they are staged and written, since the buffer is written and
    /// emptied a page at a time.
    put: u64,
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
    pub(crate) names: Names,
    pub(crate) reader: Mutex<Db>,
    writing: Option<Writing>,
    held: Mutex<Option<Lease>>,
    transaction: Mutex<Option<Transaction>>,
    /// The head's declarations as the last commit left them, and the
    /// position it folded through, for the next one's relook.
    heads: Mutex<Option<(Position, engines::Heads)>>,
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
            let (lake, schema) = (lake.to_string(), Some(target.schema.clone()));
            return Err(Error::unavailable(Situation::NoLake { lake, schema }));
        }
        // A projection is read only by the layout it was made with, and
        // its tables are not touched before that is known; a new one is
        // made with this build's (`prepare`). One opened for a rebuild is
        // made anew by `clear` before anything reads it.
        if exists && mode != OpenMode::Rebuild {
            let marked = format!("SELECT n FROM {} WHERE k = 'layout'", names.t("meta"));
            let found: Option<i64> = reader.query_opt(&marked, &[])?.and_then(|r| r.get(0));
            engines::layout(&lake, found.map(|n| n as u64), projection::LAYOUT)?;
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
            OpenMode::Rebuild => {
                let lease = connect::session_keeping_state(&target)?;
                let mut projection = connect::connect(&target)?;
                let found = vector_index::find(&mut projection, true).ok();
                let writing = Writing {
                    lease: Mutex::new(lease),
                    projection: Mutex::new(projection),
                    unanalysed: AtomicU64::new(0),
                };
                (Some(writing), found)
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
            heads: Mutex::new(None),
            pgvector,
        })
    }

    /// Run `f` on the reader. A connection the server or the network
    /// closed is opened again, once, and `f` runs on the new one: a second
    /// failure is answered as it is (lakelands#979).
    pub(crate) fn reading<R>(&self, mut f: impl FnMut(&mut Db) -> Result<R>) -> Result<R> {
        let mut db = lock(&self.reader);
        if db.is_closed() {
            *db = connect::connect(&self.target)?;
        }
        match f(&mut db) {
            Err(_) if db.is_closed() => {
                *db = connect::connect(&self.target)?;
                f(&mut db)
            }
            answer => answer,
        }
    }

    /// The writer's connection `db` closed under it: the session the
    /// lease's lock lived in is gone, so this handle holds the lease no
    /// more, whatever it was doing (lakelands#979). `take_lease` opens the
    /// writer's connections again. `undone` is said only where no `COMMIT`
    /// was sent: the server rolls back the transaction of a session that
    /// ends, while a `COMMIT` sent may have landed, and that write's
    /// outcome is read back from the log. `fence` is the lease asked with,
    /// so `held` is not locked here: it is locked before the connections
    /// ([`LakeEngine::take_lease`]).
    fn lost<R>(
        &self,
        db: &Db,
        fence: Option<u64>,
        answer: Result<R>,
        undone: Option<Act>,
    ) -> Result<R> {
        match answer {
            Err(_) if db.is_closed() => {
                let undone = undone.filter(|_| !db.committing());
                Err(Error::unavailable(Situation::LeaseNotHeld {
                    fence,
                    undone,
                }))
            }
            answer => answer,
        }
    }

    /// The writer's projection connection, or the lease not held where
    /// it closed ([`Self::lost`]).
    fn projecting<'w>(&self, writing: &'w Writing) -> Result<MutexGuard<'w, Db>> {
        let db = lock(&writing.projection);
        self.lost(&db, None, Ok(()), None).map(|()| db)
    }

    fn writing(&self) -> Result<&Writing> {
        self.writing.as_ref().ok_or_else(|| {
            let (lake, leased_elsewhere) = (Some(self.lake.to_string()), false);
            Error::unavailable(Situation::ReadOnly {
                lake,
                leased_elsewhere,
            })
        })
    }

    /// The lease connection, when this handle holds `lease`; otherwise
    /// it is `undone`.
    fn holding(&self, lease: &Lease, undone: Act) -> Result<MutexGuard<'_, Db>> {
        let writing = self.writing()?;
        if lock(&self.held).as_ref() != Some(lease) {
            let (fence, undone) = (Some(lease.fence), Some(undone));
            return Err(Error::unavailable(Situation::LeaseNotHeld {
                fence,
                undone,
            }));
        }
        let db = lock(&writing.lease);
        self.lost(&db, Some(lease.fence), Ok(()), Some(undone))
            .map(|()| db)
    }

    /// Take a put into the open transaction: `f` holds it in the buffer,
    /// which is written inside the transaction once it holds a page of
    /// puts, and at the commit. So the engine holds a page of a
    /// transaction's puts, not all of them; no view sees a written page
    /// before the commit.
    fn stage(&self, position: Position, f: impl FnOnce(&mut Buffer)) -> Result<()> {
        let writing = self.writing()?;
        let mut transaction = lock(&self.transaction);
        let t = transaction.as_mut().ok_or_else(engines::unstaged)?;
        t.latest = t.latest.max(Some(position));
        f(&mut t.buffer);
        if t.buffer.len() < projection::PAGE {
            return Ok(());
        }
        let (page, mut db) = (std::mem::take(&mut t.buffer), self.projecting(writing)?);
        t.put += page.write(&mut db, &self.names, &mut t.types, &mut t.asked)?;
        Ok(())
    }
}

/// The first and the last event of the log in the target's schema, when
/// it holds a lake with any: read, never made or changed.
pub(crate) fn log_ends(target: &Target) -> Result<Option<(CanonicalEvent, CanonicalEvent)>> {
    let names = Names::new(target.quoted_schema()?);
    let mut db = connect::connect(target)?;
    if !matches!(shape(&mut db, &target.schema)?, Shape::Lake) {
        return Ok(None);
    }
    let Some((tip, _)) = log::tip(&mut db, &names)? else {
        return Ok(None);
    };
    let mut at = |position| log::read(&mut db, &names, position, 1).map(|mut e| e.pop());
    Ok(at(Position(0))?.zip(at(tip)?))
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
        let tables = (!foreign.is_empty()).then(|| foreign.into_iter().collect());
        let schema = schema.to_owned();
        return Err(invalid(Case::SchemaNotALake { schema, tables }));
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
        let schema = schema.to_owned();
        return Err(invalid(Case::SchemaMetaForeign { schema }));
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
        db.batch(&format!(
            "INSERT INTO {} (k, n) VALUES ('layout', {}) ON CONFLICT DO NOTHING",
            names.t("meta"),
            projection::LAYOUT
        ))?;
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
        Some(held) => {
            let (schema, lake) = (Some(schema.to_owned()), Box::new(lake.clone()));
            Err(invalid(Case::LakeOther { schema, held, lake }))
        }
        None => Err(Error::unavailable(Situation::Unmade { part: Part::Name })),
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

    fn rebuild_rate(&self) -> u64 {
        REBUILD_RATE
    }

    fn lake(&self) -> &LakeId {
        &self.lake
    }

    fn take_lease(&self, holder: &str) -> Result<Lease> {
        let writing = self.writing()?;
        // The locks go in one order: `held`, the transaction, the lease's
        // connection, the projection's.
        let mut held = lock(&self.held);
        // A writer whose connection closed holds nothing: its lock went
        // with the session (lakelands#979). Both connections are opened
        // again, and the lease is taken as an open takes it.
        let mut transaction = lock(&self.transaction);
        let (mut lease, mut projection) = (lock(&writing.lease), lock(&writing.projection));
        if lease.is_closed() || projection.is_closed() {
            if let Some(old) = held.take().filter(|_| !lease.is_closed()) {
                let _ = lease::release(&mut lease, &self.names, &self.target.schema, &old);
            }
            *lease = connect::session_keeping_state(&self.target)?;
            *projection = connect::connect(&self.target)?;
            *transaction = None;
        }
        drop((transaction, lease, projection));
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
        self.reading(|db| log::tip(db, &self.names))
    }

    fn append(&self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        let mut db = self.holding(lease, Act::Appended)?;
        let appended = log::append(&mut db, &self.names, lease, events, false);
        self.lost(&db, Some(lease.fence), appended, Some(Act::Appended))
    }

    fn import(&self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        let mut db = self.holding(lease, Act::Appended)?;
        let imported = log::append(&mut db, &self.names, lease, events, true);
        self.lost(&db, Some(lease.fence), imported, Some(Act::Imported))
    }

    fn read(&self, from: Position, limit: u32) -> Result<Vec<CanonicalEvent>> {
        self.reading(|db| log::read(db, &self.names, from, limit))
    }

    fn seal_segment(
        &self,
        lease: &Lease,
        imported: bool,
        first: &mut dyn FnMut(&SegmentDigest) -> Result<Vec<CanonicalEvent>>,
    ) -> Result<Option<SegmentDigest>> {
        let mut db = self.holding(lease, Act::Sealed)?;
        let sealed = log::seal(&mut db, &self.names, lease, imported, first);
        self.lost(&db, Some(lease.fence), sealed, Some(Act::Sealed))
    }

    fn segments(&self) -> Result<Vec<SegmentDigest>> {
        self.reading(|db| log::segments(db, &self.names))
    }

    fn projection(&self) -> &dyn ProjectionStore {
        self
    }

    fn view(&self, as_of: Option<Position>) -> Result<Box<dyn ProjectionView + '_>> {
        let folded = self.reading(|db| projection::folded(db, &self.names))?;
        let at = engines::view_at(as_of, folded)?;
        Ok(Box::new(PgView { lake: self, at }))
    }

    /// One transaction's snapshot holds the position and every list. A
    /// snapshot does not hold back a rebuild's `clear`, which drops the
    /// tables and makes them again: the tables read here are taken in
    /// `ACCESS SHARE` mode before the snapshot is, in the order `clear`
    /// drops them, so a list is never an emptied table's under the
    /// position read before it (`crate::view`).
    fn ranked(&self, ty: &TypeName, asked: &Rankings, k: u32) -> Result<Ranked> {
        self.reading(|db| self.ranked_on(db, ty, asked, k))
    }

    fn vectors(&self) -> Option<&dyn VectorIndex> {
        Some(self)
    }
}

impl PgLake {
    /// [`LakeEngine::ranked`] on the reader `db`.
    fn ranked_on(&self, db: &mut Db, ty: &TypeName, asked: &Rankings, k: u32) -> Result<Ranked> {
        let read = |t: &&&str| match **t {
            "vectors" => !asked.vectors.is_empty(),
            "declarations" | "declarations_head" | "records" | "records_head" | "full_text" => {
                asked.text.is_some()
            }
            _ => false,
        };
        let tables: Vec<String> = PROJECTION_TABLES
            .iter()
            .filter(read)
            .map(|t| self.names.t(t))
            .collect();
        let lock = match tables.is_empty() {
            true => String::new(),
            false => format!("LOCK TABLE {} IN ACCESS SHARE MODE; ", tables.join(", ")),
        };
        let folded = db.simple(&format!(
            "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY; {lock}\
             SELECT n FROM {} WHERE k = 'folded'",
            self.names.t("meta")
        ));
        let ranked = folded.and_then(|folded| {
            let folded = folded.into_iter().flatten().next();
            let position = folded.and_then(|f| f.parse().ok()).map(Position);
            let mut lists = Vec::new();
            if let Some((text, fields)) = &asked.text {
                let fields = fields.as_deref();
                lists.push(crate::view::lexical(db, &self.names, ty, text, fields, k)?);
            }
            for (field, query) in &asked.vectors {
                lists.push(self.nearest(db, ty, field, query, k)?);
            }
            Ok(Ranked { position, lists })
        });
        db.end(ranked)
    }
}

impl ProjectionStore for PgLake {
    /// Opens a transaction. Beginning while one is open abandons the open
    /// one, as a rollback would. Its commit waits for no flush of its own:
    /// the next append, synced, flushes it, and a fold a crash takes is
    /// folded again from the log
    /// (`docs/decisions/061-a-projection-commit-waits-for-no-flush.md`).
    fn begin(&self) -> Result<()> {
        let writing = self.writing()?;
        let mut transaction = lock(&self.transaction);
        let mut db = self.projecting(writing)?;
        if transaction.take().is_some() {
            db.batch("ROLLBACK")?;
        }
        let begun =
            db.batch("BEGIN ISOLATION LEVEL READ COMMITTED; SET LOCAL synchronous_commit = off");
        self.lost(&db, None, begun, None)?;
        *transaction = Some(Transaction::default());
        Ok(())
    }

    fn commit(&self, through: Position) -> Result<()> {
        let writing = self.writing()?;
        let mut transaction = lock(&self.transaction);
        let t = transaction
            .take()
            .ok_or_else(|| invalid(Case::CommitUnbegun))?;
        let mut db = self.projecting(writing)?;
        // After a page the server refused, this read fails too: the
        // transaction is taken by now, so its session is rolled back here.
        let folded = projection::folded(&mut db, &self.names).or_else(|e| db.end(Err(e)));
        let folded = self.lost(&db, None, folded, None)?;
        if let Some(refused) = engines::commit_refusal(folded, t.latest, through) {
            return db.end(Err(refused));
        }
        let held = |(at, _): &(Position, _)| Some(*at) == folded;
        let mut heads = lock(&self.heads)
            .take()
            .filter(held)
            .map(|(_, heads)| heads);
        let Transaction {
            mut types,
            buffer,
            redeclared,
            mut asked,
            put,
            relook,
            ..
        } = t;
        // The last page, then the records a declaration left to index and
        // the vector indexes a setting asked for.
        let written = buffer
            .write(&mut db, &self.names, &mut types, &mut asked)
            .and_then(|rebuilt| {
                if relook.any() {
                    let read = || projection::head_declarations(&mut db, &self.names);
                    let (after, moved) = relook.heads(heads.take(), read)?;
                    projection::relook(&mut db, &self.names, &mut types, &after, moved.as_ref())?;
                    heads = Some(after);
                }
                let older = |(ty, at): (&TypeName, &Position)| {
                    projection::index_older(&mut db, &self.names, &mut types, ty, *at)
                };
                redeclared.iter().try_for_each(older)?;
                let v = self.pgvector.as_ref();
                asked.iter().try_for_each(|(ty, at)| {
                    vector_index::rebuild(&mut db, &self.names, v, ty, *at)
                })?;
                projection::set_folded(&mut db, &self.names, through)?;
                Ok(put + rebuilt)
            });
        let ended = db.end(written);
        let put = self.lost(&db, None, ended, None)?;
        *lock(&self.heads) = heads.map(|heads| (through, heads));
        // Once enough records went in since the last analysis, or order
        // index rows a setting rebuilt, the planner learns of them, after
        // the commit and outside it (module doc).
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
        self.reading(|db| projection::folded(db, &self.names))
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
        t.relook.declared(declaration);
        let mut db = self.projecting(writing)?;
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
        self.stage(record.position, |b| b.record(record))?;
        if let Some(t) = lock(&self.transaction).as_mut() {
            t.put += 1;
            t.relook.record(record);
        }
        Ok(())
    }

    fn end_head_only(&self, name: &TypeName, at: Position) -> Result<()> {
        engines::head_only(name)?;
        self.stage(at, |b| b.end_heads(name.as_str()))
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

    fn mark_applied(&self, applied: &Applied) -> Result<()> {
        self.stage(applied.at, |b| b.applied(applied))
    }

    /// Drops the projection, its kept history and the vector index, and
    /// makes the projection's tables again at this build's layout, marked
    /// with it; the log, the lease and the lake's name stay. An open
    /// transaction is abandoned.
    fn clear(&self) -> Result<()> {
        let writing = self.writing()?;
        *lock(&self.transaction) = None;
        // The tables start empty again, and so does the count (module doc).
        writing.unanalysed.store(0, Ordering::Relaxed);
        let mut db = self.projecting(writing)?;
        let _ = db.batch("ROLLBACK");
        let dropped: String = PROJECTION_TABLES
            .iter()
            .map(|t| format!("DROP TABLE IF EXISTS {} CASCADE;", self.names.t(t)))
            .collect();
        let vectors = match &self.pgvector {
            Some(v) => vector_index::prepare_sql(&self.names, v),
            None => String::new(),
        };
        db.batch(&format!(
            "BEGIN; {dropped} {schema} {vectors}; \
             DELETE FROM {m} WHERE k = 'folded' OR k LIKE 'vindex.%'; \
             INSERT INTO {m} (k, n) VALUES ('layout', {layout}) \
             ON CONFLICT (k) DO UPDATE SET n = EXCLUDED.n; COMMIT;",
            schema = self.names.schema_sql(),
            m = self.names.t("meta"),
            layout = projection::LAYOUT,
        ))
    }

    /// `VACUUM (ANALYZE)` over the projection's tables and its `meta`,
    /// outside a transaction; never `VACUUM FULL`, which would lock every
    /// reader out while it rewrote. Readers go on reading. A `VACUUM` that
    /// fails cannot be rolled back, but it changes no row a view reads at
    /// any point. The log's tables are not touched. It runs without
    /// parallel workers, which ask the server for shared memory of
    /// `maintenance_work_mem`'s size that a container's `/dev/shm` may not
    /// hold: a reclaim that fits beats a fast one that fails
    /// (lakelands#1161).
    fn compact(&self) -> Result<Reclaimed> {
        let writing = self.writing()?;
        let transaction = lock(&self.transaction);
        if transaction.is_some() {
            return Err(invalid(Case::CompactInTransaction));
        }
        let tables: Vec<&str> = PROJECTION_TABLES.iter().copied().chain(["meta"]).collect();
        let mut db = self.projecting(writing)?;
        // A transaction a failed commit left open is abandoned, as `clear`
        // abandons it: `VACUUM` cannot run inside one.
        let _ = db.batch("ROLLBACK");
        let before = size(&mut db, &self.target.schema, &tables)?;
        let listed: Vec<String> = tables.iter().map(|t| self.names.t(t)).collect();
        db.batch(&format!(
            "VACUUM (ANALYZE, PARALLEL 0) {}",
            listed.join(", ")
        ))?;
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

impl PgLake {
    /// The `k` vectors of one field nearest to `query`, read through `db`
    /// in its open transaction and ranked by the one cosine every engine
    /// uses (`lakelands_api::query::cosine`). The exact scan streams every
    /// vector of the field from the server a row at a time and keeps only
    /// the best `k` (`lakelands_core::engines::TopK`), so its cost grows
    /// linearly with them and what it holds does not: the ceiling
    /// `docs/seams.md` states, and its answers are those of every other
    /// engine to the last bit, but for the vectors of zeros it leaves out
    /// where it answers for an index. A field read through its index holds
    /// the candidates of one pull, `k` and one more or a tie's deeper pull.
    fn nearest(
        &self,
        db: &mut Db,
        ty: &TypeName,
        field: &str,
        query: &[f32],
        k: u32,
    ) -> Result<Vec<(RecordKey, f64)>> {
        let mut indexed = false;
        // A field the lake asks an index for, whose index describes the
        // head, answers the index's candidates, ranked by the one cosine
        // (`crate::vector_index`); every other search scans. The index
        // orders equal distances its own way, so a tie the `k`th
        // candidate stands in is pulled before it is cut by key: one
        // candidate more shows whether the boundary cuts one, and a cut
        // one is pulled deeper, to the scan past the ceiling. An index
        // that hands out fewer than it was asked for has not thereby
        // shown the field's end, so the scan answers then too. The
        // index's settings end with its statement: the transaction
        // reads other lists.
        if let (Some(v), Some(unit)) = (&self.pgvector, vector_index::unit_text(query))
            && let Some(n) = vector_index::usable(db, &self.names, ty, field)?
            && n as usize == query.len()
        {
            indexed = true;
            let sql = vector_index::candidates_sql(&self.names, v, ty, field, n);
            let first = k.saturating_add(1);
            let mut pull = Some(first);
            while let Some(depth) = pull {
                let mut came = Vec::new();
                db.batch(&vector_index::search_settings(depth))?;
                db.query_each(&sql, &[&unit, &i64::from(depth)], |row| {
                    let v = vector_of(&row.get::<_, Vec<u8>>(1));
                    came.push((RecordKey::from(row.get::<_, String>(0)), cosine(query, &v)));
                    Ok(())
                })?;
                db.batch("SET LOCAL enable_seqscan = on; SET LOCAL enable_sort = on")?;
                if came.len() < depth as usize {
                    break;
                }
                if !vector_index::cuts_a_tie(&came, k) {
                    // Where no tie was cut the candidate more only showed
                    // it, and the answer is the index's `k`.
                    if depth == first {
                        came.truncate(k as usize);
                    }
                    engines::rank(&mut came, k);
                    return Ok(came);
                }
                pull = vector_index::deeper(depth);
            }
        }
        // The exact scan checks every vector's dimensions; the index holds
        // only vectors of the query's.
        let mut top = lakelands_core::engines::TopK::new(k);
        db.query_each(
            &format!(
                "SELECT key, data FROM {} WHERE ty = $1 AND field = $2 ORDER BY key COLLATE \"C\"",
                self.names.t("vectors")
            ),
            &[&ty.as_str(), &field],
            |row| {
                let key: String = row.get(0);
                let v = vector_of(&row.get::<_, Vec<u8>>(1));
                if v.len() != query.len() {
                    return Err(engines::dimensions(query.len(), v.len(), ty, field));
                }
                // The index never hands out a vector of zeros, so the scan
                // that answers for it leaves one out too: a hit must not
                // show which of the two answered, and so how far the field
                // goes (043).
                if !indexed || vector_index::length(&v).is_some() {
                    top.push(&RecordKey::from(key), cosine(query, &v));
                }
                Ok(())
            },
        )?;
        Ok(top.into_ranked())
    }
}

/// The vector index's write side and its size: each field's vectors in
/// one table, indexed at the head only, in the fold's transaction.
impl VectorIndex for PgLake {
    fn put(&self, ty: &TypeName, key: &RecordKey, field: &str, vector: &[f32]) -> Result<()> {
        let writing = self.writing()?;
        let mut db = self.projecting(writing)?;
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
        self.projecting(writing)?.execute(
            &format!(
                "DELETE FROM {} WHERE ty = $1 AND field = $2 AND key = $3",
                self.names.t("vectors")
            ),
            &[&ty.as_str(), &field, &key.as_str()],
        )
    }

    fn count(&self, ty: &TypeName, field: &str) -> Result<u64> {
        let statement = format!(
            "SELECT count(*) FROM {} WHERE ty = $1 AND field = $2",
            self.names.t("vectors")
        );
        let row = self.reading(|db| db.query_one(&statement, &[&ty.as_str(), &field]))?;
        let n: i64 = row.get(0);
        Ok(n as u64)
    }
}

/// Write a lake's log as the segment files `docs/log-format.md` §4.2
/// describes, `<dir>/log/<first>.jsonl`, over a connection of its own
/// that only reads. A raw copy of the log's lines, for a lake whose
/// store cannot be opened: no event is judged, not its format either,
/// and what is written is judged by whoever verifies or imports it.
/// The schema must hold a lake of the location's name; nothing of its
/// projection is read, so one of another layout, or none, is no bar.
pub fn export(location: &lakelands_api::Location, dir: &std::path::Path) -> Result<()> {
    let target = crate::target(location)?;
    let names = Names::new(target.quoted_schema()?);
    let mut db = connect::connect(&target)?;
    if !matches!(shape(&mut db, &target.schema)?, Shape::Lake) {
        let (lake, schema) = (location.lake.to_string(), Some(target.schema.clone()));
        return Err(Error::unavailable(Situation::NoLake { lake, schema }));
    }
    check_name(&mut db, &names, &target.schema, &location.lake)?;
    log::export(&mut db, &names, dir)
}
