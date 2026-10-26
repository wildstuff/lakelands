//! The proposals door: a principal's proposals listed, and the entry a
//! proposed write is shown as.

use super::*;

impl<'a> Reader<'a> {
    /// `proposals`: for a reviewer every proposal there to it, each as
    /// its view; one's own otherwise.
    /// `asked` is the principal as the call named it, before its grants
    /// were followed to the read's position.
    pub fn proposals(&self, filter: &ProposalFilter, asked: &Principal) -> Result<Vec<Proposal>> {
        let limit = self.limit(filter.limit, false)?;
        let offset = match &filter.cursor {
            Some(c) => resume(c, "proposals", None, &Params::new())?.offset as usize,
            None => 0,
        };
        // The kind and the type are answered on the entry as its reader is
        // shown it, so they are asked behind the trim, not of the engine.
        let asking = |limit: u32| ProposalFilter {
            limit: Some(limit),
            cursor: None,
            kind: None,
            ty: None,
            ..filter.clone()
        };
        let me = self.sight.who().writer();
        // The lake as its reader is described it and the types it is shown,
        // each read once, only where a proposal asks.
        let (lake, seen) = (OnceCell::new(), OnceCell::new());
        let stands = |ident: &str| stands(&self.shape.ontology, ident);
        // In the engine's order, newest first, a page is its first entries;
        // those past it are not judged, each a read of the past where its
        // subject was declared again since (lakelands#888). The engine is
        // asked a page at a time, each twice the last, while the listing is
        // not full, so a reader kept from many asks a few (lakelands#1063).
        // Another order, or an engine without pages, judges every entry.
        let end = offset.saturating_add(limit);
        let paged = filter.order_by.is_empty();
        let (mut mine, mut before) = (Vec::new(), None);
        let mut page = u32::try_from(end.max(1)).unwrap_or(u32::MAX);
        loop {
            let rows = (paged.then(|| self.view.proposals_before(&asking(page), before)))
                .transpose()?
                .flatten();
            let full = rows.as_ref().is_some_and(|rows| rows.len() as u32 == page);
            let rows = rows.map_or_else(|| self.view.proposals(&asking(u32::MAX)), Ok)?;
            for mut p in rows {
                if paged && mine.len() >= end {
                    break;
                }
                before = Some(p.position);
                if !self.sight.who().grants.review && p.proposer != me {
                    continue;
                }
                if let Some(logged) = p.write.take() {
                    let at = (p.position, p.proposed_at);
                    let Some(shown) = self.entry(asked, &logged, at, (&lake, &seen))? else {
                        continue;
                    };
                    (p.write, p.more) = shown;
                    // A change to records' plan is its proposer's own (081).
                    if let (Some(ProposedWrite::Records { plan, .. }), true) =
                        (&mut p.write, p.proposer == me)
                        && let ProposedWrite::Records { plan: own, .. } = &logged
                    {
                        plan.clone_from(own);
                    }
                }
                // A withdrawal names the type it withdrew as the lake held it
                // when proposed, whatever stands under its name since.
                let withdrawn: Vec<String> = match &p.write {
                    Some(ProposedWrite::Declare { declarations, .. }) => (declarations.iter())
                        .filter(|d| matches!(d, Declaration::Withdraw(_)))
                        .map(core_engines::declaration_key)
                        .collect(),
                    _ => Vec::new(),
                };
                let was = OnceCell::new();
                let then = |ident: &str| {
                    let was = was.get_or_init(|| self.ontology_at(p.position));
                    was.as_ref().and_then(|o| crate::reads::stands(o, ident))
                };
                let on = |ident: &str| match withdrawn.iter().any(|w| w == ident) {
                    true => then(ident),
                    false => stands(ident),
                };
                if core_engines::matches(&p, filter, on) {
                    mine.push(p);
                }
            }
            if !full || mine.len() >= end {
                break;
            }
            page = page.saturating_mul(2);
        }
        Ok(ordered_proposals(mine, &filter.order_by)?
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect())
    }

    /// The proposal logged at `at`, writing `logged`, as the listing shows
    /// it to its reader, and whether it holds more; `None` where it is not
    /// there to it (lakelands#762). The listing and the accept ask this
    /// alone, so an entry is decided as it is listed (006 and 013,
    /// extended).
    ///
    /// The reader is `asked`, the principal as its call named it, judged
    /// under up to three sights, its grants followed to each: the lake in
    /// force here; the lake as the entry would leave it, a bundle's whole
    /// and of a migration its `to`; and the lake as it was proposed
    /// against, where the type a migration reshapes or the verb a call
    /// calls was declared again since, or a type the entry's type rests on
    /// ([`lakelands_core::describe::shown_by`]) was re-keyed since, read
    /// only for a reader its grants keep from a field. An entry is not there when one of them hides a
    /// type it names: the type it writes or a called verb is on; of a
    /// migration's `to` and of each type a bundle declares, what
    /// [`TypeDecl::names_against`] names against the type in force; and
    /// the type each verb, trigger and object set of a bundle is on, with
    /// the type the one in force under its name stands on, which would be
    /// gone with it (lakelands#893). Nor is a bundle there one of whose
    /// names is held by a declaration `describe` does not list to its
    /// reader, nor a set or a migration whose tool's name is taken by
    /// what it is not shown (lakelands#915). A link type is shown only
    /// with both its ends, and a type a bundle brings only as `describe`
    /// would show it once in force. Nor is a call there whose verb, as
    /// either lake declares it, reads a field its reader may not read:
    /// `describe` does not name the verb, and the entry would tell that
    /// its `requires` held (024, extended). Nor is a call there whose
    /// verb, as the lake declares it now or, withdrawn since, did at the
    /// proposal, asks a grant of its own its reader does not hold (070),
    /// or reads a setting its reader is not told (063, extended).
    /// A migration's report counts
    /// every owner's records, so a holder of `only` is listed none (051).
    /// A read the engine fails is the listing's error, never an entry
    /// left out (lakelands#927).
    pub(crate) fn entry(
        &self,
        asked: &Principal,
        logged: &ProposedWrite,
        (at, proposed_at): (Position, Timestamp),
        (lake, seen): (&OnceCell<LakeDescription>, &OnceCell<Ontology>),
    ) -> Result<Option<(Option<ProposedWrite>, bool)>> {
        if matches!(
            logged,
            ProposedWrite::Compute { .. } | ProposedWrite::ComputeSweep { .. }
        ) {
            return self.computed_entry(logged, at);
        }
        let (o, mut logged) = (&self.shape.ontology, logged.clone());
        // A shape not in force is completed as one in force is.
        let names = |t: &TypeDecl| t.names_against(o.ty(&t.name).ok());
        let (mut named, mut after): (Vec<TypeName>, _) = (Vec::new(), None);
        // A tool's name taken by what its reader is not shown hides the
        // entry (lakelands#915).
        let tools_shown = |set: &[Declaration]| {
            let seen = seen.get_or_init(|| crate::writes::types_shown(self.sight.who(), o));
            crate::writes::tools_as_shown(self.sight.who(), o, seen, set).is_ok()
        };
        match &mut logged {
            ProposedWrite::Upsert { ty, .. }
            | ProposedWrite::Retire { ty, .. }
            | ProposedWrite::Records { ty, .. } => named.push(ty.clone()),
            ProposedWrite::Migrate { migration: m, .. } => {
                if !tools_shown(&[Declaration::Type(m.to.clone())]) {
                    return Ok(None);
                }
                o.complete(&mut m.to);
                named.extend(names(&m.to));
                let mut renames = self.shape.renames.clone();
                renames.push(Renamed {
                    at: self.position.next(),
                    ty: m.to.name.clone(),
                    renames: m.renames.clone(),
                    to: None,
                });
                let left = Sight::new(asked.clone(), &renames, None, o);
                if self.sight.holds_only() || !left.would_show(&m.to) {
                    return Ok(None);
                }
            }
            ProposedWrite::Declare { declarations, .. } => {
                // One in force under a declared name that its reader is not
                // shown would be replaced, and the accept's refusal would
                // name it (lakelands#915).
                let mut held = declarations.iter().filter_map(|d| in_force_under(o, d));
                if !tools_shown(declarations) || held.any(|d| !self.sight.lists(&d)) {
                    return Ok(None);
                }
                // A withdrawal is judged by what it withdrew, as held when
                // proposed, and by what stands under its name now: listed to
                // a reader shown both.
                let withdraws = |d: &&Declaration| matches!(d, Declaration::Withdraw(_));
                let was = match declarations.iter().any(|d| withdraws(&d)) {
                    true => match self.ontology_at(at) {
                        Some(was) => Some(was),
                        None => return Ok(None),
                    },
                    false => None,
                };
                if let Some(was) = &was {
                    let then = Sight::new(asked.clone(), &self.shape.renames, Some(at), was);
                    for held in declarations
                        .iter()
                        .filter(withdraws)
                        .filter_map(|d| was.held(d))
                    {
                        if !then.lists(&held) {
                            return Ok(None);
                        }
                    }
                }
                let mut put = o.clone();
                for d in declarations.iter() {
                    crate::writes::put_as_appended(&mut put, d, Some);
                }
                for d in declarations.iter_mut() {
                    match d {
                        Declaration::Type(t) => {
                            put.complete(t);
                            named.extend(names(t))
                        }
                        Declaration::Withdraw(_) => named
                            .extend(core_engines::declared_on(d, &|i| {
                                was.as_ref().and_then(|was| stands(was, i))
                            })),
                        // As the listing's filter names it (lakelands#916).
                        d => named.extend(core_engines::declared_on(d, &|i| stands(o, i))),
                    }
                }
                after = Some((put.declared(), put));
            }
            ProposedWrite::MigrateValueType { .. } if self.sight.holds_only() => {
                return Ok(None);
            }
            ProposedWrite::Call { .. }
            | ProposedWrite::MigrateValueType { .. }
            | ProposedWrite::Compute { .. }
            | ProposedWrite::ComputeSweep { .. } => {}
        }
        // The type an entry is on, and what its sight rests on (lakelands#913).
        let ty = match &logged {
            ProposedWrite::Upsert { ty, .. } | ProposedWrite::Retire { ty, .. } => Some(ty),
            ProposedWrite::Migrate { migration: m, .. } => Some(&m.to.name),
            ProposedWrite::Call { verb, .. } => o.verb(verb).ok().map(|v| &v.on),
            _ => None,
        };
        let find = |n: &TypeName| o.ty(n).ok();
        let by = ty
            .and_then(find)
            .map(|t| lakelands_core::describe::shown_by(t, &find));
        let subject = |d: &Declaration| match (d, &logged) {
            (Declaration::Type(t), ProposedWrite::Migrate { migration: m, .. }) => {
                t.name == m.to.name
            }
            (Declaration::Verb(v), ProposedWrite::Call { verb, .. }) => &v.name == verb,
            _ => false,
        };
        let mut declared = self.shape.declared.iter();
        let reads = |a: &Access| a.reads();
        let typed =
            |t: &TypeGrants| t.default.as_ref().is_none_or(reads) && t.fields.values().all(reads);
        let g = &self.sight.who().grants;
        let kept = !(reads(&g.default) && g.types.values().all(typed));
        // Or one of the types its sight rests on was re-keyed since: an old
        // key, in a key or a reference, may be a value of a field its
        // reader may not know of, where it now shows (lakelands#913).
        let mut moved = declared.any(|d| d.since > at && subject(&d.declaration));
        for n in by.iter().flatten().take_while(|_| kept) {
            if moved {
                break;
            }
            moved = self.rekeyed(n, at)?;
        }
        // A call of a verb withdrawn since is judged by the lake at its
        // proposal, as a withdrawal is.
        let gone = matches!(&logged, ProposedWrite::Call { verb, .. } if o.verb(verb).is_err());
        let was = match moved && kept || gone {
            true => match self.ontology_at(at) {
                Some(was) => Some(was),
                None => return Ok(None),
            },
            false => None,
        };
        let renames = &self.shape.renames;
        let then = was.as_ref();
        let then = then.map(|was| (Sight::new(asked.clone(), renames, Some(at), was), was));
        let left = after.as_ref();
        let left = left.map(|(all, put)| (Sight::new(asked.clone(), renames, None, all), put));
        let then: Under<'_> = then.as_ref().map(|(s, o)| (s, *o));
        let left: Under<'_> = left.as_ref().map(|(s, o)| (s, *o));
        let force: Under<'_> = Some((self.sight, o));
        if let ProposedWrite::Call { verb, .. } = &logged {
            // Nor to one without the verb's own grant, as the lake declares
            // the verb now, or at the proposal where it is withdrawn since:
            // the grant hides no value, so a verb declared again without it
            // shows what it kept (070).
            let mut now = [force, then].into_iter().flatten();
            let now = now.find_map(|(s, o)| Some((s, o.verb(verb).ok()?)));
            if now.is_some_and(|(s, v)| !s.verb_granted(v)) {
                return Ok(None);
            }
            for (sees, o) in [force, then].into_iter().flatten() {
                let Ok(v) = o.verb(verb) else { continue };
                let on = o.ty(&v.on);
                let far = |n: &TypeName| o.ty(n).ok();
                if on.is_ok_and(|on| !sees.reads_what_verb_reads(on, v, &far)) {
                    return Ok(None);
                }
                // Nor to one not told a setting the verb reads: what its
                // `requires` decided is the lake's (063, extended).
                if !sees.told_verb(v) {
                    return Ok(None);
                }
                named.push(v.on.clone());
            }
        }
        let hides = |(sees, o): (&Sight, &Ontology)| {
            let mut named = named.iter();
            named.any(|ty| o.ty(ty).is_ok() && sees.ty(ty).is_err())
        };
        if [force, left, then].into_iter().flatten().any(hides) {
            return Ok(None);
        }
        // Nor is one that names a record its reader is not answered beside
        // its own, by a reference or read across by a call, as it names it
        // now or named it when proposed: a condition or a reference a
        // setting, the clock or the record decide may come to a record at
        // one and not at the other. It would tell of that record, and its
        // accept is not granted (006). Nor is one whose own record or a
        // link's end it is not answered, held or another's (lakelands#1008,
        // lakelands#1016): an `only` lists in part only what it is shown,
        // trimmed (lakelands#867). A retired record that is not held is,
        // as to its writes, a free key, and so is a setting its reader is
        // not told (063).
        let view = self.view.as_ref();
        if !self.sight.sees_all() {
            // A retarget reaches records of its new targets alone: where none
            // holds a restricted record, none is withheld from a reader who
            // holds no `only`, and its values are not walked (lakelands#1071).
            let types = crate::writes::retarget_types(o, &logged)?;
            let mut free = !types.is_empty() && !self.sight.holds_only();
            for t in &types {
                if !free {
                    break;
                }
                let open = Scan {
                    restricted: false,
                    ..crate::lake::every(true)
                };
                let all = view.count(t, &crate::lake::every(true))?;
                free = self.sight.ty(t).is_ok() && view.count(t, &open)? == all;
            }
            if !free {
                let now = (view, self.settings()?, self.judged);
                let mut named = crate::writes::reached(self.lake, o, now, &logged, true)?;
                let since = self.shape.declared.iter().any(|d| d.since > at);
                if since || crate::writes::reaches_by_state(o, &logged) {
                    // The lake as the entry was proposed against, built from
                    // the past this read holds, the one its past sight reads.
                    let then = self.lake.engine.view(Some(at))?;
                    let (lake, then) = (self.lake, then.as_ref());
                    let was = match self.ontology_at(at) {
                        Some(o) => crate::lake::settings_in(then, &o).and_then(|s| {
                            crate::writes::reached(
                                lake,
                                &o,
                                (then, &s, proposed_at),
                                &logged,
                                false,
                            )
                        }),
                        None => {
                            crate::writes::reached_then(lake, then, proposed_at, &logged, false)
                        }
                    };
                    named.extend(was?);
                }
                // Each type's records once, a page at a time.
                let mut by: BTreeMap<TypeName, BTreeSet<RecordKey>> = BTreeMap::new();
                for (t, k) in named {
                    by.entry(t).or_default().insert(k);
                }
                for (t, keys) in &by {
                    let keys: Vec<RecordKey> = keys.iter().cloned().collect();
                    for page in keys.chunks(COUNT_PAGE) {
                        let there = view.get_many(t, page)?;
                        let live = |r: &&StoredRecord| !r.retired && self.sight.told(r).is_some();
                        let held = there.iter().filter(|r| live(r) || self.sight.withheld(r));
                        let held: Vec<&RecordKey> = held.map(|r| &r.key).collect();
                        if !held.is_empty() && !answered_of(view, self.sight, t, &held, held.len())?
                        {
                            return Ok(None);
                        }
                    }
                }
            }
        }
        // A setting's write as its reader is told it: a retire, or a write
        // it is not told, of a record it was not told where it was proposed
        // is not there to it, as in its twin, whatever became of the record
        // since (063).
        let mut told = logged.clone();
        self.told_setting(&mut told);
        if let ProposedWrite::Retire { ty, key, .. } = &told
            && self.sight.narrows_settings(ty)
        {
            let then = self.lake.engine.view(Some(at))?;
            let there = then.get(ty, key)?.filter(|r| !r.retired);
            if there.is_none_or(|r| self.sight.told(&r).is_none()) {
                return Ok(None);
            }
        }
        // A migration's report kept before reports counted what they
        // compute again computed none, on both sides (062).
        if let ProposedWrite::Migrate { report, .. } = &mut logged
            && report.recomputed.is_none()
        {
            report.recomputed = Some(0);
            let before = Position(at.0.saturating_sub(1));
            report.report_id = crate::writes::report_id(report, Some(before));
        }
        // `more` is what the trim left out, not what the completion added.
        let mut shown = logged.clone();
        let known = self.known_shape(asked, &mut shown, lake, at, (left, then))?;
        // A bundle's plan is its proposer's, its counts made over the
        // lake as it stood then: to a reader kept from a field a count may
        // rest on one it may not read, there or in any lake since, and to a
        // holder of `only` it counts records outside its pairs. Either is
        // shown none; one who holds `schema` asks `plan` for one made now
        // (062).
        if (kept || self.sight.holds_only())
            && let ProposedWrite::Declare { plan, .. } = &mut shown
        {
            *plan = None;
        }
        let shown = known.then_some(shown);
        // A value type's report is made again for its reader, its conditions
        // judged and its id made over what it is shown: the logged one is
        // its proposer's, so the reader's is compared with the whole one,
        // as a reader shown everything is shown it. A setting is compared
        // as its reader is told it, as its twin without what it is not
        // told lists it (063).
        let mut whole = logged.clone();
        self.told_setting(&mut whole);
        if matches!(whole, ProposedWrite::MigrateValueType { .. }) {
            let mut all = asked.clone();
            all.grants = lakelands_api::Grants::owner();
            self.value_type_shown(&all, &|_, _| true, &mut whole, at);
        }
        // A plan is compared on neither side: what its trim leaves out is
        // what its reader may not be told, and counting it would tell twins
        // apart. The act judges every record all the same (062).
        let planless = |w: &ProposedWrite| {
            let mut w = w.clone();
            match &mut w {
                ProposedWrite::Declare { plan, .. } => *plan = None,
                ProposedWrite::Records { plan, .. } => **plan = Default::default(),
                _ => {}
            }
            w
        };
        let more = shown.as_ref().map(planless) != Some(planless(&whole));
        Ok(Some((shown, more)))
    }

    /// `write`, where it is a setting's upsert its reader is not told
    /// ([`Sight::setting`]), as its retire: one it is told is told whole.
    fn told_setting(&self, write: &mut ProposedWrite) {
        let ProposedWrite::Upsert {
            ty,
            key,
            patch,
            opts,
        } = write
        else {
            return;
        };
        let value = patch.get("value").cloned().unwrap_or(Value::Null);
        let setting = ty.as_str() == lakelands_api::seam::SETTING_TYPE;
        if setting && self.sight.narrows(ty) && self.sight.setting(key.as_str(), value).is_none() {
            let (ty, key, opts) = (ty.clone(), key.clone(), opts.clone());
            *write = ProposedWrite::Retire { ty, key, opts };
        }
    }

    /// A proposed write as its reader may know it (013 and 024, extended):
    /// a value where a read of the record would show it, a name where
    /// `describe` would. False when the reader may see nothing of it: a
    /// write to a type it may not see, or to a record it is not answered.
    /// The store logs a migration and a redeclaration completed with the
    /// fields hidden from their author, who reads its own proposal here.
    fn known_shape(
        &self,
        asked: &Principal,
        write: &mut ProposedWrite,
        lake: &OnceCell<LakeDescription>,
        at: Position,
        (left, then): (Under<'_>, Under<'_>),
    ) -> Result<bool> {
        let read = |ty: &TypeName, field: &FieldName| self.sight.reads(ty, field);
        // A record the reader is not answered: restricted, without the
        // grant, or one its `only` does not admit, before the write or
        // after it as proposed (051). An `only` that cannot be applied
        // admits none, whatever a record holds (lakelands#771).
        let only = |ty: &TypeName, f: &BTreeMap<_, _>| {
            self.sight.ty(ty).is_ok() && self.sight.admit(ty).holds(f)
        };
        // To a holder of `only` on the type, the record as the accept
        // plans it: its derived values computed (lakelands#914).
        let o = &self.shape.ontology;
        let view = self.view.as_ref();
        let holder = |ty: &TypeName| o.ty(ty).is_ok_and(|t| self.sight.only_holder(t));
        let now = || Ok::<_, Error>((crate::lake::settings_in(view, o)?, self.judged));
        // A plan that fails on the engine is its outage (lakelands#927);
        // one the write itself fails leaves the record not admitted.
        let outage = |e: Error| match e {
            Error::Unavailable { .. } => Err(e),
            _ => Ok(false),
        };
        let after = |ty: &TypeName, key: &RecordKey, patch: &lakelands_api::Patch| {
            let stored = self.view.get(ty, key)?;
            if holder(ty) {
                let (settings, now) = now()?;
                let at = (view, &settings, now);
                return match crate::writes::planned(
                    self.lake,
                    o,
                    at,
                    stored.as_ref(),
                    (ty, key),
                    patch,
                ) {
                    Ok(planned) => Ok(planned.is_some_and(|r| only(ty, &r.fields))),
                    Err(e) => outage(e),
                };
            }
            let mut fields = (stored.filter(|r| !r.retired))
                .map(|r| r.fields)
                .unwrap_or_default();
            fields.extend(patch.clone());
            Ok::<_, Error>(only(ty, &fields))
        };
        let answered = |ty: &TypeName, key: &RecordKey| answered(view, self.sight, ty, key);
        // A setting as its reader is told it ([`Self::entry`]).
        self.told_setting(write);
        match write {
            // A field the reader may not know of and a masked one both
            // leave the patch: it holds values, and no place for a name.
            // So does one it could not read where the write was proposed,
            // a reference to a type re-keyed since among them: its value
            // is the old key (lakelands#913). A retired record that is not
            // held is a free key, as to the reader's own writes, and so is
            // a setting it is not told (063).
            ProposedWrite::Upsert { ty, key, patch, .. } => {
                let admitted = after(ty, key, patch)?;
                let then_reads = |f: &FieldName| then.is_none_or(|(s, _)| s.reads(ty, f));
                let stored = self.view.get(ty, key)?;
                // A field a hiding lowers on the record, before the write
                // or after it, as a read of it would not tell it (006).
                let before = stored
                    .as_ref()
                    .map(|r| r.fields.clone())
                    .unwrap_or_default();
                let mut made = before.clone();
                made.extend(patch.clone());
                let low = [&before, &made].map(|f| self.sight.lowered(ty, f));
                let told = |f: &FieldName| low.iter().all(|l| !l.contains_key(f));
                patch.retain(|field, _| read(ty, field) && then_reads(field) && told(field));
                let untold = |r: &StoredRecord| r.retired || self.sight.told(r).is_none();
                let free = stored.is_some_and(|r| untold(&r) && !self.sight.withheld(&r));
                Ok(admitted && (free || answered(ty, key)?))
            }
            // A setting retired or not told now is a free key, as to an upsert;
            // the entry was judged where it was proposed (063).
            ProposedWrite::Retire { ty, key, .. } => {
                let stored = (self.view.get(ty, key)?).filter(|_| self.sight.narrows_settings(ty));
                let untold = |r: &StoredRecord| r.retired || self.sight.told(r).is_none();
                let free = stored.is_some_and(|r| untold(&r) && !self.sight.withheld(&r));
                Ok(free || answered(ty, key)?)
            }
            // A payload is what the verb's fields become: whole to a reader
            // who may read each field the verb sets, as it is declared and
            // as it was where the call was proposed, or see the type an
            // executor acts on, and empty otherwise. The verb's name is its
            // type's. One whose verb reads a field the reader may not read
            // is not listed ([`Self::entry`]).
            ProposedWrite::Call {
                verb, key, payload, ..
            } => {
                let Ok(v) = self.shape.ontology.verb(verb) else {
                    return Ok(false);
                };
                let sets = |(sees, o): (&Sight, &Ontology)| match o.verb(verb) {
                    Ok(VerbDecl {
                        on,
                        effect: VerbEffect::Write { set },
                        ..
                    }) => set.iter().all(|a| sees.reads(on, &a.field)),
                    _ => true,
                };
                // A call to a holder of `only` is judged by what its
                // assignments leave, the payload as proposed, as an upsert
                // is (lakelands#914).
                let leaves = |key: &RecordKey| {
                    if !holder(&v.on) {
                        return Ok(true);
                    }
                    let (settings, now) = now()?;
                    let at = (view, &settings, now);
                    match crate::writes::call_reads(o, at, v, Some(key), payload) {
                        Ok((set, _)) => after(&v.on, key, &set),
                        Err(e) => outage(e),
                    }
                };
                let whole = match key {
                    Some(key) => answered(&v.on, key)? && leaves(key)?,
                    None => self.sight.ty(&v.on).is_ok(),
                };
                let force = Some((self.sight, &self.shape.ontology));
                // A field a hiding lowers on some records is not told set.
                let hidden = |a: &lakelands_api::ontology::Assignment| {
                    (self.sight.ty(&v.on)).is_ok_and(|d| self.sight.lowers_somewhere(d, &a.field))
                };
                let lowers =
                    matches!(&v.effect, VerbEffect::Write { set } if set.iter().any(hidden));
                if lowers || ![force, then].into_iter().flatten().all(sets) {
                    payload.clear();
                }
                Ok(whole)
            }
            // The report names what the migration does to the type it was
            // proposed against. A reader kept from a field, where the type
            // was declared again since, is judged by the sight of that
            // lake; one who reads every field is judged on the type in
            // force and is listed the report whole. The totals stay whole.
            // A logged report is lossless, so it names no
            // record that would not convert or would collide. Its id is
            // made again at the head before the proposal.
            ProposedWrite::Migrate { migration, report } => {
                let Ok(was) = self.sight.ty(&migration.to.name) else {
                    return Ok(false);
                };
                let (judge, listing) = then.unwrap_or((self.sight, &self.shape.ontology));
                let made = listing.ty(&migration.to.name).unwrap_or(was);
                let sees = |ty: &TypeName| self.sight.ty(ty).ok();
                let all = || self.sight.describe(&self.sight.who().lake);
                let described = || lake.get_or_init(all);
                judge.shown_report(made, migration, report, described, sees);
                // One kept before a report counted what it computes again
                // computed none (062).
                report.recomputed.get_or_insert(0);
                // A first key only where its reader is answered it now: the
                // report was kept as its proposer saw it (062).
                let to = &migration.to.name;
                let keys = (report.unlive.iter_mut().map(|u| (to, &mut u.sample_keys)))
                    .chain(report.filled.iter_mut().map(|f| (to, &mut f.sample_keys)))
                    .chain(report.fields.iter_mut().map(|c| (to, &mut c.sample_keys)))
                    .chain(
                        report
                            .breaches
                            .iter_mut()
                            .map(|b| (&b.ty, &mut b.sample_keys)),
                    );
                for (ty, keys) in keys {
                    let mut shown = Vec::new();
                    for k in keys.drain(..) {
                        if crate::reads::answered(self.view.as_ref(), self.sight, ty, &k)? {
                            shown.push(k);
                        }
                    }
                    *keys = shown;
                }
                // One listing: its `to` hash is that of the `to` it lists.
                let find = |n: &TypeName| listing.ty(n).ok();
                judge.shown_hashes(made, migration, report, &find);
                let before = Position(at.0.saturating_sub(1));
                report.report_id = crate::writes::report_id(report, Some(before));
                judge.known_migration(made, migration, &find);
                Ok(true)
            }
            // Each declaration as `describe` would show it once the bundle
            // is in force; one it would not show leaves the list.
            ProposedWrite::Declare { declarations, plan } => {
                // Its plan as the reader may know it (062).
                if let Some(p) = plan {
                    crate::writes::shown_plan(p, self.sight, self.view.as_ref())?;
                }
                // The reader as the bundle in force would show it its
                // references (024, extended), from its grants as asked: a
                // reference hidden by the lake as it stands may be shown.
                let Some((sight, after)) = left else {
                    return Ok(false);
                };
                declarations.retain(|d| sight.lists(d));
                for d in declarations.iter_mut() {
                    if let Declaration::Type(t) = d {
                        sight.described_fields(t, &|n| after.ty(n).ok());
                    }
                }
                Ok(!declarations.is_empty())
            }
            // Its uses and conditions as the reader may know them, to one
            // `describe` lists the value type to; to another, nothing of it,
            // as a type's migration to one not shown the type
            // (lakelands#1088).
            ProposedWrite::MigrateValueType { to, .. } => {
                let vt = Declaration::ValueType(lakelands_api::ontology::ValueTypeDecl::clone(to));
                if !self.sight.lists(&vt) {
                    return Ok(false);
                }
                self.value_type_shown(asked, &read, write, at);
                Ok(true)
            }
            // A change to records to a reader who reads every field its
            // condition and its pairs read, as `find` takes them from it; its
            // values as a read of a record would show them. Its plan counts
            // its proposer's records: whole to its proposer and to one who
            // sees all, and else none; the accept plans it for its reviewer
            // (081).
            ProposedWrite::Records {
                ty,
                filter,
                to,
                only,
                plan,
                ..
            } => {
                let probed = match self.named_sets(ty, filter, &[Place::Where]) {
                    Ok(e) => core_derive::reads(&e),
                    Err(_) => return Ok(false),
                };
                if !probed.iter().chain(only.keys()).all(|f| read(ty, f)) {
                    return Ok(false);
                }
                if let lakelands_api::write::ChangeTo::Set(patch) = to {
                    let hidden = |f: &FieldName| {
                        (self.sight.ty(ty)).is_ok_and(|d| self.sight.lowers_somewhere(d, f))
                    };
                    patch.retain(|f, _| read(ty, f) && !hidden(f));
                }
                if !self.sight.sees_all() {
                    **plan = Default::default();
                }
                Ok(true)
            }
            // Judged whole or not at all ([`Reader::computed_entry`]).
            ProposedWrite::Compute { .. } | ProposedWrite::ComputeSweep { .. } => Ok(true),
        }
    }

    /// A proposed call as its reader is shown it (064). It is not there to
    /// one not shown its type, nor, of one record's call, to one not
    /// answered the record. A sweep is there to a reviewer who holds
    /// `restricted` and no `only` alone, since that it is owed tells of a
    /// record held. Of those it is whole to one who reads every input the
    /// function takes and every field it computes on the type; to any other
    /// it is there in part with nothing of it shown, so it may be rejected
    /// and not accepted. A record's call proposed at `at` by a record other
    /// than the one at its key now (retired there since, or moved away by a
    /// re-key, its `made` another) is there to a reviewer who holds
    /// `restricted` and no `only` alone, as a sweep is, with no inputs:
    /// that it was proposed and decided tells of a record the key's
    /// present reader may not have been answered.
    fn computed_entry(
        &self,
        logged: &ProposedWrite,
        at: Position,
    ) -> Result<Option<(Option<ProposedWrite>, bool)>> {
        let (ty, function, key) = match logged {
            ProposedWrite::Compute {
                ty, function, key, ..
            } => (ty, function, Some(key)),
            ProposedWrite::ComputeSweep { ty, function, .. } => (ty, function, None),
            _ => return Ok(None),
        };
        let (o, view) = (&self.shape.ontology, self.view.as_ref());
        let (Ok(decl), Ok(_)) = (o.ty(ty), self.sight.ty(ty)) else {
            return Ok(None);
        };
        let whole_sight = self.sight.sees_all();
        let mut earlier = false;
        if let Some(key) = key {
            if !answered(view, self.sight, ty, key)? {
                return Ok(None);
            }
            // Only a re-key since the call was proposed, of its type or of
            // one its key rests on, moves another record to its key: the
            // key is read as it stood then after one.
            let find = |n: &TypeName| o.ty(n).ok();
            let mut moved = false;
            for n in lakelands_core::describe::shown_by(decl, &find) {
                if moved {
                    break;
                }
                moved = self.rekeyed(n, at)?;
            }
            earlier = match view.get(ty, key)? {
                Some(r) if r.made > at => true,
                Some(r) if moved => {
                    let then = self.lake.engine.view(Some(at))?.get(ty, key)?;
                    then.is_none_or(|t| t.made != r.made)
                }
                _ => false,
            };
            if earlier && !whole_sight {
                return Ok(None);
            }
        }
        let computes = (decl.fields.iter())
            .filter(|f| f.function.as_deref() == Some(function.as_str()))
            .map(|f| f.name.clone());
        let inputs = (o.functions.get(function).into_iter())
            .flat_map(|f| f.decl.inputs.iter().map(|i| i.name().clone()));
        let mut read: Vec<FieldName> = computes.chain(inputs).collect();
        read.dedup();
        // A sweep counts every record of the type, a held one among them:
        // that it is owed at all tells a reader kept from the hold of one.
        // So it is there to a reviewer who holds `restricted` and no
        // `only`, and to no other.
        if key.is_none() && !whole_sight {
            return Ok(None);
        }
        // A field a hiding lowers on the record is read as one it may not.
        let rows = match key {
            Some(key) => view.get_many(ty, std::slice::from_ref(key))?,
            None => Vec::new(),
        };
        let low = (rows.first()).map(|r| self.sight.lowered(ty, &r.fields));
        let told = |f: &FieldName| low.as_ref().is_none_or(|l| !l.contains_key(f));
        let whole = read.iter().all(|f| self.sight.reads(ty, f) && told(f));
        if !whole {
            return Ok(Some((None, true)));
        }
        let mut shown = logged.clone();
        if let (ProposedWrite::Compute { inputs, .. }, Some(_), Some(f)) =
            (&mut shown, key, o.functions.get(function))
        {
            // The inputs are the record's as its reader is answered them,
            // never the log's: a source forgotten leaves none behind (058).
            // A retired record's are none, whatever was made at its key since.
            *inputs = (rows.first().filter(|_| !earlier))
                .map(|r| crate::backed::call_of(&f.decl, &r.fields)["inputs"].clone());
        }
        Ok(Some((Some(shown), false)))
    }

    /// A value type's migration as `asked` may know it, where `read` says
    /// which fields it reads: a use says something of a field's values, so
    /// it is shown only in a field the reader may read, of a type it may see
    /// (024), with its renames' counts. The totals and `fits` stay whole, as
    /// the preflight's do (`Call::shown_value_type`); a logged report fits,
    /// so its `first` names no misfit. Its conditions are judged again for
    /// the reader with its grants at the proposal, as the lake stood before
    /// it, where its id is made again; a failed read of that lake tells each
    /// only as one there may be.
    fn value_type_shown(
        &self,
        asked: &Principal,
        read: &dyn Fn(&TypeName, &FieldName) -> bool,
        write: &mut ProposedWrite,
        at: Position,
    ) {
        let ProposedWrite::MigrateValueType {
            to,
            renames,
            report,
        } = write
        else {
            return;
        };
        report.fields.retain(|u| read(&u.ty, &u.field));
        let before = Position(at.0.saturating_sub(1));
        if !renames.is_empty() {
            let old = renames
                .iter()
                .map(|(o, n)| (o.as_str(), n.as_str()))
                .collect();
            let view = self.lake.engine.view(Some(before));
            // Its references held as that lake shows them (024, extended).
            let judged = view.and_then(|v| {
                let o = Ontology::at(v.as_ref())?;
                let then = Sight::new(asked.clone(), &self.shape.renames, Some(at), &o);
                crate::writes::conditions(&o, v.as_ref(), then.who(), report, &old)
            });
            if judged.is_err() {
                report.conditions.clear();
                report.conditions_unshown = true;
            }
        }
        report.report_id = String::new();
        report.report_id = crate::writes::value_report_id(report, to, renames, before.0);
    }
}

const PROPOSAL_MEMBERS: [&str; 9] = [
    "id",
    "position",
    "proposer",
    "proposed_at",
    "write",
    "why",
    "state",
    "decided_by",
    "reason",
];

/// Proposals in the caller's `order_by`, applied by the store to the
/// engine's newest-first answer: each key a member of the proposal,
/// values in the one sort order, a proposal without a value after every
/// proposal with one; the engine's order breaks ties. A key that names no
/// member of a proposal, or orders by provenance, is refused.
fn ordered_proposals(proposals: Vec<Proposal>, order_by: &[Order]) -> Result<Vec<Proposal>> {
    if order_by.is_empty() {
        return Ok(proposals);
    }
    for o in order_by {
        if o.by != OrderBy::Value || !PROPOSAL_MEMBERS.contains(&o.field.as_str()) {
            let (members, field, by) = (PROPOSAL_MEMBERS.iter().collect(), o.field.clone(), o.by);
            return Err(Error::invalid(Case::ProposalOrder { members, field, by }));
        }
    }
    let mut keyed: Vec<(core_engines::Place<'_>, Proposal)> = proposals
        .into_iter()
        .map(|p| {
            let as_json = serde_json::to_value(&p).expect("a proposal serialises");
            let values = order_by
                .iter()
                .map(|o| {
                    as_json
                        .get(o.field.as_str())
                        .filter(|v| !v.is_null())
                        .cloned()
                })
                .collect();
            (core_engines::Place { values, key: "" }, p)
        })
        .collect();
    keyed.sort_by(|a, b| core_engines::cmp_places(&a.0, &b.0, order_by));
    Ok(keyed.into_iter().map(|(_, p)| p).collect())
}

/// The type the verb, trigger or set in force under `ident` (its
/// declaration's identity) stands on in `o`.
pub(super) fn stands(o: &Ontology, ident: &str) -> Option<TypeName> {
    match ident.split_once(' ')? {
        ("verb", name) => o.verb(&name.into()).ok().map(|v| v.on.clone()),
        ("trigger", name) => o.triggers.get(name).map(|t| t.decl.on.clone()),
        ("object_set", name) => o.set(name).map(|s| s.of.clone()),
        _ => None,
    }
}

/// The declaration in force under `d`'s name, of its kind; a value type
/// is shown to every reader, and is left out.
fn in_force_under(o: &Ontology, d: &Declaration) -> Option<Declaration> {
    match d {
        Declaration::Type(t) => o.ty(&t.name).ok().cloned().map(Declaration::Type),
        Declaration::Verb(v) => o.verb(&v.name).ok().cloned().map(Declaration::Verb),
        Declaration::Trigger(t) => o
            .triggers
            .get(t.name.as_str())
            .map(|e| Declaration::Trigger(e.decl.clone())),
        Declaration::ObjectSet(s) => o.set(&s.name).cloned().map(Declaration::ObjectSet),
        Declaration::Function(f) => o
            .functions
            .get(&f.name)
            .map(|e| Declaration::Function(e.decl.clone())),
        Declaration::Setting(s) => {
            (o.settings.get(&s.key)).map(|e| Declaration::Setting(e.decl.clone()))
        }
        Declaration::Withdraw(_) => o.held(d),
        _ => None,
    }
}
