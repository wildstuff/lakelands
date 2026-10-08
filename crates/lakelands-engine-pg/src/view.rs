//! Reads of the projection at one position.
//!
//! A read of more than one statement, or one that asks whether the view
//! is at the head, runs in one `REPEATABLE READ READ ONLY` transaction on
//! the handle's reader connection, so all of it sees one committed state
//! of the projection; a read of one statement runs alone, since one
//! statement already sees one. A read reads the history rows that hold at
//! the view's position. When that position is the one the projection
//! reflects (the head), a read may find its candidates through the head
//! indexes; otherwise it reads the states at the position directly. Either way
//! `lakelands_core::engines` decides what matches, where it sorts and what
//! it scores, exactly as on every other engine.
//!
//! A snapshot does not hold back DDL, and a rebuild's `clear` drops the
//! projection's tables and makes them again. So the feed's index, whose
//! `routes` must agree with the folded marker read beside it, takes
//! `routes` in `ACCESS SHARE` mode before its first `SELECT` takes the
//! snapshot: a lock takes none, and the `DROP` waits for the read. Taken
//! after the snapshot, the lock would find the table made again and empty
//! under a marker that still names the old fold.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::engine::{Declared, Matched, ProjectionView, Scan};
use lakelands_api::error::Result;
use lakelands_api::event::{Digest, EventId};
use lakelands_api::id::{FieldName, Position, RecordKey, RelationName, TypeName, VerbName};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::query::{Edge, Walk};
use lakelands_api::read::{Direction, Order};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::seam::{FeedFilter, is_kept_name};
use lakelands_api::write::{Declaration, Proposal, ProposalFilter};
use lakelands_core::engines::{self, decode};
use lakelands_core::feed::{self, Seek};
use postgres::types::ToSql;

use crate::connect::Db;
use crate::handle::PgLake;
use crate::projection::{self, DECLARATIONS, MEMBERS, Names, PROPOSALS, decode_record};

/// A read view of one lake's projection at one position.
pub(crate) struct PgView<'a> {
    pub(crate) lake: &'a PgLake,
    pub(crate) at: Option<Position>,
}

/// One read: a connection in a read transaction, the head in the state it
/// sees, and whether the view's position is that head.
struct Read<'c> {
    db: &'c mut Db,
    names: &'c Names,
    at: i64,
    folded: Option<i64>,
    head: bool,
}

/// The records in force at the position `$2`, `r`, each beside its head
/// `h`, from the table `records` and its heads: read in key order, they
/// are a walk of the heads' primary key, each head's state sought by a
/// seek (`History::at_head` says why not by a join). At the head it is
/// the state the head is at; at an earlier position the key's latest state
/// at or before it, and a key first put after it has none. A kept record
/// is put at every write to its group, so a key may hold thousands of
/// states, and none of the others is read.
fn in_force_sql(records: &str, head: bool) -> String {
    let at = match head {
        true => "x.since = h.since AND x.since <= $2",
        false => "x.since <= $2 ORDER BY x.since DESC",
    };
    format!(
        "{records}_head h CROSS JOIN LATERAL (SELECT x.state FROM {records} x \
         WHERE x.ty = h.ty AND x.key = h.key AND {at} LIMIT 1) r"
    )
}

/// The statement of [`Read::records_after`]: the records of `$1` in force
/// at `$2`, in key order after the key `$4` when `after`. At most `$3`
/// records, and every one when that is null.
///
/// At the head every head has its state, so the heads are cut at the limit
/// before their states are sought: a walk of one table's primary key, which
/// the planner takes in its order whether it holds statistics of the table
/// or not.
fn records_after_sql(records: &str, head: bool, after: bool) -> String {
    let from = if after { "AND h.key > $4" } else { "" };
    if head {
        return format!(
            "SELECT r.state FROM (SELECT * FROM {records}_head h \
             WHERE h.ty = $1 AND h.since <= $2 {from} ORDER BY h.key LIMIT $3) h \
             CROSS JOIN LATERAL (SELECT x.state FROM {records} x \
             WHERE x.ty = h.ty AND x.key = h.key AND x.since = h.since LIMIT 1) r \
             ORDER BY h.key"
        );
    }
    format!(
        "SELECT r.state FROM {} WHERE h.ty = $1 {from} ORDER BY h.key LIMIT $3",
        in_force_sql(records, false)
    )
}

/// The statement of [`Read::count_in_force`]: the records of `$1` in force
/// at `$2`, under the two flags `$3` and `$4`.
fn count_in_force_sql(records: &str, head: bool) -> String {
    format!(
        "SELECT COUNT(*) FROM {} WHERE h.ty = $1 \
         AND ($3 OR NOT (r.state::jsonb ->> 'retired')::boolean) \
         AND ($4 OR NOT (r.state::jsonb ->> 'restricted')::boolean)",
        in_force_sql(records, head)
    )
}

/// The statement of a recorded output's read from the table `outputs`:
/// the latest state of the seam and input at or before the position. A
/// view's state is recorded again at every change of its counters; none
/// of its other states is read (lakelands#575).
fn output_sql(outputs: &str) -> String {
    format!(
        "SELECT state FROM {outputs} WHERE seam = $1 AND input = $2 AND since <= $3 \
         ORDER BY since DESC LIMIT 1"
    )
}

/// The statement of [`Read::records_by_key`]: for each key `$2` names,
/// the latest state at or before the position. No other state of a key is
/// read, where a range over the keys read every state of each up to the
/// position (lakelands#575).
fn records_by_key_sql(records: &str) -> String {
    format!(
        "SELECT r.state FROM unnest($2::text[]) k(key) CROSS JOIN LATERAL (\
         SELECT h.state FROM {records} h WHERE h.ty = $1 AND h.key = k.key \
         AND h.since <= $3 ORDER BY h.since DESC LIMIT 1) r ORDER BY k.key COLLATE \"C\""
    )
}

/// The head record of `$1` whose key is the column `key`, `r`, sought
/// through its head: a seek into each primary key.
fn head_of_sql(records: &str, key: &str) -> String {
    format!(
        "CROSS JOIN LATERAL (SELECT x.state FROM {records}_head h \
         JOIN {records} x ON x.ty = h.ty AND x.key = h.key AND x.since = h.since \
         WHERE h.ty = $1 AND h.key = {key} LIMIT 1) r"
    )
}

/// The statement of [`Read::sorted_after`] over the tables `sorted`,
/// `text` and `records`: the order index's rows of `$2` for the records
/// `holders` names, `$4` and `$5` the field and text they hold, and then
/// the sort key and key bound to seek past when `from` is set. Many
/// holders are read in the index's order, each kept where the text index
/// holds its key; few are read from the text index first, materialized so
/// that no statistics turn it round, and sorted. The index's rows are cut
/// at the limit before their records are joined: every row is a head
/// record's, so the join keeps them all.
fn sorted_after_sql(
    [sorted, text, records]: &[String; 3],
    dir: Direction,
    from: bool,
    holders: &engines::Holders<'_>,
) -> String {
    use engines::Holders;
    let (k, key) = match holders {
        Holders::All => (4, 5),
        _ => (6, 7),
    };
    let (seek, by) = match dir {
        Direction::Asc => (
            format!("AND (s.sort_key, s.key COLLATE \"C\") > (${k}, ${key} COLLATE \"C\")"),
            "s.sort_key, s.key COLLATE \"C\"",
        ),
        Direction::Desc => (
            format!(
                "AND (s.sort_key < ${k} OR (s.sort_key = ${k} AND s.key COLLATE \"C\" > ${key}))"
            ),
            "s.sort_key DESC, s.key COLLATE \"C\"",
        ),
    };
    let seek = if from { seek.as_str() } else { "" };
    let (with, rows) = match holders {
        Holders::All => (String::new(), format!("{sorted} s WHERE")),
        Holders::Many(..) => (
            String::new(),
            format!(
                "{sorted} s WHERE EXISTS (SELECT 1 FROM {text} t \
                 WHERE t.ty = $1 AND t.field = $4 AND t.value = $5 AND t.key = s.key) AND"
            ),
        ),
        Holders::Few(..) => (
            format!(
                "WITH t AS MATERIALIZED \
                 (SELECT key FROM {text} WHERE ty = $1 AND field = $4 AND value = $5) "
            ),
            format!("t JOIN {sorted} s ON s.key = t.key WHERE"),
        ),
    };
    format!(
        "{with}SELECT s.sort_key, r.state FROM \
         (SELECT s.sort_key, s.key FROM {rows} s.ty = $1 AND s.field = $2 {seek} \
          ORDER BY {by} LIMIT $3) s \
         {} ORDER BY {by}",
        head_of_sql(records, "s.key")
    )
}

/// The statement of [`Read::unsorted_after`] for the records holding the
/// text `$5` in `$4`: the text index's rows in key order, bytewise, those
/// with an order row for `$2` passed over, cut at the limit before their
/// records are joined. The order row is sought by a scalar subquery, one
/// seek per row: a `NOT EXISTS` is planned on analysed tables as an anti
/// join of both indexes whole, and sorted.
fn unsorted_holding_sql([sorted, text, records]: &[String; 3], after: bool) -> String {
    let from = if after {
        "AND t.key COLLATE \"C\" > $6"
    } else {
        ""
    };
    format!(
        "SELECT r.state FROM \
         (SELECT t.key FROM {text} t \
          WHERE t.ty = $1 AND t.field = $4 AND t.value = $5 {from} \
          AND (SELECT 1 FROM {sorted} s \
          WHERE s.ty = $1 AND s.field = $2 AND s.key = t.key) IS NULL \
          ORDER BY t.key COLLATE \"C\" LIMIT $3) t \
         {} ORDER BY t.key COLLATE \"C\"",
        head_of_sql(records, "t.key")
    )
}

/// The statements a first page under an exact text that many records hold
/// is read by in the lake's `schema`, in a field's order either way and
/// among the records without the field, for the engine's own test of
/// their plans: none may sort (lakelands#587). Each takes the type, the
/// order field, the limit, the text's field and the text.
pub fn holding_statements(schema: &str) -> [String; 3] {
    let tables = ["sort_index", "text_index", "records"].map(|t| format!("{schema}.{t}"));
    let field = FieldName::from("f");
    let many = engines::Holders::Many(&field, "x");
    [
        sorted_after_sql(&tables, Direction::Asc, false, &many),
        sorted_after_sql(&tables, Direction::Desc, false, &many),
        unsorted_holding_sql(&tables, false),
    ]
}

/// The statements the view reads records by in the lake's `schema`, as it
/// runs them at the head (`head`) or at an earlier position: a first page
/// by key, a later page and the count under the flags alone; then the read
/// of records by their keys and of a recorded output. For the engine's own
/// test of their plans (lakelands#575). A page takes the name, the
/// position, the limit and the key it continues after; the count the name,
/// the position and the two flags; the read by keys the name, the keys and
/// the position; the output its seam, input and position.
pub fn read_statements(schema: &str, head: bool) -> [String; 5] {
    let records = format!("{schema}.records");
    [
        records_after_sql(&records, head, false),
        records_after_sql(&records, head, true),
        count_in_force_sql(&records, head),
        records_by_key_sql(&records),
        output_sql(&format!("{schema}.outputs")),
    ]
}

impl PgView<'_> {
    /// Run `f` in one read transaction at the view's position; an empty
    /// projection answers `empty`.
    fn read<R>(&self, empty: R, f: impl FnOnce(&mut Read<'_>) -> Result<R>) -> Result<R> {
        self.read_locked(None, empty, f)
    }

    /// [`Self::read`], holding the table `lock` in `ACCESS SHARE` mode
    /// from before the transaction's snapshot is taken.
    fn read_locked<R>(
        &self,
        lock: Option<&str>,
        empty: R,
        f: impl FnOnce(&mut Read<'_>) -> Result<R>,
    ) -> Result<R> {
        let Some(at) = self.at else {
            return Ok(empty);
        };
        let mut db = crate::lock(&self.lake.reader);
        let names = &self.lake.names;
        let lock = lock.map_or_else(String::new, |t| {
            format!("LOCK TABLE {} IN ACCESS SHARE MODE; ", names.t(t))
        });
        // The transaction opens and reads the folded marker in one round
        // trip, so the head flag is of the state every query in it sees.
        let folded = db.simple(&format!(
            "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY; {lock}\
             SELECT n FROM {} WHERE k = 'folded'",
            names.t("meta")
        ));
        let answer = folded.and_then(|folded| {
            let folded = folded.into_iter().flatten().next();
            f(&mut Read {
                db: &mut db,
                names,
                at: at.0 as i64,
                folded: folded.as_deref().and_then(|f| f.parse().ok()),
                head: folded == Some(at.0.to_string()),
            })
        });
        db.end(answer)
    }

    /// Run `f`, which runs exactly one statement and never asks whether
    /// the view is at the head, outside a transaction: one statement reads
    /// one committed state, so it needs none, and costs one round trip.
    fn single<R>(&self, empty: R, f: impl FnOnce(&mut Read<'_>) -> Result<R>) -> Result<R> {
        let Some(at) = self.at else {
            return Ok(empty);
        };
        f(&mut Read {
            db: &mut crate::lock(&self.lake.reader),
            names: &self.lake.names,
            at: at.0 as i64,
            folded: None,
            head: false,
        })
    }
}

impl Read<'_> {
    fn type_decl(&mut self, ty: &TypeName) -> Result<Option<TypeDecl>> {
        projection::type_at(self.db, self.names, ty, Some(Position(self.at as u64)))
    }

    fn record(&mut self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>> {
        self.db
            .query_opt(
                // The latest state at or before the position: no other
                // state of the key is read.
                &format!(
                    "SELECT state FROM {} WHERE ty = $1 AND key = $2 AND since <= $3 \
                     ORDER BY since DESC LIMIT 1",
                    self.names.t("records")
                ),
                &[&ty.as_str(), &key.as_str(), &self.at],
            )?
            .map(|r| decode_record(&r.get::<_, String>(0)))
            .transpose()
    }

    /// Every record of a type at the view's position, in key order.
    fn records(&mut self, ty: &TypeName) -> Result<Vec<StoredRecord>> {
        self.records_after(ty, None, None)
    }

    /// How many records of a type are in force at the view's position, the
    /// retired and the restricted ones only when asked, their flags read
    /// from the stored state.
    fn count_in_force(&mut self, ty: &TypeName, retired: bool, restricted: bool) -> Result<u64> {
        let query = count_in_force_sql(&self.names.t("records"), self.head);
        let counted: Vec<i64> = self
            .db
            .column(&query, &[&ty.as_str(), &self.at, &retired, &restricted])?;
        Ok(counted.first().map_or(0, |n| n.unsigned_abs()))
    }

    /// At most `n` records of a type at the view's position after the key
    /// `after` (from the first when `None`), in key order, bytewise: a walk
    /// of the heads' primary key. The first page and a later one are two
    /// statements, so that neither hides the index condition behind an
    /// `OR`. With no `n`, every record.
    fn records_after(
        &mut self,
        ty: &TypeName,
        after: Option<&str>,
        n: Option<u32>,
    ) -> Result<Vec<StoredRecord>> {
        let limit = n.map(i64::from);
        let query = records_after_sql(&self.names.t("records"), self.head, after.is_some());
        // `$4` is bound only when the statement names it.
        let args: [&(dyn ToSql + Sync); 4] = [&ty.as_str(), &self.at, &limit, &after];
        self.db
            .records(&query, &args[..3 + usize::from(after.is_some())])
    }

    /// What a key-ordered scan or count reads from, at most `n` records
    /// in key order, bytewise, after `after`: with a `lookup`, the head
    /// records whose field holds exactly its text, a walk of the exact-text
    /// index's unique index `text_index_value`, `(ty, field, value, key
    /// COLLATE "C")`; else the type's records. The index names candidates;
    /// the rulebook admits each.
    fn in_key_order(
        &mut self,
        ty: &TypeName,
        lookup: &Option<(FieldName, String)>,
        after: Option<&str>,
        n: u32,
    ) -> Result<Vec<StoredRecord>> {
        let Some((field, text)) = lookup else {
            return self.records_after(ty, after, Some(n));
        };
        let limit = i64::from(n);
        let from = after.map_or("", |_| "AND t.key COLLATE \"C\" > $5");
        // The index's rows first, in key order and cut at the limit, then
        // their records, as `sorted_after` reads its page. The cut before
        // the join leans on this: every index row is a head state's,
        // written in the same transaction as that state, so the join drops
        // none. It must drop none: a batch shorter than `n` ends the walk
        // (`engines::scan_in_key_order`), so this answers `n` records
        // whenever `n` holders of the text remain after `after`.
        let query = format!(
            "SELECT r.state FROM \
             (SELECT t.key FROM {text} t \
              WHERE t.ty = $1 AND t.field = $2 AND t.value = $3 {from} \
              ORDER BY t.key COLLATE \"C\" LIMIT $4) t \
             {head} ORDER BY t.key COLLATE \"C\"",
            text = self.names.t("text_index"),
            head = head_of_sql(&self.names.t("records"), "t.key"),
        );
        let args: [&(dyn ToSql + Sync); 5] = [&ty.as_str(), &field.as_str(), text, &limit, &after];
        self.db
            .records(&query, &args[..4 + usize::from(after.is_some())])
    }

    /// The fields of `ty` whose order index answers this read, in one round
    /// trip: none unless the index is kept through the head. At an earlier
    /// position it is read with the keys changed since ([`Read::changed`]).
    fn sorted_fields(&mut self, ty: &TypeName, decl: Option<&TypeDecl>) -> Result<Vec<FieldName>> {
        if self.folded.is_none() {
            return Ok(Vec::new());
        }
        // A kept name is ordered as its view at the head declares it.
        let view = engines::kept_view(ty).map(|v| format!("type {v}"));
        let row = self.db.query_one(
            &format!(
                "SELECT (SELECT n FROM {m} WHERE k = $1), \
                 (SELECT r.state FROM {r} WHERE h.ty = $2 AND h.key = $3), \
                 (SELECT r.state FROM {d} WHERE h.ident = $4)",
                m = self.names.t("meta"),
                r = projection::RECORDS.at_head(self.names),
                d = projection::DECLARATIONS.at_head(self.names),
            ),
            &[
                &engines::sorted_mark(ty),
                &lakelands_api::seam::SETTING_TYPE,
                &engines::sort_index_key(ty),
                &view,
            ],
        )?;
        let view = projection::declared_type(row.get(2))?;
        let Some(decl) = view.as_ref().or(decl) else {
            return Ok(Vec::new());
        };
        if row.get::<_, Option<i64>>(0) != self.folded {
            return Ok(Vec::new());
        }
        let setting = row
            .get::<_, Option<String>>(1)
            .map(|state| decode_record(&state))
            .transpose()?;
        Ok(engines::sorted_fields(
            ty,
            decl,
            setting.as_ref().and_then(engines::setting_value),
        ))
    }

    /// At most `n` head records of a type with a row in the order index for
    /// `order`'s field, with their sort keys, in the index's order for its
    /// direction, strictly after `from`: a seek into `sort_index_asc` or
    /// `sort_index_desc`.
    fn sorted_after(
        &mut self,
        ty: &TypeName,
        order: &Order,
        holders: &engines::Holders<'_>,
        from: Option<(&[u8], &str)>,
        n: u32,
    ) -> Result<Vec<(Vec<u8>, StoredRecord)>> {
        let limit = i64::from(n);
        let query = sorted_after_sql(&self.tables(), order.dir, from.is_some(), holders);
        let (ty, field) = (ty.as_str(), order.field.as_str());
        let mut args: Vec<&(dyn ToSql + Sync)> = vec![&ty, &field, &limit];
        let holding = holders.text();
        if let Some((f, t)) = &holding {
            args.extend([f as &(dyn ToSql + Sync), t]);
        }
        let (k, key) = from.unzip();
        if from.is_some() {
            args.extend([&k as &(dyn ToSql + Sync), &key]);
        }
        let rows = self.db.query(&query, &args)?;
        rows.iter()
            .map(|r| Ok((r.get(0), decode_record(&r.get::<_, String>(1))?)))
            .collect()
    }

    /// The order index's, the exact-text index's and the records' tables.
    fn tables(&self) -> [String; 3] {
        ["sort_index", "text_index", "records"].map(|t| self.names.t(t))
    }

    /// How many head records of a type hold `text` in `field`, counted up
    /// to `n`: a range of the exact-text index.
    fn holders(&mut self, ty: &TypeName, field: &FieldName, text: &str, n: u64) -> Result<u64> {
        let query = format!(
            "SELECT COUNT(*) FROM (SELECT 1 FROM {} \
             WHERE ty = $1 AND field = $2 AND value = $3 LIMIT $4) h",
            self.names.t("text_index")
        );
        let n = n as i64;
        let counted: Vec<i64> = self
            .db
            .column(&query, &[&ty.as_str(), &field.as_str(), &text, &n])?;
        Ok(counted.first().map_or(0, |n| n.unsigned_abs()))
    }

    /// At most `n` head records of a type without a row in the order index
    /// for `field`, in key order after `after`: those `holders` names.
    fn unsorted_after(
        &mut self,
        ty: &TypeName,
        field: &FieldName,
        holders: &engines::Holders<'_>,
        after: Option<&str>,
        n: u32,
    ) -> Result<Vec<StoredRecord>> {
        let limit = i64::from(n);
        if let Some((f, t)) = holders.text() {
            let query = unsorted_holding_sql(&self.tables(), after.is_some());
            let (ty, field) = (ty.as_str(), field.as_str());
            let args: [&(dyn ToSql + Sync); 6] = [&ty, &field, &limit, &f, &t, &after];
            let args = &args[..5 + usize::from(after.is_some())];
            return self.db.records(&query, args);
        }
        let from = after.map_or("", |_| "AND h.key > $4");
        let query = format!(
            "SELECT r.state FROM {records} WHERE h.ty = $1 {from} \
             AND NOT EXISTS (SELECT 1 FROM {sorted} s \
             WHERE s.ty = h.ty AND s.field = $2 AND s.key = h.key) \
             ORDER BY h.key LIMIT $3",
            sorted = self.names.t("sort_index"),
            records = projection::RECORDS.at_head(self.names),
        );
        let args: [&(dyn ToSql + Sync); 4] = [&ty.as_str(), &field.as_str(), &limit, &after];
        self.db
            .records(&query, &args[..3 + usize::from(after.is_some())])
    }

    /// The keys of `ty` with a state put after the view's position, and
    /// their states there; `None` past `ceiling` states put. One range read
    /// into `records_since`: a state that held at the position and ended
    /// since ended where its key's next state was put.
    fn changed(&mut self, ty: &TypeName, ceiling: usize) -> Result<Option<engines::Changed>> {
        let records = self.names.t("records");
        let args: [&(dyn ToSql + Sync); 3] = [&ty.as_str(), &self.at, &(ceiling as i64 + 1)];
        let put: Vec<String> = self.db.column(
            &format!(
                "SELECT key FROM {records} WHERE ty = $1 AND since > $2 ORDER BY since LIMIT $3"
            ),
            &args,
        )?;
        let keys: BTreeSet<String> = put.iter().cloned().collect();
        if put.len() > ceiling {
            return Ok(None);
        }
        Ok(Some(engines::Changed {
            states: self.records_by_key(ty, keys.clone())?,
            keys: keys.into_iter().map(RecordKey::from).collect(),
        }))
    }

    /// Records of a type by their keys, at the view's position, in key
    /// order: one statement, however many keys, a seek for each.
    fn records_by_key(
        &mut self,
        ty: &TypeName,
        keys: BTreeSet<String>,
    ) -> Result<Vec<StoredRecord>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let keys: Vec<String> = keys.into_iter().collect();
        self.db.records(
            &records_by_key_sql(&self.names.t("records")),
            &[&ty.as_str(), &keys, &self.at],
        )
    }

    /// The type each verb declared at the view's position is on.
    fn verbs(&mut self) -> Result<BTreeMap<VerbName, TypeName>> {
        let rows = self.db.query(
            &format!(
                "SELECT state FROM {}",
                DECLARATIONS.in_force(self.names, 1, "AND ident LIKE 'verb %'")
            ),
            &[&self.at],
        )?;
        let mut out = BTreeMap::new();
        for r in rows {
            if let Declaration::Verb(v) =
                decode::<Declared>(&r.get::<_, String>(0), "a declaration")?.declaration
            {
                out.insert(v.name, v.on);
            }
        }
        Ok(out)
    }

    /// The rows of the head's `edges` table whose `end` (`src` or `dst`)
    /// is a record of `ty` among `keys`: one statement for all of them.
    fn edge_rows(&mut self, end: &str, ty: &TypeName, keys: &[String]) -> Result<Vec<Edge>> {
        let query = format!(
            "SELECT src_ty, src_key, relation, dst_ty, dst_key FROM {} \
             WHERE {end}_ty = $1 AND {end}_key = ANY($2::text[])",
            self.names.t("edges")
        );
        Ok(self
            .db
            .query(&query, &[&ty.as_str(), &keys])?
            .iter()
            .map(|r| Edge {
                from: (r.get::<_, String>(0).into(), r.get::<_, String>(1).into()),
                relation: r.get::<_, String>(2).into(),
                to: (r.get::<_, String>(3).into(), r.get::<_, String>(4).into()),
                fields: Default::default(),
            })
            .collect())
    }

    /// Every type declared at the view's position.
    fn types(&mut self) -> Result<Vec<TypeDecl>> {
        let rows = self.db.query(
            &format!(
                "SELECT state FROM {} ORDER BY ident COLLATE \"C\"",
                DECLARATIONS.in_force(self.names, 1, "AND ident LIKE 'type %'")
            ),
            &[&self.at],
        )?;
        let mut out = Vec::new();
        for r in rows {
            out.extend(projection::declared_type(Some(r.get(0)))?);
        }
        Ok(out)
    }
}

impl ProjectionView for PgView<'_> {
    fn position(&self) -> Option<Position> {
        self.at
    }

    fn declarations(&self) -> Result<Vec<Declared>> {
        self.read(Vec::new(), |r| {
            r.db.query(
                &format!(
                    "SELECT state FROM {} ORDER BY ident COLLATE \"C\"",
                    DECLARATIONS.in_force(r.names, 1, "")
                ),
                &[&r.at],
            )?
            .iter()
            .map(|row| decode(&row.get::<_, String>(0), "a declaration"))
            .collect()
        })
    }

    fn get(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>> {
        self.single(None, |r| r.record(ty, key))
    }

    fn get_many(&self, ty: &TypeName, keys: &[RecordKey]) -> Result<Vec<StoredRecord>> {
        self.single(Vec::new(), |r| {
            r.records_by_key(ty, keys.iter().map(|k| k.to_string()).collect())
        })
    }

    fn count(&self, ty: &TypeName, scan: &Scan) -> Result<u64> {
        self.read(0, |r| {
            // At the head, and of a kept name at any position, a count that
            // selects by the flags alone is the projection's `COUNT(*)`; any
            // other is counted batch by batch.
            let counted = r.head || is_kept_name(ty);
            if counted && scan.filter.is_none() && scan.equal.is_empty() && scan.valid_at.is_none()
            {
                return r.count_in_force(ty, scan.retired, scan.restricted);
            }
            // At the head, a count that must hold a text reads the records
            // holding it, not the type.
            let decl = r.type_decl(ty)?;
            let lookup = engines::text_lookup(scan).filter(|_| r.head);
            engines::count_in_key_order(decl.as_ref(), scan, |after, n| {
                r.in_key_order(ty, &lookup, after, n)
            })
        })
    }

    fn scan(&self, ty: &TypeName, scan: &Scan) -> Result<Vec<StoredRecord>> {
        self.read(Vec::new(), |r| {
            let decl = r.type_decl(ty)?;
            let lookup = engines::text_lookup(scan).filter(|_| r.head);
            // Ordered by the key alone, the page is read from `after` in
            // key order and stops when it is full: at the head, with a text
            // the scan must hold, from the records holding it.
            if scan.order_by.is_empty() {
                return engines::scan_in_key_order(decl.as_ref(), scan, |after, n| {
                    r.in_key_order(ty, &lookup, after, n)
                });
            }
            let sorted = r.sorted_fields(ty, decl.as_ref())?;
            let records = match engines::by_order_index(&sorted, scan) {
                // Ordered by a field the lake indexes, the page is read in
                // the order index's order and stops when it is full: at the
                // head, with a text the scan must hold, of the records
                // holding it; at an earlier position, the head's index with
                // the keys changed since, and past the ceiling the sort.
                Some(first) => {
                    let (changed, holders) = if r.head {
                        let holders = engines::Holders::of(lookup.as_ref(), |f, t, n| {
                            r.holders(ty, f, t, n)
                        })?;
                        (None, holders)
                    } else {
                        let ceiling = engines::changed_ceiling(scan.limit);
                        let Some(changed) = r.changed(ty, ceiling)? else {
                            let records = r.records(ty)?;
                            return Ok(engines::scan(decl.as_ref(), &records, scan));
                        };
                        (Some(changed), engines::Holders::All)
                    };
                    let r = std::cell::RefCell::new(r);
                    let sorted = |from: Option<(&[u8], &str)>, n| {
                        r.borrow_mut().sorted_after(ty, first, &holders, from, n)
                    };
                    let missing = |after: Option<&str>, n| {
                        r.borrow_mut()
                            .unsorted_after(ty, &first.field, &holders, after, n)
                    };
                    return match &changed {
                        None => engines::scan_in_field_order(decl.as_ref(), scan, sorted, missing),
                        Some(c) => {
                            engines::scan_in_field_order_at(decl.as_ref(), scan, c, sorted, missing)
                        }
                    };
                }
                // In a field the lake does not index, every record holding
                // the text, or the type, is read and sorted.
                None => match &lookup {
                    Some((field, text)) => {
                        let query = format!(
                            "SELECT key FROM {} WHERE ty = $1 AND field = $2 AND value = $3",
                            r.names.t("text_index")
                        );
                        let keys =
                            r.db.column(&query, &[&ty.as_str(), &field.as_str(), text])?;
                        r.records_by_key(ty, keys)?
                    }
                    None => r.records(ty)?,
                },
            };
            Ok(engines::scan(decl.as_ref(), &records, scan))
        })
    }

    fn edges(
        &self,
        ty: &TypeName,
        keys: &[RecordKey],
        relation: Option<&RelationName>,
        walk: Walk,
    ) -> Result<Vec<Vec<Edge>>> {
        // One read transaction for all the keys, each of its reads one
        // statement for all of them (lakelands#227).
        self.read(vec![Vec::new(); keys.len()], |r| {
            let out = matches!(walk, Walk::Out | Walk::Both);
            let inn = matches!(walk, Walk::In | Walk::Both);
            let mut index: BTreeMap<&str, Vec<usize>> = Default::default();
            for (i, k) in keys.iter().enumerate() {
                index.entry(k.as_str()).or_default().push(i);
            }
            let names: Vec<String> = index.keys().map(|k| (*k).to_owned()).collect();
            let mut all = vec![Vec::new(); keys.len()];
            let give = |all: &mut Vec<Vec<Edge>>, at: &str, e: &Edge| {
                for &i in index.get(at).into_iter().flatten() {
                    all[i].push(e.clone());
                }
            };
            let types = r.types()?;
            if r.head {
                let wanted = |e: &Edge| relation.is_none_or(|want| *want == e.relation);
                if out {
                    for e in r.edge_rows("src", ty, &names)?.iter().filter(|e| wanted(e)) {
                        give(&mut all, e.from.1.as_str(), e);
                    }
                }
                if inn {
                    for e in r.edge_rows("dst", ty, &names)?.iter().filter(|e| wanted(e)) {
                        give(&mut all, e.to.1.as_str(), e);
                    }
                }
            } else {
                if out && let Some(decl) = r.type_decl(ty)? {
                    for record in r.records_by_key(ty, names.iter().cloned().collect())? {
                        for e in engines::links_of(&decl, &record, relation) {
                            give(&mut all, record.key.as_str(), &e);
                        }
                    }
                }
                if inn {
                    // The declaring types' records at the position, read
                    // once for the page, not once per key.
                    let mut read: BTreeMap<TypeName, Vec<StoredRecord>> = Default::default();
                    for key in index.keys() {
                        let key = RecordKey::from(*key);
                        let into = engines::in_links(ty, &key, relation, &types, |name| {
                            if !read.contains_key(name) {
                                read.insert(name.clone(), r.records(name)?);
                            }
                            Ok::<_, lakelands_api::Error>(read[name].clone())
                        })?;
                        for e in into {
                            give(&mut all, key.as_str(), &e);
                        }
                    }
                }
            }
            // Relations carried by link records: the link records whose end
            // is one of the records. At the head the exact-text index finds
            // them; at an earlier position the link type's records there are
            // read. The rulebook decides which count.
            let head = r.head;
            let text_index = r.names.t("text_index");
            let linked = lakelands_core::links::link_edges(
                ty,
                keys,
                relation,
                walk,
                &types,
                |lt, field, ks| {
                    if head {
                        let values: Vec<String> = ks.iter().map(|k| k.to_string()).collect();
                        let query = format!(
                            "SELECT key FROM {text_index} WHERE ty = $1 AND field = $2 \
                             AND value = ANY($3::text[])"
                        );
                        let found = r.db.column(&query, &[&lt.as_str(), &field, &values])?;
                        r.records_by_key(lt, found)
                    } else {
                        // The link type at the position, whole: the rulebook
                        // keeps the links whose end a record is.
                        r.records(lt)
                    }
                },
            )?;
            for (edges, linked) in all.iter_mut().zip(linked) {
                edges.extend(linked);
                engines::order_edges(edges);
            }
            Ok(all)
        })
    }

    fn lexical(
        &self,
        ty: &TypeName,
        text: &str,
        fields: Option<&[FieldName]>,
        k: u32,
    ) -> Result<Vec<(RecordKey, f64)>> {
        self.read(Vec::new(), |r| {
            // The full-text rows describe the head; a view keeps the hits
            // live at its own position (`docs/seams.md`).
            let Some(decl) = projection::type_at(r.db, r.names, ty, None)? else {
                return Ok(Vec::new());
            };
            let query: Vec<String> = engines::tokens(text, decl.language.as_deref().unwrap_or(""))
                .into_iter()
                .collect();
            if query.is_empty() {
                return Ok(Vec::new());
            }
            // The candidates are the records holding any query token: the
            // set the shared scoring counts. The rows do not say which
            // field a token came from: limited to named fields, these are
            // a superset, and the shared scoring keeps the records whose
            // named fields match before it cuts at `k`.
            let sql = format!(
                "SELECT DISTINCT key FROM {} WHERE ty = $1 AND token = ANY($2::text[])",
                r.names.t("full_text")
            );
            let keys: BTreeSet<String> = r.db.column(&sql, &[&ty.as_str(), &query])?;
            let mut head = Vec::with_capacity(keys.len());
            for key in keys {
                let key = RecordKey::from(key);
                if let Some(record) = projection::head_record(r.db, r.names, ty, &key)? {
                    let then = if r.head {
                        Some(record.clone())
                    } else {
                        r.record(ty, &key)?
                    };
                    head.push((record, engines::searchable(then.as_ref())));
                }
            }
            let head = head.iter().map(|(state, live)| (state, *live));
            Ok(engines::lexical(&decl, head, text, fields, k))
        })
    }

    fn proposals(&self, filter: &ProposalFilter) -> Result<Vec<Proposal>> {
        self.read(Vec::new(), |r| {
            let all: Vec<Proposal> =
                r.db.query(
                    &format!("SELECT state FROM {}", PROPOSALS.in_force(r.names, 1, "")),
                    &[&r.at],
                )?
                .iter()
                .map(|row| decode(&row.get::<_, String>(0), "a proposal"))
                .collect::<Result<_>>()?;
            let verbs = r.verbs()?;
            Ok(engines::proposals(&all, filter, |v| verbs.get(v).cloned()))
        })
    }

    fn is_member(&self, set: &str, ty: &TypeName, key: &RecordKey) -> Result<bool> {
        self.single(false, |r| {
            Ok(r.db
                .query_opt(
                    &format!(
                        "SELECT state FROM {} WHERE object_set = $1 AND ty = $2 AND key = $3 \
                         AND since <= $4 ORDER BY since DESC LIMIT 1",
                        r.names.t("members"),
                    ),
                    &[&set, &ty.as_str(), &key.as_str(), &r.at],
                )?
                .is_some_and(|row| row.get::<_, bool>(0)))
        })
    }

    fn members(&self, set: &str, ty: &TypeName) -> Result<BTreeSet<RecordKey>> {
        self.single(BTreeSet::new(), |r| {
            let rows = r.db.query(
                &format!(
                    "SELECT key FROM {} WHERE state",
                    MEMBERS.in_force(r.names, 3, "AND object_set = $1 AND ty = $2")
                ),
                &[&set, &ty.as_str(), &r.at],
            )?;
            Ok(rows
                .iter()
                .map(|row| RecordKey::from(row.get::<_, String>(0)))
                .collect())
        })
    }

    fn matching(
        &self,
        filter: &FeedFilter,
        from: Position,
        through: Position,
        limit: u32,
    ) -> Result<Matched> {
        let to = feed::decides_through(filter, from, through);
        // `routes` and the folded marker are read in one transaction, under
        // the lock that holds a `clear` back (module doc).
        self.read_locked(Some("routes"), feed::undecided(), |r| {
            let folded = r.folded.map(|f| Position(f as u64));
            let Some(last) = feed::last_decided(from, to.min(Position(r.at as u64)), folded) else {
                return Ok(feed::undecided());
            };
            let strs = |v: Vec<&str>| v.into_iter().map(str::to_owned).collect::<Vec<String>>();
            let kinds = strs(filter.kinds.iter().map(String::as_str).collect());
            let types = strs(filter.types.iter().map(TypeName::as_str).collect());
            let keys = strs(filter.keys.iter().map(RecordKey::as_str).collect());
            let (by, values) = match feed::seek(filter) {
                Some(Seek::Keys(_)) => (Some("key"), &keys),
                Some(Seek::Types(_)) => (Some("ty"), &types),
                Some(Seek::Kinds(_)) => (Some("kind"), &kinds),
                None => (None, &kinds),
            };
            let (from, last_at, n) = (from.0 as i64, last.0 as i64, i64::from(limit.max(1)));
            let query = projection::routes_sql(r.names, by);
            let rows = (r.db).query(
                &query,
                &[values, &from, &last_at, &n, &kinds, &types, &keys],
            )?;
            let positions = rows.iter().map(|row| Position(row.get::<_, i64>(0) as u64));
            Ok(feed::merged([positions.collect()], limit, last))
        })
    }

    fn applied(&self, id: &EventId) -> Result<Option<Position>> {
        self.single(None, |r| {
            Ok(r.db
                .query_opt(
                    &format!(
                        "SELECT at FROM {} WHERE id = $1 AND at <= $2",
                        r.names.t("applied")
                    ),
                    &[&id.0, &r.at],
                )?
                .map(|row| Position(row.get::<_, i64>(0) as u64)))
        })
    }

    fn output(&self, seam: &str, input: &Digest) -> Result<Option<Value>> {
        self.single(None, |r| {
            r.db.query_opt(
                &output_sql(&r.names.t("outputs")),
                &[&seam, &input.0, &r.at],
            )?
            .map(|row| decode(&row.get::<_, String>(0), "an output"))
            .transpose()
        })
    }
}
