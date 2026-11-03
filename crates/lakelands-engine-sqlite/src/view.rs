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

use lakelands_api::engine::{
    Chosen, Declared, HolderBatch, KeyRange, Matched, ProjectionView, Rankings, Scan, WindowAt,
    WindowBatch, WindowSides,
};
use lakelands_api::error::Result;
use lakelands_api::event::{Digest, EventId};
use lakelands_api::id::{FieldName, Position, RecordKey, RelationName, SourceId, TypeName};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::quarantine::{QuarantineFilter, QuarantineState, Quarantined};
use lakelands_api::query::{Edge, Walk};
use lakelands_api::read::{Direction, Order, ThinnedAct};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::seam::{FeedFilter, is_kept_name};
use lakelands_api::time::Timestamp;
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

/// The texts a key-ordered read looks up, and else the text it narrows by.
type Asked<'a> = (
    &'a Option<(FieldName, Vec<String>)>,
    &'a Option<(FieldName, String)>,
);

/// The field a window's walk narrows by, and the texts it must hold.
type WindowText = Option<(FieldName, Vec<String>)>;

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

/// The predicates on the key `column` that the interval `range` makes,
/// each end bound from the parameter `at` up in the order `from`, `until`
/// (`docs/decisions/103-a-filter-on-a-kept-view-narrows-by-the-group-key.md`).
fn key_range(column: &str, range: Option<&KeyRange>, at: usize) -> String {
    let Some(range) = range else {
        return String::new();
    };
    let mut sql = String::new();
    let mut n = at;
    if range.from.is_some() {
        sql.push_str(&format!(" AND {column} >= ?{n}"));
        n += 1;
    }
    if range.until.is_some() {
        sql.push_str(&format!(" AND {column} < ?{n}"));
    }
    sql
}

/// How many parameters [`key_range`] binds.
fn key_range_len(range: Option<&KeyRange>) -> usize {
    range.map_or(0, |r| {
        usize::from(r.from.is_some()) + usize::from(r.until.is_some())
    })
}

/// The statement that reads a kept name (`lakelands_api::seam::KEPT_PREFIX`):
/// `select` of its states in force at the position `?2`, from the key
/// that `after` numbers, inside the interval `range` whose ends the
/// parameters from `range_at` up hold, with `tail` after them. A kept
/// record is put at every write to its group, so a key may hold thousands
/// of states, and a stored type's statements read every state of the keys
/// they pass. Both walk the name's keys in key order and seek one state of
/// each: at the head the state with no end, through `records_at`; at an
/// earlier position the latest at or before it, through the primary key.
/// The keys are joined first, so a limit in `tail` counts the states
/// answered.
fn kept_sql(
    head: bool,
    select: &str,
    after: Option<usize>,
    range: Option<&KeyRange>,
    range_at: usize,
    tail: &str,
) -> String {
    let mut from = key_from("k.key", after);
    from.push_str(&key_range("k.key", range, range_at));
    if head {
        return format!(
            "SELECT {select} FROM keys k CROSS JOIN records r \
             ON r.ty = k.ty AND r.until IS NULL AND r.id = k.id WHERE k.ty = ?1 {from} {tail}"
        );
    }
    format!(
        "SELECT {select} FROM keys k CROSS JOIN records r \
         WHERE k.ty = ?1 {from} AND r.rowid = (SELECT rowid FROM records WHERE id = k.id \
         AND since <= ?2 ORDER BY since DESC LIMIT 1) AND (r.until IS NULL OR r.until > ?2) {tail}"
    )
}

/// The statement of [`Read::records_after`]: for a kept name
/// [`kept_sql`]'s states, at most `?3` and every one when that is
/// negative, the view at the head or not, inside `range` where a read
/// narrowed by the key. `narrow`ed, a stored type's states whose field at
/// the JSON path `?4` is the text `?5`: the scan by a text the exact-text
/// index keeps no rows for.
fn records_after_sql(
    ty: &TypeName,
    head: bool,
    after: bool,
    narrow: bool,
    range: Option<&KeyRange>,
) -> String {
    // The range's ends are bound where the statement binds none, and the
    // page's `after` follows them ([`Read::page`] binds it last).
    if is_kept_name(ty) {
        return kept_sql(
            head,
            "r.ty, k.key, r.since, r.state",
            after.then_some(4 + key_range_len(range)),
            range,
            4,
            "ORDER BY k.key LIMIT ?3",
        );
    }
    let (text, base) = match narrow {
        true => ("AND json_extract(r.state, ?4) = ?5", 5),
        false => ("", 3),
    };
    format!(
        "SELECT r.ty, k.key, r.since, r.state FROM keys k CROSS JOIN records r ON r.id = k.id \
         WHERE k.ty = ?1 AND r.{} {text} {} {} ORDER BY k.key LIMIT ?3",
        holds_at(2),
        key_range("k.key", range, base + 1),
        key_from("k.key", after.then_some(base + 1 + key_range_len(range)))
    )
}

/// The statement of [`Read::count_in_force`]: the states in force of a
/// stored type, or [`kept_sql`]'s of a kept name, under the two flags,
/// which the row holds beside its state ([`projection::flags_of`]), and
/// inside `range` where a read narrowed by the key.
fn count_in_force_sql(ty: &TypeName, head: bool, range: Option<&KeyRange>) -> String {
    let flags =
        |row: &str| format!("AND (?3 OR {row}flags & 1 = 0) AND (?4 OR {row}flags & 2 = 0)");
    if is_kept_name(ty) {
        return kept_sql(head, "COUNT(*)", None, range, 5, &flags("r."));
    }
    let holds = holds_at(2);
    format!(
        "SELECT COUNT(*) FROM records WHERE ty = ?1 AND {holds} {}",
        flags("")
    )
}

/// The statement of [`Read::in_key_order`] with a lookup: the index's
/// rows first, in key order and cut at the limit, then their records.
/// Every index row is written from a head state in the same transaction,
/// so the join keeps them all. Joined before the cut, the planner seeks
/// `records` by a range of the key for every index row.
fn holding_after_sql(after: bool) -> String {
    format!(
        "SELECT r.ty, t.key, r.since, r.state FROM \
         (SELECT t.key FROM text_index t \
          WHERE t.ty = ?1 AND t.field = ?2 AND t.value = ?4 {} ORDER BY t.key LIMIT ?3) t \
         CROSS JOIN keys k ON k.ty = ?1 AND k.key = t.key \
         CROSS JOIN records r ON r.ty = ?1 AND r.until IS NULL AND r.id = k.id ORDER BY t.key",
        key_from("t.key", after.then_some(5))
    )
}

/// The keys of the head records whose field holds one of a list of texts,
/// in key order, after `?4` where it is bound: driven from the list, each
/// text a seek into the index's primary key `(ty, field, value, key)`, so
/// the type is never walked (`CROSS JOIN` keeps the list outside).
fn holding_any_sql(after: bool) -> String {
    format!(
        "SELECT t.key FROM json_each(?3) j CROSS JOIN text_index t \
         ON t.ty = ?1 AND t.field = ?2 AND t.value = j.value {} ORDER BY t.key",
        key_from("t.key", after.then_some(4))
    )
}

/// The head records of a list of keys, in the list's order.
const HEADS_OF: &str = "SELECT r.ty, k.key, r.since, r.state FROM json_each(?2) j \
     CROSS JOIN keys k ON k.ty = ?1 AND k.key = j.value \
     CROSS JOIN records r ON r.ty = ?1 AND r.until IS NULL AND r.id = k.id ORDER BY j.id";

/// The statement of [`Read::unsorted_after`].
fn unsorted_after_sql(after: bool) -> String {
    format!(
        concat!(
            projection::heads!(),
            " {} AND NOT EXISTS (SELECT 1 FROM sort_index s \
             WHERE s.ty = ?1 AND s.field = ?2 AND s.key = k.key) ORDER BY k.key LIMIT ?3"
        ),
        key_from("k.key", after.then_some(4))
    )
}

/// The seek past the sort key `?k` and key `?key` in the order index's
/// order for `dir`, and that order. The index is kept ascending: a window
/// down walks it backwards and sorts each run of one sort key by its key,
/// so ties still ascend; the bound `s.sort_key <= ?k` is where the walk
/// starts.
fn seek_in(dir: Direction, k: usize, key: usize) -> (String, &'static str) {
    match dir {
        Direction::Asc => (
            format!("AND (s.sort_key, s.key) > (?{k}, ?{key})"),
            "s.sort_key, s.key",
        ),
        Direction::Desc => (
            format!(
                "AND s.sort_key <= ?{k} \
                 AND (s.sort_key < ?{k} OR (s.sort_key = ?{k} AND s.key > ?{key}))"
            ),
            "s.sort_key DESC, s.key",
        ),
    }
}

/// The statement of [`Read::sorted_after`]: the order index's rows of
/// `?2` for the records `holders` names, `?4` and `?5` the field and text
/// they hold, and then the sort key and key bound to seek past when `from`
/// is set. Many holders are read in the index's order, each kept where the
/// text index holds its key; few are read from the text index and sorted.
fn sorted_after_sql(
    dir: Direction,
    from: bool,
    holders: &Holders<'_>,
    window: Option<&engines::Window>,
) -> String {
    let (k, key) = match holders {
        Holders::All => (4, 5),
        _ => (6, 7),
    };
    let (seek, by) = seek_in(dir, k, key);
    let seek = if from { seek.as_str() } else { "" };
    // The window's ends follow the parameters the statement does bind.
    let at = match holders {
        Holders::All if from => 6,
        Holders::All => 4,
        _ if from => 8,
        _ => 6,
    };
    let window = window_sql(window, at, at + 1);
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
        "SELECT s.sort_key, r.ty, k.key, r.since, r.state FROM \
         (SELECT s.sort_key, s.key FROM {rows} {seek} {window} ORDER BY {by} LIMIT ?3) s \
         CROSS JOIN keys k ON k.ty = ?1 AND k.key = s.key \
         CROSS JOIN records r ON r.ty = ?1 AND r.until IS NULL AND r.id = k.id \
         ORDER BY {by}"
    )
}

/// The statement's bound on the rows it reads, when the read is narrowed
/// by a filter's window: `s.sort_key` between the window's ends, bound as
/// `?lo` and `?hi`. Empty for a read with no window.
fn window_sql(window: Option<&engines::Window>, lo: usize, hi: usize) -> String {
    match window {
        Some(_) => format!("AND s.sort_key >= ?{lo} AND s.sort_key <= ?{hi}"),
        None => String::new(),
    }
}

/// The stop of a walk counted for [`engines::rows_walked`]: rows up to
/// the last one handed, in the order `dir` walks for `seek_in`. A
/// descending walk's stop is the mirror of its seek.
fn stop_in(dir: Direction, k: usize, key: usize) -> String {
    match dir {
        Direction::Asc => format!("AND (s.sort_key, s.key) <= (?{k}, ?{key})"),
        Direction::Desc => format!(
            "AND s.sort_key >= ?{k} \
             AND (s.sort_key > ?{k} OR (s.sort_key = ?{k} AND s.key <= ?{key}))"
        ),
    }
}

/// The statement of [`Read::few_after`]'s one read a page: the order
/// rows of `?2` of every record holding the text `?4` in `?3`, read from
/// the text index and sorted, after the sort key and key `?5` and `?6`.
/// Index rows only, at most [`engines::FEW_HOLDERS`].
fn holders_in_order_sql(dir: Direction, window: Option<&engines::Window>) -> String {
    let (seek, by) = seek_in(dir, 5, 6);
    let window = window_sql(window, 7, 8);
    format!(
        "SELECT s.sort_key, s.key FROM text_index t CROSS JOIN sort_index s \
         WHERE t.ty = ?1 AND t.field = ?3 AND t.value = ?4 \
         AND s.ty = ?1 AND s.key = t.key AND s.field = ?2 {seek} {window} ORDER BY {by}"
    )
}

/// The statement counting the order rows a page under a text many records
/// hold walked, for [`engines::rows_walked`]: rows of `?2` whose keys the
/// text index holds for `?3` and `?4`, from the seek beginning at the
/// parameter `from`, to the stop beginning at `last` when the batch was
/// full, inside the window beginning at `window`. Each part's parameters
/// are the pair beginning at its index.
fn walked_sql(
    dir: Direction,
    from: Option<usize>,
    last: Option<usize>,
    window: Option<usize>,
) -> String {
    let seek = from.map_or_else(String::new, |k| seek_in(dir, k, k + 1).0);
    let stop = last.map_or_else(String::new, |k| stop_in(dir, k, k + 1));
    let window = window.map_or_else(String::new, |k| {
        format!("AND s.sort_key >= ?{k} AND s.sort_key <= ?{}", k + 1)
    });
    format!(
        "SELECT COUNT(*) FROM sort_index s WHERE s.ty = ?1 AND s.field = ?2 \
         AND EXISTS (SELECT 1 FROM text_index t \
         WHERE t.ty = ?1 AND t.field = ?3 AND t.value = ?4 AND t.key = s.key) \
         {seek} {stop} {window}"
    )
}

/// The statement of [`Read::few_after`]'s seek of one head record.
const HEAD_RECORD: &str = concat!(projection::heads!(), " AND k.key = ?2");

/// A page under a text few records hold: the batches it has read by
/// statements of their own, and the order rows of the holders after them,
/// read once and held, with how many of those its batches have taken.
#[derive(Default)]
struct Held {
    batches: u32,
    rows: Option<Vec<(Vec<u8>, String)>>,
    taken: usize,
}

/// The statement of [`Read::unsorted_after`] for the records holding the
/// text `?5` in `?4`: the text index's rows in key order, those with an
/// order row for `?2` passed over, cut at the limit before their records
/// are joined, as in [`holding_after_sql`].
fn unsorted_holding_sql(after: bool) -> String {
    format!(
        "SELECT r.ty, t.key, r.since, r.state FROM \
         (SELECT t.key FROM text_index t \
          WHERE t.ty = ?1 AND t.field = ?4 AND t.value = ?5 {} \
          AND NOT EXISTS (SELECT 1 FROM sort_index s \
          WHERE s.ty = ?1 AND s.field = ?2 AND s.key = t.key) \
          ORDER BY t.key LIMIT ?3) t \
         CROSS JOIN keys k ON k.ty = ?1 AND k.key = t.key \
         CROSS JOIN records r ON r.ty = ?1 AND r.until IS NULL AND r.id = k.id ORDER BY t.key",
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
pub(crate) const RECORD_AT: &str = "SELECT ty, key, since, state FROM (SELECT r.ty, k.key, r.since, \
     r.state, r.until FROM keys k CROSS JOIN records r ON r.id = k.id WHERE k.ty = ?1 \
     AND k.key = ?2 AND r.since <= ?3 ORDER BY r.since DESC LIMIT 1) \
     WHERE until IS NULL OR until > ?3";

/// The statement of a recorded output's read: the latest state of the
/// seam and input at or before the position, and whether it still holds
/// there. A view's state is recorded again at every change of its
/// counters; none of its other states is read (lakelands#575).
const OUTPUT_AT: &str = "SELECT since, state FROM (SELECT since, state, until FROM outputs WHERE seam = ?1 \
     AND input = ?2 AND since <= ?3 ORDER BY since DESC LIMIT 1) WHERE until IS NULL OR until > ?3";

/// The statements that seek one record's state, as the engine runs them:
/// the read of a record at a position and the end a put gives the state
/// before it, the one with the parameters type, key and position, the
/// other with the key's id and the position, for the engine's own test of
/// what they cost. Neither reads the key's other
/// states (lakelands#596).
pub fn seek_statements() -> [String; 2] {
    [RECORD_AT.to_owned(), projection::RECORDS.ends_latest()]
}

/// The plan of the feed read the view makes for the keys and types of
/// `filter` through `matching`, as it runs it, for the engine's own test
/// of the index it takes: the plan of the read by `key`, one line a step.
#[doc(hidden)]
pub fn keyed_routes_plan(
    conn: &rusqlite::Connection,
    filter: &FeedFilter,
    key: &str,
) -> Result<Option<Vec<String>>> {
    projection::routes_plan(
        conn,
        filter,
        Some(("key", key)),
        [Position(1), Position(u64::MAX >> 1)],
        100,
    )
}

/// The statement of each first page the view reads from an index in the
/// page's order, and of the read of one record, as the view runs it, for
/// the engine's own test of their plans: none may sort (lakelands#580).
/// A page under an exact text many records hold is among them; one few
/// hold sorts them, at most [`engines::FEW_HOLDERS`], and a long page of
/// it reads them once ([`few_holders_statements`]).
pub fn first_page_statements() -> Vec<String> {
    let field = FieldName::from("f");
    let many = Holders::Many(&field, "x");
    vec![
        RECORD_AT.to_owned(),
        records_after_sql(&TypeName::from("t"), true, false, false, None),
        holding_after_sql(false),
        unsorted_after_sql(false),
        sorted_after_sql(Direction::Asc, false, &Holders::All, None),
        sorted_after_sql(Direction::Desc, false, &Holders::All, None),
        sorted_after_sql(Direction::Asc, false, &many, None),
        sorted_after_sql(Direction::Desc, false, &many, None),
        unsorted_holding_sql(false),
    ]
}

/// The statements of a long page under an exact text few records hold
/// ([`Read::few_after`]), as the view runs them, for the engine's own test
/// of their plans: the one read of the holders' order rows after a row,
/// in the order either way, and the seek of one head record. The read
/// starts at the text's rows and seeks each one's order row by its key,
/// so it reads the holders and never the type; it sorts what it read, at
/// most [`engines::FEW_HOLDERS`] rows.
pub fn few_holders_statements() -> [String; 3] {
    [
        holders_in_order_sql(Direction::Asc, None),
        holders_in_order_sql(Direction::Desc, None),
        HEAD_RECORD.to_owned(),
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
        records_after_sql(ty, head, false, false, None),
        records_after_sql(ty, head, true, false, None),
        count_in_force_sql(ty, head, None),
        OUTPUT_AT.to_owned(),
    ]
}

/// The statements a page and a count of `ty` inside a key interval are, as
/// the engine runs them, for the engine's own test of their plans: a kept
/// name's read is a seek of `keys`' primary key inside the interval as
/// well (`docs/decisions/103-a-filter-on-a-kept-view-narrows-by-the-group-key.md`).
/// A page takes the name, the position, the limit and the interval's ends;
/// the count the name, the position, the two flags and the ends.
pub fn range_statements(ty: &TypeName, head: bool) -> [String; 2] {
    let range = KeyRange {
        from: Some(RecordKey::from("a")),
        until: Some(RecordKey::from("z")),
    };
    [
        records_after_sql(ty, head, false, false, Some(&range)),
        count_in_force_sql(ty, head, Some(&range)),
    ]
}

/// The full-text hits of `asked`'s text over `ty` as the index holds
/// them, at the head (`docs/seams.md` §`ranked`), read through `conn`.
pub(crate) fn lexical(
    conn: &Connection,
    ty: &TypeName,
    asked: &Rankings,
    k: u32,
) -> Result<Vec<(RecordKey, f64)>> {
    let (Some(decl), Some((text, _))) = (projection::type_at(conn, ty, None)?, &asked.text) else {
        return Ok(Vec::new());
    };
    let query = engines::tokens(text, decl.language.as_deref().unwrap_or(""));
    if query.is_empty() {
        return Ok(Vec::new());
    }
    // Every pipeline token is one term of the table (its hex), so this
    // finds exactly the records the shared scoring counts. A row holds
    // every searchable field's tokens: limited to named fields, these are
    // a superset, and the shared scoring keeps the records whose named
    // fields match before it cuts at `k`.
    let matching = query
        .iter()
        .map(|t| projection::fts_term(t))
        .collect::<Vec<_>>()
        .join(" OR ");
    let keys: BTreeSet<String> = column(
        conn,
        "SELECT s.key FROM full_text f JOIN search_rows s ON s.id = f.rowid \
         WHERE full_text MATCH ?1 AND s.ty = ?2",
        params![matching, ty.as_str()],
        Ok,
    )?;
    let mut head = Vec::with_capacity(keys.len());
    for key in keys {
        head.extend(projection::head_record(conn, ty, &RecordKey::from(key))?);
    }
    let kept = |held| projection::text_counts(conn, ty, &decl, held);
    engines::ranked_text(&decl, &head, asked, k, kept)
}

impl SqliteView<'_> {
    /// Run `f` in one read transaction at the view's position; an empty
    /// projection answers `empty`.
    fn read<R>(&self, empty: R, f: impl FnOnce(&Read<'_>) -> Result<R>) -> Result<R> {
        let Some(at) = self.at else {
            return Ok(empty);
        };
        let conn = self.lake.reader()?;
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
        let at = params![ty.as_str(), key.as_str(), self.at()];
        Ok(projection::records(self.conn, RECORD_AT, at)?.pop())
    }

    /// Every record of a type at the view's position, in key order.
    fn records(&self, ty: &TypeName) -> Result<Vec<StoredRecord>> {
        if is_kept_name(ty) {
            let statement = records_after_sql(ty, self.head, false, false, None);
            return self.page(&statement, &[&ty.as_str(), &self.at(), &-1], None);
        }
        projection::records(
            self.conn,
            &format!(
                "SELECT r.ty, k.key, r.since, r.state FROM keys k CROSS JOIN records r \
                 ON r.id = k.id WHERE k.ty = ?1 AND r.{} ORDER BY k.key",
                holds_at(2)
            ),
            params![ty.as_str(), self.at()],
        )
    }

    /// At most `n` records of a type at the view's position after the key
    /// `after` (from the first when `None`), in key order: a seek into the
    /// primary key `(ty, key)` of `keys`, which sorts bytewise as keys do,
    /// and of each key's state by its id. The
    /// first page and a later one are two statements, so that neither
    /// hides the seek behind an `OR`.
    fn records_after(
        &self,
        ty: &TypeName,
        narrow: Option<&(FieldName, String)>,
        after: Option<&str>,
        range: Option<&KeyRange>,
        n: u32,
    ) -> Result<Vec<StoredRecord>> {
        let narrow = narrow.filter(|_| !is_kept_name(ty));
        let path = narrow.map(|(f, _)| format!("$.fields.\"{f}\""));
        let (name, at) = (ty.as_str(), self.at());
        let mut args: Vec<&dyn ToSql> = vec![&name, &at, &n];
        args.extend(path.as_ref().map(|p| p as &dyn ToSql));
        args.extend(narrow.map(|(_, t)| t as &dyn ToSql));
        let from = range.and_then(|r| r.from.as_ref()).map(RecordKey::as_str);
        let until = range.and_then(|r| r.until.as_ref()).map(RecordKey::as_str);
        args.extend(from.as_ref().map(|k| k as &dyn ToSql));
        args.extend(until.as_ref().map(|k| k as &dyn ToSql));
        let statement = records_after_sql(ty, self.head, after.is_some(), narrow.is_some(), range);
        self.page(&statement, &args, after)
    }

    /// The fields of `ty` the exact-text index keeps; `None`, every field,
    /// for a kept name or off the head, where no read looks a text up.
    fn looked(
        &self,
        ty: &TypeName,
        decl: Option<&TypeDecl>,
    ) -> Result<Option<BTreeSet<FieldName>>> {
        match decl {
            Some(_) if self.head => projection::looked_fields(self.conn, ty).map(Some),
            _ => Ok(None),
        }
    }

    /// What a key-ordered scan or count reads from, at most `n` records
    /// in key order after `after`: with a `lookup`, the head records whose
    /// field holds exactly one of its texts; else the type's records. One
    /// text is a seek into the exact-text index's primary key `(ty, field,
    /// value, key)` a batch. Several are looked up once for the whole scan
    /// (`held`), the keys in key order, and each batch reads its records by
    /// key: a batch never sorts what is left, so the read grows with the
    /// matches, not their square. The index names candidates; the rulebook
    /// admits each.
    fn in_key_order(
        &self,
        ty: &TypeName,
        (lookup, narrow): Asked<'_>,
        held: &mut Option<Vec<String>>,
        after: Option<&str>,
        range: Option<&KeyRange>,
        n: u32,
    ) -> Result<Vec<StoredRecord>> {
        let Some((field, texts)) = lookup else {
            return self.records_after(ty, narrow.as_ref(), after, range, n);
        };
        if let [text] = texts.as_slice() {
            return self.page(
                &holding_after_sql(after.is_some()),
                &[&ty.as_str(), &field.as_str(), &n, text],
                after,
            );
        }
        let keys = match held {
            Some(keys) => keys,
            None => {
                let texts = serde_json::to_string(texts).expect("texts serialise");
                let (ty, field) = (ty.as_str(), field.as_str());
                let mut args: Vec<&dyn ToSql> = vec![&ty, &field, &texts];
                args.extend(after.as_ref().map(|a| a as &dyn ToSql));
                let sql = holding_any_sql(after.is_some());
                let mut keys: Vec<String> = column(self.conn, &sql, args.as_slice(), Ok)?;
                keys.dedup();
                held.insert(keys)
            }
        };
        let from = after.map_or(0, |a| keys.partition_point(|k| k.as_str() <= a));
        let page: Vec<&String> = keys[from..].iter().take(n as usize).collect();
        let page = serde_json::to_string(&page).expect("keys serialise");
        self.page(HEADS_OF, &[&ty.as_str(), &page], None)
    }

    /// How many records of a type are in force at the view's position, the
    /// retired and the restricted ones only when asked, their flags read
    /// from the stored state.
    fn count_in_force(
        &self,
        ty: &TypeName,
        retired: bool,
        restricted: bool,
        range: Option<&KeyRange>,
    ) -> Result<u64> {
        let (name, at) = (ty.as_str(), self.at());
        let from = range.and_then(|r| r.from.as_ref()).map(RecordKey::as_str);
        let until = range.and_then(|r| r.until.as_ref()).map(RecordKey::as_str);
        let mut args: Vec<&dyn ToSql> = vec![&name, &at, &retired, &restricted];
        args.extend(from.as_ref().map(|k| k as &dyn ToSql));
        args.extend(until.as_ref().map(|k| k as &dyn ToSql));
        let counted: Vec<i64> = column(
            self.conn,
            &count_in_force_sql(ty, self.head, range),
            args.as_slice(),
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
        projection::sorted_fields(self.conn, ty, decl, false)
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
            "SELECT k.key FROM records r CROSS JOIN keys k ON k.id = r.id \
             WHERE r.ty = ?1 AND r.since > ?2 ORDER BY r.since LIMIT ?3",
            args,
            Ok,
        )?;
        let ended: Vec<String> = column(
            self.conn,
            "SELECT k.key FROM records r CROSS JOIN keys k ON k.id = r.id \
             WHERE r.ty = ?1 AND r.until > ?2 AND r.since <= ?2 ORDER BY r.until LIMIT ?3",
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
    /// direction, strictly after `from`: a seek into `sort_index_asc`, walked
    /// up or down, or, of few holders of a text, the text index's rows
    /// sorted.
    fn sorted_after(
        &self,
        ty: &TypeName,
        order: &Order,
        holders: &Holders<'_>,
        window: Option<&engines::Window>,
        from: Option<(&[u8], &str)>,
        n: u32,
    ) -> Result<Vec<(Vec<u8>, StoredRecord)>> {
        let mut stmt = self
            .conn
            .prepare_cached(&sorted_after_sql(
                order.dir,
                from.is_some(),
                holders,
                window,
            ))
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
        if let Some(w) = window {
            args.extend([&w.lo as &dyn ToSql, &w.hi]);
        }
        let row =
            |r: &rusqlite::Row<'_>| Ok((r.get::<_, Vec<u8>>(0)?, projection::record_at(r, 1)?));
        let rows: Vec<(Vec<u8>, projection::Kept4)> = stmt
            .query_map(args.as_slice(), row)
            .and_then(Iterator::collect)
            .map_err(sql)?;
        self.count_walked(ty, order, holders, window, from, n, &rows)?;
        rows.into_iter()
            .map(|(k, state)| Ok((k, decode_record(self.conn, &state)?)))
            .collect()
    }

    /// Counts the index rows the statement [`Read::sorted_after`] just ran
    /// walked, for [`engines::rows_walked`]: a range of the order index
    /// walks what it hands; a text's few holders walk their own rows once
    /// a statement; a text's many holders run a `count(*)` of the same
    /// range, from the seek to the last row handed or, on a short batch,
    /// to the range's end, only while a check asked for walks.
    #[allow(clippy::too_many_arguments)]
    fn count_walked(
        &self,
        ty: &str,
        order: &Order,
        holders: &Holders<'_>,
        window: Option<&engines::Window>,
        from: Option<(&[u8], &str)>,
        n: u32,
        rows: &[(Vec<u8>, projection::Kept4)],
    ) -> Result<()> {
        match holders {
            Holders::All => engines::count_walk(rows.len()),
            Holders::Few(_, _, held) => engines::count_walk(*held as usize),
            Holders::Many(..) if engines::counting_walks() => {
                let last = (rows.len() == n as usize)
                    .then(|| rows.last())
                    .flatten()
                    .map(|(k, (_, key, _, _))| (k.clone(), key.clone()));
                let field = order.field.as_str();
                let (f, t) = holders.text().unwrap_or_default();
                let mut args: Vec<&dyn ToSql> = vec![&ty, &field, &f, &t];
                let mut next = 5;
                let (k, key) = from.unzip();
                if from.is_some() {
                    args.extend([&k as &dyn ToSql, &key]);
                    next += 2;
                }
                let from_at = from.is_some().then_some(5);
                let (lk, lkey) = last.as_ref().map_or((None, None), |(k, key)| {
                    (Some(k.as_slice()), Some(key.as_str()))
                });
                let last_at = last.is_some().then_some(next);
                if last.is_some() {
                    args.extend([&lk as &dyn ToSql, &lkey]);
                    next += 2;
                }
                let window_at = window.is_some().then_some(next);
                if let Some(w) = window {
                    args.extend([&w.lo as &dyn ToSql, &w.hi]);
                }
                let walked: Option<i64> = one(
                    self.conn,
                    &walked_sql(order.dir, from_at, last_at, window_at),
                    args.as_slice(),
                )?;
                engines::count_walk(walked.unwrap_or(0).unsigned_abs() as usize);
            }
            Holders::Many(..) => {}
        }
        Ok(())
    }

    /// At most `n` of the head records holding the text `holders` names, in
    /// the order index's order for `order`, as [`Read::sorted_after`]
    /// answers them, and by it for a page's first
    /// [`engines::BATCHES_BEFORE_HELD`] batches. A page that asks for one
    /// more reads the order rows of the holders after `from`, the last row
    /// it read, once, and holds them; that batch and every later one takes
    /// the next `n` and seeks their records
    /// (`docs/decisions/041-a-sorted-page-reads-its-page.md`, extended).
    /// The batches and the held rows are of one snapshot: the page's read
    /// transaction.
    #[allow(clippy::too_many_arguments)]
    fn few_after(
        &self,
        ty: &TypeName,
        order: &Order,
        holders: &Holders<'_>,
        window: Option<&engines::Window>,
        held: &mut Held,
        from: Option<(&[u8], &str)>,
        n: u32,
    ) -> Result<Vec<(Vec<u8>, StoredRecord)>> {
        // A batch with no row to continue after is a page's first.
        let (Some((k, key)), true) = (from, held.batches >= engines::BATCHES_BEFORE_HELD) else {
            held.batches += 1;
            return self.sorted_after(ty, order, holders, window, from, n);
        };
        if held.rows.is_none() {
            let mut stmt = self
                .conn
                .prepare_cached(&holders_in_order_sql(order.dir, window))
                .map_err(sql)?;
            let (f, t) = holders.text().unwrap_or_default();
            let (name, field) = (ty.as_str(), order.field.as_str());
            let row = |r: &rusqlite::Row<'_>| Ok((r.get(0)?, r.get(1)?));
            let mut args: Vec<&dyn ToSql> = vec![&name, &field, &f, &t, &k, &key];
            if let Some(w) = window {
                args.extend([&w.lo as &dyn ToSql, &w.hi]);
            }
            let rows = stmt
                .query_map(args.as_slice(), row)
                .and_then(Iterator::collect)
                .map_err(sql)?;
            if let Holders::Few(_, _, held_rows) = holders {
                engines::count_walk(*held_rows as usize);
            }
            engines::count_holder_read();
            (held.rows, held.taken) = (Some(rows), 0);
        }
        let rows = held.rows.as_deref().unwrap_or_default();
        let end = (held.taken + n as usize).min(rows.len());
        let mut out = Vec::with_capacity(end - held.taken);
        for (k, key) in &rows[held.taken..end] {
            let head = projection::records(self.conn, HEAD_RECORD, params![ty.as_str(), key])?;
            out.extend(head.into_iter().map(|r| (k.clone(), r)));
        }
        held.taken = end;
        Ok(out)
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

    /// The field and texts a window walk narrows by, as the scan asks for
    /// them and the exact-text index keeps them: `Some(None)` where the
    /// scan holds no text. `None` where it holds one the index keeps none
    /// of: no walk can answer it.
    fn window_text(
        &self,
        ty: &TypeName,
        decl: Option<&TypeDecl>,
        scan: &Scan,
    ) -> Result<Option<WindowText>> {
        let looked = self.looked(ty, decl)?;
        let kept = |f: &FieldName| looked.as_ref().is_none_or(|l| l.contains(f));
        let kind = |f: &FieldName| decl.and_then(|d| engines::field_kind(d, f)).copied();
        if engines::text_lookups(scan, &|_| true, &kind).is_some()
            && engines::text_lookups(scan, &kept, &kind).is_none()
        {
            return Ok(None);
        }
        Ok(Some(engines::text_lookups(scan, &kept, &kind)))
    }

    /// At most `n` order rows of `window.field` inside the window after
    /// the index position `after`, with the keys among them holding the
    /// scan's exact text: a walk of `n` rows of the ascending order index,
    /// each row kept where the text index holds its key. The walk is the
    /// batch: a short one was the whole window, and where it stopped is
    /// the last row it passed, whether that row holds the text or not.
    fn window_keys(
        &self,
        ty: &TypeName,
        window: &engines::Window,
        scan: &Scan,
        after: Option<&WindowAt>,
        n: u32,
    ) -> Result<Option<WindowBatch>> {
        let Some(decl) = self.type_decl(ty)? else {
            return Ok(None);
        };
        if !self.head || !self.sorted_fields(ty, Some(&decl))?.contains(&window.field) {
            return Ok(None);
        }
        let Some(text) = self.window_text(ty, Some(&decl), scan)? else {
            return Ok(None);
        };
        let (ty_name, field) = (ty.as_str(), window.field.as_str());
        let after_key: Option<String> = after.map(|a| a.key.to_string());
        let mut args: Vec<&dyn ToSql> = vec![&ty_name, &field, &window.lo, &window.hi];
        let mut next = 5;
        let mut walk = String::from(
            "SELECT s.sort_key, s.key FROM sort_index s WHERE s.ty = ?1 AND s.field = ?2 \
             AND s.sort_key >= ?3 AND s.sort_key <= ?4",
        );
        if let Some(at) = after {
            walk.push_str(&format!(
                " AND (s.sort_key, s.key) > (?{next}, ?{})",
                next + 1
            ));
            args.extend([&at.sort_key as &dyn ToSql, &after_key]);
            next += 2;
        }
        walk.push_str(&format!(" ORDER BY s.sort_key, s.key LIMIT ?{next}"));
        args.push(&n);
        let text_field: Option<String> = text.as_ref().map(|(f, _)| f.to_string());
        let text_json: Option<String> = text
            .as_ref()
            .map(|(_, t)| serde_json::to_string(t).expect("the texts serialise"));
        let query = if let Some((_, texts)) = &text {
            args.push(text_field.as_ref().expect("a text's field") as &dyn ToSql);
            let (field_at, value_at) = (next + 1, next + 2);
            if texts.len() == 1 {
                args.push(&texts[0] as &dyn ToSql);
                format!(
                    "SELECT w.sort_key, w.key, EXISTS (SELECT 1 FROM text_index t WHERE t.ty = ?1 \
                     AND t.field = ?{field_at} AND t.value = ?{value_at} AND t.key = w.key) \
                     FROM ({walk}) w ORDER BY w.sort_key, w.key"
                )
            } else {
                args.push(text_json.as_ref().expect("the texts") as &dyn ToSql);
                format!(
                    "SELECT w.sort_key, w.key, EXISTS (SELECT 1 FROM text_index t WHERE t.ty = ?1 \
                     AND t.field = ?{field_at} AND t.value IN (SELECT value FROM json_each(?{value_at})) \
                     AND t.key = w.key) FROM ({walk}) w ORDER BY w.sort_key, w.key"
                )
            }
        } else {
            format!("SELECT w.sort_key, w.key, 1 FROM ({walk}) w ORDER BY w.sort_key, w.key")
        };
        let mut stmt = self.conn.prepare_cached(&query).map_err(sql)?;
        let rows: Vec<(Vec<u8>, String, i64)> = stmt
            .query_map(args.as_slice(), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .and_then(Iterator::collect)
            .map_err(sql)?;
        engines::count_walk(rows.len());
        Ok(Some(WindowBatch {
            done: rows.len() < n as usize,
            after: rows.last().map(|(sort_key, key, _)| WindowAt {
                sort_key: sort_key.clone(),
                key: key.as_str().into(),
            }),
            keys: rows
                .into_iter()
                .filter(|(_, _, holds)| *holds != 0)
                .map(|(_, key, _)| key.as_str().into())
                .collect(),
        }))
    }

    /// One round of the choice: the window's index rows counted to `l + 1`,
    /// and the other side, the text's holders or the type's rows, to four
    /// times the window's count. Index-only.
    fn window_sides(
        &self,
        ty: &TypeName,
        window: &engines::Window,
        scan: &Scan,
        l: u64,
    ) -> Result<Option<WindowSides>> {
        let Some(decl) = self.type_decl(ty)? else {
            return Ok(None);
        };
        if !self.head || !self.sorted_fields(ty, Some(&decl))?.contains(&window.field) {
            return Ok(None);
        }
        let Some(text) = self.window_text(ty, Some(&decl), scan)? else {
            return Ok(None);
        };
        let bound = i64::try_from(l.saturating_add(1)).unwrap_or(i64::MAX);
        let (ty_name, field) = (ty.as_str(), window.field.as_str());
        // The window's side is the rows its road walks, the index's rows in
        // the window, whether the scan holds a text or not: the walk pays
        // for each of them and the probe only says which to read. A count
        // of the text's holders inside would price the road by its answer,
        // and take it where it walks the whole type.
        let q = "SELECT COUNT(*) FROM (SELECT s.key FROM sort_index s \
                 WHERE s.ty = ?1 AND s.field = ?2 AND s.sort_key >= ?3 \
                 AND s.sort_key <= ?4 LIMIT ?5) x";
        let args: [&dyn ToSql; 5] = [&ty_name, &field, &window.lo, &window.hi, &bound];
        let inside = self.counted(q, &args)?;
        let whole = inside <= l;
        let cap = i64::try_from(inside.saturating_mul(engines::WINDOW_RATIO)).unwrap_or(i64::MAX);
        let other = match &text {
            None => {
                let q = "SELECT COUNT(*) FROM (SELECT s.key FROM sort_index s \
                         WHERE s.ty = ?1 AND s.field = ?2 LIMIT ?3) x";
                let args: [&dyn ToSql; 3] = [&ty_name, &field, &cap];
                self.counted(q, &args)?
            }
            Some((text_field, texts)) if texts.len() == 1 => {
                let q = "SELECT COUNT(*) FROM (SELECT t.key FROM text_index t \
                         WHERE t.ty = ?1 AND t.field = ?2 AND t.value = ?3 LIMIT ?4) x";
                let args: [&dyn ToSql; 4] = [&ty_name, &text_field.as_str(), &texts[0], &cap];
                self.counted(q, &args)?
            }
            Some((text_field, texts)) => {
                let q = "SELECT COUNT(*) FROM (SELECT t.key FROM text_index t \
                         WHERE t.ty = ?1 AND t.field = ?2 AND t.value IN \
                         (SELECT value FROM json_each(?3)) LIMIT ?4) x";
                let keyed = serde_json::to_string(texts).expect("the texts serialise");
                let args: [&dyn ToSql; 4] = [&ty_name, &text_field.as_str(), &keyed, &cap];
                self.counted(q, &args)?
            }
        };
        engines::count_walk(inside as usize + other as usize);
        Ok(Some(WindowSides {
            inside,
            whole,
            other,
        }))
    }

    /// A statement's count, read as a number.
    fn counted(&self, statement: &str, args: &[&dyn ToSql]) -> Result<u64> {
        let n: Option<i64> = one(self.conn, statement, args)?;
        Ok(n.map_or(0, i64::unsigned_abs))
    }

    /// At most `n` holders of the scan's exact text in key order after
    /// `after`, each with whether its order row for `window.field` lies
    /// inside the window: the text index's rows, each joined to its order
    /// row, a holder with none outside.
    fn holders_inside(
        &self,
        ty: &TypeName,
        window: &engines::Window,
        scan: &Scan,
        after: Option<&RecordKey>,
        n: u32,
    ) -> Result<Option<HolderBatch>> {
        let Some(decl) = self.type_decl(ty)? else {
            return Ok(None);
        };
        let Some(text) = self.window_text(ty, Some(&decl), scan)? else {
            return Ok(None);
        };
        let Some((text_field, texts)) = text else {
            return Ok(None);
        };
        if !self.head {
            return Ok(None);
        }
        let (ty_name, text_field, field) = (
            ty.as_str(),
            text_field.as_str().to_string(),
            window.field.as_str().to_string(),
        );
        let mut query = String::from(
            "SELECT t.key, s.sort_key FROM (SELECT t.key FROM text_index t \
             WHERE t.ty = ?1 AND t.field = ?2 AND ",
        );
        let mut args: Vec<&dyn ToSql> = vec![&ty_name, &text_field];
        let keyed = serde_json::to_string(&texts).expect("the texts serialise");
        let after_key: Option<String> = after.map(|a| a.to_string());
        if texts.len() == 1 {
            query.push_str("t.value = ?3");
            args.push(&texts[0]);
        } else {
            query.push_str("t.value IN (SELECT value FROM json_each(?3))");
            args.push(&keyed);
        }
        let mut next = 4;
        if after.is_some() {
            query.push_str(&format!(" AND t.key > ?{next}"));
            args.push(&after_key);
            next += 1;
        }
        query.push_str(&format!(
            " ORDER BY t.key LIMIT ?{next}) t LEFT JOIN sort_index s \
             ON s.ty = ?1 AND s.field = ?{} AND s.key = t.key ORDER BY t.key",
            next + 1
        ));
        args.push(&n);
        args.push(&field);
        let mut stmt = self.conn.prepare_cached(&query).map_err(sql)?;
        let rows: Vec<(String, Option<Vec<u8>>)> = stmt
            .query_map(args.as_slice(), |r| Ok((r.get(0)?, r.get(1)?)))
            .and_then(Iterator::collect)
            .map_err(sql)?;
        engines::count_walk(rows.len());
        Ok(Some(HolderBatch {
            done: rows.len() < n as usize,
            after: rows.last().map(|(key, _)| key.as_str().into()),
            holders: rows
                .into_iter()
                .map(|(key, sort_key)| {
                    let inside = sort_key.is_some_and(|k| window.holds(&k));
                    (key.as_str().into(), inside)
                })
                .collect(),
        }))
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
        let rows = if let Some((f, t)) = holders.text() {
            self.page(
                &unsorted_holding_sql(after.is_some()),
                &[&ty.as_str(), &field.as_str(), &n, &f, &t],
                after,
            )?
        } else {
            self.page(
                &unsorted_after_sql(after.is_some()),
                &[&ty.as_str(), &field.as_str(), &n],
                after,
            )?
        };
        // A range of the key index walks what it hands.
        engines::count_walk(rows.len());
        Ok(rows)
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
        projection::records(self.conn, statement, args.as_slice())
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

    /// The type the verb, trigger or set in force under `ident` stands on.
    fn stands(&self, ident: &str) -> Option<TypeName> {
        let state: String = one(
            self.conn,
            &format!(
                "SELECT state FROM declarations WHERE ident = ?1 AND {}",
                holds_at(2)
            ),
            params![ident, self.at()],
        )
        .ok()??;
        match decode::<Declared>(&state, "a state").ok()?.declaration {
            Declaration::Verb(v) => Some(v.on),
            Declaration::Trigger(t) => Some(t.on),
            Declaration::ObjectSet(s) => Some(s.of),
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

    fn chosen(&self, ty: &TypeName) -> Result<Chosen> {
        self.read(Chosen::default(), |r| projection::chosen_kept(r.conn, ty))
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

    fn replaced(&self, from: Position, to: Position) -> Result<Option<Vec<Declared>>> {
        self.read(Some(Vec::new()), |r| {
            let to = to.min(Position(r.at() as u64));
            column(
                r.conn,
                "SELECT state FROM declarations WHERE until > ?1 AND until <= ?2",
                params![from.0 as i64, to.0 as i64],
                |s: String| decode(&s, "a state"),
            )
            .map(Some)
        })
    }

    fn get(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>> {
        self.read(None, |r| r.record(ty, key))
    }

    fn order(&self, name: &TypeName, key: &RecordKey) -> Result<Option<Vec<u8>>> {
        self.read(None, |r| projection::order(r.conn, name, key))
    }

    fn order_page(
        &self,
        name: &TypeName,
        after: Option<&RecordKey>,
        n: u32,
    ) -> Result<Vec<(RecordKey, Vec<u8>)>> {
        self.read(Vec::new(), |r| {
            projection::order_page(r.conn, name, after, n)
        })
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
            if counted
                && scan.filter.is_none()
                && scan.equal.is_empty()
                && scan.equal_any.is_empty()
                && scan.valid_at.is_none()
                && (scan.range.is_none() || is_kept_name(ty))
            {
                return r.count_in_force(ty, scan.retired, scan.restricted, scan.range.as_ref());
            }
            // A count that must hold a text in a text key reads the records
            // at the keys it makes; at the head, one that must hold another
            // text reads the records holding it, not the type. A kept name's
            // interval seeks the key-ordered road instead: a text lookup
            // would read the keys holding the text and walk past its bound.
            let decl = r.type_decl(ty)?;
            if scan.range.is_none()
                && let Some(keys) = engines::key_lookup(decl.as_ref(), scan)
            {
                let records = r.records_by_key(ty, keys)?;
                return Ok(engines::count_of(decl.as_ref(), &records, scan));
            }
            let looked = r.looked(ty, decl.as_ref())?;
            let kept = |f: &FieldName| looked.as_ref().is_none_or(|l| l.contains(f));
            let kind = |f: &FieldName| {
                decl.as_ref()
                    .and_then(|d| engines::field_kind(d, f))
                    .copied()
            };
            // A text narrowing is the storage's own: `json_extract` reads a
            // number as a number, not as the token the lookup keeps.
            let text = |f: &FieldName| engines::text_kind(kind(f));
            let lookup = engines::text_lookups(scan, &kept, &kind)
                .filter(|_| r.head && scan.range.is_none());
            let (narrow, mut held) = (engines::text_lookup(scan, &|_| true, &text), None);
            engines::count_in_key_order(decl.as_ref(), scan, |after, n| {
                r.in_key_order(
                    ty,
                    (&lookup, &narrow),
                    &mut held,
                    after,
                    scan.range.as_ref(),
                    n,
                )
            })
        })
    }

    fn window_keys(
        &self,
        ty: &TypeName,
        window: &lakelands_api::engine::Window,
        scan: &Scan,
        after: Option<&WindowAt>,
        n: u32,
    ) -> Result<Option<WindowBatch>> {
        self.read(None, |r| r.window_keys(ty, window, scan, after, n))
    }

    fn window_sides(
        &self,
        ty: &TypeName,
        window: &lakelands_api::engine::Window,
        scan: &Scan,
        l: u64,
    ) -> Result<Option<WindowSides>> {
        self.read(None, |r| r.window_sides(ty, window, scan, l))
    }

    fn holders_inside(
        &self,
        ty: &TypeName,
        window: &lakelands_api::engine::Window,
        scan: &Scan,
        after: Option<&RecordKey>,
        n: u32,
    ) -> Result<Option<HolderBatch>> {
        self.read(None, |r| r.holders_inside(ty, window, scan, after, n))
    }

    fn scan(&self, ty: &TypeName, scan: &Scan) -> Result<Vec<StoredRecord>> {
        self.read(Vec::new(), |r| {
            let decl = r.type_decl(ty)?;
            // A kept name's interval seeks the key-ordered road; a text
            // lookup would read the keys holding the text and walk past its
            // bound.
            if scan.range.is_none()
                && let Some(keys) = engines::key_lookup(decl.as_ref(), scan)
            {
                let records = r.records_by_key(ty, keys)?;
                return Ok(engines::scan(decl.as_ref(), &records, scan));
            }
            let looked = r.looked(ty, decl.as_ref())?;
            let kept = |f: &FieldName| looked.as_ref().is_none_or(|l| l.contains(f));
            let kind = |f: &FieldName| decl.as_ref().and_then(|d| engines::field_kind(d, f)).copied();
            let text = |f: &FieldName| engines::text_kind(kind(f));
            let lookup = engines::text_lookup(scan, &kept, &kind).filter(|_| r.head);
            // Ordered by the key alone, the page is read from `after` in
            // key order and stops when it is full: at the head, with a text
            // the scan must hold, from the records holding it.
            if scan.order_by.is_empty() {
                let lookup = engines::text_lookups(scan, &kept, &kind)
                    .filter(|_| r.head && scan.range.is_none());
                let (narrow, mut held) = (engines::text_lookup(scan, &|_| true, &text), None);
                return engines::scan_in_key_order(decl.as_ref(), scan, |after, n| {
                    let range = scan.range.as_ref();
                    let rows = r.in_key_order(ty, (&lookup, &narrow), &mut held, after, range, n)?;
                    // A range of the key index walks what it hands.
                    engines::count_walk(rows.len());
                    Ok(rows)
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
                    // A filter that bounds the page's own order field
                    // narrows the index to its window; the rulebook still
                    // admits each row, and no record without a value can
                    // be admitted, so the walk past the index has no tail.
                    let window = decl
                        .as_ref()
                        .and_then(|d| engines::field_kind(d, &first.field))
                        .zip(scan.filter.as_ref())
                        .and_then(|(kind, filter)| engines::window(filter, &first.field, kind));
                    // Under a text few hold, a page that asks for many
                    // batches reads the holders left once.
                    let mut held = Held::default();
                    let sorted = |from: Option<(&[u8], &str)>, n: u32| match holders {
                        Holders::Few(..) => {
                            r.few_after(ty, first, &holders, window.as_ref(), &mut held, from, n)
                        }
                        _ => r.sorted_after(ty, first, &holders, window.as_ref(), from, n),
                    };
                    let missing = |after: Option<&str>, n: u32| {
                        r.unsorted_after(ty, &first.field, &holders, after, n)
                    };
                    // A page inside a window reads no record without the
                    // field: the comparison is false with no value.
                    return match window {
                        Some(_) => {
                            engines::scan_in_field_order(decl.as_ref(), scan, sorted, |_, _| {
                                Ok(Vec::new())
                            })
                        }
                        None => engines::scan_in_field_order(decl.as_ref(), scan, sorted, missing),
                    };
                }
                // At an earlier position, the head's index with the keys
                // changed since; past the ceiling, the sort.
                Some(first) => match r.changed(ty, engines::changed_ceiling(scan.limit))? {
                    Some(changed) => {
                        return engines::scan_in_field_order_at(
                            decl.as_ref(),
                            scan,
                            &changed,
                            |from, n| {
                                r.sorted_after(ty, first, &Holders::All, None, from, n)
                            },
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

    fn proposals(&self, filter: &ProposalFilter) -> Result<Vec<Proposal>> {
        self.read(Vec::new(), |r| {
            let all: Vec<Proposal> = column(
                r.conn,
                &format!("SELECT state FROM proposals WHERE {}", holds_at(1)),
                params![r.at()],
                |s: String| decode(&s, "a state"),
            )?;
            Ok(engines::proposals(&all, filter, |i| r.stands(i)))
        })
    }

    fn proposals_before(
        &self,
        filter: &ProposalFilter,
        before: Option<Position>,
    ) -> Result<Option<Vec<Proposal>>> {
        let (state, before, limit) = lakelands_core::engines::proposals_asked(filter, before);
        self.read(Some(Vec::new()), |r| {
            column(
                r.conn,
                &format!(
                    "SELECT state FROM proposals WHERE {} \
                     AND (?2 IS NULL OR json_extract(state, '$.state') = ?2) \
                     AND json_extract(state, '$.position') < ?3 \
                     ORDER BY json_extract(state, '$.position') DESC LIMIT ?4",
                    holds_at(1)
                ),
                params![r.at(), state, before, limit],
                |s: String| decode(&s, "a state"),
            )
            .map(Some)
        })
    }

    fn quarantined(&self, filter: &QuarantineFilter) -> Result<Vec<Quarantined>> {
        let open = filter.state.map(|s| i64::from(s == QuarantineState::Open));
        let after = filter.after.as_ref();
        let limit = filter.limit.map_or(-1, i64::from);
        self.read(Vec::new(), |r| {
            let rows: Vec<Quarantined> = column(
                r.conn,
                &format!(
                    "SELECT state FROM quarantined WHERE {} \
                     AND (?2 IS NULL OR source = ?2) AND (?3 IS NULL OR delivery = ?3) \
                     AND (?4 IS NULL OR ix = ?4) \
                     AND (?5 IS NULL OR json_extract(state, '$.type') = ?5) \
                     AND (?6 IS NULL OR json_extract(state, '$.key') = ?6) \
                     AND (?7 IS NULL OR (json_extract(state, '$.closed') IS NULL) = ?7) \
                     AND (?8 IS NULL OR (source, delivery, ix) > (?8, ?9, ?10)) \
                     ORDER BY source, delivery, ix LIMIT ?11",
                    holds_at(1)
                ),
                params![
                    r.at(),
                    filter.source.as_ref().map(|s| s.as_str()),
                    filter.delivery.as_ref().map(|d| d.as_str()),
                    filter.index.map(|i| i as i64),
                    filter.ty.as_ref().map(|t| t.as_str()),
                    filter.key.as_ref().map(|k| k.as_str()),
                    open,
                    after.map(|a| a.source.as_str()),
                    after.map(|a| a.delivery.as_str()),
                    after.map(|a| a.index as i64),
                    limit,
                ],
                |s: String| decode(&s, "a quarantined record"),
            )?;
            Ok(filter.answer(rows))
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
        Ok(self.applied_many(std::slice::from_ref(id))?.remove(0))
    }

    fn applied_many(&self, ids: &[EventId]) -> Result<Vec<Option<Position>>> {
        // One read for every id, each its seek of the cached statement.
        self.read(vec![None; ids.len()], |r| {
            // An id is held as its 16 bytes; one not of its form is held
            // by no log this build keeps.
            let at = |id: &EventId| match id.of_form() {
                true => {
                    let query = "SELECT at FROM applied WHERE id = unhex(?1) AND at <= ?2";
                    one(r.conn, query, params![id.0, r.at()])
                }
                false => Ok(None),
            };
            let at = |id| at(id).map(|p: Option<i64>| p.map(|p| Position(p as u64)));
            ids.iter().map(at).collect()
        })
    }

    fn output(&self, seam: &str, input: &Digest) -> Result<Option<(Position, Value)>> {
        use rusqlite::OptionalExtension;
        self.read(None, |r| {
            let row = (r.conn.prepare_cached(OUTPUT_AT))
                .and_then(|mut s| {
                    let args = params![seam, input.0, r.at()];
                    s.query_row(args, |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                        .optional()
                })
                .map_err(sql)?;
            let decoded =
                row.map(|(since, s)| Ok((Position(since as u64), decode(&s, "a state")?)));
            decoded.transpose()
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

    fn of_source(
        &self,
        source: &SourceId,
        from: Position,
        through: Position,
        limit: u32,
    ) -> Result<Matched> {
        self.read(feed::undecided(), |r| {
            let Some(last) = feed::last_decided(from, through.min(r.at), r.folded) else {
                return Ok(feed::undecided());
            };
            let by = Some(("source", source.as_str()));
            let all = &FeedFilter::default();
            let positions = projection::routes(r.conn, all, by, [from, last], limit)?;
            Ok(feed::merged([positions], limit, last))
        })
    }

    fn thinned_acts(&self, ty: &TypeName) -> Result<Vec<ThinnedAct>> {
        // Read at the head in every read, oldest first by the position
        // each act began at.
        self.read(Vec::new(), |r| {
            let mut stmt = r
                .conn
                .prepare_cached(
                    "SELECT ty, \"by\", \"before\", at, through, gone FROM thinned_types \
                     WHERE ty = ?1 ORDER BY at, \"by\", \"before\"",
                )
                .map_err(sql)?;
            let rows = stmt
                .query_map(params![ty.as_str()], |row| {
                    Ok(ThinnedAct {
                        ty: row.get::<_, String>(0)?.into(),
                        by: row.get::<_, String>(1)?.into(),
                        before: Timestamp(row.get(2)?),
                        through: Position(row.get::<_, i64>(4)? as u64),
                        gone: row.get::<_, i64>(5)? as u64,
                    })
                })
                .map_err(sql)?;
            rows.map(|r| r.map_err(sql)).collect()
        })
    }

    fn thinned_at(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<Timestamp>> {
        self.read(None, |r| {
            one(
                r.conn,
                "SELECT \"before\" FROM thinned WHERE ty = ?1 AND key = ?2",
                params![ty.as_str(), key.as_str()],
            )
            .map(|row: Option<i64>| row.map(Timestamp))
        })
    }
}
