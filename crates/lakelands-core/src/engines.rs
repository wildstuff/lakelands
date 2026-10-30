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
//! - **Links.** A relation carried by a field (`via`) links a record to
//!   the record of the relation's target whose key is that field's text
//!   value, or, where the field holds an array, to that of each distinct
//!   string element ([`keys_of`]); the links into a record are those of
//!   every type with such a relation to its type.
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
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet};

use lakelands_api::engine::{
    After, CanonicalEvent, Choose, Chosen, Rankings, Scan, TextCounts, TextStats,
};
use lakelands_api::error::{Case, Detail, Error, Flaw, Part, Place as At, Refusal, Result, Said};
use lakelands_api::event::SegmentDigest;
use lakelands_api::expr::{Condition, Expr};
use lakelands_api::id::{FieldName, LakeId, Position, RecordKey, RelationName, TypeName};
use lakelands_api::ontology::{FieldDecl, StorageKind, TypeDecl};
use lakelands_api::query::{Edge, MATCH_SPANS, Matched, Span};
use lakelands_api::read::{Direction, Order, OrderBy};
use lakelands_api::record::{FieldMeta, Record, StoredRecord, Value};
use lakelands_api::time::Timestamp;
use lakelands_api::write::{Declaration, DeclarationKind, Proposal, ProposalFilter, ProposedWrite};

pub use crate::expr::order::{cmp_values, sort_key};
use crate::expr::{RecordScope, holds};

pub mod pool;

/// The clock an engine evaluates a filter with. The store hands an engine
/// only filters that do not read the clock, so any fixed instant will do.
const NO_CLOCK: Timestamp = Timestamp(0);

/// `name`, when it is a head-only name ([`lakelands_api::seam::is_head_only`])
/// an engine may end whole; otherwise a refusal. Ending a stored type's
/// name would delete its rows and leave its indexes behind.
pub fn head_only(name: &TypeName) -> Result<()> {
    match lakelands_api::seam::is_head_only(name) {
        true => Ok(()),
        false => Err(Error::invalid(Case::NotHeadOnly { ty: name.clone() })),
    }
}

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
        Declaration::Setting(s) => format!("setting {}", s.key),
        // A withdrawal is the next state of the declaration it withdraws.
        Declaration::Withdraw(w) => format!("{} {}", w.kind.as_str(), w.name),
        _ => unreachable!("a declaration of a kind this release does not know"),
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
    exact_text_terms(filter).into_iter().next()
}

/// Every such narrowing of a filter, its `and` terms in order.
pub fn exact_text_terms(filter: &Expr) -> Vec<(FieldName, String)> {
    use lakelands_api::expr::{CmpOp, Literal};
    match filter {
        Expr::And { args } => args.iter().flat_map(exact_text_terms).collect(),
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
                && Timestamp::denoted(t).is_err() =>
            {
                vec![(path[0].clone(), t.clone())]
            }
            _ => Vec::new(),
        },
        _ => Vec::new(),
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
pub fn cmp_key(a: Option<&Value>, b: Option<&Value>, dir: Direction) -> Ordering {
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
        meta: None,
        settings: None,
        now: NO_CLOCK,
    };
    (scan.retired || !record.retired)
        && (scan.restricted || !record.restricted)
        && (decl.zip(scan.valid_at))
            .is_none_or(|(d, t)| crate::valid_time::record_holds(d, fields, t))
        && holds_exactly(record, &scan.equal)
        && (scan.equal_any.is_empty() || scan.equal_any.iter().any(|t| holds_exactly(record, t)))
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
/// index at the head, of the fields `kept` says it keeps rows for
/// ([`looked_up`]): a string the scan must hold exactly, else one of the
/// filter's [`exact_text_terms`]. The engine still admits each candidate.
/// Asked with every field kept, it is the text a scan of the type may
/// narrow its storage's read by.
pub fn text_lookup(scan: &Scan, kept: &dyn Fn(&FieldName) -> bool) -> Option<(FieldName, String)> {
    let terms = scan.filter.iter().flat_map(exact_text_terms);
    (scan.equal.iter())
        .filter_map(|(f, v)| Some((f.clone(), v.as_str()?.to_owned())))
        .chain(terms)
        .find(|(f, _)| kept(f))
}

/// The one field and the texts an engine may look candidates up by in its
/// text index at the head, in one ask: those of `equal_any` where each of
/// its tuples holds a string for the same field, else [`text_lookup`]'s
/// one. The engine still admits each candidate.
pub fn text_lookups(
    scan: &Scan,
    kept: &dyn Fn(&FieldName) -> bool,
) -> Option<(FieldName, Vec<String>)> {
    let first = |t: &[(FieldName, Value)]| {
        t.first()
            .and_then(|(f, v)| Some((f.clone(), v.as_str()?.to_owned())))
    };
    let Some((field, _)) = (scan.equal_any.first().and_then(|t| first(t))).filter(|(f, _)| kept(f))
    else {
        return text_lookup(scan, kept).map(|(f, t)| (f, vec![t]));
    };
    let texts = scan
        .equal_any
        .iter()
        .map(|t| first(t).filter(|(f, _)| *f == field).map(|(_, v)| v));
    // A tuple that holds no text there: the one lookup `equal` gives.
    texts
        .collect::<Option<Vec<String>>>()
        .map(|texts| (field, texts))
        .or_else(|| text_lookup(scan, kept).map(|(f, t)| (f, vec![t])))
}

/// The field of a type whose key is that one text field. A record's key
/// is then its value made a key component (`lakelands_api::key`), which
/// reads back into the value, so the head state at that key is the only
/// one that can hold a text there: the exact-text index keeps no rows for
/// it, and a lookup by it reads that head by key ([`key_lookup`]).
pub fn text_key(decl: &TypeDecl) -> Option<&FieldName> {
    let [key] = decl.key.as_slice() else {
        return None;
    };
    let text = |f: &FieldDecl| f.name == *key && f.ty.kind == StorageKind::Text;
    decl.fields.iter().any(text).then_some(key)
}

/// The keys a scan of a declared type reads when it must hold exactly a
/// text ([`text_lookups`]: an `=` of the filter, `equal`, or one of the
/// texts a match's `equal_any` asks for) in the type's [`text_key`]: those
/// the texts make, in key order. The engine reads their states at its
/// position and the rulebook admits, orders and cuts them ([`scan`]).
pub fn key_lookup(decl: Option<&TypeDecl>, scan: &Scan) -> Option<BTreeSet<String>> {
    let key = text_key(decl?)?;
    let (_, texts) = text_lookups(scan, &|f| f == key)?;
    let made = texts.into_iter().map(serde_json::Value::String);
    Some(
        made.filter_map(|t| lakelands_api::key::key_component(&t))
            .collect(),
    )
}

/// The text fields of `ty` its exact-text index keeps rows for, of the
/// lake's declarations at the head `decls` and its `lookup_index.<type>`
/// setting's value: those its declarations look its records up by (its
/// key, but a [`text_key`], its references, the fields a derived relation
/// matches at either end, those a set's condition or a view's `where` over
/// it compares to a text at the top level, those a constraint's
/// `preceding` matches its series on) and those the setting names
/// (`docs/decisions/057-an-exact-text-index-is-kept-where-it-is-looked-up.md`).
/// `None`, every field, for a name no type declares: a view's kept records.
pub fn looked_up<'a>(
    ty: &TypeName,
    decls: impl IntoIterator<Item = &'a Declaration>,
    setting: Option<&Value>,
) -> Option<BTreeSet<FieldName>> {
    let (mut own, mut out) = (None, BTreeSet::new());
    let mut terms = |c: &Option<Condition>| {
        let e = c.as_ref().and_then(|c| c.typed().ok());
        out.extend(e.iter().flat_map(exact_text_terms).map(|(f, _)| f));
    };
    let mut matched = Vec::new();
    for d in decls {
        match d {
            Declaration::Type(t) => {
                own = own.or((t.name == *ty).then_some(t));
                for r in &t.relations {
                    for p in r.derived.iter().flat_map(|d| &d.pairs) {
                        (t.name == *ty).then(|| matched.push(p.from.clone()));
                        (r.target == *ty).then(|| matched.push(p.to.clone()));
                    }
                }
                for v in t.view.iter().filter(|v| v.source == *ty) {
                    terms(&v.r#where);
                }
                for m in t.view.iter().flat_map(|v| &v.merge) {
                    (m.source == *ty).then(|| terms(&m.r#where));
                }
            }
            Declaration::ObjectSet(o) if o.of == *ty => terms(&Some(o.filter.clone())),
            _ => {}
        }
    }
    let t = own?;
    let typed = t.constraints.iter().filter_map(|c| c.holds.typed().ok());
    out.extend(
        typed
            .flat_map(|e| e.series().into_keys())
            .flat_map(|s| s.into_iter().skip(1)),
    );
    let refs = t
        .fields
        .iter()
        .filter(|f| f.ty.kind == StorageKind::Reference);
    out.extend(matched.into_iter().chain(t.key.iter().cloned()));
    out.extend(refs.map(|f| f.name.clone()));
    out.extend(index_fields(t, setting, false, |k| *k == StorageKind::Text));
    let texts = |f: &FieldDecl| matches!(f.ty.kind, StorageKind::Text | StorageKind::Reference);
    out.retain(|f| t.fields.iter().any(|d| d.name == *f && texts(d)) && Some(f) != text_key(t));
    Some(out)
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
/// key order. It may leave out states the scan cannot select (an engine's
/// index names the candidates, [`text_lookup`]), but a batch shorter than
/// `n` ends the walk: it answers `n` whenever `n` remain. It answers what
/// [`scan`] answers over every state.
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
        count_read(rows.len());
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

/// How many of `records` [`scan`] would select, its order, keyset
/// position and limit ignored: a count over states already read, such as
/// those a [`key_lookup`] read.
pub fn count_of(decl: Option<&TypeDecl>, records: &[StoredRecord], scan: &Scan) -> u64 {
    count_read(records.len());
    records.iter().filter(|r| admits(decl, r, scan)).count() as u64
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

/// The fields of `ty` whose order index a lake keeps, by `decl`, the
/// declaration of `ty` (of its [`kept_view`] for a kept name) at the head,
/// and `setting`, the value of its [`sort_index_key`] setting there: those
/// it names that the type declares with a kind the index keeps
/// ([`lakelands_api::seam::sortable`]), once each, in the setting's order.
/// A view keeps none of its own; its kept records are ordered by the
/// fields it declares. A setting the store would refuse keeps what it
/// can; none keeps none.
pub fn sorted_fields(ty: &TypeName, decl: &TypeDecl, setting: Option<&Value>) -> Vec<FieldName> {
    let kept = kept_view(ty).is_some();
    index_fields(decl, setting, kept, lakelands_api::seam::sortable)
}

/// The view a kept name (`sys:kept.<view>`) holds the records of. They are
/// ordered as the view declares them, by the order index its
/// `sort_index.<view>` setting asks for.
pub fn kept_view(ty: &TypeName) -> Option<TypeName> {
    let view = ty.as_str().strip_prefix(lakelands_api::seam::KEPT_PREFIX)?;
    Some(TypeName::from(view))
}

/// The names whose order index the setting `sort_index.<ty>` asks for:
/// `ty`'s own, and its kept name's where it is a view.
pub fn ordered_names(ty: &TypeName) -> [TypeName; 2] {
    [ty.clone(), lakelands_api::seam::kept_name(ty)]
}

/// The vector fields of `decl` a lake asks an index for, by `setting`, the
/// value of its `vector_index.<type>` setting: those it names that the
/// type declares as vectors, once each, in the setting's order.
pub fn indexed_fields(decl: &TypeDecl, setting: Option<&Value>) -> Vec<FieldName> {
    index_fields(decl, setting, false, lakelands_api::seam::indexable)
}

fn index_fields(
    decl: &TypeDecl,
    setting: Option<&Value>,
    kept: bool,
    keeps: fn(&lakelands_api::ontology::StorageKind) -> bool,
) -> Vec<FieldName> {
    let mut out: Vec<FieldName> = Vec::new();
    if decl.view.is_some() != kept {
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

/// The types whose exact-text index a declaration can move: those
/// [`looked_up`] reads it for. A type's own, the targets of its derived
/// relations, its view's sources; a set's type.
pub fn looked_up_by(d: &Declaration) -> BTreeSet<TypeName> {
    let mut out = BTreeSet::new();
    match d {
        Declaration::Type(t) => {
            out.insert(t.name.clone());
            let derived = t.relations.iter().filter(|r| r.derived.is_some());
            out.extend(derived.map(|r| r.target.clone()));
            for v in t.view.iter() {
                out.insert(v.source.clone());
                out.extend(v.merge.iter().map(|m| m.source.clone()));
            }
        }
        Declaration::ObjectSet(o) => _ = out.insert(o.of.clone()),
        _ => {}
    }
    out
}

/// What a transaction put that its commit decides the exact-text index
/// again for: its declarations, in order, and the types whose
/// `lookup_index.<type>` setting it put.
#[derive(Default)]
pub struct Relook {
    declared: Vec<Declaration>,
    settings: BTreeSet<TypeName>,
}

impl Relook {
    /// A declaration put.
    pub fn declared(&mut self, d: &Declaration) {
        self.declared.push(d.clone());
    }

    /// A record put: a `lookup_index.<type>` setting moves its type.
    pub fn record(&mut self, r: &StoredRecord) {
        self.settings.extend(lookup_index_type(r));
    }

    /// Whether the commit decides anything again.
    pub fn any(&self) -> bool {
        !self.declared.is_empty() || !self.settings.is_empty()
    }

    /// The head's declarations once this transaction's are in, and the
    /// types it moved: from `kept`, the head as the last commit left it,
    /// or else from `read`, the head as this one leaves it, with every
    /// type moved (`None`).
    pub fn heads(
        &self,
        kept: Option<Heads>,
        read: impl FnOnce() -> Result<Vec<Declaration>>,
    ) -> Result<(Heads, Option<BTreeSet<TypeName>>)> {
        let Some(mut kept) = kept else {
            let decls = read()?.into_iter().map(|d| (declaration_key(&d), d));
            return Ok((decls.collect(), None));
        };
        let mut moved = self.settings.clone();
        for d in &self.declared {
            let old = kept.insert(declaration_key(d), d.clone());
            moved.extend(old.iter().chain([d]).flat_map(looked_up_by));
        }
        Ok((kept, Some(moved)))
    }
}

/// The declarations at the head, by [`declaration_key`], as a writing
/// handle keeps them from one commit to the next: the next decides the
/// exact-text index again without reading them (057).
pub type Heads = BTreeMap<String, Declaration>;

/// The fields of the exact-text index that come (`true`) and go, as
/// `(type, field)`: those [`looked_up`] decides for each type `heads`
/// declares that `moved` names (`None`, every one), under its
/// `lookup_index.<type>` setting's state at the head (`setting`), against
/// those the engine keeps of them, `was`. A type no put moved keeps its
/// fields (057).
pub fn relooked(
    heads: &Heads,
    moved: Option<&BTreeSet<TypeName>>,
    was: &BTreeSet<(String, String)>,
    mut setting: impl FnMut(&str) -> Result<Option<StoredRecord>>,
) -> Result<Vec<(String, String, bool)>> {
    let (mut now, decls) = (BTreeSet::new(), heads.values());
    for d in decls.clone() {
        let Declaration::Type(t) = d else { continue };
        if moved.is_some_and(|m| !m.contains(&t.name)) {
            continue;
        }
        let s = setting(&format!(
            "{}{}",
            lakelands_api::seam::LOOKUP_INDEX_SETTING,
            t.name
        ))?;
        let fields = looked_up(&t.name, decls.clone(), s.as_ref().and_then(setting_value));
        now.extend(
            fields
                .into_iter()
                .flatten()
                .map(|f| (t.name.to_string(), f.to_string())),
        );
    }
    let gone = was
        .difference(&now)
        .map(|(t, f)| (t.clone(), f.clone(), false));
    Ok(gone
        .chain(
            now.difference(was)
                .map(|(t, f)| (t.clone(), f.clone(), true)),
        )
        .collect())
}

/// The type whose exact-text index a setting's state asks for, if it is a
/// `lookup_index.<type>` setting.
pub fn lookup_index_type(r: &StoredRecord) -> Option<TypeName> {
    setting_for(r, lakelands_api::seam::LOOKUP_INDEX_SETTING)
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

/// The key of the setting that asks for `ty`'s order index: a kept name's
/// is its view's.
pub fn sort_index_key(ty: &TypeName) -> String {
    let ty = kept_view(ty).unwrap_or_else(|| ty.clone());
    format!("{}{ty}", lakelands_api::seam::SORT_INDEX_SETTING)
}

/// The meta row by which an engine keeps the fields of `ty`'s order index
/// the lake chose, as JSON (`Chosen`): in the projection and never in the
/// log.
pub fn chosen_mark(ty: &TypeName) -> String {
    format!("chosen.{ty}")
}

/// The meta row by which an engine marks the position every type's chosen
/// fields were last kept through. The fold moves it to the position it
/// folds through only from the one it folds from; a writer that keeps no
/// chosen field (a binary before them) moves its `sorted.<type>` marks
/// and leaves this behind, and no fold moves it again. A chosen field is
/// then read by no view, no step of choosing is taken, and a writer's open
/// drops the choice and the mark.
pub const CHOSEN_THROUGH: &str = "chosen_through";

/// The fields a type's order index keeps for the fold: those its setting
/// names, else those the lake chose, `building` ones too for the fold and
/// not for a read. A host's setting wins: where it names any field the
/// lake's choice is not read, and an engine drops it at the setting's
/// rebuild. A chosen field the declaration no longer holds with a kind the
/// index keeps is left out, and a view keeps none of its own.
pub fn order_fields(
    ty: &TypeName,
    decl: &TypeDecl,
    setting: Option<&Value>,
    chosen: &Chosen,
    building: bool,
) -> Vec<FieldName> {
    let named = sorted_fields(ty, decl, setting);
    if !named.is_empty() || decl.view.is_some() || kept_view(ty).is_some() {
        return named;
    }
    let held = |f: &&FieldName| {
        (decl.fields.iter()).any(|d| d.name == **f && lakelands_api::seam::sortable(&d.ty.kind))
    };
    let building = chosen.building.iter().filter(|_| building);
    chosen
        .kept
        .iter()
        .chain(building)
        .filter(held)
        .cloned()
        .collect()
}

/// A step of choosing as the chosen fields of a type take it: the fields
/// after it, and whether it asks the engine to fill, or to delete the rows
/// of, a field. A step that does nothing leaves them as they are.
pub fn chosen_after(chosen: &mut Chosen, step: &Choose) -> bool {
    let (kept, building) = (&mut chosen.kept, &mut chosen.building);
    match step {
        Choose::Start(f) if !kept.contains(f) && !building.contains(f) => {
            building.push(f.clone());
            true
        }
        Choose::Keep(f) if building.contains(f) => {
            building.retain(|b| b != f);
            kept.push(f.clone());
            true
        }
        Choose::Drop(f) if kept.contains(f) || building.contains(f) => {
            kept.retain(|k| k != f);
            building.retain(|b| b != f);
            true
        }
        Choose::Fill { field, .. } => building.contains(field),
        _ => false,
    }
}

/// The meta row by which an engine that keeps `ty`'s order index in
/// tables marks the position it was last kept through.
pub fn sorted_mark(ty: &TypeName) -> String {
    format!("sorted.{ty}")
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
/// lake does not index) is sorted whole by [`scan`].
pub fn by_order_index<'a>(fields: &[FieldName], scan: &'a Scan) -> Option<&'a Order> {
    let first = scan.order_by.first()?;
    (first.by == OrderBy::Value && fields.contains(&first.field)).then_some(first)
}

thread_local! {
    static ROWS_READ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static LINKS_READ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static BATCHES_READ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static HOLDER_READS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The batches of index rows [`scan_in_field_order`] asked for on this
/// thread. An engine may read more than a batch to answer one (a text's
/// few holders, read whole and sorted: [`holder_reads`]), so the engine
/// checks count by it that a page whose filter rejects rows asks for few
/// batches, not one per row.
pub fn batches_read() -> u64 {
    BATCHES_READ.with(|n| n.get())
}

/// The batches a page read text first ([`Holders::Few`]) reads each by a
/// statement of its own, cut at the batch, before its engine reads the
/// holders left once and keeps them in order. The store asks for one row
/// more than a page, so a page that passes over a retired or a restricted
/// record or two asks for a second and a third batch, and costs what a
/// small batch costs; a page that asks for a fourth is rejecting rows, and
/// its batches grow
/// (`docs/decisions/041-a-sorted-page-reads-its-page.md`, extended).
pub const BATCHES_BEFORE_HELD: u32 = 3;

/// The times an engine on this thread read a text's holders whole and
/// kept them in order for the batches a page read text first still asks
/// for: one for a page that asks for more than [`BATCHES_BEFORE_HELD`]
/// batches, however many, and none for a shorter one. The engine checks
/// count by it that a page whose filter rejects rows reads its holders
/// once, not once a batch.
pub fn holder_reads() -> u64 {
    HOLDER_READS.with(|n| n.get())
}

/// Counts one read of a text's holders for [`holder_reads`]; an engine
/// calls it where it reads them to keep.
pub fn count_holder_read() {
    HOLDER_READS.with(|n| n.set(n.get() + 1));
}

/// The rows the scans and counts on this thread were handed: the whole
/// type in the fallback, [`scan`], every batch of a key-ordered page
/// ([`scan_in_key_order`]) and of a count ([`count_in_key_order`]), the
/// index rows and missing records in [`scan_in_field_order`], and those
/// and the changed states in [`scan_in_field_order_at`]. It exists for
/// the engine checks, which count by it that a page reads its page, not
/// the type or every match; it costs one add per call or batch.
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

/// The records a page at the head in a field's order is read from, in the
/// order index's order (`docs/decisions/041-a-sorted-page-reads-its-page.md`,
/// extended): the type's, or those whose field holds exactly the scan's
/// [`text_lookup`]. When many hold it, the order index is read in its
/// order and a row kept where the text index holds its key; when at most
/// [`FEW_HOLDERS`] do, the text index's rows are sorted by their order
/// rows. Either way the rulebook admits each record.
#[derive(Clone, Copy)]
pub enum Holders<'a> {
    /// The type's records: the scan holds no exact text.
    All,
    /// More than [`FEW_HOLDERS`] hold the text in the field.
    Many(&'a FieldName, &'a str),
    /// At most [`FEW_HOLDERS`] hold it.
    Few(&'a FieldName, &'a str),
}

/// The most records holding a page's exact text that the page reads from
/// the text index and orders, rather than walk the order index: a batch
/// so read reads at most this many index rows, and a page that walks the
/// order index for more holders spread through it reads about
/// `limit × N / 4 096` of a type of N
/// (`docs/decisions/041-a-sorted-page-reads-its-page.md`, extended).
pub const FEW_HOLDERS: u64 = 4096;

impl<'a> Holders<'a> {
    /// The holders of `lookup`'s text, `count(field, text, n)` answering
    /// how many records hold it, counted up to `n`.
    pub fn of<E>(
        lookup: Option<&'a (FieldName, String)>,
        count: impl FnOnce(&FieldName, &str, u64) -> Result<u64, E>,
    ) -> Result<Holders<'a>, E> {
        let Some((field, text)) = lookup else {
            return Ok(Holders::All);
        };
        Ok(if count(field, text, FEW_HOLDERS + 1)? > FEW_HOLDERS {
            Holders::Many(field, text)
        } else {
            Holders::Few(field, text)
        })
    }

    /// The field and text the records hold, unless they are the type's.
    pub fn text(&self) -> Option<(&'a str, &'a str)> {
        match *self {
            Holders::All => None,
            Holders::Many(f, t) | Holders::Few(f, t) => Some((f.as_str(), t)),
        }
    }
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
    // whole further batch would hold twice the page. Each further one is
    // at least twice the one before, up to 1 024: a page whose filter
    // rejects rows asks for a handful of batches, not one per row.
    let rest = |have: usize, before: u32| {
        ((limit.saturating_sub(have) + 1).min(batch as usize) as u32)
            .max(before.saturating_mul(2).min(1024))
    };
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
        let (mut n, mut refill) = (batch, 0);
        loop {
            let rows = sorted(from.as_ref().map(|(k, r)| (k.as_slice(), r.as_str())), n)?;
            count(rows.len());
            BATCHES_READ.with(|c| c.set(c.get() + 1));
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
            refill = rest(out.len(), refill);
            n = refill;
        }
    }
    // The records without a value for the first field, last.
    let mut from: Option<String> = match first_after {
        Some((None, key)) if !multi => Some(key.to_owned()),
        _ => None,
    };
    let mut run = Vec::new();
    let (mut n, mut refill) = (batch, 0);
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
            refill = rest(out.len(), refill);
            n = refill;
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

/// The keys a field-carried relation reads in `value`: a string is one
/// key, an array its distinct string elements in key order, and anything
/// else, an element that is no string included, none.
pub fn keys_of(value: &Value) -> BTreeSet<&str> {
    match value {
        Value::String(key) => BTreeSet::from([key.as_str()]),
        Value::Array(items) => items.iter().filter_map(Value::as_str).collect(),
        _ => BTreeSet::new(),
    }
}

/// The links `record` declares over the relations of `decl` carried by a
/// field, over `relation` or all of them: one per key of the field.
pub fn links_of(
    decl: &TypeDecl,
    record: &StoredRecord,
    relation: Option<&RelationName>,
) -> Vec<Edge> {
    decl.relations
        .iter()
        .filter(|r| relation.is_none_or(|want| *want == r.name))
        .filter_map(|r| Some((r, record.fields.get(r.via.as_ref()?)?)))
        .flat_map(|(r, value)| {
            keys_of(value).into_iter().map(|to| Edge {
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
/// `ty` carried by a field, `records_of` answering a type's
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
    token_list(text, tag).collect()
}

/// The tokens of `text` under the language `tag`, in text order, repeats
/// kept: a field's length and a token's frequency in it count these.
fn token_list(text: &str, tag: &str) -> impl Iterator<Item = String> {
    let terms = crate::stem::stem_tokens(text, tag).map(|(_, _, term)| term);
    terms.filter(|t| !t.is_empty())
}

/// The distinct tokens a record's searchable text fields hold under its
/// type's declaration.
pub fn record_tokens(decl: &TypeDecl, record: &StoredRecord) -> BTreeSet<String> {
    field_tokens(decl, record, None)
        .flat_map(|(_, t)| t)
        .collect()
}

/// The distinct tokens each of a record's searchable text fields holds,
/// only those `fields` names when it names any: the text a search limited
/// to named fields matches in.
fn field_tokens<'r>(
    decl: &'r TypeDecl,
    record: &'r StoredRecord,
    fields: Option<&'r [FieldName]>,
) -> impl Iterator<Item = (&'r FieldName, BTreeSet<String>)> {
    let tag = decl.language.as_deref().unwrap_or("");
    searched_texts(decl, &record.fields, fields).map(move |(f, text)| (f, tokens(text, tag)))
}

/// The text each of a record's searchable fields holds in `values`, only
/// those `fields` names when it names any: the fields a search reads.
fn searched_texts<'d, 'v>(
    decl: &'d TypeDecl,
    values: &'v BTreeMap<FieldName, Value>,
    fields: Option<&'d [FieldName]>,
) -> impl Iterator<Item = (&'d FieldName, &'v str)> {
    decl.fields
        .iter()
        .filter(move |f| f.searchable && fields.is_none_or(|named| named.contains(&f.name)))
        .filter_map(move |f| Some((&f.name, values.get(&f.name)?.as_str()?)))
}

/// Where a search's `query` tokens stand in `record`, as its reader is
/// answered it: in each of the fields [`field_tokens`] walks for `fields`
/// that the record holds, every token the pipeline under `tag` makes a
/// query token of is a match, in text order, its first [`MATCH_SPANS`]
/// answered as spans and every one counted. A field none matched is not
/// named, so neither is one the record does not answer its reader.
pub fn matched(
    decl: &TypeDecl,
    record: &Record,
    fields: &[FieldName],
    query: &BTreeSet<String>,
    tag: &str,
) -> BTreeMap<FieldName, Matched> {
    let at = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    let mut out = BTreeMap::new();
    for (field, text) in searched_texts(decl, &record.fields, Some(fields)) {
        let (mut spans, mut count) = (Vec::new(), 0u32);
        let found = crate::stem::stem_tokens(text, tag).filter(|t| query.contains(&t.2));
        for (start, end, _) in found {
            count = count.saturating_add(1);
            if spans.len() < MATCH_SPANS {
                spans.push(Span {
                    start: at(start),
                    end: at(end),
                });
            }
        }
        if count > 0 {
            out.insert(field.clone(), Matched { spans, count });
        }
    }
    out
}

/// Each field of a record that a search of `fields` reads and that holds
/// a token, in its declaration's order: its length and how often it
/// holds each token of `query`, repeats counted. No score reads how often
/// a field holds a token the query does not ask for, so none is counted.
fn field_counts<'d>(
    decl: &'d TypeDecl,
    record: &StoredRecord,
    fields: Option<&'d [FieldName]>,
    query: &BTreeSet<String>,
) -> impl Iterator<Item = (&'d FieldName, u64, BTreeMap<String, u64>)> {
    let tag = decl.language.as_deref().unwrap_or("");
    searched_texts(decl, &record.fields, fields).filter_map(move |(f, text)| {
        let (mut len, mut tf) = (0, BTreeMap::new());
        for t in token_list(text, tag) {
            len += 1;
            if query.contains(&t) {
                *tf.entry(t).or_insert(0u64) += 1;
            }
        }
        (len > 0).then_some((f, len, tf))
    })
}

/// The tokens each searchable field of `record` holds, repeats counted,
/// for the fields that hold any: what an engine keeps of a head's text and
/// adds into its counts of the type's field.
pub fn text_lengths(decl: &TypeDecl, record: &StoredRecord) -> Vec<(FieldName, u64)> {
    let none = BTreeSet::new();
    let lengths = field_counts(decl, record, None, &none).map(|(f, len, _)| (f.clone(), len));
    lengths.collect()
}

/// Add into `out` the statistics a search of `text` in `fields` reads
/// over `heads` (`docs/seams.md` §`ranked`): for each searched field, the
/// heads whose field holds a token and the tokens they hold there; for
/// each query token, the heads that hold it in a searched field. `heads`
/// are live heads counted over, each as its reader is told it.
pub fn text_stats(
    decl: &TypeDecl,
    heads: impl IntoIterator<Item = impl std::borrow::Borrow<StoredRecord>>,
    (text, fields): (&str, Option<&[FieldName]>),
    out: &mut TextCounts,
) {
    let query = tokens(text, decl.language.as_deref().unwrap_or(""));
    for r in heads {
        let mut held = BTreeSet::new();
        for (f, len, tf) in field_counts(decl, r.borrow(), fields, &query) {
            let (n, l) = out.fields.entry(f.clone()).or_insert((0, 0));
            (*n, *l) = (*n + 1, *l + len);
            held.extend(query.iter().filter(|t| tf.contains_key(*t)));
        }
        for t in held {
            *out.df.entry(t.clone()).or_insert(0) += 1;
        }
    }
}

/// The text's ranking an engine answers for `asked` (`docs/seams.md`
/// §`ranked`), its candidates any superset of the hits among the live
/// heads, each once: [`lexical`] over the class `asked.text_stats` names.
/// `kept(held)` answers the counts the engine keeps of the type's fields,
/// over the heads that are not held and, with `held`, over every head; a
/// reader with no class is ranked by what the store counted for it.
pub fn ranked_text<E>(
    decl: &TypeDecl,
    candidates: impl IntoIterator<Item = impl std::borrow::Borrow<StoredRecord>>,
    asked: &Rankings,
    k: u32,
    kept: impl FnOnce(bool) -> std::result::Result<BTreeMap<FieldName, (u64, u64)>, E>,
) -> std::result::Result<Vec<(RecordKey, f64)>, E> {
    let Some((text, fields)) = &asked.text else {
        return Ok(Vec::new());
    };
    let (fields, weights) = (fields.as_deref(), &asked.text_weights);
    let candidates = candidates.into_iter();
    Ok(match &asked.text_stats {
        Some(TextStats::Counted(counted)) => {
            let counts = Counts::Counted(counted);
            lexical(decl, candidates, text, fields, weights, counts, k)
        }
        Some(TextStats::Unheld) => {
            let class = candidates.filter(|r| !r.borrow().restricted);
            let counts = Counts::Kept(&kept(false)?);
            lexical(decl, class, text, fields, weights, counts, k)
        }
        None => {
            let counts = Counts::Kept(&kept(true)?);
            lexical(decl, candidates, text, fields, weights, counts, k)
        }
    })
}

/// The statistics [`lexical`] scores by.
pub enum Counts<'a> {
    /// The counts of each field kept over the heads of the reader's class;
    /// the candidates handed in are of that class, and each query token's
    /// heads are counted over them.
    Kept(&'a BTreeMap<FieldName, (u64, u64)>),
    /// Every count, as the store counted it for a reader with no class.
    Counted(&'a TextCounts),
}

/// BM25F's saturation (`docs/seams.md` §`ranked`).
const K1: f64 = 1.2;
/// BM25F's length normalisation (`docs/seams.md` §`ranked`).
const B: f64 = 0.75;

/// Full-text hits, as `docs/seams.md` §`ranked` defines them: of the
/// `candidates`, records of one type as they stand at the head, `decl`
/// the type's declaration there, the hits are those live whose searchable
/// text holds a query token — the text of the searchable fields `fields`
/// names, when it names any. A hit scores by BM25F in its combined-field
/// form over `counts`, each field weighing what `weights` gives it and 1
/// for one it does not name; best first, ties by key, at most `k`. The
/// sums run in one order, fields in their declaration's and tokens in
/// theirs, from 0, and `ln` is `libm`'s, so a score's bits are the same
/// on every engine and target.
///
/// An engine may hand in any superset of the hits: a record whose named
/// fields hold no query token scores nothing and is dropped here, before
/// the ranking is cut at `k`.
pub fn lexical(
    decl: &TypeDecl,
    candidates: impl IntoIterator<Item = impl std::borrow::Borrow<StoredRecord>>,
    text: &str,
    fields: Option<&[FieldName]>,
    weights: &BTreeMap<FieldName, f64>,
    counts: Counts<'_>,
    k: u32,
) -> Vec<(RecordKey, f64)> {
    let query = tokens(text, decl.language.as_deref().unwrap_or(""));
    let held: Vec<_> = candidates
        .into_iter()
        .filter(|r| !r.borrow().retired)
        .filter_map(|r| {
            let r = r.borrow();
            let held: Vec<_> = field_counts(decl, r, fields, &query).collect();
            let holds = |t: &String| held.iter().any(|(_, _, tf)| tf.contains_key(t));
            query.iter().any(holds).then(|| (r.key.clone(), held))
        })
        .collect();
    let (lengths, df) = match counts {
        Counts::Counted(c) => (&c.fields, c.df.clone()),
        Counts::Kept(kept) => {
            let mut df = BTreeMap::new();
            for t in &query {
                let holds = |h: &&(_, Vec<(_, _, BTreeMap<String, u64>)>)| {
                    h.1.iter().any(|(_, _, tf)| tf.contains_key(t))
                };
                df.insert(t.clone(), held.iter().filter(holds).count() as u64);
            }
            (kept, df)
        }
    };
    let searched = decl
        .fields
        .iter()
        .filter(|f| f.searchable && fields.is_none_or(|named| named.contains(&f.name)));
    let n = searched
        .filter_map(|f| lengths.get(&f.name))
        .map(|c| c.0)
        .sum::<u64>() as f64;
    let mut hits: Vec<(RecordKey, f64)> = held
        .into_iter()
        .map(|(key, held)| {
            let mut score = 0.0;
            for t in &query {
                let (mut x, mut any) = (0.0, false);
                for (f, len, tf) in &held {
                    let Some(tf) = tf.get(t) else { continue };
                    any = true;
                    let len = *len as f64;
                    let avg = match lengths.get(*f) {
                        Some(&(n_f, l_f)) if n_f > 0 && l_f > 0 => l_f as f64 / n_f as f64,
                        _ => len,
                    };
                    let w = weights.get(*f).copied().unwrap_or(1.0);
                    x += w * *tf as f64 / (1.0 - B + B * len / avg);
                }
                if any {
                    let df = df.get(t).copied().unwrap_or(0) as f64;
                    let idf = libm::log((2.0 * n + 2.0) / (2.0 * df + 1.0));
                    score += idf * x * (K1 + 1.0) / (x + K1);
                }
            }
            (key, score)
        })
        .collect();
    rank(&mut hits, k);
    hits
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
) -> Option<Case> {
    let (fence, position) = (event.fence, event.position);
    let wanted = if position.0 == 0 { Some(0) } else { lease };
    match wanted {
        Some(wanted) => (fence != wanted).then_some(Case::FenceWrong {
            position,
            fence,
            wanted,
        }),
        None => (fence < floor).then_some(Case::FenceFalls {
            position,
            fence,
            floor,
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
            return Err(Error::invalid(why));
        }
        floor = event.fence;
        let position = event.position;
        if position.0 != expected {
            let next = expected;
            return Err(Error::invalid(Case::AppendNotNext { position, next }));
        }
        if event.prev != prev {
            let (named, hash) = (event.prev.0.clone(), prev.0.clone());
            return Err(Error::invalid(Case::AppendPrev {
                position,
                named,
                hash,
            }));
        }
        if event.bytes.contains(&b'\n') {
            return Err(Error::invalid(Case::AppendLineFeed { position }));
        }
        prev = event.hash.clone();
    }
    Ok(())
}

/// An event's hash as the log format defines it: SHA-256 of its
/// canonical bytes without `hash` (`docs/log-format.md` §1.2).
pub fn hash_of(mut event: serde_json::Value) -> String {
    if let Some(members) = event.as_object_mut() {
        members.remove("hash");
    }
    crate::canonical::sha256_hex(&event)
}

/// The events that begin the next segment as a seal's `first` answered
/// them for `sealed`: at least one, or the seal is refused and nothing is
/// sealed.
pub fn first_of_next(
    first: &mut dyn FnMut(&SegmentDigest) -> Result<Vec<CanonicalEvent>>,
    sealed: &SegmentDigest,
) -> Result<Vec<CanonicalEvent>> {
    let events = first(sealed)?;
    if events.is_empty() {
        let after = sealed.last;
        return Err(Error::invalid(Case::SealNoFirst { after }));
    }
    Ok(events)
}

/// The refusal of the writer lease to a taker while `holder` holds it
/// under `fence`.
pub fn lease_held(holder: &str, fence: u64) -> Error {
    let holder = holder.to_owned();
    Error::unavailable(lakelands_api::error::Situation::LeaseHeld { holder, fence })
}

/// Refuses a projection of `lake` laid out by another build: `found`,
/// where the engine's layout is `wanted`. Such a projection is never
/// read, since a table it lacks or holds otherwise would answer wrongly
/// without failing; it is rebuilt from the log.
pub fn layout(lake: &LakeId, found: Option<u64>, wanted: u64) -> Result<()> {
    if found == Some(wanted) {
        return Ok(());
    }
    let lake = lake.to_string();
    let situation = lakelands_api::error::Situation::ProjectionLayout {
        lake,
        found,
        wanted,
    };
    Err(Error::unavailable(situation))
}

/// The number a projection stores for each event kind of the format, in
/// the order the format lists them. A code is never reused or moved: a
/// kind a later format adds takes the next number.
pub const KIND_CODES: [(&str, i64); 24] = [
    ("lake_created", 1),
    ("lake_upgraded", 2),
    ("lease_taken", 3),
    ("declared", 4),
    ("migrated", 5),
    ("value_type_migrated", 6),
    ("record_upserted", 7),
    ("record_ingested", 8),
    ("source_forgotten", 9),
    ("values_removed", 10),
    ("record_retired", 11),
    ("record_restricted", 12),
    ("record_unrestricted", 13),
    ("verb_called", 14),
    ("write_proposed", 15),
    ("write_accepted", 16),
    ("write_rejected", 17),
    ("trigger_fired", 18),
    ("trigger_dropped", 19),
    ("object_set_changed", 20),
    ("output_recorded", 21),
    ("checkpoint", 22),
    ("record_quarantined", 23),
    ("quarantine_closed", 24),
];

/// The code of the event kind `kind`, or none for one the format does not
/// list: a filter may name a kind a newer format adds, which no event of
/// this one is of.
pub fn kind_code(kind: &str) -> Option<i64> {
    KIND_CODES.iter().find(|(k, _)| *k == kind).map(|(_, c)| *c)
}

/// The refusal of a query vector of `query` dimensions where `ty.field`
/// holds vectors of `held`. It names no record: the store refuses it
/// by the declaration before any engine reads a vector, and an engine's
/// guard that meets a stored vector of another length says the same.
pub fn dimensions(query: usize, held: usize, ty: &TypeName, field: &str) -> Error {
    let case = Case::QueryDimensions { query, held };
    let (name, field) = (ty.clone(), field.into());
    let at = Said::from(case).within(At::Field { name: field });
    Error::invalid(at.within(At::Type { name }))
}

/// The refusal of a query vector the caller says `query` made, where
/// `ty.field` is the embedding of `held`. Like [`dimensions`], it is
/// judged by the declaration, after the reader is shown the field.
pub fn model(query: &str, held: &str, ty: &TypeName, field: &str) -> Error {
    let (query, held) = (query.to_owned(), held.to_owned());
    let case = Case::QueryModel { query, held };
    let (name, field) = (ty.clone(), field.into());
    let at = Said::from(case).within(At::Field { name: field });
    Error::invalid(at.within(At::Type { name }))
}

/// Best first by score, ties by key, at most `k`.
pub fn rank(hits: &mut Vec<(RecordKey, f64)>, k: u32) {
    hits.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    hits.truncate(k as usize);
}

/// The best `k` of a field's vectors whose head record the reader's
/// selection `seen` admits, in [`rank`]'s order, `decl` the type in force.
/// `top(m)` answers the field's best `m` ([`TopK`]); the candidates are
/// judged best first, and the depth grows fourfold from `k` until `k`
/// pass or the field ends, so what a search holds is the depth it reaches
/// and a selection that admits most candidates reads few records more
/// than it keeps. `read` answers the head records of a batch's keys, in
/// any order. Where `seen` is `None`, or admits every live record, no
/// record is read.
pub fn rank_seen(
    k: u32,
    seen: Option<(&Scan, Option<&TypeDecl>)>,
    mut top: impl FnMut(u32) -> Result<Vec<(RecordKey, f64)>>,
    mut read: impl FnMut(Vec<RecordKey>) -> Result<Vec<StoredRecord>>,
) -> Result<Vec<(RecordKey, f64)>> {
    let Some((seen, decl)) = seen.filter(|(s, _)| !admits_every_live(s)) else {
        return top(k);
    };
    let (mut out, mut judged, mut depth) = (Vec::new(), 0, k.max(16));
    loop {
        let best = top(depth)?;
        let fresh = &best[judged.min(best.len())..];
        let admitted: BTreeSet<RecordKey> =
            read(fresh.iter().map(|(key, _)| key.clone()).collect())?
                .into_iter()
                .filter(|r| admits(decl, r, seen))
                .map(|r| r.key)
                .collect();
        out.extend(
            fresh
                .iter()
                .filter(|(key, _)| admitted.contains(key))
                .cloned(),
        );
        judged = best.len();
        if out.len() >= k as usize || best.len() < depth as usize {
            break;
        }
        depth = depth.saturating_mul(4);
    }
    out.truncate(k as usize);
    Ok(out)
}

/// Whether `seen` admits every live record: an engine need not read a
/// candidate's state to judge it.
pub fn admits_every_live(seen: &Scan) -> bool {
    seen.restricted
        && seen.filter.is_none()
        && seen.valid_at.is_none()
        && seen.equal.is_empty()
        && seen.equal_any.is_empty()
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

/// The types a declaration names for the proposal listing: a type's own
/// name, and the type a verb, a trigger or a set acts on, with the type the
/// one in force under its name stands on (`stands`, by its
/// [`declaration_key`]), which the declaration would take away from it
/// (lakelands#916).
pub fn declared_on(d: &Declaration, stands: &impl Fn(&str) -> Option<TypeName>) -> Vec<TypeName> {
    let on = match d {
        Declaration::Type(t) => return vec![t.name.clone()],
        Declaration::Verb(v) => v.on.clone(),
        Declaration::Trigger(t) => t.on.clone(),
        Declaration::ObjectSet(s) => s.of.clone(),
        // A withdrawal names the type of what it withdraws, or the type.
        Declaration::Withdraw(w) if w.kind == DeclarationKind::Type => {
            return vec![w.name.as_str().into()];
        }
        Declaration::Withdraw(_) => return stands(&declaration_key(d)).into_iter().collect(),
        _ => return Vec::new(),
    };
    let mut out = vec![on];
    out.extend(stands(&declaration_key(d)).filter(|was| !out.contains(was)));
    out
}

/// The types a proposed write touches: a record write's type, a change
/// to records' type, the type a
/// called verb acts on, every type a declaration names ([`declared_on`]),
/// and a migration's type. `stands` answers the type a declaration in
/// force stands on, by its [`declaration_key`].
fn touched(write: &ProposedWrite, stands: &impl Fn(&str) -> Option<TypeName>) -> Vec<TypeName> {
    match write {
        ProposedWrite::Upsert { ty, .. }
        | ProposedWrite::Retire { ty, .. }
        | ProposedWrite::Records { ty, .. } => vec![ty.clone()],
        ProposedWrite::Call { verb, .. } => stands(&format!("verb {verb}")).into_iter().collect(),
        ProposedWrite::Declare { declarations, .. } => (declarations.iter())
            .flat_map(|d| declared_on(d, stands))
            .collect(),
        ProposedWrite::Migrate { migration, .. } => vec![migration.to.name.clone()],
        // A value type's tightening names no type, as its declaration names none.
        ProposedWrite::MigrateValueType { .. } => Vec::new(),
        ProposedWrite::Compute { ty, .. } | ProposedWrite::ComputeSweep { ty, .. } => {
            vec![ty.clone()]
        }
    }
}

/// What a page of proposals asks a SQL engine (`proposals_before`): the
/// state's name as stored, the position its proposals come before, and
/// how many it holds, each the widest where it is not asked.
pub fn proposals_asked(
    filter: &lakelands_api::write::ProposalFilter,
    before: Option<Position>,
) -> (Option<String>, i64, i64) {
    let state = filter
        .state
        .and_then(|s| serde_json::to_value(s).ok()?.as_str().map(Into::into));
    let limit = filter.limit.map_or(i64::MAX, i64::from);
    (state, before.map_or(i64::MAX, |b| b.0 as i64), limit)
}

/// Whether a proposal is of `filter`'s kind and touches its type, by the
/// write it holds: one that holds none is of no kind and touches no type.
pub fn matches(
    p: &Proposal,
    filter: &ProposalFilter,
    stands: impl Fn(&str) -> Option<TypeName>,
) -> bool {
    let write = p.write.as_ref();
    filter
        .kind
        .is_none_or(|k| write.is_some_and(|w| w.kind() == k))
        && (filter.ty.as_ref())
            .is_none_or(|ty| write.is_some_and(|w| touched(w, &stands).contains(ty)))
}

/// Proposals as a view answers them: those in `filter`'s state, of its
/// kind and touching its type (`stands` says which type a verb, a trigger
/// or a set in force stands on, by its [`declaration_key`], as declared at
/// the view's position), newest first, ties by id, at most `filter.limit`.
pub fn proposals<'a>(
    all: impl IntoIterator<Item = &'a Proposal>,
    filter: &ProposalFilter,
    stands: impl Fn(&str) -> Option<TypeName>,
) -> Vec<Proposal> {
    let mut found: Vec<&Proposal> = all
        .into_iter()
        .filter(|p| filter.state.is_none_or(|s| p.state == s))
        .filter(|p| matches(p, filter, &stands))
        .collect();
    found.sort_by(|a, b| b.position.cmp(&a.position).then_with(|| a.id.cmp(&b.id)));
    found
        .into_iter()
        .take(filter.limit.map_or(usize::MAX, |l| l as usize))
        .cloned()
        .collect()
}

/// A state an engine's projection stored as JSON, read back; `what`
/// names it (`a record`) in the fault of one that cannot be read.
pub fn decode<T: serde::de::DeserializeOwned>(state: &str, what: &str) -> Result<T> {
    serde_json::from_str(state).map_err(|e| unreadable(what, &e))
}

fn unreadable(what: &str, e: &serde_json::Error) -> Error {
    let (what, detail) = (what.to_owned(), Detail::json(e));
    crate::damaged(Part::Projection, None, Flaw::Unreadable { what, detail })
}

/// A record as an engine keeps it: its provenance once, where its fields
/// share it, and per field only where a field's differs (lakelands#1018).
/// Every field a source or an author wrote in one event carries the same
/// block, so a record keeps it once, not once a field; and every record
/// of one append carries the same block but its position, so where
/// [`SHARED`] records of one write name a block the projection keeps it
/// once among its bases, and the state names it by the id it is kept
/// under; else the state keeps it inline. What the row holds beside the
/// state is not kept in it: its type, its key and its position, the
/// row's `since`; nor is a flag that is false, nor `base`'s position
/// where it is the row's (lakelands#1318); nor a vector's elements, which
/// a row of their own holds ([`Elements`]), the state naming its position.
#[derive(serde::Serialize, serde::Deserialize)]
struct State {
    /// Each field kept apart, with the position of the row holding its
    /// elements: first, so a state naming none starts otherwise.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    vectors: BTreeMap<FieldName, Position>,
    version: lakelands_api::id::Version,
    fields: BTreeMap<FieldName, Value>,
    /// The provenance most of the fields share: inline, its position left
    /// out where it is the row's, or the id of the base it is kept as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base: Option<Block>,
    /// A kept base's position, where it is not the row's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base_at: Option<Position>,
    /// Each field's whose differs from `base`, or that holds no value.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    meta: BTreeMap<FieldName, FieldMeta>,
    /// The fields holding a value with no provenance, beside a `base`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    bare: Vec<FieldName>,
    made: Position,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    retired: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    restricted: bool,
}

/// A state's base: the id it is kept under, or the block inline.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
enum Block {
    Kept(i64),
    Inline(serde_json::Map<String, Value>),
}

/// The fewest records of one write naming a base for the projection to
/// keep it once, as a row their states name by its id; a base fewer name
/// stays inline in each state. A write is what the engine puts in one
/// transaction, so a rebuild, which folds many appends in one, may keep
/// rows a fresh projection kept inline; both read the same. A row costs
/// more than the block it saves until enough states share it: on SQLite,
/// records written two an append took 917.5 bytes each with their base a
/// row and 903.2 with it inline, three an append 876.5 and 903.2
/// (lakelands#1432; PostgreSQL was not measured).
pub const SHARED: usize = 3;

/// The provenance most of `r`'s fields with a value share; ties to the
/// first field's.
fn shared(r: &StoredRecord) -> Option<&FieldMeta> {
    let mut shared: Vec<(&FieldMeta, usize)> = Vec::new();
    for m in r.fields.keys().filter_map(|f| r.meta.get(f)) {
        match shared.iter_mut().find(|(b, _)| *b == m) {
            Some((_, n)) => *n += 1,
            None => shared.push((m, 1)),
        }
    }
    (shared.iter().rev().max_by_key(|(_, n)| *n)).map(|(b, _)| *b)
}

/// A provenance block as JSON, its position left out where it is `at`.
fn block(m: &FieldMeta, at: Option<Position>) -> serde_json::Map<String, Value> {
    let Ok(Value::Object(mut block)) = serde_json::to_value(m) else {
        unreachable!("a field's provenance serialises as an object")
    };
    if at.is_none_or(|at| at == m.position) {
        block.remove("position");
    }
    block
}

/// The base of `r`: the block most of its fields with a value share,
/// without its position, as the projection keeps it, if any has one.
pub fn base_of(r: &StoredRecord) -> Option<String> {
    let block = block(shared(r)?, None);
    Some(serde_json::to_string(&block).expect("a block serialises"))
}

/// The bases [`SHARED`] or more of `records` of one write name: those
/// the projection keeps as rows. The records of an append each have a
/// position of their own and share their base whole, its time included.
pub fn shared_bases<'a>(records: impl IntoIterator<Item = &'a StoredRecord>) -> HashSet<String> {
    let mut named: HashMap<String, HashSet<(&TypeName, &RecordKey)>> = HashMap::new();
    for r in records {
        if let Some(base) = base_of(r) {
            named.entry(base).or_default().insert((&r.ty, &r.key));
        }
    }
    (named.into_iter())
        .filter(|(_, names)| names.len() >= SHARED)
        .map(|(base, _)| base)
        .collect()
}

/// The bases of the write an engine puts record by record, which knows a
/// base is shared only at the [`SHARED`]th record of the write naming it:
/// the states put before it keep it inline until then.
#[derive(Debug, Default)]
pub struct Sharing {
    named: HashMap<String, Named>,
}

#[derive(Debug)]
enum Named {
    Inline(Vec<(TypeName, RecordKey, Position)>),
    Kept(i64),
}

impl Sharing {
    /// The id the state of `r` names `base` by, `None` to keep it inline:
    /// at the [`SHARED`]th record of the write naming it, `keep` keeps it
    /// and answers its id, and `name` has the states the write put before
    /// it, by type, key and position, name that id ([`name_base`]). A
    /// write is what one `Sharing` sees.
    pub fn id(
        &mut self,
        r: &StoredRecord,
        base: &str,
        keep: &mut dyn FnMut(&str) -> Result<i64>,
        name: &mut dyn FnMut(&TypeName, &RecordKey, Position, i64) -> Result<()>,
    ) -> Result<Option<i64>> {
        let named = (self.named.entry(base.to_owned())).or_insert(Named::Inline(Vec::new()));
        let before = match named {
            Named::Kept(id) => return Ok(Some(*id)),
            Named::Inline(before) => before,
        };
        let this = |(ty, key, _): &&(TypeName, RecordKey, Position)| (ty, key) == (&r.ty, &r.key);
        before.retain(|held| !this(&held));
        before.push((r.ty.clone(), r.key.clone(), r.position));
        if before.len() < SHARED {
            return Ok(None);
        }
        let id = keep(base)?;
        for (ty, key, at) in before.iter().filter(|held| !this(held)) {
            name(ty, key, *at, id)?;
        }
        *named = Named::Kept(id);
        Ok(Some(id))
    }
}

/// The state an engine keeps for a record: [`State`]'s form, kept in a
/// row that holds its type, its key and its position as `since`, the
/// fields `apart` names left to their rows ([`put_vectors`]), and its
/// base inline, or by the id `base` answers for it where the engine keeps
/// it once.
pub fn record_state(
    r: &StoredRecord,
    apart: &BTreeMap<FieldName, Position>,
    base: &mut dyn FnMut(&str) -> Result<Option<i64>>,
) -> Result<String> {
    let shared = shared(r);
    let meta = (r.meta.iter())
        .filter(|(f, m)| !r.fields.contains_key(*f) || shared != Some(*m))
        .map(|(f, m)| (f.clone(), m.clone()));
    let bare = r
        .fields
        .keys()
        .filter(|f| shared.is_some() && !r.meta.contains_key(*f));
    let fields = r.fields.iter().filter(|(f, _)| !apart.contains_key(*f));
    let (kept, at) = match (shared, base_of(r)) {
        (Some(m), Some(b)) => match base(&b)? {
            Some(id) => (Some(Block::Kept(id)), Some(m.position)),
            None => (Some(Block::Inline(block(m, Some(r.position)))), None),
        },
        _ => (None, None),
    };
    let state = State {
        vectors: apart.clone(),
        version: r.version,
        fields: fields.map(|(f, v)| (f.clone(), v.clone())).collect(),
        meta: meta.collect(),
        bare: bare.cloned().collect(),
        base: kept,
        base_at: at.filter(|at| *at != r.position),
        made: r.made,
        retired: r.retired,
        restricted: r.restricted,
    };
    Ok(serde_json::to_string(&state).expect("a record serialises"))
}

/// `state` naming the base kept as `id` where it holds `base` inline;
/// `None` where it holds another.
pub fn name_base(state: &str, base: &str, id: i64) -> Result<Option<String>> {
    let mut s: State = decode(state, "a record")?;
    let Some(Block::Inline(mut inline)) = s.base.take() else {
        return Ok(None);
    };
    let at = inline.remove("position");
    if serde_json::to_string(&inline).expect("a block serialises") != base {
        return Ok(None);
    }
    s.base = Some(Block::Kept(id));
    s.base_at = at
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| unreadable("a record", &e))?;
    Ok(Some(
        serde_json::to_string(&s).expect("a record serialises"),
    ))
}

/// What a state keeps apart, which the caller reads from their rows: each
/// field kept apart with the position of the row holding its elements
/// ([`rejoin`]), and the base it names by its id ([`rebase`]).
#[derive(Debug, Default)]
pub struct Apart {
    pub vectors: Vec<(FieldName, Position)>,
    pub base: Option<Based>,
}

/// The base a state names: its id, and what the record takes of it.
#[derive(Debug)]
pub struct Based {
    pub id: i64,
    at: Position,
    fields: Vec<FieldName>,
}

/// A record from the state [`record_state`] kept in the row of `ty`'s
/// `key` put at `since`, and what it keeps apart, which the caller reads
/// from their rows: each field with a value and no provenance of its own
/// takes the base, but a bare one, and the base takes `since` where the
/// state holds no position for it.
pub fn decode_record(
    ty: &str,
    key: &str,
    since: Position,
    state: &str,
) -> Result<(StoredRecord, Apart)> {
    let s: State = decode(state, "a record")?;
    let valued = s.fields.keys().chain(s.vectors.keys());
    let takes: Vec<FieldName> = (valued
        .filter(|f| !s.bare.contains(f) && !s.meta.contains_key(*f)))
    .cloned()
    .collect();
    let mut meta = s.meta;
    let base = match s.base {
        Some(Block::Kept(id)) => Some(Based {
            id,
            at: s.base_at.unwrap_or(since),
            fields: takes,
        }),
        Some(Block::Inline(mut base)) => {
            let at = || Value::from(since.0);
            base.entry("position").or_insert_with(at);
            let base: FieldMeta = serde_json::from_value(Value::Object(base))
                .map_err(|e| unreadable("a record", &e))?;
            meta.extend(takes.into_iter().map(|f| (f, base.clone())));
            None
        }
        None => None,
    };
    let r = StoredRecord {
        ty: TypeName::from(ty),
        key: RecordKey::from(key),
        version: s.version,
        fields: s.fields,
        meta,
        position: since,
        made: s.made,
        retired: s.retired,
        restricted: s.restricted,
    };
    let vectors = s.vectors.into_iter().collect();
    Ok((r, Apart { vectors, base }))
}

/// Give the fields of `r` that take its base the block its row holds,
/// `block`: a state naming a base the projection does not hold is one it
/// cannot read.
pub fn rebase(r: &mut StoredRecord, based: Based, block: Option<&str>) -> Result<()> {
    let block = block.ok_or_else(|| base_missing("its row is missing"))?;
    let mut block: serde_json::Map<String, Value> = decode(block, "a record's base")?;
    block.insert("position".to_owned(), Value::from(based.at.0));
    let meta: FieldMeta = serde_json::from_value(Value::Object(block))
        .map_err(|e| unreadable("a record's base", &e))?;
    for f in based.fields {
        r.meta.insert(f, meta.clone());
    }
    Ok(())
}

/// The fault of a base whose row the projection lacks, `why`.
pub fn base_missing(why: &str) -> Error {
    let missing = <serde_json::Error as serde::de::Error>::custom(why);
    unreadable("a record's base", &missing)
}

/// A vector's elements as the projection keeps them apart from its
/// record's state (`docs/decisions/072-a-vector-is-stored-once.md`): as
/// little-endian `f32`s where every element is exactly one (`single`),
/// else as doubles.
#[derive(Clone, Debug, PartialEq)]
pub struct Elements {
    pub single: bool,
    pub data: Vec<u8>,
}

impl Elements {
    /// Elements as a row holds them.
    pub fn new(single: bool, data: Vec<u8>) -> Elements {
        Elements { single, data }
    }

    /// The elements of `v` where it is a vector they give back exactly:
    /// an array of numbers, each the one its double reads as in the log's
    /// number form ([`number`]). Anything else stays in the state.
    pub fn of(v: &Value) -> Option<Elements> {
        let a = v.as_array().filter(|a| !a.is_empty())?;
        let xs: Vec<f64> = (a.iter())
            .map(|e| e.as_f64().filter(|x| number(*x) == *e))
            .collect::<Option<_>>()?;
        let single = xs.iter().all(|x| f64::from(*x as f32) == *x);
        let data = match single {
            true => xs.iter().flat_map(|x| (*x as f32).to_le_bytes()).collect(),
            false => xs.iter().flat_map(|x| x.to_le_bytes()).collect(),
        };
        Some(Elements { single, data })
    }

    fn doubles(&self) -> Vec<f64> {
        match self.single {
            true => vector_of(&self.data).into_iter().map(f64::from).collect(),
            false => (self.data.as_chunks::<8>().0.iter())
                .map(|c| f64::from_le_bytes(*c))
                .collect(),
        }
    }

    /// The vector they hold, as the record held it.
    pub fn value(&self) -> Value {
        Value::Array(self.doubles().into_iter().map(number).collect())
    }

    /// The vector as a ranking reads it: each element as an `f32`.
    pub fn ranked(&self) -> Vec<f32> {
        match self.single {
            true => vector_of(&self.data),
            false => self.doubles().into_iter().map(|x| x as f32).collect(),
        }
    }
}

/// Put back into `r` the vector of `field` its row holds, `row`: a state
/// naming a row the projection does not hold is one it cannot read.
pub fn rejoin(r: &mut StoredRecord, field: FieldName, row: Option<Elements>) -> Result<()> {
    let missing = || <serde_json::Error as serde::de::Error>::custom("its row is missing");
    let e = row.ok_or_else(|| unreadable("a record's vector", &missing()))?;
    r.fields.insert(field, e.value());
    Ok(())
}

/// A double as the log's number form reads it: an integral one within
/// ±(2^53 − 1) an integer, `-0` among them as `0`; any other the double.
fn number(x: f64) -> Value {
    const SAFE: f64 = 9_007_199_254_740_991.0;
    match x.fract() == 0.0 && x.abs() <= SAFE {
        true => Value::from(x as i64),
        false => serde_json::Number::from_f64(x).map_or(Value::Null, Value::Number),
    }
}

/// A row of a record's vector in force at the head: its field, the
/// position it was written at, and its elements.
pub type VectorRow = (FieldName, Position, Elements);

/// What putting a record's state does to its vector rows ([`put_vectors`]).
#[derive(Debug, Default)]
pub struct VectorPut {
    /// The fields kept apart, each with the position of the row the state
    /// names.
    pub apart: BTreeMap<FieldName, Position>,
    /// The rows written at the state's position, each ending there too
    /// where `ends`.
    pub written: Vec<(FieldName, Elements)>,
    pub ends: bool,
    /// The rows written before that end at the state's position.
    pub ended: Vec<(FieldName, Position)>,
    /// The rows written at the state's own position, by a state it
    /// replaces, that it does not name: they go.
    pub gone: Vec<FieldName>,
    /// The record's rows in force after it.
    pub held: Vec<VectorRow>,
}

/// Whether `ty`'s states keep their vectors in place: the store's own
/// names, `sys.` and `sys:`, which some reads take without a join, but a
/// link type's, whose vector fields a caller declares.
pub fn vectors_in_place(ty: &TypeName) -> bool {
    let own = ["sys.", "sys:"].iter().any(|p| ty.as_str().starts_with(p));
    own && !ty.as_str().starts_with(lakelands_api::key::LINK_PREFIX)
}

/// The fields a state of `ty` keeps apart ([`put_vectors`]): those declared
/// as vectors by the declaration its records answer to, `decl_of` reading
/// a type's at the head; a kept name's is its view's, a link type's its
/// relation's fields. A value of any other field stays in the state,
/// whatever its shape, and so does every vector of the store's own names.
pub fn vector_fields(
    ty: &TypeName,
    mut decl_of: impl FnMut(&TypeName) -> Result<Option<TypeDecl>>,
) -> Result<BTreeSet<FieldName>> {
    let vectors = |fields: &[FieldDecl]| -> BTreeSet<FieldName> {
        (fields.iter())
            .filter(|f| f.ty.kind == StorageKind::Vector)
            .map(|f| f.name.clone())
            .collect()
    };
    if vectors_in_place(ty) {
        return Ok(BTreeSet::new());
    }
    Ok(match crate::links::link_type_parts(ty) {
        Some((t, name)) => (decl_of(&t)?.iter())
            .flat_map(|d| d.relations.iter().filter(|r| r.name == name))
            .flat_map(|r| vectors(&r.fields))
            .collect(),
        None => (decl_of(&kept_view(ty).unwrap_or_else(|| ty.clone()))?)
            .map_or_else(BTreeSet::new, |d| vectors(&d.fields)),
    })
}

/// What putting `r`'s state does to the vector rows of its record, `held`
/// those in force before it (`docs/decisions/072-a-vector-is-stored-once.md`):
/// each vector a row in force holds bit for bit, the state names; any
/// other is written at the state's position, and so is every one of a
/// state after a retirement. A row the state does not name ends at its
/// position, and so does every row a retired state names, so a row in
/// force is one of the head. A state put where a `later` one is kept
/// already keeps its vectors in place and changes no row: the row at its
/// position may be one a later state names with other elements. Only the
/// fields of `vectors` ([`vector_fields`]) are kept apart.
pub fn put_vectors(
    r: &StoredRecord,
    held: Vec<VectorRow>,
    later: bool,
    vectors: &BTreeSet<FieldName>,
) -> VectorPut {
    if later {
        return VectorPut {
            held,
            ..VectorPut::default()
        };
    }
    let at = r.position;
    let mut put = VectorPut {
        ends: r.retired,
        ..VectorPut::default()
    };
    let mut was: BTreeMap<_, _> = (held.into_iter())
        .map(|(f, since, e)| (f, (since, e)))
        .collect();
    let kept = (r.fields.iter()).filter(|(f, _)| vectors.contains(*f));
    for (f, e) in kept.filter_map(|(f, v)| Some((f, Elements::of(v)?))) {
        let since = match was.remove(f) {
            Some((since, h)) if h == e => {
                if put.ends {
                    put.ended.push((f.clone(), since));
                }
                since
            }
            other => {
                if let Some((since, _)) = other.filter(|(since, _)| *since != at) {
                    put.ended.push((f.clone(), since));
                }
                put.written.push((f.clone(), e.clone()));
                at
            }
        };
        put.apart.insert(f.clone(), since);
        if !put.ends {
            put.held.push((f.clone(), since, e));
        }
    }
    for (f, (since, _)) in was {
        match since == at {
            true => put.gone.push(f),
            false => put.ended.push((f, since)),
        }
    }
    put
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
        (Some(_), head) => Err(Error::refused(
            None,
            Refusal::OutOfRange {
                head: head.unwrap_or_default(),
            },
        )),
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
        return Some(Error::invalid(Case::CommitBehind { folded, through }));
    }
    let late = late.filter(|p| *p > through)?;
    Some(Error::invalid(Case::CommitLate { late, through }))
}

/// The refusal of a put made outside a transaction of the fold.
pub fn unstaged() -> Error {
    Error::invalid(Case::NotStaged)
}

#[cfg(test)]
mod tests;
