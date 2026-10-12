//! A view the fold keeps (lakelands#575): its records are records in the
//! projection, one for each group, put by the fold under a name of the
//! store's own that no type can bear, and a read of the view answers
//! them where the view's state says they are the computation's own. A
//! kept record is exactly what the read would compute, or the state says
//! at that position that it is not, and the read computes.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::engine::{ProjectionStore, ProjectionView};
use lakelands_api::event::{Event, EventBody};
use lakelands_api::expr::{Condition, Expr, Func, Literal};
use lakelands_api::ontology::{Measure, TypeDecl, ViewDecl};
use lakelands_api::record::{StoredRecord, Value};
use lakelands_api::seam::{VIEW_KEPT_SETTING as SETTING, kept_name};
use lakelands_api::write::Declaration;
use lakelands_api::{FieldName, Position, RecordKey, Result, Timestamp, TypeName};
use lakelands_core::expr::aggregate::{Keeping, KeptFolds, Row};
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

/// What the store records of a kept view at a position: counters, and
/// nothing that grows with the view.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct State {
    /// Whether the view's records are whole: made, and brought up to date
    /// by every event since.
    pub whole: bool,
    /// The groups that are not exact, on the open face and on the whole
    /// one: a measure that fails, a sum outside what is exact, a member
    /// that does not fold, an extreme given up ([`MEMBERS`]).
    pub not_exact: [u64; 2],
    /// The groups whose whole face reads otherwise than their open one.
    pub differ: u64,
}

/// The state of `view` at the position `at` reads, if the fold kept one.
pub(crate) fn state(at: &dyn ProjectionView, view: &TypeName) -> Result<Option<State>> {
    let input = crate::backed::digest(&Value::from(view.as_str()));
    let recorded = at.output(KEPT, &input)?;
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
                let filter = filter.map(|w| condition(w, Place::View, "view"));
                let filter = filter.transpose().ok()?;
                let measured = |m: &Measure| {
                    Ok((m.name.clone(), condition(&m.expr, Place::Group, "measure")?))
                };
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

/// The settings `e` names.
fn settings_named(e: &Expr, named: &mut BTreeSet<String>) {
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
}

/// What the fold's hooks are handed: the transaction's view of the
/// projection, the projection it writes, and the lake at the event.
pub(crate) struct At<'a> {
    pub view: &'a dyn ProjectionView,
    pub projection: &'a dyn ProjectionStore,
    pub ontology: &'a Ontology,
    pub settings: &'a BTreeMap<String, Value>,
}

/// A member of one source on one face: its group's key and values, and the
/// key and fields it is folded with.
struct Member {
    group: RecordKey,
    by: Row,
    key: RecordKey,
    fields: Row,
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
            None => self::state(self.view, ty)?.unwrap_or_default(),
        })
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
        self.projection.put_output(KEPT, &input, &state, at)?;
        (staged.outputs).insert((KEPT.to_owned(), input), vec![(at, state)]);
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
            ..
        } = self;
        let (src, settings, cx) = (&kept.sources[i], Some(at.settings), (at, staged));
        for face in 0..2 {
            let was = before.and_then(|r| at.member(kept, src, r, face));
            let is = after.and_then(|r| at.member(kept, src, r, face));
            match (was, is) {
                (None, None) => {}
                (Some(w), Some(n)) if w.group == n.group => {
                    let g = group((&mut *groups, &mut *touched), kept, cx, &n.group, &n.by)?;
                    let folds = &mut g.folds[face][i];
                    src.keeping
                        .change(folds, (&w.key, &w.fields), (&n.key, &n.fields), settings);
                }
                (was, is) => {
                    if let Some(w) = was {
                        let g = group((&mut *groups, &mut *touched), kept, cx, &w.group, &w.by)?;
                        src.keeping
                            .take(&mut g.folds[face][i], &w.key, &w.fields, settings);
                    }
                    if let Some(n) = is {
                        let g = group((&mut *groups, &mut *touched), kept, cx, &n.group, &n.by)?;
                        src.keeping
                            .put(&mut g.folds[face][i], &n.key, &n.fields, settings);
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
        Ok(crate::reads::landed(&kept.decl, g.by.clone(), values))
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
            for key in std::mem::take(&mut work.touched) {
                // An extreme whose holder the event took is read again
                // before its group is put: nothing waits past its event.
                // A group given up is read no more: it is given up again.
                let folds = || work.groups[&key].folds.iter().flatten();
                match (folds().any(KeptFolds::waits), folds().any(KeptFolds::lost)) {
                    (true, false) => work.again(at, staged, &key)?,
                    (true, true) => given_up(work.groups.get_mut(&key).expect("a touched group")),
                    _ => {}
                }
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
                    at.projection.put_record(&put)?;
                    staged.put(put);
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
            let ended = StoredRecord::ended(kept_name(view), key, position);
            at.projection.put_record(&ended)?;
            staged.put(ended);
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
    // a migration's step reached.
    use EventBody::{Declared, Migrated};
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
        EventBody::Migrated { .. } => declared = staged.stepped_types(),
        _ => {}
    }
    let asked = (setting.as_deref())
        .and_then(|k| k.strip_prefix(SETTING))
        .map(TypeName::from);
    let none = asking(at.settings).next().is_none();
    if none && staged.kept.idle() && asked.is_none() {
        return Ok(());
    }
    // A setting's write and a declaration may move any view: all are
    // read in. A record's write moves the views that read its type, which
    // its notes have read in.
    let any = setting.is_some() || matches!(e.body, Declared { .. } | Migrated { .. });
    let mut views = std::mem::take(&mut staged.kept);
    let settled = (|| {
        if any {
            views.load(at, staged, None, asked.as_ref())?;
        }
        for view in std::mem::take(&mut views.orphans) {
            views.unmake(at, staged, &view, e.position)?;
        }
        // A view the event changes under its records is ended: its own
        // declaration or a source's, a source whose records wait to be
        // computed again, or a setting its expressions read.
        let moved = |w: &Work| {
            let named = setting.as_deref();
            let under = |s: &Src| declared.contains(&s.ty) || staged.recomputes(&s.ty);
            declared.contains(&w.kept.decl.name)
                || w.kept.sources.iter().any(under)
                || named.is_some_and(|k| w.kept.settings.contains(k))
        };
        let ended: Vec<TypeName> = (views.views.values().filter(|w| moved(w)))
            .map(|w| w.kept.decl.name.clone())
            .collect();
        for view in &ended {
            views.unmake(at, staged, view, e.position)?;
        }
        if setting.is_some() || !declared.is_empty() {
            views.want(at, staged, e.position)?;
        }
        views.flush(at, staged, e.position)
    })();
    staged.kept = views;
    settled
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

/// How many records one group is read again over, at most: past them it
/// is given up.
const MEMBERS: usize = 8 * 1024;

impl Work {
    /// Read the group at `key` again, in the fold of the event that took
    /// an extreme's holder from it: each source's live records with the
    /// group's values, by the engine's lookup, folded from nothing. A walk
    /// past [`MEMBERS`], over all sources and both lookups, ends there,
    /// but after a kept step of the transaction, whose walk of the type
    /// goes on to its end handing nothing more ([`Staged::matching`]), and
    /// gives the group up: what waited is not the computation's own until
    /// the view is made or the group ends, and nothing reads it again.
    /// Decided by the event, so on every path that folds it alike.
    fn again(&mut self, at: &At<'_>, staged: &Staged, key: &RecordKey) -> Result<()> {
        let Work { kept, groups, .. } = self;
        let g = groups.get_mut(key).expect("a touched group");
        let own = &kept.view().group_by;
        let mut fresh = [Vec::new(), Vec::new()];
        let (mut left, mut whole) = (MEMBERS, true);
        for src in &kept.sources {
            let mut folds = [src.keeping.none(), src.keeping.none()];
            let equal: Vec<(FieldName, Value)> = (src.group_by.iter().zip(own))
                .map(|(f, from)| (f.clone(), g.by[from].clone()))
                .collect();
            let ty = match src.view {
                true => kept_name(&src.ty),
                false => src.ty.clone(),
            };
            // A member that is a view's record is found by its own fields
            // for the open face, and for the whole one too unless that
            // reads otherwise: those are found by their mark, whatever
            // their own fields say.
            let differs = [(FieldName::from(DIFFERS), Value::from("1"))];
            let mut put = |member: &StoredRecord, faces: std::ops::Range<usize>| {
                let Some(less) = left.checked_sub(1) else {
                    return false;
                };
                left = less;
                for face in faces {
                    let m = at.member(kept, src, member, face);
                    if let Some(m) = m.filter(|m| &m.group == key) {
                        (src.keeping).put(&mut folds[face], &m.key, &m.fields, Some(at.settings));
                    }
                }
                true
            };
            let own = |m: &StoredRecord| 0..2 - usize::from(m.fields.contains_key(DIFFERS));
            whole = staged.matching(at.view, &ty, &equal, |m| put(m, own(m)))?;
            if src.view && whole {
                whole = staged.matching(at.view, &ty, &differs, |m| put(m, 1..2))?;
            }
            if !whole {
                break;
            }
            let [open, all] = folds;
            fresh[0].push(open);
            fresh[1].push(all);
        }
        match whole {
            true => g.folds = fresh,
            false => given_up(g),
        }
        Ok(())
    }
}

/// Give a group up: whatever of it waits is not exact from here on.
fn given_up(g: &mut Group) {
    g.folds.iter_mut().flatten().for_each(KeptFolds::give_up);
}

#[cfg(test)]
mod tests;
