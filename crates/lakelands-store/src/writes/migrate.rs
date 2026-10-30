//! The migration doors: a type's migration and a value type's, each
//! planned over the stored records, then appended
//! (`docs/decisions/062-a-change-plans-before-it-acts.md`).

use super::*;
use crate::record;

/// The preflight of `m` over the stored records of `decl`'s type, retired
/// and restricted ones included, read a page at a time. `each` sees every
/// record before the report counts it. An error of a page read or of
/// `each` ends the walk and is answered before the report is. A sample
/// key is one of a record the principal of `sees` is answered; a
/// retargeted reference converts onto a record `onto`'s principal is.
fn shaped(
    view: &dyn ProjectionView,
    (sees, onto): (&Sight, &Sight),
    decl: &TypeDecl,
    m: &Migration,
    mut each: impl FnMut(&StoredRecord) -> Result<()>,
) -> Result<MigrationReport> {
    let mut walk = crate::lake::Walk::new(view, &decl.name, true);
    let mut seen = Ok(());
    let held = std::cell::Cell::new(false);
    let rows = walk.by_ref().map_while(|r| {
        seen = each(&r);
        held.set(sees.withheld(&r));
        seen.is_ok().then_some((r.key, r.fields))
    });
    // The record in hand is asked no read where it is held as it stands. A
    // read the engine fails is kept, and answered after (lakelands#927).
    let failed = std::cell::RefCell::new(None);
    let answered = |key: &RecordKey| {
        let read = (!held.get()).then(|| crate::reads::answered(view, sees, &decl.name, key));
        match read.transpose() {
            Ok(shown) => shown.unwrap_or(false),
            Err(e) => {
                failed.borrow_mut().get_or_insert(e);
                false
            }
        }
    };
    // A reference given another target converts only where it names a
    // live record of that target (032, lakelands#1071).
    let gone = retargeted(view, onto, decl, m)?;
    let dangling = |f: &FieldName, v: &Value| {
        let key = v.as_str();
        gone.get(f)
            .is_some_and(|g| key.is_some_and(|k| g.contains(k)))
    };
    let report = shape::referring(decl, m, rows, &answered, &dangling);
    walk.failed()?;
    seen?;
    if let Some(e) = failed.into_inner() {
        return Err(e);
    }
    report.map_err(Error::invalid)
}

/// Of each field of `m.to` whose reference target is not the one `decl`
/// gave it, the values `decl`'s records hold there that name no record of
/// the new target live to `sees`'s principal, as a write of the reference
/// finds one ([`live_for`]): one read of the target per field, of the
/// distinct values ([`retarget_values`]), and of a link's ends.
fn retargeted(
    view: &dyn ProjectionView,
    sees: &Sight,
    decl: &TypeDecl,
    m: &Migration,
) -> Result<BTreeMap<FieldName, BTreeSet<String>>> {
    let mut gone = BTreeMap::new();
    for (field, target, keys) in retarget_values(view, decl, m)? {
        let asked: Vec<RecordKey> = keys.iter().map(|k| k.as_str().into()).collect();
        let found = view.get_many(&target, &asked)?;
        let rows: Vec<_> = found.iter().map(|r| &r.fields).collect();
        let ends = match sees.ty(&target) {
            Ok(d) => sees.ends(d, &rows, &mut |t, k| view.get_many(t, k))?,
            Err(_) => vec![false; rows.len()],
        };
        let live: BTreeSet<String> = (found.iter().zip(ends))
            .filter(|(r, ends)| *ends && !r.retired && sees.answers(&target, r))
            .map(|(r, _)| r.key.to_string())
            .collect();
        gone.insert(
            field,
            keys.into_iter().filter(|k| !live.contains(k)).collect(),
        );
    }
    Ok(gone)
}

/// Of each field of `m.to` whose reference target is not the one `decl`
/// gave it, the new target and the distinct values `decl`'s records hold
/// there: one walk of the type.
pub(super) fn retarget_values(
    view: &dyn ProjectionView,
    decl: &TypeDecl,
    m: &Migration,
) -> Result<Vec<(FieldName, TypeName, BTreeSet<String>)>> {
    let moved = retargeted_fields(decl, m);
    let mut named = vec![BTreeSet::new(); moved.len()];
    if !moved.is_empty() {
        crate::lake::walk(view, &decl.name, true, |r| {
            for ((old, _, _), keys) in moved.iter().zip(named.iter_mut()) {
                keys.extend(r.fields.get(old).and_then(Value::as_str).map(str::to_owned));
            }
            Ok(())
        })?;
    }
    let values = moved.into_iter().zip(named);
    Ok(values
        .map(|((_, field, target), keys)| (field, target, keys))
        .collect())
}

/// Of each field of `m.to` whose reference target is not the one `decl`
/// gave it, renames followed: its name in `decl`, in `m.to`, and its new
/// target.
fn retargeted_fields(decl: &TypeDecl, m: &Migration) -> Vec<(FieldName, FieldName, TypeName)> {
    let was = |to: &FieldName| {
        let from = m
            .renames
            .iter()
            .find(|(_, t)| t == to)
            .map_or(to, |(f, _)| f);
        decl.field(from)
    };
    (m.to.fields.iter())
        .filter_map(|f| {
            let (target, old) = (f.ty.target.as_ref()?, was(&f.name)?);
            let other = old.ty.target.as_ref() != Some(target);
            other.then(|| (old.name.clone(), f.name.clone(), target.clone()))
        })
        .collect()
}

/// A migration's type and each link type it changes, as each is migrated.
pub(super) fn retargets(o: &Ontology, m: &Migration) -> Result<Vec<(TypeDecl, Migration)>> {
    let links = changed_link_types(o, m, false).into_iter();
    let links = links.map(|(old, new)| (old, Migration::to(new)));
    Ok(std::iter::once((o.ty(&m.to.name)?.clone(), m.clone()))
        .chain(links)
        .collect())
}

/// The types a proposed retarget reaches records of: each new target and,
/// of a link type, its ends' types; none of any other write.
pub(crate) fn retarget_types(o: &Ontology, write: &ProposedWrite) -> Result<BTreeSet<TypeName>> {
    let ProposedWrite::Migrate { migration: m, .. } = write else {
        return Ok(BTreeSet::new());
    };
    let mut out = BTreeSet::new();
    let mut next: Vec<TypeName> = Vec::new();
    for (decl, m) in retargets(o, m)? {
        next.extend(retargeted_fields(&decl, &m).into_iter().map(|(.., t)| t));
    }
    while let Some(t) = next.pop() {
        if let (true, Ok(link)) = (links::is_link_type(&t), o.ty(&t)) {
            let ends = link
                .fields
                .iter()
                .filter(|f| [links::FROM, links::TO].contains(&f.name.as_str()));
            next.extend(
                ends.filter_map(|f| f.ty.target.clone())
                    .filter(|e| !out.contains(e)),
            );
        }
        out.insert(t);
    }
    Ok(out)
}

/// The declarations that name a link type whose records a migration
/// moves, each as the lake declares it again under the new name: a trigger
/// or a verb on it, an object set of it, a view that reads it (035,
/// extended).
fn carried(o: &Ontology, m: &Migration) -> Vec<Declaration> {
    let moved = links::moved(m);
    let to = |t: &TypeName| moved.get(t).cloned();
    let mut out = Vec::new();
    for t in o.triggers.values().map(|e| &e.decl) {
        out.extend(to(&t.on).map(|on| Declaration::Trigger(TriggerDecl { on, ..t.clone() })));
    }
    for v in o.verbs.values().map(|e| &e.decl) {
        out.extend(to(&v.on).map(|on| Declaration::Verb(VerbDecl { on, ..v.clone() })));
    }
    for s in o.object_sets.values().map(|e| &e.decl) {
        out.extend(to(&s.of).map(|of| Declaration::ObjectSet(ObjectSetDecl { of, ..s.clone() })));
    }
    for t in o.types.values().map(|e| &e.decl) {
        let mut view = t.view.clone();
        for v in view.iter_mut().map(|v| &mut **v) {
            let sources = v.merge.iter_mut().map(|s| &mut s.source);
            for source in std::iter::once(&mut v.source).chain(sources) {
                *source = to(source).unwrap_or_else(|| source.clone());
            }
        }
        if view != t.view {
            out.push(Declaration::Type(TypeDecl { view, ..t.clone() }));
        }
    }
    out
}

/// How many records of `ty` the view holds, retired ones included, an
/// ended key not: read a page at a time.
pub(super) fn stored(view: &dyn ProjectionView, ty: &TypeName) -> Result<u64> {
    // A count answers none at once; past that the walk leaves out an ended
    // key by the store's own rule.
    if view.count(ty, &every(true))? == 0 {
        return Ok(0);
    }
    let mut n = 0;
    crate::lake::walk(view, ty, true, |_| {
        n += 1;
        Ok(())
    })?;
    Ok(n)
}

impl Call<'_> {
    /// The preflight, the record keys the migration moves and, given the
    /// time it is applied at, the set memberships it changes: each record
    /// as migrated is decided by the rule a write uses, in the walk the
    /// preflight makes anyway. Past the lake's `max_set_changes` it decides
    /// no further, and the plan holds one change too many: a stale report
    /// and a migration that is not lossless are refused before that is.
    /// Judged as a proposal's plan, a constraint left broken is lossy only
    /// by a record its reader is told of, and is not refused
    /// `constraint_left_broken` (062).
    pub(crate) fn preflight_plan(
        &self,
        w: &Writer,
        view: &dyn ProjectionView,
        (m, author, judged): (&Migration, Option<&Authored>, Option<&Principal>),
        (now, judging): (Option<Timestamp>, Judging),
    ) -> Result<MigrationPlan> {
        let before = w.ontology.ty(&m.to.name)?.clone();
        let sees = Sight::held(self.who, &w.ontology);
        // A migration reshapes stored records, and a view stores none. A
        // view changes, or becomes a stored type, by being declared again;
        // a stored type never becomes a view, since its records would
        // linger beside the computed ones: the view takes another name.
        if before.view.is_some() || m.to.view.is_some() {
            let ty = m.to.name.clone();
            return Err(Error::invalid(
                match (before.view.is_some(), m.to.view.is_some()) {
                    (true, true) => Case::MigrationOfView { ty },
                    (true, false) => Case::MigrationViewToStored { ty },
                    _ => Case::MigrationToView { ty },
                },
            ));
        }
        if !before.key.is_empty() && m.to.key.is_empty() {
            let ty = before.name.clone();
            return Err(Error::invalid(Case::MigrationTakesKey { ty }));
        }
        // The fields whose values it moves: those of its type it takes,
        // renames, declares otherwise or fills; where it re-keys, the
        // references to it and its links' ends; and every field of a link
        // type a relation's change moves records out of.
        let mut stepped: viewed::Stepped = BTreeMap::new();
        let kept = |f: &FieldDecl| m.to.fields.iter().any(|t| t == f);
        let own = before
            .fields
            .iter()
            .filter(|f| !kept(f))
            .map(|f| f.name.clone());
        let own = own.chain(m.fill.iter().cloned()).collect();
        stepped.insert(m.to.name.clone(), Some(own));
        let rekeyed = m.to.key != before.key;
        let to_links = links::link_types_of(&m.to);
        for l in links::link_types_of(&before) {
            let moved = !to_links.iter().any(|t| t == &l);
            let ends = [links::FROM, links::TO].map(FieldName::from);
            let ends = ends.into_iter().filter(|_| rekeyed).collect();
            stepped.insert(l.name, (!moved).then_some(ends));
        }
        for t in w
            .ontology
            .types
            .values()
            .map(|e| &e.decl)
            .filter(|_| rekeyed)
        {
            let refers = t
                .fields
                .iter()
                .filter(|f| f.ty.target.as_ref() == Some(&m.to.name));
            let refers: BTreeSet<FieldName> = refers.map(|f| f.name.clone()).collect();
            let at = stepped
                .entry(t.name.clone())
                .or_insert_with(|| Some(BTreeSet::new()));
            if let Some(fields) = at {
                fields.extend(refers);
            }
        }
        let changed = (&stepped, Some(&m.to.name));
        viewed::stepped_under(&w.ontology, changed, judged.unwrap_or(self.who))?;
        if m.to.key != before.key {
            for k in &m.to.key {
                if !m.to.fields.iter().any(|f| &f.name == k && f.required) {
                    let said = Said::from(Case::KeyFieldRequired { field: k.clone() });
                    let name = m.to.name.clone();
                    return Err(Error::invalid_at(k, said.within(Place::Type { name })));
                }
            }
        }
        let mut probe = w.ontology.clone();
        probe.put(Declaration::Type(before.clone()), 0, None);
        let to = Declaration::Type(m.to.clone());
        if let Err(e) = declare::check_declaration(&probe, &to, false) {
            // A reshape is answered before the tools' names are read, and a
            // migration leaves no problem among them either (027, extended).
            declare::unless_a_migration(e)?;
            declare::tool_names(&probe, &to)?;
        }
        // A view over the type, as its source or a merged one, reads the
        // shape the migration leaves: `declare` must still take each such
        // view against it, or the view would refuse every read after.
        let mut after = probe.clone();
        after.put(Declaration::Type(m.to.clone()), 0, None);
        // The refusal of a migration that takes away a field `reader` reads:
        // one the type, or a link type of it, has before it and not after.
        let has = |t: &TypeDecl, n: &FieldName| t.has_field(n);
        let after_it = |said: Said| {
            let name = m.to.name.clone();
            Error::invalid(said.within(Place::AfterMigration { name }))
        };
        let gone =
            |reader: Option<Place>, names: &BTreeSet<FieldName>, was: &TypeDecl, is: &TypeDecl| {
                let n = names.iter().find(|n| has(was, n) && !has(is, n))?;
                // At an accept the field may be one the accepter may not know of,
                // taken away knowingly by the proposer: it is told neither the
                // field nor its reader (013, extended).
                let unknown = |f: &&FieldDecl| sees.field_access(was, f) == Access::None;
                if was
                    .fields
                    .iter()
                    .filter(|f| &f.name == n)
                    .any(|f| unknown(&f))
                {
                    return Some(after_it(Case::ReadsTakenAway.into()));
                }
                let renamed =
                    |(from, _): &&(FieldName, FieldName)| from == n && is.name == m.to.name;
                let renamed = m.renames.iter().find(renamed).map(|(_, to)| to.clone());
                let (unnamed, field, ty) = (reader.is_none(), n.clone(), is.name.clone());
                let case = Case::MigrationTakesRead {
                    unnamed,
                    field,
                    ty,
                    renamed,
                };
                let said = Said::from(case);
                Some(after_it(match reader {
                    Some(reader) => said.within(reader),
                    None => said,
                }))
            };
        for reader in w.ontology.types.values().map(|e| &e.decl) {
            let Some(v) = reader.view.as_deref() else {
                continue;
            };
            if v.source != m.to.name && !v.merge.iter().any(|s| s.source == m.to.name) {
                continue;
            }
            let view = Declaration::Type(reader.clone());
            // `declare` checks a view's grouping against its sources; the
            // fields its `where` and measures read are checked by the rule
            // `describe` uses to show a view.
            let reads = describe::view_reads(v).unwrap_or_default();
            let taken = |reader: Option<Place>| {
                let mut of = reads.iter().filter(|(source, _)| source == &m.to.name);
                of.find_map(|(_, names)| gone(reader.clone(), names, &before, &m.to))
            };
            let after_place = Place::AfterMigration {
                name: m.to.name.clone(),
            };
            let checked =
                declare::check_declaration(&after, &view, false).map_err(|e| e.within(after_place));
            let name = reader.name.clone();
            let named =
                checked.and_then(|()| taken(Some(Place::View { name })).map_or(Ok(()), Err));
            // The view is named only to an author `describe` shows its
            // definition to (027, extended); another is told that a
            // declaration reads the field, or that one could not be declared.
            let Err(named) = named else { continue };
            if sees.shows(&view) {
                return Err(named);
            }
            return Err(taken(None).unwrap_or_else(|| {
                let ty = m.to.name.clone();
                after_it(Case::NoLongerDeclarable { ty }.into())
            }));
        }
        // So does every other declaration in force (027, extended): the
        // type's derived fields, another type's derived relation to it, and
        // the sets, triggers and verbs on it keep the fields they read; and
        // those on a link type it changes keep the relation's fields, one
        // that stands for a renamed relation's links to arrive in too.
        let links = changed_link_types(&w.ontology, m, true);
        let kept = links.iter().map(|(was, is)| (was, is));
        for (was, is) in std::iter::once((&before, &m.to)).chain(kept) {
            for d in after.declared() {
                let reads = declare::reads_of(&w.ontology, &d.declaration, &was.name);
                let Some(named) = gone(Some(d.declaration.place()), &reads, was, is) else {
                    continue;
                };
                // The reader is named only to an author `describe` shows it
                // to, as it stands in force, and by what it is shown of it: a
                // relation it is not shown reads nothing it is told of
                // (lakelands#662). Another is told that a declaration reads
                // the field. The field is one the author knows of: one
                // hidden from it was put back (`as_author`).
                if sees.shows(&d.declaration) {
                    let Declaration::Type(t) = &d.declaration else {
                        return Err(named);
                    };
                    let mut t = t.clone();
                    sees.known_fields(&mut t, &|n| w.ontology.ty(n).ok());
                    let seen = Declaration::Type(t);
                    let seen = describe::declaration_reads(&seen, &was.name, &|_| None);
                    if let Some(named) = gone(Some(d.declaration.place()), &seen, was, is) {
                        return Err(named);
                    }
                }
                gone(None, &reads, was, is).map_or(Ok(()), Err)?;
            }
        }
        // A verb is the one door left to a link type no relation carries,
        // so a relation is not removed while a verb in force is declared on
        // its link type (035; 027, extended). The verb is named to an author
        // `describe` shows it to.
        let kept = links::link_types_of(&m.to);
        for r in before.relations.iter().filter(|r| r.carried_by_links()) {
            let ty = links::link_type_name(&before.name, &r.name);
            // A renamed relation is the rename's to judge, below.
            let renamed = m.relation_renames.iter().any(|(old, _)| old == &r.name);
            if renamed || kept.iter().any(|t| t.name == ty) {
                continue;
            }
            let on = (w.ontology.verbs.values()).filter(|e| e.decl.on == ty);
            let mut on = on.map(|e| Declaration::Verb(e.decl.clone())).peekable();
            if on.peek().is_some() {
                let shown = on.find(|v| sees.shows(v));
                let (unnamed, relation) = (shown.is_none(), r.name.clone());
                let said = Said::from(Case::MigrationTakesLinkType {
                    unnamed,
                    relation,
                    ty,
                });
                return Err(after_it(match shown {
                    Some(verb) => said.within(verb.place()),
                    None => said,
                }));
            }
            // Nor while a relation or a reference field that stands after it
            // refers to the link type, a link type's own `to` aside, which is
            // its relation's: it would name the links of no relation (035);
            // named to one shown it.
            let types = after.types.values().map(|e| &e.decl);
            let field = |t: &TypeDecl, f: &FieldDecl| {
                let end = links::is_link_type(&t.name) && f.name.as_str() == links::TO;
                !end && f.ty.target.as_ref() == Some(&ty)
            };
            let refers = |t: &&TypeDecl| {
                let fields = t.fields.iter().any(|f| field(t, f));
                t.relations.iter().any(|o| o.target == ty) || fields
            };
            let mut referring = types.filter(refers).peekable();
            if referring.peek().is_some() {
                let shown = referring.find_map(|t| {
                    let related = |o: &&RelationDecl| o.target == ty && sees.relation(t, o);
                    let known =
                        |f: &&FieldDecl| field(t, f) && sees.field_access(t, f) != Access::None;
                    let place = match t.relations.iter().find(related) {
                        Some(o) => Place::Relation {
                            name: o.name.clone(),
                        },
                        None if sees.ty(&t.name).is_ok() => {
                            let f = t.fields.iter().find(known)?;
                            Place::Field {
                                name: f.name.clone(),
                            }
                        }
                        None => return None,
                    };
                    Some((t.name.clone(), place))
                });
                let (unnamed, relation) = (shown.is_none(), r.name.clone());
                let said = Said::from(Case::MigrationTakesTarget {
                    unnamed,
                    relation,
                    ty,
                });
                return Err(after_it(match shown {
                    Some((of, place)) => said.within(place).within(Place::Type { name: of }),
                    None => said,
                }));
            }
        }
        // And the type it migrates to reads only fields it has: `declare`'s
        // check above ends at the reshaping, before it comes to the reads.
        declare::reads_its_type(&after, &Declaration::Type(m.to.clone()))?;
        // So does every verb that reads into it (028, 055).
        let shows = |d: &Declaration| sees.shows(d);
        verbs_judged_again((&w.ontology, &after), &before, &m.to, &shows).map_err(after_it)?;
        // A record that does not convert is already not lossless; its
        // stored fields still tell whether it has a new key.
        // A live record holds what the migration fills, as the fold leaves
        // it, so its key and its memberships are decided with it (050).
        let convert = |decl: &TypeDecl, m: &Migration, r: &StoredRecord| {
            let fields = shape::migrate_record(decl, m, &r.fields);
            let mut fields = fields
                .map_err(Error::invalid)?
                .unwrap_or_else(|| r.fields.clone());
            for f in (m.to.fields.iter()).filter(|f| m.fill.contains(&f.name) && !r.retired) {
                if !fields.contains_key(&f.name) {
                    fields.extend(record::initial_of(&after, f).map(|v| (f.name.clone(), v)));
                }
            }
            // And what it makes the writer's takes the record's maker (084).
            let writers = shape::becoming_writer(decl, m).map_err(Error::invalid)?;
            for name in writers.into_iter().filter(|_| !r.retired) {
                fields.insert(name, Value::from(record::maker(r)));
            }
            Ok::<_, Error>(fields)
        };
        // Applied, the migration decides again every record whose fields it
        // changes, as the fold leaves it (004, extended): converted, its
        // references to moved keys rewritten, under its key after `moves`.
        let mut members = Vec::new();
        let limit = self.lake.limits.max_set_changes as usize;
        // Where the fold computes the type again, a record is decided as it
        // computes it, over the lake after the migration (lakelands#1070).
        let again = (!m.fill.is_empty() && computes(&after, &m.to.name))
            || computing_changed(&before, &m.to);
        let (computing, plans) = (
            after.computing(std::slice::from_ref(&m.to.name)),
            Default::default(),
        );
        // Its constraints are judged on the records as computed again, at
        // the lake's last time where nothing is applied (lakelands#1119).
        let judged_at = now.unwrap_or(w.last_known_at);
        let event = again.then(|| {
            let body = EventBody::Declared {
                declaration: Declaration::Type(m.to.clone()),
                schema_hash: None,
                version: 0,
                with_the_set: false,
            };
            provisional(
                self.lake,
                position_after(w, 0),
                &self.draft(judged_at, Origin::System, body),
            )
        });
        // A record as the migration leaves it: its references to moved keys
        // rewritten, under its key after `moves`, computed again where the
        // fold computes its type again.
        let migrated = |moves: &crate::rekey::Moves, decl: &TypeDecl, r: &StoredRecord, fields| {
            let moved = moves.get(&r.ty).and_then(|m| m.get(&r.key));
            let mut is = StoredRecord {
                key: moved.unwrap_or(&r.key).clone(),
                fields: crate::rekey::rewritten(decl, &fields, moves),
                ..r.clone()
            };
            if let Some(e) = event.as_ref().filter(|_| r.ty == m.to.name) {
                is = computed_again(
                    (&computing, view, &plans, &self.lake.keys),
                    &is,
                    e,
                    &w.settings,
                )?;
            }
            Ok::<_, Error>(is)
        };
        // What a set's path reads across an edge, as the migration leaves
        // it: the record at a key after `moves`, converted.
        let far = |moves: &crate::rekey::Moves, t: &TypeName, k: &RecordKey| {
            let mut back = moves.get(t).into_iter().flatten().filter(|(_, n)| *n == k);
            let old = back.next().map_or(k, |(o, _)| o);
            let Some(r) = view.get(t, old)? else {
                return Ok(None);
            };
            let fields = match t == &m.to.name {
                true => convert(&before, m, &r)?,
                false => r.fields.clone(),
            };
            migrated(moves, after.ty(t)?, &r, fields).map(Some)
        };
        let link = |t: &TypeName, r: &RelationName, k: &RecordKey| link_from(view, t, r, k);
        let max_depth = self.lake.limits.max_depth;
        let left = |moves: &crate::rekey::Moves,
                    sets: &Held<'_>,
                    decl: &TypeDecl,
                    r: &StoredRecord,
                    fields,
                    now,
                    members: &mut Vec<Change>| {
            let is = migrated(moves, decl, r, fields)?;
            let far = |t: &TypeName, k: &RecordKey| far(moves, t, k);
            let heads = Heads {
                far: &far,
                link: &link,
                max: max_depth,
            };
            decide(sets, &r.key, &is, (w, now, limit), members, heads)
        };
        let held = |ty: &TypeName| match now {
            Some(_) => sets_of(w, view, ty, |_| true),
            None => Ok(Vec::new()),
        };
        // One walk of the type, a page at a time, makes the report, the key
        // of every record as the new key fields make it, moved or not (a
        // key the new fields make differently moves the record, and what
        // names it moves with it: `crate::rekey`), and each record's
        // memberships as if no key moved.
        let (sets, mut keyed) = (held(&m.to.name)?, Vec::new());
        // The renames as the author sent them are judged first, against the
        // type as it sees it: a field it may not know of is no field to it,
        // here, as an absent one is (lakelands#750).
        // So is what it fills, against the fields it declared itself.
        if let Some(a) = author {
            a.sent.renamed_fields(&a.known).map_err(Error::invalid)?;
            a.sent.fills().map_err(Error::invalid)?;
        }
        m.fills().map_err(Error::invalid)?;
        // Each field the migration fills: the live records that hold no
        // value for it, by the name they hold it under, and the first keys,
        // in key order, of those its reader is answered.
        let none = |f: &FieldName| (f.clone(), 0, Vec::new());
        let mut fills: Vec<(FieldName, u64, Vec<RecordKey>)> = m.fill.iter().map(none).collect();
        // The types a re-key may move, told by the declarations alone: the
        // migrated type where its key, or a key field's type, changes, and
        // each type whose key references one of them. The references to
        // them, in any other type, are what the re-key may rewrite.
        let renamed_to = |k: &FieldName| {
            let to = m.renames.iter().find(|(o, _)| o == k);
            to.map_or_else(|| k.clone(), |(_, n)| n.clone())
        };
        let typed = |t: &TypeDecl, k: &FieldName| {
            let f = t.field(k);
            f.map(|f| f.ty.clone())
        };
        let rekeys = before
            .key
            .iter()
            .map(renamed_to)
            .ne(m.to.key.iter().cloned())
            || (before.key.iter().zip(&m.to.key))
                .any(|(o, n)| typed(&before, o) != typed(&m.to, n));
        let mut moving: BTreeSet<TypeName> =
            rekeys.then(|| m.to.name.clone()).into_iter().collect();
        let targets = |t: &TypeDecl, f: &FieldName, among: &BTreeSet<TypeName>| {
            typed(t, f)
                .and_then(|ty| ty.target)
                .is_some_and(|x| among.contains(&x))
        };
        let decls = || (after.types.values().map(|e| &e.decl)).filter(|t| t.view.is_none());
        loop {
            let keyed = |t: &&TypeDecl| {
                !moving.contains(&t.name) && t.key.iter().any(|k| targets(t, k, &moving))
            };
            let more: Vec<TypeName> = decls().filter(keyed).map(|t| t.name.clone()).collect();
            if more.is_empty() {
                break;
            }
            moving.extend(more);
        }
        // What the migration may move of its own type, told by its
        // declaration alone: a field it converts, fills, renames or adds,
        // one it computes again, a reference it may rewrite. A constraint
        // its author is not told is judged by none of them: one that
        // refuses and that it may move is not granted, before any record
        // is read, and none is counted in its report (lakelands#1119).
        let was = |f: &FieldName| {
            let from = m.renames.iter().find(|(_, n)| n == f);
            from.map_or_else(|| f.clone(), |(o, _)| o.clone())
        };
        let computed =
            |f: &FieldDecl| f.derived.is_some() || f.function.is_some() || f.embed.is_some();
        let moves_field = |f: &FieldDecl| {
            let old = was(&f.name);
            m.fill.contains(&f.name)
                || old != f.name
                || typed(&before, &old) != Some(f.ty.clone())
                || (again && computed(f))
                || targets(&m.to, &f.name, &moving)
        };
        let moved: BTreeSet<FieldName> = (m.to.fields.iter())
            .filter(|f| moves_field(f))
            .map(|f| f.name.clone())
            .collect();
        let shape = record::Plans::default().get(&after, &m.to);
        let told =
            |c: &record::Constraint| (c.reads.iter()).all(|f| sees.reads_both(&before, m, f));
        for c in shape
            .constraints
            .iter()
            .filter(|c| c.walked.is_some() && !c.fed.is_disjoint(&moved))
        {
            granted(!c.decl.at_ingest.refuses() || told(c))?;
        }
        let untold: BTreeSet<&ConstraintName> = (shape.constraints.iter())
            .filter(|c| !told(c))
            .map(|c| &c.decl.name)
            .collect();
        // Each constraint `to` declares: the live records it would leave
        // breaking it, as converted and filled (lakelands#1119), judged
        // without what only a write judges, its clock and the record
        // before it (lakelands#1552).
        let parse =
            |c: &ConstraintDecl| declare::typed(&c.holds, Eval::Constraint).map_err(Error::invalid);
        // The records told of (a held one only to a holder of
        // `restricted`, 062), the first keys, and whether any breaks it.
        type Breaking<'c> = (
            &'c ConstraintDecl,
            lakelands_api::expr::Expr,
            u64,
            Vec<RecordKey>,
            bool,
        );
        let breaking = (m.to.constraints.iter()).filter(|c| !untold.contains(&c.name));
        let breaking =
            breaking.map(|c| Ok(parse(c)?.walked().map(|e| (c, e, 0, Vec::new(), false))));
        let breaking = breaking.filter_map(Result::transpose);
        let mut breaking: Vec<Breaking<'_>> = breaking.collect::<Result<_>>()?;
        // A live record counted against each, as the migration leaves it.
        let tally = |breaking: &mut Vec<Breaking<'_>>, r: &StoredRecord, is: &StoredRecord| {
            let scope = WriteScope {
                after: &is.fields,
                before: Some(&is.fields),
                retiring: false,
                meta: Some(&is.meta),
                settings: Some(&w.settings),
                preceding: None,
                now: judged_at,
            };
            for (_, e, records, samples, any) in breaking.iter_mut() {
                if !expr::holds(e, &scope).unwrap_or(false) {
                    *any = true;
                    *records += u64::from(!sees.withheld(r));
                    let shown = || crate::reads::answered(view, &sees, &m.to.name, &r.key);
                    if samples.len() < 5 && !sees.withheld(r) && shown()? {
                        samples.push(r.key.clone());
                    }
                }
            }
            Ok::<_, Error>(())
        };
        // A retarget is judged as its author is answered; at an accept, its
        // proposer (lakelands#1071).
        let onto = Sight::held(judged.unwrap_or(self.who), &w.ontology);
        // The references it may move, told as `shown_report` tells them, and
        // the live records it computes again (062).
        let (mut unlived, mut recomputed) = (Unlived::of(&m.to, &moved, &sees), 0);
        let mut report = shaped(view, (&sees, &onto), &before, m, |r| {
            for (field, records, samples) in fills.iter_mut().filter(|_| !r.retired) {
                let held = m.renames.iter().find(|(_, to)| to == field);
                let held = held.map_or(&*field, |(from, _)| from);
                if !r.fields.contains_key(held) {
                    *records += u64::from(!sees.withheld(r));
                    let shown = || crate::reads::answered(view, &sees, &m.to.name, &r.key);
                    if samples.len() < 5 && !sees.withheld(r) && shown()? {
                        samples.push(r.key.clone());
                    }
                }
            }
            if !r.retired {
                recomputed += u64::from(again && !sees.withheld(r));
                if !breaking.is_empty() || unlived.watches() {
                    let is = migrated(&BTreeMap::new(), &m.to, r, convert(&before, m, r)?)?;
                    if !breaking.is_empty() {
                        tally(&mut breaking, r, &is)?;
                    }
                    unlived.note((r, &is), &sees);
                }
            }
            if m.to.key.is_empty() && sets.is_empty() {
                return Ok(());
            }
            let fields = convert(&before, m, r)?;
            if !m.to.key.is_empty() {
                let key = lakelands_core::log::record_key(&m.to, &fields);
                keyed.push((r.key.clone(), key));
            }
            match now.filter(|_| !sets.is_empty()) {
                Some(now) => left(&BTreeMap::new(), &sets, &m.to, r, fields, now, &mut members),
                None => Ok(()),
            }
        })?;
        let filled = |(field, records, sample_keys)| FieldFill {
            field,
            records,
            sample_keys,
        };
        report.filled = fills.into_iter().map(filled).collect();
        (report.recomputed, report.unlive) =
            (Some(recomputed), unlived.counted(view, &sees, &m.to.name)?);
        // A colliding key is the new key's fields' values: named to one
        // who reads each of them and is answered two of the records that
        // would take it, a collision it can see for itself.
        let reads = (m.to.key_fields().iter()).all(|f| sees.reads_both(&before, m, f));
        let seen = |ty: &TypeName, keys: &[&RecordKey]| match reads {
            true => crate::reads::answered_of(view, &sees, ty, keys, 2),
            false => Ok(false),
        };
        let plan = crate::rekey::plan(&after, view, &m.to.name, keyed, &m.renames, &seen)?;
        // With moves its constraints are judged again, on the references the
        // moves leave, a second walk; and the records of other types whose
        // references they rewrite hold theirs (lakelands#1119).
        if !breaking.is_empty() && !plan.moves.is_empty() {
            breaking
                .iter_mut()
                .for_each(|b| (b.2, b.3, b.4) = (0, Vec::new(), false));
            crate::lake::walk(view, &before.name, true, |r| match r.retired {
                true => Ok(()),
                false => tally(
                    &mut breaking,
                    &r,
                    &migrated(&plan.moves, &m.to, &r, convert(&before, m, &r)?)?,
                ),
            })?;
        }
        for (c, _, records, sample_keys, _) in breaking.into_iter().filter(|b| b.4) {
            // A proposal only tells: a record held from its proposer moves
            // nothing it is answered, so it makes the report lossy only at
            // the accept, which judges every record (062).
            report.lossless &= !c.at_ingest.refuses() || (judging.tells() && records == 0);
            if records == 0 {
                continue;
            }
            let constraint = c.name.clone();
            report.breaches.push(Breach {
                ty: m.to.name.clone(),
                constraint,
                records,
                sample_keys,
                field: None,
            });
        }
        let mut others = Vec::new();
        for decl in decls().filter(|t| t.name != m.to.name) {
            let refs = (decl.fields.iter()).filter(|f| targets(decl, &f.name, &moving));
            let refs: BTreeSet<FieldName> = refs.map(|f| f.name.clone()).collect();
            if refs.is_empty() {
                continue;
            }
            let mut walked = Walked::of(&after, decl, &refs, (&sees, Judging::Act))?;
            // The references the moves rewrite, as they leave them (062).
            let mut unlived = Unlived::of(decl, &refs, &sees);
            if (walked.judges(true) || unlived.watches()) && !plan.moves.is_empty() {
                let shown = |r: &StoredRecord| match sees.withheld(r) {
                    true => Ok(false),
                    false => crate::reads::answered(view, &sees, &decl.name, &r.key),
                };
                live(view, &decl.name, |r| {
                    let fields = crate::rekey::rewritten(decl, &r.fields, &plan.moves);
                    let is = StoredRecord {
                        fields,
                        ..r.clone()
                    };
                    // A rewritten reference names the record its old key
                    // named, which moves with it: judged by the old key.
                    unlived.note((r, r), &sees);
                    walked.judge((r, &is), (w, judged_at), &shown)
                })?;
                let unlive = unlived.counted(view, &sees, &decl.name)?;
                if !unlive.is_empty() {
                    others.push(MigrationReport {
                        ty: decl.name.clone(),
                        recomputed: Some(0),
                        unlive,
                        lossless: true,
                        ..MigrationReport::default()
                    });
                }
            }
            // As its own type's: a proposal is lossy by what it is told of.
            let (breaches, lossless) = walked.kept(judging)?;
            report.breaches.extend(breaches);
            report.lossless &= lossless;
        }
        // With moves the memberships are decided again, under the keys and
        // the references the moves leave: a second walk.
        if let Some(now) = now.filter(|_| !sets.is_empty() && !plan.moves.is_empty()) {
            members.clear();
            crate::lake::walk(view, &before.name, true, |r| {
                let fields = convert(&before, m, &r)?;
                left(&plan.moves, &sets, &m.to, &r, fields, now, &mut members)
            })?;
        }
        // A relation carried by link records whose cardinality narrows
        // keeps its links only if none would break the narrower rule.
        for r in m.to.relations.iter().filter(|r| r.carried_by_links()) {
            let Some(old) = before
                .relations
                .iter()
                .find(|o| o.name == r.name && o.carried_by_links())
            else {
                continue;
            };
            let ((was_from, was_to), (per_from, per_to)) =
                (one_per(old.cardinality), one_per(r.cardinality));
            if !(per_from && !was_from) && !(per_to && !was_to) {
                continue;
            }
            let lt = links::link_type_name(&m.to.name, &r.name);
            // The live links at each end, by the record there: a page of
            // the links at a time, the ends kept.
            let mut ends: [BTreeMap<String, u32>; 2] = Default::default();
            // Told, a link held from its reader is no more one than a
            // retired link is (062).
            live(view, &lt, |l| {
                if judging.tells() && sees.withheld(l) {
                    return Ok(());
                }
                for (seen, at) in ends.iter_mut().zip([links::FROM, links::TO]) {
                    let key = l.fields.get(at).and_then(Value::as_str).unwrap_or_default();
                    *seen.entry(key.to_owned()).or_default() += 1;
                }
                Ok(())
            })?;
            // How many records hold more than one live link at an end.
            let over = |end: usize| ends[end].values().filter(|n| **n > 1).count();
            let broken = if per_from { over(0) } else { 0 } + if per_to { over(1) } else { 0 };
            if broken > 0 {
                let (relation, cardinality, count) = (r.name.clone(), r.cardinality, broken);
                let case = Case::NarrowingBreaksLinks {
                    relation,
                    cardinality,
                    count,
                };
                return Err(Error::constraint(None, case));
            }
        }
        // The relations carried by link records whose fields change migrate
        // their link types with the type: their records are checked here,
        // each field named with its relation.
        for (old, new) in changed_link_types(&w.ontology, m, false) {
            let lm = Migration::to(new);
            let sets = held(&old.name)?;
            let lr = shaped(view, (&sees, &onto), &old, &lm, |r| {
                match now.filter(|_| !sets.is_empty()) {
                    Some(now) => {
                        let fields = convert(&old, &lm, r)?;
                        left(&plan.moves, &sets, &lm.to, r, fields, now, &mut members)
                    }
                    None => Ok(()),
                }
            })?;
            let relation = old.name.as_str().rsplit('.').next().unwrap_or_default();
            let named = |f: &FieldName| FieldName::from(format!("{relation}.{f}"));
            report.records_scanned += lr.records_scanned;
            report.lossless &= lr.lossless;
            report.fields.extend(lr.fields.into_iter().map(|mut c| {
                c.field = named(&c.field);
                c
            }));
            report
                .dropped
                .extend(lr.dropped.into_iter().map(|(f, n)| (named(&f), n)));
        }
        // A relation the migration removes is listed as a removed field
        // is: one carried by link records takes its live links from the
        // head, and is listed with their number, another with none. A
        // relation of another name is another relation; a rename is not
        // inferred.
        // So are its relation renames, against the relations it is shown.
        if let Some(a) = author {
            a.sent.renamed_relations(&a.known).map_err(Error::invalid)?;
        }
        let pairs = m.renamed_relations(&before).map_err(Error::invalid)?;
        let removed = |r: &&RelationDecl| {
            !m.to.relations.iter().any(|n| n.name == r.name)
                && !pairs.iter().any(|(was, _)| was.name == r.name)
        };
        let mut unlinked = Vec::new();
        for r in before.relations.iter().filter(removed) {
            let lt = links::link_type_name(&m.to.name, &r.name);
            let (carried, held) = (r.carried_by_links(), unlinked.len());
            // A re-key would move the links its retirements name: one
            // migration removes, another re-keys, as for a rename.
            if carried && !plan.moves.is_empty() {
                let name = r.name.clone();
                let said = Said::from(Case::RemoveWhileRekeying);
                return Err(Error::invalid(said.within(Place::Relation { name })));
            }
            if carried && now.is_some() {
                live(view, &lt, |l| {
                    unlinked.push(l.clone());
                    Ok(())
                })?;
            }
            let links = match (carried, now) {
                (true, None) => view.count(&lt, &every(false))?,
                _ => (unlinked.len() - held) as u64, // the number it retires
            };
            let name = FieldName::from(r.name.as_str());
            report.dropped.push((name, links));
        }
        // What `to` states otherwise besides, each shape completed as one
        // in force is, so a `depends_on` left to the store restates nothing.
        let (mut was, mut is) = (before.clone(), m.clone());
        w.ontology.complete(&mut was);
        after.complete(&mut is.to);
        report.restated = is.restated(&was);
        // A renamed relation keeps its edges. One carried by link records
        // moves them to the link type of its new name, which must hold none
        // of an earlier relation's, and not in the migration that re-keys.
        for (was, is) in pairs {
            let (carrier, mut moved) = (Carrier::of(was), 0);
            if carrier == Carrier::Links {
                let wrong = |why: Case| {
                    let (from, to) = (was.name.clone(), is.name.clone());
                    Error::invalid(Said::from(why).within(Place::RelationRename { from, to }))
                };
                if !plan.moves.is_empty() {
                    return Err(wrong(Case::RenameWhileRekeying));
                }
                if stored(view, &links::link_type_name(&m.to.name, &is.name))? > 0 {
                    return Err(wrong(Case::RenameNameHeld));
                }
                let old = links::link_type_name(&m.to.name, &was.name);
                // A reference into the link type would name keys that end.
                let refers = |t: &&TypeDecl| {
                    let field = |f: &FieldDecl| f.ty.target.as_ref() == Some(&old);
                    t.fields.iter().any(field) || t.relations.iter().any(|r| r.target == old)
                };
                let types = w.ontology.types.values().map(|e| &e.decl);
                let mut referring = types.filter(|t| t.name != old && refers(t)).peekable();
                if referring.peek().is_some() {
                    // The type is named only to an author `describe` shows it to.
                    let named = referring.find(|t| sees.ty(&t.name).is_ok());
                    let ty = named.map(|t| t.name.clone());
                    let links = old.clone();
                    return Err(wrong(Case::RenameReferredLinks { ty, links }));
                }
                moved = stored(view, &old)?;
            }
            report.relations.push(RelationRename {
                from: was.name.clone(),
                to: is.name.clone(),
                carrier,
                links: moved,
            });
        }
        // What `declare` needs bound, on the type and its link types, at
        // the preflight as at the apply, once the renames it reads are
        // judged, against the links stored too (lakelands#1124).
        self.check_bound(&to, Some(&w.ontology), Some(m))?;
        for d in carried(&w.ontology, m) {
            let key = lakelands_core::engines::declaration_key(&d);
            let (kind, name) = key.split_once(' ').unwrap_or_default();
            let (kind, name) = (kind.to_owned(), name.to_owned());
            report.carried.push(Carried { kind, name });
        }
        report.rekeyed = plan.rekeyed.clone();
        report.references = plan.references.clone();
        report.sample_collisions = plan.collisions.clone();
        // A record that would not convert, or would collide or lack its new
        // key, makes the report lossy to every reader alike, a proposer's
        // too: retired ones are converted and keyed with the rest, so one
        // held from its reader tells nothing that a retired one would not.
        report.lossless &= plan.lossless;
        let listed = plan.listed();
        report.report_id = report_id(&report, self.lake.report_head(view.position())?);
        // A record of another type whose reference to a moved key the fold
        // rewrites: decided for the sets whose filter reads that field, a
        // page of the type at a time.
        for (name, decl) in w.ontology.types.iter().map(|(n, e)| (n, &e.decl)) {
            let rewritten = crate::rekey::moved_references(decl, &plan.moves);
            // And for the sets whose `where` reads into the type across an
            // edge (055, extended).
            let into = BTreeSet::from([m.to.name.clone()]);
            let reads = |s: &ObjectSetDecl| {
                let sets = |n: &str| w.ontology.set(n);
                let at = [Place::ObjectSet {
                    name: s.name.clone(),
                }];
                let filter = declare::set_filter(&sets, s, &at);
                let reads = filter.map(|e| lakelands_core::derive::reads(&e));
                reads.is_ok_and(|r| rewritten.iter().any(|(f, _)| r.contains(*f)))
                    || across::reaches_into(&w.ontology, s, &into)
            };
            let other = name != &m.to.name && !links.iter().any(|(old, _)| &old.name == name);
            let Some(now) = now.filter(|_| other) else {
                continue;
            };
            let sets = sets_of(w, view, name, reads)?;
            if !sets.is_empty() {
                live(view, name, |r| {
                    left(
                        &plan.moves,
                        &sets,
                        decl,
                        r,
                        r.fields.clone(),
                        now,
                        &mut members,
                    )
                })?;
            }
        }
        Ok(MigrationPlan {
            report,
            others,
            rekeyed: listed,
            members,
            unlinked,
        })
    }

    /// `migrate_apply`. A migration to the type in force that renames
    /// nothing is answered at the head and appends nothing only where it
    /// fills no record (050), after every refusal, a stale report among
    /// them (027, extended).
    pub fn migrate_apply(&self, m: Migration, report_id: &str) -> Result<Outcome> {
        self.enacting()?;
        let against = Against {
            plan: None,
            report: Some(report_id),
        };
        let migration = Box::new(m);
        self.make(lakelands_api::write::Change::Migrate { migration }, against)
    }

    /// The drafts that apply a migration its preflight planned, refused
    /// with the report when it is not lossless: the `migrated` event first,
    /// then the set memberships it changes, each caused by the `migrated`,
    /// which is the `first` draft of its append not yet counted (an answer
    /// reads what directly follows its event), then the link types it
    /// changes or adds.
    pub(crate) fn migration_drafts(
        &self,
        w: &Writer,
        m: Migration,
        plan: MigrationPlan,
        now: Timestamp,
        first: usize,
    ) -> Result<Vec<Draft>> {
        let MigrationPlan {
            report,
            rekeyed,
            members,
            unlinked,
            ..
        } = plan;
        if !report.lossless {
            return Err(self.lossy(w, &m, report));
        }
        // Past the lake's ceiling, naming the set whose change passed it.
        let max = self.lake.limits.max_set_changes;
        if let Some((set, ..)) = members.get(max as usize) {
            return Err(Error::refused(
                Some(set.as_str().into()),
                Refusal::TooLarge { max },
            ));
        }
        let report_id = report.report_id;
        let moved: Vec<_> = (rekeyed.iter())
            .map(|r| (r.ty.clone(), r.from.clone()))
            .collect();
        let schema_hash = shape::schema_hash(&m.to).map_err(Error::invalid)?;
        let version = w.ontology.next_version(&Declaration::Type(m.to.clone()));
        let migrated = position_after(w, first);
        let changes = members.into_iter().map(|c| self.changed(c, migrated, now));
        let mut drafts: Vec<Draft> = changes.collect();
        for (_, new) in changed_link_types(&w.ontology, &m, false) {
            let hash = shape::schema_hash(&new).map_err(Error::invalid)?;
            let version = w.ontology.next_version(&Declaration::Type(new.clone()));
            drafts.push(Draft::system(
                now,
                EventBody::Migrated {
                    migration: Migration::to(new),
                    schema_hash: hash,
                    version,
                    report_id: report_id.clone(),
                    rekeyed: Vec::new(),
                },
            ));
        }
        // A relation new with this migration gets its link type declared,
        // and so does a renamed one whose new name's link type stands in
        // another shape: declared again, in the shape of the links it takes.
        let arriving = arriving(&m);
        let declared = link_type_drafts(&w.ontology, &m.to, now)?
            .into_iter()
            .filter(|d| {
                matches!(&d.body, EventBody::Declared { declaration: Declaration::Type(t), .. }
                    if !w.ontology.types.contains_key(&t.name) || arriving.contains(&t.name))
            });
        drafts.extend(declared);
        // What names a link type whose records move is declared again by
        // the lake under the new name, after that link type's declaration.
        for declaration in carried(&w.ontology, &m) {
            drafts.push(system_declared(&w.ontology, declaration, now)?);
        }
        let d = self.draft(
            now,
            Origin::Authored,
            EventBody::Migrated {
                migration: m,
                schema_hash,
                version,
                report_id,
                rekeyed,
            },
        );
        drafts.insert(0, d);
        // A moved record's pending calls are closed with the `migrated`:
        // they are digested on the key it left (064).
        if !moved.is_empty() {
            let view = self.lake.engine.view(None)?;
            let mut calls = crate::backed::pending_calls(view.as_ref())?;
            let left = moved
                .iter()
                .flat_map(|k| calls.remove(k).unwrap_or_default());
            let why = "its record moved to another key";
            drafts.extend(left.map(|id| crate::backed::closing(id, why, (now, migrated))));
        }
        // The live links of a relation the migration removes are retired
        // with it, each by its own event and with what a retirement sets
        // off, as a purge retires them (035): caused by the `migrated`.
        if !unlinked.is_empty() {
            let view = self.lake.engine.view(None)?;
            let mut d = self.decide(w, view.as_ref(), now, first + drafts.len());
            for before in unlinked {
                let body = retirement(&before.ty, &before.key, before.version);
                let retired = Draft {
                    cause: Some(migrated),
                    context: self.context.clone(),
                    ..Draft::system(now, body)
                };
                drafts.extend(d.retiring(&before, retired)?);
            }
        }
        Ok(drafts)
    }
}

/// A value type's report named by what it says to its reader and where
/// the lake stood (`head`), its own id left empty: an apply after the lake
/// moved is refused. Its uses, its misfits named and its `conditions` are
/// its reader's, so two readers shown otherwise are given two ids for one
/// lake, and what a reader is not shown moves no id; one that renames is
/// shown whole or not at all, and named whole.
pub(crate) fn value_report_id(
    report: &ValueTypeReport,
    to: &ValueTypeDecl,
    renames: &[(String, String)],
    head: u64,
) -> String {
    let mut said = serde_json::json!({
        "report": serde_json::to_value(report).expect("a report serialises"),
        "to": serde_json::to_value(to).expect("a declaration serialises"),
        "head": head,
    });
    if !renames.is_empty() {
        said["renames"] = serde_json::json!(renames);
    }
    lakelands_core::canonical::sha256_hex(&said)
}

/// The declarations in `ontology` whose expressions hold an old value of
/// `renames` as a literal, and the aging rules on a type that uses the
/// value type that move from or to one, in the report: those `who` is
/// shown by what they are, the others by that they are there. A literal
/// is listed wherever it stands, compared with a field of the value type
/// or not: they may name one. A rule is shown as a read of its record
/// would answer it, with its `on`, `from` and `to` (024).
pub(crate) fn conditions(
    ontology: &Ontology,
    view: &dyn ProjectionView,
    who: &Principal,
    report: &mut ValueTypeReport,
    renames: &BTreeMap<&str, &str>,
) -> Result<()> {
    use lakelands_api::expr::{Expr, Literal};
    let old = |e: &Expr| matches!(e, Expr::Lit { value: Literal::Text(t) } if renames.contains_key(t.as_str()));
    let declared = ontology.declared();
    let sees = Sight::held(who, &declared);
    let (mut shown, mut unshown) = (Vec::new(), false);
    for d in declared
        .iter()
        .filter(|d| lakelands_core::expr::declares(&d.declaration, &old))
    {
        match sees.shows_where(&d.declaration, &old) {
            true => shown.push(d.declaration.what()),
            false => unshown = true,
        }
    }
    let using = crate::lake::using_types(ontology, &report.value_type);
    let of_it = |r: &StoredRecord| {
        let text = |n: &str| r.fields.get(n).and_then(Value::as_str);
        let old = |n| text(n).is_some_and(|v| renames.contains_key(v));
        text("on").is_some_and(|t| using.contains(&t.into())) && (old("from") || old("to"))
    };
    let rule = TypeName::from(AGING_RULE_TYPE);
    let readable = ["on", "from", "to"]
        .iter()
        .all(|f| sees.reads(&rule, &(*f).into()));
    live(view, &rule, |r| {
        if of_it(r) {
            match readable && sees.answers(&rule, r) {
                true => shown.push(format!("the aging rule `{}`", r.key)),
                false => unshown = true,
            }
        }
        Ok(())
    })?;
    report.conditions = shown;
    report.conditions_unshown = unshown;
    Ok(())
}

/// What a value type's migration does, as its preflight plans it: the
/// report and, for one that renames values and is applied, the types
/// whose fields' `initial` it renames and the set memberships it changes.
pub(crate) struct ValueTypePlan {
    pub(crate) report: ValueTypeReport,
    initials: Vec<TypeDecl>,
    members: Vec<Change>,
}

impl Call<'_> {
    /// The preflight of a value type's migration: every value stored under
    /// it, in every type and field that uses it, retired and restricted
    /// records included, renamed as the migration renames it and checked as
    /// a write would check it. It asks for no grant: its door asks for
    /// `schema`, a proposal for none. Given `now`, the time of an apply, it
    /// decides each record's set memberships as the step leaves the record,
    /// with what reads a renamed value computed again and the new
    /// declaration in force. Judged as a proposal's plan, a constraint left
    /// broken refuses only by a record its reader is told of (062); a
    /// misfit counts every record alike, a retired one too.
    pub(crate) fn preflight_value_type(
        &self,
        w: &Writer,
        view: &dyn ProjectionView,
        m: &ValueTypeMigration,
        (now, judging): (Option<Timestamp>, Judging),
    ) -> Result<ValueTypePlan> {
        let to = &m.to;
        let current = w
            .ontology
            .value_types
            .get(to.name.as_str())
            .ok_or_else(|| {
                let value_type = to.name.clone();
                Error::invalid(Case::NoValueType { value_type })
            })?;
        match declare::value_type_change(&current.decl, to) {
            declare::ValueTypeChange::Same if m.renames.is_empty() => {
                let value_type = to.name.clone();
                return Err(Error::invalid(Case::ValueTypeSame { value_type }));
            }
            declare::ValueTypeChange::Reshaping => {
                let value_type = to.name.clone();
                return Err(Error::invalid(Case::ValueTypeReshapes { value_type }));
            }
            _ => {}
        }
        m.check_renames(&current.decl, accepts)
            .map_err(Error::invalid)?;
        // A rename rewrites the values of every field that uses it.
        let named = |f: &&FieldDecl| f.ty.value_type.as_ref() == Some(&to.name);
        let types = w.ontology.types.values().map(|e| &e.decl);
        let uses = |t: &TypeDecl| {
            t.fields
                .iter()
                .filter(named)
                .map(|f| f.name.clone())
                .collect()
        };
        let stepped = types.map(|t| (t.name.clone(), Some(uses(t))));
        let stepped = stepped.filter(|_| !m.renames.is_empty()).collect();
        viewed::stepped_under(&w.ontology, (&stepped, None), self.who)?;
        // The new declaration is checked as a declaration of its own; the
        // one in force would refuse it as a tightening.
        let mut probe = w.ontology.clone();
        probe.value_types.remove(to.name.as_str());
        declare::check_declaration(&probe, &Declaration::ValueType(to.clone()), false)?;
        probe.put(
            Declaration::ValueType(to.clone()),
            current.version + 1,
            None,
        );
        let renames: BTreeMap<&str, &str> = (m.renames.iter())
            .map(|(old, new)| (old.as_str(), new.as_str()))
            .collect();
        let renamed = |v: &Value| v.as_str().and_then(|v| renames.get(v).copied());
        let mut report = ValueTypeReport {
            value_type: to.name.clone(),
            from_version: current.version,
            records_scanned: 0,
            fields: Vec::new(),
            misfits: 0,
            first: Vec::new(),
            fits: true,
            report_id: String::new(),
            conditions: Vec::new(),
            conditions_unshown: false,
            breaches: Vec::new(),
        };
        let mut plan = ValueTypePlan {
            report: ValueTypeReport::default(),
            initials: Vec::new(),
            members: Vec::new(),
        };
        let limit = self.lake.limits.max_set_changes as usize;
        // What computing a record again reads, at the event the apply would
        // append: no call is made, an answer recorded is read.
        let event = provisional(
            self.lake,
            position_after(w, 0),
            &Draft::system(
                now.unwrap_or(Timestamp(0)),
                EventBody::Checkpoint {
                    through: Position::GENESIS,
                    hash: Digest::zero(),
                    segments: Vec::new(),
                },
            ),
        );
        // A record of a type that uses the value type as the step leaves
        // it, as what a set's path reads across an edge (055, extended).
        let using: BTreeSet<TypeName> = crate::lake::using_types(&probe, &to.name)
            .into_iter()
            .collect();
        let renamed_record = |r: StoredRecord| {
            let Some(decl) = probe.ty(&r.ty).ok().filter(|_| using.contains(&r.ty)) else {
                return Ok(r);
            };
            let reached = crate::lake::renamed_fields(&probe, decl, &to.name);
            let mut is = r.clone();
            for f in decl.fields.iter().filter(|f| reached.contains(&f.name)) {
                if let Some(new) = r.fields.get(&f.name).and_then(renamed) {
                    is.fields.insert(f.name.clone(), Value::from(new));
                }
            }
            let computing = probe.computing(std::slice::from_ref(&r.ty));
            let mut outputs = Outputs::planning(view, &w.plans, &self.lake.keys);
            let again = record::rederive(&computing, &is, &event, &w.settings, &mut outputs)?;
            Ok::<_, Error>(again.unwrap_or(is))
        };
        let far = |t: &TypeName, k: &RecordKey| view.get(t, k)?.map(renamed_record).transpose();
        let link = |t: &TypeName, r: &RelationName, k: &RecordKey| link_from(view, t, r, k);
        let heads = Heads {
            far: &far,
            link: &link,
            max: self.lake.limits.max_depth,
        };
        for name in crate::lake::using_types(&probe, &to.name) {
            let decl = probe.ty(&name)?;
            let uses: Vec<&FieldDecl> = (decl.fields.iter())
                .filter(|f| f.ty.value_type.as_ref() == Some(&to.name))
                .collect();
            let reached = crate::lake::renamed_fields(&probe, decl, &to.name);
            let reaches = |f: &FieldDecl| reached.contains(&f.name);
            // A value the lake computes from an expression that names an
            // old value would be the old value again at its next computation.
            for f in uses.iter().filter(|f| reaches(f)) {
                let function = f.function.as_deref().and_then(|n| probe.functions.get(n));
                let expr = f
                    .derived
                    .as_ref()
                    .or(function.and_then(|e| e.decl.expr.as_ref()));
                if expr.is_some_and(|c| {
                    lakelands_core::expr::names_text(c, &|t| renames.contains_key(t))
                }) {
                    let (ty, field) = (name.clone(), f.name.clone());
                    return Err(Error::invalid(Case::ComputedHoldsRenamed { ty, field }));
                }
            }
            // A field's `initial` that names an old value takes the new one:
            // the type is declared again by the lake after the event.
            let mut again = decl.clone();
            for f in again
                .fields
                .iter_mut()
                .filter(|f| reached.contains(&f.name))
            {
                if let Some(new) = f.initial.as_ref().and_then(renamed) {
                    f.initial = Some(Value::from(new));
                }
            }
            // An `initial` value no door would take is never filled (050):
            // it is a misfit of its field, told as a stored one is, by its
            // count alone to one who does not read the field.
            let room = 10 - report.first.len();
            let unfit = |f: &FieldDecl, v: &Value| {
                let patch: Patch = [(f.name.clone(), v.clone())].into();
                check_values(&probe, decl, &patch, &decl.fields).is_err()
            };
            let tally = |f: &&FieldDecl| {
                let mapped = again.field(&f.name);
                let initial = mapped.and_then(|g| g.initial.as_ref());
                ValueTypeUse {
                    ty: name.clone(),
                    field: f.name.clone(),
                    values_scanned: 0,
                    misfits: u64::from(initial.is_some_and(|v| unfit(f, v))),
                    renamed: (m.renames.iter())
                        .map(|(from, to)| ValueRenamed {
                            from: from.clone(),
                            to: to.clone(),
                            records: 0,
                        })
                        .collect(),
                }
            };
            if again != *decl {
                plan.initials.push(again.clone());
            }
            let mut used: Vec<(ValueTypeUse, Vec<Misfit>)> =
                uses.iter().map(|f| (tally(f), Vec::new())).collect();
            let sets = match now {
                Some(_) if !renames.is_empty() => sets_of(w, view, &name, |_| true)?,
                _ => Vec::new(),
            };
            let computing = probe.computing(std::slice::from_ref(&name));
            let sees = Sight::held(self.who, &w.ontology);
            // And its records as the renames leave them, computed again, hold
            // its constraints (lakelands#1119): what moves is each field of
            // the value type the renames reach.
            let moving = match renames.is_empty() {
                true => BTreeSet::new(),
                false => reached.iter().cloned().collect(),
            };
            let mut walked = Walked::of(&probe, decl, &moving, (&sees, Judging::Act))?;
            let shown = |r: &StoredRecord| match sees.withheld(r) {
                true => Ok(false),
                false => crate::reads::answered(view, &sees, &name, &r.key),
            };
            crate::lake::walk(view, &name, true, |r| {
                report.records_scanned += 1;
                let mut is = r.clone();
                for (f, (used, first)) in uses.iter().zip(&mut used) {
                    let Some(v) = r.fields.get(&f.name).filter(|v| !v.is_null()) else {
                        continue;
                    };
                    used.values_scanned += 1;
                    let misfit = match (renamed(v), reaches(f)) {
                        (Some(new), true) => {
                            if decl.key.contains(&f.name) {
                                let (ty, field) = (name.clone(), f.name.clone());
                                return Err(Error::invalid(Case::KeyHoldsRenamed { ty, field }));
                            }
                            let pair = used.renamed.iter_mut().find(|p| p.from == *v);
                            pair.into_iter().for_each(|p| p.records += 1);
                            let new = Value::from(new);
                            is.fields.insert(f.name.clone(), new.clone());
                            unfit(f, &new)
                        }
                        // An answer computed outside the lake is the
                        // backing's, which the lake does not rename: an old
                        // one would be read back against the new value type.
                        (Some(_), false) => true,
                        (None, _) => unfit(f, v),
                    };
                    if misfit {
                        used.misfits += 1;
                        // Named as a sample key is: of a record its reader
                        // is answered, one held as it stands asked no read,
                        // and in a field it reads, or it takes no place.
                        let shown = || crate::reads::answered(view, &sees, &name, &r.key);
                        let read = sees.reads(&name, &f.name) && !sees.withheld(&r);
                        if first.len() < room && read && shown()? {
                            first.push(Misfit {
                                ty: name.clone(),
                                key: r.key.clone(),
                                field: f.name.clone(),
                            });
                        }
                    }
                }
                if walked.judges(true) {
                    let mut outputs = Outputs::planning(view, &w.plans, &self.lake.keys);
                    let again =
                        record::rederive(&computing, &is, &event, &w.settings, &mut outputs)?;
                    let at = (w, now.unwrap_or(w.last_known_at));
                    walked.judge((&r, again.as_ref().unwrap_or(&is)), at, &shown)?;
                }
                let Some(now) = now.filter(|_| !sets.is_empty()) else {
                    return Ok(());
                };
                let mut outputs = Outputs::planning(view, &w.plans, &self.lake.keys);
                let is = record::rederive(&computing, &is, &event, &w.settings, &mut outputs)?
                    .unwrap_or(is);
                for (s, held) in &sets {
                    let member = held.contains(&r.key);
                    if plan.members.len() <= limit
                        && member != member_of((s, &probe), &is, &w.settings, now, heads)?
                    {
                        plan.members
                            .push((s.name.clone(), is.ty.clone(), is.key.clone(), !member));
                    }
                }
                Ok(())
            })?;
            let (breaches, lossless) = walked.kept(judging)?;
            let refuses = |b: &&Breach| {
                let c = decl.constraints.iter().find(|c| c.name == b.constraint);
                c.is_some_and(|c| c.at_ingest.refuses())
            };
            if let Some(b) = breaches.iter().find(refuses).filter(|_| !lossless) {
                let case = Case::ConstraintLeftBroken {
                    constraint: b.constraint.clone(),
                    ty: name.clone(),
                    records: b.records,
                };
                return Err(Error::constraint(None, case));
            }
            report.breaches.extend(breaches);
            for (used, first) in used {
                let room = 10 - report.first.len();
                report.first.extend(first.into_iter().take(room));
                report.misfits += used.misfits;
                report.fields.push(used);
            }
        }
        // The sets of other types whose `where` reads a renamed value across
        // an edge are decided with them.
        let others = probe.types.keys().filter(|t| !using.contains(*t));
        for name in others.filter(|_| now.is_some() && !renames.is_empty()) {
            let sets = sets_of(w, view, name, |s| across::reaches_into(&probe, s, &using))?;
            if sets.is_empty() {
                continue;
            }
            let now = now.unwrap_or(Timestamp(0));
            live(view, name, |r| {
                for (s, held) in &sets {
                    let member = held.contains(&r.key);
                    if plan.members.len() <= limit
                        && member != member_of((s, &probe), r, &w.settings, now, heads)?
                    {
                        plan.members
                            .push((s.name.clone(), r.ty.clone(), r.key.clone(), !member));
                    }
                }
                Ok(())
            })?;
        }
        report.fits = report.misfits == 0;
        if !renames.is_empty() {
            conditions(&w.ontology, view, self.who, &mut report, &renames)?;
        }
        let head = (self.lake.report_head(view.position())?).map_or(0, |p| p.0);
        // Named as its reader is shown it; one that renames is never trimmed.
        let shown = self.shown_value_type(&w.ontology, report.clone());
        let said = if renames.is_empty() { &shown } else { &report };
        report.report_id = value_report_id(said, to, &m.renames, head);
        plan.report = report;
        Ok(plan)
    }

    /// Put a value type's migration in force against the report the caller
    /// was shown: without renames, one `declared` at the next version, no
    /// record changed; with them, the drafts of [`Self::value_type_drafts`].
    pub fn migrate_value_type_apply(
        &self,
        m: ValueTypeMigration,
        report_id: &str,
    ) -> Result<Outcome> {
        self.enacting()?;
        let against = Against {
            plan: None,
            report: Some(report_id),
        };
        let migration = Box::new(m);
        let change = lakelands_api::write::Change::MigrateValueType { migration };
        self.make(change, against)
    }

    /// A migration that renames values writes every field that uses the
    /// value type: refused, naming nothing, to a caller not shown one of
    /// them.
    pub(crate) fn renames_shown(
        &self,
        o: &Ontology,
        m: &ValueTypeMigration,
        report: &ValueTypeReport,
    ) -> Result<()> {
        if m.renames.is_empty() {
            return Ok(());
        }
        let shown = self.shown_value_type(o, report.clone());
        granted(shown.fields.len() == report.fields.len())
    }

    /// The drafts that apply a value type's migration its preflight
    /// planned. Without renames, one `declared`. With them: the
    /// `value_type_migrated`, the `first` draft of its append not yet
    /// counted; then, as the system, a `declared` of each type whose field's
    /// `initial` it renames, one set (`with_the_set`), so that the value type
    /// in force accepts the new value (the fold never reads `initial`, so a
    /// page that ends between them is harmless); then the set memberships it
    /// changes, each caused by the `value_type_migrated`, refused
    /// `too_large` past the lake's `max_set_changes`, naming the set.
    pub(crate) fn value_type_drafts(
        &self,
        w: &Writer,
        m: ValueTypeMigration,
        plan: ValueTypePlan,
        now: Timestamp,
        first: usize,
    ) -> Result<Vec<Draft>> {
        if m.renames.is_empty() {
            let to = vec![Declaration::ValueType(m.to)];
            let declared = self.bundle_drafts(w, to, (now, first, Judging::Act));
            return declared.map(|b| b.drafts);
        }
        let max = self.lake.limits.max_set_changes;
        if let Some((set, ..)) = plan.members.get(max as usize) {
            return Err(Error::refused(
                Some(set.as_str().into()),
                Refusal::TooLarge { max },
            ));
        }
        let version = w
            .ontology
            .next_version(&Declaration::ValueType(m.to.clone()));
        let types = crate::lake::using_types(&w.ontology, &m.to.name);
        let body = EventBody::ValueTypeMigrated {
            declaration: m.to,
            version,
            renames: m.renames,
            report_id: plan.report.report_id,
            types,
        };
        let mut drafts = vec![self.draft(now, Origin::Authored, body)];
        let initials = plan.initials.len();
        for (i, t) in plan.initials.into_iter().enumerate() {
            let mut d = system_declared(&w.ontology, Declaration::Type(t), now)?;
            if let EventBody::Declared { with_the_set, .. } = &mut d.body {
                *with_the_set = i + 1 < initials;
            }
            drafts.push(d);
        }
        let migrated = position_after(w, first);
        drafts.extend(
            plan.members
                .into_iter()
                .map(|c| self.changed(c, migrated, now)),
        );
        Ok(drafts)
    }
}

/// Whether a value type's declaration accepts a text value: its kind, its
/// `one_of`, its `format` and its `constraint`.
fn accepts(vt: &ValueTypeDecl, v: &str) -> bool {
    let ty = lakelands_api::ontology::FieldType {
        kind: vt.kind,
        value_type: Some(vt.name.clone()),
        ..Default::default()
    };
    value::check(&Value::from(v), &ty, Some(vt)).is_ok_and(|v| satisfies(vt, &v).is_ok_and(|ok| ok))
}

/// A value type's preflight report, refused while a stored value would not
/// fit: with how many, and, for a caller with the `schema` grant
/// (`named`), the first of them. A proposal is told the count alone, so no
/// record is named in a field its proposer may not know of.
pub(super) fn fits(report: ValueTypeReport, named: bool) -> Result<ValueTypeReport> {
    if report.fits {
        return Ok(report);
    }
    let first = (report.first.iter().filter(|_| named)).map(|m| RecordName::FieldOf {
        ty: m.ty.clone(),
        key: m.key.clone(),
        field: m.field.clone(),
    });
    let (value_type, misfits, first) = (report.value_type, report.misfits, first.collect());
    Err(Error::invalid(Case::ValueTypeNotTightened {
        value_type,
        misfits,
        first,
    }))
}
