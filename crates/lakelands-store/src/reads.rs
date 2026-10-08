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

use std::cell::{OnceCell, RefCell};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;

use lakelands_api::describe::{Kept, LakeDescription, NotKept, TypeDescription};
use lakelands_api::engine::{After, ProjectionView, Scan};
use lakelands_api::error::{Case, Names, Place, Said};
use lakelands_api::expr::{CmpOp, Expr, Func, Literal};
use lakelands_api::ontology::{
    FieldType, MatchPair, RelationDecl, StorageKind, TypeDecl, VerbEffect,
};
use lakelands_api::query::{
    Axis, Edge, Hit, Hits, Pattern, Projection, QueryResult, Search, SearchMode, Step, Walk,
};
use lakelands_api::read::{
    After as ReadAfter, AggFunc, Aggregate, Find, Found, Get, Group, Order, OrderBy, Params, ReadAt,
};
use lakelands_api::record::StoredRecord;
use lakelands_api::write::{Declaration, Proposal, ProposalFilter, ProposedWrite};
use lakelands_api::{
    Cursor, Error, FieldName, Missing, Position, Principal, Record, RecordKey, Refusal,
    RelationName, Result, Timestamp, TypeName, Value, VerbName,
};
use lakelands_core::expr::{self, Place as Eval, RecordScope, aggregate};
use lakelands_core::graph::{self, EdgeSource};
use lakelands_core::{
    canonical, derive as core_derive, describe, engines as core_engines, invalid, rights,
    valid_time,
};

use crate::declare::condition;
use crate::kept;
use crate::lake::{Lake, Shape};
use crate::ontology::Ontology;

/// Everything one read sees.
pub struct Reader<'a> {
    /// The lake.
    pub lake: &'a Lake,
    /// Who reads.
    pub who: &'a Principal,
    /// The projection at the read's position.
    pub view: Box<dyn ProjectionView + 'a>,
    /// What the lake declares then, shared with every read in its range.
    pub shape: Arc<Shape>,
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
    /// For a read as of a position by a holder of `only`, the reader with
    /// its grants followed to the head, where a search judges each
    /// candidate too (051).
    pub head_who: Option<Principal>,
}

mod measures;

/// A view's parameters as one read binds them: as given, in canonical
/// form, and as the literals `param()` is replaced by.
type Bound = (Params, BTreeMap<String, Literal>);

/// What a view reads of one source, once its reader is found to read it:
/// the source, its `where` and its measures, the parameters bound.
type Probed = (TypeDecl, Option<Expr>, Vec<(FieldName, Expr)>);

/// One source of a view as read: its members, folded, and its measures, bound.
type Source = (aggregate::Grouping, Vec<(FieldName, Expr)>);

/// A row's sort values and its tiebreak.
type SortKey = (Vec<Value>, RecordKey);

/// One ranked list of a search, read to the depth it is given: the
/// candidates best first, with their scores.
type Ranking<'a> = Box<dyn Fn(u32) -> Result<Vec<(RecordKey, f64)>> + 'a>;

/// A search as judged: its type, the fields its text is matched in if it
/// names them, and its `where`.
type Searched<'s> = (TypeDecl, Option<&'s [FieldName]>, Option<Expr>);

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

/// A view's parameter `p` that a read does not give: `unbound` where no
/// read gives it (018).
fn unbound(ty: &TypeName, p: &lakelands_api::ontology::ViewParam, unbound: bool) -> Case {
    let (ty, name, kind) = (ty.clone(), p.name.clone(), p.kind);
    Case::ParamMissing {
        ty,
        name,
        kind,
        unbound,
    }
}

/// Refuse a cursor handed out for another read through the same door:
/// another filter, order, search or pattern (018).
fn same_read(state: &CursorState, of: &str) -> Result<()> {
    if state.of != of {
        let door = state.door.clone();
        return Err(invalid(Case::CursorOtherRead { door }));
    }
    Ok(())
}

/// A cursor's state, when it was handed out for this door, this type and
/// these view parameters: a page continues only the answer it began, and
/// any other use is refused, naming what differs (018).
fn resume(c: &Cursor, door: &str, ty: Option<&TypeName>, params: &Params) -> Result<CursorState> {
    let state = decode(c)?;
    if state.door != door || state.ty.as_ref() != ty {
        let (was, was_ty) = (state.door.clone(), state.ty.clone().map(Box::new));
        let (door, ty) = (door.to_owned(), ty.cloned().map(Box::new));
        return Err(invalid(Case::CursorOtherDoor {
            was,
            was_ty,
            door,
            ty,
        }));
    }
    if &state.params != params {
        return Err(invalid(Case::CursorOtherParams));
    }
    Ok(state)
}

fn decode(c: &Cursor) -> Result<CursorState> {
    let bad = || invalid(Case::CursorForeign);
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

/// A group's fields as a view's record holds them: its values, and each
/// measure landed in the kind the view type declares for it, nulls left
/// out; the computation's and the kept fold's.
pub(crate) fn landed(
    decl: &TypeDecl,
    mut fields: aggregate::Row,
    measured: impl IntoIterator<Item = (FieldName, Value)>,
) -> aggregate::Row {
    for (name, v) in measured {
        let landed = match decl.fields.iter().find(|f| f.name == name) {
            Some(f) => expr::eval::to_json(&expr::eval::from_json(&v), f.ty.kind, None)
                .unwrap_or(Value::Null),
            None => v,
        };
        fields.insert(name, landed);
    }
    fields.retain(|_, v| !v.is_null());
    fields
}

/// Whether a condition holds over a record's fields and their provenance,
/// under the lake's settings and a clock: a read's `where`, and a kept
/// view's as the fold evaluates it for a member.
pub(crate) fn holds(
    e: &Expr,
    fields: &BTreeMap<FieldName, Value>,
    meta: Option<&BTreeMap<FieldName, lakelands_api::FieldMeta>>,
    settings: &BTreeMap<String, Value>,
    now: Timestamp,
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
            settings: Some(settings),
            now,
        },
        meta,
    };
    expr::holds(e, &scope).unwrap_or(false)
}

impl<'a> Reader<'a> {
    /// Open a read at `at`, of `view` and its `shape`.
    pub fn open(
        lake: &'a Lake,
        who: &'a Principal,
        at: ReadAt,
        view: Box<dyn ProjectionView + 'a>,
        shape: Arc<Shape>,
    ) -> Result<Reader<'a>> {
        let position = view.position().unwrap_or(Position::GENESIS);
        Ok(Reader {
            lake,
            who,
            view,
            shape,
            at_head: at.as_of.is_none(),
            position,
            valid_at: at.valid_at,
            now: crate::lake::wall_clock(),
            computed: RefCell::default(),
            bound: RefCell::default(),
            computing: RefCell::default(),
            head_who: None,
        })
    }

    /// The records of `ty` with these keys, at this read. A computed
    /// view's come from its computation, any other type's and a kept
    /// view's from the engine.
    fn get_many(&self, ty: &TypeName, keys: &[RecordKey]) -> Result<Vec<StoredRecord>> {
        // A kept view's records are read by key, never computed whole.
        let decl = self.shape.ontology.ty(ty).ok();
        if let Some(kept) = decl.map(|d| self.kept(d)).transpose()?.flatten() {
            return self.kept_by_key(ty, kept, keys);
        }
        let Some(all) = self.computed(ty)? else {
            return Ok(self.at_read(ty, self.view.get_many(ty, keys)?));
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

    /// Records of `ty` as an engine holds them, as this read answers them:
    /// each field computed at the read computed over the record as stored,
    /// with this read's clock, the one a `where` reads.
    fn at_read(&self, ty: &TypeName, mut rows: Vec<StoredRecord>) -> Vec<StoredRecord> {
        let o = &self.shape.ontology;
        let Some(decl) = o
            .ty(ty)
            .ok()
            .filter(|t| t.fields.iter().any(|f| f.at_read()))
        else {
            return rows;
        };
        let (decl, value_types) = (
            crate::record::resolve_functions(decl, o),
            o.value_type_list(),
        );
        for r in &mut rows {
            core_derive::at_read(&decl, &value_types, r, &self.shape.settings, self.now);
        }
        rows
    }

    /// Whether the store evaluates `e` over records of `decl` itself: it
    /// needs the store, or reads a field computed at the read, which no
    /// engine holds.
    fn store_only(&self, decl: &TypeDecl, e: &Expr) -> bool {
        let at_read = |n: &&FieldName| decl.fields.iter().any(|f| &f.name == *n && f.at_read());
        needs_store(e) || e.fields_read().iter().any(at_read)
    }

    /// Refuse an order by a field computed at the read: it moves with the
    /// clock, so the order one page is read in would not hold to the next.
    /// Asked once the reader is known to read the field.
    fn steady_order(decl: &TypeDecl, field: &str) -> Result<()> {
        if decl
            .fields
            .iter()
            .any(|f| f.name.as_str() == field && f.at_read())
        {
            let (field, ty) = (field.into(), decl.name.clone());
            return Err(invalid(Case::OrderAtRead { field, ty }));
        }
        Ok(())
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
            return Ok(self.at_read(ty, self.view.scan(ty, &scan)?));
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
        let decl = match self.shape.ontology.ty(ty) {
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
            let names: Names = declared.iter().map(|p| &p.name).collect();
            let declared = (!names.0.is_empty()).then_some(names);
            let (ty, name) = (ty.clone(), k.clone());
            return Err(invalid(Case::ParamUnknown { ty, name, declared }));
        }
        let mut canonical = Params::new();
        let mut literals = BTreeMap::new();
        for p in declared {
            let Some(v) = given.get(&p.name) else {
                return Err(invalid(unbound(ty, p, false)));
            };
            let wrong = || {
                let (name, ty, kind, value) =
                    (p.name.clone(), ty.clone(), p.kind, Box::new(v.clone()));
                invalid(Case::ParamValue {
                    name,
                    ty,
                    kind,
                    value,
                })
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

    fn declared(&self) -> &[lakelands_api::engine::Declared] {
        &self.shape.declared
    }

    /// The declarations in force at `at`, if the lake can be read there.
    fn ontology_at(&self, at: Position) -> Option<Ontology> {
        let view = self.lake.engine.view(Some(at)).ok()?;
        Ontology::at(view.as_ref()).ok()
    }

    /// The type, if the principal may see it at all.
    fn visible_type(&self, ty: &TypeName) -> Result<&TypeDecl> {
        let decl = self.shape.ontology.ty(ty)?;
        if describe::one_type(ty, self.declared(), self.who).is_none() {
            return Err(Error::NotFound {
                missing: Missing::Type { name: ty.clone() },
            });
        }
        // A read at a valid time filters on the type's valid-time fields,
        // so it probes them as a `where` on them would (024): those it is
        // shown.
        if self.valid_at.is_some() {
            let timed = decl.valid_from.iter().chain(&decl.valid_to);
            rights::check_probe(self.who, decl, timed.filter(|n| self.knows(decl, n)))?;
        }
        Ok(decl)
    }

    /// Whether the reader may know of `decl`'s field `name`.
    fn knows(&self, decl: &TypeDecl, name: &FieldName) -> bool {
        let f = decl.fields.iter().find(|f| &f.name == name);
        !f.is_some_and(|f| rights::field_access(self.who, decl, f) == lakelands_api::Access::None)
    }

    /// The read's valid time for `decl`, as `describe` shows it to the
    /// reader (024, extended): `valid_at` where it is shown each bound. A
    /// bound over a field it may not know of is not named, so it is
    /// dropped: the other is read as a condition on the record instead,
    /// and with neither the type reads as one with no valid time.
    fn valid_for(&self, decl: &TypeDecl) -> (Option<Timestamp>, Option<Expr>) {
        let (Some(t), Some(from)) = (self.valid_at, &decl.valid_from) else {
            return (self.valid_at, None);
        };
        let to = decl.valid_to.as_ref();
        let shown = |n: &FieldName| self.knows(decl, n);
        // Built, never parsed: a field may be named as a keyword is.
        let path = |f: &FieldName| {
            Box::new(Expr::Path {
                path: vec![f.clone()],
            })
        };
        let at = || {
            Box::new(Expr::Lit {
                value: Literal::Text(t.to_string()),
            })
        };
        let condition = match (shown(from), to.filter(|to| shown(to))) {
            (true, Some(_)) => return (Some(t), None),
            (true, None) if to.is_none() => return (Some(t), None),
            (true, None) => Expr::Cmp {
                cmp: CmpOp::Le,
                lhs: path(from),
                rhs: at(),
            },
            (false, Some(to)) => Expr::Or {
                args: vec![
                    Expr::IsNull { arg: path(to) },
                    Expr::Cmp {
                        cmp: CmpOp::Gt,
                        lhs: path(to),
                        rhs: at(),
                    },
                ],
            },
            (false, None) => return (None, None),
        };
        (None, Some(condition))
    }

    /// [`Self::valid_for`]'s `valid_at`, which the engine applies.
    fn valid_at_for(&self, decl: &TypeDecl) -> Option<Timestamp> {
        self.valid_for(decl).0
    }

    /// Whether a read of `decl` is held to a valid time, whole or in part.
    fn timed(&self, decl: &TypeDecl) -> bool {
        self.valid_for(decl) != (None, None)
    }

    /// `filter` and, where a valid-time bound is dropped, the condition the
    /// other stands for ([`Self::valid_for`]).
    fn with_bound(&self, decl: &TypeDecl, filter: Option<&Expr>) -> Option<Expr> {
        let bound = self.valid_for(decl).1;
        match (filter.cloned(), bound) {
            (Some(f), Some(b)) => Some(Expr::And { args: vec![f, b] }),
            (f, b) => f.or(b),
        }
    }

    /// The type a read door names, as [`Self::visible_type`], refused where
    /// its reader's `only` on it cannot be applied (051).
    fn named_type(&self, ty: &TypeName) -> Result<&TypeDecl> {
        let decl = self.visible_type(ty)?;
        rights::check_only(self.who, decl)?;
        Ok(decl)
    }

    /// Whether the principal may see a stored record at this read, before
    /// masking: live, restricted only with the grant, admitted by its
    /// `only`, valid at `valid_at` when asked. A link record's ends are
    /// judged a page at a time ([`Self::admitted_ends`]).
    fn live(&self, decl: &TypeDecl, r: &StoredRecord) -> bool {
        self.live_by(decl, self.pairs(&decl.name).as_deref(), r)
    }

    /// [`Self::live`] with the pairs of `decl` worked out once for a batch.
    fn live_by(
        &self,
        decl: &TypeDecl,
        pairs: Option<&[(FieldName, Value)]>,
        r: &StoredRecord,
    ) -> bool {
        !r.retired
            && (self.who.grants.restricted || !r.restricted)
            && pairs.is_some_and(|p| rights::admitted(p, &r.fields))
            && (self.valid_at_for(decl))
                .is_none_or(|t| valid_time::record_holds(decl, &r.fields, t))
            && (self.valid_for(decl).1).is_none_or(|b| self.holds(&b, &r.fields, None))
    }

    /// The pairs a record of `ty` must hold for this reader (051), a kept
    /// name's those of its view; `None` where a grant they come from
    /// cannot be applied, which admits nothing.
    fn pairs(&self, ty: &TypeName) -> Option<Vec<(FieldName, Value)>> {
        if !rights::holds_only(self.who) {
            return Some(Vec::new());
        }
        let find = |t: &TypeName| self.shape.ontology.ty(t).ok();
        let pairs = rights::only(self.who, ty, &find).pairs().to_vec();
        let kept = ty.as_str().strip_prefix(lakelands_api::seam::KEPT_PREFIX);
        let Some(decl) = find(&kept.map_or_else(|| ty.clone(), TypeName::from)) else {
            return pairs.is_empty().then_some(pairs);
        };
        let source = decl.view.as_ref().and_then(|v| find(&v.source));
        let mut checked = std::iter::once(decl).chain(source);
        checked
            .all(|d| rights::check_only(self.who, d).is_ok())
            .then_some(pairs)
    }

    /// Whether a view's records are computed for this reader from the
    /// admitted records of what it reads, never served kept.
    fn computes(&self, ty: &TypeName) -> bool {
        let find = |t: &TypeName| self.shape.ontology.ty(t).ok();
        let only = rights::only(self.who, ty, &find);
        matches!(only, rights::Effective::ComputeOnly(_))
    }

    /// Whether this reader is admitted every record of `decl` as stored.
    fn whole(&self, decl: &TypeDecl) -> bool {
        self.pairs(&decl.name).is_some_and(|p| p.is_empty())
            && rights::held_ends(self.who, decl).is_empty()
    }

    /// Of link records of `decl`, which have every end this reader holds
    /// `only` on admitted: each such end type's records read once for the
    /// batch. Any other type's records all pass.
    fn admitted_ends(&self, decl: &TypeDecl, rows: &[StoredRecord]) -> Result<Vec<bool>> {
        ends_admitted(self.view.as_ref(), self.who, decl, rows, |end| {
            self.pairs(end)
        })
    }

    /// A read's condition with the filter of each set it names in the
    /// name's place, as the ontology read holds it. A set whose filter,
    /// with the sets it names, reads a field its reader is kept from is, to
    /// that reader, a set that is not declared (024): the refusal names
    /// nothing of the field.
    fn named_sets(
        &self,
        ty: &TypeName,
        c: &lakelands_api::expr::Condition,
        at: &[Place],
    ) -> Result<Expr> {
        let all = |n: &str| self.shape.ontology.set(n);
        let shown = |s: &&lakelands_api::write::ObjectSetDecl| {
            let filter = crate::declare::filter(&all, &s.of, &s.filter, Eval::Record, at);
            let reads = filter.map(|e| core_derive::reads(&e));
            let of = self.shape.ontology.ty(&s.of);
            of.is_ok_and(|t| {
                reads.is_ok_and(|r| rights::check_probe(self.who, t, r.iter()).is_ok())
            })
        };
        let set = |n: &str| self.shape.ontology.set(n).filter(shown);
        crate::declare::filter(&set, ty, c, Eval::Record, at)
    }

    /// A read's `where` over one record of `decl`, named `at`, after the
    /// principal is found to read every field it reads (024).
    fn record_filter(
        &self,
        decl: &TypeDecl,
        c: Option<&lakelands_api::expr::Condition>,
        at: &[Place],
    ) -> Result<Option<Expr>> {
        let Some(c) = c else { return Ok(None) };
        let e = self.named_sets(&decl.name, c, at)?;
        rights::check_probe(self.who, decl, core_derive::reads(&e).iter())?;
        crate::declare::evaluates(&e, decl, at)?;
        Ok(Some(e))
    }

    /// `describe`.
    pub fn describe(&self) -> LakeDescription {
        let mut lake = describe::lake(&self.lake.id, self.declared(), self.who);
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
            describe::one_type(ty, self.declared(), self.who).ok_or_else(|| Error::NotFound {
                missing: Missing::Type { name: ty.clone() },
            })?;
        self.name_sorted(&mut t);
        Ok(t)
    }

    /// Name in a type's lifecycle the fields the lake keeps an order index
    /// for and the vector fields it asks an index for, as its
    /// `sort_index.<type>` and `vector_index.<type>` settings ask, of those
    /// the principal sees; and for a view whether it is kept, as its
    /// `view_kept.<type>` setting asks.
    fn name_sorted(&self, t: &mut TypeDescription) {
        let Ok(decl) = self.shape.ontology.ty(&t.name) else {
            return;
        };
        let seen = |f: &FieldName| t.fields.iter().any(|d| d.name == *f);
        let settings = &self.shape.settings;
        // A view's order index is its kept records'.
        let ordered = match decl.view {
            Some(_) => lakelands_api::seam::kept_name(&t.name),
            None => t.name.clone(),
        };
        let setting = settings.get(&core_engines::sort_index_key(&ordered));
        t.lifecycle.sorted = core_engines::sorted_fields(&ordered, decl, setting);
        t.lifecycle.sorted.retain(seen);
        let setting = settings.get(&core_engines::vector_index_key(&t.name));
        t.lifecycle.indexed = core_engines::indexed_fields(decl, setting);
        t.lifecycle.indexed.retain(seen);
        if let Some(view) = decl.view.as_deref() {
            let key = format!("{}{}", lakelands_api::seam::VIEW_KEPT_SETTING, t.name);
            let (setting, shown) = (settings.get(&key), t.view.is_some());
            let declared = |ty: &TypeName| self.shape.ontology.ty(ty).ok();
            t.lifecycle.kept = Some(Kept::by_declaration(view, setting, shown, declared));
            // What the declarations leave open, the view's state at this
            // read answers: the store keeps views.
            let open = Kept::Computed {
                why: NotKept::NotKeptHere,
            };
            // To a holder of `only` a view is its own records, never every
            // owner's groups: whether those are exact is not told (051).
            if !self.whole(decl) || self.computes(&t.name) {
                t.lifecycle.kept = Some(Kept::Computed {
                    why: NotKept::OnlySome,
                });
            // Told to one the view's read would answer, of that reader's
            // face: a number of groups says something of their records.
            } else if t.lifecycle.kept.as_ref() == Some(&open) && self.probe_view(view).is_ok() {
                let face = usize::from(self.who.grants.restricted);
                t.lifecycle.kept = Some(match self.kept_read(&t.name, view, face) {
                    Ok(Some((_, 0))) => Kept::Kept,
                    Ok(Some((_, not_exact))) => Kept::Computed {
                        why: NotKept::NotWhole { not_exact },
                    },
                    _ => open,
                });
            }
            // An index is named where the view is kept, or asked to be and
            // can be, as `kept` tells this reader (lakelands#748).
            if t.lifecycle
                .kept
                .as_ref()
                .is_some_and(|k| !k.names_its_index())
            {
                t.lifecycle.sorted.clear();
            }
        }
    }

    fn holds(
        &self,
        e: &Expr,
        fields: &BTreeMap<FieldName, Value>,
        meta: Option<&BTreeMap<FieldName, lakelands_api::FieldMeta>>,
    ) -> bool {
        holds(e, fields, meta, &self.shape.settings, self.now)
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
        if let Some((stored, whole)) = self.kept(decl)? {
            // The engine counts the stored records; a group whose whole
            // face reads otherwise is counted by that face instead.
            let n = self.count(&stored, filter)?;
            let passes = |r: &StoredRecord| filter.is_none_or(|e| self.holds(e, &r.fields, None));
            let differ = self.differing(&stored, &decl.name, whole)?;
            let own = differ.iter().filter(|(r, _)| passes(r)).count() as u64;
            let faces = differ.iter().filter_map(|(_, w)| w.as_ref());
            let faces = faces.filter(|w| passes(w)).count() as u64;
            return Ok((n + faces).saturating_sub(own));
        }
        if let Some(v) = &decl.view {
            return Ok(self.view_records(decl, v, filter)?.len() as u64);
        }
        // A kept name's filtered count goes by the pages a read of it goes
        // by, whose order is checked there: the engine's own count pages by
        // a last key it does not check.
        let kept = lakelands_api::seam::is_kept_name(&decl.name);
        let Some(equal) = self.pairs(&decl.name) else {
            return Ok(0);
        };
        // A link's ends are judged a page at a time, never by the engine.
        let ends = !rights::held_ends(self.who, decl).is_empty();
        let bounded = self.with_bound(decl, filter);
        let filter = bounded.as_ref();
        if ends || filter.is_some_and(|e| kept || self.store_only(decl, e)) {
            let mut n = 0;
            self.each_member(decl, filter, COUNT_PAGE, |_| {
                n += 1;
                Ok(())
            })?;
            return Ok(n);
        }
        let scan = Scan {
            filter: filter.cloned(),
            equal,
            valid_at: self.valid_at_for(decl),
            restricted: self.who.grants.restricted,
            ..Default::default()
        };
        self.view.count(&decl.name, &scan)
    }

    /// The records [`Self::records`] would answer, handed to `each` one at
    /// a time in key order until it fails: a stored type's and a kept
    /// view's are read `page` at a time and never held together, a
    /// computed view's are computed first.
    fn each_member(
        &self,
        decl: &TypeDecl,
        filter: Option<&Expr>,
        page: usize,
        mut each: impl FnMut(&StoredRecord) -> Result<()>,
    ) -> Result<()> {
        if let Some(kept) = self.kept(decl)? {
            return self.each_kept(&decl.name, kept, filter, page, each);
        }
        if decl.view.is_some() {
            return self.records(decl, filter)?.iter().try_for_each(each);
        }
        self.each_stored(decl, filter, page, &mut each)
    }

    /// [`Self::each_member`] for a stored type or a kept name: its records
    /// read `page` at a time from the engine.
    fn each_stored(
        &self,
        decl: &TypeDecl,
        filter: Option<&Expr>,
        page: usize,
        each: &mut dyn FnMut(&StoredRecord) -> Result<()>,
    ) -> Result<()> {
        let mut after = None;
        loop {
            let rows = self.page(decl, filter, &[], after, page)?;
            rows.iter().try_for_each(&mut *each)?;
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
        let bounded = self.with_bound(decl, filter);
        let filter = bounded.as_ref();
        let push = filter.filter(|e| !self.store_only(decl, e)).cloned();
        let here = filter.filter(|_| push.is_none());
        let Some(equal) = self.pairs(&decl.name) else {
            return Ok(Vec::new());
        };
        let pairs = equal.clone();
        let mut scan = Scan {
            filter: push,
            equal,
            order_by: order.to_vec(),
            after: after.map(|(values, key)| After { values, key }),
            limit: u32::try_from(want).unwrap_or(u32::MAX),
            valid_at: self.valid_at_for(decl),
            restricted: self.who.grants.restricted,
            ..Default::default()
        };
        let mut out = Vec::new();
        loop {
            let rows = self.at_read(&decl.name, self.view.scan(&decl.name, &scan)?);
            // An engine answers a kept name as of an earlier position in
            // key order by its statement's plan alone, and this read goes
            // on from a page's last key: a page in another order would
            // skip or repeat a group without a word.
            if order.is_empty() && lakelands_api::seam::is_kept_name(&decl.name) {
                let mut last = scan.after.as_ref().map(|a| &a.key);
                for r in &rows {
                    if let Some(before) = last.filter(|k| **k >= r.key) {
                        let flaw = lakelands_api::error::Flaw::OutOfKeyOrder {
                            ty: decl.name.clone(),
                            key: r.key.clone(),
                            before: before.clone(),
                        };
                        let part = lakelands_api::error::Part::Projection;
                        return Err(lakelands_core::damaged(part, None, flaw));
                    }
                    last = Some(&r.key);
                }
            }
            let last_page = rows.len() < scan.limit as usize;
            if let Some(last) = rows.last() {
                scan.after = Some(After {
                    values: Self::sort_values(order, last),
                    key: last.key.clone(),
                });
            }
            let ends = self.admitted_ends(decl, &rows)?;
            for (r, _) in rows.into_iter().zip(ends).filter(|(_, ends)| *ends) {
                if self.live_by(decl, Some(&pairs), &r)
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
        let bounded = self.with_bound(decl, filter);
        let filter = bounded.as_ref();
        if let Some(at) = self.computing.borrow().iter().position(|t| *t == decl.name) {
            let chain = self.computing.borrow()[at..]
                .iter()
                .chain(std::iter::once(&decl.name))
                .collect();
            let ty = decl.name.clone();
            return Err(invalid(Case::ViewReadsItself { ty, chain }));
        }
        self.computing.borrow_mut().push(decl.name.clone());
        let records = match self.kept_as(decl, v) {
            Ok(Some(kept)) => {
                let mut all = Vec::new();
                let each = |r: &StoredRecord| {
                    all.push(r.clone());
                    Ok(())
                };
                (self.each_kept(&decl.name, kept, filter, COUNT_PAGE, each)).map(|()| all)
            }
            Ok(None) => self.computed_view(decl, v, filter),
            Err(e) => Err(e),
        };
        self.computing.borrow_mut().pop();
        // A holder is answered the groups its pairs admit (051).
        let pairs = self.pairs(&decl.name);
        let admitted = |r: &StoredRecord| {
            pairs
                .as_ref()
                .is_some_and(|p| rights::admitted(p, &r.fields))
        };
        records.map(|mut r| {
            r.retain(admitted);
            r
        })
    }

    /// The records of the kept view `view` that `filter` holds for, as
    /// this reader reads them, handed to `each` in key order until it
    /// fails: the kept records read `page` at a time, each shown with the
    /// reader's face, which a record carries whole, and held there to
    /// `filter` and the reader's pairs. It never holds the kept type, whose
    /// records carry the fold's state (lakelands#731). `kept` is what
    /// [`Self::kept_as`] answers. The record's fields are masked by the
    /// door, as a computed record's are.
    fn each_kept(
        &self,
        view: &TypeName,
        (stored, whole): (TypeDecl, bool),
        filter: Option<&Expr>,
        page: usize,
        mut each: impl FnMut(&StoredRecord) -> Result<()>,
    ) -> Result<()> {
        let pairs = self.pairs(&stored.name).unwrap_or_default();
        self.each_stored(
            &stored,
            None,
            page,
            &mut |r| match kept::shown(r.clone(), view, whole) {
                Some(r)
                    if filter.is_none_or(|e| self.holds(e, &r.fields, None))
                        && rights::admitted(&pairs, &r.fields) =>
                {
                    each(&r)
                }
                _ => Ok(()),
            },
        )
    }

    /// The records of the kept view `view` with these keys, as this reader
    /// reads them, `kept` as [`Self::kept_as`] answers it: each read by its
    /// key from the engine, as `get` reads one.
    fn kept_by_key(
        &self,
        view: &TypeName,
        (stored, whole): (TypeDecl, bool),
        keys: &[RecordKey],
    ) -> Result<Vec<StoredRecord>> {
        let held = self.view.get_many(&stored.name, keys)?;
        let pairs = self.pairs(&stored.name).unwrap_or_default();
        let live = held.into_iter().filter(|r| self.live(&stored, r));
        let shown = live.filter_map(|r| kept::shown(r, view, whole));
        Ok(shown
            .filter(|r| rights::admitted(&pairs, &r.fields))
            .collect())
    }

    /// The stored type a kept view's records are read as at this read, the
    /// computation's probes made, and whether the reader takes each
    /// record's whole face: the reader with the `restricted` grant, while
    /// a group's whole face reads otherwise than its open one. `None` where
    /// the read computes: the fold kept none at this position, they are not
    /// whole there, a group of the reader's face is not exact, or the read
    /// names a valid time, which filters the source's records and which a
    /// kept fold has no axis for. What depends on the reader is decided
    /// here, as the computation decides it: the probes of every source, in
    /// its order, and the face.
    fn kept_as(
        &self,
        decl: &TypeDecl,
        v: &lakelands_api::ontology::ViewDecl,
    ) -> Result<Option<(TypeDecl, bool)>> {
        let face = usize::from(self.who.grants.restricted);
        let state = match self.kept_read(&decl.name, v, face)? {
            Some((state, 0)) if !self.timed(decl) && !self.computes(&decl.name) => state,
            _ => return Ok(None),
        };
        self.probe_view(v)?;
        let stored = TypeDecl {
            name: lakelands_api::seam::kept_name(&decl.name),
            view: None,
            ..decl.clone()
        };
        Ok(Some((stored, face == 1 && state.differ > 0)))
    }

    /// [`Self::kept_as`] for a type that may be a view.
    fn kept(&self, decl: &TypeDecl) -> Result<Option<(TypeDecl, bool)>> {
        match &decl.view {
            Some(v) => self.kept_as(decl, v),
            None => Ok(None),
        }
    }

    /// The kept records of `view` whose whole face reads otherwise than
    /// their own fields, found by their mark, each with that face (none
    /// for a group on the open face alone, and none the reader's pairs do
    /// not admit: a pair on a measure is judged on both faces, as for a
    /// view read whole). There are as many as the
    /// state's `differ`; the text index finds them at the head, and a read
    /// at an earlier position scans the kept type for them. None unless the
    /// reader reads the `whole` face.
    fn differing(
        &self,
        stored: &TypeDecl,
        view: &TypeName,
        whole: bool,
    ) -> Result<Vec<(StoredRecord, Option<StoredRecord>)>> {
        let (true, Some(pairs)) = (whole, self.pairs(&stored.name)) else {
            return Ok(Vec::new());
        };
        let mut equal = vec![(FieldName::from(kept::DIFFERS), Value::from("1"))];
        equal.extend(pairs.iter().cloned());
        let scan = Scan {
            equal,
            limit: u32::MAX,
            valid_at: self.valid_at_for(stored),
            restricted: self.who.grants.restricted,
            ..Default::default()
        };
        let rows = self.view.scan(&stored.name, &scan)?;
        let admitted = |w: &StoredRecord| rights::admitted(&pairs, &w.fields);
        let face = |r: StoredRecord| (r.clone(), kept::shown(r, view, true).filter(admitted));
        Ok(rows.into_iter().map(face).collect())
    }

    /// A page of a kept view as its reader reads it, after `after`: the
    /// engine's page of the stored records, each of the open face; or, for
    /// the reader of the whole face, the engine's page of the records whose
    /// faces do not differ merged in the order with the whole faces of
    /// those that do (`differ`), filtered here. It holds `want` records and
    /// those faces.
    #[allow(clippy::too_many_arguments)]
    fn kept_rows(
        &self,
        stored: &TypeDecl,
        view: &TypeName,
        filter: Option<&Expr>,
        order: &[Order],
        differ: Option<&[StoredRecord]>,
        after: Option<SortKey>,
        want: usize,
    ) -> Result<Vec<StoredRecord>> {
        let Some(differ) = differ else {
            let rows = self.page(stored, filter, order, after, want)?;
            return Ok((rows.into_iter())
                .filter_map(|r| kept::shown(r, view, false))
                .collect());
        };
        let path = vec![FieldName::from(kept::DIFFERS)];
        let same = Expr::IsNull {
            arg: Box::new(Expr::Path { path }),
        };
        let args = filter.cloned().into_iter().chain([same]).collect();
        let rows = self.page(
            stored,
            Some(&Expr::And { args }),
            order,
            after.clone(),
            want,
        )?;
        let at = |r: &StoredRecord| (Self::sort_values(order, r), r.key.clone());
        let later = |r: &&StoredRecord| {
            filter.is_none_or(|e| self.holds(e, &r.fields, None))
                && after
                    .as_ref()
                    .is_none_or(|a| Self::compare(order, &at(r), a).is_gt())
        };
        let mut rows: Vec<StoredRecord> = (rows.into_iter())
            .filter_map(|r| kept::shown(r, view, true))
            .chain(differ.iter().filter(later).cloned())
            .collect();
        rows.sort_by(|a, b| Self::compare(order, &at(a), &at(b)));
        rows.truncate(want);
        Ok(rows)
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
            None => return Err(invalid(unbound(&decl.name, &v.params[0], true))),
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
        let settings = Some(&self.shape.settings);
        let measured = |folds: &aggregate::Folds,
                        by: &BTreeMap<FieldName, Value>,
                        measures: &[(FieldName, Expr)],
                        given| {
            aggregate::measures(folds, by, measures, given, settings, self.now).map_err(|e| {
                invalid(Said::from(e).within(Place::View {
                    name: decl.name.clone(),
                }))
            })
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
            out.push(StoredRecord {
                ty: decl.name.clone(),
                key,
                version: lakelands_api::Version(1),
                fields: landed(decl, by.clone(), values),
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
        let (source, where_, measures) =
            self.view_probe(source, filter, group_by, measures, literals)?;
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
                members.add(&r.fields, Some(&self.shape.settings));
            }
            Ok(())
        })?;
        Ok((members, measures))
    }

    /// The state of a kept view whose records are whole at this read, as
    /// are those of every view it reads through, and among them all the
    /// groups that are not exact for `face`: the outer folds an inner
    /// group as its record stands, so the outer is the computation's only
    /// while the inner is exact. `None` where the fold keeps no whole
    /// records of one of them: before it was made, or ended; and, with no
    /// state read, where no `view_kept.` setting asks for it.
    fn kept_read(
        &self,
        ty: &TypeName,
        v: &lakelands_api::ontology::ViewDecl,
        face: usize,
    ) -> Result<Option<(kept::State, u64)>> {
        let asked = format!("{}{ty}", lakelands_api::seam::VIEW_KEPT_SETTING);
        if !lakelands_api::seam::view_kept(self.shape.settings.get(&asked)) {
            return Ok(None);
        }
        let Some(state) = kept::state(self.view.as_ref(), ty)?.filter(|s| s.whole) else {
            return Ok(None);
        };
        let mut not_exact = state.not_exact[face];
        for source in std::iter::once(&v.source).chain(v.merge.iter().map(|m| &m.source)) {
            if let Some(inner) = self.shape.ontology.ty(source)?.view.as_deref() {
                let Some((_, n)) = self.kept_read(source, inner, face)? else {
                    return Ok(None);
                };
                not_exact += n;
            }
        }
        Ok(Some((state, not_exact)))
    }

    /// The probes computing a view without parameters would make, in the
    /// computation's order and with its refusals, and nothing read: each
    /// source's, its own first, and after each those of a source that is
    /// itself a view, which computing would read there.
    fn probe_view(&self, v: &lakelands_api::ontology::ViewDecl) -> Result<()> {
        let own = (&v.source, v.r#where.as_ref(), &v.group_by, &v.measures);
        let merged = v.merge.iter();
        let merged = merged.map(|m| (&m.source, m.r#where.as_ref(), &m.group_by, &m.measures));
        for (source, filter, group_by, measures) in std::iter::once(own).chain(merged) {
            let (source, ..) =
                self.view_probe(source, filter, group_by, measures, &BTreeMap::new())?;
            if let Some(inner) = &source.view {
                self.probe_view(inner)?;
            }
        }
        Ok(())
    }

    /// What a view reads of one source, the parameters bound: the source,
    /// its `where` and its measures, after the principal is found to read
    /// every field they and the grouping read (024).
    fn view_probe(
        &self,
        source: &TypeName,
        filter: Option<&lakelands_api::expr::Condition>,
        group_by: &[FieldName],
        measures: &[lakelands_api::ontology::Measure],
        literals: &BTreeMap<String, Literal>,
    ) -> Result<Probed> {
        let source = self.shape.ontology.ty(source)?.clone();
        let bound = |e: Expr| expr::bind_params(&e, literals);
        let mut probed: Vec<FieldName> = group_by.to_vec();
        let where_ = filter
            .map(|w| condition(w, Eval::View, &[Place::Where]).map(bound))
            .transpose()?;
        if let Some(w) = &where_ {
            probed.extend(core_derive::reads(w));
        }
        let measures: Vec<(FieldName, Expr)> = measures
            .iter()
            .map(|m| {
                let at = [Place::Measure {
                    name: m.name.clone(),
                }];
                let e = condition(&m.expr, Eval::Group, &at)?;
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
        Ok((source, where_, measures))
    }

    fn sort_values(order: &[Order], r: &StoredRecord) -> Vec<Value> {
        order
            .iter()
            .map(|o| core_engines::sort_value(r, o).unwrap_or(Value::Null))
            .collect()
    }

    /// The one ordering, the engines' own: sort values in turn, missing
    /// last either way, then the key ascending.
    fn compare(order: &[Order], a: &SortKey, b: &SortKey) -> Ordering {
        fn some(v: &Value) -> Option<&Value> {
            (!v.is_null()).then_some(v)
        }
        let keys = order.iter().zip(a.0.iter().zip(&b.0));
        keys.map(|(o, (x, y))| core_engines::cmp_key(some(x), some(y), o.dir))
            .find(|c| c.is_ne())
            .unwrap_or_else(|| a.1.as_str().cmp(b.1.as_str()))
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
        // A read that names no limit asks for nothing out of bounds.
        let l = asked.unwrap_or(limits.default_limit.min(max));
        if l == 0 || l > max {
            return Err(invalid(Case::LimitRange { max }));
        }
        Ok(l as usize)
    }

    /// `find`.
    pub fn find(&self, ty: &TypeName, find: &Find) -> Result<Found> {
        let decl = self.named_type(ty)?.clone();
        let params = self.bind_params(&decl, &find.params)?;
        for o in &find.order_by {
            if let Some((field, case)) = o.misfit() {
                let reason = Refusal::Invalid(Box::new(case.into()));
                return Err(Error::Refused { field, reason });
            }
            // A view's records are computed at the read's position; none
            // has an event of its own to order by.
            if o.by == OrderBy::Latest && decl.view.is_some() {
                let ty = Some(ty.clone());
                return Err(invalid(Case::LatestNoChange { ty }));
            }
        }
        let filter = find
            .filter
            .as_ref()
            .map(|c| self.named_sets(&decl.name, c, &[Place::Where]))
            .transpose()?;
        let mut probed: Vec<FieldName> = filter
            .as_ref()
            .map(|e| core_derive::reads(e).into_iter().collect())
            .unwrap_or_default();
        let mut asked: Vec<(Expr, Vec<Place>)> = Vec::new();
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
            let keys = aggregate::group_keys(&a.group_by).map_err(invalid)?;
            for (name, expr) in keys {
                match expr {
                    None => probed.push(name),
                    Some(e) => {
                        probed.extend(core_derive::reads(&e));
                        asked.push((e, vec![Place::Group { name }]));
                    }
                }
            }
        }
        rights::check_probe(self.who, &decl, probed.iter())?;
        for o in find.order_by.iter().filter(|o| o.by != OrderBy::Latest) {
            Self::steady_order(&decl, o.field.as_str())?;
        }
        // What can never be evaluated is refused, once the reader is known
        // to read the fields it names (028, extended).
        for (e, at) in filter
            .iter()
            .map(|e| (e.clone(), vec![Place::Where]))
            .chain(asked)
        {
            crate::declare::evaluates(&e, &decl, &at)?;
        }
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
                    return Err(invalid(Case::CursorOtherOrder));
                }
                same_read(&state, &find_read)?;
                after
            }
            None => asked,
        };
        let row = |r: StoredRecord| ((Self::sort_values(&find.order_by, &r), r.key.clone()), r);
        let mut keyed: Vec<(SortKey, StoredRecord)> = if let Some((stored, whole)) =
            self.kept(&decl)?
        {
            // A kept view's page is the engine's, as a stored type's.
            let faces = self.differing(&stored, &decl.name, whole)?;
            let faces = faces.into_iter().filter_map(|(_, w)| w);
            let differ = whole.then(|| faces.collect::<Vec<_>>());
            let page = |after, want| {
                let (f, o) = (filter.as_ref(), &find.order_by);
                self.kept_rows(&stored, &decl.name, f, o, differ.as_deref(), after, want)
            };
            let after = Self::skipped(&find.order_by, after, skip, &page)?;
            page(after, limit + 1)?.into_iter().map(row).collect()
        } else if decl.view.is_some() {
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
            let page = |after, want| self.page(&decl, filter.as_ref(), &find.order_by, after, want);
            let after = Self::skipped(&find.order_by, after, skip, &page)?;
            let rows = page(after, limit + 1)?;
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
            return Err(invalid(Case::TotalWithAggregate));
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
            let other = (*other).to_owned();
            return Err(invalid(Case::OffsetBeside { other }));
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

    /// The sort position after the first `skip` rows `page` reads in
    /// `order` after `after`, read a page at a time and dropped, so an
    /// offset holds a page whatever it skips.
    fn skipped(
        order: &[Order],
        mut after: Option<SortKey>,
        mut skip: usize,
        page: &impl Fn(Option<SortKey>, usize) -> Result<Vec<StoredRecord>>,
    ) -> Result<Option<SortKey>> {
        while skip > 0 {
            let want = skip.min(COUNT_PAGE);
            let rows = page(after.clone(), want)?;
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
            return Err(invalid(Case::AfterWithCursor));
        }
        if find.aggregate.is_some() {
            return Err(invalid(Case::AfterWithAggregate));
        }
        if a.values.len() != find.order_by.len() {
            let (values, keys) = (a.values.len(), find.order_by.len());
            return Err(invalid(Case::AfterCount { values, keys }));
        }
        let mut values = Vec::with_capacity(a.values.len());
        for (i, (v, o)) in a.values.iter().zip(&find.order_by).enumerate() {
            let unfit = |why: Case| {
                let (index, field, why) = (i, o.field.clone(), Box::new(why));
                crate::declare::invalid_field(&o.field, Case::AfterUnfit { index, field, why })
            };
            let v = match (o.by, v) {
                (_, Value::Null) => Value::Null,
                (OrderBy::Latest, Value::Number(n)) if n.is_u64() => v.clone(),
                (OrderBy::Latest, _) => {
                    return Err(unfit(Case::PositionNotInteger));
                }
                (OrderBy::KnownAt | OrderBy::SeenAt, Value::String(t)) => {
                    let value = || Case::NotDateTime { value: t.clone() };
                    let t = Timestamp::parse(t).map_err(|_| unfit(value()))?;
                    Value::String(t.to_string())
                }
                (OrderBy::KnownAt | OrderBy::SeenAt, _) => {
                    return Err(unfit(Case::InstantNotText));
                }
                (OrderBy::Value, v) => match decl.fields.iter().find(|f| f.name == o.field) {
                    Some(f) => {
                        let vt =
                            f.ty.value_type
                                .as_ref()
                                .and_then(|n| self.shape.ontology.value_types.get(n.as_str()))
                                .map(|e| &e.decl);
                        lakelands_core::value::check(v, &f.ty, vt).map_err(unfit)?
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
        let named = |e: Said| {
            invalid(e.within(Place::Aggregate {
                name: decl.name.clone(),
            }))
        };
        let mut grouping = aggregate::grouping(a, self.now).map_err(named)?;
        self.each_member(decl, filter, COUNT_PAGE, |r| {
            grouping.add(&r.fields, Some(&self.shape.settings));
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
                .and_then(|vt| self.shape.ontology.value_types.get(vt.as_str()))
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
        let decl = self.named_type(ty)?.clone();
        self.bind_params(&decl, &get.params)?;
        let missing = || Error::NotFound {
            missing: Missing::Record {
                ty: ty.clone(),
                key: get.key.clone(),
            },
        };
        let stored = if let Some(kept) = self.kept(&decl)? {
            self.kept_by_key(ty, kept, std::slice::from_ref(&get.key))?
                .pop()
        } else if decl.view.is_some() {
            self.records(&decl, None)?
                .into_iter()
                .find(|r| r.key == get.key)
        } else {
            let held = self.at_read(ty, self.view.get(ty, &get.key)?.into_iter().collect());
            let ends = self.admitted_ends(&decl, &held)?;
            let mut held = held.into_iter().zip(ends);
            held.find(|(r, ends)| *ends && self.live(&decl, r))
                .map(|(r, _)| r)
        };
        stored
            .and_then(|r| rights::visible(self.who, &decl, &r, get.with_meta))
            .ok_or_else(missing)
    }

    /// `proposals`: every proposal for a reviewer, one's own otherwise.
    /// `asked` is the principal as the call named it, before its grants
    /// were followed to the read's position.
    pub fn proposals(&self, filter: &ProposalFilter, asked: &Principal) -> Result<Vec<Proposal>> {
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
        // A migration's report counts every owner's records: a holder of
        // `only` is not listed one (051).
        let migration = |w: &ProposedWrite| !matches!(w, ProposedWrite::Declare { .. });
        let migration = |p: &Proposal| {
            p.write
                .as_ref()
                .is_some_and(|w| w.is_schema() && migration(w))
        };
        let mut mine: Vec<Proposal> = all
            .into_iter()
            .filter(|p| self.who.grants.review || p.proposer == me)
            .filter(|p| !rights::holds_only(self.who) || !migration(p))
            // A proposal on a type its reader is not shown is not there to
            // it, as its accept answers (lakelands#762).
            .filter(|p| {
                let hidden = |w| crate::writes::hidden_by_type(&self.shape.ontology, self.who, w);
                !p.write.as_ref().is_some_and(hidden)
            })
            .collect();
        let verb_on = |v: &VerbName| self.shape.ontology.verb(v).ok().map(|d| d.on.clone());
        // The lake as its reader is described it, read once for the
        // listing and only where a proposal asks.
        let lake = OnceCell::new();
        for p in &mut mine {
            // `more` is what the trim left out, not what the completion added.
            let mut logged = p.write.take();
            logged.iter_mut().for_each(|w| self.completed(w));
            let shown = logged.clone();
            // A proposed migration's reader, of a type or a value type, its
            // grants followed to the position it was proposed at: what it
            // names, it named then.
            // A bundle's reader is the one asking, its grants followed
            // here, to be shown references as the bundle in force would.
            let mut then = None;
            match &shown {
                Some(ProposedWrite::Migrate { .. } | ProposedWrite::MigrateValueType { .. }) => {
                    let mut who = asked.clone();
                    (who.grants).follow(&self.shape.renames, Some(p.position));
                    then = Some(who);
                }
                Some(ProposedWrite::Declare { .. }) => {
                    let mut who = asked.clone();
                    (who.grants).follow(&self.shape.renames, None);
                    then = Some(who);
                }
                _ => {}
            }
            let listing = (&lake, then.as_ref().unwrap_or(self.who), p.position);
            p.write = shown.and_then(|mut w| self.known_shape(&mut w, listing).then_some(w));
            p.more = p.write != logged;
        }
        mine.retain(|p| core_engines::matches(p, filter, verb_on));
        Ok(ordered_proposals(mine, &filter.order_by)?
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect())
    }

    /// A proposed shape completed as one in force is: a shape that is not
    /// in force is completed before it is judged or compared.
    fn completed(&self, write: &mut ProposedWrite) {
        match write {
            ProposedWrite::Migrate { migration, .. } => {
                self.shape.ontology.complete(&mut migration.to)
            }
            ProposedWrite::Declare { declarations } => {
                let mut after = self.shape.ontology.clone();
                let all = declarations.iter().cloned();
                all.for_each(|d| after.put(d, 0, None));
                for d in declarations.iter_mut() {
                    if let Declaration::Type(t) = d {
                        after.complete(t);
                    }
                }
            }
            _ => {}
        }
    }

    /// Whether the listing shows its reader the proposal logged at `at`,
    /// writing `write`, whole: a reviewer holding `only` decides no other
    /// (051).
    pub(crate) fn shows_whole(&self, write: &ProposedWrite, at: Position) -> bool {
        let mut shown = write.clone();
        self.known_shape(&mut shown, (&OnceCell::new(), self.who, at)) && shown == *write
    }

    /// A proposed write as its reader may know it (013 and 024, extended):
    /// a value where a read of the record would show it, a name where
    /// `describe` would. False when the reader may see nothing of it: a
    /// write to a type it may not see, or to a record it is not answered.
    /// The store logs a migration and a redeclaration completed with the
    /// fields hidden from their author, who reads its own proposal here.
    fn known_shape(
        &self,
        write: &mut ProposedWrite,
        (lake, then, at): (&OnceCell<LakeDescription>, &Principal, Position),
    ) -> bool {
        let read = |ty: &TypeName, field: &FieldName| {
            let t = self.visible_type(ty).ok();
            let f = t.and_then(|t| Some((t, t.fields.iter().find(|f| &f.name == field)?)));
            f.is_some_and(|(t, f)| rights::field_access(self.who, t, f).reads())
        };
        // A record the reader is not answered: restricted, without the
        // grant, or one its `only` does not admit, before the write or
        // after it as proposed (051). An `only` that cannot be applied
        // admits none, whatever a record holds (lakelands#771).
        let only = |ty: &TypeName, f: &BTreeMap<_, _>| {
            (self.visible_type(ty)).is_ok_and(|d| rights::admits(self.who, d, f))
        };
        let after = |ty: &TypeName, key: &RecordKey, patch: &lakelands_api::Patch| {
            let stored = self.view.get(ty, key).ok().flatten().filter(|r| !r.retired);
            let mut fields = stored.map(|r| r.fields).unwrap_or_default();
            fields.extend(patch.clone());
            only(ty, &fields)
        };
        // Retired or not, as `restricted` is judged.
        let answered = |ty: &TypeName, key: &RecordKey| {
            let stored = self.view.get(ty, key).ok().flatten();
            self.visible_type(ty).is_ok()
                && (self.who.grants.restricted || !stored.as_ref().is_some_and(|r| r.restricted))
                && stored.is_none_or(|r| only(ty, &r.fields))
        };
        match write {
            // A field the reader may not know of and a masked one both
            // leave the patch: it holds values, and no place for a name.
            ProposedWrite::Upsert { ty, key, patch, .. } => {
                let admitted = after(ty, key, patch);
                patch.retain(|field, _| read(ty, field));
                admitted && answered(ty, key)
            }
            ProposedWrite::Retire { ty, key, .. } => answered(ty, key),
            // A payload is what the verb's fields become: whole to a reader
            // who may read each field the verb sets, or see the type an
            // executor acts on, and empty otherwise. The verb's name is its
            // type's.
            ProposedWrite::Call {
                verb, key, payload, ..
            } => {
                let Ok(v) = self.shape.ontology.verb(verb) else {
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
            // The report names what the migration does to the type it was
            // proposed against, so a name is judged there, with the
            // reader's grants at the proposal; the totals stay whole. A
            // logged report is lossless, so it names no record that would
            // not convert or would collide. Its id is made again at the
            // head before the proposal.
            ProposedWrite::Migrate { migration, report } => {
                let Ok(was) = self.visible_type(&migration.to.name) else {
                    return false;
                };
                let head = self.shape.ontology.types.get(&migration.to.name);
                let stale =
                    head.is_none_or(|e| e.schema_hash != Some(report.from_schema_hash.clone()));
                // The type it was proposed against, read only where the
                // reader's grants hide a field; a failed read leaves the
                // type as it stands.
                let hides = stale && rights::hides_some(then);
                let proposed = hides.then(|| self.ontology_at(at)).flatten();
                let made = proposed
                    .as_ref()
                    .and_then(|o| o.ty(&migration.to.name).ok());
                let made = made.unwrap_or(was);
                // Its references held as the type it was proposed against
                // shows them (024, extended).
                let shown = proposed.as_ref().unwrap_or(&self.shape.ontology);
                let then = &crate::lake::Lake::hidden(then.clone(), shown).0;
                let sees = |ty: &TypeName| self.visible_type(ty).ok();
                let all = || describe::lake(&self.who.lake, self.declared(), self.who);
                let described = || lake.get_or_init(all);
                rights::shown_report(then, made, migration, report, described, sees, answered);
                // One listing: its `to` hash is that of the `to` it lists.
                let listing = proposed.as_ref().unwrap_or(&self.shape.ontology);
                let find = |n: &TypeName| listing.ty(n).ok();
                rights::shown_hashes(then, made, migration, report, &find);
                let before = Position(at.0.saturating_sub(1));
                report.report_id = crate::writes::report_id(report, Some(before));
                rights::known_migration(then, made, migration, &find);
                true
            }
            // Each declaration as `describe` would show it once the bundle
            // is in force; one it would not show leaves the list.
            ProposedWrite::Declare { declarations } => {
                let mut after = self.shape.ontology.clone();
                declarations
                    .iter()
                    .for_each(|d| after.put(d.clone(), 0, None));
                let declared = after.declared();
                // The reader as the bundle in force would show it its
                // references (024, extended), from its grants as asked: a
                // reference hidden by the lake as it stands may be shown.
                let who = &crate::lake::Lake::hidden(then.clone(), &after).0;
                declarations.retain(|d| describe::listed(d, &declared, who));
                for d in declarations.iter_mut() {
                    if let Declaration::Type(t) = d {
                        rights::described_fields(who, t, &|n| after.ty(n).ok());
                    }
                }
                !declarations.is_empty()
            }
            // A use says something of a field's values, so it is shown only
            // in a field the reader may read, of a type it may see (024),
            // with its renames' counts. The totals and `fits` stay whole, as
            // the preflight's do (`Call::shown_value_type`); a logged report
            // fits, so its `first` names no misfit. Its conditions are
            // judged again for the reader with its grants at the proposal,
            // as the lake stood before it, where its id is made again; a
            // failed read of that lake tells each only as one there may be.
            ProposedWrite::MigrateValueType {
                to,
                renames,
                report,
            } => {
                report.fields.retain(|u| read(&u.ty, &u.field));
                let before = Position(at.0.saturating_sub(1));
                if !renames.is_empty() {
                    let old = renames
                        .iter()
                        .map(|(o, n)| (o.as_str(), n.as_str()))
                        .collect();
                    let view = self.lake.engine.view(Some(before));
                    // Its references held as that lake shows them (024,
                    // extended).
                    let judged = view.and_then(|v| {
                        let o = Ontology::at(v.as_ref())?;
                        let then = crate::lake::Lake::hidden(then.clone(), &o).0;
                        crate::writes::conditions(&o, v.as_ref(), &then, report, &old)
                    });
                    if judged.is_err() {
                        report.conditions.clear();
                        report.conditions_unshown = true;
                    }
                }
                report.report_id = String::new();
                report.report_id = crate::writes::value_report_id(report, to, renames, before.0);
                true
            }
        }
    }

    /// What `search` refuses before it ranks: the type its principal is
    /// shown, a view, a misfit, the fields it probes, the `where`. A
    /// hybrid search's text is embedded only once this answers (024): one
    /// refused calls nothing, and names no model.
    pub fn searched<'s>(&self, s: &'s Search) -> Result<Searched<'s>> {
        let decl = self.named_type(&s.ty)?.clone();
        // A view's records are computed on read and no index holds them, so
        // a search could only answer nothing (018).
        if decl.view.is_some() {
            let ty = s.ty.clone();
            return Err(invalid(Case::ViewNotSearched { ty }));
        }
        if let Some((field, case)) = s.mode.misfit() {
            let reason = Refusal::Invalid(Box::new(case.into()));
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
        // A vector ranking reads a vector field, and a hybrid search without
        // a vector reads one a model fills: one that cannot use its ranking
        // is refused here, before any embedding is asked for (lakelands#743).
        let declared = |n: &FieldName| decl.fields.iter().find(|f| &f.name == n);
        for field in &vector_fields {
            if let Some(f) = declared(field).filter(|f| f.ty.kind != StorageKind::Vector) {
                let (field, kind) = ((*field).clone(), f.ty.kind);
                let case = Case::SearchNotVector { field, kind };
                return Err(crate::declare::invalid_field(&f.name, case));
            }
        }
        if let SearchMode::Hybrid {
            field,
            vector: None,
            ..
        } = &s.mode
            && declared(field).is_some_and(|f| f.embed.is_none())
        {
            let case = Case::SearchNeedsVector {
                field: field.clone(),
            };
            return Err(crate::declare::invalid_field(field, case));
        }
        // A query vector is judged by the field's declared length, never by
        // a stored vector, which another may own (024).
        let embedded = matches!(&s.mode, SearchMode::Hybrid { vector: None, .. });
        for (field, v, _) in vectors.iter().filter(|_| !embedded) {
            let f = decl.fields.iter().find(|f| &f.name == *field);
            if let Some(d) = f.and_then(|f| f.ty.dimensions)
                && v.len() != d as usize
            {
                let (query, declared) = (v.len(), d as usize);
                let field = field.as_str();
                return Err(core_engines::dimensions(query, declared, &s.ty, field));
            }
        }
        let filter = self.record_filter(&decl, s.filter.as_ref(), &[Place::Where])?;
        Ok((decl, text_fields, filter))
    }

    /// `search`.
    pub fn search(&self, s: &Search) -> Result<Hits> {
        let (decl, text_fields, filter) = self.searched(s)?;
        let vectors = s.mode.vectors();
        let vector_fields: Vec<&FieldName> = vectors.iter().map(|v| v.0).collect();
        // A search that names no fields reads only the searchable fields
        // the principal reads (024); none hidden or masked, it reads them all.
        let known: Vec<FieldName>;
        let hidden = (decl.fields.iter())
            .any(|f| f.searchable && !rights::field_access(self.who, &decl, f).reads());
        let text_fields = match text_fields {
            None if hidden => {
                known = rights::searched_fields(self.who, &decl).cloned().collect();
                Some(known.as_slice())
            }
            named => named,
        };
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
                None => Err(Error::unbound(lakelands_api::error::Seam::VectorIndex)),
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
        let axis = |field: Option<&FieldName>| Axis {
            axis: (if field.is_some() { "vector" } else { "lexical" }).into(),
            field: field.cloned(),
            candidates: None,
            weight: None,
        };
        // A ranking the search does not weigh takes its type's setting, as
        // the read finds it at its position (lakelands#561); a field the
        // type no longer holds is never named by a search, so weighs 1.
        let key = format!("{}{}", lakelands_api::seam::SEARCH_WEIGHTS_SETTING, s.ty);
        let (field_weights, text_default) =
            lakelands_api::seam::search_weights(self.shape.settings.get(&key));
        let mut axes = Vec::new();
        if let (
            SearchMode::Hybrid {
                text, text_weight, ..
            },
            false,
        ) = (&s.mode, by_text)
        {
            lists.push((
                text_weight.or(text_default).unwrap_or(1.0),
                Box::new(move |pull| lexical(text, pull)),
            ));
            axes.push(axis(None));
        }
        for (field, v, weight) in vectors.iter().filter(|_| !by_text) {
            let set = || field_weights.get(field.as_str()).copied();
            lists.push((
                weight.or_else(set).unwrap_or(1.0),
                Box::new(move |pull| vector(field, v, pull)),
            ));
            axes.push(axis(Some(field)));
        }
        let hits = match &s.mode {
            _ if lists.len() > 1 => {
                let (hits, counts) =
                    self.fused(&decl, filter.as_ref(), &lists, want, s.with_meta)?;
                for ((a, n), (w, _)) in axes.iter_mut().zip(counts).zip(&lists) {
                    (a.candidates, a.weight) = (Some(n), Some(*w));
                }
                hits
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
        // A lexical search, and a hybrid one that fell back to its text,
        // rank by the text alone: its axis is listed without a count, since
        // it reads no further than its page (lakelands#560).
        if axes.is_empty() {
            axes.push(axis(vectors.first().map(|v| v.0).filter(|_| !by_text)));
        }
        Ok(Hits {
            hits: page,
            ranked_by,
            next_cursor,
            axes,
        })
    }

    /// A fused search's hits: its lists fused by weighted reciprocal rank
    /// ([`fuse`]), each ranked among the hits this read may see and to the
    /// lake's `max_limit` of them, so a record the read may not see moves
    /// no score and every page reads one ranking (046). The lists
    /// are pulled in growing batches until the first `want` fused hits and
    /// their scores can no longer change, and each list is whole or holds
    /// its window, so that its count, answered beside the hits, is the
    /// same on every page.
    fn fused(
        &self,
        decl: &TypeDecl,
        filter: Option<&Expr>,
        lists: &[(f64, Ranking<'_>)],
        want: usize,
        with_meta: bool,
    ) -> Result<(Vec<Hit>, Vec<u64>)> {
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
            && self.whole(decl)
            && self.who.grants.restricted
            && filter.is_none()
            && !self.timed(decl);
        loop {
            // Each list's visible candidates to the window, and what a
            // record it has not ranked yet could still gain from it:
            // nothing once the list is whole or its window full.
            let (mut keyed, mut gain, mut counted) = (Vec::new(), Vec::new(), true);
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
                counted &= done;
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
            if settled && counted {
                let counts = keyed.iter().map(|(_, l)| l.len() as u64).collect();
                let first = fused.into_iter().take(want).collect();
                return Ok((self.visible_hits(decl, filter, first, with_meta)?, counts));
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
            self.shape.settings.get(&key).cloned()
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
        let rows = self.at_read(&decl.name, self.view.get_many(&decl.name, &keys)?);
        let (ends, pairs) = (self.admitted_ends(decl, &rows)?, self.pairs(&decl.name));
        let at_head = self.head_admitted(decl, &keys)?;
        let mut stored: BTreeMap<RecordKey, StoredRecord> = (rows.into_iter().zip(ends))
            .filter(|(r, ends)| *ends && at_head.as_ref().is_none_or(|h| h.contains(&r.key)))
            .map(|(r, _)| (r.key.clone(), r))
            .collect();
        let mut hits = Vec::new();
        for (key, score) in ranked {
            let Some(r) = stored.remove(&key) else {
                continue;
            };
            if !self.live_by(decl, pairs.as_deref(), &r) {
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

    /// For a search as of a position by a holder of `only` on `decl`, the
    /// candidates admitted at the head as well, under the grant followed
    /// there and by the name the type bears there: a ranking by the text
    /// and vectors at the head must find no record by another's (051).
    /// A grant that cannot be applied to the head's declaration admits
    /// none there (lakelands#771). `None` where every candidate passes.
    fn head_admitted(
        &self,
        decl: &TypeDecl,
        keys: &[RecordKey],
    ) -> Result<Option<BTreeSet<RecordKey>>> {
        let Some(who) = self.head_who.as_ref().filter(|_| !self.whole(decl)) else {
            return Ok(None);
        };
        let head = self.lake.engine.view(None)?;
        let mut name = decl.name.clone();
        for r in self.lake.renames(Some(head.as_ref()))? {
            if let Some(to) = r.to.filter(|_| r.at > self.position && r.ty == name) {
                name = to;
            }
        }
        let shape = self.lake.shape(head.as_ref())?;
        let applied = |t: &TypeName| {
            let d = shape.ontology.ty(t).ok();
            d.filter(|d| rights::check_only(who, d).is_ok())
        };
        let Some(at_head) = applied(&name) else {
            return Ok(Some(BTreeSet::new()));
        };
        let rows = head.get_many(&name, keys)?;
        // A link's ends are judged at the head too, under the same grant.
        let ends = ends_admitted(head.as_ref(), who, decl, &rows, |t| {
            applied(t).map(|_| rights::own_only(who, t))
        })?;
        let admitted = (rows.into_iter().zip(ends))
            .filter(|(r, ends)| *ends && !r.retired && rights::admits(who, at_head, &r.fields));
        Ok(Some(admitted.map(|(r, _)| r.key).collect()))
    }

    /// Refuse a walk around `here` over a relation that probes a field the
    /// principal may not read: the reference field that carries it, whose
    /// value each edge answers, or a derived relation's matched field, on
    /// either side, where an edge that appears or not answers whether the
    /// two values are equal (024, 044). A step is probed before it walks,
    /// and every hop again around the type it reached, over `relation` or,
    /// when it is `None`, over every relation it walks. Only a relation the
    /// hop walks and the principal is shown is probed, so a refusal never
    /// names a type or a field it is told is not there (006, extended):
    /// what remains is a masked field.
    fn probe_derived(
        &self,
        here: &TypeName,
        relation: Option<&RelationName>,
        walk: Walk,
    ) -> Result<()> {
        for (t, r) in self.shown_relations(Some(here)) {
            let out = t.name == *here && walk != Walk::In;
            let inn = r.target == *here && walk != Walk::Out;
            if relation.is_some_and(|n| r.name != *n) || !(out || inn) {
                continue;
            }
            rights::check_probe(self.who, t, r.via.iter())?;
            let Some(d) = &r.derived else { continue };
            rights::check_probe(self.who, t, d.pairs.iter().map(|p| &p.from))?;
            let target = self.shape.ontology.ty(&r.target)?;
            rights::check_probe(self.who, target, d.pairs.iter().map(|p| &p.to))?;
        }
        Ok(())
    }

    /// `query`: the one pattern query.
    pub fn query(&self, p: &Pattern) -> Result<QueryResult> {
        // Its steps as declared: a step naming an inverse is rewritten below.
        let mut p = p.clone();
        let p = &mut p;
        let start_decl = self.named_type(&p.start.ty)?.clone();
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
        // A graph whose steps all start at depth 0 is one walk over any of
        // their relations from the start (018): a step's relation may be
        // declared by a type another step reaches. Any other is a chain.
        let chain = p.steps.iter().any(|s| s.depth.min > 0);
        let lenient = match p.project {
            Projection::Graph => !chain,
            _ => p.project == Projection::Components,
        };
        for (i, step) in p.steps.iter_mut().enumerate() {
            // A measure's steps are the relations around its records, each
            // read from the start, not a chain.
            let here = if measure {
                start_decl.name.clone()
            } else {
                types.last().expect("the start is in").clone()
            };
            self.as_declared(&here, step);
            let reached = if measure || chain {
                self.step_target(&here, &step.relation, step.walk, lenient)?
            } else {
                self.neighbour_target(i, step)?
            };
            self.probe_derived(&here, Some(&step.relation), step.walk)?;
            let at = Place::Step {
                step: i + 1,
                relation: step.relation.clone(),
            };
            if let (Some(t), Some(r)) = (&step.ty, &reached)
                && t != r
            {
                let (reached, ty) = (r.clone(), t.clone());
                let said = Said::from(Case::StepReaches { reached, ty });
                return Err(invalid(said.within(at)));
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
                return Err(invalid(
                    Said::from(unbound(&reached, param, true)).within(at),
                ));
            }
            let at = [at, Place::Where];
            filters.push(self.record_filter(&decl, step.filter.as_ref(), &at)?);
            types.push(reached);
        }
        if measure {
            return self.measure(p, limit);
        }
        // What a pattern says and an arm cannot honour is refused, not
        // passed over: a path is a graph of one walk from one start.
        if p.to.is_some() && p.project != Projection::Graph {
            return Err(invalid(Case::PatternToNotGraph));
        }
        if p.to.is_some() && chain {
            return Err(invalid(Case::PatternToChain));
        }
        // A cursor is checked before any arm answers: a path and an
        // aggregate hand out none, so every cursor is another read's there.
        let offset = offset(p.cursor.as_ref(), p, &params)?;
        let hidden = Visible { reader: self };
        // Whether a record reached over step `i` is kept and walked from:
        // it is of the step's `type`, and its `where`, read against the type
        // the step reaches, holds of it.
        let keep = |i: usize, node: &lakelands_core::graph::Node| -> Result<bool> {
            if p.steps[i].ty.as_ref().is_some_and(|t| *t != node.0) {
                return Ok(false);
            }
            let Some(f) = &filters[i] else {
                return Ok(true);
            };
            Ok(node.0 == types[i + 1]
                && self
                    .get_one(&node.0, &node.1)?
                    .is_some_and(|r| self.holds(f, &r.fields, Some(&r.meta))))
        };
        // The start.
        let starts: Vec<StoredRecord> = if let Some(key) = &p.start.key {
            if self.start_found(p, key)? {
                self.get_one(&p.start.ty, key)?.into_iter().collect()
            } else {
                Vec::new()
            }
        } else if let Some(mode) = &p.start.search {
            let keys = self.start_hits(p, mode)?;
            let held = keys
                .iter()
                .filter_map(|k| self.view.get(&p.start.ty, k).ok().flatten());
            self.at_read(&p.start.ty, held.collect())
        } else {
            let at = [Place::Start, Place::Where];
            let f = self.record_filter(&start_decl, p.start.filter.as_ref(), &at)?;
            self.records(&start_decl, f.as_ref())?
        };
        let from: Vec<lakelands_core::graph::Node> = starts
            .iter()
            .map(|s| (s.ty.clone(), s.key.clone()))
            .collect();
        // A path, from its one start.
        if let Some((to_ty, to_key)) = &p.to {
            let (nodes, edges) = match from.as_slice() {
                [] => (Vec::new(), Vec::new()),
                [one] => {
                    let to = (to_ty.clone(), to_key.clone());
                    graph::shortest_path(&hidden, one, &to, &p.steps, max_depth, keep)?
                        .map_or_else(Default::default, |path| (path.nodes, path.edges))
                }
                more => {
                    let count = more.len();
                    return Err(invalid(Case::PathStarts { count }));
                }
            };
            return self.graph_result(nodes, edges, None);
        }
        // A graph: several starts are one walk, each record at the fewest
        // hops any start reaches it in, ordered by (depth, type, key) as
        // one walk orders what it reaches (026, 029), a page at a time.
        if p.project == Projection::Graph {
            let walk = if chain {
                graph::spanned
            } else {
                graph::subgraph
            };
            let g = walk(&hidden, &from, &p.steps, max_depth, offset, limit, keep)?;
            let next = g
                .truncated
                .then(|| self.page_cursor(p, offset + limit, params));
            let nodes = g.nodes.into_iter().map(|r| r.node).collect();
            return self.graph_result(nodes, g.edges, next);
        }
        // Rows are ordered and measured by their paths, `type.field`: each
        // probes its field as `find`'s order and aggregate do (024), so an
        // answer is never computed over values the principal may not read.
        // An aggregate's names are read as the rows hold them, and a named
        // key's fields are probed as a field key's are (018, extended).
        let mut paths: Vec<String> = p.order_by.iter().map(|o| o.field.to_string()).collect();
        let row = self.row_shape(&types);
        let mut over_rows = None;
        if let Projection::Aggregate(a) = &p.project {
            let bound = |t: &str| types.iter().any(|b| b.as_str() == t);
            let (a, read) = aggregate::over_rows(a, &bound).map_err(invalid)?;
            // A row's `<type>.key` is its record's key, unless the type
            // has a field of that name its reader may know of (024).
            let ty = |t: &str| self.shape.ontology.ty(&TypeName::from(t)).ok();
            let field = |t: &str, f: &FieldName| {
                ty(t).is_some_and(|d| d.fields.iter().any(|x| &x.name == f) && self.knows(d, f))
            };
            let probed = read
                .iter()
                .filter(|(t, f)| f.as_str() != "key" || field(t, f));
            paths.extend(probed.map(|(t, f)| format!("{t}.{f}")));
            over_rows = Some(a);
        }
        self.probe_rows(&paths, p.order_by.len())?;
        // What a named key can never compute is refused, as `find` refuses
        // it, over the row's shape (028, extended).
        if let Some(a) = &over_rows {
            let keys = aggregate::group_keys(&a.group_by).map_err(invalid)?;
            for (name, e) in keys {
                if let Some(e) = e {
                    crate::declare::evaluates(&e, &row, &[Place::Group { name }])?;
                }
            }
        }
        // Rows: one per match.
        let mut rows: Vec<BTreeMap<String, Value>> = Vec::new();
        for from in &from {
            let t = graph::traverse(&hidden, from, &p.steps, max_depth, usize::MAX, keep)?;
            for binding in t.bindings {
                let mut row = BTreeMap::new();
                self.bind(&mut row, from)?;
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
        if let Some(a) = &over_rows {
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
                aggregate::aggregate(&refs, a, &self.shape.settings, self.now).map_err(invalid)?;
            return Ok(QueryResult::Groups { rows: groups });
        }
        if p.order_by.iter().any(|o| o.by == OrderBy::Latest) {
            return Err(invalid(Case::LatestNoChange { ty: None }));
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
    /// declared type, or one the principal is not shown, reads nothing and
    /// is left alone. The first `ordered` paths are orders, and an order by
    /// a field computed at the read is refused once the field is probed.
    fn probe_rows(&self, paths: &[String], ordered: usize) -> Result<()> {
        for (i, path) in paths.iter().enumerate() {
            let Some((ty, field)) = path.rsplit_once('.') else {
                continue;
            };
            if let Ok(decl) = self.shape.ontology.ty(&TypeName::from(ty))
                && self.owner_shown(decl)
            {
                rights::check_probe(self.who, decl, [&FieldName::from(field)])?;
                if i < ordered {
                    Self::steady_order(decl, field)?;
                }
            }
        }
        Ok(())
    }

    /// Whether the principal is shown the type that declares a relation, so
    /// that a step may name the relation from its target's end: one told
    /// the owner does not exist is not told its relations' names there
    /// (006, extended). Asked last, of an owner a step would reach.
    fn owner_shown(&self, owner: &TypeDecl) -> bool {
        describe::one_type(&owner.name, self.declared(), self.who).is_some()
    }

    /// The relations the principal is shown (`describe::relation_shown`)
    /// with their owners: those that leave or reach `around`, or all.
    /// A relation it is not shown is one no type declares, at every step,
    /// inverse name and edge.
    fn shown_relations(&self, around: Option<&TypeName>) -> Vec<(&TypeDecl, &RelationDecl)> {
        let find = |n: &TypeName| self.shape.ontology.ty(n).ok();
        let near =
            |t: &TypeDecl, r: &RelationDecl| around.is_none_or(|a| &t.name == a || &r.target == a);
        let types = self.shape.ontology.types.values().map(|e| &e.decl);
        let all = types.flat_map(|t| t.relations.iter().map(move |r| (t, r)));
        all.filter(|(t, r)| near(t, r) && describe::relation_shown(t, r, &find, self.who))
            .collect()
    }

    /// Whether a type the principal is shown declares a relation of this
    /// name, so that a step over it from elsewhere walks nothing.
    fn out_any(&self, relation: &RelationName) -> bool {
        (self.shown_relations(None).iter()).any(|(_, r)| &r.name == relation)
    }

    /// Rewrite a step naming a relation's `inverse` to that relation,
    /// walked the other way: the inverse is the relation's name from its
    /// target, and the step's `walk` counts from the name it gives. A
    /// relation declared under the name wins, so a step is read as an
    /// inverse only when it walks no relation of that name from `here`.
    fn as_declared(&self, here: &TypeName, step: &mut Step) {
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
        let shown = self.shown_relations(Some(here));
        if (shown.iter()).any(|(t, r)| &r.name == name && walked(t, r, walk)) {
            return;
        }
        let inverse = (shown.iter())
            .find(|(t, r)| r.inverse.as_ref() == Some(name) && walked(t, r, flipped))
            .map(|(_, r)| r);
        if let Some(r) = inverse {
            step.relation = r.name.clone();
            step.walk = flipped;
        }
    }

    /// The type a neighbourhood's step reaches, wherever its relation is
    /// declared: its target walked out, its owner walked in. A step that
    /// reaches several types and reads a `where` names its `type`, since
    /// the `where` is read against one.
    fn neighbour_target(&self, i: usize, step: &Step) -> Result<Option<TypeName>> {
        let mut reached = BTreeSet::new();
        for (t, r) in self.shown_relations(None) {
            if r.name == step.relation {
                (step.walk != Walk::In).then(|| reached.insert(r.target.clone()));
                (step.walk != Walk::Out).then(|| reached.insert(t.name.clone()));
            }
        }
        if step.ty.is_some() || reached.len() < 2 {
            return Ok(step.ty.clone().or_else(|| reached.pop_first()));
        }
        match &step.filter {
            None => Ok(None),
            Some(_) => {
                let (step, relation) = (i + 1, step.relation.clone());
                let said = Said::from(Case::StepSeveralTypes);
                Err(invalid(said.within(Place::Step { step, relation })))
            }
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
        self.shape.ontology.ty(here)?;
        let shown = self.shown_relations(Some(here));
        let out = (shown.iter())
            .find(|(t, r)| &t.name == here && &r.name == relation)
            .map(|(_, r)| r.target.clone());
        let inn = (shown.iter())
            .find(|(_, r)| &r.name == relation && &r.target == here)
            .map(|(t, _)| t.name.clone());
        let found = match walk {
            Walk::Out => out,
            Walk::In => inn,
            Walk::Both => out.or(inn),
        };
        match found {
            Some(t) => Ok(Some(t)),
            None if lenient && self.out_any(relation) => Ok(None),
            None => {
                let (ty, relation) = (here.clone(), relation.clone());
                Err(invalid(Case::NoRelationToWalk { ty, relation, walk }))
            }
        }
    }

    /// The record a node names, as the principal may see it, if at all.
    fn shown(&self, (ty, key): &graph::Node) -> Result<Option<Record>> {
        let decl = self.shape.ontology.ty(ty)?;
        Ok((self.get_one(ty, key)?).and_then(|r| rights::visible(self.who, decl, &r, false)))
    }

    /// A pattern's row as one type: each field of each type `bound` its
    /// reader may know of under its path, `<type>.<field>`, and each one's
    /// `<type>.key`, the record's key where no such field is named `key`.
    fn row_shape(&self, bound: &[TypeName]) -> TypeDecl {
        let mut row = TypeDecl::default();
        for t in bound.iter().filter_map(|t| self.shape.ontology.ty(t).ok()) {
            let at = |f: &str| FieldName::from(format!("{}.{f}", t.name));
            let known = t.fields.iter().filter(|f| self.knows(t, &f.name));
            let fields: Vec<_> = known
                .map(|f| lakelands_api::ontology::FieldDecl {
                    name: at(f.name.as_str()),
                    ..f.clone()
                })
                .collect();
            if !fields.iter().any(|f| f.name == at("key")) {
                row.fields.push(lakelands_api::ontology::FieldDecl {
                    name: at("key"),
                    ..Default::default()
                });
            }
            row.fields.extend(fields);
        }
        row
    }

    /// A record's values into a pattern's row under their paths. A row's
    /// `<type>.key` is the record's key, unless the type declares a field
    /// `key` its reader may know of: then that field's value, null where
    /// the record holds none.
    fn bind(&self, row: &mut BTreeMap<String, Value>, node: &graph::Node) -> Result<()> {
        if let Some(rec) = self.shown(node)? {
            let decl = self.shape.ontology.ty(&node.0)?;
            let named = |f: &lakelands_api::ontology::FieldDecl| {
                f.name.as_str() == "key" && self.knows(decl, &f.name)
            };
            let key = match decl.fields.iter().any(named) {
                true => Value::Null,
                false => Value::String(node.1.to_string()),
            };
            row.insert(format!("{}.key", node.0), key);
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
        next_cursor: Option<Cursor>,
    ) -> Result<QueryResult> {
        let mut records = Vec::new();
        for node in &nodes {
            records.extend(self.shown(node)?);
        }
        Ok(QueryResult::Graph {
            nodes: records,
            edges,
            truncated: next_cursor.is_some(),
            next_cursor,
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
            let (members, field, by) = (PROPOSAL_MEMBERS.iter().collect(), o.field.clone(), o.by);
            return Err(invalid(Case::ProposalOrder { members, field, by }));
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
    (relation, shown): (Option<&RelationName>, &[(&TypeDecl, &RelationDecl)]),
    walk: Walk,
) -> Result<Vec<Vec<Edge>>> {
    // A relation the walker is not shown is never matched (006, extended).
    let wanted = |x: &RelationDecl| {
        (x.derived.is_some() && relation.is_none_or(|n| *n == x.name))
            && shown.iter().any(|(_, s)| std::ptr::eq(*s, x))
    };
    let owned: Vec<&RelationDecl> = match (walk, r.shape.ontology.ty(ty)) {
        (Walk::Out | Walk::Both, Ok(t)) => t.relations.iter().filter(|x| wanted(x)).collect(),
        _ => Vec::new(),
    };
    let targeted: Vec<(&TypeName, &RelationDecl)> = if walk == Walk::Out {
        Vec::new()
    } else {
        r.shape
            .ontology
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

/// Of link records of `decl` in `view`, which have every end `who` holds
/// `only` on admitted by `pairs` (051): each such end type's records read
/// once for the batch. Any other type's records all pass.
fn ends_admitted(
    view: &dyn ProjectionView,
    who: &Principal,
    decl: &TypeDecl,
    rows: &[StoredRecord],
    pairs: impl Fn(&TypeName) -> Option<Vec<(FieldName, Value)>>,
) -> Result<Vec<bool>> {
    let mut out = vec![true; rows.len()];
    for (field, end) in rights::held_ends(who, decl) {
        let key = |r: &StoredRecord| r.fields.get(field)?.as_str().map(RecordKey::from);
        let keys: Vec<RecordKey> = rows.iter().filter_map(key).collect();
        let Some(pairs) = pairs(end) else {
            out.fill(false);
            continue;
        };
        let ends = view.get_many(end, &keys)?.into_iter();
        let admitted = ends.filter(|e| rights::admitted(&pairs, &e.fields));
        let admitted: BTreeSet<RecordKey> = admitted.map(|e| e.key).collect();
        for (ok, r) in out.iter_mut().zip(rows) {
            *ok &= key(r).is_some_and(|k| admitted.contains(&k));
        }
    }
    Ok(out)
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
        // A hop past the first reaches types the step's own probe did not
        // look at.
        r.probe_derived(ty, relation, walk)?;
        let shown = r.shown_relations(Some(ty));
        let mut lists = ProjectionView::edges(r.view.as_ref(), ty, keys, relation, walk)?;
        for (list, derived) in
            lists
                .iter_mut()
                .zip(derived_edges(r, ty, keys, (relation, &shown), walk)?)
        {
            list.extend(derived);
        }
        // A link's edge comes from its own record: a restricted link is
        // walked only with the `restricted` grant, and its fields only as
        // far as the principal may read the link type's fields. The link
        // records are read a link type at a time.
        let carried = |e: &Edge| {
            r.shape
                .ontology
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
                // An edge of a relation the principal is not shown answers
                // nothing, whatever step asked for it: a step over another
                // type's relation of the same name walks it (024, extended).
                if !(shown.iter()).any(|(t, x)| t.name == e.from.0 && x.name == e.relation) {
                    continue;
                }
                if carried(&e) {
                    let (lt, lk) = link_of(&e);
                    let Some(link) = links.get(&(lt.clone(), lk)) else {
                        continue;
                    };
                    // `only` on the link type admits it by its fields (051).
                    let decl = r.shape.ontology.ty(&lt);
                    let restricted = link.restricted && !r.who.grants.restricted;
                    if decl.as_ref().map_or(restricted, |d| !r.live(d, link)) {
                        continue;
                    }
                    if let Ok(decl) = decl {
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
            let Ok(decl) = r.shape.ontology.ty(ty) else {
                continue;
            };
            if describe::one_type(ty, r.declared(), r.who).is_none() {
                continue;
            }
            let rows = r.get_many(ty, &keys)?;
            let (ends, pairs) = (r.admitted_ends(decl, &rows)?, r.pairs(ty));
            for (rec, _) in rows.into_iter().zip(ends).filter(|(_, ends)| *ends) {
                if r.live_by(decl, pairs.as_deref(), &rec) {
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
