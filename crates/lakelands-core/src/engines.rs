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

pub use crate::expr::order::cmp_values;
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
    let mut rows: Vec<&StoredRecord> = records
        .into_iter()
        .filter(|r| admits(decl, r, scan))
        .collect();
    rows.sort_by(|a, b| {
        cmp_places(
            &Place::of(a, &scan.order_by),
            &Place::of(b, &scan.order_by),
            &scan.order_by,
        )
    });
    if let Some(after) = &scan.after {
        let after = Place::after(after);
        rows.retain(|r| cmp_places(&Place::of(r, &scan.order_by), &after, &scan.order_by).is_gt());
    }
    rows.into_iter()
        .take(scan.limit as usize)
        .cloned()
        .collect()
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
