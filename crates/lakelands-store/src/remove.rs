//! Taking back part of a source
//! (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`): each value
//! a selected `record_ingested` of the source carries reads as erased, at
//! every position, and a record the source made left holding nothing of
//! another hand is retired.
//!
//! The act runs on the road, beside the writer
//! (`docs/decisions/099-work-beside-the-writer-runs-in-parts.md`), each
//! step idempotent:
//! 1. the walk, off the writer, gathers the selection and judges every
//!    record it reached; a report is read at its position, and its id is
//!    that position with a digest of what the selection took there;
//! 2. the first turn, under the writer, folds the tail the walk did not
//!    see into that gathering, cuts the versions again, reads again the
//!    keys the tail moved, and appends `values_removed` — naming the
//!    versions of the source's key it destroys — the quarantine closes
//!    that fit, the first retirements, and `act_whole` where it fits;
//! 3. those versions are then dropped from memory and destroyed: the
//!    longest prefix of them whose values it all selects, so what they
//!    sealed is shredded, and the rest is redacted only;
//! 4. later turns append the closes and the retirements still owed, each
//!    read again as the part takes it, and `act_whole` ends them;
//! 5. the act's last turn rebuilds the projection from the log, where the
//!    fold, told every act before it begins, folds each selected value as
//!    erased, and a value under a version an act destroyed as erased, not
//!    lost, then the engine reclaims the storage.
//!
//! What a crash leaves is taken up again by the road's owed kinds at the
//! next writer's open, which answers before they run, and a repeat of the
//! act appends nothing again ([`Lake::finish_forgetting`]). The fold
//! learns the acts from the list [`REMOVED`] keeps, read before a rebuild
//! clears it; one it meets unknown, as an import or a lost projection
//! brings it, leaves the act owed, and the rebuild after it knows it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

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
use lakelands_core::links;
use lakelands_core::sight::Sight;

use crate::exec::Turn;
use crate::forget::{History, count, counted, each_of, named_now};
use crate::lake::{Lake, Staged, Writer, owing_now};
use crate::log::Draft;
use crate::ontology::Ontology;
use crate::purge::{made_by, no_owner};
use crate::road::{Job, Reach, Turned};
use crate::writes::{self, Judging, Walked};

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
    /// The source whose keys it destroys: a removal's, a quarantined
    /// entry's for an erase of it; none for an erase of a record (115).
    pub source: Option<SourceId>,
    pub selection: Selection,
    pub before: Option<Position>,
    /// The versions of the source's key it destroyed.
    pub keys: Vec<KeyId>,
    /// The source's keys of deliveries sealed apart it destroyed.
    pub apart: Vec<KeyId>,
    /// What identifies records at it, where the fold was told it
    /// ([`told`]).
    pub fixed: Fixed,
    /// For an erase of a record, the names it bore before the act, the
    /// latest first, each from the position it holds from ([`told`]).
    pub chain: Option<Vec<Link>>,
}

/// A record's type and key from a position on, up to the next link.
pub(crate) type Link = (Position, TypeName, RecordKey);

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
    /// The act `e` is: a `values_removed` or a `values_erased` (115).
    pub(crate) fn of(e: &Event) -> Option<Removal> {
        let (source, selection, before, keys, apart) = match &e.body {
            EventBody::ValuesRemoved {
                source,
                selection,
                before,
                keys,
                apart,
            } => (
                Some(source.clone()),
                settled(selection.clone()),
                *before,
                keys,
                apart,
            ),
            EventBody::ValuesErased {
                selection,
                keys,
                apart,
            } => (owner(selection), selection.clone(), None, keys, apart),
            _ => return None,
        };
        Some(Removal {
            at: e.position,
            source,
            selection,
            before,
            keys: keys.clone(),
            apart: apart.clone(),
            fixed: Fixed::default(),
            chain: None,
        })
    }

    /// Whether it is an erase (115), not a removal.
    pub(crate) fn erases(&self) -> bool {
        matches!(
            self.selection,
            Selection::Record { .. } | Selection::Quarantined(_)
        )
    }

    /// Whether it erases the record of `ty` at `key` as an event at `at`
    /// names it: a link of its chain.
    pub(crate) fn names(&self, at: Position, ty: &TypeName, key: &RecordKey) -> bool {
        let link = (self.chain.iter().flatten()).find(|(from, ..)| *from <= at);
        at < self.at && link.is_some_and(|(_, t, k)| t == ty && k == key)
    }

    /// Whether it destroyed `source`'s key `key`, a version or a
    /// delivery's sealed apart: a value sealed under it, missing from the
    /// key store, is erased, not lost.
    pub(crate) fn destroyed(&self, source: &SourceId, key: &KeyId) -> bool {
        self.source.as_ref() == Some(source)
            && (self.keys.contains(key) || self.apart.contains(key))
    }

    /// Whether it selects `e`: a `record_ingested` of its source before
    /// it, below its position for a time of the log's clock, of its
    /// delivery, or of one of its set, for those, stating a `seen_at`
    /// before the time for a time the source observed.
    /// A quarantined record's version is selected so too, read
    /// [`crate::sealing::lifted`] (085).
    pub(crate) fn selects(&self, e: &Event) -> bool {
        match (&self.selection, &e.body) {
            (
                Selection::Record { .. },
                EventBody::RecordUpserted { ty, key, .. }
                | EventBody::RecordIngested { ty, key, .. },
            ) => self.names(e.position, ty, key),
            (Selection::Quarantined(id), EventBody::RecordQuarantined { id: of, .. }) => {
                e.position < self.at && of == id
            }
            (Selection::Record { .. } | Selection::Quarantined(_), _) => false,
            (selection, _) => {
                (self.source.as_ref()).is_some_and(|s| picks(s, selection, self.before, self.at, e))
            }
        }
    }

    /// Whether it takes the value of `field` of `ty` an event at `at`
    /// wrote: one that identifies no record at the act either.
    /// An erase of one field takes that field's alone (115).
    pub(crate) fn takes(&self, at: Position, ty: &TypeName, field: &FieldName) -> bool {
        let now = named_now(&self.fixed.renames, at, ty, field);
        let one = match &self.selection {
            Selection::Record { field: Some(f), .. } => &now.1 == f,
            _ => true,
        };
        one && !(self.fixed.fields).contains(&now)
    }
}

/// Whether a field of `decl` may be taken: neither a key field nor a
/// reference, which identify records.
pub(crate) fn takeable(decl: &TypeDecl, f: &FieldName) -> bool {
    let field = decl.field(f);
    !decl.key.contains(f) && field.is_some_and(|d| d.ty.target.is_none())
}

/// Whose keys an erase of `selection` destroys: a quarantined entry's
/// source's, which sealed its versions; a record's values are many
/// sources' and the hosts', so an erase of one destroys none (115).
pub(crate) fn owner(selection: &Selection) -> Option<SourceId> {
    match selection {
        Selection::Quarantined(id) => Some(id.source.clone()),
        _ => None,
    }
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
    // A quarantined record's version selected keeps no cell (085).
    if let EventBody::RecordQuarantined { cells, .. } = &mut plain.body {
        let cleared = std::mem::take(cells).into_keys().collect();
        return Some((plain, cleared));
    }
    let (EventBody::RecordIngested { ty, patch, .. } | EventBody::RecordUpserted { ty, patch, .. }) =
        &mut plain.body
    else {
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
        act.chain = chain(lake, view, act)?;
    }
    Ok(acts)
}

/// For an erase of a record, the names it bore before `act`, read up to
/// the head of `view` (115).
pub(crate) fn chain(
    lake: &Lake,
    view: &dyn ProjectionView,
    act: &Removal,
) -> Result<Option<Vec<Link>>> {
    let Selection::Record { ty, key, .. } = &act.selection else {
        return Ok(None);
    };
    let history = crate::forget::History::new(lake, view)?;
    Ok(Some(history.chain(ty, key, act.at)))
}

/// Whether `e` is what the selection of `source` takes at `at`: the
/// [`Removal::selects`] test, for the walk, the tail and the closes.
fn picks(
    source: &SourceId,
    selection: &Selection,
    before: Option<Position>,
    at: Position,
    e: &Event,
) -> bool {
    let ingested = matches!(
        e.body,
        EventBody::RecordIngested { .. } | EventBody::RecordQuarantined { .. }
    );
    let picked = || match selection {
        Selection::WrittenBefore(_) => before.is_some_and(|b| e.position < b),
        Selection::Delivery(d) => e.delivery.as_ref() == Some(d),
        Selection::Deliveries(ds) => {
            (e.delivery.as_ref()).is_some_and(|d| ds.binary_search(d).is_ok())
        }
        // A quarantined record states its `seen_at` in its payload.
        Selection::SeenBefore(t) => match &e.body {
            EventBody::RecordQuarantined { seen_at, .. } => *seen_at,
            _ => e.seen_at,
        }
        .is_some_and(|s| s < *t),
        // An erase selects by no source (115).
        Selection::Record { .. } | Selection::Quarantined(_) => false,
    };
    ingested && e.position < at && e.source.as_ref() == Some(source) && picked()
}

/// The versions and the keys of deliveries sealed apart that sealed a
/// value, the version first.
pub(crate) fn keys_of(s: &lakelands_api::event::Sealed) -> Vec<(bool, KeyId)> {
    let apart = s.apart.iter().map(|k| (true, k.clone()));
    std::iter::once((false, s.key.clone()))
        .chain(apart)
        .collect()
}

/// Per field: values taken, and of them those sealed, by key.
type Tally = (u64, BTreeMap<(KeyId, Vec<KeyId>), u64>);

/// What the selection takes of the source, gathered off the writer: its
/// selected events, the values they carry, the records they reached and
/// the keys that sealed them, inside the act or outside it.
struct Gathered {
    source: SourceId,
    selection: Selection,
    /// Where the act's first event lands, or landed.
    at: Position,
    before: Option<Position>,
    /// The acts before this one, whose events it leaves out.
    earlier: Vec<Removal>,
    history: History,
    events: u64,
    undelivered: u64,
    unseen: u64,
    selected: BTreeSet<Position>,
    reached: BTreeSet<(TypeName, RecordKey)>,
    /// The values of the selected events it does not take, by the event's
    /// position and the field's name now.
    untaken: BTreeSet<(Position, FieldName)>,
    fields: BTreeMap<(TypeName, FieldName), Tally>,
    inside: BTreeSet<(bool, KeyId)>,
    outside: BTreeSet<(bool, KeyId)>,
}

impl Gathered {
    fn gather(
        lake: &Lake,
        view: &dyn ProjectionView,
        ontology: &Ontology,
        (source, selection): (&SourceId, &Selection),
        (at, earlier): (Position, &[Removal]),
    ) -> Result<Gathered> {
        // `known_at` never falls. The cut is searched through the view's
        // mark, not the published head: a write folded past that head but
        // inside the view's window is this gather's to judge, or the tail
        // past the view would never read it (099).
        let before = match selection {
            Selection::WrittenBefore(t) => Some(
                match lake.position_through(Timestamp(t.0 - 1), view.position())? {
                    Some(p) => p.next(),
                    None => Position::GENESIS,
                },
            ),
            _ => None,
        };
        let mut g = Gathered {
            source: source.clone(),
            selection: selection.clone(),
            at,
            before,
            earlier: earlier.to_vec(),
            history: History::new(lake, view)?,
            events: 0,
            undelivered: 0,
            unseen: 0,
            selected: BTreeSet::new(),
            reached: BTreeSet::new(),
            untaken: BTreeSet::new(),
            fields: BTreeMap::new(),
            inside: BTreeSet::new(),
            outside: BTreeSet::new(),
        };
        each_of(lake, view, std::slice::from_ref(source), |_, e| {
            g.event(ontology, e);
        })?;
        if !matches!(selection, Selection::Delivery(_) | Selection::Deliveries(_)) {
            g.undelivered = 0;
        }
        if !matches!(selection, Selection::SeenBefore(_)) {
            g.unseen = 0;
        }
        Ok(g)
    }

    /// Take the contribution of `e`, an event of the source's, as the
    /// walk and the first turn's tail both do.
    fn event(&mut self, ontology: &Ontology, e: &Event) {
        if e.source.as_ref() != Some(&self.source) {
            return;
        }
        // A quarantined version is selected as a `record_ingested` is, its
        // keys inside the act or outside it; it lands no value (085).
        if let EventBody::RecordQuarantined { sealed, .. } = &e.body {
            let (now, taken) = (
                picks(&self.source, &self.selection, self.before, self.at, e),
                self.earlier.iter().any(|r| r.selects(e)),
            );
            let keys = sealed.iter().flat_map(keys_of);
            match now || taken {
                true => self.inside.extend(keys),
                false => self.outside.extend(keys),
            }
            self.events += u64::from(now && !taken);
            return;
        }
        let EventBody::RecordIngested { ty, key, patch, .. } = &e.body else {
            return;
        };
        self.undelivered += u64::from(e.delivery.is_none());
        self.unseen += u64::from(e.seen_at.is_none());
        let sealed = e.sealed.as_ref();
        let taken = self.earlier.iter().any(|r| r.selects(e));
        if !picks(&self.source, &self.selection, self.before, self.at, e) && !taken {
            self.outside.extend(sealed.into_iter().flat_map(keys_of));
            return;
        }
        self.inside.extend(sealed.into_iter().flat_map(keys_of));
        if taken {
            return;
        }
        self.events += 1;
        self.selected.insert(e.position);
        let then = self.history.ty(ty, e.position);
        // A box's members are not in the patch: `sealed` names them (083).
        let boxed = sealed
            .filter(|s| s.boxed.is_some())
            .map_or(&[][..], |s| &s.fields);
        let plain = patch.iter().filter(|(_, v)| !v.is_null()).map(|(f, _)| f);
        for written in plain.chain(boxed) {
            let under = sealed
                .filter(|s| s.fields.contains(written))
                .map(|s| (s.key.clone(), s.apart.clone()));
            let (t, f) = self.history.named((e.position, self.at), ty, written);
            let taken = then.is_some_and(|d| takeable(d, written))
                && self
                    .history
                    .ty(&t, self.at)
                    .is_some_and(|d| takeable(d, &f));
            let (t, f) = named_now(&self.history.renames, e.position, ty, written);
            if !taken {
                self.untaken.insert((e.position, f));
            } else if ontology.ty(&t).is_ok() {
                let tally = self.fields.entry((t, f)).or_default();
                tally.0 += 1;
                if let Some(k) = under {
                    *tally.1.entry(k).or_default() += 1;
                }
            }
        }
        self.reached.insert(self.history.moved(e.position, ty, key));
    }

    /// The versions the act destroys, and the keys of deliveries sealed
    /// apart it destroys: the longest prefix of the versions the key store
    /// holds whose values it all selects, as a forget destroys. Versions
    /// die oldest first (058), and a key store that destroys only whole
    /// sources shreds none.
    fn keys(&self, lake: &Lake) -> Result<(Vec<KeyId>, Vec<KeyId>)> {
        let held = lake.keys.ids(&self.source)?;
        let keys: Vec<KeyId> = (held.iter())
            .take_while(|k| {
                let k = (false, (*k).clone());
                lake.keys.shreds() && self.inside.contains(&k) && !self.outside.contains(&k)
            })
            .cloned()
            .collect();
        // Every key of a delivery sealed apart whose values it all selects,
        // and that the key store still holds: it is in no chain, so it goes
        // alone (065).
        let mut apart = Vec::new();
        for (_, k) in self.inside.iter().filter(|(a, _)| *a) {
            if !self.outside.contains(&(true, k.clone())) && lake.keys.holds(&self.source, k)? {
                apart.push(k.clone());
            }
        }
        Ok((keys, apart))
    }
}

/// A record the selection touched, as it stands, and what the selection
/// takes of it.
#[derive(Clone, PartialEq)]
struct Holder {
    r: StoredRecord,
    lost: BTreeSet<FieldName>,
    holds: bool,
}

/// The live records the selection touched, by type then key, and the
/// positions at which the source made the ones it retires.
#[derive(Default)]
struct Judged {
    holders: BTreeMap<TypeName, BTreeMap<RecordKey, Holder>>,
    made: BTreeSet<Position>,
}

impl Judged {
    fn of(
        lake: &Lake,
        view: &dyn ProjectionView,
        ontology: &Ontology,
        g: &Gathered,
    ) -> Result<Judged> {
        let mut by_type: BTreeMap<TypeName, Vec<RecordKey>> = BTreeMap::new();
        for (ty, key) in &g.reached {
            by_type.entry(ty.clone()).or_default().push(key.clone());
        }
        let mut holders: BTreeMap<TypeName, BTreeMap<RecordKey, Holder>> = BTreeMap::new();
        for (ty, keys) in by_type {
            let Ok(decl) = ontology.ty(&ty) else { continue };
            let mut at = BTreeMap::new();
            for r in view.get_many(&ty, &keys)? {
                if let Some(h) = holder_of(decl, g, &r) {
                    at.insert(r.key.clone(), h);
                }
            }
            if !at.is_empty() {
                holders.insert(ty, at);
            }
        }
        // A record the source did not make is never retired, by the test
        // the purge and forgetting retire by (039).
        let bare: Vec<&StoredRecord> = (holders.values().flatten().map(|(_, h)| h))
            .filter(|h| !h.holds)
            .map(|h| &h.r)
            .collect();
        let made = made_by(lake, &bare, std::slice::from_ref(&g.source))?;
        Ok(Judged { holders, made })
    }

    /// The records the act retires: the source made them and no other
    /// hand holds a value.
    fn retirable(&self) -> impl Iterator<Item = &Holder> {
        (self.holders.values().flatten().map(|(_, h)| h))
            .filter(|h| !h.holds && self.made.contains(&h.r.made))
    }

    /// Read the keys the tail moved again, as they stand under the
    /// writer, and judge them by the walk's selection: one the source
    /// wrote since is taken in, and one another hand wrote drops out.
    /// Whether any of them judges differently than the walk did.
    fn again(
        &mut self,
        lake: &Lake,
        view: &dyn ProjectionView,
        ontology: &Ontology,
        g: &Gathered,
        moved: &BTreeSet<(TypeName, RecordKey)>,
    ) -> Result<bool> {
        let mut changed = false;
        for (ty, key) in moved {
            let before = self.holders.get(ty).and_then(|h| h.get(key)).cloned();
            if let Some(h) = self.holders.get_mut(ty) {
                h.remove(key);
            }
            let now = match view.get(ty, key)? {
                Some(r) if !r.retired => {
                    ontology.ty(ty).ok().and_then(|decl| holder_of(decl, g, &r))
                }
                _ => None,
            };
            changed |= before.as_ref() != now.as_ref();
            let Some(h) = now else { continue };
            if !h.holds {
                let made = made_by(
                    lake,
                    std::slice::from_ref(&&h.r),
                    std::slice::from_ref(&g.source),
                )?;
                self.made.extend(made);
            }
            self.holders
                .entry(ty.clone())
                .or_default()
                .insert(key.clone(), h);
        }
        Ok(changed)
    }
}

/// `r` as the selection judges it, where it judged it at all.
fn holder_of(decl: &TypeDecl, g: &Gathered, r: &StoredRecord) -> Option<Holder> {
    let mine = |m: &lakelands_api::FieldMeta| {
        m.origin == Origin::Source
            && m.source.as_ref() == Some(&g.source)
            && g.selected.contains(&m.position)
    };
    let lost: BTreeSet<FieldName> = (r.fields.keys())
        .filter(|f| {
            let m = r.meta.get(*f).filter(|m| mine(m));
            let kept = m.is_some_and(|m| g.untaken.contains(&(m.position, (*f).clone())));
            takeable(decl, f) && m.is_some() && !kept
        })
        .cloned()
        .collect();
    // A record made again since holds nothing the selection wrote.
    if r.retired || (lost.is_empty() && !g.selected.contains(&r.made)) {
        return None;
    }
    let holds = r.fields.keys().any(|f| {
        let m = r.meta.get(f);
        !decl.key.contains(f)
            && !no_owner(decl, f, m)
            && m.is_some_and(|m| !mine(m) && matches!(m.origin, Origin::Authored | Origin::Source))
    });
    Some(Holder {
        r: r.clone(),
        lost,
        holds,
    })
}

/// The fields the act redacts, and of them the ones no destroyed key
/// shreds: the values of erasable fields of `g`.
fn fields_of(
    ontology: &Ontology,
    g: &Gathered,
    (keys, apart): (&[KeyId], &[KeyId]),
    forgot: bool,
) -> (Vec<RemovedField>, u64) {
    let gone = |(k, a): &(KeyId, Vec<KeyId>)| {
        keys.contains(k) || a.iter().any(|a| apart.contains(a)) || forgot
    };
    let mut unshredded = 0;
    let fields: Vec<RemovedField> = (g.fields.iter())
        .map(|((ty, field), (redacted, sealed))| {
            let shredded = sealed.iter().filter(|(k, _)| gone(k)).map(|(_, n)| n).sum();
            let erasable = ontology.ty(ty).ok().and_then(|d| d.field(field));
            if erasable.is_some_and(|f| f.erasable) {
                unshredded += redacted - shredded;
            }
            RemovedField {
                ty: ty.clone(),
                field: field.clone(),
                redacted: *redacted,
                shredded,
            }
        })
        .collect();
    (fields, unshredded)
}

/// The report of the selection `g` judged: its tally, the records held and
/// retired, and what is computed from them.
#[allow(clippy::too_many_arguments)]
fn report_of(
    lake: &Lake,
    (w, view): (&Writer, &dyn ProjectionView),
    ontology: &Ontology,
    g: &Gathered,
    judged: &Judged,
    (keys, apart): (&[KeyId], &[KeyId]),
    forgot: bool,
    quarantined: Vec<lakelands_api::quarantine::TypeQuarantined>,
    head: Option<Position>,
) -> Result<RemoveReport> {
    let retire: Vec<StoredRecord> = judged.retirable().map(|h| h.r.clone()).collect();
    let mut types = Vec::new();
    let mut kept = Vec::new();
    let mut breaches: Vec<Breach> = Vec::new();
    let host = lake.system("lakelands");
    let sight = Sight::held(&host, ontology);
    for (ty, holders) in &judged.holders {
        let Ok(decl) = ontology.ty(ty) else { continue };
        let (mut holding, mut staying) = (counted(ty), counted(ty));
        let mut judged_here: Vec<(StoredRecord, StoredRecord, BTreeSet<FieldName>)> = Vec::new();
        for h in holders.values() {
            count(&mut holding, &h.r);
            if !h.holds && judged.made.contains(&h.r.made) {
                continue;
            }
            count(&mut staying, &h.r);
            if h.lost.is_empty() {
                continue;
            }
            // What is computed from a value it takes goes with it.
            let mut gone_fields = h.lost.clone();
            while let Some(f) = decl.fields.iter().find(|f| {
                let computed = f.derived.is_some() || f.function.is_some() || f.embed.is_some();
                computed
                    && !gone_fields.contains(&f.name)
                    && f.depends_on.iter().any(|d| gone_fields.contains(d))
            }) {
                gone_fields.insert(f.name.clone());
            }
            let mut is = h.r.clone();
            is.fields.retain(|f, _| !gone_fields.contains(f));
            judged_here.push((h.r.clone(), is, h.lost.clone()));
        }
        let moving: BTreeSet<FieldName> = (judged_here.iter())
            .flat_map(|j| j.2.iter().cloned())
            .collect();
        if !moving.is_empty() {
            let mut walked = Walked::of(ontology, decl, &moving, (&sight, Judging::Proposal))?;
            for (was, is, _) in &judged_here {
                walked.judge((was, is), (w, w.last_known_at), &|_| Ok(true))?;
            }
            breaches.extend(walked.kept(Judging::Proposal)?.0);
        }
        // A field declared `required` it leaves empty is listed too, as a
        // breach of `required` on that field: the act does not refuse it.
        for f in decl.fields.iter().filter(|f| f.required) {
            let mut emptied: Vec<RecordKey> = (judged_here.iter())
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
    let (fields, unshredded) = fields_of(ontology, g, (keys, apart), forgot);
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
    let references = crate::purge::report(view, ontology, (&g.source, None), &retire, Vec::new())?;
    let log_events = head.map_or(0, |p| p.0);
    Ok(RemoveReport {
        source: g.source.clone(),
        selection: g.selection.clone(),
        before: g.before,
        events: g.events,
        undelivered: g.undelivered,
        unseen: g.unseen,
        types,
        fields,
        retired,
        kept,
        recomputed,
        views,
        breaches,
        references: references.references,
        keys: keys.to_vec(),
        apart: apart.to_vec(),
        unshredded,
        log_events,
        expected_ms: lake.expected_ms(log_events),
        reclaim_untimed: lake.untimed(),
        position: head.unwrap_or(Position::GENESIS),
        reclaimed: None,
        reclaim_owed: lake.reclaim_owed(),
        report_id: String::new(),
        quarantined,
    })
}

/// The closes of the open quarantined records of `source` whose last
/// version the selection takes, every one of which the act closes (085),
/// and their count per type.
fn closes(
    lake: &Lake,
    view: &dyn ProjectionView,
    source: &SourceId,
    selection: &Selection,
    g: &Gathered,
) -> Result<(
    Vec<EventBody>,
    Vec<lakelands_api::quarantine::TypeQuarantined>,
)> {
    let why = lakelands_api::quarantine::ClosedWhy::Removed;
    crate::quarantine::act_closes(view, (source, None), why, &mut |q| {
        let last = lake.read_these(&[q.position])?;
        Ok(last
            .first()
            .is_some_and(|e| picks(source, selection, g.before, g.at, e)))
    })
}

/// Whether any declaration here can name another record: a link type, or a
/// relation with a `via` field. The one way a write to a record the act did
/// not reach moves what a removal's report states (099).
fn relating(ontology: &Ontology) -> bool {
    ontology.types.iter().any(|(name, entry)| {
        links::link_type_parts(name).is_some()
            || entry.decl.relations.iter().any(|r| r.via.is_some())
    })
}

/// The id of a report: the position it was read at, then the digest of the
/// records it selected, by the `made` position of each (099).
fn token(
    source: &SourceId,
    selection: &Selection,
    through: Position,
    made: &BTreeSet<Position>,
) -> String {
    let made: Vec<u64> = made.iter().map(|p| p.0).collect();
    let of = serde_json::json!({"remove": source, "selection": selection, "made": made});
    format!(
        "{}-{}",
        through.0,
        lakelands_core::canonical::sha256_hex(&of)
    )
}

/// The window a `report_id` names; a string that is not one is a report of
/// no removal this lake reads.
fn window_of(id: &str) -> Result<Window> {
    let (through, digest) = id
        .split_once('-')
        .ok_or_else(|| Error::invalid(Case::RemoveReportStale))?;
    let through: u64 = through
        .parse()
        .map_err(|_| Error::invalid(Case::RemoveReportStale))?;
    Ok(Window::Report {
        through: Position(through),
        digest: digest.to_owned(),
    })
}

/// `selection` as the act takes it, a time the log cannot write, one
/// outside the years 0000 to 9999, refused.
pub(crate) fn checked(selection: &Selection) -> Result<Selection> {
    match selection {
        Selection::WrittenBefore(t) | Selection::SeenBefore(t) if !t.in_years() => {
            let value = t.to_string();
            Err(Error::invalid(Case::NotDateTime { value }))
        }
        Selection::Record { .. } | Selection::Quarantined(_) => {
            Err(Error::invalid(Case::RemoveErases))
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

/// Whether the act at `at` appended its `act_whole`.
pub(crate) fn whole(lake: &Lake, at: Position, head: Option<Position>) -> Result<bool> {
    whole_from(lake, at, at, head)
}

/// Whether the act at `at` appended its `act_whole` at or after `from`. The
/// act's whole is its last event, so a turn that has appended any of the act
/// reads only what lies past its own last append (099).
pub(crate) fn whole_from(
    lake: &Lake,
    at: Position,
    from: Position,
    head: Option<Position>,
) -> Result<bool> {
    let Some(head) = head.filter(|h| *h >= at) else {
        return Ok(false);
    };
    for page in lake.pages(from) {
        for e in page? {
            if e.position > head {
                return Ok(false);
            }
            match e.body {
                EventBody::ActWhole { act } if act == at => return Ok(true),
                // An erase is one event: it is whole where it stands (115).
                EventBody::ValuesErased { .. } if e.position == at => return Ok(true),
                _ => {}
            }
        }
    }
    Ok(false)
}

/// What [`preflight`] would do, appending nothing, under the id the act
/// may repeat.
pub fn preflight(lake: &Lake, source: &SourceId, selection: &Selection) -> Result<RemoveReport> {
    let selection = &checked(selection)?;
    let began = std::time::Instant::now();
    let view = lake.engine.view(None)?;
    let w = lake.reading(view.as_ref())?;
    let ontology = Ontology::at(view.as_ref())?;
    let all = acts(lake, view.as_ref())?;
    let at = view.position().map_or(Position::GENESIS, Position::next);
    let g = Gathered::gather(
        lake,
        view.as_ref(),
        &ontology,
        (source, selection),
        (at, &all),
    )?;
    let judged = Judged::of(lake, view.as_ref(), &ontology, &g)?;
    let (keys, apart) = g.keys(lake)?;
    let forgot =
        crate::lake::forgotten_list(view.as_ref(), &Staged::default())?.contains_key(source);
    let (_, quarantined) = closes(lake, view.as_ref(), source, selection, &g)?;
    let mut report = report_of(
        lake,
        (&w, view.as_ref()),
        &ontology,
        &g,
        &judged,
        (&keys, &apart),
        forgot,
        quarantined,
        view.position(),
    )?;
    let through = view.position().unwrap_or(Position::GENESIS);
    report.report_id = token(source, selection, through, &judged.made);
    report.expected_ms = (report.expected_ms).saturating_add(crate::lake::elapsed_ms(began));
    Ok(report)
}

/// What a remove answers its caller: the report of the act, and the
/// positions it wrote, whose backed fields settle after it.
pub(crate) type Reported = Result<(RemoveReport, BTreeSet<Position>)>;

/// What a turn leaves for the restate: nothing appended, another part, or
/// an `act_whole` whose restate and reclaim run with the view let go.
enum Next {
    Nothing,
    More,
    Whole,
}

/// How a report names the selection it read.
enum Window {
    /// No report: the whole of what the selection takes.
    Whole,
    /// A report read at `through`, whose digest is of the records it
    /// selected there.
    Report { through: Position, digest: String },
    /// An act the log began, taken up again.
    Act,
}

/// One remove on the road: the act of taking `selection` of `source`
/// back, in parts of at most `part_events` (099).
pub(crate) struct Remove {
    source: SourceId,
    selection: Selection,
    shredded: bool,
    report_id: Option<String>,
    window: Window,
    gathered: Option<Gathered>,
    judged: Judged,
    /// The records the report named as retirable, by key, with the
    /// position that made each: a tail write that changes one stales it.
    named: BTreeMap<(TypeName, RecordKey), Position>,
    /// The records still to retire, in type then key order.
    retire: BTreeMap<(TypeName, RecordKey), StoredRecord>,
    /// The closes the act owes, in the order the walk found them.
    closes: VecDeque<EventBody>,
    /// The keys the tail moved past the walk, read again under the writer
    /// in the first turn.
    moved: BTreeSet<(TypeName, RecordKey)>,
    /// The tail's events, judged in the first turn.
    tail: Vec<Event>,
    /// The acts the walk knew: one written since is judged again.
    walked: BTreeSet<Position>,
    /// The walk's report, carried past the first turn and read again only
    /// where the tail moved what it states (099).
    report: Option<RemoveReport>,
    /// The ontology the walk's report was made under: a declaration the
    /// tail's catch-up took since is not one it stands on.
    walk_ontology: Option<Ontology>,
    /// Whether the walk found the source forgotten, as its report states.
    walk_forgot: bool,
    /// Whether the tail holds an event of the source, which moves the
    /// walk's tally of its events, undelivered and unseen.
    tail_of_source: bool,
    /// The position of the act's first event; `None` until it is in the log.
    act: Option<Position>,
    /// The versions and the apart keys the act destroys.
    keys: Vec<KeyId>,
    apart: Vec<KeyId>,
    /// The last position the act wrote.
    position: Position,
    own: BTreeSet<Position>,
    /// The restate the walk planned off the writer, where the act's last
    /// turn finds it; `None` where a turn built one instead.
    ahead: Option<crate::lake::scrub::Restate>,
    /// The restate under way, a page a turn (099).
    restate: Option<crate::lake::scrub::Restate>,
    answer: Option<tokio::sync::oneshot::Sender<Reported>>,
}

/// The removes the log and [`REMOVED`] leave owed: every act that is not
/// whole, taken up again by the next writer's open.
pub(crate) fn owed(lake: &Lake, view: &dyn ProjectionView) -> Result<Vec<Box<dyn Job>>> {
    let list = removed_list(view, &Staged::default())?;
    let head = view.position();
    let mut owing: Vec<Box<dyn Job>> = Vec::new();
    for act in acts(lake, view)? {
        let Some(state) = list.get(&act.at) else {
            continue;
        };
        // An erase is one event, whole as it lands (115).
        let Some(source) = act.source.clone().filter(|_| !act.erases()) else {
            continue;
        };
        if state == "done" || whole(lake, act.at, head)? {
            continue;
        }
        owing.push(Box::new(Remove {
            source,
            selection: act.selection.clone(),
            shredded: false,
            report_id: None,
            window: Window::Act,
            gathered: None,
            judged: Judged::default(),
            named: BTreeMap::new(),
            retire: BTreeMap::new(),
            closes: VecDeque::new(),
            moved: BTreeSet::new(),
            tail: Vec::new(),
            walked: BTreeSet::new(),
            report: None,
            walk_ontology: None,
            walk_forgot: false,
            tail_of_source: false,
            act: Some(act.at),
            keys: act.keys.clone(),
            apart: act.apart.clone(),
            position: act.at,
            own: BTreeSet::new(),
            ahead: None,
            restate: None,
            answer: None,
        }));
    }
    Ok(owing)
}

/// The restate and reclaim a whole act owes, and what a crashed forgetting
/// left: one job where any act or source is not done, taken up by the next
/// writer's open, since the open no longer finishes one (099).
pub(crate) fn settle_owed(lake: &Lake, view: &dyn ProjectionView) -> Result<Vec<Box<dyn Job>>> {
    let list = crate::lake::forgotten_list(view, &Staged::default())?;
    let acts = removed_list(view, &Staged::default())?;
    let head = view.position();
    let mut pending = list.values().any(|s| s != "done");
    for (at, state) in &acts {
        // An act between its parts is the road's remove, not this: only a
        // whole act's restate and reclaim, or a rebuild's, is settled here.
        pending |= state != "done" && (state == "rebuilt" || whole(lake, *at, head)?);
    }
    Ok(match pending {
        true => vec![Box::new(Settle { restate: None })],
        false => Vec::new(),
    })
}

/// The job that finishes what a log left owed: the restate and the
/// reclaim, once the act is whole.
struct Settle {
    /// The restate the walk planned, a page a turn.
    restate: Option<crate::lake::scrub::Restate>,
}

impl Job for Settle {
    fn reach(&self) -> Reach {
        Reach::default()
    }

    fn walk(&mut self, lake: &Lake, view: &dyn ProjectionView) -> Result<()> {
        self.restate = lake.restate_owed(view)?;
        Ok(())
    }

    fn fold(&mut self, _tail: &[Event]) -> Result<()> {
        Ok(())
    }

    fn aside(&mut self, lake: &Lake) -> Result<()> {
        self.restate.as_mut().map_or(Ok(()), |r| r.aside(lake))
    }

    fn turn(&mut self, t: &Turn<'_>, w: &mut Writer) -> Result<Turned> {
        let lake: &Lake = t;
        let mut restate = match self.restate.take() {
            Some(restate) => restate,
            // A turn between the walk and this one left a restate owed:
            // plan it here, and take it a page a turn like any (099).
            None => {
                let view = lake.engine.view(None)?;
                let owed = lake.restate_owed(view.as_ref())?;
                drop(view);
                match owed {
                    Some(restate) => restate,
                    None => return Ok(Turned::Whole),
                }
            }
        };
        match restate.step(lake, w)? {
            crate::lake::scrub::Stepped::More => {
                self.restate = Some(restate);
                Ok(Turned::More)
            }
            crate::lake::scrub::Stepped::Done(_) => Ok(Turned::Whole),
        }
    }
}

impl Job for Remove {
    /// What of the tail reaches the act: its source's writes, and every
    /// write about a record of a type it reached, which another hand's
    /// write on one of its records is.
    fn reach(&self) -> Reach {
        let types = (self.gathered.iter())
            .flat_map(|g| g.reached.iter().map(|(t, _)| t.clone()))
            .collect::<BTreeSet<_>>();
        let keys = (self.gathered.iter())
            .flat_map(|g| g.reached.iter().map(|(_, k)| k.clone()))
            .collect();
        Reach {
            sources: vec![self.source.clone()],
            types: types.into_iter().collect(),
            keys,
        }
    }

    /// The walk, off the writer: gather the selection at the view, judge
    /// the records it reached, and hold the report away from the
    /// projection's own.
    fn walk(&mut self, lake: &Lake, view: &dyn ProjectionView) -> Result<()> {
        let ontology = Ontology::at(view)?;
        let head = view.position();
        let all = acts(lake, view)?;
        self.walked = all.iter().map(|r| r.at).collect();
        // An act the log already began, for this selection, is taken up
        // again, whatever report the caller names.
        if self.act.is_none()
            && let Some(at) = self.owing(&all, view)?
        {
            self.window = Window::Act;
            self.act = Some(at);
        }
        let (at, earlier) = match self.act {
            Some(at) => (
                at,
                all.iter()
                    .filter(|r| r.at < at)
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
            None => (head.map_or(Position::GENESIS, Position::next), all),
        };
        let g = Gathered::gather(
            lake,
            view,
            &ontology,
            (&self.source, &self.selection),
            (at, &earlier),
        )?;
        let judged = Judged::of(lake, view, &ontology, &g)?;
        if self.act.is_none()
            && let Some(id) = &self.report_id
        {
            let window = window_of(id)?;
            if let Window::Report { through, digest } = &window {
                if *through > head.unwrap_or(Position::GENESIS) {
                    return Err(Error::invalid(Case::RemoveReportStale));
                }
                let past = lake.engine.view(Some(*through))?;
                let past_ontology = Ontology::at(past.as_ref())?;
                let past_acts = acts(lake, past.as_ref())?;
                let earlier: Vec<Removal> =
                    past_acts.into_iter().filter(|r| r.at < *through).collect();
                let past_g = Gathered::gather(
                    lake,
                    past.as_ref(),
                    &past_ontology,
                    (&self.source, &self.selection),
                    (through.next(), &earlier),
                )?;
                let past_j = Judged::of(lake, past.as_ref(), &past_ontology, &past_g)?;
                let id = token(&self.source, &self.selection, *through, &past_j.made);
                if id.split_once('-').map(|(_, d)| d) != Some(digest.as_str()) {
                    return Err(Error::invalid(Case::RemoveReportStale));
                }
                self.named = (past_j.retirable())
                    .map(|h| ((h.r.ty.clone(), h.r.key.clone()), h.r.made))
                    .collect();
                self.window = window;
            }
        }
        let (keys, apart) = g.keys(lake)?;
        let forgot =
            crate::lake::forgotten_list(view, &Staged::default())?.contains_key(&self.source);
        // The closes the act owes, found off the writer: the first turn
        // appends them, and reads them again only where the tail moved
        // what they rest on (099).
        let (shut, quarantined) = closes(lake, view, &self.source, &self.selection, &g)?;
        let report = report_of(
            lake,
            (&lake.reading(view)?, view),
            &ontology,
            &g,
            &judged,
            (&keys, &apart),
            forgot,
            quarantined,
            head,
        )?;
        self.retire = (judged.retirable())
            .map(|h| ((h.r.ty.clone(), h.r.key.clone()), h.r.clone()))
            .collect();
        self.keys = keys;
        self.apart = apart;
        self.closes = shut.into();
        self.report = Some(report);
        self.walk_ontology = Some(ontology);
        self.walk_forgot = forgot;
        self.judged = judged;
        self.gathered = Some(g);
        // The restate the act leaves, planned here off the writer with
        // the act as this walk judges it (099).
        self.ahead = Some(lake.restate_for_act(view, &self.removal())?);
        Ok(())
    }

    /// The tail past the walk's view, held for the first turn, and the
    /// keys it moved, which the first turn reads again under the writer.
    fn fold(&mut self, tail: &[Event]) -> Result<()> {
        for e in tail {
            // An event of the source moves the tally the walk made of its
            // events, undelivered and unseen, and may close a quarantine
            // the walk did not see (099).
            self.tail_of_source |= e.source.as_ref() == Some(&self.source);
            for about in lakelands_core::feed::about(&e.body) {
                if let (Some(ty), Some(key)) = (about.ty, about.key) {
                    self.moved.insert((ty, key));
                }
            }
            self.tail.push(e.clone());
        }
        Ok(())
    }

    fn aside(&mut self, lake: &Lake) -> Result<()> {
        self.restate.as_mut().map_or(Ok(()), |r| r.aside(lake))
    }

    fn turn(&mut self, t: &Turn<'_>, w: &mut Writer) -> Result<Turned> {
        let turned = self.one_turn(t, w)?;
        // What the turn's append and the catch-up the writer took before
        // it left owed, which its caller settles.
        self.own.extend(owing_now());
        Ok(turned)
    }

    fn end(&mut self, ended: Result<()>) {
        let Some(answer) = self.answer.take() else {
            return;
        };
        let answered = ended.map(|()| {
            let report = self.report.take().expect("a whole remove has its report");
            (report, std::mem::take(&mut self.own))
        });
        let _ = answer.send(answered);
    }
}

impl Remove {
    /// An act the log began for this selection and has not ended, if there
    /// is one: the walk and the first turn both look, since a projection
    /// the writer had not caught up yet hides it from the walk.
    fn owing(&self, all: &[Removal], view: &dyn ProjectionView) -> Result<Option<Position>> {
        let list = removed_list(view, &Staged::default())?;
        for act in all {
            if act.source.as_ref() != Some(&self.source) || act.selection != self.selection {
                continue;
            }
            // An act the list does not call done is taken up: one between
            // its parts appends the rest, and one whole whose restate or
            // reclaim a crash left is settled by its repeat (099).
            let owed = list.get(&act.at).is_some_and(|s| s != "done");
            if owed {
                return Ok(Some(act.at));
            }
        }
        Ok(None)
    }

    /// Whether a record the report named is no longer what it was: gone,
    /// made again, or now held by another hand. A selected write of the
    /// source is taken in, not stale.
    fn stale(&self) -> bool {
        (self.named.iter()).any(|((ty, key), made)| {
            (self.judged.holders.get(ty).and_then(|h| h.get(key)))
                .is_none_or(|h| h.holds || h.r.made != *made)
        })
    }

    /// Whether the walk's report and closes stand as the walk read them:
    /// nothing the tail or another hand wrote since reaches what they
    /// state, so only what the writer states alone — the log's head, the
    /// act's expected time, the reclaim owed — is read (099).
    ///
    /// The keys the report and the closes name are the walk's where no
    /// event of the source is in the tail: only such an event moves the
    /// selection's tally, and only the act's own steps move the key store,
    /// so an act the walk has not seen is the new-act branch above (099).
    fn stands(
        &self,
        ontology: &Ontology,
        g: &Gathered,
        forgot: bool,
        changed: bool,
        moved: &BTreeSet<(TypeName, RecordKey)>,
    ) -> bool {
        let Some(report) = self.report.as_ref() else {
            return false;
        };
        !changed
            && !self.tail_of_source
            // The walk read a reader's ontology and the writer holds its
            // own, which carries the tool names: what must agree is the
            // declarations, not those (099).
            && self.walk_ontology.as_ref().is_some_and(|o| o.same(ontology))
            && forgot == self.walk_forgot
            && g.before == report.before
            // A record another hand wrote moves the report where a type can
            // name another record's: `references` is read from the live
            // records, and a tail that moved any is read again whole.
            && (moved.is_empty() || !relating(ontology))
    }

    /// One turn beside the writer (099): the act's first event, a part of
    /// its retirements, or its restate.
    fn one_turn(&mut self, t: &Turn<'_>, w: &mut Writer) -> Result<Turned> {
        let lake: &Lake = t;
        // A turn of the restate the act left: one page of the projection,
        // the writer handed back between pages (099).
        if self.restate.is_some() {
            // The act's appends reached the last position it wrote; a
            // page after them reports a failure through that.
            return self.restate_turn(t, w, self.position);
        }
        let view = lake.engine.view(None)?;
        // The writer's catch-up may have left the projection holding an act
        // the walk off it did not see: take that act up before beginning
        // another.
        if self.act.is_none()
            && let Some(at) = self.owing(&acts(lake, view.as_ref())?, view.as_ref())?
        {
            self.window = Window::Act;
            self.act = Some(at);
            self.gathered = None;
        }
        // The act's whole lies past the last position it wrote: only an act
        // this handle has not written a turn of reads from its start.
        let is_whole = match self.act {
            Some(at) => {
                let from = match self.position >= at {
                    true => self.position.next(),
                    false => at,
                };
                whole_from(lake, at, from, view.position())?
            }
            None => false,
        };
        if is_whole {
            // The appends ended in a turn this handle did not run: the
            // restate and the reclaim are left, answered as owed through
            // the head the act's appends reached.
            let through = view.position().unwrap_or(Position::GENESIS);
            drop(view);
            self.restating(t)?;
            return self.restate_turn(t, w, through);
        }
        let next = match self.act {
            None => self.begin(lake, w, view.as_ref())?,
            Some(_) => self.part_turn(lake, w, view.as_ref())?,
        };
        // The restate and the reclaim rebuild the projection: the view this
        // turn held is let go first.
        drop(view);
        match next {
            Next::Nothing => Ok(Turned::Whole),
            Next::More => Ok(Turned::More),
            Next::Whole => {
                self.restating(t)?;
                // The act is in the log through the last position it
                // wrote, this turn's own appends.
                self.restate_turn(t, w, self.position)
            }
        }
    }

    /// Hold the restate the act leaves: the plan its walk made, with the
    /// records its first turn took in past the walk, or one planned here
    /// where the walk's is gone.
    fn restating(&mut self, lake: &Lake) -> Result<()> {
        if self.restate.is_some() {
            return Ok(());
        }
        let act = self.removal();
        let (mut restate, acts) = match self.ahead.take() {
            // The walk's plan stands where its gather is the act's: what
            // the first turn took in past it is added below.
            Some(ahead) if self.gathered.is_some() => (ahead, vec![act]),
            // An act the walk did not see, taken up by a turn: planned
            // here from the lists, which hold the act as the log does.
            _ => {
                let view = lake.engine.view(None)?;
                let restate = (lake.restate_owed(view.as_ref())?)
                    .unwrap_or_else(crate::lake::scrub::Restate::done);
                (restate, Vec::new())
            }
        };
        let reached = (self.gathered.iter()).flat_map(|g| g.reached.iter().cloned());
        restate.absorb(reached, acts, &self.tail);
        self.restate = Some(restate);
        Ok(())
    }

    /// The records the act retires, as the walk judged them: the parts
    /// take them in this order, each read again at its live version at the
    /// part ([`crate::parts::part`]).
    fn taken(&self) -> BTreeMap<(TypeName, RecordKey), StoredRecord> {
        (self.judged.retirable())
            .map(|h| ((h.r.ty.clone(), h.r.key.clone()), h.r.clone()))
            .collect()
    }

    /// The act as the walk judged it, for the restate's plan: what the
    /// source's events before it select, and the keys it destroys.
    fn removal(&self) -> Removal {
        let (at, before) = match &self.gathered {
            Some(g) => (g.at, g.before),
            None => (self.act.unwrap_or(self.position), None),
        };
        Removal {
            at,
            source: Some(self.source.clone()),
            selection: self.selection.clone(),
            before,
            keys: self.keys.clone(),
            apart: self.apart.clone(),
            fixed: Fixed::default(),
            chain: None,
        }
    }

    /// One turn of the restate, a page of the projection: the writer is
    /// handed back between pages. A failure says the act is done in the
    /// log and names the turn that could not finish it.
    fn restate_turn(&mut self, t: &Turn<'_>, w: &mut Writer, at: Position) -> Result<Turned> {
        let lake: &Lake = t;
        let mut restate = self
            .restate
            .take()
            .expect("a restate turn holds its restate");
        match lake.step_act(w, &mut restate, at)? {
            crate::lake::scrub::Stepped::More => {
                self.restate = Some(restate);
                Ok(Turned::More)
            }
            crate::lake::scrub::Stepped::Done(reclaimed) => {
                self.answered(lake, reclaimed);
                Ok(Turned::Whole)
            }
        }
    }

    /// The report as the restate and the reclaim leave it.
    fn answered(&mut self, lake: &Lake, reclaimed: Option<lakelands_api::write::CompactReport>) {
        let mut report = self.report.take().unwrap_or_else(|| RemoveReport {
            source: self.source.clone(),
            selection: self.selection.clone(),
            before: None,
            events: 0,
            undelivered: 0,
            unseen: 0,
            types: Vec::new(),
            fields: Vec::new(),
            retired: Vec::new(),
            kept: Vec::new(),
            recomputed: Vec::new(),
            views: Vec::new(),
            breaches: Vec::new(),
            references: Vec::new(),
            keys: self.keys.clone(),
            apart: self.apart.clone(),
            unshredded: 0,
            log_events: 0,
            expected_ms: 0,
            reclaim_untimed: lake.untimed(),
            position: self.position,
            reclaimed: None,
            reclaim_owed: lake.reclaim_owed(),
            report_id: String::new(),
            quarantined: Vec::new(),
        });
        report.reclaimed = reclaimed;
        report.reclaim_owed = lake.reclaim_owed();
        report.position = report.position.max(self.position);
        self.report = Some(report);
    }

    /// The act's first turn: the tail folded into the selection, the keys
    /// judged again, `values_removed` and the first part appended, and the
    /// keys the act destroys destroyed.
    fn begin(&mut self, lake: &Lake, w: &mut Writer, view: &dyn ProjectionView) -> Result<Next> {
        let all = acts(lake, view)?;
        let at = writes::position_after(w, 0);
        // An act of this source written since the walk changes what this
        // one leaves out: judge the selection again.
        if all.iter().any(|r| !self.walked.contains(&r.at)) {
            let ontology = Ontology::at(view)?;
            let earlier: Vec<Removal> = all.iter().filter(|r| r.at < at).cloned().collect();
            let g = Gathered::gather(
                lake,
                view,
                &ontology,
                (&self.source, &self.selection),
                (at, &earlier),
            )?;
            self.judged = Judged::of(lake, view, &ontology, &g)?;
            self.gathered = Some(g);
            let (keys, apart) = self
                .gathered
                .as_ref()
                .expect("the selection was gathered")
                .keys(lake)?;
            let (shut, quarantined) = closes(
                lake,
                view,
                &self.source,
                &self.selection,
                self.gathered.as_ref().unwrap(),
            )?;
            let forgot =
                crate::lake::forgotten_list(view, &Staged::default())?.contains_key(&self.source);
            self.report = Some(report_of(
                lake,
                (&*w, view),
                &ontology,
                self.gathered.as_ref().expect("the selection was gathered"),
                &self.judged,
                (&keys, &apart),
                forgot,
                quarantined,
                view.position(),
            )?);
            self.keys = keys;
            self.apart = apart;
            self.closes = shut.into();
            self.retire = self.taken();
            // This is the plan the turns carry from: the act it names is
            // acted as this turn's (099).
            self.walk_ontology = Some(ontology);
            self.walk_forgot = forgot;
            // The walk's plan was made with the walk's act: an act of the
            // source since it is another plan, and the restate plans again
            // at this turn (099).
            self.ahead = None;
        }
        let moved = std::mem::take(&mut self.moved);
        if let Some(g) = self.gathered.as_mut() {
            g.at = at;
            // `known_at` never falls, so a time's cut moves forward over the
            // tail the first turn reads and never back (099). It is the
            // writer's view that bounds it: the tail was read through that
            // view, and what stands between it and a head not yet published
            // is nothing this act may leave out.
            if let Selection::WrittenBefore(t) = &g.selection {
                let t = *t;
                g.before = Some(
                    match lake.position_through(Timestamp(t.0 - 1), view.position())? {
                        Some(p) => p.next(),
                        None => Position::GENESIS,
                    },
                );
            }
            for e in &self.tail {
                g.event(&w.ontology, e);
            }
        }
        {
            let g = self.gathered.as_ref().expect("the walk gathered");
            // The walk read the keys from the tally and the key store, off
            // the writer. Only an event of the source in the tail moves the
            // tally, and only the act's own steps move the store, so a tail
            // of other hands' writes leaves them as the walk read them; a
            // tail of the source's own is read again (099).
            let (keys, apart) = match self.tail_of_source {
                false => (self.keys.clone(), self.apart.clone()),
                true => g.keys(lake)?,
            };
            let forgot =
                crate::lake::forgotten_list(view, &Staged::default())?.contains_key(&self.source);
            let changed = self.judged.again(lake, view, &w.ontology, g, &moved)?;
            if !self.named.is_empty() && self.stale() {
                return Err(Error::invalid(Case::RemoveReportStale));
            }
            // The walk's report and closes stand where nothing since
            // reaches what they state, and only what the writer states is
            // read again; a tail that moved either reads them whole (099).
            let carried = match self.stands(&w.ontology, g, forgot, changed, &moved) {
                true => self.report.take(),
                false => None,
            };
            let report = match carried {
                Some(mut report) => {
                    let log_events = view.position().map_or(0, |p| p.0);
                    report.log_events = log_events;
                    report.expected_ms = lake.expected_ms(log_events);
                    report.reclaim_untimed = lake.untimed();
                    report.reclaim_owed = lake.reclaim_owed();
                    report
                }
                None => {
                    let (shut, quarantined) = closes(lake, view, &self.source, &self.selection, g)?;
                    self.closes = shut.into();
                    self.retire = self.taken();
                    // The report is read again from the records as they
                    // stand: a tail event the act takes changes which
                    // records it retires, what is computed from them and
                    // what they strand, not only the tally (099).
                    report_of(
                        lake,
                        (w, view),
                        &w.ontology,
                        g,
                        &self.judged,
                        (&keys, &apart),
                        forgot,
                        quarantined,
                        view.position(),
                    )?
                }
            };
            if self.shredded && report.unshredded > 0 {
                let (source, count) = (self.source.clone(), report.unshredded);
                return Err(Error::invalid(Case::RemoveNotShredded { source, count }));
            }
            // A tail that moves the versions the act destroys moves the
            // records and outputs a scoped plan must fold: the walk's plan
            // is not this act's, and the restate plans again at this turn
            // (099).
            if keys != self.keys || apart != self.apart {
                self.ahead = None;
            }
            self.keys = keys;
            self.apart = apart;
            self.report = Some(report);
        }
        let empty = {
            let g = self.gathered.as_ref().expect("the walk gathered");
            g.events == 0 && self.retire.is_empty() && self.closes.is_empty()
        };
        if empty {
            let mut report = self.report.take().expect("the walk made the report");
            report.position = view.position().unwrap_or(Position::GENESIS);
            self.report = Some(report);
            return Ok(Next::Nothing);
        }
        let (keys, apart) = (self.keys.clone(), self.apart.clone());
        let before = self.gathered.as_ref().expect("the walk gathered").before;
        let now = lake.clock(w);
        let room = usize::try_from(lake.limits.part_events.max(1)).unwrap_or(usize::MAX);
        self.act = Some(at);
        let mut drafts = vec![Draft::system(
            now,
            EventBody::ValuesRemoved {
                source: self.source.clone(),
                selection: self.selection.clone(),
                before,
                keys: keys.clone(),
                apart: apart.clone(),
            },
        )];
        let fit = crate::parts::take_closes(&mut self.closes, room);
        drafts.extend(fit.into_iter().map(|c| Draft::system(now, c)));
        let (mut part, whole) = self.fill(lake, (w, view), now, room, drafts)?;
        lake.append(&mut *w, std::mem::take(&mut part))?;
        self.position = w.tip.as_ref().map_or(Position::GENESIS, |t| t.0);
        self.act = Some(at);
        // The keys the act destroys go in its first turn, so nothing a
        // write seals after it is sealed under one (099).
        lake.keys.destroy_versions(&self.source, &keys)?;
        lake.keys.destroy_apart(&self.source, &apart)?;
        if let Some(report) = self.report.as_mut() {
            report.position = self.position;
        }
        if whole {
            return Ok(Next::Whole);
        }
        Ok(Next::More)
    }

    /// A later turn of an act the log began: a part of the retirements and
    /// closes still owed, each read again as it stands now.
    fn part_turn(
        &mut self,
        lake: &Lake,
        w: &mut Writer,
        view: &dyn ProjectionView,
    ) -> Result<Next> {
        let at = self.act.expect("a part follows the act's beginning");
        if self.gathered.is_none() {
            // Taken up under the writer: the walk off it did not see the act.
            let ontology = Ontology::at(view)?;
            let all = acts(lake, view)?;
            let earlier: Vec<Removal> = all.iter().filter(|r| r.at < at).cloned().collect();
            let g = Gathered::gather(
                lake,
                view,
                &ontology,
                (&self.source, &self.selection),
                (at, &earlier),
            )?;
            self.judged = Judged::of(lake, view, &ontology, &g)?;
            let (shut, quarantined) = closes(lake, view, &self.source, &self.selection, &g)?;
            let (keys, apart) = g.keys(lake)?;
            let forgot =
                crate::lake::forgotten_list(view, &Staged::default())?.contains_key(&self.source);
            self.report = Some(report_of(
                lake,
                (&*w, view),
                &ontology,
                &g,
                &self.judged,
                (&keys, &apart),
                forgot,
                quarantined,
                view.position(),
            )?);
            self.closes = shut.into();
            self.keys = keys;
            self.apart = apart;
            self.retire = self.taken();
            self.walk_ontology = Some(ontology);
            self.walk_forgot = forgot;
            self.gathered = Some(g);
        }
        // The retirements were taken once, at the turn that judged them, and
        // [`crate::parts::part`] maintains them: each record it takes is
        // dropped and read again at its live version, as this turn would (099).
        let now = lake.clock(w);
        let room = usize::try_from(lake.limits.part_events.max(1)).unwrap_or(usize::MAX);
        let (mut part, whole) = self.fill(lake, (w, view), now, room, Vec::new())?;
        lake.append(&mut *w, std::mem::take(&mut part))?;
        self.position = w.tip.as_ref().map_or(Position::GENESIS, |t| t.0);
        if let Some(report) = self.report.as_mut() {
            report.position = self.position;
        }
        if whole {
            return Ok(Next::Whole);
        }
        Ok(Next::More)
    }

    /// One part of the act, by the rule a purge's parts share with it
    /// ([`crate::parts::part`]).
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
                kind: lakelands_api::event::ChangeKind::Retired,
                act,
            },
        )
    }
}

/// Send a remove of `source` along the lake's road, answering `answer`
/// where the caller waits: the store's door, which holds no writer lock
/// while the act runs.
pub(crate) fn start(
    lake: &std::sync::Arc<Lake>,
    source: SourceId,
    selection: Selection,
    shredded: bool,
    report_id: Option<String>,
    answer: Option<tokio::sync::oneshot::Sender<Reported>>,
) {
    let job = Remove {
        source,
        selection,
        shredded,
        report_id,
        window: Window::Whole,
        gathered: None,
        judged: Judged::default(),
        named: BTreeMap::new(),
        retire: BTreeMap::new(),
        closes: VecDeque::new(),
        moved: BTreeSet::new(),
        tail: Vec::new(),
        walked: BTreeSet::new(),
        report: None,
        walk_ontology: None,
        walk_forgot: false,
        tail_of_source: false,
        act: None,
        keys: Vec::new(),
        apart: Vec::new(),
        position: Position::GENESIS,
        own: BTreeSet::new(),
        ahead: None,
        restate: None,
        answer,
    };
    crate::road::send(lake, Box::new(job));
}
