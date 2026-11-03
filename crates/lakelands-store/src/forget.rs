//! Forgetting a source (`docs/decisions/058-a-source-may-be-forgotten.md`):
//! its keys destroyed, and every value it sealed read as erased, at every
//! position.
//!
//! The act runs on the road, beside the writer
//! (`docs/decisions/099-work-beside-the-writer-runs-in-parts.md`), each
//! step idempotent:
//! 1. the walk, off the writer, gathers every record a source of the set
//!    brought, with what each of them holds of each source, and counts
//!    from the log what the report says: outputs sealed under the sources'
//!    keys and values an event holds in plaintext; a report is read at its
//!    position, and its id is that position with a digest of the events
//!    the set's sources wrote there that no act of removal selected;
//! 2. the first turn, under the writer, folds the tail the walk did not
//!    see into that gathering, refuses a source whose values the log holds
//!    in plaintext, appends a `source_forgotten` for each source not yet
//!    in the log, and destroys each such source's keys as its event lands —
//!    before any part runs, so nothing is sealed under a destroyed key
//!    between parts;
//! 3. the first part follows, and later turns append the closes and the
//!    retirements still owed, each read again as the part takes it, and
//!    `act_whole` ends them;
//! 4. the act's last turn restates the projection from the log, where the
//!    fold meets sealed values with no key and the act that forgot them,
//!    and erases them, and the engine reclaims the storage.
//!
//! What a crash leaves is taken up again by the road's owed kinds at the
//! next writer's open, which answers before they run, and the act's repeat
//! appends nothing again ([`Lake::finish_forgetting`]). Either way the
//! fold is the one every write runs, so the scoped refold comes out as the
//! rebuild does, and the conformance suite compares the two. Until the
//! act is whole its source's values read erased, and a write that reads
//! one seals nothing under its key: its key is gone.
//!
//! The act is refused while the log holds a value of a source it has not
//! forgotten yet in an erasable field in plaintext, written before the
//! field was erasable or before the log sealed values: destroying a key
//! cannot reach it. The judgement is made at the act's first turn, under
//! the ontology and the names the log bears there: a declaration or a
//! migration since the walk gathers the set again under the writer, and a
//! removal act of the set since the walk is judged again, its report's id
//! moving where it took what the id counted, and the act under a moved id
//! refused stale.
//!
//! Several sources are forgotten in one act ([`start`] with a set): one
//! scan of the projection, the feed index read for each source and one
//! walk of the log from where it decides no more, one append of a
//! `source_forgotten` for each, one restatement and one reclaim. The unit
//! stays the source; only the act is batched, and one source is the set
//! of it alone.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use lakelands_api::engine::ProjectionView;
use lakelands_api::error::Case;
use lakelands_api::event::{ChangeKind, Event, EventBody};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::principal::Renamed;
use lakelands_api::record::{Origin, StoredRecord};
use lakelands_api::write::{
    Declaration, ForgetField, ForgetRecords, ForgetReport, ForgetSetReport, ForgetWay, RecordField,
    TypeRetired,
};
use lakelands_api::{Error, FieldName, Position, RecordKey, Result, SourceId, Timestamp, TypeName};

use crate::exec::Turn;
use crate::lake::{Lake, Staged, Writer, every, forgotten_list, indexed};
use crate::log::Draft;
use crate::ontology::Ontology;
use crate::purge::{brought_by, co_owned, made_by};
use crate::remove::Removal;
use crate::road::{Job, Reach, Turned};
use crate::writes;

/// How many keys a report names per type.
const NAMED: usize = 10;

/// Records of one type, counted with the first keys.
pub(crate) fn counted(ty: &TypeName) -> ForgetRecords {
    let (ty, records, first) = (ty.clone(), 0, Vec::new());
    ForgetRecords { ty, records, first }
}

/// Count `r` into `of`.
pub(crate) fn count(of: &mut ForgetRecords, r: &StoredRecord) {
    of.records += 1;
    if of.first.len() < NAMED {
        of.first.push(r.key.clone());
    }
}

/// One source's report as it is gathered, and as the tail adds to it: the
/// records holding a value of it, the field values it counts, and what the
/// log says of the source outside the projection.
#[derive(Default)]
struct Taken {
    /// The records holding a value of the source in an erasable field.
    types: BTreeMap<TypeName, BTreeSet<RecordKey>>,
    /// The records another hand made or wrote too.
    co_owned: BTreeMap<TypeName, BTreeSet<RecordKey>>,
    /// Per type and field, the values it erases.
    fields: BTreeMap<(TypeName, FieldName), u64>,
    /// The records it retires: the source made them and alone brought them.
    retired: BTreeMap<TypeName, BTreeSet<RecordKey>>,
    outputs: u64,
    plaintext: u64,
    /// Whether the log holds an event of the source.
    known: bool,
}

impl Taken {
    fn add(&mut self, i: usize, ty: &TypeName, key: &RecordKey, g: &GatheredRecord) {
        if let Some((_, fields)) = g.held.iter().find(|(j, _)| *j == i) {
            self.types
                .entry(ty.clone())
                .or_default()
                .insert(key.clone());
            for f in fields {
                *self.fields.entry((ty.clone(), f.clone())).or_default() += 1;
            }
        }
        // A record the set leaves behind is named by each source that
        // brought it, as the act's report says what stays.
        if g.shared && g.by.contains(&i) {
            self.co_owned
                .entry(ty.clone())
                .or_default()
                .insert(key.clone());
        }
        if g.shared {
            if let Some(keys) = self.retired.get_mut(ty) {
                keys.remove(key);
            }
        } else if g.first == Some(i) {
            self.retired
                .entry(ty.clone())
                .or_default()
                .insert(key.clone());
        }
    }

    fn sub(&mut self, i: usize, ty: &TypeName, key: &RecordKey, g: &GatheredRecord) {
        if let Some((_, fields)) = g.held.iter().find(|(j, _)| *j == i) {
            if let Some(keys) = self.types.get_mut(ty) {
                keys.remove(key);
            }
            for f in fields {
                if let Some(n) = self.fields.get_mut(&(ty.clone(), f.clone())) {
                    *n = n.saturating_sub(1);
                }
            }
        }
        if g.shared
            && g.by.contains(&i)
            && let Some(keys) = self.co_owned.get_mut(ty)
        {
            keys.remove(key);
        }
        if !g.shared
            && g.first == Some(i)
            && let Some(keys) = self.retired.get_mut(ty)
        {
            keys.remove(key);
        }
    }
}

/// A record the set brought, with what each source of the set holds of it.
struct GatheredRecord {
    record: StoredRecord,
    /// Per index in the set, the erasable fields holding a value of it.
    held: Vec<(usize, Vec<FieldName>)>,
    /// The indices of the set that brought it, in set order.
    by: Vec<usize>,
    /// The first source in set order that brought it.
    first: Option<usize>,
    /// Another hand made it, or wrote a value it holds.
    shared: bool,
}

/// What forgetting the sources of `set` takes at `view`, in one scan of
/// the projection and one walk of their events. A record the set's
/// sources made and alone brought is retired under the first of them that
/// brought it; one another hand made, or wrote too, is co-owned, for each
/// of them that brought it.
struct Gathered {
    records: BTreeMap<(TypeName, RecordKey), GatheredRecord>,
    taken: Vec<Taken>,
    /// The events of the sources at the view that no act of removal
    /// selects, where a report's digest was asked for.
    seen: BTreeSet<Position>,
}

fn gather(
    lake: &Lake,
    view: &dyn ProjectionView,
    ontology: &Ontology,
    set: &[SourceId],
    digest: Option<&[Removal]>,
) -> Result<Gathered> {
    let mut records: BTreeMap<(TypeName, RecordKey), GatheredRecord> = BTreeMap::new();
    for (name, entry) in &ontology.types {
        let decl = &entry.decl;
        if decl.view.is_some() || !decl.field_meta {
            continue;
        }
        let brought: Vec<StoredRecord> = (view.scan(name, &every(false))?.into_iter())
            .filter(|r| set.iter().any(|s| brought_by(r, s)))
            .collect();
        let alone: Vec<&StoredRecord> =
            brought.iter().filter(|r| !co_owned(decl, r, set)).collect();
        let made = made_by(lake, &alone, set)?;
        for r in brought {
            let held = held_by(decl, &r, set);
            let by: Vec<usize> = (0..set.len())
                .filter(|&i| brought_by(&r, &set[i]))
                .collect();
            let first = by.first().copied();
            let shared = co_owned(decl, &r, set) || !made.contains(&r.made);
            let key = (r.ty.clone(), r.key.clone());
            records.insert(
                key,
                GatheredRecord {
                    record: r,
                    held,
                    by,
                    first,
                    shared,
                },
            );
        }
    }
    let mut taken: Vec<Taken> = (0..set.len()).map(|_| Taken::default()).collect();
    for ((ty, key), g) in &records {
        for (i, t) in taken.iter_mut().enumerate() {
            t.add(i, ty, key, g);
        }
    }
    // From the log: the outputs sealed under each source's key, and its
    // values of a field erasable now that an event holds in plaintext,
    // each by the names it bears now. A type that no longer resolves says
    // nothing of what was personal, so its plaintext counts whole.
    let renames = lake.renames(Some(view))?;
    let mut seen = BTreeSet::new();
    each_of(lake, view, set, |i, e| {
        let source = &set[i];
        if let Some(earlier) = digest
            && indexed(e).as_ref() == Some(source)
            && !earlier.iter().any(|r| r.selects(e))
        {
            seen.insert(e.position);
        }
        taken[i].known |= e.source.as_ref() == Some(source);
        match &e.body {
            EventBody::RecordIngested { ty, patch, .. } if e.source.as_ref() == Some(source) => {
                let sealed: &[FieldName] = e.sealed.as_ref().map_or(&[], |s| &s.fields);
                for (f, v) in patch {
                    if v.is_null() || sealed.contains(f) {
                        continue;
                    }
                    let (ty, f) = named_now(&renames, e.position, ty, f);
                    let erasable = match ontology.ty(&ty) {
                        Ok(decl) => decl.fields.iter().any(|d| d.name == f && d.erasable),
                        Err(_) => true,
                    };
                    taken[i].plaintext += u64::from(erasable);
                }
            }
            EventBody::OutputRecorded {
                output: Some(_), ..
            } if e.sealed.as_ref().and_then(|s| s.source.as_ref()) == Some(source) => {
                taken[i].outputs += 1;
            }
            _ => {}
        }
    })?;
    Ok(Gathered {
        records,
        taken,
        seen,
    })
}

/// The erasable fields of `r` each source of `set` holds a value of.
fn held_by(decl: &TypeDecl, r: &StoredRecord, set: &[SourceId]) -> Vec<(usize, Vec<FieldName>)> {
    let erasable: Vec<&FieldName> = decl
        .fields
        .iter()
        .filter(|f| f.erasable)
        .map(|f| &f.name)
        .collect();
    let mut held = Vec::new();
    for (i, source) in set.iter().enumerate() {
        let fields: Vec<FieldName> = (erasable.iter())
            .filter(|f| {
                r.fields.contains_key(**f)
                    && r.meta.get(**f).is_some_and(|m| {
                        m.origin == Origin::Source && m.source.as_ref() == Some(source)
                    })
            })
            .map(|f| (*f).clone())
            .collect();
        if !fields.is_empty() {
            held.push((i, fields));
        }
    }
    held
}

/// The erasable fields of every type in `ontology`, by type.
fn erasable_fields(ontology: &Ontology) -> BTreeMap<TypeName, BTreeSet<FieldName>> {
    (ontology.types.iter())
        .map(|(name, entry)| {
            let fields = (entry.decl.fields.iter())
                .filter(|f| f.erasable)
                .map(|f| f.name.clone())
                .collect();
            (name.clone(), fields)
        })
        .collect()
}

/// One source's report from what was gathered of it.
fn report_of(
    lake: &Lake,
    view: &dyn ProjectionView,
    ontology: &Ontology,
    source: &SourceId,
    t: &Taken,
) -> Result<ForgetReport> {
    let types: Vec<ForgetRecords> = (t.types.iter())
        .filter(|(_, keys)| !keys.is_empty())
        .map(|(ty, keys)| ForgetRecords {
            ty: ty.clone(),
            records: keys.len() as u64,
            first: keys.iter().take(NAMED).cloned().collect(),
        })
        .collect();
    let fields: Vec<ForgetField> = (t.fields.iter())
        .filter(|(_, n)| **n > 0)
        .map(|((ty, field), values)| ForgetField {
            ty: ty.clone(),
            field: field.clone(),
            values: *values,
        })
        .collect();
    let co_owned: Vec<ForgetRecords> = (t.co_owned.iter())
        .filter(|(_, keys)| !keys.is_empty())
        .map(|(ty, keys)| ForgetRecords {
            ty: ty.clone(),
            records: keys.len() as u64,
            first: keys.iter().take(NAMED).cloned().collect(),
        })
        .collect();
    let retired: Vec<TypeRetired> = (t.retired.iter())
        .filter(|(_, keys)| !keys.is_empty())
        .map(|(ty, keys)| TypeRetired {
            ty: ty.clone(),
            retired: keys.len() as u32,
        })
        .collect();
    let (recomputed, views) = recomputed(ontology, &types);
    // Its open quarantined records, every one of which the act closes
    // (085).
    let (_, quarantined) = quarantined(view, source)?;
    let events = view.position().map_or(0, |p| p.0);
    Ok(ForgetReport {
        source: source.clone(),
        types,
        fields,
        outputs: t.outputs,
        retired,
        co_owned,
        recomputed,
        views,
        plaintext: t.plaintext,
        events,
        expected_ms: lake.expected_ms(events),
        reclaim_untimed: lake.untimed(),
        position: view.position().unwrap_or(Position::GENESIS),
        reclaimed: None,
        reclaim_owed: lake.reclaim_owed(),
        way: ForgetWay::default(),
        report_id: String::new(),
        quarantined,
    })
}

/// The closes of `source`'s open quarantined records, every one, and
/// their count per type.
fn quarantined(
    view: &dyn ProjectionView,
    source: &SourceId,
) -> Result<(
    Vec<EventBody>,
    Vec<lakelands_api::quarantine::TypeQuarantined>,
)> {
    let why = lakelands_api::quarantine::ClosedWhy::Forgotten;
    crate::quarantine::act_closes(view, (source, None), why, &mut |_| Ok(true))
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
    rekeys: Vec<(Position, Moved)>,
}

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

    pub(crate) fn ty(&self, ty: &TypeName, at: Position) -> Option<&TypeDecl> {
        let states = self.states.get(ty)?;
        let then = states.iter().rev().find(|(since, _)| *since <= at);
        then.or(states.first()).map(|(_, d)| d)
    }

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

fn renamed(r: &Renamed, ty: TypeName) -> TypeName {
    match &r.to {
        Some(to) if r.ty == ty => to.clone(),
        _ => ty,
    }
}

/// An act of forgetting the log holds, as the `source_forgotten` events
/// until its `act_whole` name it: from its first, every source it forgot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Act {
    /// Its first `source_forgotten`, which its `act_whole` names.
    pub at: Position,
    pub sources: Vec<SourceId>,
    /// Whether its `act_whole` is in the log.
    pub whole: bool,
}

/// The acts of forgetting the log holds, in order, each with the sources
/// it forgot: its `source_forgotten` events lie between the previous
/// act's `act_whole` and its own, since a job's turns append nothing of
/// another act's between them.
pub(crate) fn acts(lake: &Lake, view: &dyn ProjectionView) -> Result<Vec<Act>> {
    let Some(head) = view.position() else {
        return Ok(Vec::new());
    };
    let filter = lakelands_api::seam::FeedFilter {
        kinds: vec!["source_forgotten".into(), "act_whole".into()],
        ..Default::default()
    };
    let mut at = BTreeSet::new();
    let mut from = Position::GENESIS;
    loop {
        let m = view.matching(&filter, from, head, MATCHED)?;
        at.extend(m.positions);
        match m.through {
            Some(t) if t < head => from = t.next(),
            Some(_) => break,
            // The index decides nothing, as while a rebuild refolds: read
            // the log.
            None => {
                let mut found = Vec::new();
                for page in lake.pages(Position::GENESIS) {
                    for e in page? {
                        if e.position > head {
                            return Ok(grouped(found));
                        }
                        if matches!(
                            e.body,
                            EventBody::SourceForgotten { .. } | EventBody::ActWhole { .. }
                        ) {
                            found.push(e);
                        }
                    }
                }
                return Ok(grouped(found));
            }
        }
    }
    Ok(grouped(lake.read_these(&Vec::from_iter(at))?))
}

fn grouped(events: Vec<Event>) -> Vec<Act> {
    let mut acts: Vec<Act> = Vec::new();
    for e in events {
        match &e.body {
            EventBody::SourceForgotten { source, .. } => match acts.last_mut() {
                Some(act) if !act.whole => act.sources.push(source.clone()),
                _ => acts.push(Act {
                    at: e.position,
                    sources: vec![source.clone()],
                    whole: false,
                }),
            },
            EventBody::ActWhole { act } => {
                if let Some(open) = acts.last_mut().filter(|a| !a.whole && a.at == *act) {
                    open.whole = true;
                }
            }
            _ => {}
        }
    }
    acts
}

/// The events of the sources of `set` at `view`'s position that no act of
/// `earlier` selects: what a report's digest is over.
fn selection_events(
    lake: &Lake,
    view: &dyn ProjectionView,
    set: &[SourceId],
    earlier: &[Removal],
) -> Result<BTreeSet<Position>> {
    let mut seen = BTreeSet::new();
    each_of(lake, view, set, |i, e| {
        if indexed(e).as_ref() == Some(&set[i]) && !earlier.iter().any(|r| r.selects(e)) {
            seen.insert(e.position);
        }
    })?;
    Ok(seen)
}

/// The digest of a report over the sources of `set`, the position it was
/// read at, and the events of theirs it selected there that no earlier
/// act selects.
fn digest_of(set: &[SourceId], through: Position, seen: &BTreeSet<Position>) -> String {
    let events: Vec<u64> = seen.iter().map(|p| p.0).collect();
    let of = serde_json::json!({"forget": set, "through": through.0, "events": events});
    lakelands_core::canonical::sha256_hex(&of)
}

/// The id of a report: the position it was read at, then the digest of
/// what it selected there (099).
fn token(set: &[SourceId], through: Position, seen: &BTreeSet<Position>) -> String {
    format!("{}-{}", through.0, digest_of(set, through, seen))
}

/// The window a `report_id` names; a string that is not one is a report of
/// no forgetting this lake reads.
fn window_of(id: &str) -> Result<Window> {
    let stale = || Error::invalid(Case::ForgetReportStale);
    let (through, digest) = id.split_once('-').ok_or_else(stale)?;
    let through: u64 = through.parse().map_err(|_| stale())?;
    Ok(Window::Report {
        through: Position(through),
        digest: digest.to_owned(),
    })
}

/// The sources `sources` names, in id order: refused when it names none,
/// or one twice.
pub(crate) fn set(sources: &[SourceId]) -> Result<Vec<SourceId>> {
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
        reclaim_untimed: lake.untimed(),
        way,
        position: view.position().unwrap_or(Position::GENESIS),
        reclaimed: None,
        reclaim_owed: lake.reclaim_owed(),
        report_id: String::new(),
    };
    settle(&mut set);
    set
}

fn settle(set: &mut ForgetSetReport) {
    for r in &mut set.sources {
        r.expected_ms = set.expected_ms;
        r.reclaim_untimed = set.reclaim_untimed;
        r.position = set.position;
        r.reclaim_owed.clone_from(&set.reclaim_owed);
        r.way.clone_from(&set.way);
    }
}

pub(crate) fn alone(mut set: ForgetSetReport) -> ForgetReport {
    settle(&mut set);
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

/// What the act would do, appending nothing, under the id it may repeat.
pub fn preflight(lake: &Lake, source: &SourceId) -> Result<ForgetReport> {
    preflight_of(lake, std::slice::from_ref(source), false).map(alone)
}

/// What the act over a set would do, appending nothing, under the id it
/// may repeat: refused as the act is, for an empty set, a source named
/// twice or one the log holds no event of.
pub fn preflight_set(lake: &Lake, sources: &[SourceId]) -> Result<ForgetSetReport> {
    preflight_of(lake, &set(sources)?, true)
}

fn preflight_of(lake: &Lake, set: &[SourceId], known: bool) -> Result<ForgetSetReport> {
    let began = std::time::Instant::now();
    let view = lake.engine.view(None)?;
    let ontology = Ontology::at(view.as_ref())?;
    let earlier = crate::remove::acts(lake, view.as_ref())?;
    let g = gather(lake, view.as_ref(), &ontology, set, Some(&earlier))?;
    let reports = refuse_unknown(g.taken, set, view.as_ref(), &ontology, lake, known)?;
    let way = way(lake, view.as_ref(), set)?;
    let mut report = of_set(reports, view.as_ref(), lake, way);
    let through = lake
        .report_head(view.position())?
        .unwrap_or(Position::GENESIS);
    report.report_id = token(set, through, &g.seen);
    report.expected_ms = (report.expected_ms).saturating_add(crate::lake::elapsed_ms(began));
    settle(&mut report);
    Ok(report)
}

/// Each source's report, refused naming the first not in the log's
/// forgotten list that it holds no event of, where `known` asks for one.
fn refuse_unknown(
    taken: Vec<Taken>,
    set: &[SourceId],
    view: &dyn ProjectionView,
    ontology: &Ontology,
    lake: &Lake,
    known: bool,
) -> Result<Vec<ForgetReport>> {
    let done = forgotten_list(view, &Staged::default())?;
    let unknown = (set.iter().enumerate())
        .find(|(i, source)| known && !taken[*i].known && !done.contains_key(*source));
    if let Some((_, source)) = unknown {
        let source = source.clone();
        return Err(Error::invalid(Case::ForgetSourceUnknown { source }));
    }
    (set.iter().zip(&taken))
        .map(|(source, t)| report_of(lake, view, ontology, source, t))
        .collect()
}

/// What a forget answers its caller: the report of the act over its set.
pub(crate) type Reported = Result<ForgetSetReport>;

/// What a turn leaves for the restate: nothing to append, another part,
/// or an `act_whole` whose restate and reclaim run with the view let go.
enum Next {
    Nothing,
    More,
    Whole,
}

/// How a report names the selection it read.
enum Window {
    /// No report: everything the set selects.
    Whole,
    /// A report read at `through`, whose digest is of what it selected
    /// there.
    Report { through: Position, digest: String },
    /// An act the log began, taken up again.
    Act,
}

/// One forgetting on the road: the act over `set`, in parts of at most
/// `part_events` (099).
pub(crate) struct Forget {
    set: Vec<SourceId>,
    /// Whether a source the log holds no event of is refused.
    known: bool,
    report_id: Option<String>,
    window: Window,
    /// The sources whose `source_forgotten` the act still owes.
    pending: Vec<SourceId>,
    /// The ontology at the walk, whole: the first turn gathers again
    /// whenever the writer's differs, so that what is erasable, what a
    /// field's first `initial` makes co-owned, and what a record recomputes
    /// are the act's judgement and not the walk's (058).
    shape: Option<Ontology>,
    /// The erasable fields of the ontology at the walk, by type, for the
    /// tail's plaintext count.
    erasable: BTreeMap<TypeName, BTreeSet<FieldName>>,
    /// The renames at the walk, for the tail's plaintext count.
    renames: Vec<Renamed>,
    /// The records the set brought at the walk, with what each contributes.
    records: BTreeMap<(TypeName, RecordKey), GatheredRecord>,
    /// The report's counts per source of the set.
    taken: Vec<Taken>,
    /// The records still to retire, in type then key order.
    retire: BTreeMap<(TypeName, RecordKey), StoredRecord>,
    /// The closes the act owes, in the order the walk found them.
    closes: VecDeque<EventBody>,
    /// The keys the tail moved past the walk, read again under the writer.
    moved: BTreeSet<(TypeName, RecordKey)>,
    /// The acts of removal the walk knew.
    walked: BTreeSet<Position>,
    /// The position of the act's first event; `None` until it is in the log.
    act: Option<Position>,
    /// Whether the act's first turn ran.
    first: bool,
    /// How the act would restate the projection.
    way: ForgetWay,
    report: Option<ForgetSetReport>,
    /// The last position the act wrote.
    position: Position,
    answer: Option<tokio::sync::oneshot::Sender<Reported>>,
}

/// The forgettings the log leaves owed: every act whose `act_whole` is not
/// in the log, taken up again by the next writer's open.
pub(crate) fn owed(lake: &Lake, view: &dyn ProjectionView) -> Result<Vec<Box<dyn Job>>> {
    let mut owing: Vec<Box<dyn Job>> = Vec::new();
    for act in acts(lake, view)? {
        if act.whole {
            continue;
        }
        owing.push(Box::new(Forget {
            set: act.sources,
            known: false,
            report_id: None,
            window: Window::Act,
            pending: Vec::new(),
            shape: None,
            erasable: BTreeMap::new(),
            renames: Vec::new(),
            records: BTreeMap::new(),
            taken: Vec::new(),
            retire: BTreeMap::new(),
            closes: VecDeque::new(),
            moved: BTreeSet::new(),
            walked: BTreeSet::new(),
            act: Some(act.at),
            first: false,
            way: ForgetWay::default(),
            report: None,
            position: act.at,
            answer: None,
        }));
    }
    Ok(owing)
}

impl Job for Forget {
    /// What of the tail reaches the act: its sources' writes, and every
    /// write about a record one of them brought, which another hand's
    /// write on one of them is.
    fn reach(&self) -> Reach {
        let types: Vec<TypeName> = (self.records.keys()).map(|(t, _)| t.clone()).collect();
        let keys = (self.records.keys()).map(|(_, k)| k.clone()).collect();
        Reach {
            sources: self.set.clone(),
            types: types.into_iter().collect(),
            keys,
        }
    }

    /// The walk, off the writer: gather every record the set brought and
    /// what the log says of its sources, take up an act the log already
    /// began for one of them, and read a report's id at its position.
    fn walk(&mut self, lake: &Lake, view: &dyn ProjectionView) -> Result<()> {
        let head = view.position();
        // An act the log already began for a source of the set is taken up
        // again, whatever report the caller names, and its sources join
        // the set: a repeat appends no second event for them.
        if self.act.is_none()
            && let Some(open) = (acts(lake, view)?)
                .into_iter()
                .find(|a| !a.whole && a.sources.iter().any(|s| self.set.contains(s)))
        {
            self.window = Window::Act;
            self.act = Some(open.at);
            let mut set = open.sources;
            set.extend(self.set.iter().cloned());
            set.sort();
            set.dedup();
            self.set = set;
        }
        if self.act.is_none()
            && let Some(id) = &self.report_id
        {
            let window = window_of(id)?;
            if let Window::Report { through, digest } = &window {
                if *through > head.unwrap_or(Position::GENESIS) {
                    return Err(Error::invalid(Case::ForgetReportStale));
                }
                let past = lake.engine.view(Some(*through))?;
                // A removal act written after the report selects what it
                // counted, wherever it came: what is left moves (099).
                let earlier = crate::remove::acts(lake, view)?;
                let seen = selection_events(lake, past.as_ref(), &self.set, &earlier)?;
                if digest_of(&self.set, *through, &seen) != *digest {
                    return Err(Error::invalid(Case::ForgetReportStale));
                }
                self.window = window;
            }
        }
        self.rewalk(lake, view)?;
        // A source of the set the log holds no event of, not forgotten
        // already, is refused rather than forgotten.
        if self.known
            && let Some((_, source)) = (self.set.iter().enumerate())
                .find(|(i, s)| self.pending.contains(*s) && !self.taken[*i].known)
        {
            let source = source.clone();
            return Err(Error::invalid(Case::ForgetSourceUnknown { source }));
        }
        Ok(())
    }

    /// The tail past the walk's view: what it says of the sources without
    /// reading a record, and the keys it moved, which the first turn reads
    /// again under the writer.
    fn fold(&mut self, tail: &[Event]) -> Result<()> {
        for e in tail {
            for about in lakelands_core::feed::about(&e.body) {
                if let (Some(ty), Some(key)) = (about.ty, about.key) {
                    self.moved.insert((ty, key));
                }
            }
            let Some(source) = indexed(e).filter(|s| self.set.contains(s)) else {
                continue;
            };
            let i = self
                .set
                .iter()
                .position(|s| s == &source)
                .expect("the source is of the set");
            let sealed = e.sealed.as_ref();
            self.taken[i].known |= e.source.as_ref() == Some(&source);
            match &e.body {
                EventBody::RecordIngested { ty, patch, .. }
                    if e.source.as_ref() == Some(&source) =>
                {
                    let sealed: &[FieldName] = sealed.map_or(&[], |s| &s.fields);
                    for (f, v) in patch {
                        if v.is_null() || sealed.contains(f) {
                            continue;
                        }
                        let (ty, f) = named_now(&self.renames, e.position, ty, f);
                        let erasable = self
                            .erasable
                            .get(&ty)
                            .is_none_or(|fields| fields.contains(&f));
                        self.taken[i].plaintext += u64::from(erasable);
                    }
                }
                EventBody::OutputRecorded {
                    output: Some(_), ..
                } if sealed.and_then(|s| s.source.as_ref()) == Some(&source) => {
                    self.taken[i].outputs += 1;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn turn(&mut self, t: &Turn<'_>, w: &mut Writer) -> Result<Turned> {
        self.one_turn(t, w)
    }

    fn end(&mut self, ended: Result<()>) {
        let Some(answer) = self.answer.take() else {
            return;
        };
        let answered = ended.map(|()| self.report.take().expect("a whole forget has its report"));
        let _ = answer.send(answered);
    }
}

impl Forget {
    /// Gather the set at `view`, and hold the report it makes.
    fn rewalk(&mut self, lake: &Lake, view: &dyn ProjectionView) -> Result<()> {
        let ontology = Ontology::at(view)?;
        let list = forgotten_list(view, &Staged::default())?;
        self.pending = (self.set.iter())
            .filter(|s| !list.contains_key(*s))
            .cloned()
            .collect();
        let g = gather(lake, view, &ontology, &self.set, None)?;
        self.records = g.records;
        self.taken = g.taken;
        self.retire = (self.records.iter())
            .filter(|(_, r)| !r.shared)
            .map(|(k, r)| (k.clone(), r.record.clone()))
            .collect();
        self.erasable = erasable_fields(&ontology);
        self.renames = lake.renames(Some(view))?;
        self.walked = (crate::remove::removed_list(view, &Staged::default())?)
            .into_keys()
            .collect();
        self.way = way(lake, view, &self.set)?;
        self.report = Some(self.report_again(lake, view, &ontology)?);
        self.shape = Some(ontology);
        Ok(())
    }

    /// The report as the gathering and the tail leave it.
    fn report_again(
        &self,
        lake: &Lake,
        view: &dyn ProjectionView,
        ontology: &Ontology,
    ) -> Result<ForgetSetReport> {
        let reports: Vec<ForgetReport> = (self.set.iter().zip(&self.taken))
            .map(|(source, t)| report_of(lake, view, ontology, source, t))
            .collect::<Result<_>>()?;
        Ok(of_set(reports, view, lake, self.way.clone()))
    }

    /// Take up an act of the set the writer's catch-up left that the walk
    /// did not see: its sources join the set, and it is gathered again.
    fn adopt(&mut self, lake: &Lake, view: &dyn ProjectionView, at: Position) -> Result<()> {
        let Some(open) = (acts(lake, view)?).into_iter().find(|a| a.at == at) else {
            return Ok(());
        };
        let mut set = open.sources;
        set.extend(self.set.iter().cloned());
        set.sort();
        set.dedup();
        self.set = set;
        self.window = Window::Act;
        self.act = Some(at);
        self.first = false;
        self.rewalk(lake, view)
    }

    fn one_turn(&mut self, t: &Turn<'_>, w: &mut Writer) -> Result<Turned> {
        let lake: &Lake = t;
        let view = lake.engine.view(None)?;
        let logged = acts(lake, view.as_ref())?;
        // The writer's catch-up may have left the projection holding an act
        // the walk off it did not see: take that act up before beginning
        // another.
        if self.act.is_none()
            && let Some(at) = (logged.iter())
                .find(|a| !a.whole && a.sources.iter().any(|s| self.set.contains(s)))
                .map(|a| a.at)
        {
            self.adopt(lake, view.as_ref(), at)?;
        }
        if let Some(at) = self.act
            && logged.iter().any(|a| a.at == at && a.whole)
        {
            // The appends ended in a turn this handle did not run: the
            // restate and the reclaim are left, answered as owed through
            // the head the act's appends reached (099).
            let through = view.position().unwrap_or(Position::GENESIS);
            drop(view);
            let reclaimed = lake.finish_act(w, through)?;
            if self.answer.is_some() {
                let view = lake.engine.view(None)?;
                let ontology = Ontology::at(view.as_ref())?;
                if self.report.is_none() {
                    self.report = Some(self.report_again(lake, view.as_ref(), &ontology)?);
                }
                drop(view);
                self.finish_report(lake, reclaimed);
            }
            return Ok(Turned::Whole);
        }
        let next = match self.act {
            None => self.begin(lake, w, view.as_ref())?,
            Some(_) if !self.first => self.begin(lake, w, view.as_ref())?,
            Some(_) => self.part_turn(lake, w, view.as_ref())?,
        };
        // The restate and the reclaim rebuild the projection: the view this
        // turn held is let go first.
        drop(view);
        match next {
            Next::More => Ok(Turned::More),
            Next::Nothing | Next::Whole => {
                let reclaimed = lake.finish_act(w, self.position)?;
                self.finish_report(lake, reclaimed);
                Ok(Turned::Whole)
            }
        }
    }

    /// The act's first turn: the tail folded into the gathering, judged
    /// again under the ontology and the names the log bears now, the
    /// closes set, the keys the log already forgot destroyed, and the
    /// sources not yet in the log forgotten — each event's keys destroyed
    /// as it lands, before any part runs.
    fn begin(&mut self, lake: &Lake, w: &mut Writer, view: &dyn ProjectionView) -> Result<Next> {
        let ontology = Ontology::at(view)?;
        let renames = lake.renames(Some(view))?;
        // A removal act of the set written since the walk takes what the
        // report's id named when it was read: the id is judged again, and
        // the act under one it no longer names is refused (099).
        self.window_again(lake, view)?;
        // A declaration or a migration since the walk changes the
        // ontology the set is gathered under — what is erasable, what is
        // co-owned, what a record recomputes — and the names the log
        // bears. Then the walk's gathering is not the act's judgement: the
        // set is gathered again here, under the writer, at the ontology
        // now (058).
        if self.shape.as_ref() != Some(&ontology) || renames != self.renames {
            self.rewalk(lake, view)?;
            self.moved.clear();
        }
        let at = self.act.unwrap_or_else(|| writes::position_after(w, 0));
        let list = forgotten_list(view, &Staged::default())?;
        self.pending = (self.set.iter())
            .filter(|s| !list.contains_key(*s))
            .cloned()
            .collect();
        // A source whose `source_forgotten` is in the log, from this act or
        // a crash inside it, seals nothing from now: its keys go first.
        for source in self.set.iter().filter(|s| list.contains_key(*s)) {
            lake.keys.destroy(source)?;
        }
        self.see_the_tail(lake, view, &ontology)?;
        self.refuse_plaintext()?;
        self.closes = self.all_closes(view, &list)?;
        self.report = Some(self.report_again(lake, view, &ontology)?);
        self.first = true;
        if self.act.is_none() && self.pending.is_empty() {
            return Ok(Next::Nothing);
        }
        if self.pending.is_empty() {
            return self.come_parts(lake, w, view);
        }
        self.act = Some(at);
        self.forget_more(lake, w, view)
    }

    /// The report's id judged again under the writer: a removal act of the
    /// set written since the walk takes what the id named when it was read,
    /// so the id over what is left moves, and the act under it is refused
    /// (099).
    fn window_again(&self, lake: &Lake, view: &dyn ProjectionView) -> Result<()> {
        let Window::Report { through, digest } = &self.window else {
            return Ok(());
        };
        let all = crate::remove::acts(lake, view)?;
        if all.iter().all(|r| self.walked.contains(&r.at)) {
            return Ok(());
        }
        let past = lake.engine.view(Some(*through))?;
        let seen = selection_events(lake, past.as_ref(), &self.set, &all)?;
        if digest_of(&self.set, *through, &seen) != *digest {
            return Err(Error::invalid(Case::ForgetReportStale));
        }
        Ok(())
    }

    /// A turn of the first phase: a `source_forgotten` for as many of the
    /// sources not yet in the log as fit, each with its keys destroyed
    /// after the append. With all of them in, the first part follows in
    /// the same turn.
    fn forget_more(
        &mut self,
        lake: &Lake,
        w: &mut Writer,
        view: &dyn ProjectionView,
    ) -> Result<Next> {
        let now = lake.clock(w);
        let room = room(lake);
        let take = room.min(self.pending.len());
        let mut drafts = Vec::new();
        let mut taking = Vec::with_capacity(take);
        for source in &self.pending[..take] {
            let keys = lake.keys.ids(source)?;
            drafts.push(Draft::system(
                now,
                EventBody::SourceForgotten {
                    source: source.clone(),
                    keys,
                },
            ));
            taking.push(source.clone());
        }
        lake.append(&mut *w, drafts)?;
        self.pending.drain(..take);
        // The keys go before any part runs, so nothing a write seals after
        // this turn is sealed under one (099).
        for source in &taking {
            lake.keys.destroy(source)?;
        }
        self.position = w.tip.as_ref().map_or(Position::GENESIS, |t| t.0);
        if !self.pending.is_empty() {
            return Ok(Next::More);
        }
        self.come_parts(lake, w, view)
    }

    /// The closes and the retirements still owed, one part, and
    /// `act_whole` where nothing is left.
    fn come_parts(
        &mut self,
        lake: &Lake,
        w: &mut Writer,
        view: &dyn ProjectionView,
    ) -> Result<Next> {
        let now = lake.clock(w);
        let room = room(lake);
        let (part, whole) = self.fill(lake, (w, view), now, room, Vec::new())?;
        lake.append(&mut *w, part)?;
        self.position = w.tip.as_ref().map_or(Position::GENESIS, |t| t.0);
        if let Some(report) = self.report.as_mut() {
            report.position = report.position.max(self.position);
        }
        Ok(if whole { Next::Whole } else { Next::More })
    }

    fn part_turn(
        &mut self,
        lake: &Lake,
        w: &mut Writer,
        view: &dyn ProjectionView,
    ) -> Result<Next> {
        if !self.pending.is_empty() {
            return self.forget_more(lake, w, view);
        }
        self.come_parts(lake, w, view)
    }

    /// One part of the act, by the rule a purge's and a removal's parts
    /// share with it ([`crate::parts::part`]).
    fn fill(
        &mut self,
        lake: &Lake,
        (w, view): (&Writer, &dyn ProjectionView),
        now: Timestamp,
        room: usize,
        drafts: Vec<Draft>,
    ) -> Result<(Vec<Draft>, bool)> {
        let act = self.act.expect("a part follows the act's beginning");
        crate::parts::part(
            lake,
            (w, view),
            now,
            room,
            drafts,
            crate::parts::Part {
                closes: &mut self.closes,
                taken: &mut self.retire,
                kind: ChangeKind::Forgotten,
                act,
            },
        )
    }

    /// Read the keys the tail moved again, as they stand under the writer,
    /// and take their contribution out of the gathering and back in as
    /// they are now: one the source wrote since is taken in, one another
    /// hand wrote drops out, and one made since is retired with the rest.
    fn see_the_tail(
        &mut self,
        lake: &Lake,
        view: &dyn ProjectionView,
        ontology: &Ontology,
    ) -> Result<()> {
        let moved = std::mem::take(&mut self.moved);
        for (ty, key) in moved {
            let id = (ty.clone(), key.clone());
            if let Some(old) = self.records.remove(&id) {
                for i in 0..self.set.len() {
                    self.taken[i].sub(i, &ty, &key, &old);
                }
                self.retire.remove(&id);
            }
            let Some(r) = view.get(&ty, &key)?.filter(|r| !r.retired) else {
                continue;
            };
            let Ok(decl) = ontology.ty(&ty) else { continue };
            let Some(first) = (0..self.set.len()).find(|&i| brought_by(&r, &self.set[i])) else {
                continue;
            };
            let held = held_by(decl, &r, &self.set);
            let by: Vec<usize> = (0..self.set.len())
                .filter(|&i| brought_by(&r, &self.set[i]))
                .collect();
            let shared = if co_owned(decl, &r, &self.set) {
                true
            } else {
                let made = made_by(lake, std::slice::from_ref(&&r), &self.set)?;
                !made.contains(&r.made)
            };
            let g = GatheredRecord {
                record: r,
                held,
                by,
                first: Some(first),
                shared,
            };
            for i in 0..self.set.len() {
                self.taken[i].add(i, &ty, &key, &g);
            }
            if !g.shared {
                self.retire.insert(id.clone(), g.record.clone());
            }
            self.records.insert(id, g);
        }
        Ok(())
    }

    /// Refuse a source the act has not forgotten yet whose values the log
    /// holds in plaintext.
    fn refuse_plaintext(&self) -> Result<()> {
        for (i, source) in self.set.iter().enumerate() {
            let count = self.taken[i].plaintext;
            if self.pending.contains(source) && count > 0 {
                let source = source.clone();
                return Err(Error::invalid(Case::ForgetPlaintext { source, count }));
            }
        }
        Ok(())
    }

    /// The closes of every source the act forgets: those the list does not
    /// call done, which its `source_forgotten` closes (085).
    fn all_closes(
        &self,
        view: &dyn ProjectionView,
        list: &BTreeMap<SourceId, String>,
    ) -> Result<VecDeque<EventBody>> {
        let mut closes = VecDeque::new();
        for source in &self.set {
            if list.get(source).is_some_and(|s| s == "done") {
                continue;
            }
            let (shut, _) = quarantined(view, source)?;
            closes.extend(shut);
        }
        Ok(closes)
    }

    fn finish_report(
        &mut self,
        lake: &Lake,
        reclaimed: Option<lakelands_api::write::CompactReport>,
    ) {
        let Some(report) = self.report.as_mut() else {
            return;
        };
        report.reclaimed = reclaimed;
        report.reclaim_owed = lake.reclaim_owed();
        report.position = report.position.max(self.position);
        report.way = lake.forget_way().unwrap_or(report.way.clone());
        settle(report);
    }
}

fn room(lake: &Lake) -> usize {
    usize::try_from(lake.limits.part_events.max(1)).unwrap_or(usize::MAX)
}

/// Send a forgetting of `set` along the lake's road, answering `answer`
/// where the caller waits: the store's door, which holds no writer lock
/// while the act runs.
pub(crate) fn start(
    lake: &std::sync::Arc<Lake>,
    set: Vec<SourceId>,
    report_id: Option<String>,
    known: bool,
    answer: Option<tokio::sync::oneshot::Sender<Reported>>,
) {
    crate::road::send(lake, Box::new(act_job(set, report_id, known, answer)));
}

/// The act's job, as [`start`] builds it before it is sent; the road's
/// tests build one to drive its walk and its first turn by hand, with
/// another write between them.
pub(crate) fn act_job(
    set: Vec<SourceId>,
    report_id: Option<String>,
    known: bool,
    answer: Option<tokio::sync::oneshot::Sender<Reported>>,
) -> Forget {
    Forget {
        set,
        known,
        report_id,
        window: Window::Whole,
        pending: Vec::new(),
        shape: None,
        erasable: BTreeMap::new(),
        renames: Vec::new(),
        records: BTreeMap::new(),
        taken: Vec::new(),
        retire: BTreeMap::new(),
        closes: VecDeque::new(),
        moved: BTreeSet::new(),
        walked: BTreeSet::new(),
        act: None,
        first: false,
        way: ForgetWay::default(),
        report: None,
        position: Position::GENESIS,
        answer,
    }
}
