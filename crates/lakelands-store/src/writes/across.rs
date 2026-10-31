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
use lakelands_api::event::{ChangeKind, DepthBound};
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
    (settings, strict): (&BTreeMap<String, Value>, bool),
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
    match strict {
        true => expr::holds_strict(e, &scope),
        false => expr::holds(e, &scope),
    }
    .unwrap_or(false)
}

/// One step back along a path, from the record it reached to those that
/// reach it.
#[derive(Clone, PartialEq)]
pub(crate) enum Back<'d> {
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
    pub ty: &'d TypeName,
    pub reads: BTreeSet<FieldName>,
    pub back: Vec<Back<'d>>,
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
            r.carried_by_links() && r.name.as_str() == p[0].as_str() && !on.has_field(&p[0])
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

/// What a decision across an edge reads besides the head: the records a
/// write's drafts, or the events a fold has folded, wrote so far.
pub(crate) trait Written {
    /// A record as they leave it, or as `view` holds it.
    fn record(
        &self,
        view: &dyn ProjectionView,
        ty: &TypeName,
        key: &RecordKey,
    ) -> Result<Option<StoredRecord>>;

    /// The records of `ty` they wrote, live or not.
    fn wrote<'a>(&'a self, ty: &'a TypeName) -> Box<dyn Iterator<Item = &'a StoredRecord> + 'a>;

    /// The records of `ty` in `view` whose fields hold one of `asked`, as
    /// they leave them, those they wrote left out.
    fn scan(
        &self,
        view: &dyn ProjectionView,
        ty: &TypeName,
        asked: &[Vec<(FieldName, Value)>],
    ) -> Result<Vec<StoredRecord>> {
        let scan = Scan {
            limit: u32::MAX,
            restricted: true,
            equal_any: asked.to_vec(),
            ..Default::default()
        };
        view.scan(ty, &scan)
    }
}

impl Written for Pending {
    fn record(
        &self,
        view: &dyn ProjectionView,
        ty: &TypeName,
        key: &RecordKey,
    ) -> Result<Option<StoredRecord>> {
        Pending::record(self, view, ty, key)
    }

    fn wrote<'a>(&'a self, ty: &'a TypeName) -> Box<dyn Iterator<Item = &'a StoredRecord> + 'a> {
        let of = self.records.iter().filter(move |((t, _), _)| t == ty);
        Box::new(of.map(|(_, r)| r))
    }
}

/// The heads a decision reads across an edge: at the head of `view` and
/// over what `written` wrote, to `max` steps.
pub(crate) fn heads<R>(
    (view, written, max): (&dyn ProjectionView, &dyn Written, u32),
    f: impl FnOnce(Heads<'_>) -> R,
) -> R {
    let far = |t: &TypeName, k: &RecordKey| written.record(view, t, k);
    let link = |t: &TypeName, r: &RelationName, k: &RecordKey| link(view, written, t, r, k);
    f(Heads {
        far: &far,
        link: &link,
        max,
    })
}

impl Decide<'_> {
    /// The heads a write reads across an edge: at the head and over the
    /// append's drafts.
    fn with_heads<R>(&self, f: impl FnOnce(Heads<'_>) -> R) -> R {
        let max = self.at.depth;
        heads((self.at.view, &self.pending, max), f)
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
        let Some(c) = &t.when else { return Ok(true) };
        let sets = |n: &str| self.ontology.set(n);
        let at = [Place::Trigger {
            name: t.name.clone(),
        }];
        let e = declare::filter(&sets, &t.on, c, Eval::Guard, &at)?;
        let on = self.ontology.ty(&t.on)?;
        self.holds_at_write((&e, t.when_strict), on, before, after)
    }

    /// The event of `t` firing on the record of `ty` at `key`, by a change
    /// of the kind `change` that made it where `created`: a
    /// `trigger_fired`, or a `trigger_dropped` where the write's depth is
    /// at or above the trigger's `max_depth` or the lake's
    /// `max_reactive_depth`, the lower that applies, since a reaction to it
    /// could write nothing (lakelands#1216).
    pub(super) fn firing(
        &self,
        t: &TriggerDecl,
        (ty, key): (&TypeName, &RecordKey),
        (change, created): (ChangeKind, bool),
    ) -> EventBody {
        let depth = self.call.context.depth;
        let lake = (self.call.lake.limits.max_reactive_depth, DepthBound::Lake);
        let own = t.max_depth.map(|m| (m, DepthBound::Trigger));
        let bound = own.into_iter().chain([lake]).filter(|(m, _)| depth >= *m);
        let (trigger, ty, key) = (t.name.clone(), ty.clone(), key.clone());
        match bound.min_by_key(|(m, _)| *m) {
            Some((max_depth, bound)) => EventBody::TriggerDropped {
                trigger,
                ty,
                key,
                depth,
                max_depth,
                bound,
            },
            None => EventBody::TriggerFired {
                trigger,
                ty,
                key,
                change: Some(change),
                created: Some(created),
            },
        }
    }

    /// Whether a trigger's `when`, `e` over `on`, holds at a write that
    /// leaves `after` (it was `before`), strictly where `strict`
    /// ([`expr::holds_strict`]). Its `preceding` reads the record before
    /// `after` in each series as a constraint's does, as its writer is
    /// answered ([`Self::preceding`]); a field a hiding lowers there reads
    /// as null, since a guard refuses no write.
    fn holds_at_write(
        &self,
        (e, strict): (&Expr, bool),
        on: &TypeDecl,
        before: Option<&StoredRecord>,
        after: &StoredRecord,
    ) -> Result<bool> {
        let now = self.at.now;
        let live = before.filter(|b| !b.retired);
        let mut preceding = expr::Preceding::new();
        for series in e.series().into_keys() {
            let found = self.preceding(on, after, &series)?;
            let mut fields = found.map(|r| r.fields).unwrap_or_default();
            for s in self.at.sights() {
                let low = s.lowered(&on.name, &fields);
                fields.retain(|f, _| !low.contains_key(f));
            }
            preceding.insert(series, fields);
        }
        let scope = WriteScope {
            after: &after.fields,
            before: live.map(|b| &b.fields),
            retiring: live.is_some() && after.retired,
            meta: Some(&after.meta),
            settings: Some(self.at.settings),
            preceding: Some(&preceding),
            now,
        };
        let settings = (self.at.settings, strict);
        Ok(self.with_heads(|h| holds((h, self.ontology, on), after, e, scope, settings)))
    }

    /// Whether `record` is a member of `s` as the append leaves what its
    /// `where` reads across an edge, under `settings`.
    pub(super) fn member(
        &self,
        s: &ObjectSetDecl,
        record: &StoredRecord,
        settings: &BTreeMap<String, Value>,
    ) -> Result<bool> {
        let o = self.ontology;
        self.with_heads(|h| member_of((s, o), record, settings, self.at.now, h))
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
        (change, own): (ChangeKind, &BTreeMap<(String, TypeName, RecordKey), bool>),
        out: &mut Vec<(Draft, Option<Change>)>,
    ) -> Result<()> {
        let now = self.at.now;
        let o = self.ontology;
        // Each set and trigger: its name, its type, its condition, and
        // whether it is a set.
        let sets = (o.object_sets.values())
            .map(|e| (&*e.decl.name, &e.decl.of, Some(&e.decl.filter), true));
        let triggers = (o.triggers.values()).filter(|e| fires_on(&e.decl, change));
        let triggers = triggers.map(|e| {
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
                for rec in back((self.at.view, &self.pending), &r.back, after.clone())? {
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
                true => self.at.view.members(name, of)?,
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
                            settings: Some(self.at.settings),
                            now,
                        };
                        let is = !r.retired
                            && self.with_heads(|h| {
                                holds((h, o, on), &r, &e, scope, (self.at.settings, false))
                            });
                        if was == is {
                            continue;
                        }
                        let change = (at.0, at.1, at.2, is);
                        Some((self.call.changed(change.clone(), self.next, now), change))
                    }
                    false => {
                        let t = &o.triggers[name].decl;
                        let strict = (&e, t.when_strict);
                        if r.retired || !self.holds_at_write(strict, on, Some(&r), &r)? {
                            continue;
                        }
                        // The write that set it off made another record.
                        let body = self.firing(t, (&r.ty, &r.key), (change, false));
                        let fired = Draft {
                            cause: Some(self.next),
                            context: self.call.context.clone(),
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
                    return Err(Error::refused(field, reason));
                }
            }
        }
        Ok(())
    }
}

/// The live records the steps `back` lead to from `at`, each as
/// `written` leaves it: one scan of a type a step, over the records it
/// reached, and the records of that type `written` wrote.
pub(crate) fn back(
    (view, written): (&dyn ProjectionView, &dyn Written),
    back: &[Back<'_>],
    at: StoredRecord,
) -> Result<Vec<StoredRecord>> {
    let mut at = vec![at];
    for step in back {
        let (ty, asked): (&TypeName, Vec<Vec<(FieldName, Value)>>) = match step {
            Back::From(on) => {
                let from =
                    |r: &StoredRecord| r.fields.get(links::FROM)?.as_str().map(RecordKey::from);
                let keys: BTreeSet<RecordKey> = at.iter().filter_map(from).collect();
                let read = keys.iter().map(|k| written.record(view, on, k));
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
        let mut next: BTreeMap<RecordKey, StoredRecord> = (written.scan(view, ty, &asked)?)
            .into_iter()
            .filter(|r| holds(r))
            .map(|r| (r.key.clone(), r))
            .collect();
        for r in written.wrote(ty) {
            match holds(r) && !r.retired && !r.is_ended() {
                true => next.insert(r.key.clone(), r.clone()),
                false => next.remove(&r.key),
            };
        }
        at = next.into_values().collect();
    }
    Ok(at)
}

/// The key of the one live link `from` of `ty` holds over `r`, as
/// `written` leaves it: one it wrote, or the projection's.
fn link(
    view: &dyn ProjectionView,
    written: &dyn Written,
    ty: &TypeName,
    r: &RelationName,
    from: &RecordKey,
) -> Result<Option<RecordKey>> {
    let lt = links::link_type_name(ty, r);
    let from_it = |l: &StoredRecord| {
        !l.retired
            && !l.is_ended()
            && l.fields.get(links::FROM).and_then(Value::as_str) == Some(from.as_str())
    };
    match written.wrote(&lt).find(|l| from_it(l)) {
        Some(l) => Ok(Some(l.key.clone())),
        None => link_from(view, ty, r, from),
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

/// Whether `t` fires on a change of the kind `change`: every kind fires a
/// trigger that names none.
pub(super) fn fires_on(t: &TriggerDecl, change: ChangeKind) -> bool {
    t.on_changes.as_ref().is_none_or(|on| on.contains(&change))
}

/// The record a reference names, as one reader of it reaches it.
type Far<'a> = &'a dyn Fn(&TypeName, &RecordKey) -> Result<Option<StoredRecord>>;

/// The link the call's record holds under a relation ([`link_from`]).
type Link<'a> = &'a dyn Fn(&RelationName) -> Result<Option<RecordKey>>;

/// The key of the one live link `from` of `ty` holds over the relation
/// `r` in `view`, whoever is answered it: a relation a path reads holds one
/// at most from a record (055, extended). A verb is decided before what it
/// sets off, so no draft of its decision is a link.
pub(super) fn link_from(
    view: &dyn ProjectionView,
    ty: &TypeName,
    r: &RelationName,
    from: &RecordKey,
) -> Result<Option<RecordKey>> {
    use lakelands_api::query::Walk;
    let edges = view.edges(ty, std::slice::from_ref(from), Some(r), Walk::Out)?;
    let mut keys = edges.into_iter().flatten();
    Ok(keys.next().map(|e| links::link_key(from, &e.to.1)))
}

/// What a verb's `requires` and values read across a reference: a path
/// steps through a reference field to the one record it names and reads a
/// field there ([`TypeDecl::stepped`]), each record read once a call, as
/// `far` reaches it, and only where the evaluation comes to the path.
pub(super) struct Across<'a> {
    o: &'a Ontology,
    on: &'a TypeDecl,
    far: Far<'a>,
    link: Link<'a>,
    /// The record at a key, whoever is answered it.
    stored: Far<'a>,
    /// The most steps a path may take: the lake's `max_depth` at a call.
    max: u32,
    /// The settings and the clock a derived relation's `where` reads: the
    /// call's.
    clock: (&'a BTreeMap<String, Value>, Timestamp),
    /// The records read, none where the reference reached none.
    pub(super) read: RefCell<BTreeMap<(TypeName, RecordKey), Option<StoredRecord>>>,
    /// The refusal of the first path that reached no record.
    refusal: RefCell<Option<Error>>,
}

impl<'a> Across<'a> {
    pub(super) fn new(
        o: &'a Ontology,
        on: &'a TypeDecl,
        (far, link, stored): (Far<'a>, Link<'a>, Far<'a>),
        (max, clock): (u32, (&'a BTreeMap<String, Value>, Timestamp)),
    ) -> Self {
        let (read, refusal) = (RefCell::default(), RefCell::default());
        Across {
            o,
            on,
            far,
            link,
            stored,
            max,
            clock,
            read,
            refusal,
        }
    }

    /// The record `named`, read once a call as `far` reaches it.
    fn reach(&self, named: &(TypeName, RecordKey)) -> Option<StoredRecord> {
        let known = self.read.borrow().get(named).cloned();
        let far = || {
            (self.far)(&named.0, &named.1)
                .map_err(|e| self.refuse(e))
                .ok()
        };
        let found = known.unwrap_or_else(|| far().flatten());
        self.read.borrow_mut().insert(named.clone(), found.clone());
        found
    }

    /// Note the call's first refusal.
    fn refuse(&self, e: Error) {
        self.refusal.borrow_mut().get_or_insert(e);
    }

    /// The one record the derived relation `r` reaches from a record
    /// holding `fields`: its target's record at the key the match names,
    /// where it holds the matched values as stored and the relation's
    /// `where` holds over the two with the call's clock (044, extended).
    /// None reached is no edge, never a refusal: no reference names it.
    fn matched(
        &self,
        fields: &BTreeMap<FieldName, Value>,
        r: &RelationDecl,
    ) -> Option<StoredRecord> {
        let d = r.derived.as_ref()?;
        let target = self.o.ty(&r.target).ok()?;
        let value = |p: &lakelands_api::ontology::MatchPair| {
            Some((
                p.to.clone(),
                fields.get(&p.from).filter(|v| !v.is_null())?.clone(),
            ))
        };
        let values: BTreeMap<FieldName, Value> =
            d.pairs.iter().map(value).collect::<Option<_>>()?;
        let key = lakelands_api::key::record_key(target, &values)?;
        let named = (r.target.clone(), key);
        let Some(far) = self.reach(&named) else {
            // One there and not answered is held from the judged principal,
            // which a hold placed since a proposal can make so: never no
            // match, which would let a hold grant `not exists` (055).
            let stored = (self.stored)(&named.0, &named.1).ok().flatten();
            if stored.is_some_and(|r| !r.retired) {
                self.refuse(Error::not_granted());
            }
            return None;
        };
        // Every pair, two onto one field included, holds as stored.
        let holds =
            |p: &lakelands_api::ontology::MatchPair| far.fields.get(&p.to) == fields.get(&p.from);
        if !d.pairs.iter().all(holds) {
            return None;
        }
        let Some(c) = &d.filter else { return Some(far) };
        let (settings, now) = self.clock;
        let e = declare::condition(c, Eval::Record, &[]).ok()?;
        let ends = crate::reads::Ends {
            ends: [fields, &far.fields],
            settings,
            now,
        };
        expr::holds(&e, &ends).unwrap_or(false).then_some(far)
    }

    /// Whether the path that begins at `relation` reaches a record: the one
    /// a derived relation matches, or the one link the record holds under a
    /// relation carried by links.
    fn exists(&self, fields: &BTreeMap<FieldName, Value>, relation: &FieldName) -> bool {
        // The match or the link is a step toward `max_depth`.
        if self.max == 0 {
            self.refuse(Error::invalid(Case::StepTooDeep { max: 0, asked: 1 }));
            return false;
        }
        if let Some(r) = self.on.derived_step(relation) {
            return self.matched(fields, r).is_some();
        }
        let relation = RelationName::from(relation.as_str());
        let lt = links::link_type_name(&self.on.name, &relation);
        let Some(link) = (self.link)(&relation)
            .map_err(|e| self.refuse(e))
            .ok()
            .flatten()
        else {
            return false;
        };
        // Its caller holds nothing of the link from it (055).
        let found = self.reach(&(lt, link)).is_some();
        if !found {
            self.refuse(Error::not_granted());
        }
        found
    }

    /// The value at `path` from a record of the verb's type holding
    /// `fields`. A reference that is null reaches nothing and the path is
    /// null; one that names a record not reached is noted
    /// ([`Self::reached`]). A path that begins at a relation reads the one
    /// link the record holds under it, none reading null.
    fn value(&self, fields: &BTreeMap<FieldName, Value>, path: &[FieldName]) -> Option<Value> {
        let ty = |n: &TypeName| self.o.ty(n).ok();
        // A path that begins at a derived relation goes on from the record
        // it matches; the match is a step.
        let derived = self.on.derived_step(&path[0]).filter(|_| path.len() > 1);
        let (from, path) = match derived {
            Some(r) => (ty(&r.target)?, &path[1..]),
            None => (self.on, path),
        };
        let steps = from.stepped(path, &ty);
        let asked = steps.len().saturating_sub(1) as u32 + u32::from(derived.is_some());
        let refuse = |e: Error| self.refuse(e);
        if asked > self.max {
            refuse(Error::invalid(Case::StepTooDeep {
                max: self.max,
                asked,
            }));
            return None;
        }
        let reach = |named: &(TypeName, RecordKey)| self.reach(named);
        let mut at: Option<StoredRecord> = match derived {
            Some(r) => Some(self.matched(fields, r)?),
            None => None,
        };
        let by_link = steps
            .first()
            .filter(|s| derived.is_none() && s.0.name != self.on.name);
        if let Some((lt, _)) = by_link {
            let relation = RelationName::from(path[0].as_str());
            let link = (self.link)(&relation).map_err(refuse).ok()??;
            // Its caller holds nothing of the link from it (055): one not
            // reached anyway is refused as the verb not its, naming nothing.
            let found = reach(&(lt.name.clone(), link));
            if found.is_none() {
                refuse(Error::not_granted());
            }
            at = Some(found?);
        }
        for step in steps[usize::from(by_link.is_some())..].windows(2) {
            let (via, to) = (&step[0].1.name, &step[1].0.name);
            let held = at.as_ref().map_or(fields, |r| &r.fields);
            let named = (to.clone(), RecordKey::from(held.get(via)?.as_str()?));
            let found = reach(&named);
            if found.is_none() {
                let (field, ty, key) = (via.clone(), to.clone(), named.1.clone());
                refuse(Error::constraint(
                    Some(via.clone()),
                    Case::NotLive { field, ty, key },
                ));
            }
            at = Some(found?);
        }
        let rest = &path[steps.len().saturating_sub(1)..];
        expr::eval::lookup(at.as_ref().map_or(fields, |r| &r.fields), rest)
    }

    /// Refuse a call that read across a reference to no record: as a
    /// write that sends that reference is refused (032), whatever else
    /// the condition would answer.
    pub(super) fn reached(&self) -> Result<()> {
        self.refusal.borrow_mut().take().map_or(Ok(()), Err)
    }
}

/// The scope a verb's conditions and assignments see: the record's fields
/// and their provenance, what a path reads across a reference, and
/// `payload.<name>` for the call's payload. A set's `where` and a
/// trigger's `when` see it over their own scope, `record`, with no payload
/// (055, extended).
pub(super) struct VerbScope<'a, S = RecordScope<'a>> {
    pub(super) record: S,
    pub(super) fields: &'a BTreeMap<FieldName, Value>,
    pub(super) payload: Option<&'a Patch>,
    pub(super) across: &'a Across<'a>,
}

impl<S: Scope> Scope for VerbScope<'_, S> {
    fn value(&self, path: &[FieldName]) -> Option<Value> {
        let member = lakelands_core::derive::payload_member(path);
        match self.payload.zip(member) {
            Some((payload, name)) => payload.get(name).filter(|v| !v.is_null()).cloned(),
            None => self.across.value(self.fields, path),
        }
    }

    fn previous(&self, field: &FieldName) -> Option<Value> {
        self.record.previous(field)
    }

    fn preceding(&self, series: &[FieldName], field: &FieldName) -> Option<Value> {
        self.record.preceding(series, field)
    }

    fn in_write(&self) -> bool {
        self.record.in_write()
    }

    fn retiring(&self) -> bool {
        self.record.retiring()
    }

    fn creating(&self) -> bool {
        self.record.creating()
    }

    fn meta(&self, field: &FieldName) -> Option<lakelands_api::FieldMeta> {
        self.record.meta(field)
    }

    fn setting(&self, key: &str) -> Option<Value> {
        self.record.setting(key)
    }

    fn now(&self) -> Timestamp {
        self.record.now()
    }

    fn exists(&self, relation: &FieldName) -> Option<bool> {
        Some(self.across.exists(self.fields, relation))
    }
}
