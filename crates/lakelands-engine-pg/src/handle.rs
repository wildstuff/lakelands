//! A handle onto one lake's schema: the log and lease, the projection's
//! write side, the vector index.
//!
//! Any number of handles, in this process or others, see one lake. A
//! handle reads, views and log, on the readers the process's lakes on its
//! server share ([`connect::shared_readers`]), at most [`LAKE_READERS`] of
//! them at once, and a handle opened for writing keeps two connections of
//! its own:
//! - a lease connection, which holds the lease's lock, appends and seals;
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
//! its last analysis, it runs `ANALYZE` over `records`, the head indexes
//! and the vectors after the commit has answered, outside the fold's
//! transaction. A rebuild is a refold after `clear`, which starts the
//! count again, so it is analysed as a fresh load is: after its first
//! `ANALYZE_AFTER` records, and a lake smaller than that is left to
//! PostgreSQL's own. Analysis changes no answer, only a plan; a failed
//! one leaves the commit as it is.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use lakelands_api::engine::{
    Applied, CanonicalEvent, Choose, Declared, LakeEngine, Lease, OpenMode, ProjectionStore,
    ProjectionView, Ranked, Rankings, Reclaimed, StatesOf, VectorIndex,
};
use lakelands_api::error::{Act, Case, Error, Part, Result, Situation};
use lakelands_api::event::{Digest, SegmentDigest};
use lakelands_api::id::{LakeId, Position, RecordKey, TypeName};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::write::{Declaration, Proposal};
use lakelands_core::engines;
use lakelands_core::engines::pool::{Pool, Taken};

use crate::connect::{self, Db, Target};
use crate::projection::{self, Buffer, HeadTypes, Names, PROJECTION_TABLES, RETIRED_TABLES};
use crate::scratch;
use crate::vector_index::{self, Builder, Pgvector, Touched};
use crate::view::PgView;
use crate::{NAME, REBUILD_RATE, lease, lock, log};

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
    /// the setting's position, and the types declared in it: the commit
    /// builds their graphs again, once.
    asked: BTreeMap<TypeName, Position>,
    declared: BTreeSet<TypeName>,
    /// The records its pages wrote, erased or indexed, for the graphs.
    touched: Touched,
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

/// The connections a handle opened for writing keeps, what the writer
/// carries into its appends, and the records put since the tables were
/// last analysed.
struct Writing {
    lease: Mutex<Db>,
    projection: Mutex<Db>,
    carried: Mutex<Option<log::Carried>>,
    unanalysed: AtomicU64,
}

/// One handle onto a lake kept by the PostgreSQL engine.
pub struct PgLake {
    lake: LakeId,
    target: Target,
    pub(crate) names: Names,
    readers: Arc<Pool<Db>>,
    /// This handle's turns at `readers`, [`LAKE_READERS`] of them.
    turns: Pool<()>,
    writing: Option<Writing>,
    held: Mutex<Option<Lease>>,
    transaction: Mutex<Option<Transaction>>,
    /// The head's declarations as the last commit left them, and the
    /// position it folded through, for the next one's relook.
    heads: Mutex<Option<(Position, engines::Heads)>>,
    /// pgvector, when the server has it: the vector index a lake asks for
    /// lives there (`crate::vector_index`).
    pgvector: Option<Pgvector>,
    /// The writer's builder of the graphs (`crate::vector_index`).
    builder: Option<Builder>,
    /// The position through which a fold may have put an entry in a graph
    /// and not committed: until a commit reaches it, a hold marks B stale
    /// whatever the record's states (`vector_index::follow`).
    suspect: Mutex<Option<Position>>,
    /// On a handle onto the scratch, its schema, unquoted (`scratch.rs`);
    /// the lease connection holds the builder's lock.
    scratch: Option<String>,
}

impl PgLake {
    pub(crate) fn open(lake: LakeId, target: Target, mode: OpenMode) -> Result<PgLake> {
        let names = Names::of(&target)?;
        let (readers, turns) = (connect::shared_readers(&target), turns());
        let shaped = |db: &mut Db| shape(db, &target.schema, false);
        let (mut reader, turn, shaped) = read_on(&readers, &turns, &target, shaped)?;
        let exists = matches!(shaped, Shape::Lake);
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
            engines::layout(&lake, found.map(|v| v as u64), projection::LAYOUT)?;
        }
        let read = mode == OpenMode::Read;
        let read = read.then(|| vector_index::find(&mut reader, false).ok());
        // Given back before the writer's connections catch up, however
        // long that takes: other lakes read on it.
        drop((reader, turn));
        let (writing, pgvector) = match mode {
            OpenMode::Read => (None, read.flatten()),
            OpenMode::Write | OpenMode::Create => {
                let lease = connect::session_keeping_state(&target)?;
                let mut projection = connect::connect(&target)?;
                prepare(&mut projection, &names, &target.schema, &lake, mode)?;
                projection::catch_up_sorted(&mut projection, &names)?;
                projection::catch_up_text_stats(&mut projection, &names)?;
                scratch::sweep(&mut projection, &target, &names, &lake)?;
                let found = vector_index::find(&mut projection, true);
                let why = found.as_ref().map_err(String::as_str);
                vector_index::opened(&mut projection, &names, why, lake.as_str())?;
                vector_index::sweep(&mut projection, &names)?;
                let writing = Writing {
                    lease: Mutex::new(lease),
                    projection: Mutex::new(projection),
                    carried: Mutex::new(None),
                    unanalysed: AtomicU64::new(0),
                };
                (Some(writing), found.ok())
            }
            OpenMode::Rebuild => {
                let lease = connect::session_keeping_state(&target)?;
                let mut projection = connect::connect(&target)?;
                scratch::sweep(&mut projection, &target, &names, &lake)?;
                let found = vector_index::find(&mut projection, true).ok();
                let writing = Writing {
                    lease: Mutex::new(lease),
                    projection: Mutex::new(projection),
                    carried: Mutex::new(None),
                    unanalysed: AtomicU64::new(0),
                };
                (Some(writing), found)
            }
        };
        read_on(&readers, &turns, &target, |db| {
            check_name(db, &names, &target.schema, &lake)
        })?;
        let (builder, suspect) = match &writing {
            Some(w) => {
                let mut db = lock(&w.projection);
                let tip = log::tip(&mut db, &names)?.map(|(p, _)| p);
                let builder = match &pgvector {
                    Some(v) => {
                        // A rebuild's projection may be another layout's,
                        // without graphs; its scratch is swapped in built.
                        let waiting =
                            mode != OpenMode::Rebuild && vector_index::waiting(&mut db, &names)?;
                        Some(Builder::start(
                            target.clone(),
                            names.clone(),
                            v.clone(),
                            waiting,
                        ))
                    }
                    None => None,
                };
                (builder, tip)
            }
            None => (None, None),
        };
        Ok(PgLake {
            lake,
            target,
            names,
            readers,
            turns,
            writing,
            held: Mutex::new(None),
            transaction: Mutex::new(None),
            heads: Mutex::new(None),
            pgvector,
            builder,
            suspect: Mutex::new(suspect),
            scratch: None,
        })
    }

    /// A handle onto the scratch beside the lake in the target's schema:
    /// it reads the log as a reader does, and folds into the scratch,
    /// never the projection. A scratch unfit to keep is removed and made
    /// anew.
    pub(crate) fn open_scratch(lake: LakeId, target: Target) -> Result<PgLake> {
        let live = Names::of(&target)?;
        let (readers, turns) = (connect::shared_readers(&target), turns());
        // The projection's tables are another layout's, perhaps: only the
        // log's are judged.
        let shaped = |db: &mut Db| shape(db, &target.schema, true);
        let (mut reader, turn, shaped) = read_on(&readers, &turns, &target, shaped)?;
        if !matches!(shaped, Shape::Lake) {
            let (lake, schema) = (lake.to_string(), Some(target.schema.clone()));
            return Err(Error::unavailable(Situation::NoLake { lake, schema }));
        }
        check_name(&mut reader, &live, &target.schema, &lake)?;
        drop((reader, turn));
        let bare = target.scratch_schema();
        let names = Names::beside(Target::quote(&bare), &live);
        let mut lease = connect::session_keeping_state(&target)?;
        if !scratch::lock(&mut lease, &target.schema, true)? {
            return Err(Error::unavailable(Situation::Rebuilding {
                lake: lake.to_string(),
            }));
        }
        let mut projection = connect::connect(&target)?;
        projection.transaction("BEGIN ISOLATION LEVEL READ COMMITTED", |db| {
            if scratch::fit(db, &names, &bare, &lake, false)?.is_none() {
                db.batch(&scratch::remove_sql(&names))?;
            }
            db.batch(&names.schema_sql())?;
            db.batch(&format!(
                "INSERT INTO {m} (k, n) VALUES ('layout', {layout}) ON CONFLICT DO NOTHING; \
                 INSERT INTO {m} (k, n) VALUES ('{stats}', NULL) ON CONFLICT DO NOTHING",
                m = names.t("meta"),
                layout = projection::LAYOUT,
                stats = projection::TEXT_STATS,
            ))?;
            db.execute(
                &format!(
                    "INSERT INTO {} (k, t) VALUES ('lake', $1) ON CONFLICT DO NOTHING",
                    names.t("meta")
                ),
                &[&lake.as_str()],
            )
        })?;
        projection::catch_up_sorted(&mut projection, &names)?;
        projection::catch_up_text_stats(&mut projection, &names)?;
        let found = vector_index::find(&mut projection, true);
        let why = found.as_ref().map_err(String::as_str);
        vector_index::opened(&mut projection, &names, why, lake.as_str())?;
        let tip = log::tip(&mut projection, &names)?.map(|(p, _)| p);
        Ok(PgLake {
            lake,
            target,
            names,
            readers,
            turns,
            writing: Some(Writing {
                lease: Mutex::new(lease),
                projection: Mutex::new(projection),
                carried: Mutex::new(None),
                unanalysed: AtomicU64::new(0),
            }),
            held: Mutex::new(None),
            transaction: Mutex::new(None),
            heads: Mutex::new(None),
            pgvector: found.ok(),
            builder: None,
            suspect: Mutex::new(tip),
            scratch: Some(bare),
        })
    }

    /// The refusal of the lease, or a write of the log, through a handle
    /// opened read-only or onto the scratch.
    fn read_only(&self) -> Error {
        let (lake, leased_elsewhere) = (Some(self.lake.to_string()), false);
        Error::unavailable(Situation::ReadOnly {
            lake,
            leased_elsewhere,
        })
    }

    /// Put the scratch in the projection's place, checked as
    /// [`LakeEngine::swap`] says, in one transaction (`scratch.rs`). A
    /// reader's statement waits for it, and reads the projection it put in
    /// place once it commits.
    fn swapping(&self, lease: &Lease) -> Result<Option<Position>> {
        let writing = self.writing()?;
        if self.scratch.is_some() || lock(&self.held).as_ref() != Some(lease) {
            let (fence, undone) = (Some(lease.fence), None);
            return Err(Error::unavailable(Situation::LeaseNotHeld {
                fence,
                undone,
            }));
        }
        *lock(&self.transaction) = None;
        let mut db = self.projecting(writing)?;
        let _ = db.batch("ROLLBACK");
        let _held = self.builder.as_ref().map(|b| b.hold(&mut db));
        let bare = self.target.scratch_schema();
        let names = Names::beside(Target::quote(&bare), &self.names);
        let lake = self.lake.to_string();
        let swapped = db.transaction("BEGIN ISOLATION LEVEL READ COMMITTED", |db| {
            if !scratch::lock(db, &self.target.schema, false)? {
                return Err(Error::unavailable(Situation::Rebuilding {
                    lake: lake.clone(),
                }));
            }
            let Some(marked) = scratch::fit(db, &names, &bare, &self.lake, false)? else {
                return Err(Error::unavailable(Situation::NoScratch {
                    lake: lake.clone(),
                }));
            };
            // The mark's hash is the log's there (`fit`): it is the tip's
            // where it is the tip's position.
            let tip = log::tip(db, &self.names)?.map(|(p, _)| p);
            if tip != Some(marked) {
                let marked = Some(marked);
                return Err(Error::unavailable(Situation::ScratchBehind {
                    lake: lake.clone(),
                    marked,
                    tip,
                }));
            }
            db.batch(&scratch::swap_sql(&self.names, &names))?;
            Ok(Some(marked))
        });
        let swapped = self.lost(&db, None, swapped, None)?;
        *lock(&self.heads) = None;
        writing.unanalysed.store(0, Ordering::Relaxed);
        Ok(swapped)
    }

    /// Run `f` on a reader of the process's lakes on the server, so the
    /// reads of one lake run side by side ([`read_on`]).
    pub(crate) fn reading<R>(&self, f: impl FnMut(&mut Db) -> Result<R>) -> Result<R> {
        read_on(&self.readers, &self.turns, &self.target, f).map(|(_, _, answer)| answer)
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
        self.writing.as_ref().ok_or_else(|| self.read_only())
    }

    /// The lease connection and what the writer carries, when this handle
    /// holds `lease`; otherwise it is `undone`.
    fn holding(
        &self,
        lease: &Lease,
        undone: Act,
    ) -> Result<(MutexGuard<'_, Db>, MutexGuard<'_, Option<log::Carried>>)> {
        let writing = self.writing()?;
        if lock(&self.held).as_ref() != Some(lease) {
            let (fence, undone) = (Some(lease.fence), Some(undone));
            return Err(Error::unavailable(Situation::LeaseNotHeld {
                fence,
                undone,
            }));
        }
        let db = lock(&writing.lease);
        self.lost(&db, Some(lease.fence), Ok(()), Some(undone))?;
        Ok((db, lock(&writing.carried)))
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
        let (types, asked, touched) = (&mut t.types, &mut t.asked, &mut t.touched);
        t.put += page.write(&mut db, &self.names, types, asked, touched)?;
        Ok(())
    }

    /// Put `vector` into the vector index for `ty`'s `key`, or remove it
    /// where none: taken by the transaction's buffer where it holds a
    /// state of the key, else made now.
    fn index(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        field: &str,
        vector: Option<&[f32]>,
    ) -> Result<()> {
        let writing = self.writing()?;
        let at = (ty.as_str(), key.as_str(), field);
        let put = |db: &mut Db| match vector {
            Some(vector) => vector_index::put(db, &self.names, at, vector),
            None => vector_index::remove(db, &self.names, at),
        };
        let mut transaction = lock(&self.transaction);
        if let Some(t) = transaction.as_mut() {
            if !t.buffer.index(at, vector) {
                put(&mut *self.projecting(writing)?)?;
                let keys = t.touched.keys.entry(ty.clone()).or_default();
                keys.insert(key.to_string());
            }
            return Ok(());
        }
        // Outside the fold's transaction the graphs follow at once.
        let mut db = self.projecting(writing)?;
        let mut touched = Touched::default();
        touched
            .keys
            .entry(ty.clone())
            .or_default()
            .insert(key.to_string());
        let left = db.transaction("BEGIN ISOLATION LEVEL READ COMMITTED", |db| {
            vector_index::lock_fold(db, &self.names)?;
            put(db)?;
            let Some(folded) = projection::folded(db, &self.names)? else {
                return Ok(false);
            };
            let v = self.pgvector.as_ref();
            vector_index::follow(
                db,
                &self.names,
                v,
                folded,
                &touched,
                &BTreeSet::new(),
                false,
            )
        })?;
        if left {
            self.wake();
        }
        Ok(())
    }

    /// Ask the builder to look at the graphs' rows, once a write left it a
    /// graph to build or a table to drop.
    fn wake(&self) {
        if let Some(b) = &self.builder {
            b.wake();
        }
    }

    /// A fold through `latest` was given up, having perhaps put entries
    /// in a graph (`suspect`).
    fn given_up(&self, latest: Option<Position>) {
        let mut suspect = lock(&self.suspect);
        *suspect = (*suspect).max(latest);
    }
}

/// The first and the last event of the log in the target's schema, when
/// it holds a lake with any: read, never made or changed.
pub(crate) fn log_ends(target: &Target) -> Result<Option<(CanonicalEvent, CanonicalEvent)>> {
    let names = Names::of(target)?;
    let mut db = connect::connect(target)?;
    if !matches!(shape(&mut db, &target.schema, false)?, Shape::Lake) {
        return Ok(None);
    }
    let Some((tip, _)) = log::tip(&mut db, &names)? else {
        return Ok(None);
    };
    let mut at = |position| log::read(&mut db, &names, position, 1).map(|mut e| e.pop());
    Ok(at(Position(0))?.zip(at(tip)?))
}

/// The bytes the projection's tables in the target's schema take, with
/// their indexes and what PostgreSQL keeps out of line: measured where no
/// session holds a table against it for a second, and otherwise as the
/// catalogue last recorded it, which waits on no one.
pub(crate) fn room(target: &Target) -> Result<u64> {
    const TABLES: &str = "FROM pg_catalog.pg_class c \
        JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
        WHERE n.nspname = $1 AND c.relkind = 'r' AND c.relname <> ALL($2)";
    // A table's pages, its out-of-line pages and its indexes' pages, as
    // the catalogue last recorded them.
    const RECORDED: &str = "(SELECT coalesce(sum(r.relpages), 0) FROM pg_catalog.pg_class r \
        WHERE r.oid = c.reltoastrelid \
        OR r.oid IN (SELECT indexrelid FROM pg_catalog.pg_index WHERE indrelid = c.oid))";
    let mut db = connect::connect(target)?;
    let log = projection::LAKE_TABLES[..5].to_vec();
    let args: &[&(dyn postgres::types::ToSql + Sync)] = &[&target.schema, &log];
    db.batch("SET lock_timeout = '1s'")?;
    let measured = format!(
        "SELECT coalesce(sum(pg_catalog.pg_total_relation_size(c.oid)), 0)::bigint {TABLES}"
    );
    let row = match db.query_one(&measured, args) {
        Ok(row) => row,
        Err(_) => db.query_one(
            &format!(
                "SELECT (coalesce(sum(c.relpages + {RECORDED}), 0) \
                 * current_setting('block_size')::bigint)::bigint {TABLES}"
            ),
            args,
        )?,
    };
    Ok(u64::try_from(row.get::<_, i64>(0)).unwrap_or(0))
}

/// The most of the shared readers one handle's reads hold at once: half,
/// so the reads of a handle that wait, on a lock another session holds or
/// on a rebuild's swap, leave the other half to the process's other
/// handles. A lake with two handles open, a rebuild's among them, can
/// hold them all.
pub const LAKE_READERS: usize = connect::SHARED_READERS / 2;

fn turns() -> Pool<()> {
    Pool::new(Vec::new(), LAKE_READERS)
}

/// A reader of `readers` in one of `turns`, and what `f` answered on it.
/// A connection the server or the network closed is opened again, once,
/// and `f` runs on the new one: a second failure is answered as it is
/// (lakelands#979).
fn read_on<'p, R>(
    readers: &'p Pool<Db>,
    turns: &'p Pool<()>,
    target: &Target,
    mut f: impl FnMut(&mut Db) -> Result<R>,
) -> Result<(Taken<'p, Db>, Taken<'p, ()>, R)> {
    let turn = turns.take(|| Ok(()))?;
    let mut db = readers.take(|| connect::reader(target))?;
    if db.is_closed() {
        *db = connect::reader(target)?;
    }
    let answer = match f(&mut db) {
        Err(_) if db.is_closed() => {
            *db = connect::reader(target)?;
            f(&mut db)
        }
        answer => answer,
    };
    Ok((db, turn, answer?))
}

/// What a schema holds, as the engine sees it at open.
enum Shape {
    /// No schema, or one holding no table.
    Empty,
    /// A lake's tables (some may be added by a later version).
    Lake,
}

/// Look at the schema: empty, a lake's, or something else, which is
/// refused. One schema holds one lake and nothing beside it. `beside`, for
/// a scratch's open, which reads the log and nothing of the projection,
/// leaves tables this build does not know to the layout that made them.
fn shape(db: &mut Db, schema: &str, beside: bool) -> Result<Shape> {
    let tables: Vec<String> = db.column(
        "SELECT tablename FROM pg_catalog.pg_tables WHERE schemaname = $1 ORDER BY tablename",
        &[&schema],
    )?;
    if tables.is_empty() {
        return Ok(Shape::Empty);
    }
    let foreign: Vec<&String> = tables
        .iter()
        .filter(|t| !beside && !projection::LAKE_TABLES.contains(&t.as_str()))
        .filter(|t| !t.starts_with("vgraph_"))
        .filter(|t| !RETIRED_TABLES.contains(&t.as_str()))
        .collect();
    if !tables.iter().any(|t| t == "meta") || !foreign.is_empty() {
        let tables = (!foreign.is_empty()).then(|| foreign.into_iter().collect());
        let schema = schema.to_owned();
        return Err(Error::invalid(Case::SchemaNotALake { schema, tables }));
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
        return Err(Error::invalid(Case::SchemaMetaForeign { schema }));
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
            "INSERT INTO {m} (k, n) VALUES ('layout', {}) ON CONFLICT DO NOTHING; \
             INSERT INTO {m} (k, n) VALUES ('{}', NULL) ON CONFLICT DO NOTHING",
            projection::LAYOUT,
            projection::TEXT_STATS,
            m = names.t("meta"),
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
            Err(Error::invalid(Case::LakeOther { schema, held, lake }))
        }
        None => Err(Error::unavailable(Situation::Unmade { part: Part::Name })),
    }
}

impl Drop for PgLake {
    /// A handle that is dropped gives up the lease it holds, as a session
    /// that ends does.
    fn drop(&mut self) {
        if let (Some(builder), Some(writing)) = (&self.builder, &self.writing) {
            let mut db = lock(&writing.projection);
            let _ = db.batch("ROLLBACK");
            builder.stop(&mut db);
        }
        let held = lock(&self.held).clone();
        if let Some(lease) = held {
            let _ = self.release_lease(&lease);
        }
        // A scratch not marked built through what it folded is removed
        // before its lock is given up.
        if let (Some(bare), Some(writing)) = (&self.scratch, &self.writing) {
            let mut db = lock(&writing.projection);
            let _ = db.batch("ROLLBACK");
            if !matches!(
                scratch::fit(&mut db, &self.names, bare, &self.lake, false),
                Ok(Some(_))
            ) {
                let _ = db.batch(&scratch::remove_sql(&self.names));
            }
            drop(db);
            let _ = scratch::unlock(&mut lock(&writing.lease), &self.target.schema);
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
        if self.scratch.is_some() {
            return Err(self.read_only());
        }
        // The locks go in one order: `held`, the transaction, the lease's
        // connection, the projection's, what the writer carries.
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
        let (lease, carried) = lease::take(
            &mut lock(&writing.lease),
            &self.names,
            &self.target.schema,
            holder,
        )?;
        *lock(&writing.carried) = Some(carried);
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

    fn lease_free(&self) -> Result<bool> {
        self.reading(|db| lease::free(db, &self.target.schema))
    }

    fn tip(&self) -> Result<Option<(Position, Digest)>> {
        self.reading(|db| log::tip(db, &self.names))
    }

    fn append(&self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        let (mut db, mut carried) = self.holding(lease, Act::Appended)?;
        let appended = log::append(&mut db, &self.names, lease, &mut carried, events, false);
        self.lost(&db, Some(lease.fence), appended, Some(Act::Appended))
    }

    fn import(&self, lease: &Lease, events: &[CanonicalEvent]) -> Result<()> {
        let (mut db, mut carried) = self.holding(lease, Act::Appended)?;
        let imported = log::append(&mut db, &self.names, lease, &mut carried, events, true);
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
        let (mut db, mut carried) = self.holding(lease, Act::Sealed)?;
        let sealed = log::seal(&mut db, &self.names, lease, &mut carried, imported, first);
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
    /// position read before it (`crate::view`). `text_stats` is not
    /// taken: a lake an earlier build made has none until a writer opens
    /// it, and `clear` drops it after every table taken here.
    fn ranked(&self, ty: &TypeName, asked: &Rankings, k: u32) -> Result<Ranked> {
        self.reading(|db| self.ranked_on(db, ty, asked, k))
    }

    fn vectors(&self) -> Option<&dyn VectorIndex> {
        Some(self)
    }

    /// Writes the mark in a transaction of its own, beside the position
    /// folded through.
    fn mark_built(&self) -> Result<Option<Position>> {
        let (Some(_), Ok(writing)) = (&self.scratch, self.writing()) else {
            let lake = self.lake.to_string();
            return Err(Error::unavailable(Situation::NoScratch { lake }));
        };
        *lock(&self.transaction) = None;
        let mut db = self.projecting(writing)?;
        let _ = db.batch("ROLLBACK");
        // A scratch is marked with its graphs built.
        if let Some(v) = &self.pgvector {
            let built = vector_index::build_scratch(&mut db, &self.names, v);
            self.lost(&db, None, built, None)?;
        }
        let meta = self.names.t("meta");
        let marked = db.transaction("BEGIN ISOLATION LEVEL READ COMMITTED", |db| {
            let folded = db.query_opt(&format!("SELECT n FROM {meta} WHERE k = 'folded'"), &[])?;
            let folded = folded
                .and_then(|r| r.get::<_, Option<i64>>(0))
                .map(|n| Position(n as u64));
            db.execute(&format!("DELETE FROM {meta} WHERE k = 'built'"), &[])?;
            let Some(at) = folded else {
                return Ok(None);
            };
            let Some(e) = log::read(db, &self.names, at, 1)?
                .pop()
                .filter(|e| e.position == at)
            else {
                let tip = log::tip(db, &self.names)?.map(|(p, _)| p);
                let lake = self.lake.to_string();
                return Err(Error::unavailable(Situation::ScratchBehind {
                    lake,
                    marked: folded,
                    tip,
                }));
            };
            let mark = format!("INSERT INTO {meta} (k, n, t) VALUES ('built', $1, $2)");
            db.execute(&mark, &[&(at.0 as i64), &e.hash.0])?;
            Ok(folded)
        });
        self.lost(&db, None, marked, None)
    }

    fn swap(&self, lease: &Lease) -> Result<Option<Position>> {
        self.swapping(lease)
    }
}

impl PgLake {
    /// [`LakeEngine::ranked`] on the reader `db`.
    fn ranked_on(&self, db: &mut Db, ty: &TypeName, asked: &Rankings, k: u32) -> Result<Ranked> {
        let vectors = !asked.vectors.is_empty();
        let read = |t: &&&str| match **t {
            "vectors" | "graphs" => vectors,
            "records" | "records_head" if vectors && asked.seen.is_some() => true,
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
            if asked.text.is_some() {
                lists.push(crate::view::lexical(db, &self.names, ty, asked, k)?);
            }
            let decl = match &asked.seen {
                Some(_) if vectors => projection::head_type(db, &self.names, ty)?.decl,
                _ => None,
            };
            let seen = asked.seen.as_ref().map(|s| (s, decl.as_ref()));
            let (names, v) = (&self.names, self.pgvector.as_ref());
            for (field, query) in &asked.vectors {
                let at = (ty, field.as_str());
                let exact = asked.exact;
                lists.push(vector_index::nearest(
                    db, names, v, at, query, k, seen, exact,
                )?);
            }
            Ok(Ranked {
                position,
                lists,
                judged: asked.seen.is_some(),
            })
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
        if let Some(t) = transaction.take() {
            self.given_up(t.latest);
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
            .ok_or_else(|| Error::invalid(Case::CommitUnbegun))?;
        let mut db = self.projecting(writing)?;
        // After a page the server refused, this read fails too: the
        // transaction is taken by now, so its session is rolled back here.
        let folded = projection::folded(&mut db, &self.names).or_else(|e| db.end(Err(e)));
        let folded = self.lost(&db, None, folded, None)?;
        if let Some(refused) = engines::commit_refusal(folded, t.latest, through) {
            self.given_up(t.latest);
            return db.end(Err(refused));
        }
        let latest = t.latest.max(Some(through));
        let suspect = lock(&self.suspect).is_some_and(|s| folded.is_none_or(|f| f < s));
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
            declared,
            mut touched,
            put,
            relook,
            ..
        } = t;
        // The last page, then the records a declaration left to index, the
        // graphs a setting or a declaration asks for, and the graphs'
        // entries of what it touched.
        let mut left = false;
        let written = buffer
            .write(&mut db, &self.names, &mut types, &mut asked, &mut touched)
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
                let (v, build) = (self.pgvector.as_ref(), self.scratch.is_none());
                if v.is_some() {
                    // A build's swap waits for this commit, or this for it.
                    vector_index::lock_fold(&mut db, &self.names)?;
                }
                let again: BTreeSet<TypeName> = asked.into_keys().chain(declared).collect();
                for ty in &again {
                    vector_index::rebuild(&mut db, &self.names, v, ty, build)?;
                }
                left = vector_index::follow(
                    &mut db,
                    &self.names,
                    v,
                    through,
                    &touched,
                    &again,
                    suspect,
                )? || (v.is_some() && !again.is_empty());
                projection::set_folded(&mut db, &self.names, through)?;
                Ok(put + rebuilt)
            });
        let ended = db.end(written);
        if ended.is_err() {
            self.given_up(latest);
        }
        let put = self.lost(&db, None, ended, None)?;
        {
            let mut suspect = lock(&self.suspect);
            if suspect.is_some_and(|s| s <= through) {
                *suspect = None;
            }
        }
        if left {
            self.wake();
        }
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
        if let Some(t) = lock(&self.transaction).take() {
            self.given_up(t.latest);
        }
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
        // type's graphs are built again under it at the commit.
        if let Declaration::Type(ty) = declaration {
            t.declared.insert(ty.name.clone());
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

    fn drop_states(&self, of: StatesOf<'_>) -> Result<()> {
        self.stage(Position::GENESIS, |b| match of {
            StatesOf::Record(ty, key) => b.drop_record(ty.as_str(), key.as_str()),
            StatesOf::Output(seam, input) => b.drop_output(seam, input.0.as_str()),
            StatesOf::Bases => b.drop_bases(),
        })
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
        let _held = self.builder.as_ref().map(|b| b.hold(&mut db));
        let dropped: String = PROJECTION_TABLES
            .iter()
            .chain(RETIRED_TABLES)
            .map(|t| format!("DROP TABLE IF EXISTS {} CASCADE;", self.names.t(t)))
            .collect();
        let graphs = vector_index::tables_sql(&self.names, None);
        db.batch(&format!(
            "BEGIN; {dropped} {graphs} {schema} \
             DELETE FROM {m} WHERE k = 'folded' OR k LIKE 'vindex.%' OR k LIKE 'chosen.%' \
             OR k = '{chosen}'; \
             INSERT INTO {m} (k, n) VALUES ('layout', {layout}), ('{stats}', NULL) \
             ON CONFLICT (k) DO UPDATE SET n = EXCLUDED.n; COMMIT;",
            schema = self.names.schema_sql(),
            m = self.names.t("meta"),
            layout = projection::LAYOUT,
            chosen = engines::CHOSEN_THROUGH,
            stats = projection::TEXT_STATS,
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
            return Err(Error::invalid(Case::CompactInTransaction));
        }
        let tables: Vec<&str> = PROJECTION_TABLES.iter().copied().chain(["meta"]).collect();
        let mut db = self.projecting(writing)?;
        // A transaction a failed commit left open is abandoned, as `clear`
        // abandons it: `VACUUM` cannot run inside one.
        let _ = db.batch("ROLLBACK");
        let before = size(&mut db, &self.target.schema, &tables)?;
        let mut listed: Vec<String> = tables.iter().map(|t| self.names.t(t)).collect();
        listed.extend(vector_index::readable(&mut db, &self.names)?);
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

    /// One step of a chosen order index, in a transaction of its own
    /// (`projection::choose_order`), outside the fold's.
    fn choose_order(&self, ty: &TypeName, step: &Choose) -> Result<Option<RecordKey>> {
        let writing = self.writing()?;
        let transaction = lock(&self.transaction);
        if transaction.is_some() {
            return Err(Error::invalid(Case::CompactInTransaction));
        }
        let mut db = self.projecting(writing)?;
        let _ = db.batch("ROLLBACK");
        let next = projection::choose_order(&mut db, &self.names, ty, step)?;
        // A kept field's rows were written beside the fold's count of what
        // the planner has not seen: it is told of them now, of the order
        // index alone (module doc).
        if matches!(step, Choose::Keep(_)) {
            let _ = db.batch(&format!("ANALYZE {}", self.names.t("sort_index")));
        }
        Ok(next)
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

/// The vector index's write side and its size: the rows the records'
/// states hold in force, each field's ranked together, in the fold's
/// transaction. The state's row holds a vector the fold puts already; one
/// put or removed after it holds over the state's until the record's next
/// state. Of a key whose state the transaction's buffer holds, the buffer
/// takes the put or the remove, made when it is written where the rows it
/// leaves differ.
impl VectorIndex for PgLake {
    fn put(&self, ty: &TypeName, key: &RecordKey, field: &str, vector: &[f32]) -> Result<()> {
        self.index(ty, key, field, Some(vector))
    }

    fn remove(&self, ty: &TypeName, key: &RecordKey, field: &str) -> Result<()> {
        self.index(ty, key, field, None)
    }

    fn count(&self, ty: &TypeName, field: &str) -> Result<u64> {
        let statement = format!(
            "SELECT count(*) FROM {} WHERE ty = $1 AND field = $2 AND until IS NULL",
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
    let names = Names::of(&target)?;
    let mut db = connect::connect(&target)?;
    if !matches!(shape(&mut db, &target.schema, false)?, Shape::Lake) {
        let (lake, schema) = (location.lake.to_string(), Some(target.schema.clone()));
        return Err(Error::unavailable(Situation::NoLake { lake, schema }));
    }
    check_name(&mut db, &names, &target.schema, &location.lake)?;
    log::export(&mut db, &names, dir)
}
