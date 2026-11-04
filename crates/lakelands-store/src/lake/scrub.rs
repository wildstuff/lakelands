//! The scoped refold of a forgetting
//! (`docs/decisions/058-a-source-may-be-forgotten.md`) and of the records
//! an act of removal selects (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`):
//! in place of a rebuild, only those records are restated, each from its
//! own events and the events about the whole lake, through the fold a
//! rebuild runs. Every act the lists hold is told before the first page,
//! so a page that folds a record an older act took leaves it erased.
//!
//! A page of those records is one transaction: their states, history
//! included, are dropped where it begins ([`ProjectionStore::drop_states`]),
//! and the fold puts them again from the log, erasing what the destroyed
//! keys sealed. The fold writes through [`Scoped`], which keeps every put
//! but the page's own from the engine, and reads through [`ScopedView`],
//! which answers as of the event folded, with the page's records unknown
//! until the fold puts them. Every other state is the one the projection
//! holds already, folded from the same log, so the records come out as a
//! rebuild leaves them. A walk the lake's own events take, a migration's
//! or a declaration's, reads only the types the page's records read, a
//! reference or a view at a time, and the store's own (`sys.`) types but
//! the links of a type they do not read: what it puts of any other type
//! the page keeps from the engine, and no record restated reads it.
//!
//! What the fold puts of a record beyond its states reaches further: a
//! kept view's groups, a set's members, the vector index, another type's
//! computed field reading it through a reference, however many steps
//! away. Where the records hold any of those, or a page folds more events
//! than `limits.page_events`, the projection is rebuilt instead, and
//! [`Plan::why`] says why. Nor does the refold touch the vector index: the
//! records it restates keep no vectors, and every other record's are those
//! the head holds.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use lakelands_api::engine::{
    Applied, Choose, Chosen, Compact, Declared, LakeEngine, Matched, ProjectionStore,
    ProjectionView, Reclaimed, Scan, StatesOf,
};
use lakelands_api::event::{Digest, Event, EventBody, EventId};
use lakelands_api::ontology::{FieldDecl, StorageKind, TypeDecl};
use lakelands_api::quarantine::{QuarantineFilter, Quarantined};
use lakelands_api::query::{Edge, Walk};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::seam::{FeedFilter, SETTING_TYPE, VIEW_KEPT_SETTING};
use lakelands_api::write::{Declaration, ForgetWay, Proposal, ProposalFilter};
use lakelands_api::{Position, RecordKey, RelationName, Result, SourceId, TypeName};

use super::{Lake, Reads, Staged, Writer, every, forgotten_list, indexed};
use crate::ontology::Ontology;

/// How many records one page of a scoped refold restates.
const PAGE: usize = 256;

/// How many positions one read of the feed index answers.
const MATCHED: u32 = 1024;

/// A set of records, each named by its type and key.
type Keys = BTreeSet<(TypeName, RecordKey)>;

/// What one page of a scoped refold restates.
pub(crate) struct Scope {
    /// The page's records.
    keys: Keys,
    /// The types of the records every page restates.
    types: BTreeSet<TypeName>,
    /// The types those records read, however many steps away: the only
    /// ones beyond the store's own (`sys.`) a walk reads.
    near: BTreeSet<TypeName>,
    /// The outputs the page drops and records again.
    outputs: BTreeSet<(String, Digest)>,
    /// The position the projection stays folded through.
    head: Position,
    /// The position the event being folded reads the lake as of.
    at: std::sync::atomic::AtomicU64,
}

impl Scope {
    fn at(&self) -> Position {
        Position(self.at.load(Ordering::SeqCst))
    }

    fn hides(&self, ty: &TypeName, key: &RecordKey) -> bool {
        self.types.contains(ty) && self.keys.contains(&(ty.clone(), key.clone()))
    }

    /// Whether a walk of `ty` reads the lake: not one of a type restated,
    /// whose records the page puts, nor of a type they do not read or of
    /// its links, whose puts the page keeps from the engine and no record
    /// restated reads.
    fn walks(&self, ty: &TypeName) -> bool {
        let read = match lakelands_core::links::link_type_parts(ty) {
            Some((of, _)) => self.near.contains(&of),
            None => ty.as_str().starts_with("sys.") || self.near.contains(ty),
        };
        !self.types.contains(ty) && read
    }
}

/// The projection the fold writes to: the engine's, or under a scope the
/// page's records and outputs alone, every other put kept from it, and
/// the commit at the head the projection is folded through.
pub(crate) struct Scoped {
    engine: Arc<dyn LakeEngine>,
    scope: Mutex<Option<Arc<Scope>>>,
}

impl Scoped {
    pub(crate) fn new(engine: Arc<dyn LakeEngine>) -> Scoped {
        let scope = Mutex::new(None);
        Scoped { engine, scope }
    }

    fn scope(&self) -> Option<Arc<Scope>> {
        lakelands_core::lock(&self.scope).clone()
    }

    fn inner(&self) -> &dyn ProjectionStore {
        self.engine.projection()
    }

    /// Fold the next event as of `at`, the position before it.
    pub(crate) fn reads_at(&self, at: Position) {
        if let Some(s) = self.scope() {
            s.at.store(at.0, Ordering::SeqCst);
        }
    }

    /// Whether a scoped refold is folding.
    pub(crate) fn scoped(&self) -> bool {
        self.scope().is_some()
    }

    /// `head`, or under a scope the view the fold reads it through.
    pub(crate) fn view<'a>(
        &'a self,
        head: Box<dyn ProjectionView + 'a>,
    ) -> Box<dyn ProjectionView + 'a> {
        match self.scope() {
            Some(scope) => Box::new(ScopedView {
                head,
                engine: self.engine.as_ref(),
                scope,
                then: RefCell::new(None),
            }),
            None => head,
        }
    }
}

impl ProjectionStore for Scoped {
    fn begin(&self) -> Result<()> {
        self.inner().begin()?;
        let Some(s) = self.scope() else {
            return Ok(());
        };
        // Dropped in the transaction that puts them again.
        for (ty, key) in &s.keys {
            self.inner().drop_states(StatesOf::Record(ty, key))?;
        }
        for (seam, input) in &s.outputs {
            self.inner().drop_states(StatesOf::Output(seam, input))?;
        }
        Ok(())
    }

    fn commit(&self, through: Position) -> Result<()> {
        let through = self.scope().map_or(through, |s| s.head);
        self.inner().commit(through)
    }

    fn rollback(&self) {
        self.inner().rollback();
    }

    fn folded_through(&self) -> Result<Option<Position>> {
        self.inner().folded_through()
    }

    fn put_declaration(
        &self,
        declaration: &Declaration,
        version: u32,
        schema_hash: Option<&str>,
        at: Position,
    ) -> Result<()> {
        match self.scoped() {
            true => Ok(()),
            false => (self.inner()).put_declaration(declaration, version, schema_hash, at),
        }
    }

    fn put_record(&self, record: &StoredRecord) -> Result<()> {
        match self.scope() {
            Some(s) if !s.hides(&record.ty, &record.key) => Ok(()),
            _ => self.inner().put_record(record),
        }
    }

    fn put_record_for(&self, record: &StoredRecord, whole: bool) -> Result<()> {
        match self.scope() {
            Some(s) if !s.hides(&record.ty, &record.key) => Ok(()),
            _ => self.inner().put_record_for(record, whole),
        }
    }

    fn end_head_only(&self, name: &TypeName, at: Position) -> Result<()> {
        match self.scoped() {
            true => Ok(()),
            false => self.inner().end_head_only(name, at),
        }
    }

    fn put_order(&self, name: &TypeName, key: &RecordKey, entry: Option<&[u8]>) -> Result<()> {
        match self.scoped() {
            true => Ok(()),
            false => self.inner().put_order(name, key, entry),
        }
    }

    fn end_order(&self, name: &TypeName) -> Result<()> {
        match self.scoped() {
            true => Ok(()),
            false => self.inner().end_order(name),
        }
    }

    fn drop_states(&self, of: StatesOf<'_>) -> Result<()> {
        match (self.scope(), &of) {
            (Some(s), StatesOf::Record(ty, key)) if !s.hides(ty, key) => Ok(()),
            _ => self.inner().drop_states(of),
        }
    }
    fn states(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        after: Option<Position>,
        through: Position,
        limit: u32,
    ) -> Result<Vec<StoredRecord>> {
        match self.scope() {
            Some(s) if !s.hides(ty, key) => Ok(Vec::new()),
            _ => self.inner().states(ty, key, after, through, limit),
        }
    }

    fn put_proposal(&self, proposal: &Proposal, at: Position) -> Result<()> {
        match self.scoped() {
            true => Ok(()),
            false => self.inner().put_proposal(proposal, at),
        }
    }

    fn put_quarantined(&self, row: &Quarantined, at: Position) -> Result<()> {
        match self.scoped() {
            true => Ok(()),
            false => self.inner().put_quarantined(row, at),
        }
    }

    fn put_membership(
        &self,
        set: &str,
        ty: &TypeName,
        key: &RecordKey,
        member: bool,
        at: Position,
    ) -> Result<()> {
        match self.scoped() {
            true => Ok(()),
            false => self.inner().put_membership(set, ty, key, member, at),
        }
    }

    fn put_output(&self, seam: &str, input: &Digest, output: &Value, at: Position) -> Result<()> {
        match self.scope() {
            Some(s) if !s.outputs.contains(&(seam.to_owned(), input.clone())) => Ok(()),
            _ => self.inner().put_output(seam, input, output, at),
        }
    }

    fn mark_applied(&self, applied: &Applied) -> Result<()> {
        match self.scoped() {
            true => Ok(()),
            false => self.inner().mark_applied(applied),
        }
    }

    fn clear(&self) -> Result<()> {
        self.inner().clear()
    }

    fn compact(&self, how: Compact) -> Result<Reclaimed> {
        self.inner().compact(how)
    }

    fn choose_order(&self, ty: &TypeName, step: &Choose) -> Result<Option<RecordKey>> {
        self.inner().choose_order(ty, step)
    }

    fn put_thinned_act(&self, act: &lakelands_api::read::ThinnedAct, at: Position) -> Result<()> {
        match self.scoped() {
            true => Ok(()),
            false => self.inner().put_thinned_act(act, at),
        }
    }

    fn put_thinned(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        before: lakelands_api::time::Timestamp,
        at: Position,
    ) -> Result<()> {
        match self.scoped() {
            true => Ok(()),
            false => self.inner().put_thinned(ty, key, before, at),
        }
    }
}

/// The lake as a scoped refold reads it before the event it folds: the
/// page's records and outputs unknown, as a rebuild meets them before it
/// has put them, a type it restates or whose records they do not read
/// walked as empty, and every other state as of the position before the
/// event.
struct ScopedView<'a> {
    head: Box<dyn ProjectionView + 'a>,
    engine: &'a dyn LakeEngine,
    scope: Arc<Scope>,
    then: RefCell<Option<(Position, Box<dyn ProjectionView + 'a>)>>,
}

impl ScopedView<'_> {
    /// `read` of the view as of the position before the event folded.
    fn then<T>(&self, read: impl FnOnce(&dyn ProjectionView) -> Result<T>) -> Result<T> {
        let at = self.scope.at();
        let mut then = self.then.borrow_mut();
        if then.as_ref().is_none_or(|(p, _)| *p != at) {
            *then = Some((at, self.engine.view(Some(at))?));
        }
        read(then.as_ref().expect("the view is read").1.as_ref())
    }
}

impl ProjectionView for ScopedView<'_> {
    fn position(&self) -> Option<Position> {
        Some(self.scope.at())
    }

    fn declarations(&self) -> Result<Vec<Declared>> {
        self.head.declarations()
    }

    fn replaced(&self, from: Position, to: Position) -> Result<Option<Vec<Declared>>> {
        self.head.replaced(from, to)
    }

    fn chosen(&self, ty: &TypeName) -> Result<Chosen> {
        self.head.chosen(ty)
    }

    fn get(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>> {
        if self.scope.hides(ty, key) {
            return Ok(None);
        }
        match self.head.get(ty, key)? {
            Some(r) if r.position > self.scope.at() => self.then(|v| v.get(ty, key)),
            r => Ok(r),
        }
    }

    fn order(&self, name: &TypeName, key: &RecordKey) -> Result<Option<Vec<u8>>> {
        self.head.order(name, key)
    }

    fn order_page(
        &self,
        name: &TypeName,
        after: Option<&RecordKey>,
        n: u32,
    ) -> Result<Vec<(RecordKey, Vec<u8>)>> {
        self.head.order_page(name, after, n)
    }

    fn scan(&self, ty: &TypeName, scan: &Scan) -> Result<Vec<StoredRecord>> {
        match self.scope.walks(ty) {
            true => self.then(|v| v.scan(ty, scan)),
            false => Ok(Vec::new()),
        }
    }

    fn count(&self, ty: &TypeName, scan: &Scan) -> Result<u64> {
        match self.scope.walks(ty) {
            true => self.then(|v| v.count(ty, scan)),
            false => Ok(0),
        }
    }

    fn get_many(&self, ty: &TypeName, keys: &[RecordKey]) -> Result<Vec<StoredRecord>> {
        let keys: Vec<RecordKey> = (keys.iter())
            .filter(|k| !self.scope.hides(ty, k))
            .cloned()
            .collect();
        self.then(|v| v.get_many(ty, &keys))
    }

    fn edges(
        &self,
        ty: &TypeName,
        keys: &[RecordKey],
        relation: Option<&RelationName>,
        walk: Walk,
    ) -> Result<Vec<Vec<Edge>>> {
        let mut edges = self.then(|v| v.edges(ty, keys, relation, walk))?;
        for (key, edges) in keys.iter().zip(&mut edges) {
            if self.scope.hides(ty, key) {
                edges.clear();
            }
        }
        Ok(edges)
    }

    fn proposals(&self, filter: &ProposalFilter) -> Result<Vec<Proposal>> {
        self.then(|v| v.proposals(filter))
    }

    fn quarantined(&self, filter: &QuarantineFilter) -> Result<Vec<Quarantined>> {
        self.then(|v| v.quarantined(filter))
    }

    fn proposals_before(
        &self,
        filter: &ProposalFilter,
        before: Option<Position>,
    ) -> Result<Option<Vec<Proposal>>> {
        self.head.proposals_before(filter, before)
    }

    fn is_member(&self, set: &str, ty: &TypeName, key: &RecordKey) -> Result<bool> {
        self.then(|v| v.is_member(set, ty, key))
    }

    fn members(&self, set: &str, ty: &TypeName) -> Result<BTreeSet<RecordKey>> {
        self.then(|v| v.members(set, ty))
    }

    fn applied(&self, id: &EventId) -> Result<Option<Position>> {
        self.head.applied(id)
    }

    fn applied_many(&self, ids: &[EventId]) -> Result<Vec<Option<Position>>> {
        self.head.applied_many(ids)
    }

    fn output(&self, seam: &str, input: &Digest) -> Result<Option<(Position, Value)>> {
        if (self.scope.outputs).contains(&(seam.to_owned(), input.clone())) {
            return Ok(None);
        }
        match self.head.output(seam, input)? {
            Some((p, _)) if p > self.scope.at() => self.then(|v| v.output(seam, input)),
            o => Ok(o),
        }
    }

    fn matching(
        &self,
        filter: &FeedFilter,
        from: Position,
        through: Position,
        limit: u32,
    ) -> Result<Matched> {
        self.head.matching(filter, from, through, limit)
    }

    fn of_source(
        &self,
        source: &SourceId,
        from: Position,
        through: Position,
        limit: u32,
    ) -> Result<Matched> {
        self.head.of_source(source, from, through, limit)
    }

    fn thinned_acts(&self, ty: &TypeName) -> Result<Vec<lakelands_api::read::ThinnedAct>> {
        self.head.thinned_acts(ty)
    }

    fn thinned_at(
        &self,
        ty: &TypeName,
        key: &RecordKey,
    ) -> Result<Option<lakelands_api::time::Timestamp>> {
        self.head.thinned_at(ty, key)
    }
}

/// How forgetting a set of sources and restating the acts of removal
/// given restates the projection: the records and outputs a scoped refold
/// restates, a page at a time with the positions of the events about each
/// page, or why the projection is rebuilt instead.
#[derive(Default)]
pub(crate) struct Plan {
    /// Each page's records, with the positions of the events about any
    /// of them (and perhaps others, which the refold leaves out).
    pages: Vec<(Keys, Vec<Position>)>,
    /// The events about the whole lake, which every page folds: the
    /// declarations, the migrations and the settings.
    wide: Vec<Event>,
    /// The types the records read, however many steps away
    /// ([`Scope::walks`]).
    near: BTreeSet<TypeName>,
    /// The records of those types the lake's migrations walk, which every
    /// page walks again.
    walks: u64,
    /// The outputs the sources sealed, dropped, and those recorded under
    /// their seams and inputs by any hand, recorded again.
    outputs: BTreeSet<(String, Digest)>,
    /// The `output_recorded` events the lake held when the plan was made:
    /// a page folds those of the dropped outputs and those filling a
    /// field of one of its records.
    recorded: Vec<Event>,
    /// Whether the restate reads the lake's outputs at all: a source
    /// sealed one, a type of theirs fills a field by a function, or the
    /// act's own turns sealed one past the walk.
    outputs_read: bool,
    /// Why the projection is rebuilt instead, each in a sentence.
    pub(crate) why: Vec<String>,
}

impl Plan {
    /// The records the refold restates.
    fn records(&self) -> u64 {
        self.pages.iter().map(|(k, _)| k.len() as u64).sum()
    }

    /// The events the refold folds: each page's, the lake's own and the
    /// records they walk once per page, and the outputs it records again.
    fn events(&self) -> u64 {
        let about: usize = self.pages.iter().map(|(_, p)| p.len()).sum();
        let pages = self.pages.len().max(1) as u64;
        let wide = (self.wide.len() as u64).saturating_add(self.walks);
        let keys = self.keys();
        let recorded = (self.recorded.iter())
            .filter(|e| self.folds(e, &keys, &self.outputs))
            .count();
        (about as u64)
            .saturating_add(wide.saturating_mul(pages))
            .saturating_add(recorded as u64)
    }

    /// The types the pages' records are of.
    fn types(&self) -> BTreeSet<TypeName> {
        (self.pages.iter())
            .flat_map(|(keys, _)| keys.iter().map(|(ty, _)| ty.clone()))
            .collect()
    }

    /// The records every page restates, all pages' together.
    fn keys(&self) -> Keys {
        (self.pages.iter())
            .flat_map(|(keys, _)| keys.iter().cloned())
            .collect()
    }

    /// Whether a page of `keys`, dropping `outputs`, folds `e`: an output
    /// of one it drops, or one filling a field of one of its records.
    fn folds(&self, e: &Event, keys: &Keys, outputs: &BTreeSet<(String, Digest)>) -> bool {
        match &e.body {
            EventBody::OutputRecorded {
                seam, input, field, ..
            } => {
                outputs.contains(&(seam.clone(), input.clone()))
                    || field
                        .as_ref()
                        .is_some_and(|(ty, key, _)| keys.contains(&(ty.clone(), key.clone())))
            }
            _ => false,
        }
    }

    /// The events the fullest page folds: the events about its records,
    /// the lake's own and the records they walk, and the outputs it
    /// records again, which only the first page folds. `None` where
    /// there is no page to fold.
    fn page_events(&self) -> Option<u64> {
        let fixed = (self.wide.len() as u64).saturating_add(self.walks);
        let none = BTreeSet::new();
        let mut most = None;
        for (i, (keys, at)) in self.pages.iter().enumerate() {
            let outputs = match i {
                0 => &self.outputs,
                _ => &none,
            };
            let recorded = (self.recorded.iter())
                .filter(|e| {
                    matches!(&e.body, EventBody::OutputRecorded { seam, input, field, .. }
                        if outputs.contains(&(seam.clone(), input.clone()))
                            || field
                                .as_ref()
                                .is_some_and(|(ty, key, _)| keys.contains(&(ty.clone(), key.clone()))))
                })
                .count() as u64;
            let page = (at.len() as u64)
                .saturating_add(fixed)
                .saturating_add(recorded);
            most = Some(most.map_or(page, |m: u64| m.max(page)));
        }
        most
    }

    /// Which way the act goes and what each is expected to take, the log
    /// `events` long: the scoped refold where nothing falls back and it
    /// is expected to take less than the rebuild.
    pub(crate) fn way(&self, lake: &Lake, events: u64) -> ForgetWay {
        let mut why = self.why.clone();
        // `page_events` guards a page: each page of a scoped refold is a
        // turn of the writer of its own, so a page over it would hold the
        // writer longer than one page may, and takes the rebuild instead
        // (099). What it bounds is a write's wait, not only a page's size.
        if let Some(page) = self.page_events() {
            let bound = u64::from(lake.limits.page_events.max(1));
            if page > bound {
                why.push(format!(
                    "a page refolds {page} events, more than `limits.page_events` ({bound})"
                ));
            }
        }
        let (scoped_ms, rebuild_ms) = (lake.scoped_ms(self.events()), lake.expected_ms(events));
        if why.is_empty() && scoped_ms > rebuild_ms {
            why.push(format!(
                "the scoped refold of {} events is expected to take longer than the rebuild of {events}",
                self.events()
            ));
        }
        ForgetWay {
            scoped: why.is_empty(),
            why,
            records: self.records(),
            events: self.events(),
            scoped_ms,
            rebuild_ms,
        }
    }
}

/// The positions from `from` through `head` of the events `filter`
/// admits, by the feed index; `None` where it does not decide them all.
fn positions_from(
    view: &dyn ProjectionView,
    filter: &FeedFilter,
    from: Position,
    head: Position,
) -> Result<Option<Vec<Position>>> {
    let (mut from, mut all) = (from, Vec::new());
    loop {
        if from > head {
            return Ok(Some(all));
        }
        let m = view.matching(filter, from, head, MATCHED)?;
        all.extend(m.positions);
        match m.through {
            Some(t) if t < head => {
                if t.next() <= from {
                    return Ok(None);
                }
                from = t.next();
            }
            Some(_) => return Ok(Some(all)),
            None => return Ok(None),
        }
    }
}

/// [`positions_from`] from the log's first event.
fn positions(
    view: &dyn ProjectionView,
    filter: &FeedFilter,
    head: Position,
) -> Result<Option<Vec<Position>>> {
    positions_from(view, filter, Position::GENESIS, head)
}

/// The events `filter` admits, read at the positions the feed index
/// names; `None` where it does not decide them all.
fn admitted(
    lake: &Lake,
    view: &dyn ProjectionView,
    filter: FeedFilter,
) -> Result<Option<Vec<Event>>> {
    let head = view.position().unwrap_or(Position::GENESIS);
    admitted_from(lake, view, filter, Position::GENESIS, head)
}

/// [`admitted`] from `from` through `head`.
fn admitted_from(
    lake: &Lake,
    view: &dyn ProjectionView,
    filter: FeedFilter,
    from: Position,
    head: Position,
) -> Result<Option<Vec<Event>>> {
    match positions_from(view, &filter, from, head)? {
        Some(at) => lake.read_these(&at).map(Some),
        None => Ok(None),
    }
}

/// The events about the whole lake, which every page folds: the
/// declarations, the migrations and the thinnings, which take a page's
/// record the fold puts again.
fn wide_filter() -> FeedFilter {
    FeedFilter {
        kinds: [
            "declared",
            "migrated",
            "value_type_migrated",
            "values_thinned",
        ]
        .map(str::to_owned)
        .to_vec(),
        ..FeedFilter::default()
    }
}

/// The settings the lake holds.
fn settings_filter() -> FeedFilter {
    FeedFilter {
        types: vec![TypeName::from(SETTING_TYPE)],
        ..FeedFilter::default()
    }
}

/// The outputs the lake recorded.
fn outputs_filter() -> FeedFilter {
    FeedFilter {
        kinds: vec!["output_recorded".to_owned()],
        ..FeedFilter::default()
    }
}

/// The views the settings in `wide` name kept.
fn kept_of(wide: &[Event]) -> BTreeSet<&str> {
    (wide.iter())
        .filter_map(|e| match &e.body {
            EventBody::RecordUpserted { key, .. } | EventBody::RecordIngested { key, .. } => {
                key.as_str().strip_prefix(VIEW_KEPT_SETTING)
            }
            _ => None,
        })
        .collect()
}

/// How forgetting `set` and restating the acts `acts` restates the
/// projection at `view`'s head: the rebuild where `rebuild` asks for it.
pub(crate) fn plan(
    lake: &Lake,
    view: &dyn ProjectionView,
    set: &[SourceId],
    acts: &[crate::remove::Removal],
    rebuild: bool,
) -> Result<Plan> {
    let mut plan = Plan::default();
    if rebuild {
        plan.why
            .push("the host asks for the rebuild (`limits.forget_rebuild`)".to_owned());
    }
    let Some(head) = view.position() else {
        return Ok(plan);
    };
    // What the sources wrote and sealed, from the feed index, and the
    // records an act of removal selects, which its walk visits too: each
    // source's events are read once.
    let (mut keys, mut kinds, mut sealed_any) = (BTreeSet::new(), BTreeSet::new(), false);
    for source in set {
        let used = crate::sealing::used_by(source);
        plan.outputs.insert((crate::sealing::USED.to_owned(), used));
    }
    let forgotten: BTreeSet<&SourceId> = set.iter().collect();
    let mut sources: Vec<SourceId> = set.to_vec();
    for act in acts {
        if !sources.contains(&act.source) {
            sources.push(act.source.clone());
        }
    }
    crate::forget::each_of(lake, view, &sources, |i, e| {
        let source = &sources[i];
        if indexed(e).as_ref() != Some(source) {
            return;
        }
        let sealed = e
            .sealed
            .as_ref()
            .filter(|s| s.source.as_ref() == Some(source));
        // The records a removal takes: those of the events its selection
        // picks. An output it sealed under a key the act destroyed is
        // dropped and recorded again, as a forgotten source's is.
        for act in acts.iter().filter(|a| &a.source == source) {
            if act.selects(e)
                && let EventBody::RecordIngested { ty, key, .. }
                | EventBody::RecordQuarantined { ty, key, .. } = &e.body
            {
                keys.insert((ty.clone(), key.clone()));
            }
        }
        if let (EventBody::OutputRecorded { seam, input, .. }, Some(sealed)) = (&e.body, sealed) {
            let destroyed = std::iter::once(&sealed.key)
                .chain(&sealed.apart)
                .any(|k| acts.iter().any(|a| a.destroyed(source, k)));
            if destroyed {
                sealed_any = true;
                plan.outputs.insert((seam.clone(), input.clone()));
            }
        }
        if !forgotten.contains(source) {
            return;
        }
        match &e.body {
            EventBody::RecordUpserted { ty, key, .. }
            | EventBody::RecordIngested { ty, key, .. }
            | EventBody::RecordRetired { ty, key, .. }
            | EventBody::RecordRestricted { ty, key, .. }
            | EventBody::RecordUnrestricted { ty, key, .. } => {
                keys.insert((ty.clone(), key.clone()));
            }
            EventBody::OutputRecorded {
                seam, input, field, ..
            } => {
                if sealed.is_some() {
                    sealed_any = true;
                    plan.outputs.insert((seam.clone(), input.clone()));
                    if let Some((ty, key, _)) = field {
                        keys.insert((ty.clone(), key.clone()));
                    }
                }
            }
            EventBody::LakeCreated { .. }
            | EventBody::LeaseTaken { .. }
            | EventBody::Checkpoint { .. }
            | EventBody::VerbCalled { .. }
            | EventBody::TriggerFired { .. }
            | EventBody::TriggerDropped { .. }
            | EventBody::SourceForgotten { .. } => {}
            body => {
                kinds.insert(body.kind());
            }
        }
    })?;
    let sealed = sealed_any;
    for kind in kinds {
        let why = format!("a source wrote a `{kind}` event, which only a rebuild restates");
        plan.why.push(why);
    }
    // The events about the whole lake, and the settings it holds.
    let (Some(wide), Some(set_by)) = (
        admitted(lake, view, wide_filter())?,
        admitted(lake, view, settings_filter())?,
    ) else {
        plan.why
            .push("the feed index does not decide the lake's events".to_owned());
        return Ok(plan);
    };
    let mut wide = wide;
    wide.extend(set_by);
    wide.sort_by_key(|e| e.position);
    wide.dedup_by_key(|e| e.position);
    plan.wide = wide;
    // The outputs recorded again: read only where the sources sealed one
    // or a type of theirs fills a field by a function.
    let types: BTreeSet<TypeName> = keys.iter().map(|(ty, _)| ty.clone()).collect();
    let fills = (plan.wide.iter()).any(|e| match &e.body {
        EventBody::Declared {
            declaration: Declaration::Type(t),
            ..
        } => types.contains(&t.name) && t.fields.iter().any(|f| f.function.is_some()),
        EventBody::Migrated { migration, .. } => {
            let t = &migration.to;
            types.contains(&t.name) && t.fields.iter().any(|f| f.function.is_some())
        }
        _ => false,
    });
    let recorded = match fills || sealed {
        true => admitted(lake, view, outputs_filter())?,
        false => Some(Vec::new()),
    };
    let Some(mut recorded) = recorded else {
        plan.why
            .push("the feed index does not decide the lake's outputs".to_owned());
        return Ok(plan);
    };
    recorded.sort_by_key(|e| e.position);
    plan.recorded = recorded;
    plan.outputs_read = fills || sealed;
    for page in Vec::from_iter(keys).chunks(PAGE) {
        let filter = FeedFilter {
            types: page
                .iter()
                .map(|(ty, _)| ty.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            keys: page
                .iter()
                .map(|(_, key)| key.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            ..FeedFilter::default()
        };
        let Some(at) = positions(view, &filter, head)? else {
            plan.why
                .push("the feed index does not decide the records' events".to_owned());
            return Ok(plan);
        };
        plan.pages.push((page.iter().cloned().collect(), at));
    }
    plan.reconsider(lake, view)?;
    Ok(plan)
}

impl Plan {
    /// Say what the declarations and the settings `wide` holds, and the
    /// records these pages restate, make of the scoped refold: the
    /// reasons it falls back, the types they read, and the records a walk
    /// visits. A reason is pushed once.
    fn reconsider(&mut self, lake: &Lake, view: &dyn ProjectionView) -> Result<()> {
        let types = self.types();
        for ty in types.iter().filter(|ty| ty.as_str().starts_with("sys.")) {
            self.why
                .push(format!("a source wrote records of `{ty}`, the store's own"));
        }
        let kept = kept_of(&self.wide);
        falls_back(&mut self.why, &types, &kept, &self.wide);
        let renames = lake.renames(Some(view))?;
        for r in &renames {
            let touches = types.contains(&r.ty) || r.to.as_ref().is_some_and(|t| types.contains(t));
            if r.to.is_some() && touches {
                let ty = &r.ty;
                self.why.push(format!(
                    "a migration renamed `{ty}`, whose records a source wrote"
                ));
            }
        }
        self.why.dedup();
        self.near = near(&types, &self.wide);
        self.walks = walks(view, &self.near, &types, &self.wide)?;
        Ok(())
    }

    /// Read the lake again past `from` through `head`, in a page's turn:
    /// the declarations, the migrations and the settings of `wide`, and
    /// the outputs where the restate reads them, and judge the way again.
    fn extend(
        &mut self,
        lake: &Lake,
        view: &dyn ProjectionView,
        from: Position,
        head: Position,
    ) -> Result<()> {
        let (Some(wide), Some(settings)) = (
            admitted_from(lake, view, wide_filter(), from, head)?,
            admitted_from(lake, view, settings_filter(), from, head)?,
        ) else {
            self.why
                .push("the feed index does not decide the lake's events".to_owned());
            return Ok(());
        };
        self.wide.extend(wide);
        self.wide.extend(settings);
        self.wide.sort_by_key(|e| e.position);
        self.wide.dedup_by_key(|e| e.position);
        if self.outputs_read {
            let Some(more) = admitted_from(lake, view, outputs_filter(), from, head)? else {
                self.why
                    .push("the feed index does not decide the lake's outputs".to_owned());
                return Ok(());
            };
            self.recorded.extend(more);
            self.recorded.sort_by_key(|e| e.position);
            self.recorded.dedup_by_key(|e| e.position);
        }
        self.reconsider(lake, view)
    }

    /// Add the records `extra` names, which the act's own turns took in
    /// past the walk, and the outputs the tail `tail` sealed, judged with
    /// the forgotten sources `set` and the acts `acts` as the head holds
    /// them: what the first turn of a restate adds to the walk's plan
    /// (099). `view` is at the log's head.
    fn absorb(
        &mut self,
        lake: &Lake,
        view: &dyn ProjectionView,
        extra: &Keys,
        tail: &[Event],
        set: &[SourceId],
        acts: &[crate::remove::Removal],
    ) -> Result<()> {
        let known: Keys = self.keys();
        let added: Vec<(TypeName, RecordKey)> = (extra.iter())
            .filter(|k| !known.contains(*k))
            .cloned()
            .collect();
        let head = view.position().unwrap_or(Position::GENESIS);
        for page in added.chunks(PAGE) {
            let filter = FeedFilter {
                types: page
                    .iter()
                    .map(|(ty, _)| ty.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                keys: page
                    .iter()
                    .map(|(_, key)| key.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                ..FeedFilter::default()
            };
            let Some(at) = positions(view, &filter, head)? else {
                self.why
                    .push("the feed index does not decide the records' events".to_owned());
                return Ok(());
            };
            self.pages.push((page.iter().cloned().collect(), at));
        }
        let mut reads_outputs = self.outputs_read;
        for e in tail {
            let Some(sealed) = e.sealed.as_ref() else {
                continue;
            };
            let source = sealed.source.as_ref();
            let forgotten = source.is_some_and(|s| set.contains(s));
            let destroyed = source.is_some_and(|s| {
                std::iter::once(&sealed.key)
                    .chain(&sealed.apart)
                    .any(|k| acts.iter().any(|a| a.destroyed(s, k)))
            });
            if let (true, EventBody::OutputRecorded { seam, input, .. }) =
                (forgotten || destroyed, &e.body)
            {
                self.outputs.insert((seam.clone(), input.clone()));
                reads_outputs = true;
            }
            if self.outputs_read && matches!(e.body, EventBody::OutputRecorded { .. }) {
                self.recorded.push(e.clone());
            }
        }
        // An output the act's own turns sealed makes the plan read the
        // lake's outputs after all: every one recorded through the view,
        // so the page that drops the state records it again (099).
        if reads_outputs && !self.outputs_read {
            let Some(all) = admitted_from(lake, view, outputs_filter(), Position::GENESIS, head)?
            else {
                self.why
                    .push("the feed index does not decide the lake's outputs".to_owned());
                return Ok(());
            };
            self.outputs_read = true;
            self.recorded = all;
        }
        self.recorded.sort_by_key(|e| e.position);
        self.recorded.dedup_by_key(|e| e.position);
        self.reconsider(lake, view)
    }
}

/// Say in `why` what of the declarations `wide` holds makes a scoped
/// refold of records of `types` fall back to the rebuild, the views
/// `kept` names kept at some point: what reads those records beyond
/// their own states, and a migration that moves keys.
fn falls_back(
    why: &mut Vec<String>,
    types: &BTreeSet<TypeName>,
    kept: &BTreeSet<&str>,
    wide: &[Event],
) {
    let (mut decls, mut sets) = (Vec::new(), Vec::new());
    for e in wide {
        match &e.body {
            EventBody::Declared {
                declaration: Declaration::Type(t),
                ..
            } => decls.push(t),
            EventBody::Declared {
                declaration: Declaration::ObjectSet(s),
                ..
            } => sets.push(s),
            EventBody::Migrated {
                migration, rekeyed, ..
            } => {
                if !rekeyed.is_empty() {
                    why.push("a migration re-keyed records".to_owned());
                }
                decls.push(&migration.to);
            }
            _ => {}
        }
    }
    let ours = |ty: &TypeName| types.contains(ty);
    // The types whose records read the records, however many steps away:
    // theirs, and each type whose declaration refers to one of these,
    // through a reference or a relation, or is a view over one.
    let mut reach: BTreeSet<&TypeName> = types.iter().collect();
    loop {
        let more: Vec<&TypeName> = (decls.iter())
            .filter(|d| !reach.contains(&d.name) && refers(d).any(|t| reach.contains(t)))
            .map(|d| &d.name)
            .collect();
        if more.is_empty() {
            break;
        }
        reach.extend(more);
    }
    for s in sets {
        let (set, ty) = (&s.name, &s.of);
        if reach.contains(ty) {
            why.push(format!(
                "the set `{set}` over `{ty}` reads records a source wrote"
            ));
        }
    }
    for d in decls {
        let name = &d.name;
        if let Some(v) = &d.view {
            let sources = std::iter::once(&v.source).chain(v.merge.iter().map(|m| &m.source));
            if kept.contains(name.as_str()) && sources.into_iter().any(|t| reach.contains(t)) {
                why.push(format!(
                    "the kept view `{name}` is over records a source wrote"
                ));
            }
        }
        if ours(name) {
            let vectors = |f: &FieldDecl| f.ty.kind == StorageKind::Vector || f.embed.is_some();
            if d.fields.iter().any(vectors) {
                why.push(format!("`{name}` keeps vectors"));
            }
            if d.relations.iter().any(|r| r.via.is_none()) {
                why.push(format!("`{name}` has a relation carried by link records"));
            }
        }
        let computed =
            |f: &FieldDecl| f.derived.is_some() || f.function.is_some() || f.embed.is_some();
        if d.fields.iter().any(computed) && refers(d).any(|t| reach.contains(t)) {
            why.push(format!(
                "`{name}` computes a field and refers to records a source wrote"
            ));
        }
    }
}

/// The types the records of `types` read, theirs among them, however
/// many steps away, by the declarations `wide` holds: each a type of
/// theirs refers to.
pub(super) fn near(types: &BTreeSet<TypeName>, wide: &[Event]) -> BTreeSet<TypeName> {
    let decls: Vec<&TypeDecl> = (wide.iter())
        .filter_map(|e| match &e.body {
            EventBody::Declared {
                declaration: Declaration::Type(t),
                ..
            } => Some(t),
            EventBody::Migrated { migration, .. } => Some(&migration.to),
            _ => None,
        })
        .collect();
    let mut near = types.clone();
    loop {
        let more: Vec<&TypeName> = (decls.iter())
            .filter(|d| near.contains(&d.name))
            .flat_map(|d| refers(d))
            .collect();
        let was = near.len();
        near.extend(more.into_iter().cloned());
        if near.len() == was {
            return near;
        }
    }
}

/// How many records of the types `near` names, those of `types` left
/// out, the migrations `wide` holds walk, retired ones among them, as
/// `view` holds them: each event walks a type once.
fn walks(
    view: &dyn ProjectionView,
    near: &BTreeSet<TypeName>,
    types: &BTreeSet<TypeName>,
    wide: &[Event],
) -> Result<u64> {
    let mut counted: BTreeMap<&TypeName, u64> = BTreeMap::new();
    let mut walks = 0u64;
    for e in wide {
        let walked: BTreeSet<&TypeName> = match &e.body {
            EventBody::Migrated { migration, .. } => BTreeSet::from([&migration.to.name]),
            EventBody::ValueTypeMigrated { types, .. } => types.iter().collect(),
            _ => BTreeSet::new(),
        };
        for ty in walked
            .into_iter()
            .filter(|t| near.contains(*t) && !types.contains(*t))
        {
            let n = match counted.get(ty) {
                Some(n) => *n,
                None => {
                    let n = view.count(ty, &every(true))?;
                    counted.insert(ty, n);
                    n
                }
            };
            walks = walks.saturating_add(n);
        }
    }
    Ok(walks)
}

/// The types `d` refers to: by a reference or a relation, or as a view
/// over them.
fn refers(d: &TypeDecl) -> impl Iterator<Item = &TypeName> {
    let fields = d.fields.iter().filter_map(|f| f.ty.target.as_ref());
    let relations = d.relations.iter().map(|r| &r.target);
    let viewed = (d.view.iter())
        .flat_map(|v| std::iter::once(&v.source).chain(v.merge.iter().map(|m| &m.source)));
    fields.chain(relations).chain(viewed)
}

/// What a whole act leaves the projection: the way the plan chose, folded
/// a page a turn of the writer (099). A page refolds its records whole
/// from the log, reading their positions through the head of its own
/// turn, so a write between two pages is in what the next folds, and a
/// page folded twice comes out as once.
pub(crate) struct Restate {
    /// How it restates.
    way: Way,
    /// The sources it forgot and the acts it restated: the lists say
    /// rebuilt when the last page is folded.
    set: Vec<SourceId>,
    acts: Vec<crate::remove::Removal>,
    /// The records the act's own turns took in past the walk, and the
    /// events of the tail, added to the plan in its first turn.
    extra: Keys,
    tail: Vec<Event>,
    /// The position the plan's own events were read through, and the page
    /// to fold next.
    seen: Position,
    next: usize,
    /// Whether the first turn ran: every act told, the keys destroyed,
    /// and the writer's state let go over the folds.
    opened: bool,
    /// The events the pages folded and the microseconds those folds took,
    /// summed: the scoped rate is measured over the folds alone, not the
    /// turns between them, in which other writers hold the road.
    folded: u64,
    micros: u64,
    /// Whether the act left storage to reclaim: a call that finds nothing
    /// owed at all restates and reclaims nothing (099).
    reclaim: bool,
    /// What follows the restate's own turns, each in turns of its own.
    then: Then,
}

/// What a restate does after its own turns (099).
enum Then {
    /// Its own turns: the pages, or the rebuild.
    Restate,
    /// The sweep of the bases a scoped pass left unused, once listed.
    Sweep(Option<Swept>),
    /// The reclaim.
    Reclaim(Reclaiming),
}

/// The unused bases, listed on a view at `past` while the lake had
/// rebuilt `rebuilds` times, and how many of them the turns have swept.
struct Swept {
    ids: Vec<u64>,
    past: Position,
    rebuilds: u64,
    next: usize,
}

impl Swept {
    /// The bases no state names, on a view: the rebuilds are counted
    /// first, so a rebuild that runs beside the listing is seen.
    fn on(lake: &Lake) -> Result<Swept> {
        let rebuilds = lake.rebuilds.load(Ordering::SeqCst);
        let view = lake.engine.view(None)?;
        let (ids, past) = (view.unused_bases()?, view.position());
        let past = past.unwrap_or(Position::GENESIS);
        Ok(Swept {
            ids,
            past,
            rebuilds,
            next: 0,
        })
    }
}

/// A reclaim's turns so far: the storage before the first and after the
/// last, their time summed, and a step that failed.
#[derive(Default)]
struct Reclaiming {
    beside: bool,
    journal: bool,
    before: Option<u64>,
    after: Option<u64>,
    ms: u64,
    failed: Option<lakelands_api::Error>,
}

/// What one turn of a reclaim gives back, at most: 10 MiB of free pages.
const RECLAIM_TURN: u64 = 10 << 20;

impl Reclaiming {
    /// One step: whether free pages are left for the next.
    fn run(&mut self, lake: &Lake, how: Compact) -> bool {
        let began = std::time::Instant::now();
        match lake.engine.projection().compact(how) {
            Ok(got) => {
                self.before = self.before.or(got.before);
                self.after = got.after.or(self.after);
                self.ms += super::elapsed_ms(began).max(1);
                got.more
            }
            Err(e) => {
                self.failed = Some(e);
                false
            }
        }
    }
}

/// How a restate restates the projection.
enum Way {
    /// The scoped refold: a page a turn.
    Scoped(Plan),
    /// The whole projection, in one turn.
    Rebuild,
    /// Nothing to restate: the reclaim alone.
    Nothing,
}

/// What one turn of a restate did.
pub(crate) enum Stepped {
    /// The restate has turns left.
    More,
    /// It is done, and the reclaim began: its report, `None` where the
    /// reclaim failed.
    Done(Option<lakelands_api::write::CompactReport>),
}

impl Restate {
    /// What forgetting `set` and restating the acts `acts` leaves the
    /// projection, by the way `plan` chooses at `view`, `rebuild` where
    /// the host asks for the rebuild.
    pub(crate) fn of(
        lake: &Lake,
        view: &dyn ProjectionView,
        set: &[SourceId],
        acts: &[crate::remove::Removal],
        rebuild: bool,
    ) -> Result<Restate> {
        let plan = plan(lake, view, set, acts, rebuild)?;
        Ok(Self::planned(lake, view, set, acts, plan))
    }

    /// [`Self::of`] with the plan a walk already made, so that a job plans
    /// once.
    pub(crate) fn planned(
        lake: &Lake,
        view: &dyn ProjectionView,
        set: &[SourceId],
        acts: &[crate::remove::Removal],
        plan: Plan,
    ) -> Restate {
        let way = plan.way(lake, view.position().map_or(0, |p| p.0));
        *lakelands_core::lock(&lake.way) = Some(way.clone());
        let head = view.position().unwrap_or(Position::GENESIS);
        let way = match way.scoped {
            true => Way::Scoped(plan),
            false => Way::Rebuild,
        };
        Restate {
            way,
            set: set.to_vec(),
            acts: acts.to_vec(),
            extra: Keys::new(),
            tail: Vec::new(),
            seen: head,
            next: 0,
            opened: false,
            folded: 0,
            micros: 0,
            reclaim: true,
            then: Then::Restate,
        }
    }

    /// What a whole act leaves where nothing is restated: the reclaim.
    pub(crate) fn nothing() -> Restate {
        Restate {
            reclaim: true,
            ..Restate::done()
        }
    }

    /// A repeat of an act the lists already hold done: no restate, and
    /// nothing to reclaim.
    pub(crate) fn done() -> Restate {
        Restate {
            way: Way::Nothing,
            set: Vec::new(),
            acts: Vec::new(),
            extra: Keys::new(),
            tail: Vec::new(),
            seen: Position::GENESIS,
            next: 0,
            opened: false,
            folded: 0,
            micros: 0,
            reclaim: false,
            then: Then::Restate,
        }
    }

    /// Add what the act's own turns took in past the walk: the records
    /// `extra` names, the acts `acts` as the head holds them, and the
    /// events of `tail`. The first turn folds these into the plan.
    pub(crate) fn absorb(
        &mut self,
        extra: impl IntoIterator<Item = (TypeName, RecordKey)>,
        acts: Vec<crate::remove::Removal>,
        tail: &[Event],
    ) {
        self.extra.extend(extra);
        if !acts.is_empty() {
            self.acts = acts;
        }
        self.tail.extend(tail.iter().cloned());
    }

    /// One turn of the writer: the next page, a rebuild, or the reclaim
    /// alone.
    pub(crate) fn step(&mut self, lake: &Lake, w: &mut Writer) -> Result<Stepped> {
        match self.then {
            Then::Sweep(_) => return self.sweep(lake, w),
            Then::Reclaim(_) => return self.reclaim(lake),
            Then::Restate => {}
        }
        if !self.opened {
            self.open(lake, w)?;
            self.opened = true;
        }
        let (mut plan, page, restated, scoped) =
            match std::mem::replace(&mut self.way, Way::Nothing) {
                Way::Scoped(mut plan) => {
                    let page = self.page(lake, w, &mut plan)?;
                    (Some(plan), page, true, true)
                }
                Way::Rebuild => {
                    lake.rebuild_held(w)?;
                    (None, Page::Done, true, false)
                }
                Way::Nothing => (None, Page::Done, false, false),
            };
        let measured = match page {
            Page::More => {
                self.way = Way::Scoped(plan.expect("a restate with pages left has its plan"));
                return Ok(Stepped::More);
            }
            Page::Rebuild => {
                let plan = plan.as_mut().expect("a scoped restate has its plan");
                let way = plan.way(lake, w.tip.as_ref().map_or(0, |t| t.0.0));
                *lakelands_core::lock(&lake.way) = Some(way);
                lake.rebuild_held(w)?;
                // The pass ended as the rebuild: its pages' rate says
                // nothing of what a scoped refold takes (099).
                false
            }
            Page::Done => scoped,
        };
        if restated {
            self.finish(lake, w, measured)?;
        }
        // A rebuild leaves no base unused, so only the scoped pass sweeps;
        // the reclaim's turns follow either, since a rebuild in place, which
        // the act does not tell from a swap, leaves erased pages free and in
        // the journal, and after a swap they find little to do (099).
        self.then = match (self.reclaim, restated && measured) {
            (false, _) => return Ok(Stepped::Done(None)),
            (true, true) => Then::Sweep(None),
            (true, false) => Then::Reclaim(Reclaiming::default()),
        };
        Ok(Stepped::More)
    }

    /// Off the writer, before a turn: the sweep's list, on a view, and the
    /// reclaim an engine runs beside reads and writes.
    pub(crate) fn aside(&mut self, lake: &Lake) -> Result<()> {
        match &mut self.then {
            Then::Sweep(listed @ None) => *listed = Some(Swept::on(lake)?),
            Then::Reclaim(r) if !r.beside => {
                r.beside = true;
                r.run(lake, Compact::Beside);
            }
            _ => {}
        }
        Ok(())
    }

    /// One chunk of the sweep, of `order_chunk` bases, each tested against
    /// the states put after the view it was listed on: a write in between
    /// that names one keeps it. A rebuild or a scoped pass since put states
    /// before that view, so the list is let go, its bases left for the next
    /// sweep. A base holds provenance and no value, so no erasure waits on
    /// it.
    fn sweep(&mut self, lake: &Lake, w: &mut Writer) -> Result<Stepped> {
        let Then::Sweep(listed) = &mut self.then else {
            unreachable!("a sweep's turn");
        };
        // Listed under the writer only where no turn went before: an open
        // that finishes an act.
        let s = match listed {
            Some(s) => s,
            None => listed.insert(Swept::on(lake)?),
        };
        let chunk = usize::try_from(lake.limits.order_chunk.max(1)).unwrap_or(usize::MAX);
        let ids = &s.ids[s.next.min(s.ids.len())..];
        let ids = &ids[..chunk.min(ids.len())];
        if ids.is_empty() || lake.rebuilds.load(Ordering::SeqCst) != s.rebuilds {
            self.then = Then::Reclaim(Reclaiming::default());
            return Ok(Stepped::More);
        }
        let head = w.tip.as_ref().map_or(Position::GENESIS, |t| t.0);
        let projection = lake.engine.projection();
        projection.begin()?;
        let swept = (projection.drop_states(StatesOf::Bases(ids, s.past)))
            .and_then(|()| projection.commit(head));
        if swept.is_err() {
            projection.rollback();
        }
        swept?;
        s.next += ids.len();
        Ok(Stepped::More)
    }

    /// One turn of the reclaim: the journal truncated first, then at most
    /// [`RECLAIM_TURN`] of free pages a turn, and the last turn tells what
    /// came back. A step that fails ends it, owed.
    fn reclaim(&mut self, lake: &Lake) -> Result<Stepped> {
        let Then::Reclaim(r) = &mut self.then else {
            unreachable!("a reclaim's turn");
        };
        if !std::mem::replace(&mut r.beside, true) {
            r.run(lake, Compact::Beside);
        }
        let how = match std::mem::replace(&mut r.journal, true) {
            true => Compact::Pages(RECLAIM_TURN),
            false => Compact::Journal,
        };
        if r.failed.is_none() && r.run(lake, how) {
            return Ok(Stepped::More);
        }
        let r = std::mem::take(r);
        let told = match r.failed {
            Some(e) => Err(e),
            None => Ok(Reclaimed {
                before: r.before,
                after: r.after,
                more: false,
            }),
        };
        Ok(Stepped::Done(lake.reclaimed(told, r.ms)?.ok()))
    }

    /// The first turn: every act the lists hold is told, and every key
    /// they destroyed is destroyed, before the first page, so a page that
    /// folds a record an older act took leaves it erased; the plan takes
    /// in what the act's own turns took in past the walk.
    fn open(&mut self, lake: &Lake, w: &mut Writer) -> Result<()> {
        let view = lake.engine.view(None)?;
        let (list, acts) = (
            forgotten_list(view.as_ref(), &Staged::default())?,
            crate::remove::removed_list(view.as_ref(), &Staged::default())?,
        );
        if list.values().chain(acts.values()).any(|s| s == "owed") {
            for (source, _) in list.iter().filter(|(_, s)| *s == "owed") {
                lake.keys.destroy(source)?;
            }
            for act in (crate::remove::acts(lake, view.as_ref())?)
                .iter()
                .filter(|r| acts.get(&r.at).is_some_and(|s| s == "owed"))
            {
                lake.keys.destroy_versions(&act.source, &act.keys)?;
                lake.keys.destroy_apart(&act.source, &act.apart)?;
            }
        }
        let listed = crate::remove::told(lake, view.as_ref())?;
        drop(view);
        for act in listed {
            match w.removals.iter_mut().find(|r| **r == act) {
                Some(met) => *met = act,
                None => w.removals.push(act),
            }
        }
        if let Way::Scoped(plan) = &mut self.way {
            let view = lake.engine.view(None)?;
            plan.absorb(
                lake,
                view.as_ref(),
                &self.extra,
                &self.tail,
                &self.set,
                &self.acts,
            )?;
        }
        lake.rebuilds.fetch_add(1, Ordering::SeqCst);
        lakelands_core::lock(&lake.shapes).clear();
        lake.keys.unlist();
        Ok(())
    }

    /// One page of the scoped refold, in this turn.
    fn page(&mut self, lake: &Lake, w: &mut Writer, plan: &mut Plan) -> Result<Page> {
        let view = lake.engine.view(None)?;
        let head = w.tip.as_ref().map_or(Position::GENESIS, |t| t.0);
        if self.seen < head {
            plan.extend(lake, view.as_ref(), self.seen.next(), head)?;
        }
        self.seen = head;
        if !plan.why.is_empty() {
            return Ok(Page::Rebuild);
        }
        let Some((keys, _)) = plan.pages.get(self.next) else {
            return Ok(Page::Done);
        };
        let keys = keys.clone();
        let filter = FeedFilter {
            types: keys
                .iter()
                .map(|(ty, _)| ty.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            keys: keys
                .iter()
                .map(|(_, key)| key.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            ..FeedFilter::default()
        };
        let Some(at) = positions(view.as_ref(), &filter, head)? else {
            plan.why
                .push("the feed index does not decide the records' events".to_owned());
            return Ok(Page::Rebuild);
        };
        let about = |e: &Event| {
            lakelands_core::feed::about(&e.body).iter().any(|a| {
                let (Some(ty), Some(key)) = (&a.ty, &a.key) else {
                    return false;
                };
                keys.contains(&(ty.clone(), key.clone()))
            })
        };
        let outputs = match self.next {
            0 => plan.outputs.clone(),
            _ => BTreeSet::new(),
        };
        let mut events = lake.read_these(&at)?;
        events.retain(about);
        events.extend(plan.wide.iter().cloned());
        events.extend(
            (plan.recorded.iter())
                .filter(|e| plan.folds(e, &keys, &outputs))
                .cloned(),
        );
        events.sort_by_key(|e| e.position);
        events.dedup_by_key(|e| e.position);
        // The bound is a write's wait: the page this turn folds, held to
        // `limits.page_events`, or the rebuild instead (099).
        let bound = u64::from(lake.limits.page_events.max(1));
        let folded = (events.len() as u64).saturating_add(plan.walks);
        if folded > bound {
            plan.why.push(format!(
                "a page refolds {folded} events, more than `limits.page_events` ({bound})"
            ));
            return Ok(Page::Rebuild);
        }
        drop(view);
        let scope = Scope {
            keys: keys.clone(),
            types: plan.types(),
            near: plan.near.clone(),
            outputs,
            head,
            at: std::sync::atomic::AtomicU64::new(0),
        };
        *lakelands_core::lock(&lake.scoped.scope) = Some(Arc::new(scope));
        // Folded from the first declaration again on each page: what the
        // writer held, and the calls it owes, are as they were after.
        let (ontology, settings) = (w.ontology.clone(), w.settings.clone());
        let owed = std::mem::take(&mut w.owed);
        w.ontology = Ontology::default().named();
        w.settings = BTreeMap::new();
        let count = events.len() as u64;
        let began = std::time::Instant::now();
        let folded = lake.fold_all(w, &events, Reads::default(), false);
        // The rate is the folds' own, not the turns between them.
        self.micros = self
            .micros
            .saturating_add(u64::try_from(began.elapsed().as_micros()).unwrap_or(u64::MAX));
        *lakelands_core::lock(&lake.scoped.scope) = None;
        (w.ontology, w.settings, w.owed) = (ontology, settings, owed);
        lakelands_core::lock(&lake.shapes).clear();
        lake.keys.unlist();
        folded?;
        self.folded = self.folded.saturating_add(count);
        self.next += 1;
        Ok(match self.next < plan.pages.len() {
            true => Page::More,
            false => Page::Done,
        })
    }

    /// The last turn: the rate of the refold, where the pass was the
    /// scoped one, and the lists saying what it restated is rebuilt.
    fn finish(&self, lake: &Lake, w: &mut Writer, measured: bool) -> Result<()> {
        if measured && self.folded > 0 && self.micros > 0 {
            let rate = self.folded.saturating_mul(1_000_000) / self.micros.max(1);
            lake.scoped_rate.store(rate.max(1), Ordering::SeqCst);
        }
        // Restated: the act waits on the sweep and the reclaim alone, as
        // after a rebuild on the reclaim.
        let view = lake.engine.view(None)?;
        let mut list = forgotten_list(view.as_ref(), &Staged::default())?;
        let mut removed = crate::remove::removed_list(view.as_ref(), &Staged::default())?;
        drop(view);
        for source in &self.set {
            list.insert(source.clone(), "rebuilt".to_owned());
        }
        for act in &self.acts {
            removed.insert(act.at, "rebuilt".to_owned());
        }
        let head = w.tip.as_ref().map_or(Position::GENESIS, |t| t.0);
        lake.put_lists(&list, &removed, head)?;
        lake.publish(w);
        Ok(())
    }
}

/// What one page's turn leaves.
enum Page {
    /// The restate has pages left.
    More,
    /// The last page is folded.
    Done,
    /// The page falls back: the way is the rebuild.
    Rebuild,
}

#[cfg(test)]
mod tests;
