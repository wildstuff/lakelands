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
//! of `via` relations, the full-text entries of searchable fields, and the
//! vectors. They are rewritten in the fold's own transaction, from the
//! record and the declaration in force, so they always describe the head.
//! A view uses them only when it reads the head, and only to find
//! candidates: whether a candidate matches, where it sorts and what it
//! scores is decided by `lakelands_core::engines`, the rules every engine
//! shares, so no index can change an answer.
//!
//! **Transactions.** The fold writes between `begin` and `commit`, in one
//! SQLite transaction on the writer's connection; `commit` records the
//! position folded through in the same transaction. A crash or a rollback
//! leaves the projection at a whole append.

use std::collections::BTreeSet;

use lakelands_api::engine::Declared;
use lakelands_api::error::Result;
use lakelands_api::id::{FieldName, Position, RecordKey, TypeName};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::{SETTING_TYPE, SORT_INDEX_SETTING};
use lakelands_api::write::Declaration;
use lakelands_core::engines::{self, sorted_mark};
use rusqlite::types::FromSql;
use rusqlite::{Connection, OptionalExtension, Params, ToSql, params};

use crate::sql;

/// The schema. `state` columns hold the state as JSON.
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
CREATE INDEX IF NOT EXISTS records_since ON records (ty, since);
CREATE TABLE IF NOT EXISTS proposals (
    id TEXT NOT NULL, since INTEGER NOT NULL, until INTEGER,
    state TEXT NOT NULL,
    PRIMARY KEY (id, since)
);
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
);
CREATE TABLE IF NOT EXISTS text_index (
    ty TEXT NOT NULL, field TEXT NOT NULL, value TEXT NOT NULL, key TEXT NOT NULL,
    PRIMARY KEY (ty, field, value, key)
);
CREATE INDEX IF NOT EXISTS text_index_key ON text_index (ty, key);
CREATE TABLE IF NOT EXISTS edges (
    src_ty TEXT NOT NULL, src_key TEXT NOT NULL, relation TEXT NOT NULL,
    dst_ty TEXT NOT NULL, dst_key TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS edges_src ON edges (src_ty, src_key);
CREATE INDEX IF NOT EXISTS edges_dst ON edges (dst_ty, dst_key);
CREATE TABLE IF NOT EXISTS sort_index (
    ty TEXT NOT NULL, field TEXT NOT NULL, key TEXT NOT NULL, sort_key BLOB NOT NULL,
    PRIMARY KEY (ty, key, field)
);
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
    ty TEXT NOT NULL, field TEXT NOT NULL, key TEXT NOT NULL,
    data BLOB NOT NULL,
    PRIMARY KEY (ty, field, key)
);
";

/// Every table the projection holds, cleared by a rebuild.
pub(crate) const TABLES: [&str; 13] = [
    "meta",
    "declarations",
    "records",
    "proposals",
    "members",
    "outputs",
    "applied",
    "text_index",
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

    /// Put a state of one thing valid from `since`, keeping every other
    /// state: the one it supersedes ends at `since`, and it ends where the
    /// next one, if any, begins.
    pub(crate) fn put(
        &self,
        conn: &Connection,
        ident: &[&dyn ToSql],
        since: Position,
        state: &dyn ToSql,
    ) -> Result<()> {
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
            return Ok(());
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
        conn.prepare_cached(&format!(
            "UPDATE {} SET until = ?{n1} WHERE {} AND since < ?{n1} \
             AND (until IS NULL OR until > ?{n1})",
            self.table,
            self.ident_where(1),
            n1 = n + 1
        ))
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
        Ok(())
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
/// `sort_index.<type>` setting at the head names that `decl` declares
/// with a kind the index keeps.
pub(crate) fn sorted_fields(
    conn: &Connection,
    ty: &TypeName,
    decl: Option<&TypeDecl>,
) -> Result<Vec<FieldName>> {
    let Some(decl) = decl else {
        return Ok(Vec::new());
    };
    let setting = head_record(
        conn,
        &TypeName::from(SETTING_TYPE),
        &RecordKey::from(engines::sort_index_key(ty)),
    )?;
    Ok(engines::sorted_fields(
        decl,
        setting.as_ref().and_then(engines::setting_value),
    ))
}

/// Rewrite `ty`'s order index from its head records, under its setting and
/// declaration at the head, and mark it kept through `through`; with no
/// field to keep, drop its rows and its mark. The fold does this when it
/// puts the setting's head state, and a writer's open when the mark is
/// behind.
pub(crate) fn rebuild_sorted(conn: &Connection, ty: &TypeName, through: Position) -> Result<()> {
    run(conn, "DELETE FROM sort_index WHERE ty = ?1", [ty.as_str()])?;
    let decl = type_at(conn, ty, None)?;
    let fields = sorted_fields(conn, ty, decl.as_ref())?;
    if fields.is_empty() {
        return set_sorted(conn, ty, None);
    }
    // A page of the type at a time, so the pass holds a page, not the type.
    let mut after = String::new();
    loop {
        let page: Vec<String> = column(
            conn,
            "SELECT state FROM records WHERE ty = ?1 AND until IS NULL AND key > ?2 \
             ORDER BY key LIMIT 1024",
            params![ty.as_str(), after],
            Ok,
        )?;
        for state in &page {
            let r = decode_record(state)?;
            for (field, sort_key) in engines::order_rows(&fields, &r) {
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
/// drop the index of a type whose setting such a writer removed.
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
    let behind: BTreeSet<String> = asked
        .into_iter()
        .filter(|ty| !marks.iter().any(|(t, v)| t == ty && *v as u64 == folded.0))
        .chain(
            marks
                .iter()
                .filter(|(_, v)| *v as u64 != folded.0)
                .map(|(t, _)| t.clone()),
        )
        .collect();
    if behind.is_empty() {
        return Ok(());
    }
    conn.execute_batch("BEGIN IMMEDIATE").map_err(sql)?;
    let rebuilt = behind
        .iter()
        .try_for_each(|ty| rebuild_sorted(conn, &TypeName::from(ty.as_str()), folded));
    match rebuilt {
        Ok(()) => conn.execute_batch("COMMIT").map_err(sql),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// Record that every order index the lake keeps is kept through
/// `through`: the fold kept each with the head.
pub(crate) fn mark_sorted(conn: &Connection, through: Position) -> Result<()> {
    conn.prepare_cached("UPDATE meta SET v = ?1 WHERE k LIKE 'sorted.%'")
        .and_then(|mut s| s.execute(params![through.0 as i64]))
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
    let state: Option<String> = one(
        conn,
        "SELECT state FROM records WHERE ty = ?1 AND key = ?2 AND until IS NULL",
        params![ty.as_str(), key.as_str()],
    )?;
    state.as_deref().map(decode_record).transpose()
}

pub(crate) fn decode_record(state: &str) -> Result<StoredRecord> {
    engines::decode(state, "a record")
}

/// Whether a record's state put at `position` is its latest.
fn is_head(conn: &Connection, ty: &TypeName, key: &RecordKey, position: Position) -> Result<bool> {
    conn.prepare_cached(
        "SELECT until IS NULL FROM records WHERE ty = ?1 AND key = ?2 AND since = ?3",
    )
    .and_then(|mut s| {
        s.query_row(params![ty.as_str(), key.as_str(), position.0 as i64], |r| {
            r.get(0)
        })
    })
    .map_err(sql)
}

/// Rewrite the head indexes of one record from its latest state, under
/// the declaration of its type at the head.
pub(crate) fn index_record(
    conn: &Connection,
    decl: Option<&TypeDecl>,
    sorted: &[FieldName],
    r: &StoredRecord,
) -> Result<()> {
    let (ty, key) = (r.ty.as_str(), r.key.as_str());
    conn.prepare_cached("DELETE FROM text_index WHERE ty = ?1 AND key = ?2")
        .and_then(|mut s| s.execute(params![ty, key]))
        .map_err(sql)?;
    conn.prepare_cached("DELETE FROM edges WHERE src_ty = ?1 AND src_key = ?2")
        .and_then(|mut s| s.execute(params![ty, key]))
        .map_err(sql)?;
    // Only a type the lake keeps an order index for has rows in it: the
    // setting's rebuild and a redeclaration drop a type's rows whole.
    if !sorted.is_empty() {
        conn.prepare_cached("DELETE FROM sort_index WHERE ty = ?1 AND key = ?2")
            .and_then(|mut s| s.execute(params![ty, key]))
            .map_err(sql)?;
    }
    for (field, value) in &r.fields {
        if let Some(text) = value.as_str() {
            conn.prepare_cached(
                "INSERT OR IGNORE INTO text_index (ty, field, value, key) VALUES (?1, ?2, ?3, ?4)",
            )
            .and_then(|mut s| s.execute(params![ty, field.as_str(), text, key]))
            .map_err(sql)?;
        }
    }
    let row: i64 = {
        conn.prepare_cached("INSERT OR IGNORE INTO search_rows (ty, key) VALUES (?1, ?2)")
            .and_then(|mut s| s.execute(params![ty, key]))
            .map_err(sql)?;
        conn.prepare_cached("SELECT id FROM search_rows WHERE ty = ?1 AND key = ?2")
            .and_then(|mut s| s.query_row(params![ty, key], |r| r.get(0)))
            .map_err(sql)?
    };
    conn.prepare_cached("DELETE FROM full_text WHERE rowid = ?1")
        .and_then(|mut s| s.execute(params![row]))
        .map_err(sql)?;
    let Some(decl) = decl else {
        return Ok(());
    };
    for (field, sort_key) in engines::order_rows(sorted, r) {
        conn.prepare_cached(
            "INSERT INTO sort_index (ty, field, key, sort_key) VALUES (?1, ?2, ?3, ?4)",
        )
        .and_then(|mut s| s.execute(params![ty, field.as_str(), key, sort_key]))
        .map_err(sql)?;
    }
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
    // Every head state is indexed, whatever its flags: a view at an
    // earlier position may keep a record the head has retired.
    let tokens = engines::record_tokens(decl, r);
    if !tokens.is_empty() {
        let doc = tokens
            .iter()
            .map(|t| fts_term(t))
            .collect::<Vec<_>>()
            .join(" ");
        conn.prepare_cached("INSERT INTO full_text (rowid, tokens) VALUES (?1, ?2)")
            .and_then(|mut s| s.execute(params![row, doc]))
            .map_err(sql)?;
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

/// A type as a record of it is indexed: its declaration at the head and
/// the fields its order index keeps.
pub(crate) type Kept = (TypeName, Option<TypeDecl>, Vec<FieldName>);

/// Put a record's state and, when it is the latest, its head indexes. Its
/// type is read into `kept` and not again for the next record of the same
/// type. A record of another type replaces what is kept; a
/// `sort_index.<type>` setting forgets it, since the type it names may be
/// the settings' own; the caller forgets it at a declaration.
pub(crate) fn put_record(
    conn: &Connection,
    kept: &mut Option<Kept>,
    r: &StoredRecord,
) -> Result<()> {
    let state = serde_json::to_string(r).expect("a record serialises");
    RECORDS.put(conn, &[&r.ty.as_str(), &r.key.as_str()], r.position, &state)?;
    if is_head(conn, &r.ty, &r.key, r.position)? {
        if kept.as_ref().is_none_or(|k| k.0 != r.ty) {
            let decl = type_at(conn, &r.ty, None)?;
            let sorted = sorted_fields(conn, &r.ty, decl.as_ref())?;
            *kept = Some((r.ty.clone(), decl, sorted));
        }
        let (_, decl, sorted) = kept.as_ref().expect("kept above");
        index_record(conn, decl.as_ref(), sorted, r)?;
        // The setting's write rebuilds the index it asks for, so the index
        // describes the head from the next read on.
        if let Some(ty) = engines::sort_index_type(r) {
            rebuild_sorted(conn, &ty, r.position)?;
            *kept = None;
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
    let kept = !sorted_fields(conn, &t.name, Some(t))?.is_empty();
    set_sorted(conn, &t.name, kept.then_some(d.since))?;
    Ok(Some(t.name.clone()))
}

/// Rewrite the head indexes of the records of `ty` whose latest state is
/// older than `at`, under the type's declaration at the head. A record put
/// at `at` or after was indexed under that declaration when it was put: a
/// migration puts every record again, and nothing is left to do here.
pub(crate) fn index_older(conn: &Connection, ty: &TypeName, at: Position) -> Result<()> {
    let head = type_at(conn, ty, None)?;
    let sorted = sorted_fields(conn, ty, head.as_ref())?;
    // A page of the type at a time; an ended key has no index rows.
    let mut after = String::new();
    loop {
        let older: Vec<StoredRecord> = column(
            conn,
            "SELECT state FROM records WHERE ty = ?1 AND until IS NULL AND since < ?2 \
             AND key > ?3 ORDER BY key LIMIT 1024",
            params![ty.as_str(), at.0 as i64, after],
            |s: String| decode_record(&s),
        )?;
        for r in older.iter().filter(|r| !r.is_ended()) {
            index_record(conn, head.as_ref(), &sorted, r)?;
        }
        match older.last() {
            Some(last) if older.len() == 1024 => after = last.key.to_string(),
            _ => return Ok(()),
        }
    }
}
