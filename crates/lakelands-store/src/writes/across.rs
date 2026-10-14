//! A set's `where` and a trigger's `when` across an edge
//! (`docs/decisions/055-a-path-steps-through-a-reference.md`, extended).
//!
//! Such a condition takes the paths a verb's takes, but is decided for no
//! reader (004): a far record is read at the head, whoever is answered it,
//! and one that is not live reads null. A write of the far record decides
//! again, in its own append, the records whose condition reads a field of
//! it that moved, or its liveness: found by stepping back along the path,
//! a scan of a type a step, and their memberships read once a set.

use super::*;
use lakelands_api::engine::Scan;
use lakelands_api::expr::Expr;
use lakelands_api::ontology::MatchPair;

/// The link the record of a type at a key holds under a relation.
pub(super) type Links<'a> =
    &'a dyn Fn(&TypeName, &RelationName, &RecordKey) -> Result<Option<RecordKey>>;

/// Where a set's `where` and a trigger's `when` read across an edge: the
/// record at a key, whoever is answered it; the one link a record holds
/// under a relation; and the lake's `max_depth`.
#[derive(Clone, Copy)]
pub(super) struct Heads<'a> {
    pub far: Far<'a>,
    pub link: Links<'a>,
    pub max: u32,
}

/// Whether `e` holds over `record` of `on` in `inner`'s scope, a path
/// reading across an edge as `heads` reaches: a far record that is not
/// live reads null, and so does a path past `max_depth`.
pub(super) fn holds<S: Scope>(
    (heads, o, on): (Heads<'_>, &Ontology, &TypeDecl),
    record: &StoredRecord,
    e: &Expr,
    inner: S,
    settings: &BTreeMap<String, Value>,
) -> bool {
    let live = |t: &TypeName, k: &RecordKey| Ok((heads.far)(t, k)?.filter(|r| !r.retired));
    let link = |r: &RelationName| (heads.link)(&on.name, r, &record.key);
    let now = inner.now();
    let across = Across::new(o, on, (&live, &link, &live), (heads.max, (settings, now)));
    let scope = VerbScope {
        record: inner,
        fields: &record.fields,
        payload: None,
        across: &across,
    };
    expr::holds(e, &scope).unwrap_or(false)
}

/// One step back along a path, from the record it reached to those that
/// reach it.
#[derive(Clone)]
enum Back<'d> {
    /// The records of a type whose reference field names it.
    Ref(&'d TypeName, &'d FieldName),
    /// The record of a type its link's `from` names.
    From(&'d TypeName),
    /// The records of a type whose derived relation matches onto it.
    Matched(&'d TypeName, &'d [MatchPair]),
}

/// One way a condition reaches a record of a type: the fields it reads
/// there, and the steps back to the condition's own record, nearest first.
pub(super) struct Reach<'d> {
    ty: &'d TypeName,
    reads: BTreeSet<FieldName>,
    back: Vec<Back<'d>>,
}

/// The ways the paths of `e`, a condition over `on`, reach records of
/// other types, and of `on` past its own record.
pub(super) fn reaches<'d>(o: &'d Ontology, on: &'d TypeDecl, e: &Expr) -> Vec<Reach<'d>> {
    let ty = |n: &TypeName| o.ty(n).ok();
    let mut paths = Vec::new();
    lakelands_api::expr::paths(e, &mut paths);
    let mut out = Vec::new();
    for p in paths {
        let link = on.relations.iter().find(|r| {
            r.carried_by_links()
                && r.name.as_str() == p[0].as_str()
                && !on.fields.iter().any(|f| f.name == p[0])
        });
        // Where the path begins at the condition's own record, its first
        // step reads there; from a link or a match it reads the far record.
        // A step reads one record, and a reference to read further from
        // leads to the next: the step back goes by the path's step, never
        // by its type, so a reference to a record of the same type (a
        // manager, a parent) is stepped back through too.
        let (mut back, steps, own, from) = match (on.derived_step(&p[0]), link) {
            (Some(r), _) => {
                let (Some(d), Some(target)) = (&r.derived, ty(&r.target)) else {
                    continue;
                };
                let [_, to] = d.reads();
                let back = vec![Back::Matched(&on.name, &d.pairs)];
                out.push(Reach {
                    ty: &target.name,
                    reads: to.into_iter().collect(),
                    back: back.clone(),
                });
                (back, target.stepped(&p[1..], &ty), false, 0)
            }
            (None, Some(r)) if p.len() == 1 => {
                let lt = links::link_type_name(&on.name, &r.name);
                if let Some(lt) = ty(&lt) {
                    let back = vec![Back::From(&on.name)];
                    let reads = BTreeSet::new();
                    out.push(Reach {
                        ty: &lt.name,
                        reads,
                        back,
                    });
                }
                continue;
            }
            // A link's `from` is read where the path begins: the next
            // step reads the same link.
            (None, Some(_)) => (vec![Back::From(&on.name)], on.stepped(p, &ty), false, 1),
            (None, None) => (Vec::new(), on.stepped(p, &ty), true, 0),
        };
        for (i, (t, f)) in steps.iter().enumerate() {
            if i > 0 || !own {
                let reads = BTreeSet::from([f.name.clone()]);
                out.push(Reach {
                    ty: &t.name,
                    reads,
                    back: back.clone(),
                });
            }
            if i + 1 < steps.len() && i >= from {
                back.insert(0, Back::Ref(&t.name, &f.name));
            }
        }
    }
    out
}

impl Decide<'_> {
    /// The heads a write reads across an edge: at the head and over the
    /// append's drafts.
    fn with_heads<R>(&self, f: impl FnOnce(Heads<'_>) -> R) -> R {
        let (view, pending) = (self.view, &self.pending);
        let far = |t: &TypeName, k: &RecordKey| pending.record(view, t, k);
        let link = |t: &TypeName, r: &RelationName, k: &RecordKey| pending.link(view, t, r, k);
        let max = self.call.lake.limits.max_depth;
        f(Heads {
            far: &far,
            link: &link,
            max,
        })
    }

    /// Whether a trigger fires on a write that leaves `after` (it was
    /// `before`): it has no `when`, or its `when` holds, with what a path
    /// reads across an edge.
    pub(super) fn fires(
        &self,
        t: &TriggerDecl,
        before: Option<&StoredRecord>,
        after: &StoredRecord,
    ) -> Result<bool> {
        let w = self.w;
        let Some(c) = &t.when else { return Ok(true) };
        let sets = |n: &str| w.ontology.set(n);
        let at = [Place::Trigger {
            name: t.name.clone(),
        }];
        let e = declare::filter(&sets, &t.on, c, Eval::Guard, &at)?;
        Ok(self.holds_at_write(&e, w.ontology.ty(&t.on)?, before, after))
    }

    /// Whether a trigger's `when`, `e` over `on`, holds at a write that
    /// leaves `after` (it was `before`).
    fn holds_at_write(
        &self,
        e: &Expr,
        on: &TypeDecl,
        before: Option<&StoredRecord>,
        after: &StoredRecord,
    ) -> bool {
        let (w, now) = (self.w, self.now);
        let live = before.filter(|b| !b.retired);
        let scope = WriteScope {
            after: &after.fields,
            before: live.map(|b| &b.fields),
            retiring: live.is_some() && after.retired,
            meta: Some(&after.meta),
            settings: Some(&w.settings),
            now,
        };
        self.with_heads(|h| holds((h, &w.ontology, on), after, e, scope, &w.settings))
    }

    /// Whether `record` is a member of `s` as the append leaves what its
    /// `where` reads across an edge.
    pub(super) fn member(&self, s: &ObjectSetDecl, record: &StoredRecord) -> Result<bool> {
        let w = self.w;
        self.with_heads(|h| member_of((s, &w.ontology), record, w, self.now, h))
    }

    /// What a write that leaves `after` (it was `before`) sets off across
    /// an edge, after its record's own reactions: the memberships of the
    /// records whose set reads a field of it that moved, or its liveness,
    /// decided again, and the triggers whose `when` reads so fired for each
    /// such record it holds for; `own` the memberships the write changed
    /// already. Past the lake's `max_set_changes`, counted over every
    /// append's such changes and firings, refused `too_large` naming the
    /// set or the trigger to a writer shown it, and naming nothing to
    /// another (004, extended).
    pub(super) fn across(
        &self,
        (before, after): (Option<&StoredRecord>, &StoredRecord),
        own: &BTreeMap<(String, TypeName, RecordKey), bool>,
        out: &mut Vec<(Draft, Option<Change>)>,
    ) -> Result<()> {
        let (w, now) = (self.w, self.now);
        let o = &w.ontology;
        // Each set and trigger: its name, its type, its condition, and
        // whether it is a set.
        let sets = (o.object_sets.values())
            .map(|e| (&*e.decl.name, &e.decl.of, Some(&e.decl.filter), true));
        let triggers = (o.triggers.values()).map(|e| {
            (
                e.decl.name.as_str(),
                &e.decl.on,
                e.decl.when.as_ref(),
                false,
            )
        });
        let was = before.filter(|b| !b.retired);
        let is = Some(after).filter(|a| !a.retired);
        let moved = |r: &Reach| {
            was.is_some() != is.is_some()
                || (r.reads.iter())
                    .any(|f| was.and_then(|b| b.fields.get(f)) != is.and_then(|a| a.fields.get(f)))
        };
        let max = self.call.lake.limits.max_set_changes as usize;
        for (name, of, c, set) in sets.chain(triggers) {
            let place = if set { Eval::Verb } else { Eval::Guard };
            let (Some(c), Ok(on)) = (c, o.ty(of)) else {
                continue;
            };
            let e = declare::filter(&|n| o.set(n), of, c, place, &[])?;
            let reached = reaches(o, on, &e);
            let mut found = BTreeMap::new();
            for r in reached.iter().filter(|r| r.ty == &after.ty && moved(r)) {
                for rec in self.back(&r.back, after.clone())? {
                    found.insert(rec.key.clone(), rec);
                }
            }
            if of == &after.ty {
                found.remove(&after.key);
            }
            if found.is_empty() {
                continue;
            }
            let held = match set {
                true => self.view.members(name, of)?,
                false => BTreeSet::new(),
            };
            for r in found.into_values() {
                let change = match set {
                    true => {
                        let at = (name.to_owned(), r.ty.clone(), r.key.clone());
                        let pending = self.pending.members.get(&at);
                        let was = own.get(&at).or(pending).copied();
                        let was = was.unwrap_or_else(|| held.contains(&r.key));
                        let scope = RecordScope {
                            fields: &r.fields,
                            meta: Some(&r.meta),
                            settings: Some(&w.settings),
                            now,
                        };
                        let is = !r.retired
                            && self.with_heads(|h| holds((h, o, on), &r, &e, scope, &w.settings));
                        if was == is {
                            continue;
                        }
                        let change = (at.0, at.1, at.2, is);
                        Some((self.call.changed(change.clone(), self.next, now), change))
                    }
                    false => {
                        if r.retired || !self.holds_at_write(&e, on, Some(&r), &r) {
                            continue;
                        }
                        let body = EventBody::TriggerFired {
                            trigger: name.into(),
                            ty: r.ty.clone(),
                            key: r.key.clone(),
                        };
                        let fired = Draft {
                            cause: Some(self.next),
                            context: self.call.context,
                            ..Draft::system(now, body)
                        };
                        out.push((fired, None));
                        None
                    }
                };
                if let Some((draft, change)) = change {
                    out.push((draft, Some(change)));
                }
                if self.bound && self.pending.across + out.len() > max {
                    // Named to a caller shown it; to another the size alone
                    // is told (004).
                    let decl = match set {
                        true => (o
                            .object_sets
                            .values()
                            .map(|e| Declaration::ObjectSet(e.decl.clone())))
                        .find(|d| matches!(d, Declaration::ObjectSet(s) if s.name == name)),
                        false => (o
                            .triggers
                            .values()
                            .map(|e| Declaration::Trigger(e.decl.clone())))
                        .find(|d| matches!(d, Declaration::Trigger(t) if t.name.as_str() == name)),
                    };
                    let shown = decl.is_some_and(|d| Sight::held(self.call.who, o).shows(&d));
                    let field = shown.then(|| name.into());
                    let reason = Refusal::TooLarge { max: max as u32 };
                    return Err(Error::Refused { field, reason });
                }
            }
        }
        Ok(())
    }

    /// The live records the steps `back` lead to from `at`, each as the
    /// append leaves it: one scan of a type a step, over the records it
    /// reached, and the append's own drafts of that type.
    fn back(&self, back: &[Back<'_>], at: StoredRecord) -> Result<Vec<StoredRecord>> {
        let mut at = vec![at];
        for step in back {
            let (ty, asked): (&TypeName, Vec<Vec<(FieldName, Value)>>) = match step {
                Back::From(on) => {
                    let from =
                        |r: &StoredRecord| r.fields.get(links::FROM)?.as_str().map(RecordKey::from);
                    let keys: BTreeSet<RecordKey> = at.iter().filter_map(from).collect();
                    let read = keys.iter().map(|k| self.pending.record(self.view, on, k));
                    at = read
                        .collect::<Result<Vec<_>>>()?
                        .into_iter()
                        .flatten()
                        .collect();
                    at.retain(|r| !r.retired);
                    continue;
                }
                Back::Ref(t, f) => {
                    let key = |r: &StoredRecord| vec![((*f).clone(), Value::from(r.key.as_str()))];
                    (t, at.iter().map(key).collect())
                }
                Back::Matched(t, pairs) => {
                    let pair = |r: &StoredRecord, p: &MatchPair| {
                        Some((p.from.clone(), r.fields.get(&p.to)?.clone()))
                    };
                    let tuple = |r: &StoredRecord| pairs.iter().map(|p| pair(r, p)).collect();
                    (t, at.iter().filter_map(tuple).collect())
                }
            };
            if asked.is_empty() {
                return Ok(Vec::new());
            }
            let holds = |r: &StoredRecord| {
                (asked.iter()).any(|a| a.iter().all(|(f, v)| r.fields.get(f) == Some(v)))
            };
            let scan = Scan {
                limit: u32::MAX,
                restricted: true,
                equal_any: asked.clone(),
                ..Default::default()
            };
            let mut next: BTreeMap<RecordKey, StoredRecord> = (self.view.scan(ty, &scan)?)
                .into_iter()
                .map(|r| (r.key.clone(), r))
                .collect();
            let drafted = self.pending.records.iter().filter(|((t, _), _)| t == ty);
            for ((_, k), r) in drafted {
                match holds(r) && !r.retired {
                    true => next.insert(k.clone(), r.clone()),
                    false => next.remove(k),
                };
            }
            at = next.into_values().collect();
        }
        Ok(at)
    }
}

impl Pending {
    /// The key of the one live link `from` of `ty` holds over `r`, as this
    /// append leaves it: one it drafted, or the projection's.
    pub(super) fn link(
        &self,
        view: &dyn ProjectionView,
        ty: &TypeName,
        r: &RelationName,
        from: &RecordKey,
    ) -> Result<Option<RecordKey>> {
        let lt = links::link_type_name(ty, r);
        let from_it = |l: &StoredRecord| {
            !l.retired && l.fields.get(links::FROM).and_then(Value::as_str) == Some(from.as_str())
        };
        let mut drafted = self
            .records
            .iter()
            .filter(|((t, _), l)| t == &lt && from_it(l));
        match drafted.next() {
            Some(((_, k), _)) => Ok(Some(k.clone())),
            None => link_from(view, ty, r, from),
        }
    }
}

/// Whether a set's `where` reaches a record of one of `types` across an
/// edge.
pub(super) fn reaches_into(o: &Ontology, s: &ObjectSetDecl, types: &BTreeSet<TypeName>) -> bool {
    reads_into(o, (&s.of, Some(&s.filter), Eval::Verb), types)
}

/// Whether a trigger's `when` reaches a record of one of `types` across
/// an edge.
pub(super) fn when_reaches_into(o: &Ontology, t: &TriggerDecl, types: &BTreeSet<TypeName>) -> bool {
    reads_into(o, (&t.on, t.when.as_ref(), Eval::Guard), types)
}

/// Whether a condition over `of`, in `place`, reaches a record of one of
/// `types` across an edge.
fn reads_into(
    o: &Ontology,
    (of, c, place): (&TypeName, Option<&lakelands_api::expr::Condition>, Eval),
    types: &BTreeSet<TypeName>,
) -> bool {
    let Some(e) = c.and_then(|c| declare::filter(&|n| o.set(n), of, c, place, &[]).ok()) else {
        return false;
    };
    o.ty(of)
        .is_ok_and(|on| reaches(o, on, &e).iter().any(|r| types.contains(r.ty)))
}

/// Whether a set's `where` reads across an edge: one of its paths reaches
/// another record.
pub(crate) fn reads_across(o: &Ontology, s: &ObjectSetDecl) -> bool {
    let e = declare::set_filter(&|n| o.set(n), s, &[]);
    let (Ok(on), Ok(e)) = (o.ty(&s.of), e) else {
        return false;
    };
    !reaches(o, on, &e).is_empty()
}
