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
//!   vectors, computed in `f64`; 0 when either is all zeros. Best first,
//!   ties by key.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::engine::{After, Scan};
use lakelands_api::expr::Expr;
use lakelands_api::id::{FieldName, RecordKey, RelationName, TypeName, VerbName};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::query::Edge;
use lakelands_api::read::{Direction, Order, OrderBy};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::time::Timestamp;
use lakelands_api::write::{Declaration, Proposal, ProposalFilter, ProposedWrite};

pub use crate::expr::order::{cmp_values, sort_key};
use crate::expr::{RecordScope, holds};

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

/// Whether `filter` selects `record`; a filter that cannot be evaluated
/// does not.
pub fn selects(filter: Option<&Expr>, record: &StoredRecord) -> bool {
    filter.is_none_or(|f| {
        let scope = RecordScope {
            fields: &record.fields,
            settings: None,
            now: NO_CLOCK,
        };
        holds(f, &scope).unwrap_or(false)
    })
}

/// Whether `record` holds at `valid_at` under its type's declaration. A
/// type not in force is not valid-timed.
pub fn valid(decl: Option<&TypeDecl>, record: &StoredRecord, valid_at: Option<Timestamp>) -> bool {
    match (decl, valid_at) {
        (Some(decl), Some(t)) => crate::valid_time::record_holds(decl, &record.fields, t),
        _ => true,
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
/// it was learned or observed. `None` when absent.
pub fn sort_value(record: &StoredRecord, order: &Order) -> Option<Value> {
    match order.by {
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

/// Whether a scan selects `record`: its flags, `valid_at` and the filter.
pub fn admits(decl: Option<&TypeDecl>, record: &StoredRecord, scan: &Scan) -> bool {
    (scan.retired || !record.retired)
        && (scan.restricted || !record.restricted)
        && valid(decl, record, scan.valid_at)
        && selects(scan.filter.as_ref(), record)
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
        .map(|r| {
            (
                PlaceKey::of(&Place::of(r, &scan.order_by), &scan.order_by),
                r,
            )
        })
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
                .any(|f| f.name.as_str() == name && lakelands_api::seam::sortable(&f.ty.kind))
        {
            out.push(FieldName::from(name));
        }
    }
    out
}

/// The type a record asks an order index for, when it is a state of the
/// setting `sort_index.<type>`.
pub fn sort_index_type(r: &StoredRecord) -> Option<TypeName> {
    (r.ty.as_str() == lakelands_api::seam::SETTING_TYPE)
        .then(|| {
            r.key
                .as_str()
                .strip_prefix(lakelands_api::seam::SORT_INDEX_SETTING)
        })
        .flatten()
        .map(TypeName::from)
}

/// The key of the setting that asks for `ty`'s order index.
pub fn sort_index_key(ty: &TypeName) -> String {
    format!("{}{ty}", lakelands_api::seam::SORT_INDEX_SETTING)
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
}

/// The rows the scans on this thread were handed to order: the whole
/// type in the fallback, [`scan`], and the index rows and missing records
/// in [`scan_in_field_order`]. It exists for the engine checks, which
/// count by it that a page ordered by an indexed field reads its page,
/// not the type; it costs one add per call or batch.
pub fn rows_read() -> u64 {
    ROWS_READ.with(|n| n.get())
}

fn count_read(n: usize) {
    ROWS_READ.with(|c| c.set(c.get() + n as u64));
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
    mut sorted: impl FnMut(Option<(&[u8], &str)>, u32) -> Result<Vec<(Vec<u8>, StoredRecord)>, E>,
    mut missing: impl FnMut(Option<&str>, u32) -> Result<Vec<StoredRecord>, E>,
) -> Result<Vec<StoredRecord>, E> {
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
    // A run's rows ordered by the rulebook, those past `after` kept.
    let take_run = |run: &mut Vec<StoredRecord>, out: &mut Vec<StoredRecord>| {
        let mut keyed: Vec<(PlaceKey, StoredRecord)> = run
            .drain(..)
            .filter(|r| admits(decl, r, scan))
            .map(|r| {
                (
                    PlaceKey::of(&Place::of(&r, &scan.order_by), &scan.order_by),
                    r,
                )
            })
            .filter(|(k, _)| after.as_ref().is_none_or(|a| k > a))
            .collect();
        keyed.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        out.extend(keyed.into_iter().map(|(_, r)| r).take(limit - out.len()));
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
            count_read(rows.len());
            let read_all = rows.len() < n as usize;
            for (k, r) in rows {
                from = Some((k.clone(), r.key.to_string()));
                if !is_run(&k) {
                    take_run(&mut run, &mut out);
                    run_key = None;
                    if out.len() >= limit {
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
                    take_run(&mut run, &mut out);
                    if out.len() >= limit {
                        return Ok(out);
                    }
                    run_key = Some(k);
                }
                run.push(r);
            }
            if read_all {
                take_run(&mut run, &mut out);
                if out.len() >= limit {
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
        count_read(rows.len());
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
        for record in records_of(&decl.name)? {
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
    let tag = decl.language.as_deref().unwrap_or("");
    decl.fields
        .iter()
        .filter(|f| f.searchable)
        .filter_map(|f| record.fields.get(&f.name)?.as_str())
        .flat_map(|s| tokens(s, tag))
        .collect()
}

/// Full-text hits, as `docs/seams.md` §`lexical` defines them: the
/// records of one type as they stand at the head, `decl` the type's
/// declaration there, are hits when their searchable text holds a query
/// token; `visible` keeps those live at the view's own
/// position. A hit scores the distinct query tokens its head text holds;
/// best first, ties by key, at most `k`.
pub fn lexical<'a>(
    decl: &TypeDecl,
    head: impl IntoIterator<Item = &'a StoredRecord>,
    visible: impl Fn(&RecordKey) -> bool,
    text: &str,
    k: u32,
) -> Vec<(RecordKey, f64)> {
    let query = tokens(text, decl.language.as_deref().unwrap_or(""));
    let mut hits: Vec<(RecordKey, f64)> = head
        .into_iter()
        .filter_map(|r| {
            let score = query.intersection(&record_tokens(decl, r)).count();
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

/// Best first by score, ties by key, at most `k`.
pub fn rank(hits: &mut Vec<(RecordKey, f64)>, k: u32) {
    hits.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    hits.truncate(k as usize);
}

/// Proposals as a view answers them: those in `filter`'s state and of its
/// type (`verb_on` says which type a verb acts on, as declared at the
/// view's position), newest first, ties by id, at most `filter.limit`.
pub fn proposals<'a>(
    all: impl IntoIterator<Item = &'a Proposal>,
    filter: &ProposalFilter,
    verb_on: impl Fn(&VerbName) -> Option<TypeName>,
) -> Vec<Proposal> {
    let mut found: Vec<&Proposal> = all
        .into_iter()
        .filter(|p| filter.state.is_none_or(|s| p.state == s))
        .filter(|p| {
            filter.ty.as_ref().is_none_or(|ty| {
                let touched = match &p.write {
                    ProposedWrite::Upsert { ty, .. } | ProposedWrite::Retire { ty, .. } => {
                        Some(ty.clone())
                    }
                    ProposedWrite::Call { verb, .. } => verb_on(verb),
                };
                touched.as_ref() == Some(ty)
            })
        })
        .collect();
    found.sort_by(|a, b| b.position.cmp(&a.position).then_with(|| a.id.cmp(&b.id)));
    found
        .into_iter()
        .take(filter.limit.map_or(usize::MAX, |l| l as usize))
        .cloned()
        .collect()
}

/// The cosine similarity of two vectors of one length, in `f64`; 0 when
/// either is all zeros.
pub fn cosine(a: &[f32], b: &[f32]) -> f64 {
    let (mut dot, mut na, mut nb) = (0.0f64, 0.0f64, 0.0f64);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (f64::from(*x), f64::from(*y));
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_compare_numerically_even_as_text() {
        assert_eq!(cmp_values(&json!(9), &json!("10")), Ordering::Less);
        assert_eq!(cmp_values(&json!("1.0"), &json!(1)), Ordering::Equal);
        assert_eq!(cmp_values(&json!(2.5), &json!(-3)), Ordering::Greater);
    }

    /// Values of every kind the rulebook ranks, with equal values written
    /// differently: scales, numbers as text, one instant in two offsets,
    /// JSON a float and an integer print the same in.
    fn sort_corpus() -> Vec<serde_json::Value> {
        let mut values = vec![
            json!(0),
            json!(0.0),
            json!("0"),
            json!("-0.00"),
            json!(1),
            json!(1.0),
            json!("1.00"),
            json!("1"),
            json!(-1),
            json!(-1.5),
            json!("-1.55"),
            json!(-10),
            json!(10),
            json!(100),
            json!("100.5"),
            json!(0.001),
            json!("0.0010"),
            json!(1e20),
            json!(-1e-20),
            json!(1e300),
            json!(2.5),
            json!("-3"),
            json!("79228162514264337593543950335"),
            json!("-0.0000000000000000000000000001"),
            json!("2026-01-01"),
            json!("2026-01-01T00:00:00Z"),
            json!("2026-01-01T01:00:00+01:00"),
            json!("2025-12-31T23:59:59.999999Z"),
            json!("1969-07-20T20:17:00Z"),
            json!("abc"),
            json!("Abc"),
            json!("ab"),
            json!("ä"),
            json!(""),
            json!(" "),
            json!("1e3"),
            json!(format!("{}b", "long prefix ".repeat(4))),
            json!(format!("{}a", "long prefix ".repeat(4))),
            json!(format!("{}c", "long prefix ".repeat(4))),
            json!("long prefix ".repeat(4)),
            json!(false),
            json!(true),
            json!({"a": 1}),
            json!({"a": 1.0}),
            json!([1, 2]),
            json!([]),
            json!(null),
        ];
        // Decimals of many scales and both signs, as numbers and as text.
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        for _ in 0..300 {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            let mantissa = (x >> 20) % 2_000_001;
            let scale = (x >> 8) % 7;
            let sign = if x & 1 == 0 { "" } else { "-" };
            let text = format!(
                "{sign}{}.{:0>width$}",
                mantissa / 10u64.pow(scale as u32),
                mantissa % 10u64.pow(scale as u32),
                width = scale as usize
            );
            let text = text.trim_end_matches('.').to_owned();
            values.push(json!(text));
            if let Ok(n) = text.parse::<f64>() {
                values.push(json!(n));
            }
        }
        values
    }

    #[test]
    fn sort_keys_order_as_cmp_values_and_are_equal_where_it_is() {
        let values = sort_corpus();
        for a in &values {
            for b in &values {
                assert_eq!(
                    sort_key(a).cmp(&sort_key(b)),
                    cmp_values(a, b),
                    "the sort keys of {a} and {b} order them otherwise than cmp_values"
                );
            }
        }
    }

    #[test]
    fn a_scan_sorted_by_keys_answers_what_cmp_places_orders() {
        let records: Vec<StoredRecord> = sort_corpus()
            .into_iter()
            .enumerate()
            .map(|(i, v)| StoredRecord {
                ty: TypeName::from("t"),
                key: RecordKey::from(format!("k{:04}", (i * 37) % 1000)),
                version: lakelands_api::Version(1),
                fields: [(FieldName::from("v"), v)].into_iter().collect(),
                meta: BTreeMap::new(),
                position: lakelands_api::Position(1),
                retired: false,
                restricted: false,
            })
            .collect();
        for dir in [Direction::Asc, Direction::Desc] {
            let order_by = vec![Order {
                field: FieldName::from("v"),
                by: OrderBy::Value,
                dir,
            }];
            let mut expected: Vec<&StoredRecord> = records.iter().collect();
            expected.sort_by(|a, b| {
                cmp_places(
                    &Place::of(a, &order_by),
                    &Place::of(b, &order_by),
                    &order_by,
                )
            });
            let got = scan(
                None,
                &records,
                &Scan {
                    order_by: order_by.clone(),
                    limit: u32::MAX,
                    ..Default::default()
                },
            );
            let keys = |rows: Vec<&StoredRecord>| -> Vec<String> {
                rows.into_iter().map(|r| r.key.to_string()).collect()
            };
            assert_eq!(keys(got.iter().collect()), keys(expected), "{dir:?}");
        }
    }

    /// Walks `scan` page by page through [`scan_in_field_order`] over an
    /// index kept in a vector, continuing after each page's last row.
    fn walk_by_index(decl: &TypeDecl, records: &[StoredRecord], scan: &Scan) -> Vec<String> {
        let first = &scan.order_by[0];
        let mut index: Vec<(Vec<u8>, &StoredRecord)> = records
            .iter()
            .filter_map(|r| {
                order_rows(&sorted_fields(decl, Some(&json!(["v", "w"]))), r)
                    .into_iter()
                    .find(|(f, _)| **f == first.field)
                    .map(|(_, k)| (k, r))
            })
            .collect();
        index.sort_by(|a, b| match first.dir {
            Direction::Asc => (&a.0, &a.1.key).cmp(&(&b.0, &b.1.key)),
            Direction::Desc => b.0.cmp(&a.0).then(a.1.key.cmp(&b.1.key)),
        });
        let position = |k: &[u8], key: &str| -> (Vec<u8>, String) { (k.to_vec(), key.to_owned()) };
        let past = |row: &(Vec<u8>, &StoredRecord), from: &(Vec<u8>, String)| match first.dir {
            Direction::Asc => position(&row.0, row.1.key.as_str()) > *from,
            Direction::Desc => {
                row.0 < from.0 || (row.0 == from.0 && row.1.key.as_str() > from.1.as_str())
            }
        };
        let mut missing: Vec<&StoredRecord> = records
            .iter()
            .filter(|r| r.fields.get(&first.field).is_none_or(|v| v.is_null()))
            .collect();
        missing.sort_by(|a, b| a.key.cmp(&b.key));
        let mut keys = Vec::new();
        let mut page = scan.clone();
        loop {
            let rows = scan_in_field_order::<()>(
                Some(decl),
                &page,
                |from, n| {
                    let from = from.map(|(k, r)| position(k, r));
                    Ok(index
                        .iter()
                        .filter(|row| from.as_ref().is_none_or(|f| past(row, f)))
                        .take(n as usize)
                        .map(|(k, r)| (k.clone(), (*r).clone()))
                        .collect())
                },
                |after, n| {
                    Ok(missing
                        .iter()
                        .filter(|r| after.is_none_or(|a| r.key.as_str() > a))
                        .take(n as usize)
                        .map(|r| (*r).clone())
                        .collect())
                },
            )
            .unwrap();
            let Some(last) = rows.last() else { break };
            page.after = Some(After {
                values: page
                    .order_by
                    .iter()
                    .map(|o| sort_value(last, o).unwrap_or(Value::Null))
                    .collect(),
                key: last.key.clone(),
            });
            keys.extend(rows.iter().map(|r| r.key.to_string()));
        }
        keys
    }

    #[test]
    fn a_walk_in_index_order_answers_what_a_whole_sort_answers() {
        let decl: TypeDecl = serde_json::from_value(json!({
            "name": "t", "fields": [
                {"name": "v", "type": {"kind": "text"}},
                {"name": "w", "type": {"kind": "text"}}
            ]
        }))
        .unwrap();
        let corpus = sort_corpus();
        let records: Vec<StoredRecord> = (0..corpus.len() + 40)
            .map(|i| {
                let mut fields = BTreeMap::new();
                // Every fifth record misses `v`; `w` has few values, so
                // two fields make runs.
                if i % 5 != 0 {
                    fields.insert(FieldName::from("v"), corpus[i % corpus.len()].clone());
                }
                if i % 7 != 0 {
                    fields.insert(FieldName::from("w"), json!(format!("w{}", i % 3)));
                }
                StoredRecord {
                    ty: TypeName::from("t"),
                    key: RecordKey::from(format!("k{:04}", (i * 37) % 1009)),
                    version: lakelands_api::Version(1),
                    fields,
                    meta: BTreeMap::new(),
                    position: lakelands_api::Position(1),
                    retired: i % 11 == 0,
                    restricted: false,
                }
            })
            .collect();
        let order = |field: &str, dir| Order {
            field: FieldName::from(field),
            by: OrderBy::Value,
            dir,
        };
        for dir in [Direction::Asc, Direction::Desc] {
            for order_by in [
                vec![order("v", dir)],
                vec![order("w", dir), order("v", Direction::Asc)],
                vec![order("v", dir), order("w", Direction::Desc)],
            ] {
                for filter in [None, Some(crate::expr::parse("w != 'w1'").unwrap())] {
                    let scan = Scan {
                        order_by: order_by.clone(),
                        filter: filter.clone(),
                        limit: 3,
                        ..Default::default()
                    };
                    let whole: Vec<String> = super::scan(
                        Some(&decl),
                        &records,
                        &Scan {
                            limit: u32::MAX,
                            ..scan.clone()
                        },
                    )
                    .into_iter()
                    .map(|r| r.key.to_string())
                    .collect();
                    assert_eq!(
                        walk_by_index(&decl, &records, &scan),
                        whole,
                        "{order_by:?} with {filter:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn mixed_kinds_order_totally() {
        let mut values = vec![
            json!("abc"),
            json!(true),
            json!("10"),
            json!({"a": 1}),
            json!("2026-01-01"),
            json!(9),
            json!(false),
        ];
        values.sort_by(cmp_values);
        assert_eq!(
            values,
            vec![
                json!(9),
                json!("10"),
                json!("2026-01-01"),
                json!("abc"),
                json!(false),
                json!(true),
                json!({"a": 1}),
            ]
        );
    }

    #[test]
    fn missing_values_sort_last_in_both_directions() {
        let (one, two) = (json!(1), json!(2));
        for dir in [Direction::Asc, Direction::Desc] {
            assert_eq!(cmp_key(None, Some(&one), dir), Ordering::Greater);
            assert_eq!(cmp_key(Some(&one), None, dir), Ordering::Less);
        }
        assert_eq!(
            cmp_key(Some(&one), Some(&two), Direction::Desc),
            Ordering::Greater
        );
    }

    #[test]
    fn only_text_that_compares_as_text_narrows() {
        let term = |s: &str| exact_text_term(&crate::expr::parse(s).unwrap());
        assert_eq!(
            term("status = 'open'"),
            Some(("status".into(), "open".into()))
        );
        assert_eq!(
            term("n > 1 and 'open' = status"),
            Some(("status".into(), "open".into()))
        );
        assert_eq!(term("status = '10'"), None, "compares as a number");
        assert_eq!(
            term("at = '2026-01-01T00:00:00Z'"),
            None,
            "compares as an instant"
        );
        assert_eq!(term("status = 'a' or status = 'b'"), None);
    }

    #[test]
    fn cosine_ignores_length_and_a_zero_vector_scores_zero() {
        assert!((cosine(&[3.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-12);
        assert!((cosine(&[1.0, 1.0], &[1.0, 0.0]) - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-9);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }

    #[test]
    fn ranking_is_best_first_with_ties_by_key() {
        let mut hits = vec![("b".into(), 1.0), ("a".into(), 1.0), ("c".into(), 2.0)];
        rank(&mut hits, 2);
        let keys: Vec<&str> = hits
            .iter()
            .map(|(k, _): &(RecordKey, f64)| k.as_str())
            .collect();
        assert_eq!(keys, ["c", "a"]);
    }
}
