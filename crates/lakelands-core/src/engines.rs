//! What every engine computes the same way.
//!
//! An engine stores and returns; where the engine seam asks it to select,
//! order, link, score or rank, the answer is part of the specification and
//! must be the same, byte for byte, on every engine
//! (`docs/decisions/031-engines-share-one-rulebook.md`). So the rules live
//! here, once, and each engine applies them to the states it keeps: it
//! may use its own indexes to find the candidates faster, never to decide
//! which of them match or where they sort (`docs/seams.md` §Engine,
//! `docs/decisions/029-every-answer-has-one-order.md`).
//!
//! - **Selection.** A scan's filter is evaluated by the one evaluator over
//!   the record's own fields, with no settings and no clock (the store
//!   hands an engine only filters that need neither); a filter that
//!   cannot be evaluated for a record does not select it.
//! - **Order.** Sort keys in turn, a record without a value for a key after
//!   every record with one in either direction, then the record key
//!   ascending. Values compare by [`cmp_values`]: ranked by kind, then
//!   compared by the evaluator within a kind.
//! - **Links.** A relation carried by a reference field (`via`) links a
//!   record to the record of the relation's target whose key is that
//!   field's text value; the links into a record are those of every type
//!   with such a relation to its type.
//! - **Full text.** A type's searchable text fields go through the one
//!   token pipeline (`crate::stem`), stemmed by the type's language, as
//!   they stand at the head; a record scores the number of distinct query
//!   tokens it holds, one holding none is not a hit, and a view keeps the
//!   hits live at its own position — restricted ones included, since
//!   the store applies the principal's grants to them. Best first, ties by
//!   key.
//! - **Proposals** come newest first, ties by id.
//! - **Vectors.** Similarity is the cosine of the angle between two
//!   vectors ([`lakelands_api::query::cosine`]). Best first, ties by key.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use lakelands_api::engine::{After, Scan};
use lakelands_api::error::{Error, Refusal, Result};
use lakelands_api::expr::Expr;
use lakelands_api::id::{FieldName, Position, RecordKey, RelationName, TypeName, VerbName};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::query::Edge;
use lakelands_api::read::{Direction, Order, OrderBy};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::time::Timestamp;
use lakelands_api::write::{Declaration, Proposal, ProposalFilter, ProposedWrite};

pub use crate::expr::order::{cmp_values, sort_key};
use crate::expr::{RecordScope, holds};
use crate::invalid;

/// The clock an engine evaluates a filter with. The store hands an engine
/// only filters that do not read the clock, so any fixed instant will do.
const NO_CLOCK: Timestamp = Timestamp(0);

/// A declaration's identity: its kind and its name. A later declaration
/// with the same identity supersedes an earlier one.
pub fn declaration_key(d: &Declaration) -> String {
    match d {
        Declaration::Type(t) => format!("type {}", t.name),
        Declaration::ValueType(v) => format!("value_type {}", v.name),
        Declaration::Verb(v) => format!("verb {}", v.name),
        Declaration::Trigger(t) => format!("trigger {}", t.name),
        Declaration::ObjectSet(o) => format!("object_set {}", o.name),
        Declaration::Function(f) => format!("function {}", f.name),
    }
}

/// A narrowing an engine may take from a filter, with the one evaluator's
/// meaning: when the filter is, or has as one of its `and` terms,
/// `field = 'text'` whose text reads neither as a number nor as an
/// instant, only records whose `field` holds exactly that JSON string can
/// be selected (the evaluator compares such text as text). An engine may
/// then look the candidates up by that value; it still evaluates the whole
/// filter on each.
pub fn exact_text_term(filter: &Expr) -> Option<(FieldName, String)> {
    use lakelands_api::expr::{CmpOp, Literal};
    match filter {
        Expr::And { args } => args.iter().find_map(exact_text_term),
        Expr::Cmp {
            cmp: CmpOp::Eq,
            lhs,
            rhs,
        } => match (lhs.as_ref(), rhs.as_ref()) {
            (
                Expr::Path { path },
                Expr::Lit {
                    value: Literal::Text(t),
                },
            )
            | (
                Expr::Lit {
                    value: Literal::Text(t),
                },
                Expr::Path { path },
            ) if path.len() == 1
                && t.parse::<rust_decimal::Decimal>().is_err()
                && Timestamp::parse(t).is_err() =>
            {
                Some((path[0].clone(), t.clone()))
            }
            _ => None,
        },
        _ => None,
    }
}

/// The value a sort key reads from a record: the field's value, or when
/// it was learned or observed, or for `latest` the position of the event
/// that made the state. `None` when absent.
pub fn sort_value(record: &StoredRecord, order: &Order) -> Option<Value> {
    match order.by {
        OrderBy::Latest => Some(Value::from(record.position.0)),
        OrderBy::Value => record
            .fields
            .get(&order.field)
            .filter(|v| !v.is_null())
            .cloned(),
        OrderBy::KnownAt => record
            .meta
            .get(&order.field)
            .map(|m| Value::String(m.known_at.to_string())),
        OrderBy::SeenAt => record
            .meta
            .get(&order.field)
            .and_then(|m| m.seen_at)
            .map(|t| Value::String(t.to_string())),
    }
}

/// One sort key's order between two optional values: missing last in
/// either direction.
fn cmp_key(a: Option<&Value>, b: Option<&Value>, dir: Direction) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => match dir {
            Direction::Asc => cmp_values(a, b),
            Direction::Desc => cmp_values(b, a),
        },
    }
}

/// A row's place in a scan: its sort values and its key.
pub struct Place<'a> {
    /// The sort keys' values, `None` where absent.
    pub values: Vec<Option<Value>>,
    /// The record key.
    pub key: &'a str,
}

impl<'a> Place<'a> {
    /// Where a record sorts under `order_by`.
    pub fn of(record: &'a StoredRecord, order_by: &[Order]) -> Place<'a> {
        Place {
            values: order_by.iter().map(|o| sort_value(record, o)).collect(),
            key: record.key.as_str(),
        }
    }

    /// Where a keyset continuation sits; a null value there is absent.
    pub fn after(after: &'a After) -> Place<'a> {
        Place {
            values: after
                .values
                .iter()
                .map(|v| (!v.is_null()).then(|| v.clone()))
                .collect(),
            key: after.key.as_str(),
        }
    }
}

/// Two places under `order_by`; the key, ascending, breaks every tie.
pub fn cmp_places(a: &Place<'_>, b: &Place<'_>, order_by: &[Order]) -> Ordering {
    order_by
        .iter()
        .enumerate()
        .map(|(i, o)| {
            cmp_key(
                a.values.get(i).and_then(Option::as_ref),
                b.values.get(i).and_then(Option::as_ref),
                o.dir,
            )
        })
        .find(|o| o.is_ne())
        .unwrap_or_else(|| a.key.cmp(b.key))
}

/// Whether a scan selects `record`: its flags, `valid_at` under the
/// record's type (a type not in force is not valid-timed), the fields it
/// must hold exactly, and the filter (one that cannot be evaluated does
/// not select).
pub fn admits(decl: Option<&TypeDecl>, record: &StoredRecord, scan: &Scan) -> bool {
    let fields = &record.fields;
    let scope = RecordScope {
        fields,
        settings: None,
        now: NO_CLOCK,
    };
    (scan.retired || !record.retired)
        && (scan.restricted || !record.restricted)
        && (decl.zip(scan.valid_at))
            .is_none_or(|(d, t)| crate::valid_time::record_holds(d, fields, t))
        && holds_exactly(record, &scan.equal)
        && (scan.filter.as_ref()).is_none_or(|f| holds(f, &scope).unwrap_or(false))
}

/// Whether `record` holds each value exactly: the same JSON value, never
/// null (a derived relation's match, 044).
pub fn holds_exactly(record: &StoredRecord, equal: &[(FieldName, Value)]) -> bool {
    equal
        .iter()
        .all(|(f, v)| !v.is_null() && record.fields.get(f) == Some(v))
}

/// The one field and text an engine may look candidates up by in its text
/// index at the head: a string the scan must hold exactly, else the
/// filter's [`exact_text_term`]. The engine still admits each candidate.
pub fn text_lookup(scan: &Scan) -> Option<(FieldName, String)> {
    scan.equal
        .iter()
        .find_map(|(f, v)| v.as_str().map(|t| (f.clone(), t.to_owned())))
        .or_else(|| scan.filter.as_ref().and_then(exact_text_term))
}

/// How many rows a key-ordered scan asks its storage for at a time: the
/// page it wants, at most 1 024, so an unfiltered page reads its rows and
/// no more, and a filtered one reads batch after batch until it is full.
pub fn key_batch(scan: &Scan) -> u32 {
    scan.limit.clamp(1, 1024)
}

/// A scan ordered by the key alone, which storage kept in key order can
/// answer by reading from `after` and stopping at `limit`: its rows are
/// read in batches (`key_batch`), each continuing after the last key read,
/// until the page is full or the type is out. `read(after, n)` answers at
/// most `n` states after the key `after` (from the first when `None`), in
/// key order. It answers what [`scan`] answers over every state.
pub fn scan_in_key_order<E>(
    decl: Option<&TypeDecl>,
    scan: &Scan,
    mut read: impl FnMut(Option<&str>, u32) -> Result<Vec<StoredRecord>, E>,
) -> Result<Vec<StoredRecord>, E> {
    debug_assert!(scan.order_by.is_empty());
    let mut out = Vec::new();
    if scan.limit == 0 {
        return Ok(out);
    }
    let batch = key_batch(scan);
    let mut after: Option<String> = scan.after.as_ref().map(|a| a.key.to_string());
    loop {
        let rows = read(after.as_deref(), batch)?;
        let read_all = rows.len() < batch as usize;
        for r in rows {
            after = Some(r.key.to_string());
            if admits(decl, &r, scan) {
                out.push(r);
                if out.len() >= scan.limit as usize {
                    return Ok(out);
                }
            }
        }
        if read_all {
            return Ok(out);
        }
    }
}

/// How many states [`scan`] would select over every state, its order,
/// keyset position and limit ignored, read as [`scan_in_key_order`] reads
/// them: in key-ordered batches, each admitted or not and dropped, so a
/// count holds one batch whatever the type holds.
pub fn count_in_key_order<E>(
    decl: Option<&TypeDecl>,
    scan: &Scan,
    mut read: impl FnMut(Option<&str>, u32) -> Result<Vec<StoredRecord>, E>,
) -> Result<u64, E> {
    const BATCH: u32 = 1024;
    let (mut n, mut after) = (0, None::<String>);
    loop {
        let rows = read(after.as_deref(), BATCH)?;
        count_read(rows.len());
        n += rows.iter().filter(|r| admits(decl, r, scan)).count() as u64;
        match rows.last() {
            Some(last) if rows.len() == BATCH as usize => after = Some(last.key.to_string()),
            _ => return Ok(n),
        }
    }
}

/// A scan over the states of one type's records at one position, `decl`
/// the type's declaration in force there: the flags, `valid_at` and the
/// filter select; the sort keys order; the keyset position continues;
/// `limit` is literal (0 answers nothing).
pub fn scan<'a>(
    decl: Option<&TypeDecl>,
    records: impl IntoIterator<Item = &'a StoredRecord>,
    scan: &Scan,
) -> Vec<StoredRecord> {
    // Each row's place is encoded once (`sort_key`), and the sort compares
    // bytes: the order `cmp_places` gives, without ranking and parsing
    // every value again on every comparison.
    let mut handed = 0;
    let mut rows: Vec<(PlaceKey, &StoredRecord)> = records
        .into_iter()
        .inspect(|_| handed += 1)
        .filter(|r| admits(decl, r, scan))
        .map(|r| (PlaceKey::record(r, &scan.order_by), r))
        .collect();
    count_read(handed);
    rows.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    let from = match &scan.after {
        Some(after) => {
            let after = PlaceKey::of(&Place::after(after), &scan.order_by);
            rows.partition_point(|(k, _)| *k <= after)
        }
        None => 0,
    };
    rows.into_iter()
        .skip(from)
        .take(scan.limit as usize)
        .map(|(_, r)| r.clone())
        .collect()
}

/// The longest key the order index keeps: a [`sort_key`] is cut to it
/// ([`order_key`]). A kept key this long may have been cut, so the rows
/// sharing it are a run that [`cmp_places`] orders from the values.
pub const ORDER_KEY_BYTES: usize = 32;

/// A value's key in the order index: its [`sort_key`], cut to
/// [`ORDER_KEY_BYTES`]. Cutting keeps the order between keys that differ
/// before the cut; values that share the first bytes share a key.
pub fn order_key(value: &Value) -> Vec<u8> {
    let mut key = sort_key(value);
    key.truncate(ORDER_KEY_BYTES);
    key
}

/// The fields of `decl` whose order index a lake keeps, by `setting`, the
/// value of its `sort_index.<type>` setting at the head: those it names
/// that the type declares with a kind the index keeps
/// ([`lakelands_api::seam::sortable`]), once each, in the setting's order.
/// A setting the store would refuse keeps what it can; none keeps none.
pub fn sorted_fields(decl: &TypeDecl, setting: Option<&Value>) -> Vec<FieldName> {
    index_fields(decl, setting, lakelands_api::seam::sortable)
}

/// The vector fields of `decl` a lake asks an index for, by `setting`, the
/// value of its `vector_index.<type>` setting: those it names that the
/// type declares as vectors, once each, in the setting's order.
pub fn indexed_fields(decl: &TypeDecl, setting: Option<&Value>) -> Vec<FieldName> {
    index_fields(decl, setting, lakelands_api::seam::indexable)
}

fn index_fields(
    decl: &TypeDecl,
    setting: Option<&Value>,
    keeps: fn(&lakelands_api::ontology::StorageKind) -> bool,
) -> Vec<FieldName> {
    let mut out: Vec<FieldName> = Vec::new();
    if decl.view.is_some() {
        return out;
    }
    for name in setting
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        if !out.iter().any(|f| f.as_str() == name)
            && decl
                .fields
                .iter()
                .any(|f| f.name.as_str() == name && keeps(&f.ty.kind))
        {
            out.push(FieldName::from(name));
        }
    }
    out
}

/// The type whose order index a setting's state asks for, if it is a
/// `sort_index.<type>` setting.
pub fn sort_index_type(r: &StoredRecord) -> Option<TypeName> {
    setting_for(r, lakelands_api::seam::SORT_INDEX_SETTING)
}

/// The type whose vector index a setting's state asks for, if it is a
/// `vector_index.<type>` setting.
pub fn vector_index_type(r: &StoredRecord) -> Option<TypeName> {
    setting_for(r, lakelands_api::seam::VECTOR_INDEX_SETTING)
}

fn setting_for(r: &StoredRecord, prefix: &str) -> Option<TypeName> {
    (r.ty.as_str() == lakelands_api::seam::SETTING_TYPE)
        .then(|| r.key.as_str().strip_prefix(prefix))
        .flatten()
        .map(TypeName::from)
}

/// The key of the setting that asks for `ty`'s order index.
pub fn sort_index_key(ty: &TypeName) -> String {
    format!("{}{ty}", lakelands_api::seam::SORT_INDEX_SETTING)
}

/// The key of the setting that asks for `ty`'s vector index.
pub fn vector_index_key(ty: &TypeName) -> String {
    format!("{}{ty}", lakelands_api::seam::VECTOR_INDEX_SETTING)
}

/// The value a head state of a setting holds: none when it is retired.
pub fn setting_value(r: &StoredRecord) -> Option<&Value> {
    (!r.retired).then(|| r.fields.get("value")).flatten()
}

/// The order index rows of a record's state: one per field of `fields`
/// (the type's [`sorted_fields`]) that the state holds with a value, with
/// its [`order_key`]. Retired and restricted states have rows too:
/// [`admits`] decides on them when a page is read.
pub fn order_rows<'a>(
    fields: &'a [FieldName],
    record: &StoredRecord,
) -> Vec<(&'a FieldName, Vec<u8>)> {
    fields
        .iter()
        .filter_map(|f| {
            let v = record.fields.get(f).filter(|v| !v.is_null())?;
            Some((f, order_key(v)))
        })
        .collect()
}

/// Whether an order index kept by [`order_rows`] for `fields` can answer
/// `scan`: it is ordered, first by the value of one of them. What the
/// index cannot answer (an order by `known_at` or `seen_at`, a field the
/// lake does not index, a view type) is sorted whole by [`scan`].
pub fn by_order_index<'a>(
    decl: Option<&TypeDecl>,
    fields: &[FieldName],
    scan: &'a Scan,
) -> Option<&'a Order> {
    decl.filter(|d| d.view.is_none())?;
    let first = scan.order_by.first()?;
    (first.by == OrderBy::Value && fields.contains(&first.field)).then_some(first)
}

thread_local! {
    static ROWS_READ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static LINKS_READ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The rows the scans on this thread were handed to order: the whole
/// type in the fallback, [`scan`], the index rows and missing records
/// in [`scan_in_field_order`], and those and the changed states in
/// [`scan_in_field_order_at`]. It exists for the engine checks, which
/// count by it that a page ordered by an indexed field reads its page,
/// not the type; it costs one add per call or batch.
pub fn rows_read() -> u64 {
    ROWS_READ.with(|n| n.get())
}

/// The records the edge walks on this thread were handed to find a
/// record's links: the link records [`crate::links::link_edges`] was
/// given, and the declaring type's records [`in_links`] was given. It
/// exists for the engine checks, which count by it that a walk reads
/// what it reaches at the head, not the link type; it costs one add per
/// call.
pub fn links_read() -> u64 {
    LINKS_READ.with(|n| n.get())
}

pub(crate) fn count_links(n: usize) {
    LINKS_READ.with(|c| c.set(c.get() + n as u64));
}

/// [`rows_read`] over every thread of the process: for the streaming
/// check, whose reads go through a store that runs them on its blocking
/// pool. It counts what other checks running at the same time read too.
pub fn rows_read_in_process() -> u64 {
    ROWS_READ_IN_PROCESS.load(std::sync::atomic::Ordering::Relaxed)
}

static ROWS_READ_IN_PROCESS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn count_read(n: usize) {
    ROWS_READ.with(|c| c.set(c.get() + n as u64));
    ROWS_READ_IN_PROCESS.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
}

/// A scan answered from an order index, in the order [`scan`] gives, by
/// reading about `limit` rows.
///
/// `sorted(from, n)` answers at most `n` rows of the index for the first
/// sort field, each with its record's state and its [`sort_key`], in the
/// index's order for that field's direction: `(sort key, record key)`
/// ascending, or the sort key descending and the record key ascending;
/// strictly after `from` in that order, or from the first. `missing(after,
/// n)` answers at most `n` of the type's records that have no row for the
/// field, in key order after `after`.
///
/// For one sort field and a key shorter than [`ORDER_KEY_BYTES`], the
/// index's order is the rulebook's, so rows are taken as they come. Rows
/// sharing a key that long, which may have been cut, and for several sort
/// fields every run of rows sharing the first field's key, are read whole
/// and ordered by [`cmp_places`], and so are the records missing the
/// first field, which come last: the index orders the runs and never
/// decides inside one. A page that continues inside a run reads the run
/// from its start and keeps what lies strictly after `after`, since the
/// record keys inside a run are not in the values' order.
pub fn scan_in_field_order<E>(
    decl: Option<&TypeDecl>,
    scan: &Scan,
    sorted: impl FnMut(Option<(&[u8], &str)>, u32) -> Result<Vec<(Vec<u8>, StoredRecord)>, E>,
    missing: impl FnMut(Option<&str>, u32) -> Result<Vec<StoredRecord>, E>,
) -> Result<Vec<StoredRecord>, E> {
    in_field_order(decl, scan, true, sorted, missing)
}

/// The keys of a type changed after an earlier position, and the states
/// they had there: what a page at that position reads in place of the head
/// order index's rows for those keys
/// (`docs/decisions/041-a-sorted-page-reads-its-page.md`, extended). A key
/// written only after the position has no state among them.
#[derive(Debug, Default)]
pub struct Changed {
    /// Every key of the type with a state put or ended after the position.
    pub keys: BTreeSet<RecordKey>,
    /// The states of those keys at the position.
    pub states: Vec<StoredRecord>,
}

/// The most states put to a type since an earlier position, and the most
/// keys changed, that a page there takes into the head order index's order
/// before it sorts the type instead: ten pages' worth, and never more than
/// 4 096, so that what a page holds stays near a page. Far below where the
/// merge stops paying: at 100 000 records it reads 4 096 changed keys in a
/// tenth of the sort's time
/// (`docs/decisions/041-a-sorted-page-reads-its-page.md`).
pub fn changed_ceiling(limit: u32) -> usize {
    (limit as usize).saturating_mul(10).min(4096)
}

/// [`scan_in_field_order`] at an earlier position, over the head's order
/// index: `sorted` and `missing` answer the head, and `changed` the keys
/// changed since the position with their states there. The head's rows
/// for those keys are passed over, and their states at the position are
/// merged in, in the index's order (sort key, then key; the sort key
/// descending for a descending first field) and, for the records without
/// the first field, in key order. Every other key's state is the same at
/// the head and at the position, so the page is the one a sort of the type
/// at the position gives. Every row read counts, the passed-over ones and
/// the changed states included.
pub fn scan_in_field_order_at<E>(
    decl: Option<&TypeDecl>,
    scan: &Scan,
    changed: &Changed,
    mut sorted: impl FnMut(Option<(&[u8], &str)>, u32) -> Result<Vec<(Vec<u8>, StoredRecord)>, E>,
    mut missing: impl FnMut(Option<&str>, u32) -> Result<Vec<StoredRecord>, E>,
) -> Result<Vec<StoredRecord>, E> {
    let Some(first) = scan.order_by.first() else {
        return Ok(Vec::new());
    };
    count_read(changed.states.len());
    let desc = first.dir == Direction::Desc;
    let cmp = |a: (&[u8], &str), b: (&[u8], &str)| {
        let by_value = if desc { b.0.cmp(a.0) } else { a.0.cmp(b.0) };
        by_value.then_with(|| a.1.cmp(b.1))
    };
    fn row(r: &(Vec<u8>, StoredRecord)) -> (&[u8], &str) {
        (r.0.as_slice(), r.1.key.as_str())
    }
    let (mut indexed, mut unindexed) = (Vec::new(), Vec::new());
    for r in &changed.states {
        match r.fields.get(&first.field).filter(|v| !v.is_null()) {
            Some(v) => indexed.push((order_key(v), r.clone())),
            None => unindexed.push(r.clone()),
        }
    }
    indexed.sort_by(|a, b| cmp(row(a), row(b)));
    unindexed.sort_by(|a, b| a.key.cmp(&b.key));
    let ours = |key: &RecordKey| changed.keys.contains(key);
    let mut rows = Ahead::default();
    let mut rest = Ahead::default();
    in_field_order(
        decl,
        scan,
        false,
        |from, n| {
            let past = |r: &&(Vec<u8>, StoredRecord)| from.is_none_or(|f| cmp(row(r), f).is_gt());
            let mine = indexed.iter().filter(past).take(n as usize).cloned();
            rows.next(
                from.map(|(k, key)| (k.to_vec(), key.to_owned())),
                n,
                |at: Option<&(Vec<u8>, String)>, n| {
                    sorted(at.map(|(k, key)| (k.as_slice(), key.as_str())), n)
                },
                |r| (r.0.clone(), r.1.key.to_string()),
                |r| ours(&r.1.key),
                mine,
                |a, b| cmp(row(a), row(b)),
            )
        },
        |after, n| {
            let past = |r: &&StoredRecord| after.is_none_or(|a| r.key.as_str() > a);
            let mine = unindexed.iter().filter(past).take(n as usize).cloned();
            rest.next(
                after.map(str::to_owned),
                n,
                |at: Option<&String>, n| missing(at.map(String::as_str), n),
                |r| r.key.to_string(),
                |r| ours(&r.key),
                mine,
                |a, b| a.key.cmp(&b.key),
            )
        },
    )
}

/// One of the head's two streams of rows at an earlier position, the
/// changed keys passed over. The rows read ahead of what was answered are
/// kept, so a call that goes on where the last one ended reads no row
/// twice; a call from anywhere else reads from there.
struct Ahead<T, P> {
    pending: std::collections::VecDeque<T>,
    at: Option<P>,
    done: bool,
    served: Option<P>,
}

impl<T, P> Default for Ahead<T, P> {
    fn default() -> Self {
        Ahead {
            pending: Default::default(),
            at: None,
            done: false,
            served: None,
        }
    }
}

impl<T, P: PartialEq> Ahead<T, P> {
    /// At most `n` rows after `from`: the head's, read by `read` and less
    /// those `ours` passes over, merged with `mine` in `cmp`'s order.
    #[allow(clippy::too_many_arguments)]
    fn next<E>(
        &mut self,
        from: Option<P>,
        n: u32,
        mut read: impl FnMut(Option<&P>, u32) -> Result<Vec<T>, E>,
        place: impl Fn(&T) -> P,
        ours: impl Fn(&T) -> bool,
        mine: impl Iterator<Item = T>,
        cmp: impl Fn(&T, &T) -> Ordering,
    ) -> Result<Vec<T>, E> {
        if from != self.served {
            self.pending.clear();
            self.at = from;
            self.done = false;
        }
        while self.pending.len() < n as usize && !self.done {
            let want = n - self.pending.len() as u32;
            let rows = read(self.at.as_ref(), want)?;
            count_read(rows.len());
            self.done = rows.len() < want as usize;
            for r in rows {
                self.at = Some(place(&r));
                if !ours(&r) {
                    self.pending.push_back(r);
                }
            }
        }
        // Merged in `cmp`'s order, each row moved, not copied: a page
        // holds its rows once.
        let mut mine = mine.peekable();
        let mut out = Vec::with_capacity(n as usize);
        while out.len() < n as usize {
            let theirs = match (self.pending.front(), mine.peek()) {
                (Some(head), Some(own)) => cmp(own, head).is_gt(),
                (head, _) => head.is_some(),
            };
            match if theirs {
                self.pending.pop_front()
            } else {
                mine.next()
            } {
                Some(r) => out.push(r),
                None => break,
            }
        }
        self.served = out.last().map(place);
        Ok(out)
    }
}

/// [`scan_in_field_order`]'s body; `counted` says whether the rows the two
/// closures answer count as read here, or were counted where they were.
fn in_field_order<E>(
    decl: Option<&TypeDecl>,
    scan: &Scan,
    counted: bool,
    mut sorted: impl FnMut(Option<(&[u8], &str)>, u32) -> Result<Vec<(Vec<u8>, StoredRecord)>, E>,
    mut missing: impl FnMut(Option<&str>, u32) -> Result<Vec<StoredRecord>, E>,
) -> Result<Vec<StoredRecord>, E> {
    let count = |n: usize| {
        if counted {
            count_read(n);
        }
    };
    let mut out = Vec::new();
    if scan.limit == 0 || scan.order_by.is_empty() {
        return Ok(out);
    }
    let limit = scan.limit as usize;
    let multi = scan.order_by.len() > 1;
    let batch = key_batch(scan);
    // After the first batch, what the page still lacks and one more: a
    // page continued inside a run read the rows up to `after` again, and a
    // whole further batch would hold twice the page.
    let rest = |have: usize| (limit.saturating_sub(have) + 1).min(batch as usize) as u32;
    let after = scan
        .after
        .as_ref()
        .map(|a| PlaceKey::of(&Place::after(a), &scan.order_by));
    let first_after = scan
        .after
        .as_ref()
        .map(|a| (a.values.first().filter(|v| !v.is_null()), a.key.as_str()));
    // A run's rows ordered by the rulebook, those past `after` kept; whether the page is full.
    let take_run = |run: &mut Vec<StoredRecord>, out: &mut Vec<StoredRecord>| {
        let mut keyed: Vec<(PlaceKey, StoredRecord)> = run
            .drain(..)
            .filter(|r| admits(decl, r, scan))
            .map(|r| (PlaceKey::record(&r, &scan.order_by), r))
            .filter(|(k, _)| after.as_ref().is_none_or(|a| k > a))
            .collect();
        keyed.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        out.extend(keyed.into_iter().map(|(_, r)| r).take(limit - out.len()));
        out.len() >= limit
    };
    // The indexed rows, unless the page continues among the missing.
    if !matches!(first_after, Some((None, _))) {
        let is_run = |k: &[u8]| multi || k.len() >= ORDER_KEY_BYTES;
        let mut from: Option<(Vec<u8>, String)> = match first_after {
            // Inside a run: from the start of the run `after` is in.
            Some((Some(v), key)) => {
                let k = order_key(v);
                let key = if is_run(&k) { "" } else { key };
                Some((k, key.to_owned()))
            }
            _ => None,
        };
        let mut run: Vec<StoredRecord> = Vec::new();
        let mut run_key: Option<Vec<u8>> = None;
        let mut n = batch;
        loop {
            let rows = sorted(from.as_ref().map(|(k, r)| (k.as_slice(), r.as_str())), n)?;
            count(rows.len());
            let read_all = rows.len() < n as usize;
            for (k, r) in rows {
                from = Some((k.clone(), r.key.to_string()));
                if !is_run(&k) {
                    run_key = None;
                    if take_run(&mut run, &mut out) {
                        return Ok(out);
                    }
                    if admits(decl, &r, scan) {
                        out.push(r);
                        if out.len() >= limit {
                            return Ok(out);
                        }
                    }
                    continue;
                }
                if run_key.as_ref() != Some(&k) {
                    if take_run(&mut run, &mut out) {
                        return Ok(out);
                    }
                    run_key = Some(k);
                }
                run.push(r);
            }
            if read_all {
                if take_run(&mut run, &mut out) {
                    return Ok(out);
                }
                break;
            }
            n = rest(out.len());
        }
    }
    // The records without a value for the first field, last.
    let mut from: Option<String> = match first_after {
        Some((None, key)) if !multi => Some(key.to_owned()),
        _ => None,
    };
    let mut run = Vec::new();
    let mut n = batch;
    loop {
        let rows = missing(from.as_deref(), n)?;
        count(rows.len());
        let read_all = rows.len() < n as usize;
        for r in rows {
            from = Some(r.key.to_string());
            if multi {
                run.push(r);
            } else if admits(decl, &r, scan) {
                out.push(r);
                if out.len() >= limit {
                    return Ok(out);
                }
            }
        }
        if read_all {
            break;
        }
        if !multi {
            n = rest(out.len());
        }
    }
    if multi {
        take_run(&mut run, &mut out);
    }
    Ok(out)
}

/// A place as bytes: its order is [`cmp_places`]' order, and two places
/// are equal exactly when that order calls them equal.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct PlaceKey {
    values: Vec<SortPart>,
    key: String,
}

/// One sort key of a place. A column is sorted one way throughout, so an
/// ascending part never meets a descending one; a missing value is last
/// either way.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum SortPart {
    Asc(Vec<u8>),
    Desc(std::cmp::Reverse<Vec<u8>>),
    Missing,
}

impl PlaceKey {
    fn record(r: &StoredRecord, order_by: &[Order]) -> PlaceKey {
        PlaceKey::of(&Place::of(r, order_by), order_by)
    }

    fn of(place: &Place<'_>, order_by: &[Order]) -> PlaceKey {
        PlaceKey {
            values: order_by
                .iter()
                .enumerate()
                .map(
                    |(i, o)| match (place.values.get(i).and_then(Option::as_ref), o.dir) {
                        (None, _) => SortPart::Missing,
                        (Some(v), Direction::Asc) => SortPart::Asc(sort_key(v)),
                        (Some(v), Direction::Desc) => {
                            SortPart::Desc(std::cmp::Reverse(sort_key(v)))
                        }
                    },
                )
                .collect(),
            key: place.key.to_owned(),
        }
    }
}

/// The links `record` declares over the relations of `decl` carried by a
/// reference field, over `relation` or all of them.
pub fn links_of(
    decl: &TypeDecl,
    record: &StoredRecord,
    relation: Option<&RelationName>,
) -> Vec<Edge> {
    decl.relations
        .iter()
        .filter(|r| relation.is_none_or(|want| *want == r.name))
        .filter_map(|r| {
            let to = record.fields.get(r.via.as_ref()?)?.as_str()?;
            Some(Edge {
                from: (record.ty.clone(), record.key.clone()),
                relation: r.name.clone(),
                to: (r.target.clone(), RecordKey::from(to)),
                fields: BTreeMap::new(),
            })
        })
        .collect()
}

/// The links that point at the record `ty` `key`, over `relation` or all:
/// from the records of every type in `types` that declares a relation to
/// `ty` carried by a reference field, `records_of` answering a type's
/// records at the view's position. The one place that decides which types
/// can link here and which of their links do.
pub fn in_links<'a, E>(
    ty: &TypeName,
    key: &RecordKey,
    relation: Option<&RelationName>,
    types: impl IntoIterator<Item = &'a TypeDecl>,
    mut records_of: impl FnMut(&TypeName) -> Result<Vec<StoredRecord>, E>,
) -> Result<Vec<Edge>, E> {
    let mut edges = Vec::new();
    for decl in types {
        if !decl.relations.iter().any(|r| {
            r.target == *ty && r.via.is_some() && relation.is_none_or(|want| *want == r.name)
        }) {
            continue;
        }
        let records = records_of(&decl.name)?;
        count_links(records.len());
        for record in records {
            edges.extend(
                links_of(decl, &record, relation)
                    .into_iter()
                    .filter(|e| e.to.0 == *ty && e.to.1 == *key),
            );
        }
    }
    Ok(edges)
}

/// Edges as a view answers them: each link once, ordered by its source,
/// its relation and its target. The seam promises no order, and the
/// store's walks order their own answers; one order still makes every
/// engine's answer the same bytes.
pub fn order_edges(edges: &mut Vec<Edge>) {
    edges.sort_by(|a, b| (&a.from, &a.relation, &a.to).cmp(&(&b.from, &b.relation, &b.to)));
    edges.dedup_by(|a, b| (&a.from, &a.relation, &a.to) == (&b.from, &b.relation, &b.to));
}

/// The distinct tokens of `text` under the language `tag`.
pub fn tokens(text: &str, tag: &str) -> BTreeSet<String> {
    crate::stem::stem_text(text, tag)
        .split(' ')
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The distinct tokens a record's searchable text fields hold under its
/// type's declaration.
pub fn record_tokens(decl: &TypeDecl, record: &StoredRecord) -> BTreeSet<String> {
    field_tokens(decl, record, None)
}

/// The distinct tokens a record's searchable text fields hold, only those
/// `fields` names when it names any: the text a search limited to named
/// fields matches in.
fn field_tokens(
    decl: &TypeDecl,
    record: &StoredRecord,
    fields: Option<&[FieldName]>,
) -> BTreeSet<String> {
    let tag = decl.language.as_deref().unwrap_or("");
    decl.fields
        .iter()
        .filter(|f| f.searchable && fields.is_none_or(|named| named.contains(&f.name)))
        .filter_map(|f| record.fields.get(&f.name)?.as_str())
        .flat_map(|s| tokens(s, tag))
        .collect()
}

/// Full-text hits, as `docs/seams.md` §`lexical` defines them: the
/// records of one type as they stand at the head, `decl` the type's
/// declaration there, are hits when their searchable text holds a query
/// token — the text of the searchable fields `fields` names, when it
/// names any; `visible` keeps those live at the view's own
/// position. A hit scores the distinct query tokens its head text holds;
/// best first, ties by key, at most `k`.
///
/// An engine may hand in any superset of the hits as `head`: a record
/// whose named fields hold no query token scores nothing and is dropped
/// here, before the ranking is cut at `k`.
pub fn lexical<'a>(
    decl: &TypeDecl,
    head: impl IntoIterator<Item = &'a StoredRecord>,
    visible: impl Fn(&RecordKey) -> bool,
    text: &str,
    fields: Option<&[FieldName]>,
    k: u32,
) -> Vec<(RecordKey, f64)> {
    let query = tokens(text, decl.language.as_deref().unwrap_or(""));
    let mut hits: Vec<(RecordKey, f64)> = head
        .into_iter()
        .filter_map(|r| {
            let score = query.intersection(&field_tokens(decl, r, fields)).count();
            (score > 0 && visible(&r.key)).then(|| (r.key.clone(), score as f64))
        })
        .collect();
    rank(&mut hits, k);
    hits
}

/// Whether a record's state is live: what `lexical` keeps at a view's
/// position. A restricted record is a candidate like any other; whether a
/// principal may see it is the store's, as for `find` and `get`.
pub fn searchable(state: Option<&StoredRecord>) -> bool {
    state.is_some_and(|r| !r.retired)
}

/// Why an event may not be added to a log, as far as its fence goes
/// (`docs/log-format.md` §4.1): with `lease`, every event carries that
/// lease's fence, as an append requires; with none, as an import requires,
/// each keeps the fence it was written under, never below `floor`, the
/// fence of the event before it. Either way the event at position 0
/// carries fence 0, written before any lease existed.
pub fn fence_fault(
    event: &lakelands_api::engine::CanonicalEvent,
    lease: Option<u64>,
    floor: u64,
) -> Option<String> {
    let fence = event.fence;
    let at = event.position;
    match lease {
        _ if at.0 == 0 => (fence != 0).then(|| {
            format!(
                "the event at position 0 carries fence {fence}, and it must carry fence 0; \
                 nothing was appended"
            )
        }),
        Some(wanted) => (fence != wanted).then(|| {
            format!(
                "the event at position {at} carries fence {fence}, and it must carry fence \
                 {wanted}; nothing was appended"
            )
        }),
        None => (fence < floor).then(|| {
            format!(
                "the event at position {at} carries fence {fence}, below the fence {floor} of \
                 the event before it; fences never fall, and nothing was appended"
            )
        }),
    }
}

/// Refuses `events` unless they continue a log kept as lines whose next
/// position is `first` and whose last hash is `prev`, checked event by
/// event, each in this order: its fence ([`fence_fault`], `floor` the
/// fence of the event before the first), its position, its `prev`, and a
/// line feed in its bytes, which canonical bytes never hold.
pub fn append_fault(
    events: &[lakelands_api::engine::CanonicalEvent],
    first: u64,
    mut prev: lakelands_api::event::Digest,
    lease: Option<u64>,
    mut floor: u64,
) -> Result<()> {
    for (expected, event) in (first..).zip(events) {
        if let Some(why) = fence_fault(event, lease, floor) {
            return Err(invalid(why));
        }
        floor = event.fence;
        if event.position.0 != expected {
            return Err(invalid(format!(
                "the event at position {} does not continue the log, whose next position \
                 is {expected}; nothing was appended",
                event.position
            )));
        }
        if event.prev != prev {
            return Err(invalid(format!(
                "the event at position {} names `{}` as the hash before it, but that hash \
                 is `{}`; nothing was appended",
                event.position, event.prev.0, prev.0
            )));
        }
        if event.bytes.contains(&b'\n') {
            return Err(invalid(format!(
                "the event at position {} holds a line feed in its bytes, which canonical \
                 bytes never do; nothing was appended",
                event.position
            )));
        }
        prev = event.hash.clone();
    }
    Ok(())
}

/// The refusal of the writer lease to a taker while `holder` holds it
/// under `fence`.
pub fn lease_held(holder: &str, fence: u64) -> Error {
    crate::unavailable(format!(
        "the lake's writer lease is held by `{holder}` (fence {fence}); it must be released \
         before another writer can take it"
    ))
}

/// Best first by score, ties by key, at most `k`.
pub fn rank(hits: &mut Vec<(RecordKey, f64)>, k: u32) {
    hits.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    hits.truncate(k as usize);
}

/// The best `k` of a stream of scored keys, holding at most `k` of them
/// at any time: what `rank` answers over the whole stream, without
/// keeping it. `rank`'s order is total, so its first `k` are one set in
/// one order however the stream arrives, and a bounded heap ordered by it
/// keeps exactly them. An engine's vector scan feeds every vector's score
/// here, so a search holds `k` entries, not the field.
#[derive(Debug)]
pub struct TopK {
    k: usize,
    /// The kept entries, the one ranking last on top.
    heap: BinaryHeap<Kept>,
}

/// A kept entry, ordered as `rank` orders: greater ranks later.
#[derive(Debug)]
struct Kept(RecordKey, f64);

impl PartialEq for Kept {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Kept {}

impl PartialOrd for Kept {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Kept {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .1
            .total_cmp(&self.1)
            .then_with(|| self.0.cmp(&other.0))
    }
}

impl TopK {
    /// Keeps the best `k`; `k` may exceed what the stream holds.
    pub fn new(k: u32) -> Self {
        Self {
            k: k as usize,
            heap: BinaryHeap::new(),
        }
    }

    /// Offers one scored key; the key is cloned only when it is kept.
    pub fn push(&mut self, key: &RecordKey, score: f64) {
        if self.heap.len() < self.k {
            self.heap.push(Kept(key.clone(), score));
            return;
        }
        let Some(mut last) = self.heap.peek_mut() else {
            return;
        };
        let before = match score.total_cmp(&last.1) {
            Ordering::Greater => true,
            Ordering::Less => false,
            Ordering::Equal => *key < last.0,
        };
        if before {
            *last = Kept(key.clone(), score);
        }
    }

    /// The kept entries, best first, ties by key.
    pub fn into_ranked(self) -> Vec<(RecordKey, f64)> {
        self.heap
            .into_sorted_vec()
            .into_iter()
            .map(|Kept(key, score)| (key, score))
            .collect()
    }
}

/// The types a proposed write touches: a record write's type, the type a
/// called verb acts on, every type a declaration declares or acts on, and a
/// migration's type.
fn touched(
    write: &ProposedWrite,
    verb_on: &impl Fn(&VerbName) -> Option<TypeName>,
) -> Vec<TypeName> {
    match write {
        ProposedWrite::Upsert { ty, .. } | ProposedWrite::Retire { ty, .. } => vec![ty.clone()],
        ProposedWrite::Call { verb, .. } => verb_on(verb).into_iter().collect(),
        ProposedWrite::Declare { declarations } => declarations
            .iter()
            .filter_map(|d| match d {
                Declaration::Type(t) => Some(t.name.clone()),
                Declaration::Verb(v) => Some(v.on.clone()),
                Declaration::Trigger(t) => Some(t.on.clone()),
                Declaration::ObjectSet(s) => Some(s.of.clone()),
                Declaration::ValueType(_) | Declaration::Function(_) => None,
            })
            .collect(),
        ProposedWrite::Migrate { migration, .. } => vec![migration.to.name.clone()],
        // A value type's tightening names no type, as its declaration names none.
        ProposedWrite::MigrateValueType { .. } => Vec::new(),
    }
}

/// Proposals as a view answers them: those in `filter`'s state, of its
/// kind and touching its type (`verb_on` says which type a verb acts on,
/// as declared at the view's position), newest first, ties by id, at most
/// `filter.limit`.
pub fn proposals<'a>(
    all: impl IntoIterator<Item = &'a Proposal>,
    filter: &ProposalFilter,
    verb_on: impl Fn(&VerbName) -> Option<TypeName>,
) -> Vec<Proposal> {
    let mut found: Vec<&Proposal> = all
        .into_iter()
        .filter(|p| filter.state.is_none_or(|s| p.state == s))
        .filter(|p| filter.kind.is_none_or(|k| p.write.kind() == k))
        .filter(|p| {
            filter
                .ty
                .as_ref()
                .is_none_or(|ty| touched(&p.write, &verb_on).contains(ty))
        })
        .collect();
    found.sort_by(|a, b| b.position.cmp(&a.position).then_with(|| a.id.cmp(&b.id)));
    found
        .into_iter()
        .take(filter.limit.map_or(usize::MAX, |l| l as usize))
        .cloned()
        .collect()
}

/// A vector as an index stores it: its components as little-endian
/// `f32`s.
pub fn vector_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

/// The vector [`vector_bytes`] stored.
pub fn vector_of(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

/// The position a view reads at: `as_of`, or the position the projection
/// reflects (`folded`) when none is asked; a position past it is out of
/// range.
pub fn view_at(as_of: Option<Position>, folded: Option<Position>) -> Result<Option<Position>> {
    match (as_of, folded) {
        (None, folded) => Ok(folded),
        (Some(p), Some(head)) if p <= head => Ok(Some(p)),
        (Some(_), head) => Err(Error::Refused {
            field: None,
            reason: Refusal::OutOfRange {
                head: head.unwrap_or_default(),
            },
        }),
    }
}

/// Why a transaction cannot commit through `through`, if it cannot: the
/// projection already reflects a later position (`folded`), or it puts a
/// state valid only from a later one (`late`).
pub fn commit_refusal(
    folded: Option<Position>,
    late: Option<Position>,
    through: Position,
) -> Option<Error> {
    if let Some(folded) = folded.filter(|f| *f > through) {
        return Some(invalid(format!(
            "the projection reflects the log through position {folded}; a transaction \
             cannot commit through the earlier position {through}"
        )));
    }
    let late = late.filter(|p| *p > through)?;
    Some(invalid(format!(
        "a state valid from position {late} cannot be committed through the earlier \
         position {through}"
    )))
}

#[cfg(test)]
mod tests;
