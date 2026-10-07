//! The projection in PostgreSQL, with its kept history.
//!
//! **History.** Every state the fold puts is a row valid from a position
//! (`since`) until the position of the state that supersedes it (`until`,
//! none while it is the latest). A view at a position reads, for each
//! record, declaration, proposal, membership and recorded output, the row
//! whose interval holds that position; nothing is replayed. A state put
//! again at the same position replaces the row there, which is the same
//! state folded twice.
//!
//! **Head indexes.** Beside the history the engine keeps indexes of the
//! latest states only: exact text values for equality lookups, the links
//! of `via` relations, one row per full-text token of a record, and the
//! vectors. They are rewritten in the fold's own transaction, from the
//! record and the declaration in force, so they always describe the head.
//! A view uses them only when it reads the head, and only to find
//! candidates: whether a candidate matches, where it sorts and what it
//! scores is decided by `lakelands_core::engines`, the rules every engine
//! shares, so no index can change an answer.
//!
//! **Full text.** A record's full-text rows are the tokens the one
//! pipeline makes of its searchable fields, one per distinct token. A
//! query's candidates are the records holding any of its tokens, which is
//! exactly the set the shared scoring counts: there is no second tokenizer
//! to agree with
//! (`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`).
//!
//! **Recorded outputs** keep their history too: a later output for the
//! same call (a failed call made again after its function was declared
//! again) supersedes the earlier from its own position on, so the head
//! answers the latest and a view at an earlier position answers what was
//! recorded by then.
//!
//! Every table is named with the lake's schema ([`Names`]).

use std::collections::{BTreeMap, HashMap};

use lakelands_api::engine::Declared;
use lakelands_api::error::Result;
use lakelands_api::id::{FieldName, Position, RecordKey, TypeName};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::{KEPT_PREFIX, SETTING_TYPE, SORT_INDEX_SETTING};
use lakelands_api::write::Declaration;
use lakelands_core::engines::{self, decode};
use postgres::types::ToSql;

use crate::connect::Db;

/// The lake's tables, each named with its schema.
#[derive(Clone, Debug)]
pub(crate) struct Names {
    schema: String,
}

impl Names {
    /// The tables of the schema written as `quoted`.
    pub(crate) fn new(quoted: String) -> Names {
        Names { schema: quoted }
    }

    /// One table, schema-qualified.
    pub(crate) fn t(&self, table: &str) -> String {
        format!("{}.{table}", self.schema)
    }

    /// The statements that create the lake's tables, if absent.
    ///
    /// `text_index` has no primary key: a record's value is in it once by
    /// the unique index `text_index_value`, whose last column is the key
    /// in byte order, so the records holding a text are read from it in
    /// key order (`in_key_order` in `view.rs`). A lake made with a primary
    /// key there keeps it and gains the index.
    ///
    /// `records_head` holds the states with no end of the kept names
    /// ([`kept_range`]) in the order keys are read in, bytewise:
    /// `records_at` holds the key in the database's collation and
    /// `records_key_c` every state. A kept name's page and count at the
    /// head read from it (`kept_sql` in `view.rs`).
    pub(crate) fn schema_sql(&self) -> String {
        let (s, kept) = (&self.schema, kept_range());
        format!(
            "
CREATE SCHEMA IF NOT EXISTS {s};
CREATE TABLE IF NOT EXISTS {s}.meta (
    k TEXT PRIMARY KEY, n BIGINT, t TEXT
);
CREATE TABLE IF NOT EXISTS {s}.lease (
    one INTEGER PRIMARY KEY CHECK (one = 1),
    fence BIGINT NOT NULL, holder TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS {s}.segments (
    first BIGINT PRIMARY KEY, digest TEXT
);
CREATE TABLE IF NOT EXISTS {s}.log (
    position BIGINT PRIMARY KEY, segment BIGINT NOT NULL, fence BIGINT NOT NULL,
    id TEXT NOT NULL, prev TEXT NOT NULL, hash TEXT NOT NULL, bytes BYTEA NOT NULL
);
CREATE INDEX IF NOT EXISTS log_segment ON {s}.log (segment, position);
CREATE TABLE IF NOT EXISTS {s}.declarations (
    ident TEXT NOT NULL, since BIGINT NOT NULL, until BIGINT, state TEXT NOT NULL,
    PRIMARY KEY (ident, since)
);
CREATE TABLE IF NOT EXISTS {s}.records (
    ty TEXT NOT NULL, key TEXT NOT NULL, since BIGINT NOT NULL, until BIGINT,
    state TEXT NOT NULL,
    PRIMARY KEY (ty, key, since)
);
CREATE INDEX IF NOT EXISTS records_at ON {s}.records (ty, until, key);
CREATE INDEX IF NOT EXISTS records_since ON {s}.records (ty, since);
CREATE INDEX IF NOT EXISTS records_key_c ON {s}.records (ty, key COLLATE \"C\", since);
CREATE INDEX IF NOT EXISTS records_head ON {s}.records (ty, key COLLATE \"C\")
    WHERE until IS NULL AND {kept};
CREATE TABLE IF NOT EXISTS {s}.proposals (
    id TEXT NOT NULL, since BIGINT NOT NULL, until BIGINT, state TEXT NOT NULL,
    PRIMARY KEY (id, since)
);
CREATE TABLE IF NOT EXISTS {s}.members (
    object_set TEXT NOT NULL, ty TEXT NOT NULL, key TEXT NOT NULL,
    since BIGINT NOT NULL, until BIGINT, state BOOLEAN NOT NULL,
    PRIMARY KEY (object_set, ty, key, since)
);
CREATE TABLE IF NOT EXISTS {s}.outputs (
    seam TEXT NOT NULL, input TEXT NOT NULL, since BIGINT NOT NULL, until BIGINT,
    state TEXT NOT NULL,
    PRIMARY KEY (seam, input, since)
);
CREATE TABLE IF NOT EXISTS {s}.applied (
    id TEXT PRIMARY KEY, at BIGINT NOT NULL
);
CREATE TABLE IF NOT EXISTS {s}.text_index (
    ty TEXT NOT NULL, field TEXT NOT NULL, value TEXT NOT NULL, key TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS text_index_value
    ON {s}.text_index (ty, field, value, key COLLATE \"C\");
CREATE INDEX IF NOT EXISTS text_index_key ON {s}.text_index (ty, key);
CREATE TABLE IF NOT EXISTS {s}.edges (
    src_ty TEXT NOT NULL, src_key TEXT NOT NULL, relation TEXT NOT NULL,
    dst_ty TEXT NOT NULL, dst_key TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS edges_src ON {s}.edges (src_ty, src_key);
CREATE INDEX IF NOT EXISTS edges_dst ON {s}.edges (dst_ty, dst_key);
CREATE TABLE IF NOT EXISTS {s}.full_text (
    ty TEXT NOT NULL, token TEXT NOT NULL, key TEXT NOT NULL,
    PRIMARY KEY (ty, token, key)
);
CREATE INDEX IF NOT EXISTS full_text_key ON {s}.full_text (ty, key);
CREATE TABLE IF NOT EXISTS {s}.sort_index (
    ty TEXT NOT NULL, field TEXT NOT NULL, key TEXT NOT NULL, sort_key BYTEA NOT NULL,
    PRIMARY KEY (ty, key, field)
);
CREATE INDEX IF NOT EXISTS sort_index_asc ON {s}.sort_index (ty, field, sort_key, key COLLATE \"C\");
CREATE INDEX IF NOT EXISTS sort_index_desc
    ON {s}.sort_index (ty, field, sort_key DESC, key COLLATE \"C\");
CREATE TABLE IF NOT EXISTS {s}.vectors (
    ty TEXT NOT NULL, field TEXT NOT NULL, key TEXT NOT NULL, data BYTEA NOT NULL,
    PRIMARY KEY (ty, field, key)
);
"
        )
    }
}

/// The condition that the column `ty` holds a kept name
/// (`lakelands_api::seam::KEPT_PREFIX`): the range, bytewise, that the
/// names with that prefix fill. `records_head` is over these rows alone,
/// so a stored type's put does not maintain it, and a kept name's
/// statements at the head repeat the condition word for word, by which
/// the planner knows the index holds their rows.
pub(crate) fn kept_range() -> String {
    let (last, rest) = KEPT_PREFIX.as_bytes().split_last().expect("a prefix");
    let end = format!("{}{}", String::from_utf8_lossy(rest), char::from(last + 1));
    format!("ty COLLATE \"C\" >= '{KEPT_PREFIX}' AND ty COLLATE \"C\" < '{end}'")
}

/// Every table of a lake's schema: the log's and the lease's, then the
/// projection's.
pub(crate) const LAKE_TABLES: [&str; 15] = [
    "meta",
    "lease",
    "segments",
    "log",
    "declarations",
    "records",
    "proposals",
    "members",
    "outputs",
    "applied",
    "text_index",
    "edges",
    "full_text",
    "sort_index",
    "vectors",
];

/// The projection's tables, cleared by a rebuild; the log, its segments,
/// the lease and the lake's name stay.
pub(crate) const PROJECTION_TABLES: &[&str] = LAKE_TABLES.split_at(4).1;

/// A history table: its name and the columns naming one thing in it.
pub(crate) struct History {
    pub(crate) table: &'static str,
    pub(crate) ident: &'static [&'static str],
    /// The SQL type of its `state` column.
    pub(crate) state: &'static str,
}

pub(crate) const DECLARATIONS: History = History::new("declarations", &["ident"], "text");
pub(crate) const RECORDS: History = History::new("records", &["ty", "key"], "text");
pub(crate) const PROPOSALS: History = History::new("proposals", &["id"], "text");
pub(crate) const OUTPUTS: History = History::new("outputs", &["seam", "input"], "text");
pub(crate) const MEMBERS: History =
    History::new("members", &["object_set", "ty", "key"], "boolean");

impl History {
    const fn new(table: &'static str, ident: &'static [&'static str], state: &'static str) -> Self {
        History {
            table,
            ident,
            state,
        }
    }

    /// The identifying columns, each as `f` writes it with its number
    /// (from 1), joined by `sep`.
    fn each(&self, sep: &str, f: impl Fn(usize, &str) -> String) -> String {
        let all: Vec<String> = (1..).zip(self.ident).map(|(i, c)| f(i, c)).collect();
        all.join(sep)
    }

    /// Put a state of one thing valid from `since`, keeping every other
    /// state: the one it supersedes ends at `since`, and it ends where the
    /// next one, if any, begins. A state already at `since` is replaced
    /// and keeps its end.
    ///
    /// One statement: the supersede and the insert or replace, in one
    /// snapshot. Declarations are put so, as they come; every other state
    /// is buffered and put with [`History::put_many`].
    pub(crate) fn put(
        &self,
        db: &mut Db,
        names: &Names,
        ident: &[&(dyn ToSql + Sync)],
        since: Position,
        state: &(dyn ToSql + Sync),
    ) -> Result<()> {
        let table = names.t(self.table);
        let n = self.ident.len();
        let at = since.0 as i64;
        let mut args: Vec<&(dyn ToSql + Sync)> = ident.to_vec();
        args.push(&at);
        args.push(state);
        let columns = self.ident.join(", ");
        let marks: Vec<String> = (1..=n + 1).map(|i| format!("${i}")).collect();
        db.execute(
            &format!(
                "WITH superseded AS (\
                   UPDATE {table} SET until = ${s} WHERE {w} AND since = \
                   (SELECT max(since) FROM {table} WHERE {w} AND since < ${s}) \
                   AND (until IS NULL OR until > ${s})\
                 ) \
                 INSERT INTO {table} ({columns}, since, until, state) VALUES ({marks}, \
                 (SELECT min(since) FROM {table} WHERE {w} AND since > ${s}), ${v}) \
                 ON CONFLICT ({columns}, since) DO UPDATE SET state = EXCLUDED.state",
                w = self.each(" AND ", |i, c| format!("{c} = ${i}")),
                s = n + 1,
                v = n + 2,
                marks = marks.join(", "),
            ),
            &args,
        )
    }

    /// Put the states a transaction buffered, all in one statement, as
    /// the one-row put would put them one after the other: for every
    /// thing, its rows from the one in force at its earliest buffered
    /// position on and its buffered rows (a buffered row replacing one at
    /// the same position) are ordered by position, and each ends where the
    /// next begins. Its earlier rows are not touched.
    ///
    /// Answers the things whose latest state is now a buffered one, each
    /// with whether the history held a state of it before.
    pub(crate) fn put_many<S: ToSql + Sync>(
        &self,
        db: &mut Db,
        names: &Names,
        rows: &BTreeMap<(Vec<String>, i64), S>,
    ) -> Result<Vec<(Vec<String>, bool)>> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let table = names.t(self.table);
        let n = self.ident.len();
        let mut idents: Vec<Vec<&str>> = vec![Vec::with_capacity(rows.len()); n];
        let mut since = Vec::with_capacity(rows.len());
        let mut states = Vec::with_capacity(rows.len());
        for ((ident, at), state) in rows {
            for (column, v) in idents.iter_mut().zip(ident) {
                column.push(v.as_str());
            }
            since.push(*at);
            states.push(state);
        }
        let mut args: Vec<&(dyn ToSql + Sync)> = Vec::with_capacity(n + 2);
        for column in &idents {
            args.push(column);
        }
        args.push(&since);
        args.push(&states);
        let columns = self.ident.join(", ");
        let arrays: Vec<String> = (1..=n).map(|i| format!("${i}::text[]")).collect();
        let answer = db.query(
            &format!(
                "WITH b AS (\
                   SELECT * FROM unnest({arrays}, ${s}::int8[], ${v}::{ty}[]) \
                   AS b({columns}, since, state)\
                 ), f AS (\
                   SELECT {columns}, min(since) AS first FROM b GROUP BY {columns}\
                 ), u AS (\
                   SELECT {columns}, since, bool_or(fresh) AS fresh FROM (\
                     SELECT {scolumns}, s.since, false AS fresh FROM f CROSS JOIN LATERAL (\
                       (SELECT {pcolumns}, p.since, p.until FROM {table} p \
                        WHERE {pf} AND p.since <= f.first ORDER BY p.since DESC LIMIT 1) \
                       UNION ALL \
                       (SELECT {pcolumns}, p.since, p.until FROM {table} p \
                        WHERE {pf} AND p.since > f.first)\
                     ) s WHERE s.until IS NULL OR s.until > f.first \
                     UNION ALL SELECT {columns}, since, true FROM b\
                   ) x GROUP BY {columns}, since\
                 ), n AS (\
                   SELECT {columns}, since, fresh, \
                   lead(since) OVER (PARTITION BY {columns} ORDER BY since) AS next FROM u\
                 ), ended AS (\
                   UPDATE {table} t SET until = n.next FROM n \
                   WHERE NOT n.fresh AND {tn} AND t.since = n.since \
                   AND t.until IS DISTINCT FROM n.next\
                 ), put AS (\
                   INSERT INTO {table} ({columns}, since, until, state) \
                   SELECT {bcolumns}, b.since, n.next, b.state FROM b JOIN n ON {bn} \
                   AND b.since = n.since \
                   ON CONFLICT ({columns}, since) DO UPDATE \
                   SET state = EXCLUDED.state, until = EXCLUDED.until\
                 ) \
                 SELECT {ncolumns}, EXISTS (SELECT 1 FROM {table} t WHERE {tn}) \
                 FROM n WHERE n.fresh AND n.next IS NULL",
                arrays = arrays.join(", "),
                s = n + 1,
                v = n + 2,
                ty = self.state,
                scolumns = self.each(", ", |_, c| format!("s.{c}")),
                pcolumns = self.each(", ", |_, c| format!("p.{c}")),
                bcolumns = self.each(", ", |_, c| format!("b.{c}")),
                ncolumns = self.each(", ", |_, c| format!("n.{c}")),
                pf = self.each(" AND ", |_, c| format!("p.{c} = f.{c}")),
                tn = self.each(" AND ", |_, c| format!("t.{c} = n.{c}")),
                bn = self.each(" AND ", |_, c| format!("b.{c} = n.{c}")),
            ),
            &args,
        )?;
        Ok(answer
            .iter()
            .map(|row| ((0..n).map(|i| row.get(i)).collect(), row.get(n)))
            .collect())
    }
}

/// The declarations of types in force at the head, as read in the fold's
/// open transaction, each with the fields its order index keeps: each
/// type's read once, with its `sort_index.<type>` setting in the same
/// round trip, and all forgotten when a declaration is put, since it may
/// change one. A type whose setting the transaction puts is forgotten
/// when its buffer is written. Dropped with the transaction.
#[derive(Default)]
pub(crate) struct HeadTypes(HashMap<TypeName, HeadType>);

/// A type's declaration at the head and the fields its order index keeps.
#[derive(Clone, Default)]
pub(crate) struct HeadType {
    pub(crate) decl: Option<TypeDecl>,
    pub(crate) sorted: Vec<FieldName>,
}

impl HeadTypes {
    fn get(&mut self, db: &mut Db, names: &Names, ty: &TypeName) -> Result<HeadType> {
        if let Some(head) = self.0.get(ty) {
            return Ok(head.clone());
        }
        let head = head_type(db, names, ty)?;
        self.0.insert(ty.clone(), head.clone());
        Ok(head)
    }
}

/// `ty`'s declaration at the head and the fields its order index keeps:
/// those its `sort_index.<type>` setting at the head names that the
/// declaration declares with a kind the index keeps. One round trip.
pub(crate) fn head_type(db: &mut Db, names: &Names, ty: &TypeName) -> Result<HeadType> {
    let row = db.query_one(
        &format!(
            "SELECT (SELECT state FROM {d} WHERE ident = $1 AND until IS NULL), \
             (SELECT state FROM {r} WHERE ty = $2 AND key = $3 AND until IS NULL)",
            d = names.t("declarations"),
            r = names.t("records"),
        ),
        &[
            &format!("type {ty}"),
            &SETTING_TYPE,
            &engines::sort_index_key(ty),
        ],
    )?;
    let decl = declared_type(row.get(0))?;
    let setting = row
        .get::<_, Option<String>>(1)
        .map(|state| decode::<StoredRecord>(&state, "a record"))
        .transpose()?;
    let sorted = match &decl {
        Some(d) => engines::sorted_fields(d, setting.as_ref().and_then(engines::setting_value)),
        None => Vec::new(),
    };
    Ok(HeadType { decl, sorted })
}

/// The condition that a history row holds at the position bound as `$n`.
pub(crate) fn holds_at(n: usize) -> String {
    format!("since <= ${n} AND (until IS NULL OR until > ${n})")
}

/// The position the projection reflects, if any.
pub(crate) fn folded(db: &mut Db, names: &Names) -> Result<Option<Position>> {
    let row = db.query_opt(
        &format!("SELECT n FROM {} WHERE k = 'folded'", names.t("meta")),
        &[],
    )?;
    Ok(row
        .and_then(|r| r.get::<_, Option<i64>>(0))
        .map(|v| Position(v as u64)))
}

/// Record the position folded through, and that every order index and
/// vector index the lake keeps (a `sorted.<type>` or `vindex.<type>.<field>`
/// mark each) is kept through it too, in one statement.
pub(crate) fn set_folded(db: &mut Db, names: &Names, through: Position) -> Result<()> {
    db.execute(
        &format!(
            "WITH s AS (UPDATE {m} SET n = $1 WHERE k LIKE 'sorted.%' OR k LIKE 'vindex.%') \
             INSERT INTO {m} (k, n) VALUES ('folded', $1) ON CONFLICT (k) DO UPDATE SET n = $1",
            m = names.t("meta")
        ),
        &[&(through.0 as i64)],
    )
}

/// The records an order index's rebuild reads at a time.
const REBUILD_PAGE: i64 = 1024;

/// `ANALYZE` over the tables a view plans from: `records` and the head
/// indexes. One statement, run outside any transaction.
pub(crate) fn analyze(db: &mut Db, names: &Names) -> Result<()> {
    let tables: Vec<String> = ["records", "text_index", "edges", "full_text", "sort_index"]
        .iter()
        .map(|t| names.t(t))
        .collect();
    db.batch(&format!("ANALYZE {}", tables.join(", ")))
}

/// Rewrite `ty`'s order index from its head records, under its setting and
/// declaration at the head, and mark it kept through `through`; with no
/// field to keep, drop its rows and its mark. The fold does this when it
/// writes the setting's head state, and a writer's open when the mark is
/// behind. It answers the rows it wrote, which the planner is told of as
/// of the records put (`handle`'s module doc).
pub(crate) fn rebuild_sorted(
    db: &mut Db,
    names: &Names,
    ty: &TypeName,
    through: Position,
) -> Result<u64> {
    db.execute(
        &format!("DELETE FROM {} WHERE ty = $1", names.t("sort_index")),
        &[&ty.as_str()],
    )?;
    let head = head_type(db, names, ty)?;
    if head.sorted.is_empty() {
        return set_sorted(db, names, ty, None).map(|()| 0);
    }
    // A page of the type at a time, so the pass holds a page, not the type.
    let (mut after, mut rows) = (String::new(), 0);
    loop {
        let page = head_records_after(db, names, ty, &after, REBUILD_PAGE)?;
        let Some(last) = page.last() else { break };
        after = last.key.to_string();
        let (mut fields, mut keys, mut sort_keys) = (Vec::new(), Vec::new(), Vec::new());
        for r in &page {
            for (field, sort_key) in engines::order_rows(&head.sorted, r) {
                fields.push(field.as_str().to_owned());
                keys.push(r.key.as_str().to_owned());
                sort_keys.push(sort_key);
            }
        }
        db.execute(
            &format!(
                "INSERT INTO {} (ty, field, key, sort_key) \
                 SELECT $1, * FROM unnest($2::text[], $3::text[], $4::bytea[])",
                names.t("sort_index"),
            ),
            &[&ty.as_str(), &fields, &keys, &sort_keys],
        )?;
        rows += keys.len() as u64;
        if (page.len() as i64) < REBUILD_PAGE {
            break;
        }
    }
    set_sorted(db, names, ty, Some(through)).map(|()| rows)
}

/// Mark `ty`'s order index kept through `through`; with none, drop its
/// mark: the index keeps no field.
fn set_sorted(db: &mut Db, names: &Names, ty: &TypeName, through: Option<Position>) -> Result<()> {
    let (meta, mark) = (names.t("meta"), engines::sorted_mark(ty));
    let Some(at) = through else {
        return db.execute(&format!("DELETE FROM {meta} WHERE k = $1"), &[&mark]);
    };
    db.execute(
        &format!("INSERT INTO {meta} (k, n) VALUES ($1, $2) ON CONFLICT (k) DO UPDATE SET n = $2"),
        &[&mark, &(at.0 as i64)],
    )
}

/// At a writer's open: rebuild, in one transaction, every order index
/// that does not describe the head, because a writer that does not keep
/// it (a binary before it) folded past its mark or wrote its setting, and
/// drop the index of a type whose setting such a writer removed.
pub(crate) fn catch_up_sorted(db: &mut Db, names: &Names) -> Result<()> {
    db.transaction("BEGIN ISOLATION LEVEL READ COMMITTED", |db| {
        let Some(folded) = folded(db, names)? else {
            return Ok(());
        };
        let behind: Vec<String> = db.column(
            &format!(
                "SELECT substr(r.key, $1) FROM {r} r WHERE r.ty = $2 AND left(r.key, $1 - 1) = $3 \
                   AND r.until IS NULL AND NOT EXISTS (SELECT 1 FROM {m} m \
                   WHERE m.k = 'sorted.' || substr(r.key, $1) AND m.n = $4) \
                 UNION SELECT substr(k, 8) FROM {m} WHERE k LIKE 'sorted.%' AND n <> $4",
                r = names.t("records"),
                m = names.t("meta"),
            ),
            &[
                &(SORT_INDEX_SETTING.len() as i32 + 1),
                &SETTING_TYPE,
                &SORT_INDEX_SETTING,
                &(folded.0 as i64),
            ],
        )?;
        for ty in behind {
            rebuild_sorted(db, names, &TypeName::from(ty), folded)?;
        }
        Ok(())
    })
}

pub(crate) fn decode_record(state: &str) -> Result<StoredRecord> {
    decode(state, "a record")
}

/// The declaration of a type in force at `at` (`None` for the head).
pub(crate) fn type_at(
    db: &mut Db,
    names: &Names,
    ty: &TypeName,
    at: Option<Position>,
) -> Result<Option<TypeDecl>> {
    let ident = format!("type {ty}");
    let table = names.t("declarations");
    let row = match at {
        Some(at) => db.query_opt(
            &format!(
                "SELECT state FROM {table} WHERE ident = $1 AND {}",
                holds_at(2)
            ),
            &[&ident, &(at.0 as i64)],
        )?,
        None => db.query_opt(
            &format!("SELECT state FROM {table} WHERE ident = $1 AND until IS NULL"),
            &[&ident],
        )?,
    };
    declared_type(row.map(|r| r.get(0)))
}

/// The type a declaration's stored state declares, if it is a type's.
pub(crate) fn declared_type(state: Option<String>) -> Result<Option<TypeDecl>> {
    let Some(state) = state else { return Ok(None) };
    let declared: Declared = decode(&state, "a declaration")?;
    Ok(match declared.declaration {
        Declaration::Type(t) => Some(t),
        _ => None,
    })
}

/// At most `n` latest states of one type's records, in key order after
/// `after`: a page of the type, so a pass over it holds a page at a time.
pub(crate) fn head_records_after(
    db: &mut Db,
    names: &Names,
    ty: &TypeName,
    after: &str,
    n: i64,
) -> Result<Vec<StoredRecord>> {
    db.records(
        &format!(
            "SELECT state FROM {} WHERE ty = $1 AND until IS NULL AND key COLLATE \"C\" > $2 \
             ORDER BY key COLLATE \"C\" LIMIT $3",
            names.t("records")
        ),
        &[&ty.as_str(), &after, &n],
    )
}

/// The latest state of one record.
pub(crate) fn head_record(
    db: &mut Db,
    names: &Names,
    ty: &TypeName,
    key: &RecordKey,
) -> Result<Option<StoredRecord>> {
    db.query_opt(
        &format!(
            "SELECT state FROM {} WHERE ty = $1 AND key = $2 AND until IS NULL",
            names.t("records")
        ),
        &[&ty.as_str(), &key.as_str()],
    )?
    .map(|r| decode_record(&r.get::<_, String>(0)))
    .transpose()
}

/// The head index rows of records, as arrays for one statement each.
#[derive(Default)]
struct IndexRows<'r> {
    text: [Vec<&'r str>; 4],
    tokens: [Vec<String>; 3],
    edges: [Vec<String>; 5],
    sorted: SortRows<'r>,
}

/// The order index rows of records, as arrays: types, fields, keys and
/// sort keys.
type SortRows<'r> = (Vec<&'r str>, Vec<String>, Vec<&'r str>, Vec<Vec<u8>>);

/// Rewrite the head indexes of records from their latest states, each
/// under the declaration of its type at the head, in two statements: the
/// old rows of the `stale` records that are not among the new ones go,
/// then the new rows of all of them go in, passing over those that stayed.
/// A write that changes one field of a record, or a migration that
/// converts one, leaves most of its rows as they were, and those are not
/// written again. A record not in `stale` had no state before, so no index
/// holds a row of it.
///
/// A row stays only when the new rows hold it column for column, and no
/// column of `text_index`, `full_text` or `sort_index` can be null, so `=`
/// compares them whole. A row of `full_text` or `sort_index` that stayed is
/// passed over by its key, not by a conflict: most of their rows are new
/// (a declaration drops a type's `sort_index` rows whole), and a plain
/// insert is the cheaper one. `edges` has no key (a record may hold one
/// link twice), so its rows go and come as before.
pub(crate) fn index_records(
    db: &mut Db,
    names: &Names,
    records: &[(&HeadType, &StoredRecord)],
    stale: &[(&str, &str)],
) -> Result<()> {
    let mut rows = IndexRows::default();
    for (head, r) in records {
        let decl = head.decl.as_ref();
        let (ty, key) = (r.ty.as_str(), r.key.as_str());
        for (field, value) in &r.fields {
            if let Some(value) = value.as_str() {
                for (column, v) in rows.text.iter_mut().zip([ty, field.as_str(), value, key]) {
                    column.push(v);
                }
            }
        }
        for (field, sort_key) in engines::order_rows(&head.sorted, r) {
            rows.sorted.0.push(ty);
            rows.sorted.1.push(field.as_str().to_owned());
            rows.sorted.2.push(key);
            rows.sorted.3.push(sort_key);
        }
        let Some(decl) = decl else { continue };
        for e in engines::links_of(decl, r, None) {
            let link = [
                ty,
                key,
                e.relation.as_str(),
                e.to.0.as_str(),
                e.to.1.as_str(),
            ];
            for (column, v) in rows.edges.iter_mut().zip(link) {
                column.push(v.to_owned());
            }
        }
        // Every head state is indexed, whatever its flags: a view at an
        // earlier position may keep a record the head has retired.
        for token in engines::record_tokens(decl, r) {
            rows.tokens[0].push(ty.to_owned());
            rows.tokens[1].push(token);
            rows.tokens[2].push(key.to_owned());
        }
    }
    let [t1, t2, t3, t4] = &rows.text;
    let [f1, f2, f3] = &rows.tokens;
    let [e1, e2, e3, e4, e5] = &rows.edges;
    let (s1, s2, s3, s4) = &rows.sorted;
    let sort = [s1 as &(dyn ToSql + Sync), s2, s3, s4];
    if !stale.is_empty() {
        // Only a type the lake keeps an order index for has rows in it; the
        // setting's rebuild drops a type's rows with its last field. So the
        // statement names `sort_index` only when a record's type has one.
        let sorted = records.iter().any(|(head, _)| !head.sorted.is_empty());
        let (tys, keys): (Vec<&str>, Vec<&str>) = stale.iter().copied().unzip();
        let mut params: Vec<&(dyn ToSql + Sync)> = vec![&tys, &keys, t1, t2, t3, t4, f1, f2, f3];
        params.extend(sort.iter().filter(|_| sorted));
        // The rows of the stale records in `table`, but for those among the
        // new rows `new`, which have `columns`.
        let gone = |table: &str, new: &str, columns: &[&str]| {
            let same: Vec<String> = columns.iter().map(|c| format!("n.{c} = t.{c}")).collect();
            format!(
                "DELETE FROM {} t USING d WHERE t.ty = d.ty AND t.key = d.key AND NOT EXISTS \
                 (SELECT 1 FROM unnest({new}) AS n({}) WHERE {})",
                names.t(table),
                columns.join(", "),
                same.join(" AND ")
            )
        };
        let z = if sorted {
            let new = "$10::text[], $11::text[], $12::text[], $13::bytea[]";
            let columns = ["ty", "field", "key", "sort_key"];
            format!(", z AS ({})", gone("sort_index", new, &columns))
        } else {
            String::new()
        };
        db.execute(
            &format!(
                "WITH d AS (SELECT * FROM unnest($1::text[], $2::text[]) AS d(ty, key)), \
                 x AS ({x}), y AS ({y}){z} \
                 DELETE FROM {edges} t USING d WHERE t.src_ty = d.ty AND t.src_key = d.key",
                x = gone(
                    "text_index",
                    "$3::text[], $4::text[], $5::text[], $6::text[]",
                    &["ty", "field", "value", "key"]
                ),
                y = gone(
                    "full_text",
                    "$7::text[], $8::text[], $9::text[]",
                    &["ty", "token", "key"]
                ),
                edges = names.t("edges"),
            ),
            &params,
        )?;
    }
    if t1.is_empty() && f1.is_empty() && e1.is_empty() && s1.is_empty() {
        return Ok(());
    }
    let mut params: Vec<&(dyn ToSql + Sync)> = vec![t1, t2, t3, t4, f1, f2, f3, e1, e2, e3, e4, e5];
    let z = if s1.is_empty() {
        String::new()
    } else {
        params.extend(sort);
        format!(
            ", z AS (INSERT INTO {sorted} (ty, field, key, sort_key) \
               SELECT * FROM unnest($13::text[], $14::text[], $15::text[], $16::bytea[]) \
               AS n(ty, field, key, sort_key) WHERE NOT EXISTS (SELECT 1 FROM {sorted} t \
               WHERE t.ty = n.ty AND t.key = n.key AND t.field = n.field))",
            sorted = names.t("sort_index")
        )
    };
    db.execute(
        &format!(
            "WITH x AS (INSERT INTO {text} (ty, field, value, key) \
               SELECT * FROM unnest($1::text[], $2::text[], $3::text[], $4::text[]) \
               ON CONFLICT DO NOTHING), \
             y AS (INSERT INTO {full} (ty, token, key) \
               SELECT * FROM unnest($5::text[], $6::text[], $7::text[]) AS n(ty, token, key) \
               WHERE NOT EXISTS (SELECT 1 FROM {full} t \
               WHERE t.ty = n.ty AND t.token = n.token AND t.key = n.key)){z} \
             INSERT INTO {edges} (src_ty, src_key, relation, dst_ty, dst_key) \
             SELECT * FROM unnest($8::text[], $9::text[], $10::text[], $11::text[], $12::text[])",
            text = names.t("text_index"),
            full = names.t("full_text"),
            edges = names.t("edges"),
        ),
        &params,
    )
}

/// How many puts the buffer holds before it is written, and how many
/// records a pass over a type reads at a time.
pub(crate) const PAGE: usize = 1_024;

/// The puts of the fold's open transaction, held until they are a page
/// or it commits, then written a table at a time ([`Buffer::write`]). Each thing's
/// states are keyed by its identifying columns and position, so a put at
/// a position already put in the transaction replaces it, as the one-row
/// put replaces a state at the same position.
///
/// Nothing reads the buffer before the commit: no view sees an open
/// transaction, and the store reads a state it put in the same fold from
/// what it staged. The questions the engine asks of a put (the state it
/// supersedes, where it ends, whether it is the head, whether the thing
/// had a state before) are answered when it is written, over the buffer
/// and the table together.
#[derive(Default)]
pub(crate) struct Buffer {
    records: BTreeMap<(Vec<String>, i64), StoredRecord>,
    proposals: BTreeMap<(Vec<String>, i64), String>,
    outputs: BTreeMap<(Vec<String>, i64), String>,
    members: BTreeMap<(Vec<String>, i64), bool>,
    /// Applied ids, each with the first position it was marked at.
    applied: BTreeMap<String, i64>,
}

impl Buffer {
    /// The puts it holds, of every kind.
    pub(crate) fn len(&self) -> usize {
        let (records, states) = (
            self.records.len(),
            self.proposals.len() + self.outputs.len(),
        );
        records + states + self.members.len() + self.applied.len()
    }

    pub(crate) fn record(&mut self, r: &StoredRecord) {
        let ident = vec![r.ty.as_str().to_owned(), r.key.as_str().to_owned()];
        self.records.insert((ident, r.position.0 as i64), r.clone());
    }

    pub(crate) fn proposal(&mut self, id: &str, at: Position, state: String) {
        self.proposals
            .insert((vec![id.to_owned()], at.0 as i64), state);
    }

    pub(crate) fn output(&mut self, seam: &str, input: &str, at: Position, state: String) {
        let ident = vec![seam.to_owned(), input.to_owned()];
        self.outputs.insert((ident, at.0 as i64), state);
    }

    pub(crate) fn member(&mut self, set: &str, ty: &str, key: &str, at: Position, member: bool) {
        let ident = vec![set.to_owned(), ty.to_owned(), key.to_owned()];
        self.members.insert((ident, at.0 as i64), member);
    }

    pub(crate) fn applied(&mut self, id: &str, at: Position) {
        self.applied.entry(id.to_owned()).or_insert(at.0 as i64);
    }

    /// Write every buffered put: one statement per history table, the
    /// head indexes of the records whose latest state is now a buffered
    /// one, under their types' declarations at the head as `types` reads
    /// them after every declaration the transaction put, and the applied
    /// ids. That leaves what putting them one by one leaves. The types
    /// whose `vector_index.<type>` setting it wrote go into `asked`. It
    /// answers the order index rows the settings it wrote rebuilt.
    pub(crate) fn write(
        self,
        db: &mut Db,
        names: &Names,
        types: &mut HeadTypes,
        asked: &mut BTreeMap<TypeName, Position>,
    ) -> Result<u64> {
        let records: BTreeMap<(Vec<String>, i64), String> = self
            .records
            .iter()
            .map(|(k, r)| {
                let state = serde_json::to_string(r).expect("a record serialises");
                (k.clone(), state)
            })
            .collect();
        let heads = RECORDS.put_many(db, names, &records)?;
        PROPOSALS.put_many(db, names, &self.proposals)?;
        OUTPUTS.put_many(db, names, &self.outputs)?;
        MEMBERS.put_many(db, names, &self.members)?;
        let mut latest = Vec::with_capacity(heads.len());
        let mut settings = Vec::new();
        for (ident, had) in &heads {
            let r = self
                .records
                .range((ident.clone(), i64::MIN)..=(ident.clone(), i64::MAX))
                .next_back()
                .map(|(_, r)| r)
                .expect("a head the buffer put");
            // A `sort_index.<type>` setting whose head state the buffer holds
            // changes which fields of its type are indexed: read again.
            if let Some(ty) = engines::sort_index_type(r) {
                types.0.remove(&ty);
                settings.push((ty, r.position));
            }
            // A `vector_index.<type>` setting whose head state the buffer
            // holds has its type's vector indexes built again at the commit:
            // a build holds `vectors` against every other session until then.
            if let Some(ty) = engines::vector_index_type(r) {
                asked.insert(ty, r.position);
            }
            latest.push((r, *had));
        }
        let mut indexed = Vec::with_capacity(latest.len());
        let mut stale = Vec::new();
        for (r, had) in latest {
            indexed.push((types.get(db, names, &r.ty)?, r));
            if had {
                stale.push((r.ty.as_str(), r.key.as_str()));
            }
        }
        let indexed: Vec<(&HeadType, &StoredRecord)> =
            indexed.iter().map(|(h, r)| (h, *r)).collect();
        index_records(db, names, &indexed, &stale)?;
        // The setting's write rebuilds the index it asks for, so the index
        // describes the head from the next read on.
        let mut rebuilt = 0;
        for (ty, at) in settings {
            rebuilt += rebuild_sorted(db, names, &ty, at)?;
        }
        if !self.applied.is_empty() {
            let (ids, at): (Vec<&str>, Vec<i64>) = self
                .applied
                .iter()
                .map(|(id, at)| (id.as_str(), *at))
                .unzip();
            db.execute(
                &format!(
                    "INSERT INTO {} (id, at) SELECT * FROM unnest($1::text[], $2::int8[]) \
                     ON CONFLICT DO NOTHING",
                    names.t("applied")
                ),
                &[&ids, &at],
            )?;
        }
        Ok(rebuilt)
    }
}

/// Put a declaration. `types` forgets every head declaration it held,
/// since this one may change one. For a type now in force at the head its
/// records' head indexes are to be rewritten under it, and it answers the
/// type: the commit does that with [`index_older`], for the records the
/// transaction has not put again by then. Records the transaction buffered
/// are indexed as they are written, under the declaration in force then.
pub(crate) fn put_declaration(
    db: &mut Db,
    names: &Names,
    types: &mut HeadTypes,
    d: &Declared,
) -> Result<Option<TypeName>> {
    types.0.clear();
    let ident = engines::declaration_key(&d.declaration);
    let state = serde_json::to_string(d).expect("a declaration serialises");
    DECLARATIONS.put(db, names, &[&ident], d.since, &state)?;
    let Declaration::Type(t) = &d.declaration else {
        return Ok(None);
    };
    let head = types.get(db, names, &t.name)?;
    if head.decl.as_ref() != Some(t) {
        return Ok(None);
    }
    // The rows go whole: the declaration may have dropped a field the index
    // kept. The mark follows the fields it leaves the index, which the
    // records written after it and the commit fill: a setting written before
    // the declaration that first declares its fields found none to mark.
    db.execute(
        &format!("DELETE FROM {} WHERE ty = $1", names.t("sort_index")),
        &[&t.name.as_str()],
    )?;
    let kept = (!head.sorted.is_empty()).then_some(d.since);
    set_sorted(db, names, &t.name, kept)?;
    Ok(Some(t.name.clone()))
}

/// Rewrite the head indexes of the records of `ty` whose latest state is
/// older than `at`, under the type's declaration at the head. A record put
/// at `at` or after is indexed under that declaration when its page is
/// written: a migration puts every record again, and nothing is left to
/// do here.
pub(crate) fn index_older(
    db: &mut Db,
    names: &Names,
    types: &mut HeadTypes,
    ty: &TypeName,
    at: Position,
) -> Result<()> {
    let head = types.get(db, names, ty)?;
    let page = format!(
        "SELECT state FROM {} WHERE ty = $1 AND until IS NULL AND since < $2 \
         AND key COLLATE \"C\" > $3 ORDER BY key COLLATE \"C\" LIMIT {PAGE}",
        names.t("records")
    );
    // A page of the type at a time; an ended key has no index rows.
    let mut after = String::new();
    loop {
        let records = db.records(&page, &[&ty.as_str(), &(at.0 as i64), &after])?;
        let live = || records.iter().filter(|r| !r.is_ended());
        let indexed: Vec<(&HeadType, &StoredRecord)> = live().map(|r| (&head, r)).collect();
        let stale: Vec<(&str, &str)> = live().map(|r| (r.ty.as_str(), r.key.as_str())).collect();
        index_records(db, names, &indexed, &stale)?;
        match records.last() {
            Some(last) if records.len() == PAGE => after = last.key.to_string(),
            _ => return Ok(()),
        }
    }
}
