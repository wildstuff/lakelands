//! The scoped refold of a forgetting
//! (`docs/decisions/058-a-source-may-be-forgotten.md`): in place of a
//! rebuild, only the records the forgotten sources wrote are restated,
//! each from its own events and the events about the whole lake, through
//! the fold a rebuild runs.
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
//! away. Where the records hold any of those, or the lake holds an act of
//! removal, the projection is rebuilt instead, and [`Plan::why`] says why.
//! Nor does the refold touch the vector index: the records it restates
//! keep no vectors, and every other record's are those the head holds.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use lakelands_api::engine::{
    Applied, Choose, Chosen, Declared, LakeEngine, Matched, ProjectionStore, ProjectionView,
    Reclaimed, Scan, StatesOf,
};
use lakelands_api::event::{Digest, Event, EventBody, EventId};
use lakelands_api::ontology::{FieldDecl, StorageKind, TypeDecl};
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

    fn end_head_only(&self, name: &TypeName, at: Position) -> Result<()> {
        match self.scoped() {
            true => Ok(()),
            false => self.inner().end_head_only(name, at),
        }
    }

    fn drop_states(&self, of: StatesOf<'_>) -> Result<()> {
        self.inner().drop_states(of)
    }

    fn put_proposal(&self, proposal: &Proposal, at: Position) -> Result<()> {
        match self.scoped() {
            true => Ok(()),
            false => self.inner().put_proposal(proposal, at),
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

    fn compact(&self) -> Result<Reclaimed> {
        self.inner().compact()
    }

    fn choose_order(&self, ty: &TypeName, step: &Choose) -> Result<Option<RecordKey>> {
        self.inner().choose_order(ty, step)
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
}

/// How forgetting a set of sources restates the projection: the records
/// and outputs a scoped refold restates, a page at a time with the
/// positions of the events about each page, or why the projection is
/// rebuilt instead.
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
    /// The `output_recorded` events a page folds: those of the dropped
    /// outputs, and those filling a field of a record restated.
    recorded: Vec<Event>,
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
        (about as u64)
            .saturating_add(wide.saturating_mul(pages))
            .saturating_add(self.recorded.len() as u64)
    }

    /// Which way the act goes and what each is expected to take, the log
    /// `events` long: the scoped refold where nothing falls back and it
    /// is expected to take less than the rebuild.
    pub(crate) fn way(&self, lake: &Lake, events: u64) -> ForgetWay {
        let mut why = self.why.clone();
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

/// The positions through `head` of the events `filter` admits, by the
/// feed index; `None` where it does not decide them all.
fn positions(
    view: &dyn ProjectionView,
    filter: &FeedFilter,
    head: Position,
) -> Result<Option<Vec<Position>>> {
    let (mut from, mut all) = (Position::GENESIS, Vec::new());
    loop {
        let m = view.matching(filter, from, head, MATCHED)?;
        all.extend(m.positions);
        match m.through {
            Some(t) if t < head => from = t.next(),
            Some(_) => return Ok(Some(all)),
            None => return Ok(None),
        }
    }
}

/// The events `filter` admits, read at the positions the feed index
/// names; `None` where it does not decide them all.
fn admitted(
    lake: &Lake,
    view: &dyn ProjectionView,
    filter: FeedFilter,
) -> Result<Option<Vec<Event>>> {
    let head = view.position().unwrap_or(Position::GENESIS);
    match positions(view, &filter, head)? {
        Some(at) => lake.read_these(&at).map(Some),
        None => Ok(None),
    }
}

/// How forgetting `set` restates the projection at `view`'s head: the
/// rebuild where `rebuild` asks for it.
pub(crate) fn plan(
    lake: &Lake,
    view: &dyn ProjectionView,
    set: &[SourceId],
    rebuild: bool,
) -> Result<Plan> {
    let mut plan = Plan::default();
    if rebuild {
        plan.why
            .push("the host asks for the rebuild (`limits.forget_rebuild`)".to_owned());
    }
    if !crate::remove::removed_list(view, &Staged::default())?.is_empty() {
        plan.why
            .push("the lake holds an act of removal, which a rebuild is told".to_owned());
    }
    let Some(head) = view.position() else {
        return Ok(plan);
    };
    // What the sources wrote and sealed, from the feed index.
    let (mut keys, mut kinds, mut sealed_any) = (BTreeSet::new(), BTreeSet::new(), false);
    for source in set {
        let used = crate::sealing::used_by(source);
        plan.outputs.insert((crate::sealing::USED.to_owned(), used));
    }
    crate::forget::each_of(lake, view, set, |i, e| {
        if indexed(e).as_ref() != Some(&set[i]) {
            return;
        }
        let sealed = e
            .sealed
            .as_ref()
            .filter(|s| s.source.as_ref() == Some(&set[i]));
        match &e.body {
            EventBody::RecordUpserted { ty, key, .. }
            | EventBody::RecordRetired { ty, key, .. }
            | EventBody::RecordRestricted { ty, key, .. }
            | EventBody::RecordUnrestricted { ty, key, .. } => {
                keys.insert((ty.clone(), key.clone()));
            }
            EventBody::RecordIngested { ty, key, .. } => {
                keys.insert((ty.clone(), key.clone()));
                if e.sealed.as_ref().is_some_and(|s| !s.apart.is_empty()) {
                    let at = crate::sealing::apart_at(&set[i], e.position);
                    plan.outputs.insert((crate::sealing::APART.to_owned(), at));
                }
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
            | EventBody::LakeUpgraded { .. }
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
    let types: BTreeSet<TypeName> = keys.iter().map(|(ty, _)| ty.clone()).collect();
    for ty in types.iter().filter(|ty| ty.as_str().starts_with("sys.")) {
        plan.why
            .push(format!("a source wrote records of `{ty}`, the store's own"));
    }
    // The events about the whole lake, and what of them falls back.
    let lake_wide = FeedFilter {
        kinds: ["declared", "migrated", "value_type_migrated"]
            .map(str::to_owned)
            .to_vec(),
        ..FeedFilter::default()
    };
    let settings = FeedFilter {
        types: vec![TypeName::from(SETTING_TYPE)],
        ..FeedFilter::default()
    };
    let (Some(mut wide), Some(set_by)) = (
        admitted(lake, view, lake_wide)?,
        admitted(lake, view, settings)?,
    ) else {
        plan.why
            .push("the feed index does not decide the lake's events".to_owned());
        return Ok(plan);
    };
    let kept: BTreeSet<&str> = (set_by.iter())
        .filter_map(|e| match &e.body {
            EventBody::RecordUpserted { key, .. } | EventBody::RecordIngested { key, .. } => {
                key.as_str().strip_prefix(VIEW_KEPT_SETTING)
            }
            _ => None,
        })
        .collect();
    falls_back(&mut plan.why, &types, &kept, &wide);
    let renames = lake.renames(Some(view))?;
    for r in &renames {
        let touches = types.contains(&r.ty) || r.to.as_ref().is_some_and(|t| types.contains(t));
        if r.to.is_some() && touches {
            let ty = &r.ty;
            plan.why.push(format!(
                "a migration renamed `{ty}`, whose records a source wrote"
            ));
        }
    }
    plan.why.dedup();
    // The outputs recorded again: read only where the sources sealed one
    // or a type of theirs fills a field by a function.
    let fills = (wide.iter()).any(|e| match &e.body {
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
    let outputs = FeedFilter {
        kinds: vec!["output_recorded".to_owned()],
        ..FeedFilter::default()
    };
    let recorded = match fills || sealed {
        true => admitted(lake, view, outputs)?,
        false => Some(Vec::new()),
    };
    let Some(recorded) = recorded else {
        plan.why
            .push("the feed index does not decide the lake's outputs".to_owned());
        return Ok(plan);
    };
    plan.near = near(&types, &wide);
    plan.walks = walks(view, &plan.near, &types, &wide)?;
    wide.extend(set_by);
    wide.sort_by_key(|e| e.position);
    plan.wide = wide;
    plan.recorded = (recorded.into_iter())
        .filter(|e| match &e.body {
            EventBody::OutputRecorded {
                seam, input, field, ..
            } => {
                plan.outputs.contains(&(seam.clone(), input.clone()))
                    || field
                        .as_ref()
                        .is_some_and(|(ty, key, _)| keys.contains(&(ty.clone(), key.clone())))
            }
            _ => false,
        })
        .collect();
    let keys = Vec::from_iter(keys);
    for page in keys.chunks(PAGE) {
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
    Ok(plan)
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

impl Lake {
    /// The scoped refold of `plan`, for the sources `set` forgot, under
    /// the writer's lock its caller holds: each page in one transaction,
    /// the list of forgotten sources saying each is rebuilt once all are
    /// folded. One stopped between pages leaves the act owed, and the
    /// next open, or the act's repeat, plans and folds it again from the
    /// first page: each page is restated whole from the log, so a page
    /// folded twice comes out as once.
    pub(crate) fn scrub_held(&self, w: &mut Writer, plan: &Plan, set: &[SourceId]) -> Result<()> {
        let head = w.tip.as_ref().map_or(Position::GENESIS, |t| t.0);
        let types: BTreeSet<TypeName> = (plan.pages.iter())
            .flat_map(|(keys, _)| keys.iter().map(|(ty, _)| ty.clone()))
            .collect();
        self.rebuilds.fetch_add(1, Ordering::SeqCst);
        lakelands_core::lock(&self.shapes).clear();
        self.keys.unlist();
        let began = std::time::Instant::now();
        // Folded from the first declaration again on each page: what the
        // writer held, and the calls it owes, are as they were after.
        let (ontology, settings) = (w.ontology.clone(), w.settings.clone());
        let owed = std::mem::take(&mut w.owed);
        let folded = (|| -> Result<()> {
            for (i, (keys, at)) in plan.pages.iter().enumerate() {
                let about = |e: &Event| {
                    lakelands_core::feed::about(&e.body).iter().any(|a| {
                        let (Some(ty), Some(key)) = (&a.ty, &a.key) else {
                            return false;
                        };
                        keys.contains(&(ty.clone(), key.clone()))
                    })
                };
                let outputs = match i {
                    0 => plan.outputs.clone(),
                    _ => BTreeSet::new(),
                };
                let mut events = self.read_these(at)?;
                events.retain(about);
                events.extend(plan.wide.iter().cloned());
                events.extend(
                    plan.recorded
                        .iter()
                        .filter(|e| match &e.body {
                            EventBody::OutputRecorded {
                                seam, input, field, ..
                            } => {
                                outputs.contains(&(seam.clone(), input.clone()))
                                    || field.as_ref().is_some_and(|(ty, key, _)| {
                                        keys.contains(&(ty.clone(), key.clone()))
                                    })
                            }
                            _ => false,
                        })
                        .cloned(),
                );
                events.sort_by_key(|e| e.position);
                events.dedup_by_key(|e| e.position);
                let scope = Scope {
                    keys: keys.clone(),
                    types: types.clone(),
                    near: plan.near.clone(),
                    outputs,
                    head,
                    at: std::sync::atomic::AtomicU64::new(0),
                };
                *lakelands_core::lock(&self.scoped.scope) = Some(Arc::new(scope));
                w.ontology = Ontology::default().named();
                w.settings = BTreeMap::new();
                let folded = self.fold_all(w, &events, Reads::default(), false);
                *lakelands_core::lock(&self.scoped.scope) = None;
                folded?;
            }
            Ok(())
        })();
        (w.ontology, w.settings, w.owed) = (ontology, settings, owed);
        self.rebuilds.fetch_add(1, Ordering::SeqCst);
        lakelands_core::lock(&self.shapes).clear();
        self.keys.unlist();
        folded?;
        let micros = u64::try_from(began.elapsed().as_micros()).unwrap_or(u64::MAX);
        let rate = plan.events().saturating_mul(1_000_000) / micros.max(1);
        if plan.events() > 0 {
            self.scoped_rate.store(rate.max(1), Ordering::SeqCst);
        }
        // Restated: the act waits on the reclaim alone, as after a rebuild.
        let view = self.engine.view(None)?;
        let mut list = forgotten_list(view.as_ref(), &Staged::default())?;
        drop(view);
        for source in set {
            list.insert(source.clone(), "rebuilt".to_owned());
        }
        self.put_lists(&list, &BTreeMap::new(), head)?;
        self.publish(w);
        Ok(())
    }
}
