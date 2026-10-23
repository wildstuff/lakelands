//! The record doors: `upsert`, `retire`, `link`, `unlink` and `call`, and
//! the decision each write is made in.

use super::*;
use crate::record;

/// What a decision's own events carry beside the call's writer and
/// context, set before the record they make is computed, so that what is
/// judged on that record (a trigger's guard) reads the provenance the fold
/// keeps: aging writes as the system, with its rule as the procedure,
/// ingest as its source states each record, an accept as the proposer and
/// caused by the acceptance, and a verb's write caused by its call.
#[derive(Clone)]
pub(crate) struct Envelope {
    /// The authority that writes them.
    pub origin: Origin,
    /// Who writes them, where it is not the caller.
    pub writer: Option<String>,
    /// The event they follow from.
    pub cause: Option<Position>,
    /// The procedure that produced their values.
    pub procedure: Option<Procedure>,
    /// A chosen id.
    pub id: Option<EventId>,
    /// The source they come from.
    pub source: Option<lakelands_api::SourceId>,
    /// When the source observed the values.
    pub seen_at: Option<Timestamp>,
    /// Where the source read them.
    pub upstream: Option<lakelands_api::Upstream>,
    /// What the source's key seals of a `record_ingested`'s patch.
    pub sealed: Option<lakelands_api::event::Sealed>,
}

impl Envelope {
    /// That of events `origin` writes, carrying nothing more.
    pub(crate) fn of(origin: Origin) -> Envelope {
        Envelope {
            origin,
            writer: None,
            cause: None,
            procedure: None,
            id: None,
            source: None,
            seen_at: None,
            upstream: None,
            sealed: None,
        }
    }
}

/// What one write is decided in, from the writer's lock to its append: the
/// writer and the view it reads, what its drafts so far leave, its time,
/// where its next event lands, who is judged at which door, and the
/// envelope of its own events.
pub(crate) struct Decide<'a> {
    pub(super) call: &'a Call<'a>,
    pub(super) w: &'a Writer,
    pub(super) view: &'a dyn ProjectionView,
    /// The records and memberships its drafts change, not in the view yet.
    pub pending: Pending,
    pub(super) now: Timestamp,
    /// Where the event being decided lands; [`Self::reactions`] moves it
    /// past that event and what it sets off.
    pub(super) next: Position,
    /// Who is judged: the caller, or at an accept the proposer.
    pub who: &'a Principal,
    /// The door its records are written through.
    pub door: Door,
    /// What its own events carry.
    pub envelope: Envelope,
    /// Whether what its far writes set off is held to `max_set_changes`:
    /// a purge's preflight counts it instead.
    pub bound: bool,
    /// What kind of change its retirements are to a trigger: a forget's
    /// and a purge's say so; `retired` otherwise.
    pub retiring_as: ChangeKind,
}

impl Decide<'_> {
    /// One of the decision's own events.
    pub(crate) fn draft(&self, body: EventBody) -> Draft {
        let e = self.envelope.clone();
        let ingested = matches!(body, EventBody::RecordIngested { .. });
        let own = self.call.draft(self.now, e.origin, body);
        Draft {
            sealed: e.sealed.filter(|_| ingested),
            writer: e.writer.unwrap_or(own.writer),
            cause: e.cause,
            procedure: e.procedure,
            id: e.id,
            source: e.source,
            seen_at: e.seen_at,
            upstream: e.upstream,
            ..own
        }
    }

    /// The record of `decl` at `key`, as the drafts so far leave it, for
    /// the judged principal.
    pub(crate) fn target(&self, decl: &TypeDecl, key: &RecordKey) -> Result<Target> {
        let (view, pending, who) = (self.view, &self.pending, self.who);
        let before = pending.record(view, &decl.name, key)?;
        let mine = match &before {
            Some(b) => {
                let sees = Sight::held(who, &self.w.ontology);
                sees.answers(&decl.name, b) && ends(view, pending, &sees, decl, &b.fields)?
            }
            None => false,
        };
        Ok(Target { before, mine })
    }

    /// The live record the judged principal reaches at `key`, at the
    /// version `expected`, compared before anything else is judged on the
    /// record (024): what a retire takes and a verb runs on. A retired one
    /// is not found, and to a holder of `only` another's is not found, as a
    /// missing one is (051); so is a restricted one, or a link with a
    /// restricted end, to a writer without the grant (006), and a setting
    /// it is not told (063).
    fn reached(
        &self,
        decl: &TypeDecl,
        key: &RecordKey,
        expected: Option<Version>,
    ) -> Result<StoredRecord> {
        let t = self.target(decl, key)?;
        let told = |b: &StoredRecord| Sight::held(self.who, &self.w.ontology).told(b).is_some();
        match t.before {
            Some(b) if t.mine && !b.retired && told(&b) => {
                cas(expected, Some(&b))?;
                Ok(b)
            }
            _ => Err(Error::NotFound {
                missing: Missing::Record {
                    ty: decl.name.clone(),
                    key: key.clone(),
                },
            }),
        }
    }

    /// A retirement of `ty` at `key`. It answers in its own order: whether
    /// the judged principal may retire the type, then whether this door
    /// retires it (lakelands#1148), before whether the record is there,
    /// since a key it does not reach leaves nothing it could make.
    fn retire(
        &mut self,
        ty: &TypeName,
        key: &RecordKey,
        expected: Option<Version>,
    ) -> Result<Plan> {
        let decl = self.w.ontology.ty(ty)?;
        rights::not_a_view(decl)?;
        granted(rights::may_retire(self.who, decl))?;
        if !self.door.admits_retirement(decl.retired_by) {
            return Err(self.retired_by_verbs(decl));
        }
        untold_setting(&Sight::held(self.who, &self.w.ontology), decl, key)?;
        let before = self.reached(decl, key, expected)?;
        let own = self.draft(retirement(ty, key, before.version));
        let (drafts, after) = (self.retiring(&before, own)?, None);
        Ok(Plan { drafts, after })
    }

    /// What `op` names, as the judged principal is shown it (006 and 024,
    /// extended): one not shown its type is answered as for a type that is
    /// not there, and one not shown the relation a link door names as for
    /// a relation the type does not carry. A verb is shown by its door.
    fn sight(&self, op: Op<'_>) -> Result<()> {
        let (ty, relation) = match op {
            Op::Upsert(ty, ..) | Op::Retire(ty, ..) => (ty, None),
            Op::Link((ty, _, r, _), _) | Op::Unlink((ty, _, r, _)) => (ty, Some(r)),
            Op::Call(..) => return Ok(()),
        };
        let sees = Sight::held(self.who, &self.w.ontology);
        let decl = sees.ty(ty)?;
        let mut named = decl.relations.iter().filter(|r| Some(&r.name) == relation);
        match relation {
            Some(n) if named.any(|r| !sees.relation(decl, r)) => Err(no_link_relation(ty, n)),
            _ => Ok(()),
        }
    }

    /// A link door's own questions, with the link type and the relation
    /// they find: the relation is carried by link records, on a type that
    /// is no view; and for `link`, its links are authored and no end is
    /// among the fields. Another door's op has none.
    fn shape(&self, op: Op<'_>) -> Result<Option<(TypeName, RelationDecl)>> {
        let ((ty, _, relation, _), fields) = match op {
            Op::Link(ends, fields) => (ends, Some(fields)),
            Op::Unlink(ends) => (ends, None),
            _ => return Ok(None),
        };
        let decl = self.w.ontology.ty(ty)?;
        rights::not_a_view(decl)?;
        let mut carried = decl.relations.iter();
        let r = carried.find(|r| &r.name == relation && r.carried_by_links());
        let r = r.ok_or_else(|| no_link_relation(ty, relation))?;
        let lt = links::link_type_name(ty, relation);
        self.w.ontology.ty(&lt)?;
        if fields.is_some() && r.authority != Authority::Authored {
            return Err(Error::refused(None, Refusal::WrongDoor));
        }
        let ends = [links::FROM, links::TO].map(FieldName::from);
        let mut ends = ends
            .iter()
            .filter(|e| fields.is_some_and(|f| f.contains_key(*e)));
        if let Some(end) = ends.next() {
            return Err(Error::invalid_at(
                end,
                Case::LinkEndAsField { end: end.clone() },
            ));
        }
        Ok(Some((lt, r.clone())))
    }

    /// Decide `op` for the judged principal, its own event landing next,
    /// in the one order every door shares: what the principal is shown of
    /// what the op names ([`Self::sight`]); a link door's own questions
    /// ([`Self::shape`]); the record's own checks, by [`Self::record`],
    /// [`Self::retire`] or [`Self::run_verb`]; a link's cardinality, which
    /// reads other records, so the grants come first (024); then what the
    /// write sets off. `call`, `propose` and `accept` run it as the four
    /// record doors do, so what one refuses the others refuse in the same
    /// words.
    pub(crate) fn plan(&mut self, op: Op<'_>, expected: Option<Version>) -> Result<Plan> {
        self.sight(op)?;
        match (op, self.shape(op)?) {
            (Op::Upsert(ty, key, patch), _) => {
                let plan = self.record(ty, key, patch, expected, None)?;
                self.planned(plan)
            }
            (Op::Retire(ty, key), _) => self.retire(ty, key, expected),
            (Op::Call(verb, key, payload), _) => self.run_verb(verb, key, payload, expected),
            (Op::Link((ty, key, _, to), fields), Some((lt, r))) => {
                let mut patch = fields.clone();
                for (end, k) in [(links::FROM, key), (links::TO, to)] {
                    patch.insert(end.into(), Value::String(k.to_string()));
                }
                let plan = self.record(&lt, &links::link_key(key, to), &patch, expected, None)?;
                let sees = Sight::held(self.who, &self.w.ontology);
                cardinality((self.view, &sees), None, (ty, &r), key, to)?;
                self.planned(plan)
            }
            (Op::Unlink((_, key, _, to)), Some((lt, _))) => {
                self.retire(&lt, &links::link_key(key, to), expected)
            }
            (Op::Link(..) | Op::Unlink(..), None) => unreachable!("a link door has its relation"),
        }
    }

    /// A patch as the door takes it from the judged principal: the key's
    /// form, then the door and the grant of every field it carries, then
    /// its values, each in its written form.
    pub(crate) fn checked(&self, decl: &TypeDecl, key: &RecordKey, patch: &Patch) -> Result<Patch> {
        check_key(key)?;
        let door = (self.door, self.envelope.source.as_ref());
        rights::check_write(self.who, decl, door, patch).map_err(|e| self.verbs_only(decl, e))?;
        check_values(&self.w.ontology, decl, patch, &decl.fields)
    }

    /// A field only verbs write, refused here: named the verbs its writer is
    /// shown and may call, else `wrong_door` (lakelands#1130).
    fn verbs_only(&self, decl: &TypeDecl, e: Error) -> Error {
        let Error::Refused {
            field: Some(field),
            reason: Refusal::WrongDoor,
        } = &e
        else {
            return e;
        };
        let ours = |f: &FieldDecl| &f.name == field && f.authority == Authority::Verb;
        let sees = Sight::held(self.who, &self.w.ontology);
        let sets = |v: &VerbDecl| matches!(&v.effect, VerbEffect::Write { set } if set.iter().any(|a| &a.field == field));
        let verbs: Vec<VerbName> = (self.w.ontology.verbs.values().map(|e| &e.decl))
            .filter(|v| v.on == decl.name && sets(v) && sees.verb_shown(v))
            .map(|v| v.name.clone())
            .collect();
        match decl.fields.iter().any(ours) && !verbs.is_empty() {
            true => Error::refused(Some(field.clone()), Refusal::VerbsOnly { verbs }),
            false => e,
        }
    }

    /// A retirement of a type only its verbs retire, refused at another
    /// door: named the verbs that retire it which its writer is shown and
    /// may call, else `wrong_door`, whatever the key (lakelands#1148).
    fn retired_by_verbs(&self, decl: &TypeDecl) -> Error {
        let sees = Sight::held(self.who, &self.w.ontology);
        let verbs: Vec<VerbName> = (self.w.ontology.verbs.values().map(|e| &e.decl))
            .filter(|v| v.on == decl.name && matches!(v.effect, VerbEffect::Retire))
            .filter(|v| sees.verb_shown(v))
            .map(|v| v.name.clone())
            .collect();
        match verbs.is_empty() {
            true => Error::refused(None, Refusal::WrongDoor),
            false => Error::refused(None, Refusal::VerbsOnly { verbs }),
        }
    }

    /// Decide one authored record write (`upsert`, a verb, aging): its
    /// event, which lands next, and the record it makes. `reached` is the
    /// record a verb read at the key already ([`Self::reached`]).
    pub(crate) fn record(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        patch: &Patch,
        expected: Option<Version>,
        reached: Option<StoredRecord>,
    ) -> Result<RecordPlan> {
        let decl = self.w.ontology.ty(ty)?;
        if patch.is_empty() {
            return Err(Error::invalid(Case::PatchEmpty));
        }
        let patch = self.checked(decl, key, patch)?;
        let t = match reached {
            Some(r) => Target {
                before: Some(r),
                mine: true,
            },
            None => self.target(decl, key)?,
        };
        self.write(decl, key, t, patch, expected)
    }

    /// Each constraint of `decl` that the write moves holds on `after`
    /// (lakelands#1119). Whether the write is judged is told by what it
    /// creates and sends, and the derived fields those feed, never by a
    /// value: one who does not read every field a judged constraint reads
    /// is not granted the write, whether it would hold or not. A source's
    /// window does not judge a constraint that admits at ingest.
    fn constraints_hold(
        &self,
        decl: &TypeDecl,
        live: Option<&StoredRecord>,
        after: &StoredRecord,
        sent: &[FieldName],
    ) -> Result<()> {
        let (w, who) = (self.w, self.who);
        let plan = w.plans.get(&w.ontology, decl);
        let reads = |f: &FieldName| {
            let mut fields = decl.fields.iter().filter(|d| &d.name == f);
            fields.any(|d| rights::field_access(who, decl, d).reads())
        };
        for c in &plan.constraints {
            let admits = matches!(self.door, Door::Source) && !c.decl.at_ingest.refuses();
            if admits || (live.is_some() && !sent.iter().any(|f| c.fed.contains(f))) {
                continue;
            }
            granted(c.reads.iter().all(reads))?;
            let before = live.map(|b| &b.fields);
            let moved = |f: &FieldName| before.and_then(|b| b.get(f)) != after.fields.get(f);
            if before.is_some() && !c.reads.iter().any(moved) {
                continue;
            }
            let scope = WriteScope {
                after: &after.fields,
                before,
                retiring: false,
                meta: Some(&after.meta),
                settings: Some(&w.settings),
                now: self.now,
            };
            if !expr::holds(&c.holds, &scope).unwrap_or(false) {
                let (constraint, ty) = (c.decl.name.clone(), decl.name.clone());
                let key = after.key.clone();
                let case = Case::ConstraintBroken {
                    constraint,
                    ty,
                    key,
                };
                return Err(Error::constraint(None, case));
            }
        }
        Ok(())
    }

    /// The rest of a record write, authored or from a source (`ingest`),
    /// once its patch is checked and its target read.
    pub(crate) fn write(
        &self,
        decl: &TypeDecl,
        key: &RecordKey,
        t: Target,
        mut patch: Patch,
        expected: Option<Version>,
    ) -> Result<RecordPlan> {
        let (w, view, who) = (self.w, self.view, self.who);
        let sees = Sight::held(who, &w.ontology);
        untold_setting(&sees, decl, key)?;
        // A holder of `only` is judged against the record it owns at the
        // key: another's is, to its write, a key that is free, refused
        // only once every check has passed, so no probe tells them apart
        // (051).
        let live = t.live();
        // A retired record the writer is not answered is another's too: its
        // hold is not written into (006).
        let other = live.is_none()
            && t.before
                .as_ref()
                .is_some_and(|b| !b.retired || sees.withheld(b));
        if live.is_none() {
            let door = (self.door, self.envelope.source.as_ref());
            fill_key_fields(w, decl, key, &mut patch, door, who)?;
        }
        if decl.name.as_str() == SETTING_TYPE {
            let told = |s: &SettingDecl| sees.setting_visible(s);
            let before = live.map(|b| &b.fields);
            stamped(&w.ontology, decl, key, before, &told, &mut patch)?;
        }
        // A setting is judged as it was written, before its version is: one
        // of a type it is not shown is answered as one never declared (063).
        let declared = |t: &TypeName| w.ontology.ty(t).ok();
        let shown = |t: &TypeName| sees.describe_type(t);
        let judged_early = sees.narrows(&decl.name);
        if judged_early {
            let value = patch.get("value");
            lakelands_api::seam::check_setting(key.as_str(), value, &declared, &shown)?;
        }
        check_key_agrees(decl, key, live, &patch)?;
        // An authored write of a link has both ends live and answered
        // (035). `link` sends them; a verb's patch holds only what it sets,
        // so the ends the record holds are judged beside it. A source's
        // window is judged by what it sends (016).
        let end = |f: &FieldName| [links::FROM, links::TO].contains(&f.as_str());
        let authored = !matches!(self.door, Door::Source) && links::is_link_type(&decl.name);
        let held = live.filter(|_| authored);
        let held = held.into_iter().flat_map(|b| &b.fields);
        let ends: Patch = (held.filter(|(f, _)| end(f) && !patch.contains_key(*f)))
            .map(|(f, v)| (f.clone(), v.clone()))
            .collect();
        for judged in [&patch, &ends] {
            let (pending, o) = (&self.pending, &w.ontology);
            check_references(view, pending, o, &decl.name, key, judged, who)?;
        }
        cas(expected, t.own())?;
        let sent: Vec<FieldName> = patch.keys().cloned().collect();
        let version = Version(t.before.as_ref().map_or(0, |b| b.version.0) + 1);
        // Said once, here, of what this writer holds, its own drafts too.
        let made = live.is_none();
        let (ty, key) = (decl.name.clone(), key.clone());
        let draft = self.draft(match self.door {
            Door::Authored | Door::Verb => EventBody::RecordUpserted {
                ty,
                key,
                version,
                patch,
                made,
            },
            Door::Source => EventBody::RecordIngested {
                ty,
                key,
                version,
                patch,
                made,
            },
        });
        let before = t.before.as_ref().filter(|_| !other);
        let after = self.apply(before, &draft)?;
        record::check_required(decl, &after.fields)?;
        self.constraints_hold(decl, live.filter(|_| !other), &after, &sent)?;
        if after.ty.as_str() == AGING_RULE_TYPE {
            let rule = crate::aging::check(&w.ontology, &after.fields)
                .map_err(|(field, said)| Error::invalid_at(&field, *said))?;
            crate::aging::check_authority(who, &w.ontology, &rule)?;
        }
        if after.ty.as_str() == SETTING_TYPE && !judged_early {
            let value = after.fields.get("value");
            lakelands_api::seam::check_setting(after.key.as_str(), value, &declared, &shown)?;
        }
        // A reference the record computes is judged as a written one, from
        // the record as planned (032). Its key is computed from fields its
        // writer may not all read, so one who may not read the reference
        // is told only that the write is not granted. An accept judges
        // as one who reads every field; what its reviewer is told is
        // `told`'s.
        let computed = computed_references(decl, live.map(|b| &b.fields), &after.fields, &sent);
        let reads = |f: &FieldName| {
            let mut fields = decl.fields.iter().filter(|d| &d.name == f);
            fields.any(|d| rights::field_access(who, decl, d).reads())
        };
        let (pending, o) = (&self.pending, &w.ontology);
        match check_references(view, pending, o, &decl.name, &after.key, &computed, who) {
            Err(Error::Refused { field: Some(f), .. }) if !reads(&f) => granted(false)?,
            judged => judged?,
        }
        granted(!other && owns(view, &self.pending, &sees, decl, &after.fields)?)?;
        Ok(RecordPlan {
            draft,
            after,
            before: t.before,
        })
    }

    /// The record `d`, which lands next, makes of `before`: computed with
    /// the fold's own function, which hands out no call here.
    fn apply(&self, before: Option<&StoredRecord>, d: &Draft) -> Result<StoredRecord> {
        let (w, event) = (self.w, provisional(self.call.lake, self.next, d));
        // Its own keys sealed apart are read from its event, so only a
        // version's first use is staged (065).
        let mut kept = self.kept(d)?;
        kept.retain(|(at, _)| at.0 == crate::sealing::USED);
        let staged = match kept.is_empty() {
            false => {
                let mut staged = self.pending.sealed.clone();
                for (at, list) in kept {
                    staged.entry(at).or_default().push((self.next, list));
                }
                std::borrow::Cow::Owned(staged)
            }
            true => std::borrow::Cow::Borrowed(&self.pending.sealed),
        };
        let outputs = &mut Outputs {
            staged: Some(&staged),
            ..Outputs::planning(self.view, &w.plans, &self.call.lake.keys)
        };
        let after = record::apply(&w.ontology, before, &event, &w.settings, outputs)?;
        Ok(after.expect("a record event makes a record"))
    }

    /// The triggers and object-set changes a record change of the kind
    /// `change` causes, the event that makes it landing next: its own,
    /// then what it sets off across an edge ([`Self::across`]). A trigger
    /// whose `on_changes` leaves `change` out fires nothing. A refused
    /// write leaves the append's drafts as they were.
    pub(crate) fn reactions(
        &mut self,
        change: ChangeKind,
        before: Option<&StoredRecord>,
        after: &StoredRecord,
    ) -> Result<Vec<Draft>> {
        let (w, now, cause, context) = (self.w, self.now, self.next, self.call.context);
        let mut out = Vec::new();
        // A reaction carries the context of the write that caused it, so
        // a host acting on it writes one level deeper (021).
        let system = |body| Draft {
            cause: Some(cause),
            context,
            ..Draft::system(now, body)
        };
        let at = (after.ty.clone(), after.key.clone());
        let prior = self.pending.records.insert(at.clone(), after.clone());
        let mut own = BTreeMap::new();
        let mut far = Vec::new();
        let decided = (|| {
            let on = |t: &&TriggerDecl| t.on == after.ty && fires_on(t, change);
            let created = before.is_none_or(|b| b.retired);
            for t in w.ontology.triggers.values().map(|e| &e.decl).filter(on) {
                if self.fires(t, before, after)? {
                    let at = (&after.ty, &after.key);
                    out.push(system(self.firing(t, at, (change, created))));
                }
            }
            let of = |s: &&ObjectSetDecl| s.of == after.ty;
            for s in w.ontology.object_sets.values().map(|e| &e.decl).filter(of) {
                let was = self
                    .pending
                    .member(self.view, &s.name, &after.ty, &after.key)?;
                let is = self.member(s, after, &w.settings)?;
                if was != is {
                    own.insert((s.name.clone(), at.0.clone(), at.1.clone()), is);
                    let change = (s.name.clone(), at.0.clone(), at.1.clone(), is);
                    out.push(self.call.changed(change, cause, now));
                }
            }
            self.across((before, after), (change, &own), &mut far)?;
            self.viewed(change, (before, after), &mut far)
        })();
        if let Err(e) = decided {
            match prior {
                Some(p) => self.pending.records.insert(at, p),
                None => self.pending.records.remove(&at),
            };
            return Err(e);
        }
        self.pending.members.extend(own);
        self.pending.across += far.len();
        for (draft, change) in far {
            if let Some((set, ty, key, is)) = change {
                self.pending.members.insert((set, ty, key), is);
            }
            out.push(draft);
        }
        self.next = Position(cause.0 + 1 + out.len() as u64);
        Ok(out)
    }

    /// The drafts of a retirement: `d`, which retires `before` and lands
    /// next, then the triggers and object-set changes it causes, judged
    /// on the record its provisional event makes, then the closing of its
    /// calls' pending proposals (064).
    pub(crate) fn retiring(&mut self, before: &StoredRecord, d: Draft) -> Result<Vec<Draft>> {
        let (at, now) = (self.next, self.now);
        let after = self.apply(Some(before), &d)?;
        let mut drafts = vec![d];
        drafts.extend(self.reactions(self.retiring_as, Some(before), &after)?);
        let calls = match &mut self.pending.calls {
            Some(calls) => calls,
            none => none.insert(crate::backed::pending_calls(self.view)?),
        };
        let mine = calls.remove(&(before.ty.clone(), before.key.clone()));
        let why = "its record is retired";
        let closed: Vec<Draft> = (mine.unwrap_or_default().into_iter())
            .map(|id| crate::backed::closing(id, why, (now, at)))
            .collect();
        self.next = Position(self.next.0 + closed.len() as u64);
        drafts.extend(closed);
        Ok(drafts)
    }

    /// What the fold keeps once `d`, landing next, seals: the list of the
    /// version it uses first, if it is the first use of that version of
    /// its source's key, and its keys sealed apart ([`crate::sealing::kept`]).
    fn kept(&self, d: &Draft) -> Result<Vec<((String, Digest), Value)>> {
        let ingested = matches!(d.body, EventBody::RecordIngested { .. });
        let (Some(sealed), Some(source), true) = (&d.sealed, d.source.as_ref(), ingested) else {
            return Ok(Vec::new());
        };
        let used = (source, sealed, self.next, self.now);
        let read = (&self.pending.sealed, self.view, &*self.call.lake.keys);
        crate::sealing::kept(read, used)
    }

    /// A planned record write with what it sets off: its own event, which
    /// lands next, then the triggers and object-set changes it causes.
    pub(crate) fn planned(&mut self, plan: RecordPlan) -> Result<Plan> {
        for (at, list) in self.kept(&plan.draft)? {
            self.pending
                .sealed
                .entry(at)
                .or_default()
                .push((self.next, list));
        }
        let change = match plan.draft.body {
            EventBody::RecordIngested { .. } => ChangeKind::Ingested,
            EventBody::RecordRetired { .. } => self.retiring_as,
            _ => ChangeKind::Upserted,
        };
        let mut drafts = self.reactions(change, plan.before.as_ref(), &plan.after)?;
        drafts.insert(0, plan.draft);
        let after = Some(plan.after);
        Ok(Plan { drafts, after })
    }

    /// The events a verb's call appends when it runs: `verb_called`, and
    /// for a write effect the record write it causes, for a retire effect
    /// the retirement, then the triggers and object-set changes that
    /// write causes, as for any other write;
    /// and the record that write leaves. `key` is `None` for a verb on its
    /// type, which reads no record and writes none. The record is read as
    /// the judged principal's write reaches it (051). The write is caused
    /// by the call and carries the verb as its procedure before its record
    /// is computed.
    fn run_verb(
        &mut self,
        verb: &VerbDecl,
        key: Option<&RecordKey>,
        payload: &Patch,
        expected: Option<Version>,
    ) -> Result<Plan> {
        let (w, now) = (self.w, self.now);
        let called = self.draft(EventBody::VerbCalled {
            verb: verb.name.clone(),
            key: key.cloned(),
            payload: payload.clone(),
        });
        let mut plan = Plan {
            drafts: vec![called],
            after: None,
        };
        // A verb on its type is external and declares no `requires`: it
        // reads no record and writes none.
        let Some(key) = key else {
            return Ok(plan);
        };
        let (o, on) = (&w.ontology, w.ontology.ty(&verb.on)?);
        let record = self.reached(on, key, expected)?;
        // A record read across a reference is reached where a write that
        // sends the reference would be accepted, and by nothing else.
        let (view, pending, who) = (self.view, &self.pending, self.who);
        let far = |ty: &TypeName, k: &RecordKey| live_for(view, pending, o, ty, k, who);
        let link = |r: &RelationName| link_from(view, &verb.on, r, key);
        let limits = (self.call.lake.limits.max_depth, (&w.settings, now));
        let stored = |ty: &TypeName, k: &RecordKey| pending.record(view, ty, k);
        let across = Across::new(o, on, (&far, &link, &stored), limits);
        // A part is named as the caller is shown what it reads: at an
        // accept, the reviewer.
        let told = |k: &str| {
            let (value, declared) = (w.settings.get(k), o.settings.get(k).map(|e| &e.decl));
            Sight::held(self.call.who, o).setting_shown(k, value, declared)
        };
        requires(&w.settings, verb, key, &record, now, &across, &told)?;
        // A verb on a link finds both its ends live and answered, whatever
        // its effect (035): one that writes the link has them judged where
        // the record is written, any other here, as `link` judges them.
        if links::is_link_type(&verb.on) && !matches!(verb.effect, VerbEffect::Write { .. }) {
            let end = |f: &&FieldName| [links::FROM, links::TO].contains(&f.as_str());
            let ends: Patch = (record.fields.iter().filter(|(f, _)| end(f)))
                .map(|(f, v)| (f.clone(), v.clone()))
                .collect();
            check_references(view, pending, o, &verb.on, key, &ends, who)?;
        }
        let patch = assigned(o, &w.settings, verb, &record, payload, (now, &across))?;
        let called_at = self.next;
        self.next = called_at.next();
        if matches!(verb.effect, VerbEffect::Write { .. } | VerbEffect::Retire) {
            let version = w.ontology.verbs.get(&verb.name).map_or(1, |e| e.version);
            let procedure = Some(Procedure {
                name: verb.name.to_string(),
                version: version.to_string(),
            });
            let of_the_write = Envelope {
                cause: Some(called_at),
                procedure,
                ..self.envelope.clone()
            };
            let of_the_call = std::mem::replace(&mut self.envelope, of_the_write);
            // Through the verbs' own door: a field only its verbs write is
            // written here and refused at `upsert` (lakelands#1130), and a
            // type only its verbs retire is retired here and refused at
            // `retire`, by the caller as `retire` judges it (lakelands#1148).
            let door = std::mem::replace(&mut self.door, Door::Verb);
            let write = match &verb.effect {
                VerbEffect::Retire => self.retire(&verb.on, key, expected).map(Ok),
                _ => (self.record(&verb.on, key, &patch, expected, Some(record))).map(Err),
            };
            (self.envelope, self.door) = (of_the_call, door);
            let write = match write? {
                Ok(retired) => retired,
                Err(written) => self.planned(written)?,
            };
            plan.drafts.extend(write.drafts);
            plan.after = write.after;
        }
        Ok(plan)
    }
}

impl Call<'_> {
    /// The context a write of this call is decided in, under the writer
    /// `w` and over `view`, at `now`, `n` drafts already in its append:
    /// the caller is judged, at the authored door.
    pub(crate) fn decide<'a>(
        &'a self,
        w: &'a Writer,
        view: &'a dyn ProjectionView,
        now: Timestamp,
        n: usize,
    ) -> Decide<'a> {
        Decide {
            call: self,
            w,
            view,
            pending: Pending::default(),
            now,
            next: position_after(w, n),
            who: self.who,
            door: Door::Authored,
            envelope: Envelope::of(Origin::Authored),
            bound: true,
            retiring_as: ChangeKind::Retired,
        }
    }

    /// `upsert`.
    pub fn upsert(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        patch: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        ordinary_door(ty)?;
        self.write(Op::Upsert(ty, key, &patch), opts)
    }

    /// An authored write of one record, as `upsert`, `retire`, `link` and
    /// `unlink` make it, their context already checked, under one hold of
    /// the writer: the plan and its append. A retry of a write the caller
    /// made is answered that write, as every retry is before the grants
    /// are asked: before a refusal for what the caller is not shown, and
    /// after a link door's own questions (035, extended).
    fn write(&self, op: Op<'_>, opts: WriteOpts) -> Result<Outcome> {
        let mut w = self.writer()?;
        let view = self.lake.engine.view(None)?;
        let first = self.replay(&w, &opts.idempotency_key, view.as_ref());
        let now = self.lake.clock(&mut w);
        let mut d = self.decide(&w, view.as_ref(), now, 0);
        if let Err(gone) = d.sight(op) {
            return first?.ok_or(gone);
        }
        d.shape(op)?;
        if let Some(first) = first? {
            return Ok(first);
        }
        let mut drafts = d.plan(op, opts.expected_version)?.drafts;
        self.keyed(&mut drafts[0], &opts.idempotency_key);
        drop(view);
        let events = self.lake.append(&mut w, drafts)?;
        self.told_of(&w.ontology, &events, outcome_of(&events))
    }

    /// `outcome` as its caller is told it: a set joined or left, or a
    /// trigger fired, at a record other than the write's own, which a far
    /// write sets off across an edge, only where the caller is answered
    /// one of the records it moved, as a write's own record is its
    /// writer's (055, extended); and, after an act, only where one of its
    /// reactions stands ([`overruled`]): a replay answers what the history
    /// without the erased values sets off (065).
    pub(super) fn told_of(
        &self,
        o: &Ontology,
        events: &[Event],
        mut outcome: Outcome,
    ) -> Result<Outcome> {
        let Outcome::Applied {
            position, set_off, ..
        } = &mut outcome
        else {
            return Ok(outcome);
        };
        let at = events.iter().find(|e| e.position == *position);
        let own = match at.map(|e| &e.body) {
            Some(
                EventBody::RecordUpserted { ty, key, .. }
                | EventBody::RecordIngested { ty, key, .. }
                | EventBody::RecordRetired { ty, key, .. },
            ) => (ty, key),
            _ => return Ok(outcome),
        };
        let mut moved: BTreeMap<(&str, &TypeName), Vec<&RecordKey>> = BTreeMap::new();
        let mut mine = BTreeSet::new();
        let reacts = |e: &&Event| e.cause == Some(*position);
        let (gone, _) = overruled(self.lake, events.iter().filter(reacts))?;
        let caused = |e: &&Event| reacts(e) && gone.binary_search(&e.position).is_err();
        for e in events.iter().filter(caused) {
            let (name, ty, key) = match &e.body {
                EventBody::ObjectSetChanged { set, ty, key, .. } => (set.as_str(), ty, key),
                EventBody::TriggerFired {
                    trigger, ty, key, ..
                } => (trigger.as_str(), ty, key),
                _ => continue,
            };
            match (ty, key) == own {
                true => mine.insert(name),
                false => {
                    moved.entry((name, ty)).or_default().push(key);
                    false
                }
            };
        }
        if moved.is_empty() && gone.is_empty() {
            return Ok(outcome);
        }
        let view = self.lake.engine.view(None)?;
        let sees = Sight::held(self.who, o);
        let mut told = mine;
        for ((name, ty), keys) in &moved {
            if !told.contains(name) && crate::reads::answered_of(view.as_ref(), &sees, ty, keys, 1)?
            {
                told.insert(name);
            }
        }
        let told: BTreeSet<String> = told.into_iter().map(str::to_owned).collect();
        set_off.fired.retain(|t| told.contains(t.as_str()));
        set_off.joined.retain(|s| told.contains(s));
        set_off.left.retain(|s| told.contains(s));
        Ok(outcome)
    }

    /// `retire`.
    pub fn retire(&self, ty: &TypeName, key: &RecordKey, opts: WriteOpts) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        ordinary_door(ty)?;
        self.write(Op::Retire(ty, key), opts)
    }

    /// `link`: the link over `relation` from `key` of `ty` to `to`, as a
    /// write of its link record (`docs/decisions/035-a-link-is-a-record.md`).
    pub fn link(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        relation: &RelationName,
        to: &RecordKey,
        fields: Patch,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        self.write(Op::Link((ty, key, relation, to), &fields), opts)
    }

    /// `unlink`: the link's record retired, its history kept.
    pub fn unlink(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        relation: &RelationName,
        to: &RecordKey,
        opts: WriteOpts,
    ) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        self.write(Op::Unlink((ty, key, relation, to)), opts)
    }

    /// The verb `verb` for its caller: one on a type the caller is not
    /// shown is answered as a verb that is not declared, before its scope
    /// is judged in a sentence that names the type.
    fn shown_verb<'o>(&self, o: &'o Ontology, verb: &VerbName) -> Result<&'o VerbDecl> {
        let v = o.verb(verb)?;
        let missing = Missing::Verb { name: verb.clone() };
        let shown = Sight::held(self.who, o).ty(&v.on);
        shown.map(|_| v).map_err(|_| Error::NotFound { missing })
    }

    /// Check a verb's payload against its declaration: a member the verb
    /// does not declare is refused, naming the verb, before any value is
    /// read, even when its type has a field of that name.
    fn verb_payload(&self, w: &Writer, verb: &VerbDecl, payload: &Patch) -> Result<Patch> {
        let decl = w.ontology.ty(&verb.on)?;
        let declared = |n: &&FieldName| verb.payload.iter().any(|f| &f.name == *n);
        if let Some(name) = payload.keys().find(|n| !declared(n)) {
            let (verb, member) = (verb.name.clone(), name.clone());
            return Err(Error::invalid_at(
                name,
                Case::PayloadUndeclared { verb, member },
            ));
        }
        let checked = check_values(&w.ontology, decl, payload, &verb.payload)?;
        for f in verb.payload.iter().filter(|f| f.required) {
            if checked.get(&f.name).is_none_or(Value::is_null) {
                let (verb, member) = (verb.name.clone(), f.name.clone());
                let case = Case::PayloadRequired { verb, member };
                return Err(Error::constraint(Some(f.name.clone()), case));
            }
        }
        Ok(checked)
    }

    /// A call's first questions, as `call` and `propose` ask them of the
    /// caller, in one order (013, the order of refusals): the verb is
    /// declared and shown, the key fits its scope, the caller may call it,
    /// an external verb's executor is bound, and the payload. Whether a
    /// call that is proposed may be pinned is part of whether its caller
    /// may call it (051). Answers the verb, the payload as checked, and
    /// whether its proposal is pinned.
    pub(super) fn callable(
        &self,
        w: &Writer,
        verb: &VerbName,
        proposed: bool,
        key: Option<&RecordKey>,
        payload: &Patch,
        expected: Option<Version>,
    ) -> Result<(VerbDecl, Patch, bool)> {
        let decl = self.shown_verb(&w.ontology, verb)?.clone();
        scoped(&decl, key, expected)?;
        let on = w.ontology.ty(&decl.on)?;
        granted(Sight::held(self.who, &w.ontology).callable(on, &decl))?;
        // A call is pinned to the records it reads across a reference too,
        // where its caller holds `only` on their type.
        let far = decl.reads_across(on, &|n| w.ontology.ty(n).ok());
        let mut far = far.iter().flatten();
        let pinned = (proposed || decl.risk != Risk::Low)
            && (self.pin(w, on, Some(&decl))?
                || far.any(|r| rights::only_holder(self.who, r.0, &w.ontology)));
        if let VerbEffect::External { executor } = &decl.effect {
            bound_executor(self.lake, executor)?;
        }
        let payload = self.verb_payload(w, &decl, payload)?;
        Ok((decl, payload, pinned))
    }
}
