//! A view the fold keeps (lakelands#575): its records are records in the
//! projection, one for each group, put by the fold under a name of the
//! store's own that no type can bear, and a read of the view answers
//! them where the view's state says they are the computation's own. A
//! kept record is exactly what the read would compute, or the state says
//! at that position that it is not, and the read computes.

use std::collections::{BTreeMap, BTreeSet};

use std::ops::Bound::{Excluded, Included, Unbounded};

use lakelands_api::canonical::hex;
use lakelands_api::engine::{After, ProjectionStore, ProjectionView, Scan};
use lakelands_api::event::{Event, EventBody};
use lakelands_api::expr::{Condition, Expr, Func, Literal};
use lakelands_api::ontology::{Measure, TypeDecl, ViewDecl};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::seam::{VIEW_KEPT_SETTING as SETTING, kept_name, order_name};
use lakelands_api::write::Declaration;
use lakelands_api::{FieldName, Position, RecordKey, Result, Timestamp, TypeName};
use lakelands_core::expr::aggregate::{Entry, Folds, Grouping, Keeping, KeptFolds, Row};
use lakelands_core::expr::{self, Place};
use serde::{Deserialize, Serialize};

use crate::declare::condition;
use crate::lake::Staged;
use crate::ontology::Ontology;

/// The seam name a kept view's state is recorded under, one output for
/// each view: the store's own, which no seam records under.
pub(crate) const KEPT: &str = "sys:kept";

/// The field of a kept record that holds its folds, for each face, and
/// the whole face's fields where they differ from the record's own, or
/// that the whole face has none. An object, so the engines keep no text
/// row for it; never answered.
pub(crate) const FOLDS: &str = "sys:folds";

/// The field that marks a kept record whose whole face reads otherwise
/// than its own fields: a text, so the lookup finds the members of a view
/// over this one that a group has on the whole face alone.
pub(crate) const DIFFERS: &str = "sys:differs";

/// The member of [`FOLDS`] that holds the whole face's fields.
pub(crate) const WHOLE: &str = "whole";

/// The member of [`FOLDS`] that says the group is on the open face alone.
pub(crate) const ALONE: &str = "alone";

/// The field of an entry of an extreme's order that holds it ([`Entry`]):
/// an object, so the engines keep no text row for it.
const ENTRY: &str = "sys:entry";

/// The entries of one view's orders as a transaction leaves them, by key:
/// an entry's JSON text, none for one ended. Each extreme of each source
/// has an order for each group, whose entries begin [`within`]; after
/// that, a member's place in it and its key. On the open face every
/// member has an entry under `o`. On the whole face a member has one under
/// `w`, unless its `o` entry is the same: a source's record is the same
/// member on both faces but where it is restricted, and a view's record
/// may read otherwise on its whole face, so it has both. Where the
/// transaction ended the name as a whole, the engine's entries are not
/// read, and these are all it holds.
#[derive(Default)]
pub(crate) struct Orders {
    ended: bool,
    entries: Entries,
}

/// Entries by key: an entry's JSON text, none for one ended.
type Entries = BTreeMap<Box<str>, Option<Box<str>>>;

/// Where the entries of the `j`th call of the source `i` begin, for the
/// group at `group` and one face's letter: its key's digest, so that an
/// entry's key is short whatever the group's is.
fn within(i: usize, j: usize, group: &RecordKey, letter: char) -> String {
    let digest = crate::backed::digest(&Value::from(group.as_str())).0;
    format!("{i}.{j}.{}.{letter}.", &digest[..32])
}

/// What the store records of a kept view at a position: counters, and
/// nothing that grows with the view.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct State {
    /// Whether the view's records are whole: made, and brought up to date
    /// by every event since.
    pub whole: bool,
    /// The groups that are not exact, on the open face and on the whole
    /// one: a measure that fails, a sum outside what is exact, a member
    /// that does not fold.
    pub not_exact: [u64; 2],
    /// The groups whose whole face reads otherwise than their open one.
    pub differ: u64,
}

/// The state of `view` at the position `at` reads, if the fold kept one.
pub(crate) fn state(at: &dyn ProjectionView, view: &TypeName) -> Result<Option<State>> {
    let input = crate::backed::digest(&Value::from(view.as_str()));
    let recorded = at.output(KEPT, &input)?.map(|(_, v)| v);
    Ok(recorded.and_then(|s| serde_json::from_value(s).ok()))
}

/// A kept record as the view's own, for a reader of the open face or, when
/// `whole`, of the whole one: under the view's name, without what the
/// store keeps beside the group's fields. None for the whole face's reader
/// where the group is on the open face alone.
pub(crate) fn shown(mut r: StoredRecord, view: &TypeName, whole: bool) -> Option<StoredRecord> {
    let folds = r.fields.remove(FOLDS);
    r.fields.remove(DIFFERS);
    let folds = folds.as_ref().filter(|_| whole);
    if folds.and_then(|f| f.get(ALONE)) == Some(&Value::Bool(true)) {
        return None;
    }
    let face = folds.and_then(|f| f.get(WHOLE));
    if let Some(fields) = face.and_then(|f| serde_json::from_value(f.clone()).ok()) {
        r.fields = fields;
    }
    (r.ty, r.restricted) = (view.clone(), false);
    Some(r)
}

/// One source of a kept view as the fold reads it: its own or a merged
/// one, with its `where`, its grouping and the calls its measures make.
struct Src {
    ty: TypeName,
    /// Whether the source is itself a view: its members are its kept
    /// records.
    view: bool,
    filter: Option<Expr>,
    group_by: Vec<FieldName>,
    measures: Vec<(FieldName, Expr)>,
    keeping: Keeping,
}

/// A view as the fold keeps it under the declarations in force.
struct Kept {
    /// The view's declaration, keyed by its group fields: the one key
    /// rule makes a group's key from its values.
    decl: TypeDecl,
    /// Its own source first, then the merged ones as declared.
    sources: Vec<Src>,
    /// The settings its expressions name, each by a text: a declared
    /// expression names one no other way.
    settings: BTreeSet<String>,
}

impl Kept {
    /// `decl` as the fold would keep it; none for what is no view, or
    /// one `keepable` refuses.
    fn of(o: &Ontology, decl: &TypeDecl) -> Option<Kept> {
        let v = decl.view.as_deref()?;
        lakelands_api::expr::keepable(v, |t| o.ty(t).ok()).ok()?;
        let mut settings = BTreeSet::new();
        let mut source =
            |ty: &TypeName, filter: Option<&Condition>, by: &[FieldName], m: &[Measure]| {
                let filter = filter.map(|w| condition(w, Place::View, &[]));
                let filter = filter.transpose().ok()?;
                let measured =
                    |m: &Measure| Ok((m.name.clone(), condition(&m.expr, Place::Group, &[])?));
                let measures: Result<Vec<(FieldName, Expr)>> = m.iter().map(measured).collect();
                let measures = measures.ok()?;
                let read = filter.iter().chain(measures.iter().map(|(_, e)| e));
                read.for_each(|e| settings_named(e, &mut settings));
                Some(Src {
                    ty: ty.clone(),
                    view: o.ty(ty).is_ok_and(|t| t.view.is_some()),
                    filter,
                    group_by: by.to_vec(),
                    keeping: Keeping::of(&measures),
                    measures,
                })
            };
        let own = source(&v.source, v.r#where.as_ref(), &v.group_by, &v.measures)?;
        let mut sources = vec![own];
        for m in &v.merge {
            let merged = source(&m.source, m.r#where.as_ref(), &m.group_by, &m.measures)?;
            sources.push(merged);
        }
        let key = v.group_by.clone();
        let decl = TypeDecl {
            key,
            ..decl.clone()
        };
        Some(Kept {
            decl,
            sources,
            settings,
        })
    }

    fn view(&self) -> &ViewDecl {
        self.decl.view.as_deref().expect("a kept view is a view")
    }

    /// The key of the group with the values `by`, under the view's own
    /// group fields: the reserved one for a view without any, none for a
    /// null value or for values the key rule makes no key from, as the
    /// computation has it.
    fn key(&self, by: &Row) -> Option<RecordKey> {
        if self.view().group_by.is_empty() {
            return Some(RecordKey::from(lakelands_api::key::UNGROUPED));
        }
        lakelands_api::key::record_key(&self.decl, by)
    }
}

/// Whether the computation of the view `decl` names a setting, kept as
/// the fold would keep it.
pub(crate) fn names_settings(o: &Ontology, decl: &TypeDecl) -> bool {
    Kept::of(o, decl).is_some_and(|k| !k.settings.is_empty())
}

/// The settings the computation of the view `decl` names, kept as the
/// fold would keep it.
pub(crate) fn settings_read(o: &Ontology, decl: &TypeDecl) -> BTreeSet<String> {
    Kept::of(o, decl).map(|k| k.settings).unwrap_or_default()
}

/// The settings `e` names.
pub(crate) fn settings_named(e: &Expr, named: &mut BTreeSet<String>) {
    if let Expr::Call {
        func: Func::Setting,
        args,
    } = e
        && let Some(Expr::Lit {
            value: Literal::Text(key),
        }) = args.first()
    {
        named.insert(key.clone());
    }
    lakelands_api::expr::children(e).for_each(|c| settings_named(c, named));
}

/// What a kept record carries beside its group's fields, in [`FOLDS`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Carried {
    /// The open face's folds, one for each source.
    open: Vec<KeptFolds>,
    /// The whole face's, where a restricted member makes them differ.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    all: Option<Vec<KeptFolds>>,
    /// Whether the open face and the whole one are not exact.
    #[serde(default)]
    flagged: [bool; 2],
    /// The whole face's fields, where both faces have a record and they
    /// differ.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    whole: Option<Row>,
    /// Whether the group is on the open face alone: a member of a source
    /// that is a view can be one there and none on the whole face, where
    /// its `where` reads another record of the same group.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    alone: bool,
}

impl Carried {
    /// Whether the whole face reads the group otherwise than the open one.
    fn differs(&self) -> bool {
        self.whole.is_some() || self.alone
    }
}

/// One group of a kept view while a transaction works on it.
struct Group {
    by: Row,
    /// The folds of the open face and of the whole one, each one for each
    /// source.
    folds: [Vec<KeptFolds>; 2],
    /// The group's record as the transaction found it or last put it.
    stored: Option<StoredRecord>,
}

/// One kept view while a transaction works on it.
struct Work {
    kept: Kept,
    /// The entries the event's notes ended or put, written at its flush.
    entries: Entries,
    state: State,
    groups: BTreeMap<RecordKey, Group>,
    touched: BTreeSet<RecordKey>,
}

/// What a transaction holds of the views the lake keeps: each view named
/// by a `view_kept.` setting whose state says its records are whole.
#[derive(Default)]
pub(crate) struct Views {
    /// The views the transaction has looked at, kept or not, and the
    /// types whose readers it has looked at: a view is read in when a
    /// record it reads is written, and all of them at a setting's write
    /// or a declaration. What an append does not write costs it nothing.
    looked: BTreeSet<TypeName>,
    noted: BTreeSet<TypeName>,
    views: BTreeMap<TypeName, Work>,
    /// Views whose state says whole and that the lake would not keep as
    /// it stands: ended by the first event the transaction settles.
    orphans: Vec<TypeName>,
    /// The groups put or ended, where the write path asks ([`step`]).
    moved: Option<Moved>,
}

/// The groups of kept views a run of the hooks put or ended, by view.
pub(crate) type Moved = BTreeSet<(TypeName, RecordKey)>;

/// What a run of the hooks in the write path overwrote in its [`Staged`],
/// oldest first: put back where its write is refused, so that the next
/// write runs over the writes accepted before it (lakelands#1185).
#[derive(Default)]
pub(crate) struct Undo(Vec<Undone>);

/// One thing overwritten, as it was: none where there was nothing.
enum Undone {
    Record((TypeName, RecordKey), Option<StoredRecord>),
    Output(
        (String, lakelands_api::event::Digest),
        Option<Vec<(Position, Value)>>,
    ),
    Orders(TypeName, Option<Orders>),
    Entry(TypeName, Box<str>, Option<Option<Box<str>>>),
}

impl Undo {
    /// The record at `at` was `was` before the run put one there.
    pub(crate) fn record(&mut self, at: (TypeName, RecordKey), was: Option<StoredRecord>) {
        self.0.push(Undone::Record(at, was));
    }
}

/// Whether the group at `key` of `view` was flagged, or is flagged, as not
/// exact on the whole face (049), where a measure that fails is among it,
/// after the run since `staged`'s undo began. A flag that stays says not
/// whether the failure it stands for cleared, which a read of the face
/// answers from the members, so a group flagged at either end is decided
/// again, with every group of the sets' views that read it.
pub(crate) fn reflagged(
    at: &At<'_>,
    staged: &Staged,
    (view, key): (&TypeName, &RecordKey),
) -> Result<bool> {
    let place = (kept_name(view), key.clone());
    let Some(was) = was(at, staged, &place)? else {
        return Ok(false);
    };
    let flagged = |r: Option<&StoredRecord>| {
        r.filter(|r| !r.is_ended())
            .is_some_and(|r| carried(r).flagged[1])
    };
    Ok(flagged(was.as_ref()) || flagged(staged.records.get(&place)))
}

/// The record the run since `staged`'s undo began found at `place`
/// before it put one there; `None` where it put none there.
fn was(
    at: &At<'_>,
    staged: &Staged,
    place: &(TypeName, RecordKey),
) -> Result<Option<Option<StoredRecord>>> {
    let Some(Undo(done)) = &staged.undo else {
        return Ok(None);
    };
    let first = done.iter().find_map(|u| match u {
        Undone::Record(p, was) if p == place => Some(was),
        _ => None,
    });
    Ok(match first {
        None => None,
        Some(Some(r)) => Some(Some(r.clone())),
        Some(None) => Some(staged.before(at.view, &place.0, &place.1)?),
    })
}

/// The group at `key` of `view` as the run since `staged`'s undo began
/// found it: whether it had a record on the whole face, and that face,
/// which a trigger on the view reads as the row before (lakelands#1185).
/// The face is none where the fold had flagged the group as not exact, or
/// where a view it reads through holds such a group (049): a read
/// computes that face from members the run has moved since.
pub(crate) fn before(
    at: &At<'_>,
    staged: &Staged,
    (view, key): (&TypeName, &RecordKey),
) -> Result<(bool, Option<StoredRecord>)> {
    let place = (kept_name(view), key.clone());
    let was = match was(at, staged, &place)? {
        Some(was) => was,
        None => staged.record(at.view, &place.0, key)?,
    };
    let Some(was) = was.filter(|r| !r.is_ended()) else {
        return Ok((false, None));
    };
    let flagged = carried(&was).flagged[1];
    let Some(face) = shown(was, view, true) else {
        return Ok((false, None));
    };
    let kept = (at.ontology.ty(view).ok()).and_then(|d| Kept::of(at.ontology, d));
    let below = match &kept {
        Some(kept) => inexact_below(at, staged, kept)?,
        None => false,
    };
    Ok((true, (!flagged && !below).then_some(face)))
}

/// Put back what the run since `staged`'s undo began overwrote, newest
/// first, and keep no more. The views as the run worked on them go too: each is read again
/// from what stays, on first use, as the next transaction reads it.
pub(crate) fn undo(staged: &mut Staged) {
    let Some(Undo(done)) = staged.undo.take() else {
        return;
    };
    for undone in done.into_iter().rev() {
        match undone {
            Undone::Record(at, Some(r)) => drop(staged.records.insert(at, r)),
            Undone::Record(at, None) => drop(staged.records.remove(&at)),
            Undone::Output(at, Some(o)) => drop(staged.outputs.insert(at, o)),
            Undone::Output(at, None) => drop(staged.outputs.remove(&at)),
            Undone::Orders(name, Some(o)) => drop(staged.orders.insert(name, o)),
            Undone::Orders(name, None) => drop(staged.orders.remove(&name)),
            Undone::Entry(name, key, was) => {
                let orders = staged.orders.get_mut(&name).expect("an entry's orders");
                match was {
                    Some(e) => orders.entries.insert(key, e),
                    None => orders.entries.remove(&key),
                };
            }
        }
    }
    staged.kept = Views::default();
}

/// What the fold's hooks are handed: the transaction's view of the
/// projection, the projection it writes, and the lake at the event. The
/// write path runs the hooks over no projection ([`step`]): what they put
/// is held in its [`Staged`] alone.
pub(crate) struct At<'a> {
    pub view: &'a dyn ProjectionView,
    pub projection: Option<&'a dyn ProjectionStore>,
    pub ontology: &'a Ontology,
    pub settings: &'a BTreeMap<String, Value>,
}

/// A member of one source on one face: its group's key and values, and the
/// key, the fields and the provenance it is folded with, a view's record
/// with none.
struct Member {
    group: RecordKey,
    by: Row,
    key: RecordKey,
    fields: Row,
    meta: Option<BTreeMap<FieldName, lakelands_api::FieldMeta>>,
}

impl Member {
    fn keyed(&self) -> (&RecordKey, lakelands_core::expr::aggregate::Member<'_>) {
        (&self.key, (&self.fields, self.meta.as_ref()))
    }
}

impl At<'_> {
    /// The state of `ty` as the transaction leaves it so far.
    fn state(&self, staged: &Staged, ty: &TypeName) -> Result<State> {
        let input = crate::backed::digest(&Value::from(ty.as_str()));
        let held = staged.outputs.get(&(KEPT.to_owned(), input));
        let held = held.and_then(|all| all.last());
        let state = held.and_then(|(_, s)| serde_json::from_value(s.clone()).ok());
        Ok(match state {
            Some(state) => state,
            None => staged.kept_state(self.view, ty)?.unwrap_or_default(),
        })
    }

    /// Put `r` into the projection, where there is one.
    fn put(&self, r: &StoredRecord) -> Result<()> {
        self.projection.map_or(Ok(()), |p| p.put_record(r))
    }

    /// Record `state` for `ty` at `at`.
    fn put_state(
        &self,
        staged: &mut Staged,
        ty: &TypeName,
        state: &State,
        at: Position,
    ) -> Result<()> {
        let input = crate::backed::digest(&Value::from(ty.as_str()));
        let state = serde_json::json!(state);
        if let Some(p) = self.projection {
            p.put_output(KEPT, &input, &state, at)?;
        }
        let key = (KEPT.to_owned(), input);
        let was = (staged.outputs).insert(key.clone(), vec![(at, state)]);
        if let Some(undo) = &mut staged.undo {
            undo.0.push(Undone::Output(key, was));
        }
        Ok(())
    }

    /// Whether `r`, a record of the source `src`, is a member on `face`,
    /// and of which group: live, on the open face unrestricted, its
    /// `where` holding. A source that is a view gives its kept record's
    /// face as it stands: where that face is not exact the inner view's
    /// state says so, and a read through it computes.
    fn member(&self, kept: &Kept, src: &Src, r: &StoredRecord, face: usize) -> Option<Member> {
        if r.retired || (face == 0 && r.restricted) {
            return None;
        }
        let (fields, meta) = match src.view {
            false => (r.fields.clone(), Some(&r.meta)),
            true => (shown(r.clone(), &src.ty, face == 1)?.fields, None),
        };
        let filter = src.filter.as_ref();
        if !filter.is_none_or(|w| crate::reads::holds(w, &fields, meta, self.settings, EPOCH)) {
            return None;
        }
        let value = |f: &FieldName| fields.get(f).cloned().unwrap_or(Value::Null);
        let own = &kept.view().group_by;
        let by: Row = (own.iter().zip(&src.group_by))
            .map(|(to, from)| (to.clone(), value(from)))
            .collect();
        Some(Member {
            group: kept.key(&by)?,
            by,
            key: r.key.clone(),
            fields,
            meta: meta.cloned(),
        })
    }
}

/// The clock a kept view's expressions are handed: none reads it.
const EPOCH: Timestamp = Timestamp(0);

/// What a kept record carries; nothing for a record without it.
fn carried(r: &StoredRecord) -> Carried {
    let held = r.fields.get(FOLDS).cloned();
    held.and_then(|c| serde_json::from_value(c).ok())
        .unwrap_or_default()
}

/// The group at `key` of the view `kept`, read from its record on first
/// use and marked as touched.
fn group<'g>(
    (groups, touched): (&'g mut BTreeMap<RecordKey, Group>, &mut BTreeSet<RecordKey>),
    kept: &Kept,
    (at, staged): (&At<'_>, &Staged),
    key: &RecordKey,
    by: &Row,
) -> Result<&'g mut Group> {
    if !groups.contains_key(key) {
        let stored = staged.record(at.view, &kept_name(&kept.decl.name), key)?;
        let stored = stored.filter(|r| !r.is_ended());
        let carried = stored.as_ref().map(carried).unwrap_or_default();
        let none = || {
            kept.sources
                .iter()
                .map(|s| s.keeping.none())
                .collect::<Vec<_>>()
        };
        let lined = |folds: Vec<KeptFolds>| match folds.len() == kept.sources.len() {
            true => folds,
            false => none(),
        };
        let open = lined(carried.open);
        let all = carried.all.map_or_else(|| open.clone(), lined);
        let group = Group {
            by: by.clone(),
            folds: [open, all],
            stored,
        };
        groups.insert(key.clone(), group);
    }
    touched.insert(key.clone());
    Ok(groups.get_mut(key).expect("the group was put"))
}

impl Work {
    /// A record of the source `i` was `before` and is `after`: it leaves
    /// the group it was a member of, arrives in the one it is a member of,
    /// or changes within it, on each face.
    fn note(
        &mut self,
        at: &At<'_>,
        staged: &Staged,
        i: usize,
        before: Option<&StoredRecord>,
        after: Option<&StoredRecord>,
    ) -> Result<()> {
        let Work {
            kept,
            groups,
            touched,
            entries,
            ..
        } = self;
        let (src, settings, cx) = (&kept.sources[i], Some(at.settings), (at, staged));
        for face in 0..2 {
            let was = before.and_then(|r| at.member(kept, src, r, face));
            let is = after.and_then(|r| at.member(kept, src, r, face));
            let placed = |m: &Option<Member>, r: Option<&StoredRecord>| {
                let letter = match face {
                    0 => Some('o'),
                    _ => r.filter(|r| src.view || r.restricted).map(|_| 'w'),
                };
                let (Some(m), Some(letter)) = (m, letter) else {
                    return Vec::new();
                };
                let of = src.keeping.entries(m.keyed(), settings).into_iter();
                let key = |j, place: &[u8]| {
                    format!("{}{}.{}", within(i, j, &m.group, letter), hex(place), m.key)
                };
                let entry = |e: Entry| serde_json::to_string(&e).expect("an entry serialises");
                of.map(|(j, place, e)| (key(j, &place), entry(e))).collect()
            };
            let (gone, put) = (placed(&was, before), placed(&is, after));
            for (key, _) in gone.iter().filter(|e| !put.contains(e)) {
                entries.insert(key.as_str().into(), None);
            }
            for (key, e) in put.into_iter().filter(|e| !gone.contains(e)) {
                entries.insert(key.into(), Some(e.into()));
            }
            match (was, is) {
                (None, None) => {}
                (Some(w), Some(n)) if w.group == n.group => {
                    let g = group((&mut *groups, &mut *touched), kept, cx, &n.group, &n.by)?;
                    let folds = &mut g.folds[face][i];
                    src.keeping.change(folds, w.keyed(), n.keyed(), settings);
                }
                (was, is) => {
                    if let Some(w) = was {
                        let g = group((&mut *groups, &mut *touched), kept, cx, &w.group, &w.by)?;
                        src.keeping.take(&mut g.folds[face][i], w.keyed(), settings);
                    }
                    if let Some(n) = is {
                        let g = group((&mut *groups, &mut *touched), kept, cx, &n.group, &n.by)?;
                        src.keeping.put(&mut g.folds[face][i], n.keyed(), settings);
                    }
                }
            }
        }
        Ok(())
    }

    /// One face of a group as the view's record would hold it: its group
    /// values and its measures, each landed in the kind the view declares
    /// for it; the error of a measure that fails over the group.
    fn measured(
        &self,
        g: &Group,
        face: usize,
        settings: &BTreeMap<String, Value>,
    ) -> expr::eval::Result<Row> {
        let (kept, folds) = (&self.kept, &g.folds[face]);
        let own = &kept.view().group_by;
        let mut given = Row::new();
        for (src, folds) in kept.sources.iter().zip(folds).skip(1) {
            let by: Row = (src.group_by.iter().zip(own))
                .map(|(to, from)| (to.clone(), g.by[from].clone()))
                .collect();
            given.extend(src.keeping.measures(
                folds,
                &by,
                &src.measures,
                Row::new(),
                Some(settings),
            )?);
        }
        let first = &kept.sources[0];
        let values =
            first
                .keeping
                .measures(&folds[0], &g.by, &first.measures, given, Some(settings))?;
        crate::reads::landed(&kept.decl, g.by.clone(), values).map_err(|(_, why)| why)
    }

    /// The record a group has after the event at `at`, and the counters
    /// it moves: none for a group with no member on either face.
    fn record(
        &mut self,
        key: &RecordKey,
        settings: &BTreeMap<String, Value>,
        at: Position,
    ) -> Option<StoredRecord> {
        let g = &self.groups[key];
        let ungrouped = self.kept.view().group_by.is_empty();
        let has = |face: usize| ungrouped || g.folds[face].iter().any(|f| !f.is_empty());
        let mut flagged = [false; 2];
        let mut faces = [None, None];
        for face in (0..2).filter(|f| has(*f)) {
            let measured = self.measured(g, face, settings);
            flagged[face] = measured.is_err() || !g.folds[face].iter().all(KeptFolds::exact);
            faces[face] = Some(measured.unwrap_or_else(|_| g.by.clone()));
        }
        let [open, all] = faces;
        let was = g.stored.as_ref().map(carried).unwrap_or_default();
        let whole = match (&open, &all) {
            (Some(open), Some(all)) if open != all => Some(all.clone()),
            _ => None,
        };
        let carried = Carried {
            open: g.folds[0].clone(),
            all: (g.folds[0] != g.folds[1]).then(|| g.folds[1].clone()),
            flagged,
            whole,
            alone: open.is_some() && all.is_none(),
        };
        let count = |n: &mut u64, was: bool, is: bool| {
            *n = (*n + u64::from(is)).saturating_sub(u64::from(was))
        };
        let faces = self
            .state
            .not_exact
            .iter_mut()
            .zip(was.flagged.iter().zip(flagged));
        faces.for_each(|(n, (was, is))| count(n, *was, is));
        count(&mut self.state.differ, was.differs(), carried.differs());
        let restricted = open.is_none();
        let mut fields = open.or(all)?;
        if carried.differs() {
            fields.insert(DIFFERS.into(), Value::from("1"));
        }
        fields.insert(FOLDS.into(), serde_json::json!(carried));
        Some(StoredRecord {
            ty: kept_name(&self.kept.decl.name),
            key: key.clone(),
            version: lakelands_api::Version(1),
            fields,
            meta: BTreeMap::new(),
            position: at,
            made: at,
            retired: false,
            restricted,
        })
    }
}

impl Views {
    /// Whether the transaction has looked at no view yet.
    pub(crate) fn idle(&self) -> bool {
        self.looked.is_empty() && self.noted.is_empty()
    }

    /// Read in, once for the transaction, the views a `view_kept.` setting
    /// names that read records of `reading`, or all of them with none;
    /// and `also`, whose state says whole.
    fn load(
        &mut self,
        at: &At<'_>,
        staged: &Staged,
        reading: Option<&TypeName>,
        also: Option<&TypeName>,
    ) -> Result<()> {
        let reads = |view: &TypeName| {
            let v = at.ontology.ty(view).ok().and_then(|d| d.view.as_deref());
            let merged = v
                .into_iter()
                .flat_map(|v| v.merge.iter().map(|m| &m.source));
            let mut sources = v.map(|v| &v.source).into_iter().chain(merged);
            reading.is_none_or(|ty| sources.any(|s| s == ty))
        };
        let named = asking(at.settings).map(TypeName::from).filter(reads);
        for ty in named.chain(also.cloned()) {
            if !self.looked.insert(ty.clone()) {
                continue;
            }
            let state = at.state(staged, &ty)?;
            let kept = (at.ontology.ty(&ty).ok()).and_then(|d| Kept::of(at.ontology, d));
            match kept {
                Some(kept) if state.whole => self.insert(kept, state),
                None if state.whole => self.orphans.push(ty),
                _ => {}
            }
        }
        Ok(())
    }

    fn insert(&mut self, kept: Kept, state: State) {
        self.looked.insert(kept.decl.name.clone());
        let work = Work {
            kept,
            entries: Entries::new(),
            state,
            groups: BTreeMap::new(),
            touched: BTreeSet::new(),
        };
        self.views.insert(work.kept.decl.name.clone(), work);
    }

    /// How many kept views `ty` reads through, itself counted: a view is
    /// put after those it reads.
    fn depth(&self, ty: &TypeName) -> usize {
        let Some(work) = self.views.get(ty) else {
            return 0;
        };
        let inner = work.kept.sources.iter().map(|s| self.depth(&s.ty));
        1 + inner.max().unwrap_or(0)
    }

    /// The views that read records of `ty`, each with the source it is.
    fn readers(&self, ty: &TypeName) -> Vec<(TypeName, usize)> {
        let reads = |w: &Work| {
            let sources = w.kept.sources.iter().enumerate();
            let of = sources
                .filter(|(_, s)| &s.ty == ty)
                .map(|(i, _)| (w.kept.decl.name.clone(), i));
            of.collect::<Vec<_>>()
        };
        self.views.values().flat_map(reads).collect()
    }

    /// Tell every kept view that reads `ty` what one of its records was
    /// and is.
    fn note(
        &mut self,
        at: &At<'_>,
        staged: &Staged,
        ty: &TypeName,
        before: Option<&StoredRecord>,
        after: Option<&StoredRecord>,
    ) -> Result<()> {
        if !self.noted.contains(ty) {
            self.load(at, staged, Some(ty), None)?;
            self.noted.insert(ty.clone());
        }
        for (view, i) in self.readers(ty) {
            let work = self.views.get_mut(&view).expect("a reader is a kept view");
            work.note(at, staged, i, before, after)?;
        }
        Ok(())
    }

    /// Put every group the event touched, once, at its position, in the
    /// order of the views' sources: a view's record that changes is noted
    /// for the views that read it, which are put after it.
    fn flush(&mut self, at: &At<'_>, staged: &mut Staged, position: Position) -> Result<()> {
        loop {
            // The innermost touched view: a view that reads it is touched
            // by its notes and comes after it.
            let touched = self.views.values().filter(|w| !w.touched.is_empty());
            let named = touched.map(|w| w.kept.decl.name.clone());
            let Some((_, view)) = named.map(|v| (self.depth(&v), v)).min() else {
                return Ok(());
            };
            let work = self.views.get_mut(&view).expect("a touched view is kept");
            let before = work.state.clone();
            let mut changed = Vec::new();
            // The event's entries first: the seeks read them.
            let name = order_name(&view);
            if let Some(undo) = staged
                .undo
                .as_mut()
                .filter(|_| !staged.orders.contains_key(&name))
            {
                undo.0.push(Undone::Orders(name.clone(), None));
            }
            let orders = staged.orders.entry(name.clone()).or_default();
            for (key, entry) in std::mem::take(&mut work.entries) {
                let mut r = StoredRecord::ended(name.clone(), RecordKey::from(&*key), position);
                if let Some(e) = &entry {
                    (r.version, r.retired) = (lakelands_api::Version(1), false);
                    r.fields
                        .insert(ENTRY.into(), serde_json::from_str(e).expect("an entry"));
                }
                at.put(&r)?;
                let was = orders.entries.insert(key.clone(), entry);
                if let Some(undo) = &mut staged.undo {
                    undo.0.push(Undone::Entry(name.clone(), key, was));
                }
            }
            for key in std::mem::take(&mut work.touched) {
                // An extreme whose holder the event took is read from its
                // order before its group is put: nothing waits past its
                // event.
                work.seat(at.view, staged.orders.get(&name), &key)?;
                let was = work.groups[&key].stored.clone();
                let is = work.record(&key, at.settings, position);
                let same = |a: &StoredRecord, b: &StoredRecord| {
                    (&a.fields, a.restricted) == (&b.fields, b.restricted)
                };
                let put = match (&was, &is) {
                    (None, None) => None,
                    (Some(a), Some(b)) if same(a, b) => None,
                    (_, Some(is)) => Some(is.clone()),
                    (Some(_), None) => {
                        Some(StoredRecord::ended(kept_name(&view), key.clone(), position))
                    }
                };
                if let Some(put) = put {
                    at.put(&put)?;
                    staged.put(put);
                    if let Some(moved) = &mut self.moved {
                        moved.insert((view.clone(), key.clone()));
                    }
                    changed.push((was, is.clone()));
                }
                // A group with no member leaves the transaction: one that
                // returns in it starts from nothing, as in the next one.
                match is {
                    None => drop(work.groups.remove(&key)),
                    is => work.groups.get_mut(&key).expect("a touched group").stored = is,
                }
            }
            if work.state != before {
                let state = work.state.clone();
                at.put_state(staged, &view, &state, position)?;
            }
            for (was, is) in changed {
                self.note(at, staged, &view, was.as_ref(), is.as_ref())?;
            }
        }
    }

    /// End every record of `view` and say that it is kept no more, and the
    /// same for every view that reads it, outermost first.
    fn unmake(
        &mut self,
        at: &At<'_>,
        staged: &mut Staged,
        view: &TypeName,
        position: Position,
    ) -> Result<()> {
        for (reader, _) in self.readers(view) {
            self.unmake(at, staged, &reader, position)?;
        }
        self.views.remove(view);
        if !at.state(staged, view)?.whole {
            return Ok(());
        }
        let mut keys = Vec::new();
        staged.each(at.view, &kept_name(view), |r| {
            keys.push(r.key);
            Ok(())
        })?;
        for key in keys {
            let ended = StoredRecord::ended(kept_name(view), key.clone(), position);
            at.put(&ended)?;
            staged.put(ended);
            if let Some(moved) = &mut self.moved {
                moved.insert((view.clone(), key));
            }
        }
        // The entries end as a whole, whatever their number: a seek in the
        // transaction reads only those put after.
        let name = order_name(view);
        if let Some(p) = at.projection {
            p.end_head_only(&name, position)?;
        }
        let ended = Orders {
            ended: true,
            ..Orders::default()
        };
        let was = staged.orders.insert(name.clone(), ended);
        if let Some(undo) = &mut staged.undo {
            undo.0.push(Undone::Orders(name, was));
        }
        at.put_state(staged, view, &State::default(), position)
    }

    /// Make `view` whole: every record of its sources noted from nothing,
    /// then one flush. No source is read otherwise.
    fn make(
        &mut self,
        at: &At<'_>,
        staged: &mut Staged,
        kept: Kept,
        position: Position,
    ) -> Result<()> {
        let view = kept.decl.name.clone();
        let whole = State {
            whole: true,
            ..State::default()
        };
        self.insert(kept, whole);
        let work = self.views.get_mut(&view).expect("the view was put");
        for i in 0..work.kept.sources.len() {
            let src = &work.kept.sources[i];
            let ty = match src.view {
                true => kept_name(&src.ty),
                false => src.ty.clone(),
            };
            staged.each(at.view, &ty, |r| work.note(at, staged, i, None, Some(&r)))?;
        }
        if work.kept.view().group_by.is_empty() {
            let all = RecordKey::from(lakelands_api::key::UNGROUPED);
            let Work {
                kept,
                groups,
                touched,
                ..
            } = work;
            group((groups, touched), kept, (at, staged), &all, &Row::new())?;
        }
        let state = work.state.clone();
        at.put_state(staged, &view, &state, position)?;
        self.flush(at, staged, position)
    }

    /// Whether `ty` is to be kept under the lake at the event: its setting
    /// holds, it can be kept, every view it reads is kept, and no source
    /// of it waits to be computed again ([`Staged::recomputes`]).
    fn wanted(&self, at: &At<'_>, staged: &Staged, ty: &TypeName) -> Option<Kept> {
        let asked = lakelands_api::seam::view_kept(at.settings.get(&format!("{SETTING}{ty}")));
        let kept = at.ontology.ty(ty).ok().filter(|_| asked);
        let kept = kept.and_then(|d| Kept::of(at.ontology, d))?;
        let read = |s: &Src| match s.view {
            true => self.views.contains_key(&s.ty),
            false => !staged.recomputes(&s.ty),
        };
        kept.sources.iter().all(read).then_some(kept)
    }

    /// Whatever is to be kept now and is not is made, and whatever is
    /// kept and is not to be is ended, innermost first.
    fn want(&mut self, at: &At<'_>, staged: &mut Staged, position: Position) -> Result<()> {
        loop {
            let unwanted = (self.views.keys())
                .find(|v| self.wanted(at, staged, v).is_none())
                .cloned();
            if let Some(view) = unwanted {
                self.unmake(at, staged, &view, position)?;
                continue;
            }
            let missing = asking(at.settings).map(TypeName::from);
            let mut missing = missing.filter(|v| !self.views.contains_key(v));
            let Some(kept) = missing.find_map(|v| self.wanted(at, staged, &v)) else {
                return Ok(());
            };
            self.make(at, staged, kept, position)?;
        }
    }
}

/// The views a `view_kept.` setting names, from one seek: the setting
/// that asks for a view to be kept, which the contract judges at its write.
fn asking(settings: &BTreeMap<String, Value>) -> impl Iterator<Item = &str> {
    let from = settings.range(SETTING.to_owned()..);
    from.map_while(|(k, _)| k.strip_prefix(SETTING))
}

/// A record of the lake was `before` and is `after`, in the fold of one
/// event: every kept view that reads its type is told.
pub(crate) fn note(
    at: &At<'_>,
    staged: &mut Staged,
    before: Option<&StoredRecord>,
    after: &StoredRecord,
) -> Result<()> {
    if asking(at.settings).next().is_none() && staged.kept.idle() {
        return Ok(());
    }
    let mut views = std::mem::take(&mut staged.kept);
    let noted = views.note(at, staged, &after.ty, before, Some(after));
    staged.kept = views;
    noted
}

/// The end of one event's fold: the views it makes or ends are made or
/// ended, and every group it touched is put, at its position.
pub(crate) fn settle(at: &At<'_>, staged: &mut Staged, e: &Event) -> Result<()> {
    // What the event may move: a setting, or the types whose declaration
    // or records it changes under a view: a declared type, and every type
    // a migration's step, or a value type's, reached.
    use EventBody::{Declared, Migrated, ValueTypeMigrated};
    let (mut setting, mut declared) = (None, BTreeSet::new());
    match &e.body {
        EventBody::RecordUpserted { ty, key, .. }
        | EventBody::RecordIngested { ty, key, .. }
        | EventBody::RecordRetired { ty, key, .. }
        | EventBody::RecordRestricted { ty, key, .. }
        | EventBody::RecordUnrestricted { ty, key, .. }
            if ty.as_str() == lakelands_api::seam::SETTING_TYPE =>
        {
            setting = Some(key.to_string());
        }
        EventBody::Declared {
            declaration: Declaration::Type(t),
            ..
        } => declared.extend([t.name.clone()]),
        EventBody::Migrated { .. } | EventBody::ValueTypeMigrated { .. } => {
            declared = staged.stepped_types();
        }
        _ => {}
    }
    let asked = (setting.as_deref())
        .and_then(|k| k.strip_prefix(SETTING))
        .map(TypeName::from);
    let none = asking(at.settings).next().is_none();
    if none && staged.kept.idle() && asked.is_none() {
        return Ok(());
    }
    let any = setting.is_some()
        || matches!(
            e.body,
            Declared { .. } | Migrated { .. } | ValueTypeMigrated { .. }
        );
    let mut views = std::mem::take(&mut staged.kept);
    let moves = (any, &declared, setting.as_deref());
    let settled = views.settle(at, staged, moves, e.position);
    staged.kept = views;
    settled
}

impl Views {
    /// [`settle`] of an event that writes `setting`, declares or steps
    /// the types `declared`, or, where not `any`, writes a record.
    fn settle(
        &mut self,
        at: &At<'_>,
        staged: &mut Staged,
        (any, declared, setting): (bool, &BTreeSet<TypeName>, Option<&str>),
        position: Position,
    ) -> Result<()> {
        // A setting's write and a declaration may move any view: all are
        // read in. A record's write moves the views that read its type,
        // which its notes have read in.
        if any {
            let asked = setting.and_then(|k| k.strip_prefix(SETTING));
            self.load(at, staged, None, asked.map(TypeName::from).as_ref())?;
        }
        for view in std::mem::take(&mut self.orphans) {
            self.unmake(at, staged, &view, position)?;
        }
        // A view the event changes under its records is ended: its own
        // declaration or a source's, a source whose records wait to be
        // computed again, or a setting its expressions read.
        let moved = |w: &Work| {
            let under = |s: &Src| declared.contains(&s.ty) || staged.recomputes(&s.ty);
            declared.contains(&w.kept.decl.name)
                || w.kept.sources.iter().any(under)
                || setting.is_some_and(|k| w.kept.settings.contains(k))
        };
        let ended: Vec<TypeName> = (self.views.values().filter(|w| moved(w)))
            .map(|w| w.kept.decl.name.clone())
            .collect();
        for view in &ended {
            self.unmake(at, staged, view, position)?;
        }
        if setting.is_some() || !declared.is_empty() {
            self.want(at, staged, position)?;
        }
        self.flush(at, staged, position)
    }
}

/// The end of a flush: the recomputations that were pending have put
/// their records, and the views ended for them are made from those, at
/// the position of the last event folded. Between a recomputing
/// declaration and that position the state says not whole, and a read
/// there computes.
pub(crate) fn made(at: &At<'_>, staged: &mut Staged, position: Position) -> Result<()> {
    if asking(at.settings).next().is_none() {
        return Ok(());
    }
    let mut views = std::mem::take(&mut staged.kept);
    let made = (views.load(at, staged, None, None)).and_then(|()| views.want(at, staged, position));
    staged.kept = views;
    made
}

/// The write path's run of the fold's own hooks (lakelands#1185), over
/// no projection: where `write`, a record of the lake was `before` and is
/// `after`; else a set of declarations declared `declared`. The views it
/// moves are put as the fold of the same events will put them, from the
/// same records, and the groups put or ended are answered.
pub(crate) fn step(
    at: &At<'_>,
    staged: &mut Staged,
    write: Option<(Option<&StoredRecord>, &StoredRecord)>,
    declared: &BTreeSet<TypeName>,
    position: Position,
) -> Result<Moved> {
    let mut setting = None;
    if let Some((before, after)) = write {
        note(at, staged, before, after)?;
        staged.put(after.clone());
        let named = after.ty.as_str() == lakelands_api::seam::SETTING_TYPE;
        setting = named.then(|| after.key.to_string());
    }
    let mut views = std::mem::take(&mut staged.kept);
    views.moved = Some(Moved::new());
    let any = write.is_none() || setting.is_some();
    let settled = views.settle(at, staged, (any, declared, setting.as_deref()), position);
    let moved = views.moved.take().unwrap_or_default();
    staged.kept = views;
    settled.map(|()| moved)
}

/// Whether the views as the transaction leaves them hold `view` whole.
pub(crate) fn whole(at: &At<'_>, staged: &Staged, view: &TypeName) -> Result<bool> {
    Ok(at.state(staged, view)?.whole)
}

/// The record a set over `view` decides on at `key`, as the transaction
/// leaves the view: the group's whole face, decided for no reader (004).
/// Where the fold flags the group, or a group of a view it reads through,
/// as not exact, the face is what a read of the view computes from the
/// members (049). None where the group has no record on that face, or a
/// measure fails over it or over a group of a view it reads.
pub(crate) fn face(
    at: &At<'_>,
    staged: &Staged,
    view: &TypeName,
    key: &RecordKey,
) -> Result<Option<StoredRecord>> {
    Ok(faced(at, staged, view, key)?.flatten())
}

/// [`face`], where a measure that fails, which a read of the view
/// refuses, is told apart from no record: none where one fails, and
/// `Some(None)` where the group has no record on the whole face.
fn faced(
    at: &At<'_>,
    staged: &Staged,
    view: &TypeName,
    key: &RecordKey,
) -> Result<Option<Option<StoredRecord>>> {
    let held = staged.record(at.view, &kept_name(view), key)?;
    let Some(r) = held.filter(|r| !r.is_ended()) else {
        return Ok(Some(None));
    };
    let flagged = carried(&r).flagged[1];
    let Some(mut r) = shown(r, view, true) else {
        return Ok(Some(None));
    };
    let kept = at
        .ontology
        .ty(view)
        .ok()
        .and_then(|d| Kept::of(at.ontology, d));
    let Some(kept) = kept else {
        return Ok(Some(Some(r)));
    };
    if flagged || inexact_below(at, staged, &kept)? {
        let value = |f: &FieldName| (f.clone(), r.fields.get(f).cloned().unwrap_or(Value::Null));
        let by: Row = kept.view().group_by.iter().map(value).collect();
        match computed(at, staged, &kept, key, &by)? {
            Some(Some(fields)) => r.fields = fields,
            Some(None) => return Ok(Some(None)),
            None => return Ok(None),
        }
    }
    Ok(Some(Some(r)))
}

/// Whether a view `kept` reads through holds a group that is not exact on
/// the whole face: the fold folds that group's record as it stands.
fn inexact_below(at: &At<'_>, staged: &Staged, kept: &Kept) -> Result<bool> {
    for src in kept.sources.iter().filter(|s| s.view) {
        let inner = (at.ontology.ty(&src.ty).ok()).and_then(|d| Kept::of(at.ontology, d));
        if at.state(staged, &src.ty)?.not_exact[1] > 0
            || inner.map_or(Ok(false), |k| inexact_below(at, staged, &k))?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The whole face of the group at `key` of `kept`, its values `by`, as a
/// read of the view computes it from its members as the transaction leaves
/// them, every record of each source read. `Some(None)` where it has no
/// member; none where a measure fails over it or does not land, or over a
/// group of a view it reads: a read refuses there.
fn computed(
    at: &At<'_>,
    staged: &Staged,
    kept: &Kept,
    key: &RecordKey,
    by: &Row,
) -> Result<Option<Option<Row>>> {
    let own = &kept.view().group_by;
    let mut folds = Vec::with_capacity(kept.sources.len());
    for src in &kept.sources {
        let keys = src.group_by.iter().map(|f| (f.clone(), None)).collect();
        let mut members = Grouping::new(keys, &src.measures, EPOCH);
        let ty = match src.view {
            true => kept_name(&src.ty),
            false => src.ty.clone(),
        };
        let mut fails = false;
        staged.each(at.view, &ty, |r| {
            let r = match src.view {
                true => faced(at, staged, &src.ty, &r.key)?,
                false => Some(Some(r)),
            };
            fails |= r.is_none();
            let m = r.flatten().and_then(|r| at.member(kept, src, &r, 1));
            if let Some(m) = m.filter(|m| &m.group == key) {
                members.add((&m.fields, m.meta.as_ref()), Some(at.settings));
            }
            Ok(())
        })?;
        if fails {
            return Ok(None);
        }
        folds.push(members.groups().pop().map(|(_, f)| f));
    }
    if !own.is_empty() && folds.iter().all(Option::is_none) {
        return Ok(Some(None));
    }
    let none = Folds::default();
    let measured = |i: usize, given: Row| {
        let src: &Src = &kept.sources[i];
        let value = |(to, from): (&FieldName, &FieldName)| {
            (to.clone(), by.get(from).cloned().unwrap_or(Value::Null))
        };
        let own: Row = src.group_by.iter().zip(own).map(value).collect();
        let folds = folds[i].as_ref().unwrap_or(&none);
        expr::aggregate::measures(folds, &own, &src.measures, given, Some(at.settings), EPOCH)
    };
    let mut given = Row::new();
    for i in 1..kept.sources.len() {
        let Ok(values) = measured(i, Row::new()) else {
            return Ok(None);
        };
        given.extend(values);
    }
    let Ok(values) = measured(0, given) else {
        return Ok(None);
    };
    Ok(crate::reads::landed(&kept.decl, by.clone(), values)
        .ok()
        .map(Some))
}

impl Work {
    /// Each extreme of the group at `key` whose holder left is held by the
    /// first entry of its order from the holder it kept on: that is where
    /// it stands if it stayed, and the next if it left. A face that reads
    /// from both `o` and `w` takes the lesser of the two. A source's whole
    /// face whose state is the open one's has no restricted member, and is
    /// seated as the open one is.
    fn seat(
        &mut self,
        view: &dyn ProjectionView,
        orders: Option<&Orders>,
        key: &RecordKey,
    ) -> Result<()> {
        let Work { kept, groups, .. } = self;
        let g = groups.get_mut(key).expect("a touched group");
        let name = order_name(&kept.decl.name);
        for (i, src) in kept.sources.iter().enumerate() {
            let same = !src.view && g.folds[0][i] == g.folds[1][i];
            for face in 0..2 - usize::from(same) {
                let folds = &mut g.folds[face][i];
                let letters = [('o', face == 0 || !src.view), ('w', face == 1)];
                let mut first = Vec::new();
                for (j, from) in src.keeping.waiting(folds) {
                    let from = from.map(|(place, m)| format!("{}.{m}", hex(&place)));
                    let mut held: Option<(String, Entry)> = None;
                    for letter in letters.iter().filter_map(|(l, on)| on.then_some(*l)) {
                        let within = within(i, j, key, letter);
                        let start = format!("{within}{}", from.as_deref().unwrap_or_default());
                        let Some((at, e)) = seek(view, orders, &name, &within, start)? else {
                            continue;
                        };
                        let place = at[within.len()..].to_owned();
                        if held.as_ref().is_none_or(|(p, _)| place < *p) {
                            held = Some((place, e));
                        }
                    }
                    first.push((j, held.map(|(_, e)| e)));
                }
                if !first.is_empty() {
                    src.keeping.seated(folds, first);
                }
            }
            if same {
                g.folds[1][i] = g.folds[0][i].clone();
            }
        }
        Ok(())
    }
}

/// At most `n` records of the order `name` after the key `after`, in key
/// order: the page every engine reads by a seek.
fn page(
    view: &dyn ProjectionView,
    name: &TypeName,
    after: &str,
    n: u32,
) -> Result<Vec<StoredRecord>> {
    let after = Some(After {
        values: Vec::new(),
        key: after.into(),
    });
    view.scan(
        name,
        &Scan {
            after,
            limit: n,
            ..Scan::default()
        },
    )
}

/// The first live entry of the order `name` from `start` on whose key
/// begins `within`, none past them: the transaction's own entries over
/// the engine's, which are read in batches that double, so that a seek
/// past entries the transaction ended reads as many batches as it doubles.
/// Where the transaction ended the name, its own are all there is.
fn seek(
    view: &dyn ProjectionView,
    orders: Option<&Orders>,
    name: &TypeName,
    within: &str,
    start: String,
) -> Result<Option<(String, Entry)>> {
    let read = |r: &StoredRecord| serde_json::from_value(r.fields.get(ENTRY)?.clone()).ok();
    let parse = |e: &str| serde_json::from_str(e).ok();
    let mine = |key: &str| orders.and_then(|o| o.entries.get(key));
    let ended = orders.is_some_and(|o| o.ended);
    match mine(&start) {
        Some(Some(e)) => return Ok(parse(e).map(|e| (start, e))),
        Some(None) => {}
        None if ended => {}
        None => {
            let held = view
                .get(name, &start.as_str().into())?
                .filter(|r| !r.is_ended());
            if let Some(e) = held.as_ref().and_then(read) {
                return Ok(Some((start, e)));
            }
        }
    }
    let (mut after, mut n) = (start, 1);
    loop {
        let batch = match ended {
            true => Vec::new(),
            false => page(view, name, &after, n)?,
        };
        let end = batch
            .last()
            .filter(|_| batch.len() == n as usize)
            .map(|r| r.key.to_string());
        let theirs = batch.iter().filter(|r| mine(r.key.as_str()).is_none());
        let mut merged: BTreeMap<String, Option<Entry>> =
            theirs.map(|r| (r.key.to_string(), read(r))).collect();
        let to = end.as_deref().map_or(Unbounded, Included);
        let range = orders
            .into_iter()
            .flat_map(|o| o.entries.range::<str, _>((Excluded(after.as_str()), to)));
        merged.extend(range.map(|(k, e)| (k.to_string(), e.as_deref().and_then(parse))));
        for (key, e) in merged {
            match e {
                _ if !key.starts_with(within) => return Ok(None),
                Some(e) => return Ok(Some((key, e))),
                None => {}
            }
        }
        match end {
            Some(end) => (after, n) = (end, (n * 2).min(1024)),
            None => return Ok(None),
        }
    }
}

#[cfg(test)]
mod tests;
