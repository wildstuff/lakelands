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
//! its `where` keeps. A view the setting `view_kept.<type>` asks for is
//! kept by the fold and read from its kept records where they are exactly
//! what the read would compute (049). A walk's step reads a view so too,
//! one with parameters only where the start bound them.

use std::cell::{Cell, OnceCell, RefCell};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;

use lakelands_api::describe::{Kept, LakeDescription, NotKept, TypeDescription};
use lakelands_api::engine::{After, ProjectionView, Ranked, Rankings, Scan};
use lakelands_api::error::{Case, Names, Naming, Place, Said};
use lakelands_api::expr::{CmpOp, Expr, Func, Literal};
use lakelands_api::ontology::{
    FieldDecl, FieldType, MatchPair, RelationDecl, StorageKind, TypeDecl, VerbDecl, VerbEffect,
};
use lakelands_api::principal::{Access, Renamed, TypeGrants};
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
use lakelands_core::rights::Admit;
use lakelands_core::sight::Sight;
use lakelands_core::{
    canonical, derive as core_derive, engines as core_engines, invalid, rights, valid_time,
};

use crate::declare::condition;
use crate::kept;
use crate::lake::{Lake, Shape};
use crate::ontology::Ontology;

/// A sight an entry of the listing is judged under beside the one in
/// force, and the lake it is of ([`Reader::entry`]).
type Under<'s> = Option<(&'s Sight<'s>, &'s Ontology)>;

/// Everything one read sees.
pub struct Reader<'a> {
    /// The lake.
    pub lake: &'a Lake,
    /// Who reads, as [`Self::sight`] judges it.
    pub who: &'a Principal,
    /// What the reader is shown of the read's shape.
    pub sight: &'a Sight<'a>,
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
    /// What [`Self::kept_read`] answered for each view and face, so that a
    /// view many views read is asked once a read (lakelands#942).
    kept_reads: RefCell<BTreeMap<(TypeName, usize), Option<KeptRead>>>,
    /// The views whose sources [`Self::probe_view`] found readable, so
    /// that a view many views read is probed once a read (lakelands#942).
    probed: RefCell<BTreeSet<TypeName>>,
    /// What [`Self::computes`] answered for each type this read, so that a
    /// view many views read is asked once (lakelands#942).
    computes_known: RefCell<BTreeMap<TypeName, bool>>,
    /// For a search as of a position, the head, whose indexes its rankings
    /// read (025, 051).
    pub head: Option<Head<'a>>,
    /// For a read as of a position by a reader without `restricted`, which
    /// reads each record held as it is at the head ([`HeldNow`]): a kept
    /// view's face folded then is not read.
    pub held_now: bool,
    /// Whether the records read are the retired ones too: set while
    /// [`Self::retired_count`] counts.
    retired: Cell<bool>,
}

mod held;
mod measures;

pub(crate) use held::HeldNow;

/// The head as a search as of a position reads it, from one view: its
/// shape, the reader with its grants followed there, and where each record
/// of the read's position stands there. The read's holds are read from
/// this view too ([`HeldNow`]): a re-key landing between two views of the
/// head would be in one and not in the other's moves, and a held record
/// would be judged by another's hold.
///
/// The indexes are not a view: the engine reads the text and the vectors
/// as they stand when it is asked, and says the position they stood at
/// ([`Pulled`]). A search's head is that position, at the head and as of
/// an earlier one alike. A head taken before the rankings were read would
/// not know a write that landed in between: a re-key's moves, so that a
/// visible record bore a held record's score, or a hold, so that a record
/// was ranked by a text its reader may not read (lakelands#826).
pub struct Head<'a> {
    /// The projection at the head.
    view: Rc<dyn ProjectionView + 'a>,
    /// What the lake declares there.
    shape: Arc<Shape>,
    /// The reader, its grants followed through the head's shape.
    who: Principal,
    /// Each type and key of the read's position, as named there.
    since: Rc<held::Since>,
}

impl<'a> Head<'a> {
    /// The head of `lake` for `who`, as unfollowed, reading as of `at`: as
    /// it stands, or at exactly the position `head`.
    pub fn new(
        lake: &'a Lake,
        who: &Principal,
        at: Position,
        head: Option<Position>,
    ) -> Result<Head<'a>> {
        let view: Rc<dyn ProjectionView + 'a> = Rc::from(lake.engine.view(head)?);
        let shape = lake.shape(view.as_ref())?;
        let sight = lake.sight(who.clone(), None, Some(&shape.renames), shape.as_ref())?;
        let who = sight.0.who().clone();
        let since = Rc::new(held::Since::new(lake, view.as_ref(), at)?);
        Ok(Head {
            view,
            shape,
            who,
            since,
        })
    }
}

/// One pull of a search's rankings, as the engine answered it: what was
/// asked, under the head's names and to which depth, and the lists with
/// the position they were read at.
pub struct Pulled {
    ty: TypeName,
    asked: Rankings,
    k: u32,
    ranked: Ranked,
}

impl Pulled {
    /// The position the lists were read at.
    pub fn position(&self) -> Option<Position> {
        self.ranked.position
    }
}

/// What a read that ranks answers: its answer, or a pull read at another
/// position than the head the read was given, which the door makes the
/// read again for (`Doors::read_to`).
pub type Judged<T> = std::result::Result<T, Pulled>;

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

/// A page of a pattern's starts, each with its position.
type Starts = [(SortKey, StoredRecord)];

/// What a page of rows does with each binding of a start at its position.
type EachBinding<'a> = dyn FnMut(&SortKey, &graph::Node, &[graph::Reached]) -> Result<bool> + 'a;

/// One ranked list of a search: the candidates best first, with their
/// scores.
type Ranking = Vec<(RecordKey, f64)>;

/// A search as judged: its type, the fields its text is matched in if it
/// names them, its `where`, and its rankings as the head declares them.
type Searched<'s> = (TypeDecl, Option<&'s [FieldName]>, Option<Expr>, Ranks);

/// A search's rankings as the head declares them, whose indexes they read
/// (025): the type there, and each field a ranking names, by the name the
/// read knows it by, as declared there under the name it bears there. A
/// field the head does not declare, or does not show the reader, is not
/// in `fields`: it holds no vectors and no text there.
pub struct Ranks {
    /// The type's name at the head.
    pub ty: TypeName,
    /// The type's declaration at the head.
    pub decl: Option<TypeDecl>,
    /// By its name at the read's position, each field at the head.
    pub fields: BTreeMap<FieldName, FieldDecl>,
}

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

/// Where a `query` page continues: past the start a cursor names, where
/// it names one, and past as many rows as its offset says.
fn offset(c: Option<&Cursor>, p: &Pattern, params: &Params) -> Result<(usize, Option<SortKey>)> {
    let Some(c) = c else { return Ok((0, None)) };
    let state = resume(c, "query", p.start.ty.as_ref(), params)?;
    same_read(&state, &pattern_read(p))?;
    Ok((state.offset as usize, state.after))
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

/// The start's type, which every read of a pattern but `components`
/// needs (018).
fn start_type(p: &Pattern) -> Result<&TypeName> {
    let asks = || projection_name(&p.project);
    (p.start.ty.as_ref()).ok_or_else(|| invalid(Case::StartNoType { asks: asks() }))
}

/// A projection by the name its pattern gives it.
fn projection_name(p: &Projection) -> String {
    match p {
        Projection::Rows { .. } => "rows",
        Projection::Graph => "graph",
        Projection::Aggregate(_) => "aggregate",
        Projection::Degree => "degree",
        Projection::Components => "components",
        Projection::Reach => "reach",
    }
    .to_owned()
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

/// A row's column under each bound name for the hops its step took to
/// reach the record (018, extended).
const DEPTH: &str = "_depth";

/// A row's column under each bound name for the record's type (018,
/// extended).
const TYPE: &str = "_type";

/// A step over several types, at `at`, refused where one is read: naming
/// every type it reaches.
fn several_types(at: &Place, reached: &BTreeSet<TypeName>) -> Error {
    let mut all = reached.iter().cloned();
    let reached = all.next().expect("several types");
    let case = Case::StepSeveralTypes {
        reached: Some(reached),
        also: Some(all.collect()),
    };
    invalid(Said::from(case).within(at.clone()))
}

/// The most records a hop over a derived relation holds at once: a page of
/// starts asks its matches a page of this many at a time (`derived_edges`).
const MATCHED: u32 = 65_536;

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
/// out; the computation's and the kept fold's. A measure whose value its
/// field would not take written fails, named with what the field refuses:
/// a number no reader reads exactly, an instant out of the years, a value
/// of another kind.
pub(crate) fn landed(
    decl: &TypeDecl,
    mut fields: aggregate::Row,
    measured: impl IntoIterator<Item = (FieldName, Value)>,
) -> Result<aggregate::Row, (FieldName, Case)> {
    for (name, v) in measured {
        let landed = match decl.fields.iter().find(|f| f.name == name) {
            Some(f) if !v.is_null() => {
                let fits = expr::eval::to_json(&expr::eval::from_json(&v), f.ty.kind, None);
                let judged = lakelands_core::value::check_kind(fits.as_ref().unwrap_or(&v), &f.ty);
                judged.map_err(|why| (name.clone(), why))?
            }
            _ => v,
        };
        fields.insert(name, landed);
    }
    fields.retain(|_, v| !v.is_null());
    Ok(fields)
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
    let scope = RecordScope {
        fields,
        meta,
        settings: Some(settings),
        now,
    };
    expr::holds(e, &scope).unwrap_or(false)
}

/// What [`Reader::kept_read`] answers of a kept view: its state, and how
/// many of its groups and its sources' are not exact.
type KeptRead = (kept::State, u64);

impl<'a> Reader<'a> {
    /// Open a read at `at`, of `view` and its `shape`.
    pub fn open(
        lake: &'a Lake,
        sight: &'a Sight<'a>,
        at: ReadAt,
        view: Box<dyn ProjectionView + 'a>,
        shape: Arc<Shape>,
    ) -> Result<Reader<'a>> {
        let position = view.position().unwrap_or(Position::GENESIS);
        Ok(Reader {
            lake,
            who: sight.who(),
            sight,
            view,
            shape,
            at_head: at.as_of.is_none(),
            position,
            valid_at: at.valid_at,
            now: crate::lake::wall_clock(),
            computed: RefCell::default(),
            bound: RefCell::default(),
            computing: RefCell::default(),
            kept_reads: RefCell::default(),
            probed: RefCell::default(),
            computes_known: RefCell::default(),
            head: None,
            held_now: false,
            retired: Cell::new(false),
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

    /// The records of `ty` whose fields hold every value of one of the
    /// tuples `any`, as [`Self::get_many`] reads them, and the key the
    /// next page reads after where more follow: of a type that is not a
    /// view, a page of at most [`MATCHED`] in key order after `after`; a
    /// view's all at once.
    fn matching(
        &self,
        ty: &TypeName,
        any: &[Vec<(FieldName, Value)>],
        after: Option<RecordKey>,
    ) -> Result<(Vec<StoredRecord>, Option<RecordKey>)> {
        let Some(all) = self.computed(ty)? else {
            let scan = Scan {
                equal_any: any.to_vec(),
                limit: MATCHED,
                after: after.map(|key| After {
                    values: Vec::new(),
                    key,
                }),
                restricted: true,
                ..Scan::default()
            };
            let page = self.view.scan(ty, &scan)?;
            let next =
                (page.last().map(|r| r.key.clone())).filter(|_| page.len() == MATCHED as usize);
            return Ok((self.at_read(ty, page), next));
        };
        let holds = |r: &StoredRecord, t: &[(FieldName, Value)]| {
            t.iter().all(|(f, v)| r.fields.get(f) == Some(v))
        };
        let matched = all.iter().filter(|r| any.iter().any(|t| holds(r, t)));
        Ok((matched.cloned().collect(), None))
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

    /// The declarations in force at `at`, if the lake can be read there.
    fn ontology_at(&self, at: Position) -> Option<Ontology> {
        let view = self.lake.engine.view(Some(at)).ok()?;
        Ontology::at(view.as_ref()).ok()
    }

    /// The type whose records a read reads, if the principal is shown it
    /// ([`Sight::ty`]). What is shown does not turn on the read's valid
    /// time: a listing asks the sight alone.
    fn visible_type(&self, ty: &TypeName) -> Result<&TypeDecl> {
        let decl = self.sight.ty(ty)?;
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
        self.live_by(decl, &self.sight.admit(&decl.name), r)
    }

    /// [`Self::live`] with what is admitted of `decl` worked out once for a
    /// batch.
    fn live_by(&self, decl: &TypeDecl, admit: &Admit, r: &StoredRecord) -> bool {
        (!r.retired || self.retired.get())
            && !self.sight.withheld(r)
            && admit.holds(&r.fields)
            && (self.valid_at_for(decl))
                .is_none_or(|t| valid_time::record_holds(decl, &r.fields, t))
            && (self.valid_for(decl).1).is_none_or(|b| self.holds(&b, &r.fields, None))
    }

    /// Whether a view's records are computed for this reader from the
    /// admitted records of what it reads, never served kept: under its
    /// `only` (051), or, without `restricted`, over a link type, or a view
    /// computed so, whose links are judged by their ends, as no kept face
    /// can (006).
    fn computes(&self, ty: &TypeName) -> bool {
        if let Some(known) = self.computes_known.borrow().get(ty) {
            return *known;
        }
        let computes = self.computes_once(ty);
        self.computes_known
            .borrow_mut()
            .insert(ty.clone(), computes);
        computes
    }

    /// [`Self::computes`], asked: each source through [`Self::computes`]
    /// again.
    fn computes_once(&self, ty: &TypeName) -> bool {
        let find = |t: &TypeName| self.shape.ontology.ty(t).ok();
        let view = find(ty).and_then(|d| d.view.as_deref());
        let mut sources = view
            .into_iter()
            .flat_map(|v| std::iter::once(&v.source).chain(v.merge.iter().map(|m| &m.source)));
        let links = |s: &TypeName| lakelands_core::links::is_link_type(s) || self.computes(s);
        rights::computed_only(self.who, ty, &find)
            || (!self.who.grants.restricted && sources.any(links))
    }

    /// Whether this reader is admitted every record of `decl` as stored.
    fn whole(&self, decl: &TypeDecl) -> bool {
        let admit = self.sight.admit(&decl.name);
        admit.pairs().is_some_and(|p| p.is_empty()) && !self.sight.judges_ends(decl)
    }

    /// Of link records of `decl`, which have every end this reader holds
    /// `only` on admitted: each such end type's records read once for the
    /// batch. Any other type's records all pass.
    fn admitted_ends(&self, decl: &TypeDecl, rows: &[StoredRecord]) -> Result<Vec<bool>> {
        let rows: Vec<_> = rows.iter().map(|r| &r.fields).collect();
        (self.sight).ends(decl, &rows, &mut |end, keys| self.view.get_many(end, keys))
    }

    /// A read's condition with the filter of each set it names in the
    /// name's place, as the ontology read holds it. A set of a type its
    /// reader is not shown, or whose filter, with the sets it names, reads
    /// a field its reader is kept from is, to that reader, a set that is
    /// not declared (024): the refusal names nothing of the type or field.
    /// Over a view's records, which carry no provenance, a condition that
    /// reads one is refused (028, extended).
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
            let of = self.sight.ty(&s.of);
            of.is_ok_and(|t| {
                reads.is_ok_and(|r| rights::check_probe(self.who, t, r.iter()).is_ok())
            })
        };
        let set = |n: &str| self.shape.ontology.set(n).filter(shown);
        let e = crate::declare::filter(&set, ty, c, Eval::Record, at)?;
        let view = self.shape.ontology.ty(ty).is_ok_and(|t| t.view.is_some());
        crate::declare::record_stands(!view, e, at)
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
        let mut lake = self.sight.describe(&self.lake.id);
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
        let missing = Missing::Type { name: ty.clone() };
        let mut t = (self.sight.describe_type(ty)).ok_or(Error::NotFound { missing })?;
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
            // face: a number of groups says something of their records. A
            // read held as now computes every view, as `kept_as` does.
            } else if t.lifecycle.kept.as_ref() == Some(&open)
                && !self.held_now
                && self.probe_view(view).is_ok()
            {
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
        let Some(equal) = self.sight.admit(&decl.name).pairs().map(<[_]>::to_vec) else {
            return Ok(0);
        };
        // A link's ends are judged a page at a time, never by the engine.
        let ends = self.sight.judges_ends(decl);
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
            retired: self.retired.get(),
            restricted: self.who.grants.restricted,
            ..Default::default()
        };
        self.view.count(&decl.name, &scan)
    }

    /// How many retired records of a type [`Self::count`] would count were
    /// they live, under the same filter, grants and position: the count
    /// with the retired ones, less the live count, `live` where the read
    /// counted it already. A view's records are computed and never
    /// retired (045).
    fn retired_count(
        &self,
        decl: &TypeDecl,
        filter: Option<&Expr>,
        live: Option<u64>,
    ) -> Result<u64> {
        if decl.view.is_some() {
            return Ok(0);
        }
        let live = live.map_or_else(|| self.count(decl, filter), Ok)?;
        self.retired.set(true);
        let all = self.count(decl, filter);
        self.retired.set(false);
        Ok(all?.saturating_sub(live))
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
        // A view is computed once a read, however many views read it, and
        // a reader's `where` on it held after, as its computation holds it
        // (lakelands#942).
        if let Some(all) = self.computed(&decl.name)? {
            let bounded = self.with_bound(decl, filter);
            let held = |r: &&StoredRecord| {
                (bounded.as_ref()).is_none_or(|e| self.holds(e, &r.fields, None))
            };
            return all.iter().filter(held).try_for_each(each);
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
        let admit = self.sight.admit(&decl.name);
        let Some(equal) = admit.pairs().map(<[_]>::to_vec) else {
            return Ok(Vec::new());
        };
        let mut scan = Scan {
            filter: push,
            equal,
            order_by: order.to_vec(),
            after: after.map(|(values, key)| After { values, key }),
            limit: u32::try_from(want).unwrap_or(u32::MAX),
            valid_at: self.valid_at_for(decl),
            retired: self.retired.get(),
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
                if self.live_by(decl, &admit, &r)
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
        let admit = self.sight.admit(&decl.name);
        records.map(|mut r| {
            r.retain(|r| admit.holds(&r.fields));
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
        let admit = self.sight.admit(&stored.name);
        self.each_stored(
            &stored,
            None,
            page,
            &mut |r| match kept::shown(r.clone(), view, whole) {
                Some(r)
                    if filter.is_none_or(|e| self.holds(e, &r.fields, None))
                        && admit.holds(&r.fields) =>
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
        let admit = self.sight.admit(&stored.name);
        let live = held
            .into_iter()
            .filter(|r| self.live_by(&stored, &admit, r));
        let shown = live.filter_map(|r| kept::shown(r, view, whole));
        Ok(shown.filter(|r| admit.holds(&r.fields)).collect())
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
        if self.held_now {
            return Ok(None);
        }
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
        let admit = self.sight.admit(&stored.name);
        let (true, Some(pairs)) = (whole, admit.pairs()) else {
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
        let admitted = |w: &StoredRecord| admit.holds(&w.fields);
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
            // A measure that does not land fails as one that cannot be
            // computed: the read is refused, naming it and its group.
            let fields = landed(decl, by.clone(), values).map_err(|(name, why)| {
                let (group, decimal) = (key.clone(), matches!(why, Case::IntegerTooLarge { .. }));
                let why = Box::new(why);
                let said = Said::from(Case::MeasureUnfit {
                    group,
                    why,
                    decimal,
                });
                let view = Place::View {
                    name: decl.name.clone(),
                };
                invalid(said.within(Place::Measure { name }).within(view))
            })?;
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
        let (source, where_, measures) =
            self.view_probe(source, filter, group_by, measures, literals, true)?;
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
                members.add((&r.fields, Some(&r.meta)), Some(&self.shape.settings));
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
    ) -> Result<Option<KeptRead>> {
        let at = (ty.clone(), face);
        if let Some(known) = self.kept_reads.borrow().get(&at) {
            return Ok(known.clone());
        }
        let read = self.kept_read_once(ty, v, face)?;
        self.kept_reads.borrow_mut().insert(at, read.clone());
        Ok(read)
    }

    /// [`Self::kept_read`], asked: each source that is a view through
    /// [`Self::kept_read`] again.
    fn kept_read_once(
        &self,
        ty: &TypeName,
        v: &lakelands_api::ontology::ViewDecl,
        face: usize,
    ) -> Result<Option<KeptRead>> {
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
                self.view_probe(source, filter, group_by, measures, &BTreeMap::new(), false)?;
            if let Some(inner) = &source.view
                && !self.probed.borrow().contains(&source.name)
            {
                self.probe_view(inner)?;
                self.probed.borrow_mut().insert(source.name.clone());
            }
        }
        Ok(())
    }

    /// What a view reads of one source, the parameters bound: the source,
    /// its `where` and its measures, after the principal is found to read
    /// every field they and the grouping read (024), and, where the source
    /// is `read` for the computation, those its valid time filters by.
    fn view_probe(
        &self,
        source: &TypeName,
        filter: Option<&lakelands_api::expr::Condition>,
        group_by: &[FieldName],
        m: &[lakelands_api::ontology::Measure],
        literals: &BTreeMap<String, Literal>,
        read: bool,
    ) -> Result<Probed> {
        let source = self.shape.ontology.ty(source)?.clone();
        let bound = |e: Expr| expr::bind_params(&e, literals);
        let where_ = filter
            .map(|w| condition(w, Eval::View, &[Place::Where]).map(bound))
            .transpose()?;
        let measures: Vec<(FieldName, Expr)> = m
            .iter()
            .map(|m| {
                let at = [Place::Measure {
                    name: m.name.clone(),
                }];
                let e = condition(&m.expr, Eval::Group, &at)?;
                Ok((m.name.clone(), bound(e)))
            })
            .collect::<Result<_>>()?;
        // What it reads of the source is what `describe` shows it by; one
        // that cannot be read there is read by no one.
        let probed = lakelands_core::describe::source_reads(filter, group_by, m);
        let mut probed = probed.ok_or_else(|| crate::writes::refused(Refusal::NotGranted))?;
        // The computation at a valid time filters the source by its
        // valid-time fields besides, which `describe` does not judge by: it
        // probes those its reader may know of (024, lakelands#881). Kept
        // records are filtered by none.
        if read && self.valid_at.is_some() {
            let timed = source.valid_from.iter().chain(&source.valid_to);
            probed.extend(timed.filter(|n| self.knows(&source, n)).cloned());
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
            // has an event of its own to order by, nor a provenance (028,
            // extended).
            if o.by == OrderBy::Latest && decl.view.is_some() {
                let ty = Some(ty.clone());
                return Err(invalid(Case::LatestNoChange { ty }));
            }
            if o.by != OrderBy::Value && decl.view.is_some() {
                let case = Case::NoRecordsProvenance;
                return Err(crate::declare::invalid_field(&o.field, case));
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
                        let at = vec![Place::Group { name }];
                        let e = crate::declare::record_stands(decl.view.is_none(), e, &at)?;
                        probed.extend(core_derive::reads(&e));
                        asked.push((e, at));
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
            .filter_map(|(_, r)| rights::visible(self.sight, &decl, &r, find.with_meta))
            .collect();
        // Counted at the same position, so every page of a walk agrees.
        let total = (find.total)
            .then(|| self.count(&decl, filter.as_ref()))
            .transpose()?;
        let retired = (find.retired)
            .then(|| self.retired_count(&decl, filter.as_ref(), total))
            .transpose()?;
        Ok(Found::Records {
            rows,
            next_cursor,
            total,
            retired,
        })
    }

    /// The rows `find` skips before its page: none unless it names an
    /// `offset`, which is refused beside a cursor, an `after` or an
    /// aggregate (each already says where the answer starts, or has no
    /// rows), and past the lake's `max_offset` as too large; `total` and
    /// `retired` are refused beside an aggregate too
    /// (`docs/decisions/045-a-table-reads-a-page-and-its-total.md`).
    fn offset(&self, find: &Find) -> Result<usize> {
        if find.total && find.aggregate.is_some() {
            return Err(invalid(Case::TotalWithAggregate));
        }
        if find.retired && find.aggregate.is_some() {
            return Err(invalid(Case::RetiredWithAggregate));
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
            grouping.add((&r.fields, Some(&r.meta)), Some(&self.shape.settings));
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
            .and_then(|r| rights::visible(self.sight, &decl, &r, get.with_meta))
            .ok_or_else(missing)
    }

    /// `proposals`: for a reviewer every proposal there to it, each as
    /// its view; one's own otherwise.
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
        // The lake as its reader is described it, read once for the
        // listing and only where a proposal asks.
        let lake = OnceCell::new();
        let mut mine = Vec::new();
        for mut p in all {
            if !self.who.grants.review && p.proposer != me {
                continue;
            }
            if let Some(logged) = p.write.take() {
                let Some(shown) = self.entry(asked, &logged, (p.position, p.proposed_at), &lake)?
                else {
                    continue;
                };
                (p.write, p.more) = shown;
            }
            mine.push(p);
        }
        let verb_on = |v: &VerbName| self.shape.ontology.verb(v).ok().map(|d| d.on.clone());
        mine.retain(|p| core_engines::matches(p, filter, verb_on));
        Ok(ordered_proposals(mine, &filter.order_by)?
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect())
    }

    /// The proposal logged at `at`, writing `logged`, as the listing shows
    /// it to its reader, and whether it holds more; `None` where it is not
    /// there to it (lakelands#762). The listing and the accept ask this
    /// alone, so an entry is decided as it is listed (006 and 013,
    /// extended).
    ///
    /// The reader is `asked`, the principal as its call named it, judged
    /// under up to three sights, its grants followed to each: the lake in
    /// force here; the lake as the entry would leave it, a bundle's whole
    /// and of a migration its `to`; and the lake as it was proposed
    /// against, where the type a migration reshapes or the verb a call
    /// calls was declared again since, read only for a reader its grants
    /// keep from a field. An entry is not there when one of them hides a
    /// type it names: the type it writes or a called verb is on; of a
    /// migration's `to` and of each type a bundle declares, what
    /// [`TypeDecl::names_against`] names against the type in force; and
    /// the type each verb, trigger and object set of a bundle is on, with
    /// the type the one in force under its name stands on, which would be
    /// gone with it (lakelands#893). A link type is shown only with both
    /// its ends, and a type a bundle brings only as `describe` would show
    /// it once in force. Nor is a call there whose verb, as either lake
    /// declares it, reads a field its reader may not read: `describe`
    /// does not name the verb, and the entry would tell that its
    /// `requires` held (024, extended). A migration's report counts every
    /// owner's records, so a holder of `only` is listed none (051). A read
    /// the engine fails is the listing's error, never an entry left out
    /// (lakelands#927).
    pub(crate) fn entry(
        &self,
        asked: &Principal,
        logged: &ProposedWrite,
        (at, proposed_at): (Position, Timestamp),
        lake: &OnceCell<LakeDescription>,
    ) -> Result<Option<(Option<ProposedWrite>, bool)>> {
        let (o, mut logged) = (&self.shape.ontology, logged.clone());
        // A shape that is not in force is completed as one in force is
        // before it is judged or compared.
        let names = |t: &TypeDecl| t.names_against(o.ty(&t.name).ok());
        let (mut named, mut after): (Vec<TypeName>, _) = (Vec::new(), None);
        match &mut logged {
            ProposedWrite::Upsert { ty, .. } | ProposedWrite::Retire { ty, .. } => {
                named.push(ty.clone())
            }
            ProposedWrite::Migrate { migration: m, .. } => {
                o.complete(&mut m.to);
                named.extend(names(&m.to));
                let mut renames = self.shape.renames.clone();
                renames.push(Renamed {
                    at: self.position.next(),
                    ty: m.to.name.clone(),
                    renames: m.renames.clone(),
                    to: None,
                });
                let left = Sight::new(asked.clone(), &renames, None, o);
                if rights::holds_only(self.who) || !left.would_show(&m.to) {
                    return Ok(None);
                }
            }
            ProposedWrite::Declare { declarations } => {
                let mut put = o.clone();
                for d in declarations.iter() {
                    crate::writes::put_as_appended(&mut put, d, Some);
                }
                for d in declarations.iter_mut() {
                    match d {
                        Declaration::Type(t) => {
                            put.complete(t);
                            named.extend(names(t))
                        }
                        Declaration::Verb(v) => {
                            named.extend(o.verb(&v.name).ok().map(|x| x.on.clone()));
                            named.push(v.on.clone())
                        }
                        Declaration::Trigger(t) => {
                            let was = o.triggers.get(t.name.as_str());
                            named.extend(was.map(|x| x.decl.on.clone()));
                            named.push(t.on.clone())
                        }
                        Declaration::ObjectSet(s) => {
                            named.extend(o.set(&s.name).map(|x| x.of.clone()));
                            named.push(s.of.clone())
                        }
                        Declaration::ValueType(_) | Declaration::Function(_) => {}
                    }
                }
                after = Some((put.declared(), put));
            }
            ProposedWrite::MigrateValueType { .. } if rights::holds_only(self.who) => {
                return Ok(None);
            }
            ProposedWrite::Call { .. } | ProposedWrite::MigrateValueType { .. } => {}
        }
        let subject = |d: &Declaration| match (d, &logged) {
            (Declaration::Type(t), ProposedWrite::Migrate { migration: m, .. }) => {
                t.name == m.to.name
            }
            (Declaration::Verb(v), ProposedWrite::Call { verb, .. }) => &v.name == verb,
            _ => false,
        };
        let mut declared = self.shape.declared.iter();
        let moved = declared.any(|d| d.since > at && subject(&d.declaration));
        let reads = |a: &Access| a.reads();
        let typed =
            |t: &TypeGrants| t.default.as_ref().is_none_or(reads) && t.fields.values().all(reads);
        let g = &self.who.grants;
        let kept = !(reads(&g.default) && g.types.values().all(typed));
        // A lake that cannot be read there shows its entry to no one.
        let was = match moved && kept {
            true => match self.ontology_at(at) {
                Some(was) => Some(was),
                None => return Ok(None),
            },
            false => None,
        };
        let renames = &self.shape.renames;
        let then = was.as_ref();
        let then = then.map(|was| (Sight::new(asked.clone(), renames, Some(at), was), was));
        let left = after.as_ref();
        let left = left.map(|(all, put)| (Sight::new(asked.clone(), renames, None, all), put));
        let then: Under<'_> = then.as_ref().map(|(s, o)| (s, *o));
        let left: Under<'_> = left.as_ref().map(|(s, o)| (s, *o));
        let force: Under<'_> = Some((self.sight, o));
        if let ProposedWrite::Call { verb, .. } = &logged {
            for (sees, o) in [force, then].into_iter().flatten() {
                let Ok(v) = o.verb(verb) else { continue };
                let on = o.ty(&v.on);
                let far = |n: &TypeName| o.ty(n).ok();
                if on.is_ok_and(|on| !rights::reads_what_verb_reads(sees.who(), on, v, &far)) {
                    return Ok(None);
                }
                named.push(v.on.clone());
            }
        }
        let hides = |(sees, o): (&Sight, &Ontology)| {
            let mut named = named.iter();
            named.any(|ty| o.ty(ty).is_ok() && sees.ty(ty).is_err())
        };
        if [force, left, then].into_iter().flatten().any(hides) {
            return Ok(None);
        }
        // Nor is one that names a record its reader is not answered beside
        // its own, by a reference or read across by a call, as it names it
        // now or named it when proposed: a condition or a reference a
        // setting, the clock or the record decide may come to a record at
        // one and not at the other. It would tell of that record, and its
        // accept is not granted (006). Nor is one whose own record or a
        // link's end it is not answered, held or another's (lakelands#1008,
        // lakelands#1016): an `only` lists in part only what it is shown,
        // trimmed (lakelands#867). A retired record that is not held is,
        // as to its writes, a free key.
        let (g, view) = (&self.who.grants, self.view.as_ref());
        if !g.restricted || rights::holds_only(self.who) {
            let settings = crate::lake::settings_in(view, o)?;
            let now = (view, &settings, crate::lake::wall_clock());
            let mut named = crate::writes::reached(self.lake, o, now, &logged, true)?;
            let since = self.shape.declared.iter().any(|d| d.since > at);
            if since || crate::writes::reaches_by_state(o, &logged) {
                let then = self.lake.engine.view(Some(at))?;
                let lake = self.lake;
                let was =
                    crate::writes::reached_then(lake, then.as_ref(), proposed_at, &logged, false);
                named.extend(was?);
            }
            for (t, k) in &named {
                let there = view.get(t, k)?;
                let held = there.is_some_and(|r| !r.retired || self.sight.withheld(&r));
                if held && !answered(view, self.sight, t, k)? {
                    return Ok(None);
                }
            }
        }
        // `more` is what the trim left out, not what the completion added.
        let mut shown = logged.clone();
        let known = self.known_shape(asked, &mut shown, lake, at, (left, then))?;
        let shown = known.then_some(shown);
        // A value type's report is made again for its reader, its conditions
        // judged and its id made over what it is shown: the logged one is
        // its proposer's, so the reader's is compared with the whole one,
        // as a reader shown everything is shown it.
        let mut whole = logged.clone();
        if matches!(whole, ProposedWrite::MigrateValueType { .. }) {
            let mut all = asked.clone();
            all.grants = lakelands_api::Grants::owner();
            self.value_type_shown(&all, &|_, _| true, &mut whole, at);
        }
        let more = shown.as_ref() != Some(&whole);
        Ok(Some((shown, more)))
    }

    /// A proposed write as its reader may know it (013 and 024, extended):
    /// a value where a read of the record would show it, a name where
    /// `describe` would. False when the reader may see nothing of it: a
    /// write to a type it may not see, or to a record it is not answered.
    /// The store logs a migration and a redeclaration completed with the
    /// fields hidden from their author, who reads its own proposal here.
    fn known_shape(
        &self,
        asked: &Principal,
        write: &mut ProposedWrite,
        lake: &OnceCell<LakeDescription>,
        at: Position,
        (left, then): (Under<'_>, Under<'_>),
    ) -> Result<bool> {
        let read = |ty: &TypeName, field: &FieldName| self.sight.reads(ty, field);
        // A record the reader is not answered: restricted, without the
        // grant, or one its `only` does not admit, before the write or
        // after it as proposed (051). An `only` that cannot be applied
        // admits none, whatever a record holds (lakelands#771).
        let only = |ty: &TypeName, f: &BTreeMap<_, _>| {
            self.sight.ty(ty).is_ok() && self.sight.admit(ty).holds(f)
        };
        // To a holder of `only` on the type, the record as the accept
        // plans it: its derived values computed (lakelands#914).
        let o = &self.shape.ontology;
        let view = self.view.as_ref();
        let holder = |ty: &TypeName| {
            o.ty(ty)
                .is_ok_and(|t| lakelands_core::rights::only_holder(self.who, t, o))
        };
        let now = || {
            Ok::<_, Error>((
                crate::lake::settings_in(view, o)?,
                crate::lake::wall_clock(),
            ))
        };
        // A plan that fails on the engine is its outage (lakelands#927);
        // one the write itself fails leaves the record not admitted.
        let outage = |e: Error| match e {
            Error::Unavailable { .. } => Err(e),
            _ => Ok(false),
        };
        let after = |ty: &TypeName, key: &RecordKey, patch: &lakelands_api::Patch| {
            let stored = self.view.get(ty, key)?;
            if holder(ty) {
                let (settings, now) = now()?;
                let at = (view, &settings, now);
                return match crate::writes::planned(
                    self.lake,
                    o,
                    at,
                    stored.as_ref(),
                    (ty, key),
                    patch,
                ) {
                    Ok(planned) => Ok(planned.is_some_and(|r| only(ty, &r.fields))),
                    Err(e) => outage(e),
                };
            }
            let mut fields = (stored.filter(|r| !r.retired))
                .map(|r| r.fields)
                .unwrap_or_default();
            fields.extend(patch.clone());
            Ok::<_, Error>(only(ty, &fields))
        };
        let answered = |ty: &TypeName, key: &RecordKey| answered(view, self.sight, ty, key);
        match write {
            // A field the reader may not know of and a masked one both
            // leave the patch: it holds values, and no place for a name.
            // A retired record that is not held is a free key, as to the
            // reader's own writes.
            ProposedWrite::Upsert { ty, key, patch, .. } => {
                let admitted = after(ty, key, patch)?;
                patch.retain(|field, _| read(ty, field));
                let stored = self.view.get(ty, key)?;
                let free = stored.is_some_and(|r| r.retired && !self.sight.withheld(&r));
                Ok(admitted && (free || answered(ty, key)?))
            }
            ProposedWrite::Retire { ty, key, .. } => answered(ty, key),
            // A payload is what the verb's fields become: whole to a reader
            // who may read each field the verb sets, as it is declared and
            // as it was where the call was proposed, or see the type an
            // executor acts on, and empty otherwise. The verb's name is its
            // type's. One whose verb reads a field the reader may not read
            // is not listed ([`Self::entry`]).
            ProposedWrite::Call {
                verb, key, payload, ..
            } => {
                let Ok(v) = self.shape.ontology.verb(verb) else {
                    return Ok(false);
                };
                let sets = |(sees, o): (&Sight, &Ontology)| match o.verb(verb) {
                    Ok(VerbDecl {
                        on,
                        effect: VerbEffect::Write { set },
                        ..
                    }) => set.iter().all(|a| sees.reads(on, &a.field)),
                    _ => true,
                };
                // A call to a holder of `only` is judged by what its
                // assignments leave, the payload as proposed, as an upsert
                // is (lakelands#914).
                let leaves = |key: &RecordKey| {
                    if !holder(&v.on) {
                        return Ok(true);
                    }
                    let (settings, now) = now()?;
                    let at = (view, &settings, now);
                    match crate::writes::call_reads(o, at, v, Some(key), payload) {
                        Ok((set, _)) => after(&v.on, key, &set),
                        Err(e) => outage(e),
                    }
                };
                let whole = match key {
                    Some(key) => answered(&v.on, key)? && leaves(key)?,
                    None => self.sight.ty(&v.on).is_ok(),
                };
                let force = Some((self.sight, &self.shape.ontology));
                if ![force, then].into_iter().flatten().all(sets) {
                    payload.clear();
                }
                Ok(whole)
            }
            // The report names what the migration does to the type it was
            // proposed against. A reader kept from a field, where the type
            // was declared again since, is judged by the sight of that
            // lake; one who reads every field is judged on the type in
            // force and is listed the report whole. The totals stay whole.
            // A logged report is lossless, so it names no
            // record that would not convert or would collide. Its id is
            // made again at the head before the proposal.
            ProposedWrite::Migrate { migration, report } => {
                let Ok(was) = self.sight.ty(&migration.to.name) else {
                    return Ok(false);
                };
                let (judge, listing) = then.unwrap_or((self.sight, &self.shape.ontology));
                let made = listing.ty(&migration.to.name).unwrap_or(was);
                let then = judge.who();
                let sees = |ty: &TypeName| self.sight.ty(ty).ok();
                let all = || self.sight.describe(&self.who.lake);
                let described = || lake.get_or_init(all);
                rights::shown_report(then, made, migration, report, described, sees);
                // One listing: its `to` hash is that of the `to` it lists.
                let find = |n: &TypeName| listing.ty(n).ok();
                rights::shown_hashes(then, made, migration, report, &find);
                let before = Position(at.0.saturating_sub(1));
                report.report_id = crate::writes::report_id(report, Some(before));
                rights::known_migration(then, made, migration, &find);
                Ok(true)
            }
            // Each declaration as `describe` would show it once the bundle
            // is in force; one it would not show leaves the list.
            ProposedWrite::Declare { declarations } => {
                // The reader as the bundle in force would show it its
                // references (024, extended), from its grants as asked: a
                // reference hidden by the lake as it stands may be shown.
                let Some((sight, after)) = left else {
                    return Ok(false);
                };
                declarations.retain(|d| sight.lists(d));
                for d in declarations.iter_mut() {
                    if let Declaration::Type(t) = d {
                        rights::described_fields(sight.who(), t, &|n| after.ty(n).ok());
                    }
                }
                Ok(!declarations.is_empty())
            }
            // Its uses and conditions as the reader may know them.
            ProposedWrite::MigrateValueType { .. } => {
                self.value_type_shown(asked, &read, write, at);
                Ok(true)
            }
        }
    }

    /// A value type's migration as `asked` may know it, where `read` says
    /// which fields it reads: a use says something of a field's values, so
    /// it is shown only in a field the reader may read, of a type it may see
    /// (024), with its renames' counts. The totals and `fits` stay whole, as
    /// the preflight's do (`Call::shown_value_type`); a logged report fits,
    /// so its `first` names no misfit. Its conditions are judged again for
    /// the reader with its grants at the proposal, as the lake stood before
    /// it, where its id is made again; a failed read of that lake tells each
    /// only as one there may be.
    fn value_type_shown(
        &self,
        asked: &Principal,
        read: &dyn Fn(&TypeName, &FieldName) -> bool,
        write: &mut ProposedWrite,
        at: Position,
    ) {
        let ProposedWrite::MigrateValueType {
            to,
            renames,
            report,
        } = write
        else {
            return;
        };
        report.fields.retain(|u| read(&u.ty, &u.field));
        let before = Position(at.0.saturating_sub(1));
        if !renames.is_empty() {
            let old = renames
                .iter()
                .map(|(o, n)| (o.as_str(), n.as_str()))
                .collect();
            let view = self.lake.engine.view(Some(before));
            // Its references held as that lake shows them (024, extended).
            let judged = view.and_then(|v| {
                let o = Ontology::at(v.as_ref())?;
                let then = Sight::new(asked.clone(), &self.shape.renames, Some(at), &o);
                crate::writes::conditions(&o, v.as_ref(), then.who(), report, &old)
            });
            if judged.is_err() {
                report.conditions.clear();
                report.conditions_unshown = true;
            }
        }
        report.report_id = String::new();
        report.report_id = crate::writes::value_report_id(report, to, renames, before.0);
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
        // The names, the rights and the `where` are read at the read's
        // position; the rankings are judged by the head's declaration, whose
        // indexes they read, each field under the name it bears there (025).
        let texts = match text_fields {
            Some(named) => Box::new(named.iter()) as Box<dyn Iterator<Item = &FieldName>>,
            None => Box::new(rights::searched_fields(self.who, &decl)),
        };
        let ranks = self.ranks(&decl, vector_fields.iter().copied().chain(texts))?;
        // A vector ranking reads a vector field, and a hybrid search without
        // a vector reads one a model fills: one that cannot use its ranking
        // is refused here, before any embedding is asked for (lakelands#743).
        // A field the head does not show its reader holds no vectors there,
        // and no model fills it.
        let declared = |n: &FieldName| ranks.fields.get(n);
        for field in &vector_fields {
            if let Some(f) = declared(field).filter(|f| f.ty.kind != StorageKind::Vector) {
                let kind = f.ty.kind;
                let case = Case::SearchNotVector {
                    field: (*field).clone(),
                    kind,
                };
                return Err(crate::declare::invalid_field(field, case));
            }
        }
        if let SearchMode::Hybrid {
            field,
            vector: None,
            ..
        } = &s.mode
            && declared(field).is_none_or(|f| f.embed.is_none())
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
            if let Some(d) = declared(field).and_then(|f| f.ty.dimensions)
                && v.len() != d as usize
            {
                let (query, declared) = (v.len(), d as usize);
                let field = field.as_str();
                return Err(core_engines::dimensions(query, declared, &s.ty, field));
            }
        }
        let filter = self.record_filter(&decl, s.filter.as_ref(), &[Place::Where])?;
        Ok((decl, text_fields, filter, ranks))
    }

    /// The rankings over `named` fields of `decl` as the head declares
    /// them: the type and each field followed through the lake's renames
    /// since the read's position, and a field kept only where the reader,
    /// its grants followed to the head, is shown it there. Following a name
    /// never reaches a field the reader is not shown at the head (051). A
    /// read before the head that was not given the head ranks by nothing.
    fn ranks<'n>(
        &self,
        decl: &TypeDecl,
        named: impl Iterator<Item = &'n FieldName>,
    ) -> Result<Ranks> {
        let (shape, who) = match (self.at_head, &self.head) {
            (true, _) => (&self.shape, self.who),
            (false, Some(h)) => (&h.shape, &h.who),
            (false, None) => {
                let (ty, decl, fields) = (decl.name.clone(), None, BTreeMap::new());
                return Ok(Ranks { ty, decl, fields });
            }
        };
        let mut ty = decl.name.clone();
        let mut names: Vec<(FieldName, FieldName)> =
            named.map(|n| (n.clone(), n.clone())).collect();
        for r in shape.renames.iter().filter(|r| r.at > self.position) {
            if r.ty != ty {
                continue;
            }
            for (_, now) in &mut names {
                if let Some((_, to)) = r.renames.iter().find(|(from, _)| from == now) {
                    *now = to.clone();
                }
            }
            if let Some(to) = &r.to {
                ty = to.clone();
            }
        }
        let decl = shape.ontology.ty(&ty).ok().cloned();
        let shown = |now: &FieldName| {
            let d = decl.as_ref()?;
            let f = d.fields.iter().find(|f| &f.name == now)?;
            rights::field_access(who, d, f).reads().then(|| f.clone())
        };
        let fields = (names.iter())
            .filter_map(|(asked, now)| Some((asked.clone(), shown(now)?)))
            .collect();
        Ok(Ranks { ty, decl, fields })
    }

    /// `search`, `carried` the pull the door made this read for, if it did:
    /// it is judged here, at the position it was read at.
    pub fn search(&self, s: &Search, mut carried: Option<Pulled>) -> Result<Judged<Hits>> {
        let (decl, text_fields, filter, ranks) = self.searched(s)?;
        let vectors = s.mode.vectors();
        let vector_fields: Vec<&FieldName> = vectors.iter().map(|v| v.0).collect();
        // A search that names no fields reads only the searchable fields
        // the principal reads (024); none hidden or masked, it reads them all.
        // Before the head, each is read under its name there, where the
        // head declares it searchable.
        let hidden = (decl.fields.iter())
            .any(|f| f.searchable && !rights::field_access(self.who, &decl, f).reads());
        let known: Option<Vec<FieldName>> = (hidden || text_fields.is_some() || !self.at_head)
            .then(|| {
                let named = text_fields.map(|n| n.iter().collect::<Vec<_>>());
                let named =
                    named.unwrap_or_else(|| rights::searched_fields(self.who, &decl).collect());
                let now = named.into_iter().filter_map(|n| ranks.fields.get(n));
                now.filter(|f| f.searchable)
                    .map(|f| f.name.clone())
                    .collect()
            });
        let text_fields = known.as_deref();
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
        // A search without a query vector ranks its text alone; one over
        // several vector fields answers an index's set when any of them is
        // asked for an index.
        let (mut by_text, mut indexed) = (true, false);
        if !matches!(&s.mode, SearchMode::Hybrid { vector: None, .. }) {
            for field in &vector_fields {
                let asked = |(d, f): (&TypeDecl, &FieldDecl)| self.indexed(d, &f.name);
                let at_head = ranks.decl.as_ref().zip(ranks.fields.get(*field));
                (by_text, indexed) = (
                    false,
                    indexed || at_head.map(asked).transpose()? == Some(true),
                );
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
        // The rankings a search reads, each with its weight, in the order
        // they are summed (046): its text, then each vector field; a search
        // of one ranking fuses nothing, and its axis is listed without a
        // count, since it reads no further than its page (lakelands#560).
        // The engine is asked for all of them at once, under the head's
        // names, and before the head the text is ranked as the head holds
        // it, as the vectors are; a ranking over no field the head shows
        // the reader has no candidate, and is not asked.
        let (mut asked, mut lists, mut axes) = (Rankings::default(), Vec::new(), Vec::new());
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
        if let SearchMode::Lexical { text, .. } | SearchMode::Hybrid { text, .. } = &s.mode {
            let weight = match &s.mode {
                SearchMode::Hybrid { text_weight, .. } => text_weight.or(text_default),
                _ => None,
            };
            let ranked = text_fields != Some(&[]);
            if ranked {
                asked.text = Some((text.clone(), text_fields.map(<[FieldName]>::to_vec)));
            }
            lists.push((weight.unwrap_or(1.0), ranked));
            axes.push(axis(None));
        }
        for (field, v, weight) in vectors.iter().filter(|_| !by_text) {
            let set = || field_weights.get(field.as_str()).copied();
            let at_head = ranks.fields.get(*field);
            asked
                .vectors
                .extend(at_head.map(|f| (f.name.to_string(), v.to_vec())));
            lists.push((weight.or_else(set).unwrap_or(1.0), at_head.is_some()));
            axes.push(axis(Some(field)));
        }
        // Candidates are pulled in growing batches until the page and one
        // more are visible, or the engine has no more: a filter, `valid_at`
        // or the grants never shorten a page silently. A list read is a
        // scan of its index whatever it is asked for, so a fused search's
        // first pull asks for the whole window at once. Each pull stands
        // alone: a page is judged from one pull, at the position it was read
        // at, and a read made again for a pull goes on from its depth.
        let fuses = lists.len() > 1;
        let first = if fuses {
            self.lake.limits.max_limit as usize
        } else {
            64
        };
        let mut pull = u32::try_from(want.saturating_mul(4).max(first)).unwrap_or(u32::MAX);
        pull = pull.max(carried.as_ref().map_or(0, |p| p.k));
        let (hits, counts) = loop {
            let mut read = match self.pull(&ranks.ty, &asked, pull, &mut carried)? {
                Ok(read) => read.into_iter(),
                Err(moved) => return Ok(Err(moved)),
            };
            let mut pulled: Vec<(f64, Ranking)> = (lists.iter())
                .map(|(w, ranked)| {
                    (
                        *w,
                        ranked.then(|| read.next()).flatten().unwrap_or_default(),
                    )
                })
                .collect();
            let filter = filter.as_ref();
            let done = if fuses {
                self.fused(&decl, &ranks.ty, filter, pulled, pull, want, s.with_meta)?
            } else {
                let ranked = pulled.pop().map_or_else(Vec::new, |l| l.1);
                let exhausted = ranked.len() < pull as usize;
                let ranked = self.keyed_then(&ranks.ty, ranked);
                let hits = self.visible_hits(&decl, filter, ranked, s.with_meta)?;
                (hits.len() >= want || exhausted || pull == u32::MAX).then(|| (hits, Vec::new()))
            };
            if let Some(done) = done {
                break done;
            }
            pull = pull.saturating_mul(4);
        };
        for ((a, n), (w, _)) in axes.iter_mut().zip(counts).zip(&lists) {
            (a.candidates, a.weight) = (Some(n), Some(*w));
        }
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
        Ok(Ok(Hits {
            hits: page,
            ranked_by,
            next_cursor,
            axes,
        }))
    }

    /// The lists of one pull to the depth `k`, as the engine ranks `asked`
    /// under the head's name `ty`, all from one state: `carried`'s, where
    /// the door made this read for them and it asks the same. They are
    /// handed back unjudged where they were read at another position than
    /// this read's head: a write landed since it was taken.
    fn pull(
        &self,
        ty: &TypeName,
        asked: &Rankings,
        k: u32,
        carried: &mut Option<Pulled>,
    ) -> Result<Judged<Vec<Ranking>>> {
        if *asked == Rankings::default() {
            return Ok(Ok(Vec::new()));
        }
        let head = (self.head.as_ref()).map_or(self.view.position(), |h| h.view.position());
        let same = |p: &Pulled| (&p.ty, &p.asked, p.k, p.position()) == (ty, asked, k, head);
        let pulled = match carried.take().filter(same) {
            Some(pulled) => pulled,
            None if !asked.vectors.is_empty() && self.lake.engine.vectors().is_none() => {
                return Err(Error::unbound(lakelands_api::error::Seam::VectorIndex));
            }
            None => Pulled {
                ty: ty.clone(),
                asked: asked.clone(),
                k,
                ranked: self.lake.engine.ranked(ty, asked, k)?,
            },
        };
        Ok(match pulled.position() == head {
            true => Ok(pulled.ranked.lists),
            false => Err(pulled),
        })
    }

    /// A fused search's hits: its lists fused by weighted reciprocal rank
    /// ([`fuse`]), each ranked among the hits this read may see and to the
    /// lake's `max_limit` of them, so a record the read may not see moves
    /// no score and every page reads one ranking (046). `lists`
    /// are one pull to the depth `pull`; none is answered until the first
    /// `want` fused hits and their scores can no longer change, and each
    /// list is whole or holds its window, so that its count, answered
    /// beside the hits, is the same on every page: the search pulls again.
    #[allow(clippy::too_many_arguments)]
    fn fused(
        &self,
        decl: &TypeDecl,
        named: &TypeName,
        filter: Option<&Expr>,
        lists: Vec<(f64, Ranking)>,
        pull: u32,
        want: usize,
        with_meta: bool,
    ) -> Result<Option<(Vec<Hit>, Vec<u64>)>> {
        let window = self.lake.limits.max_limit as usize;
        // At the head, a read that sees every live record and narrows by
        // nothing sees every candidate an engine ranks, so it reads no
        // record to rank them. That rests on two seam facts: `ranked`
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
        {
            // Each list's visible candidates to the window, and what a
            // record it has not ranked yet could still gain from it:
            // nothing once the list is whole or its window full.
            let (mut keyed, mut gain, mut counted) = (Vec::new(), Vec::new(), true);
            for (weight, candidates) in lists {
                let whole = candidates.len() < pull as usize;
                let mut seen: Vec<(RecordKey, f64)> = if open {
                    candidates
                } else {
                    let candidates = self.keyed_then(named, candidates);
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
                keyed.push((weight, seen));
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
            if !(settled && counted) {
                return Ok(None);
            }
            let counts = keyed.iter().map(|(_, l)| l.len() as u64).collect();
            let first = fused.into_iter().take(want).collect();
            let hits = self.visible_hits(decl, filter, first, with_meta)?;
            Ok(Some((hits, counts)))
        }
    }

    /// An engine's candidates, ranked at the head where the type is
    /// `named`, under the keys they bear at the read's position: before the
    /// head, a re-key since may have moved them (lakelands#797). One whose
    /// key names another record now than then is not a candidate. Equal
    /// scores stay in the order of the keys at the head, the order the
    /// engines rank and a cursor pages in: sorting one batch again by the
    /// keys answered would answer some of a tie twice and some never (029).
    fn keyed_then(&self, named: &TypeName, ranked: Vec<(RecordKey, f64)>) -> Vec<(RecordKey, f64)> {
        let Some(h) = &self.head else {
            return ranked;
        };
        (ranked.into_iter())
            .filter_map(|(k, score)| Some((h.since.key_then(named, &k)?, score)))
            .collect()
    }

    /// Whether the lake asks an index for `field` of `decl`'s type at the
    /// head, where the engines keep their vector indexes: a search over it
    /// answers the index's set, at any position.
    fn indexed(&self, decl: &TypeDecl, field: &FieldName) -> Result<bool> {
        let key = core_engines::vector_index_key(&decl.name);
        let shape = self.head.as_ref().map_or(&self.shape, |h| &h.shape);
        let setting = shape.settings.get(&key);
        Ok(core_engines::indexed_fields(decl, setting).contains(field))
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
        let (ends, admit) = (
            self.admitted_ends(decl, &rows)?,
            self.sight.admit(&decl.name),
        );
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
            if !self.live_by(decl, &admit, &r) {
                continue;
            }
            if filter.is_some_and(|e| !self.holds(e, &r.fields, Some(&r.meta))) {
                continue;
            }
            if let Some(record) = rights::visible(self.sight, decl, &r, with_meta) {
                hits.push(Hit { record, score });
            }
        }
        Ok(hits)
    }

    /// For a search as of a position by a holder of `only` on `decl`, the
    /// candidates admitted at the head as well, under the grant followed
    /// there and by the name and the key it bears there: a ranking by the text
    /// and vectors at the head must find no record by another's (051).
    /// A grant that cannot be applied to the head's declaration admits
    /// none there (lakelands#771). `None` where every candidate passes.
    fn head_admitted(
        &self,
        decl: &TypeDecl,
        keys: &[RecordKey],
    ) -> Result<Option<BTreeSet<RecordKey>>> {
        let Some(h) = self.head.as_ref().filter(|_| !self.whole(decl)) else {
            return Ok(None);
        };
        let (head, who, since, shape) = (&h.view, &h.who, &h.since, &h.shape);
        // Each candidate under the name and the key it bears at the head.
        let name = since.name(&decl.name);
        let moved: Vec<RecordKey> = keys.iter().map(|k| since.key(&name, k)).collect();
        let sees = Sight::held(who, shape.as_ref());
        // A type the head does not declare has no record there.
        let Ok(at_head) = shape.ontology.ty(&name).map(|_| sees.admit(&name)) else {
            return Ok(Some(BTreeSet::new()));
        };
        let rows = head.get_many(&name, &moved)?;
        // A link's ends are judged at the head too, under the same grant.
        let fields: Vec<_> = rows.iter().map(|r| &r.fields).collect();
        let ends = sees.ends(decl, &fields, &mut |end, keys| head.get_many(end, keys))?;
        let admitted: BTreeSet<RecordKey> = (rows.into_iter().zip(ends))
            .filter(|(r, ends)| *ends && !r.retired && at_head.holds(&r.fields))
            .map(|(r, _)| r.key)
            .collect();
        let admitted = keys
            .iter()
            .zip(&moved)
            .filter(|(_, k)| admitted.contains(*k));
        Ok(Some(admitted.map(|(k, _)| k.clone()).collect()))
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
    /// what remains is a masked field. A relation the step the hop is
    /// walked for does not walk (`reach`) is not probed, as it is not read.
    fn probe_derived(
        &self,
        here: &TypeName,
        relation: Option<&RelationName>,
        walk: Walk,
        reach: &Reach,
    ) -> Result<()> {
        for (t, r) in self.shown_relations(Some(here)) {
            if relation.is_some_and(|n| r.name != *n) || !reach.walks(here, walk, t, r) {
                continue;
            }
            rights::check_probe(self.who, t, r.via.iter())?;
            let Some(d) = &r.derived else { continue };
            let [from, to] = d.reads();
            rights::check_probe(self.who, t, from.iter())?;
            let target = self.shape.ontology.ty(&r.target)?;
            rights::check_probe(self.who, target, to.iter())?;
        }
        Ok(())
    }

    /// `query`: the one pattern query.
    pub fn query(&self, p: &Pattern, carried: Option<Pulled>) -> Result<Judged<QueryResult>> {
        // Its steps as declared: a step naming an inverse is rewritten below.
        let mut p = p.clone();
        let p = &mut p;
        let Some(start_ty) = p.start.ty.clone() else {
            return self.untyped(p);
        };
        let start_decl = self.named_type(&start_ty)?.clone();
        let params = self.bind_params(&start_decl, &p.start.params)?;
        let limit = self.limit(p.limit, measures::is_measure(&p.project))?;
        let max_depth = self.lake.limits.max_depth;
        // Rows and an aggregate bind each record under a name, the one
        // `as` gives or its type's, once (018, extended); nothing else
        // binds by name.
        let binds = matches!(
            p.project,
            Projection::Rows { .. } | Projection::Aggregate(_)
        );
        let given = std::iter::once(&p.start.named).chain(p.steps.iter().map(|s| &s.named));
        for name in given.flatten() {
            if !binds {
                return Err(invalid(Case::PatternAsNotRows));
            }
            if let Some(fault) = lakelands_core::names::fault(Naming::Binding, name) {
                return Err(invalid(fault));
            }
        }
        if measures::is_measure(&p.project) {
            measures::check(p)?;
        } else if !matches!(p.project, Projection::Rows { .. }) && !p.order_by.is_empty() {
            // A graph and a path come by depth, type and key, an
            // aggregate's groups by their key (025): an order they would
            // pass over is refused, as `as` is.
            return Err(invalid(Case::PatternOrderNotRows));
        }
        // A row is a projection of a walk, not a record: it has no change
        // of its own and no provenance to order by (028, 025). Refused
        // before a record is read, whatever the walk would meet.
        for o in &p.order_by {
            if let Some((field, case)) = o.misfit() {
                let reason = Refusal::Invalid(Box::new(case.into()));
                return Err(Error::Refused { field, reason });
            }
            match o.by {
                OrderBy::Value => {}
                OrderBy::Latest => return Err(invalid(Case::LatestNoChange { ty: None })),
                _ => {
                    let case = Case::NoRecordsProvenance;
                    return Err(crate::declare::invalid_field(&o.field, case));
                }
            }
        }
        // What a pattern says and an arm cannot honour is refused, not
        // passed over: a path is a graph of one walk from one start. Rows
        // are told so before their steps are judged, which a path's, each
        // from depth 0, would not pass.
        let reaches = p.project == Projection::Reach;
        if (binds || reaches) && p.to.is_some() {
            return Err(invalid(Case::PatternToNotGraph));
        }
        // A reach of no steps could never answer a row: it is refused as a
        // measure of none is.
        if reaches && p.steps.is_empty() {
            let measure = "reach".to_owned();
            return Err(invalid(Case::MeasureNoSteps { measure }));
        }
        let start_name = p.start.named.clone();
        let mut names = vec![start_name.unwrap_or_else(|| start_decl.name.to_string())];
        // The type each step reaches, checked against the declarations.
        let mut types = vec![start_decl.name.clone()];
        // Each name bound over several types: where its step stands, and
        // the types it reaches.
        let mut several: Vec<Option<(Place, BTreeSet<TypeName>)>> = vec![None];
        let mut filters = Vec::new();
        let mut reach: Vec<Reach> = Vec::new();
        let measure = measures::is_measure(&p.project);
        // A graph whose steps all start at depth 0 is one walk over any of
        // their relations from the start (018): a step's relation may be
        // declared by a type another step reaches. Any other is a chain, and
        // so is a reach, whatever its depths.
        let chain = reaches || p.steps.iter().any(|s| s.depth.min > 0);
        let lenient = match p.project {
            Projection::Graph => !chain,
            _ => p.project == Projection::Components,
        };
        let last = p.steps.len().saturating_sub(1);
        for (i, step) in p.steps.iter_mut().enumerate() {
            // A measure's steps are the relations around its records, each
            // read from the start, not a chain.
            let here = if measure {
                start_decl.name.clone()
            } else {
                types.last().expect("the start is in").clone()
            };
            // A place names the step's relation as its caller wrote it.
            let at = Place::Step {
                step: i + 1,
                relation: step.relation.clone(),
            };
            let owner = self.as_declared(&here, step);
            let owner = owner.as_ref();
            // Rows are walked as a chain whatever their depths, so a step
            // that binds is judged from where it stands, as a chain's and a
            // measure's are.
            let walked = measure || chain || binds;
            let reached = match walked {
                true => self.step_target(&here, &step.relation, step.walk, (lenient, owner))?,
                // A neighbourhood's step may stand anywhere, but a relation
                // no type its reader is shown declares is walked from
                // nowhere: hidden or absent, it is refused alike (006).
                false if !self.out_any(&step.relation) => {
                    let (ty, relation, walk) = (here, step.relation.clone(), step.walk);
                    return Err(invalid(Case::NoRelationToWalk { ty, relation, walk }));
                }
                false => None,
            };
            // A neighbourhood and a path keep a record at every hop; a chain
            // walks a step's whole range first. A measure takes no `type`.
            let far = self.through(step, !(binds || chain), owner);
            let owner = owner.cloned();
            reach.push(Reach { owner, far });
            self.probe_derived(&here, Some(&step.relation), step.walk, &reach[i])?;
            // Such a step reaches every type of its whole range from `here`,
            // not only the first that declares its relation, and its `type`
            // may name any of them (a measure's has none); a neighbourhood's
            // reaches every type its relation does. It is judged once the relation is probed around
            // `here`, so one held from the field that carries it there is
            // answered that first.
            let all = match walked {
                true => self.ranged(step, &here, reach[i].owner.as_ref()),
                false => self.neighbours(step, None, reach[i].owner.as_ref()),
            };
            // A row binds the step's records under one name, and the step
            // after it in a chain is judged from one type. Rows and an
            // aggregate may bind a last step over several types where no
            // `where` reads it (018, extended): under the relation's name as
            // its caller wrote it, its records hold their key, `_type` and
            // `_depth` alone, and a path that reads a field is refused.
            let open = binds && i == last && step.ty.is_none() && step.filter.is_none();
            let open = open && all.len() > 1;
            let one = (binds && !open) || (chain && !measure && i < last);
            let reached = self.neighbour_target(&at, step, &all, one)?.or(reached);
            // At a valid time a walk keeps each record it meets by its own
            // type's valid time, so a step of a hop or more probes every type
            // it can meet (024, lakelands#831): from the declarations, since a
            // refusal that waited for a record would say that one was reached.
            if self.valid_at.is_some() && step.depth.max > 0 {
                for t in self.met(step, (!lenient).then_some(&here), &reach[i]) {
                    self.visible_type(&t)?;
                }
            }
            let reached = reached.or_else(|| step.ty.clone()).unwrap_or(here);
            let decl = self.visible_type(&reached)?.clone();
            // A view with parameters is bound only by a read that names it
            // (018): a step that arrives at one other than the start has no
            // values to read it with, whatever records the walk would reach,
            // so the pattern is refused before any record is read.
            // Every type the step reaches is a view it arrives at, not only
            // the one it binds by.
            for ty in all
                .iter()
                .chain([&reached])
                .filter(|t| **t != start_decl.name)
            {
                let decl = self.sight.ty(ty)?;
                if let Some(param) = decl.view.as_ref().and_then(|v| v.params.first()) {
                    return Err(invalid(
                        Said::from(unbound(ty, param, true)).within(at.clone()),
                    ));
                }
            }
            // An open step's name is its relation as its caller wrote it,
            // which its place keeps: the inverse's where it named that.
            let written = match &at {
                Place::Step { relation, .. } => relation.to_string(),
                _ => step.relation.to_string(),
            };
            let name = match (&step.named, open) {
                (Some(name), _) => name.clone(),
                (None, true) => written,
                (None, false) => reached.to_string(),
            };
            several.push(open.then(|| (at.clone(), all.clone())));
            if binds && names.contains(&name) {
                let said = Said::from(Case::PatternBindsTwice { name });
                return Err(invalid(said.within(at)));
            }
            names.push(name);
            let at = [at, Place::Where];
            filters.push(self.record_filter(&decl, step.filter.as_ref(), &at)?);
            types.push(reached);
        }
        // A depth no walk takes is refused for what the pattern says,
        // before a record is read: with no start there is no walk to say it.
        for s in &p.steps {
            graph::check_depth(s.depth, max_depth)?;
        }
        let (reader, reach) = (self, &reach[..]);
        // Rows, a reach and every walk a step's `where` reads hold what the
        // walk reads, so that `where` reads each record its visibility read
        // already brought for the batch, never one alone (lakelands#940,
        // lakelands#1027). A graph or a measure without one walks more
        // records than it answers, and holds none.
        let wheres = p.steps.iter().any(|s| s.filter.is_some());
        let records = (binds || reaches || wheres).then(RefCell::default);
        let hidden = Visible {
            reader,
            reach,
            records,
        };
        if measure {
            return self.measure(p, limit, carried, &hidden);
        }
        if p.to.is_some() && chain {
            return Err(invalid(Case::PatternToChain));
        }
        // A cursor is checked before any arm answers: a path and an
        // aggregate hand out none, so every cursor is another read's there.
        let (offset, after) = offset(p.cursor.as_ref(), p, &params)?;
        // Whether a record reached over step `i` is kept and walked from:
        // it is of the step's `type`, and its `where`, read against the type
        // the step reaches, holds of it.
        let keep =
            |i: usize, node: &lakelands_core::graph::Node| -> Result<bool> {
                if p.steps[i].ty.as_ref().is_some_and(|t| *t != node.0) {
                    return Ok(false);
                }
                let Some(f) = &filters[i] else {
                    return Ok(true);
                };
                let r = hidden.record(node)?;
                Ok(node.0 == types[i + 1]
                    && r.is_some_and(|r| self.holds(f, &r.fields, Some(&r.meta))))
            };
        // The start: the one record a key names, what a search finds, or
        // the records of the start's type a filter (or none) selects, which
        // rows read a page at a time.
        let few: Option<Vec<StoredRecord>> = if let Some(key) = &p.start.key {
            if self.start_found(p, key)? {
                Some(self.get_one(&start_ty, key)?.into_iter().collect())
            } else {
                Some(Vec::new())
            }
        } else if let Some(mode) = &p.start.search {
            let keys = match self.start_hits(p, mode, carried)? {
                Ok(keys) => keys,
                Err(moved) => return Ok(Err(moved)),
            };
            let held = keys
                .iter()
                .filter_map(|k| self.view.get(&start_ty, k).ok().flatten());
            Some(self.at_read(&start_ty, held.collect()))
        } else {
            None
        };
        let at = [Place::Start, Place::Where];
        let filter = self.record_filter(&start_decl, p.start.filter.as_ref(), &at)?;
        // A chain and a path read every start, and so do rows of a view,
        // whose records are computed whole. A neighbourhood's starts are
        // its first records, so it reads as many as its page reaches and
        // one more (lakelands#963).
        let hood = p.project == Projection::Graph && !chain && p.to.is_none();
        let few = match few {
            None if hood && start_decl.view.is_none() => {
                let want = offset.saturating_add(limit).saturating_add(1);
                Some(self.page(&start_decl, filter.as_ref(), &[], None, want)?)
            }
            None if !binds || start_decl.view.is_some() => {
                Some(self.records(&start_decl, filter.as_ref())?)
            }
            few => few,
        };
        let from: Vec<lakelands_core::graph::Node> = (few.iter().flatten())
            .map(|s| (s.ty.clone(), s.key.clone()))
            .collect();
        // A path, from its one start.
        if let Some((to_ty, to_key)) = &p.to {
            let (nodes, edges) = match from.as_slice() {
                [] => (Vec::new(), Vec::new()),
                [one] => {
                    let to = (to_ty.clone(), to_key.clone());
                    graph::shortest_path(&hidden, one, &to, &p.steps, max_depth, keep)?.map_or_else(
                        Default::default,
                        |path| {
                            // Along a path a record's depth is its hop.
                            let hops = (0..).zip(path.nodes);
                            let nodes = hops.map(|(depth, node)| graph::Reached { node, depth });
                            (nodes.collect(), path.edges)
                        },
                    )
                }
                more => {
                    let count = more.len();
                    return Err(invalid(Case::PathStarts { count }));
                }
            };
            return self.graph_result(nodes, edges, None).map(Ok);
        }
        // A graph: several starts are one walk, each record at the fewest
        // hops any start reaches it in, ordered by (depth, type, key) as
        // one walk orders what it reaches (026, 029), a page at a time.
        if p.project == Projection::Graph {
            let bound = (max_depth, self.lake.limits.max_graph_nodes);
            let g = match chain {
                true => graph::spanned(&hidden, &from, &p.steps, bound, offset, limit, keep)?,
                false => graph::subgraph(&hidden, &from, &p.steps, max_depth, offset, limit, keep)?,
            };
            let next = g
                .truncated
                .then(|| self.page_cursor(p, (offset + limit, None), params));
            return self.graph_result(g.nodes, g.edges, next).map(Ok);
        }
        // What each step reaches (lakelands#991): a step walked whole before
        // the next, each record once at the first step, in the order
        // (step, depth, type, key). A page walks until it holds one row
        // more than it answers, so a later page walks again the steps up
        // to the one its cursor names; the cursor is the last row's.
        if reaches {
            type Row = (u64, u64, String, RecordKey);
            let past: Option<Row> = match &after {
                None => None,
                Some((v, key)) => match v.as_slice() {
                    [Value::Number(s), Value::Number(d), Value::String(t)] => {
                        let (s, d) = (s.as_u64(), d.as_u64());
                        let at = s.zip(d).map(|(s, d)| (s, d, t.clone(), key.clone()));
                        Some(at.ok_or_else(|| invalid(Case::CursorForeign))?)
                    }
                    _ => return Err(invalid(Case::CursorForeign)),
                },
            };
            let (mut rows, mut more): (Vec<Row>, bool) = (Vec::new(), false);
            let bound = (max_depth, self.lake.limits.max_graph_nodes);
            graph::reach(&hidden, &from, &p.steps, bound, keep, |step, first| {
                for r in first {
                    let row = (
                        step as u64,
                        u64::from(r.depth),
                        r.node.0.to_string(),
                        r.node.1,
                    );
                    if past.as_ref().is_some_and(|p| row <= *p) {
                        continue;
                    }
                    if rows.len() == limit {
                        more = true;
                        return Ok(false);
                    }
                    rows.push(row);
                }
                Ok(true)
            })?;
            let next_cursor = (more.then(|| rows.last()).flatten()).map(|(s, d, t, k)| {
                let at = vec![Value::from(*s), Value::from(*d), Value::String(t.clone())];
                self.page_cursor(p, (0, Some((at, k.clone()))), params)
            });
            let cell = |(s, d, t, k): Row| {
                BTreeMap::from([
                    ("step".to_owned(), Value::from(s)),
                    ("depth".to_owned(), Value::from(d)),
                    ("type".to_owned(), Value::String(t)),
                    ("key".to_owned(), Value::String(k.to_string())),
                ])
            };
            let rows = rows.into_iter().map(cell).collect();
            return Ok(Ok(QueryResult::Rows { rows, next_cursor }));
        }
        // Rows are ordered, projected and measured by their paths,
        // `name.field`, each read as the rows hold it, and a name they do
        // not hold is refused (018, extended). An order or a measure probes
        // its field as `find`'s do (024), so an answer is never computed
        // over values the principal may not read; a named key's fields are
        // probed as a field key's are.
        let bound = |n: &str| names.iter().any(|b| b == n);
        // A name over several types holds no field: a path that reads
        // one is refused, naming the types.
        // Its `key`, `_type` and `_depth` are the lake's columns, read
        // against no type and never probed (`None`).
        let typed = |n: &str, f: FieldName| {
            let i = names.iter().position(|b| b == n).expect("a bound name");
            match &several[i] {
                Some(_) if ["key", DEPTH, TYPE].contains(&f.as_str()) => Ok(None),
                Some((at, all)) => Err(several_types(at, all)),
                None => Ok(Some((types[i].clone(), f))),
            }
        };
        let path = |name: &str| {
            let (n, f) = aggregate::row_path(name, &bound).map_err(invalid)?;
            typed(&n, f)
        };
        let ordered: Vec<Option<_>> = (p.order_by.iter())
            .map(|o| path(o.field.as_str()))
            .collect::<Result<_>>()?;
        let mut paths: Vec<(TypeName, FieldName)> = ordered.into_iter().flatten().collect();
        let orders = paths.len();
        let open: Vec<bool> = several.iter().map(Option::is_some).collect();
        let row = self.row_shape(&names, &types, &open);
        let mut over_rows = None;
        if let Projection::Aggregate(a) = &p.project {
            let (a, read) = aggregate::over_rows(a, &bound).map_err(invalid)?;
            for (n, f) in read {
                paths.extend(typed(&n, f)?);
            }
            over_rows = Some(a);
        }
        self.probe_rows(&paths, orders)?;
        // A projected field is named, not probed: one its reader may not
        // know of is one its type does not have, and a masked one is left
        // out of the row, as an absent value is.
        if let Projection::Rows { fields } = &p.project {
            let read = fields.iter().map(|f| path(f)).collect::<Result<Vec<_>>>()?;
            for (t, f) in read.into_iter().flatten() {
                let decl = self.shape.ontology.ty(&t)?;
                if !self.lakes(decl, &f) {
                    rights::check_known(self.who, decl, [&f])?;
                }
            }
        }
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
        // Rows: one per match, walked a page of starts at a time
        // (lakelands#940), each page one frontier and each record read
        // once for the walk, its `where` and its row. The starts come in
        // key order, or in an order the engine gives them (041): each of
        // its paths a stored field of the start's own, as `page` reads.
        let mut keep = keep;
        let ordered = few.is_none().then(|| {
            let own = |o: &Order| {
                let (n, f) = o.field.as_str().split_once('.')?;
                let field = FieldName::from(f);
                let stored = start_decl
                    .fields
                    .iter()
                    .any(|x| x.name == field && !x.at_read());
                let own = n == names[0] && stored && !self.lakes(&start_decl, &field);
                own.then(|| Order { field, ..o.clone() })
            };
            p.order_by.iter().map(own).collect::<Option<Vec<Order>>>()
        });
        let row = |start: &graph::Node, binding: &[graph::Reached]| -> Result<_> {
            let mut row = BTreeMap::new();
            self.bind(
                &mut row,
                &names[0],
                (start, 0, false),
                hidden.record(start)?,
            )?;
            for ((r, name), open) in binding.iter().zip(&names[1..]).zip(&several[1..]) {
                let at = (&r.node, r.depth, open.is_some());
                self.bind(&mut row, name, at, hidden.record(&r.node)?)?;
            }
            if let Projection::Rows { fields } = &p.project
                && !fields.is_empty()
            {
                row.retain(|k, _| fields.iter().any(|f| f == k));
            }
            Ok(row)
        };
        let walk = |starts: &Starts,
                    each: &mut EachBinding,
                    keep: &mut dyn FnMut(usize, &graph::Node) -> Result<bool>| {
            let nodes: Vec<graph::Node> = starts
                .iter()
                .map(|(_, r)| (r.ty.clone(), r.key.clone()))
                .collect();
            hidden.remember(starts.iter().map(|(_, r)| r.clone()));
            let at: BTreeMap<&graph::Node, &SortKey> =
                nodes.iter().zip(starts.iter().map(|(k, _)| k)).collect();
            let walked =
                graph::traverse_each(&hidden, &nodes, &p.steps, max_depth, keep, |start, b| {
                    each(at[start], start, b)
                });
            hidden.forget(&nodes);
            walked
        };
        let streamed = match p.order_by.is_empty() {
            true => Some(Vec::new()),
            false => ordered.flatten(),
        };
        let streamed = streamed.filter(|_| over_rows.is_none());
        // A cursor's start is a position in the order the starts are read
        // in, one of the starts at hand where they are few, and none where
        // every row is read: any other is not one this lake handed out.
        let fits = |(v, k): &SortKey| match (&streamed, &few) {
            (None, _) => false,
            (Some(o), Some(few)) => v.len() == o.len() && few.iter().any(|r| r.key == *k),
            (Some(o), None) => v.len() == o.len(),
        };
        if after.as_ref().is_some_and(|a| !fits(a)) {
            return Err(invalid(Case::CursorForeign));
        }
        if let Some(order) = streamed {
            // The page and one row more, in the starts' order; a cursor
            // names the last start whose rows are all answered, and how
            // many of the next one's are.
            let (mut rows, mut skipped, mut more) = (Vec::new(), 0, None);
            let (mut done, mut at, mut of_at) = (after.clone(), None::<SortKey>, 0);
            self.start_pages(
                few,
                (&start_decl, filter.as_ref(), &order),
                after,
                &mut |starts| {
                    walk(
                        starts,
                        &mut |here, start, binding| {
                            if at.as_ref() != Some(here) {
                                if of_at > 0 {
                                    done = at.take();
                                }
                                (at, of_at) = (Some(here.clone()), 0);
                            }
                            of_at += 1;
                            if skipped < offset {
                                skipped += 1;
                                return Ok(true);
                            }
                            if rows.len() == limit {
                                more = Some((done.clone(), of_at - 1));
                                return Ok(false);
                            }
                            rows.push(row(start, binding)?);
                            Ok(true)
                        },
                        &mut keep,
                    )?;
                    Ok(more.is_none())
                },
            )?;
            let next_cursor = more.map(|(after, n)| self.page_cursor(p, (n, after), params));
            return Ok(Ok(QueryResult::Rows { rows, next_cursor }));
        }
        // Every row: an aggregate's whole, an order's as their sort values
        // and records until the page is cut. An order over a record a step
        // reaches, and an aggregate, hold at most the lake's
        // `max_graph_nodes` rows, past which the read is refused, never
        // cut, as a whole-graph measure is (026); a start's own order over
        // starts at hand (a key, a search, a view's records) as before.
        let own = |o: &Order| {
            o.field
                .as_str()
                .split_once('.')
                .is_some_and(|(n, _)| n == names[0])
        };
        let bounded = over_rows.is_some() || !p.order_by.iter().all(own);
        let max = self.lake.limits.max_graph_nodes;
        let member = ["order_by", "project"][usize::from(over_rows.is_some())];
        let (mut rows, mut held) = (Vec::new(), Vec::new());
        self.start_pages(
            few,
            (&start_decl, filter.as_ref(), &[]),
            None,
            &mut |starts| {
                walk(
                    starts,
                    &mut |_, start, binding| {
                        if bounded && rows.len() + held.len() >= max as usize {
                            let (field, reason) = (Some(member.into()), Refusal::TooLarge { max });
                            return Err(Error::Refused { field, reason });
                        }
                        let r = row(start, binding)?;
                        let value = |o: &Order| r.get(o.field.as_str()).cloned();
                        let vals = p.order_by.iter().map(|o| value(o).unwrap_or(Value::Null));
                        let vals: Vec<_> = vals.collect();
                        match over_rows {
                            Some(_) => rows.push(r),
                            None => held.push((vals, start.clone(), binding.to_vec())),
                        }
                        Ok(true)
                    },
                    &mut keep,
                )?;
                Ok(true)
            },
        )?;
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
            return Ok(Ok(QueryResult::Groups { rows: groups }));
        }
        let order: Vec<Order> = p.order_by.clone();
        let mut keyed: Vec<_> = (held.into_iter().enumerate())
            .map(|(i, (vals, start, b))| ((vals, RecordKey::from(format!("{i:020}"))), (start, b)))
            .collect();
        keyed.sort_by(|a, b| Self::compare(&order, &a.0, &b.0));
        // Sorted first: an order may read a path `fields` leaves out. The
        // page's rows alone are bound again.
        let next_cursor = (keyed.len() > offset + limit)
            .then(|| self.page_cursor(p, (offset + limit, None), params));
        let page = keyed.into_iter().skip(offset).take(limit);
        let rows = page.map(|(_, (s, b))| row(&s, &b)).collect::<Result<_>>()?;
        Ok(Ok(QueryResult::Rows { rows, next_cursor }))
    }

    /// Hands `each` the starts of a pattern's rows a page at a time, each
    /// with its position (its sort values and key), after `after`: the
    /// records of `few` in their order, or those of the stored type a
    /// filter selects in `order`, read by `page`, until `each` answers
    /// that it has what it needs.
    fn start_pages(
        &self,
        few: Option<Vec<StoredRecord>>,
        (decl, filter, order): (&TypeDecl, Option<&Expr>, &[Order]),
        mut after: Option<SortKey>,
        each: &mut dyn FnMut(&Starts) -> Result<bool>,
    ) -> Result<()> {
        let at = |r: &StoredRecord| (Self::sort_values(order, r), r.key.clone());
        if let Some(few) = few {
            let from = after
                .and_then(|(_, k)| few.iter().position(|r| r.key == k))
                .map_or(0, |i| i + 1);
            let few: Vec<_> = few.into_iter().skip(from).map(|r| (at(&r), r)).collect();
            for page in few.chunks(graph::PAGE) {
                if !each(page)? {
                    break;
                }
            }
            return Ok(());
        }
        // An engine page in a field's order sorts the type again, so an
        // ordered read asks for twice as many starts each time, as `page`
        // does within a call.
        let mut want = graph::PAGE;
        loop {
            let page = self.page(decl, filter, order, after.take(), want)?;
            let page: Vec<_> = page.into_iter().map(|r| (at(&r), r)).collect();
            after = page.last().map(|(k, _)| k.clone());
            if page.is_empty() || !each(&page)? || page.len() < want {
                return Ok(());
            }
            want *= 1 + usize::from(!order.is_empty());
        }
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
        match self.get(start_type(p)?, &get) {
            Ok(_) => Ok(true),
            Err(Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// The keys a pattern's start search finds, best first.
    fn start_hits(
        &self,
        p: &Pattern,
        mode: &SearchMode,
        carried: Option<Pulled>,
    ) -> Result<Judged<Vec<RecordKey>>> {
        let s = Search {
            ty: start_type(p)?.clone(),
            mode: mode.clone(),
            k: self.lake.limits.max_limit,
            cursor: None,
            filter: p.start.filter.clone(),
            with_meta: false,
            at: ReadAt::default(),
        };
        let found = self.search(&s, carried)?;
        Ok(found.map(|f| f.hits.into_iter().map(|h| h.record.key).collect()))
    }

    /// Refuse an order or a measure over a path of a pattern's rows, the
    /// type of a bound name and one of its fields, whose field the principal
    /// may not read, or that the type does not declare, as `find` refuses
    /// one (024, 006). A row's `<name>.key` and `<name>._depth` are the
    /// lake's columns and probe nothing.
    /// The first `ordered` paths are orders, and an order by a field
    /// computed at the read is refused once the field is probed.
    fn probe_rows(&self, paths: &[(TypeName, FieldName)], ordered: usize) -> Result<()> {
        for (i, (ty, field)) in paths.iter().enumerate() {
            let decl = self.shape.ontology.ty(ty)?;
            if self.lakes(decl, field) {
                continue;
            }
            rights::check_probe(self.who, decl, [field])?;
            if i < ordered {
                Self::steady_order(decl, field.as_str())?;
            }
        }
        Ok(())
    }

    /// Whether a row's `<name>.<field>` is one of the lake's own columns,
    /// not a field of the record: `_depth` and `_type` always (018,
    /// extended), and its key where [`Self::keyed`]. It is never probed.
    fn lakes(&self, decl: &TypeDecl, field: &FieldName) -> bool {
        [DEPTH, TYPE].contains(&field.as_str()) || self.keyed(decl, field)
    }

    /// Whether a row's `<name>.<field>` is its record's key: the field is
    /// `key`, and the type has no field of that name its reader may know
    /// of (024).
    fn keyed(&self, decl: &TypeDecl, field: &FieldName) -> bool {
        field.as_str() == "key"
            && !decl
                .fields
                .iter()
                .any(|f| &f.name == field && self.knows(decl, field))
    }

    /// The relations the principal is shown ([`Sight::relation`])
    /// with their owners: those that leave or reach `around`, or all.
    /// A relation it is not shown is one no type declares, at every step,
    /// inverse name and edge.
    fn shown_relations(&self, around: Option<&TypeName>) -> Vec<(&TypeDecl, &RelationDecl)> {
        let near =
            |t: &TypeDecl, r: &RelationDecl| around.is_none_or(|a| &t.name == a || &r.target == a);
        let types = self.shape.ontology.types.values().map(|e| &e.decl);
        let all = types.flat_map(|t| t.relations.iter().map(move |r| (t, r)));
        all.filter(|(t, r)| near(t, r) && self.sight.relation(t, r))
            .collect()
    }

    /// Whether a type the principal is shown declares a relation of this
    /// name, so that a step over it from elsewhere walks nothing.
    fn out_any(&self, relation: &RelationName) -> bool {
        (self.shown_relations(None).iter()).any(|(_, r)| &r.name == relation)
    }

    /// [`Self::as_declared`] where the start names no type: a step names a
    /// relation some type the principal is shown declares, or else the
    /// `inverse` of each relation it is shown that bears it, answered with
    /// its owner. None where a relation is declared under the name.
    fn inverses_anywhere(&self, name: &RelationName) -> Vec<(RelationName, TypeName)> {
        let shown = self.shown_relations(None);
        if (shown.iter()).any(|(_, r)| r.name == *name) {
            return Vec::new();
        }
        let inverse = shown
            .into_iter()
            .filter(|(_, r)| r.inverse.as_ref() == Some(name));
        inverse
            .map(|(t, r)| (r.name.clone(), t.name.clone()))
            .collect()
    }

    /// Rewrite a step naming a relation's `inverse` to that relation,
    /// walked the other way: the inverse is the relation's name from its
    /// target, and the step's `walk` counts from the name it gives. A
    /// relation declared under the name wins, so a step is read as an
    /// inverse only when it walks no relation of that name from `here`.
    ///
    /// An inverse is one relation (044, extended), so such a step walks
    /// that relation alone, whatever else bears its name: its owner is
    /// answered, for the step to be judged and walked by.
    fn as_declared(&self, here: &TypeName, step: &mut Step) -> Option<TypeName> {
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
            return None;
        }
        let inverse = |(t, r): &&(&TypeDecl, &RelationDecl)| {
            r.inverse.as_ref() == Some(name) && walked(t, r, flipped)
        };
        let (t, r) = shown.iter().find(inverse)?;
        (step.relation, step.walk) = (r.name.clone(), flipped);
        Some(t.name.clone())
    }

    /// The relations a step walks by its name among those shown around
    /// `around`, or anywhere: its `owner`'s alone where it names an inverse.
    fn named(
        &self,
        around: Option<&TypeName>,
        relation: &RelationName,
        owner: Option<&TypeName>,
    ) -> Vec<(&TypeDecl, &RelationDecl)> {
        let mut shown = self.shown_relations(around);
        shown.retain(|(t, r)| &r.name == relation && owner.is_none_or(|o| *o == t.name));
        shown
    }

    /// The types a typed step's walk passes through, its `type` among
    /// them: a neighbourhood and a path keep a record at `every_hop`, so
    /// only that type; a chain's step walks its range and keeps at its end,
    /// so each type its `type` is reached from in the hops left as well. A
    /// relation that leads to none of them answers nothing the step keeps.
    fn through(
        &self,
        step: &Step,
        every_hop: bool,
        owner: Option<&TypeName>,
    ) -> Option<BTreeSet<TypeName>> {
        let mut far = BTreeSet::from([step.ty.clone()?]);
        let hops = if every_hop { 1 } else { step.depth.max };
        let shown = self.named(None, &step.relation, owner);
        for _ in 1..hops.min(self.lake.limits.max_depth) {
            // One hop back from what the hops after it lead to.
            let known = far.clone();
            for (t, r) in &shown {
                if step.walk != Walk::In && known.contains(&r.target) {
                    far.insert(t.name.clone());
                }
                if step.walk != Walk::Out && known.contains(&t.name) {
                    far.insert(r.target.clone());
                }
            }
            if far.len() == known.len() {
                break;
            }
        }
        Some(far)
    }

    /// The types a step reaches from `here`, or for a neighbourhood from
    /// wherever its relation is declared: its target walked out, its owner
    /// walked in.
    fn neighbours(
        &self,
        step: &Step,
        here: Option<&TypeName>,
        owner: Option<&TypeName>,
    ) -> BTreeSet<TypeName> {
        let at = |t: &TypeName| here.is_none_or(|h| h == t);
        let mut reached = BTreeSet::new();
        for (t, r) in self.named(here, &step.relation, owner) {
            (step.walk != Walk::In && at(&t.name)).then(|| reached.insert(r.target.clone()));
            (step.walk != Walk::Out && at(&r.target)).then(|| reached.insert(t.name.clone()));
        }
        reached
    }

    /// The types a step's records may be of from `here`, over its whole
    /// range: `here` itself where it starts at depth 0, and each type a hop
    /// of it reaches. A row would bind a record of each under the step's
    /// one name, and a `where` be read against each. A range that reaches
    /// none is judged by its first hop.
    fn ranged(&self, step: &Step, here: &TypeName, owner: Option<&TypeName>) -> BTreeSet<TypeName> {
        let mut reached = BTreeSet::new();
        let mut at = BTreeSet::from([here.clone()]);
        for hop in 0..=step.depth.max.min(self.lake.limits.max_depth) {
            if hop > 0 {
                at = (at.iter())
                    .flat_map(|t| self.neighbours(step, Some(t), owner))
                    .collect();
            }
            // A hop of the range that adds no type is the last that can.
            let known = reached.len();
            if hop >= step.depth.min {
                reached.extend(at.iter().cloned());
                if reached.len() == known {
                    break;
                }
            }
        }
        match reached.is_empty() {
            true => self.neighbours(step, Some(here), owner),
            false => reached,
        }
    }

    /// The types of the records a step of a hop or more can meet: the far
    /// end of each relation its [`Reach`] walks, a hop at a time from `here`,
    /// or with none, as a neighbourhood, a path and a component walk it,
    /// wherever it is declared. A read at a valid time probes each.
    fn met(&self, step: &Step, here: Option<&TypeName>, reach: &Reach) -> BTreeSet<TypeName> {
        let far = |t: &TypeName| reach.far.as_ref().is_none_or(|f| f.contains(t));
        let ends = |t: Option<&TypeName>| self.neighbours(step, t, reach.owner.as_ref());
        let Some(here) = here else {
            return ends(None).into_iter().filter(far).collect();
        };
        let (mut met, mut at) = (BTreeSet::new(), BTreeSet::from([here.clone()]));
        for _ in 0..step.depth.max.min(self.lake.limits.max_depth) {
            let next = at.iter().flat_map(|t| ends(Some(t))).filter(far);
            at = next.filter(|t| met.insert(t.clone())).collect();
        }
        met
    }

    /// The type a step reaches, one of `reached`: the one it names as its
    /// `type`, or the only one. None where it reaches several and names
    /// none, or reaches none. A step that reaches several types and reads
    /// a `where`, or that must reach `one` (a row binds its records, or a
    /// chain's next step is judged from it), names its `type`; a `type` it
    /// does not reach is refused, naming every type it does.
    fn neighbour_target(
        &self,
        at: &Place,
        step: &Step,
        reached: &BTreeSet<TypeName>,
        one: bool,
    ) -> Result<Option<TypeName>> {
        let Some(first) = reached.first() else {
            return Ok(None);
        };
        let said = match &step.ty {
            Some(t) if reached.contains(t) => return Ok(Some(t.clone())),
            Some(t) => {
                let also = (reached.len() > 1).then(|| reached.iter().skip(1).collect());
                let (reached, ty) = (first.clone(), t.clone());
                Case::StepReaches { reached, ty, also }
            }
            None if reached.len() < 2 => return Ok(Some(first.clone())),
            None if step.filter.is_none() && !one => return Ok(None),
            None => return Err(several_types(at, reached)),
        };
        Err(invalid(Said::from(said).within(at.clone())))
    }

    /// The type a step over `relation` reaches from `here`, walking `walk`.
    fn step_target(
        &self,
        here: &TypeName,
        relation: &RelationName,
        walk: Walk,
        (lenient, owner): (bool, Option<&TypeName>),
    ) -> Result<Option<TypeName>> {
        self.shape.ontology.ty(here)?;
        let shown = self.named(Some(here), relation, owner);
        let out = (shown.iter())
            .find(|(t, _)| &t.name == here)
            .map(|(_, r)| r.target.clone());
        let inn = (shown.iter())
            .find(|(_, r)| &r.target == here)
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

    /// A pattern's row as one type: for each name bound to a type in
    /// `types`, each field of the type its reader may know of under its
    /// path, `<name>.<field>`, and `<name>.key`, the record's key where no
    /// such field is named `key`. A field named `_depth` or `_type` is left
    /// out: under a bound name that path is the lake's column. A name over
    /// several types holds no field.
    fn row_shape(&self, names: &[String], types: &[TypeName], open: &[bool]) -> TypeDecl {
        let mut row = TypeDecl::default();
        for ((name, ty), open) in names.iter().zip(types).zip(open) {
            let Ok(t) = self.shape.ontology.ty(ty) else {
                continue;
            };
            let at = |f: &str| FieldName::from(format!("{name}.{f}"));
            let lakes = |f: &FieldName| [DEPTH, TYPE].contains(&f.as_str());
            let known =
                (t.fields.iter()).filter(|f| !open && self.knows(t, &f.name) && !lakes(&f.name));
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

    /// A record's values into a pattern's row under their paths,
    /// `<name>.<field>`. A row's `<name>.key` is the record's key, unless
    /// its type declares a field `key` its reader may know of: then that
    /// field's value, null where the record holds none. `<name>._depth`
    /// is the hops its step took to reach it, 0 for the start, and
    /// `<name>._type` its type, whatever field of those names a lake
    /// written before the rule holds. A name over several types (`open`)
    /// binds the key, the type and the depth alone.
    fn bind(
        &self,
        row: &mut BTreeMap<String, Value>,
        name: &str,
        (node, depth, open): (&graph::Node, u32, bool),
        rec: Option<StoredRecord>,
    ) -> Result<()> {
        let decl = self.shape.ontology.ty(&node.0)?;
        if let Some(rec) = rec.and_then(|r| rights::visible(self.sight, decl, &r, false)) {
            let decl = self.shape.ontology.ty(&node.0)?;
            let key = match open || self.keyed(decl, &FieldName::from("key")) {
                true => Value::String(node.1.to_string()),
                false => Value::Null,
            };
            row.insert(format!("{name}.key"), key);
            for (f, v) in rec.fields.into_iter().filter(|_| !open) {
                row.insert(format!("{name}.{f}"), v);
            }
            row.insert(format!("{name}.{DEPTH}"), Value::from(depth));
            row.insert(format!("{name}.{TYPE}"), Value::from(node.0.as_str()));
        }
        Ok(())
    }

    /// The records a graph reached as the principal may see them, each with
    /// its depth.
    fn graph_result(
        &self,
        nodes: Vec<graph::Reached>,
        edges: Vec<Edge>,
        next_cursor: Option<Cursor>,
    ) -> Result<QueryResult> {
        // Each type's records read in one ask, never one by one
        // (lakelands#1027).
        let all: Vec<graph::Node> = nodes.iter().map(|r| r.node.clone()).collect();
        let mut read: BTreeMap<graph::Node, StoredRecord> = BTreeMap::new();
        for (ty, keys) in graph::by_type(&all) {
            let rows = self.get_many(ty, &keys)?;
            read.extend(rows.into_iter().map(|r| ((r.ty.clone(), r.key.clone()), r)));
        }
        let (mut records, mut depths) = (Vec::new(), Vec::new());
        for r in &nodes {
            let Some(rec) = read.remove(&r.node) else {
                continue;
            };
            let decl = self.shape.ontology.ty(&r.node.0)?;
            if let Some(record) = rights::visible(self.sight, decl, &rec, false) {
                records.push(record);
                depths.push(r.depth);
            }
        }
        Ok(QueryResult::Graph {
            nodes: records,
            depths,
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
/// read a page at a time, and a page's matches are one scan a relation
/// side, paged by [`MATCHED`] records. Nothing is stored; the edges are as the view's position has
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
    // An owned relation leaves `near` for its target; a targeted one
    // reaches it. Each with its condition, read once for the hop: one that
    // cannot be read connects nothing.
    let filter = |x: &RelationDecl| {
        let c = x.derived.as_ref().and_then(|d| d.filter.as_ref());
        c.map(|c| condition(c, Eval::Record, &[]).ok())
    };
    let leaving = owned.iter().map(|x| (&x.target, *x, true));
    let reaching = targeted.iter().map(|&(owner, x)| (owner, x, false));
    let walked: Vec<_> = (leaving.chain(reaching))
        .map(|(other, x, out)| (other, x, out, filter(x)))
        .filter(|w| !matches!(w.3, Some(None)))
        .collect();
    // The values a near record holds for one side of a match, and the
    // pairs a scan of the other side must hold: none when a value is
    // missing, for null never matches.
    let equal = |near: &StoredRecord, pairs: &[MatchPair], out: bool| {
        let pair = |p: &MatchPair| {
            let (here, there) = if out {
                (&p.from, &p.to)
            } else {
                (&p.to, &p.from)
            };
            let v = near.fields.get(here).filter(|v| !v.is_null())?;
            Some((there.clone(), v.clone()))
        };
        pairs
            .iter()
            .map(pair)
            .collect::<Option<Vec<(FieldName, Value)>>>()
    };
    // One scan a relation side for the whole batch, with every tuple its
    // near records hold (lakelands#971); each near record is handed the
    // matches that hold its own.
    for (other, x, out, filter) in &walked {
        let (other, x, out) = (*other, *x, *out);
        let pairs = &x.derived.as_ref().expect("derived").pairs;
        let tuples: Vec<Option<Vec<(FieldName, Value)>>> = keys
            .iter()
            .map(|k| nears.get(k).and_then(|n| equal(n, pairs, out)))
            .collect();
        // A tuple by its canonical bytes: two are the same exactly where
        // their values are identical.
        let id = |t: &Vec<(FieldName, Value)>| canonical::of(t);
        let asked: BTreeMap<Vec<u8>, &Vec<(FieldName, Value)>> =
            tuples.iter().flatten().map(|t| (id(t), t)).collect();
        let asked: Vec<Vec<(FieldName, Value)>> = asked.into_values().cloned().collect();
        // The matches a page at a time, in key order, at most `MATCHED`
        // records held: a tuple's matches may span pages, and each start's
        // edges still come in key order.
        let mut after = None;
        loop {
            let found;
            (found, after) = r.matching(other, &asked, after.take())?;
            let mut by: BTreeMap<Vec<u8>, Vec<&StoredRecord>> = BTreeMap::new();
            for m in &found {
                let held = pairs.iter().map(|p| {
                    let there = if out { &p.to } else { &p.from };
                    (
                        there.clone(),
                        m.fields.get(there).cloned().unwrap_or(Value::Null),
                    )
                });
                by.entry(id(&held.collect())).or_default().push(m);
            }
            for ((key, list), tuple) in keys.iter().zip(all.iter_mut()).zip(&tuples) {
                let (Some(near), Some(tuple)) = (nears.get(key), tuple) else {
                    continue;
                };
                for m in by.get(&id(tuple)).into_iter().flatten() {
                    let ends = match out {
                        true => [&near.fields, &m.fields],
                        false => [&m.fields, &near.fields],
                    };
                    let scope = Ends {
                        ends,
                        settings: &r.shape.settings,
                        now: r.now,
                    };
                    if let Some(Some(e)) = filter
                        && !expr::holds(e, &scope).unwrap_or(false)
                    {
                        continue;
                    }
                    let (this, that) = ((ty.clone(), key.clone()), (other.clone(), m.key.clone()));
                    let (from, to) = if out { (this, that) } else { (that, this) };
                    list.push(Edge {
                        from,
                        relation: x.name.clone(),
                        to,
                        fields: BTreeMap::new(),
                    });
                }
            }
            if after.is_none() {
                break;
            }
        }
    }
    Ok(all)
}

/// The two records of a derived relation's edge, as its `where` reads
/// them: `from.<field>` the owner's, `to.<field>` the target's, with the
/// lake's settings and the read's clock (044, extended).
pub(crate) struct Ends<'a> {
    pub ends: [&'a BTreeMap<FieldName, Value>; 2],
    pub settings: &'a BTreeMap<String, Value>,
    pub now: Timestamp,
}

impl expr::Scope for Ends<'_> {
    fn value(&self, path: &[FieldName]) -> Option<Value> {
        let (end, rest) = path.split_first()?;
        let i = [lakelands_api::key::FROM, lakelands_api::key::TO]
            .iter()
            .position(|e| end.as_str() == *e)?;
        expr::eval::lookup(self.ends[i], rest)
    }

    fn setting(&self, key: &str) -> Option<Value> {
        self.settings.get(key).cloned()
    }

    fn now(&self) -> Timestamp {
        self.now
    }
}

/// Whether the key `key` of `ty` may be named to the principal of `sees`:
/// it is shown the type and answered the record at the key in `view`,
/// retired or not, as `restricted` is judged, and a link with its ends,
/// as a read of it answers (lakelands#871). A key no record holds keeps
/// nothing from it. A read the engine fails is the caller's error, never
/// a hold (lakelands#927). One who holds `restricted` and no `only` is
/// answered every record of a type it is shown, and is asked no read.
pub(crate) fn answered(
    view: &dyn ProjectionView,
    sees: &Sight,
    ty: &TypeName,
    key: &RecordKey,
) -> Result<bool> {
    answered_of(view, sees, ty, &[key], 1)
}

/// Whether [`answered`] holds of `need` or more of `keys`. The records are
/// read a batch at a time, and a link's ends once for the batch, until
/// enough are answered.
pub(crate) fn answered_of(
    view: &dyn ProjectionView,
    sees: &Sight,
    ty: &TypeName,
    keys: &[&RecordKey],
    need: usize,
) -> Result<bool> {
    let (Ok(decl), who) = (sees.ty(ty), sees.who()) else {
        return Ok(false);
    };
    if who.grants.restricted && !rights::holds_only(who) {
        return Ok(keys.len() >= need);
    }
    let mut found = 0;
    for page in keys.chunks(COUNT_PAGE) {
        let keys: Vec<RecordKey> = page.iter().map(|k| (*k).clone()).collect();
        let rows = view.get_many(ty, &keys)?;
        // A page whose keys hold no record asks no end's type.
        let ends = match rows.is_empty() {
            true => Vec::new(),
            false => {
                let fields: Vec<_> = rows.iter().map(|r| &r.fields).collect();
                sees.ends(decl, &fields, &mut |end, keys| view.get_many(end, keys))?
            }
        };
        let held = rows
            .iter()
            .zip(ends)
            .filter(|(r, ends)| !(*ends && sees.answers(ty, r)));
        found += page.len() - held.count();
        if found >= need {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The walks' view of the lake for one principal: a record is visible when
/// it is live, not restricted unless the principal may see restricted
/// records, valid at the read's `valid_at`, and of a type the principal
/// may see.
struct Visible<'r, 'a> {
    reader: &'r Reader<'a>,
    /// What each of the pattern's steps walks, by the step's index.
    reach: &'r [Reach],
    /// For rows, a reach and a walk a step's `where` reads, the records the
    /// walk has read, each read once for the walk, a step's `where` and a
    /// row (lakelands#940, lakelands#1027).
    records: Option<RefCell<BTreeMap<graph::Node, StoredRecord>>>,
}

impl Visible<'_, '_> {
    /// The record at `node` as this read answers it, read once.
    fn record(&self, node: &graph::Node) -> Result<Option<StoredRecord>> {
        if let Some(r) = self
            .records
            .as_ref()
            .and_then(|h| h.borrow().get(node).cloned())
        {
            return Ok(Some(r));
        }
        let r = self.reader.get_one(&node.0, &node.1)?;
        self.remember(r.clone());
        Ok(r)
    }

    fn remember(&self, rows: impl IntoIterator<Item = StoredRecord>) {
        if let Some(held) = &self.records {
            let rows = rows.into_iter().map(|r| ((r.ty.clone(), r.key.clone()), r));
            held.borrow_mut().extend(rows);
        }
    }

    /// Lets the records of a page of starts go: no other page reads them.
    fn forget(&self, nodes: &[graph::Node]) {
        if let Some(held) = &self.records {
            let mut held = held.borrow_mut();
            nodes.iter().for_each(|n| _ = held.remove(n));
        }
    }
}

/// What a step walks of the relations that bear its relation's name
/// (lakelands#829): a step naming an inverse walks the one relation that
/// declares it, its `owner`'s, and a step with a `type` only towards the
/// types its walk passes through, `far`. A relation a step does not walk
/// is neither probed nor read, so an answer holds what was probed and a
/// refusal names only what the walk would have read.
struct Reach {
    owner: Option<TypeName>,
    far: Option<BTreeSet<TypeName>>,
}

impl Reach {
    /// Whether the step walks relation `r` of `owner` around `here`:
    /// out to its target, or in from its owner.
    fn walks(&self, here: &TypeName, walk: Walk, owner: &TypeDecl, r: &RelationDecl) -> bool {
        let far = |t: &TypeName| self.far.as_ref().is_none_or(|f| f.contains(t));
        let out = owner.name == *here && walk != Walk::In && far(&r.target);
        let inn = r.target == *here && walk != Walk::Out && far(&owner.name);
        self.owner.as_ref().is_none_or(|o| *o == owner.name) && (out || inn)
    }
}

impl EdgeSource for Visible<'_, '_> {
    fn edges(
        &self,
        ty: &TypeName,
        keys: &[RecordKey],
        relation: Option<&RelationName>,
        walk: Walk,
        step: Option<usize>,
    ) -> Result<Vec<Vec<Edge>>> {
        let r = self.reader;
        // Every relation, where the hop is no step's.
        const ALL: &Reach = &Reach {
            owner: None,
            far: None,
        };
        let reach = step.and_then(|i| self.reach.get(i)).unwrap_or(ALL);
        // A hop past the first reaches types the step's own probe did not
        // look at.
        r.probe_derived(ty, relation, walk, reach)?;
        // A relation the step does not walk around `ty` is not read either.
        let mut shown = r.shown_relations(Some(ty));
        shown.retain(|(t, x)| reach.walks(ty, walk, t, x));
        let mut lists = ProjectionView::edges(r.view.as_ref(), ty, keys, relation, walk)?;
        for (list, derived) in
            lists
                .iter_mut()
                .zip(derived_edges(r, ty, keys, (relation, &shown), walk)?)
        {
            list.extend(derived);
        }
        // An edge of a relation the principal is not shown, or the step
        // does not walk, answers nothing, and no link record is read for
        // it: a step over another type's relation of the same name walks
        // its own (024, extended).
        let read =
            |e: &Edge| (shown.iter()).any(|(t, x)| t.name == e.from.0 && x.name == e.relation);
        lists.iter_mut().for_each(|list| list.retain(read));
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
                .is_some_and(RelationDecl::carried_by_links)
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
                    // `only` on the link type admits it by its fields (051).
                    let decl = r.shape.ontology.ty(&lt);
                    let restricted = r.sight.withheld(link);
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
            let Ok(decl) = r.sight.ty(ty) else {
                continue;
            };
            let rows = r.get_many(ty, &keys)?;
            self.remember(rows.iter().cloned());
            let (ends, admit) = (r.admitted_ends(decl, &rows)?, r.sight.admit(ty));
            for (rec, _) in rows.into_iter().zip(ends).filter(|(_, ends)| *ends) {
                if r.live_by(decl, &admit, &rec) {
                    shown.insert((rec.ty, rec.key));
                }
            }
        }
        Ok(nodes.iter().map(|n| shown.contains(n)).collect())
    }
}

/// Reciprocal rank fusion of ranked lists, each with its weight: a list
/// adds `weight / (60 + rank)` for every key it ranks (rank from 0, best
/// first), in the lists' order. Ties are broken by the key answered, the
/// one a record bears at the read's position, so the fused order is the
/// same on every engine and on every page (029). A search hands it only
/// what the read may see (046).
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
