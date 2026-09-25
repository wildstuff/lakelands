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

use lakelands_api::engine::Declared;
use lakelands_api::error::Result;
use lakelands_api::id::{Position, RecordKey, TypeName};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::record::StoredRecord;
use lakelands_api::write::Declaration;
use lakelands_core::engines;
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
    pub(crate) fn schema_sql(&self) -> String {
        let s = &self.schema;
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
CREATE INDEX IF NOT EXISTS records_key_c ON {s}.records (ty, key COLLATE \"C\", since);
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
    ty TEXT NOT NULL, field TEXT NOT NULL, value TEXT NOT NULL, key TEXT NOT NULL,
    PRIMARY KEY (ty, field, value, key)
);
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
CREATE TABLE IF NOT EXISTS {s}.vectors (
    ty TEXT NOT NULL, field TEXT NOT NULL, key TEXT NOT NULL, data BYTEA NOT NULL,
    PRIMARY KEY (ty, field, key)
);
"
        )
    }
}

/// Every table of a lake's schema.
pub(crate) const LAKE_TABLES: [&str; 14] = [
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
    "vectors",
];

/// The projection's tables, cleared by a rebuild; the log, its segments,
/// the lease and the lake's name stay.
pub(crate) const PROJECTION_TABLES: [&str; 10] = [
    "declarations",
    "records",
    "proposals",
    "members",
    "outputs",
    "applied",
    "text_index",
    "edges",
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
pub(crate) const OUTPUTS: History = History {
    table: "outputs",
    ident: &["seam", "input"],
};
pub(crate) const MEMBERS: History = History {
    table: "members",
    ident: &["object_set", "ty", "key"],
};

impl History {
    fn ident_where(&self, from: usize) -> String {
        self.ident
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{c} = ${}", i + from))
            .collect::<Vec<_>>()
            .join(" AND ")
    }

    /// Put a state of one thing valid from `since`, keeping every other
    /// state: the one it supersedes ends at `since`, and it ends where the
    /// next one, if any, begins.
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
        let mut replace = args.clone();
        replace.push(state);
        let replaced = db.execute(
            &format!(
                "UPDATE {table} SET state = ${} WHERE {} AND since = ${}",
                n + 2,
                self.ident_where(1),
                n + 1
            ),
            &replace,
        )?;
        if replaced > 0 {
            return Ok(());
        }
        let next: Option<i64> = db
            .query_one(
                &format!(
                    "SELECT min(since) FROM {table} WHERE {} AND since > ${}",
                    self.ident_where(1),
                    n + 1
                ),
                &args,
            )?
            .get(0);
        db.execute(
            &format!(
                "UPDATE {table} SET until = ${n1} WHERE {} AND since < ${n1} \
                 AND (until IS NULL OR until > ${n1})",
                self.ident_where(1),
                n1 = n + 1
            ),
            &args,
        )?;
        let columns = self.ident.join(", ");
        let marks: Vec<String> = (1..=n + 3).map(|i| format!("${i}")).collect();
        let mut insert = args.clone();
        insert.push(&next);
        insert.push(state);
        db.execute(
            &format!(
                "INSERT INTO {table} ({columns}, since, until, state) VALUES ({})",
                marks.join(", ")
            ),
            &insert,
        )?;
        Ok(())
    }
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

pub(crate) fn set_folded(db: &mut Db, names: &Names, through: Position) -> Result<()> {
    db.execute(
        &format!(
            "INSERT INTO {} (k, n) VALUES ('folded', $1) ON CONFLICT (k) DO UPDATE SET n = $1",
            names.t("meta")
        ),
        &[&(through.0 as i64)],
    )?;
    Ok(())
}

pub(crate) fn decode<T: serde::de::DeserializeOwned>(state: &str, what: &str) -> Result<T> {
    serde_json::from_str(state)
        .map_err(|e| crate::unavailable(format!("the projection holds {what} it cannot read: {e}")))
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
    let Some(row) = row else { return Ok(None) };
    let declared: Declared = decode(&row.get::<_, String>(0), "a declaration")?;
    Ok(match declared.declaration {
        Declaration::Type(t) => Some(t),
        _ => None,
    })
}

/// The latest states of one type's records, in key order.
pub(crate) fn head_records(db: &mut Db, names: &Names, ty: &TypeName) -> Result<Vec<StoredRecord>> {
    db.query(
        &format!(
            "SELECT state FROM {} WHERE ty = $1 AND until IS NULL ORDER BY key COLLATE \"C\"",
            names.t("records")
        ),
        &[&ty.as_str()],
    )?
    .iter()
    .map(|r| decode_record(&r.get::<_, String>(0)))
    .collect()
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

/// Whether a record's state put at `position` is its latest.
fn is_head(
    db: &mut Db,
    names: &Names,
    ty: &TypeName,
    key: &RecordKey,
    position: Position,
) -> Result<bool> {
    Ok(db
        .query_one(
            &format!(
                "SELECT until IS NULL FROM {} WHERE ty = $1 AND key = $2 AND since = $3",
                names.t("records")
            ),
            &[&ty.as_str(), &key.as_str(), &(position.0 as i64)],
        )?
        .get(0))
}

/// Rewrite the head indexes of one record from its latest state, under
/// the declaration of its type at the head.
pub(crate) fn index_record(
    db: &mut Db,
    names: &Names,
    decl: Option<&TypeDecl>,
    r: &StoredRecord,
) -> Result<()> {
    let (ty, key) = (r.ty.as_str(), r.key.as_str());
    for table in ["text_index", "full_text"] {
        db.execute(
            &format!("DELETE FROM {} WHERE ty = $1 AND key = $2", names.t(table)),
            &[&ty, &key],
        )?;
    }
    db.execute(
        &format!(
            "DELETE FROM {} WHERE src_ty = $1 AND src_key = $2",
            names.t("edges")
        ),
        &[&ty, &key],
    )?;
    for (field, value) in &r.fields {
        if let Some(text) = value.as_str() {
            db.execute(
                &format!(
                    "INSERT INTO {} (ty, field, value, key) VALUES ($1, $2, $3, $4) \
                     ON CONFLICT DO NOTHING",
                    names.t("text_index")
                ),
                &[&ty, &field.as_str(), &text, &key],
            )?;
        }
    }
    let Some(decl) = decl else {
        return Ok(());
    };
    for e in engines::links_of(decl, r, None) {
        db.execute(
            &format!(
                "INSERT INTO {} (src_ty, src_key, relation, dst_ty, dst_key) \
                 VALUES ($1, $2, $3, $4, $5)",
                names.t("edges")
            ),
            &[
                &ty,
                &key,
                &e.relation.as_str(),
                &e.to.0.as_str(),
                &e.to.1.as_str(),
            ],
        )?;
    }
    // Every head state is indexed, whatever its flags: a view at an
    // earlier position may keep a record the head has retired.
    let tokens: Vec<String> = engines::record_tokens(decl, r).into_iter().collect();
    if !tokens.is_empty() {
        db.execute(
            &format!(
                "INSERT INTO {} (ty, token, key) SELECT $1, t, $2 FROM unnest($3::text[]) AS t",
                names.t("full_text")
            ),
            &[&ty, &key, &tokens],
        )?;
    }
    Ok(())
}

/// Put a record's state and, when it is the latest, its head indexes.
pub(crate) fn put_record(db: &mut Db, names: &Names, r: &StoredRecord) -> Result<()> {
    let state = serde_json::to_string(r).expect("a record serialises");
    RECORDS.put(
        db,
        names,
        &[&r.ty.as_str(), &r.key.as_str()],
        r.position,
        &state,
    )?;
    if is_head(db, names, &r.ty, &r.key, r.position)? {
        let decl = type_at(db, names, &r.ty, None)?;
        index_record(db, names, decl.as_ref(), r)?;
    }
    Ok(())
}

/// Put a declaration and, for a type now in force at the head, rewrite
/// the head indexes of its records under it.
pub(crate) fn put_declaration(db: &mut Db, names: &Names, d: &Declared) -> Result<()> {
    let ident = engines::declaration_key(&d.declaration);
    let state = serde_json::to_string(d).expect("a declaration serialises");
    DECLARATIONS.put(db, names, &[&ident], d.since, &state)?;
    if let Declaration::Type(t) = &d.declaration {
        let head = type_at(db, names, &t.name, None)?;
        if head.as_ref() == Some(t) {
            for r in head_records(db, names, &t.name)? {
                index_record(db, names, head.as_ref(), &r)?;
            }
        }
    }
    Ok(())
}
