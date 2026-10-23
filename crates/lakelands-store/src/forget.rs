//! Forgetting a source (`docs/decisions/058-a-source-may-be-forgotten.md`):
//! its keys destroyed, and every value it sealed read as erased, at every
//! position.
//!
//! The act runs under the writer's lock, in this order, each step
//! idempotent:
//! 1. one append: `source_forgotten`, and a `record_retired` for each live
//!    record the source made and alone brought in, as a purge retires it;
//! 2. the source's keys are dropped from memory and destroyed;
//! 3. the projection is restated from the log, where the fold meets
//!    sealed values with no key and the act that forgot them, and erases
//!    them: the records the source wrote are refolded alone where nothing
//!    else reads them, and the whole log is folded again otherwise
//!    (`lake::scrub`), each report naming the way and why;
//! 4. the engine reclaims the storage.
//!
//! What a crash leaves after the append is finished by the next writer's
//! open, and what a failure leaves by the act's repeat, which appends
//! nothing again ([`Lake::finish_forgetting`]). Either way the fold is
//! the one every write runs, so the scoped refold comes out as the
//! rebuild does, and the conformance suite compares the two.
//!
//! The act is refused while the log holds a value of the source in an
//! erasable field in plaintext, written before the field was erasable or
//! before the log sealed values: destroying a key cannot reach it.
//!
//! Several sources are forgotten in one act ([`run_set`]): one scan of
//! the projection, the feed index read for each source and one walk of
//! the log from where it decides no more, one append of a
//! `source_forgotten` for each, one restatement and one reclaim. The unit
//! stays the source; only the act is batched, and one source is the set
//! of it alone.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::engine::ProjectionView;
use lakelands_api::error::Case;
use lakelands_api::event::{Event, EventBody};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::principal::Renamed;
use lakelands_api::record::{Origin, StoredRecord};
use lakelands_api::write::{
    Declaration, ForgetField, ForgetRecords, ForgetReport, ForgetSetReport, ForgetWay, RecordField,
    TypeRetired,
};
use lakelands_api::{
    Error, FieldName, Position, RecordKey, Result, SourceId, TypeName, WriteContext,
};

use crate::exec::Turn;
use crate::lake::{Lake, Staged, every, forgotten_list};
use crate::log::Draft;
use crate::ontology::Ontology;
use crate::purge::{brought_by, co_owned, made_by};
use crate::writes::{self, Call};

/// How many keys a report names per type.
const NAMED: usize = 10;

/// Records of one type, counted with the first keys.
fn counted(ty: &TypeName) -> ForgetRecords {
    let (ty, records, first) = (ty.clone(), 0, Vec::new());
    ForgetRecords { ty, records, first }
}

/// Count `r` into `of`.
fn count(of: &mut ForgetRecords, r: &StoredRecord) {
    of.records += 1;
    if of.first.len() < NAMED {
        of.first.push(r.key.clone());
    }
}

/// Each source's report, with whether the log holds an event of it.
type Known = Vec<(ForgetReport, bool)>;

/// What one source of a set takes: its report as it is gathered, and
/// whether the log holds an event of it.
struct Taken {
    types: Vec<ForgetRecords>,
    co_owned: Vec<ForgetRecords>,
    fields: BTreeMap<(TypeName, FieldName), u64>,
    retired: Vec<TypeRetired>,
    outputs: u64,
    plaintext: u64,
    known: bool,
}

/// What forgetting the sources of `set` takes at `view`, in one scan of
/// the projection and one walk of their events: each one's report, with
/// whether the log holds an event of it, and the live records the set
/// retires. A record the set's sources made and alone brought is retired
/// under the first of them that brought it; one another hand made, or
/// wrote too, is co-owned, for each of them that brought it.
fn found(
    lake: &Lake,
    view: &dyn ProjectionView,
    ontology: &Ontology,
    set: &[SourceId],
) -> Result<(Known, Vec<StoredRecord>)> {
    let mut taken: Vec<Taken> = (set.iter())
        .map(|_| Taken {
            types: Vec::new(),
            co_owned: Vec::new(),
            fields: BTreeMap::new(),
            retired: Vec::new(),
            outputs: 0,
            plaintext: 0,
            known: false,
        })
        .collect();
    let mut retire = Vec::new();
    let mine = |r: &StoredRecord, f: &FieldName, source: &SourceId| {
        let m = r.meta.get(f);
        m.is_some_and(|m| m.origin == Origin::Source && m.source.as_ref() == Some(source))
    };
    for (name, entry) in &ontology.types {
        let decl = &entry.decl;
        if decl.view.is_some() || !decl.field_meta {
            continue;
        }
        let erasable: Vec<&FieldName> = decl
            .fields
            .iter()
            .filter(|f| f.erasable)
            .map(|f| &f.name)
            .collect();
        let mut holding: Vec<_> = set.iter().map(|_| (counted(name), counted(name))).collect();
        let brought: Vec<StoredRecord> = (view.scan(name, &every(false))?.into_iter())
            .filter(|r| set.iter().any(|s| brought_by(r, s)))
            .collect();
        let alone: Vec<&StoredRecord> =
            brought.iter().filter(|r| !co_owned(decl, r, set)).collect();
        let made = made_by(lake, &alone, set)?;
        for r in brought {
            let by: Vec<usize> = (0..set.len())
                .filter(|&i| brought_by(&r, &set[i]))
                .collect();
            let Some(&first) = by.first() else {
                continue;
            };
            let shared = co_owned(decl, &r, set) || !made.contains(&r.made);
            for &i in &by {
                let held: Vec<&&FieldName> = (erasable.iter())
                    .filter(|f| r.fields.contains_key(**f) && mine(&r, f, &set[i]))
                    .collect();
                if !held.is_empty() {
                    count(&mut holding[i].0, &r);
                }
                for f in held {
                    *taken[i]
                        .fields
                        .entry((name.clone(), (*f).clone()))
                        .or_default() += 1;
                }
                if shared {
                    count(&mut holding[i].1, &r);
                }
            }
            if !shared {
                match taken[first].retired.last_mut() {
                    Some(t) if t.ty == r.ty => t.retired += 1,
                    _ => taken[first].retired.push(TypeRetired {
                        ty: r.ty.clone(),
                        retired: 1,
                    }),
                }
                retire.push(r);
            }
        }
        for (t, (holding, shared)) in taken.iter_mut().zip(holding) {
            t.types.extend(Some(holding).filter(|h| h.records > 0));
            t.co_owned.extend(Some(shared).filter(|s| s.records > 0));
        }
    }
    // From the log: the outputs sealed under each source's key, and its
    // values of a field erasable now that an event holds in plaintext,
    // each by the names it bears now. A type that no longer resolves says
    // nothing of what was personal, so its plaintext counts whole.
    let renames = lake.renames(Some(view))?;
    each_of(lake, view, set, |i, e| {
        let (source, taken) = (&set[i], &mut taken[i]);
        let sealed = e.sealed.as_ref();
        taken.known |= e.source.as_ref() == Some(source);
        match &e.body {
            EventBody::RecordIngested { ty, patch, .. } if e.source.as_ref() == Some(source) => {
                let sealed: &[FieldName] = sealed.map_or(&[], |s| &s.fields);
                let plain = patch
                    .iter()
                    .filter(|(f, v)| !v.is_null() && !sealed.contains(f));
                for (f, _) in plain {
                    let (ty, f) = named_now(&renames, e.position, ty, f);
                    let erasable = match ontology.ty(&ty) {
                        Ok(decl) => decl.fields.iter().any(|d| d.name == f && d.erasable),
                        Err(_) => true,
                    };
                    taken.plaintext += u64::from(erasable);
                }
            }
            EventBody::OutputRecorded {
                output: Some(_), ..
            } if sealed.and_then(|s| s.source.as_ref()) == Some(source) => taken.outputs += 1,
            _ => {}
        }
    })?;
    let events = view.position().map_or(0, |p| p.0);
    let reports = set.iter().zip(taken).map(|(source, t)| {
        let (recomputed, views) = recomputed(ontology, &t.types);
        let report = ForgetReport {
            source: source.clone(),
            types: t.types,
            fields: (t.fields.into_iter())
                .map(|((ty, field), values)| ForgetField { ty, field, values })
                .collect(),
            outputs: t.outputs,
            retired: t.retired,
            co_owned: t.co_owned,
            recomputed,
            views,
            plaintext: t.plaintext,
            events,
            expected_ms: lake.expected_ms(events),
            position: view.position().unwrap_or(Position::GENESIS),
            reclaimed: None,
            reclaim_owed: lake.reclaim_owed(),
            way: ForgetWay::default(),
            report_id: String::new(),
        };
        (report, t.known)
    });
    Ok((reports.collect(), retire))
}

/// What is computed again for a source holding values in `holding`: a
/// computed field that reads an erasable field of a type holding them,
/// and a view over one.
fn recomputed(ontology: &Ontology, holding: &[ForgetRecords]) -> (Vec<RecordField>, Vec<TypeName>) {
    let holds = |t: &TypeName| holding.iter().any(|h| &h.ty == t);
    let mut recomputed = Vec::new();
    let mut views = Vec::new();
    for (name, entry) in &ontology.types {
        let decl = &entry.decl;
        if decl.view.as_ref().is_some_and(|v| holds(&v.source)) {
            views.push(name.clone());
        }
        if !holds(name) {
            continue;
        }
        let erasable = |n: &FieldName| decl.fields.iter().any(|f| &f.name == n && f.erasable);
        for f in &decl.fields {
            let computed = f.derived.is_some() || f.function.is_some() || f.embed.is_some();
            if computed && f.depends_on.iter().any(erasable) {
                let (ty, field) = (name.clone(), f.name.clone());
                recomputed.push(RecordField { ty, field });
            }
        }
    }
    (recomputed, views)
}

/// Call `f` with the index in `set` of a source and each event of it
/// through `view`'s position, and perhaps others, in log order per
/// source: those the feed index decides, read at the positions it names,
/// then every event from where it decides no more for that source, as
/// while the projection is rebuilt, in one walk for the whole set.
pub(crate) fn each_of(
    lake: &Lake,
    view: &dyn ProjectionView,
    set: &[SourceId],
    mut f: impl FnMut(usize, &Event),
) -> Result<()> {
    let Some(head) = view.position() else {
        return Ok(());
    };
    let mut walk = Vec::with_capacity(set.len());
    for (i, source) in set.iter().enumerate() {
        let mut from = Position::GENESIS;
        while from <= head {
            let m = view.of_source(source, from, head, MATCHED)?;
            let Some(through) = m.through else { break };
            lake.read_these(&m.positions)?.iter().for_each(|e| f(i, e));
            from = through.next();
        }
        walk.push(from);
    }
    let Some(&start) = walk.iter().min() else {
        return Ok(());
    };
    if start > head {
        return Ok(());
    }
    for page in lake.pages(start) {
        for e in page? {
            if e.position > head {
                return Ok(());
            }
            for (i, from) in walk.iter().enumerate() {
                if e.position >= *from {
                    f(i, &e);
                }
            }
        }
    }
    Ok(())
}

/// How many positions of a source one read of the feed index answers.
const MATCHED: u32 = 1024;

/// The names a type and a field bear now, from those an event at `at`
/// wrote them under: each rename since, in the order of the log.
pub(crate) fn named_now(
    renames: &[Renamed],
    at: Position,
    ty: &TypeName,
    field: &FieldName,
) -> (TypeName, FieldName) {
    let (mut ty, mut field) = (ty.clone(), field.clone());
    for r in renames.iter().filter(|r| r.at > at) {
        if r.ty != ty {
            continue;
        }
        match &r.to {
            Some(to) => ty = to.clone(),
            None => {
                if let Some((_, new)) = r.renames.iter().find(|(old, _)| *old == field) {
                    field = new.clone();
                }
            }
        }
    }
    (ty, field)
}

/// What the log changed of the types up to the head of a view: each state
/// of each type's declaration with the position it holds from, the
/// renames, and the keys each re-keying migration moved, so that a value
/// an event wrote is followed to the record it is in at any later
/// position (lakelands#1228).
pub(crate) struct History {
    states: BTreeMap<TypeName, Vec<(Position, TypeDecl)>>,
    pub renames: Vec<Renamed>,
    /// By position, in the log's order: each key it moved, by the type's
    /// name and the key there.
    rekeys: Vec<(Position, Moved)>,
}

/// The keys one re-keying migration moved, by the type's name and the key
/// before.
type Moved = BTreeMap<(TypeName, RecordKey), RecordKey>;

impl History {
    /// Up to the head of `view`: the states in force and those replaced,
    /// one read for all (lakelands#1064), and the `migrated` events at the
    /// positions they hold from. An engine that keeps no replaced state
    /// gives each type's state in force for every position.
    pub(crate) fn new(lake: &Lake, view: &dyn ProjectionView) -> Result<History> {
        let head = view.position().unwrap_or(Position::GENESIS);
        let mut declared = view.declarations()?;
        declared.extend(view.replaced(Position::GENESIS, head)?.unwrap_or_default());
        let mut states: BTreeMap<TypeName, Vec<(Position, TypeDecl)>> = BTreeMap::new();
        for d in declared {
            if let Declaration::Type(t) = d.declaration {
                states.entry(t.name.clone()).or_default().push((d.since, t));
            }
        }
        states
            .values_mut()
            .for_each(|s| s.sort_by_key(|(at, _)| *at));
        let at: BTreeSet<Position> = states.values().flatten().map(|(at, _)| *at).collect();
        let rekeys = (lake.read_these(&Vec::from_iter(at))?.into_iter())
            .filter_map(|e| match e.body {
                EventBody::Migrated { rekeyed, .. } if !rekeyed.is_empty() => {
                    let moves = rekeyed.into_iter().map(|r| ((r.ty, r.from), r.to));
                    Some((e.position, moves.collect()))
                }
                _ => None,
            })
            .collect();
        let renames = lake.renames(Some(view))?;
        Ok(History {
            states,
            renames,
            rekeys,
        })
    }

    /// The declaration of the type named `ty` at `at`, or the first held.
    pub(crate) fn ty(&self, ty: &TypeName, at: Position) -> Option<&TypeDecl> {
        let states = self.states.get(ty)?;
        let then = states.iter().rev().find(|(since, _)| *since <= at);
        then.or(states.first()).map(|(_, d)| d)
    }

    /// The names at `to` of a type and a field an event at `at` wrote.
    pub(crate) fn named(
        &self,
        (at, to): (Position, Position),
        ty: &TypeName,
        field: &FieldName,
    ) -> (TypeName, FieldName) {
        let before = self.renames.partition_point(|r| r.at < to);
        named_now(&self.renames[..before], at, ty, field)
    }

    /// The type and the key at the head of the record an event at `at`
    /// wrote at `key` of `ty`: each re-key and rename since, in the order
    /// of the log, a migration's moves before the renames it records.
    pub(crate) fn moved(
        &self,
        at: Position,
        ty: &TypeName,
        key: &RecordKey,
    ) -> (TypeName, RecordKey) {
        let (mut ty, mut key) = (ty.clone(), key.clone());
        let mut renames = self.renames.iter().filter(|r| r.at > at).peekable();
        for (p, moves) in self.rekeys.iter().filter(|(p, _)| *p > at) {
            while let Some(r) = renames.next_if(|r| r.at < *p) {
                ty = renamed(r, ty);
            }
            if let Some(to) = moves.get(&(ty.clone(), key.clone())) {
                key = to.clone();
            }
        }
        (renames.fold(ty, |ty, r| renamed(r, ty)), key)
    }
}

/// `ty` after the rename `r`.
fn renamed(r: &Renamed, ty: TypeName) -> TypeName {
    match &r.to {
        Some(to) if r.ty == ty => to.clone(),
        _ => ty,
    }
}

/// The id of the preflight of forgetting the sources of `set`, read at
/// `head`; of a set of one, the id of forgetting that source.
fn id(set: &[SourceId], head: Option<Position>) -> String {
    let head = head.map_or(0, |p| p.0);
    let of = match set {
        [source] => serde_json::json!({"forget": source, "head": head}),
        set => serde_json::json!({"forget": set, "head": head}),
    };
    lakelands_core::canonical::sha256_hex(&of)
}

/// The sources `sources` names, in id order: refused when it names none,
/// or one twice.
fn set(sources: &[SourceId]) -> Result<Vec<SourceId>> {
    let mut set = sources.to_vec();
    set.sort();
    if let Some(twice) = set.windows(2).find(|w| w[0] == w[1]) {
        let source = twice[0].clone();
        return Err(Error::invalid(Case::ForgetSourceTwice { source }));
    }
    match set.is_empty() {
        true => Err(Error::invalid(Case::ForgetSetEmpty)),
        false => Ok(set),
    }
}

/// The report over a set, from each source's: the set's time, position,
/// owed reclaim and way in each, and in the set's. The time is the way's.
fn of_set(
    reports: Vec<ForgetReport>,
    view: &dyn ProjectionView,
    lake: &Lake,
    way: ForgetWay,
) -> ForgetSetReport {
    let events = view.position().map_or(0, |p| p.0);
    let mut set = ForgetSetReport {
        sources: reports,
        events,
        expected_ms: match way.scoped {
            true => way.scoped_ms,
            false => way.rebuild_ms,
        },
        way,
        position: view.position().unwrap_or(Position::GENESIS),
        reclaimed: None,
        reclaim_owed: lake.reclaim_owed(),
        report_id: String::new(),
    };
    settle(&mut set);
    set
}

/// Give each source's report the set's time, position and owed reclaim.
fn settle(set: &mut ForgetSetReport) {
    for r in &mut set.sources {
        r.expected_ms = set.expected_ms;
        r.position = set.position;
        r.reclaim_owed.clone_from(&set.reclaim_owed);
        r.way.clone_from(&set.way);
    }
}

/// The report of forgetting one source, from the set of it alone.
fn alone(set: ForgetSetReport) -> ForgetReport {
    let mut report = (set.sources.into_iter().next()).expect("a set of one has one report");
    report.reclaimed = set.reclaimed;
    report.report_id = set.report_id;
    report
}

/// How forgetting `set` at `view`'s head would restate the projection:
/// by the scoped refold or the rebuild, and why.
fn way(lake: &Lake, view: &dyn ProjectionView, set: &[SourceId]) -> Result<ForgetWay> {
    let plan = crate::lake::scrub::plan(lake, view, set, lake.limits.forget_rebuild)?;
    Ok(plan.way(lake, view.position().map_or(0, |p| p.0)))
}

/// What [`run`] would do, appending nothing, under the id the act may
/// repeat.
pub fn preflight(lake: &Lake, source: &SourceId) -> Result<ForgetReport> {
    preflight_of(lake, std::slice::from_ref(source), false).map(alone)
}

/// What [`run_set`] would do, appending nothing, under the id the act may
/// repeat: refused as the act is, for an empty set, a source named twice
/// or one the log holds no event of.
pub fn preflight_set(lake: &Lake, sources: &[SourceId]) -> Result<ForgetSetReport> {
    preflight_of(lake, &set(sources)?, true)
}

/// The preflight of forgetting `set`, a source the log holds no event of
/// refused where `known` asks for one.
fn preflight_of(lake: &Lake, set: &[SourceId], known: bool) -> Result<ForgetSetReport> {
    let began = std::time::Instant::now();
    let view = lake.engine.view(None)?;
    let ontology = Ontology::at(view.as_ref())?;
    let (found, _) = found(lake, view.as_ref(), &ontology, set)?;
    let reports = refuse_unknown(found, view.as_ref(), known)?;
    let way = way(lake, view.as_ref(), set)?;
    let mut report = of_set(reports, view.as_ref(), lake, way);
    report.report_id = id(set, view.position());
    // The act walks the log again under the lock, as this walk did, once
    // for the whole set.
    report.expected_ms = (report.expected_ms).saturating_add(crate::lake::elapsed_ms(began));
    settle(&mut report);
    Ok(report)
}

/// Each source's report, refused naming the first the log holds no event
/// of, and did not forget, where `known` asks for one.
fn refuse_unknown(
    found: Known,
    view: &dyn ProjectionView,
    known: bool,
) -> Result<Vec<ForgetReport>> {
    if known && found.iter().any(|(_, seen)| !seen) {
        let done = forgotten_list(view, &Staged::default())?;
        let unknown = |(r, seen): &&(ForgetReport, bool)| !seen && !done.contains_key(&r.source);
        if let Some((r, _)) = found.iter().find(unknown) {
            let source = r.source.clone();
            return Err(Error::invalid(Case::ForgetSourceUnknown { source }));
        }
    }
    Ok(found.into_iter().map(|(r, _)| r).collect())
}

/// Forget `source`, as the module says; with a preflight's `report_id`,
/// only if that report is of this source and the lake has not moved since.
pub fn run(lake: &Turn<'_>, source: &SourceId, report_id: Option<&str>) -> Result<ForgetReport> {
    run_of(lake, std::slice::from_ref(source), report_id, false).map(alone)
}

/// Forget the sources `sources` names in one act, as the module says of
/// one: one append of a `source_forgotten` for each with the retirements
/// of the set, one restatement and one reclaim, under one hold of the writer.
/// Refused whole, with nothing appended, for an empty set, a source named
/// twice, one the log holds no event of, one whose values the log holds
/// in plaintext, or a report of another set or head.
pub fn run_set(
    lake: &Turn<'_>,
    sources: &[SourceId],
    report_id: Option<&str>,
) -> Result<ForgetSetReport> {
    run_of(lake, &set(sources)?, report_id, true)
}

/// The act over `set`, a source the log holds no event of refused where
/// `known` asks for one.
fn run_of(
    lake: &Turn<'_>,
    set: &[SourceId],
    report_id: Option<&str>,
    known: bool,
) -> Result<ForgetSetReport> {
    let system = lake.system("lakelands");
    let call = Call {
        lake,
        who: &system,
        context: WriteContext::default(),
        followed: None,
        shown: None,
    };
    let mut w = lake.for_write()?;
    let view = lake.engine.view(None)?;
    // A source the log forgot already: the act is in the log, and its
    // repeat finishes what a failure left owed, whatever report it names,
    // or answers that it is done. Nothing is appended again for it.
    let done = forgotten_list(view.as_ref(), &Staged::default())?;
    let owed: Vec<&SourceId> = set.iter().filter(|s| !done.contains_key(*s)).collect();
    if owed.is_empty() {
        drop(view);
        let reclaimed = lake.finish_forgetting(&mut w)?;
        let view = lake.engine.view(None)?;
        let (found, _) = found(lake, view.as_ref(), &w.ontology, set)?;
        let reports = found.into_iter().map(|(r, _)| r).collect();
        let way = lake.forget_way().unwrap_or_default();
        let mut report = of_set(reports, view.as_ref(), lake, way);
        report.reclaimed = reclaimed;
        return Ok(report);
    }
    if report_id.is_some_and(|given| given != id(set, view.position())) {
        return Err(Error::invalid(Case::ForgetReportStale));
    }
    let (found, retire) = found(lake, view.as_ref(), &w.ontology, set)?;
    let reports = refuse_unknown(found, view.as_ref(), known)?;
    let owes = |r: &&ForgetReport| owed.contains(&&r.source);
    if let Some(r) = reports.iter().filter(owes).find(|r| r.plaintext > 0) {
        let (source, count) = (r.source.clone(), r.plaintext);
        return Err(Error::invalid(Case::ForgetPlaintext { source, count }));
    }
    let now = lake.clock(&mut w);
    let mut drafts = Vec::new();
    for source in owed {
        let keys = lake.keys.ids(source)?;
        let source = source.clone();
        let forgotten = EventBody::SourceForgotten { source, keys };
        drafts.push(Draft::system(now, forgotten));
    }
    // The retirements land after the `source_forgotten`s, and so does what
    // they cause.
    let mut d = call.decide(&w, view.as_ref(), now, drafts.len());
    d.retiring_as = lakelands_api::event::ChangeKind::Forgotten;
    for before in retire {
        let body = writes::retirement(&before.ty, &before.key, before.version);
        drafts.extend(d.retiring(&before, Draft::system(now, body))?);
    }
    let way = way(lake, view.as_ref(), set)?;
    let mut report = of_set(reports, view.as_ref(), lake, way);
    drop(view);
    lake.append(&mut w, drafts)?;
    report.reclaimed = lake.finish_forgetting(&mut w)?;
    // The way the act took, chosen again over the log it appended to.
    report.way = lake.forget_way().unwrap_or(report.way);
    report.reclaim_owed = lake.reclaim_owed();
    report.position = w.tip.as_ref().map_or(Position::GENESIS, |t| t.0);
    settle(&mut report);
    Ok(report)
}
