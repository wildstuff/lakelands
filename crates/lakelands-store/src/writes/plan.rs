//! A change to the ontology plans before it acts
//! (`docs/decisions/062-a-change-plans-before-it-acts.md`). Every door that
//! changes the ontology is its plan, made on the act's own path up to its
//! append, and then the append: `plan` answers the plan alone, and so
//! refuses what the act refuses; `enact` and every other such door append
//! it, and answer it in their outcome. A plan id, where one is given, is
//! compared before anything that a lossy change is refused for, as a
//! stale report is.

use lakelands_api::write::{Change as Changed, Plan};

use super::*;

/// Whether a change is planned only, or planned and made.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum When {
    /// Planned, appending nothing: at the lake's last time.
    Plan,
    /// Planned and made, at the time the clock reads now.
    Act,
}

/// The ids a change is made against: its plan's (`enact`), or its report's
/// (`migrate_apply`, `migrate_value_type_apply`). Either refused where the
/// change planned now is another.
#[derive(Clone, Copy, Default)]
pub(crate) struct Against<'a> {
    pub(crate) plan: Option<&'a str>,
    pub(crate) report: Option<&'a str>,
}

/// A change as its plan leaves it to its act.
pub(crate) struct Planned {
    /// Its plan, as its author is shown it.
    pub(crate) plan: Plan,
    pub(super) drafts: Vec<Draft>,
    /// Whether nothing is appended: it is answered at the head.
    pub(super) nothing: bool,
    /// How its outcome reads what it appended.
    pub(super) answer: Answer,
}

/// How a change's outcome reads what it appended.
pub(super) enum Answer {
    /// Each declaration sent, as it stands in force after the call: at
    /// the position of its first event (`declare`), or of its last
    /// (`apply`), from which all of it is in force.
    Declared(Vec<Declaration>, bool),
    /// The fields a migration renamed, as its author may know of them.
    Migrated(Vec<(FieldName, FieldName)>),
    /// What it appended, alone.
    Plain,
    /// The position of its last event: a change to records (081).
    Last,
}

/// A plan's id: named by what it says to its reader and where the lake
/// stood, as a report's is ([`report_id`]).
pub(crate) fn plan_id(plan: &Plan) -> String {
    let plan = Plan {
        report_id: String::new(),
        ..plan.clone()
    };
    lakelands_core::canonical::sha256_hex(&serde_json::json!({
        "plan": serde_json::to_value(&plan).expect("a plan serialises"),
        "head": plan.at.0,
    }))
}

/// The sections of the other types whose references a re-key rewrites, as
/// `sight` is shown them: per type it is shown, the reference fields it
/// reads whose target type it is shown, and none left with no field
/// (062).
pub(crate) fn others_shown(
    sight: &Sight<'_>,
    others: Vec<MigrationReport>,
) -> impl Iterator<Item = MigrationReport> {
    others.into_iter().filter_map(move |mut t| {
        let decl = sight.ty(&t.ty).ok()?;
        let target = |f: &FieldName| (decl.field(f))?.ty.target.clone();
        let shows = |f: &FieldName| target(f).is_some_and(|x| sight.ty(&x).is_ok());
        t.unlive
            .retain(|u| sight.reads_from(&t.ty, &u.field) && shows(&u.field));
        t.more = (t.unlive.iter()).any(|u| u.records > u.sample_keys.len() as u64);
        (!t.unlive.is_empty()).then_some(t)
    })
}

/// A plan of `types` and `value_types`, at the head of `w`, its id made.
pub(crate) fn plan_of(
    w: &Writer,
    (types, value_types): (Vec<MigrationReport>, Vec<ValueTypeReport>),
) -> Plan {
    let lossless = types.iter().all(|t| t.lossless) && value_types.iter().all(|v| v.fits);
    let mut plan = Plan {
        report_id: String::new(),
        at: head(w),
        types,
        value_types,
        lossless,
        records: None,
    };
    plan.report_id = plan_id(&plan);
    plan
}

impl Call<'_> {
    /// `plan`.
    pub fn plan(&self, change: Changed) -> Result<lakelands_api::read::Reply<Plan>> {
        check_context(self.lake, &self.context)?;
        let mut w = self.writer()?;
        let planned = self.planned(&mut w, change, When::Plan, Against::default())?;
        Ok(lakelands_api::read::Reply {
            position: head(&w),
            data: planned.plan,
        })
    }

    /// `enact`: the change made against the plan its caller was shown.
    pub fn enact(&self, change: Changed, plan_id: &str) -> Result<Outcome> {
        // A change to records is its caller's own writes (081).
        if !matches!(change, Changed::Records { .. }) {
            self.enacting()?;
        }
        let against = Against {
            plan: Some(plan_id),
            report: None,
        };
        self.make(change, against)
    }

    /// A change of the lake's types is made by a holder of `schema` and
    /// `restricted` alone, refused before anything is read (062): its act
    /// judges every record, so what it refuses for a record held from its
    /// caller would tell of that record.
    pub(crate) fn enacting(&self) -> Result<()> {
        check_context(self.lake, &self.context)?;
        granted(self.authors())?;
        self.restricted_to_act()
    }

    /// Refused `grant_needed` to one without `restricted`, whatever the
    /// lake holds (062).
    pub(crate) fn restricted_to_act(&self) -> Result<()> {
        match self.sees_restricted() {
            true => Ok(()),
            false => Err(Error::refused(
                None,
                Refusal::GrantNeeded {
                    grant: "restricted".into(),
                },
            )),
        }
    }

    /// How a change is judged `when`: made, as an act; planned or
    /// previewed for one who may not make it, only telling, on the records
    /// it is told of, as a proposal's plan (062).
    pub(crate) fn judging(&self, when: When) -> Judging {
        match when == When::Plan && !self.sees_restricted() {
            true => Judging::Proposal,
            false => Judging::Act,
        }
    }

    /// A change made: planned now, compared with what it is made against,
    /// appended, and answered with its plan.
    pub(crate) fn make(&self, change: Changed, against: Against<'_>) -> Result<Outcome> {
        self.made(|c, w| c.planned(w, change, When::Act, against))
    }

    /// `declare`'s act, its context and the grants checked.
    pub(crate) fn declare_granted(&self, declaration: Declaration) -> Result<Outcome> {
        let against = Against::default();
        self.made(|c, w| c.declaring_one(w, declaration, When::Act, against))
    }

    /// What `plan` makes, appended and answered with its plan.
    fn made(&self, plan: impl FnOnce(&Self, &mut Writer) -> Result<Planned>) -> Result<Outcome> {
        check_context(self.lake, &self.context)?;
        let mut w = self.writer()?;
        let planned = plan(self, &mut w)?;
        let from = head(&w).next();
        let Planned {
            plan,
            drafts,
            nothing,
            answer,
        } = planned;
        let mut outcome = match nothing {
            true => applied(head(&w), None),
            false => {
                let events = self.lake.append(&mut w, drafts)?;
                match &answer {
                    Answer::Migrated(renamed) => self.answered(&events, renamed.clone()),
                    Answer::Declared(_, true) | Answer::Last => {
                        applied(events.last().expect("drafts were sent").position, None)
                    }
                    _ => outcome_of(&events),
                }
            }
        };
        if let Answer::Declared(sent, _) = &answer {
            outcome = with_declared(outcome, &w.ontology, sent, from);
        }
        if let Outcome::Applied { plan: answered, .. } = &mut outcome {
            *answered = Some(Box::new(plan));
        }
        Ok(outcome)
    }

    /// The plan of `change` and what makes it, on the act's own path.
    pub(crate) fn planned(
        &self,
        w: &mut Writer,
        change: Changed,
        when: When,
        against: Against<'_>,
    ) -> Result<Planned> {
        let sees = Sight::held(self.who, &w.ontology);
        if let Changed::Records {
            ty,
            filter,
            to,
            only,
            skip_restricted,
        } = change
        {
            let now = match when {
                When::Plan => w.last_known_at,
                When::Act => self.lake.clock(w),
            };
            let records = (ty, *filter, to, only, skip_restricted);
            return self.changing(w, records, (now, 0, None), against);
        }
        granted(sees.authors())?;
        match change {
            Changed::Declare { mut declarations } => {
                self.redeclared(&w.ontology, &mut declarations)?;
                self.check_bundle(&w.ontology, &declarations)?;
                let sent = declarations.clone();
                declarations.retain(|d| !w.ontology.in_force(d));
                let mut planned = self.declaring(w, (sent, declarations), when, against)?;
                if let Answer::Declared(_, last) = &mut planned.answer {
                    *last = true;
                }
                Ok(planned)
            }
            Changed::AddField { ty, field } => {
                let mut t = sees.ty(&ty)?.clone();
                // The field is added to its author's view of the type (027).
                sees.known_fields(&mut t, &|n| w.ontology.ty(n).ok());
                if t.has_field(&field.name) {
                    let (ty, name) = (ty.clone(), field.name.clone());
                    let case = Case::FieldExists { ty, field: name };
                    return Err(Error::invalid_at(&field.name, case));
                }
                t.fields.push(*field);
                self.declaring_one(w, Declaration::Type(t), when, against)
            }
            Changed::Recompute { what } => {
                let declaration = self.recomputed(w, &what)?;
                self.declaring(w, (Vec::new(), vec![declaration]), when, against)
            }
            Changed::Migrate { migration } => self.migrating(w, *migration, when, against),
            Changed::MigrateValueType { migration } => {
                self.migrating_value_type(w, *migration, when, against)
            }
            Changed::Records { .. } => unreachable!("a change to records is planned above"),
        }
    }

    /// `declare`'s plan, its context and the `schema` grant checked: a
    /// declaration that is the one in force is answered at the head and
    /// appends nothing, after every refusal (027, extended).
    pub(crate) fn declaring_one(
        &self,
        w: &mut Writer,
        mut declaration: Declaration,
        when: When,
        against: Against<'_>,
    ) -> Result<Planned> {
        self.redeclared(&w.ontology, std::slice::from_mut(&mut declaration))?;
        // One who reads every field was judged against the lake itself.
        if !Sight::held(self.who, &w.ontology).reads_all_fields() {
            declare::check_declaration(&w.ontology, &declaration, false)?;
        }
        self.check_bound(&declaration, Some(&w.ontology), None)?;
        self.verbs_still_read(&w.ontology, std::slice::from_ref(&declaration))?;
        let changed = (!w.ontology.in_force(&declaration)).then(|| declaration.clone());
        let set = (vec![declaration], changed.into_iter().collect());
        self.declaring(w, set, when, against)
    }

    /// The plan of `changed`, the checked declarations of `sent` that are
    /// not in force: nothing is appended where there is none.
    fn declaring(
        &self,
        w: &mut Writer,
        (sent, changed): (Vec<Declaration>, Vec<Declaration>),
        when: When,
        against: Against<'_>,
    ) -> Result<Planned> {
        let bundled = match changed.is_empty() {
            true => Bundled {
                drafts: Vec::new(),
                types: Vec::new(),
            },
            false => {
                let now = match when {
                    When::Plan => w.last_known_at,
                    When::Act => self.lake.clock(w),
                };
                let judging = self.judging(when);
                self.bundle_drafts(w, changed, (now, 0, judging))?
            }
        };
        let plan = plan_of(w, (bundled.types, Vec::new()));
        stale(&plan, against)?;
        Ok(Planned {
            plan,
            nothing: bundled.drafts.is_empty(),
            drafts: bundled.drafts,
            answer: match sent.is_empty() {
                true => Answer::Plain,
                false => Answer::Declared(sent, false),
            },
        })
    }

    /// A migration's plan: its preflight as its author is shown it, the
    /// one section. Made, it appends what the preflight planned, refused
    /// where it is not lossless.
    fn migrating(
        &self,
        w: &mut Writer,
        mut m: Migration,
        when: When,
        against: Against<'_>,
    ) -> Result<Planned> {
        let author = self.authored(&w.ontology, &mut m)?;
        let view = self.lake.engine.view(None)?;
        // Planned, it decides its set changes too, at the lake's last time,
        // as the act does at the time it is made (062).
        let now = match when {
            When::Plan => w.last_known_at,
            When::Act => self.lake.clock(w),
        };
        let judged = (&m, author.as_ref(), None);
        let judging = self.judging(when);
        let preflight = self.preflight_plan(w, view.as_ref(), judged, (Some(now), judging))?;
        let report = self.shown(w, &m, preflight.report.clone())?;
        if against.report.is_some_and(|id| id != report.report_id) {
            return Err(Error::invalid(Case::MigrationStale));
        }
        // And each type whose references a re-key rewrites, as its author
        // is shown it (062).
        let sight = Sight::held(self.who, &w.ontology);
        let mut types = vec![report];
        types.extend(others_shown(&sight, preflight.others.clone()));
        let plan = plan_of(w, (types, Vec::new()));
        stale(&plan, against)?;
        let renamed = Self::known_renames(&sight, &w.ontology, &m);
        let nothing = unchanged(&w.ontology, &m, &preflight);
        // A plan of a lossy change tells it; any other refuses what its act
        // refuses, past `max_set_changes` too.
        let lossy = when == When::Plan && !preflight.report.lossless;
        let drafts = match lossy {
            true => Vec::new(),
            false => self.migration_drafts(w, m, preflight, now, 0)?,
        };
        drop(view);
        Ok(Planned {
            plan,
            drafts,
            nothing,
            answer: Answer::Migrated(renamed),
        })
    }

    /// A value type's migration's plan: its preflight as its author is
    /// shown it. Made, it appends what the preflight planned, refused while
    /// a stored value would not fit.
    fn migrating_value_type(
        &self,
        w: &mut Writer,
        m: ValueTypeMigration,
        when: When,
        against: Against<'_>,
    ) -> Result<Planned> {
        let view = self.lake.engine.view(None)?;
        let now = match when {
            When::Plan => w.last_known_at,
            When::Act => self.lake.clock(w),
        };
        let judging = self.judging(when);
        let preflight = self.preflight_value_type(w, view.as_ref(), &m, (Some(now), judging))?;
        if against
            .report
            .is_some_and(|id| id != preflight.report.report_id)
        {
            return Err(Error::invalid(Case::MigrationStale));
        }
        self.renames_shown(&w.ontology, &m, &preflight.report)?;
        let report = self.shown_value_type(&w.ontology, preflight.report.clone());
        let plan = plan_of(w, (Vec::new(), vec![report.clone()]));
        stale(&plan, against)?;
        let drafts = match (when, report.fits) {
            (When::Plan, false) => Vec::new(),
            _ => {
                fits(report, true)?;
                self.value_type_drafts(w, m, preflight, now, 0)?
            }
        };
        drop(view);
        Ok(Planned {
            plan,
            nothing: drafts.is_empty(),
            drafts,
            answer: Answer::Plain,
        })
    }
}

/// Refused `plan_stale` where the change is made against another plan.
pub(super) fn stale(plan: &Plan, against: Against<'_>) -> Result<()> {
    match against.plan {
        Some(id) if id != plan.report_id => Err(Error::invalid(Case::PlanStale)),
        _ => Ok(()),
    }
}

/// A stored plan as `sight`'s principal is shown it (062), read at `view`:
/// a section of a type it is not shown is left out; a reference's count
/// is told to one who reads it and is shown its target, a constraint's
/// to one who reads every field it reads; a first key only where it is
/// answered. Its id is made again over what it is shown.
pub(crate) fn shown_plan(
    plan: &mut Plan,
    sight: &Sight<'_>,
    view: &dyn ProjectionView,
) -> Result<()> {
    // A change to records is its reader's own, made for it (081).
    if plan.records.is_some() {
        return Ok(());
    }
    plan.types.retain(|t| sight.ty(&t.ty).is_ok());
    for t in &mut plan.types {
        let decl = sight.ty(&t.ty)?;
        let target = |f: &FieldName| (decl.field(f))?.ty.target.clone();
        let shows = |f: &FieldName| target(f).is_some_and(|x| sight.ty(&x).is_ok());
        let reads = |f: &FieldName| sight.reads_from(&t.ty, f);
        t.unlive.retain(|u| reads(&u.field) && shows(&u.field));
        let told = |c: &ConstraintName| {
            let c = decl.constraints.iter().find(|x| &x.name == c);
            let e = c.and_then(|c| c.holds.typed().ok());
            e.is_some_and(|e| e.fields_read().iter().all(|f| reads(f)))
        };
        t.breaches.retain(|b| told(&b.constraint));
        let keys = (t.unlive.iter_mut().map(|u| &mut u.sample_keys))
            .chain(t.breaches.iter_mut().map(|b| &mut b.sample_keys));
        for keys in keys {
            let mut shown = Vec::new();
            for k in keys.drain(..) {
                if crate::reads::answered(view, sight, &t.ty, &k)? {
                    shown.push(k);
                }
            }
            *keys = shown;
        }
        t.more = (t.unlive.iter()).any(|u| u.records > u.sample_keys.len() as u64)
            || (t.breaches.iter()).any(|b| b.records > b.sample_keys.len() as u64);
    }
    plan.report_id = plan_id(plan);
    Ok(())
}
