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
//! the one key rule of `docs/log-format.md` §2.3) and measured. A group
//! with no member has no record, except that a view without `group_by` is
//! always one record, keyed [`lakelands_api::key::UNGROUPED`], whatever
//! its `where` keeps. Keeping a view's records materialised would be an
//! engine's optimisation, and it could never change an answer. A walk reads a view's records from that
//! computation too, once per read, never from the engine.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use lakelands_api::describe::{LakeDescription, TypeDescription};
use lakelands_api::engine::{After, Declared, ProjectionView, Scan};
use lakelands_api::expr::{Expr, Func, Literal};
use lakelands_api::ontology::{
    FieldType, MatchPair, RelationDecl, StorageKind, TypeDecl, VerbEffect,
};
use lakelands_api::query::{
    Edge, Hit, Hits, Pattern, Projection, QueryResult, Search, SearchMode, Step, Walk,
};
use lakelands_api::read::{
    After as ReadAfter, AggFunc, Aggregate, Direction, Find, Found, Get, Group, Order, OrderBy,
    Params, ReadAt,
};
use lakelands_api::record::StoredRecord;
use lakelands_api::write::{Declaration, Migration, Proposal, ProposalFilter, ProposedWrite};
use lakelands_api::{
    Access, Cursor, Error, FieldName, Missing, Position, Principal, Record, RecordKey, Refusal,
    RelationName, Result, Timestamp, TypeName, Value, VerbName,
};
use lakelands_core::expr::{self, Place, RecordScope, aggregate};
use lakelands_core::graph::{self, EdgeSource};
use lakelands_core::{
    canonical, derive as core_derive, describe, engines as core_engines, invalid, rights,
    valid_time,
};

use crate::declare::condition;
use crate::lake::{Lake, settings_in};
use crate::ontology::Ontology;

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
    /// The parameters this read binds, by the view they bind: as given, in
    /// canonical form, and as the literals `param()` is replaced by.
    bound: RefCell<BTreeMap<TypeName, Bound>>,
    /// The views being computed, outermost first.
    computing: RefCell<Vec<TypeName>>,
}

mod measures;

/// A view's parameters as one read binds them: as given, in canonical
/// form, and as the literals `param()` is replaced by.
type Bound = (Params, BTreeMap<String, Literal>);

/// One source of a view as read: its members, folded, and its measures, bound.
type Source = (aggregate::Grouping, Vec<(FieldName, Expr)>);

/// A row's sort values and its tiebreak.
type SortKey = (Vec<Value>, RecordKey);

/// One ranked list of a search, read to the depth it is given: the
/// candidates best first, with their scores.
type Ranking<'a> = Box<dyn Fn(u32) -> Result<Vec<(RecordKey, f64)>> + 'a>;

/// The state a cursor carries.
#[derive(serde::Serialize, serde::Deserialize)]
struct CursorState {
    at: Position,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    after: Option<(Vec<Value>, RecordKey)>,
    #[serde(default)]
    offset: u64,
    /// The view parameters the first page was read with: a page read
    /// with others would continue a different answer.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    params: Params,
    /// The door and the type it was handed out for, which a page
    /// continues and nothing else (018).
    #[serde(default)]
    door: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ty: Option<TypeName>,
    /// The digest of the read it was handed out for ([`read_of`]).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    of: String,
}

fn encode(c: &CursorState) -> Cursor {
    Cursor(canonical::hex(&canonical::of(c)))
}

/// The offset a `query` page continues at: none without a cursor.
fn offset(c: Option<&Cursor>, p: &Pattern, params: &Params) -> Result<usize> {
    let Some(c) = c else { return Ok(0) };
    let state = resume(c, "query", Some(&p.start.ty), params)?;
    same_read(&state, &pattern_read(p))?;
    Ok(state.offset as usize)
}

/// The digest of what decides a read's rows and their order: a cursor
/// carries it, and continues no other read through the same door. The
/// page size, the position (the cursor's own) and what a row carries
/// are left out by the callers.
fn read_of(shape: impl serde::Serialize) -> String {
    canonical::sha256_hex(&serde_json::to_value(shape).unwrap_or(Value::Null))
}

/// The digest of a pattern without its cursor, its page size and the
/// position, which the cursor holds.
fn pattern_read(p: &Pattern) -> String {
    let at = ReadAt {
        as_of: None,
        ..p.at
    };
    read_of(Pattern {
        cursor: None,
        limit: None,
        at,
        ..p.clone()
    })
}

/// Refuse a cursor handed out for another read through the same door:
/// another filter, order, search or pattern (018).
fn same_read(state: &CursorState, of: &str) -> Result<()> {
    if state.of != of {
        return Err(invalid(format!(
            "the cursor was handed out for another `{}`; continue it with the read its first \
             page was read with",
            state.door
        )));
    }
    Ok(())
}

/// A cursor's state, when it was handed out for this door, this type and
/// these view parameters: a page continues only the answer it began, and
/// any other use is refused, naming what differs (018).
fn resume(c: &Cursor, door: &str, ty: Option<&TypeName>, params: &Params) -> Result<CursorState> {
    let state = decode(c)?;
    let on = |door: &str, ty: Option<&TypeName>| match ty {
        Some(ty) => format!("`{door}` on `{ty}`"),
        None => format!("`{door}`"),
    };
    if state.door != door || state.ty.as_ref() != ty {
        return Err(invalid(format!(
            "the cursor was handed out for {}, not {}; continue it there",
            on(&state.door, state.ty.as_ref()),
            on(door, ty)
        )));
    }
    if &state.params != params {
        return Err(invalid(
            "the cursor was handed out for other parameters; continue with the parameters \
             its first page was read with",
        ));
    }
    Ok(state)
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

/// The records a count that needs the store reads at a time.
const COUNT_PAGE: usize = 1024;

/// Whether an expression needs something only the store knows or does:
/// the clock, the settings, per-field provenance, or a pattern match,
/// which is never handed to an engine (`docs/seams.md`, `scan`).
fn needs_store(e: &Expr) -> bool {
    use Func::*;
    matches!(
        e,
        Expr::Call {
            func: Now | Today | Setting | KnownAt | SeenAt | Matches,
            ..
        }
    ) || expr::children(e).any(needs_store)
}

impl<'a> Reader<'a> {
    /// Open a read at `at`.
    pub fn open(lake: &'a Lake, who: &'a Principal, at: ReadAt) -> Result<Reader<'a>> {
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
            now: crate::lake::wall_clock(),
            computed: RefCell::default(),
            bound: RefCell::default(),
            computing: RefCell::default(),
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

    /// The parameters this read bound for `ty`, in canonical form.
    fn bound_params(&self, ty: &TypeName) -> Params {
        self.bound
            .borrow()
            .get(ty)
            .map(|(p, _)| p.clone())
            .unwrap_or_default()
    }

    /// Binds the parameters a read gives for `decl`: refused as invalid,
    /// naming it, when one is unknown, missing or not of its declared kind
    /// (a type that is not a view declares none). Answers them in their
    /// canonical form, as a cursor carries them.
    fn bind_params(&self, decl: &TypeDecl, given: &Params) -> Result<Params> {
        let ty = &decl.name;
        let declared = decl.view.as_ref().map_or(&[][..], |v| &v.params[..]);
        if let Some(k) = given
            .keys()
            .find(|k| !declared.iter().any(|p| &p.name == *k))
        {
            let names: Vec<String> = declared.iter().map(|p| format!("`{}`", p.name)).collect();
            return Err(invalid(if names.is_empty() {
                format!("`{ty}` declares no parameters, so `{k}` binds nothing")
            } else {
                format!(
                    "view `{ty}` declares no parameter `{k}`; its parameters are {}",
                    names.join(", ")
                )
            }));
        }
        let mut canonical = Params::new();
        let mut literals = BTreeMap::new();
        for p in declared {
            let kind = lakelands_core::value::kind_name(p.kind);
            let Some(v) = given.get(&p.name) else {
                return Err(invalid(format!(
                    "view `{ty}` needs parameter `{}` ({kind})",
                    p.name
                )));
            };
            let wrong = || {
                invalid(format!(
                    "parameter `{}` of view `{ty}` is {kind}, and {v} is not one",
                    p.name
                ))
            };
            let v = lakelands_core::value::check(v, &FieldType::kind(p.kind), None)
                .ok()
                .filter(|v| !v.is_null())
                .ok_or_else(wrong)?;
            let literal = match &v {
                Value::Bool(b) => Literal::Bool(*b),
                Value::Number(n) => Literal::Number(n.to_string()),
                Value::String(s) if p.kind == StorageKind::Decimal => Literal::Number(s.clone()),
                Value::String(s) => Literal::Text(s.clone()),
                _ => return Err(wrong()),
            };
            literals.insert(p.name.to_string(), literal);
            canonical.insert(p.name.clone(), v);
        }
        if !declared.is_empty() {
            self.bound
                .borrow_mut()
                .insert(ty.clone(), (canonical.clone(), literals));
        }
        Ok(canonical)
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
        // A read at a valid time filters on the type's valid-time fields,
        // so it probes them as a `where` on them would (024).
        if self.valid_at.is_some() {
            let timed = decl.valid_from.iter().chain(&decl.valid_to);
            rights::check_probe(self.who, decl, timed)?;
        }
        Ok(decl)
    }

    /// Whether the principal may see a stored record at this read, before
    /// masking: live, restricted only with the grant, valid at `valid_at`
    /// when asked.
    fn live(&self, decl: &TypeDecl, r: &StoredRecord) -> bool {
        !r.retired
            && (self.who.grants.restricted || !r.restricted)
            && (self.valid_at).is_none_or(|t| valid_time::record_holds(decl, &r.fields, t))
    }

    /// A read's `where` over one record of `decl`, named `at`, after the
    /// principal is found to read every field it reads (024).
    fn record_filter(
        &self,
        decl: &TypeDecl,
        c: Option<&lakelands_api::expr::Condition>,
        at: &str,
    ) -> Result<Option<Expr>> {
        let Some(c) = c else { return Ok(None) };
        let e = condition(c, Place::Record, at)?;
        rights::check_probe(self.who, decl, core_derive::reads(&e).iter())?;
        Ok(Some(e))
    }

    /// `describe`.
    pub fn describe(&self) -> LakeDescription {
        let mut lake = describe::lake(&self.lake.id, &self.declared(), self.who);
        for t in &mut lake.types {
            self.name_sorted(t);
        }
        // Whether the host binds a verb's executor is the binding now, at
        // any position: it is not in the log.
        let executors = &self.lake.seams.executors;
        for e in lake.verbs.iter_mut().filter_map(|v| v.executor.as_mut()) {
            e.bound = executors.iter().any(|x| x.name() == e.name);
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
        let seen = |f: &FieldName| t.fields.iter().any(|d| d.name == *f);
        let setting = self.settings.get(&core_engines::sort_index_key(&t.name));
        t.lifecycle.sorted = core_engines::sorted_fields(decl, setting);
        t.lifecycle.sorted.retain(seen);
        let setting = self.settings.get(&core_engines::vector_index_key(&t.name));
        t.lifecycle.indexed = core_engines::indexed_fields(decl, setting);
        t.lifecycle.indexed.retain(seen);
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
        self.page(decl, filter, &[], None, usize::MAX)
    }

    /// How many records of a type [`Self::records`] would answer, holding
    /// no more than a page of a stored type: the engine counts what it can
    /// select itself, and a filter only the store evaluates is applied a
    /// page at a time. A view's records, one per group, are computed first.
    fn count(&self, decl: &TypeDecl, filter: Option<&Expr>) -> Result<u64> {
        if let Some(v) = &decl.view {
            return Ok(self.view_records(decl, v, filter)?.len() as u64);
        }
        if let Some(e) = filter.filter(|e| needs_store(e)) {
            let mut n = 0;
            self.each_member(decl, Some(e), COUNT_PAGE, |_| {
                n += 1;
                Ok(())
            })?;
            return Ok(n);
        }
        let scan = Scan {
            filter: filter.cloned(),
            valid_at: self.valid_at,
            restricted: self.who.grants.restricted,
            ..Default::default()
        };
        self.view.count(&decl.name, &scan)
    }

    /// The records [`Self::records`] would answer, handed to `each` one at
    /// a time in key order until it fails: a stored type's are read `page`
    /// at a time and never held together, a view's are computed first.
    fn each_member(
        &self,
        decl: &TypeDecl,
        filter: Option<&Expr>,
        page: usize,
        mut each: impl FnMut(&StoredRecord) -> Result<()>,
    ) -> Result<()> {
        if decl.view.is_some() {
            return self.records(decl, filter)?.iter().try_for_each(each);
        }
        let mut after = None;
        loop {
            let rows = self.page(decl, filter, &[], after, page)?;
            rows.iter().try_for_each(&mut each)?;
            if rows.len() < page {
                return Ok(());
            }
            after = rows.last().map(|r| (Vec::new(), r.key.clone()));
        }
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
        let here = filter.filter(|_| push.is_none());
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
                if self.live(decl, &r)
                    && here.is_none_or(|e| self.holds(e, &r.fields, Some(&r.meta)))
                {
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

    /// A view type's records, computed. A view met again while it is
    /// still being computed is a cycle over `source` that came in with a
    /// log `declare` did not check; it is refused, naming the chain, never
    /// followed.
    fn view_records(
        &self,
        decl: &TypeDecl,
        v: &lakelands_api::ontology::ViewDecl,
        filter: Option<&Expr>,
    ) -> Result<Vec<StoredRecord>> {
        if let Some(at) = self.computing.borrow().iter().position(|t| *t == decl.name) {
            let chain = self.computing.borrow()[at..]
                .iter()
                .chain(std::iter::once(&decl.name))
                .map(|t| format!("`{t}`"))
                .collect::<Vec<_>>()
                .join(" reads ");
            return Err(invalid(format!(
                "view `{}` reads itself through other views ({chain}), so it has no records",
                decl.name
            )));
        }
        self.computing.borrow_mut().push(decl.name.clone());
        let records = self.computed_view(decl, v, filter);
        self.computing.borrow_mut().pop();
        records
    }

    fn computed_view(
        &self,
        decl: &TypeDecl,
        v: &lakelands_api::ontology::ViewDecl,
        filter: Option<&Expr>,
    ) -> Result<Vec<StoredRecord>> {
        let literals = match self.bound.borrow().get(&decl.name) {
            Some((_, literals)) => literals.clone(),
            None if v.params.is_empty() => BTreeMap::new(),
            // A view reached by no read that binds it: a walk's step, a
            // search.
            None => {
                return Err(invalid(format!(
                    "view `{}` needs parameter `{}` ({}), and only a read that names the view \
                     binds it",
                    decl.name,
                    v.params[0].name,
                    lakelands_core::value::kind_name(v.params[0].kind)
                )));
            }
        };
        let (members, measures) = self.view_source(
            &v.source,
            v.r#where.as_ref(),
            &v.group_by,
            &v.measures,
            &literals,
        )?;
        // A view record is keyed like any record: its group_by values, in
        // declared order, by the one key rule. Sources meet where their
        // keys are identical (044's identity of values, not 017's `=`).
        let keyed_by = TypeDecl {
            key: v.group_by.clone(),
            ..decl.clone()
        };
        // A view without `group_by` is one total, kept over no records too,
        // under the key reserved for it: the rule over no fields makes none.
        let (ungrouped, none) = (v.group_by.is_empty(), aggregate::Folds::default());
        // A group's values under the names the other side groups by, field for field.
        let renamed = |values: &aggregate::Row, from: &[FieldName], to: &[FieldName]| {
            let pairs = to.iter().zip(from);
            aggregate::Row::from_iter(pairs.map(|(to, from)| (to.clone(), values[from].clone())))
        };
        let key_of = |by: &BTreeMap<FieldName, Value>| {
            if ungrouped {
                Some(RecordKey::from(lakelands_api::key::UNGROUPED))
            } else {
                lakelands_core::log::record_key(&keyed_by, by)
            }
        };
        // A group keeps its values and its folds, never its members.
        let mut groups: BTreeMap<RecordKey, (aggregate::Row, aggregate::Folds)> = BTreeMap::new();
        for (by, folds) in members.groups() {
            if by.values().any(Value::is_null) {
                continue;
            }
            if let Some(key) = key_of(&by) {
                groups.insert(key, (by, folds));
            }
        }
        if ungrouped {
            groups
                .entry(RecordKey::from(lakelands_api::key::UNGROUPED))
                .or_default();
        }
        // One group's measures, seeing the values `given` by name.
        let measured = |folds: &aggregate::Folds,
                        by: &BTreeMap<FieldName, Value>,
                        measures: &[(FieldName, Expr)],
                        given| {
            aggregate::measures(folds, by, measures, given, Some(&self.settings), self.now)
                .map_err(|e| invalid(format!("view `{}`: {e}", decl.name)))
        };
        // Each merged source's measures by key; a key it lacks is measured
        // over its empty group below.
        let mut merged = Vec::with_capacity(v.merge.len());
        for m in &v.merge {
            let (members, measures) = self.view_source(
                &m.source,
                m.r#where.as_ref(),
                &m.group_by,
                &m.measures,
                &literals,
            )?;
            let mut at = BTreeMap::new();
            for (own, folds) in members.groups() {
                if own.values().any(Value::is_null) {
                    continue;
                }
                let by = renamed(&own, &m.group_by, &v.group_by);
                let Some(key) = key_of(&by) else {
                    continue;
                };
                let values = measured(&folds, &own, &measures, BTreeMap::new())?;
                groups
                    .entry(key.clone())
                    .or_insert_with(|| (by, none.clone()));
                at.insert(key, values);
            }
            merged.push((m, measures, at));
        }
        let mut out = Vec::new();
        for (key, (by, folds)) in groups {
            let mut given = BTreeMap::new();
            for (m, measures, at) in &merged {
                let values = match at.get(&key) {
                    Some(values) => values.clone(),
                    None => {
                        let own = renamed(&by, &v.group_by, &m.group_by);
                        measured(&none, &own, measures, BTreeMap::new())?
                    }
                };
                given.extend(values);
            }
            let values = measured(&folds, &by, &measures, given)?;
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

    /// One source of a view, read as a read of that source: the records
    /// its `where` covers, the parameters bound, folded into their groups
    /// as they are read, and its measures, after
    /// the principal is found to read every field they and the grouping
    /// read (024).
    fn view_source(
        &self,
        source: &TypeName,
        filter: Option<&lakelands_api::expr::Condition>,
        group_by: &[FieldName],
        measures: &[lakelands_api::ontology::Measure],
        literals: &BTreeMap<String, Literal>,
    ) -> Result<Source> {
        let source = self.ontology.ty(source)?.clone();
        let bound = |e: Expr| expr::bind_params(&e, literals);
        let mut probed: Vec<FieldName> = group_by.to_vec();
        let where_ = filter
            .map(|w| condition(w, Place::View, "view").map(bound))
            .transpose()?;
        if let Some(w) = &where_ {
            probed.extend(core_derive::reads(w));
        }
        let measures: Vec<(FieldName, Expr)> = measures
            .iter()
            .map(|m| {
                let e = condition(&m.expr, Place::Group, "measure")?;
                Ok((m.name.clone(), bound(e)))
            })
            .collect::<Result<_>>()?;
        for (_, m) in &measures {
            probed.extend(
                core_derive::reads(m)
                    .into_iter()
                    .filter(|f| source.fields.iter().any(|d| &d.name == f)),
            );
        }
        // The probe is the view's own, so its refusal names nothing of the
        // source to a reader `describe` withholds the definition from (024).
        let unnamed = |e| match self.who.grants.schema {
            true => e,
            false => crate::writes::refused(Refusal::NotGranted),
        };
        rights::check_probe(self.who, &source, probed.iter()).map_err(unnamed)?;
        // A `where` that read a parameter is evaluated here, never handed to
        // an engine: a bound date is text, and its arithmetic is the
        // evaluator's.
        let (down, here) = match literals.is_empty() {
            true => (where_.as_ref(), None),
            false => (None, where_.as_ref()),
        };
        let keys = group_by.iter().map(|f| (f.clone(), None)).collect();
        let mut members = aggregate::Grouping::new(keys, &measures, self.now);
        self.each_member(&source, down, COUNT_PAGE, |r| {
            if here.is_none_or(|w| self.holds(w, &r.fields, Some(&r.meta))) {
                members.add(&r.fields, Some(&self.settings));
            }
            Ok(())
        })?;
        Ok((members, measures))
    }

    fn sort_values(order: &[Order], r: &StoredRecord) -> Vec<Value> {
        order
            .iter()
            .map(|o| core_engines::sort_value(r, o).unwrap_or(Value::Null))
            .collect()
    }

    /// The one ordering: sort values in turn, missing last either way, then
    /// the key ascending.
    fn compare(order: &[Order], a: &SortKey, b: &SortKey) -> Ordering {
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

    /// The page a read asks for, at most `max` rows: the lake's
    /// `max_limit`, or for a whole-graph measure, which is computed whole
    /// for every page, its `max_graph_nodes` where that is larger (026).
    fn limit(&self, asked: Option<u32>, measure: bool) -> Result<usize> {
        let limits = &self.lake.limits;
        let max = match measure {
            true => limits.max_limit.max(limits.max_graph_nodes),
            false => limits.max_limit,
        };
        let l = asked.unwrap_or(limits.default_limit);
        if l == 0 || l > max {
            return Err(invalid(format!("a limit is between 1 and {max}")));
        }
        Ok(l as usize)
    }

    /// `find`.
    pub fn find(&self, ty: &TypeName, find: &Find) -> Result<Found> {
        let decl = self.visible_type(ty)?.clone();
        let params = self.bind_params(&decl, &find.params)?;
        for o in &find.order_by {
            if let Some((field, message)) = o.misfit() {
                let reason = Refusal::Invalid { message };
                return Err(Error::Refused { field, reason });
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
        let skip = self.offset(find)?;
        if let Some(a) = &find.aggregate {
            // Counting alone holds no record: the count answers it.
            let counts = a.group_by.is_empty()
                && a.measures
                    .iter()
                    .all(|m| m.func == AggFunc::Count && m.field.is_none());
            if counts && !a.measures.is_empty() {
                let n = Value::String(self.count(&decl, filter.as_ref())?.to_string());
                let values = a.measures.iter().map(|m| (m.name.clone(), n.clone()));
                return Ok(Found::Groups {
                    rows: vec![Group {
                        by: BTreeMap::new(),
                        values: values.collect(),
                    }],
                });
            }
            return self.aggregated(&decl, filter.as_ref(), a);
        }
        let limit = self.limit(find.limit, false)?;
        let find_read = read_of((&find.filter, &find.order_by, find.at.valid_at));
        let after = match &find.cursor {
            Some(c) => {
                let state = resume(c, "find", Some(ty), &params)?;
                let after = state.after.clone();
                // A cursor holds one sort value per key of the order its
                // first page was read in; under another number of keys it
                // places no page, and is refused before anything compares.
                if after
                    .as_ref()
                    .is_some_and(|(v, _)| v.len() != find.order_by.len())
                {
                    return Err(invalid(
                        "the cursor was handed out for another order; continue it with the \
                         `order_by` its first page was read with",
                    ));
                }
                same_read(&state, &find_read)?;
                after
            }
            None => asked,
        };
        let row = |r: StoredRecord| ((Self::sort_values(&find.order_by, &r), r.key.clone()), r);
        let mut keyed: Vec<(SortKey, StoredRecord)> = if decl.view.is_some() {
            // A view's records are computed from its source, whole.
            let all = self.records(&decl, filter.as_ref())?;
            let mut keyed: Vec<_> = all.into_iter().map(row).collect();
            keyed.sort_by(|a, b| Self::compare(&find.order_by, &a.0, &b.0));
            if let Some(after) = &after {
                keyed.retain(|(k, _)| Self::compare(&find.order_by, k, after) == Ordering::Greater);
            }
            keyed.drain(..skip.min(keyed.len()));
            keyed
        } else {
            let after = self.skipped(&decl, filter.as_ref(), &find.order_by, after, skip)?;
            let rows = self.page(&decl, filter.as_ref(), &find.order_by, after, limit + 1)?;
            rows.into_iter().map(row).collect()
        };
        let more = keyed.len() > limit;
        keyed.truncate(limit);
        let next_cursor = keyed.last().filter(|_| more).map(|(k, _)| {
            encode(&CursorState {
                at: self.position,
                after: Some(k.clone()),
                offset: 0,
                params: params.clone(),
                door: "find".into(),
                ty: Some(ty.clone()),
                of: find_read.clone(),
            })
        });
        let rows = keyed
            .into_iter()
            .filter_map(|(_, r)| rights::visible(self.who, &decl, &r, find.with_meta))
            .collect();
        // Counted at the same position, so every page of a walk agrees.
        let total = (find.total)
            .then(|| self.count(&decl, filter.as_ref()))
            .transpose()?;
        Ok(Found::Records {
            rows,
            next_cursor,
            total,
        })
    }

    /// The rows `find` skips before its page: none unless it names an
    /// `offset`, which is refused beside a cursor, an `after` or an
    /// aggregate (each already says where the answer starts, or has no
    /// rows), and past the lake's `max_offset` as too large; `total` is
    /// refused beside an aggregate too
    /// (`docs/decisions/045-a-table-reads-a-page-and-its-total.md`).
    fn offset(&self, find: &Find) -> Result<usize> {
        if find.total && find.aggregate.is_some() {
            return Err(invalid(
                "`total` counts the records a page lists; an `aggregate` answers groups, so \
                 give one",
            ));
        }
        let Some(skip) = find.offset else {
            return Ok(0);
        };
        let beside = [
            (find.cursor.is_some(), "cursor"),
            (find.after.is_some(), "after"),
            (find.aggregate.is_some(), "aggregate"),
        ];
        if let Some((_, other)) = beside.iter().find(|(given, _)| *given) {
            return Err(invalid(format!(
                "`offset` and `{other}` both say where the answer starts; give one"
            )));
        }
        let max = self.lake.limits.max_offset;
        if skip > max {
            return Err(Error::Refused {
                field: Some("offset".into()),
                reason: Refusal::TooLarge { max },
            });
        }
        Ok(skip as usize)
    }

    /// The sort position after the first `skip` rows of a stored type in
    /// `order` after `after`, read a page at a time and dropped, so an
    /// offset holds a page whatever it skips.
    fn skipped(
        &self,
        decl: &TypeDecl,
        filter: Option<&Expr>,
        order: &[Order],
        mut after: Option<SortKey>,
        mut skip: usize,
    ) -> Result<Option<SortKey>> {
        while skip > 0 {
            let want = skip.min(COUNT_PAGE);
            let rows = self.page(decl, filter, order, after.clone(), want)?;
            if let Some(last) = rows.last() {
                after = Some((Self::sort_values(order, last), last.key.clone()));
            }
            if rows.len() < want {
                break;
            }
            skip -= want;
        }
        Ok(after)
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
            let unfit = |why: String| {
                let message = format!("`after.values[{i}]` does not fit `{}`: {why}", o.field);
                crate::declare::invalid_field(&o.field, message)
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

    /// `find`'s aggregate over the records `filter` holds for, folded into
    /// their groups as they are read.
    fn aggregated(&self, decl: &TypeDecl, filter: Option<&Expr>, a: &Aggregate) -> Result<Found> {
        let named = |e| invalid(format!("aggregate over `{}`: {e}", decl.name));
        let mut grouping = aggregate::grouping(a, self.now).map_err(named)?;
        self.each_member(decl, filter, COUNT_PAGE, |r| {
            grouping.add(&r.fields, None);
            Ok(())
        })?;
        let mut groups = aggregate::answer(grouping, a).map_err(named)?;
        // A sum, minimum or maximum of a decimal field whose value type
        // declares a scale answers at that scale, so a sum over nothing is
        // `0.00` and not `0`.
        for m in &a.measures {
            if !matches!(m.func, AggFunc::Sum | AggFunc::Min | AggFunc::Max) {
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
        self.bind_params(&decl, &get.params)?;
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
            self.view.get(ty, &get.key)?.filter(|r| self.live(&decl, r))
        };
        stored
            .and_then(|r| rights::visible(self.who, &decl, &r, get.with_meta))
            .ok_or_else(missing)
    }

    /// `proposals`: every proposal for a reviewer, one's own otherwise.
    pub fn proposals(&self, filter: &ProposalFilter) -> Result<Vec<Proposal>> {
        let limit = self.limit(filter.limit, false)?;
        let offset = match &filter.cursor {
            Some(c) => resume(c, "proposals", None, &Params::new())?.offset as usize,
            None => 0,
        };
        // The kind and the type are answered on the entry as its reader is
        // shown it, so they are asked behind the trim, not of the engine.
        let all = self.view.proposals(&ProposalFilter {
            limit: Some(u32::MAX),
            cursor: None,
            kind: None,
            ty: None,
            ..filter.clone()
        })?;
        let me = self.who.writer();
        let mut mine: Vec<Proposal> = all
            .into_iter()
            .filter(|p| self.who.grants.review || p.proposer == me)
            .collect();
        let verb_on = |v: &VerbName| self.ontology.verb(v).ok().map(|d| d.on.clone());
        for p in &mut mine {
            let logged = p.write.take();
            let shown = logged.clone();
            p.write = shown.and_then(|mut w| self.known_shape(&mut w).then_some(w));
            p.more = p.write != logged;
        }
        mine.retain(|p| core_engines::matches(p, filter, verb_on));
        Ok(ordered_proposals(mine, &filter.order_by)?
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect())
    }

    /// A proposed write as its reader may know it (013 and 024, extended):
    /// a value where a read of the record would show it, a name where
    /// `describe` would. False when the reader may see nothing of it: a
    /// write to a type it may not see, or to a record it is not answered.
    /// The store logs a migration and a redeclaration completed with the
    /// fields hidden from their author, who reads its own proposal here.
    fn known_shape(&self, write: &mut ProposedWrite) -> bool {
        let read = |ty: &TypeName, field: &FieldName| {
            let t = self.visible_type(ty).ok();
            let f = t.and_then(|t| Some((t, t.fields.iter().find(|f| &f.name == field)?)));
            f.is_some_and(|(t, f)| rights::field_access(self.who, t, f).reads())
        };
        // A record the reader is not answered: restricted, without the grant.
        let answered = |ty: &TypeName, key: &RecordKey| {
            let stored = self.view.get(ty, key).ok().flatten();
            self.visible_type(ty).is_ok()
                && (self.who.grants.restricted || !stored.is_some_and(|r| r.restricted))
        };
        match write {
            // A field the reader may not know of and a masked one both
            // leave the patch: it holds values, and no place for a name.
            ProposedWrite::Upsert { ty, key, patch, .. } => {
                patch.retain(|field, _| read(ty, field));
                answered(ty, key)
            }
            ProposedWrite::Retire { ty, key, .. } => answered(ty, key),
            // A payload is what the verb's fields become: whole to a reader
            // who may read each field the verb sets, or see the type an
            // executor acts on, and empty otherwise. The verb's name is its
            // type's.
            ProposedWrite::Call {
                verb, key, payload, ..
            } => {
                let Ok(v) = self.ontology.verb(verb) else {
                    return false;
                };
                let whole = match &v.effect {
                    VerbEffect::Write { set } => set.iter().all(|a| read(&v.on, &a.field)),
                    VerbEffect::External { .. } => true,
                };
                if !whole {
                    payload.clear();
                }
                match key {
                    Some(key) => answered(&v.on, key),
                    None => self.visible_type(&v.on).is_ok(),
                }
            }
            // The report names what the migration does to the type as it
            // stands, so a name is judged there; the totals, the hashes and
            // the report's id stay whole. A logged report is lossless, so
            // it names no record that would not convert or would collide.
            ProposedWrite::Migrate { migration, report } => {
                let Ok(was) = self.visible_type(&migration.to.name) else {
                    return false;
                };
                let sees = |ty: &TypeName| self.visible_type(ty).ok();
                rights::shown_report(self.who, was, migration, report, sees, answered);
                let (mut was, Migration { to, renames }) = (was.clone(), &mut **migration);
                rights::known_fields(self.who, &mut was);
                rights::known_fields(self.who, to);
                renames.retain(|(old, new)| rights::holds(&was, old) && rights::holds(to, new));
                true
            }
            // Each declaration as `describe` would show it once the bundle
            // is in force; one it would not show leaves the list.
            ProposedWrite::Declare { declarations } => {
                let proposed = declarations.iter().cloned().map(|declaration| Declared {
                    declaration,
                    version: 0,
                    schema_hash: None,
                    since: Position(0),
                });
                let after: Vec<Declared> = proposed.chain(self.declared()).collect();
                declarations.retain(|d| describe::listed(d, &after, self.who));
                for d in declarations.iter_mut() {
                    if let Declaration::Type(t) = d {
                        rights::known_fields(self.who, t);
                    }
                }
                !declarations.is_empty()
            }
            // A use says something of a field's values, so it is shown only
            // in a field the reader may read, of a type it may see (024);
            // the report's totals stay whole. Its `first` needs no such
            // care: a logged proposal's report fits, so it names no misfit.
            ProposedWrite::MigrateValueType { report, .. } => {
                report.fields.retain(|u| read(&u.ty, &u.field));
                true
            }
        }
    }

    /// `search`.
    pub fn search(&self, s: &Search) -> Result<Hits> {
        let decl = self.visible_type(&s.ty)?.clone();
        // A view's records are computed on read and no index holds them, so
        // a search could only answer nothing (018).
        if decl.view.is_some() {
            return Err(invalid(format!(
                "`{}` is a view, and a view is not searched: its records are computed on read \
                 and no index holds them",
                s.ty
            )));
        }
        if let Some((field, message)) = s.mode.misfit() {
            let reason = Refusal::Invalid { message };
            return Err(Error::Refused { field, reason });
        }
        let vectors = s.mode.vectors();
        let vector_fields: Vec<&FieldName> = vectors.iter().map(|v| v.0).collect();
        let text_fields = match &s.mode {
            SearchMode::Lexical { fields, .. } | SearchMode::Hybrid { fields, .. } => {
                fields.as_deref()
            }
            SearchMode::Vector { .. } => None,
        };
        rights::check_search(self.who, &decl, text_fields, &vector_fields)?;
        // A search that names no fields reads only the searchable fields
        // the principal knows of (024); none hidden, it reads them all.
        let known: Vec<FieldName>;
        let hidden = (decl.fields.iter())
            .any(|f| f.searchable && rights::field_access(self.who, &decl, f) == Access::None);
        let text_fields = match text_fields {
            None if hidden => {
                known = rights::searched_fields(self.who, &decl).cloned().collect();
                Some(known.as_slice())
            }
            named => named,
        };
        let filter = self.record_filter(&decl, s.filter.as_ref(), "where")?;
        let search_read = read_of((&s.mode, &s.filter, s.at.valid_at));
        let offset = match &s.cursor {
            Some(c) => {
                let state = resume(c, "search", Some(&s.ty), &Params::new())?;
                same_read(&state, &search_read)?;
                state.offset as usize
            }
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
        // A search without a query vector ranks its text alone; one over
        // several vector fields answers an index's set when any of them is
        // asked for an index.
        let (mut by_text, mut indexed) = (true, false);
        if !matches!(&s.mode, SearchMode::Hybrid { vector: None, .. }) {
            for field in &vector_fields {
                (by_text, indexed) = (false, indexed || self.indexed(&decl, field)?);
            }
        }
        let ranked_by = match (&s.mode, indexed) {
            (SearchMode::Lexical { .. }, _) => "lexical",
            (SearchMode::Hybrid { .. }, _) if by_text => {
                "lexical (no query vector and no embedding backend)"
            }
            (SearchMode::Hybrid { .. }, true) => "hybrid (index)",
            (SearchMode::Hybrid { .. }, false) => "hybrid",
            (SearchMode::Vector { .. }, true) => "index",
            (SearchMode::Vector { .. }, false) => "vector",
        }
        .to_owned();
        // The rankings a search fuses, each with its weight, in the order
        // they are summed (046): a hybrid search's text, then each vector
        // field. A search of one ranking fuses nothing.
        let mut lists: Vec<(f64, Ranking<'_>)> = Vec::new();
        if let (
            SearchMode::Hybrid {
                text, text_weight, ..
            },
            false,
        ) = (&s.mode, by_text)
        {
            lists.push((
                text_weight.unwrap_or(1.0),
                Box::new(move |pull| lexical(text, pull)),
            ));
        }
        for (field, v, weight) in vectors.iter().filter(|_| !by_text) {
            lists.push((
                weight.unwrap_or(1.0),
                Box::new(move |pull| vector(field, v, pull)),
            ));
        }
        let hits = match &s.mode {
            _ if lists.len() > 1 => {
                self.fused(&decl, filter.as_ref(), &lists, want, s.with_meta)?
            }
            mode => {
                // Candidates are pulled in growing batches until the page and
                // one more are visible, or the engine has no more: a filter,
                // `valid_at` or the grants never shorten a page silently.
                let mut pull = u32::try_from(want.saturating_mul(4).max(64)).unwrap_or(u32::MAX);
                loop {
                    let ranked = match mode {
                        SearchMode::Vector {
                            field, vector: v, ..
                        } => vector(field, v, pull)?,
                        SearchMode::Lexical { text, .. } | SearchMode::Hybrid { text, .. } => {
                            lexical(text, pull)?
                        }
                    };
                    let exhausted = ranked.len() < pull as usize;
                    let hits = self.visible_hits(&decl, filter.as_ref(), ranked, s.with_meta)?;
                    if hits.len() >= want || exhausted || pull == u32::MAX {
                        break hits;
                    }
                    pull = pull.saturating_mul(4);
                }
            }
        };
        let more = hits.len() > offset + k;
        let page: Vec<Hit> = hits.into_iter().skip(offset).take(k).collect();
        let next_cursor = more.then(|| {
            encode(&CursorState {
                at: self.position,
                after: None,
                offset: (offset + k) as u64,
                params: Params::new(),
                door: "search".into(),
                ty: Some(s.ty.clone()),
                of: search_read,
            })
        });
        Ok(Hits {
            hits: page,
            ranked_by,
            next_cursor,
        })
    }

    /// A fused search's hits: its lists fused by weighted reciprocal rank
    /// ([`fuse`]), each ranked among the hits this read may see and to the
    /// lake's `max_limit` of them, so a record the read may not see moves
    /// no score and every page reads one ranking (046). The lists
    /// are pulled in growing batches until the first `want` fused hits and
    /// their scores can no longer change.
    fn fused(
        &self,
        decl: &TypeDecl,
        filter: Option<&Expr>,
        lists: &[(f64, Ranking<'_>)],
        want: usize,
        with_meta: bool,
    ) -> Result<Vec<Hit>> {
        let window = self.lake.limits.max_limit as usize;
        // A list read is a scan of its index whatever it is asked for, so
        // the first asks for the whole window at once.
        let mut pull = u32::try_from(want.saturating_mul(4).max(window)).unwrap_or(u32::MAX);
        // At the head, a read that sees every live record and narrows by
        // nothing sees every candidate an engine ranks, so it reads no
        // record to rank them. That rests on two seam facts: `lexical`
        // ranks live records only, restricted ones included (the engine
        // seam's `LEXICAL_HITS`), and the fold takes a retired record's
        // vectors out of the index, which is kept at the head (the suite's
        // `VECTOR_LEAVES_THE_NEIGHBOURS`). `SAME_LIST_UNREAD` holds this
        // path to the one that reads.
        let open = self.at_head
            && self.who.grants.restricted
            && filter.is_none()
            && self.valid_at.is_none();
        loop {
            // Each list's visible candidates to the window, and what a
            // record it has not ranked yet could still gain from it:
            // nothing once the list is whole or its window full.
            let (mut keyed, mut gain) = (Vec::new(), Vec::new());
            for (weight, list) in lists {
                let candidates = list(pull)?;
                let whole = candidates.len() < pull as usize;
                let mut seen: Vec<(RecordKey, f64)> = if open {
                    candidates
                } else {
                    (self.visible_hits(decl, filter, candidates, false)?)
                        .into_iter()
                        .map(|h| (h.record.key, h.score))
                        .collect()
                };
                let done = whole || seen.len() >= window || pull == u32::MAX;
                seen.truncate(window);
                gain.push(if done {
                    0.0
                } else {
                    weight / (60.0 + seen.len() as f64)
                });
                keyed.push((*weight, seen));
            }
            let fused = fuse(&keyed);
            let ranked: Vec<BTreeSet<&RecordKey>> = (keyed.iter())
                .map(|(_, l)| l.iter().map(|(k, _)| k).collect())
                .collect();
            let slack = |key: &RecordKey| -> f64 {
                (ranked.iter().zip(&gain))
                    .filter(|(l, _)| !l.contains(key))
                    .map(|(_, g)| g)
                    .sum()
            };
            // Settled when the first `want` are ranked by every list that
            // can still rank them, and nothing behind them, seen or not,
            // could still reach the last of them.
            let unseen: f64 = gain.iter().sum();
            let settled = match fused.get(want.saturating_sub(1)) {
                None => unseen == 0.0,
                Some((_, last)) => {
                    fused[..want].iter().all(|(k, _)| slack(k) == 0.0)
                        && unseen < *last
                        && fused[want..].iter().all(|(k, s)| {
                            let g = slack(k);
                            g == 0.0 || s + g < *last
                        })
                }
            };
            if settled {
                let first = fused.into_iter().take(want).collect();
                return self.visible_hits(decl, filter, first, with_meta);
            }
            pull = pull.saturating_mul(4);
        }
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
        // One read for the batch, not one per candidate.
        let keys: Vec<RecordKey> = ranked.iter().map(|(k, _)| k.clone()).collect();
        let mut stored: BTreeMap<RecordKey, StoredRecord> =
            (self.view.get_many(&decl.name, &keys)?)
                .into_iter()
                .map(|r| (r.key.clone(), r))
                .collect();
        let mut hits = Vec::new();
        for (key, score) in ranked {
            let Some(r) = stored.remove(&key) else {
                continue;
            };
            if !self.live(decl, &r) {
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
        // Its steps as declared: a step naming an inverse is rewritten below.
        let mut p = p.clone();
        let p = &mut p;
        let start_decl = self.visible_type(&p.start.ty)?.clone();
        let params = self.bind_params(&start_decl, &p.start.params)?;
        let limit = self.limit(p.limit, measures::is_measure(&p.project))?;
        let max_depth = self.lake.limits.max_depth;
        if measures::is_measure(&p.project) {
            measures::check(p)?;
        }
        // The type each step reaches, checked against the declarations.
        let mut types = vec![start_decl.name.clone()];
        let mut filters = Vec::new();
        let measure = measures::is_measure(&p.project);
        let lenient = matches!(p.project, Projection::Graph | Projection::Components);
        for (i, step) in p.steps.iter_mut().enumerate() {
            // A measure's steps are the relations around its records, each
            // read from the start, not a chain.
            let here = if measure {
                start_decl.name.clone()
            } else {
                types.last().expect("the start is in").clone()
            };
            self.as_declared(&here, step);
            let reached = self.step_target(&here, &step.relation, step.walk, lenient)?;
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
            // A view with parameters is bound only by a read that names it
            // (018): a step that arrives at one other than the start has no
            // values to read it with, whatever records the walk would reach,
            // so the pattern is refused before any record is read.
            if reached != start_decl.name
                && let Some(param) = decl.view.as_ref().and_then(|v| v.params.first())
            {
                return Err(invalid(format!(
                    "step {} over `{}` arrives at view `{reached}`, which needs parameter `{}` \
                     ({}), and only a read that names the view binds it",
                    i + 1,
                    step.relation,
                    param.name,
                    lakelands_core::value::kind_name(param.kind)
                )));
            }
            filters.push(self.record_filter(&decl, step.filter.as_ref(), "step where")?);
            types.push(reached);
        }
        if measure {
            return self.measure(p, limit);
        }
        let hidden = Visible { reader: self };
        // The start.
        let starts: Vec<StoredRecord> = if let Some(key) = &p.start.key {
            if self.start_found(p, key)? {
                self.get_one(&p.start.ty, key)?.into_iter().collect()
            } else {
                Vec::new()
            }
        } else if let Some(mode) = &p.start.search {
            let keys = self.start_hits(p, mode)?;
            keys.iter()
                .filter_map(|k| self.view.get(&p.start.ty, k).ok().flatten())
                .collect()
        } else {
            let f = self.record_filter(&start_decl, p.start.filter.as_ref(), "start where")?;
            self.records(&start_decl, f.as_ref())?
        };
        let relations: Vec<RelationName> = p.steps.iter().map(|s| s.relation.clone()).collect();
        // A path.
        if let Some((to_ty, to_key)) = &p.to {
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
            core_engines::order_edges(&mut edges);
            return self.graph_result(nodes, edges, truncated);
        }
        // Rows are ordered and measured by their paths, `type.field`: each
        // probes its field as `find`'s order and aggregate do (024), so an
        // answer is never computed over values the principal may not read.
        let mut paths: Vec<String> = p.order_by.iter().map(|o| o.field.to_string()).collect();
        if let Projection::Aggregate(a) = &p.project {
            paths.extend(
                a.measures
                    .iter()
                    .filter_map(|m| Some(m.field.as_ref()?.to_string())),
            );
            for key in &a.group_by {
                if let lakelands_api::read::GroupKey::Field(f) = key {
                    paths.push(f.to_string());
                }
            }
        }
        self.probe_rows(&paths)?;
        // Rows: one per match.
        let mut rows: Vec<BTreeMap<String, Value>> = Vec::new();
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
            for binding in t.bindings {
                let mut row = BTreeMap::new();
                self.bind(&mut row, &from)?;
                for r in &binding {
                    self.bind(&mut row, &r.node)?;
                }
                rows.push(row);
            }
        }
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
        let offset = offset(p.cursor.as_ref(), p, &params)?;
        let rows = keyed.into_iter().map(|(_, r)| r).collect();
        Ok(self.rows_page(p, rows, offset, limit, params))
    }

    /// Whether `get` answers the record a pattern's start names by key,
    /// with the start's view parameters.
    fn start_found(&self, p: &Pattern, key: &RecordKey) -> Result<bool> {
        let get = Get {
            key: key.clone(),
            with_meta: false,
            at: ReadAt::default(),
            params: p.start.params.clone(),
        };
        match self.get(&p.start.ty, &get) {
            Ok(_) => Ok(true),
            Err(Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// The keys a pattern's start search finds, best first.
    fn start_hits(&self, p: &Pattern, mode: &SearchMode) -> Result<Vec<RecordKey>> {
        let s = Search {
            ty: p.start.ty.clone(),
            mode: mode.clone(),
            k: self.lake.limits.max_limit,
            cursor: None,
            filter: p.start.filter.clone(),
            with_meta: false,
            at: ReadAt::default(),
        };
        Ok(self
            .search(&s)?
            .hits
            .into_iter()
            .map(|h| h.record.key)
            .collect())
    }

    /// Refuse an order or a measure over a path of a pattern's rows whose
    /// field the principal may not read, or that the path's type does not
    /// declare, as `find` refuses one (024, 006). A path that names no
    /// declared type reads nothing and is left alone.
    fn probe_rows(&self, paths: &[String]) -> Result<()> {
        for path in paths {
            let Some((ty, field)) = path.rsplit_once('.') else {
                continue;
            };
            if let Ok(decl) = self.ontology.ty(&TypeName::from(ty)) {
                rights::check_probe(self.who, decl, [&FieldName::from(field)])?;
            }
        }
        Ok(())
    }

    /// Rewrite a step naming a relation's `inverse` to that relation,
    /// walked the other way: the inverse is the relation's name from its
    /// target, and the step's `walk` counts from the name it gives. A
    /// relation declared under the name wins, so a step is read as an
    /// inverse only when it walks no relation of that name from `here`.
    fn as_declared(&self, here: &TypeName, step: &mut Step) {
        let types = || self.ontology.types.values().map(|e| &e.decl);
        let (name, walk) = (&step.relation, step.walk);
        // Whether a relation leaves `here` (out) or reaches it (in) by the
        // way the step walks.
        let leaves = |t: &TypeDecl, w: Walk| &t.name == here && w != Walk::In;
        let reaches = |r: &RelationDecl, w: Walk| &r.target == here && w != Walk::Out;
        let flipped = match walk {
            Walk::Out => Walk::In,
            Walk::In => Walk::Out,
            Walk::Both => Walk::Both,
        };
        let walked = |t: &TypeDecl, r: &RelationDecl, w: Walk| leaves(t, w) || reaches(r, w);
        if types().any(|t| {
            t.relations
                .iter()
                .any(|r| &r.name == name && walked(t, r, walk))
        }) {
            return;
        }
        let inverse = types().find_map(|t| {
            t.relations
                .iter()
                .find(|r| r.inverse.as_ref() == Some(name) && walked(t, r, flipped))
        });
        if let Some(r) = inverse {
            step.relation = r.name.clone();
            step.walk = flipped;
        }
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
        let reaches = |r: &RelationDecl| &r.name == relation && &r.target == here;
        let mut owners = self.ontology.types.values().map(|e| &e.decl);
        let inn = owners
            .find(|t| t.relations.iter().any(reaches))
            .map(|t| t.name.clone());
        let found = match walk {
            Walk::Out => out,
            Walk::In => inn,
            Walk::Both => out.or(inn),
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

    /// The record a node names, as the principal may see it, if at all.
    fn shown(&self, (ty, key): &graph::Node) -> Result<Option<Record>> {
        let decl = self.ontology.ty(ty)?;
        Ok((self.get_one(ty, key)?).and_then(|r| rights::visible(self.who, decl, &r, false)))
    }

    fn bind(&self, row: &mut BTreeMap<String, Value>, node: &graph::Node) -> Result<()> {
        if let Some(rec) = self.shown(node)? {
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
        for node in &nodes {
            records.extend(self.shown(node)?);
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
    let mut keyed: Vec<(core_engines::Place<'_>, Proposal)> = proposals
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
            (core_engines::Place { values, key: "" }, p)
        })
        .collect();
    keyed.sort_by(|a, b| core_engines::cmp_places(&a.0, &b.0, order_by));
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
        // An owned relation leaves `near` for its target; a targeted one reaches it.
        let leaving = owned.iter().map(|x| (&x.target, *x, true));
        let reaching = targeted.iter().map(|&(owner, x)| (owner, x, false));
        for (other, x, out) in leaving.chain(reaching) {
            let pairs = &x.derived.as_ref().expect("derived").pairs;
            if let Some(equal) = equal(pairs, out) {
                for m in r.matching(other, equal)? {
                    let (this, that) = ((ty.clone(), key.clone()), (other.clone(), m.key));
                    let (from, to) = if out { (this, that) } else { (that, this) };
                    list.push(Edge {
                        from,
                        relation: x.name.clone(),
                        to,
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
                                &f.name == name && rights::field_access(r.who, decl, f).reads()
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
                if r.live(decl, &rec) {
                    shown.insert((rec.ty, rec.key));
                }
            }
        }
        Ok(nodes.iter().map(|n| shown.contains(n)).collect())
    }
}

/// Reciprocal rank fusion of ranked lists, each with its weight: a list
/// adds `weight / (60 + rank)` for every key it ranks (rank from 0, best
/// first), in the lists' order. Ties are broken by key, so the fused
/// order is the same on every engine. A search hands it only what the
/// read may see (046).
pub fn fuse(lists: &[(f64, Vec<(RecordKey, f64)>)]) -> Vec<(RecordKey, f64)> {
    const K: f64 = 60.0;
    let mut score: BTreeMap<RecordKey, f64> = BTreeMap::new();
    for (weight, list) in lists {
        for (rank, (key, _)) in list.iter().enumerate() {
            *score.entry(key.clone()).or_default() += weight / (K + rank as f64);
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
