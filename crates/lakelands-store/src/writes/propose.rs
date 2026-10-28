//! Proposals: a write proposed, accepted or rejected, and what its
//! proposer is told of it.

use super::*;

impl Call<'_> {
    /// The `write_proposed` of `write`, which lands next under the writer
    /// `w`: its proposal is named by the event's id.
    fn proposal(
        &self,
        w: &Writer,
        now: Timestamp,
        (write, why, pinned): (ProposedWrite, String, bool),
        key: &Option<IdempotencyKey>,
    ) -> Draft {
        let id = key.as_ref().map_or_else(
            || log::positional_id(&self.lake.id, position_after(w, 0)),
            |k| log::authored_id(&self.who.writer(), k),
        );
        let proposal = ProposalId::from(id.0.as_str());
        let body = EventBody::WriteProposed {
            proposal,
            write,
            why,
            pinned,
        };
        let mut d = self.draft(now, Origin::Authored, body);
        self.keyed(&mut d, key);
        d
    }

    /// `call`. Answers the outcome, and for an external verb that ran,
    /// the invocation the caller must hand its executor.
    pub fn call(
        &self,
        verb: &VerbName,
        key: Option<&RecordKey>,
        payload: Patch,
        opts: WriteOpts,
    ) -> Result<(Outcome, Option<External>)> {
        check_context(self.lake, &self.context, self.who)?;
        let mut w = self.writer()?;
        let view = self.lake.engine.view(None)?;
        if let Some(o) = self.replay(&w, &opts.idempotency_key, view.as_ref())? {
            return Ok((o, None));
        }
        let now = self.lake.clock(&mut w);
        let expected = opts.expected_version;
        let (decl, payload, pinned) = self.callable(&w, verb, false, key, &payload, expected)?;
        // A low-risk call runs. Any other is proposed, and one that could
        // not run now is not: the version, then `requires`, and the write
        // it would make, as the caller, as when it runs; accepting it
        // judges the record again (013).
        let low = decl.risk == Risk::Low;
        let mut d = self.decide(&w, view.as_ref(), now, usize::from(!low));
        let mut drafts = d.plan(Op::Call(&decl, key, &payload), expected)?.drafts;
        if low {
            self.keyed(&mut drafts[0], &opts.idempotency_key);
        } else {
            let write = ProposedWrite::Call {
                verb: verb.clone(),
                key: key.cloned(),
                payload: payload.clone(),
                opts: opts.clone(),
            };
            let why = format!(
                "`{verb}` is declared {} risk; it runs when accepted",
                risk_word(decl.risk)
            );
            let proposed = (write, why, pinned);
            drafts = vec![self.proposal(&w, now, proposed, &opts.idempotency_key)];
        }
        drop(view);
        let due = low && matches!(decl.effect, VerbEffect::External { .. });
        let appended = self.lake.append(&mut w, drafts);
        let keyed = opts.idempotency_key.is_some();
        let by = MadeDue::Call { keyed };
        let events = appended.map_err(|e| not_handed(e, due && w.behind, by))?;
        let outcome = self.told_of(&w.ontology, &events, outcome_of(&events), None)?;
        let external = match &decl.effect {
            VerbEffect::External { executor } if low => Some(External {
                executor: executor.clone(),
                verb: verb.clone(),
                ty: decl.on.clone(),
                key: key.cloned(),
                payload,
                position: events[0].position,
            }),
            _ => None,
        };
        Ok((outcome, external))
    }

    /// `propose`: record a write, checked whole as if it were applied, as a
    /// proposal.
    ///
    /// What is logged is the write as checked — its values normalised, its
    /// key fields filled — so the proposal holds the one written form of
    /// every value (`docs/log-format.md` §1.3), exactly as an applied write
    /// would.
    pub fn propose(&self, mut write: ProposedWrite, why: String) -> Result<Outcome> {
        check_context(self.lake, &self.context, self.who)?;
        let mut w = self.writer()?;
        let view = self.lake.engine.view(None)?;
        let key_opt = match &write {
            ProposedWrite::Upsert { opts, .. }
            | ProposedWrite::Retire { opts, .. }
            | ProposedWrite::Call { opts, .. } => opts.idempotency_key.clone(),
            _ => None,
        };
        if let Some(o) = self.replay(&w, &key_opt, view.as_ref())? {
            return Ok(o);
        }
        let now = self.lake.clock(&mut w);
        // A holder of `only` changes no shape, and proposes no write that
        // anything but the write itself could move off its pairs (051).
        granted(!write.is_schema() || !Sight::held(self.who, &w.ontology).holds_only())?;
        let mut d = self.decide(&w, view.as_ref(), now, 1);
        let pin = |d: &Decide<'_>, ty: &TypeName| {
            ordinary_door(ty)?;
            Self::pin(&d.at.sight, d.at.sight.ty(ty)?, None)
        };
        let (mut pinned, judging) = (false, Judging::Proposal);
        // A change of the ontology keeps what it would do as its proposer
        // is shown it without `restricted`, so no count it keeps tells of a
        // hold; each reader is shown it as its listing shows it (062).
        let mut plain = self.who.clone();
        plain.grants.restricted = false;
        let (who, followed, shown) = (&plain, None, None);
        let as_plain = Call {
            who,
            followed,
            shown,
            lake: self.lake,
            context: self.context.clone(),
        };
        // A re-key's sections of the other types, for the outcome (062).
        let mut others = Vec::new();
        // A bundle wholly in force, answered as `apply` answers it.
        let mut in_force = None;
        match &mut write {
            ProposedWrite::Upsert {
                ty,
                key,
                patch,
                opts,
            } => {
                pinned = pin(&d, ty)?;
                let mut plan = d.plan(Op::Upsert(ty, key, patch), opts.expected_version)?;
                let EventBody::RecordUpserted {
                    patch: mut checked, ..
                } = plan.drafts.remove(0).body
                else {
                    unreachable!("an authored record write is an upsert")
                };
                // A setting's group is stamped at the accept, from the
                // declaration then in force (063, extended).
                if ty.as_str() == SETTING_TYPE && !patch.contains_key("group") {
                    checked.remove("group");
                }
                *patch = checked;
            }
            ProposedWrite::Retire { ty, key, opts } => {
                pinned = pin(&d, ty)?;
                d.plan(Op::Retire(ty, key), opts.expected_version)?;
            }
            ProposedWrite::Call {
                verb,
                key,
                payload,
                opts,
            } => {
                let (key, expected) = (key.as_ref(), opts.expected_version);
                let (decl, checked, may) = self.callable(&w, verb, true, key, payload, expected)?;
                // A call that could not run when accepted is not proposed (013).
                d.plan(Op::Call(&decl, key, &checked), expected)?;
                (*payload, pinned) = (checked, may);
            }
            // A change of the ontology is checked now as its door would
            // check it, and nothing changes until it is accepted; the
            // proposer needs no `schema` grant (013, extended).
            ProposedWrite::Declare { declarations, plan } => {
                self.redeclared(&w.ontology, declarations)?;
                self.check_bundle(&w.ontology, declarations)?;
                // Its plan, telling only: what the walk finds is the accept's
                // to refuse, as before (062).
                let mut changed = declarations.clone();
                changed.retain(|d| !w.ontology.in_force(d));
                let types = match changed.is_empty() {
                    true => {
                        in_force = Some(declarations.clone());
                        Vec::new()
                    }
                    false => (as_plain.bundle_drafts(&w, changed, (now, 0, judging))?).types,
                };
                *plan = Some(Box::new(plan::plan_of(self.lake, &w, (types, Vec::new()))?));
            }
            ProposedWrite::Migrate { migration, report } => {
                let author = self.authored(&w.ontology, migration)?;
                // A retarget converts as its author is answered (lakelands#1071).
                let judged = (&**migration, author.as_ref(), Some(self.who));
                let planned =
                    as_plain.preflight_plan(&w, view.as_ref(), judged, (None, judging))?;
                // One whose target is in force changes nothing: answered as
                // a declaration in force is, and not filed (027, extended).
                if unchanged(&w.ontology, migration, &planned) {
                    in_force = Some(vec![Declaration::Type(migration.to.clone())]);
                }
                // Refused as lossy only by what its proposer is told of, as
                // a proposal of declarations is: the accept judges the rest.
                let found = planned.report;
                if !found.lossless {
                    return Err(self.lossy(&w, migration, found));
                }
                **report = found;
                others = planned.others;
            }
            ProposedWrite::MigrateValueType {
                to,
                renames,
                report,
            } => {
                let m = ValueTypeMigration {
                    to: (**to).clone(),
                    renames: renames.clone(),
                };
                // Refused by a constraint left broken only on what its
                // proposer is told of, as a migration is.
                let found = as_plain
                    .preflight_value_type(&w, view.as_ref(), &m, (None, judging))?
                    .report;
                self.renames_shown(&w.ontology, &m, &found)?;
                **report = fits(found, false)?;
            }
            // A change to records is planned as its proposer's `plan` plans
            // it, over its proposer's own records: its pairs, and restricted
            // records left out where it does not hold `restricted`, are held
            // beside the condition, so the accept selects no record the
            // proposer could not. One its proposer's plan refuses is not
            // filed (081).
            ProposedWrite::Records {
                ty,
                filter,
                to,
                only,
                skip_restricted,
                plan,
            } => {
                d.at.sight.ty(ty)?;
                let admit = d.at.sight.admit(ty);
                let pairs = admit.pairs().ok_or_else(Error::not_granted)?;
                *only = pairs.iter().cloned().collect();
                *skip_restricted = !self.sees_restricted();
                let records = (
                    ty.clone(),
                    filter.clone(),
                    to.clone(),
                    only.clone(),
                    *skip_restricted,
                );
                let planned = self.changing(&w, records, (now, 1, None), Against::default())?;
                let section = planned.plan.records.as_deref();
                if let Some(r) = section.and_then(|r| r.refused.first()) {
                    return Err((*r.refusal).clone());
                }
                // Its `writes_only` pairs are held beside them too, so the
                // accept writes no record its proposer could not (006).
                let writes = d.at.sight.writes(d.at.sight.ty(ty)?);
                for (f, v) in writes.pairs().ok_or_else(Error::not_granted)? {
                    granted(only.get(f).is_none_or(|held| held == v))?;
                    only.insert(f.clone(), v.clone());
                }
                **plan = planned.plan;
            }
            // A call of a function above `low` is the store's to propose,
            // when a write makes it due (064).
            ProposedWrite::Compute { .. } | ProposedWrite::ComputeSweep { .. } => granted(false)?,
        }
        // Its plan as its proposer is shown it, for the outcome (062).
        let mut shown = match &write {
            ProposedWrite::Declare { plan, .. } => plan.as_deref().cloned(),
            ProposedWrite::Migrate { migration, report } => {
                let mut types = vec![self.shown(&w, migration, (**report).clone())?];
                types.extend(plan::others_shown(&d.at.sight, std::mem::take(&mut others)));
                Some(plan::plan_of(self.lake, &w, (types, Vec::new()))?)
            }
            ProposedWrite::MigrateValueType { report, .. } => {
                let report = self.shown_value_type(&w.ontology, (**report).clone());
                Some(plan::plan_of(self.lake, &w, (Vec::new(), vec![report]))?)
            }
            ProposedWrite::Records { plan, .. } => Some((**plan).clone()),
            _ => None,
        };
        if let Some(p) = &mut shown {
            plan::shown_plan(p, &d.at.sight, view.as_ref())?;
        }
        // A bundle wholly in force is filed for nothing: its accept would
        // change nothing. It is answered at the head, as `apply` answers it,
        // and appends nothing (027, extended).
        if let Some(sent) = in_force {
            let at = head(&w);
            let mut outcome = with_declared(applied(at, None), &w.ontology, &sent, at.next());
            if let Outcome::Applied { plan, .. } = &mut outcome {
                *plan = shown.map(Box::new);
            }
            return Ok(outcome);
        }
        let d = self.proposal(&w, now, (write, why, pinned), &key_opt);
        drop(view);
        let events = self.lake.append(&mut w, vec![d])?;
        let mut outcome = outcome_of(&events);
        if let Outcome::Proposed { plan, .. } = &mut outcome {
            *plan = shown.map(Box::new);
        }
        Ok(outcome)
    }

    /// Whether a proposal by the principal of `sees` on `decl`, or of a
    /// call of `verb`, is pinned, refusing one that may not be: one by a
    /// holder of `only` or `writes_only` there (051, 006).
    pub(super) fn pin(sees: &Sight<'_>, decl: &TypeDecl, verb: Option<&VerbDecl>) -> Result<bool> {
        let pinned = sees.only_holder(decl) || sees.write_holder(decl);
        granted(!pinned || sees.pinnable(decl, verb))?;
        Ok(pinned)
    }

    /// The proposal as the head's listing shows it to its reviewer, with
    /// whether it holds more ([`crate::reads::Reader::entry`]). One that
    /// is not there to it is answered as an id that does not exist, as its
    /// listing leaves it out, before anything else of the proposal is told
    /// or done (lakelands#762).
    fn listed(
        &self,
        p: &lakelands_api::write::Proposal,
        asked: &Principal,
    ) -> Result<(Option<ProposedWrite>, bool)> {
        self.listed_at(p, asked, None)
    }

    /// [`Self::listed`] as the lake stood at `at`, the head where `None`.
    fn listed_at(
        &self,
        p: &lakelands_api::write::Proposal,
        asked: &Principal,
        at: Option<Position>,
    ) -> Result<(Option<ProposedWrite>, bool)> {
        let Some(write) = &p.write else {
            return Ok((None, false));
        };
        let view = self.lake.engine.view(at)?;
        let shape = self.lake.shape(view.as_ref())?;
        let (sight, at) = (Sight::held(self.who, shape.as_ref()), Default::default());
        let read = crate::reads::Reader::open(self.lake, &sight, (at, None), view, shape.clone())?;
        let missing = Missing::Proposal { id: p.id.clone() };
        let once = (&Default::default(), &Default::default());
        let at = (p.position, p.proposed_at, p.proposer.as_str());
        let listed = read.entry(asked, write, at, once)?;
        listed.ok_or(Error::NotFound { missing })
    }

    /// Refuse a reviewer holding `only` a proposal it is not listed whole:
    /// a change of shape, which it never makes, and a write to a type it
    /// holds `only` on that its listing would trim or withhold (051). A
    /// write that reaches a held record is not listed to one without
    /// `restricted`, so it is not found before this (006, lakelands#1008).
    /// It is judged before the reviewer is told whether the proposal is
    /// decided.
    fn decides(
        &self,
        w: &Writer,
        p: &lakelands_api::write::Proposal,
        listed: &(Option<ProposedWrite>, bool),
    ) -> Result<()> {
        let (o, sees) = (&w.ontology, Sight::held(self.who, &w.ontology));
        let held = sees.holds_only() || sees.holds_writes_only();
        let Some(write) = p.write.as_ref().filter(|_| held) else {
            return Ok(());
        };
        let ty = match write {
            ProposedWrite::Upsert { ty, .. } | ProposedWrite::Retire { ty, .. } => ty,
            ProposedWrite::Call { verb, .. } => match o.verb(verb) {
                Ok(v) => &v.on,
                Err(_) => return granted(false),
            },
            // A change to records its reviewer would select only part of
            // is not its to accept: its `only` on the type is the
            // proposal's pairs or within them (081).
            // So with its `writes_only` (006).
            ProposedWrite::Records { ty, only, .. } => {
                let admit = sees.admit(ty);
                let within =
                    |p: &[(FieldName, Value)]| p.iter().all(|(f, v)| only.get(f) == Some(v));
                let writes = |t: &TypeDecl| sees.writes(t).pairs().is_some_and(within);
                let writes = !sees.holds_writes_only() || o.ty(ty).is_ok_and(writes);
                return granted(admit.pairs().is_some_and(within) && writes);
            }
            _ => return granted(false),
        };
        if sees.holds_only() && !o.ty(ty).is_ok_and(|t| !sees.only_holder(t)) {
            granted(listed.0.is_some() && !listed.1)?;
        }
        match o.ty(ty) {
            Ok(t) if sees.write_holder(t) => granted(self.writes_proposed(w, p, write, &sees, t)?),
            _ => Ok(()),
        }
    }

    /// Whether a reviewer holding `writes_only` on `decl` could make the
    /// proposed write itself, as the head plans it: the record at its key,
    /// live or retired, and the one it leaves hold its pairs (006). Its
    /// accept judges the record the write leaves again, as it is made.
    fn writes_proposed(
        &self,
        w: &Writer,
        p: &lakelands_api::write::Proposal,
        write: &ProposedWrite,
        sees: &Sight<'_>,
        decl: &TypeDecl,
    ) -> Result<bool> {
        let (o, view) = (&w.ontology, self.lake.engine.view(None)?);
        let settings = crate::lake::settings_in(view.as_ref(), o)?;
        let at = (view.as_ref(), &settings, clock(w));
        let (key, patch) = match write {
            ProposedWrite::Upsert { key, patch, .. } => (Some(key), Some(patch.clone())),
            ProposedWrite::Retire { key, .. } => (Some(key), None),
            ProposedWrite::Call {
                verb, key, payload, ..
            } => {
                let v = o.verb(verb)?;
                let sets = matches!(v.effect, VerbEffect::Write { .. });
                let patch = sets.then(|| call_reads(o, at, v, key.as_ref(), payload));
                (key.as_ref(), patch.transpose()?.map(|(patch, _)| patch))
            }
            _ => return Ok(false),
        };
        let Some(key) = key else {
            return Ok(sees.writes_record(decl, None, None));
        };
        let before = view.get(&decl.name, key)?;
        let after = match &patch {
            Some(patch) => {
                let made = (&decl.name, key, p.proposer.as_str());
                planned(self.lake, o, at, before.as_ref(), made, patch)?
            }
            None => None,
        };
        let fields = |r: &Option<StoredRecord>| r.as_ref().map(|r| &r.fields).cloned();
        let (before, after) = (fields(&before), fields(&after));
        Ok(sees.writes_record(decl, before.as_ref(), after.as_ref()))
    }

    /// Refuse the accept of a pinned proposal, the one logged at `at`,
    /// once the record it writes or its type changed after it, or, for a
    /// link, an end record, as deep as its ends go, or a record its call
    /// reads across a reference: its proposer was judged on them as they
    /// stood (051).
    fn unmoved(&self, w: &Writer, view: &dyn ProjectionView, at: Position) -> Result<()> {
        let o = &w.ontology;
        let logged = self.lake.read_events(at, 1)?;
        let Some(EventBody::WriteProposed {
            write,
            pinned: true,
            ..
        }) = logged.first().map(|e| &e.body)
        else {
            return Ok(());
        };
        let mut far = Vec::new();
        let (ty, key) = match write {
            ProposedWrite::Upsert { ty, key, .. } | ProposedWrite::Retire { ty, key, .. } => {
                (ty, Some(key))
            }
            ProposedWrite::Call {
                verb, key, payload, ..
            } => {
                // The records it read across then and those it reads now: a
                // condition a setting or the clock decides may come to a
                // record at one and not at the other.
                let (v, now) = (o.verb(verb)?, clock(w));
                far = call_reads(o, (view, &w.settings, now), v, key.as_ref(), payload)?.1;
                far.extend(read_then(
                    self.lake,
                    &logged[0],
                    verb,
                    key.as_ref(),
                    payload,
                )?);
                (&v.on, key.as_ref())
            }
            _ => return Ok(()),
        };
        let declared = self.lake.engine.view(Some(at))?.declarations()?;
        let then = declared.iter().find_map(|d| match &d.declaration {
            Declaration::Type(t) if &t.name == ty => Some(d.version),
            _ => None,
        });
        let moved_at = |ty: &TypeName, key: &RecordKey| -> Result<bool> {
            Ok(view.get(ty, key)?.is_some_and(|r| r.position > at))
        };
        // A link is its holder's by its end records too, and a change to
        // an end moves no link.
        let mut moved = false;
        let own = key.iter().map(|k| (ty.clone(), (*k).clone()));
        for (t, k) in own.chain(far).flat_map(|(t, k)| with_ends(o, &t, &k)) {
            moved = moved || moved_at(&t, &k)?;
        }
        if !moved && then == o.types.get(ty).map(|e| e.version) {
            return Ok(());
        }
        let (ty, key) = (ty.clone(), key.cloned());
        Err(Error::invalid(Case::WriteChangedSinceProposed { ty, key }))
    }

    /// `accept`: apply a pending proposal's write, written as the proposer.
    pub fn accept(
        &self,
        proposal: &ProposalId,
        asked: &Principal,
        plan_id: Option<&str>,
    ) -> Result<(Outcome, Option<External>)> {
        check_context(self.lake, &self.context, self.who)?;
        granted(self.who.grants.review)?;
        let mut w = self.writer()?;
        let view = self.lake.engine.view(None)?;
        let p = find_proposal(view.as_ref(), proposal)?;
        // A call whose verb is withdrawn since is judged by the lake as it
        // stood where it was proposed, as a withdrawal is: one not listed it
        // there is answered as for an id that does not exist, before anything
        // else is told; to one listed it whole it is a call of a verb not
        // declared, as a direct call is (027).
        if p.state == ProposalState::Pending
            && let Some(ProposedWrite::Call { verb, .. }) = &p.write
            && w.ontology.verb(verb).is_err()
            && let (Some(_), false) = self.listed_at(&p, asked, Some(p.position))?
        {
            let name = verb.clone();
            return Err(Error::NotFound {
                missing: Missing::Verb { name },
            });
        }
        let listed = self.listed(&p, asked)?;
        // Before a retry is answered: an accept sent again would hand a
        // holder of `only` what an earlier accepter was answered (051).
        self.decides(&w, &p, &listed)?;
        // It accepts only what it is listed whole, as a reviewer without
        // the grant would be answered; listed in part, it may reject
        // (051, lakelands#867).
        granted(listed.0.is_some() && !listed.1)?;
        if p.state != ProposalState::Pending {
            let again = self.accepted_again(&w, view.as_ref(), proposal)?;
            return Ok((again, None));
        }
        // A change of the ontology is the lake's `schema` grant's to make:
        // the accepter holds it as well as `review` (013, extended).
        // A stored proposal holds its write; a row without one is a
        // projection the store cannot use.
        let Some(write) = p.write.as_ref() else {
            let flaw = Flaw::ProposalWithoutWrite {
                proposal: proposal.clone(),
            };
            return Err(lakelands_core::damaged(Part::Projection, None, flaw));
        };
        granted(!write.is_schema() || self.who.grants.schema)?;
        // A change to records whose proposer held `restricted` is accepted
        // only by a holder of it: one without would leave out what its
        // proposer selected (081).
        if let ProposedWrite::Records {
            skip_restricted: false,
            ..
        } = write
        {
            granted(self.sees_restricted())?;
        }
        // A change to records is accepted against the plan its reviewer is
        // shown, as its reviewer's own `enact` makes it (081).
        if let (ProposedWrite::Records { .. }, None) = (write, plan_id) {
            return Err(Error::invalid(Case::PlanStale));
        }
        // And `restricted`, as its act's own door needs (062).
        if write.is_schema() {
            self.restricted_to_act()?;
        }
        // Accepted against a plan: the plan of its change made now, as the
        // reviewer is shown it, by `plan`'s own path, is that one (062); a
        // change to records is compared where it is made, below.
        let records = matches!(write, ProposedWrite::Records { .. });
        if let Some(id) = plan_id.filter(|_| !records) {
            let Some(change) = write.change() else {
                let proposal = proposal.clone();
                return Err(Error::invalid(Case::ProposalPlansNothing { proposal }));
            };
            let against = Against {
                plan: Some(id),
                report: None,
            };
            self.planned(&mut w, change, When::Plan, against)?;
        }
        // A proposed call is accepted while the record still makes it: the
        // reviewer judged those inputs. The fold leaves the call owed, made
        // after the append with the accept as its cause (064).
        if let ProposedWrite::Compute {
            ty,
            key,
            field,
            input,
            ..
        } = write
        {
            let then = self.lake.engine.view(Some(p.position))?.get(ty, key)?;
            let now = view.get(ty, key)?;
            let made = crate::backed::call_now(self.lake, &w, view.as_ref(), ty, now.as_ref())?;
            if !made.iter().any(|(f, i)| f == field && i == input) {
                let version = |r: Option<&lakelands_api::record::StoredRecord>| {
                    r.map_or(Version(0), |r| r.version)
                };
                let (expected, actual) = (version(then.as_ref()), version(now.as_ref()));
                return Err(Error::Conflict { expected, actual });
            }
        }
        if matches!(
            write,
            ProposedWrite::Compute { .. } | ProposedWrite::ComputeSweep { .. }
        ) {
            let now = self.lake.clock(&mut w);
            let accepted = Draft {
                id: Some(log::accepted_id(&self.lake.id, proposal)),
                ..self.draft(
                    now,
                    Origin::Authored,
                    EventBody::WriteAccepted {
                        proposal: proposal.clone(),
                    },
                )
            };
            drop(view);
            let events = self.lake.append(&mut w, vec![accepted])?;
            return Ok((outcome_of(&events), None));
        }
        self.unmoved(&w, view.as_ref(), p.position)?;
        let now = self.lake.clock(&mut w);
        // The write was checked under the proposer's grants when it was
        // proposed; it is applied as the proposer, with what was checked.
        // Its `restricted` grant is not logged: it held it where the write
        // reached a record restricted then, and else a record restricted
        // since is not there to the accept (006).
        let mut proposer = self.lake.system(p.proposer.clone());
        proposer.grants.restricted = held_then(self.lake, &p, write)?;
        let accepted = Draft {
            id: Some(log::accepted_id(&self.lake.id, proposal)),
            ..self.draft(
                now,
                Origin::Authored,
                EventBody::WriteAccepted {
                    proposal: proposal.clone(),
                },
            )
        };
        let accepted_at = position_after(&w, 0);
        let mut drafts = vec![accepted];
        // Whom the proposer wrote for goes with it, never the accepter's
        // (088).
        let made_for = proposed_for(self.lake, &p)?;
        let as_accepted = |mut d: Draft| {
            if d.origin == Origin::Authored {
                d.writer = p.proposer.clone();
                d.context.on_behalf_of = made_for.clone();
            }
            d.cause = d.cause.or(Some(accepted_at));
            d
        };
        let (mut external, mut renamed) = (None, Vec::new());
        // The sections of the plan that applies, for a change of the
        // ontology, as the reviewer is shown them (062).
        let mut sections = (Vec::new(), Vec::new());
        let mut records_plan = None;
        // The proposer's events are written as the proposer and caused by
        // the acceptance before their record is computed.
        let mut d = self.decide(&w, view.as_ref(), now, 1);
        d.at.sight = Sight::held(&proposer, &w.ontology);
        (d.envelope.writer, d.envelope.cause) = (Some(p.proposer.clone()), Some(accepted_at));
        d.envelope.on_behalf_of = made_for.clone();
        // What the write leaves is judged by the reviewer's pairs too: a
        // holder of `only` accepts no write that takes a record out of its
        // reach, as it writes none (051). A refusal of the plan is told as
        // the reviewer's listing shows the write, whatever the op.
        // A holder of `writes_only` accepts one only where the record at
        // the key and the one it leaves hold its pairs (006).
        let mut run = |op: Op<'_>, opts: &WriteOpts| -> Result<Vec<Draft>> {
            let told = |e| self.told(&w.ontology, write, listed.0.as_ref(), e);
            let (o, sees) = (&w.ontology, Sight::held(self.who, &w.ontology));
            let at = match op {
                Op::Upsert(ty, key, _) | Op::Retire(ty, key) => Some((ty, Some(key))),
                Op::Call(v, key, _) => Some((&v.on, key)),
                _ => None,
            };
            let held = at.and_then(|(ty, key)| Some((o.ty(ty).ok()?, key)));
            let held = held.filter(|(decl, _)| sees.write_holder(decl));
            let before = match held {
                Some((decl, Some(key))) => d.pending.record(view.as_ref(), &decl.name, key)?,
                _ => None,
            };
            let plan = d.plan(op, opts.expected_version).map_err(told)?;
            if let Some(after) = &plan.after {
                let decl = o.ty(&after.ty)?;
                let reach = owns(view.as_ref(), &d.pending, &sees, decl, &after.fields)?;
                granted(reach)?;
            }
            if let Some((decl, _)) = held {
                let after = plan.after.as_ref().map(|r| &r.fields);
                granted(sees.writes_record(decl, before.as_ref().map(|r| &r.fields), after))?;
            }
            Ok(plan.drafts)
        };
        match write {
            ProposedWrite::Upsert {
                ty,
                key,
                patch,
                opts,
            } => drafts.extend(run(Op::Upsert(ty, key, patch), opts)?),
            ProposedWrite::Retire { ty, key, opts } => {
                drafts.extend(run(Op::Retire(ty, key), opts)?)
            }
            ProposedWrite::Call {
                verb,
                key,
                payload,
                opts,
            } => {
                let decl = w.ontology.verb(verb)?.clone();
                // The verb may have been declared again since: its scope at
                // the head must still fit the proposal, or the accept is
                // refused and the proposal stays pending.
                let key = scoped(&decl, key.as_ref(), opts.expected_version)?;
                // The reviewer applies what was checked: a verb declared
                // differently since the call was proposed refuses the
                // accept, and the proposal stays pending (013).
                let then = self.lake.engine.view(Some(p.position))?.declarations()?;
                if !then
                    .iter()
                    .any(|d| matches!(&d.declaration, Declaration::Verb(v) if *v == decl))
                {
                    let verb = verb.clone();
                    return Err(Error::invalid(Case::VerbRedeclared { verb }));
                }
                if let VerbEffect::External { executor } = &decl.effect {
                    bound_executor(self.lake, executor)?;
                    external = Some(External {
                        executor: executor.clone(),
                        verb: verb.clone(),
                        ty: decl.on.clone(),
                        key: key.cloned(),
                        payload: payload.clone(),
                        position: accepted_at,
                    });
                }
                drafts.extend(run(Op::Call(&decl, key, payload), opts)?);
            }
            // Checked again on the head, which may have moved: a change it
            // no longer admits refuses the accept, and the proposal stays
            // pending. The proposer writes what it declares; the link types
            // the store derives stay the system's.
            ProposedWrite::Declare { declarations, .. } => {
                // One declared since under a name the bundle declares was
                // shown to no one at the proposal; the bundle would take
                // its place as its proposer's (lakelands#915).
                let key = lakelands_core::engines::declaration_key;
                let since = view.declarations()?;
                let since = |d: &Declaration| {
                    let mut held = since.iter().filter(|x| x.since > p.position);
                    !w.ontology.in_force(d) && held.any(|x| key(&x.declaration) == key(d))
                };
                if let Some(d) = declarations.iter().find(|d| since(d)) {
                    let (what, name) = naming(d);
                    return Err(Error::invalid(Case::DeclaredSinceProposed { what, name }));
                }
                // A withdrawal something reads is refused as its reviewer is
                // shown the lake: naming a reader it is shown, and not
                // granted where only what it is not shown reads it (024).
                let read = |d: &&Declaration| matches!(d, Declaration::Withdraw(x) if declare::reader(&w.ontology, x).is_some());
                let read: Vec<Declaration> = declarations.iter().filter(read).cloned().collect();
                // So a function whose input's kind a binding it is not shown
                // holds otherwise: the refusal would name it. The lake as its
                // reviewer is shown it is made only where one of them asks.
                let binds = |d: &Declaration| matches!(d, Declaration::Function(f) if declare::input_kind_broken(&w.ontology, f).is_some());
                if !read.is_empty() || declarations.iter().any(binds) {
                    let mut seen = types_shown(self.who, &w.ontology);
                    retain_listed(&mut seen, &Sight::held(self.who, &w.ontology.declared()));
                    if !read.is_empty() {
                        let shown = |d: &Declaration| matches!(d, Declaration::Withdraw(x) if declare::reader(&seen, x).is_some());
                        granted(read.iter().all(shown))?;
                        self.check_set((&seen, &w.ontology), &read, false)?;
                    }
                    let unseen = |d: &Declaration| unseen_binding(&w.ontology, &seen, d);
                    granted(!declarations.iter().any(unseen))?;
                }
                self.check_bundle(&w.ontology, declarations)?;
                let mut changed = declarations.clone();
                changed.retain(|d| !w.ontology.in_force(d));
                let own = self.bundle_drafts(&w, changed, (now, drafts.len(), Judging::Act))?;
                sections.0 = own.types;
                drafts.extend(own.drafts.into_iter().map(&as_accepted));
            }
            ProposedWrite::Migrate { migration, report } => {
                // The shape is the type's whole: on a type changed since the
                // proposal it would remove what was added, shown to no one
                // (013, extended).
                let ty = &migration.to.name;
                let head = shape::schema_hash(w.ontology.ty(ty)?).ok();
                let changed = || Error::invalid(Case::ChangedSinceProposed { ty: ty.clone() });
                if head.as_ref() != Some(&report.from_schema_hash) {
                    return Err(changed());
                }
                // Judged as its author's at the proposal: the lake's own
                // names are judged again.
                let judged = (&**migration, None, Some(&proposer));
                let plan =
                    self.preflight_plan(&w, view.as_ref(), judged, (Some(now), Judging::Act))?;
                // So would it take away a mark set since, which the hash
                // does not cover: it restates what its report said, no more.
                if plan.report.restated != report.restated {
                    return Err(changed());
                }
                let sight = Sight::held(self.who, &w.ontology);
                renamed = Self::known_renames(&sight, &w.ontology, migration);
                sections.0 = vec![self.shown(&w, migration, plan.report.clone())?];
                (sections.0).extend(plan::others_shown(&sight, plan.others.clone()));
                let (first, same) = (drafts.len(), unchanged(&w.ontology, migration, &plan));
                let own = self.migration_drafts(&w, (**migration).clone(), plan, now, first)?;
                let own = own.into_iter().filter(|_| !same);
                drafts.extend(own.map(&as_accepted));
            }
            ProposedWrite::MigrateValueType {
                to,
                renames,
                report,
            } => {
                // As a migration is accepted only on the shape it was
                // proposed on: a value type declared again since would lose
                // what that added, shown to no one (013, extended).
                let head = w.ontology.value_types.get(to.name.as_str());
                let changed = || {
                    let value_type = to.name.clone();
                    Error::invalid(Case::ValueTypeChangedSinceProposed { value_type })
                };
                if head.map(|v| v.version) != Some(report.from_version) {
                    return Err(changed());
                }
                let m = ValueTypeMigration {
                    to: (**to).clone(),
                    renames: renames.clone(),
                };
                let plan =
                    self.preflight_value_type(&w, view.as_ref(), &m, (Some(now), Judging::Act))?;
                // Renames reach only the fields the proposal's report named:
                // one that came to use the value type since would be
                // rewritten unseen by the proposer and the reviewer.
                let uses = |r: &ValueTypeReport| {
                    let fields = r.fields.iter();
                    fields
                        .map(|u| (u.ty.clone(), u.field.clone()))
                        .collect::<Vec<_>>()
                };
                if !renames.is_empty() && uses(&plan.report) != uses(report) {
                    return Err(changed());
                }
                fits(plan.report.clone(), false)?;
                sections.1 = vec![self.shown_value_type(&w.ontology, plan.report.clone())];
                let own = self.value_type_drafts(&w, m, plan, now, drafts.len())?;
                drafts.extend(own.into_iter().map(&as_accepted));
            }
            // Selected and judged as its reviewer's `enact` would, within
            // its proposer's pairs and restricted records left out where
            // they were, written as its proposer and caused by the
            // acceptance; refused `plan_stale` where the plan made now is
            // not the one the reviewer was shown, and the proposal stays
            // pending (081).
            ProposedWrite::Records {
                ty,
                filter,
                to,
                only,
                skip_restricted,
                ..
            } => {
                let records = (
                    ty.clone(),
                    filter.clone(),
                    to.clone(),
                    only.clone(),
                    *skip_restricted,
                );
                let envelope = Envelope {
                    writer: Some(p.proposer.clone()),
                    on_behalf_of: made_for.clone(),
                    cause: Some(accepted_at),
                    ..Envelope::of(Origin::Authored)
                };
                let against = Against {
                    plan: plan_id,
                    report: None,
                };
                let at = (now, drafts.len(), Some(envelope));
                let planned = self.changing(&w, records, at, against)?;
                drafts.extend(planned.drafts);
                records_plan = Some(planned.plan);
            }
            ProposedWrite::Compute { .. } | ProposedWrite::ComputeSweep { .. } => {
                unreachable!("a proposed call is accepted above")
            }
        }
        drop(view);
        let appended = self.lake.append(&mut w, drafts);
        let by = MadeDue::Accepted;
        let events = appended.map_err(|e| not_handed(e, external.is_some() && w.behind, by))?;
        let outcome = self.answered(&events, renamed);
        let mut outcome = self.told_of(&w.ontology, &events, outcome, None)?;
        // Each declaration of the bundle as it stands after the accept, as
        // `apply` answers it (027, extended).
        if let ProposedWrite::Declare { declarations, .. } = write {
            outcome = with_declared(outcome, &w.ontology, declarations, accepted_at);
        }
        if let (true, Outcome::Applied { plan, .. }) = (write.is_schema(), &mut outcome) {
            *plan = Some(Box::new(plan::plan_of(self.lake, &w, sections)?));
        }
        if let (Some(p), Outcome::Applied { plan, .. }) = (records_plan, &mut outcome) {
            *plan = Some(Box::new(p));
        }
        Ok((outcome, external))
    }

    /// A refusal of the proposer's plan as an accept answers its reviewer
    /// (lakelands#780), judged by the reviewer's view as its listing is:
    /// whole where the listing shows the reviewer all it names, and else
    /// as a proposal stale since it was proposed, which names only what
    /// the listing does. A conflict names versions only. Its proposer is
    /// judged as any reviewer, by its grants at the accept: it hears what
    /// its own write would tell it now (lakelands#791).
    fn told(
        &self,
        o: &Ontology,
        write: &ProposedWrite,
        listed: Option<&ProposedWrite>,
        e: Error,
    ) -> Error {
        let (ty, key) = match write {
            ProposedWrite::Upsert { ty, key, .. } | ProposedWrite::Retire { ty, key, .. } => {
                (ty.clone(), Some(key.clone()))
            }
            ProposedWrite::Call { verb, key, .. } => match o.verb(verb) {
                Ok(v) => (v.on.clone(), key.clone()),
                Err(_) => return e,
            },
            _ => return e,
        };
        let judged = matches!(e, Error::Refused { .. } | Error::NotFound { .. });
        let sight = Sight::held(self.who, o);
        // The verbs a refusal names are those its reviewer is shown and may
        // call, as its own write would name them, else none: a verb kept
        // from it is told by nothing (lakelands#1130, lakelands#1148).
        let e = match e {
            Error::Refused {
                field,
                reason: Refusal::VerbsOnly { verbs },
            } => {
                let shown = |v: &VerbName| o.verb(v).is_ok_and(|d| sight.verb_shown(d));
                let verbs: Vec<VerbName> = verbs.into_iter().filter(shown).collect();
                let reason = match verbs.is_empty() {
                    true => Refusal::WrongDoor,
                    false => Refusal::VerbsOnly { verbs },
                };
                Error::refused(field, reason)
            }
            e => e,
        };
        if !judged || sight.holds_all() {
            return e;
        }
        let read = |f: &FieldName| sight.reads_from(&ty, f);
        let sees = |t: &TypeName| sight.ty(t).is_ok();
        // All a verb requires, as one `and`; one that does not parse as itself.
        let requires = |v: &VerbName| {
            let v = o.verb(v).ok()?;
            let typed = v.conditions().map(|c| c.typed().map_err(|_| c.clone()));
            let all = typed.collect::<std::result::Result<Vec<_>, _>>();
            Some(all.map_or_else(|c| c, |args| lakelands_api::expr::Expr::And { args }.into()))
        };
        let mut named = Named::default();
        e.names(&mut named);
        // A constraint is told to one who reads every field it reads, as
        // `describe_type` tells it (lakelands#1119).
        let constraints = o
            .ty(&ty)
            .map(|t| t.constraints.as_slice())
            .unwrap_or_default();
        let told = |n: &ConstraintName| {
            let c = constraints.iter().find(|c| &c.name == n);
            let e = c.and_then(|c| c.holds.typed().ok());
            e.is_some_and(|e| e.fields_read().into_iter().all(read))
        };
        if named.constraints.iter().all(told) {
            named.constraints.clear();
        }
        match listed {
            Some(l) if named.shown_by(l, &sees, &read, &requires) => e,
            _ => Error::invalid(Case::WriteChangedSinceProposed { ty, key }),
        }
    }

    /// A migration's renames as the principal of `sees` may know of them,
    /// taken before the append, while the type stands as it was in `o`.
    pub(super) fn known_renames(
        sees: &Sight<'_>,
        o: &Ontology,
        m: &Migration,
    ) -> Vec<(FieldName, FieldName)> {
        let mut m = m.clone();
        if let Ok(was) = o.ty(&m.to.name) {
            sees.known_migration(was, &mut m, &|n| o.ty(n).ok());
        }
        m.renames
    }

    /// [`outcome_of`], with the fields the migration it applied renamed.
    pub(super) fn answered(&self, events: &[Event], known: Vec<(FieldName, FieldName)>) -> Outcome {
        let mut outcome = outcome_of(events);
        if let Outcome::Applied { renamed, .. } = &mut outcome {
            *renamed = known;
        }
        outcome
    }

    /// `reject`.
    pub fn reject(&self, proposal: &ProposalId, asked: &Principal, why: String) -> Result<Outcome> {
        check_context(self.lake, &self.context, self.who)?;
        granted(self.who.grants.review)?;
        let mut w = self.writer()?;
        let view = self.lake.engine.view(None)?;
        let p = find_proposal(view.as_ref(), proposal)?;
        self.decides(&w, &p, &self.listed(&p, asked)?)?;
        if p.state != ProposalState::Pending {
            return Err(Error::refused(None, Refusal::Decided));
        }
        let now = self.lake.clock(&mut w);
        // A rejected call that its own record no longer makes is recorded
        // for that record all the same, under the digest it was proposed
        // with: the rejection holds for the record should it make the call
        // again, and none is left to carry it out (064).
        let moved = match p.write.as_ref() {
            Some(ProposedWrite::Compute {
                ty,
                key,
                field,
                function,
                input,
                ..
            }) => {
                let now_record = view.get(ty, key)?;
                let made =
                    crate::backed::call_now(self.lake, &w, view.as_ref(), ty, now_record.as_ref())?;
                let still = made.iter().any(|(f, i)| f == field && i == input);
                let f = w.ontology.functions.get(function).filter(|_| !still);
                f.map(|f| {
                    let procedure = lakelands_api::event::Procedure {
                        name: function.clone(),
                        version: f.version.to_string(),
                    };
                    let body = EventBody::OutputRecorded {
                        seam: crate::backed::seam(&f.decl),
                        input: input.clone(),
                        output: Some(crate::backed::rejected(&why)),
                        output_at: None,
                        field: Some((ty.clone(), key.clone(), field.clone())),
                    };
                    (procedure, body)
                })
            }
            _ => None,
        };
        let d = self.draft(
            now,
            Origin::Authored,
            EventBody::WriteRejected {
                proposal: proposal.clone(),
                reason: why,
            },
        );
        let mut drafts = vec![d];
        if let Some((procedure, body)) = moved {
            drafts.push(Draft {
                cause: Some(position_after(&w, 0)),
                procedure: Some(procedure),
                ..Draft::system(now, body)
            });
        }
        drop(view);
        let events = self.lake.append(&mut w, drafts)?;
        Ok(outcome_of(&events))
    }

    /// A function computed outside the store is declared only where a
    /// backing of its kind is bound; replay never asks. An embedding is
    /// compared with the one `in_force` holds, where there is one to
    /// compare with: across a migration's renames, with the field or the
    /// relation it is renamed from, whose vectors it carries.
    pub(super) fn check_bound(
        &self,
        d: &Declaration,
        in_force: Option<&Ontology>,
        m: Option<&Migration>,
    ) -> Result<()> {
        // A trigger's ceiling lies within the lake's (021).
        if let Declaration::Trigger(t) = d
            && let Some(m) = t.max_depth
            && m > self.lake.limits.max_reactive_depth
        {
            let (depth, max) = (m, self.lake.limits.max_reactive_depth);
            let said = Said::from(Case::TriggerTooDeep { depth, max });
            let name = t.name.clone();
            return Err(Error::invalid(said.within(Place::Trigger { name })));
        }
        let backings = &self.lake.seams.backings;
        if let Declaration::Function(f) = d
            && crate::backed::is_backed(f)
            && !backings.iter().any(|b| b.kind() == f.backing)
        {
            return Err(Error::unbound(Seam::FunctionBacking {
                kind: f.backing.clone(),
            }));
        }
        if let Declaration::Type(t) = d {
            let renames = m.map_or(&[][..], |m| &m.renames);
            self.check_embeddings(t, in_force, (&t.name, renames))?;
            // A relation's fields embed on its link type's records.
            let moved = m.map(links::moved).unwrap_or_default();
            for lt in links::link_types_of(t) {
                let was = moved.iter().find(|(_, new)| **new == lt.name);
                let was = was.map_or(&lt.name, |(old, _)| old);
                self.check_embeddings(&lt, in_force, (was, &[]))?;
            }
        }
        Ok(())
    }

    /// A type's embeddings are declared only where the bound embedding
    /// backend names their model and makes vectors of their dimensions,
    /// unless `in_force` holds the field with the same embedding: one
    /// declared unchanged makes nothing due, so a lake with no backend
    /// bound can still restate or grow the type around it. `named` and
    /// `renames` give the type and its fields as `in_force` holds them,
    /// before a migration renames them.
    fn check_embeddings(
        &self,
        t: &TypeDecl,
        in_force: Option<&Ontology>,
        (named, renames): (&TypeName, &[(FieldName, FieldName)]),
    ) -> Result<()> {
        // The name a field holds in force: the one it is renamed from, or
        // its own, unless a rename frees that name and `to` declares it
        // anew, a field with none in force.
        let old = |n: &FieldName| match renames.iter().find(|(_, new)| new == n) {
            Some((old, _)) => Some(old.clone()),
            None => (!renames.iter().any(|(old, _)| old == n)).then(|| n.clone()),
        };
        let (bound, mut was) = (self.lake.embed(), None);
        for f in &t.fields {
            let Some(e) = &f.embed else {
                continue;
            };
            let dimensions = f.ty.dimensions.unwrap_or(0);
            if bound
                .as_ref()
                .is_some_and(|b| b.model() == e.model && b.dimensions() == dimensions)
            {
                continue;
            }
            let was = was.get_or_insert_with(|| in_force.and_then(|o| o.as_declared(named).ok()));
            let kept = was
                .iter()
                .flat_map(|w| &w.fields)
                .find(|k| Some(&k.name) == old(&f.name).as_ref());
            let from: Option<Vec<FieldName>> = e.from.iter().map(old).collect();
            let same = |k: &EmbedDecl| k.model == e.model && from.as_ref() == Some(&k.from);
            if kept.is_some_and(|k| {
                k.ty.dimensions == f.ty.dimensions && k.embed.as_ref().is_some_and(same)
            }) {
                continue;
            }
            let bound = bound.map(|b| lakelands_api::error::Bound {
                model: b.model().to_owned(),
                dimensions: b.dimensions(),
            });
            return Err(Error::unbound(Seam::Embedding {
                model: e.model.clone(),
                dimensions,
                ty: t.name.clone(),
                field: f.name.to_string(),
                bound,
            }));
        }
        Ok(())
    }

    /// `declare`.
    pub fn declare(&self, declaration: Declaration) -> Result<Outcome> {
        self.enacting()?;
        self.declare_granted(declaration)
    }
}

/// A write's clock, never before the last `known_at`.
fn clock(w: &Writer) -> Timestamp {
    crate::lake::wall_clock().max(w.last_known_at)
}

/// Whom the proposer of `p` acted for, as its `write_proposed` names it
/// (088).
fn proposed_for(lake: &Lake, p: &lakelands_api::write::Proposal) -> Result<Option<String>> {
    let stored = lake.engine.read(p.position, 1)?;
    match stored.first().map(log::open).transpose()? {
        Some(e) if e.position == p.position => Ok(e.on_behalf_of),
        _ => Ok(None),
    }
}
