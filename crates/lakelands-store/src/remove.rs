//! Taking back part of a source
//! (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`): each value
//! a selected `record_ingested` of the source carries reads as erased, at
//! every position, and a record left holding nothing of another hand is
//! retired.
//!
//! The act runs as forgetting runs (058), under the writer's lock, each
//! step idempotent:
//! 1. one append: `values_removed`, and a `record_retired` for each live
//!    record left holding nothing but what the selection wrote;
//! 2. the projection is rebuilt from the log, where the fold, told every
//!    act before it begins, folds each selected value as erased;
//! 3. the engine reclaims the storage.
//!
//! What a crash leaves after the append is finished by the next writer's
//! open, and a repeat of the act appends nothing again
//! ([`Lake::finish_forgetting`]). The fold learns the acts from the list
//! [`REMOVED`] keeps, read before a rebuild clears it; one it meets
//! unknown, as an import or a lost projection brings it, leaves the act
//! owed, and the rebuild after it knows it.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::engine::ProjectionView;
use lakelands_api::error::Case;
use lakelands_api::event::{Digest, Event, EventBody};
use lakelands_api::ontology::TypeDecl;
use lakelands_api::record::{Origin, StoredRecord, Value};
use lakelands_api::write::{
    Breach, ForgetRecords, RecordField, RemoveReport, RemovedField, Selection, TypeRetired,
};
use lakelands_api::{FieldName, Position, RecordKey, Result, SourceId, Timestamp, TypeName};
use lakelands_core::invalid;
use lakelands_core::sight::Sight;

use crate::forget::{each_of, named_now};
use crate::lake::{Lake, Staged, Writer};
use crate::log::Draft;
use crate::ontology::Ontology;
use crate::writes::{self, Call, Walked};

/// How many keys a report names per type.
const NAMED: usize = 10;

/// The store's own seam under which the fold keeps the acts of removal
/// the log holds, by position, each with what of it is owed: `owed`
/// until a rebuild that knew it has folded the log, `rebuilt` until the
/// reclaim ran, then `done`.
pub(crate) const REMOVED: &str = "sys:removed";

/// The input [`REMOVED`] keeps its one list under.
pub(crate) fn removed_at() -> Digest {
    crate::backed::digest(&Value::from("removed"))
}

/// One act of removal, as its event names it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Removal {
    /// The act's position: only events before it are selected.
    pub at: Position,
    pub source: SourceId,
    pub selection: Selection,
    pub before: Option<Position>,
}

impl Removal {
    /// The act an event is, if it is one.
    pub(crate) fn of(e: &Event) -> Option<Removal> {
        let EventBody::ValuesRemoved {
            source,
            selection,
            before,
        } = &e.body
        else {
            return None;
        };
        let (source, selection, before) = (source.clone(), selection.clone(), *before);
        Some(Removal {
            at: e.position,
            source,
            selection,
            before,
        })
    }

    /// Whether it selects `e`: a `record_ingested` of its source before
    /// it, below its position for a time, of its delivery for one.
    pub(crate) fn selects(&self, e: &Event) -> bool {
        let ingested = matches!(e.body, EventBody::RecordIngested { .. });
        let picked = match &self.selection {
            Selection::WrittenBefore(_) => self.before.is_some_and(|b| e.position < b),
            Selection::Delivery(d) => e.delivery.as_ref() == Some(d),
        };
        ingested && e.position < self.at && e.source.as_ref() == Some(&self.source) && picked
    }
}

/// Whether a field of `decl` may be taken: neither a key field nor a
/// reference, which identify records.
fn takeable(decl: &TypeDecl, f: &FieldName) -> bool {
    let field = decl.fields.iter().find(|d| &d.name == f);
    !decl.key.contains(f) && field.is_some_and(|d| d.ty.target.is_none())
}

/// `e` with each value the acts `removals` select cleared, and the
/// fields cleared; `None` where none selects it.
pub(crate) fn redacted(
    removals: &[Removal],
    ontology: &Ontology,
    e: &Event,
) -> Option<(Event, Vec<FieldName>)> {
    if !removals.iter().any(|r| r.selects(e)) {
        return None;
    }
    let mut plain = e.clone();
    let EventBody::RecordIngested { ty, patch, .. } = &mut plain.body else {
        return None;
    };
    let decl = ontology.ty(ty).ok()?;
    let mut cleared = Vec::new();
    for (f, v) in patch.iter_mut() {
        if !v.is_null() && takeable(decl, f) {
            *v = Value::Null;
            cleared.push(f.clone());
        }
    }
    Some((plain, cleared))
}

/// The acts of removal as the list [`REMOVED`] keeps them, at their
/// positions, as the transaction leaves it.
pub(crate) fn removed_list(
    view: &dyn ProjectionView,
    staged: &Staged,
) -> Result<BTreeMap<Position, String>> {
    let at = (REMOVED.to_owned(), removed_at());
    let list = match staged.outputs.get(&at).and_then(|l| l.last()) {
        Some((_, v)) => Some(v.clone()),
        None => view.output(&at.0, &at.1)?.map(|(_, v)| v),
    };
    let all = list
        .as_ref()
        .and_then(Value::as_object)
        .into_iter()
        .flatten();
    let state =
        |(p, s): (&String, &Value)| Some((Position(p.parse().ok()?), s.as_str()?.to_owned()));
    Ok(all.filter_map(state).collect())
}

/// `list` as [`REMOVED`] keeps it.
pub(crate) fn list_value(list: &BTreeMap<Position, String>) -> Value {
    let all = list
        .iter()
        .map(|(p, s)| (p.0.to_string(), Value::from(s.as_str())));
    Value::Object(all.collect())
}

/// The acts the list at `view` names, read from the log.
pub(crate) fn acts(lake: &Lake, view: &dyn ProjectionView) -> Result<Vec<Removal>> {
    let at: Vec<Position> = removed_list(view, &Staged::default())?
        .into_keys()
        .collect();
    Ok(lake
        .read_these(&at)?
        .iter()
        .filter_map(Removal::of)
        .collect())
}

/// What taking back `selection` of `source` at `at` (the head's next
/// position when `None`) does at `view`, beside the acts `earlier`, whose
/// events it leaves out: the report, and the live records it retires.
fn found(
    lake: &Lake,
    (w, view): (&Writer, &dyn ProjectionView),
    (source, selection): (&SourceId, &Selection),
    (at, earlier): (Option<Position>, &[Removal]),
) -> Result<(RemoveReport, Vec<StoredRecord>)> {
    let ontology = &w.ontology;
    let head = view.position();
    // The first position at or after the time: `known_at` never falls.
    let before = match selection {
        Selection::WrittenBefore(t) => Some(match lake.position_at(Timestamp(t.0 - 1))? {
            Some(p) => p.next(),
            None => Position::GENESIS,
        }),
        Selection::Delivery(_) => None,
    };
    let act = Removal {
        at: at.unwrap_or(head.map_or(Position::GENESIS, Position::next)),
        source: source.clone(),
        selection: selection.clone(),
        before,
    };
    // From the log: the selected events, the records they reached and the
    // values they carry, each by the names it bears now.
    let renames = lake.renames(Some(view))?;
    let (mut events, mut undelivered) = (0, 0);
    let mut selected = BTreeSet::new();
    let mut reached: BTreeSet<(TypeName, RecordKey)> = BTreeSet::new();
    let mut fields: BTreeMap<(TypeName, FieldName), u64> = BTreeMap::new();
    each_of(lake, view, source, |e| {
        let EventBody::RecordIngested { ty, key, patch, .. } = &e.body else {
            return;
        };
        if e.source.as_ref() != Some(source) {
            return;
        }
        undelivered += u64::from(e.delivery.is_none());
        if !act.selects(e) || earlier.iter().any(|r| r.selects(e)) {
            return;
        }
        events += 1;
        selected.insert(e.position);
        let mut ty_now = ty.clone();
        for (f, _) in patch.iter().filter(|(_, v)| !v.is_null()) {
            let (t, f) = named_now(&renames, e.position, ty, f);
            ty_now = t.clone();
            if ontology.ty(&t).is_ok_and(|d| takeable(d, &f)) {
                *fields.entry((t, f)).or_default() += 1;
            }
        }
        reached.insert((ty_now, key.clone()));
    })?;
    if matches!(selection, Selection::WrittenBefore(_)) {
        undelivered = 0;
    }
    // At the head: what each record reached keeps, and what it loses.
    let mine = |m: &lakelands_api::FieldMeta| {
        m.origin == Origin::Source
            && m.source.as_ref() == Some(source)
            && selected.contains(&m.position)
    };
    let (mut types, mut kept, mut retire) = (Vec::new(), Vec::new(), Vec::new());
    let mut breaches: Vec<Breach> = Vec::new();
    let host = lake.system("lakelands");
    let sight = Sight::held(&host, ontology);
    let by_type = reached.iter().fold(BTreeMap::new(), |mut m, (t, k)| {
        m.entry(t.clone()).or_insert_with(Vec::new).push(k.clone());
        m
    });
    for (ty, keys) in by_type {
        let Ok(decl) = ontology.ty(&ty) else { continue };
        let (mut holding, mut staying) = (counted(&ty), counted(&ty));
        let mut judged = Vec::new();
        for r in view.get_many(&ty, &keys)? {
            let lost: BTreeSet<FieldName> = (r.fields.keys())
                .filter(|f| takeable(decl, f) && r.meta.get(*f).is_some_and(mine))
                .cloned()
                .collect();
            // A record made again since holds nothing the selection wrote.
            if r.retired || (lost.is_empty() && !selected.contains(&r.made)) {
                continue;
            }
            count(&mut holding, &r);
            let starts = |f: &FieldName| {
                decl.fields
                    .iter()
                    .any(|d| &d.name == f && d.initial.is_some())
            };
            let holds = r.fields.keys().any(|f| {
                let m = r.meta.get(f);
                !decl.key.contains(f)
                    && !starts(f)
                    && m.is_some_and(|m| {
                        !mine(m) && matches!(m.origin, Origin::Authored | Origin::Source)
                    })
            });
            if !holds {
                retire.push(r);
                continue;
            }
            count(&mut staying, &r);
            if !lost.is_empty() {
                // What is computed from a value it takes goes with it.
                let mut gone = lost.clone();
                while let Some(f) = decl.fields.iter().find(|f| {
                    let computed = f.derived.is_some() || f.function.is_some() || f.embed.is_some();
                    computed
                        && !gone.contains(&f.name)
                        && f.depends_on.iter().any(|d| gone.contains(d))
                }) {
                    gone.insert(f.name.clone());
                }
                let mut is = r.clone();
                is.fields.retain(|f, _| !gone.contains(f));
                judged.push((r, is, lost));
            }
        }
        let moving: BTreeSet<FieldName> = judged.iter().flat_map(|j| j.2.iter().cloned()).collect();
        if !moving.is_empty() {
            let mut walked = Walked::of(ontology, decl, &moving, (&sight, true))?;
            for (was, is, _) in &judged {
                walked.judge((was, is), (w, w.last_known_at), &|_| Ok(true))?;
            }
            breaches.extend(walked.kept(true)?.0);
        }
        // A field declared `required` it leaves empty is listed too, as a
        // breach of `required` on that field: the act does not refuse it.
        for f in decl.fields.iter().filter(|f| f.required) {
            let mut emptied: Vec<RecordKey> = (judged.iter())
                .filter(|(was, is, _)| {
                    was.fields.contains_key(&f.name) && !is.fields.contains_key(&f.name)
                })
                .map(|(was, _, _)| was.key.clone())
                .collect();
            if emptied.is_empty() {
                continue;
            }
            emptied.sort();
            let records = emptied.len() as u64;
            emptied.truncate(5);
            breaches.push(Breach {
                ty: ty.clone(),
                constraint: "required".into(),
                records,
                sample_keys: emptied,
                field: Some(f.name.clone()),
            });
        }
        types.extend(Some(holding).filter(|h| h.records > 0));
        kept.extend(Some(staying).filter(|s| s.records > 0));
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
    // What is computed again: a computed field that reads a field it
    // takes values of, and a view over a type it reaches.
    let fields: Vec<RemovedField> = (fields.into_iter())
        .map(|((ty, field), redacted)| RemovedField {
            ty,
            field,
            redacted,
        })
        .collect();
    let mut recomputed = Vec::new();
    let mut views = Vec::new();
    for (name, entry) in &ontology.types {
        let decl = &entry.decl;
        if decl
            .view
            .as_ref()
            .is_some_and(|v| types.iter().any(|t| t.ty == v.source))
        {
            views.push(name.clone());
        }
        let taken = |n: &FieldName| fields.iter().any(|f| &f.ty == name && &f.field == n);
        for f in &decl.fields {
            let computed = f.derived.is_some() || f.function.is_some() || f.embed.is_some();
            if computed && f.depends_on.iter().any(taken) {
                let (ty, field) = (name.clone(), f.name.clone());
                recomputed.push(RecordField { ty, field });
            }
        }
    }
    let references = crate::purge::report(view, ontology, (source, None), &retire, Vec::new())?;
    let log_events = head.map_or(0, |p| p.0);
    let rate = lake.engine.rebuild_rate().max(1);
    let report = RemoveReport {
        source: source.clone(),
        selection: selection.clone(),
        before,
        events,
        undelivered,
        types,
        fields,
        retired,
        kept,
        recomputed,
        views,
        breaches,
        references: references.references,
        log_events,
        expected_ms: log_events.saturating_mul(1000) / rate,
        position: head.unwrap_or(Position::GENESIS),
        reclaimed: None,
        report_id: String::new(),
    };
    Ok((report, retire))
}

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

/// The id of the preflight of taking back `selection` of `source`, read
/// at `head`: the report is a function of the three.
fn id(source: &SourceId, selection: &Selection, head: Option<Position>) -> String {
    let of = serde_json::json!({"remove": source, "selection": selection,
                                "head": head.map_or(0, |p| p.0)});
    lakelands_core::canonical::sha256_hex(&of)
}

/// Refuse a time the log cannot write: one outside the years 0000 to
/// 9999.
fn checked(selection: &Selection) -> Result<()> {
    match selection {
        Selection::WrittenBefore(t) if !t.in_years() => {
            let value = t.to_string();
            Err(invalid(Case::NotDateTime { value }))
        }
        _ => Ok(()),
    }
}

/// What [`run`] would do, appending nothing, under the id the act may
/// repeat.
pub fn preflight(lake: &Lake, source: &SourceId, selection: &Selection) -> Result<RemoveReport> {
    checked(selection)?;
    let view = lake.engine.view(None)?;
    let w = lake.reading(view.as_ref())?;
    let earlier = acts(lake, view.as_ref())?;
    let of = (None, earlier.as_slice());
    let (mut report, _) = found(lake, (&w, view.as_ref()), (source, selection), of)?;
    report.report_id = id(source, selection, view.position());
    Ok(report)
}

/// Take back `selection` of `source`, as the module says; with a
/// preflight's `report_id`, only if that report is of this removal and
/// the lake has not moved since. A selection that selects nothing
/// appends nothing.
pub fn run(
    lake: &Lake,
    source: &SourceId,
    selection: &Selection,
    report_id: Option<&str>,
) -> Result<RemoveReport> {
    checked(selection)?;
    let mut w = lake.for_write()?;
    let view = lake.engine.view(None)?;
    // The same act in the log, still owed: its repeat finishes what a
    // failure left, whatever report it names, and appends nothing again.
    let earlier = acts(lake, view.as_ref())?;
    let list = removed_list(view.as_ref(), &Staged::default())?;
    let owed = |r: &&Removal| list.get(&r.at).is_some_and(|s| s != "done");
    let same = |r: &&Removal| &r.source == source && &r.selection == selection;
    if let Some(act) = earlier.iter().filter(same).find(owed) {
        drop(view);
        let reclaimed = lake.finish_forgetting(&mut w)?;
        let view = lake.engine.view(None)?;
        let others: Vec<Removal> = (earlier.iter())
            .filter(|r| r.at < act.at)
            .cloned()
            .collect();
        let of = (Some(act.at), others.as_slice());
        let (mut report, _) = found(lake, (&w, view.as_ref()), (source, selection), of)?;
        report.reclaimed = reclaimed;
        return Ok(report);
    }
    if report_id.is_some_and(|given| given != id(source, selection, view.position())) {
        return Err(invalid(Case::RemoveReportStale));
    }
    // What an earlier act took is not taken again: a repeat that selects
    // nothing more appends nothing.
    let of = (None, earlier.as_slice());
    let (mut report, retire) = found(lake, (&w, view.as_ref()), (source, selection), of)?;
    if report.events == 0 {
        return Ok(report);
    }
    let now = lake.clock(&mut w);
    let removed = EventBody::ValuesRemoved {
        source: source.clone(),
        selection: selection.clone(),
        before: report.before,
    };
    let system = lake.system("lakelands");
    let call = Call {
        lake,
        who: &system,
        context: lakelands_api::WriteContext::default(),
        followed: None,
        shown: None,
    };
    let mut drafts = vec![Draft::system(now, removed)];
    let mut d = call.decide(&w, view.as_ref(), now, 0);
    for before in retire {
        let body = writes::retirement(&before.ty, &before.key, before.version);
        drafts.extend(d.retiring(&before, Draft::system(now, body))?);
    }
    drop(view);
    lake.append(&mut w, drafts)?;
    report.reclaimed = lake.finish_forgetting(&mut w)?;
    report.position = w.tip.as_ref().map_or(Position::GENESIS, |t| t.0);
    Ok(report)
}
