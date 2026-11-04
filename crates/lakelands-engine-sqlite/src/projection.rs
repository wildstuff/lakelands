//! The projection in SQLite, with its kept history.
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
//! of `via` relations, and the full-text entries of searchable fields.
//! They are rewritten in the fold's own transaction, from the
//! record and the declaration in force, so they always describe the head.
//! A view uses them only when it reads the head, and only to find
//! candidates: whether a candidate matches, where it sorts and what it
//! scores is decided by `lakelands_core::engines`, the rules every engine
//! shares, so no index can change an answer.
//!
//! **Vectors.** A vector's elements are kept once, in a row of `vectors`
//! written by the state that changed them, which every later state naming
//! it shares; a row ends where its record's state stops naming it, or is
//! retired, so the rows in force (`until` none) are the head's, which a
//! ranking reads (`docs/decisions/072-a-vector-is-stored-once.md`).
//!
//! **Transactions.** The fold writes between `begin` and `commit`, in one
//! SQLite transaction on the writer's connection; `commit` records the
//! position folded through in the same transaction. A crash or a rollback
//! leaves the projection at a whole append.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::engine::{Choose, Chosen, Declared, Look, SeriesState, TextCounts};
use lakelands_api::error::Result;
use lakelands_api::id::{FieldName, Position, RecordKey, TypeName};
use lakelands_api::ontology::{StorageKind, TypeDecl};
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::{FeedFilter, SETTING_TYPE, SORT_INDEX_SETTING, is_kept_name};
use lakelands_api::write::Declaration;
use lakelands_core::engines::{self, sorted_mark};
use rusqlite::types::{FromSql, Value};
use rusqlite::{Connection, OptionalExtension, Params, ToSql, params, params_from_iter};

use crate::sql;

/// The schema. `state` columns hold the state as JSON; a record's leaves
/// out what its row holds ([`engines::record_state`]). A table whose
/// primary key is the whole row keeps no rowid beside it (`WITHOUT ROWID`),
/// which would hold every row twice. `search_rows` holds a row only for a
/// record with full-text tokens. `vectors` finds a record's rows by its
/// key and a field's rows in force by `vectors_head`, which a ranking
/// scans; `since` -1 marks a vector put by the index alone ([`put_vector`]).
/// `bases` keeps a base [`engines::SHARED`] records of one write share
/// once, which their states name by id.
///
/// A record's type and key are held once, in `keys`, which gives each an
/// integer id in the order the fold first meets it ([`key_id`]); `records`
/// and `routes` hold that id. A key that comes in no order (a tick's
/// `symbol:at:seq`) then lands at the right edge of every b-tree that holds
/// it, where its text would land anywhere. `sort_index` and `text_index`
/// keep the text, since their readers break a tie by the key ascending.
/// `records.flags` holds the two flags of the state beside it
/// ([`flags_of`]), so a count tests a column and parses no JSON. An
/// applied id is its 16 bytes. The order index is kept ascending only: a
/// window down walks it backwards and sorts each run of one sort key by
/// the key. A text row goes by its primary key, from the state that wrote
/// it, so no index of `text_index` by key is kept.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (
    k TEXT PRIMARY KEY,
    v INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS declarations (
    ident TEXT NOT NULL, since INTEGER NOT NULL, until INTEGER,
    state TEXT NOT NULL,
    PRIMARY KEY (ident, since)
);
CREATE TABLE IF NOT EXISTS keys (
    ty TEXT NOT NULL, key TEXT NOT NULL, id INTEGER NOT NULL,
    PRIMARY KEY (ty, key)
) WITHOUT ROWID;
CREATE UNIQUE INDEX IF NOT EXISTS keys_id ON keys (id);
CREATE TABLE IF NOT EXISTS records (
    ty TEXT NOT NULL, id INTEGER NOT NULL, since INTEGER NOT NULL, until INTEGER,
    flags INTEGER NOT NULL, state TEXT NOT NULL,
    PRIMARY KEY (id, since)
);
CREATE TABLE IF NOT EXISTS view_order (
    name TEXT NOT NULL, key TEXT NOT NULL, entry BLOB NOT NULL,
    PRIMARY KEY (name, key)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS records_at ON records (ty, until, id);
CREATE INDEX IF NOT EXISTS declarations_until ON declarations (until);
CREATE INDEX IF NOT EXISTS records_since ON records (ty, since);
CREATE TABLE IF NOT EXISTS proposals (
    id TEXT NOT NULL, since INTEGER NOT NULL, until INTEGER,
    state TEXT NOT NULL,
    PRIMARY KEY (id, since)
);
CREATE INDEX IF NOT EXISTS proposals_newest ON proposals (json_extract(state, '$.position'));
CREATE TABLE IF NOT EXISTS quarantined (
    source TEXT NOT NULL, delivery TEXT NOT NULL, ix INTEGER NOT NULL,
    since INTEGER NOT NULL, until INTEGER,
    state TEXT NOT NULL,
    PRIMARY KEY (source, delivery, ix, since)
);
CREATE INDEX IF NOT EXISTS quarantined_at ON quarantined
    (json_extract(state, '$.type'), json_extract(state, '$.key'), until);
CREATE TABLE IF NOT EXISTS members (
    object_set TEXT NOT NULL, ty TEXT NOT NULL, key TEXT NOT NULL,
    since INTEGER NOT NULL, until INTEGER,
    state INTEGER NOT NULL,
    PRIMARY KEY (object_set, ty, key, since)
);
CREATE TABLE IF NOT EXISTS outputs (
    seam TEXT NOT NULL, input TEXT NOT NULL, since INTEGER NOT NULL, until INTEGER,
    state TEXT NOT NULL,
    PRIMARY KEY (seam, input, since)
);
CREATE TABLE IF NOT EXISTS applied (
    id BLOB PRIMARY KEY,
    at INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS routes (
    at INTEGER NOT NULL, kind INTEGER NOT NULL, ty INTEGER, key INTEGER, source INTEGER
);
CREATE TABLE IF NOT EXISTS names (
    id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE
);
CREATE TABLE IF NOT EXISTS bases (
    id INTEGER PRIMARY KEY, block TEXT NOT NULL UNIQUE
);
CREATE INDEX IF NOT EXISTS routes_kind ON routes (kind, at);
CREATE INDEX IF NOT EXISTS routes_ty ON routes (ty, at);
CREATE INDEX IF NOT EXISTS routes_key ON routes (key, at);
CREATE INDEX IF NOT EXISTS routes_source ON routes (source, at);
CREATE TABLE IF NOT EXISTS text_index (
    ty TEXT NOT NULL, field TEXT NOT NULL, value TEXT NOT NULL, key TEXT NOT NULL,
    PRIMARY KEY (ty, field, value, key)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS lookup_fields (
    ty TEXT NOT NULL, field TEXT NOT NULL, PRIMARY KEY (ty, field)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS edges (
    src_ty TEXT NOT NULL, src_key TEXT NOT NULL, relation TEXT NOT NULL,
    dst_ty TEXT NOT NULL, dst_key TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS edges_src ON edges (src_ty, src_key);
CREATE INDEX IF NOT EXISTS edges_dst ON edges (dst_ty, dst_key);
CREATE TABLE IF NOT EXISTS sort_index (
    ty TEXT NOT NULL, field TEXT NOT NULL, key TEXT NOT NULL, sort_key BLOB NOT NULL,
    PRIMARY KEY (ty, key, field)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS sort_index_asc ON sort_index (ty, field, sort_key, key);
CREATE TABLE IF NOT EXISTS search_rows (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ty TEXT NOT NULL, key TEXT NOT NULL,
    UNIQUE (ty, key)
);
CREATE VIRTUAL TABLE IF NOT EXISTS full_text USING fts5(
    tokens, tokenize = 'ascii'
);
CREATE TABLE IF NOT EXISTS vectors (
    ty TEXT NOT NULL, key TEXT NOT NULL, field TEXT NOT NULL,
    since INTEGER NOT NULL, until INTEGER, single INTEGER NOT NULL, data BLOB NOT NULL,
    PRIMARY KEY (ty, key, field, since)
);
CREATE INDEX IF NOT EXISTS vectors_head ON vectors (ty, field, key) WHERE until IS NULL;
CREATE TABLE IF NOT EXISTS text_stats (
    ty TEXT NOT NULL, field TEXT NOT NULL, held INTEGER NOT NULL,
    records INTEGER NOT NULL, tokens INTEGER NOT NULL,
    PRIMARY KEY (ty, field, held)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS text_lengths (
    ty TEXT NOT NULL, key TEXT NOT NULL, field TEXT NOT NULL,
    held INTEGER NOT NULL, tokens INTEGER NOT NULL,
    PRIMARY KEY (ty, key, field)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS thinned_types (
    ty TEXT NOT NULL, \"by\" TEXT NOT NULL, \"before\" INTEGER NOT NULL,
    at INTEGER NOT NULL, through INTEGER NOT NULL, gone INTEGER NOT NULL,
    PRIMARY KEY (ty, \"by\", \"before\")
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS thinned (
    ty TEXT NOT NULL, key TEXT NOT NULL, \"before\" INTEGER NOT NULL, at INTEGER NOT NULL,
    PRIMARY KEY (ty, key)
) WITHOUT ROWID;
";

/// Every table the projection holds, cleared by a rebuild.
pub(crate) const TABLES: [&str; 24] = [
    "meta",
    "declarations",
    "keys",
    "records",
    "view_order",
    "proposals",
    "quarantined",
    "members",
    "outputs",
    "applied",
    "routes",
    "names",
    "bases",
    "text_index",
    "lookup_fields",
    "edges",
    "sort_index",
    "search_rows",
    "full_text",
    "vectors",
    "text_stats",
    "text_lengths",
    "thinned_types",
    "thinned",
];

/// A history table: its name and the columns naming one thing in it.
pub(crate) struct History {
    pub(crate) table: &'static str,
    pub(crate) ident: &'static [&'static str],
}

pub(crate) const DECLARATIONS: History = History::new("declarations", &["ident"]);
pub(crate) const RECORDS: History = History::new("records", &["id"]);
pub(crate) const PROPOSALS: History = History::new("proposals", &["id"]);
pub(crate) const QUARANTINED: History = History::new("quarantined", &["source", "delivery", "ix"]);
pub(crate) const MEMBERS: History = History::new("members", &["object_set", "ty", "key"]);
pub(crate) const OUTPUTS: History = History::new("outputs", &["seam", "input"]);

impl History {
    const fn new(table: &'static str, ident: &'static [&'static str]) -> History {
        History { table, ident }
    }

    fn ident_where(&self, from: usize) -> String {
        self.ident
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{c} = ?{}", i + from))
            .collect::<Vec<_>>()
            .join(" AND ")
    }

    /// The statement that ends the state in force before a put: only the
    /// latest earlier state can be the one, so it is sought and the
    /// thing's other states are not read (lakelands#596). Its parameters
    /// are the thing's columns and then the put's position.
    pub(crate) fn ends_latest(&self) -> String {
        format!(
            "UPDATE {t} SET until = ?{n1} WHERE {w} AND since = \
             (SELECT max(since) FROM {t} WHERE {w} AND since < ?{n1}) \
             AND (until IS NULL OR until > ?{n1})",
            t = self.table,
            w = self.ident_where(1),
            n1 = self.ident.len() + 1
        )
    }

    /// Put a state of one thing valid from `since`, keeping every other
    /// state: the one it supersedes ends at `since`, and it ends where the
    /// next one, if any, begins. Answers whether it held a state before.
    pub(crate) fn put(
        &self,
        conn: &Connection,
        ident: &[&dyn ToSql],
        since: Position,
        state: &dyn ToSql,
    ) -> Result<bool> {
        self.put_row(conn, ident, since, &[("state", state)])
    }

    /// [`History::put`] of a row whose state is the `columns` given, each
    /// written again where the state at `since` is replaced.
    pub(crate) fn put_row(
        &self,
        conn: &Connection,
        ident: &[&dyn ToSql],
        since: Position,
        columns: &[(&str, &dyn ToSql)],
    ) -> Result<bool> {
        let n = self.ident.len();
        let at = since.0 as i64;
        let mut args: Vec<&dyn ToSql> = ident.to_vec();
        args.push(&at);
        let values = columns.iter().map(|(_, v)| *v);
        let set: Vec<String> = (columns.iter().enumerate())
            .map(|(i, (c, _))| format!("{c} = ?{}", n + 2 + i))
            .collect();
        let replaced = conn
            .prepare_cached(&format!(
                "UPDATE {} SET {} WHERE {} AND since = ?{}",
                self.table,
                set.join(", "),
                self.ident_where(1),
                n + 1
            ))
            .and_then(|mut s| {
                let a: Vec<&dyn ToSql> = args.iter().copied().chain(values.clone()).collect();
                s.execute(a.as_slice())
            })
            .map_err(sql)?;
        if replaced > 0 {
            return Ok(true);
        }
        let next: Option<i64> = conn
            .prepare_cached(&format!(
                "SELECT min(since) FROM {} WHERE {} AND since > ?{}",
                self.table,
                self.ident_where(1),
                n + 1
            ))
            .and_then(|mut s| s.query_row(args.as_slice(), |r| r.get(0)))
            .map_err(sql)?;
        let ended = conn
            .prepare_cached(&self.ends_latest())
            .and_then(|mut s| s.execute(args.as_slice()))
            .map_err(sql)?;
        let named: Vec<&str> = columns.iter().map(|(c, _)| *c).collect();
        let marks: Vec<String> = (1..=n + 2 + columns.len())
            .map(|i| format!("?{i}"))
            .collect();
        conn.prepare_cached(&format!(
            "INSERT INTO {} ({}, since, until, {}) VALUES ({})",
            self.table,
            self.ident.join(", "),
            named.join(", "),
            marks.join(", ")
        ))
        .and_then(|mut s| {
            let a: Vec<&dyn ToSql> = (args.iter().copied())
                .chain([&next as &dyn ToSql])
                .chain(values)
                .collect();
            s.execute(a.as_slice())
        })
        .map_err(sql)?;
        Ok(ended > 0 || next.is_some())
    }
}

/// The condition that a history row holds at the position bound as `?n`.
pub(crate) fn holds_at(n: usize) -> String {
    format!("since <= ?{n} AND (until IS NULL OR until > ?{n})")
}

/// Run one cached statement.
pub(crate) fn run(conn: &Connection, statement: &str, args: impl Params) -> Result<()> {
    conn.prepare_cached(statement)
        .and_then(|mut s| s.execute(args))
        .map(|_| ())
        .map_err(sql)
}

/// The first column of the row a cached statement answers, if any.
pub(crate) fn one<T: FromSql>(
    conn: &Connection,
    statement: &str,
    args: impl Params,
) -> Result<Option<T>> {
    conn.prepare_cached(statement)
        .and_then(|mut s| s.query_row(args, |r| r.get(0)).optional())
        .map_err(sql)
}

/// The first column of every row a cached statement answers, each passed
/// through `f` as it is read.
pub(crate) fn column<T: FromSql, R, C: FromIterator<R>>(
    conn: &Connection,
    statement: &str,
    args: impl Params,
    mut f: impl FnMut(T) -> Result<R>,
) -> Result<C> {
    let mut stmt = conn.prepare_cached(statement).map_err(sql)?;
    let rows = stmt.query_map(args, |r| r.get::<_, T>(0)).map_err(sql)?;
    rows.map(|r| f(r.map_err(sql)?)).collect()
}

/// What a route's `column` may hold for the name `value`: the kind's
/// code, the id `names` gives a type or a source, each id `keys` gives the
/// key. Nothing where no route can hold it: a kind the format does not
/// list, or a name no event has brought.
fn stored(conn: &Connection, name: &str, value: &str) -> Result<Vec<Value>> {
    Ok(match name {
        "kind" => engines::kind_code(value)
            .map(Value::Integer)
            .into_iter()
            .collect(),
        "key" => column(conn, KEY_IDS, [value], |id: i64| Ok(Value::Integer(id)))?,
        _ => one::<i64>(conn, "SELECT id FROM names WHERE name = ?1", [value])?
            .map(Value::Integer)
            .into_iter()
            .collect(),
    })
}

/// The ids `keys` gives the key `?1` under every type, and under none: a
/// step from type to type through its primary key, so that a key is
/// sought once a type and the keys are never walked.
const KEY_IDS: &str = "WITH RECURSIVE t(ty) AS (SELECT min(ty) FROM keys \
     UNION ALL SELECT (SELECT min(ty) FROM keys WHERE ty > t.ty) FROM t WHERE t.ty IS NOT NULL) \
     SELECT k.id FROM t CROSS JOIN keys k ON k.ty = t.ty AND k.key = ?1";

/// The id `keys` gives `ty`'s `key`, if it has one. A route about a key of
/// no type holds the id of the key under the empty name, which no type has.
pub(crate) fn key_id(conn: &Connection, ty: &str, key: &str) -> Result<Option<i64>> {
    one(
        conn,
        "SELECT id FROM keys WHERE ty = ?1 AND key = ?2",
        [ty, key],
    )
}

/// The id `keys` gives `ty`'s `key`, and whether it was made now, for a
/// key the projection had not met: the next after the greatest.
pub(crate) fn make_key_id(conn: &Connection, ty: &str, key: &str) -> Result<(i64, bool)> {
    if let Some(id) = key_id(conn, ty, key)? {
        return Ok((id, false));
    }
    let made = "INSERT INTO keys (ty, key, id) \
                VALUES (?1, ?2, (SELECT coalesce(max(id), 0) + 1 FROM keys)) RETURNING id";
    let id = (conn.prepare_cached(made))
        .and_then(|mut s| s.query_row([ty, key], |r| r.get(0)))
        .map_err(sql)?;
    Ok((id, true))
}

/// The flags a record's row holds beside its state: 1 retired, 2
/// restricted. The fold writes them from the state it writes, and a count
/// reads them in place of the state.
pub(crate) fn flags_of(r: &StoredRecord) -> i64 {
    i64::from(r.retired) | i64::from(r.restricted) << 1
}

/// The head records of the type `?1`, each as [`record_at`] reads it:
/// `keys` walked in its primary key, so in key order, and each key's head
/// state sought through `records_at`. A statement continues it with tests
/// of `k.key` and of `r`.
macro_rules! heads {
    () => {
        "SELECT r.ty, k.key, r.since, r.state FROM keys k CROSS JOIN records r \
         ON r.ty = k.ty AND r.until IS NULL AND r.id = k.id WHERE k.ty = ?1"
    };
}
pub(crate) use heads;

/// The id `names` gives `name`, made if it has none.
pub(crate) fn name_id(conn: &Connection, name: &str) -> Result<i64> {
    let insert = "INSERT INTO names (name) VALUES (?1) ON CONFLICT (name) DO NOTHING";
    conn.prepare_cached(insert)
        .and_then(|mut s| s.execute([name]))
        .map_err(sql)?;
    let id = one(conn, "SELECT id FROM names WHERE name = ?1", [name])?;
    Ok(id.expect("a name just inserted is held"))
}

/// The statement of a read of the feed index with a test per `(column,
/// names)`. Where `lead` is `key`, the other columns are tested on their
/// values, not their index (`+`): `ty` holds few values over many rows, and
/// a planner that takes `routes_ty` reads the whole type to find a key's
/// rows, where `routes_key` reads the key's alone.
fn routes_sql(tests: &[(&str, usize)], lead: Option<&str>) -> String {
    let tests: Vec<String> = tests
        .iter()
        .map(|(column, n)| {
            let plus = if lead == Some("key") && *column != "key" {
                "+"
            } else {
                ""
            };
            format!("AND {plus}{column} IN ({})", vec!["?"; *n].join(", "))
        })
        .collect();
    format!(
        "SELECT DISTINCT at FROM routes WHERE 1 {} AND at BETWEEN ? AND ? ORDER BY at LIMIT ?",
        tests.join(" ")
    )
}

/// The positions from `from` through `through` of the feed index's rows
/// whose `column` is `value`, in order and at most `limit`, each row
/// checked against the filter's other lists (`lakelands_core::feed::Seek`).
/// Each call looks its names up; a list none of whose names a route can
/// hold matches nothing.
pub(crate) fn routes(
    conn: &Connection,
    filter: &FeedFilter,
    by: Option<(&str, &str)>,
    range: [Position; 2],
    limit: u32,
) -> Result<Vec<Position>> {
    let Some((statement, args)) = routes_query(conn, filter, by, range, limit)? else {
        return Ok(Vec::new());
    };
    column(conn, &statement, params_from_iter(args), |at: i64| {
        Ok(Position(at as u64))
    })
}

/// The plan SQLite makes for the statement [`routes`] runs for these
/// arguments, one line a step; none where the names match nothing.
pub(crate) fn routes_plan(
    conn: &Connection,
    filter: &FeedFilter,
    by: Option<(&str, &str)>,
    range: [Position; 2],
    limit: u32,
) -> Result<Option<Vec<String>>> {
    let Some((statement, args)) = routes_query(conn, filter, by, range, limit)? else {
        return Ok(None);
    };
    let mut plan = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {statement}"))
        .map_err(sql)?;
    let rows = plan
        .query_map(params_from_iter(args), |r| r.get::<_, String>(3))
        .map_err(sql)?;
    rows.collect::<std::result::Result<_, _>>()
        .map(Some)
        .map_err(sql)
}

/// The statement and arguments of [`routes`]: none where a list holds no
/// name a route can hold.
fn routes_query(
    conn: &Connection,
    filter: &FeedFilter,
    by: Option<(&str, &str)>,
    [from, through]: [Position; 2],
    limit: u32,
) -> Result<Option<(String, Vec<Value>)>> {
    let lists: [(&str, Vec<&str>); 3] = [
        ("kind", filter.kinds.iter().map(String::as_str).collect()),
        ("ty", filter.types.iter().map(TypeName::as_str).collect()),
        ("key", filter.keys.iter().map(RecordKey::as_str).collect()),
    ];
    let lead = by.map(|(c, _)| c);
    let mut args: Vec<Value> = Vec::new();
    let mut tests = Vec::new();
    for (column, values) in by.iter().map(|(c, v)| (*c, vec![*v])).chain(
        (lists.into_iter()).filter(|(c, v)| !v.is_empty() && by.is_none_or(|(b, _)| b != *c)),
    ) {
        let mut held = Vec::new();
        for v in values {
            held.extend(stored(conn, column, v)?);
        }
        held.dedup();
        if held.is_empty() {
            return Ok(None);
        }
        tests.push((column, held.len()));
        args.extend(held);
    }
    let (from, through, limit) = (from.0 as i64, through.0 as i64, i64::from(limit.max(1)));
    args.extend([from, through, limit].map(Value::Integer));
    Ok(Some((routes_sql(&tests, lead), args)))
}

/// The layout of the projection this build reads and writes. It is
/// raised whenever a table changes so that a projection made before
/// would answer wrongly, not fail: one made with another layout is never
/// read, and is rebuilt from the log.
pub(crate) const LAYOUT: u64 = 11;

/// The layout `conn`'s projection was made with: none where it holds no
/// `meta`, or was made before projections were marked.
pub(crate) fn layout(conn: &Connection) -> Result<Option<u64>> {
    let marked: Option<i64> = one(
        conn,
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta'",
        [],
    )?;
    if marked.is_none() {
        return Ok(None);
    }
    let found: Option<i64> = one(conn, "SELECT v FROM meta WHERE k = 'layout'", [])?;
    Ok(found.map(|v| v as u64))
}

/// Whether `conn`'s database holds a table. One that holds none is a
/// projection its maker has connected to and not made yet.
pub(crate) fn made(conn: &Connection) -> Result<bool> {
    one(conn, "SELECT 1 FROM sqlite_master LIMIT 1", []).map(|t: Option<i64>| t.is_some())
}

/// The statement that marks a projection with this build's layout, as
/// it is made or cleared. The full-text table removes a deleted row's
/// terms from its index as it deletes them, where it would otherwise keep
/// them, marked deleted, until a merge: `secure_delete` overwrites the
/// pages a delete frees, and a term the index still holds is in no freed
/// page (`ERASED_VALUE_LEAVES_THE_FILE`). The table keeps the setting, so
/// it is written once: an open that changes nothing writes nothing.
pub(crate) fn mark() -> String {
    format!(
        "INSERT INTO meta (k, v) VALUES ('layout', {LAYOUT}) ON CONFLICT (k) DO NOTHING; \
         INSERT INTO meta (k, v) VALUES ('{TEXT_STATS}', -1) ON CONFLICT (k) DO NOTHING; \
         INSERT INTO full_text (full_text, rank) SELECT 'secure-delete', 1 \
         WHERE NOT EXISTS (SELECT 1 FROM full_text_config WHERE k = 'secure-delete');"
    )
}

/// The position the projection reflects, if any.
pub(crate) fn folded(conn: &Connection) -> Result<Option<Position>> {
    one(conn, "SELECT v FROM meta WHERE k = 'folded'", [])
        .map(|v: Option<i64>| v.map(|v| Position(v as u64)))
}

/// The position `ty`'s order index was last kept through, if the lake
/// keeps one. It describes the head only when it is the position folded
/// through: a writer that does not keep the index leaves it behind.
pub(crate) fn sorted(conn: &Connection, ty: &TypeName) -> Result<Option<Position>> {
    one(conn, "SELECT v FROM meta WHERE k = ?1", [sorted_mark(ty)])
        .map(|v: Option<i64>| v.map(|v| Position(v as u64)))
}

/// The fields of `ty` whose order index the lake keeps: those its
/// `sort_index.<type>` setting at the head names that `decl` (a kept
/// name's view at the head) declares with a kind the index keeps.
pub(crate) fn sorted_fields(
    conn: &Connection,
    ty: &TypeName,
    decl: Option<&TypeDecl>,
    building: bool,
) -> Result<Vec<FieldName>> {
    let view = match engines::kept_view(ty) {
        Some(view) => type_at(conn, &view, None)?,
        None => None,
    };
    let Some(decl) = view.as_ref().or(decl) else {
        return Ok(Vec::new());
    };
    let setting = head_record(
        conn,
        &TypeName::from(SETTING_TYPE),
        &RecordKey::from(engines::sort_index_key(ty)),
    )?;
    let chosen = match building {
        true => chosen_of(conn, ty)?,
        false => chosen_kept(conn, ty)?,
    };
    Ok(engines::order_fields(
        ty,
        decl,
        setting.as_ref().and_then(engines::setting_value),
        &chosen,
        building,
    ))
}

/// The fields of `ty`'s order index the lake chose, as the meta row holds
/// them: none where it holds none.
pub(crate) fn chosen_of(conn: &Connection, ty: &TypeName) -> Result<Chosen> {
    let text: Option<String> = one(
        conn,
        "SELECT v FROM meta WHERE k = ?1",
        [engines::chosen_mark(ty)],
    )?;
    Ok(text.map_or_else(Chosen::default, |t| {
        serde_json::from_str(&t).unwrap_or_default()
    }))
}

/// Whether the lake's chosen fields are kept through the position folded
/// through: no writer that keeps none folded past them. A lake that never
/// chose holds no mark and is.
fn chosen_held(conn: &Connection) -> Result<bool> {
    one(
        conn,
        "SELECT NOT EXISTS (SELECT 1 FROM meta WHERE k = ?1 \
         AND v IS NOT (SELECT v FROM meta WHERE k = 'folded'))",
        [engines::CHOSEN_THROUGH],
    )
    .map(|held: Option<bool>| held.unwrap_or(true))
}

/// The fields of `ty`'s order index the lake chose, as a read takes them:
/// none unless they are [held](chosen_held).
pub(crate) fn chosen_kept(conn: &Connection, ty: &TypeName) -> Result<Chosen> {
    match chosen_held(conn)? {
        true => chosen_of(conn, ty),
        false => Ok(Chosen::default()),
    }
}

/// Keep `ty`'s chosen fields as `chosen`: no row where it holds none.
/// The lake's first choice marks them kept through the position folded
/// through, which the fold moves on from there.
fn set_chosen(conn: &Connection, ty: &TypeName, chosen: &Chosen) -> Result<()> {
    if *chosen == Chosen::default() {
        return run(
            conn,
            "DELETE FROM meta WHERE k = ?1",
            [engines::chosen_mark(ty)],
        );
    }
    let text = serde_json::to_string(chosen).expect("the chosen fields serialise");
    // A mark written before the first fold of a rebuild commits has no
    // position to be held through: none is written, and a read takes it as
    // held until the commit marks it.
    run(
        conn,
        "INSERT INTO meta (k, v) SELECT ?1, v FROM meta WHERE k = 'folded' \
         ON CONFLICT (k) DO NOTHING",
        [engines::CHOSEN_THROUGH],
    )?;
    run(
        conn,
        "INSERT INTO meta (k, v) VALUES (?1, ?2) ON CONFLICT (k) DO UPDATE SET v = ?2",
        params![engines::chosen_mark(ty), text],
    )
}

/// The series pairs of `ty`, and whether each is whole, as the `series.`
/// meta row holds them: none where it holds none.
pub(crate) fn series_state(conn: &Connection, ty: &TypeName) -> Result<SeriesState> {
    let text: Option<String> = one(
        conn,
        "SELECT v FROM meta WHERE k = ?1",
        [engines::series_mark(ty)],
    )?;
    Ok(engines::series_decoded(text))
}

/// Keep `ty`'s series pairs as `state`: no row where it holds none.
fn set_series(conn: &Connection, ty: &TypeName, state: &SeriesState) -> Result<()> {
    if *state == SeriesState::default() {
        return run(
            conn,
            "DELETE FROM meta WHERE k = ?1",
            [engines::series_mark(ty)],
        );
    }
    let text = serde_json::to_string(state).expect("the series pairs serialise");
    run(
        conn,
        "INSERT INTO meta (k, v) VALUES (?1, ?2) ON CONFLICT (k) DO UPDATE SET v = ?2",
        params![engines::series_mark(ty), text],
    )
}

/// One step of choosing an order index for `ty` (`ProjectionStore::
/// choose_order`), in a transaction of its own that moves no position.
#[allow(deprecated)]
pub(crate) fn choose_order(
    conn: &Connection,
    ty: &TypeName,
    step: &Choose,
) -> Result<Option<RecordKey>> {
    // A series pair's derived field is kept in the type's `series.` row,
    // not in `Chosen`: its own road fills it from the pair (097 §The
    // series index).
    if let Some(field) = chosen_field(step).filter(|f| engines::is_series_field(f)) {
        return choose_series(conn, ty, step, field);
    }
    // A kept name is built as the view it keeps, as a host's setting for
    // it names the view's fields.
    let kept = engines::kept_view(ty);
    let declared = match &kept {
        Some(view) => type_at(conn, view, None)?,
        None => type_at(conn, ty, None)?,
    };
    let Some(decl) = declared.filter(|d| kept.is_some() || d.view.is_none()) else {
        return Ok(None);
    };
    let setting = head_record(
        conn,
        &TypeName::from(SETTING_TYPE),
        &RecordKey::from(engines::sort_index_key(ty)),
    )?;
    let named =
        engines::sorted_fields(ty, &decl, setting.as_ref().and_then(engines::setting_value));
    let sortable = |f: &FieldName| {
        (decl.fields.iter()).any(|d| d.name == *f && lakelands_api::seam::sortable(&d.ty.kind))
    };
    let mut chosen = chosen_of(conn, ty)?;
    // A field a host's setting names is filled by its own build, field by
    // field; the setting and the lake share the rows, and a step on any
    // other field is the lake's.
    let setting_build = matches!(
        step,
        Choose::Fill { field, .. } | Choose::Keep(field)
            if chosen.setting_building.contains(field)
    );
    let names_field = match step {
        Choose::Start(f) | Choose::Keep(f) | Choose::Drop(f) => named.contains(f),
        Choose::Fill { field, .. } | Choose::DropChunk { field, .. } => named.contains(field),
    };
    // Nothing is chosen on a lake a writer that keeps no chosen field
    // folded past, before a writer's open drops what it left.
    if matches!(step, Choose::Start(f) if !sortable(f))
        || (names_field && !setting_build)
        || !chosen_held(conn)?
    {
        return Ok(None);
    }
    if !engines::chosen_after(&mut chosen, step) {
        return Ok(None);
    }
    conn.execute_batch("BEGIN IMMEDIATE").map_err(sql)?;
    let done = (|| match step {
        Choose::Fill {
            field,
            after,
            limit,
        } => fill_chosen(conn, ty, field, after.as_ref(), *limit),
        // A drop clears a chunk of the field's rows a call; its last, which
        // finds no more records, takes the mark away. Both write the mark,
        // so one a writer went between is taken up by the next. `Drop` is
        // the whole field in one step, kept for a host's build: it answers
        // none.
        Choose::Drop(field) => {
            let _ = drop_chosen(conn, ty, field, None, u32::MAX)?;
            chosen.dropping.retain(|f| f != field);
            if chosen == Chosen::default() {
                set_sorted(conn, ty, None)?;
            }
            set_chosen(conn, ty, &chosen)?;
            Ok(None)
        }
        Choose::DropChunk {
            field,
            after,
            limit,
        } => {
            let last = drop_chosen(conn, ty, field, after.as_ref(), *limit)?;
            if last.is_none() {
                chosen.dropping.retain(|f| f != field);
                if chosen == Chosen::default() {
                    set_sorted(conn, ty, None)?;
                }
            }
            set_chosen(conn, ty, &chosen)?;
            Ok(last)
        }
        _ => {
            set_chosen(conn, ty, &chosen)?;
            match step {
                Choose::Start(_) if sorted(conn, ty)?.is_none() => {
                    set_sorted(conn, ty, folded(conn)?)?;
                }
                _ => {}
            }
            Ok(None)
        }
    })();
    match done {
        Ok(next) => conn.execute_batch("COMMIT").map(|()| next).map_err(sql),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// The field a step of choosing names.
#[allow(deprecated)]
fn chosen_field(step: &Choose) -> Option<&FieldName> {
    match step {
        Choose::Start(f) | Choose::Keep(f) | Choose::Drop(f) => Some(f),
        Choose::Fill { field, .. } | Choose::DropChunk { field, .. } => Some(field),
    }
}

/// One step of a series pair's build, in a transaction of its own that
/// moves no position: the pair's state is kept in `series.<type>`, and its
/// rows under the derived field of `field`.
#[allow(deprecated)]
fn choose_series(
    conn: &Connection,
    ty: &TypeName,
    step: &Choose,
    field: &FieldName,
) -> Result<Option<RecordKey>> {
    let Some((order, series)) = lakelands_api::seam::split_series_field(field.as_str()) else {
        return Ok(None);
    };
    let pair = lakelands_api::engine::SeriesPair {
        series: FieldName::from(series),
        order: FieldName::from(order),
    };
    let without = |state: &mut SeriesState| {
        state.kept.retain(|p| p != &pair);
        state.building.retain(|p| p != &pair);
    };
    conn.execute_batch("BEGIN IMMEDIATE").map_err(sql)?;
    let done = (|| {
        let mut state = series_state(conn, ty)?;
        // A build fills and keeps a pair only while it is `building`: a
        // setting put back beside the writer takes the pair back, whole, in
        // that write, and the part in hand must not fill it again.
        let building = state.building.contains(&pair);
        match step {
            Choose::Start(_) => {
                if !state.kept.contains(&pair) && !state.building.contains(&pair) {
                    state.building.push(pair.clone());
                    state.started += 1;
                }
                set_series(conn, ty, &state)?;
                Ok(None)
            }
            Choose::Fill {
                field,
                after,
                limit,
            } => match building {
                true => fill_chosen(conn, ty, field, after.as_ref(), *limit),
                false => Ok(None),
            },
            Choose::Keep(_) => {
                if building {
                    state.building.retain(|p| p != &pair);
                    if !state.kept.contains(&pair) {
                        state.kept.push(pair.clone());
                    }
                    set_series(conn, ty, &state)?;
                }
                Ok(None)
            }
            // A pair taken back: its rows go whole in one step, or a chunk
            // a call, and its state leaves with the last.
            Choose::Drop(field) => {
                let _ = drop_chosen(conn, ty, field, None, u32::MAX)?;
                without(&mut state);
                set_series(conn, ty, &state)?;
                Ok(None)
            }
            Choose::DropChunk {
                field,
                after,
                limit,
            } => {
                let last = drop_chosen(conn, ty, field, after.as_ref(), *limit)?;
                if last.is_none() {
                    without(&mut state);
                    set_series(conn, ty, &state)?;
                }
                Ok(last)
            }
        }
    })();
    match done {
        Ok(next) => conn.execute_batch("COMMIT").map(|()| next).map_err(sql),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// One step of building the exact-text index a `lookup_index.<type>`
/// setting asks for (`ProjectionStore::look_index`), in a transaction of
/// its own that moves no position: a read takes `lookup_fields` as it
/// stands until the `Keep`.
pub(crate) fn look_index(
    conn: &Connection,
    ty: &TypeName,
    step: &Look,
) -> Result<Option<RecordKey>> {
    conn.execute_batch("BEGIN IMMEDIATE").map_err(sql)?;
    let done = (|| {
        let mut chosen = chosen_of(conn, ty)?;
        match step {
            Look::Fill {
                fields,
                after,
                limit,
            } => fill_looked(conn, ty, fields, after.as_ref(), *limit),
            Look::Keep { fields } => {
                for f in fields {
                    run(
                        conn,
                        "INSERT OR IGNORE INTO lookup_fields (ty, field) VALUES (?1, ?2)",
                        params![ty.as_str(), f.as_str()],
                    )?;
                }
                chosen.looked_building.retain(|f| !fields.contains(f));
                for f in std::mem::take(&mut chosen.looked_going) {
                    run(
                        conn,
                        "DELETE FROM text_index WHERE ty = ?1 AND field = ?2",
                        params![ty.as_str(), f.as_str()],
                    )?;
                    run(
                        conn,
                        "DELETE FROM lookup_fields WHERE ty = ?1 AND field = ?2",
                        params![ty.as_str(), f.as_str()],
                    )?;
                }
                set_chosen(conn, ty, &chosen)?;
                Ok(None)
            }
        }
    })();
    match done {
        Ok(next) => conn.execute_batch("COMMIT").map(|()| next).map_err(sql),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// Write the rows of `fields` for up to `limit` head records of `ty` after
/// `after`; the key the chunk ended at, none once the type is filled.
fn fill_looked(
    conn: &Connection,
    ty: &TypeName,
    fields: &[FieldName],
    after: Option<&RecordKey>,
    limit: u32,
) -> Result<Option<RecordKey>> {
    let after = after.map_or("", RecordKey::as_str);
    let page = records(
        conn,
        concat!(heads!(), " AND k.key > ?2 ORDER BY k.key LIMIT ?3"),
        params![ty.as_str(), after, limit],
    )?;
    let mut last = None;
    for r in &page {
        for field in fields {
            run(
                conn,
                "DELETE FROM text_index WHERE ty = ?1 AND field = ?2 AND key = ?3",
                params![ty.as_str(), field.as_str(), r.key.as_str()],
            )?;
            let Some(text) = r.fields.get(field).and_then(|v| v.as_str()) else {
                continue;
            };
            run(
                conn,
                "INSERT OR IGNORE INTO text_index (ty, field, value, key) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![ty.as_str(), field.as_str(), text, r.key.as_str()],
            )?;
        }
        last = Some(r.key.clone());
    }
    Ok(last.filter(|_| page.len() == limit as usize))
}

/// Write the rows of `field` for up to `limit` head records of `ty` after
/// `after`; the key the chunk ended at, none once the type is filled.
fn fill_chosen(
    conn: &Connection,
    ty: &TypeName,
    field: &FieldName,
    after: Option<&RecordKey>,
    limit: u32,
) -> Result<Option<RecordKey>> {
    let decl = type_at(conn, ty, None)?;
    let after = after.map_or("", RecordKey::as_str);
    let page = records(
        conn,
        concat!(heads!(), " AND k.key > ?2 ORDER BY k.key LIMIT ?3"),
        params![ty.as_str(), after, limit],
    )?;
    let mut last = None;
    for r in &page {
        run(
            conn,
            "DELETE FROM sort_index WHERE ty = ?1 AND field = ?2 AND key = ?3",
            params![ty.as_str(), field.as_str(), r.key.as_str()],
        )?;
        for (field, sort_key) in engines::order_rows(std::slice::from_ref(field), decl.as_ref(), r)
        {
            run(
                conn,
                "INSERT INTO sort_index (ty, field, key, sort_key) VALUES (?1, ?2, ?3, ?4)",
                params![ty.as_str(), field.as_str(), r.key.as_str(), sort_key],
            )?;
        }
        last = Some(r.key.clone());
    }
    Ok(last.filter(|_| page.len() == limit as usize))
}

/// Clear the `field`'s rows of up to `limit` head records of `ty` after
/// `after`, in key order; the key the chunk ended at, none once the type
/// is cleared.
fn drop_chosen(
    conn: &Connection,
    ty: &TypeName,
    field: &FieldName,
    after: Option<&RecordKey>,
    limit: u32,
) -> Result<Option<RecordKey>> {
    let after = after.map_or("", RecordKey::as_str);
    let page = records(
        conn,
        concat!(heads!(), " AND k.key > ?2 ORDER BY k.key LIMIT ?3"),
        params![ty.as_str(), after, limit],
    )?;
    for r in &page {
        run(
            conn,
            "DELETE FROM sort_index WHERE ty = ?1 AND field = ?2 AND key = ?3",
            params![ty.as_str(), field.as_str(), r.key.as_str()],
        )?;
    }
    Ok(page
        .last()
        .map(|r| r.key.clone())
        .filter(|_| page.len() == limit as usize))
}

/// Rewrite `ty`'s order index from its head records, under its setting and
/// declaration at the head, and mark it kept through `through`; with no
/// field to keep, drop its rows and its mark. The fold does this when it
/// puts the setting's head state, and a writer's open when the mark is
/// behind.
pub(crate) fn rebuild_sorted(conn: &Connection, ty: &TypeName, through: Position) -> Result<()> {
    // A series pair's rows are the host's: a `sort_index` rebuild leaves
    // them, and `sorted_fields` never names one (097 §How a lake with
    // records gets it).
    run(
        conn,
        "DELETE FROM sort_index WHERE ty = ?1 AND instr(field, char(31)) = 0",
        [ty.as_str()],
    )?;
    let decl = type_at(conn, ty, None)?;
    // A host's setting that names a field wins: the lake's choice goes.
    let setting = head_record(
        conn,
        &TypeName::from(SETTING_TYPE),
        &RecordKey::from(engines::sort_index_key(ty)),
    )?;
    let named = decl
        .as_ref()
        .map(|d| engines::sorted_fields(ty, d, setting.as_ref().and_then(engines::setting_value)));
    // A host's setting names its fields: the lake's choice of those goes,
    // and its choice of the type's other fields shares the rebuilt rows.
    if let Some(named) = named.filter(|n| !n.is_empty()) {
        let mut chosen = chosen_of(conn, ty)?;
        chosen.kept.retain(|f| !named.contains(f));
        chosen.building.retain(|f| !named.contains(f));
        chosen.dropping.retain(|f| !named.contains(f));
        set_chosen(conn, ty, &chosen)?;
    }
    let fields = sorted_fields(conn, ty, decl.as_ref(), true)?;
    if fields.is_empty() {
        return set_sorted(conn, ty, None);
    }
    // A page of the type at a time, so the pass holds a page, not the type.
    let mut after = String::new();
    loop {
        let page = records(
            conn,
            concat!(heads!(), " AND k.key > ?2 ORDER BY k.key LIMIT 1024"),
            params![ty.as_str(), after],
        )?;
        for r in &page {
            for (field, sort_key) in engines::order_rows(&fields, decl.as_ref(), r) {
                run(
                    conn,
                    "INSERT INTO sort_index (ty, field, key, sort_key) VALUES (?1, ?2, ?3, ?4)",
                    params![ty.as_str(), field.as_str(), r.key.as_str(), sort_key],
                )?;
            }
            after = r.key.to_string();
        }
        if page.len() < 1024 {
            break;
        }
    }
    set_sorted(conn, ty, Some(through))
}

/// Mark `ty`'s order index kept through `through`; with none, drop its
/// mark: the index keeps no field.
fn set_sorted(conn: &Connection, ty: &TypeName, through: Option<Position>) -> Result<()> {
    let Some(at) = through else {
        return run(conn, "DELETE FROM meta WHERE k = ?1", [sorted_mark(ty)]);
    };
    run(
        conn,
        "INSERT INTO meta (k, v) VALUES (?1, ?2) ON CONFLICT (k) DO UPDATE SET v = ?2",
        params![sorted_mark(ty), at.0 as i64],
    )
}

/// At a writer's open: rebuild, in one transaction, every order index
/// that does not describe the head, because a writer that does not keep
/// it (a binary before it) folded past its mark or wrote its setting, and
/// drop the index of a type whose setting such a writer removed. A
/// setting asks for the indexes of [`engines::ordered_names`]; one of
/// those that keeps no field has no mark and is not behind. The lake's
/// choice goes whole where such a writer folded past it: it left the
/// chosen fields' rows as they were while it moved every `sorted.` mark.
pub(crate) fn catch_up_sorted(conn: &Connection) -> Result<()> {
    let Some(folded) = folded(conn)? else {
        return Ok(());
    };
    let marks: Vec<(String, i64)> = conn
        .prepare("SELECT substr(k, 8), v FROM meta WHERE k LIKE 'sorted.%'")
        .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
        .map_err(sql)?;
    let asked: Vec<String> = column(
        conn,
        "SELECT substr(k.key, ?1) FROM keys k CROSS JOIN records r \
         ON r.ty = k.ty AND r.until IS NULL AND r.id = k.id \
         WHERE k.ty = ?2 AND substr(k.key, 1, ?1 - 1) = ?3",
        params![
            SORT_INDEX_SETTING.len() as i64 + 1,
            SETTING_TYPE,
            SORT_INDEX_SETTING
        ],
        Ok,
    )?;
    let mut behind: BTreeSet<String> = marks
        .iter()
        .filter(|(_, v)| *v as u64 != folded.0)
        .map(|(t, _)| t.clone())
        .collect();
    for ty in asked
        .iter()
        .flat_map(|t| engines::ordered_names(&t.as_str().into()))
    {
        if !marks.iter().any(|(t, _)| t == ty.as_str())
            && !sorted_fields(conn, &ty, type_at(conn, &ty, None)?.as_ref(), true)?.is_empty()
        {
            behind.insert(ty.to_string());
        }
    }
    let through: Option<i64> = one(
        conn,
        "SELECT v FROM meta WHERE k = ?1",
        [engines::CHOSEN_THROUGH],
    )?;
    let left = through.is_some_and(|v| v != folded.0 as i64);
    let stale: Vec<TypeName> = match !left {
        true => Vec::new(),
        false => column(
            conn,
            "SELECT substr(k, 8) FROM meta WHERE k LIKE 'chosen.%'",
            [],
            |t: String| Ok(TypeName::from(t)),
        )?,
    };
    behind.extend(stale.iter().map(ToString::to_string));
    if behind.is_empty() && !left {
        return Ok(());
    }
    conn.execute_batch("BEGIN IMMEDIATE").map_err(sql)?;
    let rebuilt = (stale.iter())
        .try_for_each(|ty| set_chosen(conn, ty, &Chosen::default()))
        .and_then(|()| match left {
            true => run(
                conn,
                "DELETE FROM meta WHERE k = ?1",
                [engines::CHOSEN_THROUGH],
            ),
            false => Ok(()),
        })
        .and_then(|()| {
            (behind.iter())
                .try_for_each(|ty| rebuild_sorted(conn, &TypeName::from(ty.as_str()), folded))
        });
    match rebuilt {
        Ok(()) => conn.execute_batch("COMMIT").map_err(sql),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// Record that every order index the lake keeps is kept through
/// `through`: the fold kept each with the head. So are the chosen fields
/// among them if they were kept through `from`, the position it folded
/// from; a writer that keeps none folded past them otherwise, and they
/// stay behind until a writer's open drops them.
pub(crate) fn mark_sorted(
    conn: &Connection,
    from: Option<Position>,
    through: Position,
) -> Result<()> {
    let from = from.map(|p| p.0 as i64);
    conn.prepare_cached("UPDATE meta SET v = ?1 WHERE k LIKE 'sorted.%' OR (k = ?2 AND v IS ?3)")
        .and_then(|mut s| s.execute(params![through.0 as i64, engines::CHOSEN_THROUGH, from]))
        .map(|_| ())
        .map_err(sql)
}

pub(crate) fn set_folded(conn: &Connection, through: Position) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (k, v) VALUES ('folded', ?1) ON CONFLICT (k) DO UPDATE SET v = ?1",
        params![through.0 as i64],
    )
    .map(|_| ())
    .map_err(sql)
}

/// The declaration of a type in force at `at` (`None` for the head).
pub(crate) fn type_at(
    conn: &Connection,
    ty: &TypeName,
    at: Option<Position>,
) -> Result<Option<TypeDecl>> {
    let ident = format!("type {ty}");
    let state: Option<String> = match at {
        Some(at) => conn
            .prepare_cached(&format!(
                "SELECT state FROM declarations WHERE ident = ?1 AND {}",
                holds_at(2)
            ))
            .and_then(|mut s| {
                s.query_row(params![ident, at.0 as i64], |r| r.get(0))
                    .optional()
            }),
        None => conn
            .prepare_cached("SELECT state FROM declarations WHERE ident = ?1 AND until IS NULL")
            .and_then(|mut s| s.query_row(params![ident], |r| r.get(0)).optional()),
    }
    .map_err(sql)?;
    Ok(state.and_then(
        |s| match serde_json::from_str::<Declared>(&s).ok()?.declaration {
            Declaration::Type(t) => Some(t),
            _ => None,
        },
    ))
}

/// Put the entry of the order `name` at `key`, or delete the one there
/// where `entry` is none (record 098, F10).
pub(crate) fn put_order(
    conn: &Connection,
    name: &TypeName,
    key: &RecordKey,
    entry: Option<&[u8]>,
) -> Result<()> {
    match entry {
        None => run(
            conn,
            "DELETE FROM view_order WHERE name = ?1 AND key = ?2",
            params![name.as_str(), key.as_str()],
        ),
        Some(entry) => run(
            conn,
            "INSERT INTO view_order (name, key, entry) VALUES (?1, ?2, ?3) \
             ON CONFLICT (name, key) DO UPDATE SET entry = EXCLUDED.entry",
            params![name.as_str(), key.as_str(), entry],
        ),
    }
}

/// Delete every entry of the order `name`.
pub(crate) fn end_order(conn: &Connection, name: &TypeName) -> Result<()> {
    run(
        conn,
        "DELETE FROM view_order WHERE name = ?1",
        [name.as_str()],
    )
}

/// The entry of the order `name` at `key`, if it holds one.
pub(crate) fn order(
    conn: &Connection,
    name: &TypeName,
    key: &RecordKey,
) -> Result<Option<Vec<u8>>> {
    one(conn, ORDER_AT, params![name.as_str(), key.as_str()])
}

/// The statement an order's entry is read by, as the engine runs it.
pub(crate) const ORDER_AT: &str = "SELECT entry FROM view_order WHERE name = ?1 AND key = ?2";

/// The statement a page of an order is read by, as the engine runs it:
/// after a key, in key order, at most `n`.
pub(crate) const ORDER_PAGE: &str =
    "SELECT key, entry FROM view_order WHERE name = ?1 AND key > ?2 ORDER BY key LIMIT ?3";

/// The statements an order's entry and its page are read by, for the
/// engine's own tests of their plans (lakelands#1690).
pub fn order_statements() -> [String; 2] {
    [ORDER_AT.to_owned(), ORDER_PAGE.to_owned()]
}

/// At most `n` entries of the order `name` after `after`, in key order.
pub(crate) fn order_page(
    conn: &Connection,
    name: &TypeName,
    after: Option<&RecordKey>,
    n: u32,
) -> Result<Vec<(RecordKey, Vec<u8>)>> {
    let after = after.map(RecordKey::as_str).unwrap_or("");
    let mut stmt = conn.prepare_cached(ORDER_PAGE).map_err(sql)?;
    let rows = stmt
        .query_map(params![name.as_str(), after, n], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
        })
        .map_err(sql)?;
    rows.map(|row| {
        let (key, entry) = row.map_err(sql)?;
        Ok((RecordKey::from(key), entry))
    })
    .collect()
}

/// The latest state of one record.
pub(crate) fn head_record(
    conn: &Connection,
    ty: &TypeName,
    key: &RecordKey,
) -> Result<Option<StoredRecord>> {
    let head = concat!(heads!(), " AND k.key = ?2");
    let mut found = records(conn, head, params![ty.as_str(), key.as_str()])?;
    Ok(found.pop())
}

/// A record as a statement answers it from the column `at` on: its row's
/// type, key and position, then its state ([`engines::decode_record`]).
pub(crate) fn record_at(row: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<Kept4> {
    Ok((
        row.get(at)?,
        row.get(at + 1)?,
        row.get(at + 2)?,
        row.get(at + 3)?,
    ))
}

/// A record's row and state as [`record_at`] reads them.
pub(crate) type Kept4 = (String, String, i64, String);

/// The record [`record_at`] read, each vector its state keeps apart read
/// from its row.
pub(crate) fn decode_record(
    conn: &Connection,
    (ty, key, since, state): &Kept4,
) -> Result<StoredRecord> {
    let (mut r, apart) = engines::decode_record(ty, key, Position(*since as u64), state)?;
    if let Some(based) = apart.base {
        let block: Option<String> = one(conn, "SELECT block FROM bases WHERE id = ?1", [based.id])?;
        engines::rebase(&mut r, based, block.as_deref())?;
    }
    for (field, at) in apart.vectors {
        let row = (conn.prepare_cached(
            "SELECT single, data FROM vectors WHERE ty = ?1 AND key = ?2 AND field = ?3 \
             AND since = ?4",
        ))
        .and_then(|mut s| {
            let args = params![ty, key, field.as_str(), at.0 as i64];
            let e = |r: &rusqlite::Row<'_>| Ok(engines::Elements::new(r.get(0)?, r.get(1)?));
            s.query_row(args, e).optional()
        });
        engines::rejoin(&mut r, field, row.map_err(sql)?)?;
    }
    Ok(r)
}

/// The records a cached statement answers, each as [`record_at`] reads
/// it from its first column; decoded once all are read.
pub(crate) fn records(
    conn: &Connection,
    statement: &str,
    args: impl Params,
) -> Result<Vec<StoredRecord>> {
    let mut stmt = conn.prepare_cached(statement).map_err(sql)?;
    let rows: Vec<Kept4> = (stmt.query_map(args, |r| record_at(r, 0)))
        .and_then(Iterator::collect)
        .map_err(sql)?;
    rows.iter().map(|r| decode_record(conn, r)).collect()
}

/// Rewrite the head indexes of one record from its latest state, under
/// the declaration of its type at the head, exact-text rows for `looked`
/// (`None`: every field), dropping first the rows its `prior` head state
/// wrote; a record new to the projection has none and drops none.
pub(crate) fn index_record(
    conn: &Connection,
    (decl, sorted, looked): (
        Option<&TypeDecl>,
        &[FieldName],
        Option<&BTreeSet<FieldName>>,
    ),
    prior: Option<&StoredRecord>,
    r: &StoredRecord,
) -> Result<()> {
    let (ty, key) = (r.ty.as_str(), r.key.as_str());
    let drop = |sql_text: &str| {
        (conn.prepare_cached(sql_text))
            .and_then(|mut s| s.execute(params![ty, key]))
            .map_err(sql)
    };
    let fresh = prior.is_none();
    if let Some(prior) = prior {
        unlength(conn, &r.ty, &r.key)?;
        unlook(conn, decl, prior)?;
        drop("DELETE FROM edges WHERE src_ty = ?1 AND src_key = ?2")?;
        // Only a type the lake keeps an order index for has rows in it:
        // the setting's rebuild and a redeclaration drop a type's rows whole.
        if !sorted.is_empty() {
            drop("DELETE FROM sort_index WHERE ty = ?1 AND key = ?2")?;
        }
    }
    for (field, value) in &r.fields {
        if looked.is_some_and(|l| !l.contains(field)) {
            continue;
        }
        // A number or decimal field keeps its value's token; a text
        // field keeps its text (097 §A number's exact lookup).
        let kind = decl.and_then(|d| engines::field_kind(d, field)).copied();
        if let Some(token) = engines::lookup_token(value, kind) {
            conn.prepare_cached(
                "INSERT OR IGNORE INTO text_index (ty, field, value, key) VALUES (?1, ?2, ?3, ?4)",
            )
            .and_then(|mut s| s.execute(params![ty, field.as_str(), token, key]))
            .map_err(sql)?;
        }
    }
    if !fresh {
        drop(
            "DELETE FROM full_text WHERE rowid = \
             (SELECT id FROM search_rows WHERE ty = ?1 AND key = ?2)",
        )?;
    }
    for (field, sort_key) in engines::order_rows(sorted, decl, r) {
        conn.prepare_cached(
            "INSERT INTO sort_index (ty, field, key, sort_key) VALUES (?1, ?2, ?3, ?4)",
        )
        .and_then(|mut s| s.execute(params![ty, field.as_str(), key, sort_key]))
        .map_err(sql)?;
    }
    // A record keeps a search row only while it has tokens: most have none.
    let tokens = decl
        .map(|d| engines::record_tokens(d, r))
        .unwrap_or_default();
    if tokens.is_empty() {
        if !fresh {
            drop("DELETE FROM search_rows WHERE ty = ?1 AND key = ?2")?;
        }
    } else {
        let row: i64 = {
            conn.prepare_cached("INSERT OR IGNORE INTO search_rows (ty, key) VALUES (?1, ?2)")
                .and_then(|mut s| s.execute(params![ty, key]))
                .map_err(sql)?;
            conn.prepare_cached("SELECT id FROM search_rows WHERE ty = ?1 AND key = ?2")
                .and_then(|mut s| s.query_row(params![ty, key], |r| r.get(0)))
                .map_err(sql)?
        };
        let doc = tokens
            .iter()
            .map(|t| fts_term(t))
            .collect::<Vec<_>>()
            .join(" ");
        conn.prepare_cached("INSERT INTO full_text (rowid, tokens) VALUES (?1, ?2)")
            .and_then(|mut s| s.execute(params![row, doc]))
            .map_err(sql)?;
    }
    let Some(decl) = decl else {
        return Ok(());
    };
    length(conn, decl, r)?;
    for e in engines::links_of(decl, r, None) {
        run(
            conn,
            "INSERT INTO edges (src_ty, src_key, relation, dst_ty, dst_key) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                ty,
                key,
                e.relation.as_str(),
                e.to.0.as_str(),
                e.to.1.as_str()
            ],
        )?;
    }
    Ok(())
}

/// Delete the exact-text rows a head state wrote, each by the primary key
/// its field and token give: a row is written for a text, number or
/// decimal field and none other, so these are all the record has.
fn unlook(conn: &Connection, decl: Option<&TypeDecl>, prior: &StoredRecord) -> Result<()> {
    let (ty, key) = (prior.ty.as_str(), prior.key.as_str());
    for (field, value) in &prior.fields {
        let kind = decl.and_then(|d| engines::field_kind(d, field)).copied();
        if let Some(token) = engines::lookup_token(value, kind) {
            run(
                conn,
                "DELETE FROM text_index WHERE ty = ?1 AND field = ?2 AND value = ?3 AND key = ?4",
                params![ty, field.as_str(), token, key],
            )?;
        }
    }
    Ok(())
}

/// A pipeline token as the full-text table holds it: its UTF-8 bytes in
/// hex. The table's tokenizer then sees every pipeline token as exactly one
/// term of its own alphabet, so whatever characters the one pipeline keeps
/// in a token, the table finds it: there is no second tokenizer to agree
/// with (`docs/decisions/025-full-text-match-sets-are-the-same-on-every-engine.md`).
pub(crate) fn fts_term(token: &str) -> String {
    lakelands_core::canonical::hex(token.as_bytes())
}

/// The types a transaction's records are indexed under, each as its
/// records are: its declaration at the head, the fields its order index
/// keeps, those its exact-text index keeps, and those its states keep
/// apart ([`engines::vector_fields`]).
pub(crate) type Kept = BTreeMap<TypeName, Indexed>;
/// How one type's records are indexed: see [`Kept`].
pub(crate) type Indexed = (
    Option<TypeDecl>,
    Vec<FieldName>,
    Option<BTreeSet<FieldName>>,
    BTreeSet<FieldName>,
);

/// Put a record's state and, when it is the latest, its head indexes. Its
/// type is read into `kept` and not again for another record of that type
/// in the transaction, whatever is put between them: a source's record
/// and a view's kept record follow each other at every event
/// (lakelands#575). A `sort_index.<type>` setting forgets them all, since
/// the type it names may be the settings' own; the caller forgets them at
/// a declaration.
pub(crate) fn put_record(
    conn: &Connection,
    (kept, sharing): (&mut Kept, &mut engines::Sharing),
    r: &StoredRecord,
    whole: bool,
) -> Result<()> {
    let flags = flags_of(r);
    // A head-only name keeps one state for each key and no row beside it:
    // an end takes its key's number with it, since an entry's key holds
    // the value it orders by.
    if lakelands_api::seam::is_head_only(&r.ty) {
        if r.is_ended() {
            let Some(id) = key_id(conn, r.ty.as_str(), r.key.as_str())? else {
                return Ok(());
            };
            run(conn, "DELETE FROM records WHERE id = ?1", [id])?;
            return run(conn, "DELETE FROM keys WHERE id = ?1", [id]);
        }
        let (id, _) = make_key_id(conn, r.ty.as_str(), r.key.as_str())?;
        let state = state(conn, sharing, r, &BTreeMap::new())?;
        run(conn, "DELETE FROM records WHERE id = ?1", [id])?;
        return run(
            conn,
            "INSERT INTO records (ty, id, since, flags, state) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![r.ty.as_str(), id, r.position.0 as i64, flags, state],
        );
    }
    // A kept name keeps one state for each of its keys: a put deletes the
    // states the key held, with their index rows, and an ended record
    // only deletes (098). A state put before one the key already holds is
    // dropped, since the later one is the head.
    if is_kept_name(&r.ty) {
        match key_id(conn, r.ty.as_str(), r.key.as_str())? {
            // An end of a key the name never held writes nothing.
            None if r.is_ended() => return Ok(()),
            None => {}
            Some(id) => {
                let later: Option<i64> = one(
                    conn,
                    "SELECT 1 FROM records WHERE id = ?1 AND since > ?2 LIMIT 1",
                    params![id, r.position.0 as i64],
                )?;
                if later.is_some() {
                    return Ok(());
                }
                if !kept.contains_key(&r.ty) {
                    kept.insert(r.ty.clone(), indexed(conn, &r.ty)?);
                }
                let prior = records(
                    conn,
                    "SELECT r.ty, ?3, r.since, r.state FROM records r \
                     WHERE r.ty = ?1 AND r.id = ?2",
                    params![r.ty.as_str(), id, r.key.as_str()],
                )?
                .pop();
                if let Some(prior) = &prior {
                    let (decl, sorted, looked, vectors) = &kept[&r.ty];
                    let ended = StoredRecord::ended(r.ty.clone(), r.key.clone(), r.position);
                    index_record(
                        conn,
                        (decl.as_ref(), sorted, looked.as_ref()),
                        Some(prior),
                        &ended,
                    )?;
                    put_vectors(conn, &ended, false, vectors)?;
                }
                run(conn, "DELETE FROM records WHERE id = ?1", [id])?;
                if r.is_ended() {
                    return run(conn, "DELETE FROM keys WHERE id = ?1", [id]);
                }
            }
        }
    }
    let (id, made) = make_key_id(conn, r.ty.as_str(), r.key.as_str())?;
    let later: Option<i64> = match made {
        true => None,
        false => one(
            conn,
            "SELECT 1 FROM records WHERE id = ?1 AND since > ?2 LIMIT 1",
            params![id, r.position.0 as i64],
        )?,
    };
    // The head state this one supersedes, whose index rows it replaces.
    let prior = match made || later.is_some() {
        true => None,
        false => records(
            conn,
            "SELECT r.ty, ?3, r.since, r.state FROM records r \
             WHERE r.ty = ?1 AND r.until IS NULL AND r.id = ?2",
            params![r.ty.as_str(), id, r.key.as_str()],
        )?
        .pop(),
    };
    if !kept.contains_key(&r.ty) {
        kept.insert(r.ty.clone(), indexed(conn, &r.ty)?);
    }
    let apart = put_vectors(conn, r, later.is_some(), &kept[&r.ty].3)?;
    let state = state(conn, sharing, r, &apart)?;
    let columns: [(&str, &dyn ToSql); 3] =
        [("ty", &r.ty.as_str()), ("flags", &flags), ("state", &state)];
    RECORDS.put_row(conn, &[&id], r.position, &columns)?;
    if later.is_none() {
        let (decl, sorted, looked, _) = &kept[&r.ty];
        index_record(
            conn,
            (decl.as_ref(), sorted, looked.as_ref()),
            prior.as_ref(),
            r,
        )?;
        // A setting's write no longer walks its type: beside the writer it
        // marks what it asks for as building, and the build fills it in
        // turns; a fold from the log makes it whole at its event, a retired
        // setting included, since what it took back is the writer's to drop.
        if let Some(ty) = engines::sort_index_type(r) {
            for ty in engines::ordered_names(&ty) {
                match whole {
                    true => rebuild_sorted(conn, &ty, r.position)?,
                    false => start_sorted(conn, &ty)?,
                }
            }
            kept.clear();
        }
        // A `series_index.<type>` setting is the host's, kept in its own
        // row: beside the writer it marks each pair as building, and a fold
        // from the log fills the pairs whole and writes them kept.
        if let Some(ty) = engines::series_index_type(r) {
            match whole {
                true => rebuild_series(conn, &ty)?,
                false => mark_series(conn, &ty)?,
            }
            kept.clear();
        }
        // A live `lookup_index.<type>` setting marks the fields that come
        // and go for the build; the commit's relook is not asked for it.
        if let Some(ty) = engines::lookup_index_type(r)
            && !whole
            && !r.retired
        {
            start_looked(conn, &ty)?;
        }
    }
    Ok(())
}

/// Mark the fields `ty`'s `sort_index` setting asks for as building, and
/// ensure the type has a `sorted` mark: reads answer as before until each
/// field's `Keep`. A field the setting no longer names is marked dropping,
/// and a `Drop` clears its rows in parts. The setting and the lake share
/// the rows field by field: a chosen field the setting names is the
/// setting's now.
fn start_sorted(conn: &Connection, ty: &TypeName) -> Result<()> {
    let own = engines::kept_view(ty).unwrap_or_else(|| ty.clone());
    let decl = type_at(conn, &own, None)?;
    let setting = head_record(
        conn,
        &TypeName::from(SETTING_TYPE),
        &RecordKey::from(engines::sort_index_key(ty)),
    )?;
    let named = decl
        .as_ref()
        .map(|d| engines::sorted_fields(ty, d, setting.as_ref().and_then(engines::setting_value)))
        .unwrap_or_default();
    let mut chosen = chosen_of(conn, ty)?;
    chosen.kept.retain(|f| !named.contains(f));
    chosen.building.retain(|f| !named.contains(f));
    let mut dropping = std::mem::take(&mut chosen.dropping);
    // A series pair's derived field is the host's setting's, not a field
    // of the order index the lake chose: it is never marked dropping here.
    let rows: Vec<FieldName> = column(
        conn,
        "SELECT DISTINCT field FROM sort_index WHERE ty = ?1 AND instr(field, char(31)) = 0",
        [ty.as_str()],
        |f: String| Ok(FieldName::from(f)),
    )?;
    for f in rows {
        let held = chosen.kept.contains(&f) || chosen.building.contains(&f);
        if !named.contains(&f) && !held && !dropping.contains(&f) {
            dropping.push(f);
        }
    }
    dropping.retain(|f| !named.contains(f));
    chosen.dropping = dropping;
    chosen.setting_building = named;
    set_chosen(conn, ty, &chosen)?;
    if !chosen.setting_building.is_empty() {
        if sorted(conn, ty)?.is_none() {
            set_sorted(conn, ty, folded(conn)?)?;
        }
    } else if chosen.kept.is_empty() && chosen.building.is_empty() && chosen.dropping.is_empty() {
        set_sorted(conn, ty, None)?;
    }
    Ok(())
}

/// Mark the pairs `ty`'s `series_index` setting asks for as building, and
/// leave the pairs it no longer names out: a read keeps the old road until
/// each pair's `Keep`, and the fold keeps a building pair's rows from its
/// `Start`. A pair still named and already kept stays kept.
fn mark_series(conn: &Connection, ty: &TypeName) -> Result<()> {
    let decl = type_at(conn, ty, None)?;
    let setting = head_record(
        conn,
        &TypeName::from(SETTING_TYPE),
        &RecordKey::from(engines::series_index_key(ty)),
    )?;
    let named = decl
        .as_ref()
        .map(|d| engines::series_pairs(d, setting.as_ref().and_then(engines::setting_value)))
        .unwrap_or_default();
    let mut state = series_state(conn, ty)?;
    // A pair the setting no longer names is taken back: its rows go in the
    // writer's turn, as a `Drop` does, so none is left without a pair.
    let gone: Vec<_> = state
        .kept
        .iter()
        .chain(state.building.iter())
        .filter(|p| !named.contains(p))
        .cloned()
        .collect();
    for pair in &gone {
        let field = FieldName::from(lakelands_api::seam::series_field(
            pair.order.as_str(),
            pair.series.as_str(),
        ));
        let _ = drop_chosen(conn, ty, &field, None, u32::MAX)?;
    }
    state.kept.retain(|p| named.contains(p));
    state.building.retain(|p| named.contains(p));
    let mut started = false;
    for pair in named {
        if !state.kept.contains(&pair) && !state.building.contains(&pair) {
            state.building.push(pair);
            started = true;
        }
    }
    if started {
        state.started += 1;
    }
    set_series(conn, ty, &state)
}

/// Fill `ty`'s series pairs whole from its head records, under its setting
/// and declaration at the head, and write them kept: a fold from the log
/// makes a pair whole at its event rather than marking it for a build. A
/// setting taken back leaves no pair and drops their rows.
fn rebuild_series(conn: &Connection, ty: &TypeName) -> Result<()> {
    // A fold from the log makes the pairs whole, so a build may go on from
    // its cursor; the count goes on rather than back, so a build never
    // reads a number again for a pair whose rows a take-back beside the
    // writer has since cleared.
    let started = series_state(conn, ty)?.started;
    run(
        conn,
        "DELETE FROM sort_index WHERE ty = ?1 AND instr(field, char(31)) > 0",
        [ty.as_str()],
    )?;
    let decl = type_at(conn, ty, None)?;
    let setting = head_record(
        conn,
        &TypeName::from(SETTING_TYPE),
        &RecordKey::from(engines::series_index_key(ty)),
    )?;
    let pairs = decl
        .as_ref()
        .map(|d| engines::series_pairs(d, setting.as_ref().and_then(engines::setting_value)))
        .unwrap_or_default();
    let state = SeriesState {
        kept: pairs,
        building: Vec::new(),
        started,
    };
    set_series(conn, ty, &state)?;
    let fields = engines::series_fields(&state);
    if fields.is_empty() {
        return Ok(());
    }
    let mut after = String::new();
    loop {
        let page = records(
            conn,
            concat!(heads!(), " AND k.key > ?2 ORDER BY k.key LIMIT 1024"),
            params![ty.as_str(), after],
        )?;
        for r in &page {
            for (field, sort_key) in engines::order_rows(&fields, decl.as_ref(), r) {
                run(
                    conn,
                    "INSERT INTO sort_index (ty, field, key, sort_key) VALUES (?1, ?2, ?3, ?4)",
                    params![ty.as_str(), field.as_str(), r.key.as_str(), sort_key],
                )?;
            }
            after = r.key.to_string();
        }
        if page.len() < 1024 {
            return Ok(());
        }
    }
}

/// Mark the fields `ty`'s `lookup_index` setting asks the exact-text
/// index for as building — those coming, and those going — and leave
/// `lookup_fields` as it is, so a read answers as before until `Keep`.
fn start_looked(conn: &Connection, ty: &TypeName) -> Result<()> {
    let decls = head_declarations(conn)?;
    let setting = head_record(
        conn,
        &TypeName::from(SETTING_TYPE),
        &RecordKey::from(format!("{}{ty}", lakelands_api::seam::LOOKUP_INDEX_SETTING)),
    )?;
    let now: BTreeSet<FieldName> = engines::looked_up(
        ty,
        decls.iter(),
        setting.as_ref().and_then(engines::setting_value),
    )
    .unwrap_or_default();
    let was = looked_fields(conn, ty)?;
    let mut chosen = chosen_of(conn, ty)?;
    chosen.looked_building = now.difference(&was).cloned().collect();
    chosen.looked_going = was.difference(&now).cloned().collect();
    set_chosen(conn, ty, &chosen)
}

/// The state of `r` ([`engines::record_state`]), its base kept in `bases`
/// once where its write shares it ([`engines::Sharing`]): the states of
/// the write put before that keep it inline until then, and are named
/// again.
fn state(
    conn: &Connection,
    sharing: &mut engines::Sharing,
    r: &StoredRecord,
    apart: &BTreeMap<FieldName, Position>,
) -> Result<String> {
    let mut keep = |base: &str| {
        if let Some(id) = one(conn, "SELECT id FROM bases WHERE block = ?1", [base])? {
            return Ok(id);
        }
        (conn.prepare_cached("INSERT INTO bases (block) VALUES (?1) RETURNING id"))
            .and_then(|mut s| s.query_row([base], |r| r.get(0)))
            .map_err(sql)
    };
    engines::record_state(r, apart, &mut |base| {
        let mut name = |ty: &TypeName, key: &RecordKey, at: Position, id| {
            let at = at.0 as i64;
            let ident = params![ty.as_str(), key.as_str(), at];
            let held: Option<String> = one(
                conn,
                "SELECT state FROM records WHERE id = \
                 (SELECT id FROM keys WHERE ty = ?1 AND key = ?2) AND since = ?3",
                ident,
            )?;
            let Some(named) = held.map(|s| engines::name_base(&s, base, id)) else {
                return Ok(());
            };
            let Some(named) = named? else { return Ok(()) };
            run(
                conn,
                "UPDATE records SET state = ?4 WHERE id = \
                 (SELECT id FROM keys WHERE ty = ?1 AND key = ?2) AND since = ?3",
                params![ty.as_str(), key.as_str(), at, named],
            )
        };
        sharing.id(r, base, &mut keep, &mut name)
    })
}

/// Write the vector rows of `r`'s state ([`engines::put_vectors`]), `later`
/// where a later state of its record is kept, and answer the fields of
/// `vectors` it keeps apart. A vector the index put alone for a field the
/// state names goes.
fn put_vectors(
    conn: &Connection,
    r: &StoredRecord,
    later: bool,
    vectors: &BTreeSet<FieldName>,
) -> Result<BTreeMap<FieldName, Position>> {
    let (ty, key, at) = (r.ty.as_str(), r.key.as_str(), r.position.0 as i64);
    let mut stmt = (conn.prepare_cached(
        "SELECT field, since, single, data FROM vectors WHERE ty = ?1 AND key = ?2 \
         AND until IS NULL",
    ))
    .map_err(sql)?;
    let rows = stmt.query_map(params![ty, key], |row| {
        let (field, since): (String, i64) = (row.get(0)?, row.get(1)?);
        let e = engines::Elements::new(row.get(2)?, row.get(3)?);
        Ok((FieldName::from(field), since, e))
    });
    let rows: Vec<_> = rows.and_then(Iterator::collect).map_err(sql)?;
    let (loose, held): (Vec<_>, Vec<_>) = rows.into_iter().partition(|r| r.1 < 0);
    let held = (held.into_iter()).map(|(f, since, e)| (f, Position(since as u64), e));
    let put = engines::put_vectors(r, held.collect(), later, vectors);
    let until = put.ends.then_some(at);
    for (f, e) in &put.written {
        run(
            conn,
            "INSERT INTO vectors (ty, key, field, since, until, single, data) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT DO UPDATE SET \
             until = excluded.until, single = excluded.single, data = excluded.data",
            params![ty, key, f.as_str(), at, until, e.single, e.data],
        )?;
    }
    for (f, since) in &put.ended {
        run(
            conn,
            "UPDATE vectors SET until = ?5 WHERE ty = ?1 AND key = ?2 AND field = ?3 \
             AND since = ?4",
            params![ty, key, f.as_str(), since.0 as i64, at],
        )?;
    }
    let gone = (put.gone.iter().map(|f| (f, at))).chain(
        (loose.iter()).filter_map(|(f, since, _)| put.apart.contains_key(f).then_some((f, *since))),
    );
    for (f, since) in gone {
        run(
            conn,
            "DELETE FROM vectors WHERE ty = ?1 AND key = ?2 AND field = ?3 AND since = ?4",
            params![ty, key, f.as_str(), since],
        )?;
    }
    Ok(put.apart)
}

/// Put a vector the index is asked to hold for `ty`'s `key`, as a row of
/// its own (`since` -1). The fold asks it after the state's row, which
/// holds it already; a state's row holding other elements is taken out
/// ([`remove_vector`]) and the vector put alone stands until the record's
/// next state.
pub(crate) fn put_vector(conn: &Connection, at: (&str, &str, &str), vector: &[f32]) -> Result<()> {
    let held = (conn.prepare_cached(
        "SELECT single, data FROM vectors WHERE ty = ?1 AND key = ?2 AND field = ?3 \
         AND since >= 0 AND until IS NULL",
    ))
    .and_then(|mut s| {
        s.query_row(params![at.0, at.1, at.2], |r| {
            Ok(engines::Elements::new(r.get(0)?, r.get(1)?))
        })
        .optional()
    })
    .map_err(sql)?;
    if held.is_some_and(|e| e.ranked() == vector) {
        return Ok(());
    }
    remove_vector(conn, at)?;
    run(
        conn,
        "INSERT INTO vectors (ty, key, field, since, until, single, data) \
         VALUES (?1, ?2, ?3, -1, NULL, 1, ?4)",
        params![at.0, at.1, at.2, engines::vector_bytes(vector)],
    )
}

/// Forget the vector the index holds for `ty`'s `key`: one put alone goes,
/// and a state's row in force ends at its own position, so a view of that
/// state still reads it, a read joining a row by its position alone.
pub(crate) fn remove_vector(conn: &Connection, (ty, key, field): (&str, &str, &str)) -> Result<()> {
    let at = params![ty, key, field];
    run(
        conn,
        "DELETE FROM vectors WHERE ty = ?1 AND key = ?2 AND field = ?3 AND since = -1",
        at,
    )?;
    run(
        conn,
        "UPDATE vectors SET until = since WHERE ty = ?1 AND key = ?2 AND field = ?3 \
         AND until IS NULL",
        at,
    )
}

/// Delete every state of a record, its memberships in every set the
/// projection ever declared, and its rows in the engine's indexes.
pub(crate) fn drop_record(conn: &Connection, ty: &TypeName, key: &RecordKey) -> Result<()> {
    if let Some(head) = head_record(conn, ty, key)? {
        let decl = type_at(conn, ty, None)?;
        unlook(conn, decl.as_ref(), &head)?;
    }
    run(
        conn,
        "DELETE FROM records WHERE id = (SELECT id FROM keys WHERE ty = ?1 AND key = ?2)",
        params![ty.as_str(), key.as_str()],
    )?;
    for statement in [
        "DELETE FROM vectors WHERE ty = ?1 AND key = ?2",
        "DELETE FROM members WHERE object_set IN (SELECT DISTINCT substr(ident, 12) \
         FROM declarations WHERE ident LIKE 'object\\_set %' ESCAPE '\\') AND ty = ?1 AND key = ?2",
        "DELETE FROM edges WHERE src_ty = ?1 AND src_key = ?2",
        "DELETE FROM sort_index WHERE ty = ?1 AND key = ?2",
        "DELETE FROM full_text WHERE rowid IN (SELECT id FROM search_rows WHERE ty = ?1 AND key = ?2)",
        "DELETE FROM search_rows WHERE ty = ?1 AND key = ?2",
    ] {
        (conn.prepare_cached(statement))
            .and_then(|mut s| s.execute(params![ty.as_str(), key.as_str()]))
            .map_err(sql)?;
    }
    unlength(conn, ty, key)
}

/// The meta key of the position the text statistics are folded through;
/// -1 where they count an empty projection.
const TEXT_STATS: &str = "text_stats";

/// Add a live head's text into its type's counts (`docs/seams.md`
/// §`ranked`): each searchable field's tokens, under its hold, kept beside
/// so that the next write of the record takes out exactly this.
fn length(conn: &Connection, decl: &TypeDecl, r: &StoredRecord) -> Result<()> {
    if r.retired {
        return Ok(());
    }
    let (ty, key, held) = (r.ty.as_str(), r.key.as_str(), r.restricted);
    for (field, tokens) in engines::text_lengths(decl, r) {
        let at = params![ty, key, field.as_str(), held, tokens as i64];
        (conn.prepare_cached(
            "INSERT INTO text_lengths (ty, key, field, held, tokens) VALUES (?1, ?2, ?3, ?4, ?5)",
        ))
        .and_then(|mut s| s.execute(at))
        .map_err(sql)?;
        (conn.prepare_cached(
            "INSERT INTO text_stats (ty, field, held, records, tokens) VALUES (?1, ?2, ?3, 1, ?4) \
             ON CONFLICT (ty, field, held) DO UPDATE \
             SET records = records + 1, tokens = tokens + excluded.tokens",
        ))
        .and_then(|mut s| s.execute(params![ty, field.as_str(), held, tokens as i64]))
        .map_err(sql)?;
    }
    Ok(())
}

/// Take what [`length`] added for a record out of its type's counts.
fn unlength(conn: &Connection, ty: &TypeName, key: &RecordKey) -> Result<()> {
    let at = params![ty.as_str(), key.as_str()];
    (conn.prepare_cached(
        "UPDATE text_stats SET records = text_stats.records - 1, \
         tokens = text_stats.tokens - l.tokens \
         FROM (SELECT field, held, tokens FROM text_lengths WHERE ty = ?1 AND key = ?2) AS l \
         WHERE text_stats.ty = ?1 AND text_stats.field = l.field AND text_stats.held = l.held",
    ))
    .and_then(|mut s| s.execute(at))
    .map_err(sql)?;
    (conn.prepare_cached("DELETE FROM text_lengths WHERE ty = ?1 AND key = ?2"))
        .and_then(|mut s| s.execute(at))
        .map(drop)
        .map_err(sql)
}

/// Whether the text statistics describe the head: their mark is the
/// position folded through, or -1 on an empty projection.
fn text_stats_held(conn: &Connection) -> Result<bool> {
    let mark: Option<i64> = one(conn, "SELECT v FROM meta WHERE k = ?1", [TEXT_STATS])?;
    let folded = folded(conn)?.map_or(-1, |p| p.0 as i64);
    Ok(mark == Some(folded))
}

/// Record that the text statistics are folded through `through`, where
/// they were through `from`, the position the fold folded from; a writer
/// that keeps none folded past them otherwise, and they stay behind until
/// a writer's open counts them again.
pub(crate) fn mark_text_stats(
    conn: &Connection,
    from: Option<Position>,
    through: Position,
) -> Result<()> {
    let from = from.map_or(-1, |p| p.0 as i64);
    (conn.prepare_cached("UPDATE meta SET v = ?1 WHERE k = ?2 AND v = ?3"))
        .and_then(|mut s| s.execute(params![through.0 as i64, TEXT_STATS, from]))
        .map(drop)
        .map_err(sql)
}

/// Each head of `ty` that is not ended, a page at a time.
fn head_pages(
    conn: &Connection,
    ty: &str,
    mut each: impl FnMut(Vec<StoredRecord>) -> Result<()>,
) -> Result<()> {
    let mut after = String::new();
    loop {
        let page = records(
            conn,
            concat!(heads!(), " AND k.key > ?2 ORDER BY k.key LIMIT 1024"),
            params![ty, after],
        )?;
        let next = (page.len() == 1024).then(|| page.last().map(|r| r.key.to_string()));
        each(page.into_iter().filter(|r| !r.is_ended()).collect())?;
        match next.flatten() {
            Some(last) => after = last,
            None => return Ok(()),
        }
    }
}

/// Count the text statistics again from the head where their mark is
/// absent or behind: a lake written before they were kept, or folded by
/// a writer that keeps none. Once, in one transaction.
pub(crate) fn catch_up_text_stats(conn: &Connection) -> Result<()> {
    if text_stats_held(conn)? {
        return Ok(());
    }
    conn.execute_batch("BEGIN IMMEDIATE").map_err(sql)?;
    let counted = (|| {
        conn.execute_batch("DELETE FROM text_stats; DELETE FROM text_lengths;")
            .map_err(sql)?;
        let types: Vec<String> = column(
            conn,
            "SELECT DISTINCT ty FROM records WHERE until IS NULL",
            [],
            Ok,
        )?;
        for ty in types
            .iter()
            .filter(|t| !lakelands_api::seam::is_head_only(&t.as_str().into()))
        {
            let Some(decl) = type_at(conn, &ty.as_str().into(), None)? else {
                continue;
            };
            head_pages(conn, ty, |page| {
                page.iter().try_for_each(|r| length(conn, &decl, r))
            })?;
        }
        let folded = folded(conn)?.map_or(-1, |p| p.0 as i64);
        run(
            conn,
            "INSERT INTO meta (k, v) VALUES (?1, ?2) ON CONFLICT (k) DO UPDATE SET v = ?2",
            params![TEXT_STATS, folded],
        )
    })();
    match counted {
        Ok(()) => conn.execute_batch("COMMIT").map_err(sql),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// The counts of `ty`'s fields over its live heads that are not held,
/// and with `held` over every one: as kept, where they describe the head,
/// else counted from it at the read.
pub(crate) fn text_counts(
    conn: &Connection,
    ty: &TypeName,
    decl: &TypeDecl,
    held: bool,
) -> Result<BTreeMap<FieldName, (u64, u64)>> {
    if !text_stats_held(conn)? {
        let mut counts = TextCounts::default();
        head_pages(conn, ty.as_str(), |page| {
            let class = page
                .iter()
                .filter(|r| !r.retired && (held || !r.restricted));
            engines::text_stats(decl, class, ("", None), &mut counts);
            Ok(())
        })?;
        return Ok(counts.fields);
    }
    let mut stmt = conn
        .prepare_cached(
            "SELECT field, sum(records), sum(tokens) FROM text_stats \
             WHERE ty = ?1 AND (held = 0 OR ?2) GROUP BY field",
        )
        .map_err(sql)?;
    let rows = stmt.query_map(params![ty.as_str(), held], |r| {
        let (n, l): (i64, i64) = (r.get(1)?, r.get(2)?);
        Ok((
            FieldName::from(r.get::<_, String>(0)?),
            (n as u64, l as u64),
        ))
    });
    rows.and_then(Iterator::collect).map_err(sql)
}

/// Delete the bases no record's state names.
pub(crate) fn drop_bases(conn: &Connection) -> Result<()> {
    run(
        conn,
        "DELETE FROM bases WHERE id NOT IN (SELECT json_extract(state, '$.base') \
         FROM records WHERE json_type(state, '$.base') = 'integer')",
        [],
    )
}

/// Delete every state of a seam's output for one input.
pub(crate) fn drop_output(conn: &Connection, seam: &str, input: &str) -> Result<()> {
    run(
        conn,
        "DELETE FROM outputs WHERE seam = ?1 AND input = ?2",
        params![seam, input],
    )
}

/// How `ty`'s records are indexed at the head ([`Kept`]); a kept name's
/// order fields are its view's, and every text field has rows.
fn indexed(conn: &Connection, ty: &TypeName) -> Result<Indexed> {
    let decl = match engines::kept_view(ty) {
        Some(_) => None,
        None => type_at(conn, ty, None)?,
    };
    let mut sorted = sorted_fields(conn, ty, decl.as_ref(), true)?;
    // A type's series pairs keep their rows beside its order fields', from
    // the pair's `Start` on (097 §The series index).
    if decl.is_some() {
        sorted.extend(engines::series_fields(&series_state(conn, ty)?));
    }
    let vectors = engines::vector_fields(ty, |t| match t == ty {
        true => Ok(decl.clone()),
        false => type_at(conn, t, None),
    })?;
    let looked = match decl {
        Some(_) => {
            // A field the setting is having built is no read's until its
            // `Keep`, but a write beside the build keeps its rows too.
            let mut fields = looked_fields(conn, ty)?;
            fields.extend(chosen_of(conn, ty)?.looked_building);
            Some(fields)
        }
        None => None,
    };
    Ok((decl, sorted, looked, vectors))
}

/// The fields of a declared type the exact-text index keeps ([`relook`]).
pub(crate) fn looked_fields(conn: &Connection, ty: &TypeName) -> Result<BTreeSet<FieldName>> {
    let sql = "SELECT field FROM lookup_fields WHERE ty = ?1";
    column(conn, sql, [ty.as_str()], |f: String| Ok(FieldName::from(f)))
}

/// The declarations at the head.
pub(crate) fn head_declarations(conn: &Connection) -> Result<Vec<Declaration>> {
    let decode = |s: String| Ok(engines::decode::<Declared>(&s, "a declaration")?.declaration);
    let sql = "SELECT state FROM declarations WHERE until IS NULL";
    column(conn, sql, [], decode)
}

/// Decide again which fields the exact-text index keeps ([`engines::relooked`])
/// for the types `moved` names (`None`, every type), and fill or drop each
/// field's rows from the head records: at the commit of a transaction that
/// put a declaration or a `lookup_index.<type>`.
pub(crate) fn relook(
    conn: &Connection,
    heads: &engines::Heads,
    moved: Option<&BTreeSet<TypeName>>,
) -> Result<()> {
    let mut was = BTreeSet::new();
    for ty in moved.into_iter().flatten() {
        let fields = looked_fields(conn, ty)?.into_iter();
        was.extend(fields.map(|f| (ty.to_string(), f.to_string())));
    }
    if moved.is_none() {
        let sql = "SELECT ty || char(9) || field FROM lookup_fields";
        let rows: Vec<String> = column(conn, sql, [], Ok)?;
        let rows = rows.iter().filter_map(|r| r.split_once('\t'));
        was.extend(rows.map(|(t, f)| (t.to_owned(), f.to_owned())));
    }
    let setting =
        |key: &str| head_record(conn, &TypeName::from(SETTING_TYPE), &RecordKey::from(key));
    for (ty, f, comes) in engines::relooked(heads, moved, &was, setting)? {
        let field = FieldName::from(f.as_str());
        let numbered = heads.values().find_map(|d| match d {
            Declaration::Type(t) if t.name.as_str() == ty => {
                engines::field_kind(t, &field).copied()
            }
            _ => None,
        });
        if comes && matches!(numbered, Some(StorageKind::Number | StorageKind::Decimal)) {
            // The token is made in Rust, so the fill reads the type's
            // head states rather than asking `json_extract` for them.
            let rows = records(conn, heads!(), params![ty.as_str()])?;
            for r in &rows {
                let value = r.fields.get(&field);
                if let Some(token) = value.and_then(|v| engines::lookup_token(v, numbered)) {
                    run(
                        conn,
                        "INSERT OR IGNORE INTO text_index (ty, field, value, key) \
                         VALUES (?1, ?2, ?3, ?4)",
                        params![ty.as_str(), f.as_str(), token, r.key.as_str()],
                    )?;
                }
            }
            run(
                conn,
                "INSERT INTO lookup_fields (ty, field) VALUES (?1, ?2)",
                params![ty, f],
            )?;
            continue;
        }
        let statements = match comes {
            true => [
                "INSERT OR IGNORE INTO text_index (ty, field, value, key) \
                 SELECT r.ty, ?2, json_extract(r.state, '$.fields.' || ?2), k.key \
                 FROM records r CROSS JOIN keys k ON k.id = r.id WHERE r.ty = ?1 \
                 AND r.until IS NULL AND json_type(r.state, '$.fields.' || ?2) = 'text'",
                "INSERT INTO lookup_fields (ty, field) VALUES (?1, ?2)",
            ],
            false => [
                "DELETE FROM text_index WHERE ty = ?1 AND field = ?2",
                "DELETE FROM lookup_fields WHERE ty = ?1 AND field = ?2",
            ],
        };
        for statement in statements {
            run(conn, statement, params![ty, f])?;
        }
    }
    Ok(())
}

/// Put a declaration. For a type now in force at the head its records'
/// head indexes are to be rewritten under it, and it answers the type:
/// the commit does that with [`index_older`], for the records the
/// transaction has not put again by then.
pub(crate) fn put_declaration(conn: &Connection, d: &Declared) -> Result<Option<TypeName>> {
    let ident = engines::declaration_key(&d.declaration);
    let state = serde_json::to_string(d).expect("a declaration serialises");
    DECLARATIONS.put(conn, &[&ident], d.since, &state)?;
    let Declaration::Type(t) = &d.declaration else {
        return Ok(None);
    };
    if type_at(conn, &t.name, None)?.as_ref() != Some(t) {
        return Ok(None);
    }
    // The rows go whole: the declaration may have dropped a field the index
    // kept. The mark follows the fields it leaves the index, which the
    // records put after it and the commit fill: a setting put before the
    // declaration that first declares its fields found none to mark.
    conn.prepare_cached("DELETE FROM sort_index WHERE ty = ?1")
        .and_then(|mut s| s.execute([t.name.as_str()]))
        .map_err(sql)?;
    let kept = !sorted_fields(conn, &t.name, Some(t), true)?.is_empty();
    set_sorted(conn, &t.name, kept.then_some(d.since))?;
    // A pair goes back to `Start`: the declaration may have dropped or
    // changed a field its rows were filled under, so no reader takes rows
    // the declaration removed.
    let mut series = series_state(conn, &t.name)?;
    if !series.kept.is_empty() {
        series.building.append(&mut series.kept);
        set_series(conn, &t.name, &series)?;
    }
    // The pairs the setting at the head names are building under it, so a
    // setting put before the declaration that first declares its fields
    // starts its pairs here, as one put over a built pair restarts them.
    mark_series(conn, &t.name)?;
    Ok(Some(t.name.clone()))
}

/// Rewrite the head indexes of the records of `ty` whose latest state is
/// older than `at`, under the type's declaration at the head. A record put
/// at `at` or after was indexed under that declaration when it was put: a
/// migration puts every record again, and nothing is left to do here.
pub(crate) fn index_older(conn: &Connection, ty: &TypeName, at: Position) -> Result<()> {
    let (head, sorted, looked, _) = indexed(conn, ty)?;
    // A page of the type at a time; an ended key has no index rows.
    let mut after = String::new();
    loop {
        let older = records(
            conn,
            concat!(
                heads!(),
                " AND r.since < ?2 AND k.key > ?3 ORDER BY k.key LIMIT 1024"
            ),
            params![ty.as_str(), at.0 as i64, after],
        )?;
        for r in older.iter().filter(|r| !r.is_ended()) {
            index_record(conn, (head.as_ref(), &sorted, looked.as_ref()), Some(r), r)?;
        }
        match older.last() {
            Some(last) if older.len() == 1024 => after = last.key.to_string(),
            _ => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests;
