//! An object set of a view the lake keeps
//! (`docs/decisions/066-a-set-holds-a-kept-view.md`).
//!
//! A membership lands in the append of the write that moves it, and the
//! fold never appends (049), so the write path computes the view's rows
//! itself: it runs the fold's own hooks over a [`Staged`] of its own and no
//! projection, from the same records the fold will read, and decides each
//! group they put or end. A set of declarations, or a setting a view's
//! expressions name, makes the view again the same way; a setting a set's
//! `where` names decides every group of the set's view.

use super::*;
use crate::kept::{self, At};
use crate::lake::Staged;

/// What one append holds of the views its sets of views read: the fold's
/// hooks run over its own [`Staged`], with the settings as its writes
/// leave them. A write refused is undone in it ([`kept::undo`]).
#[derive(Default)]
pub(crate) struct Viewed {
    /// The views a set holds, and every view they read through.
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
    /// What the sets of views of `o` read.
    fn of(o: &Ontology) -> Viewed {
        let mut v = Viewed::default();
        let sets = o.object_sets.values().map(|e| &e.decl);
        let mut next = Vec::new();
        for s in sets.filter(|s| viewed(o, &s.of).is_some()) {
            next.push(s.of.clone());
            let at = [Place::ObjectSet {
                name: s.name.clone(),
            }];
            // A filter that does not expand decides nothing here: deciding
            // a membership refuses it.
            let mut asked = BTreeSet::new();
            if let Ok(e) = declare::set_filter(&|n| o.set(n), s, &at) {
                kept::settings_named(&e, &mut asked);
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
    /// The memberships a write that leaves `after` (it was `before`) moves
    /// in the sets of the views that read it, after what it sets off
    /// across an edge: each group the fold's hooks put or end at its event
    /// is decided on its whole face, and counted with `out` toward
    /// `max_set_changes`; at a write of a setting a set's `where` names,
    /// every group of its view. A write that ends the keeping of such a
    /// view is refused. A write of a type no such view reads pays nothing.
    pub(super) fn viewed(
        &mut self,
        (before, after): (Option<&StoredRecord>, &StoredRecord),
        out: &mut Vec<(Draft, Option<Change>)>,
    ) -> Result<()> {
        let w = self.w;
        let mut v = match self.pending.viewed.take() {
            Some(v) => v,
            None => Box::new(Viewed::of(&w.ontology)),
        };
        let setting = after.ty.as_str() == SETTING_TYPE;
        let moves = v.reads.contains(&after.ty) || setting && v.named.contains(after.key.as_str());
        let decided = match moves {
            true => self.viewed_by(&mut v, (before, after), out),
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
        (before, after): (Option<&StoredRecord>, &StoredRecord),
        out: &mut Vec<(Draft, Option<Change>)>,
    ) -> Result<()> {
        let w = self.w;
        let settings = (v.settings).get_or_insert_with(|| held((&v.views, &v.named), &w.settings));
        let setting = after.ty.as_str() == SETTING_TYPE;
        let was = setting.then(|| settings.clone());
        if setting {
            // As the fold takes it in before its hooks.
            match after.fields.get("value").filter(|_| !after.retired) {
                Some(value) => settings.insert(after.key.to_string(), value.clone()),
                None => settings.remove(after.key.as_str()),
            };
        }
        v.staged.undo = Some(kept::Undo::default());
        let decided = self.viewed_run(v, (before, after), out);
        match decided {
            Ok(()) => v.staged.undo = None,
            Err(_) => {
                kept::undo(&mut v.staged);
                if was.is_some() {
                    v.settings = was;
                }
            }
        }
        decided
    }

    /// The hooks run over `v` for the write, and the memberships of every
    /// group they moved decided into `out`.
    fn viewed_run(
        &self,
        v: &mut Viewed,
        (before, after): (Option<&StoredRecord>, &StoredRecord),
        out: &mut Vec<(Draft, Option<Change>)>,
    ) -> Result<()> {
        let w = self.w;
        let at = At {
            view: self.view,
            projection: None,
            ontology: &w.ontology,
            settings: v.settings.as_ref().expect("the settings are held"),
        };
        let none = BTreeSet::new();
        let staged = &mut v.staged;
        let mut moved = kept::step(&at, staged, Some((before, after)), &none, self.next)?;
        let sees = Sight::held(self.call.who, &w.ontology);
        let max = self.call.lake.limits.max_set_changes as usize;
        let named = |s: &ObjectSetDecl| sees.shows(&Declaration::ObjectSet(s.clone()));
        // Only a setting's write ends a view's keeping.
        let setting = (after.ty.as_str() == SETTING_TYPE).then(|| after.key.as_str());
        for view in v.views.iter().filter(|_| setting.is_some()) {
            if !kept::whole(&at, staged, view)? {
                let reads = |s: &&ObjectSetDecl| {
                    &s.of == view
                        || sources_read(&w.ontology, &s.of)
                            .iter()
                            .any(|(t, _)| t == view)
                };
                let held = w.ontology.object_sets.values().map(|e| &e.decl);
                return Err(match held.filter(reads).find(|s| named(s)) {
                    Some(s) => refused_for(s, Case::ViewKeptForSet { ty: view.clone() }),
                    None => refused(Refusal::NotGranted),
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
            for s in w.ontology.object_sets.values().map(|e| &e.decl) {
                if sources_read(&w.ontology, &s.of)
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
        for (view, key) in &moved {
            let sets = (w.ontology.object_sets.values()).map(|e| &e.decl);
            let sets: Vec<&ObjectSetDecl> = sets.filter(|s| &s.of == view).collect();
            if sets.is_empty() {
                continue;
            }
            let row = kept::face(&at, staged, view, key)?;
            for s in sets {
                let of = (s.name.clone(), view.clone(), key.clone());
                let was = match mine.get(&of) {
                    Some(m) => *m,
                    None => self.pending.member(self.view, &s.name, view, key)?,
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
                    self.call.changed(change.clone(), self.next, self.now),
                    Some(change),
                ));
                if self.bound && self.pending.across + out.len() > max {
                    let field = named(s).then(|| s.name.as_str().into());
                    let reason = Refusal::TooLarge { max: max as u32 };
                    return Err(Error::Refused { field, reason });
                }
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
    /// view the lake would not keep is refused, and so is a change of a
    /// field such a view reads that the lake computes again.
    pub(super) fn viewed_sets(
        &self,
        (w, after): (&Writer, &Ontology),
        (declared, again, settings): Moves<'_>,
        (cause, now): (Position, Timestamp),
    ) -> Result<Vec<Draft>> {
        let sets = after.object_sets.values().map(|e| &e.decl);
        let sets: Vec<&ObjectSetDecl> = sets.filter(|s| viewed(after, &s.of).is_some()).collect();
        if sets.is_empty() {
            return Ok(Vec::new());
        }
        let sees = Sight::held(self.who, after);
        let shown = |s: &ObjectSetDecl| sees.shows(&Declaration::ObjectSet(s.clone()));
        // A field such a view reads that the set of declarations computes
        // again changes records the write path does not hold.
        for s in &sets {
            for (source, fields) in sources_read(after, &s.of) {
                if again.get(&source).is_some_and(|f| !f.is_disjoint(&fields)) {
                    let case = Case::ViewSourceStepped {
                        ty: s.of.clone(),
                        source: source.clone(),
                    };
                    return Err(match shown(s) {
                        true => refused_for(s, case),
                        false if declared.contains(&source) => {
                            invalid(Case::NoLongerDeclarable { ty: source })
                        }
                        false => refused(Refusal::NotGranted),
                    });
                }
            }
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
        let max = self.lake.limits.max_set_changes as usize;
        let mut out = Vec::new();
        // A set whose `where` names a setting the set stamps again.
        let asked = stamped.iter().filter_map(|k| viewed.asked.get(*k));
        let asked: BTreeSet<&TypeName> = asked.flatten().collect();
        for s in &sets {
            if !kept::whole(&at, &staged, &s.of)? {
                let ty = s.of.clone();
                return Err(match (shown(s), w.ontology.set(&s.name)) {
                    (false, _) => refused(Refusal::NotGranted),
                    (true, Some(_)) => refused_for(s, Case::ViewKeptForSet { ty }),
                    (true, None) => refused_for(s, Case::SetOfView { ty }),
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
                    return Err(Error::Refused { field, reason });
                }
                let change = (s.name.clone(), s.of.clone(), key, is);
                out.push(self.changed(change, cause, now));
            }
        }
        Ok(out)
    }
}

/// The refusal, by `case`, of a change that would leave the set `s`
/// without its rows, to an author shown the set: said of the set where it
/// moves a source, and of its `of` where the view would not be kept.
fn refused_for(s: &ObjectSetDecl, case: Case) -> Error {
    let place = Place::ObjectSet {
        name: s.name.clone(),
    };
    let said = Said::from(case).within(place);
    match said.case {
        Case::ViewSourceStepped { .. } => invalid(said),
        _ => declare::invalid_field(&"of".into(), said),
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
/// a set's view reads: a field its sources read there, or any field where
/// one they read is computed. Its step changes records the write path does
/// not hold, so it is refused naming the first such set where its author
/// is shown it; to another, as no longer declarable where it migrates the
/// type `changed`, and as not granted where it renames a value type's
/// values. One that moves nothing a view reads makes the view again with
/// the same records, and moves no membership.
pub(crate) fn stepped_under(
    o: &Ontology,
    (stepped, changed): (&Stepped, Option<&TypeName>),
    who: &Principal,
) -> Result<()> {
    let computed = |t: &TypeName, f: &FieldName| {
        let decl = o
            .ty(t)
            .ok()
            .and_then(|t| t.fields.iter().find(|d| &d.name == f));
        decl.is_some_and(|d| d.derived.is_some() || d.function.is_some() || d.embed.is_some())
    };
    let moves = |t: &TypeName, read: &BTreeSet<FieldName>| match stepped.get(t) {
        Some(None) => true,
        Some(Some(f)) => {
            !f.is_disjoint(read) || (!f.is_empty() && read.iter().any(|r| computed(t, r)))
        }
        None => false,
    };
    let sets = o.object_sets.values().map(|e| &e.decl);
    for s in sets.filter(|s| viewed(o, &s.of).is_some()) {
        let reads = sources_read(o, &s.of).into_iter();
        if let Some((source, _)) = reads.into_iter().find(|(t, f)| moves(t, f)) {
            let shown = Sight::held(who, o).shows(&Declaration::ObjectSet(s.clone()));
            let ty = s.of.clone();
            return Err(match (shown, changed) {
                (true, _) => refused_for(s, Case::ViewSourceStepped { ty, source }),
                (false, Some(ty)) => invalid(Case::NoLongerDeclarable { ty: ty.clone() }),
                (false, None) => refused(Refusal::NotGranted),
            });
        }
    }
    Ok(())
}
