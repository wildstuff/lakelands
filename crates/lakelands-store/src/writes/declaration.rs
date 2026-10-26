//! The declaration doors: a declaration, a bundle of them and a setting,
//! judged as their author's view and appended.

use super::*;
use crate::record;

impl Call<'_> {
    /// Refuse each type of `set` declared again with a change, as its
    /// migration would be refused for a verb that reads into it
    /// ([`verbs_judged_again`]): over `held`, the lake's own, with the set
    /// in it as its author's view is completed.
    pub(super) fn verbs_still_read(&self, held: &Ontology, set: &[Declaration]) -> Result<()> {
        let changed = set.iter().filter(|d| !held.in_force(d));
        let changed: Vec<_> = (changed.filter_map(|d| match d {
            Declaration::Type(t) => Some((&held.types.get(&t.name)?.decl, t, d.place())),
            _ => None,
        }))
        .collect();
        // No verb on or into a changed type, in force or in the set, and no
        // set or trigger reading into it across an edge: nothing is judged,
        // and the lake is not copied.
        let verbs = || {
            let sent = set.iter().filter_map(|d| match d {
                Declaration::Verb(v) => Some(v),
                _ => None,
            });
            (held.verbs.values().map(|e| &e.decl)).chain(sent)
        };
        let judged = |(was, t, _): &(&TypeDecl, &TypeDecl, Place)| {
            verbs().any(judged_by(held, was, t)) || !across_judged(held, was, t).is_empty()
        };
        if !changed.iter().any(judged) {
            return Ok(());
        }
        let mut after = held.clone();
        set.iter()
            .for_each(|d| put_as_appended(&mut after, d, Some));
        let sees = Sight::held(self.who, held);
        for (was, t, at) in changed {
            let judged = verbs_judged_again((held, &after), was, t, &|d| sees.shows(d));
            judged.map_err(|said| Error::invalid(said).within(at))?;
        }
        Ok(())
    }

    /// `recompute`: the declaration in force appended again as it was
    /// declared, at its next version, so that the fold does at this event
    /// what it does at any `declared` of it (022, extended). A deliberate
    /// act: nothing is compared, and the log shows it.
    pub fn recompute(&self, what: &Recompute) -> Result<Outcome> {
        self.enacting()?;
        let what = what.clone();
        self.make(
            lakelands_api::write::Change::Recompute { what },
            Against::default(),
        )
    }

    /// The declaration `recompute` appends again: the one in force that
    /// `what` names, whose purpose is the calls, so it needs the backend
    /// whatever is in force.
    pub(super) fn recomputed(&self, w: &Writer, what: &Recompute) -> Result<Declaration> {
        let declaration = match what {
            Recompute::Function(name) => match w.ontology.functions.get(name) {
                Some(f) => Declaration::Function(f.decl.clone()),
                None => {
                    let missing = Missing::Function { name: name.clone() };
                    return Err(Error::NotFound { missing });
                }
            },
            Recompute::Type(name) => {
                let t = Sight::held(self.who, &w.ontology).ty(name)?;
                if t.view.is_some() || !crate::backed::computed_outside(&w.ontology, t) {
                    let ty = name.clone();
                    return Err(Error::invalid(Case::NothingComputedOutside { ty }));
                }
                Declaration::Type(w.ontology.as_declared(name)?)
            }
        };
        self.check_bound(&declaration, None, None)?;
        Ok(declaration)
    }

    /// `apply`: a set of declarations validated together and appended in
    /// one append, or refused whole. Each declaration is checked against
    /// the lake's ontology with every other declaration of the set already
    /// in it, so a reference may point forward within the set and tool
    /// names are checked over all of it; every problem is collected, and
    /// one refusal names them all. Of a set that is admitted, what is in
    /// force before it is not appended again (027, extended).
    pub fn apply(&self, declarations: Vec<Declaration>) -> Result<Outcome> {
        self.enacting()?;
        let change = lakelands_api::write::Change::Declare { declarations };
        self.make(change, Against::default())
    }

    /// A declaration reads only what its author may read (024 and 028,
    /// extended). The set is checked as sent against the ontology as its
    /// author is shown it, so a field the author may not know of answers as
    /// one the type does not have, whatever reads it. Then a field it knows
    /// of and may not read is refused as not granted, where what its author
    /// adds or changes reads its value. Before either, a declaration under
    /// a name, or with a tool's name, that what its author is not shown
    /// holds is not granted.
    fn reads_as_author(&self, o: &Ontology, sent: &[Declaration], reshapes: bool) -> Result<()> {
        let sees = Sight::held(self.who, o);
        // A constraint under the name of one its author is not told, as
        // `describe_type` tells it, is not granted, naming nothing, before
        // anything judges the type as its author knows it (lakelands#1119).
        for t in sent.iter().filter_map(|d| match d {
            Declaration::Type(t) => o.types.get(&t.name).map(|held| (t, &held.decl)),
            _ => None,
        }) {
            let (t, held) = t;
            let untold = |c: &ConstraintDecl| {
                let e = c.holds.typed().ok();
                !e.is_some_and(|e| {
                    e.fields_read()
                        .into_iter()
                        .all(|f| sees.reads_from(&held.name, f))
                })
            };
            let named_like = |c: &ConstraintDecl| {
                (held.constraints.iter()).any(|h| h.name == c.name && untold(h))
            };
            granted(!t.constraints.iter().any(named_like))?;
        }
        // One who reads every field and holds `schema` is shown the lake
        // whole: its view is the lake, and nothing it adds or changes
        // probes a field it may not read.
        // A migration's `to` is judged for what it needs bound by its
        // preflight, across its renames (lakelands#1124).
        let set = |seen: &Ontology, shaped: &[Declaration]| match self.check_set(
            (seen, o),
            shaped,
            !reshapes,
        ) {
            Err(e) if reshapes => declare::unless_a_migration(e),
            checked => checked,
        };
        if sees.reads_all_fields() && self.who.grants.schema {
            // But what stands in force listed to no one, as a verb whose
            // `requires` does not parse, is not its to declare again; nor is
            // a tool's name that such a verb bears (the slow path's `taken`).
            let computes = |f: &str| {
                let mut types = o.types.values();
                types.any(|e| {
                    e.decl
                        .fields
                        .iter()
                        .any(|x| x.function.as_deref() == Some(f))
                })
            };
            let unlisted = |d: &Declaration| match d {
                Declaration::Type(_) | Declaration::Withdraw(_) => false,
                Declaration::Function(f) if !computes(&f.name) => false,
                d => o.held(d).is_some_and(|x| !sees.lists(&x)),
            };
            granted(!sent.iter().any(unlisted))?;
            tools_as_shown(self.who, o, o, sent)?;
            return set(&unlisted_withdrawn(o, &sees, sent), sent);
        }
        // A type its author is not shown is a type that is not declared,
        // for whatever names it; to declare it is not granted, and names
        // nothing of it.
        let mut seen = types_shown(self.who, o);
        // A set its author is not shown is one that is not declared, as a
        // read of it answers (024): a proposal that names it learns nothing
        // of what its filter reads, and to declare it is not granted, as
        // for a type (027). A holder of `schema` is shown each set of a
        // type it is shown. So for a trigger, as what reads a set.
        let declared = o.declared();
        let sees = Sight::held(self.who, &declared);
        retain_listed(&mut seen, &sees);
        // A withdrawal of what its author is shown, read by what it is not
        // (a function computing a field of a type it is not shown, a set a
        // trigger it is not shown watches), would be refused naming that
        // reader: it is not granted, as declaring it again would not be
        // (lakelands#917).
        let unseen_reader = |d: &Declaration| match d {
            Declaration::Withdraw(w) => {
                o.held(d).is_some_and(|held| sees.lists(&held))
                    && declare::reader(o, w).is_some()
                    && declare::reader(&seen, w).is_none()
            }
            _ => false,
        };
        granted(!sent.iter().any(unseen_reader))?;
        // So a function whose input's kind a type it is not shown holds
        // otherwise: refused for that type, it would name it.
        granted(!sent.iter().any(|d| unseen_binding(o, &seen, d)))?;
        if let std::borrow::Cow::Owned(stripped) = unlisted_withdrawn(&seen, &sees, sent) {
            seen = stripped;
        }
        let hidden = |d: &Declaration| matches!(d, Declaration::Type(t) if o.types.contains_key(&t.name) && seen.ty(&t.name).is_err());
        granted(!sent.iter().any(hidden))?;
        // So for a trigger, a verb and a function that computes a field,
        // known by their name alone: the one in force would be gone with
        // the one declared (lakelands#893). A function stands on the types
        // whose fields it computes: one shown to every holder of `schema`
        // is taken all the same where it computes a field of a type its
        // author is not shown, whose records its new body would compute
        // again (lakelands#917).
        let computed = |x: &FieldDecl, f: &str| x.function.as_deref() == Some(f);
        let computes = |f: &str, unseen: bool| {
            let mut types = o.types.values().map(|e| &e.decl);
            let on = |t: &TypeDecl| !unseen || !seen.types.contains_key(&t.name);
            types.any(|t| on(t) && t.fields.iter().any(|x| computed(x, f)))
        };
        let taken = |d: &Declaration| {
            let mut held = declared.iter().map(|x| &x.declaration);
            let f = match d {
                Declaration::Function(f) => Some(f.name.as_str()),
                _ => None,
            };
            !matches!(d, Declaration::Type(_) | Declaration::Withdraw(_))
                && f.is_none_or(|f| computes(f, false))
                && (f.is_some_and(|f| computes(f, true))
                    || held.any(|x| x.what() == d.what() && !sees.lists(x)))
        };
        granted(!sent.iter().any(taken))?;
        tools_as_shown(self.who, o, &seen, sent)?;
        // A valid time left out where the one in force names a field outside
        // its author's view stands as declared: left out of both sides, it is
        // checked whole at apply. A valid time its author sends, the one in
        // force too, is read against its view, where such a field is no field
        // (027, extended, lakelands#726, lakelands#692). A key and a
        // relation's `via` name fields their author knows of, or the type and
        // the relation are not shown to it (006 and 024, extended).
        let mut shaped = sent.to_vec();
        for d in &mut shaped {
            let Declaration::Type(t) = d else { continue };
            let (Ok(held), Some(e)) = (o.ty(&t.name), seen.types.get_mut(&t.name)) else {
                continue;
            };
            let e = Arc::make_mut(e);
            let view: Vec<FieldName> = e.decl.fields.iter().map(|f| f.name.clone()).collect();
            let unseen = |f: &Option<FieldName>| f.as_ref().is_some_and(|f| !view.contains(f));
            for (sent, kept, was) in [
                (&mut t.valid_from, &mut e.decl.valid_from, &held.valid_from),
                (&mut t.valid_to, &mut e.decl.valid_to, &held.valid_to),
            ] {
                if sent.is_none() && unseen(was) {
                    (*sent, *kept) = (None, None);
                }
            }
        }
        // A migration's shape is refused as a declaration; its preflight is next.
        set(&seen, &shaped)?;
        let mut after = o.clone();
        sent.iter().for_each(|d| after.put(d.clone(), 0, None));
        for d in sent {
            // Only what its author adds or changes: of a type, the fields,
            // the relations and the view that do not stand as they are.
            let key = lakelands_core::engines::declaration_key;
            let mut was = declared.iter().map(|b| &b.declaration);
            let (was, mut new) = (was.find(|b| key(b) == key(d)), d.clone());
            match (&mut new, was) {
                (Declaration::Type(t), Some(Declaration::Type(w))) => {
                    *t = after.ty(&t.name)?.clone();
                    t.fields.retain(|f| !w.fields.contains(f));
                    t.relations.retain(|r| !w.relations.contains(r));
                    t.view = t.view.take().filter(|v| Some(v) != w.view.as_ref());
                }
                (_, Some(w)) if w == d => continue,
                _ => {}
            }
            let named = describe::probed_types(&new);
            for t in named.iter().filter_map(|n| o.ty(n).ok()) {
                let sets = |n: &str| after.set(n).map(|s| s.filter.clone());
                let mut names = describe::declaration_probes(&new, &t.name, &sets);
                // A function's inputs and what an embedding is made from.
                if let Declaration::Type(x) = &new
                    && x.name == t.name
                {
                    let read = |f| crate::backed::computed_reads(&after, f);
                    names.extend(x.fields.iter().filter_map(read).flatten());
                }
                let has = |n: &&FieldName| t.has_field(n);
                sees.check_probe(t, names.iter().filter(has))?;
            }
        }
        Ok(())
    }

    /// A value type's report as the caller may know it (024): a use and a
    /// misfit are shown where it is shown the type and reads the field. The
    /// totals and `fits` stay whole, so that a reader never sees no misfit
    /// beside a report that does not fit, and the id is made over this.
    /// A report that renames values is never trimmed: one not shown every
    /// field it writes is not granted it ([`Call::renames_shown`]).
    pub fn shown_value_type(&self, o: &Ontology, mut report: ValueTypeReport) -> ValueTypeReport {
        // A use is shown in a field the caller reads, of a type it is
        // shown: one keyed by a reference it may not know of is not (024).
        let sees = Sight::held(self.who, o);
        report.fields.retain(|u| sees.reads(&u.ty, &u.field));
        report.first.retain(|m| sees.reads(&m.ty, &m.field));
        // A breach to one who reads every field its constraint reads, as
        // `describe_type` tells it (lakelands#1119).
        report.breaches.retain(|b| {
            let t = o.ty(&b.ty).ok();
            let c = t.and_then(|t| t.constraints.iter().find(|c| c.name == b.constraint));
            let e = c.and_then(|c| c.holds.typed().ok());
            e.is_some_and(|e| {
                e.fields_read()
                    .into_iter()
                    .all(|f| sees.reads_from(&b.ty, f))
            })
        });
        report
    }

    /// Reads a migration as its author's view of its type ([`as_author`]),
    /// and answers that view, for [`Self::preflight_plan`] to judge its
    /// names against.
    pub(super) fn authored(&self, o: &Ontology, m: &mut Migration) -> Result<Option<Authored>> {
        let sees = Sight::held(self.who, o);
        if let Some(before) = o.types.get(&m.to.name) {
            sees.ty(&m.to.name)?;
            reads_stand(&sees, &before.decl, &m.to, Some(m))?;
        }
        self.reads_as_author(o, &[Declaration::Type(m.to.clone())], true)?;
        initials(o, &mut m.to);
        let Some(before) = o.types.get(&m.to.name) else {
            return Ok(None);
        };
        let find = |n: &TypeName| o.ty(n).ok();
        let (sent, mut known) = (m.clone(), before.decl.clone());
        sees.known_fields(&mut known, &find);
        let (to, pairs) = (&mut m.to, &m.relation_renames);
        as_author(&sees, &before.decl, to, pairs)?;
        Ok(Some(Authored { sent, known }))
    }

    /// Reads each type of `declarations` that is declared already as its
    /// author's view of it ([`as_author`]).
    pub(super) fn redeclared(&self, o: &Ontology, declarations: &mut [Declaration]) -> Result<()> {
        let sees = Sight::held(self.who, o);
        for d in declarations.iter() {
            if let Declaration::Type(t) = d
                && let Some(before) = o.types.get(&t.name)
            {
                reads_stand(&sees, &before.decl, t, None)?;
            }
        }
        self.reads_as_author(o, declarations, false)?;
        // An `initial` is read against the value types the set declares.
        let mut after = std::borrow::Cow::Borrowed(o);
        let types = declarations
            .iter()
            .filter(|d| matches!(d, Declaration::ValueType(_)));
        types.for_each(|d| after.to_mut().put(d.clone(), 0, None));
        for d in declarations {
            let Declaration::Type(t) = d else { continue };
            initials(&after, t);
            if let Some(before) = o.types.get(&t.name) {
                as_author(&sees, &before.decl, t, &[])?;
            }
        }
        Ok(())
    }

    /// Check a set of declarations as `apply` checks it, against
    /// `ontology`, whoever applies it: each against the ontology with
    /// every other declaration of the set already in it, every problem
    /// collected into one refusal.
    pub(super) fn check_bundle(
        &self,
        ontology: &Ontology,
        declarations: &[Declaration],
    ) -> Result<()> {
        self.check_set((ontology, ontology), declarations, true)?;
        self.verbs_still_read(ontology, declarations)
    }

    /// [`Self::check_bundle`] against `ontology` as the set's author is
    /// shown `held`, the lake's own; what each needs bound only if `bound`.
    pub(super) fn check_set(
        &self,
        (ontology, held): (&Ontology, &Ontology),
        declarations: &[Declaration],
        bound: bool,
    ) -> Result<()> {
        if declarations.is_empty() {
            return Err(Error::invalid(Case::SetEmpty));
        }
        let identities: Vec<String> = declarations
            .iter()
            .map(lakelands_core::engines::declaration_key)
            .collect();
        let own = types_withdrawn(declarations);
        let mut problems: Vec<(Place, Error)> = Vec::new();
        for (i, id) in identities.iter().enumerate() {
            if identities[..i].contains(id) {
                let twice = Error::invalid(Case::SetDeclaresTwice);
                problems.push((declarations[i].place(), twice));
            }
        }
        for (i, d) in declarations.iter().enumerate() {
            let mut probe = std::borrow::Cow::Borrowed(ontology);
            for (j, other) in declarations.iter().enumerate() {
                if identities[j] != identities[i] {
                    // A link type the lake holds is brought as the set's
                    // author is shown it: not at all where it is not shown
                    // the type, and without a field it is not shown.
                    let shown = |mut t: TypeDecl| {
                        let Some(h) = held.types.get(&t.name) else {
                            return Some(t);
                        };
                        let seen = &ontology.types.get(&t.name)?.decl;
                        let has = |d: &TypeDecl, f: &FieldDecl| d.has_field(&f.name);
                        t.fields.retain(|f| has(seen, f) || !has(&h.decl, f));
                        Some(t)
                    };
                    put_leaving(probe.to_mut(), other, shown, &own);
                }
            }
            let checked = declare::check_declaration(&probe, d, false);
            let checked = checked.and_then(|()| match bound {
                true => self.check_bound(d, Some(ontology), None),
                false => Ok(()),
            });
            if let Err(e) = checked {
                problems.push((d.place(), e));
            }
        }
        match problems.len() {
            0 => Ok(()),
            1 => {
                let (at, e) = problems.remove(0);
                Err(e.within(at))
            }
            count => {
                let problems = problems.into_iter().map(|(at, e)| e.within(at)).collect();
                Err(Error::invalid(Case::SetRefused { count, problems }))
            }
        }
    }

    /// The drafts that declare a checked set: each declaration at its next
    /// version, with the link types its relations need; and the sections
    /// of its plan, of the types it computes again (062). Judged as a
    /// proposal's plan, its sections alone: nothing it finds refuses
    /// it but the records a type withdrawn or declared again finds held,
    /// and its sets are not decided.
    pub(crate) fn bundle_drafts(
        &self,
        w: &Writer,
        declarations: Vec<Declaration>,
        (now, first, judging): (Timestamp, usize, Judging),
    ) -> Result<Bundled> {
        let mut drafts = Vec::with_capacity(declarations.len());
        let mut decided = Vec::new();
        // Only a function, or a type the lake holds, computes records again
        // when it is declared: a set without one is written as before.
        let computes = declarations.iter().any(|d| match d {
            Declaration::Function(_) => true,
            Declaration::Type(t) => w.ontology.types.contains_key(&t.name),
            _ => false,
        });
        // A set is decided over the sets it names as the bundle leaves them,
        // and the lake is copied only for a bundle that declares one.
        let sets = (declarations.iter()).filter(|d| matches!(d, Declaration::ObjectSet(_)));
        let after = sets.clone().next().map(|_| {
            let mut after = w.ontology.clone();
            sets.for_each(|d| after.put(d.clone(), 0, None));
            after
        });
        // The records the bundle computes again, and the lake it leaves.
        let again = declarations.iter();
        let again: BTreeSet<TypeName> =
            again.flat_map(|d| computes_again(&w.ontology, d)).collect();
        let mut fields: BTreeMap<TypeName, BTreeSet<FieldName>> = BTreeMap::new();
        for (ty, f) in declarations
            .iter()
            .flat_map(|d| fields_again(&w.ontology, d))
        {
            fields.entry(ty).or_default().insert(f);
        }
        let whole = (!again.is_empty()).then(|| {
            let mut whole = w.ontology.clone();
            (declarations.iter()).for_each(|d| whole.put(d.clone(), 0, None));
            whole
        });
        // The types it declares, and the lake it leaves where a set holds a
        // view or a trigger watches one, whose sets it decides and whose
        // views it judges (lakelands#1185).
        let declared: BTreeSet<TypeName> = (declarations.iter())
            .filter_map(|d| match d {
                Declaration::Type(t) => Some(t.name.clone()),
                _ => None,
            })
            .collect();
        let of_view = |ty: &TypeName, o: &Ontology| o.ty(ty).is_ok_and(|t| t.view.is_some());
        let reads_view = |o: &Ontology| {
            (o.object_sets.values()).any(|e| of_view(&e.decl.of, o))
                || (o.triggers.values()).any(|e| of_view(&e.decl.on, o))
        };
        // Of those, only a type, a set, a function or a setting moves one.
        let moves = |d: &Declaration| {
            !matches!(
                d,
                Declaration::Verb(_) | Declaration::Trigger(_) | Declaration::ValueType(_)
            )
        };
        // A set or a trigger it declares that reads a view.
        let reads = |d: &Declaration| {
            let ty = match d {
                Declaration::ObjectSet(s) => &s.of,
                Declaration::Trigger(t) => &t.on,
                _ => return false,
            };
            let declared = |d: &Declaration| matches!(d, Declaration::Type(t) if &t.name == ty);
            match declarations.iter().find(|d| declared(d)) {
                Some(Declaration::Type(t)) => t.view.is_some(),
                _ => of_view(ty, &w.ontology),
            }
        };
        let viewing = declarations.iter().any(moves) && reads_view(&w.ontology)
            || declarations.iter().any(reads);
        let left = (viewing && !judging.tells()).then(|| {
            let mut left = w.ontology.clone();
            (declarations.iter()).for_each(|d| left.put(d.clone(), 0, None));
            left
        });
        // A setting's held value is judged by the lake as the bundle
        // leaves it (063, extended).
        let settled = (declarations.iter())
            .any(|d| matches!(d, Declaration::Setting(_)))
            .then(|| {
                let mut whole = w.ontology.clone();
                (declarations.iter()).for_each(|d| whole.put(d.clone(), 0, None));
                whole
            });
        // The types it withdraws itself, whose withdrawals the lake does not
        // append again beside a type's.
        let withdrawn = types_withdrawn(&declarations);
        for declaration in declarations {
            let declared = position_after(w, first + drafts.len());
            // A type's hash and link types, or the memberships a set decides.
            let (schema_hash, with) = match &declaration {
                Declaration::Type(t) => {
                    self.name_held(w, t)?;
                    let hash = shape::schema_hash(t).map_err(Error::invalid)?;
                    (Some(hash), link_type_drafts(&w.ontology, t, now)?)
                }
                // A set of a type computed again is decided by its walk, over
                // the records as the bundle computes them.
                Declaration::ObjectSet(set) if again.contains(&set.of) => (None, Vec::new()),
                Declaration::ObjectSet(_) if judging.tells() => (None, Vec::new()),
                // A set of a view is decided over its kept records, below.
                Declaration::ObjectSet(set)
                    if after.as_ref().is_some_and(|a| of_view(&set.of, a)) =>
                {
                    (None, Vec::new())
                }
                Declaration::ObjectSet(set) => {
                    let after = after.as_ref().expect("a bundle with a set has its copy");
                    let with = (w, after, &mut decided);
                    (None, self.set_drafts(with, set, declared, now)?)
                }
                // A proposal's plan judges no setting's held value: the
                // accept does, and the plan would tell of a value its
                // proposer may not be answered.
                Declaration::Setting(_) if judging.tells() => (None, Vec::new()),
                Declaration::Setting(s) => {
                    let whole = (settled.as_ref()).expect("a bundle with a setting has its copy");
                    (None, self.restamped((w, whole), s, now)?)
                }
                Declaration::Withdraw(x) if x.kind == DeclarationKind::Type => {
                    (None, self.type_withdrawn(w, (&x.name, &withdrawn), now)?)
                }
                _ => (None, Vec::new()),
            };
            let version = w.ontology.next_version(&declaration);
            drafts.push(self.draft(
                now,
                Origin::Authored,
                EventBody::Declared {
                    declaration,
                    schema_hash,
                    version,
                    with_the_set: false,
                },
            ));
            drafts.extend(with);
        }
        // Every `declared` of such a set but the last, the link types the
        // system declares with a type among them, says that the set goes
        // on: the records it computes again are computed once, at its end.
        let last = drafts
            .iter()
            .rposition(|d| matches!(d.body, EventBody::Declared { .. }));
        for d in &mut drafts[..last.unwrap_or(0)] {
            if let EventBody::Declared { with_the_set, .. } = &mut d.body {
                *with_the_set = computes;
            }
        }
        // Their sets are decided in this append, caused by the last
        // declaration, where the fold computes them again (lakelands#941).
        let mut types = Vec::new();
        if let (Some(last), Some(whole)) = (last, &whole) {
            let event = provisional(self.lake, position_after(w, first + last), &drafts[last]);
            types = self.walked_again((w, whole), (&again, &fields), (&event, now, judging))?;
            if !judging.tells() {
                let changes = self.recomputed_sets((w, whole, &decided), &again, &event, now)?;
                drafts.extend(changes);
            }
        }
        if let (Some(last), Some(left)) = (last, &left) {
            let cause = position_after(w, first + last);
            // The settings the bundle stamps again, in the form it keeps.
            let setting = |d: &Draft| match &d.body {
                EventBody::RecordUpserted { ty, key, patch, .. } if ty.as_str() == SETTING_TYPE => {
                    Some((key.to_string(), patch.get("value")?.clone()))
                }
                _ => None,
            };
            let mut settings = w.settings.clone();
            settings.extend(drafts.iter().filter_map(setting));
            let moved = (&declared, &fields, &settings);
            let changes = self.viewed_sets((w, left), moved, (cause, now))?;
            drafts.extend(changes);
        }
        Ok(Bundled { drafts, types })
    }

    /// The withdrawals of the link types the type `name` takes out of force
    /// with it, its relations' (lakelands#1292). Refused while it or one of
    /// them holds a live record, restricted or not: naming them to one
    /// answered one of them, not granted to another (024).
    fn type_withdrawn(
        &self,
        w: &Writer,
        (name, withdrawn): (&str, &BTreeSet<String>),
        now: Timestamp,
    ) -> Result<Vec<Draft>> {
        let ty = TypeName::from(name);
        let o = &w.ontology;
        let links = o.types.get(&ty).map(|e| links::link_types_of(&e.decl));
        let links = links.unwrap_or_default().into_iter().map(|l| l.name);
        let links = links.filter(|l| o.types.contains_key(l) && !withdrawn.contains(l.as_str()));
        let links: Vec<TypeName> = links.collect();
        let view = self.lake.engine.view(None)?;
        let sees = Sight::held(self.who, o);
        for t in std::iter::once(&ty).chain(&links) {
            if view.count(t, &every(false))? == 0 {
                continue;
            }
            let mut told = sees.holds_all();
            if !told {
                live(view.as_ref(), t, |r| {
                    told |= sees.answers(t, r);
                    Ok(())
                })?;
            }
            granted(told)?;
            let by = match links::link_type_parts(t) {
                Some((_, r)) if t != &ty => format!("the links of its relation `{r}`"),
                _ => "its records".to_owned(),
            };
            let (kind, name) = (Naming::Type, name.to_owned());
            return Err(Error::invalid(Case::WithdrawnInUse { kind, name, by }));
        }
        let kind = DeclarationKind::Type;
        let gone = |l: TypeName| {
            Declaration::Withdraw(Withdrawal {
                kind,
                name: l.as_str().to_owned(),
            })
        };
        (links.into_iter())
            .map(|l| system_declared(o, gone(l), now))
            .collect()
    }

    /// Refuse `t` declared again where it, or a link type it brings, was
    /// withdrawn and the lake keeps records of it: they were written to the
    /// shape that left, and would read as the new one's (lakelands#1292).
    fn name_held(&self, w: &Writer, t: &TypeDecl) -> Result<()> {
        let o = &w.ontology;
        let names = std::iter::once(t.clone()).chain(links::link_types_of(t));
        let names = names.filter(|d| !o.types.contains_key(&d.name));
        let names = names.filter(|d| o.next_version(&Declaration::Type(d.clone())) > 1);
        let names: Vec<TypeName> = names.map(|d| d.name).collect();
        if names.is_empty() {
            return Ok(());
        }
        let view = self.lake.engine.view(None)?;
        for ty in names {
            if stored(view.as_ref(), &ty)? > 0 {
                return Err(Error::invalid(Case::WithdrawnNameHeld { ty }));
            }
        }
        Ok(())
    }

    /// The record of a host's setting the lake holds, stamped again in the
    /// declaration's append where the declaration moves it to another
    /// group, and its value rewritten in the form its declared kind keeps;
    /// one whose value that kind refuses is refused, naming the setting.
    /// The kind is judged by the lake as the whole bundle leaves it, so a
    /// value type declared beside the setting is the one it reads (063,
    /// extended).
    fn restamped(
        &self,
        (w, whole): (&Writer, &Ontology),
        s: &SettingDecl,
        now: Timestamp,
    ) -> Result<Vec<Draft>> {
        let ty = TypeName::from(SETTING_TYPE);
        let key = RecordKey::from(s.key.as_str());
        let view = self.lake.engine.view(None)?;
        let Some(held) = view.get(&ty, &key)?.filter(|r| !r.retired) else {
            return Ok(Vec::new());
        };
        let decl = w.ontology.ty(&ty)?;
        let mut patch = Patch::new();
        if let Some(v) = held.fields.get("value").filter(|v| !v.is_null()) {
            let kept = judged(whole, decl, s, v)?;
            if &kept != v {
                patch.insert("value".into(), kept);
            }
        }
        let group = s.group.clone().map_or(Value::Null, Value::String);
        if held.fields.get("group").unwrap_or(&Value::Null) != &group {
            patch.insert("group".into(), group);
        }
        if patch.is_empty() {
            return Ok(Vec::new());
        }
        let body = EventBody::RecordUpserted {
            ty,
            key,
            version: Version(held.version.0 + 1),
            patch,
            made: false,
        };
        Ok(vec![Draft::system(now, body)])
    }

    /// Each of `types` as `whole` computes its records again, walked once:
    /// a constraint that refuses is left held (lakelands#1119), and the
    /// change's plan is told the type's section, as its author is shown it
    /// (062): the records computed again, those that would be left naming
    /// no live record, and the constraints that admit that they would
    /// break. A type nothing is judged of is not counted, and is walked
    /// only where it may read a source's sealed value (058). A
    /// type its author is not shown, or knows of none of the fields it
    /// computes again (those `fields` names of it, and those derived or
    /// embedded from them), has no section, and one it is not told of a reference of has
    /// none of it; one judged of by nothing it is told counts nothing.
    /// Judged as a proposal's plan, nothing is refused and no held record
    /// is computed.
    fn walked_again(
        &self,
        (w, whole): (&Writer, &Ontology),
        (types, fields): (
            &BTreeSet<TypeName>,
            &BTreeMap<TypeName, BTreeSet<FieldName>>,
        ),
        (event, now, judging): (&Event, Timestamp, Judging),
    ) -> Result<Vec<MigrationReport>> {
        let view = self.lake.engine.view(None)?;
        let computing = whole.computing(&types.iter().cloned().collect::<Vec<_>>());
        let (plans, sight) = (record::Plans::default(), Sight::held(self.who, &w.ontology));
        let mut out = Vec::new();
        let v = view.as_ref();
        for ty in types {
            // What it may move: the fields the type computes.
            let decl = whole.ty(ty)?;
            let computed = (decl.fields.iter())
                .filter(|f| f.derived.is_some() || f.function.is_some() || f.embed.is_some());
            let moving: BTreeSet<FieldName> = computed.map(|f| f.name.clone()).collect();
            let mut walked = Walked::of(whole, decl, &moving, (&sight, judging))?;
            // Its author is told of the fields it knows of alone, of those
            // the bundle computes again and what is derived from them: a
            // type none of whose such fields it knows of has no section,
            // and what is judged of one hidden from it counts nothing it is
            // told.
            // What is derived from a field computed again is too, and an
            // embedding made from one.
            let mut again = fields.get(ty).cloned().unwrap_or_default();
            while let Some(f) = (decl.fields.iter()).find(|f| {
                let from = f
                    .embed
                    .as_ref()
                    .map(|e| e.from.as_slice())
                    .unwrap_or_default();
                !again.contains(&f.name)
                    && (f.depends_on.iter().chain(from)).any(|d| again.contains(d))
            }) {
                again.insert(f.name.clone());
            }
            let known: BTreeSet<FieldName> = (decl.fields.iter())
                .filter(|f| moving.contains(&f.name) && again.contains(&f.name))
                .filter(|f| sight.field_access(decl, f) != Access::None)
                .map(|f| f.name.clone())
                .collect();
            let mut unlived = Unlived::of(decl, &known, &sight);
            // Walked for what its author is told. A constraint that refuses
            // is told wherever the act judges it, as `Walked::of` grants it
            // only to one who reads it; `judges(false)` guards that.
            let tells = walked.tells() || unlived.watches();
            let judges = walked.judges(false) || tells;
            let shown = |r: &StoredRecord| {
                Ok(!sight.withheld(r) && crate::reads::answered(v, &sight, ty, &r.key)?)
            };
            let with = (&computing, v, &plans, &*self.lake.keys);
            // Unjudged, its outputs are planned again all the same where one
            // may read a source's sealed value: under a key the key store
            // lost, the fold would refuse it past the append, so it is
            // refused here, appending nothing, as a write is (058).
            let sealed = decl.fields.iter().any(|f| f.erasable)
                && crate::backed::computed_outside(whole, decl);
            // Nothing judged: every record is computed again, uncounted. A
            // held record is counted only for one who holds `restricted`.
            let recomputed = match judges {
                false if !judging.tells() && sealed => {
                    live(v, ty, |r| {
                        computed_again(with, r, event, &w.settings).map(drop)
                    })?;
                    None
                }
                false => None,
                true => Some({
                    let mut n = 0;
                    live(v, ty, |r| {
                        // A plan that only tells computes no held record: it
                        // counts none, and its inputs are not its author's.
                        if judging.tells() && sight.withheld(r) {
                            return Ok(());
                        }
                        n += u64::from(!sight.withheld(r));
                        let is = computed_again(with, r, event, &w.settings)?;
                        unlived.note((r, &is), &sight);
                        walked.judge((r, &is), (w, now), &shown)
                    })?;
                    n
                }),
            };
            let (breaches, lossless) = walked.kept(judging)?;
            // A reference's count to one who reads it and is shown its target.
            let target = |f: &FieldName| (decl.field(f))?.ty.target.clone();
            let shows = |f: &FieldName| target(f).is_some_and(|t| sight.ty(&t).is_ok());
            let reads = |f: &FieldName| sight.reads_from(ty, f);
            let mut unlive = unlived.counted(v, &sight, ty)?;
            unlive.retain(|u| reads(&u.field) && shows(&u.field));
            if sight.ty(ty).is_err() || known.is_empty() {
                continue;
            }
            let hash = shape::schema_hash(decl).map_err(Error::invalid)?;
            let mut report = MigrationReport {
                ty: ty.clone(),
                from_schema_hash: hash.clone(),
                to_schema_hash: hash,
                records_scanned: recomputed.unwrap_or(0),
                recomputed,
                unlive,
                breaches,
                lossless,
                ..MigrationReport::default()
            };
            let same = Migration::to(decl.clone());
            sight.shown_hashes(decl, &same, &mut report, &|n| whole.ty(n).ok());
            report.more = (report.unlive.iter()).any(|u| u.records > u.sample_keys.len() as u64)
                || (report.breaches.iter()).any(|b| b.records > b.sample_keys.len() as u64);
            out.push(report);
        }
        Ok(out)
    }

    /// The changes to the sets of `types`, as `whole` declares them, whose
    /// filter reads a field they compute or that `whole` declares other
    /// than the lake (a set naming one reads its filter too), but those
    /// `decided`; each record as `whole` computes it again, a field
    /// computed outside the lake kept as it was. Past `max_set_changes`,
    /// refused `too_large` naming the set.
    fn recomputed_sets(
        &self,
        (w, whole, decided): (&Writer, &Ontology, &[String]),
        types: &BTreeSet<TypeName>,
        event: &Event,
        now: Timestamp,
    ) -> Result<Vec<Draft>> {
        let (view, max) = (
            self.lake.engine.view(None)?,
            self.lake.limits.max_set_changes,
        );
        let computing = whole.computing(&types.iter().cloned().collect::<Vec<_>>());
        let (plans, mut out) = (record::Plans::default(), Vec::new());
        // A record of one of them as the fold computes it again.
        let again = |r: &StoredRecord| match types.contains(&r.ty) {
            true => computed_again(
                (&computing, view.as_ref(), &plans, &self.lake.keys),
                r,
                event,
                &w.settings,
            ),
            false => Ok(r.clone()),
        };
        // And the sets of other types that read into them across an edge.
        let sets = whole.object_sets.values().map(|e| &e.decl);
        let into = sets.filter(|s| across::reaches_into(whole, s, types));
        let referring: BTreeSet<&TypeName> = into.map(|s| &s.of).collect();
        for ty in types
            .iter()
            .chain(referring.into_iter().filter(|t| !types.contains(*t)))
        {
            let computed: BTreeSet<&FieldName> = (whole.ty(ty)?.fields.iter())
                .filter(|f| f.derived.is_some() || f.function.is_some() || f.embed.is_some())
                .filter(|_| types.contains(ty))
                .map(|f| &f.name)
                .collect();
            let reads = |s: &ObjectSetDecl| {
                let at = [Place::ObjectSet {
                    name: s.name.clone(),
                }];
                let filter = declare::set_filter(&|n| whole.set(n), s, &at);
                across::reaches_into(whole, s, types)
                    || filter.is_ok_and(|e| {
                        lakelands_core::derive::reads(&e)
                            .iter()
                            .any(|f| computed.contains(f))
                    })
            };
            let moved = |s: &ObjectSetDecl| w.ontology.set(&s.name) != Some(s);
            let mut sets = Vec::new();
            for s in (whole.object_sets.values().map(|e| &e.decl)).filter(|s| &s.of == ty) {
                let named = |m: &ObjectSetDecl| moved(m) && declare::names_set(whole, s, &m.name);
                let bundled = whole.object_sets.values().any(|m| named(&m.decl));
                if !decided.contains(&s.name) && (reads(s) || moved(s) || bundled) {
                    let held = match w.ontology.set(&s.name) {
                        Some(_) => view.members(&s.name, ty)?,
                        None => BTreeSet::new(),
                    };
                    sets.push((s, held));
                }
            }
            if sets.is_empty() {
                continue;
            }
            let far = |t: &TypeName, k: &RecordKey| view.get(t, k)?.map(|r| again(&r)).transpose();
            let link =
                |t: &TypeName, r: &RelationName, k: &RecordKey| link_from(view.as_ref(), t, r, k);
            let max_depth = self.lake.limits.max_depth;
            let heads = Heads {
                far: &far,
                link: &link,
                max: max_depth,
            };
            live(view.as_ref(), ty, |r| {
                let is = &again(r)?;
                for (s, held) in &sets {
                    let member = member_of((s, whole), is, &w.settings, now, heads)?;
                    if held.contains(&r.key) == member {
                        continue;
                    }
                    if out.len() as u64 >= u64::from(max) {
                        let (field, reason) =
                            (Some(s.name.as_str().into()), Refusal::TooLarge { max });
                        return Err(Error::refused(field, reason));
                    }
                    let change = (s.name.clone(), ty.clone(), r.key.clone(), member);
                    out.push(self.changed(change, event.position, now));
                }
                Ok(())
            })?;
        }
        Ok(out)
    }

    /// The membership a set's declaration decides for the records already
    /// there: an `object_set_changed` for each record of its type whose
    /// membership differs from the index, read a page at a time, each
    /// caused by the `declared` at `declared` (004, extended). The sets in
    /// force that name a set are decided with it, over `after`, the
    /// ontology as the append leaves it: a set is its filter, so a change
    /// reaches every set built on it, in the same append, and each of them
    /// once, whatever number of the sets it names the append declares
    /// (`decided`). A set declared again unchanged decides nothing; one
    /// declared again as decided at the read leaves its members. Past the lake's
    /// `max_set_changes`, counted over all of them, the declaration is
    /// refused `too_large`, naming the set whose change passed it, before
    /// anything is appended; the walk stops there.
    fn set_drafts(
        &self,
        (w, after, decided): (&Writer, &Ontology, &mut Vec<String>),
        set: &ObjectSetDecl,
        declared: Position,
        now: Timestamp,
    ) -> Result<Vec<Draft>> {
        let old = w.ontology.object_sets.get(&set.name).map(|e| &e.decl);
        if old.is_some_and(|o| o.filter == set.filter && o.decided_at == set.decided_at) {
            return Ok(Vec::new());
        }
        let (view, ty) = (self.lake.engine.view(None)?, &set.of);
        // The sets in force that name this one and stand as they are, each
        // decided once in an append, with its members so far, read once.
        let mut sets = sets_of(w, view.as_ref(), ty, |s| {
            let stands = after.object_sets.get(&s.name).is_some_and(|e| &e.decl == s);
            stands && !decided.contains(&s.name) && declare::names_set(after, s, &set.name)
        })?;
        decided.extend(sets.iter().map(|(s, _)| s.name.clone()));
        // A new set has no member so far.
        let held = match old {
            Some(_) => view.members(&set.name, ty)?,
            None => BTreeSet::new(),
        };
        sets.insert(0, (set, held));
        let max = self.lake.limits.max_set_changes;
        let mut out = Vec::new();
        at_head(view.as_ref(), self.lake.limits.max_depth, |heads| {
            live(view.as_ref(), ty, |r| {
                for (s, held) in &sets {
                    let is = member_of((s, after), r, &w.settings, now, heads)?;
                    if held.contains(&r.key) != is {
                        if out.len() as u64 >= u64::from(max) {
                            return Err(Error::refused(
                                Some(s.name.as_str().into()),
                                Refusal::TooLarge { max },
                            ));
                        }
                        let change = (s.name.clone(), ty.clone(), r.key.clone(), is);
                        out.push(self.changed(change, declared, now));
                    }
                }
                Ok(())
            })
        })?;
        Ok(out)
    }

    /// `add_field`.
    pub fn add_field(&self, ty: &TypeName, field: FieldDecl) -> Result<Outcome> {
        self.enacting()?;
        let (ty, field) = (ty.clone(), Box::new(field));
        self.make(
            lakelands_api::write::Change::AddField { ty, field },
            Against::default(),
        )
    }

    /// The preflight of a migration over every stored record of the type,
    /// retired and restricted ones included: they are history, the fold
    /// converts them with the rest, and a migration that cannot convert
    /// one is not lossless, so it is refused before anything is appended.
    ///
    /// A migration that makes a record's key differently (other key
    /// fields, or a key field's value converted) re-keys it: the preflight
    /// counts the moves, with the closure and the references they take
    /// along (`crate::rekey`), and a collision or a missing key value makes
    /// it not lossless. One that takes every key field away is refused.
    pub fn preflight(
        &self,
        w: &Writer,
        view: &dyn ProjectionView,
        m: &Migration,
    ) -> Result<MigrationReport> {
        granted(Sight::held(self.who, &w.ontology).authors())?;
        let mut m = m.clone();
        let author = self.authored(&w.ontology, &mut m)?;
        let judged = (&m, author.as_ref(), None);
        let report =
            (self.preflight_plan(w, view, judged, (None, self.judging(When::Plan))))?.report;
        self.shown(w, &m, report)
    }

    /// A report as the caller may know it (013, 024 and 027, extended):
    /// wherever one is answered or refuses a write, its reader is a reader
    /// like any other, the author of its own preview too. Its id is made
    /// over what it is shown, and an apply compares that.
    pub(super) fn shown(
        &self,
        w: &Writer,
        m: &Migration,
        mut report: MigrationReport,
    ) -> Result<MigrationReport> {
        let view = self.lake.engine.view(None)?;
        let sight = Sight::held(self.who, &w.ontology);
        let (was, mut m) = (w.ontology.ty(&m.to.name)?, m.clone());
        w.ontology.complete(&mut m.to);
        let described = || sight.describe(&sight.who().lake);
        sight.shown_report(was, &m, &mut report, described, |n| sight.ty(n).ok());
        sight.shown_hashes(was, &m, &mut report, &|n| w.ontology.ty(n).ok());
        report.report_id = report_id(&report, self.lake.report_head(view.position())?);
        Ok(report)
    }

    /// The refusal of a migration that is not lossless, with its report as
    /// the caller may know it.
    pub(super) fn lossy(&self, w: &Writer, m: &Migration, report: MigrationReport) -> Error {
        match self.shown(w, m, report) {
            Ok(report) => Error::refused(
                None,
                Refusal::Migration {
                    report: Box::new(report),
                },
            ),
            Err(e) => e,
        }
    }
}

/// Each `initial` of `t` in the one form a write of its field stores, so
/// that two spellings of one value are one declaration (050). A value the
/// check refuses is left as sent, for the check to refuse.
fn initials(o: &Ontology, t: &mut TypeDecl) {
    for f in &mut t.fields {
        let vt =
            f.ty.value_type
                .as_ref()
                .and_then(|n| o.value_types.get(n.as_str()));
        if let Some(v) = &mut f.initial
            && let Ok(one) = value::check(v, &f.ty, vt.map(|e| &e.decl))
        {
            *v = one;
        }
    }
}

/// Reads `to`, a migration's or a redeclaration's shape of the type
/// declared as `before`, as its author's view of the type (027 and 006,
/// extended): a field `who` may not know of, or a relation it is not
/// shown, was not its to leave out, so it is put back as declared, at its
/// declared place, and so are such link fields of each relation `to`
/// keeps or renames that it is shown. No refusal names one: a rename from
/// it is judged against its author's view, where it is no field or
/// relation ([`Call::preflight_plan`]), and a field it reads is refused
/// on that field. `sees` is its author's sight.
fn as_author(
    sees: &Sight<'_>,
    before: &TypeDecl,
    to: &mut TypeDecl,
    relations: &[(RelationName, RelationName)],
) -> Result<()> {
    fn put_back<T: Clone>(declared: &[T], hidden: impl Fn(&T) -> bool, into: &mut Vec<T>) {
        for (i, x) in declared.iter().enumerate().filter(|(_, x)| hidden(x)) {
            into.insert(i.min(into.len()), x.clone());
        }
    }
    let hidden = |t: &TypeDecl, f: &FieldDecl| sees.field_access(t, f) == Access::None;
    let unshown = |r: &RelationDecl| !sees.relation(before, r);
    let unknown: Vec<&FieldDecl> = (before.fields.iter().filter(|f| hidden(before, f))).collect();
    put_back(&before.fields, |f| hidden(before, f), &mut to.fields);
    // A constraint over a field it does not read is not told to it, as
    // `describe_type` tells none, so left out it stands as declared
    // (lakelands#1119).
    let read = |c: &ConstraintDecl| -> Vec<FieldName> {
        let e = c.holds.typed().ok();
        e.map_or_else(Vec::new, |e| e.fields_read().into_iter().cloned().collect())
    };
    let untold = |c: &ConstraintDecl| !read(c).iter().all(|n| sees.reads_from(&before.name, n));
    put_back(&before.constraints, untold, &mut to.constraints);
    // A valid time over a field it may not know of is not named to it, so
    // left out it stands as declared (024 and 027, extended).
    let unseen = |v: &Option<FieldName>| {
        v.as_ref()
            .is_some_and(|v| unknown.iter().any(|f| &f.name == v))
    };
    // An end sent with no start is judged on the type as it is shown:
    // one alone there keeps one; else it is refused as on a type with no
    // valid time, before a hidden start is put back, which would tell
    // that one stands (028, lakelands#909).
    let alone = (before.valid_from.is_none() || unseen(&before.valid_from))
        && before.valid_to.is_some()
        && !unseen(&before.valid_to);
    if let (None, Some(field), false) = (&to.valid_from, &to.valid_to, alone) {
        let (name, field) = (to.name.clone(), field.clone());
        let said = Said::from(Case::ValidToAlone {
            field: field.clone(),
        });
        return Err(Error::invalid_at(&field, said.within(Place::Type { name })));
    }
    for (axis, sent, held) in [
        ("valid_from", &mut to.valid_from, &before.valid_from),
        ("valid_to", &mut to.valid_to, &before.valid_to),
    ] {
        if sent.is_none() && unseen(held) {
            sent.clone_from(held);
        } else if *sent != *held && unseen(held) && !unseen(sent) {
            // A field its author may name, sent where a bound over a field
            // it may not know of stands: the bound is taken, as a name a
            // hidden field holds is (027), and stands (lakelands#692). A
            // name it may not give was refused before, as on any type.
            // A hidden field's name sent there is the author's own field
            // named like it, which the declaration refuses as declared
            // twice, as on any type.
            let (name, axis) = (to.name.clone(), axis.to_owned());
            let said = Said::from(Case::ValidTimeStands { axis });
            return Err(Error::invalid(said.within(Place::Type { name })));
        }
    }
    for r in &mut to.relations {
        // A renamed relation is the one it was, under its old name.
        let was = relations.iter().find(|(_, new)| new == &r.name);
        let name = was.map_or(&r.name, |(old, _)| old);
        // To its author a relation it is not shown is none to rename.
        let shown = |o: &&RelationDecl| &o.name == name && !unshown(o);
        if let Some(old) = before.relations.iter().find(shown) {
            let link = links::link_type(&before.name, old);
            // A link type its author may not know of is a type it is not
            // shown: to give it a field is not granted, naming nothing, as
            // to declare it is (lakelands#965).
            granted(r.fields.is_empty() || sees.ty(&link.name).is_ok())?;
            put_back(&old.fields, |f| hidden(&link, f), &mut r.fields);
        }
    }
    put_back(&before.relations, unshown, &mut to.relations);
    Ok(())
}

/// A field a part its author may not know of reads stands, to that
/// author, as it is, judged on what it sent before anything else is: the
/// parts are an embedding, a derived or computed field, a relation it is
/// not shown, a constraint it is not told and a link field, and each
/// field of theirs it may know of (lakelands#1124).
fn reads_stand(
    sees: &Sight<'_>,
    before: &TypeDecl,
    to: &TypeDecl,
    m: Option<&Migration>,
) -> Result<()> {
    let (renames, relations, fill) = m.map_or((&[][..], &[][..], &[][..]), |m| {
        (&m.renames[..], &m.relation_renames[..], &m.fill[..])
    });
    let hidden = |t: &TypeDecl, f: &FieldDecl| sees.field_access(t, f) == Access::None;
    let known = |t: &TypeDecl, fields: &[FieldDecl], n: &FieldName| {
        fields.iter().any(|f| &f.name == n && !hidden(t, f))
    };
    let unshown = |r: &RelationDecl| !sees.relation(before, r);
    let read = |c: &ConstraintDecl| -> Vec<FieldName> {
        let e = c.holds.typed().ok();
        e.map_or_else(Vec::new, |e| e.fields_read().into_iter().cloned().collect())
    };
    let untold = |c: &ConstraintDecl| !read(c).iter().all(|n| sees.reads_from(&before.name, n));
    let constrained: Vec<FieldName> = (before.constraints.iter().filter(|c| untold(c)))
        .flat_map(read)
        .collect();
    // What a relation it is not shown reads: its `via`, its match's own side.
    let hid = before.relations.iter().filter(|r| unshown(r));
    let owner_reads: Vec<FieldName> = hid
        .clone()
        .flat_map(|r| r.derived.iter().flat_map(|d| d.reads()[0].clone()))
        .collect();
    let reads = (before.fields.iter().filter(|f| hidden(before, f)))
        .flat_map(inputs)
        .chain(hid.flat_map(|r| r.via.iter()))
        .chain(&owner_reads)
        .chain(&constrained)
        .filter(|n| known(before, &before.fields, n));
    stands(&to.name, reads, &before.fields, &to.fields, renames, fill)?;
    for r in &to.relations {
        // A renamed relation is the one it was, under its old name.
        let was = relations.iter().find(|(_, new)| new == &r.name);
        let name = was.map_or(&r.name, |(old, _)| old);
        let shown = |o: &&RelationDecl| &o.name == name && !unshown(o);
        let Some(old) = before.relations.iter().find(shown) else {
            continue;
        };
        let link = links::link_type(&before.name, old);
        let reads = (old.fields.iter().filter(|f| hidden(&link, f)))
            .flat_map(inputs)
            .filter(|n| known(&link, &old.fields, n));
        stands(&link.name, reads, &old.fields, &r.fields, &[], &[])?;
    }
    Ok(())
}

/// The fields a computed field reads: its `depends_on`, and what an
/// embedding is made from.
fn inputs(f: &FieldDecl) -> impl Iterator<Item = &FieldName> {
    (f.depends_on.iter()).chain(f.embed.iter().flat_map(|e| &e.from))
}

/// A field a part its author may not know of reads stands, to that
/// author, as it is: removed, renamed (its name freed, taken by another
/// or swapped), retyped, made sensitive, made computed or computed
/// otherwise, or filled, it is refused on that field
/// before anything judges the hidden part, which would name it
/// (lakelands#1124). The owner, or one shown the part, may.
fn stands<'a>(
    ty: &TypeName,
    reads: impl IntoIterator<Item = &'a FieldName>,
    before: &[FieldDecl],
    to: &[FieldDecl],
    renames: &[(FieldName, FieldName)],
    fill: &[FieldName],
) -> Result<()> {
    let moved = |n: &FieldName| renames.iter().any(|(old, new)| old == n || new == n);
    let computing = |f: &FieldDecl| (f.derived.clone(), f.function.clone(), f.embed.clone());
    let changed = |n: &FieldName| {
        let (was, is) = (
            before.iter().find(|f| &f.name == n),
            to.iter().find(|f| &f.name == n),
        );
        match (was, is) {
            // Made sensitive, or computed otherwise; taking `sensitive`
            // away is a change the report lists (027), and no part's check
            // minds it.
            (Some(w), Some(i)) => {
                let computed = computing(i) != (None, None, None);
                w.ty != i.ty
                    || (!w.sensitive && i.sensitive)
                    || (computed && (computing(w), &w.depends_on) != (computing(i), &i.depends_on))
            }
            (_, None) => true,
            (None, Some(_)) => false,
        }
    };
    match reads
        .into_iter()
        .find(|n| moved(n) || fill.contains(n) || changed(n))
    {
        Some(n) => {
            let (field, ty) = (n.clone(), ty.clone());
            Err(Error::invalid_at(n, Case::FieldReadElsewhere { field, ty }))
        }
        None => Ok(()),
    }
}

/// A migration as its author sent it, and the type as its author sees it
/// (`Sight::known_fields`): the fields it may know of and the relations
/// it is shown, with their link fields it may know of.
pub(crate) struct Authored {
    pub(super) sent: Migration,
    pub(super) known: TypeDecl,
}

pub(crate) fn risk_word(r: Risk) -> &'static str {
    match r {
        Risk::Low => "low",
        Risk::Medium => "medium",
        Risk::High => "high",
    }
}

/// The system declarations of the link types `t`'s relations call for
/// that differ from those in force: a new relation's, or one whose fields
/// changed. Appended with `t`'s declaration, in the same append
/// (`docs/log-format.md` §2.3).
pub(super) fn link_type_drafts(o: &Ontology, t: &TypeDecl, now: Timestamp) -> Result<Vec<Draft>> {
    let mut out = Vec::new();
    for lt in links::link_types_of(t) {
        if o.types.get(&lt.name).map(|e| &e.decl) == Some(&lt) {
            continue;
        }
        out.push(system_declared(o, Declaration::Type(lt), now)?);
    }
    Ok(out)
}

/// Refuse `to`, a type declared again or migrated from `was`, where a verb
/// on it or on a link type its relations carry, before or after, or a
/// verb, a set or a trigger reading across into one of them, could not be
/// declared over `after` as `to` leaves it, its link types with it: a
/// path read across, a relation stepped through, a kind its condition
/// computes with (028, 055). A declaration `shows` names is named;
/// another is told one could not be declared. `o` is the lake before,
/// where they read.
pub(super) fn verbs_judged_again(
    (o, after): (&Ontology, &Ontology),
    was: &TypeDecl,
    to: &TypeDecl,
    shows: &dyn Fn(&Declaration) -> bool,
) -> std::result::Result<(), Said> {
    let judges = judged_by(o, was, to);
    let judged = (after.verbs.values()).filter(|e| judges(&e.decl));
    let mut judged: Vec<_> = judged.map(|v| Declaration::Verb(v.decl.clone())).collect();
    judged.extend(across_judged(o, was, to));
    if judged.is_empty() {
        return Ok(());
    }
    let mut linked = after.clone();
    put_as_appended(&mut linked, &Declaration::Type(to.clone()), Some);
    for verb in judged {
        if let Err(e) = declare::reads_its_type(&linked, &verb) {
            let said = e.case().filter(|_| shows(&verb)).cloned();
            let ty = to.name.clone();
            return Err(said.unwrap_or(Case::NoLongerDeclarable { ty }.into()));
        }
    }
    Ok(())
}

/// The sets and triggers in `o` whose condition reads across an edge into
/// a type `was` becomes `to`, or into a link type its relations carry,
/// before or after: [`verbs_judged_again`] judges them as it judges a verb
/// that reads across into it (055, extended).
fn across_judged(o: &Ontology, was: &TypeDecl, to: &TypeDecl) -> Vec<Declaration> {
    let mut types = BTreeSet::from([to.name.clone()]);
    for t in [was, to] {
        types.extend(links::link_types_of(t).into_iter().map(|l| l.name));
    }
    let sets = o.object_sets.values().map(|e| &e.decl);
    let sets = sets.filter(|s| across::reaches_into(o, s, &types));
    let triggers = o.triggers.values().map(|e| &e.decl);
    let triggers = triggers.filter(|t| across::when_reaches_into(o, t, &types));
    (sets.map(|s| Declaration::ObjectSet(s.clone())))
        .chain(triggers.map(|t| Declaration::Trigger(t.clone())))
        .collect()
}

/// Whether [`verbs_judged_again`] judges a verb where `was` becomes `to`:
/// one on it or on a link type its relations carry, before or after, or
/// one reading across into one of them, as `o`, the lake before, reads it.
fn judged_by<'o>(
    o: &'o Ontology,
    was: &TypeDecl,
    to: &TypeDecl,
) -> impl Fn(&VerbDecl) -> bool + 'o {
    let mut moved = vec![to.name.clone()];
    for t in [was, to] {
        moved.extend(links::link_types_of(t).into_iter().map(|l| l.name));
    }
    move |v| {
        let far = o.ty(&v.on).ok();
        let far = far.and_then(|on| v.reads_across(on, &|n| o.ty(n).ok()));
        let into = far.is_some_and(|far| far.iter().any(|(t, _)| moved.contains(&t.name)));
        moved.contains(&v.on) || into
    }
}

/// Puts `d` into `o` as an append leaves it: a type with the link types
/// its relations call for, each as `brings` admits it (035), and a type
/// withdrawn without the link types in force it carries (027).
pub(crate) fn put_as_appended(
    o: &mut Ontology,
    d: &Declaration,
    brings: impl Fn(TypeDecl) -> Option<TypeDecl>,
) {
    put_leaving(o, d, brings, &BTreeSet::new());
}

/// [`put_as_appended`], leaving a link type in `own`, one the set
/// withdraws itself, to its own withdrawal.
fn put_leaving(
    o: &mut Ontology,
    d: &Declaration,
    brings: impl Fn(TypeDecl) -> Option<TypeDecl>,
    own: &BTreeSet<String>,
) {
    let (with, gone) = match d {
        Declaration::Type(t) => (links::link_types_of(t), Vec::new()),
        Declaration::Withdraw(x) if x.kind == DeclarationKind::Type => {
            let ty = o.types.get(&TypeName::from(x.name.as_str()));
            let links = ty
                .map(|e| links::link_types_of(&e.decl))
                .unwrap_or_default();
            let links = (links.into_iter())
                .filter(|l| o.types.contains_key(&l.name) && !own.contains(l.name.as_str()));
            let gone = links.map(|l| {
                let name = l.name.as_str().to_owned();
                Declaration::Withdraw(Withdrawal { kind: x.kind, name })
            });
            (Vec::new(), gone.collect())
        }
        _ => (Vec::new(), Vec::new()),
    };
    let with = with.into_iter().filter_map(brings).map(Declaration::Type);
    (with.chain([d.clone()]).chain(gone)).for_each(|d| o.put(d, 0, None));
}

/// The names of the types `declarations` withdraws.
fn types_withdrawn(declarations: &[Declaration]) -> BTreeSet<String> {
    let names = declarations.iter().filter_map(|d| match d {
        Declaration::Withdraw(x) if x.kind == DeclarationKind::Type => Some(x.name.clone()),
        _ => None,
    });
    names.collect()
}

/// The system's `declared` of a declaration, at its next version.
pub(super) fn system_declared(
    o: &Ontology,
    declaration: Declaration,
    now: Timestamp,
) -> Result<Draft> {
    let schema_hash = match &declaration {
        Declaration::Type(t) => Some(shape::schema_hash(t).map_err(Error::invalid)?),
        _ => None,
    };
    let version = o.next_version(&declaration);
    let body = EventBody::Declared {
        declaration,
        schema_hash,
        version,
        with_the_set: false,
    };
    Ok(Draft::system(now, body))
}

/// The link types in force whose derivation from the migration's `to`
/// differs: each with the declaration in force and the one `to` calls for.
/// One a renamed relation moves its links to is among them only where
/// `standing` asks: it stands from an earlier relation of that name and
/// holds no record, so it is declared again in the renamed relation's
/// shape, never migrated from its own ([`arriving`]; 035, extended).
pub(super) fn changed_link_types(
    o: &Ontology,
    m: &Migration,
    standing: bool,
) -> Vec<(TypeDecl, TypeDecl)> {
    let arriving = arriving(m);
    links::link_types_of(&m.to)
        .into_iter()
        .filter_map(|new| {
            let old = &o.types.get(&new.name)?.decl;
            let asked = standing || !arriving.contains(&new.name);
            (old != &new && asked).then(|| (old.clone(), new))
        })
        .collect()
}

/// The link types a migration's renamed relations move their links to.
pub(super) fn arriving(m: &Migration) -> BTreeSet<TypeName> {
    links::moved(m).into_values().collect()
}

/// Refuse a link type's records at a door other than the link doors.
pub(super) fn ordinary_door(ty: &TypeName) -> Result<()> {
    if links::is_link_type(ty) {
        return Err(Error::refused(None, Refusal::WrongDoor));
    }
    Ok(())
}

/// The answer for a relation `ty` does not carry by link records, which is
/// also the answer for one the caller is not shown.
pub(super) fn no_link_relation(ty: &TypeName, relation: &RelationName) -> Error {
    let (ty, relation) = (ty.clone(), relation.clone());
    Error::invalid(Case::NoLinkRelation { ty, relation })
}
