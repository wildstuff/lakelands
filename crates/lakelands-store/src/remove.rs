//! Taking back part of a source
//! (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`): each value
//! a selected `record_ingested` of the source carries reads as erased, at
//! every position, and a record the source made left holding nothing of
//! another hand is retired.
//!
//! The act runs as forgetting runs (058), under the writer's lock, each
//! step idempotent:
//! 1. one append: `values_removed`, naming the versions of the source's
//!    key it destroys, and a `record_retired` for each live record the
//!    source made left holding nothing but what the selection wrote;
//! 2. those versions are dropped from memory and destroyed: the longest
//!    prefix of them whose values it all selects, so what they sealed is
//!    shredded, and the rest is redacted only;
//! 3. the projection is rebuilt from the log, where the fold, told every
//!    act before it begins, folds each selected value as erased, and a
//!    value under a version an act destroyed as erased, not lost;
//! 4. the engine reclaims the storage.
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
use lakelands_api::id::KeyId;
use lakelands_api::ontology::TypeDecl;
use lakelands_api::principal::Renamed;
use lakelands_api::record::{Origin, StoredRecord, Value};
use lakelands_api::write::{
    Breach, RecordField, RemoveReport, RemovedField, Selection, TypeRetired,
};
use lakelands_api::{Error, FieldName, Position, RecordKey, Result, SourceId, Timestamp, TypeName};
use lakelands_core::sight::Sight;

use crate::exec::Turn;
use crate::forget::{History, count, counted, each_of, named_now};
use crate::lake::{Lake, Staged, Writer};
use crate::log::Draft;
use crate::ontology::Ontology;
use crate::purge::made_by;
use crate::writes::{self, Call, Judging, Walked};

/// The store's own seam under which the fold keeps the acts of removal
/// the log holds, by position, each with what of it is owed: `owed`
/// until a rebuild that knew it has folded the log, `rebuilt` until the
/// reclaim ran, then `done`.
pub(crate) const REMOVED: &str = "sys:removed";

pub(crate) fn removed_at() -> Digest {
    crate::backed::digest(&Value::from("removed"))
}

#[derive(Clone, Debug)]
pub(crate) struct Removal {
    /// Only events before it are selected.
    pub at: Position,
    pub source: SourceId,
    pub selection: Selection,
    pub before: Option<Position>,
    /// The versions of the source's key it destroyed.
    pub keys: Vec<KeyId>,
    /// The source's keys of deliveries sealed apart it destroyed.
    pub apart: Vec<KeyId>,
    /// What identifies records at it, where the fold was told it
    /// ([`told`]).
    pub fixed: Fixed,
}

impl PartialEq for Removal {
    /// One act is its event, at its position: what it fixes is read with it.
    fn eq(&self, other: &Removal) -> bool {
        self.at == other.at
    }
}

/// The fields that identify records at an act: each type's key fields
/// and references, by the names they bear there, with the renames before
/// it. A value of one is never taken, though it was not one where it was
/// written (lakelands#1228).
#[derive(Clone, Debug, Default)]
pub(crate) struct Fixed {
    fields: BTreeSet<(TypeName, FieldName)>,
    renames: Vec<Renamed>,
}

impl Fixed {
    /// At `at`, where `o` is in force, by the lake's `renames`.
    pub(crate) fn of(o: &Ontology, renames: &[Renamed], at: Position) -> Fixed {
        let mut fields = BTreeSet::new();
        for d in o.types.values().map(|t| &t.decl) {
            let fixed = d.fields.iter().filter(|f| !takeable(d, &f.name));
            fields.extend(fixed.map(|f| (d.name.clone(), f.name.clone())));
        }
        let renames = renames.iter().filter(|r| r.at < at).cloned().collect();
        Fixed { fields, renames }
    }
}

impl Removal {
    pub(crate) fn of(e: &Event) -> Option<Removal> {
        let EventBody::ValuesRemoved {
            source,
            selection,
            before,
            keys,
            apart,
        } = &e.body
        else {
            return None;
        };
        let (source, selection, before) = (source.clone(), settled(selection.clone()), *before);
        Some(Removal {
            at: e.position,
            source,
            selection,
            before,
            keys: keys.clone(),
            apart: apart.clone(),
            fixed: Fixed::default(),
        })
    }

    /// Whether it destroyed `source`'s key `key`, a version or a
    /// delivery's sealed apart: a value sealed under it, missing from the
    /// key store, is erased, not lost.
    pub(crate) fn destroyed(&self, source: &SourceId, key: &KeyId) -> bool {
        &self.source == source && (self.keys.contains(key) || self.apart.contains(key))
    }

    /// Whether it selects `e`: a `record_ingested` of its source before
    /// it, below its position for a time of the log's clock, of its
    /// delivery, or of one of its set, for those, stating a `seen_at`
    /// before the time for a time the source observed.
    pub(crate) fn selects(&self, e: &Event) -> bool {
        let ingested = matches!(e.body, EventBody::RecordIngested { .. });
        let picked = || match &self.selection {
            Selection::WrittenBefore(_) => self.before.is_some_and(|b| e.position < b),
            Selection::Delivery(d) => e.delivery.as_ref() == Some(d),
            Selection::Deliveries(ds) => {
                (e.delivery.as_ref()).is_some_and(|d| ds.binary_search(d).is_ok())
            }
            Selection::SeenBefore(t) => e.seen_at.is_some_and(|s| s < *t),
        };
        ingested && e.position < self.at && e.source.as_ref() == Some(&self.source) && picked()
    }

    /// Whether it takes the value of `field` of `ty` an event at `at`
    /// wrote: one that identifies no record at the act either.
    fn takes(&self, at: Position, ty: &TypeName, field: &FieldName) -> bool {
        !(self.fixed.fields).contains(&named_now(&self.fixed.renames, at, ty, field))
    }
}

/// Whether a field of `decl` may be taken: neither a key field nor a
/// reference, which identify records.
pub(crate) fn takeable(decl: &TypeDecl, f: &FieldName) -> bool {
    let field = decl.field(f);
    !decl.key.contains(f) && field.is_some_and(|d| d.ty.target.is_none())
}

/// `e` with each value the acts `removals` select cleared, and the
/// fields cleared; `None` where none selects it. A value is taken where
/// its field identifies records neither at the event nor at the act.
pub(crate) fn redacted(
    removals: &[Removal],
    ontology: &Ontology,
    e: &Event,
) -> Option<(Event, Vec<FieldName>)> {
    let acts: Vec<&Removal> = removals.iter().filter(|r| r.selects(e)).collect();
    if acts.is_empty() {
        return None;
    }
    let mut plain = e.clone();
    let EventBody::RecordIngested { ty, patch, .. } = &mut plain.body else {
        return None;
    };
    let decl = ontology.ty(ty).ok()?;
    let mut cleared = Vec::new();
    let taken = |f: &FieldName| acts.iter().any(|r| r.takes(e.position, &decl.name, f));
    for (f, v) in patch.iter_mut() {
        if !v.is_null() && takeable(decl, f) && taken(f) {
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

pub(crate) fn list_value(list: &BTreeMap<Position, String>) -> Value {
    let all = list
        .iter()
        .map(|(p, s)| (p.0.to_string(), Value::from(s.as_str())));
    Value::Object(all.collect())
}

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

/// [`acts`], each with what identifies records at it, as the fold is told
/// them before a rebuild clears `view`: read as of the act.
pub(crate) fn told(lake: &Lake, view: &dyn ProjectionView) -> Result<Vec<Removal>> {
    let renames = lake.renames(Some(view))?;
    let mut acts = acts(lake, view)?;
    for act in &mut acts {
        let then = Ontology::at(lake.engine.view(Some(act.at))?.as_ref())?;
        act.fixed = Fixed::of(&then, &renames, act.at);
    }
    Ok(acts)
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
    // `known_at` never falls.
    let before = match selection {
        Selection::WrittenBefore(t) => Some(match lake.position_at(Timestamp(t.0 - 1))? {
            Some(p) => p.next(),
            None => Position::GENESIS,
        }),
        Selection::Delivery(_) | Selection::Deliveries(_) | Selection::SeenBefore(_) => None,
    };
    let act = Removal {
        at: at.unwrap_or(head.map_or(Position::GENESIS, Position::next)),
        source: source.clone(),
        selection: selection.clone(),
        before,
        keys: Vec::new(),
        apart: Vec::new(),
        fixed: Fixed::default(),
    };
    // From the log: the selected events, the records they reached and the
    // values they carry, each by the names and the key it bears now. A
    // value is taken where its field identifies records neither at its
    // event nor at the act, as the fold takes it (lakelands#1228).
    let history = History::new(lake, view)?;
    let (mut events, mut undelivered, mut unseen) = (0, 0, 0);
    let mut selected = BTreeSet::new();
    let mut reached: BTreeSet<(TypeName, RecordKey)> = BTreeSet::new();
    // The values of the selected events it does not take, by the event's
    // position and the field's name now.
    let mut untaken: BTreeSet<(Position, FieldName)> = BTreeSet::new();
    // Per field: values taken, and of them those sealed, by key.
    type Tally = (u64, BTreeMap<(KeyId, Vec<KeyId>), u64>);
    let mut fields: BTreeMap<(TypeName, FieldName), Tally> = BTreeMap::new();
    // Keys (versions, and of deliveries sealed apart) sealing values it selects / others.
    let (mut inside, mut outside) = (BTreeSet::new(), BTreeSet::new());
    let keys_of = |s: &lakelands_api::event::Sealed| -> Vec<(bool, KeyId)> {
        let apart = s.apart.iter().map(|k| (true, k.clone()));
        std::iter::once((false, s.key.clone()))
            .chain(apart)
            .collect()
    };
    each_of(lake, view, std::slice::from_ref(source), |_, e| {
        let EventBody::RecordIngested { ty, key, patch, .. } = &e.body else {
            return;
        };
        if e.source.as_ref() != Some(source) {
            return;
        }
        undelivered += u64::from(e.delivery.is_none());
        unseen += u64::from(e.seen_at.is_none());
        let sealed = e.sealed.as_ref();
        let taken = earlier.iter().any(|r| r.selects(e));
        if !act.selects(e) && !taken {
            outside.extend(sealed.into_iter().flat_map(keys_of));
            return;
        }
        inside.extend(sealed.into_iter().flat_map(keys_of));
        if taken {
            return;
        }
        events += 1;
        selected.insert(e.position);
        let then = history.ty(ty, e.position);
        // A box's members are not in the patch: `sealed` names them (083).
        let boxed = sealed
            .filter(|s| s.boxed.is_some())
            .map_or(&[][..], |s| &s.fields);
        let plain = patch.iter().filter(|(_, v)| !v.is_null()).map(|(f, _)| f);
        for written in plain.chain(boxed) {
            let under = sealed
                .filter(|s| s.fields.contains(written))
                .map(|s| (s.key.clone(), s.apart.clone()));
            let (t, f) = history.named((e.position, act.at), ty, written);
            let taken = then.is_some_and(|d| takeable(d, written))
                && history.ty(&t, act.at).is_some_and(|d| takeable(d, &f));
            let (t, f) = named_now(&history.renames, e.position, ty, written);
            if !taken {
                untaken.insert((e.position, f));
            } else if ontology.ty(&t).is_ok() {
                let tally = fields.entry((t, f)).or_default();
                tally.0 += 1;
                if let Some(k) = under {
                    *tally.1.entry(k).or_default() += 1;
                }
            }
        }
        reached.insert(history.moved(e.position, ty, key));
    })?;
    // The longest prefix of the versions the key store holds whose values
    // it all selects: versions die oldest first (058). A key store that
    // destroys only whole sources shreds none.
    let held = lake.keys.ids(source)?;
    let keys: Vec<KeyId> = (held.iter())
        .take_while(|k| {
            let k = (false, (*k).clone());
            lake.keys.shreds() && inside.contains(&k) && !outside.contains(&k)
        })
        .cloned()
        .collect();
    // Every key of a delivery sealed apart whose values it all selects,
    // and that the key store still holds: it is in no chain, so it goes
    // alone (065).
    let mut apart = Vec::new();
    for (_, k) in inside.iter().filter(|(a, _)| *a) {
        if !outside.contains(&(true, k.clone())) && lake.keys.holds(source, k)? {
            apart.push(k.clone());
        }
    }
    // A value under a version it destroys, or under any of a source the
    // log forgot, is shredded; one a key store lost is not. An earlier
    // act's values are not counted again.
    let forgot = crate::lake::forgotten_list(view, &Staged::default())?.contains_key(source);
    let gone = |(k, a): &(KeyId, Vec<KeyId>)| {
        keys.contains(k) || a.iter().any(|a| apart.contains(a)) || forgot
    };
    if !matches!(selection, Selection::Delivery(_) | Selection::Deliveries(_)) {
        undelivered = 0;
    }
    if !matches!(selection, Selection::SeenBefore(_)) {
        unseen = 0;
    }
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
        let mut holders = Vec::new();
        for r in view.get_many(&ty, &keys)? {
            let lost: BTreeSet<FieldName> = (r.fields.keys())
                .filter(|f| {
                    let m = r.meta.get(*f).filter(|m| mine(m));
                    let kept = m.is_some_and(|m| untaken.contains(&(m.position, (*f).clone())));
                    takeable(decl, f) && m.is_some() && !kept
                })
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
            holders.push((r, lost, holds));
        }
        // A record the source did not make is never retired, by the test
        // the purge and forgetting retire by (039).
        let bare: Vec<&StoredRecord> = (holders.iter().filter(|h| !h.2)).map(|h| &h.0).collect();
        let made = made_by(lake, &bare, std::slice::from_ref(source))?;
        for (r, lost, holds) in holders {
            if !holds && made.contains(&r.made) {
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
            let mut walked = Walked::of(ontology, decl, &moving, (&sight, Judging::Proposal))?;
            for (was, is, _) in &judged {
                walked.judge((was, is), (w, w.last_known_at), &|_| Ok(true))?;
            }
            breaches.extend(walked.kept(Judging::Proposal)?.0);
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
    let mut unshredded = 0;
    let fields: Vec<RemovedField> = (fields.into_iter())
        .map(|((ty, field), (redacted, sealed))| {
            let shredded = sealed.iter().filter(|(k, _)| gone(k)).map(|(_, n)| n).sum();
            let erasable = ontology.ty(&ty).ok().and_then(|d| d.field(&field));
            if erasable.is_some_and(|f| f.erasable) {
                unshredded += redacted - shredded;
            }
            RemovedField {
                ty,
                field,
                redacted,
                shredded,
            }
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
    let report = RemoveReport {
        source: source.clone(),
        selection: selection.clone(),
        before,
        events,
        undelivered,
        unseen,
        types,
        fields,
        retired,
        kept,
        recomputed,
        views,
        breaches,
        references: references.references,
        keys,
        apart,
        unshredded,
        log_events,
        expected_ms: lake.expected_ms(log_events),
        position: head.unwrap_or(Position::GENESIS),
        reclaimed: None,
        reclaim_owed: lake.reclaim_owed(),
        report_id: String::new(),
    };
    Ok((report, retire))
}

/// The id of the preflight of taking back `selection` of `source`, read
/// at `head`: the report is a function of the three.
fn id(source: &SourceId, selection: &Selection, head: Option<Position>) -> String {
    let of = serde_json::json!({"remove": source, "selection": selection,
                                "head": head.map_or(0, |p| p.0)});
    lakelands_core::canonical::sha256_hex(&of)
}

/// `selection` as the act takes it, a time the log cannot write, one
/// outside the years 0000 to 9999, refused.
fn checked(selection: &Selection) -> Result<Selection> {
    match selection {
        Selection::WrittenBefore(t) | Selection::SeenBefore(t) if !t.in_years() => {
            let value = t.to_string();
            Err(Error::invalid(Case::NotDateTime { value }))
        }
        _ => Ok(settled(selection.clone())),
    }
}

/// `selection` with a set's ids in order, each once: an event's delivery
/// is found in it by a binary search, as the fold asks of every event.
fn settled(selection: Selection) -> Selection {
    match selection {
        Selection::Deliveries(mut ds) => {
            ds.sort_unstable();
            ds.dedup();
            Selection::Deliveries(ds)
        }
        other => other,
    }
}

/// What [`run`] would do, appending nothing, under the id the act may repeat.
pub fn preflight(lake: &Lake, source: &SourceId, selection: &Selection) -> Result<RemoveReport> {
    let selection = &checked(selection)?;
    let began = std::time::Instant::now();
    let view = lake.engine.view(None)?;
    let w = lake.reading(view.as_ref())?;
    let earlier = acts(lake, view.as_ref())?;
    let (mut report, _) = found(
        lake,
        (&w, view.as_ref()),
        (source, selection),
        (None, &earlier),
    )?;
    report.report_id = id(source, selection, view.position());
    report.expected_ms = (report.expected_ms).saturating_add(crate::lake::elapsed_ms(began));
    Ok(report)
}

/// Take back `selection` of `source`, as the module says; with a
/// preflight's `report_id`, only if that report is of this removal and
/// the lake has not moved since; with `shredded`, only if no value of an
/// erasable field would be redacted alone. A selection that selects
/// nothing appends nothing.
pub fn run(
    lake: &Turn<'_>,
    source: &SourceId,
    selection: &Selection,
    (shredded, report_id): (bool, Option<&str>),
) -> Result<RemoveReport> {
    let selection = &checked(selection)?;
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
        return Err(Error::invalid(Case::RemoveReportStale));
    }
    // What an earlier act took is not taken again: a repeat that selects
    // nothing more appends nothing.
    let of = (None, earlier.as_slice());
    let (mut report, retire) = found(lake, (&w, view.as_ref()), (source, selection), of)?;
    if report.events == 0 {
        return Ok(report);
    }
    if shredded && report.unshredded > 0 {
        let (source, count) = (source.clone(), report.unshredded);
        return Err(Error::invalid(Case::RemoveNotShredded { source, count }));
    }
    let now = lake.clock(&mut w);
    let removed = EventBody::ValuesRemoved {
        source: source.clone(),
        selection: selection.clone(),
        before: report.before,
        keys: report.keys.clone(),
        apart: report.apart.clone(),
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
    let mut d = call.decide(&w, view.as_ref(), now, drafts.len());
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
