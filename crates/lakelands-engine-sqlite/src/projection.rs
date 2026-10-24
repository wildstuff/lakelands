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

use lakelands_api::engine::{Choose, Chosen, Declared};
use lakelands_api::error::Result;
use lakelands_api::id::{FieldName, Position, RecordKey, TypeName};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::{FeedFilter, SETTING_TYPE, SORT_INDEX_SETTING};
use lakelands_api::write::Declaration;
use lakelands_core::engines::{self, sorted_mark};
use rusqlite::types::FromSql;
use rusqlite::{Connection, OptionalExtension, Params, ToSql, params};

use crate::sql;

/// The schema. `state` columns hold the state as JSON; a record's leaves
/// out what its row holds ([`engines::record_state`]). A table whose
/// primary key is the whole row keeps no rowid beside it (`WITHOUT ROWID`),
/// which would hold every row twice. `search_rows` holds a row only for a
/// record with full-text tokens. `vectors` finds a record's rows by its
/// key and a field's rows in force by `vectors_head`, which a ranking
/// scans; `since` -1 marks a vector put by the index alone ([`put_vector`]).
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
CREATE TABLE IF NOT EXISTS records (
    ty TEXT NOT NULL, key TEXT NOT NULL, since INTEGER NOT NULL, until INTEGER,
    state TEXT NOT NULL,
    PRIMARY KEY (ty, key, since)
);
CREATE INDEX IF NOT EXISTS records_at ON records (ty, until, key);
CREATE INDEX IF NOT EXISTS declarations_until ON declarations (until);
CREATE INDEX IF NOT EXISTS records_since ON records (ty, since);
CREATE TABLE IF NOT EXISTS proposals (
    id TEXT NOT NULL, since INTEGER NOT NULL, until INTEGER,
    state TEXT NOT NULL,
    PRIMARY KEY (id, since)
);
CREATE INDEX IF NOT EXISTS proposals_newest ON proposals (json_extract(state, '$.position'));
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
    id TEXT PRIMARY KEY,
    at INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS routes (
    at INTEGER NOT NULL, kind TEXT NOT NULL, ty TEXT, key TEXT, source TEXT
);
CREATE INDEX IF NOT EXISTS routes_kind ON routes (kind, at);
CREATE INDEX IF NOT EXISTS routes_ty ON routes (ty, at);
CREATE INDEX IF NOT EXISTS routes_key ON routes (key, at);
CREATE INDEX IF NOT EXISTS routes_source ON routes (source, at);
CREATE TABLE IF NOT EXISTS text_index (
    ty TEXT NOT NULL, field TEXT NOT NULL, value TEXT NOT NULL, key TEXT NOT NULL,
    PRIMARY KEY (ty, field, value, key)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS text_index_key ON text_index (ty, key);
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
CREATE INDEX IF NOT EXISTS sort_index_desc ON sort_index (ty, field, sort_key DESC, key);
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
";

/// Every table the projection holds, cleared by a rebuild.
pub(crate) const TABLES: [&str; 15] = [
    "meta",
    "declarations",
    "records",
    "proposals",
    "members",
    "outputs",
    "applied",
    "routes",
    "text_index",
    "lookup_fields",
    "edges",
    "sort_index",
    "search_rows",
    "full_text",
    "vectors",
];

/// A history table: its name and the columns naming one thing in it.
pub(crate) struct History {
    pub(crate) table: &'static str,
    pub(crate) ident: &'static [&'static str],
}

pub(crate) const DECLARATIONS: History = History::new("declarations", &["ident"]);
pub(crate) const RECORDS: History = History::new("records", &["ty", "key"]);
pub(crate) const PROPOSALS: History = History::new("proposals", &["id"]);
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
        let n = self.ident.len();
        let at = since.0 as i64;
        let mut args: Vec<&dyn ToSql> = ident.to_vec();
        args.push(&at);
        let replaced = conn
            .prepare_cached(&format!(
                "UPDATE {} SET state = ?{} WHERE {} AND since = ?{}",
                self.table,
                n + 2,
                self.ident_where(1),
                n + 1
            ))
            .and_then(|mut s| {
                let mut a = args.clone();
                a.push(state);
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
        let columns = self.ident.join(", ");
        let marks: Vec<String> = (1..=n + 3).map(|i| format!("?{i}")).collect();
        conn.prepare_cached(&format!(
            "INSERT INTO {} ({columns}, since, until, state) VALUES ({})",
            self.table,
            marks.join(", ")
        ))
        .and_then(|mut s| {
            let mut a = args.clone();
            a.push(&next);
            a.push(state);
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

/// The positions from `from` through `through` of the feed index's rows
/// whose `column` is `value`, in order and at most `limit`, each row
/// checked against the filter's other lists (`lakelands_core::feed::Seek`).
pub(crate) fn routes(
    conn: &Connection,
    filter: &FeedFilter,
    by: Option<(&str, &str)>,
    [from, through]: [Position; 2],
    limit: u32,
) -> Result<Vec<Position>> {
    let lists: [(&str, Vec<&str>); 3] = [
        ("kind", filter.kinds.iter().map(String::as_str).collect()),
        ("ty", filter.types.iter().map(TypeName::as_str).collect()),
        ("key", filter.keys.iter().map(RecordKey::as_str).collect()),
    ];
    let mut args: Vec<&dyn ToSql> = Vec::new();
    let mut tests = Vec::new();
    for (column, values) in by.iter().map(|(c, v)| (*c, std::slice::from_ref(v))).chain(
        (lists.iter())
            .filter(|(c, v)| !v.is_empty() && by.is_none_or(|(b, _)| b != *c))
            .map(|(c, v)| (*c, v.as_slice())),
    ) {
        let marks = vec!["?"; values.len()].join(", ");
        tests.push(format!("AND {column} IN ({marks})"));
        args.extend(values.iter().map(|v| v as &dyn ToSql));
    }
    let (from, through, limit) = (from.0 as i64, through.0 as i64, i64::from(limit.max(1)));
    args.extend([&from as &dyn ToSql, &through, &limit]);
    let statement = format!(
        "SELECT DISTINCT at FROM routes WHERE 1 {} AND at BETWEEN ? AND ? ORDER BY at LIMIT ?",
        tests.join(" ")
    );
    column(conn, &statement, args.as_slice(), |at: i64| {
        Ok(Position(at as u64))
    })
}

/// The layout of the projection this build reads and writes. It is
/// raised whenever a table changes so that a projection made before
/// would answer wrongly, not fail: one made with another layout is never
/// read, and is rebuilt from the log.
pub(crate) const LAYOUT: u64 = 5;

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
    // A build between the leaner states and the vectors kept apart
    // (lakelands#1318, lakelands#1323) marked this layout with `vectors`
    // still a head index: that projection is the layout before.
    let apart = "SELECT 1 FROM pragma_table_info('vectors') WHERE name = 'since'";
    let shape = |v: i64| match v as u64 == LAYOUT && one::<i64>(conn, apart, [])?.is_none() {
        true => Ok(LAYOUT - 1),
        false => Ok(v as u64),
    };
    found.map(shape).transpose()
}

/// Whether `conn`'s database holds a table. One that holds none is a
/// projection its maker has connected to and not made yet.
pub(crate) fn made(conn: &Connection) -> Result<bool> {
    one(conn, "SELECT 1 FROM sqlite_master LIMIT 1", []).map(|t: Option<i64>| t.is_some())
}

/// The statement that marks a projection with this build's layout, as
/// it is made or cleared.
pub(crate) fn mark() -> String {
    format!("INSERT INTO meta (k, v) VALUES ('layout', {LAYOUT}) ON CONFLICT (k) DO NOTHING;")
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
    run(
        conn,
        "INSERT INTO meta (k, v) VALUES (?1, (SELECT v FROM meta WHERE k = 'folded')) \
         ON CONFLICT (k) DO NOTHING",
        [engines::CHOSEN_THROUGH],
    )?;
    run(
        conn,
        "INSERT INTO meta (k, v) VALUES (?1, ?2) ON CONFLICT (k) DO UPDATE SET v = ?2",
        params![engines::chosen_mark(ty), text],
    )
}

/// One step of choosing an order index for `ty` (`ProjectionStore::
/// choose_order`), in a transaction of its own that moves no position.
pub(crate) fn choose_order(
    conn: &Connection,
    ty: &TypeName,
    step: &Choose,
) -> Result<Option<RecordKey>> {
    let Some(decl) = type_at(conn, ty, None)? else {
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
    if !named.is_empty() || decl.view.is_some() || matches!(step, Choose::Start(f) if !sortable(f))
    {
        return Ok(None);
    }
    // Nothing is chosen on a lake a writer that keeps no chosen field
    // folded past, before a writer's open drops what it left.
    let mut chosen = chosen_of(conn, ty)?;
    if !chosen_held(conn)? || !engines::chosen_after(&mut chosen, step) {
        return Ok(None);
    }
    conn.execute_batch("BEGIN IMMEDIATE").map_err(sql)?;
    let done = (|| match step {
        Choose::Fill {
            field,
            after,
            limit,
        } => fill_chosen(conn, ty, field, after.as_ref(), *limit),
        _ => {
            set_chosen(conn, ty, &chosen)?;
            match step {
                Choose::Start(_) if sorted(conn, ty)?.is_none() => {
                    set_sorted(conn, ty, folded(conn)?)?;
                }
                Choose::Drop(f) => {
                    run(
                        conn,
                        "DELETE FROM sort_index WHERE ty = ?1 AND field = ?2",
                        params![ty.as_str(), f.as_str()],
                    )?;
                    if sorted_fields(conn, ty, Some(&decl), true)?.is_empty() {
                        set_sorted(conn, ty, None)?;
                    }
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

/// Write the rows of `field` for up to `limit` head records of `ty` after
/// `after`; the key the chunk ended at, none once the type is filled.
fn fill_chosen(
    conn: &Connection,
    ty: &TypeName,
    field: &FieldName,
    after: Option<&RecordKey>,
    limit: u32,
) -> Result<Option<RecordKey>> {
    let after = after.map_or("", RecordKey::as_str);
    let page = records(
        conn,
        "SELECT ty, key, since, state FROM records WHERE ty = ?1 AND until IS NULL AND key > ?2 \
         ORDER BY key LIMIT ?3",
        params![ty.as_str(), after, limit],
    )?;
    let mut last = None;
    for r in &page {
        run(
            conn,
            "DELETE FROM sort_index WHERE ty = ?1 AND field = ?2 AND key = ?3",
            params![ty.as_str(), field.as_str(), r.key.as_str()],
        )?;
        for (field, sort_key) in engines::order_rows(std::slice::from_ref(field), r) {
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

/// Rewrite `ty`'s order index from its head records, under its setting and
/// declaration at the head, and mark it kept through `through`; with no
/// field to keep, drop its rows and its mark. The fold does this when it
/// puts the setting's head state, and a writer's open when the mark is
/// behind.
pub(crate) fn rebuild_sorted(conn: &Connection, ty: &TypeName, through: Position) -> Result<()> {
    run(conn, "DELETE FROM sort_index WHERE ty = ?1", [ty.as_str()])?;
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
    if named.is_some_and(|n| !n.is_empty()) {
        set_chosen(conn, ty, &Chosen::default())?;
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
            "SELECT ty, key, since, state FROM records WHERE ty = ?1 AND until IS NULL \
             AND key > ?2 ORDER BY key LIMIT 1024",
            params![ty.as_str(), after],
        )?;
        for r in &page {
            for (field, sort_key) in engines::order_rows(&fields, r) {
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
        "SELECT substr(key, ?1) FROM records \
         WHERE ty = ?2 AND substr(key, 1, ?1 - 1) = ?3 AND until IS NULL",
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

/// The latest state of one record.
pub(crate) fn head_record(
    conn: &Connection,
    ty: &TypeName,
    key: &RecordKey,
) -> Result<Option<StoredRecord>> {
    let head = "SELECT ty, key, since, state FROM records WHERE ty = ?1 AND key = ?2 \
                AND until IS NULL";
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
    for (field, at) in apart {
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
/// (`None`: every field); a record `fresh` to the projection drops none.
pub(crate) fn index_record(
    conn: &Connection,
    (decl, sorted, looked): (
        Option<&TypeDecl>,
        &[FieldName],
        Option<&BTreeSet<FieldName>>,
    ),
    fresh: bool,
    r: &StoredRecord,
) -> Result<()> {
    let (ty, key) = (r.ty.as_str(), r.key.as_str());
    let drop = |sql_text: &str| {
        (conn.prepare_cached(sql_text))
            .and_then(|mut s| s.execute(params![ty, key]))
            .map_err(sql)
    };
    if !fresh {
        drop("DELETE FROM text_index WHERE ty = ?1 AND key = ?2")?;
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
        if let Some(text) = value.as_str() {
            conn.prepare_cached(
                "INSERT OR IGNORE INTO text_index (ty, field, value, key) VALUES (?1, ?2, ?3, ?4)",
            )
            .and_then(|mut s| s.execute(params![ty, field.as_str(), text, key]))
            .map_err(sql)?;
        }
    }
    if !fresh {
        drop(
            "DELETE FROM full_text WHERE rowid = \
             (SELECT id FROM search_rows WHERE ty = ?1 AND key = ?2)",
        )?;
    }
    for (field, sort_key) in engines::order_rows(sorted, r) {
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
pub(crate) fn put_record(conn: &Connection, kept: &mut Kept, r: &StoredRecord) -> Result<()> {
    // A head-only name keeps one state for each key and no row beside it.
    if lakelands_api::seam::is_head_only(&r.ty) {
        let state = engines::record_state(r, &BTreeMap::new());
        let key = [r.ty.as_str(), r.key.as_str()];
        (conn.prepare_cached("DELETE FROM records WHERE ty = ?1 AND key = ?2"))
            .and_then(|mut s| s.execute(key))
            .map_err(sql)?;
        if r.is_ended() {
            return Ok(());
        }
        return (conn.prepare_cached(
            "INSERT INTO records (ty, key, since, state) VALUES (?1, ?2, ?3, ?4)",
        ))
        .and_then(|mut s| s.execute(params![key[0], key[1], r.position.0 as i64, state]))
        .map(drop)
        .map_err(sql);
    }
    let later: Option<i64> = one(
        conn,
        "SELECT 1 FROM records WHERE ty = ?1 AND key = ?2 AND since > ?3 LIMIT 1",
        params![r.ty.as_str(), r.key.as_str(), r.position.0 as i64],
    )?;
    if !kept.contains_key(&r.ty) {
        kept.insert(r.ty.clone(), indexed(conn, &r.ty)?);
    }
    let apart = put_vectors(conn, r, later.is_some(), &kept[&r.ty].3)?;
    let state = engines::record_state(r, &apart);
    let held = RECORDS.put(conn, &[&r.ty.as_str(), &r.key.as_str()], r.position, &state)?;
    if later.is_none() {
        let (decl, sorted, looked, _) = &kept[&r.ty];
        index_record(conn, (decl.as_ref(), sorted, looked.as_ref()), !held, r)?;
        // The setting's write rebuilds the index it asks for, so the index
        // describes the head from the next read on.
        if let Some(ty) = engines::sort_index_type(r) {
            for ty in engines::ordered_names(&ty) {
                rebuild_sorted(conn, &ty, r.position)?;
            }
            kept.clear();
        }
    }
    Ok(())
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
    for statement in [
        "DELETE FROM records WHERE ty = ?1 AND key = ?2",
        "DELETE FROM vectors WHERE ty = ?1 AND key = ?2",
        "DELETE FROM members WHERE object_set IN (SELECT DISTINCT substr(ident, 12) \
         FROM declarations WHERE ident LIKE 'object\\_set %' ESCAPE '\\') AND ty = ?1 AND key = ?2",
        "DELETE FROM text_index WHERE ty = ?1 AND key = ?2",
        "DELETE FROM edges WHERE src_ty = ?1 AND src_key = ?2",
        "DELETE FROM sort_index WHERE ty = ?1 AND key = ?2",
        "DELETE FROM full_text WHERE rowid IN (SELECT id FROM search_rows WHERE ty = ?1 AND key = ?2)",
        "DELETE FROM search_rows WHERE ty = ?1 AND key = ?2",
    ] {
        (conn.prepare_cached(statement))
            .and_then(|mut s| s.execute(params![ty.as_str(), key.as_str()]))
            .map_err(sql)?;
    }
    Ok(())
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
    let sorted = sorted_fields(conn, ty, decl.as_ref(), true)?;
    let vectors = engines::vector_fields(ty, |t| match t == ty {
        true => Ok(decl.clone()),
        false => type_at(conn, t, None),
    })?;
    let looked = match decl {
        Some(_) => Some(looked_fields(conn, ty)?),
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
        let statements = match comes {
            true => [
                "INSERT OR IGNORE INTO text_index (ty, field, value, key) \
                 SELECT ty, ?2, json_extract(state, '$.fields.' || ?2), key FROM records \
                 WHERE ty = ?1 AND until IS NULL AND json_type(state, '$.fields.' || ?2) = 'text'",
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
            "SELECT ty, key, since, state FROM records WHERE ty = ?1 AND until IS NULL \
             AND since < ?2 AND key > ?3 ORDER BY key LIMIT 1024",
            params![ty.as_str(), at.0 as i64, after],
        )?;
        for r in older.iter().filter(|r| !r.is_ended()) {
            index_record(conn, (head.as_ref(), &sorted, looked.as_ref()), false, r)?;
        }
        match older.last() {
            Some(last) if older.len() == 1024 => after = last.key.to_string(),
            _ => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests;
