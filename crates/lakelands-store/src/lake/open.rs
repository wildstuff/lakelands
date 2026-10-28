//! Opening a lake: the engine and its lease, the projection brought up to
//! the log, and the handle assembled around them.

use super::*;

/// Why a writer's open rebuilds the projection `e` refuses: an older
/// build laid it out. None for any other refusal.
fn older_layout(e: &Error) -> Option<String> {
    match e.situation() {
        Some(Situation::ProjectionLayout {
            found: Some(found),
            wanted,
            ..
        }) if found < wanted => Some(format!(
            "an older build laid it out (layout {found}; this build's is {wanted})"
        )),
        _ => None,
    }
}

/// What a build ahead is refused on an engine that keeps no scratch.
fn keeps_no_scratch() -> Error {
    Error::unbound(Seam::Operation {
        op: "open_scratch".to_owned(),
    })
}

/// The opener `seams` passes for the engine `location` names, for a
/// location whose bounds a lake may be opened under.
pub(crate) fn opener(
    location: &Location,
    seams: &Seams,
) -> Result<Arc<dyn lakelands_api::engine::EngineOpener>> {
    // A bound of no window would refuse every second stage for a cause no
    // caller can mend: an open or an import at such a location is refused
    // before anything is made (067).
    if location.limits.max_rerank_top == 0 {
        let case = lakelands_api::error::Case::RerankNoWindow;
        return Err(Error::invalid_at(&"limits.max_rerank_top".into(), case));
    }
    // The lake's own window is one a delivery could not ask for: refused
    // before anything is made, as above (016).
    let limits = &location.limits;
    if limits.commit_window_events > limits.commit_window_max {
        let case = lakelands_api::error::Case::LimitRange {
            max: limits.commit_window_max,
        };
        return Err(Error::invalid_at(
            &"limits.commit_window_events".into(),
            case,
        ));
    }
    let name: String = match &location.engine {
        EngineConfig::Sqlite { .. } => "sqlite".into(),
        EngineConfig::Memory => "memory".into(),
        EngineConfig::Postgres { .. } => "pg".into(),
        EngineConfig::Other { name, .. } => name.clone(),
        _ => String::new(),
    };
    seams
        .engines
        .iter()
        .find(|e| e.name() == name)
        .cloned()
        .ok_or_else(|| {
            Error::unbound(Seam::Engine {
                name: name.to_owned(),
            })
        })
}

impl Lake {
    /// Open a lake: find its engine among the openers the host passed,
    /// take the lease unless opening read-only, create the lake if asked
    /// and absent, and fold whatever the log holds beyond the projection.
    /// Opened for a rebuild, the whole log is folded under the lease into
    /// a scratch beside the projection, which is then swapped in: that open
    /// is the rebuild. So is a writer's open of a projection an older
    /// build laid out, which goes on from where a build ahead stopped
    /// ([`build_ahead`]).
    pub fn open(location: &Location, seams: Seams, mode: OpenMode) -> Result<Lake> {
        let opener = opener(location, &seams)?;
        // A lake of a format this release does not read is not opened: its
        // engine judges, makes and changes nothing of it, in any mode (054).
        // One of an older format it reads is upgraded by a writer's open,
        // and is not read as it is.
        let written = log::one_format(&location.lake, opener.log_ends(location)?.as_ref())?;
        if mode == OpenMode::Read {
            log::read_as_is(&location.lake, written)?;
        }
        // A writer's open of a projection an older build laid out rebuilds
        // it, as an upgrade's open is the place a release pays for its
        // steps; a reader's, and one of a newer layout or of none, is
        // refused (004).
        let (engine, rebuilt_at_open) = match opener.open(location, mode) {
            Err(e) if mode != OpenMode::Read && older_layout(&e).is_some() => {
                (opener.open(location, OpenMode::Rebuild)?, older_layout(&e))
            }
            opened => (opened?, None),
        };
        let engine: Arc<dyn LakeEngine> = Arc::from(engine);
        // A writer is judged before it takes the lease: an engine may cut
        // its log back to what is committed as it hands the lease out,
        // and a lake that is refused is left as it was found. A rebuild
        // is not judged: it makes the projection from the log as it
        // stands.
        if matches!(mode, OpenMode::Write | OpenMode::Create) {
            ends(engine.as_ref())?;
        }
        let lease = match mode {
            OpenMode::Read => None,
            OpenMode::Write | OpenMode::Create | OpenMode::Rebuild => {
                Some(engine.take_lease(&holder())?)
            }
        };
        let rebuilds = mode == OpenMode::Rebuild || rebuilt_at_open.is_some();
        let began = std::time::Instant::now();
        let mut assembled = Lake::assemble(location, seams, engine, lease.clone());
        let lake = assembled.turn();
        // The last reclaim's time is read before the rebuild replaces the
        // projection that keeps it.
        if rebuilds {
            lake.reclaim_kept();
        }
        // The rebuild folds beside the projection and swaps the scratch in
        // (071): an upgrade's goes on from where a build ahead stopped, an
        // open to rebuild folds the whole log. Either is folded as the
        // catch-up below would fold it in place, where the engine keeps no
        // scratch.
        let how = match mode {
            OpenMode::Rebuild => Beside::Open,
            _ => Beside::Resume,
        };
        let built = match rebuilds {
            true => lake.rebuilt_beside(&mut lake.writer(), how)?,
            false => Swapped::Kept,
        };
        if rebuilds && !matches!(built, Swapped::In(_)) {
            lake.engine.projection().clear()?;
        }
        let mut said = rebuilt_at_open.map(|why| match built {
            Swapped::In(Built {
                ahead: Some(at),
                events,
                ..
            }) => format!(
                "{why}; a build ahead had folded it through position {}, and the open folded the {events} events after it",
                at.0
            ),
            Swapped::Busy(true) => format!(
                "{why}; another handle read the projection, so the open folded the whole log in \
                 place, and removed the build ahead it could not swap in"
            ),
            Swapped::Busy(false) => format!(
                "{why}; another handle read the projection, so the open folded the whole log in place"
            ),
            _ => why,
        });
        lake.catch_up()?;
        // A reader is refused a lake that holds a type of its vocabulary
        // in an older form, as one written in an older format is: only a
        // writer's open declares it again.
        if mode == OpenMode::Read {
            let w = lake.writer();
            for t in lake.vocabulary() {
                if crate::declare::vocabulary_held(&w.ontology, &t)? == Held::Older {
                    let (lake, ty) = (lake.id.to_string(), t.name);
                    let situation = Situation::VocabularyUpgradeOwed { lake, ty };
                    return Err(Error::unavailable(situation));
                }
            }
        }
        if rebuilds {
            let tip = lake.writer().tip.as_ref().map_or(0, |t| t.0.0);
            let events = match built {
                Swapped::In(b) => b.events,
                _ => tip,
            };
            lake.measured(began, events);
        }
        let refolded = match &lease {
            Some(lease) => lake.leased(lease)?,
            None => false,
        };
        // An act erasing values the folded log held and the projection had
        // not erased, written after a build ahead or met by a fold not told
        // of it, is finished by a rebuild of the whole log: said, since the
        // open then took as long as one.
        if let Some(why) = said.as_mut().filter(|_| refolded) {
            why.push_str(
                "; the log held an act erasing values that the projection had not erased, so the \
                 open then folded the whole log again to finish it",
            );
        }
        {
            // A read-only handle keeps the folded position `catch_up`
            // published; only the writer publishes its tip.
            let w = lake.writer();
            if w.lease.is_some() {
                lake.publish(&w);
            }
        }
        assembled.rebuilt_at_open = said;
        Ok(assembled)
    }

    /// Build the projection this release lays out beside the one the lake
    /// at `location` answers from, ahead of the writer's open that upgrades
    /// it, while an older release still holds its lease (071). It opens
    /// the log and the scratch only, never the projection or the lease, so
    /// it runs beside any writer; run again, it goes on from where it
    /// stopped. The writer's open of a projection an older build laid out
    /// then folds only the events after it, and swaps.
    ///
    /// A build whose fold met an act that erased values, a source
    /// forgotten or values removed, may hold what the act erased: it folds
    /// the whole log again, told the acts, as a rebuild does, so the
    /// scratch holds nothing an act erased. Refused as unbound by an
    /// engine that keeps no scratch.
    pub fn build_ahead(location: &Location, seams: Seams) -> Result<Built> {
        let opener = opener(location, &seams)?;
        log::one_format(&location.lake, opener.log_ends(location)?.as_ref())?;
        let mut scratch = Lake::scratch(location, &seams)?.ok_or_else(keeps_no_scratch)?;
        let builder = scratch.turn();
        let mut w = builder.writer();
        w.rebuilding = builder.engine.projection().folded_through()?.is_none();
        let built = builder.build(&mut w, false)?;
        let (list, acts) = builder.acts_listed()?;
        if !list.values().chain(acts.values()).any(|s| s == "owed") {
            return Ok(built);
        }
        let view = builder.engine.view(None)?;
        w.removals = crate::remove::told(&builder, view.as_ref())?;
        drop(view);
        w.rebuilding = true;
        let again = builder.build(&mut w, true)?;
        Ok(Built {
            ahead: None,
            events: built.events + again.events,
            ..again
        })
    }

    /// Remove the scratch built ahead beside the lake at `location`, of
    /// this release's layout or another's: what a build ahead left that no
    /// writer's open will swap in is the operator's to remove (071).
    /// Answers whether one was marked built. Refused `rebuilding` while a
    /// builder holds it, and as unbound by an engine that keeps no scratch.
    pub fn discard_ahead(location: &Location, seams: Seams) -> Result<bool> {
        let opener = opener(location, &seams)?;
        // Opening it removes one another layout marked, and one unfit.
        let scratch = opener.open_scratch(location)?;
        let marked = scratch.projection().folded_through()?.is_some();
        // Emptied, it no longer holds its mark, and goes with its handle.
        scratch.projection().clear()?;
        drop(scratch);
        Ok(marked)
    }

    /// The types of the host's vocabulary this lake may hold.
    fn vocabulary(&self) -> Vec<TypeDecl> {
        let Some(v) = &self.seams.vocabulary else {
            return Vec::new();
        };
        let types = v.types().into_iter();
        types.filter(|t| v.may_hold(&self.id, &t.name)).collect()
    }

    /// A lake over `engine`, holding `lease` if one was taken, before
    /// anything is read from its log.
    pub(crate) fn assemble(
        location: &Location,
        mut seams: Seams,
        engine: Arc<dyn LakeEngine>,
        lease: Option<Lease>,
    ) -> Lake {
        let (seams_keys, log) = (seams.keys.clone(), Some(engine.clone()));
        Lake {
            id: location.lake.clone(),
            scoped: scrub::Scoped::new(engine.clone()),
            engine,
            embed: Mutex::new(seams.embed.take()),
            rerank: Mutex::new(seams.rerank.take()),
            seams,
            limits: location.limits,
            location: location.clone(),
            calls: tokio::sync::Mutex::new(()),
            queue: Arc::default(),
            waiting: Mutex::new(BTreeMap::new()),
            sorted_whole: Mutex::new(BTreeMap::new()),
            choosing: Mutex::default(),
            writer: Mutex::new(Writer {
                lease,
                tip: None,
                staged: None,
                closing: BTreeSet::new(),
                last_known_at: Timestamp(i64::MIN),
                ontology: Ontology::default().named(),
                settings: BTreeMap::new(),
                owed: Vec::new(),
                held: BTreeMap::new(),
                behind: false,
                failed: None,
                answers: Vec::new(),
                plans: record::Plans::default(),
                lost: false,
                forgotten: BTreeSet::new(),
                removals: Vec::new(),
                ahead: BTreeSet::new(),
                rebuilding: false,
                zone: None,
                sized: None,
            }),
            keys: Arc::new(crate::sealing::Keys::new(
                location.lake.clone(),
                seams_keys,
                log,
            )),
            head: AtomicU64::new(0),
            heads: tokio::sync::watch::Sender::new(0),
            holds_lease: AtomicBool::new(false),
            shapes: Mutex::default(),
            rebuilds: AtomicU64::new(0),
            rebuilt_rate: AtomicU64::new(0),
            reclaim_ms: AtomicU64::new(0),
            scoped_rate: AtomicU64::new(0),
            way: Mutex::new(None),
            acted: (AtomicU64::new(0), AtomicI64::new(i64::MIN)),
            reclaim_owed: Mutex::new(None),
            rebuilt_at_open: None,
        }
    }
}

impl Turn<'_> {
    /// What a writer writes once it holds `lease` and has folded the log,
    /// on its open and on a standby's taking over: the lake's creation,
    /// the `lease_taken`, an upgrade's step, the types the lake may hold
    /// and does not, a forgetting a crash left half done, and a seal past
    /// the bound. Answers whether finishing the forgetting folded the whole
    /// log again.
    pub(crate) fn leased(&self, lease: &Lease) -> Result<bool> {
        let mut w = self.writer();
        let now = self.clock(&mut w);
        let mut drafts = Vec::new();
        if w.tip.is_none() {
            drafts.push(Draft::system(
                now,
                EventBody::LakeCreated {
                    format: LOG_FORMAT_VERSION,
                },
            ));
        }
        drafts.push(Draft::system(
            now,
            EventBody::LeaseTaken {
                fence: lease.fence,
                holder: lease.holder.clone(),
            },
        ));
        // A lake written in an older format is written in this one from
        // here on, the step marked right after the new writer's lease.
        // The events before it stay as they were written; the step from
        // 14 changes no fold, so the projection stays
        // (`docs/log-format.md` §6).
        // Judged by the tip read under the lease: another writer may
        // have upgraded the lake since its ends were read.
        let tip = w.tip.as_ref().map(|t| t.0);
        let tip = tip.map(|p| self.read_events(p, 1)).transpose()?;
        let newest = tip.and_then(|t| t.first().map(|e| e.v));
        if let Some(newest) = newest.filter(|v| *v < LOG_FORMAT_VERSION) {
            let (from, to) = (newest, LOG_FORMAT_VERSION);
            drafts.push(Draft::system(now, EventBody::LakeUpgraded { from, to }));
        }
        // The system types this lake may hold and does not hold yet are
        // declared on every open for writing, so a host type added after
        // the lake was created reaches it too.
        let mut types = vec![setting_type(), aging_rule_type()];
        let held = self.vocabulary();
        // Every vocabulary type is checked before anything is appended,
        // held already or not: a process whose vocabulary collides with
        // the store's own names must not write, nor one that changes a
        // type the lake holds beyond adding optional fields to it and
        // rewording it. One the lake holds without those fields, or
        // reworded, is declared again, as the store's own are below.
        let mut again = BTreeSet::new();
        for t in &held {
            crate::declare::check_vocabulary_type(t)?;
            if crate::declare::vocabulary_held(&w.ontology, t)? != Held::Same {
                again.insert(t.name.clone());
            }
        }
        types.extend(held);
        // The store's own types are declared again where the lake holds
        // an older form, one without a field they have since gained: a
        // field added, which reshapes no record (063, extended).
        let own = |t: &lakelands_api::ontology::TypeDecl| {
            [SETTING_TYPE, AGING_RULE_TYPE].contains(&t.name.as_str())
        };
        let older = |t: &lakelands_api::ontology::TypeDecl| {
            let held = w.ontology.types.get(&t.name);
            held.is_some_and(|h| {
                let has = |f: &lakelands_api::ontology::FieldDecl| h.decl.has_field(&f.name);
                !t.fields.iter().all(has)
            })
        };
        types.retain(|t| {
            !w.ontology.types.contains_key(&t.name)
                || (own(t) && older(t))
                || again.contains(&t.name)
        });
        // No door leaves a lake with a problem among its tools' names,
        // and this is one (027, extended): each type is judged beside
        // the lake and the types before it, before anything is
        // appended. A type the lake holds came through a door.
        let mut with = w.ontology.clone();
        for d in types.iter().cloned().map(Declaration::Type) {
            crate::declare::tool_names(&with, &d)?;
            crate::writes::put_as_appended(&mut with, &d, Some);
        }
        let grouped = types.iter().any(|t| t.name.as_str() == SETTING_TYPE)
            && w.ontology.types.contains_key(&TypeName::from(SETTING_TYPE));
        for t in types {
            let hash = shape::schema_hash(&t).map_err(|e| {
                let _ = e;
                let flaw = Flaw::NoSchemaHash { ty: t.name.clone() };
                damaged(Part::Projection, None, flaw)
            })?;
            let declaration = Declaration::Type(t);
            let version = w.ontology.next_version(&declaration);
            drafts.push(Draft::system(
                now,
                EventBody::Declared {
                    declaration,
                    schema_hash: Some(hash),
                    version,
                    with_the_set: false,
                },
            ));
        }
        // The settings the lake held before `sys.setting` had a
        // `group` are stamped in the same append, so no record stands
        // in a group its declaration does not name (063, extended).
        if grouped {
            let ty = TypeName::from(SETTING_TYPE);
            for r in self.engine.view(None)?.scan(&ty, &every(false))? {
                let group = crate::writes::setting_group(&w.ontology, r.key.as_str());
                if r.fields.get("group").unwrap_or(&Value::Null) == &group {
                    continue;
                }
                let body = EventBody::RecordUpserted {
                    ty: ty.clone(),
                    key: r.key.clone(),
                    version: Version(r.version.0 + 1),
                    patch: [("group".into(), group)].into(),
                    made: false,
                };
                drafts.push(Draft::system(now, body));
            }
        }
        self.append(&mut w, drafts)?;
        // What a crash left of a forgetting is finished first (058).
        // An owed rebuild refuses the open, since the projection is not
        // whole without it; a reclaim that fails again stays owed and
        // is told, never a lock-out (lakelands#1161).
        let rebuilt = self.rebuilds.load(Ordering::SeqCst);
        self.finish_forgetting(&mut w)?;
        let refolded = self.rebuilds.load(Ordering::SeqCst) != rebuilt;
        // A segment past the bound from before is sealed here, its
        // checkpoint in the log and the feed (074).
        drop(w);
        self.seal_by_size();
        Ok(refolded)
    }
}
