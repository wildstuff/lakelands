//! The record doors: `upsert`, `retire`, `link`, `unlink` and `call`, and
//! the decision each write is made in.

use std::panic::{AssertUnwindSafe, catch_unwind};

use lakelands_api::seam::{Count, Spread};

use super::*;
use crate::exec::{Reply, stopped_by_panic};
use crate::lake::{Reads, owing};
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

/// What a decision reads beside its own drafts, the same from the
/// writer's lock to its append.
pub(crate) struct At<'a> {
    /// What the judged principal is shown, made once under the lock: the
    /// caller, or at an accept the proposer.
    pub sight: Sight<'a>,
    /// The view it reads.
    pub view: &'a dyn ProjectionView,
    /// The settings in force.
    pub settings: &'a BTreeMap<String, Value>,
    /// Its time.
    pub now: Timestamp,
    /// The most steps a path across an edge may take: the lake's
    /// `max_depth`.
    pub depth: u32,
}

/// What one write is decided in, from the writer's lock to its append: the
/// declarations in force and the plans their records are computed by,
/// what it reads ([`At`]), what its drafts so far leave, where its next
/// event lands, at which door, and the envelope of its own events.
pub(crate) struct Decide<'a> {
    pub(super) call: Asked<'a>,
    pub(super) ontology: &'a Ontology,
    pub(super) plans: &'a crate::record::Plans,
    pub at: At<'a>,
    /// The records and memberships its drafts change, not in the view yet.
    pub pending: Pending,
    /// Where the event being decided lands; [`Self::reactions`] moves it
    /// past that event and what it sets off.
    pub(super) next: Position,
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
        let own = self.call.draft(self.at.now, e.origin, body);
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
        let (view, pending, sees) = (self.at.view, &self.pending, &self.at.sight);
        let before = pending.record(view, &decl.name, key)?;
        let mine = match &before {
            Some(b) => sees.answers(&decl.name, b) && ends(view, pending, sees, decl, &b.fields)?,
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
        match t.before {
            Some(b) if t.mine && !b.retired && self.at.sight.told(&b).is_some() => {
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
        let decl = self.ontology.ty(ty)?;
        self.retires(decl)?;
        untold_setting(&self.at.sight, decl, key)?;
        let before = self.reached(decl, key, expected)?;
        let own = self.draft(retirement(ty, key, before.version));
        let (drafts, after) = (self.retiring(&before, own)?, None);
        Ok(Plan { drafts, after })
    }

    /// Whether the judged principal may retire a record of `decl`, then
    /// whether this door retires it, before a record is read.
    fn retires(&self, decl: &TypeDecl) -> Result<()> {
        not_a_view(decl)?;
        granted(self.at.sight.may_retire(decl))?;
        match self.door.admits_retirement(decl.retired_by) {
            true => Ok(()),
            false => Err(self.retired_by_verbs(decl)),
        }
    }

    /// What [`Self::plan`] refuses of `op` before a record is read, in its
    /// order: what the op names, a link door's own questions, then a
    /// retirement's grant and door, or a write's values; its checked
    /// values, which every record a change selects is written (081).
    pub(crate) fn unread(&self, op: Op<'_>) -> Result<Patch> {
        self.sight(op)?;
        let link = self.shape(op)?.map(|(lt, _)| lt);
        let (ty, patch) = match op {
            Op::Upsert(ty, _, patch) => (ty, Some(patch)),
            Op::Retire(ty, _) => (ty, None),
            Op::Link(_, fields) => (
                link.as_ref().expect("a link door has its type"),
                Some(fields),
            ),
            Op::Unlink(_) => (link.as_ref().expect("a link door has its type"), None),
            Op::Call(..) => unreachable!("a change names no verb"),
        };
        let decl = self.ontology.ty(ty)?;
        match patch {
            None => self.retires(decl).map(|()| Patch::new()),
            Some(p) if p.is_empty() => Err(Error::invalid(Case::PatchEmpty)),
            Some(p) => self.values(decl, p),
        }
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
        let sees = &self.at.sight;
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
        let decl = self.ontology.ty(ty)?;
        not_a_view(decl)?;
        let mut carried = decl.relations.iter();
        let r = carried.find(|r| &r.name == relation && r.carried_by_links());
        let r = r.ok_or_else(|| no_link_relation(ty, relation))?;
        let lt = links::link_type_name(ty, relation);
        self.ontology.ty(&lt)?;
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
                cardinality(
                    (self.at.view, &self.at.sight),
                    &self.pending,
                    (ty, &r),
                    key,
                    to,
                )?;
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
        self.values(decl, patch)
    }

    /// The door and the grant of every field `patch` carries, then its
    /// values, each in its written form.
    fn values(&self, decl: &TypeDecl, patch: &Patch) -> Result<Patch> {
        let door = (self.door, self.envelope.source.as_ref());
        let checked = self.at.sight.check_write(decl, door, patch);
        checked.map_err(|e| self.verbs_only(decl, e))?;
        check_values(self.ontology, decl, patch, &decl.fields)
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
        let sees = &self.at.sight;
        let sets = |v: &VerbDecl| matches!(&v.effect, VerbEffect::Write { set } if set.iter().any(|a| &a.field == field));
        let verbs: Vec<VerbName> = (self.ontology.verbs.values().map(|e| &e.decl))
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
        let sees = &self.at.sight;
        let verbs: Vec<VerbName> = (self.ontology.verbs.values().map(|e| &e.decl))
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
        let decl = self.ontology.ty(ty)?;
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
        let plan = self.plans.get(self.ontology, decl);
        // A field a hiding lowers on the record, before or after, is read
        // as one its grant does not let it read (006).
        let low = [live.map(|b| &b.fields), Some(&after.fields)]
            .map(|f| f.map(|f| self.at.sight.lowered(&decl.name, f)));
        let reads = |f: &FieldName| {
            let mut fields = decl.fields.iter().filter(|d| &d.name == f);
            let told = low.iter().flatten().all(|l| !l.contains_key(f));
            told && fields.any(|d| self.at.sight.field_access(decl, d).reads())
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
                settings: Some(self.at.settings),
                now: self.at.now,
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
        let (o, view, sees) = (self.ontology, self.at.view, &self.at.sight);
        untold_setting(sees, decl, key)?;
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
            fill_key_fields(o, decl, key, &mut patch, door, sees)?;
        }
        if decl.name.as_str() == SETTING_TYPE {
            let told = |s: &SettingDecl| sees.setting_visible(s);
            let before = live.map(|b| &b.fields);
            stamped(o, decl, key, before, &told, &mut patch)?;
        }
        // A setting is judged as it was written, before its version is: one
        // of a type it is not shown is answered as one never declared (063).
        let declared = |t: &TypeName| o.ty(t).ok();
        let shown = |t: &TypeName| sees.describe_type(t);
        let judged_early = decl.name.as_str() == SETTING_TYPE && sees.narrows(&decl.name);
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
            check_references(view, &self.pending, o, &decl.name, key, judged, sees)?;
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
        // A field its writer's grant hides on the record, before the write
        // or after it, its derived fields made, is not written (012).
        let records = [live.map(|b| &b.fields), Some(&after.fields)];
        sees.check_hidden_write(&decl.name, records, &sent)?;
        record::check_required(decl, &after.fields)?;
        self.constraints_hold(decl, live.filter(|_| !other), &after, &sent)?;
        if after.ty.as_str() == AGING_RULE_TYPE {
            let rule = crate::aging::check(o, &after.fields)
                .map_err(|(field, said)| Error::invalid_at(&field, *said))?;
            crate::aging::check_authority(sees, o, &rule)?;
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
            fields.any(|d| sees.field_access(decl, d).reads())
        };
        let pending = &self.pending;
        match check_references(view, pending, o, &decl.name, &after.key, &computed, sees) {
            Err(Error::Refused { field: Some(f), .. }) if !reads(&f) => granted(false)?,
            judged => judged?,
        }
        granted(!other && owns(view, &self.pending, sees, decl, &after.fields)?)?;
        Ok(RecordPlan {
            draft,
            after,
            before: t.before,
        })
    }

    /// The record `d`, which lands next, makes of `before`: computed with
    /// the fold's own function, which hands out no call here.
    fn apply(&self, before: Option<&StoredRecord>, d: &Draft) -> Result<StoredRecord> {
        let event = provisional(self.call.lake, self.next, d);
        // Its own keys sealed apart are read from its event, so only a
        // version's first use is staged (065).
        let staged = match self.kept(d)? {
            Some((at, list)) => {
                let mut staged = self.pending.sealed.clone();
                staged.entry(at).or_default().push((self.next, list));
                std::borrow::Cow::Owned(staged)
            }
            None => std::borrow::Cow::Borrowed(&self.pending.sealed),
        };
        let outputs = &mut Outputs {
            staged: Some(&staged),
            apart: Some(&self.pending.apart),
            ..Outputs::planning(self.at.view, self.plans, &self.call.lake.keys)
        };
        let after = record::apply(self.ontology, before, &event, self.at.settings, outputs)?;
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
        let (o, now, cause, context) = (self.ontology, self.at.now, self.next, self.call.context);
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
            for t in o.triggers.values().map(|e| &e.decl).filter(on) {
                if self.fires(t, before, after)? {
                    let at = (&after.ty, &after.key);
                    out.push(system(self.firing(t, at, (change, created))));
                }
            }
            let of = |s: &&ObjectSetDecl| s.of == after.ty;
            for s in o.object_sets.values().map(|e| &e.decl).filter(of) {
                let was = self
                    .pending
                    .member(self.at.view, &s.name, &after.ty, &after.key)?;
                let is = self.member(s, after, self.at.settings)?;
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
        let (at, now) = (self.next, self.at.now);
        let after = self.apply(Some(before), &d)?;
        let mut drafts = vec![d];
        drafts.extend(self.reactions(self.retiring_as, Some(before), &after)?);
        let calls = match &mut self.pending.calls {
            Some(calls) => calls,
            none => none.insert(crate::backed::pending_calls(self.at.view)?),
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
    /// its source's key ([`crate::sealing::kept`]).
    fn kept(&self, d: &Draft) -> Result<Option<((String, Digest), Value)>> {
        let ingested = matches!(d.body, EventBody::RecordIngested { .. });
        let (Some(sealed), Some(source), true) = (&d.sealed, d.source.as_ref(), ingested) else {
            return Ok(None);
        };
        let used = (source, sealed, self.next, self.at.now);
        let read = (&self.pending.sealed, self.at.view, &*self.call.lake.keys);
        crate::sealing::kept(read, used)
    }

    /// A planned record write with what it sets off: its own event, which
    /// lands next, then the triggers and object-set changes it causes.
    pub(crate) fn planned(&mut self, plan: RecordPlan) -> Result<Plan> {
        if let Some((at, list)) = self.kept(&plan.draft)? {
            self.pending
                .sealed
                .entry(at)
                .or_default()
                .push((self.next, list));
        }
        // Its keys sealed apart are in no log yet: what the append computes
        // after it reads them here (065).
        let ingested = matches!(plan.draft.body, EventBody::RecordIngested { .. });
        if let (Some(sealed), Some(source), true) =
            (&plan.draft.sealed, &plan.draft.source, ingested)
            && !sealed.apart.is_empty()
        {
            self.pending.apart.stage(source, self.next, &sealed.apart);
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
        let now = self.at.now;
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
        let (o, on) = (self.ontology, self.ontology.ty(&verb.on)?);
        let record = self.reached(on, key, expected)?;
        // A record read across a reference is reached where a write that
        // sends the reference would be accepted, and by nothing else.
        // One a hiding selects is refused where the verb reads a field
        // lowered there, as on its own record below (006).
        let (view, pending, sees) = (self.at.view, &self.pending, &self.at.sight);
        let reads = verb.reads_across(on, &|n| o.ty(n).ok()).unwrap_or_default();
        let far = |ty: &TypeName, k: &RecordKey| {
            let found = live_for(view, pending, ty, k, sees)?;
            let low = (found.as_ref())
                .map(|r| sees.lowered(ty, &r.fields))
                .unwrap_or_default();
            let read =
                |(t, f): &(&TypeDecl, &FieldDecl)| t.name == *ty && low.contains_key(&f.name);
            granted(!reads.iter().any(read)).map(|()| found)
        };
        let link = |r: &RelationName| link_from(view, &verb.on, r, key);
        let limits = (self.at.depth, (self.at.settings, now));
        let stored = |ty: &TypeName, k: &RecordKey| pending.record(view, ty, k);
        let across = Across::new(o, on, (&far, &link, &stored), limits);
        // A field a hiding lowers on the record is one its caller does not
        // read there: a verb that reads it would say what it holds (006).
        let low = self.at.sight.lowered(&verb.on, &record.fields);
        let clear = |r: BTreeSet<FieldName>| r.iter().all(|f| !low.contains_key(f));
        granted(low.is_empty() || verb.fields_read().is_some_and(clear))?;
        // A part is named as the caller is shown what it reads: at an
        // accept, the reviewer.
        let told = |k: &str| {
            let (value, declared) = (self.at.settings.get(k), o.settings.get(k).map(|e| &e.decl));
            Sight::held(self.call.who, o).setting_shown(k, value, declared)
        };
        requires(
            self.at.settings,
            (verb, payload),
            key,
            &record,
            (now, &across),
            &told,
        )?;
        // A verb on a link finds both its ends live and answered, whatever
        // its effect (035): one that writes the link has them judged where
        // the record is written, any other here, as `link` judges them.
        if links::is_link_type(&verb.on) && !matches!(verb.effect, VerbEffect::Write { .. }) {
            let end = |f: &&FieldName| [links::FROM, links::TO].contains(&f.as_str());
            let ends: Patch = (record.fields.iter().filter(|(f, _)| end(f)))
                .map(|(f, v)| (f.clone(), v.clone()))
                .collect();
            check_references(view, pending, o, &verb.on, key, &ends, sees)?;
        }
        let patch = assigned(o, self.at.settings, verb, &record, payload, (now, &across))?;
        let called_at = self.next;
        self.next = called_at.next();
        if matches!(verb.effect, VerbEffect::Write { .. } | VerbEffect::Retire) {
            let version = self.ontology.verbs.get(&verb.name).map_or(1, |e| e.version);
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

impl<'a> Asked<'a> {
    /// The context a write of this call is decided in, under the writer
    /// `w` and over `view`, at `now`, `n` drafts already in its append:
    /// the caller is judged, at the authored door.
    pub(crate) fn decide(
        self,
        w: &'a Writer,
        view: &'a dyn ProjectionView,
        now: Timestamp,
        n: usize,
    ) -> Decide<'a> {
        let sight = Sight::held(self.who, &w.ontology);
        let depth = self.lake.limits.max_depth;
        let (settings, ontology, plans) = (&w.settings, &w.ontology, &w.plans);
        Decide {
            call: self,
            ontology,
            plans,
            at: At {
                sight,
                view,
                settings,
                now,
                depth,
            },
            pending: Pending::default(),
            next: position_after(w, n),
            door: Door::Authored,
            envelope: Envelope::of(Origin::Authored),
            bound: true,
            retiring_as: ChangeKind::Retired,
        }
    }

    /// `upsert_preflight`: `upsert`'s decision, at the head, appending
    /// nothing. It reads as a preflight does, with no turn and under no
    /// lock: the writer that decides the write later may find a head that
    /// has moved.
    pub fn upsert_preflight(
        self,
        ty: &TypeName,
        key: &RecordKey,
        patch: &Patch,
        expected: Option<Version>,
    ) -> Result<()> {
        check_context(self.lake, &self.context)?;
        ordinary_door(ty)?;
        let view = self.lake.engine.view(None)?;
        let w = self.lake.reading(view.as_ref())?;
        let mut d = self.decide(&w, view.as_ref(), w.last_known_at, 0);
        d.plan(Op::Upsert(ty, key, patch), expected).map(drop)
    }
}

impl Call<'_> {
    /// The context a write of this call is decided in: [`Asked::decide`].
    pub(crate) fn decide<'a>(
        &'a self,
        w: &'a Writer,
        view: &'a dyn ProjectionView,
        now: Timestamp,
        n: usize,
    ) -> Decide<'a> {
        self.asked().decide(w, view, now, n)
    }

    /// A record door's write, its context checked, under one hold of the
    /// writer: the plan and its append.
    pub(crate) fn record(&self, write: &Write, opts: WriteOpts) -> Result<Outcome> {
        let op = write.op();
        self.checked(op)?;
        let mut w = self.writer()?;
        let view = self.lake.engine.view(None)?;
        let now = self.lake.clock(&mut w);
        let mut pending = Pending::default();
        let drafts =
            match self.decided((&w, view.as_ref(), now), &mut pending, (op, &opts), None)? {
                Decided::Drafts(drafts) => drafts,
                Decided::Replayed(first) => return Ok(first),
                Decided::Again(..) => unreachable!("a write alone has no batch"),
            };
        drop(view);
        let events = self.lake.append(&mut w, drafts)?;
        self.told_of(&w.ontology, &events, outcome_of(&events), None)
    }

    /// A record door's own questions of its context: its depth, and for
    /// `upsert` and `retire` a type written through no other door.
    fn checked(&self, op: Op<'_>) -> Result<()> {
        check_context(self.lake, &self.context)?;
        match op {
            Op::Upsert(ty, ..) | Op::Retire(ty, _) => ordinary_door(ty),
            _ => Ok(()),
        }
    }

    /// A record write decided under the writer at a time, over a view,
    /// against what `pending` holds: its drafts, or what a retry is
    /// answered, the first write's, `again` naming the job of its batch
    /// that used its key. A retry is answered that write as every retry
    /// is before the grants are asked: before a refusal for what the
    /// caller is not shown, and after a link door's own questions (035,
    /// extended).
    fn decided(
        &self,
        (w, view, now): (&Writer, &dyn ProjectionView, Timestamp),
        pending: &mut Pending,
        (op, opts): (Op<'_>, &WriteOpts),
        again: Option<usize>,
    ) -> Result<Decided> {
        let key = &opts.idempotency_key;
        let first = match again {
            Some(job) => Ok(Some(Decided::Again(job, head(w)))),
            None => (self.replay(w, key, view)).map(|f| f.map(Decided::Replayed)),
        };
        let mut d = Lent(self.decide(w, view, now, 0), pending);
        std::mem::swap(&mut d.0.pending, d.1);
        if let Err(gone) = d.0.sight(op) {
            return first?.ok_or(gone);
        }
        d.0.shape(op)?;
        if let Some(first) = first? {
            return Ok(first);
        }
        let mut drafts = d.0.plan(op, opts.expected_version)?.drafts;
        self.keyed(&mut drafts[0], key);
        Ok(Decided::Drafts(drafts))
    }

    /// `outcome` as its caller is told it: a set joined or left, or a
    /// trigger fired, at a record other than the write's own, which a far
    /// write sets off across an edge, only where the caller is answered
    /// one of the records it moved, as a write's own record is its
    /// writer's (055, extended); and, after an act, only where one of its
    /// reactions stands ([`overruled`]): a replay answers what the history
    /// without the erased values sets off (065). Read at the head, or at
    /// `at`, where a job of a batch is told.
    pub(super) fn told_of(
        &self,
        o: &Ontology,
        events: &[Event],
        mut outcome: Outcome,
        at: Option<Position>,
    ) -> Result<Outcome> {
        let Outcome::Applied {
            position, set_off, ..
        } = &mut outcome
        else {
            return Ok(outcome);
        };
        let written = events.iter().find(|e| e.position == *position);
        let own = match written.map(|e| &e.body) {
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
        let view = self.lake.engine.view(at)?;
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
        let sees = Sight::held(self.who, &w.ontology);
        granted(sees.callable(on, &decl))?;
        // A call is pinned to the records it reads across a reference too,
        // where its caller holds `only` on their type.
        let far = decl.reads_across(on, &|n| w.ontology.ty(n).ok());
        let mut far = far.iter().flatten();
        let pinned = (proposed || decl.risk != Risk::Low)
            && (Self::pin(&sees, on, Some(&decl))? || far.any(|r| sees.only_holder(r.0)));
        if let VerbEffect::External { executor } = &decl.effect {
            bound_executor(self.lake, executor)?;
        }
        let payload = self.verb_payload(w, &decl, payload)?;
        Ok((decl, payload, pinned))
    }
}

/// A record door's write as data: what a turn decides ([`write_many`]).
#[derive(Clone, Debug)]
pub enum Write {
    /// `upsert`: the type, the key and the patch.
    Upsert(TypeName, RecordKey, Patch),
    /// `retire`: the type and the key.
    Retire(TypeName, RecordKey),
    /// `link`: the type and the key it starts from, its relation, the key
    /// it ends at, and the link's own fields, as a write of its link
    /// record (`docs/decisions/035-a-link-is-a-record.md`).
    Link(TypeName, RecordKey, RelationName, RecordKey, Patch),
    /// `unlink`: the link's record retired, its history kept.
    Unlink(TypeName, RecordKey, RelationName, RecordKey),
}

impl Write {
    fn op(&self) -> Op<'_> {
        match self {
            Write::Upsert(ty, key, patch) => Op::Upsert(ty, key, patch),
            Write::Retire(ty, key) => Op::Retire(ty, key),
            Write::Link(ty, key, r, to, fields) => Op::Link((ty, key, r, to), fields),
            Write::Unlink(ty, key, r, to) => Op::Unlink((ty, key, r, to)),
        }
    }

    /// Whether it may be decided in a batch under `o`: not a setting nor
    /// an aging rule, with no cause the host names, and of no type, a
    /// link's link type, computed outside the lake, whose append makes
    /// calls due.
    fn batches(&self, o: &Ontology, context: &WriteContext) -> bool {
        let (ty, relation) = match self {
            Write::Upsert(ty, ..) | Write::Retire(ty, _) => (ty, None),
            Write::Link(ty, _, r, ..) | Write::Unlink(ty, _, r, _) => (ty, Some(r)),
        };
        let system = [SETTING_TYPE, AGING_RULE_TYPE].contains(&ty.as_str());
        let Ok(decl) = o.ty(ty) else {
            return !system && context.cause.is_none();
        };
        let written = match relation {
            None => Some(decl.clone()),
            Some(r) => {
                (decl.relations.iter().find(|d| d.name == *r)).map(|r| links::link_type(ty, r))
            }
        };
        let outside = written.is_some_and(|t| crate::backed::computed_outside(o, &t));
        !system && context.cause.is_none() && !outside
    }
}

/// A record write queued for its turn, its caller as it asked, seen at
/// the turn as a door's write is, and where it is answered: its outcome
/// and the causes of what it left owed.
pub(crate) struct Job {
    pub who: Principal,
    pub context: WriteContext,
    pub write: Write,
    pub opts: WriteOpts,
    pub answer: Answer,
}

/// Where a [`Job`] is answered.
pub(crate) type Answer = tokio::sync::oneshot::Sender<Result<(Outcome, BTreeSet<Position>)>>;

/// A record write decided ([`Call::decided`]).
enum Decided {
    /// Its drafts.
    Drafts(Vec<Draft>),
    /// A retry, answered the first write's outcome.
    Replayed(Outcome),
    /// A retry of the write of an earlier job of its batch, by its index,
    /// told at the batch's tip as it was decided.
    Again(usize, Position),
}

/// A decision lent a batch's [`Pending`], which goes back to it however
/// the decision ends.
struct Lent<'d, 'p>(Decide<'d>, &'p mut Pending);

impl Drop for Lent<'_, '_> {
    fn drop(&mut self) {
        std::mem::swap(&mut self.0.pending, self.1);
    }
}

/// A job of a batch once it is decided: its caller, where it is answered,
/// and its events' range in the batch's append or its answer.
struct Decision {
    who: Principal,
    answer: Answer,
    told: Result<Told>,
}

enum Told {
    Events(std::ops::Range<usize>),
    Again(usize, Position),
    Replayed(Outcome),
}

/// The head of record writes queued together, decided under one turn;
/// the jobs left behind it are answered. The head runs alone, as
/// [`Call::record`] decides it, where the writer must first catch up,
/// regain its lease or append a held answer, where it holds no lease (a
/// handle opened read-only, a standby), and where it does not batch
/// ([`Write::batches`]). Otherwise it and the jobs behind it that batch
/// are decided one by one, each against the drafts of those before it,
/// and appended together, up to `commit_window_events`; none is answered
/// before that append returns. A job refused, or stopped, is answered
/// alone and leaves nothing behind; one that stopped ends its batch.
pub(crate) fn write_many(turn: &Turn<'_>, jobs: Vec<Job>, replies: &mut Vec<Reply>) -> Vec<Job> {
    let mut jobs = std::collections::VecDeque::from(jobs);
    let Some(head) = jobs.pop_front() else {
        return Vec::new();
    };
    let w = turn.writer();
    let ready = w.lease.is_some() && !w.lost && !w.behind && w.answers.is_empty();
    let batches = |j: &Job| j.write.batches(&w.ontology, &j.context);
    if !(ready && batches(&head)) {
        drop(w);
        replies.push(head.alone(turn));
        return jobs.into();
    }
    let n = jobs.iter().take_while(|j| batches(j)).count();
    let batch = std::iter::once(head).chain(jobs.drain(..n)).collect();
    drop(w);
    let mut left = batched(turn, batch, replies);
    left.extend(jobs);
    left
}

impl Job {
    /// Decided and appended alone, as its door did before batches, under
    /// its own [`owing`].
    fn alone(self, turn: &Turn<'_>) -> Reply {
        let (who, context, write, opts) = (self.who, self.context, &self.write, self.opts);
        let done = catch_unwind(AssertUnwindSafe(|| {
            let (who, followed, shown) = crate::doors::seen(turn, who)?;
            owing(|| call(turn, &who, (context, followed, shown)).record(write, opts))
        }));
        let done = done.unwrap_or_else(|p| Err(stopped_by_panic(p)));
        turn.observe(Spread::BatchWrites, 1);
        turn.count(Count::Writes, 1);
        Box::new(move || drop(self.answer.send(done)))
    }
}

/// The call `who` makes at `turn`, with its context, how far its grants
/// were followed, and what it was shown.
fn call<'a>(
    turn: &'a Turn<'a>,
    who: &'a Principal,
    (context, followed, shown): (WriteContext, Option<usize>, Option<Shown>),
) -> Call<'a> {
    Call {
        lake: turn,
        who,
        context,
        followed,
        shown,
    }
}

/// Decide `jobs` one by one under one hold of the writer, then append
/// them together and answer each into `replies`; answers the jobs a
/// stopped one left undecided. Where the writer cannot be taken, a job
/// its caller may not write is refused that first, as alone.
fn batched(turn: &Turn<'_>, jobs: Vec<Job>, replies: &mut Vec<Reply>) -> Vec<Job> {
    let opened = catch_unwind(AssertUnwindSafe(|| -> Result<_> {
        let w = turn.for_write()?;
        Ok((w, turn.engine.view(None)?))
    }));
    let (mut w, view) = match opened.unwrap_or_else(|p| Err(stopped_by_panic(p))) {
        Ok(opened) => opened,
        Err(e) => {
            for job in jobs {
                let checked = catch_unwind(AssertUnwindSafe(|| {
                    let (who, followed, shown) = crate::doors::seen(turn, job.who)?;
                    call(turn, &who, (job.context, followed, shown)).checked(job.write.op())
                }));
                let refused = match checked {
                    Ok(Ok(())) => e.clone(),
                    Ok(Err(refused)) => refused,
                    Err(p) => stopped_by_panic(p),
                };
                let answer = job.answer;
                replies.push(Box::new(move || drop(answer.send(Err(refused)))));
            }
            return Vec::new();
        }
    };
    let (mut pending, mut keys) = (Pending::default(), BTreeMap::new());
    let (mut decisions, mut lease, mut events, mut stored) = (vec![], None, vec![], vec![]);
    let (mut jobs, mut stopped) = (jobs.into_iter(), false);
    // A batch closes after the job its events reach the bound with.
    let bound = turn.limits.commit_window_events.max(1) as usize;
    for job in jobs.by_ref() {
        let Job {
            mut who,
            context,
            write,
            opts,
            answer,
        } = job;
        let mark = pending.mark();
        let now = turn.clock(&mut w);
        let id = (opts.idempotency_key.as_ref()).map(|k| log::authored_id(&who.writer(), k));
        let again = id.as_ref().and_then(|id| keys.get(id).copied());
        let decided = catch_unwind(AssertUnwindSafe(|| -> Result<_> {
            let (judged, followed, shown) = crate::doors::seen(turn, who.clone())?;
            let call = call(turn, &judged, (context, followed, shown));
            let op = write.op();
            call.checked(op)?;
            call.moved(&w)?;
            let at = (&*w, view.as_ref(), now);
            let told = match call.decided(at, &mut pending, (op, &opts), again)? {
                Decided::Drafts(drafts) => {
                    // Its door refused what the seal would, and the batch
                    // opened on a writer that may append.
                    let stamped = turn.stamp(&w, drafts);
                    debug_assert!(stamped.is_ok(), "a batched stamp refused");
                    (Told::Events(0..0), Some(stamped?))
                }
                Decided::Again(j, tip) => (Told::Again(j, tip), None),
                Decided::Replayed(first) => (Told::Replayed(first), None),
            };
            Ok((told, judged))
        }));
        let decided = decided.map(|d| {
            d.map(|(told, judged)| {
                who = judged;
                told
            })
        });
        let told = match decided {
            Ok(Ok((_, Some((l, mut e, mut s))))) => {
                if let Some(last) = e.last() {
                    w.staged = Some((last.position, last.hash.clone()));
                }
                let range = events.len()..events.len() + e.len();
                events.append(&mut e);
                stored.append(&mut s);
                lease = Some(l);
                if let Some(id) = id {
                    keys.insert(id, decisions.len());
                }
                Ok(Told::Events(range))
            }
            Ok(told) => told.map(|(t, _)| t),
            Err(p) => {
                stopped = true;
                Err(stopped_by_panic(p))
            }
        };
        match &told {
            Ok(_) => pending.keep(),
            Err(_) => pending.undo(mark),
        }
        decisions.push(Decision { who, answer, told });
        if stopped || events.len() >= bound {
            break;
        }
    }
    drop(view);
    w.staged = None;
    let first = events.first().map(|e: &Event| e.position);
    let owed = w.owed.len();
    let shared = catch_unwind(AssertUnwindSafe(|| -> Result<_> {
        let (events, causes) = match lease {
            Some(l) => {
                owing(|| turn.append_stamped(&mut w, (l, events, stored), Reads::default()))?
            }
            None => (Vec::new(), BTreeSet::new()),
        };
        debug_assert_eq!(w.owed.len(), owed, "a batched write makes no call due");
        let tell = |r: &std::ops::Range<usize>, who: &Principal, at: Position| {
            let call = call(turn, who, (WriteContext::default(), None, None));
            let e = &events[r.clone()];
            call.told_of(&w.ontology, e, outcome_of(e), Some(at))
        };
        let told = |d: &Decision| match d.told.as_ref().map_err(Clone::clone)? {
            Told::Events(r) => {
                let (from, to) = (events[r.start].position, events[r.end - 1].position);
                let own = causes.range(from..=to).copied().collect();
                Ok((tell(r, &d.who, to)?, own))
            }
            Told::Again(j, tip) => match &decisions[*j].told {
                Ok(Told::Events(r)) => Ok((tell(r, &d.who, *tip)?, BTreeSet::new())),
                _ => unreachable!("a key is kept for a job that appended"),
            },
            Told::Replayed(first) => Ok((first.clone(), BTreeSet::new())),
        };
        Ok(decisions.iter().map(told).collect::<Vec<_>>())
    }));
    let answers = match shared {
        Ok(Ok(answers)) => answers,
        Ok(Err(e)) => (decisions.iter())
            .map(|d| Err(failed(d, &e, first)))
            .collect(),
        Err(p) => {
            turn.stopped_append(&mut w, first);
            let e = stopped_by_panic(p);
            decisions.iter().map(|_| Err(e.clone())).collect()
        }
    };
    drop(w);
    turn.observe(Spread::BatchWrites, decisions.len() as u64);
    turn.count(Count::Writes, decisions.len() as u64);
    for (d, answer) in decisions.into_iter().zip(answers) {
        replies.push(Box::new(move || drop(d.answer.send(answer))));
    }
    jobs.collect()
}

/// What a job of a batch whose append from `first` failed `e` is
/// answered: where only its fold failed, a refusal stays, decided over
/// what is durable, and a job that appended is told so at its own first
/// position; otherwise `e`, as nothing of the batch is in the log.
fn failed(d: &Decision, e: &Error, first: Option<Position>) -> Error {
    match (&d.told, e.situation(), first) {
        (Err(refused), Some(Situation::NotFolded { .. }), _) => refused.clone(),
        (Ok(Told::Events(r)), Some(Situation::NotFolded { cause, .. }), Some(first)) => {
            let (from, cause) = (Position(first.0 + r.start as u64), cause.clone());
            Error::unavailable(Situation::NotFolded { from, cause })
        }
        _ => e.clone(),
    }
}
