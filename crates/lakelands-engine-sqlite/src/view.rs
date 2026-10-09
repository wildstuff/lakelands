//! Reads of the projection at one position.
//!
//! Every read runs in one read transaction on a reader connection, so it
//! sees one committed state of the projection, and reads the history rows
//! that hold at the view's position. When that position is the one the
//! projection reflects (the head), a read may find its candidates through
//! the head indexes; otherwise it reads the states at the position
//! directly. Either way `lakelands_core::engines` decides what matches,
//! where it sorts and what it scores, exactly as on every other engine.

use std::collections::BTreeSet;

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
use lakelands_core::engines::{self, Holders, decode};
use lakelands_core::feed::{self, Seek};
use rusqlite::{Connection, ToSql, params};

use crate::handle::SqliteLake;
use crate::projection::{self, column, decode_record, holds_at, one};
use crate::sql;

/// A read view of one lake's projection at one position.
pub(crate) struct SqliteView<'a> {
    pub(crate) lake: &'a SqliteLake,
    pub(crate) at: Option<Position>,
}

/// One read: a connection in a read transaction, the head in the state it
/// sees, and whether the view's position is that head.
struct Read<'c> {
    conn: &'c Connection,
    at: Position,
    folded: Option<Position>,
    head: bool,
}

/// The bound on the key `column` that every key-ordered page's statement
/// carries: after the key bound as the parameter `after` numbers, or for
/// the first page from the empty key, which every key passes. With no
/// bound the planner, which has no statistics, may read a type through an
/// index in another order and sort it for the page (lakelands#580).
fn key_from(column: &str, after: Option<usize>) -> String {
    match after {
        Some(n) => format!("AND {column} > ?{n}"),
        None => format!("AND {column} >= ''"),
    }
}

/// The statement that reads a kept name (`lakelands_api::seam::KEPT_PREFIX`):
/// `select` of its states in force at the position `?2`, from the key
/// that `after` numbers, with `tail` after them. A kept record is put at
/// every write to its group, so a key may hold thousands of states, and a
/// stored type's statements read every state of the keys they pass. At
/// the head this reads the states with no end, in key order through
/// `records_at`. At an earlier position it steps from key to key through
/// the primary key and seeks each key's latest state at or before the
/// position: the steps come in key order, which the statement answers in
/// with no `ORDER BY`, and `step` is joined first, so a limit in `tail`
/// counts the states answered and not the keys stepped over.
fn kept_sql(head: bool, select: &str, after: Option<usize>, tail: &str) -> String {
    if head {
        let from = key_from("r.key", after);
        return format!(
            "SELECT {select} FROM records r WHERE r.ty = ?1 AND r.until IS NULL {from} {tail}"
        );
    }
    format!(
        "WITH RECURSIVE step(key) AS (\
         SELECT (SELECT key FROM records WHERE ty = ?1 {} ORDER BY key LIMIT 1) \
         UNION ALL \
         SELECT (SELECT key FROM records WHERE ty = ?1 AND key > step.key ORDER BY key LIMIT 1) \
         FROM step WHERE step.key IS NOT NULL) \
         SELECT {select} FROM step CROSS JOIN records r \
         WHERE r.rowid = (SELECT rowid FROM records WHERE ty = ?1 AND key = step.key \
         AND since <= ?2 ORDER BY since DESC LIMIT 1) AND (r.until IS NULL OR r.until > ?2) {tail}",
        key_from("key", after)
    )
}

/// The statement of [`Read::records_after`]: for a kept name
/// [`kept_sql`]'s states, at most `?3` and every one when that is
/// negative, the view at the head or not.
fn records_after_sql(ty: &TypeName, head: bool, after: bool) -> String {
    if is_kept_name(ty) {
        let tail = if head { "ORDER BY r.key " } else { "" };
        return kept_sql(
            head,
            "r.state",
            after.then_some(4),
            &format!("{tail}LIMIT ?3"),
        );
    }
    format!(
        "SELECT state FROM records WHERE ty = ?1 AND {} {} ORDER BY key LIMIT ?3",
        holds_at(2),
        key_from("key", after.then_some(4))
    )
}

/// The statement of [`Read::count_in_force`]: the states in force of a
/// stored type, or [`kept_sql`]'s of a kept name, under the two flags.
fn count_in_force_sql(ty: &TypeName, head: bool) -> String {
    let flags = |state: &str| {
        format!(
            "AND (?3 OR json_extract({state}, '$.retired') = 0) \
             AND (?4 OR json_extract({state}, '$.restricted') = 0)"
        )
    };
    if is_kept_name(ty) {
        return kept_sql(head, "COUNT(*)", None, &flags("r.state"));
    }
    let holds = holds_at(2);
    format!(
        "SELECT COUNT(*) FROM records WHERE ty = ?1 AND {holds} {}",
        flags("state")
    )
}

/// The statement of [`Read::in_key_order`] with a lookup: the index's
/// rows first, in key order and cut at the limit, then their records.
/// Every index row is written from a head state in the same transaction,
/// so the join keeps them all. Joined before the cut, the planner seeks
/// `records` by a range of the key for every index row.
fn holding_after_sql(after: bool) -> String {
    format!(
        "SELECT r.state FROM \
         (SELECT t.key FROM text_index t \
          WHERE t.ty = ?1 AND t.field = ?2 AND t.value = ?4 {} ORDER BY t.key LIMIT ?3) t \
         JOIN records r ON r.ty = ?1 AND r.key = t.key AND r.until IS NULL \
         ORDER BY t.key",
        key_from("t.key", after.then_some(5))
    )
}

/// The statement of [`Read::unsorted_after`].
fn unsorted_after_sql(after: bool) -> String {
    format!(
        "SELECT r.state FROM records r WHERE r.ty = ?1 AND r.until IS NULL {} \
         AND NOT EXISTS (SELECT 1 FROM sort_index s \
         WHERE s.ty = r.ty AND s.field = ?2 AND s.key = r.key) \
         ORDER BY r.key LIMIT ?3",
        key_from("r.key", after.then_some(4))
    )
}

/// The statement of [`Read::sorted_after`]: the order index's rows of
/// `?2` for the records `holders` names, `?4` and `?5` the field and text
/// they hold, and then the sort key and key bound to seek past when `from`
/// is set. Many holders are read in the index's order, each kept where the
/// text index holds its key; few are read from the text index and sorted.
fn sorted_after_sql(dir: Direction, from: bool, holders: &Holders<'_>) -> String {
    let (k, key) = match holders {
        Holders::All => (4, 5),
        _ => (6, 7),
    };
    let (seek, by) = match dir {
        Direction::Asc => (
            format!("AND (s.sort_key, s.key) > (?{k}, ?{key})"),
            "s.sort_key, s.key",
        ),
        Direction::Desc => (
            format!("AND (s.sort_key < ?{k} OR (s.sort_key = ?{k} AND s.key > ?{key}))"),
            "s.sort_key DESC, s.key",
        ),
    };
    let seek = if from { seek.as_str() } else { "" };
    let rows = match holders {
        Holders::All => "sort_index s WHERE s.ty = ?1 AND s.field = ?2",
        Holders::Many(..) => {
            "sort_index s WHERE s.ty = ?1 AND s.field = ?2 AND EXISTS (SELECT 1 FROM text_index t \
             WHERE t.ty = ?1 AND t.field = ?4 AND t.value = ?5 AND t.key = s.key)"
        }
        Holders::Few(..) => {
            "text_index t CROSS JOIN sort_index s WHERE t.ty = ?1 AND t.field = ?4 \
             AND t.value = ?5 AND s.ty = ?1 AND s.key = t.key AND s.field = ?2"
        }
    };
    format!(
        "SELECT s.sort_key, r.state FROM \
         (SELECT s.sort_key, s.key FROM {rows} {seek} ORDER BY {by} LIMIT ?3) s \
         JOIN records r ON r.ty = ?1 AND r.key = s.key AND r.until IS NULL \
         ORDER BY {by}"
    )
}

/// The statement of [`Read::unsorted_after`] for the records holding the
/// text `?5` in `?4`: the text index's rows in key order, those with an
/// order row for `?2` passed over, cut at the limit before their records
/// are joined, as in [`holding_after_sql`].
fn unsorted_holding_sql(after: bool) -> String {
    format!(
        "SELECT r.state FROM \
         (SELECT t.key FROM text_index t \
          WHERE t.ty = ?1 AND t.field = ?4 AND t.value = ?5 {} \
          AND NOT EXISTS (SELECT 1 FROM sort_index s \
          WHERE s.ty = ?1 AND s.field = ?2 AND s.key = t.key) \
          ORDER BY t.key LIMIT ?3) t \
         JOIN records r ON r.ty = ?1 AND r.key = t.key AND r.until IS NULL \
         ORDER BY t.key",
        key_from("t.key", after.then_some(6))
    )
}

/// The statement of [`Read::holders`]: how many records hold the text,
/// counted up to `?4`.
const HOLDERS: &str = "SELECT COUNT(*) FROM (SELECT 1 FROM text_index \
     WHERE ty = ?1 AND field = ?2 AND value = ?3 LIMIT ?4)";

/// The statement of [`Read::record`]: the latest state of the key at or
/// before the position, and whether it still holds there. No other state
/// of the key is read (lakelands#596).
const RECORD_AT: &str = "SELECT state FROM (SELECT state, until FROM records WHERE ty = ?1 \
     AND key = ?2 AND since <= ?3 ORDER BY since DESC LIMIT 1) WHERE until IS NULL OR until > ?3";

/// The statement of a recorded output's read: the latest state of the
/// seam and input at or before the position, and whether it still holds
/// there. A view's state is recorded again at every change of its
/// counters; none of its other states is read (lakelands#575).
const OUTPUT_AT: &str = "SELECT state FROM (SELECT state, until FROM outputs WHERE seam = ?1 \
     AND input = ?2 AND since <= ?3 ORDER BY since DESC LIMIT 1) WHERE until IS NULL OR until > ?3";

/// The statements that seek one record's state, as the engine runs them:
/// the read of a record at a position and the end a put gives the state
/// before it, each with the parameters type, key and position, for the
/// engine's own test of what they cost. Neither reads the key's other
/// states (lakelands#596).
pub fn seek_statements() -> [String; 2] {
    [RECORD_AT.to_owned(), projection::RECORDS.ends_latest()]
}

/// The statement of each first page the view reads from an index in the
/// page's order, and of the read of one record, as the view runs it, for
/// the engine's own test of their plans: none may sort (lakelands#580).
/// A page under an exact text many records hold is among them; one few
/// hold sorts them, at most [`engines::FEW_HOLDERS`].
pub fn first_page_statements() -> Vec<String> {
    let field = FieldName::from("f");
    let many = Holders::Many(&field, "x");
    vec![
        RECORD_AT.to_owned(),
        records_after_sql(&TypeName::from("t"), true, false),
        holding_after_sql(false),
        unsorted_after_sql(false),
        sorted_after_sql(Direction::Asc, false, &Holders::All),
        sorted_after_sql(Direction::Desc, false, &Holders::All),
        sorted_after_sql(Direction::Asc, false, &many),
        sorted_after_sql(Direction::Desc, false, &many),
        unsorted_holding_sql(false),
    ]
}

/// The statements the view reads the records of `ty` by, as it runs them
/// at the head (`head`) or at an earlier position: a first page by key, a
/// later page and the count under the flags alone; and the read of a
/// recorded output. For the engine's own tests of their plans and of the
/// steps they take for a kept name (lakelands#575). A page takes the
/// name, the position, the limit and the key it continues after; the
/// count the name, the position and the two flags; the output its seam,
/// input and position.
pub fn read_statements(ty: &TypeName, head: bool) -> [String; 4] {
    [
        records_after_sql(ty, head, false),
        records_after_sql(ty, head, true),
        count_in_force_sql(ty, head),
        OUTPUT_AT.to_owned(),
    ]
}

impl SqliteView<'_> {
    /// Run `f` in one read transaction at the view's position; an empty
    /// projection answers `empty`.
    fn read<R>(&self, empty: R, f: impl FnOnce(&Read<'_>) -> Result<R>) -> Result<R> {
        let Some(at) = self.at else {
            return Ok(empty);
        };
        let conn = crate::lock(&self.lake.reader);
        conn.execute_batch("BEGIN").map_err(sql)?;
        let result = projection::folded(&conn).and_then(|folded| {
            f(&Read {
                conn: &conn,
                at,
                folded,
                head: folded == Some(at),
            })
        });
        let _ = conn.execute_batch("COMMIT");
        result
    }
}

impl Read<'_> {
    fn at(&self) -> i64 {
        self.at.0 as i64
    }

    fn type_decl(&self, ty: &TypeName) -> Result<Option<TypeDecl>> {
        projection::type_at(self.conn, ty, Some(self.at))
    }

    fn record(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>> {
        let state: Option<String> = one(
            self.conn,
            RECORD_AT,
            params![ty.as_str(), key.as_str(), self.at()],
        )?;
        state.as_deref().map(decode_record).transpose()
    }

    /// Every record of a type at the view's position, in key order.
    fn records(&self, ty: &TypeName) -> Result<Vec<StoredRecord>> {
        if is_kept_name(ty) {
            let statement = records_after_sql(ty, self.head, false);
            return self.page(&statement, &[&ty.as_str(), &self.at(), &-1], None);
        }
        column(
            self.conn,
            &format!(
                "SELECT state FROM records WHERE ty = ?1 AND {} ORDER BY key",
                holds_at(2)
            ),
            params![ty.as_str(), self.at()],
            |s: String| decode_record(&s),
        )
    }

    /// At most `n` records of a type at the view's position after the key
    /// `after` (from the first when `None`), in key order: a seek into the
    /// primary key `(ty, key, since)`, which sorts bytewise as keys do. The
    /// first page and a later one are two statements, so that neither
    /// hides the seek behind an `OR`.
    fn records_after(
        &self,
        ty: &TypeName,
        after: Option<&str>,
        n: u32,
    ) -> Result<Vec<StoredRecord>> {
        self.page(
            &records_after_sql(ty, self.head, after.is_some()),
            &[&ty.as_str(), &self.at(), &n],
            after,
        )
    }

    /// What a key-ordered scan or count reads from, at most `n` records
    /// in key order after `after`: with a `lookup`, the head records whose
    /// field holds exactly its text, a seek into the exact-text index's
    /// primary key `(ty, field, value, key)`; else the type's records. The
    /// index names candidates; the rulebook admits each.
    fn in_key_order(
        &self,
        ty: &TypeName,
        lookup: &Option<(FieldName, String)>,
        after: Option<&str>,
        n: u32,
    ) -> Result<Vec<StoredRecord>> {
        let Some((field, text)) = lookup else {
            return self.records_after(ty, after, n);
        };
        self.page(
            &holding_after_sql(after.is_some()),
            &[&ty.as_str(), &field.as_str(), &n, text],
            after,
        )
    }

    /// How many records of a type are in force at the view's position, the
    /// retired and the restricted ones only when asked, their flags read
    /// from the stored state.
    fn count_in_force(&self, ty: &TypeName, retired: bool, restricted: bool) -> Result<u64> {
        let counted: Vec<i64> = column(
            self.conn,
            &count_in_force_sql(ty, self.head),
            params![ty.as_str(), self.at(), retired, restricted],
            Ok,
        )?;
        Ok(counted.first().map_or(0, |n| n.unsigned_abs()))
    }

    /// The fields of `ty` whose order index answers this read: none unless
    /// the index is kept through the head. At an earlier position it is
    /// read with the keys changed since ([`Read::changed`]).
    fn sorted_fields(&self, ty: &TypeName, decl: Option<&TypeDecl>) -> Result<Vec<FieldName>> {
        if self.folded.is_none() || projection::sorted(self.conn, ty)? != self.folded {
            return Ok(Vec::new());
        }
        projection::sorted_fields(self.conn, ty, decl)
    }

    /// The keys of `ty` with a state put or ended after the view's
    /// position, and their states there; `None` past `ceiling` states put
    /// or keys ended. Two range reads, one into `records_since` for the
    /// states put, one into `records_at` for the states that held at the
    /// position and ended: a `UNION` of the two is planned as a merge in
    /// key order, which reads the type.
    fn changed(&self, ty: &TypeName, ceiling: usize) -> Result<Option<engines::Changed>> {
        let args = params![ty.as_str(), self.at(), ceiling as i64 + 1];
        let put: Vec<String> = column(
            self.conn,
            "SELECT key FROM records WHERE ty = ?1 AND since > ?2 ORDER BY since LIMIT ?3",
            args,
            Ok,
        )?;
        let ended: Vec<String> = column(
            self.conn,
            "SELECT key FROM records WHERE ty = ?1 AND until > ?2 AND since <= ?2 ORDER BY until LIMIT ?3",
            args,
            Ok,
        )?;
        let keys: BTreeSet<String> = put.iter().chain(&ended).cloned().collect();
        if put.len().max(ended.len()).max(keys.len()) > ceiling {
            return Ok(None);
        }
        Ok(Some(engines::Changed {
            states: self.records_by_key(ty, keys.clone())?,
            keys: keys.into_iter().map(RecordKey::from).collect(),
        }))
    }

    /// At most `n` head records of a type with a row in the order index for
    /// `order`'s field, with their sort keys, in the index's order for its
    /// direction, strictly after `from`: a seek into `sort_index_asc` or
    /// `sort_index_desc`.
    fn sorted_after(
        &self,
        ty: &TypeName,
        order: &Order,
        holders: &Holders<'_>,
        from: Option<(&[u8], &str)>,
        n: u32,
    ) -> Result<Vec<(Vec<u8>, StoredRecord)>> {
        let mut stmt = self
            .conn
            .prepare_cached(&sorted_after_sql(order.dir, from.is_some(), holders))
            .map_err(sql)?;
        let (ty, field) = (ty.as_str(), order.field.as_str());
        let mut args: Vec<&dyn ToSql> = vec![&ty, &field, &n];
        let holding = holders.text();
        if let Some((f, t)) = &holding {
            args.extend([f as &dyn ToSql, t]);
        }
        let (k, key) = from.unzip();
        if from.is_some() {
            args.extend([&k as &dyn ToSql, &key]);
        }
        let row = |r: &rusqlite::Row<'_>| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?));
        let rows: Vec<(Vec<u8>, String)> = stmt
            .query_map(args.as_slice(), row)
            .and_then(Iterator::collect)
            .map_err(sql)?;
        rows.into_iter()
            .map(|(k, state)| Ok((k, decode_record(&state)?)))
            .collect()
    }

    /// How many head records of a type hold `text` in `field`, counted up
    /// to `n`: a range of the exact-text index.
    fn holders(&self, ty: &TypeName, field: &FieldName, text: &str, n: u64) -> Result<u64> {
        let counted: Option<i64> = one(
            self.conn,
            HOLDERS,
            params![ty.as_str(), field.as_str(), text, n as i64],
        )?;
        Ok(counted.map_or(0, i64::unsigned_abs))
    }

    /// At most `n` head records of a type without a row in the order index
    /// for `field`, in key order after `after`: those `holders` names.
    fn unsorted_after(
        &self,
        ty: &TypeName,
        field: &FieldName,
        holders: &Holders<'_>,
        after: Option<&str>,
        n: u32,
    ) -> Result<Vec<StoredRecord>> {
        if let Some((f, t)) = holders.text() {
            return self.page(
                &unsorted_holding_sql(after.is_some()),
                &[&ty.as_str(), &field.as_str(), &n, &f, &t],
                after,
            );
        }
        self.page(
            &unsorted_after_sql(after.is_some()),
            &[&ty.as_str(), &field.as_str(), &n],
            after,
        )
    }

    /// The records a page's statement answers, `after` bound as its last
    /// parameter when there is one; decoded once all are read.
    fn page(
        &self,
        statement: &str,
        args: &[&dyn ToSql],
        after: Option<&str>,
    ) -> Result<Vec<StoredRecord>> {
        let mut args = args.to_vec();
        args.extend(after.as_ref().map(|a| a as &dyn ToSql));
        let rows: Vec<String> = column(self.conn, statement, args.as_slice(), Ok)?;
        rows.iter().map(|r| decode_record(r)).collect()
    }

    /// Records of a type by their keys, at the view's position.
    fn records_by_key(&self, ty: &TypeName, keys: BTreeSet<String>) -> Result<Vec<StoredRecord>> {
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            if let Some(r) = self.record(ty, &RecordKey::from(key))? {
                out.push(r);
            }
        }
        Ok(out)
    }

    fn keys(&self, query: &str, args: &[&dyn ToSql]) -> Result<BTreeSet<String>> {
        column(self.conn, query, args, Ok)
    }

    fn verb_on(&self, verb: &VerbName) -> Option<TypeName> {
        let state: String = one(
            self.conn,
            &format!(
                "SELECT state FROM declarations WHERE ident = ?1 AND {}",
                holds_at(2)
            ),
            params![format!("verb {verb}"), self.at()],
        )
        .ok()??;
        match decode::<Declared>(&state, "a state").ok()?.declaration {
            Declaration::Verb(v) => Some(v.on),
            _ => None,
        }
    }

    /// The head's edges whose `end` (`src` or `dst`) is the record.
    fn edge_rows(&self, end: &str, ty: &TypeName, key: &RecordKey) -> Result<Vec<Edge>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!(
                "SELECT src_ty, src_key, relation, dst_ty, dst_key FROM edges \
                 WHERE {end}_ty = ?1 AND {end}_key = ?2"
            ))
            .map_err(sql)?;
        let rows = stmt
            .query_map(params![ty.as_str(), key.as_str()], |r| {
                Ok(Edge {
                    from: (r.get::<_, String>(0)?.into(), r.get::<_, String>(1)?.into()),
                    relation: r.get::<_, String>(2)?.into(),
                    to: (r.get::<_, String>(3)?.into(), r.get::<_, String>(4)?.into()),
                    fields: Default::default(),
                })
            })
            .map_err(sql)?;
        rows.map(|r| r.map_err(sql)).collect()
    }

    /// Every type declared at the view's position.
    fn types(&self) -> Result<Vec<TypeDecl>> {
        let types: Vec<Option<TypeDecl>> = column(
            self.conn,
            &format!(
                "SELECT state FROM declarations WHERE ident LIKE 'type %' AND {} ORDER BY ident",
                holds_at(1)
            ),
            params![self.at()],
            |s: String| match decode::<Declared>(&s, "a state")?.declaration {
                Declaration::Type(t) => Ok(Some(t)),
                _ => Ok(None),
            },
        )?;
        Ok(types.into_iter().flatten().collect())
    }
}

impl ProjectionView for SqliteView<'_> {
    fn position(&self) -> Option<Position> {
        self.at
    }

    fn declarations(&self) -> Result<Vec<Declared>> {
        self.read(Vec::new(), |r| {
            column(
                r.conn,
                &format!(
                    "SELECT state FROM declarations WHERE {} ORDER BY ident",
                    holds_at(1)
                ),
                params![r.at()],
                |s: String| decode(&s, "a state"),
            )
        })
    }

    fn get(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>> {
        self.read(None, |r| r.record(ty, key))
    }

    fn get_many(&self, ty: &TypeName, keys: &[RecordKey]) -> Result<Vec<StoredRecord>> {
        self.read(Vec::new(), |r| {
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
                // Ordered by a field the lake indexes, at the head, the page
                // is read in the order index's order and stops when it is
                // full: with a text the scan must hold, of the records
                // holding it.
                Some(first) if r.head => {
                    let holders = Holders::of(lookup.as_ref(), |f, t, n| {
                        r.holders(ty, f, t, n)
                    })?;
                    return engines::scan_in_field_order(
                        decl.as_ref(),
                        scan,
                        |from, n| r.sorted_after(ty, first, &holders, from, n),
                        |after, n| r.unsorted_after(ty, &first.field, &holders, after, n),
                    );
                }
                // At an earlier position, the head's index with the keys
                // changed since; past the ceiling, the sort.
                Some(first) => match r.changed(ty, engines::changed_ceiling(scan.limit))? {
                    Some(changed) => {
                        return engines::scan_in_field_order_at(
                            decl.as_ref(),
                            scan,
                            &changed,
                            |from, n| r.sorted_after(ty, first, &Holders::All, from, n),
                            |after, n| r.unsorted_after(ty, &first.field, &Holders::All, after, n),
                        );
                    }
                    None => r.records(ty)?,
                },
                // In a field the lake does not index, every record holding
                // the text, or the type, is read and sorted.
                None => match &lookup {
                    Some((field, text)) => {
                        let keys = r.keys(
                            "SELECT key FROM text_index WHERE ty = ?1 AND field = ?2 AND value = ?3",
                            &[&ty.as_str(), &field.as_str(), text],
                        )?;
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
        // One read transaction for all the keys; its statements are
        // in-process, so each key keeps its own.
        self.read(vec![Vec::new(); keys.len()], |r| {
            let out = matches!(walk, Walk::Out | Walk::Both);
            let inn = matches!(walk, Walk::In | Walk::Both);
            let types = r.types()?;
            let mut all = Vec::with_capacity(keys.len());
            for key in keys {
                let mut edges = Vec::new();
                if r.head {
                    if out {
                        edges.extend(r.edge_rows("src", ty, key)?);
                    }
                    if inn {
                        edges.extend(r.edge_rows("dst", ty, key)?);
                    }
                    edges.retain(|e| relation.is_none_or(|want| *want == e.relation));
                } else {
                    if out
                        && let (Some(decl), Some(record)) = (r.type_decl(ty)?, r.record(ty, key)?)
                    {
                        edges.extend(engines::links_of(&decl, &record, relation));
                    }
                    if inn {
                        edges.extend(engines::in_links(ty, key, relation, &types, |name| {
                            r.records(name)
                        })?);
                    }
                }
                all.push(edges);
            }
            // Relations carried by link records: the link records whose end
            // is one of the records. At the head the exact-text index finds
            // them; at an earlier position the link type's records there are
            // read. The rulebook decides which count.
            let linked = lakelands_core::links::link_edges(
                ty,
                keys,
                relation,
                walk,
                &types,
                |lt, field, ks| {
                    if r.head {
                        let mut found = BTreeSet::new();
                        for k in ks {
                            found.extend(r.keys(
                                "SELECT key FROM text_index WHERE ty = ?1 AND field = ?2 AND value = ?3",
                                &[&lt.as_str(), &field, &k.as_str()],
                            )?);
                        }
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
            // The full-text index describes the head; a view keeps the hits
            // live at its own position (`docs/seams.md`).
            let Some(decl) = projection::type_at(r.conn, ty, None)? else {
                return Ok(Vec::new());
            };
            let query = engines::tokens(text, decl.language.as_deref().unwrap_or(""));
            if query.is_empty() {
                return Ok(Vec::new());
            }
            // Every pipeline token is one term of the table (its hex), so
            // this finds exactly the records the shared scoring counts. A
            // row holds every searchable field's tokens: limited to named
            // fields, these are a superset, and the shared scoring keeps
            // the records whose named fields match before it cuts at `k`.
            let matching = query
                .iter()
                .map(|t| projection::fts_term(t))
                .collect::<Vec<_>>()
                .join(" OR ");
            let keys = r.keys(
                "SELECT s.key FROM full_text f JOIN search_rows s ON s.id = f.rowid \
                 WHERE full_text MATCH ?1 AND s.ty = ?2",
                &[&matching, &ty.as_str()],
            )?;
            let mut head = Vec::with_capacity(keys.len());
            for key in keys {
                let key = RecordKey::from(key);
                if let Some(record) = projection::head_record(r.conn, ty, &key)? {
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
            let all: Vec<Proposal> = column(
                r.conn,
                &format!("SELECT state FROM proposals WHERE {}", holds_at(1)),
                params![r.at()],
                |s: String| decode(&s, "a state"),
            )?;
            Ok(engines::proposals(&all, filter, |v| r.verb_on(v)))
        })
    }

    fn is_member(&self, set: &str, ty: &TypeName, key: &RecordKey) -> Result<bool> {
        self.read(false, |r| {
            one(
                r.conn,
                &format!(
                    "SELECT state FROM members WHERE object_set = ?1 AND ty = ?2 AND key = ?3 AND {}",
                    holds_at(4)
                ),
                params![set, ty.as_str(), key.as_str(), r.at()],
            )
            .map(|m| m.unwrap_or(false))
        })
    }

    fn members(&self, set: &str, ty: &TypeName) -> Result<BTreeSet<RecordKey>> {
        self.read(BTreeSet::new(), |r| {
            column(
                r.conn,
                &format!(
                    "SELECT key FROM members WHERE object_set = ?1 AND ty = ?2 AND state AND {}",
                    holds_at(3)
                ),
                params![set, ty.as_str(), r.at()],
                |key: String| Ok(RecordKey::from(key)),
            )
        })
    }

    fn applied(&self, id: &EventId) -> Result<Option<Position>> {
        self.read(None, |r| {
            one(
                r.conn,
                "SELECT at FROM applied WHERE id = ?1 AND at <= ?2",
                params![id.0, r.at()],
            )
            .map(|p: Option<i64>| p.map(|p| Position(p as u64)))
        })
    }

    fn output(&self, seam: &str, input: &Digest) -> Result<Option<Value>> {
        self.read(None, |r| {
            let state: Option<String> = one(r.conn, OUTPUT_AT, params![seam, input.0, r.at()])?;
            state.as_deref().map(|s| decode(s, "a state")).transpose()
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
        self.read(feed::undecided(), |r| {
            let Some(last) = feed::last_decided(from, to.min(r.at), r.folded) else {
                return Ok(feed::undecided());
            };
            let range = [from, last];
            let by = |column: &'static str, values: Vec<&str>| -> Result<Vec<Vec<Position>>> {
                let of = |v| projection::routes(r.conn, filter, Some((column, v)), range, limit);
                values.into_iter().map(of).collect()
            };
            let lists = match feed::seek(filter) {
                Some(Seek::Keys(keys)) => by("key", keys.iter().map(RecordKey::as_str).collect()),
                Some(Seek::Types(types)) => by("ty", types.iter().map(TypeName::as_str).collect()),
                Some(Seek::Kinds(kinds)) => by("kind", kinds.iter().map(String::as_str).collect()),
                None => Ok(vec![projection::routes(
                    r.conn, filter, None, range, limit,
                )?]),
            }?;
            Ok(feed::merged(lists, limit, last))
        })
    }
}
