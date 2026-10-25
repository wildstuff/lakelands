//! The vector index a lake asks for: a graph per class of reader, each a
//! table of its own indexed by pgvector in the server
//! (`docs/decisions/078-under-an-index-a-search-pages-one-ranking-of-what-its-reader-may-see.md`).
//!
//! **What it is.** The `sys.setting` `vector_index.<type>` names vector
//! fields of the type. Each such field of a record type keeps graph B, the
//! unit vectors of its live heads that are not held, and, from the type's
//! first hold, graph A, those of every live head; a link type's field
//! keeps A alone. A graph is a table (the key, the unit vector) with one
//! HNSW index, ordered by L2 distance, which on unit vectors is cosine's
//! order. A row of `graphs` names each graph's generation (its table),
//! the one being built beside it, the position it was copied through and
//! the position it is stale from. `vectors` keeps every position's
//! elements and no unit vector.
//!
//! **The fold follows the graphs at its commit.** The setting's write and
//! a redeclaration build the type's graphs in the fold's transaction. Every
//! other commit writes the entries of the records it touched into each
//! built graph whose class sees them, from their states at its end. A hold
//! of a record that may have had an entry in B since B's copy deletes it
//! and marks B stale: its graph keeps the shape the entry gave it, so no
//! search reads it again. The first hold of a type adds A's row.
//!
//! **The writer builds beside.** A thread of the writer builds each graph
//! whose row asks for one (not yet built, or stale) on a connection of its
//! own: it names the next generation in the row, copies the graph's
//! entries in one snapshot, indexes them, and swaps the row to it in one
//! short transaction under the fold's lock, after applying what was folded
//! since the snapshot. A mark set after the snapshot starts a new copy.
//! Only the sweep drops a table a row read, once no reader holds
//! `graphs`, so a search reads the generation its snapshot names whole; a
//! swap or a release drops only a next generation no row read.
//!
//! **A search reads one stream.** A reader is streamed from the graph of
//! its class, while that graph is built and not stale, at fixed settings
//! ([`STREAM`]), each candidate judged on its head record where its
//! selection needs that, until the `k`th admitted candidate and every one
//! within the index's rounding of it; where the stream ends first, the
//! graph's other entries follow, ranked exactly. Every other read ranks
//! the field exactly.
//!
//! **Without pgvector.** The extension is the host's (`docs/hosting.md`).
//! A writer's open creates it if it can; when it cannot, or finds a
//! version before 0.8, the lake keeps no graph, every search ranks
//! exactly, and the open says so once on standard error when the lake
//! asks for an index.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use lakelands_api::engine::Scan;
use lakelands_api::error::Result;
use lakelands_api::id::{FieldName, Position, RecordKey, TypeName};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::query::cosine;
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::{SETTING_TYPE, VECTOR_INDEX_SETTING};
use lakelands_core::engines;
use lakelands_core::links::is_link_type;
use postgres::types::ToSql;

use crate::connect::{self, Db, Target};
use crate::lock;
use crate::projection::{self, Names};

/// Neighbours per node of the HNSW graph (layer 0 keeps twice as many).
const M: u32 = 16;
/// The candidate list while the graph is built.
const EF_CONSTRUCTION: u32 = 64;
/// The oldest pgvector with iterative index scans.
const MIN_VERSION: (u32, u32) = (0, 8);
/// The rows a fill reads at a time.
const FILL_PAGE: usize = 1024;
/// The settings a stream is read under, the same for every search so
/// that its answer follows no server value: pgvector's widest candidate
/// list, no row nearer than the one before, the scan's bounds lifted
/// to some 4 000 MiB of memory, and no plan that sorts the field or
/// compiles the statement (`docs/hosting.md`).
const STREAM: &str = "SET LOCAL hnsw.ef_search = 1000; \
     SET LOCAL hnsw.iterative_scan = strict_order; \
     SET LOCAL hnsw.max_scan_tuples = 2147483647; SET LOCAL hnsw.scan_mem_multiplier = 1000; \
     SET LOCAL work_mem = '4MB'; SET LOCAL jit = off; \
     SET LOCAL enable_seqscan = off; SET LOCAL enable_sort = off";
/// The most rows a stream fetches at once.
const FETCH_MOST: usize = 4096;
/// The wait before a build that failed is tried again.
const BACK_OFF: Duration = Duration::from_secs(1);
/// Whether a record's state `r` is held, as `records.state` writes it.
const HELD: &str = "coalesce((r.state::jsonb ->> 'restricted') = 'true', false)";
/// Whether a record's state `r` is retired, likewise.
const RETIRED: &str = "coalesce((r.state::jsonb ->> 'retired') = 'true', false)";

/// pgvector, found in the server: the schema its type and operators live
/// in, quoted.
#[derive(Clone, Debug)]
pub(crate) struct Pgvector {
    schema: String,
}

impl Pgvector {
    /// The `vector` type of `n` dimensions.
    fn ty(&self, n: u32) -> String {
        format!("{}.vector({n})", self.schema)
    }
}

/// pgvector in the server `db` is connected to, creating the extension
/// first when `create`; why not, when it is not there or too old.
pub(crate) fn find(db: &mut Db, create: bool) -> std::result::Result<Pgvector, String> {
    let created = if create {
        db.batch("CREATE EXTENSION IF NOT EXISTS vector").err()
    } else {
        None
    };
    let row = db
        .query_opt(
            "SELECT e.extversion, n.nspname FROM pg_extension e \
             JOIN pg_namespace n ON n.oid = e.extnamespace WHERE e.extname = 'vector'",
            &[],
        )
        .map_err(|e| format!("the server's extensions cannot be read: {e}"))?;
    let Some(row) = row else {
        return Err(match created {
            Some(e) => format!("the extension `vector` (pgvector) cannot be created: {e}"),
            None => "the extension `vector` (pgvector) is not installed".to_owned(),
        });
    };
    let (version, schema): (String, String) = (row.get(0), row.get(1));
    let mut parts = version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let (major, minor) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    if (major, minor) < MIN_VERSION {
        return Err(format!(
            "the extension `vector` (pgvector) is version {version}; the index needs {}.{} or \
             later",
            MIN_VERSION.0, MIN_VERSION.1
        ));
    }
    Ok(Pgvector {
        schema: format!("\"{}\"", schema.replace('"', "\"\"")),
    })
}

/// A vector's length, taken in `f64`; none where it is zero or not
/// finite. A vector of zeros points nowhere: it is in no graph, and never
/// answered for a field the lake keeps graphs of, whichever way it is
/// read.
pub(crate) fn length(v: &[f32]) -> Option<f64> {
    let norm = v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>().sqrt();
    (norm != 0.0 && norm.is_finite()).then_some(norm)
}

/// A vector's direction as pgvector reads it: `v / |v|`, each component
/// rounded to `f32`; none for a vector without a [`length`].
pub(crate) fn unit_text(v: &[f32]) -> Option<String> {
    let norm = length(v)?;
    let parts: Vec<String> = v
        .iter()
        .map(|x| ((f64::from(*x) / norm) as f32).to_string())
        .collect();
    Some(format!("[{}]", parts.join(",")))
}

/// A string as an SQL literal.
fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// One graph's row: its field and class (`a` every live head, `b` those
/// not held), its dimensions, the generation read, the position it is
/// stale from and the one it was copied through.
#[derive(Clone, Debug)]
struct Graph {
    field: String,
    class: String,
    dims: u32,
    generation: Option<i64>,
    stale: Option<i64>,
    copied: Option<i64>,
}

impl Graph {
    /// The table a search may read, built and not stale.
    fn readable(&self, ty: &str) -> Option<String> {
        let g = self.generation.filter(|_| self.stale.is_none())?;
        Some(bare(ty, &self.field, &self.class, g))
    }
}

/// The graphs of `ty`, of one field where given.
fn graphs(db: &mut Db, names: &Names, ty: &str, field: Option<&str>) -> Result<Vec<Graph>> {
    let rows = db.query(
        &format!(
            "SELECT field, class, dims, gen, stale, copied FROM {} \
             WHERE ty = $1 AND ($2::text IS NULL OR field = $2) ORDER BY field, class",
            names.t("graphs")
        ),
        &[&ty, &field],
    )?;
    Ok(rows
        .iter()
        .map(|r| Graph {
            field: r.get(0),
            class: r.get(1),
            dims: r.get::<_, i32>(2) as u32,
            generation: r.get(3),
            stale: r.get(4),
            copied: r.get(5),
        })
        .collect())
}

/// The table of one generation of a graph: stable, and short enough for
/// an identifier whatever the names. Unqualified; [`Names::t`] qualifies
/// it.
fn bare(ty: &str, field: &str, class: &str, generation: i64) -> String {
    use sha2::{Digest as _, Sha256};
    let digest = Sha256::digest(format!("{ty}\0{field}\0{class}\0{generation}").as_bytes());
    format!("vgraph_{}", lakelands_core::canonical::hex(&digest[..8]))
}

/// A new generation, from the lake's one counter, whose table is free.
fn generation(db: &mut Db, names: &Names, (ty, field, class): (&str, &str, &str)) -> Result<i64> {
    loop {
        let g: i64 = db
            .query_one(
                &format!(
                    "INSERT INTO {} AS m (k, n) VALUES ('graph', 1) \
                     ON CONFLICT (k) DO UPDATE SET n = m.n + 1 RETURNING n",
                    names.t("meta")
                ),
                &[],
            )?
            .get(0);
        let table = names.t(&bare(ty, field, class, g));
        let taken: bool = db
            .query_one("SELECT to_regclass($1) IS NOT NULL", &[&table])?
            .get(0);
        if !taken {
            return Ok(g);
        }
    }
}

/// Take the lock the fold holds through its transaction: a build's
/// claim and swap take it too, so either waits for the other.
pub(crate) fn lock_fold(db: &mut Db, names: &Names) -> Result<()> {
    db.query_one(
        "SELECT pg_advisory_xact_lock(hashtextextended('lakelands graphs ' || $1, 0))",
        &[&names.schema()],
    )
    .map(drop)
}

/// Make a generation's table, empty and without its index, where the
/// lake's vectors go.
fn create(db: &mut Db, names: &Names, v: &Pgvector, table: &str, n: u32) -> Result<()> {
    let space = &names.at.vectors;
    let key_space = (space.as_deref().map(Target::quote))
        .map_or_else(String::new, |t| format!(" USING INDEX TABLESPACE {t}"));
    db.batch(&format!(
        "CREATE TABLE {} (key TEXT COLLATE \"C\" PRIMARY KEY{key_space}, unit {} NOT NULL){}",
        names.t(table),
        v.ty(n),
        Names::space(space),
    ))
}

/// Build a generation's HNSW index.
fn index(db: &mut Db, names: &Names, v: &Pgvector, table: &str) -> Result<()> {
    db.batch(&format!(
        "CREATE INDEX {table}_unit ON {} USING hnsw (unit {}.vector_l2_ops) \
         WITH (m = {M}, ef_construction = {EF_CONSTRUCTION}){}",
        names.t(table),
        v.schema,
        Names::space(&names.at.vectors),
    ))
}

/// Write into `table` the entries of `ty.field` its class sees: of
/// `keys` where given, each key's old entry deleted first, else of every
/// live head, a page at a time. A held head is no entry of B, and a
/// vector of zeros or of other dimensions no entry of any graph.
fn fill(
    db: &mut Db,
    names: &Names,
    v: &Pgvector,
    (ty, field, class): (&str, &str, &str),
    (table, n): (&str, u32),
    keys: Option<&[String]>,
) -> Result<()> {
    let (vectors, heads, records) = (
        names.t("vectors"),
        names.t("records_head"),
        names.t("records"),
    );
    let select = |which: &str| {
        format!(
            "SELECT v.key, v.single, v.data, {HELD}, {RETIRED} FROM {vectors} v \
             LEFT JOIN {heads} h ON h.ty = v.ty AND h.key = v.key \
             LEFT JOIN {records} r ON r.ty = h.ty AND r.key = h.key AND r.since = h.since \
             WHERE v.ty = $1 AND v.field = $2 AND v.until IS NULL AND {which}"
        )
    };
    let put = |db: &mut Db, rows: &[postgres::Row]| -> Result<()> {
        let (mut keys, mut units) = (Vec::new(), Vec::new());
        for row in rows {
            let (held, retired): (bool, bool) = (row.get(3), row.get(4));
            if retired || (held && class == "b") {
                continue;
            }
            let e = engines::Elements::new(row.get(1), row.get(2)).ranked();
            if let Some(unit) = unit_text(&e).filter(|_| e.len() == n as usize) {
                keys.push(row.get::<_, String>(0));
                units.push(unit);
            }
        }
        if keys.is_empty() {
            return Ok(());
        }
        db.execute(
            &format!(
                "INSERT INTO {} (key, unit) SELECT k, CAST(u AS {}) \
                 FROM unnest($1::text[], $2::text[]) AS b(k, u)",
                names.t(table),
                v.ty(n)
            ),
            &[&keys, &units],
        )
        .map(drop)
    };
    match keys {
        Some(keys) => {
            for part in keys.chunks(FILL_PAGE) {
                db.execute(
                    &format!("DELETE FROM {} WHERE key = ANY($1)", names.t(table)),
                    &[&part],
                )?;
                let rows = db.query(&select("v.key = ANY($3)"), &[&ty, &field, &part])?;
                put(db, &rows)?;
            }
        }
        None => {
            let page = select("v.key > $3 ORDER BY v.key LIMIT $4");
            let mut after = String::new();
            loop {
                let rows = db.query(&page, &[&ty, &field, &after, &(FILL_PAGE as i64)])?;
                put(db, &rows)?;
                match rows.last() {
                    Some(last) if rows.len() == FILL_PAGE => after = last.get(0),
                    _ => break,
                }
            }
        }
    }
    Ok(())
}

/// Whether a live head of `ty` is held.
fn held_any(db: &mut Db, names: &Names, ty: &str) -> Result<bool> {
    let row = db.query_one(
        &format!(
            "SELECT EXISTS (SELECT 1 FROM {} h JOIN {} r ON r.ty = h.ty AND r.key = h.key \
             AND r.since = h.since WHERE h.ty = $1 AND {HELD})",
            names.t("records_head"),
            names.t("records")
        ),
        &[&ty],
    )?;
    Ok(row.get(0))
}

/// The vector fields of `decl` the lake asks an index for, with their
/// dimensions, by its `vector_index.<type>` setting at the head.
fn wanted(db: &mut Db, names: &Names, decl: &TypeDecl) -> Result<Vec<(FieldName, u32)>> {
    let setting = projection::head_record(
        db,
        names,
        &TypeName::from(SETTING_TYPE),
        &RecordKey::from(engines::vector_index_key(&decl.name)),
    )?;
    Ok(
        engines::indexed_fields(decl, setting.as_ref().and_then(engines::setting_value))
            .into_iter()
            .filter_map(|f| {
                let n = decl.field(&f)?.ty.dimensions?;
                Some((f, n))
            })
            .collect(),
    )
}

/// The classes `ty` keeps a graph of each indexed field in: A alone for a
/// link type, else B, and A beside it once a head is held.
fn classes(db: &mut Db, names: &Names, ty: &TypeName) -> Result<&'static [&'static str]> {
    Ok(if is_link_type(ty) {
        &["a"]
    } else if held_any(db, names, ty.as_str())? {
        &["a", "b"]
    } else {
        &["b"]
    })
}

/// Make `ty`'s graphs again under its setting and declaration at the
/// head, copied through `through`: its rows go, and a row is written for
/// each class of each field it asks for, its generation built now where
/// `build`, else left to the builder. Old tables are the sweep's.
/// Without pgvector only the rows go. The fold does this when it commits
/// the type's setting or a declaration of it.
pub(crate) fn rebuild(
    db: &mut Db,
    names: &Names,
    v: Option<&Pgvector>,
    ty: &TypeName,
    through: Position,
    build: bool,
) -> Result<()> {
    let rows = names.t("graphs");
    db.execute(
        &format!("DELETE FROM {rows} WHERE ty = $1"),
        &[&ty.as_str()],
    )?;
    let Some(v) = v else { return Ok(()) };
    let Some(decl) = projection::head_type(db, names, ty)?.decl else {
        return Ok(());
    };
    let fields = wanted(db, names, &decl)?;
    if fields.is_empty() {
        return Ok(());
    }
    let classes = classes(db, names, ty)?;
    for (field, n) in fields {
        for class in classes {
            let at = (ty.as_str(), field.as_str(), *class);
            let built = match build {
                true => {
                    let g = generation(db, names, at)?;
                    let table = bare(at.0, at.1, at.2, g);
                    create(db, names, v, &table, n)?;
                    fill(db, names, v, at, (&table, n), None)?;
                    index(db, names, v, &table)?;
                    Some(g)
                }
                false => None,
            };
            let copied = built.map(|_| through.0 as i64);
            db.execute(
                &format!(
                    "INSERT INTO {rows} (ty, field, class, dims, gen, copied) \
                     VALUES ($1, $2, $3, $4, $5, $6)"
                ),
                &[&at.0, &at.1, &at.2, &(n as i32), &built, &copied],
            )?;
        }
    }
    Ok(())
}

/// What one fold's transaction touched, for [`follow`]: the keys of each
/// type whose state or vector it wrote, and the types it erased a record
/// of.
#[derive(Default)]
pub(crate) struct Touched {
    pub(crate) keys: BTreeMap<TypeName, BTreeSet<String>>,
    pub(crate) erased: BTreeSet<TypeName>,
}

/// At the fold's commit through `through`, from `before`: write the
/// entries of the keys it touched into every built graph, from their
/// states at its end; for a held key, add A's row where the type has
/// none, and mark B stale where the key may have had an entry in it
/// since its copy (`suspect`: whatever its states, after a fold that
/// failed may have put one; so does a state of it since the copy that
/// names a vector, as a remove leaves no row saying until when one was
/// in force). An erased record marks its type's graphs stale. The types
/// in `skip` were built anew in the transaction. The answer says whether
/// it left a graph for the builder.
pub(crate) fn follow(
    db: &mut Db,
    names: &Names,
    v: Option<&Pgvector>,
    (through, before): (Position, Option<Position>),
    touched: &Touched,
    skip: &BTreeSet<TypeName>,
    suspect: bool,
) -> Result<bool> {
    let Some(v) = v else { return Ok(false) };
    let (rows, at) = (names.t("graphs"), through.0 as i64);
    let mut left = false;
    let stale = format!(
        "UPDATE {rows} SET stale = greatest(coalesce(stale, $2), $2) \
         WHERE ty = $1 AND ($3::text IS NULL OR (field = $3 AND class = 'b')) RETURNING 1"
    );
    for ty in touched.erased.iter().filter(|t| !skip.contains(*t)) {
        left |= !db
            .query(&stale, &[&ty.as_str(), &at, &None::<String>])?
            .is_empty();
    }
    let had = format!(
        "SELECT EXISTS (SELECT 1 FROM unnest($3::text[]) AS b(key) \
         WHERE (EXISTS (SELECT 1 FROM {vectors} v WHERE v.ty = $1 AND v.field = $2 \
           AND v.key = b.key AND (v.until IS NULL OR v.until > $4)) \
         OR EXISTS (SELECT 1 FROM {records} r WHERE r.ty = $1 AND r.key = b.key \
           AND jsonb_typeof(r.state::jsonb -> 'fields' -> $2) = 'array' AND r.since <= $6 \
           AND r.since >= coalesce((SELECT max(x.since) FROM {records} x \
             WHERE x.ty = $1 AND x.key = b.key AND x.since <= $4), -1))) \
         AND ($5 OR EXISTS (SELECT 1 FROM {records} r WHERE r.ty = $1 AND r.key = b.key \
           AND r.since <= $6 AND NOT {HELD} AND NOT {RETIRED} \
           AND r.since >= coalesce((SELECT max(x.since) FROM {records} x \
             WHERE x.ty = $1 AND x.key = b.key AND x.since <= $4), -1))))",
        vectors = names.t("vectors"),
        records = names.t("records"),
    );
    let before = before.map_or(-1, |p| p.0 as i64);
    for (ty, keys) in &touched.keys {
        if skip.contains(ty) {
            continue;
        }
        let graphs = graphs(db, names, ty.as_str(), None)?;
        if graphs.is_empty() {
            continue;
        }
        let keys: Vec<String> = keys.iter().cloned().collect();
        if !touched.erased.contains(ty) {
            for g in &graphs {
                if let Some(table) = g.readable(ty.as_str()) {
                    let at = (ty.as_str(), g.field.as_str(), g.class.as_str());
                    fill(db, names, v, at, (&table, g.dims), Some(&keys))?;
                }
            }
        }
        if is_link_type(ty) {
            continue;
        }
        let held: Vec<String> = db.column(
            &format!(
                "SELECT h.key FROM unnest($2::text[]) AS b(key) JOIN {} h \
                 ON h.ty = $1 AND h.key = b.key JOIN {} r ON r.ty = h.ty AND r.key = h.key \
                 AND r.since = h.since WHERE {HELD}",
                names.t("records_head"),
                names.t("records")
            ),
            &[&ty.as_str(), &keys],
        )?;
        if held.is_empty() {
            continue;
        }
        if !graphs.iter().any(|g| g.class == "a") {
            for g in graphs.iter().filter(|g| g.class == "b") {
                let added = db.query(
                    &format!(
                        "INSERT INTO {rows} (ty, field, class, dims) VALUES ($1, $2, 'a', $3) \
                         ON CONFLICT DO NOTHING RETURNING 1"
                    ),
                    &[&ty.as_str(), &g.field, &(g.dims as i32)],
                )?;
                left |= !added.is_empty();
            }
        }
        for g in graphs.iter().filter(|g| g.class == "b") {
            let copied = g.copied.unwrap_or(-1);
            let args: [&(dyn ToSql + Sync); 6] =
                [&ty.as_str(), &g.field, &held, &copied, &suspect, &before];
            if db.query_one(&had, &args)?.get::<_, bool>(0) {
                left |= !db
                    .query(&stale, &[&ty.as_str(), &at, &Some(&g.field)])?
                    .is_empty();
            }
        }
    }
    Ok(left)
}

/// At a writer's open, in one transaction under the fold's lock: give up
/// every build a writer before left half done, and make the graphs of
/// every type whose rows do not describe what the head asks for again,
/// for the builder. Without pgvector the rows go, since this writer
/// will not follow them, and the open says once why there is no index
/// when the lake asks for one.
pub(crate) fn opened(
    db: &mut Db,
    names: &Names,
    v: std::result::Result<&Pgvector, &str>,
    lake: &str,
) -> Result<()> {
    let rows = names.t("graphs");
    db.transaction("BEGIN ISOLATION LEVEL READ COMMITTED", |db| {
        lock_fold(db, names)?;
        db.execute(
            &format!("UPDATE {rows} SET next = NULL WHERE next IS NOT NULL"),
            &[],
        )?;
        let Some(folded) = projection::folded(db, names)? else {
            return Ok(());
        };
        let asked: Vec<String> = db.column(
            &format!(
                "SELECT substr(key, $1) FROM {} WHERE ty = $2 AND left(key, $1 - 1) = $3 \
                 UNION SELECT ty FROM {rows}",
                names.t("records_head"),
            ),
            &[
                &(VECTOR_INDEX_SETTING.len() as i32 + 1),
                &SETTING_TYPE,
                &VECTOR_INDEX_SETTING,
            ],
        )?;
        let mut unserved = false;
        for ty in asked {
            let ty = TypeName::from(ty);
            let want = match projection::head_type(db, names, &ty)?.decl {
                Some(decl) => wanted(db, names, &decl)?,
                None => Vec::new(),
            };
            let Ok(v) = v else {
                unserved |= !want.is_empty();
                continue;
            };
            let have: BTreeSet<(String, u32)> = graphs(db, names, ty.as_str(), None)?
                .into_iter()
                .map(|g| (g.field, g.dims))
                .collect();
            let want: BTreeSet<(String, u32)> =
                want.into_iter().map(|(f, n)| (f.to_string(), n)).collect();
            if have != want {
                rebuild(db, names, Some(v), &ty, folded, false)?;
            }
        }
        if let Err(why) = v {
            db.execute(&format!("DELETE FROM {rows}"), &[])?;
            if unserved {
                eprintln!(
                    "lakelands: lake `{lake}` asks for a vector index, and {why}; its searches \
                     read every vector, exactly, until pgvector 0.8 or later is installed \
                     (docs/hosting.md)"
                );
            }
        }
        Ok(())
    })
}

/// Whether a graph's row asks the builder for a generation.
pub(crate) fn waiting(db: &mut Db, names: &Names) -> Result<bool> {
    let row = db.query_one(
        &format!(
            "SELECT EXISTS (SELECT 1 FROM {} WHERE gen IS NULL OR stale IS NOT NULL)",
            names.t("graphs")
        ),
        &[],
    )?;
    Ok(row.get(0))
}

/// The graph tables of the schema that no row reads or builds.
fn unnamed(db: &mut Db, names: &Names) -> Result<Vec<String>> {
    let named: HashSet<String> = db
        .query(
            &format!(
                "SELECT ty, field, class, gen, next FROM {}",
                names.t("graphs")
            ),
            &[],
        )?
        .iter()
        .flat_map(|r| {
            let (ty, field, class): (String, String, String) = (r.get(0), r.get(1), r.get(2));
            [r.get::<_, Option<i64>>(3), r.get(4)]
                .into_iter()
                .flatten()
                .map(move |g| bare(&ty, &field, &class, g))
        })
        .collect();
    let tables: Vec<String> = db.column(
        "SELECT relname::text FROM pg_class WHERE relnamespace = to_regnamespace($1) \
         AND relkind = 'r' AND relname LIKE 'vgraph\\_%'",
        &[&names.schema()],
    )?;
    Ok(tables.into_iter().filter(|t| !named.contains(t)).collect())
}

/// Drop every graph table of the schema that no row reads or builds,
/// once no transaction holds `graphs`: waiting for that where `wait`,
/// else failing at once, to try again later.
pub(crate) fn sweep(db: &mut Db, names: &Names, wait: bool) -> Result<()> {
    if unnamed(db, names)?.is_empty() {
        return Ok(());
    }
    db.transaction("BEGIN ISOLATION LEVEL READ COMMITTED", |db| {
        let nowait = if wait { "" } else { " NOWAIT" };
        db.batch(&format!(
            "LOCK TABLE {} IN ACCESS EXCLUSIVE MODE{nowait}",
            names.t("graphs")
        ))?;
        for t in unnamed(db, names)? {
            db.batch(&format!("DROP TABLE IF EXISTS {}", names.t(&t)))?;
        }
        Ok(())
    })
}

/// The statement that drops every graph table of `from`'s schema, or,
/// with `to`, moves each into `to`'s.
pub(crate) fn tables_sql(from: &Names, to: Option<&Names>) -> String {
    let act = match to {
        None => "'DROP TABLE %s.%I'".to_owned(),
        Some(to) => format!(
            "'ALTER TABLE %s.%I SET SCHEMA ' || {}",
            literal(to.schema())
        ),
    };
    format!(
        "DO $graphs$ DECLARE t text; BEGIN FOR t IN SELECT relname FROM pg_class \
         WHERE relnamespace = to_regnamespace({s}) AND relkind = 'r' \
         AND relname LIKE 'vgraph\\_%' LOOP EXECUTE format({act}, {s}, t); END LOOP; END \
         $graphs$;",
        s = literal(from.schema()),
    )
}

/// The tables of the graphs a search may read, for `VACUUM`.
pub(crate) fn readable(db: &mut Db, names: &Names) -> Result<Vec<String>> {
    Ok(db
        .query(
            &format!(
                "SELECT ty, field, class, gen FROM {} WHERE gen IS NOT NULL",
                names.t("graphs")
            ),
            &[],
        )?
        .iter()
        .map(|r| names.t(&bare(r.get(0), r.get(1), r.get(2), r.get(3))))
        .collect())
}

/// What a build of one graph came to.
enum Built {
    /// Swapped in, given up, or no longer asked for.
    Done,
    /// Marked stale after its copy: to be copied again.
    Again,
}

/// Build every graph whose row asks for one, one at a time, on `db`, a
/// connection of the builder's own (or the scratch's, where nothing runs
/// beside it), until `hold` says to stop.
fn build_all(db: &mut Db, names: &Names, v: &Pgvector, hold: &dyn Fn() -> bool) -> Result<()> {
    loop {
        if hold() {
            return Ok(());
        }
        let Some(row) = db.query_opt(
            &format!(
                "SELECT ty, field, class, dims FROM {} \
                 WHERE (gen IS NULL OR stale IS NOT NULL) AND next IS NULL \
                 ORDER BY ty, field, class LIMIT 1",
                names.t("graphs")
            ),
            &[],
        )?
        else {
            return Ok(());
        };
        let (ty, field, class): (String, String, String) = (row.get(0), row.get(1), row.get(2));
        let n = row.get::<_, i32>(3) as u32;
        while let Built::Again = build_one(db, names, v, (&ty, &field, &class), n, hold)? {}
    }
}

/// Build one graph's next generation beside the one read: claim it,
/// copy its entries in one snapshot, index them, and swap it in under
/// the fold's lock after applying what was folded since the snapshot.
fn build_one(
    db: &mut Db,
    names: &Names,
    v: &Pgvector,
    at: (&str, &str, &str),
    n: u32,
    hold: &dyn Fn() -> bool,
) -> Result<Built> {
    let (ty, field, class) = at;
    let rows = names.t("graphs");
    let row = format!(
        "SELECT gen, stale, next FROM {rows} WHERE ty = $1 AND field = $2 AND class = $3 \
         FOR UPDATE"
    );
    let claimed = db.transaction("BEGIN ISOLATION LEVEL READ COMMITTED", |db| {
        lock_fold(db, names)?;
        let Some(r) = db.query_opt(&row, &[&ty, &field, &class])? else {
            return Ok(None);
        };
        let (g, stale, next): (Option<i64>, Option<i64>, Option<i64>) =
            (r.get(0), r.get(1), r.get(2));
        if next.is_some() || (g.is_some() && stale.is_none()) {
            return Ok(None);
        }
        let g = generation(db, names, at)?;
        db.execute(
            &format!("UPDATE {rows} SET next = $4 WHERE ty = $1 AND field = $2 AND class = $3"),
            &[&ty, &field, &class, &g],
        )?;
        Ok(Some(g))
    })?;
    let Some(g) = claimed else {
        return Ok(Built::Done);
    };
    let table = bare(ty, field, class, g);
    let copy = |db: &mut Db| -> Result<Option<i64>> {
        let p = db.transaction("BEGIN ISOLATION LEVEL REPEATABLE READ", |db| {
            let p = projection::folded(db, names)?.map_or(-1, |p| p.0 as i64);
            create(db, names, v, &table, n)?;
            fill(db, names, v, at, (&table, n), None)?;
            Ok(p)
        })?;
        if hold() {
            return Ok(None);
        }
        index(db, names, v, &table)?;
        Ok(Some(p))
    };
    let swap = |db: &mut Db, p: i64| {
        db.transaction("BEGIN ISOLATION LEVEL READ COMMITTED", |db| {
            lock_fold(db, names)?;
            let r = db.query_opt(&row, &[&ty, &field, &class])?;
            let (stale, next): (Option<i64>, Option<i64>) = match &r {
                Some(r) => (r.get(1), r.get(2)),
                None => (None, None),
            };
            let drop_table = format!("DROP TABLE IF EXISTS {}", names.t(&table));
            if next != Some(g) {
                db.batch(&drop_table)?;
                return Ok(Built::Done);
            }
            let ours = "WHERE ty = $1 AND field = $2 AND class = $3";
            if stale.is_some_and(|s| s > p) {
                db.execute(
                    &format!("UPDATE {rows} SET next = NULL {ours}"),
                    &[&ty, &field, &class],
                )?;
                db.batch(&drop_table)?;
                return Ok(Built::Again);
            }
            let keys: Vec<String> = db.column(
                &format!(
                    "SELECT key FROM {} WHERE ty = $1 AND since > $3 UNION \
                 SELECT key FROM {} WHERE ty = $1 AND field = $2 \
                 AND (since > $3 OR until > $3 OR since = -1)",
                    names.t("records"),
                    names.t("vectors")
                ),
                &[&ty, &field, &p],
            )?;
            fill(db, names, v, at, (&table, n), Some(&keys))?;
            if class == "a" && !is_link_type(&TypeName::from(ty)) && !held_any(db, names, ty)? {
                db.execute(
                    &format!("DELETE FROM {rows} {ours}"),
                    &[&ty, &field, &class],
                )?;
                db.batch(&drop_table)?;
                return Ok(Built::Done);
            }
            db.execute(
                &format!(
                    "UPDATE {rows} SET gen = $4, next = NULL, copied = $5, stale = NULL {ours}"
                ),
                &[&ty, &field, &class, &g, &p],
            )?;
            Ok(Built::Done)
        })
    };
    match copy(db).and_then(|p| p.map(|p| swap(db, p)).transpose()) {
        Ok(Some(built)) => Ok(built),
        given_up => {
            release(db, names, at, g, &table)?;
            given_up.map(|_| Built::Done)
        }
    }
}

/// Give up a claimed build: its row builds nothing, and its table goes.
fn release(
    db: &mut Db,
    names: &Names,
    (ty, field, class): (&str, &str, &str),
    g: i64,
    table: &str,
) -> Result<()> {
    db.transaction("BEGIN ISOLATION LEVEL READ COMMITTED", |db| {
        db.execute(
            &format!(
                "UPDATE {} SET next = NULL WHERE ty = $1 AND field = $2 AND class = $3 \
                 AND next = $4",
                names.t("graphs")
            ),
            &[&ty, &field, &class, &g],
        )?;
        db.batch(&format!("DROP TABLE IF EXISTS {}", names.t(table)))
    })
}

/// Build, on the scratch's own connection, every graph its folds left to
/// build, then drop what no row reads: a scratch is marked built with
/// its graphs.
pub(crate) fn build_scratch(db: &mut Db, names: &Names, v: &Pgvector) -> Result<()> {
    build_all(db, names, v, &|| false)?;
    sweep(db, names, true)
}

/// What the writer and its builder share.
#[derive(Default)]
struct State {
    /// A commit asked for a look at the rows since the builder last
    /// looked.
    wake: bool,
    /// The handle is going.
    stop: bool,
    /// How many of the writer's acts hold the builder off.
    held: u32,
    /// The builder's backend, while it has a connection: from the round
    /// a wake starts to the one that leaves nothing to build or drop.
    pid: Option<i32>,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    turn: Condvar,
    /// Held by the builder through each round of builds.
    work: Mutex<()>,
}

/// The writer's builder: a thread building the graphs beside, on a
/// connection of its own while there is something to build, woken by the
/// commits that leave it some.
pub(crate) struct Builder {
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

/// The builder held off while it lives ([`Builder::hold`]).
pub(crate) struct Held<'b> {
    shared: &'b Shared,
    _work: std::sync::MutexGuard<'b, ()>,
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        let mut s = lock(&self.shared.state);
        s.held -= 1;
        s.wake = true;
        self.shared.turn.notify_all();
    }
}

impl Builder {
    /// Start the builder of the lake `names` holds, on `target`'s server,
    /// woken at once where a graph is `waiting`.
    pub(crate) fn start(target: Target, names: Names, v: Pgvector, waiting: bool) -> Builder {
        let shared = Arc::new(Shared::default());
        let ours = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("lakelands-graphs".into())
            .spawn(move || run(&ours, &target, &names, &v))
            .ok();
        let builder = Builder {
            shared,
            thread: Mutex::new(thread),
        };
        if waiting {
            builder.wake();
        }
        builder
    }

    /// Ask the builder to look at the rows again.
    pub(crate) fn wake(&self) {
        lock(&self.shared.state).wake = true;
        self.shared.turn.notify_all();
    }

    /// Hold the builder off until the answer is dropped: its statement
    /// is cancelled through `db`, the writer's own connection, and its
    /// round ends at its next step.
    pub(crate) fn hold(&self, db: &mut Db) -> Held<'_> {
        let pid = {
            let mut s = lock(&self.shared.state);
            s.held += 1;
            s.pid
        };
        if let Some(pid) = pid {
            let _ = db.query_one("SELECT pg_cancel_backend($1)", &[&pid]);
        }
        Held {
            shared: &self.shared,
            _work: lock(&self.shared.work),
        }
    }

    /// Stop the builder and wait for its thread.
    pub(crate) fn stop(&self, db: &mut Db) {
        let held = self.hold(db);
        lock(&self.shared.state).stop = true;
        drop(held);
        if let Some(thread) = lock(&self.thread).take() {
            let _ = thread.join();
        }
    }
}

/// The builder's loop: on each wake, build what the rows ask for, then
/// sweep, and close its connection once that is done; after a failure,
/// or a round the writer held off, again later.
fn run(shared: &Shared, target: &Target, names: &Names, v: &Pgvector) {
    let mut db: Option<Db> = None;
    loop {
        {
            let mut s = lock(&shared.state);
            while !s.stop && (!s.wake || s.held > 0) {
                s = shared.turn.wait(s).unwrap_or_else(|e| e.into_inner());
            }
            if s.stop {
                return;
            }
            s.wake = false;
        }
        let work = lock(&shared.work);
        let hold = || {
            let s = lock(&shared.state);
            s.held > 0 || s.stop
        };
        let mut round = || -> Result<()> {
            if db.as_ref().is_none_or(Db::is_closed) {
                let mut opened = connect::connect(target)?;
                opened.batch("SET max_parallel_maintenance_workers = 0")?;
                let pid: i32 = opened.query_one("SELECT pg_backend_pid()", &[])?.get(0);
                lock(&shared.state).pid = Some(pid);
                db = Some(opened);
            }
            let Some(db) = db.as_mut() else {
                return Ok(());
            };
            build_all(db, names, v, &hold)?;
            if hold() {
                return Ok(());
            }
            sweep(db, names, false)
        };
        let failed = round().is_err();
        drop(work);
        if failed {
            if let Some(db) = db.as_mut() {
                let _ = db.batch("ROLLBACK");
            }
            let s = lock(&shared.state);
            let (mut s, _) = shared
                .turn
                .wait_timeout(s, BACK_OFF)
                .unwrap_or_else(|e| e.into_inner());
            s.wake = true;
        } else if hold() {
            lock(&shared.state).wake = true;
        } else {
            lock(&shared.state).pid = None;
            db = None;
        }
    }
}

/// The graph a search of `ty`'s field for a reader of `restricted`'s
/// class reads, of `n` dimensions; none where it ranks exactly. A reader
/// holding `restricted` reads A where the type keeps A, else B, and one
/// without it B; a graph not built, stale, or of other dimensions is
/// read by no one.
fn pick(graphs: &[Graph], ty: &str, restricted: bool, n: usize) -> Option<String> {
    let class = |c: &str| graphs.iter().find(|g| g.class == c);
    let graph = match (restricted, class("a")) {
        (true, Some(a)) => a,
        _ => class("b")?,
    };
    graph.readable(ty).filter(|_| graph.dims as usize == n)
}

/// The `k` vectors of one field nearest to `query`, of those `seen`
/// admits by its head record (every live one where `None`), in the open
/// transaction of `db`, ranked by the one cosine. A field the lake keeps
/// a readable graph of for the reader's class is streamed from it
/// ([`stream`]); every other read, and every one asked `exact` or
/// holding `only`, ranks exactly ([`exact`]).
#[allow(clippy::too_many_arguments)]
pub(crate) fn nearest(
    db: &mut Db,
    names: &Names,
    v: Option<&Pgvector>,
    (ty, field): (&TypeName, &str),
    query: &[f32],
    k: u32,
    seen: Option<(&Scan, Option<&TypeDecl>)>,
    exact_asked: bool,
) -> Result<Vec<(RecordKey, f64)>> {
    let graphs = graphs(db, names, ty.as_str(), Some(field))?;
    if k == 0 {
        return Ok(Vec::new());
    }
    let scan = seen.map(|(s, _)| s);
    let streamed = !exact_asked && scan.is_none_or(|s| s.equal.is_empty());
    let restricted = scan.is_none_or(|s| s.restricted);
    let table = (v.zip(unit_text(query)).filter(|_| streamed)).and_then(|(v, unit)| {
        let table = pick(&graphs, ty.as_str(), restricted, query.len())?;
        Some((v, unit, table))
    });
    let (indexed, db) = (!graphs.is_empty(), RefCell::new(db));
    match table {
        Some((v, unit, table)) => {
            stream(&db, names, v, (ty, field), (query, &unit), &table, k, seen)
        }
        None => exact(&db, names, (ty, field), query, k, seen, indexed),
    }
}

/// The head records of `keys` of `ty`, in any order.
fn heads(
    db: &RefCell<&mut Db>,
    names: &Names,
    ty: &TypeName,
    keys: Vec<RecordKey>,
) -> Result<Vec<StoredRecord>> {
    let keys: Vec<&str> = keys.iter().map(RecordKey::as_str).collect();
    let mut db = db.borrow_mut();
    let rows = db.query(
        &format!(
            "SELECT r.ty, r.key, r.since, r.state FROM unnest($2::text[]) AS b(key) \
             JOIN {} h ON h.ty = $1 AND h.key = b.key \
             JOIN {} r ON r.ty = h.ty AND r.key = h.key AND r.since = h.since",
            names.t("records_head"),
            names.t("records")
        ),
        &[&ty.as_str(), &keys],
    )?;
    projection::records_at(&mut db, names, &rows, 0)
}

/// The exact ranking: every live vector of the field scored, the best
/// judged a batch at a time ([`engines::rank_seen`]). Where the reader
/// must hold a text of a field the exact-text index keeps, only the
/// vectors of the records holding it are scored, as `find` finds them.
/// It checks every vector's dimensions; on a field the lake keeps graphs
/// of it leaves out a vector of zeros, as a stream does.
fn exact(
    db: &RefCell<&mut Db>,
    names: &Names,
    (ty, field): (&TypeName, &str),
    query: &[f32],
    k: u32,
    seen: Option<(&Scan, Option<&TypeDecl>)>,
    indexed: bool,
) -> Result<Vec<(RecordKey, f64)>> {
    let lookup = match seen.filter(|(s, _)| !engines::admits_every_live(s)) {
        Some((scan, decl)) => {
            let looked: BTreeSet<String> = db.borrow_mut().column(
                &format!(
                    "SELECT field FROM {} WHERE ty = $1",
                    names.t("lookup_fields")
                ),
                &[&ty.as_str()],
            )?;
            let key = decl.and_then(engines::text_key);
            let kept = |f: &FieldName| looked.contains(f.as_str()) && Some(f) != key;
            engines::text_lookups(scan, &kept)
        }
        None => None,
    };
    let holding = match lookup {
        Some(_) => format!(
            " AND key IN (SELECT t.key FROM unnest($4::text[]) x(value) JOIN {} t \
             ON t.ty = $1 AND t.field = $3 AND md5(t.value) = md5(x.value))",
            names.t("text_index")
        ),
        None => String::new(),
    };
    let statement = format!(
        "SELECT key, single, data FROM {} WHERE ty = $1 AND field = $2 AND until IS NULL{holding}",
        names.t("vectors")
    );
    let (by, texts) = lookup.map_or((None, Vec::new()), |(f, t)| (Some(f), t));
    let (by, ty_text) = (by.as_ref().map(FieldName::as_str), ty.as_str());
    let top = |m: u32| {
        let mut top = engines::TopK::new(m);
        let mut args: Vec<&(dyn ToSql + Sync)> = vec![&ty_text, &field];
        if by.is_some() {
            args.extend([&by as &(dyn ToSql + Sync), &texts]);
        }
        db.borrow_mut().query_each(&statement, &args, |row| {
            let v = engines::Elements::new(row.get(1), row.get(2)).ranked();
            if v.len() != query.len() {
                return Err(engines::dimensions(query.len(), v.len(), ty, field));
            }
            if !indexed || length(&v).is_some() {
                top.push(&RecordKey::from(row.get::<_, String>(0)), cosine(query, &v));
            }
            Ok(())
        })?;
        Ok(top.into_ranked())
    };
    engines::rank_seen(k, seen, top, |keys| heads(db, names, ty, keys))
}

/// A stream of `table`, the graph of the reader's class: read in the
/// index's order in growing batches, each row judged on its head record
/// where `seen`'s selection needs that, until the `k`th admitted row and
/// every row within the index's rounding of its distance, then ranked by
/// the one cosine. Where it ends short, the graph's other entries
/// follow, ranked exactly.
#[allow(clippy::too_many_arguments)]
fn stream(
    db: &RefCell<&mut Db>,
    names: &Names,
    v: &Pgvector,
    (ty, field): (&TypeName, &str),
    (query, unit): (&[f32], &str),
    table: &str,
    k: u32,
    seen: Option<(&Scan, Option<&TypeDecl>)>,
) -> Result<Vec<(RecordKey, f64)>> {
    // The graph holds only what the reader's class sees: a head is read
    // only for the reader's own selection, by its key, row by row (the
    // `LIMIT` keeps the planner from joining every head at once).
    let judged =
        seen.filter(|(s, _)| s.filter.is_some() || s.valid_at.is_some() || !s.equal_any.is_empty());
    let (head, joined) = match judged {
        Some(_) => (
            ", r.ty, r.key, r.since, r.state",
            format!(
                " CROSS JOIN LATERAL (SELECT x.ty, x.key, x.since, x.state FROM {} h \
                 JOIN {} x ON x.ty = h.ty AND x.key = h.key AND x.since = h.since \
                 WHERE h.ty = v.ty AND h.key = g.key LIMIT 1) r",
                names.t("records_head"),
                names.t("records")
            ),
        ),
        None => ("", String::new()),
    };
    let distance = format!(
        "g.unit OPERATOR({}.<->) CAST($1::text AS {})",
        v.schema,
        v.ty(query.len() as u32)
    );
    let from = format!(
        "FROM {} g JOIN {} v ON v.ty = {} AND v.field = {} AND v.key = g.key \
         AND v.until IS NULL",
        names.t(table),
        names.t("vectors"),
        literal(ty.as_str()),
        literal(field),
    );
    // A cursor's name per shape of row: a `FETCH` is prepared once per
    // text, with the columns of the cursor it first read.
    let cursor = if judged.is_some() {
        "graph_heads"
    } else {
        "graph_keys"
    };
    let mut d = db.borrow_mut();
    d.batch(STREAM)?;
    d.execute(
        &format!(
            "DECLARE {cursor} NO SCROLL CURSOR FOR SELECT g.key, v.single, v.data, \
             {distance}{head} {from}{joined} ORDER BY {distance}"
        ),
        &[&unit],
    )?;
    // pgvector computes a distance in `f32`: within this of the `k`th's,
    // a row may still score at or above it.
    let eps = (query.len() as f64 + 8.0) / f64::from(1u32 << 22);
    let near = |d: f64| 1.0 - d * d / 2.0;
    let (mut came, mut handed) = (Vec::new(), HashSet::new());
    let mut bound = None::<f64>;
    let mut batch = (2 * k as usize).clamp(32, FETCH_MOST);
    let mut ended = false;
    'read: loop {
        let rows = d.query(&format!("FETCH {batch} FROM {cursor}"), &[])?;
        let mut admitted = vec![judged.is_none(); rows.len()];
        if let Some((scan, decl)) = judged {
            let read = (rows.iter().enumerate())
                .map(|(i, r)| Ok((i, projection::record_at(r, 4)?)))
                .collect::<Result<Vec<_>>>()?;
            for (i, r) in projection::joined(&mut d, names, read)? {
                admitted[i] = engines::admits(decl, &r, scan);
            }
        }
        for (row, admitted) in rows.iter().zip(admitted) {
            let distance: f64 = row.get(3);
            if bound.is_some_and(|b| near(distance) < b - eps) {
                break 'read;
            }
            let key: String = row.get(0);
            if !handed.insert(key.clone()) || !admitted {
                continue;
            }
            let e = engines::Elements::new(row.get(1), row.get(2)).ranked();
            came.push((RecordKey::from(key), cosine(query, &e)));
            if came.len() == k as usize {
                bound = Some(near(distance));
            }
        }
        if rows.len() < batch {
            ended = true;
            break;
        }
        batch = (batch * 4).min(FETCH_MOST);
    }
    d.batch(&format!(
        "CLOSE {cursor}; SET LOCAL enable_seqscan = on; SET LOCAL enable_sort = on"
    ))?;
    drop(d);
    let short = came.len() < k as usize;
    engines::rank(&mut came, k);
    if ended && short {
        let statement = format!("SELECT g.key, v.single, v.data {from}");
        let rest = |m: u32| {
            let mut top = engines::TopK::new(m);
            db.borrow_mut().query_each(&statement, &[], |row| {
                let key: String = row.get(0);
                if !handed.contains(&key) {
                    let e = engines::Elements::new(row.get(1), row.get(2)).ranked();
                    top.push(&RecordKey::from(key), cosine(query, &e));
                }
                Ok(())
            })?;
            Ok(top.into_ranked())
        };
        let more = k - came.len() as u32;
        let tail = engines::rank_seen(more, seen, rest, |keys| heads(db, names, ty, keys))?;
        came.extend(tail);
    }
    Ok(came)
}

/// Put a vector the index is asked to hold for `ty`'s `key`, as a row of
/// its own (`since` -1). A state's row in force holding it already stays;
/// one holding other elements is taken out ([`remove`]) and the vector
/// put alone stands until the record's next state. The graphs follow at
/// the commit.
pub(crate) fn put(
    db: &mut Db,
    names: &Names,
    (ty, key, field): (&str, &str, &str),
    vector: &[f32],
) -> Result<()> {
    let vectors = names.t("vectors");
    let held = db.query_opt(
        &format!(
            "SELECT single, data FROM {vectors} WHERE ty = $1 AND key = $2 AND field = $3 \
             AND since >= 0 AND until IS NULL"
        ),
        &[&ty, &key, &field],
    )?;
    if held.is_some_and(|r| engines::Elements::new(r.get(0), r.get(1)).ranked() == vector) {
        return Ok(());
    }
    remove(db, names, (ty, key, field))?;
    db.execute(
        &format!(
            "INSERT INTO {vectors} (ty, key, field, since, single, data) \
             VALUES ($1, $2, $3, -1, true, $4)"
        ),
        &[&ty, &key, &field, &engines::vector_bytes(vector)],
    )
    .map(drop)
}

/// Forget the vector the index holds for `ty`'s `key`: one put alone goes,
/// and a state's row in force ends at its own position, so a view of that
/// state still reads it, a read joining a row by its position alone.
pub(crate) fn remove(
    db: &mut Db,
    names: &Names,
    (ty, key, field): (&str, &str, &str),
) -> Result<()> {
    db.execute(
        &format!(
            "WITH e AS (UPDATE {t} SET until = since WHERE ty = $1 AND key = $2 \
             AND field = $3 AND since >= 0 AND until IS NULL) \
             DELETE FROM {t} WHERE ty = $1 AND key = $2 AND field = $3 AND since = -1",
            t = names.t("vectors")
        ),
        &[&ty, &key, &field],
    )
    .map(drop)
}

#[cfg(test)]
mod tests;
