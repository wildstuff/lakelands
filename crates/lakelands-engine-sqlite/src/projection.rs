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

use lakelands_api::engine::Declared;
use lakelands_api::error::Result;
use lakelands_api::id::{Position, RecordKey, TypeName};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::record::StoredRecord;
use lakelands_api::write::Declaration;
use lakelands_core::engines;
use rusqlite::{Connection, OptionalExtension, ToSql, params};

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

pub(crate) const DECLARATIONS: History = History {
    table: "declarations",
    ident: &["ident"],
};
pub(crate) const RECORDS: History = History {
    table: "records",
    ident: &["ty", "key"],
};
pub(crate) const PROPOSALS: History = History {
    table: "proposals",
    ident: &["id"],
};
pub(crate) const MEMBERS: History = History {
    table: "members",
    ident: &["object_set", "ty", "key"],
};
pub(crate) const OUTPUTS: History = History {
    table: "outputs",
    ident: &["seam", "input"],
};

impl History {
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

/// The position the projection reflects, if any.
pub(crate) fn folded(conn: &Connection) -> Result<Option<Position>> {
    conn.prepare_cached("SELECT v FROM meta WHERE k = 'folded'")
        .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)).optional())
        .map(|v| v.map(|v| Position(v as u64)))
        .map_err(sql)
}

/// The position the order index was last kept through, if any. It
/// describes the head only when it is the position folded through: a
/// writer that does not keep the index leaves it behind.
pub(crate) fn sorted(conn: &Connection) -> Result<Option<Position>> {
    conn.prepare_cached("SELECT v FROM meta WHERE k = 'sorted'")
        .and_then(|mut s| s.query_row([], |r| r.get::<_, i64>(0)).optional())
        .map(|v| v.map(|v| Position(v as u64)))
        .map_err(sql)
}

/// Rebuild the order index from the head records when it does not
/// describe the head, in one transaction, and mark it kept through the
/// position folded through.
pub(crate) fn rebuild_sorted(conn: &Connection) -> Result<()> {
    let folded = folded(conn)?;
    if folded.is_none() || sorted(conn)? == folded {
        return Ok(());
    }
    conn.execute_batch("BEGIN IMMEDIATE; DELETE FROM sort_index;")
        .map_err(sql)?;
    let rebuilt =
        (|| {
            let types: Vec<String> = conn
                .prepare("SELECT DISTINCT ty FROM records WHERE until IS NULL")
                .and_then(|mut s| s.query_map([], |r| r.get(0))?.collect())
                .map_err(sql)?;
            for ty in types {
                let ty = TypeName::from(ty);
                let Some(decl) = type_at(conn, &ty, None)? else {
                    continue;
                };
                for r in head_records(conn, &ty)? {
                    for (field, sort_key) in engines::order_rows(&decl, &r) {
                        conn.prepare_cached(
                        "INSERT INTO sort_index (ty, field, key, sort_key) VALUES (?1, ?2, ?3, ?4)",
                    )
                    .and_then(|mut s| {
                        s.execute(params![ty.as_str(), field.as_str(), r.key.as_str(), sort_key])
                    })
                    .map_err(sql)?;
                    }
                }
            }
            mark_sorted(conn, folded.expect("checked above"))
        })();
    match rebuilt {
        Ok(()) => conn.execute_batch("COMMIT").map_err(sql),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

/// Record that the order index is kept through `through`.
pub(crate) fn mark_sorted(conn: &Connection, through: Position) -> Result<()> {
    conn.prepare_cached(
        "INSERT INTO meta (k, v) VALUES ('sorted', ?1) ON CONFLICT (k) DO UPDATE SET v = ?1",
    )
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

/// The latest states of one type's records.
pub(crate) fn head_records(conn: &Connection, ty: &TypeName) -> Result<Vec<StoredRecord>> {
    let mut stmt = conn
        .prepare_cached("SELECT state FROM records WHERE ty = ?1 AND until IS NULL ORDER BY key")
        .map_err(sql)?;
    let rows = stmt
        .query_map(params![ty.as_str()], |r| r.get::<_, String>(0))
        .map_err(sql)?;
    rows.map(|r| decode_record(&r.map_err(sql)?)).collect()
}

/// The latest state of one record.
pub(crate) fn head_record(
    conn: &Connection,
    ty: &TypeName,
    key: &RecordKey,
) -> Result<Option<StoredRecord>> {
    let state: Option<String> = conn
        .prepare_cached("SELECT state FROM records WHERE ty = ?1 AND key = ?2 AND until IS NULL")
        .and_then(|mut s| {
            s.query_row(params![ty.as_str(), key.as_str()], |r| r.get(0))
                .optional()
        })
        .map_err(sql)?;
    state.as_deref().map(decode_record).transpose()
}

pub(crate) fn decode_record(state: &str) -> Result<StoredRecord> {
    serde_json::from_str(state).map_err(|e| {
        crate::unavailable(format!("the projection holds a record it cannot read: {e}"))
    })
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
    r: &StoredRecord,
) -> Result<()> {
    let (ty, key) = (r.ty.as_str(), r.key.as_str());
    conn.prepare_cached("DELETE FROM text_index WHERE ty = ?1 AND key = ?2")
        .and_then(|mut s| s.execute(params![ty, key]))
        .map_err(sql)?;
    conn.prepare_cached("DELETE FROM edges WHERE src_ty = ?1 AND src_key = ?2")
        .and_then(|mut s| s.execute(params![ty, key]))
        .map_err(sql)?;
    conn.prepare_cached("DELETE FROM sort_index WHERE ty = ?1 AND key = ?2")
        .and_then(|mut s| s.execute(params![ty, key]))
        .map_err(sql)?;
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
    for (field, sort_key) in engines::order_rows(decl, r) {
        conn.prepare_cached(
            "INSERT INTO sort_index (ty, field, key, sort_key) VALUES (?1, ?2, ?3, ?4)",
        )
        .and_then(|mut s| s.execute(params![ty, field.as_str(), key, sort_key]))
        .map_err(sql)?;
    }
    for e in engines::links_of(decl, r, None) {
        conn.prepare_cached(
            "INSERT INTO edges (src_ty, src_key, relation, dst_ty, dst_key) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .and_then(|mut s| {
            s.execute(params![
                ty,
                key,
                e.relation.as_str(),
                e.to.0.as_str(),
                e.to.1.as_str()
            ])
        })
        .map_err(sql)?;
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

/// Put a record's state and, when it is the latest, its head indexes.
pub(crate) fn put_record(conn: &Connection, r: &StoredRecord) -> Result<()> {
    let state = serde_json::to_string(r).expect("a record serialises");
    RECORDS.put(conn, &[&r.ty.as_str(), &r.key.as_str()], r.position, &state)?;
    if is_head(conn, &r.ty, &r.key, r.position)? {
        let decl = type_at(conn, &r.ty, None)?;
        index_record(conn, decl.as_ref(), r)?;
    }
    Ok(())
}

/// Put a declaration and, for a type now in force at the head, rewrite
/// the head indexes of its records under it.
pub(crate) fn put_declaration(conn: &Connection, d: &Declared) -> Result<()> {
    let ident = engines::declaration_key(&d.declaration);
    let state = serde_json::to_string(d).expect("a declaration serialises");
    DECLARATIONS.put(conn, &[&ident], d.since, &state)?;
    if let Declaration::Type(t) = &d.declaration {
        let head = type_at(conn, &t.name, None)?;
        if head.as_ref() == Some(t) {
            for r in head_records(conn, &t.name)? {
                index_record(conn, head.as_ref(), &r)?;
            }
        }
    }
    Ok(())
}
