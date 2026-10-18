//! The memberships a fold decides itself (lakelands#1232, 065).
//!
//! A set whose `where` reads across an edge (055), and a set of a kept
//! view (066), hold memberships the log decided at a write over the values
//! then. From the first value a fold of the log erased that such a set
//! reads to the end of that fold, its logged `object_set_changed` no longer
//! stands: the fold decides it from its own state where a write decides
//! it, at a record event of its type or one moving what it reads across an
//! edge ([`across`]), at a group of its view the hooks put or end
//! ([`viewed`]), and over all its records after a declaration, a migration
//! or a setting that moves what it reads. A fold that begins midway in a
//! lake with an act cannot tell what was erased before: it decides every
//! such set from its first event.

use super::across::{self, Written};
use super::*;
use crate::kept::{self, At, Moved};
use crate::lake::Staged;
use lakelands_api::engine::ProjectionStore;

/// What one fold of the log holds of the sets it decides itself.
#[derive(Default)]
pub(crate) struct Zone {
    /// The fields it erased values of, by type, by their names now.
    erased: BTreeSet<(TypeName, FieldName)>,
    /// It began midway in a lake with an act.
    midway: bool,
    /// The sets it decides itself.
    touched: BTreeSet<String>,
    /// What each set of a view or across an edge reads: read again after
    /// a declaration.
    sets: Option<Vec<Reads>>,
    /// Sets to decide over all their records at the next flush, with the
    /// position and the time of the event that moved what they read.
    whole: BTreeMap<String, (Position, Timestamp)>,
}

/// What one set reads: whether it holds a kept view, the fields its
/// `where` and its view's sources read by type, its type and theirs, and
/// the settings its `where` names.
struct Reads {
    name: String,
    view: bool,
    fields: BTreeSet<(TypeName, FieldName)>,
    types: BTreeSet<TypeName>,
    settings: BTreeSet<String>,
}

/// The fold's state a decision reads, the lake's `max_depth`, and the
/// position and time of the event it lands at.
#[derive(Clone, Copy)]
pub(crate) struct Folding<'a> {
    pub ontology: &'a Ontology,
    pub settings: &'a BTreeMap<String, Value>,
    pub view: &'a dyn ProjectionView,
    pub projection: &'a dyn ProjectionStore,
    pub max: u32,
    pub at: (Position, Timestamp),
}

impl Zone {
    /// A fold's, beginning where `o` is in force, `midway` in a lake with
    /// an act.
    pub(crate) fn new(o: &Ontology, midway: bool) -> Zone {
        let mut zone = Zone {
            midway,
            ..Zone::default()
        };
        if midway {
            zone.touch(o, None);
        }
        zone
    }

    /// Whether it decides `set`, whose logged changes do not stand: one
    /// declared now as a set it decides.
    pub(crate) fn decides(&self, set: &str) -> bool {
        self.mine().any(|r| r.name == set)
    }

    /// Whether it decides a set: the record events ask.
    pub(crate) fn deciding(&self) -> bool {
        !self.touched.is_empty()
    }

    /// Whether it decides a set of a view: the fold's hooks are asked
    /// which groups they move.
    pub(crate) fn viewing(&self) -> bool {
        self.mine().any(|r| r.view)
    }

    /// The sets it decides.
    fn mine(&self) -> impl Iterator<Item = &Reads> {
        let sets = self.sets.iter().flatten();
        sets.filter(|r| self.touched.contains(&r.name))
    }

    /// The fold erased `fields` of a record of `ty`: each set reading one,
    /// or a field computed from one, is decided from here on.
    pub(crate) fn erased(&mut self, o: &Ontology, ty: &TypeName, fields: &[FieldName]) {
        let Ok(decl) = o.ty(ty) else { return };
        let mut gone: BTreeSet<FieldName> = fields.iter().cloned().collect();
        let computed =
            |f: &FieldDecl| f.derived.is_some() || f.function.is_some() || f.embed.is_some();
        while let Some(f) = decl.fields.iter().find(|f| {
            computed(f) && !gone.contains(&f.name) && f.depends_on.iter().any(|d| gone.contains(d))
        }) {
            gone.insert(f.name.clone());
        }
        self.erased
            .extend(gone.into_iter().map(|f| (ty.clone(), f)));
        self.touch(o, None);
    }

    /// Each set reading a value erased so far is decided from here on;
    /// over all its records at the next flush where `whole` names when.
    fn touch(&mut self, o: &Ontology, whole: Option<(Position, Timestamp)>) {
        let sets = self.sets.get_or_insert_with(|| reads(o));
        for r in sets.iter() {
            let reads = self.midway || r.fields.iter().any(|f| self.erased.contains(f));
            if reads
                && self.touched.insert(r.name.clone())
                && let Some(at) = whole
            {
                self.whole.insert(r.name.clone(), at);
            }
        }
    }

    /// After the fold of `e`, whose declaration or setting may move what
    /// a set reads: each set it decides that reads what `e` moves is
    /// decided whole at the next flush, and a declaration reads the sets
    /// again, a set that reads a value erased so far decided whole too.
    pub(crate) fn shaped(&mut self, o: &Ontology, staged: &Staged, e: &Event) {
        let at = (e.position, e.known_at);
        let mut moved: BTreeSet<TypeName> = BTreeSet::new();
        let mut set = None;
        let mut setting = None;
        match &e.body {
            EventBody::Declared { declaration, .. } => {
                moved.extend(crate::lake::computes_again(o, declaration));
                match declaration {
                    Declaration::Type(t) => drop(moved.insert(t.name.clone())),
                    Declaration::ObjectSet(s) => set = Some(&s.name),
                    _ => {}
                }
            }
            EventBody::Migrated { migration, .. } => {
                let ty = &migration.to.name;
                for (old, new) in &migration.renames {
                    if self.erased.remove(&(ty.clone(), old.clone())) {
                        self.erased.insert((ty.clone(), new.clone()));
                    }
                }
                moved = staged.stepped_types();
            }
            EventBody::ValueTypeMigrated { types, .. } => moved.extend(types.iter().cloned()),
            EventBody::RecordUpserted { ty, key, .. }
            | EventBody::RecordIngested { ty, key, .. }
            | EventBody::RecordRetired { ty, key, .. }
                if ty.as_str() == SETTING_TYPE =>
            {
                setting = Some(key.as_str());
            }
            _ => return,
        }
        let names = |r: &Reads, s: &String| {
            &r.name == s
                || o.set(&r.name)
                    .is_some_and(|me| declare::names_set(o, me, s))
        };
        let whole: Vec<String> = (self.mine())
            .filter(|r| {
                !r.types.is_disjoint(&moved)
                    || set.is_some_and(|s| names(r, s))
                    || setting.is_some_and(|k| r.settings.contains(k))
            })
            .map(|r| r.name.clone())
            .collect();
        self.whole.extend(whole.into_iter().map(|s| (s, at)));
        if setting.is_none() {
            self.sets = None;
            if self.midway || !self.erased.is_empty() {
                self.touch(o, Some(at));
            }
        }
    }

    /// A record of the lake was `before` and is `after` at the event: each
    /// set it decides of that record's type, and each whose `where`
    /// reaches it across an edge, decided as a write decides it.
    pub(crate) fn record(
        &self,
        f: Folding<'_>,
        staged: &mut Staged,
        (before, after): (Option<&StoredRecord>, &StoredRecord),
    ) -> Result<()> {
        let o = f.ontology;
        let was = before.filter(|b| !b.retired);
        let is = Some(after).filter(|a| !a.retired);
        let moved = |r: &across::Reach| {
            was.is_some() != is.is_some()
                || (r.reads.iter())
                    .any(|f| was.and_then(|b| b.fields.get(f)) != is.and_then(|a| a.fields.get(f)))
        };
        for r in self
            .mine()
            .filter(|r| !r.view && r.types.contains(&after.ty))
        {
            let Some(s) = o.set(&r.name) else { continue };
            let (Ok(on), Ok(e)) = (o.ty(&s.of), declare::set_filter(&|n| o.set(n), s, &[])) else {
                continue;
            };
            let mut found = Vec::new();
            if s.of == after.ty {
                found.push(after.clone());
            }
            // Paths that step back alike are walked once.
            let reached = across::reaches(o, on, &e);
            let mut walked = Vec::new();
            for x in reached.iter().filter(|x| x.ty == &after.ty && moved(x)) {
                if walked.contains(&&x.back) {
                    continue;
                }
                walked.push(&x.back);
                let back = across::back((f.view, &*staged), &x.back, after.clone())?;
                found.extend(
                    back.into_iter()
                        .filter(|b| s.of != after.ty || b.key != after.key),
                );
            }
            for rec in found {
                let is = member(f, staged, s, &rec)?;
                put(f, staged, s, &rec.key, is)?;
            }
        }
        Ok(())
    }

    /// The groups of kept views `moved` at the event: each set of their
    /// view it decides, decided on the group's whole face.
    pub(crate) fn groups(&self, f: Folding<'_>, staged: &mut Staged, moved: &Moved) -> Result<()> {
        let sets: Vec<&ObjectSetDecl> = (self.mine().filter(|r| r.view))
            .filter_map(|r| f.ontology.set(&r.name))
            .collect();
        for (view, key) in moved {
            for s in sets.iter().filter(|s| &s.of == view) {
                let is = grouped(f, staged, s, key)?;
                put(f, staged, s, key, is)?;
            }
        }
        Ok(())
    }

    /// Each set to be decided whole, over every record of its type or
    /// every group of its view as the fold leaves them, at the event that
    /// moved what it reads; a member found in neither holds no more.
    pub(crate) fn decide_whole(&mut self, f: Folding<'_>, staged: &mut Staged) -> Result<()> {
        for (name, at) in std::mem::take(&mut self.whole) {
            let Some(s) = f.ontology.set(&name) else {
                continue;
            };
            let f = Folding { at, ..f };
            let view = f.ontology.ty(&s.of).is_ok_and(|t| t.view.is_some());
            let ty = match view {
                true => lakelands_api::seam::kept_name(&s.of),
                false => s.of.clone(),
            };
            let mut decided = BTreeMap::new();
            staged.each(f.view, &ty, |r| {
                let is = match view {
                    true => grouped(f, staged, s, &r.key)?,
                    false => member(f, staged, s, &r)?,
                };
                decided.insert(r.key, is);
                Ok(())
            })?;
            for key in staged.members(f.view, &s.name, (&s.of, &s.of))? {
                decided.entry(key).or_insert(false);
            }
            for (key, is) in decided {
                put(f, staged, s, &key, is)?;
            }
        }
        Ok(())
    }
}

/// What each set of `o` whose `where` reads across an edge, or that holds
/// a kept view, reads.
fn reads(o: &Ontology) -> Vec<Reads> {
    let mut out = Vec::new();
    for s in o.object_sets.values().map(|e| &e.decl) {
        let (Ok(on), Ok(e)) = (o.ty(&s.of), declare::set_filter(&|n| o.set(n), s, &[])) else {
            continue;
        };
        let reached = across::reaches(o, on, &e);
        if on.view.is_none() && reached.is_empty() {
            continue;
        }
        let mut r = Reads {
            name: s.name.clone(),
            view: on.view.is_some(),
            fields: BTreeSet::new(),
            types: BTreeSet::from([s.of.clone()]),
            settings: BTreeSet::new(),
        };
        // A derived relation's own side reads its `from` fields.
        for f in lakelands_core::derive::reads(&e) {
            if let Some(d) = on.derived_step(&f).and_then(|r| r.derived.as_ref()) {
                let [from, _] = d.reads();
                r.fields.extend(from.into_iter().map(|f| (s.of.clone(), f)));
            }
            r.fields.insert((s.of.clone(), f));
        }
        let far = reached.iter().map(|x| (x.ty.clone(), x.reads.clone()));
        for (t, fields) in far.chain(viewed::sources_read(o, &s.of)) {
            r.fields.extend(fields.into_iter().map(|f| (t.clone(), f)));
            r.types.insert(t);
        }
        kept::settings_named(&e, &mut r.settings);
        out.push(r);
    }
    out
}

/// Whether `r` is a member of `s` as the fold leaves what its `where`
/// reads, at the event.
fn member(f: Folding<'_>, staged: &Staged, s: &ObjectSetDecl, r: &StoredRecord) -> Result<bool> {
    let heads = (f.view, staged as &dyn Written, f.max);
    across::heads(heads, |h| {
        member_of((s, f.ontology), r, f.settings, f.at.1, h)
    })
}

/// Whether the group at `key` of `s`'s view is a member of `s`, on its
/// whole face as the fold leaves it.
fn grouped(f: Folding<'_>, staged: &Staged, s: &ObjectSetDecl, key: &RecordKey) -> Result<bool> {
    let at = At {
        view: f.view,
        projection: None,
        ontology: f.ontology,
        settings: f.settings,
    };
    match kept::face(&at, staged, &s.of, key)? {
        Some(row) => member(f, staged, s, &row),
        None => Ok(false),
    }
}

/// Put `key`'s membership of `s` at the event, where it changes.
fn put(
    f: Folding<'_>,
    staged: &mut Staged,
    s: &ObjectSetDecl,
    key: &RecordKey,
    is: bool,
) -> Result<()> {
    if staged.is_member(f.view, &s.name, &s.of, key)? != is {
        staged.member(f.projection, &s.name, &s.of, key, is, f.at.0)?;
    }
    Ok(())
}
