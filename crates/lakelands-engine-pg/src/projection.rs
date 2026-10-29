//! The projection in PostgreSQL, with its kept history.
//!
//! **History.** Every state the fold puts is a row from its position
//! (`since`) on, written once and never changed: a state holds until the
//! next one of the same thing begins, and an ended record is a state too,
//! stored as the seam hands it (version 0): the seam reads an end as a
//! state, so no read rebuilds one.
//! A view at a position reads, for each record, declaration, proposal,
//! membership and recorded output, the latest row at or before it; nothing
//! is replayed. A state put again at the same position replaces the row
//! there, which is the same state folded twice.
//!
//! **Heads.** Beside the declarations and the records, the two histories
//! read at the head, `<table>_head` holds one narrow row per thing: the
//! position of its latest state. Proposals, members and outputs are read
//! only by their states in force at a position, and keep no heads. A read at the head joins it
//! to its state. Only `since` changes in it, a column no index holds, and
//! its pages are left room (`fillfactor`), so moving a head is an update in
//! place, without a new index entry; superseding a state writes one row
//! and moves one head, where an end written onto the superseded row wrote
//! the whole state twice and every index entry of both.
//!
//! **Head indexes.** Beside the history the engine keeps indexes of the
//! latest states only: exact text values for equality lookups, the links
//! of `via` relations, and one row per full-text token of a record. They
//! are rewritten in the fold's own transaction, from the
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
//! **Vectors.** A vector's elements are kept once, in a row of `vectors`
//! written by the state that changed them, which every later state naming
//! it shares; a row ends (`until`) where its record's state stops naming
//! it, or is retired, so the rows in force are the head's, which a ranking
//! reads and the vector index holds
//! (`docs/decisions/072-a-vector-is-stored-once.md`). A read joins the rows
//! its states name, in one statement for a batch of them ([`joined`]).
//!
//! **Recorded outputs** keep their history too: a later output for the
//! same call (a failed call made again after its function was declared
//! again) supersedes the earlier from its own position on, so the head
//! answers the latest and a view at an earlier position answers what was
//! recorded by then.
//!
//! Every table is named with the lake's schema ([`Names`]).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use lakelands_api::engine::{Applied, Choose, Chosen, Declared, TextCounts};
use lakelands_api::error::Result;
use lakelands_api::id::{FieldName, Position, RecordKey, TypeName};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::quarantine::QuarantineId;
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::{SETTING_TYPE, SORT_INDEX_SETTING};
use lakelands_api::write::Declaration;
use lakelands_core::engines::{self, decode};
use postgres::Row;
use postgres::types::ToSql;

use crate::connect::{Db, Target};
use crate::vector_index::{self, Touched};

/// The lake's tables, each named with its schema.
#[derive(Clone, Debug)]
pub(crate) struct Names {
    schema: String,
    /// For a scratch, the lake's schema, which keeps the log's tables.
    log: Option<String>,
    /// Where the lake's parts are made.
    pub(crate) at: lakelands_api::location::PgTablespaces,
    /// Which of [`HEX_COLUMNS`] `log` keeps as their bytes, a bit each, as
    /// the catalogue said at open and after a conversion: shared by every
    /// copy, and a scratch's with its lake's.
    bytes: std::sync::Arc<std::sync::atomic::AtomicU8>,
}

/// The tables of the log and the lease, which a scratch reads in the
/// lake's schema.
const LOG_TABLES: [&str; 4] = ["lease", "segments", "log", "frames"];

/// The columns of `log` the store hands as hex digits: `bytea` from
/// 0.21.0 on, `text` in a log 0.20.0 or before made and not yet converted
/// (`crate::log::convert`).
pub(crate) const HEX_COLUMNS: [&str; 3] = ["id", "prev", "hash"];

impl Names {
    /// The tables of the lake `target` names.
    pub(crate) fn of(target: &Target) -> Result<Names> {
        let (schema, log, at) = (target.quoted_schema()?, None, target.tablespaces.clone());
        let bytes = Default::default();
        Ok(Names {
            schema,
            log,
            at,
            bytes,
        })
    }

    /// The tables of a scratch in the schema written as `quoted`, beside
    /// the lake `live` names: the projection's and `meta` there, the log's
    /// and the lease in the lake's schema (`scratch.rs`).
    pub(crate) fn beside(quoted: String, live: &Names) -> Names {
        Names {
            schema: quoted,
            log: Some(live.schema.clone()),
            at: live.at.clone(),
            bytes: live.bytes.clone(),
        }
    }

    /// Find from the catalogue which of [`HEX_COLUMNS`] `log` keeps as
    /// bytes: at a handle's open, and after a conversion commits.
    pub(crate) fn find_bytes(&self, db: &mut Db) -> Result<()> {
        let kept: Vec<String> = db.column(
            "SELECT attname::text FROM pg_catalog.pg_attribute WHERE attrelid = to_regclass($1) \
             AND atttypid = 'bytea'::regtype AND NOT attisdropped",
            &[&self.t("log")],
        )?;
        let bits = (HEX_COLUMNS.iter().enumerate())
            .filter(|(_, c)| kept.iter().any(|k| k == *c))
            .fold(0, |bits, (i, _)| bits | 1 << i);
        self.bytes.store(bits, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    /// Whether `log` keeps `column`, one of [`HEX_COLUMNS`], as bytes.
    fn in_bytes(&self, column: &str) -> bool {
        let name = column.rsplit('.').next().unwrap_or(column);
        let bit = HEX_COLUMNS.iter().position(|c| *c == name);
        let bit = bit.expect("a column of `log` the store hands as hex");
        self.bytes.load(std::sync::atomic::Ordering::Relaxed) & 1 << bit != 0
    }

    /// `column` of `log`, `id`, `prev` or `hash` (perhaps qualified), as
    /// SQL reads it: the hex digits it spells, whichever type it is kept in.
    pub(crate) fn hex(&self, column: &str) -> String {
        match self.in_bytes(column) {
            true => format!("encode({column}, 'hex')"),
            false => column.to_owned(),
        }
    }

    /// `digits`, an expression of hex digits, as `column` of `log` keeps it.
    pub(crate) fn kept(&self, column: &str, digits: &str) -> String {
        match self.in_bytes(column) {
            true => format!("decode({digits}, 'hex')"),
            false => digits.to_owned(),
        }
    }

    /// The clause that makes a table or an index in `space`, if named.
    pub(crate) fn space(space: &Option<String>) -> String {
        let named = space.as_deref().map(Target::quote);
        named.map_or_else(String::new, |t| format!(" TABLESPACE {t}"))
    }

    /// The statement that makes what follows in the transaction in `space`.
    fn default_space(space: &Option<String>) -> String {
        let named = space.as_deref().map_or("DEFAULT".into(), Target::quote);
        format!("SET LOCAL default_tablespace = {named};\n")
    }

    /// The schema, as SQL writes it.
    pub(crate) fn schema(&self) -> &str {
        &self.schema
    }

    /// One table, schema-qualified.
    pub(crate) fn t(&self, table: &str) -> String {
        match &self.log {
            Some(log) if LOG_TABLES.contains(&table) => format!("{log}.{table}"),
            _ => format!("{}.{table}", self.schema),
        }
    }

    /// The statements that create the lake's tables, if absent.
    ///
    /// `text_index` has no primary key: a record's value is in it once by
    /// the unique index `text_index_value`, which holds the value's `md5`,
    /// so no text is too long for a btree row, and whose last column is the key
    /// in byte order, so the records holding a text are read from it in
    /// key order (`in_key_order` in `view.rs`). A lake made with a primary
    /// key there keeps it and gains the index.
    ///
    /// A record's key is compared bytewise (`COLLATE "C"`), so the primary
    /// keys of `records` and `records_head` hold the order keys are read
    /// in.
    ///
    /// `records_since` ends with the key, so a seek of one state by its
    /// type, key and position uses every column of either index the
    /// planner picks. Before a table is analysed the planner may pick it
    /// for that seek; ending at `since`, it filtered a type's states put at
    /// one position row by row, every head read paying for all of them
    /// (lakelands#1018).
    ///
    /// Each part goes in its tablespace where named: a table by its clause,
    /// its keys and indexes by `default_tablespace`, set for the transaction.
    pub(crate) fn schema_sql(&self) -> String {
        let (s, pt, a) = (&self.schema, Names::space(&self.at.projection), &self.at);
        let [ix, vs] = [&a.indexes, &a.vectors].map(Names::default_space);
        let heads = heads_sql(s, &pt);
        let log = self.log_sql();
        format!(
            "
CREATE SCHEMA IF NOT EXISTS {s};
{ix}CREATE TABLE IF NOT EXISTS {s}.meta (
    k TEXT PRIMARY KEY, n BIGINT, t TEXT
){pt};{log}
{ix}CREATE TABLE IF NOT EXISTS {s}.declarations (
    ident TEXT NOT NULL, since BIGINT NOT NULL, state TEXT NOT NULL,
    PRIMARY KEY (ident, since)
){pt};
CREATE INDEX IF NOT EXISTS declarations_since ON {s}.declarations (since);
CREATE TABLE IF NOT EXISTS {s}.records (
    ty TEXT NOT NULL, key TEXT COLLATE \"C\" NOT NULL, since BIGINT NOT NULL,
    state TEXT NOT NULL,
    PRIMARY KEY (ty, key, since)
){pt};
CREATE INDEX IF NOT EXISTS records_since ON {s}.records (ty, since, key);
CREATE TABLE IF NOT EXISTS {s}.proposals (
    id TEXT NOT NULL, since BIGINT NOT NULL, state TEXT NOT NULL,
    PRIMARY KEY (id, since)
){pt};
CREATE INDEX IF NOT EXISTS proposals_newest ON {s}.proposals (((state::jsonb ->> 'position')::int8));
CREATE TABLE IF NOT EXISTS {s}.quarantined (
    source TEXT COLLATE \"C\" NOT NULL, delivery TEXT COLLATE \"C\" NOT NULL, ix TEXT NOT NULL,
    since BIGINT NOT NULL, state TEXT NOT NULL,
    PRIMARY KEY (source, delivery, ix, since)
){pt};
CREATE INDEX IF NOT EXISTS quarantined_at ON {s}.quarantined ((state::jsonb ->> 'type'), (state::jsonb ->> 'key'));
CREATE TABLE IF NOT EXISTS {s}.members (
    object_set TEXT NOT NULL, ty TEXT NOT NULL, key TEXT NOT NULL,
    since BIGINT NOT NULL, state BOOLEAN NOT NULL,
    PRIMARY KEY (object_set, ty, key, since)
){pt};
CREATE TABLE IF NOT EXISTS {s}.outputs (
    seam TEXT NOT NULL, input TEXT NOT NULL, since BIGINT NOT NULL,
    state TEXT NOT NULL,
    PRIMARY KEY (seam, input, since)
){pt};
{heads}CREATE TABLE IF NOT EXISTS {s}.routes (
    at BIGINT NOT NULL, kind INTEGER NOT NULL, ty INTEGER, key TEXT, source INTEGER
){pt};
CREATE TABLE IF NOT EXISTS {s}.names (
    id INTEGER GENERATED ALWAYS AS IDENTITY PRIMARY KEY, name TEXT NOT NULL UNIQUE
){pt};
CREATE TABLE IF NOT EXISTS {s}.bases (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY, block TEXT NOT NULL
){pt};
CREATE INDEX IF NOT EXISTS bases_block ON {s}.bases (md5(block));
CREATE INDEX IF NOT EXISTS routes_kind ON {s}.routes (kind, at);
CREATE INDEX IF NOT EXISTS routes_ty ON {s}.routes (ty, at);
CREATE INDEX IF NOT EXISTS routes_key ON {s}.routes (key, at);
CREATE INDEX IF NOT EXISTS routes_source ON {s}.routes (source, at);
CREATE TABLE IF NOT EXISTS {s}.text_index (
    ty TEXT NOT NULL, field TEXT NOT NULL, value TEXT NOT NULL, key TEXT NOT NULL
){pt};
CREATE UNIQUE INDEX IF NOT EXISTS text_index_value
    ON {s}.text_index (ty, field, md5(value), key COLLATE \"C\");
CREATE INDEX IF NOT EXISTS text_index_key ON {s}.text_index (ty, key);
CREATE TABLE IF NOT EXISTS {s}.lookup_fields (
    ty TEXT NOT NULL, field TEXT NOT NULL, PRIMARY KEY (ty, field)
){pt};
CREATE TABLE IF NOT EXISTS {s}.edges (
    src_ty TEXT NOT NULL, src_key TEXT NOT NULL, relation TEXT NOT NULL,
    dst_ty TEXT NOT NULL, dst_key TEXT NOT NULL
){pt};
CREATE INDEX IF NOT EXISTS edges_src ON {s}.edges (src_ty, src_key);
CREATE INDEX IF NOT EXISTS edges_dst ON {s}.edges (dst_ty, dst_key);
CREATE TABLE IF NOT EXISTS {s}.full_text (
    ty TEXT NOT NULL, token TEXT NOT NULL, key TEXT NOT NULL,
    PRIMARY KEY (ty, token, key)
){pt};
CREATE INDEX IF NOT EXISTS full_text_key ON {s}.full_text (ty, key);
CREATE TABLE IF NOT EXISTS {s}.sort_index (
    ty TEXT NOT NULL, field TEXT NOT NULL, key TEXT NOT NULL, sort_key BYTEA NOT NULL,
    PRIMARY KEY (ty, key, field)
){pt};
CREATE INDEX IF NOT EXISTS sort_index_asc ON {s}.sort_index (ty, field, sort_key, key COLLATE \"C\");
CREATE INDEX IF NOT EXISTS sort_index_desc
    ON {s}.sort_index (ty, field, sort_key DESC, key COLLATE \"C\");
CREATE TABLE IF NOT EXISTS {s}.graphs (
    ty TEXT NOT NULL, field TEXT NOT NULL, class TEXT NOT NULL, dims INT NOT NULL,
    gen BIGINT, next BIGINT, stale BIGINT,
    PRIMARY KEY (ty, field, class)
){pt};
CREATE TABLE IF NOT EXISTS {s}.graph_keys (
    ty TEXT NOT NULL, key TEXT COLLATE \"C\" NOT NULL, PRIMARY KEY (ty, key)
){pt};
CREATE TABLE IF NOT EXISTS {s}.text_stats (
    ty TEXT NOT NULL, field TEXT NOT NULL, held BOOLEAN NOT NULL,
    records BIGINT NOT NULL, tokens BIGINT NOT NULL,
    PRIMARY KEY (ty, field, held)
){pt};
CREATE TABLE IF NOT EXISTS {s}.text_lengths (
    ty TEXT NOT NULL, key TEXT NOT NULL, field TEXT NOT NULL,
    held BOOLEAN NOT NULL, tokens BIGINT NOT NULL,
    PRIMARY KEY (ty, key, field)
){pt};
{vs}CREATE TABLE IF NOT EXISTS {s}.vectors (
    ty TEXT NOT NULL, key TEXT COLLATE \"C\" NOT NULL, field TEXT NOT NULL,
    since BIGINT NOT NULL, until BIGINT, single BOOLEAN NOT NULL, data BYTEA NOT NULL,
    PRIMARY KEY (ty, key, field, since)
);
CREATE INDEX IF NOT EXISTS vectors_head ON {s}.vectors (ty, field, key) WHERE until IS NULL;
"
        )
    }

    /// The statements that create the log's tables and the lease's, if
    /// absent, in the lake's schema; none for a scratch, which reads them
    /// there. A table a later build adds to the log (`frames`) is made by
    /// the first writer's open of this build, by `clear`, and by the swap
    /// of a rebuild (`scratch.rs`).
    pub(crate) fn log_sql(&self) -> String {
        if self.log.is_some() {
            return String::new();
        }
        let (s, lg) = (&self.schema, Names::default_space(&self.at.log));
        format!(
            "
{lg}CREATE TABLE IF NOT EXISTS {s}.lease (
    one INTEGER PRIMARY KEY CHECK (one = 1),
    fence BIGINT NOT NULL, holder TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS {s}.segments (
    first BIGINT PRIMARY KEY, digest TEXT
);
CREATE TABLE IF NOT EXISTS {s}.log (
    position BIGINT PRIMARY KEY, segment BIGINT NOT NULL, fence BIGINT NOT NULL,
    id BYTEA NOT NULL, prev BYTEA NOT NULL, hash BYTEA NOT NULL, bytes BYTEA NOT NULL
);
CREATE INDEX IF NOT EXISTS log_segment ON {s}.log (segment, position);
CREATE TABLE IF NOT EXISTS {s}.frames (
    first BIGINT PRIMARY KEY, events INT NOT NULL, bytes BYTEA NOT NULL
);"
        )
    }
}

/// The heads tables, one per history that keeps heads: each thing's
/// identifying columns, as its history types them, and the position of
/// its latest state.
fn heads_sql(s: &str, pt: &str) -> String {
    HISTORIES
        .iter()
        .filter(|h| h.head)
        .map(|h| {
            let columns = h.each(", ", |_, c| match c {
                "key" if h.table == "records" => "key TEXT COLLATE \"C\" NOT NULL".to_owned(),
                _ => format!("{c} TEXT NOT NULL"),
            });
            format!(
                "CREATE TABLE IF NOT EXISTS {s}.{t}_head ({columns}, since BIGINT NOT NULL, \
                 PRIMARY KEY ({ident})) WITH (fillfactor = 70){pt};\n",
                t = h.table,
                ident = h.ident.join(", "),
            )
        })
        .collect()
}

/// The layout of the projection this build reads and writes, marked in
/// `meta` as `layout` when the lake is made. It is raised whenever a
/// table changes so that a projection made before would answer wrongly,
/// not fail: one made with another layout is never read, and is rebuilt
/// from the log.
pub(crate) const LAYOUT: u64 = 10;

/// Every table of a lake's schema: the log's and the lease's, then the
/// projection's.
pub(crate) const LAKE_TABLES: [&str; 26] = [
    "meta",
    "lease",
    "segments",
    "log",
    "frames",
    "declarations",
    "records",
    "proposals",
    "quarantined",
    "members",
    "outputs",
    "routes",
    "names",
    "bases",
    "text_index",
    "lookup_fields",
    "edges",
    "full_text",
    "sort_index",
    "vectors",
    "declarations_head",
    "records_head",
    "graphs",
    "graph_keys",
    "text_stats",
    "text_lengths",
];

/// The projection's tables, cleared by a rebuild; the log, its segments,
/// the lease and the lake's name stay.
pub(crate) const PROJECTION_TABLES: &[&str] = LAKE_TABLES.split_at(5).1;

/// The projection's tables an earlier layout had and this one does not:
/// a layout that drops or renames a table names the old one here, so a
/// lake that layout made still opens to be rebuilt, and the rebuild's
/// `clear` or swap drops it (`scratch.rs`).
/// `applied` went with 0.21.0: an applied id is answered from the log's
/// index on `id` (`view.rs`).
pub(crate) const RETIRED_TABLES: &[&str] = &["applied"];

/// A history table: its name and the columns naming one thing in it.
pub(crate) struct History {
    pub(crate) table: &'static str,
    pub(crate) ident: &'static [&'static str],
    /// The SQL type of its `state` column.
    pub(crate) state: &'static str,
    /// Whether it keeps a heads table, for reads at the head.
    head: bool,
}

pub(crate) const DECLARATIONS: History = History::new("declarations", &["ident"], "text", true);
pub(crate) const RECORDS: History = History::new("records", &["ty", "key"], "text", true);
pub(crate) const PROPOSALS: History = History::new("proposals", &["id"], "text", false);
pub(crate) const QUARANTINED: History =
    History::new("quarantined", &["source", "delivery", "ix"], "text", false);
pub(crate) const OUTPUTS: History = History::new("outputs", &["seam", "input"], "text", false);
pub(crate) const MEMBERS: History =
    History::new("members", &["object_set", "ty", "key"], "boolean", false);

/// Every history.
const HISTORIES: [&History; 6] = [
    &DECLARATIONS,
    &RECORDS,
    &PROPOSALS,
    &QUARANTINED,
    &MEMBERS,
    &OUTPUTS,
];

impl History {
    const fn new(
        table: &'static str,
        ident: &'static [&'static str],
        state: &'static str,
        head: bool,
    ) -> Self {
        History {
            table,
            ident,
            state,
            head,
        }
    }

    /// The identifying columns, each as `f` writes it with its number
    /// (from 1), joined by `sep`.
    fn each(&self, sep: &str, f: impl Fn(usize, &str) -> String) -> String {
        let all: Vec<String> = (1..).zip(self.ident).map(|(i, c)| f(i, c)).collect();
        all.join(sep)
    }

    /// Each thing's head, `h`, beside its latest state, `r`: what a read at
    /// the head reads from. The state is sought by its primary key, one
    /// seek per head; a join of the two tables is estimated as if position
    /// and key were independent, at about one row, and a page planned on
    /// that sorts every head instead of walking them in order.
    pub(crate) fn at_head(&self, names: &Names) -> String {
        let t = names.t(self.table);
        format!(
            "{t}_head h CROSS JOIN LATERAL (SELECT x.* FROM {t} x \
             WHERE {on} AND x.since = h.since LIMIT 1) r",
            on = self.each(" AND ", |_, c| format!("x.{c} = h.{c}")),
        )
    }

    /// The states in force at the position bound as `$n` among the rows
    /// `cond` admits, `r`: each thing's latest state at or before it.
    pub(crate) fn in_force(&self, names: &Names, n: usize, cond: &str) -> String {
        let ident = self.ident.join(", ");
        format!(
            "(SELECT DISTINCT ON ({ident}) * FROM {} WHERE since <= ${n} {cond} \
             ORDER BY {ident}, since DESC) r",
            names.t(self.table)
        )
    }

    /// The upsert that moves the heads of the things `rows` names, with
    /// their positions, to those positions, never back past the state a
    /// head is at; a head at the same position is touched, since the state
    /// there is replaced.
    fn move_heads(&self, names: &Names, rows: &str) -> String {
        let columns = self.ident.join(", ");
        format!(
            "INSERT INTO {}_head AS h ({columns}, since) {rows} \
             ON CONFLICT ({columns}) DO UPDATE SET since = EXCLUDED.since \
             WHERE h.since <= EXCLUDED.since",
            names.t(self.table)
        )
    }

    /// Put a state of one thing from `since` on, and move its head there
    /// unless a later state holds it. A state already at `since` is
    /// replaced. Declarations are put so, as they come; every other state
    /// is buffered and put with [`History::put_many`].
    pub(crate) fn put(
        &self,
        db: &mut Db,
        names: &Names,
        ident: &[&(dyn ToSql + Sync)],
        since: Position,
        state: &(dyn ToSql + Sync),
    ) -> Result<()> {
        let n = self.ident.len();
        let at = since.0 as i64;
        let mut args: Vec<&(dyn ToSql + Sync)> = ident.to_vec();
        args.push(&at);
        args.push(state);
        let columns = self.ident.join(", ");
        let marks: Vec<String> = (1..=n + 1).map(|i| format!("${i}")).collect();
        let marks = marks.join(", ");
        db.execute(
            &format!(
                "WITH h AS ({heads}) INSERT INTO {table} ({columns}, since, state) \
                 VALUES ({marks}, ${v}) \
                 ON CONFLICT ({columns}, since) DO UPDATE SET state = EXCLUDED.state",
                heads = self.move_heads(names, &format!("VALUES ({marks})")),
                table = names.t(self.table),
                v = n + 2,
            ),
            &args,
        )
    }

    /// Put the states a transaction buffered in one statement: each row
    /// inserted (a buffered row replacing one at the same position), and,
    /// in a history that keeps heads, each thing's head moved to its latest
    /// buffered position. No row put before is touched.
    ///
    /// Answers the things whose latest state is now a buffered one, each
    /// with whether the history held a state of it before; nothing where
    /// the history keeps no heads.
    pub(crate) fn put_many<S: ToSql + Sync>(
        &self,
        db: &mut Db,
        names: &Names,
        rows: &BTreeMap<(Vec<String>, i64), S>,
    ) -> Result<Vec<(Vec<String>, bool)>> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
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
        // An upserted head reads `xmax` 0 when it was inserted: the thing
        // had no state before.
        let heads = match self.head {
            true => format!(
                "{} RETURNING {columns}, xmax <> 0",
                self.move_heads(
                    names,
                    &format!("SELECT {columns}, max(since) FROM b GROUP BY {columns}")
                )
            ),
            false => "SELECT 1 WHERE false".to_owned(),
        };
        let answer = db.query(
            &format!(
                "WITH b AS (\
                   SELECT * FROM unnest({arrays}, ${s}::int8[], ${v}::{ty}[]) \
                   AS b({columns}, since, state)\
                 ), put AS (\
                   INSERT INTO {table} ({columns}, since, state) SELECT * FROM b \
                   ON CONFLICT ({columns}, since) DO UPDATE SET state = EXCLUDED.state\
                 ) {heads}",
                arrays = arrays.join(", "),
                s = n + 1,
                v = n + 2,
                ty = self.state,
                table = names.t(self.table),
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

/// A type's declaration at the head, its order index's fields, its
/// exact-text index's (`None`, every field, for a kept name), and those
/// its states keep apart ([`engines::vector_fields`]).
#[derive(Clone, Default)]
pub(crate) struct HeadType {
    pub(crate) decl: Option<TypeDecl>,
    pub(crate) sorted: Vec<FieldName>,
    pub(crate) looked: Option<BTreeSet<FieldName>>,
    pub(crate) vectors: BTreeSet<FieldName>,
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
/// declaration (a kept name's view's) declares with a kind the index
/// keeps. A kept name has no declaration. One round trip, the setting's
/// head row read once.
pub(crate) fn head_type(db: &mut Db, names: &Names, ty: &TypeName) -> Result<HeadType> {
    let view = engines::kept_view(ty);
    let row = db.query_one(
        &format!(
            "SELECT (SELECT r.state FROM {d} WHERE h.ident = $1), s.state, \
             (SELECT array_agg(field) FROM {l} WHERE ty = $4), \
             (SELECT t FROM {m} WHERE k = $5), s.since \
             FROM (SELECT) AS one LEFT JOIN LATERAL \
             (SELECT r.state, r.since FROM {r} WHERE h.ty = $2 AND h.key = $3) AS s ON true",
            d = DECLARATIONS.at_head(names),
            r = RECORDS.at_head(names),
            l = names.t("lookup_fields"),
            m = names.t("meta"),
        ),
        &[
            &format!("type {}", view.as_ref().unwrap_or(ty)),
            &SETTING_TYPE,
            &engines::sort_index_key(ty),
            &ty.as_str(),
            &engines::chosen_mark(ty),
        ],
    )?;
    let looked: Option<Vec<String>> = row.get(2);
    let declared = declared_type(row.get(0))?;
    let setting = setting_at(&row, ty, [1, 4])?;
    let chosen = chosen_decoded(row.get(3));
    let sorted = match &declared {
        Some(d) => engines::order_fields(ty, d, setting.as_ref(), &chosen, true),
        None => Vec::new(),
    };
    let own = view.as_ref().unwrap_or(ty);
    let vectors = engines::vector_fields(ty, |t| match t == own {
        true => Ok(declared.clone()),
        false => type_at(db, names, t, None),
    })?;
    let decl = declared.filter(|_| view.is_none());
    let looked = decl
        .as_ref()
        .map(|_| looked.into_iter().flatten().map(FieldName::from).collect());
    Ok(HeadType {
        decl,
        sorted,
        looked,
        vectors,
    })
}

/// The fields of an order index the lake chose, as the meta row holds
/// them: none where it holds none.
pub(crate) fn chosen_decoded(text: Option<String>) -> Chosen {
    text.map_or_else(Chosen::default, |t| {
        serde_json::from_str(&t).unwrap_or_default()
    })
}

/// Keep `ty`'s chosen fields as `chosen`: no row where it holds none.
/// The lake's first choice marks them kept through the position folded
/// through, which the fold moves on from there, and writes the mark's twin
/// row in `sort_index` ([`held`]).
fn set_chosen(db: &mut Db, names: &Names, ty: &TypeName, chosen: &Chosen) -> Result<()> {
    let (meta, mark) = (names.t("meta"), engines::chosen_mark(ty));
    if *chosen == Chosen::default() {
        return db.execute(&format!("DELETE FROM {meta} WHERE k = $1"), &[&mark]);
    }
    let text = serde_json::to_string(chosen).expect("the chosen fields serialise");
    db.execute(
        &format!(
            "WITH marked AS (INSERT INTO {meta} (k, n) \
             VALUES ($1, (SELECT n FROM {meta} WHERE k = 'folded')) \
             ON CONFLICT (k) DO NOTHING RETURNING k) \
             INSERT INTO {} (ty, field, key, sort_key) SELECT '', k, '', ''::bytea FROM marked \
             ON CONFLICT DO NOTHING",
            names.t("sort_index")
        ),
        &[&engines::CHOSEN_THROUGH],
    )?;
    db.execute(
        &format!("INSERT INTO {meta} (k, t) VALUES ($1, $2) ON CONFLICT (k) DO UPDATE SET t = $2"),
        &[&mark, &text],
    )
}

/// The fields of `ty`'s order index the lake chose, as a read takes them:
/// none unless they are kept through the position folded through, which
/// a writer that keeps no chosen field folds past.
pub(crate) fn chosen_kept(db: &mut Db, names: &Names, ty: &TypeName) -> Result<Chosen> {
    let row = db.query_one(
        &format!(
            "SELECT (SELECT t FROM {} WHERE k = $1), {}",
            names.t("meta"),
            held(names, "$2")
        ),
        &[&engines::chosen_mark(ty), &engines::CHOSEN_THROUGH],
    )?;
    Ok(match row.get::<_, bool>(1) {
        true => chosen_decoded(row.get(0)),
        false => Chosen::default(),
    })
}

/// Whether the lake's chosen fields are kept through the position folded
/// through, as SQL with `mark` the parameter naming their mark: a lake
/// that never chose holds no mark and is. A mark is held only beside its
/// twin row in `sort_index` (type `''`, field the mark's name): a binary
/// before the chosen fields clears the projection by dropping its tables,
/// `sort_index` with every chosen row among them, and keeps `meta`, so
/// its rebuild through the same position leaves the mark where it stood
/// and takes the twin.
pub(crate) fn held(names: &Names, mark: &str) -> String {
    format!(
        "NOT EXISTS (SELECT 1 FROM {m} WHERE k = {mark} \
         AND (n IS DISTINCT FROM (SELECT n FROM {m} WHERE k = 'folded') \
         OR NOT EXISTS (SELECT 1 FROM {s} WHERE ty = '' AND field = {mark})))",
        m = names.t("meta"),
        s = names.t("sort_index"),
    )
}

/// The fields of `ty`'s order index the lake chose, as its meta row holds
/// them.
pub(crate) fn chosen_of(db: &mut Db, names: &Names, ty: &TypeName) -> Result<Chosen> {
    let row = db.query_opt(
        &format!("SELECT t FROM {} WHERE k = $1", names.t("meta")),
        &[&engines::chosen_mark(ty)],
    )?;
    Ok(chosen_decoded(row.and_then(|row| row.get(0))))
}

/// One step of choosing an order index for `ty` (`ProjectionStore::
/// choose_order`), in a transaction of its own that moves no position.
pub(crate) fn choose_order(
    db: &mut Db,
    names: &Names,
    ty: &TypeName,
    step: &Choose,
) -> Result<Option<RecordKey>> {
    let setting = head_record(
        db,
        names,
        &TypeName::from(SETTING_TYPE),
        &RecordKey::from(engines::sort_index_key(ty)),
    )?;
    let head = head_type(db, names, ty)?;
    let Some(decl) = head.decl.as_ref().filter(|d| d.view.is_none()) else {
        return Ok(None);
    };
    let named = engines::sorted_fields(ty, decl, setting.as_ref().and_then(engines::setting_value));
    let sortable = |f: &FieldName| {
        (decl.fields.iter()).any(|d| d.name == *f && lakelands_api::seam::sortable(&d.ty.kind))
    };
    if !named.is_empty() || matches!(step, Choose::Start(f) if !sortable(f)) {
        return Ok(None);
    }
    // Nothing is chosen on a lake a writer that keeps no chosen field
    // folded past, before a writer's open drops what it left.
    let held = db.query_one(
        &format!("SELECT {}", held(names, "$1")),
        &[&engines::CHOSEN_THROUGH],
    )?;
    let mut chosen = chosen_of(db, names, ty)?;
    if !held.get::<_, bool>(0) || !engines::chosen_after(&mut chosen, step) {
        return Ok(None);
    }
    db.batch("BEGIN ISOLATION LEVEL READ COMMITTED; SET LOCAL synchronous_commit = off")?;
    let done = match step {
        Choose::Fill {
            field,
            after,
            limit,
        } => fill_chosen(db, names, ty, field, after.as_ref(), *limit),
        _ => set_chosen(db, names, ty, &chosen).and_then(|()| {
            match step {
                Choose::Start(_) => {
                    let marked = db.query_opt(
                        &format!("SELECT 1 FROM {} WHERE k = $1", names.t("meta")),
                        &[&engines::sorted_mark(ty)],
                    )?;
                    if marked.is_none()
                        && let Some(at) = folded(db, names)?
                    {
                        set_sorted(db, names, ty, Some(at))?;
                    }
                }
                Choose::Drop(f) => {
                    db.execute(
                        &format!(
                            "DELETE FROM {} WHERE ty = $1 AND field = $2",
                            names.t("sort_index")
                        ),
                        &[&ty.as_str(), &f.as_str()],
                    )?;
                    if chosen == Chosen::default() {
                        set_sorted(db, names, ty, None)?;
                    }
                }
                _ => {}
            }
            Ok(None)
        }),
    };
    db.end(done)
}

/// Write the rows of `field` for up to `limit` head records of `ty` after
/// `after`; the key the chunk ended at, none once the type is filled.
fn fill_chosen(
    db: &mut Db,
    names: &Names,
    ty: &TypeName,
    field: &FieldName,
    after: Option<&RecordKey>,
    limit: u32,
) -> Result<Option<RecordKey>> {
    let after = after.map_or("", RecordKey::as_str);
    let page = head_records_after(db, names, ty, after, i64::from(limit))?;
    let (mut fields, mut keys, mut sort_keys) = (Vec::new(), Vec::new(), Vec::new());
    for r in &page {
        for (field, sort_key) in engines::order_rows(std::slice::from_ref(field), r) {
            fields.push(field.as_str().to_owned());
            keys.push(r.key.as_str().to_owned());
            sort_keys.push(sort_key);
        }
    }
    let all: Vec<&str> = page.iter().map(|r| r.key.as_str()).collect();
    db.execute(
        &format!(
            "DELETE FROM {} WHERE ty = $1 AND field = $2 AND key = ANY($3)",
            names.t("sort_index")
        ),
        &[&ty.as_str(), &field.as_str(), &all],
    )?;
    db.execute(
        &format!(
            "INSERT INTO {} (ty, field, key, sort_key) \
             SELECT $1, * FROM unnest($2::text[], $3::text[], $4::bytea[])",
            names.t("sort_index"),
        ),
        &[&ty.as_str(), &fields, &keys, &sort_keys],
    )?;
    Ok(page
        .last()
        .map(|r| r.key.clone())
        .filter(|_| page.len() == limit as usize))
}

/// The declarations at the head.
pub(crate) fn head_declarations(db: &mut Db, names: &Names) -> Result<Vec<Declaration>> {
    let sql = format!("SELECT r.state FROM {}", DECLARATIONS.at_head(names));
    let states = db.query(&sql, &[])?;
    let decls = states
        .iter()
        .map(|row| decode::<Declared>(row.get(0), "a declaration"));
    decls.map(|d| d.map(|d| d.declaration)).collect()
}

/// Decide again which fields the exact-text index keeps ([`engines::relooked`])
/// for the types `moved` names (`None`, every type), and fill or drop each
/// field's rows from the head records: at the commit of a transaction that
/// put a declaration or a `lookup_index.<type>`, after its last page; the
/// types it read are read again after it.
pub(crate) fn relook(
    db: &mut Db,
    names: &Names,
    types: &mut HeadTypes,
    kept: &engines::Heads,
    moved: Option<&BTreeSet<TypeName>>,
) -> Result<()> {
    types.0.clear();
    let lookup = names.t("lookup_fields");
    let rows = match moved {
        Some(moved) => {
            let moved: Vec<&str> = moved.iter().map(TypeName::as_str).collect();
            let sql = format!("SELECT ty, field FROM {lookup} WHERE ty = ANY($1)");
            db.query(&sql, &[&moved])?
        }
        None => db.query(&format!("SELECT ty, field FROM {lookup}"), &[])?,
    };
    let was = rows.iter().map(|row| (row.get(0), row.get(1))).collect();
    let heads = RECORDS.at_head(names);
    let changes = engines::relooked(kept, moved, &was, |key| {
        let sql = format!("SELECT {RECORD} FROM {heads} WHERE h.ty = $1 AND h.key = $2");
        let row = db.query_opt(&sql, &[&SETTING_TYPE, &key])?;
        row.map(|row| record_at(&row, 0).map(|(r, _)| r))
            .transpose()
    })?;
    let text = names.t("text_index");
    for (ty, f, comes) in changes {
        let statements = match comes {
            true => [
                format!(
                    "INSERT INTO {text} (ty, field, value, key) \
                     SELECT h.ty, $2, r.state::jsonb -> 'fields' ->> $2, h.key FROM {heads} \
                     WHERE h.ty = $1 AND jsonb_typeof(r.state::jsonb -> 'fields' -> $2) = 'string' \
                     ON CONFLICT DO NOTHING"
                ),
                format!("INSERT INTO {lookup} (ty, field) VALUES ($1, $2)"),
            ],
            false => [
                format!("DELETE FROM {text} WHERE ty = $1 AND field = $2"),
                format!("DELETE FROM {lookup} WHERE ty = $1 AND field = $2"),
            ],
        };
        for statement in statements {
            db.execute(&statement, &[&ty, &f])?;
        }
    }
    Ok(())
}

/// The statement of a feed filter's positions in `routes` from `$2`
/// through `$3`, at most `$4`: those of each value of `$1` in the column
/// `by` the filter is sought by (`lakelands_core::feed::Seek`), or every
/// row's without one, each row checked against the kind codes `$5`, type
/// ids `$6` and keys `$7` that are not empty. A value of `$1` is a key
/// for `key`, else a code or an id.
pub(crate) fn routes_sql(names: &Names, by: Option<&str>) -> String {
    let routes = names.t("routes");
    let checked = "at BETWEEN $2 AND $3 AND (cardinality($5::int4[]) = 0 OR kind = ANY($5)) \
         AND (cardinality($6::int4[]) = 0 OR ty = ANY($6)) \
         AND (cardinality($7::text[]) = 0 OR key = ANY($7))";
    match by {
        Some(column) => {
            let held = if column == "key" { "text" } else { "int4" };
            format!(
                "SELECT DISTINCT r.at FROM unnest($1::{held}[]) v(value) CROSS JOIN LATERAL (\
                 SELECT DISTINCT at FROM {routes} WHERE {column} = v.value AND {checked} \
                 ORDER BY at LIMIT $4) r ORDER BY r.at LIMIT $4"
            )
        }
        None => format!(
            "SELECT DISTINCT at FROM {routes} WHERE cardinality($1::int4[]) = 0 AND {checked} \
             ORDER BY at LIMIT $4"
        ),
    }
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

/// Record the position folded through, and that every order index the
/// lake keeps (a `sorted.<type>` mark each) is kept through it too, in
/// one statement. So are the chosen fields among them, and the text
/// statistics, if they were kept through the position it folds from (the
/// statement reads `folded` as it was); a writer that keeps none folded
/// past them otherwise, and they stay behind until a writer's open drops
/// or counts them.
pub(crate) fn set_folded(db: &mut Db, names: &Names, through: Position) -> Result<()> {
    db.execute(
        &format!(
            "WITH s AS (UPDATE {m} SET n = $1 \
             WHERE k LIKE 'sorted.%' OR (k IN ($2, '{TEXT_STATS}') \
             AND n IS NOT DISTINCT FROM (SELECT n FROM {m} WHERE k = 'folded'))) \
             INSERT INTO {m} (k, n) VALUES ('folded', $1) ON CONFLICT (k) DO UPDATE SET n = $1",
            m = names.t("meta")
        ),
        &[&(through.0 as i64), &engines::CHOSEN_THROUGH],
    )
}

/// The records an order index's rebuild reads at a time.
const REBUILD_PAGE: i64 = 1024;

/// `ANALYZE` over the tables a view plans from: `records`, its heads, the
/// head indexes and the vectors, which the graphs' writes read by key.
/// One statement, run outside any transaction.
pub(crate) fn analyze(db: &mut Db, names: &Names) -> Result<()> {
    let tables: Vec<String> = [
        "records",
        "records_head",
        "vectors",
        "text_index",
        "edges",
        "full_text",
        "sort_index",
    ]
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
    // A host's setting that names a field wins: the lake's choice goes.
    let setting = head_record(
        db,
        names,
        &TypeName::from(SETTING_TYPE),
        &RecordKey::from(engines::sort_index_key(ty)),
    )?;
    let head = head_type(db, names, ty)?;
    let named = head
        .decl
        .as_ref()
        .map(|d| engines::sorted_fields(ty, d, setting.as_ref().and_then(engines::setting_value)));
    let head = match named.is_some_and(|n| !n.is_empty()) {
        true => {
            set_chosen(db, names, ty, &Chosen::default())?;
            head_type(db, names, ty)?
        }
        false => head,
    };
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
/// drop the index of a type whose setting such a writer removed. A
/// setting asks for the indexes of [`engines::ordered_names`]; one of
/// those that keeps no field has no mark and is not behind. The lake's
/// choice goes whole where such a writer folded past it: it left the
/// chosen fields' rows as they were while it moved every `sorted.` mark.
pub(crate) fn catch_up_sorted(db: &mut Db, names: &Names) -> Result<()> {
    db.transaction("BEGIN ISOLATION LEVEL READ COMMITTED", |db| {
        let Some(folded) = folded(db, names)? else {
            return Ok(());
        };
        let asked: Vec<String> = db.column(
            &format!(
                "SELECT substr(key, $1) FROM {} WHERE ty = $2 AND left(key, $1 - 1) = $3",
                names.t("records_head"),
            ),
            &[
                &(SORT_INDEX_SETTING.len() as i32 + 1),
                &SETTING_TYPE,
                &SORT_INDEX_SETTING,
            ],
        )?;
        let (marked, mut behind): (BTreeSet<String>, BTreeSet<String>) = (
            db.column(
                &format!(
                    "SELECT substr(k, 8) FROM {} WHERE k LIKE 'sorted.%'",
                    names.t("meta")
                ),
                &[],
            )?,
            db.column(
                &format!(
                    "SELECT substr(k, 8) FROM {} WHERE k LIKE 'sorted.%' AND n <> $1",
                    names.t("meta")
                ),
                &[&(folded.0 as i64)],
            )?,
        );
        for ty in asked
            .iter()
            .flat_map(|t| engines::ordered_names(&t.as_str().into()))
        {
            if !marked.contains(ty.as_str()) && !head_type(db, names, &ty)?.sorted.is_empty() {
                behind.insert(ty.to_string());
            }
        }
        let held: bool = db
            .query_one(
                &format!("SELECT {}", held(names, "$1")),
                &[&engines::CHOSEN_THROUGH],
            )?
            .get(0);
        if !held {
            let (m, s) = (names.t("meta"), names.t("sort_index"));
            let stale: Vec<String> = db.column(
                &format!("SELECT substr(k, 8) FROM {m} WHERE k LIKE 'chosen.%'"),
                &[],
            )?;
            for ty in stale {
                set_chosen(db, names, &TypeName::from(ty.as_str()), &Chosen::default())?;
                behind.insert(ty);
            }
            db.execute(
                &format!(
                    "WITH twin AS (DELETE FROM {s} WHERE ty = '' AND field = $1) \
                     DELETE FROM {m} WHERE k = $1"
                ),
                &[&engines::CHOSEN_THROUGH],
            )?;
        }
        for ty in behind {
            rebuild_sorted(db, names, &TypeName::from(ty), folded)?;
        }
        Ok(())
    })
}

/// What a statement selects of a record `r`: its row's type, key and
/// position, then its state, which leaves them out.
pub(crate) const RECORD: &str = "r.ty, r.key, r.since, r.state";

/// The record a row holds from the column `at` on, as [`RECORD`] selects
/// it, and the fields its state keeps apart ([`joined`] reads them).
pub(crate) fn record_at(row: &Row, at: usize) -> Result<(StoredRecord, engines::Apart)> {
    let since = Position(row.get::<_, i64>(at + 2) as u64);
    engines::decode_record(row.get(at), row.get(at + 1), since, row.get(at + 3))
}

/// The records `read`, each vector its state keeps apart read from its
/// row and each base it names from its row of `bases`: one statement for
/// all the vectors and one for all the bases, none where no state keeps
/// one.
pub(crate) fn joined<T>(
    db: &mut Db,
    names: &Names,
    read: Vec<(T, (StoredRecord, engines::Apart))>,
) -> Result<Vec<(T, StoredRecord)>> {
    let mut asked = VectorRows::default();
    for (_, (r, apart)) in &read {
        let ident = [r.ty.to_string(), r.key.to_string()];
        for (f, at) in &apart.vectors {
            asked.push(&ident, f, at.0 as i64, None);
        }
    }
    let mut ids: Vec<i64> = (read.iter())
        .filter_map(|(_, (_, apart))| Some(apart.base.as_ref()?.id))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    let mut blocks = HashMap::new();
    if !ids.is_empty() {
        let bases = names.t("bases");
        let sql = format!("SELECT id, block FROM {bases} WHERE id = ANY($1::int8[])");
        for row in db.query(&sql, &[&ids])? {
            blocks.insert(row.get::<_, i64>(0), row.get::<_, String>(1));
        }
    }
    // Each row found by its place among those asked for.
    let mut rows = HashMap::new();
    if !asked.since.is_empty() {
        let [tys, keys, fields] = &asked.ident;
        let found = db.query(
            &format!(
                "SELECT w.n, v.single, v.data FROM unnest($1::text[], $2::text[], $3::text[], \
                 $4::int8[]) WITH ORDINALITY AS w(ty, key, field, since, n) \
                 JOIN {} v ON v.ty = w.ty AND v.key = w.key AND v.field = w.field \
                 AND v.since = w.since",
                names.t("vectors")
            ),
            &[tys, keys, fields, &asked.since],
        )?;
        for row in found {
            let e = engines::Elements::new(row.get(1), row.get(2));
            rows.insert(row.get::<_, i64>(0), e);
        }
    }
    let (mut out, mut n) = (Vec::with_capacity(read.len()), 0);
    for (t, (mut r, apart)) in read {
        if let Some(based) = apart.base {
            let block = blocks.get(&based.id).map(String::as_str);
            engines::rebase(&mut r, based, block)?;
        }
        for (f, _) in apart.vectors {
            n += 1;
            engines::rejoin(&mut r, f, rows.remove(&n))?;
        }
        out.push((t, r));
    }
    Ok(out)
}

/// The records `rows` hold from the column `at` on, joined ([`joined`]).
pub(crate) fn records_at(
    db: &mut Db,
    names: &Names,
    rows: &[Row],
    at: usize,
) -> Result<Vec<StoredRecord>> {
    let read: Vec<_> = rows
        .iter()
        .map(|r| Ok(((), record_at(r, at)?)))
        .collect::<Result<_>>()?;
    Ok(joined(db, names, read)?
        .into_iter()
        .map(|(_, r)| r)
        .collect())
}

/// The value of the `sort_index.<type>` setting of `ty` a row holds as
/// two scalar reads of its head, its state in the column `state` and its
/// position in `since`; `None` where it is unset. Its value alone: the
/// provenance of a base its state names is not read.
pub(crate) fn setting_at(
    row: &Row,
    ty: &TypeName,
    [state, since]: [usize; 2],
) -> Result<Option<serde_json::Value>> {
    let (Some(s), Some(at)) = (
        row.get::<_, Option<&str>>(state),
        row.get::<_, Option<i64>>(since),
    ) else {
        return Ok(None);
    };
    let key = engines::sort_index_key(ty);
    let at = Position(at as u64);
    let (setting, _) = engines::decode_record(SETTING_TYPE, &key, at, s)?;
    Ok(engines::setting_value(&setting).cloned())
}

/// The declaration of a type in force at `at` (`None` for the head).
pub(crate) fn type_at(
    db: &mut Db,
    names: &Names,
    ty: &TypeName,
    at: Option<Position>,
) -> Result<Option<TypeDecl>> {
    let ident = format!("type {ty}");
    let row = match at {
        Some(at) => db.query_opt(
            &format!(
                "SELECT state FROM {} WHERE ident = $1 AND since <= $2 \
                 ORDER BY since DESC LIMIT 1",
                names.t("declarations")
            ),
            &[&ident, &(at.0 as i64)],
        )?,
        None => db.query_opt(
            &format!(
                "SELECT r.state FROM {} WHERE h.ident = $1",
                DECLARATIONS.at_head(names)
            ),
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
        names,
        &format!(
            "SELECT {RECORD} FROM {} WHERE h.ty = $1 AND h.key > $2 ORDER BY h.key LIMIT $3",
            RECORDS.at_head(names)
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
    let row = db.query_opt(
        &format!(
            "SELECT {RECORD} FROM {} WHERE h.ty = $1 AND h.key = $2",
            RECORDS.at_head(names)
        ),
        &[&ty.as_str(), &key.as_str()],
    )?;
    Ok(records_at(db, names, &Vec::from_iter(row), 0)?.pop())
}

/// The latest states of the records of `ty` that `keys` names, in no
/// order: one statement, and one more each for the bases and the vectors
/// they keep apart ([`joined`]).
pub(crate) fn head_records(
    db: &mut Db,
    names: &Names,
    ty: &TypeName,
    keys: &[String],
) -> Result<Vec<StoredRecord>> {
    db.records(
        names,
        &format!(
            "SELECT {RECORD} FROM {} WHERE h.ty = $1 AND h.key = ANY($2::text[])",
            RECORDS.at_head(names)
        ),
        &[&ty.as_str(), &keys],
    )
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
            if head.looked.as_ref().is_some_and(|l| !l.contains(field)) {
                continue;
            }
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
    if !stale.is_empty() {
        let (tys, keys): (Vec<&str>, Vec<&str>) = stale.iter().copied().unzip();
        unlength(db, names, &tys, &keys)?;
    }
    let lengths = records
        .iter()
        .filter_map(|(head, r)| Some((head.decl.as_ref()?, *r)));
    length(db, names, lengths)?;
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

/// The meta key of the position the text statistics are folded through;
/// null where they count an empty projection.
pub(crate) const TEXT_STATS: &str = "text_stats";

/// Add live heads' text into their types' counts (`docs/seams.md`
/// §`ranked`), each under its type's declaration at the head: each
/// searchable field's tokens, under its hold, kept beside so that the next
/// write of the record takes out exactly this. One statement.
fn length<'r>(
    db: &mut Db,
    names: &Names,
    records: impl Iterator<Item = (&'r TypeDecl, &'r StoredRecord)>,
) -> Result<()> {
    let (mut tys, mut keys, mut fields) = (Vec::new(), Vec::new(), Vec::new());
    let (mut held, mut tokens) = (Vec::new(), Vec::new());
    for (decl, r) in records.filter(|(_, r)| !r.retired) {
        for (field, n) in engines::text_lengths(decl, r) {
            tys.push(r.ty.as_str());
            keys.push(r.key.as_str());
            fields.push(field.to_string());
            held.push(r.restricted);
            tokens.push(n as i64);
        }
    }
    if tys.is_empty() {
        return Ok(());
    }
    db.execute(
        &format!(
            "WITH n AS (SELECT * FROM unnest($1::text[], $2::text[], $3::text[], $4::bool[], \
             $5::bigint[]) AS n(ty, key, field, held, tokens)), \
             x AS (INSERT INTO {lengths} (ty, key, field, held, tokens) SELECT * FROM n) \
             INSERT INTO {stats} AS s (ty, field, held, records, tokens) \
             SELECT ty, field, held, count(*), sum(tokens)::bigint FROM n GROUP BY ty, field, held \
             ON CONFLICT (ty, field, held) DO UPDATE \
             SET records = s.records + EXCLUDED.records, tokens = s.tokens + EXCLUDED.tokens",
            lengths = names.t("text_lengths"),
            stats = names.t("text_stats"),
        ),
        &[&tys, &keys, &fields, &held, &tokens],
    )
}

/// Take what [`length`] added for records out of their types' counts.
fn unlength(db: &mut Db, names: &Names, tys: &[&str], keys: &[&str]) -> Result<()> {
    db.execute(
        &format!(
            "WITH d AS (SELECT * FROM unnest($1::text[], $2::text[]) AS d(ty, key)), \
             gone AS (DELETE FROM {lengths} t USING d WHERE t.ty = d.ty AND t.key = d.key \
             RETURNING t.ty, t.field, t.held, t.tokens), \
             sums AS (SELECT ty, field, held, count(*) AS n, sum(tokens)::bigint AS l \
             FROM gone GROUP BY ty, field, held) \
             UPDATE {stats} s SET records = s.records - sums.n, tokens = s.tokens - sums.l \
             FROM sums WHERE s.ty = sums.ty AND s.field = sums.field AND s.held = sums.held",
            lengths = names.t("text_lengths"),
            stats = names.t("text_stats"),
        ),
        &[&tys, &keys],
    )
}

/// Whether the text statistics describe the head: their mark is the
/// position folded through, or null on an empty projection.
fn text_stats_held(db: &mut Db, names: &Names) -> Result<bool> {
    let m = names.t("meta");
    let row = db.query_opt(
        &format!(
            "SELECT n IS NOT DISTINCT FROM (SELECT n FROM {m} WHERE k = 'folded') \
             FROM {m} WHERE k = $1"
        ),
        &[&TEXT_STATS],
    )?;
    Ok(row.is_some_and(|r| r.get(0)))
}

/// Count the text statistics again from the head where their mark is
/// absent or behind: a lake written before they were kept, or folded by
/// a writer that keeps none. Once, in one transaction.
pub(crate) fn catch_up_text_stats(db: &mut Db, names: &Names) -> Result<()> {
    db.transaction("BEGIN ISOLATION LEVEL READ COMMITTED", |db| {
        if text_stats_held(db, names)? {
            return Ok(());
        }
        let (stats, lengths, m) = (
            names.t("text_stats"),
            names.t("text_lengths"),
            names.t("meta"),
        );
        db.batch(&format!("DELETE FROM {stats}; DELETE FROM {lengths}"))?;
        let types: Vec<String> = db.column(
            &format!("SELECT DISTINCT ty FROM {}", names.t("records_head")),
            &[],
        )?;
        for ty in types.iter().map(|t| TypeName::from(t.as_str())) {
            let Some(decl) = head_type(db, names, &ty)?.decl else {
                continue;
            };
            head_pages(db, names, &ty, |db, page| {
                length(db, names, page.iter().map(|r| (&decl, r)))
            })?;
        }
        db.execute(
            &format!(
                "INSERT INTO {m} (k, n) VALUES ($1, (SELECT n FROM {m} WHERE k = 'folded')) \
                 ON CONFLICT (k) DO UPDATE SET n = EXCLUDED.n"
            ),
            &[&TEXT_STATS],
        )
    })
}

/// Each head of `ty` that is not ended, a page at a time.
fn head_pages(
    db: &mut Db,
    names: &Names,
    ty: &TypeName,
    mut each: impl FnMut(&mut Db, Vec<StoredRecord>) -> Result<()>,
) -> Result<()> {
    let mut after = String::new();
    loop {
        let page = head_records_after(db, names, ty, &after, PAGE as i64)?;
        let next = (page.len() == PAGE).then(|| page.last().map(|r| r.key.to_string()));
        each(db, page.into_iter().filter(|r| !r.is_ended()).collect())?;
        match next.flatten() {
            Some(last) => after = last,
            None => return Ok(()),
        }
    }
}

/// The counts of `ty`'s fields over its live heads that are not held,
/// and with `held` over every one: as kept, where they describe the head,
/// else counted from it at the read.
pub(crate) fn text_counts(
    db: &mut Db,
    names: &Names,
    (ty, decl): (&TypeName, &TypeDecl),
    held: bool,
) -> Result<BTreeMap<FieldName, (u64, u64)>> {
    if !text_stats_held(db, names)? {
        let mut counts = TextCounts::default();
        head_pages(db, names, ty, |_, page| {
            let class = page
                .iter()
                .filter(|r| !r.retired && (held || !r.restricted));
            engines::text_stats(decl, class, ("", None), &mut counts);
            Ok(())
        })?;
        return Ok(counts.fields);
    }
    let rows = db.query(
        &format!(
            "SELECT field, sum(records)::bigint, sum(tokens)::bigint FROM {} \
             WHERE ty = $1 AND (NOT held OR $2) GROUP BY field",
            names.t("text_stats")
        ),
        &[&ty.as_str(), &held],
    )?;
    let count = |r: &Row, i| r.get::<_, i64>(i) as u64;
    let field = |r: &Row| FieldName::from(r.get::<_, String>(0));
    Ok(rows
        .iter()
        .map(|r| (field(r), (count(r, 1), count(r, 2))))
        .collect())
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
    /// The last state put of each key of a head-only name, none for one
    /// ended: written over whatever the key held, with no row beside it.
    heads: BTreeMap<(String, String), (i64, Option<StoredRecord>)>,
    /// The head-only names whose every key was ended: deleted before the
    /// heads are written.
    ended: BTreeSet<String>,
    proposals: BTreeMap<(Vec<String>, i64), String>,
    quarantined: BTreeMap<(Vec<String>, i64), String>,
    outputs: BTreeMap<(Vec<String>, i64), String>,
    members: BTreeMap<(Vec<String>, i64), bool>,
    /// The feed index's rows of the events applied.
    routes: Vec<Route>,
    /// The records whose every state, membership and index row goes,
    /// before any put of the buffer is written.
    dropped: BTreeSet<(String, String)>,
    /// The outputs whose every state goes, by seam and input, likewise.
    dropped_outputs: BTreeSet<(String, String)>,
    /// Whether the bases no state names go, once every put is written.
    dropped_bases: bool,
    /// The vector index's last put (`Some`) or remove of each field of a
    /// key whose state it holds, by type, key and field: made when it is
    /// written where the rows its last state leaves differ.
    indexed: BTreeMap<(String, String, String), Option<Vec<f32>>>,
}

/// A row of the feed index: position, kind, type, key and source, the
/// types and the source still by name.
type Route = (i64, i32, Option<String>, Option<String>, Option<String>);

impl Buffer {
    /// The puts it holds, of every kind.
    pub(crate) fn len(&self) -> usize {
        let (records, states) = (
            self.records.len() + self.heads.len() + self.ended.len(),
            self.proposals.len() + self.quarantined.len() + self.outputs.len(),
        );
        let dropped =
            self.dropped.len() + self.dropped_outputs.len() + usize::from(self.dropped_bases);
        records + states + self.members.len() + self.routes.len() + dropped
    }

    /// Every state of a record goes, with its memberships and its index
    /// rows: what the buffer holds of it, and the rows, before any put.
    pub(crate) fn drop_record(&mut self, ty: &str, key: &str) {
        let of = |ident: &[String]| ident[0] == ty && ident[1] == key;
        self.records.retain(|(ident, _), _| !of(ident));
        self.members.retain(|(ident, _), _| !of(&ident[1..]));
        self.indexed.retain(|(t, k, _), _| t != ty || k != key);
        self.dropped.insert((ty.to_owned(), key.to_owned()));
    }

    /// Every state of a seam's output for one input goes, likewise.
    pub(crate) fn drop_output(&mut self, seam: &str, input: &str) {
        self.outputs
            .retain(|(ident, _), _| ident[0] != seam || ident[1] != input);
        (self.dropped_outputs).insert((seam.to_owned(), input.to_owned()));
    }

    /// The bases no state names go, once the buffer's puts are written.
    pub(crate) fn drop_bases(&mut self) {
        self.dropped_bases = true;
    }

    /// The state of `ty`'s `key` the buffer puts last, if any.
    pub(crate) fn latest(&self, ty: &str, key: &str) -> Option<&StoredRecord> {
        let ident = vec![ty.to_owned(), key.to_owned()];
        let of = (ident.clone(), i64::MIN)..=(ident, i64::MAX);
        self.records.range(of).next_back().map(|(_, r)| r)
    }

    /// Take the vector index's put (`Some`) or remove of `ty`'s `key`
    /// where a state of it is held, answering whether it took it.
    pub(crate) fn index(
        &mut self,
        (ty, key, field): (&str, &str, &str),
        v: Option<&[f32]>,
    ) -> bool {
        let held = self.latest(ty, key).is_some();
        if held {
            let at = (ty.to_owned(), key.to_owned(), field.to_owned());
            self.indexed.insert(at, v.map(<[f32]>::to_vec));
        }
        held
    }

    pub(crate) fn record(&mut self, r: &StoredRecord) {
        if lakelands_api::seam::is_head_only(&r.ty) {
            let state = (!r.is_ended()).then(|| r.clone());
            let key = (r.ty.to_string(), r.key.to_string());
            self.heads.insert(key, (r.position.0 as i64, state));
            return;
        }
        let ident = vec![r.ty.as_str().to_owned(), r.key.as_str().to_owned()];
        self.records.insert((ident, r.position.0 as i64), r.clone());
    }

    /// Every key of the head-only name `ty` ended: what the buffer holds
    /// of it goes, and so do its rows, before any head put after.
    pub(crate) fn end_heads(&mut self, ty: &str) {
        self.heads.retain(|(t, _), _| t != ty);
        self.ended.insert(ty.to_owned());
    }

    pub(crate) fn proposal(&mut self, id: &str, at: Position, state: String) {
        self.proposals
            .insert((vec![id.to_owned()], at.0 as i64), state);
    }

    pub(crate) fn quarantined(&mut self, id: &QuarantineId, at: Position, state: String) {
        let ident = vec![
            id.source.to_string(),
            id.delivery.to_string(),
            id.index.to_string(),
        ];
        self.quarantined.insert((ident, at.0 as i64), state);
    }

    pub(crate) fn output(&mut self, seam: &str, input: &str, at: Position, state: String) {
        let ident = vec![seam.to_owned(), input.to_owned()];
        self.outputs.insert((ident, at.0 as i64), state);
    }

    pub(crate) fn member(&mut self, set: &str, ty: &str, key: &str, at: Position, member: bool) {
        let ident = vec![set.to_owned(), ty.to_owned(), key.to_owned()];
        self.members.insert((ident, at.0 as i64), member);
    }

    pub(crate) fn applied(&mut self, applied: &Applied) {
        // The id is the log's at `at`, which answers it (`view.rs`).
        let at = applied.at.0 as i64;
        for a in &applied.about {
            let (ty, key) = (a.ty.as_ref().map(|t| t.to_string()), a.key.as_ref());
            let (key, source) = (key.map(|k| k.to_string()), applied.source.clone());
            let kind = engines::kind_code(applied.kind).expect("a kind the format lists");
            (self.routes).push((at, kind as i32, ty, key, source.map(|s| s.to_string())));
        }
    }

    /// Delete the rows of what the buffer dropped, in one statement.
    fn drop_rows(&self, db: &mut Db, names: &Names) -> Result<()> {
        if !self.dropped.is_empty() {
            let (tys, keys): (Vec<&str>, Vec<&str>) = (self.dropped.iter())
                .map(|(t, k)| (t.as_str(), k.as_str()))
                .unzip();
            let gone = |table: &str, ty: &str, key: &str| {
                format!(
                    "DELETE FROM {} t USING d WHERE t.{ty} = d.ty AND t.{key} = d.key",
                    names.t(table)
                )
            };
            let sets = format!(
                "SELECT DISTINCT substr(ident, 12) FROM {} WHERE starts_with(ident, 'object_set ')",
                names.t("declarations")
            );
            db.execute(
                &format!(
                    "WITH d AS (SELECT * FROM unnest($1::text[], $2::text[]) AS d(ty, key)), \
                     r AS ({}), h AS ({}), m AS ({} AND t.object_set IN ({sets})), \
                     x AS ({}), y AS ({}), z AS ({}), v AS ({}) {}",
                    gone("records", "ty", "key"),
                    gone("records_head", "ty", "key"),
                    gone("members", "ty", "key"),
                    gone("text_index", "ty", "key"),
                    gone("full_text", "ty", "key"),
                    gone("sort_index", "ty", "key"),
                    gone("vectors", "ty", "key"),
                    gone("edges", "src_ty", "src_key"),
                ),
                &[&tys, &keys],
            )?;
            unlength(db, names, &tys, &keys)?;
        }
        if !self.dropped_outputs.is_empty() {
            let (seams, inputs): (Vec<&str>, Vec<&str>) = (self.dropped_outputs.iter())
                .map(|(s, i)| (s.as_str(), i.as_str()))
                .unzip();
            db.execute(
                &format!(
                    "DELETE FROM {} t USING unnest($1::text[], $2::text[]) AS d(seam, input) \
                     WHERE t.seam = d.seam AND t.input = d.input",
                    names.t("outputs")
                ),
                &[&seams, &inputs],
            )?;
        }
        Ok(())
    }

    /// Write every buffered put: one statement per history table, the
    /// head indexes of the records whose latest state is now a buffered
    /// one, under their types' declarations at the head as `types` reads
    /// them after every declaration the transaction put, and the applied
    /// ids; the bases [`engines::SHARED`] states of one write share before
    /// them, and last, after a sweep, the rows of `bases` no state names
    /// any more. That leaves what putting them one by one leaves. The types
    /// whose `vector_index.<type>` setting it wrote go into `asked`, and
    /// the records it wrote or erased into `touched`, for the graphs
    /// (`crate::vector_index`). It answers the order index rows the
    /// settings it wrote rebuilt.
    pub(crate) fn write(
        self,
        db: &mut Db,
        names: &Names,
        types: &mut HeadTypes,
        asked: &mut BTreeMap<TypeName, Position>,
        touched: &mut Touched,
    ) -> Result<u64> {
        self.drop_rows(db, names)?;
        for (ty, _) in &self.dropped {
            touched.erased.insert(TypeName::from(ty.as_str()));
        }
        for (ident, _) in self.records.keys() {
            let keys = touched.keys.entry(TypeName::from(ident[0].as_str()));
            keys.or_default().insert(ident[1].clone());
        }
        for (ty, key, _) in self.indexed.keys() {
            let keys = touched.keys.entry(TypeName::from(ty.as_str()));
            keys.or_default().insert(key.clone());
        }
        let mut apart = self.put_vectors(db, names, types)?;
        let heads = self.heads.values().filter_map(|(_, r)| r.as_ref());
        // A write the buffer flushes in two pages is counted a page at a time.
        let shared = engines::shared_bases(self.records.values().chain(heads));
        let ids = put_bases(db, names, &shared)?;
        let base = |b: &str| {
            if !shared.contains(b) {
                return Ok(None);
            }
            let id = ids.get(b).copied().map(Some);
            id.ok_or_else(|| engines::base_missing("its write kept no row of it"))
        };
        let records: BTreeMap<(Vec<String>, i64), String> = (self.records.iter())
            .map(|(k, r)| {
                let apart = apart.remove(k).unwrap_or_default();
                let state = engines::record_state(r, &apart, &mut |b| base(b))?;
                Ok((k.clone(), state))
            })
            .collect::<Result<_>>()?;
        let heads = RECORDS.put_many(db, names, &records)?;
        // A head-only name's key is one row, written over in place, and its
        // head beside it.
        let (table, head) = (names.t("records"), names.t("records_head"));
        if !self.ended.is_empty() {
            let tys: Vec<&str> = self.ended.iter().map(String::as_str).collect();
            let gone = format!(
                "WITH h AS (DELETE FROM {head} WHERE ty = ANY($1::text[])) \
                 DELETE FROM {table} WHERE ty = ANY($1::text[])"
            );
            db.execute(&gone, &[&tys])?;
        }
        if !self.heads.is_empty() {
            let (mut tys, mut keys, mut since, mut states) = (vec![], vec![], vec![], vec![]);
            for ((ty, key), (at, r)) in &self.heads {
                tys.push(ty.as_str());
                keys.push(key.as_str());
                since.push(*at);
                let state = r
                    .as_ref()
                    .map(|r| engines::record_state(r, &BTreeMap::new(), &mut |b| base(b)));
                states.push(state.transpose()?);
            }
            let b = "unnest($1::text[], $2::text[]) AS b(ty, key)";
            let gone = |t: &str| {
                format!("DELETE FROM {t} t USING {b} WHERE t.ty = b.ty AND t.key = b.key")
            };
            let (h, r) = (gone(&head), gone(&table));
            db.execute(&format!("WITH h AS ({h}) {r}"), &[&tys, &keys])?;
            db.execute(
                &format!(
                    "WITH b AS (SELECT * FROM \
                     unnest($1::text[], $2::text[], $3::int8[], $4::text[]) AS b(ty, key, since, state) \
                     WHERE state IS NOT NULL), \
                     h AS (INSERT INTO {head} (ty, key, since) SELECT ty, key, since FROM b) \
                     INSERT INTO {table} (ty, key, since, state) SELECT * FROM b"
                ),
                &[&tys, &keys, &since, &states],
            )?;
        }
        PROPOSALS.put_many(db, names, &self.proposals)?;
        QUARANTINED.put_many(db, names, &self.quarantined)?;
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
                for ty in engines::ordered_names(&ty) {
                    types.0.remove(&ty);
                    settings.push((ty, r.position));
                }
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
        if !self.routes.is_empty() {
            let (mut at, mut kinds, mut tys, mut keys) = (vec![], vec![], vec![], vec![]);
            let mut sources = vec![];
            for (p, kind, ty, key, source) in &self.routes {
                at.push(*p);
                kinds.push(*kind);
                tys.push(ty.as_deref());
                keys.push(key.as_deref());
                sources.push(source.as_deref());
            }
            let held = names.t("names");
            let mut all: Vec<&str> = tys.iter().chain(&sources).flatten().copied().collect();
            all.sort_unstable();
            all.dedup();
            db.execute(
                &format!(
                    "INSERT INTO {held} (name) SELECT n FROM unnest($1::text[]) n \
                     WHERE NOT EXISTS (SELECT 1 FROM {held} WHERE name = n) \
                     ON CONFLICT DO NOTHING"
                ),
                &[&all],
            )?;
            db.execute(
                &format!(
                    "INSERT INTO {} (at, kind, ty, key, source) \
                     SELECT u.at, u.kind, t.id, u.key, s.id FROM \
                     unnest($1::int8[], $2::int4[], $3::text[], $4::text[], $5::text[]) \
                     AS u(at, kind, ty, key, source) \
                     LEFT JOIN {held} t ON t.name = u.ty LEFT JOIN {held} s ON s.name = u.source",
                    names.t("routes")
                ),
                &[&at, &kinds, &tys, &keys, &sources],
            )?;
        }
        if self.dropped_bases {
            db.execute(
                &format!(
                    "DELETE FROM {} b WHERE NOT EXISTS \
                     (SELECT 1 FROM {} r WHERE r.state::jsonb -> 'base' = to_jsonb(b.id))",
                    names.t("bases"),
                    names.t("records")
                ),
                &[],
            )?;
        }
        Ok(rebuilt)
    }
}

/// Keep each of the `shared` bases that `bases` does not hold, in one
/// statement, and answer the id of every one by its block. A base is
/// found by the hash of its block and then the block, so two blocks of one
/// hash are two rows.
fn put_bases(db: &mut Db, names: &Names, shared: &HashSet<String>) -> Result<HashMap<String, i64>> {
    let blocks: BTreeSet<&str> = shared.iter().map(String::as_str).collect();
    if blocks.is_empty() {
        return Ok(HashMap::new());
    }
    let blocks: Vec<&str> = blocks.into_iter().collect();
    let held = names.t("bases");
    let rows = db.query(
        &format!(
            "WITH u AS (SELECT * FROM unnest($1::text[]) AS u(block)), \
             i AS (INSERT INTO {held} (block) SELECT block FROM u \
             WHERE NOT EXISTS (SELECT 1 FROM {held} b \
             WHERE md5(b.block) = md5(u.block) AND b.block = u.block) RETURNING id, block) \
             SELECT id, block FROM i UNION ALL SELECT b.id, b.block FROM {held} b JOIN u \
             ON md5(b.block) = md5(u.block) AND b.block = u.block"
        ),
        &[&blocks],
    )?;
    Ok((rows.iter())
        .map(|r| (r.get::<_, String>(1), r.get::<_, i64>(0)))
        .collect())
}

/// A record's rows of `vectors` in force, as [`Buffer::put_vectors`] reads
/// them: those of its states, and those the index put alone (`since` -1).
#[derive(Default)]
struct Held {
    rows: Vec<engines::VectorRow>,
    alone: BTreeSet<String>,
    head: Option<i64>,
}

/// The fields each buffered state keeps apart, by its row of `records`.
type StatesApart = BTreeMap<(Vec<String>, i64), BTreeMap<FieldName, Position>>;

/// The rows [`Buffer::put_vectors`] writes, as arrays for one statement.
#[derive(Default)]
struct VectorRows {
    ident: [Vec<String>; 3],
    since: Vec<i64>,
    until: Vec<Option<i64>>,
    single: Vec<bool>,
    data: Vec<Vec<u8>>,
}

impl VectorRows {
    fn push(&mut self, ident: &[String], field: &FieldName, since: i64, until: Option<i64>) {
        let ident = ident.iter().map(String::as_str).chain([field.as_str()]);
        for (column, v) in self.ident.iter_mut().zip(ident) {
            column.push(v.to_owned());
        }
        self.since.push(since);
        self.until.push(until);
    }
}

impl Buffer {
    /// Write the vector rows of the buffered records' states
    /// ([`engines::put_vectors`]), over the rows in force each record holds
    /// and whether a later state of it is kept, and answer the fields each
    /// state keeps apart, of those its type's declaration in `types` keeps.
    /// A vector the index put alone for a field a state names goes.
    fn put_vectors(
        &self,
        db: &mut Db,
        names: &Names,
        types: &mut HeadTypes,
    ) -> Result<StatesApart> {
        let mut apart = BTreeMap::new();
        let mut held: BTreeMap<(String, String), Held> = BTreeMap::new();
        for (ident, _) in self.records.keys() {
            held.entry((ident[0].clone(), ident[1].clone()))
                .or_default();
        }
        if held.is_empty() {
            return Ok(apart);
        }
        let (tys, keys): (Vec<String>, Vec<String>) = held.keys().cloned().unzip();
        let b = "unnest($1::text[], $2::text[]) AS b(ty, key)";
        let rows = db.query(
            &format!(
                "SELECT v.ty, v.key, v.field, v.since, v.single, v.data FROM {b} JOIN {} v \
                 ON v.ty = b.ty AND v.key = b.key AND v.until IS NULL",
                names.t("vectors")
            ),
            &[&tys, &keys],
        )?;
        for row in rows {
            let h = held
                .get_mut(&(row.get(0), row.get(1)))
                .expect("a key asked for");
            let (field, since): (String, i64) = (row.get(2), row.get(3));
            match since < 0 {
                true => drop(h.alone.insert(field)),
                false => {
                    let e = engines::Elements::new(row.get(4), row.get(5));
                    h.rows.push((field.into(), Position(since as u64), e));
                }
            }
        }
        let heads = db.query(
            &format!(
                "SELECT h.ty, h.key, h.since FROM {b} JOIN {} h ON h.ty = b.ty AND h.key = b.key",
                names.t("records_head")
            ),
            &[&tys, &keys],
        )?;
        for row in heads {
            held.get_mut(&(row.get(0), row.get(1)))
                .expect("a key asked for")
                .head = Some(row.get(2));
        }
        let (mut written, mut ended, mut gone) = <[VectorRows; 3]>::default().into();
        let mut vectors: BTreeMap<&TypeName, BTreeSet<FieldName>> = BTreeMap::new();
        for ((ident, at), r) in &self.records {
            if !vectors.contains_key(&r.ty) {
                vectors.insert(&r.ty, types.get(db, names, &r.ty)?.vectors);
            }
            let h = held
                .get_mut(&(ident[0].clone(), ident[1].clone()))
                .expect("a key held");
            let behind = h.head.is_some_and(|head| head > *at);
            let put = engines::put_vectors(r, std::mem::take(&mut h.rows), behind, &vectors[&r.ty]);
            h.rows = put.held;
            let until = put.ends.then_some(*at);
            for (f, e) in put.written {
                written.push(ident, &f, *at, until);
                written.single.push(e.single);
                written.data.push(e.data);
            }
            for (f, since) in put.ended {
                ended.push(ident, &f, since.0 as i64, Some(*at));
            }
            for f in put.gone {
                gone.push(ident, &f, *at, None);
            }
            for f in put.apart.keys() {
                if h.alone.remove(f.as_str()) {
                    gone.push(ident, f, -1, None);
                }
            }
            apart.insert((ident.clone(), *at), put.apart);
        }
        // An index put or remove after the states stands where the rows they
        // leave hold otherwise: made once they are written.
        let mut later = Vec::new();
        for ((ty, key, field), vector) in &self.indexed {
            let h = &held[&(ty.clone(), key.clone())];
            let row = (h.rows.iter()).find(|(f, ..)| f.as_str() == field);
            let row = row.map(|(.., e)| e.ranked());
            if h.alone.contains(field) || row != *vector {
                later.push(((ty.as_str(), key.as_str(), field.as_str()), vector));
            }
        }
        let t = names.t("vectors");
        let ident = "unnest($1::text[], $2::text[], $3::text[], $4::int8[]";
        if !written.since.is_empty() {
            let w = &written;
            let [tys, keys, fields] = &w.ident;
            db.execute(
                &format!(
                    "INSERT INTO {t} (ty, key, field, since, until, single, data) \
                     SELECT * FROM {ident}, $5::int8[], $6::bool[], $7::bytea[]) \
                     ON CONFLICT (ty, key, field, since) DO UPDATE SET until = EXCLUDED.until, \
                     single = EXCLUDED.single, data = EXCLUDED.data"
                ),
                &[tys, keys, fields, &w.since, &w.until, &w.single, &w.data],
            )?;
        }
        if !ended.since.is_empty() {
            let [tys, keys, fields] = &ended.ident;
            db.execute(
                &format!(
                    "UPDATE {t} t SET until = b.until FROM \
                     {ident}, $5::int8[]) AS b(ty, key, field, since, until) \
                     WHERE t.ty = b.ty AND t.key = b.key AND t.field = b.field \
                     AND t.since = b.since"
                ),
                &[tys, keys, fields, &ended.since, &ended.until],
            )?;
        }
        if !gone.since.is_empty() {
            let [tys, keys, fields] = &gone.ident;
            db.execute(
                &format!(
                    "DELETE FROM {t} t USING {ident}) AS b(ty, key, field, since) \
                     WHERE t.ty = b.ty AND t.key = b.key AND t.field = b.field \
                     AND t.since = b.since"
                ),
                &[tys, keys, fields, &gone.since],
            )?;
        }
        for (at, vector) in later {
            match vector {
                Some(vector) => vector_index::put(db, names, at, vector)?,
                None => vector_index::remove(db, names, at)?,
            }
        }
        Ok(apart)
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
        "SELECT {RECORD} FROM {} WHERE h.ty = $1 AND h.since < $2 AND h.key > $3 \
         ORDER BY h.key LIMIT {PAGE}",
        RECORDS.at_head(names)
    );
    // A page of the type at a time; an ended key has no index rows.
    let mut after = String::new();
    loop {
        let records = db.records(names, &page, &[&ty.as_str(), &(at.0 as i64), &after])?;
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
