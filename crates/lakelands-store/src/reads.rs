//! The read doors: `describe`, `find`, `get`, `search`, `query`,
//! `proposals`.
//!
//! Every read happens at one position — the head, or `as_of` — and
//! answers it (`docs/decisions/009-every-response-carries-its-position.md`).
//! A read that filters on, orders by, aggregates or searches a field the
//! principal may not read is refused (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`);
//! what the principal may read of each record is then masked by its grants.
//!
//! **Filters.** A filter that needs neither the clock, the lake's settings
//! nor per-field provenance is handed to the engine, which applies it
//! exactly; any other filter is evaluated here, by the same evaluator
//! (`docs/decisions/023-one-evaluator-decides-what-an-expression-means.md`).
//!
//! **Order and pages.** Records sort by the `order_by` keys, a record
//! without a value for a key after every record with one in either
//! direction, and by key, ascending, to break ties. A cursor carries the
//! position its first page was read at and the sort values and key of the
//! last row, so the next page starts strictly after it and the page set is
//! stable while the lake is written.
//!
//! **A page reads a page.** `find` over a stored type asks the engine for
//! the page in that order, `order_by`, `after` and `limit + 1` pushed
//! down, and never for the type. A filter the engine cannot apply is
//! applied here over engine pages read one after another, each continuing
//! after the last row read, until the page and one row more are kept or
//! the type is out. So a page's cost is the rows read to fill it: a
//! selective filter the store evaluates over a large type is bounded in
//! memory and linear in time, by design. A page ordered by the key alone
//! reads about `limit` rows in the engine too; one ordered by a field is
//! sorted by the engine over the type at the position, for that answer
//! only, and since each such engine page sorts the type again, the store
//! asks for twice as many rows each time it reads on
//! (`docs/seams.md`, the ceiling of `find`).
//!
//! **Views** are computed on read: the source's records at the same
//! position, filtered by the view's `where`, grouped by its `group_by`
//! fields (whose values form the view record's key, in declared order, by
//! the one key rule of `docs/log-format.md` §2.3) and measured. A group with no member has no record. Keeping a view's
//! records materialised would be an engine's optimisation, and it could
//! never change an answer. A walk reads a view's records from that
//! computation too, once per read, never from the engine.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use lakelands_api::describe::{LakeDescription, TypeDescription};
use lakelands_api::engine::{After, ProjectionView, Scan};
use lakelands_api::expr::{Expr, Func};
use lakelands_api::ontology::{MatchPair, RelationDecl, TypeDecl};
use lakelands_api::query::{
    Edge, Hit, Hits, Pattern, Projection, QueryResult, Search, SearchMode, Walk,
};
use lakelands_api::read::{
    After as ReadAfter, Aggregate, Direction, Find, Found, Get, Order, OrderBy, ReadAt,
};
use lakelands_api::record::StoredRecord;
use lakelands_api::write::{Proposal, ProposalFilter};
use lakelands_api::{
    Cursor, Error, FieldName, Missing, Position, Principal, Record, RecordKey, Refusal,
    RelationName, Result, Timestamp, TypeName, Value,
};
use lakelands_core::expr::{self, Place, RecordScope, aggregate};
use lakelands_core::graph::{self, EdgeSource};
use lakelands_core::{
    canonical, derive as core_derive, describe, engines as core_engines, rights, valid_time,
};

use crate::declare::condition;
use crate::lake::{Lake, settings_in};
use crate::ontology::Ontology;

fn invalid(message: impl Into<String>) -> Error {
    Error::Refused {
        field: None,
        reason: Refusal::Invalid {
            message: message.into(),
        },
    }
}

fn now() -> Timestamp {
    let micros = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0);
    Timestamp(micros)
}

/// Everything one read sees.
pub struct Reader<'a> {
    /// The lake.
    pub lake: &'a Lake,
    /// Who reads.
    pub who: &'a Principal,
    /// The projection at the read's position.
    pub view: Box<dyn ProjectionView + 'a>,
    /// The ontology then.
    pub ontology: Ontology,
    /// The settings then.
    pub settings: BTreeMap<String, Value>,
    /// Whether the read is at the head: it asked for no earlier position.
    pub at_head: bool,
    /// The position read at.
    pub position: Position,
    /// Business time, if the read asked for it.
    pub valid_at: Option<Timestamp>,
    /// The clock `now()` and `today()` read.
    pub now: Timestamp,
    /// The records of each view this read has computed, so that a walk
    /// meeting a view many times computes it once.
    computed: RefCell<BTreeMap<TypeName, Rc<Vec<StoredRecord>>>>,
}

mod measures;

/// A row's sort values and its tiebreak.
type SortKey = (Vec<Value>, RecordKey);

/// The state a cursor carries.
#[derive(serde::Serialize, serde::Deserialize)]
struct CursorState {
    at: Position,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    after: Option<(Vec<Value>, RecordKey)>,
    #[serde(default)]
    offset: u64,
}

fn encode(c: &CursorState) -> Cursor {
    Cursor(canonical::hex(&canonical::of(c)))
}

fn decode(c: &Cursor) -> Result<CursorState> {
    let bad = || invalid("the cursor is not one this lake handed out");
    let bytes: Vec<u8> = (0..c.0.len())
        .step_by(2)
        .map(|i| {
            c.0.get(i..i + 2)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        })
        .collect::<Option<_>>()
        .ok_or_else(bad)?;
    serde_json::from_slice(&bytes).map_err(|_| bad())
}

/// Whether an expression needs something only the store knows or does:
/// the clock, the settings, per-field provenance, or a pattern match,
/// which is never handed to an engine (`docs/seams.md`, `scan`).
fn needs_store(e: &Expr) -> bool {
    match e {
        Expr::Call { func, args } => {
            matches!(
                func,
                Func::Now
                    | Func::Today
                    | Func::Setting
                    | Func::KnownAt
                    | Func::SeenAt
                    | Func::Matches
            ) || args.iter().any(needs_store)
        }
        Expr::Cmp { lhs, rhs, .. } | Expr::Arith { lhs, rhs, .. } => {
            needs_store(lhs) || needs_store(rhs)
        }
        Expr::Neg { arg } | Expr::Not { arg } | Expr::IsNull { arg } => needs_store(arg),
        Expr::And { args } | Expr::Or { args } => args.iter().any(needs_store),
        Expr::In { arg, list } => needs_store(arg) || list.iter().any(needs_store),
        Expr::Lit { .. } | Expr::Path { .. } => false,
    }
}

impl<'a> Reader<'a> {
    /// Open a read at `at`.
    pub fn open(
        lake: &'a Lake,
        who: &'a Principal,
        at: lakelands_api::ReadAt,
    ) -> Result<Reader<'a>> {
        let view = lake.engine.view(at.as_of)?;
        let position = view.position().unwrap_or(Position::GENESIS);
        let ontology = Ontology::from_declared(&view.declarations()?);
        let settings = settings_in(view.as_ref())?;
        Ok(Reader {
            lake,
            who,
            view,
            ontology,
            settings,
            at_head: at.as_of.is_none(),
            position,
            valid_at: at.valid_at,
            now: now(),
            computed: RefCell::default(),
        })
    }

    /// The records of `ty` with these keys, at this read. A view's come
    /// from its computation, any other type's from the engine, which keeps
    /// no view records and would answer none for one.
    fn get_many(&self, ty: &TypeName, keys: &[RecordKey]) -> Result<Vec<StoredRecord>> {
        let Some(all) = self.computed(ty)? else {
            return self.view.get_many(ty, keys);
        };
        let keys: BTreeSet<&RecordKey> = keys.iter().collect();
        Ok(all
            .iter()
            .filter(|r| keys.contains(&r.key))
            .cloned()
            .collect())
    }

    /// The record of `ty` with this key, at this read, as [`Self::get_many`].
    fn get_one(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>> {
        Ok(self.get_many(ty, std::slice::from_ref(key))?.pop())
    }

    /// The records of `ty` whose fields hold every value of `equal`, as
    /// [`Self::get_many`] reads them.
    fn matching(&self, ty: &TypeName, equal: Vec<(FieldName, Value)>) -> Result<Vec<StoredRecord>> {
        let Some(all) = self.computed(ty)? else {
            let scan = Scan {
                equal,
                limit: u32::MAX,
                restricted: true,
                ..Scan::default()
            };
            return self.view.scan(ty, &scan);
        };
        Ok(all
            .iter()
            .filter(|r| equal.iter().all(|(f, v)| r.fields.get(f) == Some(v)))
            .cloned()
            .collect())
    }

    /// A view's records at this read, computed once; `None` for a type
    /// that is not a view.
    fn computed(&self, ty: &TypeName) -> Result<Option<Rc<Vec<StoredRecord>>>> {
        let decl = match self.ontology.ty(ty) {
            Ok(decl) if decl.view.is_some() => decl.clone(),
            _ => return Ok(None),
        };
        if let Some(all) = self.computed.borrow().get(ty) {
            return Ok(Some(Rc::clone(all)));
        }
        let all = Rc::new(self.records(&decl, None)?);
        self.computed
            .borrow_mut()
            .insert(ty.clone(), Rc::clone(&all));
        Ok(Some(all))
    }

    fn declared(&self) -> Vec<lakelands_api::engine::Declared> {
        self.ontology.declared()
    }

    /// The type, if the principal may see it at all.
    fn visible_type(&self, ty: &TypeName) -> Result<&TypeDecl> {
        let decl = self.ontology.ty(ty)?;
        if describe::one_type(ty, &self.declared(), self.who).is_none() {
            return Err(Error::NotFound {
                missing: Missing::Type { name: ty.clone() },
            });
        }
        Ok(decl)
    }

    /// `describe`.
    pub fn describe(&self) -> LakeDescription {
        let mut lake = describe::lake(&self.lake.id, &self.declared(), self.who);
        for t in &mut lake.types {
            self.name_sorted(t);
        }
        lake
    }

    /// `describe_type`.
    pub fn describe_type(&self, ty: &TypeName) -> Result<TypeDescription> {
        let mut t =
            describe::one_type(ty, &self.declared(), self.who).ok_or_else(|| Error::NotFound {
                missing: Missing::Type { name: ty.clone() },
            })?;
        self.name_sorted(&mut t);
        Ok(t)
    }

    /// Name in a type's lifecycle the fields the lake keeps an order index
    /// for and the vector fields it asks an index for, as its
    /// `sort_index.<type>` and `vector_index.<type>` settings ask, of those
    /// the principal sees.
    fn name_sorted(&self, t: &mut TypeDescription) {
        let Ok(decl) = self.ontology.ty(&t.name) else {
            return;
        };
        let setting = self.settings.get(&core_engines::sort_index_key(&t.name));
        t.lifecycle.sorted = core_engines::sorted_fields(decl, setting)
            .into_iter()
            .filter(|f| t.fields.iter().any(|d| d.name == *f))
            .collect();
        let setting = self.settings.get(&core_engines::vector_index_key(&t.name));
        t.lifecycle.indexed = core_engines::indexed_fields(decl, setting)
            .into_iter()
            .filter(|f| t.fields.iter().any(|d| d.name == *f))
            .collect();
    }

    fn holds(
        &self,
        e: &Expr,
        fields: &BTreeMap<FieldName, Value>,
        meta: Option<&BTreeMap<FieldName, lakelands_api::FieldMeta>>,
    ) -> bool {
        struct S<'s> {
            r: RecordScope<'s>,
            meta: Option<&'s BTreeMap<FieldName, lakelands_api::FieldMeta>>,
        }
        impl expr::Scope for S<'_> {
            fn value(&self, path: &[FieldName]) -> Option<Value> {
                self.r.value(path)
            }
            fn meta(&self, f: &FieldName) -> Option<lakelands_api::FieldMeta> {
                self.meta.and_then(|m| m.get(f)).cloned()
            }
            fn setting(&self, key: &str) -> Option<Value> {
                self.r.setting(key)
            }
            fn now(&self) -> Timestamp {
                self.r.now
            }
        }
        let scope = S {
            r: RecordScope {
                fields,
                settings: Some(&self.settings),
                now: self.now,
            },
            meta,
        };
        expr::holds(e, &scope).unwrap_or(false)
    }

    /// The records of a type at this read's position that the principal
    /// is allowed to see, before masking: live, restricted ones only with
    /// the grant, valid at `valid_at` when asked. `filter` is pushed down
    /// when the engine can apply it.
    fn records(&self, decl: &TypeDecl, filter: Option<&Expr>) -> Result<Vec<StoredRecord>> {
        if let Some(v) = &decl.view {
            return self.view_records(decl, v, filter);
        }
        let push = filter.filter(|e| !needs_store(e)).cloned();
        let scan = Scan {
            filter: push.clone(),
            limit: u32::MAX,
            valid_at: self.valid_at,
            restricted: self.who.grants.restricted,
            ..Default::default()
        };
        let mut out = self.view.scan(&decl.name, &scan)?;
        out.retain(|r| !r.retired && (self.who.grants.restricted || !r.restricted));
        if let Some(t) = self.valid_at {
            out.retain(|r| valid_time::record_holds(decl, &r.fields, t));
        }
        if let (Some(e), None) = (filter, push) {
            out.retain(|r| self.holds(e, &r.fields, Some(&r.meta)));
        }
        Ok(out)
    }

    /// At most `want` records of a stored type the principal may see, in
    /// `order` after `after`, read from the engine a page at a time: the
    /// filter goes down when the engine can apply it, and is applied here
    /// over one engine page after another when it cannot.
    fn page(
        &self,
        decl: &TypeDecl,
        filter: Option<&Expr>,
        order: &[Order],
        after: Option<(Vec<Value>, RecordKey)>,
        want: usize,
    ) -> Result<Vec<StoredRecord>> {
        let push = filter.filter(|e| !needs_store(e)).cloned();
        let here = match (filter, &push) {
            (Some(e), None) => Some(e),
            _ => None,
        };
        let mut scan = Scan {
            filter: push,
            order_by: order.to_vec(),
            after: after.map(|(values, key)| After { values, key }),
            limit: u32::try_from(want).unwrap_or(u32::MAX),
            valid_at: self.valid_at,
            restricted: self.who.grants.restricted,
            ..Default::default()
        };
        let mut out = Vec::new();
        loop {
            let rows = self.view.scan(&decl.name, &scan)?;
            let last_page = rows.len() < scan.limit as usize;
            if let Some(last) = rows.last() {
                scan.after = Some(After {
                    values: Self::sort_values(order, last),
                    key: last.key.clone(),
                });
            }
            for r in rows {
                let keep = !r.retired
                    && (self.who.grants.restricted || !r.restricted)
                    && self
                        .valid_at
                        .is_none_or(|t| valid_time::record_holds(decl, &r.fields, t))
                    && here.is_none_or(|e| self.holds(e, &r.fields, Some(&r.meta)));
                if keep {
                    out.push(r);
                    if out.len() >= want {
                        return Ok(out);
                    }
                }
            }
            if last_page {
                return Ok(out);
            }
            // An engine page ordered by a field sorts the type again, so the
            // store asks for twice as many rows each time it reads on: a
            // few sorts per page, not one per `want` rows. A key-ordered
            // read stays at `want` rows, which is what bounds its memory.
            if !order.is_empty() {
                scan.limit = scan.limit.saturating_mul(2);
            }
        }
    }

    /// A view type's records, computed.
    fn view_records(
        &self,
        decl: &TypeDecl,
        v: &lakelands_api::ontology::ViewDecl,
        filter: Option<&Expr>,
    ) -> Result<Vec<StoredRecord>> {
        let source = self.ontology.ty(&v.source)?.clone();
        let mut probed: Vec<FieldName> = v.group_by.clone();
        let where_ = v
            .r#where
            .as_ref()
            .map(|w| condition(w, Place::Record, "view"))
            .transpose()?;
        if let Some(w) = &where_ {
            probed.extend(core_derive::reads(w));
        }
        let measures: Vec<(FieldName, Expr)> = v
            .measures
            .iter()
            .map(|m| Ok((m.name.clone(), condition(&m.expr, Place::Group, "measure")?)))
            .collect::<Result<_>>()?;
        for (_, m) in &measures {
            probed.extend(
                core_derive::reads(m)
                    .into_iter()
                    .filter(|f| source.fields.iter().any(|d| &d.name == f)),
            );
        }
        rights::check_probe(self.who, &source, probed.iter())?;
        let members = self.records(&source, where_.as_ref())?;
        // A view record is keyed like any record: its group_by values, in
        // declared order, by the one key rule.
        let keyed_by = TypeDecl {
            key: v.group_by.clone(),
            ..decl.clone()
        };
        let rows: Vec<&BTreeMap<FieldName, Value>> = members.iter().map(|r| &r.fields).collect();
        let mut out = Vec::new();
        for (by, group) in aggregate::groups(&rows, &v.group_by) {
            if by.values().any(Value::is_null) {
                continue;
            }
            let values =
                aggregate::measures(&group, &by, &measures, Some(&self.settings), self.now)
                    .map_err(|e| invalid(format!("view `{}`: {e}", decl.name)))?;
            let Some(key) = lakelands_core::log::record_key(&keyed_by, &by) else {
                continue;
            };
            let mut fields = by.clone();
            // A measure lands in the kind the view type declares for it.
            for (name, v) in values {
                let landed = match decl.fields.iter().find(|f| f.name == name) {
                    Some(f) => expr::eval::to_json(&expr::eval::from_json(&v), f.ty.kind, None)
                        .unwrap_or(Value::Null),
                    None => v,
                };
                fields.insert(name, landed);
            }
            fields.retain(|_, v| !v.is_null());
            out.push(StoredRecord {
                ty: decl.name.clone(),
                key,
                version: lakelands_api::Version(1),
                fields,
                meta: BTreeMap::new(),
                position: self.position,
                retired: false,
                restricted: false,
            });
        }
        if let Some(e) = filter {
            out.retain(|r| self.holds(e, &r.fields, None));
        }
        Ok(out)
    }

    fn sort_values(order: &[Order], r: &StoredRecord) -> Vec<Value> {
        order
            .iter()
            .map(|o| match o.by {
                OrderBy::Latest => Value::from(r.position.0),
                OrderBy::Value => r.fields.get(&o.field).cloned().unwrap_or(Value::Null),
                OrderBy::KnownAt => r
                    .meta
                    .get(&o.field)
                    .map_or(Value::Null, |m| Value::String(m.known_at.to_string())),
                OrderBy::SeenAt => r
                    .meta
                    .get(&o.field)
                    .and_then(|m| m.seen_at)
                    .map_or(Value::Null, |t| Value::String(t.to_string())),
            })
            .collect()
    }

    /// The one ordering: sort values in turn, missing last either way, then
    /// the key ascending.
    fn compare(
        order: &[Order],
        a: &(Vec<Value>, RecordKey),
        b: &(Vec<Value>, RecordKey),
    ) -> Ordering {
        for (i, o) in order.iter().enumerate() {
            let (x, y) = (&a.0[i], &b.0[i]);
            let c = match (x.is_null(), y.is_null()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => {
                    let c = expr::order::cmp_values(x, y);
                    if o.dir == Direction::Desc {
                        c.reverse()
                    } else {
                        c
                    }
                }
            };
            if c != Ordering::Equal {
                return c;
            }
        }
        a.1.cmp(&b.1)
    }

    fn limit(&self, asked: Option<u32>) -> Result<usize> {
        let l = asked.unwrap_or(self.lake.limits.default_limit);
        if l == 0 || l > self.lake.limits.max_limit {
            return Err(invalid(format!(
                "a limit is between 1 and {}",
                self.lake.limits.max_limit
            )));
        }
        Ok(l as usize)
    }

    /// `find`.
    pub fn find(&self, ty: &TypeName, find: &Find) -> Result<Found> {
        let decl = self.visible_type(ty)?.clone();
        for o in &find.order_by {
            if let Some((field, message)) = o.misfit() {
                return Err(Error::Refused {
                    field,
                    reason: Refusal::Invalid { message },
                });
            }
            // A view's records are computed at the read's position; none
            // has an event of its own to order by.
            if o.by == OrderBy::Latest && decl.view.is_some() {
                return Err(invalid(format!(
                    "`order_by` `latest` orders records by their latest change, and `{ty}` is a \
                     view, whose records are computed and have none"
                )));
            }
        }
        let filter = find
            .filter
            .as_ref()
            .map(|c| condition(c, Place::Record, "where"))
            .transpose()?;
        let mut probed: Vec<FieldName> = filter
            .as_ref()
            .map(|e| core_derive::reads(e).into_iter().collect())
            .unwrap_or_default();
        probed.extend(
            find.order_by
                .iter()
                .filter(|o| o.by != OrderBy::Latest)
                .map(|o| o.field.clone()),
        );
        if let Some(a) = &find.aggregate {
            probed.extend(a.measures.iter().filter_map(|m| m.field.clone()));
            // A field key probes its field; a named key, every field its
            // expression reads (024).
            let keys = aggregate::group_keys(&a.group_by).map_err(|e| invalid(e.to_string()))?;
            for (name, expr) in keys {
                match expr {
                    None => probed.push(name),
                    Some(e) => probed.extend(core_derive::reads(&e)),
                }
            }
        }
        rights::check_probe(self.who, &decl, probed.iter())?;
        let asked = find
            .after
            .as_ref()
            .map(|a| self.resume_after(&decl, find, a))
            .transpose()?;
        if let Some(a) = &find.aggregate {
            let rows = self.records(&decl, filter.as_ref())?;
            return self.aggregated(&decl, &rows, a);
        }
        let limit = self.limit(find.limit)?;
        let after = match &find.cursor {
            Some(c) => decode(c)?.after,
            None => asked,
        };
        let mut keyed: Vec<((Vec<Value>, RecordKey), StoredRecord)> = if decl.view.is_some() {
            // A view's records are computed from its source, whole.
            let mut keyed: Vec<_> = self
                .records(&decl, filter.as_ref())?
                .into_iter()
                .map(|r| ((Self::sort_values(&find.order_by, &r), r.key.clone()), r))
                .collect();
            keyed.sort_by(|a, b| Self::compare(&find.order_by, &a.0, &b.0));
            if let Some(after) = &after {
                keyed.retain(|(k, _)| Self::compare(&find.order_by, k, after) == Ordering::Greater);
            }
            keyed
        } else {
            self.page(&decl, filter.as_ref(), &find.order_by, after, limit + 1)?
                .into_iter()
                .map(|r| ((Self::sort_values(&find.order_by, &r), r.key.clone()), r))
                .collect()
        };
        let more = keyed.len() > limit;
        keyed.truncate(limit);
        let next_cursor = if more {
            keyed.last().map(|(k, _)| {
                encode(&CursorState {
                    at: self.position,
                    after: Some(k.clone()),
                    offset: 0,
                })
            })
        } else {
            None
        };
        let rows = keyed
            .into_iter()
            .filter_map(|(_, r)| rights::visible(self.who, &decl, &r, find.with_meta))
            .collect();
        Ok(Found::Records { rows, next_cursor })
    }

    /// The sort position a caller's `after` names, each value in its
    /// field's canonical form, so it resumes exactly where a cursor holding
    /// the same row would. Refused as invalid, naming the member, beside a
    /// cursor or an aggregate, with a value count other than `order_by`'s,
    /// or with a value the key's field cannot hold.
    fn resume_after(&self, decl: &TypeDecl, find: &Find, a: &ReadAfter) -> Result<SortKey> {
        if find.cursor.is_some() {
            return Err(invalid(
                "`after` and `cursor` both name where a page starts; give one",
            ));
        }
        if find.aggregate.is_some() {
            return Err(invalid(
                "`after` pages records; an `aggregate` has no pages",
            ));
        }
        if a.values.len() != find.order_by.len() {
            return Err(invalid(format!(
                "`after.values` holds {} value(s) and `order_by` has {} key(s); give one value per key",
                a.values.len(),
                find.order_by.len()
            )));
        }
        let mut values = Vec::with_capacity(a.values.len());
        for (i, (v, o)) in a.values.iter().zip(&find.order_by).enumerate() {
            let unfit = |why: String| Error::Refused {
                field: Some(o.field.clone()),
                reason: Refusal::Invalid {
                    message: format!("`after.values[{i}]` does not fit `{}`: {why}", o.field),
                },
            };
            let v = match (o.by, v) {
                (_, Value::Null) => Value::Null,
                (OrderBy::Latest, Value::Number(n)) if n.is_u64() => v.clone(),
                (OrderBy::Latest, _) => {
                    return Err(unfit("a position is a non-negative integer".into()));
                }
                (OrderBy::KnownAt | OrderBy::SeenAt, Value::String(t)) => {
                    let t = Timestamp::parse(t).map_err(|e| unfit(e.to_string()))?;
                    Value::String(t.to_string())
                }
                (OrderBy::KnownAt | OrderBy::SeenAt, _) => {
                    return Err(unfit("an instant is given as text".into()));
                }
                (OrderBy::Value, v) => match decl.fields.iter().find(|f| f.name == o.field) {
                    Some(f) => {
                        let vt =
                            f.ty.value_type
                                .as_ref()
                                .and_then(|n| self.ontology.value_types.get(n.as_str()))
                                .map(|e| &e.decl);
                        lakelands_core::value::check(v, &f.ty, vt)
                            .map_err(|e| unfit(e.to_string()))?
                    }
                    None => v.clone(),
                },
            };
            values.push(v);
        }
        Ok((values, a.key.clone()))
    }

    fn aggregated(&self, decl: &TypeDecl, rows: &[StoredRecord], a: &Aggregate) -> Result<Found> {
        let fields: Vec<&BTreeMap<FieldName, Value>> = rows.iter().map(|r| &r.fields).collect();
        let mut groups = aggregate::aggregate(&fields, a, self.now)
            .map_err(|e| invalid(format!("aggregate over `{}`: {e}", decl.name)))?;
        // A sum, minimum or maximum of a decimal field whose value type
        // declares a scale answers at that scale, so a sum over nothing is
        // `0.00` and not `0`.
        for m in &a.measures {
            if !matches!(
                m.func,
                lakelands_api::read::AggFunc::Sum
                    | lakelands_api::read::AggFunc::Min
                    | lakelands_api::read::AggFunc::Max
            ) {
                continue;
            }
            let Some(scale) = m
                .field
                .as_ref()
                .and_then(|f| decl.fields.iter().find(|d| &d.name == f))
                .and_then(|f| f.ty.value_type.as_ref())
                .and_then(|vt| self.ontology.value_types.get(vt.as_str()))
                .and_then(|vt| vt.decl.scale)
            else {
                continue;
            };
            for g in &mut groups {
                if let Some(Value::String(s)) = g.values.get_mut(&m.name)
                    && let Ok(mut d) = s.parse::<rust_decimal::Decimal>()
                    && d.scale() <= scale
                {
                    d.rescale(scale);
                    *s = d.to_string();
                }
            }
        }
        Ok(Found::Groups { rows: groups })
    }

    /// `get`.
    pub fn get(&self, ty: &TypeName, get: &Get) -> Result<Record> {
        let decl = self.visible_type(ty)?.clone();
        let missing = || Error::NotFound {
            missing: Missing::Record {
                ty: ty.clone(),
                key: get.key.clone(),
            },
        };
        let stored = if decl.view.is_some() {
            self.records(&decl, None)?
                .into_iter()
                .find(|r| r.key == get.key)
        } else {
            self.view.get(ty, &get.key)?.filter(|r| {
                !r.retired
                    && (self.who.grants.restricted || !r.restricted)
                    && self
                        .valid_at
                        .is_none_or(|t| valid_time::record_holds(&decl, &r.fields, t))
            })
        };
        stored
            .and_then(|r| rights::visible(self.who, &decl, &r, get.with_meta))
            .ok_or_else(missing)
    }

    /// `proposals`: every proposal for a reviewer, one's own otherwise.
    pub fn proposals(&self, filter: &ProposalFilter) -> Result<Vec<Proposal>> {
        let limit = self.limit(filter.limit)?;
        let offset = match &filter.cursor {
            Some(c) => decode(c)?.offset as usize,
            None => 0,
        };
        let all = self.view.proposals(&ProposalFilter {
            limit: Some(u32::MAX),
            cursor: None,
            ..filter.clone()
        })?;
        let me = self.who.writer();
        let mine: Vec<Proposal> = all
            .into_iter()
            .filter(|p| self.who.grants.review || p.proposer == me)
            .collect();
        Ok(ordered_proposals(mine, &filter.order_by)?
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect())
    }

    /// `search`.
    pub fn search(&self, s: &Search) -> Result<Hits> {
        let decl = self.visible_type(&s.ty)?.clone();
        let vector_field = match &s.mode {
            SearchMode::Vector { field, .. } | SearchMode::Hybrid { field, .. } => {
                Some(field.clone())
            }
            SearchMode::Lexical { .. } => None,
        };
        let text_fields = match &s.mode {
            SearchMode::Lexical { fields, .. } | SearchMode::Hybrid { fields, .. } => {
                fields.as_deref()
            }
            SearchMode::Vector { .. } => None,
        };
        rights::check_search(self.who, &decl, text_fields, vector_field.as_ref())?;
        let filter = s
            .filter
            .as_ref()
            .map(|c| condition(c, Place::Record, "where"))
            .transpose()?;
        if let Some(e) = &filter {
            rights::check_probe(self.who, &decl, core_derive::reads(e).iter())?;
        }
        let offset = match &s.cursor {
            Some(c) => decode(c)?.offset as usize,
            None => 0,
        };
        let k = s.k.max(1) as usize;
        // One more than the page, to know whether a next page exists.
        let want = offset + k + 1;
        let lexical = |text: &str, pull: u32| self.view.lexical(&s.ty, text, text_fields, pull);
        let vector = |field: &FieldName, v: &[f32], pull: u32| -> Result<Vec<(RecordKey, f64)>> {
            match self.lake.engine.vectors() {
                Some(index) => index.nearest(&s.ty, field.as_str(), v, pull),
                None => Err(Error::Unbound {
                    seam: "a vector index (this engine keeps none)".into(),
                }),
            }
        };
        // The candidates ranked best first, at most `pull` from each list,
        // and whether every list gave fewer than that — nothing is left.
        let rank = |pull: u32| -> Result<(Vec<(RecordKey, f64)>, bool)> {
            let short = |l: &Vec<(RecordKey, f64)>| l.len() < pull as usize;
            Ok(match &s.mode {
                SearchMode::Lexical { text, .. }
                | SearchMode::Hybrid {
                    text, vector: None, ..
                } => {
                    let l = lexical(text, pull)?;
                    let done = short(&l);
                    (l, done)
                }
                SearchMode::Vector { field, vector: v } => {
                    let l = vector(field, v, pull)?;
                    let done = short(&l);
                    (l, done)
                }
                SearchMode::Hybrid {
                    text,
                    field,
                    vector: Some(v),
                    ..
                } => {
                    let (a, b) = (lexical(text, pull)?, vector(field, v, pull)?);
                    let done = short(&a) && short(&b);
                    (fuse(&[a, b]), done)
                }
            })
        };
        let ranked_by = match &s.mode {
            SearchMode::Lexical { .. } => "lexical",
            SearchMode::Vector { field, .. } if self.indexed(&decl, field)? => "index",
            SearchMode::Vector { .. } => "vector",
            SearchMode::Hybrid {
                field,
                vector: Some(_),
                ..
            } if self.indexed(&decl, field)? => "hybrid (index)",
            SearchMode::Hybrid {
                vector: Some(_), ..
            } => "hybrid",
            SearchMode::Hybrid { vector: None, .. } => {
                "lexical (no query vector and no embedding backend)"
            }
        }
        .to_owned();
        // Candidates are pulled in growing batches until the page and one
        // more are visible, or the engine has no more: a filter, `valid_at`
        // or the grants never shorten a page silently.
        let mut pull = u32::try_from(want.saturating_mul(4).max(64)).unwrap_or(u32::MAX);
        let hits = loop {
            let (ranked, exhausted) = rank(pull)?;
            let hits = self.visible_hits(&decl, filter.as_ref(), ranked, s.with_meta)?;
            if hits.len() >= want || exhausted || pull == u32::MAX {
                break hits;
            }
            pull = pull.saturating_mul(4);
        };
        let more = hits.len() > offset + k;
        let page: Vec<Hit> = hits.into_iter().skip(offset).take(k).collect();
        let next_cursor = more.then(|| {
            encode(&CursorState {
                at: self.position,
                after: None,
                offset: (offset + k) as u64,
            })
        });
        Ok(Hits {
            hits: page,
            ranked_by,
            next_cursor,
        })
    }

    /// Whether the lake asks an index for `field` of `decl`'s type at the
    /// head, where the engines keep their vector indexes: a search over it
    /// answers the index's set, at any position.
    fn indexed(&self, decl: &TypeDecl, field: &FieldName) -> Result<bool> {
        let key = core_engines::vector_index_key(&decl.name);
        let setting = if self.at_head {
            self.settings.get(&key).cloned()
        } else {
            self.lake
                .engine
                .view(None)?
                .get(
                    &TypeName::from(lakelands_api::seam::SETTING_TYPE),
                    &RecordKey::from(key),
                )?
                .and_then(|r| core_engines::setting_value(&r).cloned())
        };
        Ok(core_engines::indexed_fields(decl, setting.as_ref()).contains(field))
    }

    /// The ranked candidates this read may see, as hits, in rank order.
    fn visible_hits(
        &self,
        decl: &TypeDecl,
        filter: Option<&Expr>,
        ranked: Vec<(RecordKey, f64)>,
        with_meta: bool,
    ) -> Result<Vec<Hit>> {
        let mut hits = Vec::new();
        for (key, score) in ranked {
            let Some(r) = self.view.get(&decl.name, &key)? else {
                continue;
            };
            if r.retired || (r.restricted && !self.who.grants.restricted) {
                continue;
            }
            if self
                .valid_at
                .is_some_and(|t| !valid_time::record_holds(decl, &r.fields, t))
            {
                continue;
            }
            if filter.is_some_and(|e| !self.holds(e, &r.fields, Some(&r.meta))) {
                continue;
            }
            if let Some(record) = rights::visible(self.who, decl, &r, with_meta) {
                hits.push(Hit { record, score });
            }
        }
        Ok(hits)
    }

    /// Refuse a walk over a derived relation around `here` that probes a
    /// matched field the principal may not read, on either side: an edge
    /// that appears or not answers whether the two values are equal (024,
    /// 044).
    fn probe_derived(&self, here: &TypeName, relation: &RelationName) -> Result<()> {
        for e in self.ontology.types.values() {
            for r in &e.decl.relations {
                let Some(d) = &r.derived else { continue };
                if r.name != *relation || (e.decl.name != *here && r.target != *here) {
                    continue;
                }
                rights::check_probe(self.who, &e.decl, d.pairs.iter().map(|p| &p.from))?;
                rights::check_probe(
                    self.who,
                    self.ontology.ty(&r.target)?,
                    d.pairs.iter().map(|p| &p.to),
                )?;
            }
        }
        Ok(())
    }

    /// `query`: the one pattern query.
    pub fn query(&self, p: &Pattern) -> Result<QueryResult> {
        let start_decl = self.visible_type(&p.start.ty)?.clone();
        let limit = self.limit(p.limit)?;
        let max_depth = self.lake.limits.max_depth;
        if measures::is_measure(&p.project) {
            measures::check(p)?;
        }
        // The type each step reaches, checked against the declarations.
        let mut types = vec![start_decl.name.clone()];
        let mut filters = Vec::new();
        let measure = measures::is_measure(&p.project);
        for (i, step) in p.steps.iter().enumerate() {
            // A measure's steps are the relations around its records, each
            // read from the start, not a chain.
            let here = if measure {
                start_decl.name.clone()
            } else {
                types.last().expect("the start is in").clone()
            };
            let reached = self.step_target(
                &here,
                &step.relation,
                step.walk,
                matches!(p.project, Projection::Graph | Projection::Components),
            )?;
            self.probe_derived(&here, &step.relation)?;
            if let (Some(t), Some(r)) = (&step.ty, &reached)
                && t != r
            {
                return Err(invalid(format!(
                    "step {} over `{}` reaches `{r}`, not `{t}`",
                    i + 1,
                    step.relation
                )));
            }
            let reached = reached.or_else(|| step.ty.clone()).unwrap_or(here);
            let decl = self.visible_type(&reached)?.clone();
            let f = step
                .filter
                .as_ref()
                .map(|c| condition(c, Place::Record, "step where"))
                .transpose()?;
            if let Some(e) = &f {
                rights::check_probe(self.who, &decl, core_derive::reads(e).iter())?;
            }
            filters.push(f);
            types.push(reached);
        }
        if measure {
            return self.measure(p, limit);
        }
        let hidden = Visible { reader: self };
        // The start.
        let starts: Vec<StoredRecord> = if let Some(key) = &p.start.key {
            match self.get(
                &p.start.ty,
                &lakelands_api::read::Get {
                    key: key.clone(),
                    with_meta: false,
                    at: ReadAt::default(),
                },
            ) {
                Ok(_) => self.get_one(&p.start.ty, key)?.into_iter().collect(),
                Err(Error::NotFound { .. }) => Vec::new(),
                Err(e) => return Err(e),
            }
        } else if let Some(mode) = &p.start.search {
            let s = Search {
                ty: p.start.ty.clone(),
                mode: mode.clone(),
                k: self.lake.limits.max_limit,
                cursor: None,
                filter: p.start.filter.clone(),
                with_meta: false,
                at: ReadAt::default(),
            };
            let keys: Vec<RecordKey> = self
                .search(&s)?
                .hits
                .into_iter()
                .map(|h| h.record.key)
                .collect();
            keys.iter()
                .filter_map(|k| self.view.get(&p.start.ty, k).ok().flatten())
                .collect()
        } else {
            let f = p
                .start
                .filter
                .as_ref()
                .map(|c| condition(c, Place::Record, "start where"))
                .transpose()?;
            if let Some(e) = &f {
                rights::check_probe(self.who, &start_decl, core_derive::reads(e).iter())?;
            }
            self.records(&start_decl, f.as_ref())?
        };
        // A path.
        if let Some((to_ty, to_key)) = &p.to {
            let relations: Vec<RelationName> = p.steps.iter().map(|s| s.relation.clone()).collect();
            let depth = p.steps.iter().map(|s| s.depth.max).max().unwrap_or(1);
            let mut nodes = Vec::new();
            let mut edges = Vec::new();
            if let Some(start) = starts.first() {
                let from = (start.ty.clone(), start.key.clone());
                if let Some(path) = graph::shortest_path(
                    &hidden,
                    &from,
                    &(to_ty.clone(), to_key.clone()),
                    &relations,
                    Walk::Both,
                    depth,
                    max_depth,
                )? {
                    nodes = path.nodes;
                    edges = path.edges;
                }
            }
            return self.graph_result(nodes, edges, false);
        }
        // A subgraph.
        if p.project == Projection::Graph {
            let relations: Vec<RelationName> = p.steps.iter().map(|s| s.relation.clone()).collect();
            let depth = p.steps.iter().map(|s| s.depth.max).max().unwrap_or(0);
            let walk = p.steps.first().map_or(Walk::Both, |s| s.walk);
            // Several starts merge into one walk: each record at the fewest
            // hops any start reaches it in, ordered by (depth, type, key) as
            // one walk orders what it reaches (026, 029).
            let mut depth_of: BTreeMap<lakelands_core::graph::Node, u32> = BTreeMap::new();
            let mut edges = Vec::new();
            let mut truncated = false;
            for start in &starts {
                let g = graph::subgraph(
                    &hidden,
                    &(start.ty.clone(), start.key.clone()),
                    &relations,
                    walk,
                    depth,
                    limit,
                    max_depth,
                )?;
                truncated |= g.truncated;
                for r in g.nodes {
                    let d = depth_of.entry(r.node).or_insert(r.depth);
                    *d = (*d).min(r.depth);
                }
                edges.extend(g.edges);
            }
            let mut nodes: Vec<(u32, lakelands_core::graph::Node)> =
                depth_of.into_iter().map(|(n, d)| (d, n)).collect();
            nodes.sort();
            let nodes = nodes.into_iter().map(|(_, n)| n).collect();
            edges.sort_by(|a, b| (&a.from, &a.relation, &a.to).cmp(&(&b.from, &b.relation, &b.to)));
            edges.dedup_by(|a, b| (&a.from, &a.relation, &a.to) == (&b.from, &b.relation, &b.to));
            return self.graph_result(nodes, edges, truncated);
        }
        // Rows: one per match.
        let mut rows: Vec<BTreeMap<String, Value>> = Vec::new();
        let mut truncated_any = false;
        for start in &starts {
            let from = (start.ty.clone(), start.key.clone());
            let keep = |i: usize, node: &lakelands_core::graph::Node| -> Result<bool> {
                let Some(f) = &filters[i] else {
                    return Ok(true);
                };
                Ok(self
                    .get_one(&node.0, &node.1)?
                    .is_some_and(|r| self.holds(f, &r.fields, Some(&r.meta))))
            };
            let t = graph::traverse(&hidden, &from, &p.steps, max_depth, usize::MAX, keep)?;
            truncated_any |= t.truncated;
            for binding in t.bindings {
                let mut row = BTreeMap::new();
                let mut chain = vec![from.clone()];
                chain.extend(binding.into_iter().map(|r| r.node));
                for node in &chain {
                    self.bind(&mut row, node)?;
                }
                rows.push(row);
            }
        }
        let _ = truncated_any;
        if let Projection::Rows { fields } = &p.project
            && !fields.is_empty()
        {
            for row in &mut rows {
                row.retain(|k, _| fields.iter().any(|f| f == k));
            }
        }
        if let Projection::Aggregate(a) = &p.project {
            let as_fields: Vec<BTreeMap<FieldName, Value>> = rows
                .iter()
                .map(|r| {
                    r.iter()
                        .map(|(k, v)| (FieldName::from(k.as_str()), v.clone()))
                        .collect()
                })
                .collect();
            let refs: Vec<&BTreeMap<FieldName, Value>> = as_fields.iter().collect();
            let groups =
                aggregate::aggregate(&refs, a, self.now).map_err(|e| invalid(e.to_string()))?;
            return Ok(QueryResult::Groups { rows: groups });
        }
        if p.order_by.iter().any(|o| o.by == OrderBy::Latest) {
            return Err(invalid(
                "`order_by` `latest` orders records by their latest change, and a query's rows \
                 are projections of a walk, not records",
            ));
        }
        let order: Vec<Order> = p.order_by.clone();
        let mut keyed: Vec<(SortKey, BTreeMap<String, Value>)> = rows
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                let vals = order
                    .iter()
                    .map(|o| r.get(o.field.as_str()).cloned().unwrap_or(Value::Null))
                    .collect();
                ((vals, RecordKey::from(format!("{i:020}"))), r)
            })
            .collect();
        keyed.sort_by(|a, b| Self::compare(&order, &a.0, &b.0));
        let offset = match &p.cursor {
            Some(c) => decode(c)?.offset as usize,
            None => 0,
        };
        let more = keyed.len() > offset + limit;
        let rows = keyed
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|(_, r)| r)
            .collect();
        let next_cursor = more.then(|| {
            encode(&CursorState {
                at: self.position,
                after: None,
                offset: (offset + limit) as u64,
            })
        });
        Ok(QueryResult::Rows { rows, next_cursor })
    }

    /// The type a step over `relation` reaches from `here`, walking `walk`.
    fn step_target(
        &self,
        here: &TypeName,
        relation: &RelationName,
        walk: Walk,
        lenient: bool,
    ) -> Result<Option<TypeName>> {
        let decl = self.ontology.ty(here)?;
        let out = decl
            .relations
            .iter()
            .find(|r| &r.name == relation)
            .map(|r| r.target.clone());
        let inn: Vec<TypeName> = self
            .ontology
            .types
            .values()
            .filter(|e| {
                e.decl
                    .relations
                    .iter()
                    .any(|r| &r.name == relation && &r.target == here)
            })
            .map(|e| e.decl.name.clone())
            .collect();
        let found = match walk {
            Walk::Out => out,
            Walk::In => inn.first().cloned(),
            Walk::Both => out.or_else(|| inn.first().cloned()),
        };
        match found {
            Some(t) => Ok(Some(t)),
            None if lenient && (out_any(&self.ontology, relation)) => Ok(None),
            None => Err(invalid(format!(
                "`{here}` has no relation `{relation}` to walk {}",
                match walk {
                    Walk::Out => "out",
                    Walk::In => "in",
                    Walk::Both => "either way",
                }
            ))),
        }
    }

    fn bind(
        &self,
        row: &mut BTreeMap<String, Value>,
        node: &lakelands_core::graph::Node,
    ) -> Result<()> {
        let decl = self.ontology.ty(&node.0)?;
        if let Some(r) = self.get_one(&node.0, &node.1)?
            && let Some(rec) = rights::visible(self.who, decl, &r, false)
        {
            row.insert(format!("{}.key", node.0), Value::String(node.1.to_string()));
            for (f, v) in rec.fields {
                row.insert(format!("{}.{f}", node.0), v);
            }
        }
        Ok(())
    }

    fn graph_result(
        &self,
        nodes: Vec<lakelands_core::graph::Node>,
        edges: Vec<Edge>,
        truncated: bool,
    ) -> Result<QueryResult> {
        let mut records = Vec::new();
        for (ty, key) in &nodes {
            let decl = self.ontology.ty(ty)?;
            if let Some(r) = self.get_one(ty, key)?
                && let Some(rec) = rights::visible(self.who, decl, &r, false)
            {
                records.push(rec);
            }
        }
        Ok(QueryResult::Graph {
            nodes: records,
            edges,
            truncated,
        })
    }
}

/// The members of a proposal an `order_by` may name.
const PROPOSAL_MEMBERS: [&str; 9] = [
    "id",
    "position",
    "proposer",
    "proposed_at",
    "write",
    "why",
    "state",
    "decided_by",
    "reason",
];

/// Proposals in the caller's `order_by`, applied by the store to the
/// engine's newest-first answer: each key a member of the proposal,
/// values in the one sort order, a proposal without a value after every
/// proposal with one; the engine's order breaks ties. A key that names no
/// member of a proposal, or orders by provenance, is refused.
fn ordered_proposals(proposals: Vec<Proposal>, order_by: &[Order]) -> Result<Vec<Proposal>> {
    if order_by.is_empty() {
        return Ok(proposals);
    }
    for o in order_by {
        if o.by != OrderBy::Value || !PROPOSAL_MEMBERS.contains(&o.field.as_str()) {
            return Err(invalid(format!(
                "`order_by` orders proposals by their members ({}) and their values, and `{}` \
                 by `{}` is not one",
                PROPOSAL_MEMBERS.join(", "),
                o.field,
                serde_json::to_value(o.by).unwrap_or_default()
            )));
        }
    }
    let mut keyed: Vec<(Vec<Option<Value>>, Proposal)> = proposals
        .into_iter()
        .map(|p| {
            let as_json = serde_json::to_value(&p).expect("a proposal serialises");
            let values = order_by
                .iter()
                .map(|o| {
                    as_json
                        .get(o.field.as_str())
                        .filter(|v| !v.is_null())
                        .cloned()
                })
                .collect();
            (values, p)
        })
        .collect();
    keyed.sort_by(|a, b| {
        let place = |values: &Vec<Option<Value>>| core_engines::Place {
            values: values.clone(),
            key: "",
        };
        core_engines::cmp_places(&place(&a.0), &place(&b.0), order_by)
    });
    Ok(keyed.into_iter().map(|(_, p)| p).collect())
}

fn out_any(o: &Ontology, relation: &RelationName) -> bool {
    o.types
        .values()
        .any(|e| e.decl.relations.iter().any(|r| &r.name == relation))
}

/// The edges of a derived relation around each of some records of one
/// type, one list per key, resolved from the view by identity of the
/// matched values: out to the target's records that hold the owner's
/// values, in from the owners that hold the target's. The records are
/// read a page at a time; each one's matches are a scan of its own. Nothing is stored; the edges are as the view's position has
/// the values (`docs/decisions/044-a-relation-may-be-derived-from-matching-values.md`).
fn derived_edges(
    r: &Reader<'_>,
    ty: &TypeName,
    keys: &[RecordKey],
    relation: Option<&RelationName>,
    walk: Walk,
) -> Result<Vec<Vec<Edge>>> {
    let wanted = |x: &RelationDecl| x.derived.is_some() && relation.is_none_or(|n| *n == x.name);
    let owned: Vec<&RelationDecl> = match (walk, r.ontology.ty(ty)) {
        (Walk::Out | Walk::Both, Ok(t)) => t.relations.iter().filter(|x| wanted(x)).collect(),
        _ => Vec::new(),
    };
    let targeted: Vec<(&TypeName, &RelationDecl)> = if walk == Walk::Out {
        Vec::new()
    } else {
        r.ontology
            .types
            .values()
            .flat_map(|e| e.decl.relations.iter().map(move |x| (&e.decl.name, x)))
            .filter(|(_, x)| x.target == *ty && wanted(x))
            .collect()
    };
    let mut all = vec![Vec::new(); keys.len()];
    if owned.is_empty() && targeted.is_empty() {
        return Ok(all);
    }
    let nears: BTreeMap<RecordKey, StoredRecord> = r
        .get_many(ty, keys)?
        .into_iter()
        .map(|near| (near.key.clone(), near))
        .collect();
    for (key, list) in keys.iter().zip(all.iter_mut()) {
        let Some(near) = nears.get(key) else {
            continue;
        };
        // The values `near` holds for one side of a match, and the pairs a
        // scan of the other side must hold: none when a value is missing, for
        // null never matches.
        let equal = |pairs: &[MatchPair], out: bool| -> Option<Vec<(FieldName, Value)>> {
            pairs
                .iter()
                .map(|p| {
                    let (here, there) = if out {
                        (&p.from, &p.to)
                    } else {
                        (&p.to, &p.from)
                    };
                    near.fields
                        .get(here)
                        .filter(|v| !v.is_null())
                        .map(|v| (there.clone(), v.clone()))
                })
                .collect()
        };
        let scan = |other: &TypeName, equal| r.matching(other, equal);
        for x in &owned {
            let pairs = &x.derived.as_ref().expect("derived").pairs;
            if let Some(equal) = equal(pairs, true) {
                for m in scan(&x.target, equal)? {
                    list.push(Edge {
                        from: (ty.clone(), key.clone()),
                        relation: x.name.clone(),
                        to: (x.target.clone(), m.key),
                        fields: BTreeMap::new(),
                    });
                }
            }
        }
        for &(owner, x) in &targeted {
            let pairs = &x.derived.as_ref().expect("derived").pairs;
            if let Some(equal) = equal(pairs, false) {
                for m in scan(owner, equal)? {
                    list.push(Edge {
                        from: (owner.clone(), m.key),
                        relation: x.name.clone(),
                        to: (ty.clone(), key.clone()),
                        fields: BTreeMap::new(),
                    });
                }
            }
        }
    }
    Ok(all)
}

/// The walks' view of the lake for one principal: a record is visible when
/// it is live, not restricted unless the principal may see restricted
/// records, valid at the read's `valid_at`, and of a type the principal
/// may see.
struct Visible<'r, 'a> {
    reader: &'r Reader<'a>,
}

impl EdgeSource for Visible<'_, '_> {
    fn edges(
        &self,
        ty: &TypeName,
        keys: &[RecordKey],
        relation: Option<&RelationName>,
        walk: Walk,
    ) -> Result<Vec<Vec<Edge>>> {
        let r = self.reader;
        let mut lists = ProjectionView::edges(r.view.as_ref(), ty, keys, relation, walk)?;
        for (list, derived) in lists
            .iter_mut()
            .zip(derived_edges(r, ty, keys, relation, walk)?)
        {
            list.extend(derived);
        }
        // A link's edge comes from its own record: a restricted link is
        // walked only with the `restricted` grant, and its fields only as
        // far as the principal may read the link type's fields. The link
        // records are read a link type at a time.
        let carried = |e: &Edge| {
            r.ontology
                .ty(&e.from.0)
                .ok()
                .and_then(|d| d.relations.iter().find(|x| x.name == e.relation))
                .is_some_and(lakelands_core::links::carried_by_links)
        };
        let link_of = |e: &Edge| {
            (
                lakelands_core::links::link_type_name(&e.from.0, &e.relation),
                lakelands_core::links::link_key(&e.from.1, &e.to.1),
            )
        };
        let mut wanted: BTreeMap<TypeName, Vec<RecordKey>> = BTreeMap::new();
        for e in lists.iter().flatten().filter(|e| carried(e)) {
            let (lt, lk) = link_of(e);
            wanted.entry(lt).or_default().push(lk);
        }
        let mut links: BTreeMap<(TypeName, RecordKey), StoredRecord> = BTreeMap::new();
        for (lt, lks) in wanted {
            for link in r.view.get_many(&lt, &lks)? {
                links.insert((lt.clone(), link.key.clone()), link);
            }
        }
        let mut out = Vec::with_capacity(lists.len());
        for list in lists {
            let mut kept = Vec::with_capacity(list.len());
            for mut e in list {
                if carried(&e) {
                    let (lt, lk) = link_of(&e);
                    let Some(link) = links.get(&(lt.clone(), lk)) else {
                        continue;
                    };
                    if link.restricted && !r.who.grants.restricted {
                        continue;
                    }
                    if let Ok(decl) = r.ontology.ty(&lt) {
                        e.fields.retain(|name, _| {
                            decl.fields.iter().any(|f| {
                                &f.name == name && rights::field_access(r.who, &lt, f).reads()
                            })
                        });
                    }
                }
                kept.push(e);
            }
            out.push(kept);
        }
        Ok(out)
    }

    fn visible(&self, nodes: &[graph::Node]) -> Result<Vec<bool>> {
        let r = self.reader;
        let mut shown: BTreeSet<graph::Node> = BTreeSet::new();
        for (ty, keys) in graph::by_type(nodes) {
            let Ok(decl) = r.ontology.ty(ty) else {
                continue;
            };
            if describe::one_type(ty, &r.declared(), r.who).is_none() {
                continue;
            }
            for rec in r.get_many(ty, &keys)? {
                if !rec.retired
                    && (r.who.grants.restricted || !rec.restricted)
                    && r.valid_at
                        .is_none_or(|t| valid_time::record_holds(decl, &rec.fields, t))
                {
                    shown.insert((rec.ty, rec.key));
                }
            }
        }
        Ok(nodes.iter().map(|n| shown.contains(n)).collect())
    }
}

/// Reciprocal rank fusion of ranked lists: each list adds `1 / (60 +
/// rank)` for every key it ranks (rank from 0, best first). Ties are broken
/// by key, so the fused order is the same on every engine.
pub fn fuse(lists: &[Vec<(RecordKey, f64)>]) -> Vec<(RecordKey, f64)> {
    const K: f64 = 60.0;
    let mut score: BTreeMap<RecordKey, f64> = BTreeMap::new();
    for list in lists {
        for (rank, (key, _)) in list.iter().enumerate() {
            *score.entry(key.clone()).or_default() += 1.0 / (K + rank as f64);
        }
    }
    let mut out: Vec<(RecordKey, f64)> = score.into_iter().collect();
    out.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    out
}

#[cfg(test)]
mod tests;
