//! Forgetting a source (`docs/decisions/058-a-source-may-be-forgotten.md`):
//! its keys destroyed, and every value it sealed read as erased, at every
//! position.
//!
//! The act runs under the writer's lock, in this order, each step
//! idempotent:
//! 1. one append: `source_forgotten`, and a `record_retired` for each live
//!    record the source alone brought in, as a purge retires it;
//! 2. the source's keys are dropped from memory and destroyed;
//! 3. the projection is rebuilt from the log, where the fold meets sealed
//!    values with no key and the act that forgot them, and erases them;
//! 4. the engine reclaims the storage.
//!
//! What a crash leaves after the append is finished by the next writer's
//! open, and what a failure leaves by the act's repeat, which appends
//! nothing again ([`Lake::finish_forgetting`]). A rebuild is the
//! projection's fold of the log and the key store, so no second path has
//! to come out equal to it.
//!
//! The act is refused while the log holds a value of the source in an
//! erasable field in plaintext, written before the field was erasable or
//! before the log sealed values: destroying a key cannot reach it.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::engine::ProjectionView;
use lakelands_api::error::Case;
use lakelands_api::event::{Event, EventBody};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::principal::Renamed;
use lakelands_api::record::{Origin, StoredRecord};
use lakelands_api::write::{
    Declaration, ForgetField, ForgetRecords, ForgetReport, RecordField, TypeRetired,
};
use lakelands_api::{FieldName, Position, RecordKey, Result, SourceId, TypeName, WriteContext};
use lakelands_core::invalid;

use crate::lake::{Lake, Staged, every, forgotten_list};
use crate::log::Draft;
use crate::ontology::Ontology;
use crate::purge::{brought_by, co_owned};
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

/// What forgetting `source` takes at `view`: the report, and the live
/// records it retires.
fn found(
    lake: &Lake,
    view: &dyn ProjectionView,
    ontology: &Ontology,
    source: &SourceId,
) -> Result<(ForgetReport, Vec<StoredRecord>)> {
    let (mut types, mut co_owned_by, mut retire) = (Vec::new(), Vec::new(), Vec::new());
    let mut fields: BTreeMap<(TypeName, FieldName), u64> = BTreeMap::new();
    let mine = |r: &StoredRecord, f: &FieldName| {
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
        let (mut holding, mut shared) = (counted(name), counted(name));
        for r in view.scan(name, &every(false))? {
            if !brought_by(&r, source) {
                continue;
            }
            let held: Vec<&&FieldName> = (erasable.iter())
                .filter(|f| r.fields.contains_key(**f) && mine(&r, f))
                .collect();
            if !held.is_empty() {
                count(&mut holding, &r);
            }
            for f in held {
                *fields.entry((name.clone(), (*f).clone())).or_default() += 1;
            }
            match co_owned(decl, &r, source) {
                true => count(&mut shared, &r),
                false => retire.push(r),
            }
        }
        types.extend(Some(holding).filter(|h| h.records > 0));
        co_owned_by.extend(Some(shared).filter(|s| s.records > 0));
    }
    let mut retired: Vec<TypeRetired> = Vec::new();
    for r in &retire {
        match retired.last_mut() {
            Some(t) if t.ty == r.ty => t.retired += 1,
            _ => retired.push(TypeRetired {
                ty: r.ty.clone(),
                retired: 1,
            }),
        }
    }
    // What is computed again: a computed field that reads an erasable
    // field of a type holding the source's values, and a view over one.
    let holds = |t: &TypeName| types.iter().any(|h: &ForgetRecords| &h.ty == t);
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
    // From the log: the outputs sealed under the source's key, and its
    // values of a field erasable now that an event holds in plaintext,
    // each by the names it bears now. A type that no longer resolves says
    // nothing of what was personal, so its plaintext counts whole.
    let (mut outputs, mut plaintext) = (0, 0);
    let renames = lake.renames(Some(view))?;
    each_of(lake, view, source, |e| {
        let sealed = e.sealed.as_ref();
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
                    plaintext += u64::from(erasable);
                }
            }
            EventBody::OutputRecorded {
                output: Some(_), ..
            } if sealed.and_then(|s| s.source.as_ref()) == Some(source) => outputs += 1,
            _ => {}
        }
    })?;
    let events = view.position().map_or(0, |p| p.0);
    let report = ForgetReport {
        source: source.clone(),
        types,
        fields: (fields.into_iter())
            .map(|((ty, field), values)| ForgetField { ty, field, values })
            .collect(),
        outputs,
        retired,
        co_owned: co_owned_by,
        recomputed,
        views,
        plaintext,
        events,
        expected_ms: lake.expected_ms(events),
        position: view.position().unwrap_or(Position::GENESIS),
        reclaimed: None,
        reclaim_owed: lake.reclaim_owed(),
        report_id: String::new(),
    };
    Ok((report, retire))
}

/// Call `f` with each event of `source` through `view`'s position, and
/// perhaps others, in log order: those the feed index decides, read at the
/// positions it names, then every event from where it decides no more, as
/// while the projection is rebuilt.
pub(crate) fn each_of(
    lake: &Lake,
    view: &dyn ProjectionView,
    source: &SourceId,
    mut f: impl FnMut(&Event),
) -> Result<()> {
    let Some(head) = view.position() else {
        return Ok(());
    };
    let mut from = Position::GENESIS;
    while from <= head {
        let m = view.of_source(source, from, head, MATCHED)?;
        let Some(through) = m.through else { break };
        lake.read_these(&m.positions)?.iter().for_each(&mut f);
        from = through.next();
    }
    if from > head {
        return Ok(());
    }
    for page in lake.pages(from) {
        for e in page? {
            if e.position > head {
                return Ok(());
            }
            f(&e);
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

/// The id of the preflight of forgetting `source`, read at `head`.
fn id(source: &SourceId, head: Option<Position>) -> String {
    let of = serde_json::json!({"forget": source, "head": head.map_or(0, |p| p.0)});
    lakelands_core::canonical::sha256_hex(&of)
}

/// Whether the log forgot `source` already.
fn forgotten(view: &dyn ProjectionView, source: &SourceId) -> Result<bool> {
    Ok(forgotten_list(view, &Staged::default())?.contains_key(source))
}

/// What [`run`] would do, appending nothing, under the id the act may
/// repeat.
pub fn preflight(lake: &Lake, source: &SourceId) -> Result<ForgetReport> {
    let began = std::time::Instant::now();
    let view = lake.engine.view(None)?;
    let ontology = Ontology::at(view.as_ref())?;
    let (mut report, _) = found(lake, view.as_ref(), &ontology, source)?;
    report.report_id = id(source, view.position());
    // The act walks the log again under the lock, as this walk did.
    report.expected_ms = (report.expected_ms).saturating_add(crate::lake::elapsed_ms(began));
    Ok(report)
}

/// Forget `source`, as the module says; with a preflight's `report_id`,
/// only if that report is of this source and the lake has not moved since.
pub fn run(lake: &Lake, source: &SourceId, report_id: Option<&str>) -> Result<ForgetReport> {
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
    // or answers that it is done. Nothing is appended again.
    if forgotten(view.as_ref(), source)? {
        drop(view);
        let reclaimed = lake.finish_forgetting(&mut w)?;
        let view = lake.engine.view(None)?;
        let (mut report, _) = found(lake, view.as_ref(), &w.ontology, source)?;
        report.reclaimed = reclaimed;
        return Ok(report);
    }
    if report_id.is_some_and(|given| given != id(source, view.position())) {
        return Err(invalid(Case::ForgetReportStale));
    }
    let (mut report, retire) = found(lake, view.as_ref(), &w.ontology, source)?;
    if report.plaintext > 0 {
        let (source, count) = (source.clone(), report.plaintext);
        return Err(invalid(Case::ForgetPlaintext { source, count }));
    }
    let now = lake.clock(&mut w);
    let keys = lake.keys.ids(source)?;
    let forgotten = EventBody::SourceForgotten {
        source: source.clone(),
        keys,
    };
    let mut drafts = vec![Draft::system(now, forgotten)];
    // The retirements land after the `source_forgotten`, and so does what
    // they cause.
    let mut d = call.decide(&w, view.as_ref(), now, drafts.len());
    d.retiring_as = lakelands_api::event::ChangeKind::Forgotten;
    for before in retire {
        let body = writes::retirement(&before.ty, &before.key, before.version);
        drafts.extend(d.retiring(&before, Draft::system(now, body))?);
    }
    drop(view);
    lake.append(&mut w, drafts)?;
    report.reclaimed = lake.finish_forgetting(&mut w)?;
    report.reclaim_owed = lake.reclaim_owed();
    report.position = w.tip.as_ref().map_or(Position::GENESIS, |t| t.0);
    Ok(report)
}
