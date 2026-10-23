//! The vector index a lake asks for, kept by pgvector in the server
//! (`docs/decisions/043-an-index-asked-for-answers-its-set.md`).
//!
//! **What it is.** The `sys.setting` `vector_index.<type>` names vector
//! fields of the type. For each, the engine keeps the unit vector of every
//! row of `vectors` in force (`until` empty) in its `unit` column and one
//! partial HNSW index over those rows, ordered by L2 distance, which on
//! unit vectors is cosine's order. A row that ends gives its unit vector
//! up.
//! A search over such a field reads its candidates from the index, as many
//! as it asks for, and ranks them by the one cosine
//! (`lakelands_api::query::cosine`), ties by key: the answer is the
//! index's set, which may miss a neighbour the exact scan would find.
//! Every other field is searched by the exact scan.
//!
//! **A tie at the boundary is pulled deeper.** The index hands out equal
//! distances in its graph's order, not the keys'. A search asks for one
//! candidate more than it answers; while that one scores as the last one
//! answered, it asks for twice as many, so the members of that tie the
//! index hands out are all in the set before the keys cut it. A tie at
//! the boundary of 4 096 candidates or more is the exact scan's.
//!
//! **A short answer is not the field's end.** The index hands out fewer
//! candidates than it was asked for where the field holds no more, and
//! also where it holds more: pgvector reaches only a part of a large
//! group of identical vectors, and ends an iterative scan at
//! `hnsw.max_scan_tuples`. The engine cannot tell these apart by what
//! came, so a pull that comes back short is answered by the exact scan.
//!
//! **Only while it describes the head.** Each indexed field has a mark
//! (`vindex.<type>.<field>`, its dimensions beside it) that the fold keeps
//! at `folded`. The write of a state's rows fills `unit` for a marked
//! field in the same statement, as a `put` of a vector no state names
//! does, so the index follows every write. A search uses the index
//! only while the mark equals `folded`; otherwise it scans, with the exact
//! answer. The commit of the setting's write rebuilds its type's indexes, in
//! the fold's transaction; a writer's open rebuilds those whose mark is behind.
//!
//! **Without pgvector.** The extension is the host's (`docs/hosting.md`).
//! A writer's open creates it if it can; when it cannot, or finds a
//! version before 0.8, no field is marked, every search scans, and the
//! open says so once on standard error when the lake asks for an index.
//!
//! **Constants.** `m = 16` and `ef_construction = 64`, pgvector's defaults;
//! a pull asks the index with `ef_search` at the candidates it pulls, at
//! least 200 and at most 1 000; with `hnsw.iterative_scan =
//! relaxed_order`, so a pull of more candidates than `ef_search` gets
//! them as far as the index hands them out; and with
//! `hnsw.max_scan_tuples = 20 000`, pgvector's default, set by the search
//! itself, so that where a pull comes back short does not move with the
//! server's value of it.

use lakelands_api::error::Result;
use lakelands_api::id::{FieldName, Position, RecordKey, TypeName};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::seam::{SETTING_TYPE, VECTOR_INDEX_SETTING};
use lakelands_core::engines;

use crate::connect::Db;
use crate::projection::{self, Names};

/// Neighbours per node of the HNSW graph (layer 0 keeps twice as many).
const M: u32 = 16;
/// The candidate list while the graph is built.
const EF_CONSTRUCTION: u32 = 64;
/// The candidate list a search keeps at least ...
const EF_SEARCH_FLOOR: u32 = 200;
/// ... and at most: pgvector's own ceiling.
const EF_SEARCH_MAX: u32 = 1000;
/// The most candidates a search pulls for a tie at its boundary; a tie
/// still cut there is the exact scan's
/// (`docs/decisions/043-an-index-asked-for-answers-its-set.md`).
const TIE_CEILING: u32 = 4096;
/// About how many tuples an iterative scan visits before it ends:
/// pgvector's default, which the search sets itself, so that an answer
/// does not move with the server's value.
const MAX_SCAN_TUPLES: u32 = 20_000;
/// The oldest pgvector with iterative index scans.
const MIN_VERSION: (u32, u32) = (0, 8);
/// The rows a rebuild fills at a time.
const FILL_PAGE: i64 = 1024;

/// pgvector, found in the server: the schema its type and operators live
/// in, quoted.
#[derive(Clone, Debug)]
pub(crate) struct Pgvector {
    schema: String,
}

impl Pgvector {
    /// The `vector` type, of `n` dimensions when given.
    pub(crate) fn ty(&self, n: Option<u32>) -> String {
        match n {
            Some(n) => format!("{}.vector({n})", self.schema),
            None => format!("{}.vector", self.schema),
        }
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

/// Give `vectors` its `unit` column, once pgvector is there.
pub(crate) fn prepare(db: &mut Db, names: &Names, v: &Pgvector) -> Result<()> {
    db.batch(&prepare_sql(names, v))
}

/// The statement of [`prepare`].
pub(crate) fn prepare_sql(names: &Names, v: &Pgvector) -> String {
    format!(
        "ALTER TABLE {} ADD COLUMN IF NOT EXISTS unit {}",
        names.t("vectors"),
        v.ty(None)
    )
}

/// The meta row marking `ty.field`'s index kept through a position.
pub(crate) fn mark(ty: &TypeName, field: &str) -> String {
    format!("vindex.{ty}.{field}")
}

/// A vector's length, taken in `f64`; none where it is zero or not
/// finite. A vector of zeros points nowhere: it is never indexed, and
/// never answered for a field read through its index, whichever way the
/// field is then read.
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

/// The name of `ty.field`'s index: stable, and short enough for an
/// identifier whatever the names. `CREATE INDEX` takes it bare (the index
/// goes in its table's schema); [`Names::t`] qualifies it.
fn index_bare(ty: &TypeName, field: &str) -> String {
    use sha2::{Digest as _, Sha256};
    let digest = Sha256::digest(format!("{ty}\0{field}").as_bytes());
    format!("vindex_{}", lakelands_core::canonical::hex(&digest[..8]))
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

/// The fields of `ty` marked as indexed, whatever their marks.
fn marked(db: &mut Db, names: &Names, ty: &TypeName) -> Result<Vec<(String, i64)>> {
    let prefix = format!("vindex.{ty}.");
    Ok(db
        .query(
            &format!(
                "SELECT substr(k, $1), n FROM {} WHERE left(k, $2) = $3",
                names.t("meta")
            ),
            &[&(prefix.len() as i32 + 1), &(prefix.len() as i32), &prefix],
        )?
        .iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect())
}

/// Rebuild `ty`'s vector indexes under its setting and declaration at the
/// head, and mark each kept through `through`: drop every index the type
/// had, then fill the unit vectors of each field asked for and build its
/// index. Without pgvector, only the marks go. The fold does this when it
/// puts the type's head declaration and when it commits the setting's head
/// state, and a writer's open when a mark is behind.
pub(crate) fn rebuild(
    db: &mut Db,
    names: &Names,
    v: Option<&Pgvector>,
    ty: &TypeName,
    through: Position,
) -> Result<()> {
    for (field, _) in marked(db, names, ty)? {
        db.batch(&format!(
            "DROP INDEX IF EXISTS {}",
            names.t(&index_bare(ty, &field))
        ))?;
        if v.is_some() {
            db.execute(
                &format!(
                    "UPDATE {} SET unit = NULL WHERE ty = $1 AND field = $2 AND unit IS NOT NULL",
                    names.t("vectors")
                ),
                &[&ty.as_str(), &field],
            )?;
        }
        db.execute(
            &format!("DELETE FROM {} WHERE k = $1", names.t("meta")),
            &[&mark(ty, &field)],
        )?;
    }
    let Some(v) = v else { return Ok(()) };
    let Some(decl) = projection::head_type(db, names, ty)?.decl else {
        return Ok(());
    };
    for (field, n) in wanted(db, names, &decl)? {
        fill(db, names, ty, field.as_str(), v)?;
        // An index a clear left without its mark goes too.
        db.batch(&format!(
            "DROP INDEX IF EXISTS {}",
            names.t(&index_bare(ty, field.as_str()))
        ))?;
        db.batch(&format!(
            "CREATE INDEX {name} ON {vectors} USING hnsw ((unit::{vt}) {s}.vector_l2_ops) \
             WITH (m = {M}, ef_construction = {EF_CONSTRUCTION}) \
             WHERE ty = {ty} AND field = {field} AND until IS NULL",
            name = index_bare(ty, field.as_str()),
            vectors = names.t("vectors"),
            vt = v.ty(Some(n)),
            s = v.schema,
            ty = literal(ty.as_str()),
            field = literal(field.as_str()),
        ))?;
        db.execute(
            &format!(
                "INSERT INTO {} (k, n, t) VALUES ($1, $2, $3) \
                 ON CONFLICT (k) DO UPDATE SET n = $2, t = $3",
                names.t("meta"),
            ),
            &[
                &mark(ty, field.as_str()),
                &(through.0 as i64),
                &n.to_string(),
            ],
        )?;
    }
    Ok(())
}

/// After `ty`'s declaration is put: rebuild its vector indexes when it has
/// any or asks for any, since the declaration may change a field's
/// dimensions or drop it.
pub(crate) fn redeclared(
    db: &mut Db,
    names: &Names,
    v: Option<&Pgvector>,
    ty: &TypeName,
    at: Position,
) -> Result<()> {
    let asks = match (v, projection::head_type(db, names, ty)?.decl) {
        (Some(_), Some(decl)) => !wanted(db, names, &decl)?.is_empty(),
        _ => false,
    };
    if asks || !marked(db, names, ty)?.is_empty() {
        rebuild(db, names, v, ty, at)?;
    }
    Ok(())
}

/// Fill the unit vectors of every vector `ty.field` holds, a page at a
/// time.
fn fill(db: &mut Db, names: &Names, ty: &TypeName, field: &str, v: &Pgvector) -> Result<()> {
    let mut after = String::new();
    loop {
        let rows = db.query(
            &format!(
                "SELECT key, single, data FROM {} WHERE ty = $1 AND field = $2 AND until IS NULL \
                 AND key > $3 ORDER BY key LIMIT $4",
                names.t("vectors")
            ),
            &[&ty.as_str(), &field, &after, &FILL_PAGE],
        )?;
        let Some(last) = rows.last() else { break };
        after = last.get(0);
        let (keys, units): (Vec<String>, Vec<Option<String>>) = rows
            .iter()
            .map(|r| {
                let key: String = r.get(0);
                let e = engines::Elements {
                    single: r.get(1),
                    data: r.get(2),
                };
                (key, unit_text(&e.ranked()))
            })
            .unzip();
        db.execute(
            &format!(
                "UPDATE {vectors} AS t SET unit = CAST(u.unit AS {vt}) \
                 FROM unnest($3::text[], $4::text[]) AS u (key, unit) \
                 WHERE t.ty = $1 AND t.field = $2 AND t.key = u.key AND t.until IS NULL",
                vectors = names.t("vectors"),
                vt = v.ty(None),
            ),
            &[&ty.as_str(), &field, &keys, &units],
        )?;
        if (rows.len() as i64) < FILL_PAGE {
            break;
        }
    }
    Ok(())
}

/// At a writer's open: rebuild, in one transaction, the indexes of every
/// type whose marks do not describe the head (a writer before the index
/// folded past them, wrote the setting, or ran without pgvector), and say
/// once why there is no index when the lake asks for one and pgvector is
/// missing.
pub(crate) fn catch_up(
    db: &mut Db,
    names: &Names,
    v: std::result::Result<&Pgvector, &str>,
    lake: &str,
) -> Result<()> {
    db.transaction("BEGIN ISOLATION LEVEL READ COMMITTED", |db| {
        let Some(folded) = projection::folded(db, names)? else {
            return Ok(());
        };
        let asked: Vec<String> = db.column(
            &format!(
                "SELECT substr(key, $1) FROM {} WHERE ty = $2 AND left(key, $1 - 1) = $3",
                names.t("records_head"),
            ),
            &[
                &(VECTOR_INDEX_SETTING.len() as i32 + 1),
                &SETTING_TYPE,
                &VECTOR_INDEX_SETTING,
            ],
        )?;
        let with_marks: Vec<String> = db
            .query(
                &format!("SELECT k FROM {} WHERE k LIKE 'vindex.%'", names.t("meta")),
                &[],
            )?
            .iter()
            .filter_map(|r| {
                let k: String = r.get(0);
                let (ty, _) = k.strip_prefix("vindex.")?.rsplit_once('.')?;
                Some(ty.to_owned())
            })
            .collect();
        let mut types: Vec<String> = asked.iter().chain(&with_marks).cloned().collect();
        types.sort();
        types.dedup();
        let mut unserved = false;
        for ty in types {
            let ty = TypeName::from(ty);
            let want: Vec<(String, i64)> = match (v, projection::head_type(db, names, &ty)?.decl) {
                (Ok(_), Some(decl)) => wanted(db, names, &decl)?
                    .into_iter()
                    .map(|(f, _)| (f.as_str().to_owned(), folded.0 as i64))
                    .collect(),
                (Err(_), Some(decl)) => {
                    unserved |= !wanted(db, names, &decl)?.is_empty();
                    Vec::new()
                }
                (_, None) => Vec::new(),
            };
            let mut have = marked(db, names, &ty)?;
            have.sort();
            let mut want = want;
            want.sort();
            if have != want {
                rebuild(db, names, v.ok(), &ty, folded)?;
            }
        }
        if let (Err(why), true) = (v, unserved) {
            eprintln!(
                "lakelands: lake `{lake}` asks for a vector index, and {why}; its searches read \
                 every vector, exactly, until pgvector 0.8 or later is installed \
                 (docs/hosting.md)"
            );
        }
        Ok(())
    })
}

/// Whether a search of `ty.field` may use its index: the field's mark
/// equals `folded`. Its dimensions when it may.
pub(crate) fn usable(
    db: &mut Db,
    names: &Names,
    ty: &TypeName,
    field: &str,
) -> Result<Option<u32>> {
    Ok(db
        .query_opt(
            &format!(
                "SELECT m.t FROM {meta} m JOIN {meta} f ON f.k = 'folded' AND f.n = m.n \
                 WHERE m.k = $1",
                meta = names.t("meta")
            ),
            &[&mark(ty, field)],
        )?
        .and_then(|r| r.get::<_, Option<String>>(0))
        .and_then(|t| t.parse().ok()))
}

/// The statement a search of `ty.field` reads its candidates with, nearest
/// first by the index; `$1` the query's unit vector, `$2` how many.
pub(crate) fn candidates_sql(
    names: &Names,
    v: &Pgvector,
    ty: &TypeName,
    field: &str,
    n: u32,
) -> String {
    format!(
        "SELECT key, single, data FROM {vectors} WHERE ty = {ty} AND field = {field} \
         AND until IS NULL ORDER BY (unit::{vt}) OPERATOR({s}.<->) CAST($1::text AS {vt}) LIMIT $2",
        vectors = names.t("vectors"),
        ty = literal(ty.as_str()),
        field = literal(field),
        vt = v.ty(Some(n)),
        s = v.schema,
    )
}

/// Whether the boundary of the best `k` cuts a tie among the candidates
/// the index handed out, more than `k` of them: fewer than `k` score
/// above the least, so the `k`th scores as the least does, and a
/// candidate the index did not hand out may score so too.
pub(crate) fn cuts_a_tie(came: &[(RecordKey, f64)], k: u32) -> bool {
    let Some(least) = came.iter().map(|c| c.1).min_by(f64::total_cmp) else {
        return false;
    };
    let above = came.iter().filter(|c| c.1.total_cmp(&least).is_gt());
    came.len() > k as usize && above.count() < k as usize
}

/// The depth a search asks its index for after `depth` candidates cut a
/// tie: twice as many, at most the ceiling; none past it, where the scan
/// answers.
pub(crate) fn deeper(depth: u32) -> Option<u32> {
    let next = depth.saturating_mul(2).min(TIE_CEILING);
    (next > depth).then_some(next)
}

/// The settings a pull of the index starts with: the candidate list the
/// index keeps for a pull of `k` candidates, iterative scans past it as far as the
/// engine's own number of tuples, and no plan that sorts the field
/// instead. On a small field the planner would rather scan and sort, and
/// its choice moves with the table's statistics; an indexed field answers
/// the index's set whatever the statistics say.
pub(crate) fn search_settings(k: u32) -> String {
    let ef = k.clamp(EF_SEARCH_FLOOR, EF_SEARCH_MAX);
    format!(
        "SET LOCAL hnsw.ef_search = {ef}; SET LOCAL hnsw.iterative_scan = relaxed_order; \
         SET LOCAL hnsw.max_scan_tuples = {MAX_SCAN_TUPLES}; \
         SET LOCAL enable_seqscan = off; SET LOCAL enable_sort = off"
    )
}

/// The statement a `put` writes a vector with, as a row of its own
/// (`since` -1) where no row of a state holds one in force, filling its
/// unit vector when its field is marked; `$5` the mark, `$6` the unit
/// vector, unbound without pgvector.
pub(crate) fn put_sql(names: &Names, v: Option<&Pgvector>) -> String {
    let (column, unit, set) = match v {
        Some(v) => (
            ", unit",
            format!(
                ", CASE WHEN EXISTS (SELECT 1 FROM {} WHERE k = $5) THEN CAST($6::text AS {}) END",
                names.t("meta"),
                v.ty(None)
            ),
            ", unit = EXCLUDED.unit",
        ),
        None => ("", String::new(), ""),
    };
    format!(
        "INSERT INTO {vectors} (ty, key, field, since, single, data{column}) \
         SELECT $1, $3, $2, -1, true, $4{unit} WHERE NOT EXISTS (SELECT 1 FROM {vectors} \
         WHERE ty = $1 AND key = $3 AND field = $2 AND since >= 0 AND until IS NULL) \
         ON CONFLICT (ty, key, field, since) DO UPDATE SET data = EXCLUDED.data{set}",
        vectors = names.t("vectors"),
    )
}

#[cfg(test)]
mod tests;
