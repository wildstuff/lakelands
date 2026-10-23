//! An object set of a view the lake keeps, and a trigger on one
//! (`docs/decisions/066-a-set-holds-a-kept-view.md`).
//!
//! A membership or a firing lands in the append of the write that moves
//! it, and the fold never appends (049), so the write path computes the
//! view's rows itself: it runs the fold's own hooks over a [`Staged`] of
//! its own and no projection, from the same records the fold will read,
//! and decides each group they put or end. A set of declarations, or a
//! setting a view's expressions name, makes the view again the same way; a
//! setting a set's `where` names decides every group of the set's view.

use super::*;
use crate::kept::{self, At};
use crate::lake::Staged;

/// A set of a view, or a trigger on one: what reads the view's rows.
#[derive(Clone, Copy)]
enum Reader<'o> {
    Set(&'o ObjectSetDecl),
    Trigger(&'o TriggerDecl),
}

impl Reader<'_> {
    /// The view it reads.
    fn view(&self) -> &TypeName {
        match self {
            Reader::Set(s) => &s.of,
            Reader::Trigger(t) => &t.on,
        }
    }

    /// The refusal, by `case`, of a change that would leave this reader, a set or a
    /// trigger, without its rows, to an author shown it: said of it where the
    /// change moves a source, and of its `of` or `on` where the view would not
    /// be kept.
    fn refusal(self, case: Case) -> Error {
        let (place, member) = match self {
            Reader::Set(s) => (
                Place::ObjectSet {
                    name: s.name.clone(),
                },
                "of",
            ),
            Reader::Trigger(t) => (
                Place::Trigger {
                    name: t.name.clone(),
                },
                "on",
            ),
        };
        let said = Said::from(case).within(place);
        match said.case {
            Case::ViewSourceStepped { .. } => Error::invalid(said),
            _ => Error::invalid_at(&member.into(), said),
        }
    }

    fn declaration(&self) -> Declaration {
        match self {
            Reader::Set(s) => Declaration::ObjectSet((*s).clone()),
            Reader::Trigger(t) => Declaration::Trigger((*t).clone()),
        }
    }
}

/// The sets and the triggers of `o` that read a view.
fn readers(o: &Ontology) -> impl Iterator<Item = Reader<'_>> {
    let sets = o.object_sets.values().map(|e| Reader::Set(&e.decl));
    let triggers = o.triggers.values().map(|e| Reader::Trigger(&e.decl));
    sets.chain(triggers)
        .filter(|r| viewed(o, r.view()).is_some())
}

/// What one append holds of the views its sets of views read: the fold's
/// hooks run over its own [`Staged`], with the settings as its writes
/// leave them. A write refused is undone in it ([`kept::undo`]).
#[derive(Default)]
pub(crate) struct Viewed {
    /// The views a set holds or a trigger watches, and every view they
    /// read through.
    views: BTreeSet<TypeName>,
    /// The types whose records those views read.
    reads: BTreeSet<TypeName>,
    /// The settings that keep those views, that their expressions name,
    /// or that a set's `where` names.
    named: BTreeSet<String>,
    /// The settings a set's `where` names, each with the views of the
    /// sets that name it.
    asked: BTreeMap<String, BTreeSet<TypeName>>,
    staged: Staged,
    /// The settings at the next write, [`held`] by `views`.
    settings: Option<BTreeMap<String, Value>>,
}

impl Viewed {
    /// Open the undo of a job of a batch.
    pub(super) fn open(&mut self) {
        self.staged.undo = Some(kept::Undo::default());
    }

    /// Close it: keep what the job's runs put, or put it back.
    pub(super) fn close(&mut self, keep: bool) {
        match keep {
            true => self.staged.undo = None,
            false => kept::undo(&mut self.staged),
        }
    }

    /// What the sets of views of `o`, and its triggers on views, read.
    fn of(o: &Ontology) -> Viewed {
        let mut v = Viewed::default();
        let mut next = Vec::new();
        for r in readers(o) {
            next.push(r.view().clone());
            let Reader::Set(s) = r else { continue };
            let at = [Place::ObjectSet {
                name: s.name.clone(),
            }];
            // A filter that does not expand decides nothing here: deciding
            // a membership refuses it.
            let mut asked = BTreeSet::new();
            if let Ok(e) = declare::set_filter(&|n| o.set(n), s, &at) {
                lakelands_core::derive::settings(&e, &mut asked);
            }
            for k in asked {
                v.asked.entry(k).or_default().insert(s.of.clone());
            }
        }
        v.named.extend(v.asked.keys().cloned());
        while let Some(ty) = next.pop() {
            let Some(decl) = viewed(o, &ty) else {
                v.reads.insert(ty);
                continue;
            };
            if v.views.insert(ty.clone()) {
                v.named
                    .insert(format!("{}{ty}", lakelands_api::seam::VIEW_KEPT_SETTING));
                v.named.extend(kept::settings_read(o, decl));
                let view = decl.view.as_deref().expect("a view");
                next.push(view.source.clone());
                next.extend(view.merge.iter().map(|m| m.source.clone()));
            }
        }
        v
    }

    /// The records the hooks read from the projection, for the fold.
    pub(crate) fn into_reads(self) -> crate::lake::Reads {
        self.staged.into_reads()
    }
}

/// `settings` without the keeping of any view but `views`: the hooks run
/// over those alone, and read the state of no other. A write of another
/// view's keeping moves none of them: it is not `named`, and what is
/// `named` stays, as an expression reads it.
fn held(
    (views, named): (&BTreeSet<TypeName>, &BTreeSet<String>),
    settings: &BTreeMap<String, Value>,
) -> BTreeMap<String, Value> {
    let other = |k: &str| {
        let kept = k.strip_prefix(lakelands_api::seam::VIEW_KEPT_SETTING);
        !named.contains(k) && kept.is_some_and(|t| !views.contains(&TypeName::from(t)))
    };
    let kept = settings.iter().filter(|(k, _)| !other(k));
    kept.map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// The declaration of `ty` where it is a view.
fn viewed<'o>(o: &'o Ontology, ty: &TypeName) -> Option<&'o TypeDecl> {
    o.ty(ty).ok().filter(|t| t.view.is_some())
}

impl Decide<'_> {
    /// The memberships a write of the kind `change` that leaves `after`
    /// (it was `before`) moves in the sets of the views that read it, and
    /// the triggers it fires on them, after what it sets off across an
    /// edge: each group the fold's hooks put or end at its event is decided
    /// on its whole face, and counted with `out` toward `max_set_changes`;
    /// at a write of a setting a set's `where` names, every group of its
    /// view. A write that ends the keeping of such a view is refused. A
    /// write of a type no such view reads pays nothing.
    pub(super) fn viewed(
        &mut self,
        change: ChangeKind,
        (before, after): (Option<&StoredRecord>, &StoredRecord),
        out: &mut Vec<(Draft, Option<Change>)>,
    ) -> Result<()> {
        let mut v = match self.pending.viewed.take() {
            Some(v) => v,
            None => Box::new(Viewed::of(self.ontology)),
        };
        let setting = after.ty.as_str() == SETTING_TYPE;
        let moves = v.reads.contains(&after.ty) || setting && v.named.contains(after.key.as_str());
        let decided = match moves {
            true => self.viewed_by(&mut v, change, (before, after), out),
            false => Ok(()),
        };
        self.pending.viewed = Some(v);
        decided
    }

    /// [`Self::viewed`] of a write that moves a view: run over `v`, and
    /// undone in it where it is refused.
    fn viewed_by(
        &self,
        v: &mut Viewed,
        change: ChangeKind,
        (before, after): (Option<&StoredRecord>, &StoredRecord),
        out: &mut Vec<(Draft, Option<Change>)>,
    ) -> Result<()> {
        let settings =
            (v.settings).get_or_insert_with(|| held((&v.views, &v.named), self.at.settings));
        let setting = after.ty.as_str() == SETTING_TYPE;
        let was = setting.then(|| settings.clone());
        if setting {
            // As the fold takes it in before its hooks.
            match after.fields.get("value").filter(|_| !after.retired) {
                Some(value) => settings.insert(after.key.to_string(), value.clone()),
                None => settings.remove(after.key.as_str()),
            };
        }
        // Inside a job of a batch, whose own undo is open, a refusal puts
        // back this run alone.
        let outer = v.staged.undo.is_some();
        let from = v.staged.undo.get_or_insert_default().len();
        let decided = self.viewed_run(v, change, (before, after), out);
        if decided.is_err() {
            kept::undo_to(&mut v.staged, from);
            if was.is_some() {
                v.settings = was;
            }
        }
        if !outer {
            v.staged.undo = None;
        }
        decided
    }

    /// The hooks run over `v` for the write, and the memberships of every
    /// group they moved decided into `out`, and the triggers they fire.
    fn viewed_run(
        &self,
        v: &mut Viewed,
        change: ChangeKind,
        (before, after): (Option<&StoredRecord>, &StoredRecord),
        out: &mut Vec<(Draft, Option<Change>)>,
    ) -> Result<()> {
        let at = At {
            view: self.at.view,
            projection: None,
            ontology: self.ontology,
            settings: v.settings.as_ref().expect("the settings are held"),
        };
        let none = BTreeSet::new();
        let staged = &mut v.staged;
        let mut moved = kept::step(&at, staged, Some((before, after)), &none, self.next)?;
        let sees = Sight::held(self.call.who, self.ontology);
        let max = self.call.lake.limits.max_set_changes as usize;
        let named = |s: &ObjectSetDecl| sees.shows(&Declaration::ObjectSet(s.clone()));
        // Only a setting's write ends a view's keeping.
        let setting = (after.ty.as_str() == SETTING_TYPE).then(|| after.key.as_str());
        for view in v.views.iter().filter(|_| setting.is_some()) {
            if !kept::whole(&at, staged, view)? {
                let reads = |r: &Reader| {
                    r.view() == view
                        || sources_read(self.ontology, r.view())
                            .iter()
                            .any(|(t, _)| t == view)
                };
                let shown = |r: &Reader| sees.shows(&r.declaration());
                return Err(match readers(self.ontology).filter(reads).find(shown) {
                    Some(r) => r.refusal(Case::ViewKeptForSet { ty: view.clone() }),
                    None => Error::not_granted(),
                });
            }
        }
        // A setting a set's `where` names moves every group of its view;
        // a group of a view that turns flagged or exact again, every group
        // of each set's view that reads it, as a read of that view refuses
        // while a group it reads fails.
        let mut whole = setting
            .and_then(|k| v.asked.get(k))
            .cloned()
            .unwrap_or_default();
        for (view, key) in &moved {
            if !kept::reflagged(&at, staged, (view, key))? {
                continue;
            }
            for s in self.ontology.object_sets.values().map(|e| &e.decl) {
                if sources_read(self.ontology, &s.of)
                    .iter()
                    .any(|(t, _)| t == view)
                {
                    whole.insert(s.of.clone());
                }
            }
        }
        for view in &whole {
            staged.each(at.view, &lakelands_api::seam::kept_name(view), |r| {
                moved.insert((view.clone(), r.key));
                Ok(())
            })?;
        }
        let mut mine = BTreeMap::new();
        let mut fired = Vec::new();
        for (view, key) in &moved {
            let sets = (self.ontology.object_sets.values()).map(|e| &e.decl);
            let sets: Vec<&ObjectSetDecl> = sets.filter(|s| &s.of == view).collect();
            let triggers = self.ontology.triggers.values().map(|e| &e.decl);
            let on = |t: &&TriggerDecl| &t.on == view && fires_on(t, change);
            let triggers: Vec<&TriggerDecl> = triggers.filter(on).collect();
            if sets.is_empty() && triggers.is_empty() {
                continue;
            }
            let row = kept::face(&at, staged, view, key)?;
            // A trigger fires where the group's row is put, it arrived or
            // its fields moved, and its `when` holds over the row after,
            // reading the row before; an ended group fires nothing.
            for t in triggers.into_iter().filter(|_| row.is_some()) {
                let row = row.as_ref().expect("a row");
                let (had, was) = kept::before(&at, staged, (view, key))?;
                let put = was.as_ref().is_none_or(|was| was.fields != row.fields);
                if put && self.fires(t, was.as_ref(), row)? {
                    let body = self.firing(t, (view, key), (change, !had));
                    fired.push((t, body));
                }
            }
            for s in sets {
                let of = (s.name.clone(), view.clone(), key.clone());
                let was = match mine.get(&of) {
                    Some(m) => *m,
                    None => self.pending.member(self.at.view, &s.name, view, key)?,
                };
                let is = match &row {
                    Some(r) => self.member(s, r, at.settings)?,
                    None => false,
                };
                if was == is {
                    continue;
                }
                mine.insert(of.clone(), is);
                let change = (of.0, of.1, of.2, is);
                out.push((
                    self.call.changed(change.clone(), self.next, self.at.now),
                    Some(change),
                ));
                if self.bound && self.pending.across + out.len() > max {
                    let field = named(s).then(|| s.name.as_str().into());
                    let reason = Refusal::TooLarge { max: max as u32 };
                    return Err(Error::refused(field, reason));
                }
            }
        }
        // The firings land after the memberships, as across an edge.
        for (t, body) in fired {
            let draft = Draft {
                cause: Some(self.next),
                context: self.call.context,
                ..Draft::system(self.at.now, body)
            };
            out.push((draft, None));
            if self.bound && self.pending.across + out.len() > max {
                let shown = sees.shows(&Declaration::Trigger(t.clone()));
                let field = shown.then(|| t.name.as_str().into());
                let reason = Refusal::TooLarge { max: max as u32 };
                return Err(Error::refused(field, reason));
            }
        }
        Ok(())
    }
}

/// What a set of declarations moves under the views: the types it declares,
/// the fields it computes again by type, and the settings it leaves.
type Moves<'a> = (
    &'a BTreeSet<TypeName>,
    &'a BTreeMap<TypeName, BTreeSet<FieldName>>,
    &'a BTreeMap<String, Value>,
);

impl Call<'_> {
    /// The memberships a set of declarations moves in the sets of views,
    /// decided in its append and caused by its last declaration at `cause`,
    /// over `after`, the lake it leaves: of a set it declares or declares
    /// again otherwise, of one that names such a set, and of every set of
    /// a view it makes again or whose `where` names a setting it stamps
    /// again, `declared` being the types it declares. Each is decided over
    /// every group of its view, as the fold's hooks leave it. A set of a
    /// view the lake would not keep is refused, and so is a trigger on one,
    /// and a change of a field such a view reads that the lake computes
    /// again. A trigger decides nothing at its declaration.
    pub(super) fn viewed_sets(
        &self,
        (w, after): (&Writer, &Ontology),
        (declared, again, settings): Moves<'_>,
        (cause, now): (Position, Timestamp),
    ) -> Result<Vec<Draft>> {
        let all: Vec<Reader> = readers(after).collect();
        if all.is_empty() {
            return Ok(Vec::new());
        }
        let sets: Vec<&ObjectSetDecl> = all
            .iter()
            .filter_map(|r| match r {
                Reader::Set(s) => Some(*s),
                Reader::Trigger(_) => None,
            })
            .collect();
        let sees = Sight::held(self.who, after);
        let shown = |s: &ObjectSetDecl| sees.shows(&Declaration::ObjectSet(s.clone()));
        // A field such a view reads that the set of declarations computes
        // again changes records the write path does not hold: refused
        // naming the first reader of it the author is shown.
        let stepped = all.iter().filter_map(|r| {
            let reads = sources_read(after, r.view()).into_iter();
            let mut moved = reads.filter(|(s, f)| again.get(s).is_some_and(|a| !a.is_disjoint(f)));
            moved.next().map(|(source, _)| (*r, source))
        });
        let stepped: Vec<(Reader, TypeName)> = stepped.collect();
        if let Some((r, source)) = stepped.iter().find(|(r, _)| sees.shows(&r.declaration())) {
            let ty = r.view().clone();
            let source = source.clone();
            return Err(r.refusal(Case::ViewSourceStepped { ty, source }));
        }
        if let Some((_, source)) = stepped.into_iter().next() {
            return Err(match declared.contains(&source) {
                true => Error::invalid(Case::NoLongerDeclarable { ty: source }),
                false => Error::not_granted(),
            });
        }
        // A view whose setting, or a setting its expressions name, the set
        // stamps again is made again like one it declares.
        let stamped = |k: &String| w.settings.get(k) != settings.get(k);
        let stamped: BTreeSet<&String> = settings.keys().filter(|k| stamped(k)).collect();
        let mut declared = declared.clone();
        for t in after
            .types
            .values()
            .map(|e| &e.decl)
            .filter(|t| t.view.is_some())
        {
            let kept = format!("{}{}", lakelands_api::seam::VIEW_KEPT_SETTING, t.name);
            let named = kept::settings_read(after, t);
            if stamped.contains(&kept) || named.iter().any(|n| stamped.contains(n)) {
                declared.insert(t.name.clone());
            }
        }
        let declared = &declared;
        let viewed = Viewed::of(after);
        let view = self.lake.engine.view(None)?;
        let mut staged = Staged::default();
        let at = At {
            view: view.as_ref(),
            projection: None,
            ontology: after,
            settings: &held((&viewed.views, &viewed.named), settings),
        };
        let moved = match declared.is_empty() {
            true => kept::Moved::new(),
            false => kept::step(&at, &mut staged, None, declared, cause)?,
        };
        let changed = |s: &ObjectSetDecl| w.ontology.set(&s.name) != Some(s);
        // A trigger on a view the lake would not keep could never fire.
        for r in &all {
            let Reader::Trigger(t) = r else { continue };
            if !kept::whole(&at, &staged, &t.on)? {
                let ty = t.on.clone();
                // One that watched it before is told the keep stays.
                let was = w.ontology.triggers.get(t.name.as_str());
                let was = was.is_some_and(|e| e.decl.on == t.on);
                return Err(match (sees.shows(&r.declaration()), was) {
                    (false, _) => Error::not_granted(),
                    (true, true) => r.refusal(Case::ViewKeptForSet { ty }),
                    (true, false) => r.refusal(Case::WatchesView { ty }),
                });
            }
        }
        let max = self.lake.limits.max_set_changes as usize;
        let mut out = Vec::new();
        // A set whose `where` names a setting the set stamps again.
        let asked = stamped.iter().filter_map(|k| viewed.asked.get(*k));
        let asked: BTreeSet<&TypeName> = asked.flatten().collect();
        for s in &sets {
            if !kept::whole(&at, &staged, &s.of)? {
                let ty = s.of.clone();
                let r = Reader::Set(s);
                return Err(match (shown(s), w.ontology.set(&s.name)) {
                    (false, _) => Error::not_granted(),
                    (true, Some(_)) => r.refusal(Case::ViewKeptForSet { ty }),
                    (true, None) => r.refusal(Case::SetOfView { ty }),
                });
            }
            let names = |m: &&ObjectSetDecl| changed(m) && declare::names_set(after, s, &m.name);
            let remade = moved.iter().any(|(v, _)| v == &s.of) || asked.contains(&s.of);
            if !changed(s) && !remade && !sets.iter().any(names) {
                continue;
            }
            let held = match w.ontology.set(&s.name) {
                Some(_) => view.members(&s.name, &s.of)?,
                None => BTreeSet::new(),
            };
            // Every group the view holds, then every member it holds no more.
            let mut found = BTreeMap::new();
            at_head(view.as_ref(), self.lake.limits.max_depth, |heads| {
                staged.each(at.view, &lakelands_api::seam::kept_name(&s.of), |r| {
                    let is = match kept::face(&at, &staged, &s.of, &r.key)? {
                        Some(r) => member_of((s, after), &r, settings, now, heads)?,
                        None => false,
                    };
                    found.insert(r.key, is);
                    Ok(())
                })
            })?;
            let gone = held.iter().filter(|k| !found.contains_key(*k));
            let gone: Vec<(RecordKey, bool)> = gone.map(|k| (k.clone(), false)).collect();
            for (key, is) in found.into_iter().chain(gone) {
                if held.contains(&key) == is {
                    continue;
                }
                if out.len() >= max {
                    let field = shown(s).then(|| s.name.as_str().into());
                    let reason = Refusal::TooLarge { max: max as u32 };
                    return Err(Error::refused(field, reason));
                }
                let change = (s.name.clone(), s.of.clone(), key, is);
                out.push(self.changed(change, cause, now));
            }
        }
        Ok(out)
    }
}

/// Each type `ty`, a view, reads records of, through the views it reads
/// too, with the fields it reads there.
pub(crate) fn sources_read(o: &Ontology, ty: &TypeName) -> Vec<(TypeName, BTreeSet<FieldName>)> {
    let mut out = Vec::new();
    let mut next = vec![ty.clone()];
    let mut seen = BTreeSet::new();
    while let Some(ty) = next.pop() {
        let Some(v) = viewed(o, &ty).and_then(|d| d.view.as_deref()) else {
            continue;
        };
        if !seen.insert(ty) {
            continue;
        }
        for (source, fields) in describe::view_reads(v).unwrap_or_default() {
            next.push(source.clone());
            out.push((source, fields));
        }
    }
    out
}

/// The fields of each type whose values a migration or a value type's
/// moves, by type; `None` for every field of it.
pub(crate) type Stepped = BTreeMap<TypeName, Option<BTreeSet<FieldName>>>;

/// Refuses a migration or a value type's that moves, by `stepped`, a value
/// the view of a set or a trigger reads: a field its sources read there,
/// or any field where one they read is computed. Its step changes records
/// the write path does not hold, so it is refused naming the first such
/// set or trigger its author is shown; where it is shown none, as no
/// longer declarable where it migrates the type `changed`, and as not
/// granted where it renames a value type's values. One that moves nothing a view reads makes the view again with
/// the same records, and moves no membership.
pub(crate) fn stepped_under(
    o: &Ontology,
    (stepped, changed): (&Stepped, Option<&TypeName>),
    who: &Principal,
) -> Result<()> {
    let computed = |t: &TypeName, f: &FieldName| {
        let decl = o.ty(t).ok().and_then(|t| t.field(f));
        decl.is_some_and(|d| d.derived.is_some() || d.function.is_some() || d.embed.is_some())
    };
    let moves = |t: &TypeName, read: &BTreeSet<FieldName>| match stepped.get(t) {
        Some(None) => true,
        Some(Some(f)) => {
            !f.is_disjoint(read) || (!f.is_empty() && read.iter().any(|r| computed(t, r)))
        }
        None => false,
    };
    let stepped = readers(o).filter_map(|r| {
        let mut reads = sources_read(o, r.view()).into_iter();
        reads
            .find(|(t, f)| moves(t, f))
            .map(|(source, _)| (r, source))
    });
    let stepped: Vec<(Reader, TypeName)> = stepped.collect();
    let sees = Sight::held(who, o);
    if let Some((r, source)) = stepped.iter().find(|(r, _)| sees.shows(&r.declaration())) {
        let (ty, source) = (r.view().clone(), source.clone());
        return Err(r.refusal(Case::ViewSourceStepped { ty, source }));
    }
    match (stepped.is_empty(), changed) {
        (true, _) => Ok(()),
        (false, Some(ty)) => Err(Error::invalid(Case::NoLongerDeclarable { ty: ty.clone() })),
        (false, None) => Err(Error::not_granted()),
    }
}
