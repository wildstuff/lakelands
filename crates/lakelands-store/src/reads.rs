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
//! position of its first page and the last row's sort values and key, so
//! the page set is stable while the lake is written.
//!
//! **A page reads a page.** `find` over a stored type pushes the order,
//! `after` and `limit + 1` down to the engine, and never asks for the
//! type. A filter the engine cannot apply is applied here over engine
//! pages read one after another until the page and one row more are kept
//! or the type is out: a page costs the rows read to fill it, bounded in
//! memory and linear in time. A page ordered by a field is sorted by the
//! engine over the type for that answer only, so the store asks for twice
//! as many rows each time it reads on (`docs/seams.md`, the ceiling of
//! `find`).
//!
//! **Views** are computed on read: the source's records at the same
//! position, filtered by the view's `where`, grouped by its `group_by`
//! fields (whose values form the view record's key, in declared order, by
//! the one key rule of `docs/log-format.md` §2.3) and measured. A group
//! with no member has no record, except that a view without `group_by` is
//! always one record, keyed [`lakelands_api::key::UNGROUPED`]. A view the
//! setting `view_kept.<type>` asks for is read from its kept records where
//! they are exactly what the read would compute (049); a walk's step reads
//! a view so too, one with parameters only where the start bound them.

use std::cell::{Cell, OnceCell, RefCell};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;

use lakelands_api::describe::{Kept, LakeDescription, NotKept, SortedWhole, TypeDescription};
use lakelands_api::engine::{After, Declared, ProjectionView, Ranked, Rankings, Scan};
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
use lakelands_api::seam::SETTING_TYPE as SETTING;
use lakelands_api::write::{Declaration, Proposal, ProposalFilter, ProposedWrite};
use lakelands_api::{
    Cursor, Error, FieldName, Missing, Position, Principal, Record, RecordKey, Refusal,
    RelationName, Result, Timestamp, TypeName, Value,
};
use lakelands_core::expr::{self, Place as Eval, RecordScope, aggregate};
use lakelands_core::graph::{self, EdgeSource};
use lakelands_core::sight::{Admit, Sight};
use lakelands_core::{canonical, derive as core_derive, engines as core_engines, valid_time};

use crate::declare::condition;
use crate::kept;
use crate::lake::{Lake, Shape};
use crate::ontology::Ontology;

/// A sight an entry of the listing is judged under beside the one in
/// force, and its lake ([`Reader::entry`]).
type Under<'s> = Option<(&'s Sight<'s>, &'s Ontology)>;

/// Everything one read sees.
pub struct Reader<'a> {
    /// The lake.
    pub lake: &'a Lake,
    /// Who reads, and what it is shown of the read's shape.
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
    /// The clock a listing judges what a proposed write reaches under:
    /// the position's, or the host's, never `valid_at`, which would let
    /// a reviewer move it until a hidden record's turn shows. A cursor's
    /// clock is taken too, but no listing is paged by a cursor.
    judged: Timestamp,
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
    /// The views whose sources [`Self::probe_view`] found readable, and
    /// whether for a computation, so that a view many views read is
    /// probed once a read (lakelands#942).
    probed: RefCell<BTreeSet<(TypeName, bool)>>,
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
    /// By type, its declarations' positions and key fields, newest first,
    /// read back as far as an entry asks ([`Self::rekeyed`]).
    keys: RefCell<BTreeMap<TypeName, KeyChain>>,
    /// The kept members of each set across an edge a read's `in_set` names,
    /// read once a read where its condition is read ([`Self::named_sets`]).
    members: RefCell<BTreeMap<String, BTreeSet<RecordKey>>>,
    /// The declarations' past this read has needed ([`Self::ontology_at`]).
    past: RefCell<Past>,
    /// The second stage of a search or of a pattern's start, as the door
    /// handed it and as this read decided it.
    second: RefCell<stage::Second>,
    /// The settings at the read's position, read once.
    settings: OnceCell<BTreeMap<String, Value>>,
    /// The settings as its reader is told them, where that is not whole
    /// ([`Self::told`]).
    told: OnceCell<Option<BTreeMap<String, Value>>>,
}

/// What a read holds of the declarations' past (lakelands#1064): those in
/// force at its position, every state replaced after `from`, read a range
/// at a time as entries further back ask, and the past it last built.
#[derive(Default)]
struct Past {
    /// In force at the read's position, once asked.
    now: Option<Vec<Declared>>,
    /// The states replaced after `from`; `None` where the engine keeps no
    /// such read, and each position is read whole.
    replaced: Option<(Position, Vec<Declared>)>,
    /// The engine keeps no such read.
    unkept: bool,
    /// The ontology built last, and where.
    built: Option<(Position, Ontology)>,
}

mod describe;
mod edges;
mod find;
mod held;
mod measures;
mod proposals;
mod query;
mod search;
mod stage;

pub(crate) use edges::{Ends, answered, answered_of};
use edges::{Reach, Visible, fuse};
pub(crate) use held::HeldNow;
use proposals::stands;
use stage::First;
pub use stage::{Staged, Window};

/// The head as a search as of a position reads it, from one view: its
/// shape, the reader with its grants followed there, and where each record
/// of the read's position stands there. The read's holds are read from
/// this view too ([`HeldNow`]): a re-key landing between two views of the
/// head would be in one and not in the other's moves.
///
/// The indexes are not a view: the engine reads them as they stand when
/// asked, and says the position they stood at ([`Pulled`]). A search's
/// head is that position. A head taken before the rankings were read would
/// not know a write that landed in between, so a visible record could bear
/// a held record's score, or be ranked by a text its reader may not read
/// (lakelands#826).
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

/// A view's parameters as one read binds them: canonical, and as the
/// literals `param()` becomes.
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
type EachBinding<'a> =
    dyn FnMut(&SortKey, &graph::Node, &[Option<graph::Reached>]) -> Result<bool> + 'a;

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
    /// The clock its first page read, which every page reads
    /// (lakelands#571); a cursor handed out without one reads as
    /// [`Reader::open`] decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    now: Option<Timestamp>,
    /// For a read with a second stage: the stage's order of its window and
    /// what its first page said of the first stage (067).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stage: Option<stage::Staged>,
}

/// The weights a lexical or hybrid search gives its text fields itself.
fn own_text_weights(mode: &SearchMode) -> Option<&BTreeMap<FieldName, f64>> {
    match mode {
        SearchMode::Lexical { text_fields, .. } | SearchMode::Hybrid { text_fields, .. } => {
            text_fields.as_ref()
        }
        SearchMode::Vector { .. } => None,
    }
}

/// The digest of what decides a search's hits and their order: its mode,
/// its `where`, `valid_at` and its second stage.
fn search_read(s: &Search) -> String {
    match &s.rerank {
        None => read_of((&s.mode, &s.filter, s.at.valid_at)),
        Some(r) => read_of((&s.mode, &s.filter, s.at.valid_at, r)),
    }
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
        return Err(Error::invalid(Case::CursorOtherRead { door }));
    }
    Ok(())
}

/// The start's type, which every read of a pattern but `components`
/// needs (018).
fn start_type(p: &Pattern) -> Result<&TypeName> {
    let asks = || projection_name(&p.project);
    (p.start.ty.as_ref()).ok_or_else(|| Error::invalid(Case::StartNoType { asks: asks() }))
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
        Projection::PageRank => "page_rank",
        Projection::Cheapest { .. } => "cheapest",
    }
    .into()
}

/// A cursor's state, when it was handed out for this door, this type and
/// these view parameters: a page continues only the answer it began, and
/// any other use is refused, naming what differs (018).
fn resume(c: &Cursor, door: &str, ty: Option<&TypeName>, params: &Params) -> Result<CursorState> {
    let state = decode(c)?;
    if state.door != door || state.ty.as_ref() != ty {
        let (was, was_ty) = (state.door.clone(), state.ty.clone().map(Box::new));
        let (door, ty) = (door.to_owned(), ty.cloned().map(Box::new));
        return Err(Error::invalid(Case::CursorOtherDoor {
            was,
            was_ty,
            door,
            ty,
        }));
    }
    if &state.params != params {
        return Err(Error::invalid(Case::CursorOtherParams));
    }
    Ok(state)
}

fn decode(c: &Cursor) -> Result<CursorState> {
    let bad = || Error::invalid(Case::CursorForeign);
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

/// A step over several types, at `at`, refused where one is read, or
/// where it reads a field they do not all declare alike: naming every
/// type it reaches, and the field with the types that differ, those
/// missing it and those that declare it otherwise.
fn several_types(at: &Place, reached: &BTreeSet<TypeName>, unlike: Option<Unlike>) -> Error {
    let mut all = reached.iter().cloned();
    let reached = all.next().expect("several types");
    let some = |n: Names| (!n.0.is_empty()).then(|| Box::new(n));
    let (field, missing, other) = match unlike {
        Some((f, missing, other)) => (Some(f), some(missing), some(other)),
        None => (None, None, None),
    };
    let case = Case::StepSeveralTypes {
        reached: Some(reached),
        also: Some(all.collect()),
        field,
        missing,
        other,
    };
    Error::invalid(Said::from(case).within(at.clone()))
}

/// A field the types a step reaches do not declare alike: the types
/// missing it, and those that declare it otherwise than most of them.
type Unlike = (FieldName, Names, Names);

/// A path of a pattern's rows read against a type: the type, its field,
/// and whether its name is over several types (`wide`).
type Read = (TypeName, FieldName, bool);

/// The fields a name over several types holds ([`Reader::shared`]), or
/// None for a name over one.
type Shared = Option<BTreeSet<FieldName>>;

/// A refusal of a field of `ty`, read over a step that reaches several
/// types, as a read of that type alone refuses it, its field named by
/// the type: `<type>.<field>`.
fn of_type(ty: &TypeName, e: Error) -> Error {
    match e {
        Error::Refused {
            field: Some(f),
            reason,
        } => Error::refused(Some(FieldName::from(format!("{ty}.{f}"))), reason),
        e => e,
    }
}

/// The most records a hop over a derived relation holds at once: a page of
/// starts asks its matches a page of this many at a time (`derived_edges`).
const MATCHED: u32 = 65_536;

/// Whether an expression needs something only the store knows or does:
/// the clock, the settings, per-field provenance, a pattern match, or a
/// set's kept membership, which is never handed to an engine
/// (`docs/seams.md`, `scan`).
fn needs_store(e: &Expr) -> bool {
    use Func::*;
    matches!(
        e,
        Expr::Call {
            func: Now | Today | Setting | KnownAt | SeenAt | Matches | InSet,
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
        let landed = match decl.field(&name) {
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

/// A read's scope over one record: its own, and the kept members of each
/// set across an edge its condition names by `in_set`
/// (`docs/decisions/055-a-path-steps-through-a-reference.md`).
struct Members<'a> {
    record: RecordScope<'a>,
    key: &'a RecordKey,
    members: &'a BTreeMap<String, BTreeSet<RecordKey>>,
}

impl expr::Scope for Members<'_> {
    fn value(&self, path: &[FieldName]) -> Option<Value> {
        self.record.value(path)
    }

    fn meta(&self, field: &FieldName) -> Option<lakelands_api::FieldMeta> {
        self.record.meta(field)
    }

    fn setting(&self, key: &str) -> Option<Value> {
        self.record.setting(key)
    }

    fn now(&self) -> Timestamp {
        self.record.now
    }

    fn in_set(&self, set: &str) -> Option<bool> {
        Some(self.members.get(set)?.contains(self.key))
    }
}

/// What [`Reader::kept_read`] answers of a kept view: its state, and how
/// many of its groups and its sources' are not exact.
type KeptRead = (kept::State, u64);

impl<'a> Reader<'a> {
    /// Open a read at `at`, of `view` and its `shape`, under its clock:
    /// the one a cursor carries from its first page; else the instant
    /// `valid_at` names; else the host's (`ReadAt::clock`); else, as of a
    /// position, the `known_at` of the event there, the clock the fold
    /// read it under (017); else the wall clock. A listing's judgments
    /// read all but `valid_at`. It is decided here once, and no other
    /// read path in the store reads the wall clock (lakelands#571).
    pub fn open(
        lake: &'a Lake,
        sight: &'a Sight<'a>,
        (at, clock): (ReadAt, Option<Timestamp>),
        view: Box<dyn ProjectionView + 'a>,
        shape: Arc<Shape>,
    ) -> Result<Reader<'a>> {
        let then = view.position();
        let position = then.unwrap_or(Position::GENESIS);
        // A folded position the log holds no event at is a damaged log,
        // not a read under the wall clock.
        let known = |p: Position| match lake.read_these(&[p])?.first() {
            Some(e) => Ok(e.known_at),
            None => {
                let flaw = lakelands_api::error::Flaw::NoEvent { position: p };
                let part = lakelands_api::error::Part::Log;
                Err(lakelands_core::damaged(part, Some(p), flaw))
            }
        };
        // A page a cursor continues lists no proposal, and reads no log
        // for a clock it does not read; nor does a read the host clocks.
        let host = clock.or(at.clock);
        let judged = match (host, at.as_of.and(then)) {
            (Some(clock), _) => clock,
            (None, Some(p)) => known(p)?,
            (None, None) => crate::lake::wall_clock(),
        };
        let now = clock.or(at.valid_at).or(at.clock).unwrap_or(judged);
        Ok(Reader {
            lake,
            sight,
            view,
            shape,
            at_head: at.as_of.is_none(),
            position,
            valid_at: at.valid_at,
            now,
            judged,
            computed: RefCell::default(),
            bound: RefCell::default(),
            computing: RefCell::default(),
            kept_reads: RefCell::default(),
            probed: RefCell::default(),
            computes_known: RefCell::default(),
            head: None,
            held_now: false,
            retired: Cell::new(false),
            keys: RefCell::default(),
            members: RefCell::default(),
            past: RefCell::default(),
            second: RefCell::default(),
            settings: OnceCell::new(),
            told: OnceCell::new(),
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
    /// fields computed at the read, with this read's clock; then as its
    /// reader is told it ([`Sight::told`]: a setting narrowed, a field a
    /// hiding lowers gone), so a `where`, an order and an aggregate read
    /// what it is told.
    fn at_read(&self, ty: &TypeName, mut rows: Vec<StoredRecord>) -> Vec<StoredRecord> {
        let o = &self.shape.ontology;
        if let Some(decl) = o
            .ty(ty)
            .ok()
            .filter(|t| t.fields.iter().any(|f| f.at_read()))
        {
            let (decl, value_types) = (
                crate::record::resolve_functions(decl, o),
                o.value_type_list(),
            );
            for r in &mut rows {
                core_derive::at_read(&decl, &value_types, r, self.told(), self.now);
            }
        }
        if self.sight.narrows(ty) {
            let told = |r: StoredRecord| match self.sight.told(&r)? {
                std::borrow::Cow::Owned(o) => Some(o),
                std::borrow::Cow::Borrowed(_) => Some(r),
            };
            rows = rows.into_iter().filter_map(told).collect();
        }
        rows
    }

    /// Whether the store evaluates `e` over records of `decl` itself: it
    /// needs the store, or reads a field computed at the read, which no
    /// engine holds, or a setting its reader may not be told, or a field a
    /// hiding lowers on some records (006).
    fn store_only(&self, decl: &TypeDecl, e: &Expr) -> bool {
        let at_read = |n: &&FieldName| decl.fields.iter().any(|f| &f.name == *n && f.at_read());
        let read = e.fields_read();
        needs_store(e)
            || read.iter().any(at_read)
            || self.sight.narrows_reading(&decl.name, read.iter().copied())
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
            return Err(Error::invalid(Case::OrderAtRead { field, ty }));
        }
        Ok(())
    }

    /// The records of `ty` whose fields hold every value of one of the
    /// tuples `any`, as [`Self::get_many`] reads them, and the key the
    /// next page reads after: a page of at most [`MATCHED`] in key order
    /// after `after`; a view's all at once.
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
            return Err(Error::invalid(Case::ParamUnknown { ty, name, declared }));
        }
        let mut canonical = Params::new();
        let mut literals = BTreeMap::new();
        for p in declared {
            let Some(v) = given.get(&p.name) else {
                return Err(Error::invalid(unbound(ty, p, false)));
            };
            let wrong = || {
                let (name, ty, kind, value) =
                    (p.name.clone(), ty.clone(), p.kind, Box::new(v.clone()));
                Error::invalid(Case::ParamValue {
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

    /// Whether `ty` was re-keyed since `at`: a declaration of it made
    /// since keys it otherwise than the one before. A type's declarations
    /// are read back once a read, and only as far as asked.
    fn rekeyed(&self, ty: &TypeName, at: Position) -> Result<bool> {
        let mut keys = self.keys.borrow_mut();
        let chain = keys.entry(ty.clone()).or_default();
        if chain.is_empty() {
            let mut held = self
                .shape
                .declared
                .iter()
                .filter_map(|d| match &d.declaration {
                    Declaration::Type(t) if &t.name == ty => {
                        Some((d.since, t.key_fields().to_vec()))
                    }
                    _ => None,
                });
            chain.extend(held.next());
        }
        while let Some((since, key)) = chain.last().filter(|(s, _)| *s > at).cloned() {
            let before = self.lake.engine.view(Some(Position(since.0 - 1)))?;
            let was = before
                .declarations()?
                .into_iter()
                .find_map(|d| match d.declaration {
                    Declaration::Type(t) if &t.name == ty => {
                        Some((d.since, t.key_fields().to_vec()))
                    }
                    _ => None,
                });
            // Not declared before: as one that keeps its key.
            chain.push(was.unwrap_or((Position::GENESIS, key)));
        }
        Ok(chain.windows(2).any(|w| w[0].0 > at && w[0].1 != w[1].1))
    }

    /// The declarations in force at `at`, if the lake can be read there.
    /// A read asks the engine for the past once, not once a position: the
    /// declarations in force at its own position and the states replaced
    /// since the earliest position asked, each the engine's row, so the
    /// ontology built is the one a view at `at` reads (lakelands#1064).
    fn ontology_at(&self, at: Position) -> Option<Ontology> {
        if let Some((p, o)) = &self.past.borrow().built
            && *p == at
        {
            return Some(o.clone());
        }
        let o = match self.past_at(at).ok()? {
            Some(declared) => Ontology::from_declared(&declared),
            None => {
                let view = self.lake.engine.view(Some(at)).ok()?;
                Ontology::at(view.as_ref()).ok()?
            }
        };
        self.past.borrow_mut().built = Some((at, o.clone()));
        Some(o)
    }

    /// The declarations in force at `at`, from what [`Past`] holds and
    /// the range it is short of; `None` where the engine keeps no such read.
    fn past_at(&self, at: Position) -> Result<Option<Vec<Declared>>> {
        let mut past = self.past.borrow_mut();
        if past.unkept {
            return Ok(None);
        }
        let to = past
            .replaced
            .as_ref()
            .map_or(self.position, |(from, _)| *from);
        if at < to {
            let Some(rows) = self.view.replaced(at, to)? else {
                past.unkept = true;
                return Ok(None);
            };
            let held = past.replaced.get_or_insert_with(|| (at, Vec::new()));
            held.0 = at;
            held.1.extend(rows);
        }
        if past.now.is_none() {
            past.now = Some(self.view.declarations()?);
        }
        let now = past.now.iter().flatten();
        let replaced = past.replaced.iter().flat_map(|(_, r)| r);
        // Of each declaration, the latest state at or before `at`, in the
        // engine's order.
        let mut by: BTreeMap<String, &Declared> = BTreeMap::new();
        for d in now.chain(replaced).filter(|d| d.since <= at) {
            let k = core_engines::declaration_key(&d.declaration);
            if by.get(&k).is_none_or(|e| e.since < d.since) {
                by.insert(k, d);
            }
        }
        Ok(Some(by.into_values().cloned().collect()))
    }

    /// The settings at the read's position as its reader is told them
    /// ([`Sight::settings`]): what every expression the read evaluates
    /// reads by `setting('…')`, a view's computation too, so a view whose
    /// computation names a setting is computed for a reader they are
    /// narrowed to ([`Self::computes`]).
    fn told(&self) -> &BTreeMap<String, Value> {
        let told = self
            .told
            .get_or_init(|| match self.sight.settings(&self.shape.settings) {
                std::borrow::Cow::Borrowed(_) => None,
                std::borrow::Cow::Owned(s) => Some(s),
            });
        told.as_ref().unwrap_or(&self.shape.settings)
    }

    /// The settings at the read's position, read once a read.
    fn settings(&self) -> Result<&BTreeMap<String, Value>> {
        if let Some(s) = self.settings.get() {
            return Ok(s);
        }
        let s = crate::lake::settings_in(self.view.as_ref(), &self.shape.ontology)?;
        Ok(self.settings.get_or_init(|| s))
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
            let timed = valid_time::bounds(decl).filter(|n| self.knows(decl, n));
            self.sight.check_probe(decl, timed)?;
        }
        Ok(decl)
    }

    /// Whether the reader may know of `decl`'s field `name`.
    fn knows(&self, decl: &TypeDecl, name: &FieldName) -> bool {
        let f = decl.field(name);
        !f.is_some_and(|f| self.sight.field_access(decl, f) == lakelands_api::Access::None)
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
    /// its reader's `only` on it, or a hiding, cannot be applied (051, 006).
    fn named_type(&self, ty: &TypeName) -> Result<&TypeDecl> {
        let decl = self.visible_type(ty)?;
        self.sight.check_only(decl)?;
        self.sight.check_hidings(decl)?;
        Ok(decl)
    }

    /// Whether the principal may see a stored record at this read, before
    /// masking: live, admitted by its `only`, told ([`Sight::told`]), valid
    /// at `valid_at`. A link's ends are judged a page at a time
    /// ([`Self::admitted_ends`]).
    fn live(&self, decl: &TypeDecl, r: &StoredRecord) -> bool {
        self.live_by(decl, &self.sight.admit(&decl.name), r)
    }

    /// [`Self::live`] with what is admitted of `decl` worked out once for a
    /// batch.
    fn live_by(&self, decl: &TypeDecl, admit: &Admit, r: &StoredRecord) -> bool {
        (!r.retired || self.retired.get())
            && !self.sight.withheld(r)
            && admit.holds(&r.fields)
            && self.sight.told(r).is_some()
            && (self.valid_at_for(decl))
                .is_none_or(|t| valid_time::record_holds(decl, &r.fields, t))
            && (self.valid_for(decl).1).is_none_or(|b| self.holds(&b, r, false))
    }

    /// Whether a view's records are computed for this reader from the
    /// admitted records of what it reads, never served kept: under its
    /// `only` (051), or, without `restricted`, over a link type, or a view
    /// computed so, whose links are judged by their ends, as no kept face
    /// can (006); or whose computation names a setting the reader is told
    /// otherwise than the fold reads it ([`Self::told`]); or that reads a
    /// type a hiding of the reader's lowers, at any depth (006).
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
        let sources: Vec<&TypeName> = view
            .into_iter()
            .flat_map(|v| std::iter::once(&v.source).chain(v.merge.iter().map(|m| &m.source)))
            .collect();
        let links = |s: &&TypeName| lakelands_core::links::is_link_type(s) || self.computes(s);
        // To a reader the settings are narrowed to, a view that names a
        // setting, or reads `sys.setting` or a view computed for it: one
        // told them otherwise than whole ([`Self::told`]).
        let narrowed = !std::ptr::eq(self.told(), &self.shape.settings);
        let settings = |s: &&TypeName| {
            s.as_str() == SETTING || (find(s).is_some_and(|d| d.view.is_some()) && self.computes(s))
        };
        // A view over a type a hiding narrows reads it as told (006).
        let hides = (self.sight.who().grants.types.values()).any(|t| !t.hide_where.is_empty());
        let hidden = |s: &&TypeName| self.sight.hides(s) || self.computes(s);
        self.sight.computed_only(ty, &find)
            || (hides && sources.iter().any(hidden))
            || (!self.sight.sees_restricted() && sources.iter().any(links))
            || (narrowed
                && (find(ty).is_some_and(|d| crate::kept::names_settings(&self.shape.ontology, d))
                    || sources.iter().any(settings)))
    }

    /// Whether this reader is admitted every record of `decl` as stored.
    fn whole(&self, decl: &TypeDecl) -> bool {
        let admit = self.sight.admit(&decl.name);
        admit.pairs().is_some_and(|p| p.is_empty())
            && !self.sight.judges_ends(decl)
            && !self.sight.narrows(&decl.name)
    }

    /// Of link records of `decl`, which have every end this reader holds
    /// `only` on admitted: each such end type's records read once for the
    /// batch. Any other type's records all pass.
    fn admitted_ends(&self, decl: &TypeDecl, rows: &[StoredRecord]) -> Result<Vec<bool>> {
        let rows: Vec<_> = rows.iter().map(|r| &r.fields).collect();
        (self.sight).ends(decl, &rows, &mut |end, keys| self.view.get_many(end, keys))
    }

    /// A read's condition with the filter of each set it names in the
    /// name's place. A set of a type its reader is not shown, or whose
    /// filter reads a field its reader is kept from, or across an edge what
    /// is held from it (055, extended), is to that reader a set not
    /// declared (024): the refusal names nothing of the type or field.
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
            let filter = crate::declare::set_filter(&all, s, at);
            let reads = filter.map(|e| core_derive::reads(&e));
            let of = self.sight.ty(&s.of);
            of.is_ok_and(|t| reads.is_ok_and(|r| self.sight.check_probe(t, r.iter()).is_ok()))
                && self.sight.set_across_unheld(s)
        };
        let set = |n: &str| self.shape.ontology.set(n).filter(shown);
        // A set across an edge is read by its kept membership, its members
        // read once a read (055, extended). One decided at the read keeps
        // none: it is expanded, and a kept set it names is read by its own.
        let across = |n: &str| {
            set(n).is_some_and(|s| {
                !s.at_read() && crate::writes::reads_across(&self.shape.ontology, s)
            })
        };
        let e = crate::declare::filter_kept(&set, &across, ty, c, Eval::Record, at)?;
        let mut kept = Vec::new();
        lakelands_core::describe::in_sets(&e, &mut kept);
        for n in kept
            .into_iter()
            .filter(|n| !self.members.borrow().contains_key(n))
        {
            let held = self.view.members(&n, ty)?;
            self.members.borrow_mut().insert(n, held);
        }
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
        self.sight.check_probe(decl, &core_derive::reads(&e))?;
        crate::declare::evaluates(&e, decl, at)?;
        Ok(Some(e))
    }
}

/// A type's declarations, newest first, each with its position and the
/// fields it is keyed by.
type KeyChain = Vec<(Position, Vec<FieldName>)>;

#[cfg(test)]
mod tests;
