//! The describe door and the reads beneath it: counts, pages, views and the
//! kept state a read of a type is made over.

use super::*;

impl<'a> Reader<'a> {
    /// `describe`, as `asked` asked it.
    pub fn describe(&self, asked: &Principal) -> Result<LakeDescription> {
        let mut lake = self.sight.describe(&self.lake.id);
        for t in &mut lake.types {
            self.name_sorted(t);
        }
        self.renamed_since(asked, &mut lake.types)?;
        // Whether the host binds a verb's executor is the binding now, at
        // any position: it is not in the log.
        let executors = &self.lake.seams.executors;
        for e in lake.verbs.iter_mut().filter_map(|v| v.executor.as_mut()) {
            e.bound = executors.iter().any(|x| x.name() == e.name);
        }
        // A host's setting is told with its declaration to a holder of
        // `schema` and to one who may write it (063, extended), with its
        // values from dates and the one in force on the read's clock (090).
        let host = (self.shape.ontology.settings.values())
            .filter(|e| self.sight.setting_visible(&e.decl))
            .map(|e| {
                use lakelands_api::settings::{in_force, periods_key, told_periods};
                let value = self.shape.settings.get(&e.decl.key);
                let periods = self.shape.settings.get(&periods_key(&e.decl.key));
                lakelands_api::describe::SettingValue {
                    key: e.decl.key.clone(),
                    value: value.map(|v| in_force(v.clone(), periods, self.now)),
                    default: lakelands_api::Value::Null,
                    declared: Some(e.decl.clone()),
                    periods: told_periods(value, periods),
                }
            });
        let host: Vec<_> = host.collect();
        if self.sight.who().grants.schema {
            // A setting of a type it is not shown is told as were the type
            // absent, one naming fields of a type whole or not at all, and
            // one for a source only where nothing is held from it.
            let shown = |ty: &str| self.sight.ty(&ty.into()).is_ok();
            let told = lakelands_api::settings::told(&self.shape.settings).into_iter();
            lake.settings = told
                .filter(|s| self.sight.setting_told(&s.key))
                .filter_map(|s| lakelands_api::settings::seen(s, &shown, &|_, _| true))
                .collect();
        }
        lake.settings.extend(host);
        Ok(lake)
    }

    /// `describe_type`, as `asked` asked it.
    pub fn describe_type(&self, asked: &Principal, ty: &TypeName) -> Result<TypeDescription> {
        let missing = Missing::Type { name: ty.clone() };
        let mut t = (self.sight.describe_type(ty)).ok_or(Error::NotFound { missing })?;
        self.name_sorted(&mut t);
        self.renamed_since(asked, std::slice::from_mut(&mut t))?;
        Ok(t)
    }

    /// Tell each type described as of a position the names it bore from
    /// it on, from the lake's list of renames at the head, where `asked`, its
    /// grants named there, is shown it under its last name: a name is told
    /// only to a reader shown the type at both positions (006, extended).
    fn renamed_since(&self, asked: &Principal, types: &mut [TypeDescription]) -> Result<()> {
        if self.at_head {
            return Ok(());
        }
        let view = self.lake.engine.view(None)?;
        let shape = self.lake.shape(view.as_ref())?;
        let list = &shape.renames;
        let (head, _) =
            (self.lake).sight(asked.clone(), (None, true), Some(list), shape.as_ref())?;
        // One at the position itself is told: the link type stands there
        // under its old name, and its links walk by the new one.
        let since = list.iter().filter(|r| r.at >= self.position);
        for t in types {
            let (mut last, mut renamed) = (t.name.clone(), Vec::new());
            for r in since.clone() {
                if let Some(to) = r.to.as_ref().filter(|_| r.ty == last) {
                    renamed.push(r.clone());
                    last = to.clone();
                }
            }
            if head.ty(&last).is_ok() {
                t.lineage.renamed = renamed;
            }
        }
        Ok(())
    }

    /// Name in a type's lifecycle the fields the lake keeps an order index
    /// for and the vector fields it asks an index for, as its
    /// `sort_index.<type>` and `vector_index.<type>` settings ask, of those
    /// the principal sees and where it is told the setting; and for a view
    /// whether it is kept, as its `view_kept.<type>` setting asks.
    fn name_sorted(&self, t: &mut TypeDescription) {
        let Ok(decl) = self.shape.ontology.ty(&t.name) else {
            return;
        };
        let seen = |f: &FieldName| t.fields.iter().any(|d| &d.name == f);
        let settings = &self.shape.settings;
        // A view's order index is its kept records'.
        let ordered = match decl.view {
            Some(_) => lakelands_api::seam::kept_name(&t.name),
            None => t.name.clone(),
        };
        // A setting its reader is not told is named as were it absent.
        let set = |key: String| settings.get(&key).filter(|_| self.sight.setting_told(&key));
        let setting = set(core_engines::sort_index_key(&ordered));
        t.lifecycle.sorted = core_engines::sorted_fields(&ordered, decl, setting);
        t.lifecycle.sorted.retain(seen);
        // What the lake chose by itself, told like the pages it sorted
        // whole: to a holder of `schema` who reads the field, and at the
        // head only, where it is kept (041).
        let chosen = self.view.chosen(&t.name).ok().filter(|_| self.at_head);
        let chosen = chosen.unwrap_or_default();
        let schema = self.sight.who().grants.schema;
        let reads = |f: &&FieldName| schema && self.sight.reads(&t.name, f) && seen(f);
        t.lifecycle.chosen = chosen.kept.iter().filter(reads).cloned().collect();
        // What was ordered by a whole sort, told to a holder of `schema`
        // who reads the field, who can weigh `sort_index.<type>`, and never
        // once it is indexed: its pages count every principal's reads and
        // its time grows with records a reader may not be answered
        // (lakelands#1029).
        let tally = lakelands_core::lock(&self.lake.sorted_whole).clone();
        let schema = self.sight.who().grants.schema;
        let told = |f: &FieldName| {
            schema
                && self.sight.reads(&t.name, f)
                && !t.lifecycle.sorted.contains(f)
                && !chosen.kept.contains(f)
                && !chosen.building.contains(f)
        };
        t.lifecycle.sorted_whole = (tally.into_iter())
            .filter(|((ty, f), _)| *ty == t.name && told(f))
            .map(|((_, field), (pages, micros))| SortedWhole {
                field,
                pages,
                millis: micros / 1000,
            })
            .collect();
        let setting = set(core_engines::vector_index_key(&t.name));
        t.lifecycle.indexed = core_engines::indexed_fields(decl, setting);
        t.lifecycle.indexed.retain(seen);
        if let Some(view) = decl.view.as_deref() {
            let key = format!("{}{}", lakelands_api::seam::VIEW_KEPT_SETTING, t.name);
            let (setting, shown) = (settings.get(&key), t.view.is_some());
            let declared = |ty: &TypeName| self.shape.ontology.ty(ty).ok();
            t.lifecycle.kept = Some(Kept::by_declaration(view, setting, shown, declared));
            // What the declarations leave open, the view's state at this
            // read answers: the store keeps views.
            let open = Kept::Computed {
                why: NotKept::NotKeptHere,
            };
            // To a holder of `only` a view is its own records, never every
            // owner's groups: whether those are exact is not told (051).
            if !self.whole(decl) || self.computes(&t.name) {
                t.lifecycle.kept = Some(Kept::Computed {
                    why: NotKept::OnlySome,
                });
            // Told to one the view's read would answer, of that reader's
            // face: a number of groups says something of their records. A
            // read held as now computes every view, as `kept_as` does.
            } else if t.lifecycle.kept.as_ref() == Some(&open)
                && !self.held_now
                && self.probe_view(view, false).is_ok()
            {
                let face = usize::from(self.sight.sees_restricted());
                t.lifecycle.kept = Some(match self.kept_read(&t.name, view, face) {
                    Ok(Some((_, 0))) => Kept::Kept,
                    Ok(Some((_, not_exact))) => Kept::Computed {
                        why: NotKept::NotWhole { not_exact },
                    },
                    _ => open,
                });
            }
            // An index is named where the view is kept, or asked to be and
            // can be, as `kept` tells this reader (lakelands#748).
            if t.lifecycle
                .kept
                .as_ref()
                .is_some_and(|k| !k.names_its_index())
            {
                t.lifecycle.sorted.clear();
            }
        }
    }

    /// Whether a read's condition holds over `r`, with its provenance
    /// where `meta`: a set across an edge it names is answered by the
    /// set's kept membership.
    pub(super) fn holds(&self, e: &Expr, r: &StoredRecord, meta: bool) -> bool {
        let record = RecordScope {
            fields: &r.fields,
            meta: meta.then_some(&r.meta),
            settings: Some(self.told()),
            now: self.now,
        };
        let members = self.members.borrow();
        let scope = Members {
            record,
            key: &r.key,
            members: &members,
        };
        expr::holds(e, &scope).unwrap_or(false)
    }

    /// The records of a type at this read's position that the principal
    /// is allowed to see, before masking: live, restricted ones only with
    /// the grant, valid at `valid_at` when asked. `filter` is pushed down
    /// when the engine can apply it.
    pub(super) fn records(
        &self,
        decl: &TypeDecl,
        filter: Option<&Expr>,
    ) -> Result<Vec<StoredRecord>> {
        if let Some(v) = &decl.view {
            return self.view_records(decl, v, filter);
        }
        self.page(decl, filter, &[], None, usize::MAX)
    }

    /// How many records of a type [`Self::records`] would answer, holding
    /// no more than a page of a stored type: the engine counts what it can
    /// select itself, and a filter only the store evaluates is applied a
    /// page at a time. A view's records, one per group, are computed first.
    pub(super) fn count(&self, decl: &TypeDecl, filter: Option<&Expr>) -> Result<u64> {
        if let Some((stored, whole)) = self.kept(decl)? {
            // The engine counts the stored records; a group whose whole
            // face reads otherwise is counted by that face instead.
            let n = self.count(&stored, filter)?;
            let passes = |r: &StoredRecord| filter.is_none_or(|e| self.holds(e, r, false));
            let differ = self.differing(&stored, &decl.name, whole)?;
            let own = differ.iter().filter(|(r, _)| passes(r)).count() as u64;
            let faces = differ.iter().filter_map(|(_, w)| w.as_ref());
            let faces = faces.filter(|w| passes(w)).count() as u64;
            return Ok((n + faces).saturating_sub(own));
        }
        if let Some(v) = &decl.view {
            return Ok(self.view_records(decl, v, filter)?.len() as u64);
        }
        // A kept name's filtered count goes by the pages a read of it goes
        // by, whose order is checked there: the engine's own count pages by
        // a last key it does not check.
        let kept = lakelands_api::seam::is_kept_name(&decl.name);
        let Some(equal) = self.sight.admit(&decl.name).pairs().map(<[_]>::to_vec) else {
            return Ok(0);
        };
        // A link's ends are judged a page at a time, never by the engine,
        // and so is a setting its reader may not be told.
        let ends = self.sight.judges_ends(decl) || self.sight.narrows(&decl.name);
        let bounded = self.with_bound(decl, filter);
        let filter = bounded.as_ref();
        if ends || filter.is_some_and(|e| kept || self.store_only(decl, e)) {
            let mut n = 0;
            self.each_member(decl, filter, COUNT_PAGE, |_| {
                n += 1;
                Ok(())
            })?;
            return Ok(n);
        }
        let scan = Scan {
            filter: filter.cloned(),
            equal,
            valid_at: self.valid_at_for(decl),
            retired: self.retired.get(),
            restricted: self.sight.sees_restricted(),
            ..Default::default()
        };
        self.view.count(&decl.name, &scan)
    }

    /// How many retired records of a type [`Self::count`] would count were
    /// they live, under the same filter, grants and position: the count
    /// with the retired ones, less the live count, `live` where the read
    /// counted it already. A view's records are computed and never
    /// retired (045).
    pub(super) fn retired_count(
        &self,
        decl: &TypeDecl,
        filter: Option<&Expr>,
        live: Option<u64>,
    ) -> Result<u64> {
        if decl.view.is_some() {
            return Ok(0);
        }
        let live = live.map_or_else(|| self.count(decl, filter), Ok)?;
        self.retired.set(true);
        let all = self.count(decl, filter);
        self.retired.set(false);
        Ok(all?.saturating_sub(live))
    }

    /// The records [`Self::records`] would answer, handed to `each` one at
    /// a time in key order until it fails: a stored type's and a kept
    /// view's are read `page` at a time and never held together, a
    /// computed view's are computed first.
    pub(super) fn each_member(
        &self,
        decl: &TypeDecl,
        filter: Option<&Expr>,
        page: usize,
        mut each: impl FnMut(&StoredRecord) -> Result<()>,
    ) -> Result<()> {
        if let Some(kept) = self.kept(decl)? {
            return self.each_kept(&decl.name, kept, filter, page, each);
        }
        // A view is computed once a read, however many views read it, and
        // a reader's `where` on it held after, as its computation holds it
        // (lakelands#942).
        if let Some(all) = self.computed(&decl.name)? {
            let bounded = self.with_bound(decl, filter);
            let held =
                |r: &&StoredRecord| (bounded.as_ref()).is_none_or(|e| self.holds(e, r, false));
            return all.iter().filter(held).try_for_each(each);
        }
        self.each_stored(decl, filter, page, &mut each)
    }

    /// [`Self::each_member`] for a stored type or a kept name: its records
    /// read `page` at a time from the engine.
    fn each_stored(
        &self,
        decl: &TypeDecl,
        filter: Option<&Expr>,
        page: usize,
        each: &mut dyn FnMut(&StoredRecord) -> Result<()>,
    ) -> Result<()> {
        let mut after = None;
        loop {
            let rows = self.page(decl, filter, &[], after, page)?;
            rows.iter().try_for_each(&mut *each)?;
            if rows.len() < page {
                return Ok(());
            }
            after = rows.last().map(|r| (Vec::new(), r.key.clone()));
        }
    }

    /// At most `want` records of a stored type the principal may see, in
    /// `order` after `after`, read from the engine a page at a time: the
    /// filter goes down when the engine can apply it, and is applied here
    /// over one engine page after another when it cannot.
    pub(super) fn page(
        &self,
        decl: &TypeDecl,
        filter: Option<&Expr>,
        order: &[Order],
        after: Option<(Vec<Value>, RecordKey)>,
        want: usize,
    ) -> Result<Vec<StoredRecord>> {
        let bounded = self.with_bound(decl, filter);
        let filter = bounded.as_ref();
        let push = filter.filter(|e| !self.store_only(decl, e)).cloned();
        let here = filter.filter(|_| push.is_none());
        let admit = self.sight.admit(&decl.name);
        let Some(equal) = admit.pairs().map(<[_]>::to_vec) else {
            return Ok(Vec::new());
        };
        let mut scan = Scan {
            filter: push,
            equal,
            order_by: order.to_vec(),
            after: after.map(|(values, key)| After { values, key }),
            limit: u32::try_from(want).unwrap_or(u32::MAX),
            valid_at: self.valid_at_for(decl),
            retired: self.retired.get(),
            restricted: self.sight.sees_restricted(),
            ..Default::default()
        };
        let mut out = Vec::new();
        loop {
            let rows = self.view.scan(&decl.name, &scan)?;
            // An engine answers a kept name as of an earlier position in
            // key order by its statement's plan alone, and this read goes
            // on from a page's last key: a page in another order would
            // skip or repeat a group without a word.
            if order.is_empty() && lakelands_api::seam::is_kept_name(&decl.name) {
                let mut last = scan.after.as_ref().map(|a| &a.key);
                for r in &rows {
                    if let Some(before) = last.filter(|k| **k >= r.key) {
                        let flaw = lakelands_api::error::Flaw::OutOfKeyOrder {
                            ty: decl.name.clone(),
                            key: r.key.clone(),
                            before: before.clone(),
                        };
                        let part = lakelands_api::error::Part::Projection;
                        return Err(lakelands_core::damaged(part, None, flaw));
                    }
                    last = Some(&r.key);
                }
            }
            // The engine's page, before a setting is told, says where the
            // next one starts and whether there is one.
            let last_page = rows.len() < scan.limit as usize;
            if let Some(last) = rows.last() {
                scan.after = Some(After {
                    values: Self::sort_values(order, last),
                    key: last.key.clone(),
                });
            }
            let rows = self.at_read(&decl.name, rows);
            let ends = self.admitted_ends(decl, &rows)?;
            for (r, _) in rows.into_iter().zip(ends).filter(|(_, ends)| *ends) {
                if self.live_by(decl, &admit, &r) && here.is_none_or(|e| self.holds(e, &r, true)) {
                    out.push(r);
                    if out.len() >= want {
                        return Ok(out);
                    }
                }
            }
            if last_page {
                return Ok(out);
            }
            // An engine page ordered by a field sorts the type again, so the
            // store asks for twice as many rows each time it reads on: a
            // few sorts per page, not one per `want` rows. A key-ordered
            // read stays at `want` rows, which is what bounds its memory.
            if !order.is_empty() {
                scan.limit = scan.limit.saturating_mul(2);
            }
        }
    }

    /// A view type's records, computed. A view met again while it is
    /// still being computed is a cycle over `source` that came in with a
    /// log `declare` did not check; it is refused, naming the chain, never
    /// followed.
    fn view_records(
        &self,
        decl: &TypeDecl,
        v: &lakelands_api::ontology::ViewDecl,
        filter: Option<&Expr>,
    ) -> Result<Vec<StoredRecord>> {
        let bounded = self.with_bound(decl, filter);
        let filter = bounded.as_ref();
        if let Some(at) = self.computing.borrow().iter().position(|t| *t == decl.name) {
            let chain = self.computing.borrow()[at..]
                .iter()
                .chain(std::iter::once(&decl.name))
                .collect();
            let ty = decl.name.clone();
            return Err(Error::invalid(Case::ViewReadsItself { ty, chain }));
        }
        self.computing.borrow_mut().push(decl.name.clone());
        let records = match self.kept_as(decl, v) {
            Ok(Some(kept)) => {
                let mut all = Vec::new();
                let each = |r: &StoredRecord| {
                    all.push(r.clone());
                    Ok(())
                };
                (self.each_kept(&decl.name, kept, filter, COUNT_PAGE, each)).map(|()| all)
            }
            Ok(None) => self.computed_view(decl, v, filter),
            Err(e) => Err(e),
        };
        self.computing.borrow_mut().pop();
        // A holder is answered the groups its pairs admit (051).
        let admit = self.sight.admit(&decl.name);
        records.map(|mut r| {
            r.retain(|r| admit.holds(&r.fields));
            r
        })
    }

    /// The records of the kept view `view` that `filter` holds for, as
    /// this reader reads them, handed to `each` in key order until it
    /// fails: read `page` at a time, each shown with the reader's face and
    /// held there to `filter` and the reader's pairs. It never holds the
    /// kept type, whose records carry the fold's state (lakelands#731).
    /// `kept` is what [`Self::kept_as`] answers. The door masks the fields.
    fn each_kept(
        &self,
        view: &TypeName,
        (stored, whole): (TypeDecl, bool),
        filter: Option<&Expr>,
        page: usize,
        mut each: impl FnMut(&StoredRecord) -> Result<()>,
    ) -> Result<()> {
        let admit = self.sight.admit(&stored.name);
        self.each_stored(
            &stored,
            None,
            page,
            &mut |r| match kept::shown(r.clone(), view, whole) {
                Some(r)
                    if filter.is_none_or(|e| self.holds(e, &r, false))
                        && admit.holds(&r.fields) =>
                {
                    each(&r)
                }
                _ => Ok(()),
            },
        )
    }

    /// The records of the kept view `view` with these keys, as this reader
    /// reads them, `kept` as [`Self::kept_as`] answers it: each read by its
    /// key from the engine, as `get` reads one.
    pub(super) fn kept_by_key(
        &self,
        view: &TypeName,
        (stored, whole): (TypeDecl, bool),
        keys: &[RecordKey],
    ) -> Result<Vec<StoredRecord>> {
        let held = self.view.get_many(&stored.name, keys)?;
        let admit = self.sight.admit(&stored.name);
        let live = held
            .into_iter()
            .filter(|r| self.live_by(&stored, &admit, r));
        let shown = live.filter_map(|r| kept::shown(r, view, whole));
        Ok(shown.filter(|r| admit.holds(&r.fields)).collect())
    }

    /// The stored type a kept view's records are read as at this read, the
    /// computation's probes made, and whether the reader takes each
    /// record's whole face: the reader with the `restricted` grant, while
    /// a group's whole face reads otherwise than its open one. `None` where
    /// the read computes: the fold kept none at this position, they are not
    /// whole there, a group of the reader's face is not exact, or the read
    /// names a valid time where one is declared in the view's tree
    /// ([`Self::timed_tree`]), which a kept fold has no axis for. What
    /// depends on the reader is decided here, as the computation decides it.
    fn kept_as(
        &self,
        decl: &TypeDecl,
        v: &lakelands_api::ontology::ViewDecl,
    ) -> Result<Option<(TypeDecl, bool)>> {
        let face = usize::from(self.sight.sees_restricted());
        if self.held_now || (self.valid_at.is_some() && self.timed_tree(decl)) {
            return Ok(None);
        }
        let state = match self.kept_read(&decl.name, v, face)? {
            Some((state, 0)) if !self.computes(&decl.name) => state,
            _ => return Ok(None),
        };
        self.probe_view(v, false)?;
        let stored = TypeDecl {
            name: lakelands_api::seam::kept_name(&decl.name),
            view: None,
            ..decl.clone()
        };
        Ok(Some((stored, face == 1 && state.differ > 0)))
    }

    /// Whether a valid time is declared in the tree of the view `decl`: its
    /// own, or one of a source it reads, merged and nested ones included.
    /// Judged by the declarations alone, never by what the reader is shown,
    /// so a read at `valid_at` reads the kept records of a view alike for
    /// every reader (lakelands#910).
    fn timed_tree(&self, decl: &TypeDecl) -> bool {
        let (mut seen, mut next) = (BTreeSet::new(), vec![decl]);
        while let Some(t) = next.pop() {
            if !seen.insert(&t.name) {
                continue;
            }
            if lakelands_core::valid_time::bounds(t).next().is_some() {
                return true;
            }
            if let Some(v) = &t.view {
                let sources = std::iter::once(&v.source).chain(v.merge.iter().map(|m| &m.source));
                next.extend(sources.filter_map(|s| self.shape.ontology.ty(s).ok()));
            }
        }
        false
    }

    /// [`Self::kept_as`] for a type that may be a view.
    pub(super) fn kept(&self, decl: &TypeDecl) -> Result<Option<(TypeDecl, bool)>> {
        match &decl.view {
            Some(v) => self.kept_as(decl, v),
            None => Ok(None),
        }
    }

    /// The kept records of `view` whose whole face reads otherwise than
    /// their own fields, found by their mark, each with that face (none
    /// for a group on the open face alone, and none the reader's pairs do
    /// not admit: a pair on a measure is judged on both faces). There are
    /// as many as the state's `differ`; the text index finds them at the
    /// head, and a read at an earlier position scans the kept type. None
    /// unless the reader reads the `whole` face.
    pub(super) fn differing(
        &self,
        stored: &TypeDecl,
        view: &TypeName,
        whole: bool,
    ) -> Result<Vec<(StoredRecord, Option<StoredRecord>)>> {
        let admit = self.sight.admit(&stored.name);
        let (true, Some(pairs)) = (whole, admit.pairs()) else {
            return Ok(Vec::new());
        };
        let mut equal = vec![(FieldName::from(kept::DIFFERS), Value::from("1"))];
        equal.extend(pairs.iter().cloned());
        let scan = Scan {
            equal,
            limit: u32::MAX,
            valid_at: self.valid_at_for(stored),
            restricted: self.sight.sees_restricted(),
            ..Default::default()
        };
        let rows = self.view.scan(&stored.name, &scan)?;
        let admitted = |w: &StoredRecord| admit.holds(&w.fields);
        let face = |r: StoredRecord| (r.clone(), kept::shown(r, view, true).filter(admitted));
        Ok(rows.into_iter().map(face).collect())
    }

    /// A page of a kept view as its reader reads it, after `after`: the
    /// engine's page of the stored records, each of the open face; or, for
    /// the reader of the whole face, the engine's page of the records whose
    /// faces do not differ merged in the order with the whole faces of
    /// those that do (`differ`), filtered here. It holds `want` records and
    /// those faces.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn kept_rows(
        &self,
        stored: &TypeDecl,
        view: &TypeName,
        filter: Option<&Expr>,
        order: &[Order],
        differ: Option<&[StoredRecord]>,
        after: Option<SortKey>,
        want: usize,
    ) -> Result<Vec<StoredRecord>> {
        let Some(differ) = differ else {
            let rows = self.page(stored, filter, order, after, want)?;
            return Ok((rows.into_iter())
                .filter_map(|r| kept::shown(r, view, false))
                .collect());
        };
        let path = vec![FieldName::from(kept::DIFFERS)];
        let same = Expr::IsNull {
            arg: Box::new(Expr::Path { path }),
        };
        let args = filter.cloned().into_iter().chain([same]).collect();
        let rows = self.page(
            stored,
            Some(&Expr::And { args }),
            order,
            after.clone(),
            want,
        )?;
        let at = |r: &StoredRecord| (Self::sort_values(order, r), r.key.clone());
        let later = |r: &&StoredRecord| {
            filter.is_none_or(|e| self.holds(e, r, false))
                && after
                    .as_ref()
                    .is_none_or(|a| Self::compare(order, &at(r), a).is_gt())
        };
        let mut rows: Vec<StoredRecord> = (rows.into_iter())
            .filter_map(|r| kept::shown(r, view, true))
            .chain(differ.iter().filter(later).cloned())
            .collect();
        rows.sort_by(|a, b| Self::compare(order, &at(a), &at(b)));
        rows.truncate(want);
        Ok(rows)
    }

    fn computed_view(
        &self,
        decl: &TypeDecl,
        v: &lakelands_api::ontology::ViewDecl,
        filter: Option<&Expr>,
    ) -> Result<Vec<StoredRecord>> {
        let literals = match self.bound.borrow().get(&decl.name) {
            Some((_, literals)) => literals.clone(),
            None if v.params.is_empty() => BTreeMap::new(),
            // A view reached by no read that binds it: a walk's step, a
            // search.
            None => return Err(Error::invalid(unbound(&decl.name, &v.params[0], true))),
        };
        // Every source's probes before a record is read, so what a reader
        // is told does not turn on the order of the sources (024).
        self.probe_view(v, true)?;
        let (members, measures) = self.view_source(
            &v.source,
            v.r#where.as_ref(),
            &v.group_by,
            &v.measures,
            &literals,
        )?;
        // A view record is keyed like any record: its group_by values, in
        // declared order, by the one key rule. Sources meet where their
        // keys are identical (044's identity of values, not 017's `=`).
        let keyed_by = TypeDecl {
            key: v.group_by.clone(),
            ..decl.clone()
        };
        // A view without `group_by` is one total, kept over no records too,
        // under the key reserved for it: the rule over no fields makes none.
        let (ungrouped, none) = (v.group_by.is_empty(), aggregate::Folds::default());
        // A group's values under the names the other side groups by, field for field.
        let renamed = |values: &aggregate::Row, from: &[FieldName], to: &[FieldName]| {
            let pairs = to.iter().zip(from);
            aggregate::Row::from_iter(pairs.map(|(to, from)| (to.clone(), values[from].clone())))
        };
        let key_of = |by: &BTreeMap<FieldName, Value>| {
            if ungrouped {
                Some(RecordKey::from(lakelands_api::key::UNGROUPED))
            } else {
                lakelands_core::log::record_key(&keyed_by, by)
            }
        };
        // A group keeps its values and its folds, never its members.
        let mut groups: BTreeMap<RecordKey, (aggregate::Row, aggregate::Folds)> = BTreeMap::new();
        for (by, folds) in members.groups() {
            if by.values().any(Value::is_null) {
                continue;
            }
            if let Some(key) = key_of(&by) {
                groups.insert(key, (by, folds));
            }
        }
        if ungrouped {
            groups
                .entry(RecordKey::from(lakelands_api::key::UNGROUPED))
                .or_default();
        }
        // One group's measures, seeing the values `given` by name.
        let settings = Some(self.told());
        // A measure that fails is named with its group, as one that does
        // not land is (023, lakelands#925).
        let failed = |group: &RecordKey, name: FieldName, why: Case| {
            let (group, why) = (group.clone(), Box::new(why));
            let said = Said::from(Case::MeasureFails { group, why });
            let view = Place::View {
                name: decl.name.clone(),
            };
            Error::invalid(said.within(Place::Measure { name }).within(view))
        };
        let measured = |folds: &aggregate::Folds,
                        by: &BTreeMap<FieldName, Value>,
                        measures: &[(FieldName, Expr)],
                        given,
                        key: &RecordKey| {
            aggregate::named_measures(folds, by, measures, given, settings, self.now)
                .map_err(|(name, why)| failed(key, name, why))
        };
        // Each merged source's measures by key; a key it lacks is measured
        // over its empty group below.
        let mut merged = Vec::with_capacity(v.merge.len());
        for m in &v.merge {
            let (members, measures) = self.view_source(
                &m.source,
                m.r#where.as_ref(),
                &m.group_by,
                &m.measures,
                &literals,
            )?;
            let mut at = BTreeMap::new();
            for (own, folds) in members.groups() {
                if own.values().any(Value::is_null) {
                    continue;
                }
                let by = renamed(&own, &m.group_by, &v.group_by);
                let Some(key) = key_of(&by) else {
                    continue;
                };
                let values = measured(&folds, &own, &measures, BTreeMap::new(), &key)?;
                groups
                    .entry(key.clone())
                    .or_insert_with(|| (by, none.clone()));
                at.insert(key, values);
            }
            merged.push((m, measures, at));
        }
        let mut out = Vec::new();
        for (key, (by, folds)) in groups {
            let mut given = BTreeMap::new();
            for (m, measures, at) in &merged {
                let values = match at.get(&key) {
                    Some(values) => values.clone(),
                    None => {
                        let own = renamed(&by, &v.group_by, &m.group_by);
                        measured(&none, &own, measures, BTreeMap::new(), &key)?
                    }
                };
                given.extend(values);
            }
            let values = measured(&folds, &by, &measures, given, &key)?;
            // A measure that does not land fails as one that cannot be
            // computed: the read is refused, naming it and its group.
            let fields = landed(decl, by.clone(), values).map_err(|(name, why)| {
                let (group, decimal) = (key.clone(), matches!(why, Case::IntegerTooLarge { .. }));
                let why = Box::new(why);
                let said = Said::from(Case::MeasureUnfit {
                    group,
                    why,
                    decimal,
                });
                let view = Place::View {
                    name: decl.name.clone(),
                };
                Error::invalid(said.within(Place::Measure { name }).within(view))
            })?;
            out.push(StoredRecord {
                ty: decl.name.clone(),
                key,
                version: lakelands_api::Version(1),
                fields,
                meta: BTreeMap::new(),
                position: self.position,
                made: self.position,
                retired: false,
                restricted: false,
            });
        }
        if let Some(e) = filter {
            out.retain(|r| self.holds(e, r, false));
        }
        // Its own valid time keeps its records as a type's does, by the
        // bounds its reader is shown: here those it is shown each of, the
        // rest by `with_bound`'s condition (006, lakelands#910).
        if let Some(t) = self.valid_at_for(decl) {
            out.retain(|r| lakelands_core::valid_time::record_holds(decl, &r.fields, t));
        }
        Ok(out)
    }

    /// One source of a view, read as a read of that source: the records
    /// its `where` covers, the parameters bound, folded into their groups
    /// as they are read, and its measures, after
    /// the principal is found to read every field they and the grouping
    /// read (024).
    fn view_source(
        &self,
        source: &TypeName,
        filter: Option<&lakelands_api::expr::Condition>,
        group_by: &[FieldName],
        measures: &[lakelands_api::ontology::Measure],
        literals: &BTreeMap<String, Literal>,
    ) -> Result<Source> {
        let (source, where_, measures) =
            self.view_probe(source, filter, group_by, measures, literals, true)?;
        // A `where` that read a parameter is evaluated here, never handed to
        // an engine: a bound date is text, and its arithmetic is the
        // evaluator's.
        let (down, here) = match literals.is_empty() {
            true => (where_.as_ref(), None),
            false => (None, where_.as_ref()),
        };
        let keys = group_by.iter().map(|f| (f.clone(), None)).collect();
        let mut members = aggregate::Grouping::new(keys, &measures, self.now);
        self.each_member(&source, down, COUNT_PAGE, |r| {
            if here.is_none_or(|w| self.holds(w, r, true)) {
                members.add((&r.fields, Some(&r.meta)), Some(self.told()));
            }
            Ok(())
        })?;
        Ok((members, measures))
    }

    /// The state of a kept view whose records are whole at this read, as
    /// are those of every view it reads through, and among them all the
    /// groups that are not exact for `face`: the outer folds an inner
    /// group as its record stands, so the outer is the computation's only
    /// while the inner is exact. `None` where the fold keeps no whole
    /// records of one of them: before it was made, or ended; and, with no
    /// state read, where no `view_kept.` setting asks for it.
    fn kept_read(
        &self,
        ty: &TypeName,
        v: &lakelands_api::ontology::ViewDecl,
        face: usize,
    ) -> Result<Option<KeptRead>> {
        let at = (ty.clone(), face);
        if let Some(known) = self.kept_reads.borrow().get(&at) {
            return Ok(known.clone());
        }
        let read = self.kept_read_once(ty, v, face)?;
        self.kept_reads.borrow_mut().insert(at, read.clone());
        Ok(read)
    }

    /// [`Self::kept_read`], asked: each source that is a view through
    /// [`Self::kept_read`] again.
    fn kept_read_once(
        &self,
        ty: &TypeName,
        v: &lakelands_api::ontology::ViewDecl,
        face: usize,
    ) -> Result<Option<KeptRead>> {
        let asked = format!("{}{ty}", lakelands_api::seam::VIEW_KEPT_SETTING);
        if !lakelands_api::seam::view_kept(self.shape.settings.get(&asked)) {
            return Ok(None);
        }
        let Some(state) = kept::state(self.view.as_ref(), ty)?.filter(|s| s.whole) else {
            return Ok(None);
        };
        let mut not_exact = state.not_exact[face];
        for source in std::iter::once(&v.source).chain(v.merge.iter().map(|m| &m.source)) {
            if let Some(inner) = self.shape.ontology.ty(source)?.view.as_deref() {
                let Some((_, n)) = self.kept_read(source, inner, face)? else {
                    return Ok(None);
                };
                not_exact += n;
            }
        }
        Ok(Some((state, not_exact)))
    }

    /// The probes computing a view would make, in the computation's order
    /// and with its refusals, and nothing read: each source's, its own
    /// first, and after each those of a source that is itself a view,
    /// which computing would read there; with those its valid time filters
    /// by where the source is `read` for the computation.
    fn probe_view(&self, v: &lakelands_api::ontology::ViewDecl, read: bool) -> Result<()> {
        let own = (&v.source, v.r#where.as_ref(), &v.group_by, &v.measures);
        let merged = v.merge.iter();
        let merged = merged.map(|m| (&m.source, m.r#where.as_ref(), &m.group_by, &m.measures));
        for (source, filter, group_by, measures) in std::iter::once(own).chain(merged) {
            let (source, ..) =
                self.view_probe(source, filter, group_by, measures, &BTreeMap::new(), read)?;
            // Marked before it is walked: a cycle of views, which a log
            // the store did not write may hold, is refused where it is
            // read, not walked here for ever.
            let probed = (source.name.clone(), read);
            if let Some(inner) = &source.view
                && self.probed.borrow_mut().insert(probed)
            {
                self.probe_view(inner, read)?;
            }
        }
        Ok(())
    }

    /// What a view reads of one source, the parameters bound: the source,
    /// its `where` and its measures, after the principal is found to read
    /// every field they and the grouping read (024), and, where the source
    /// is `read` for the computation, those its valid time filters by.
    fn view_probe(
        &self,
        source: &TypeName,
        filter: Option<&lakelands_api::expr::Condition>,
        group_by: &[FieldName],
        m: &[lakelands_api::ontology::Measure],
        literals: &BTreeMap<String, Literal>,
        read: bool,
    ) -> Result<Probed> {
        let source = self.shape.ontology.ty(source)?.clone();
        let bound = |e: Expr| expr::bind_params(&e, literals);
        let where_ = filter
            .map(|w| condition(w, Eval::View, &[Place::Where]).map(bound))
            .transpose()?;
        let measures: Vec<(FieldName, Expr)> = m
            .iter()
            .map(|m| {
                let at = [Place::Measure {
                    name: m.name.clone(),
                }];
                let e = condition(&m.expr, Eval::Group, &at)?;
                Ok((m.name.clone(), bound(e)))
            })
            .collect::<Result<_>>()?;
        // What it reads of the source is what `describe` shows it by; one
        // that cannot be read there is read by no one.
        let probed = lakelands_core::describe::source_reads(filter, group_by, m);
        let mut probed = probed.ok_or_else(|| Error::refused(None, Refusal::NotGranted))?;
        // The computation at a valid time filters the source by its
        // valid-time fields besides, which `describe` does not judge by: it
        // probes those its reader may know of (024, lakelands#881), unless
        // its grant dates the source by them unread (092). Kept records are
        // filtered by none.
        if read && self.valid_at.is_some() {
            let timed = lakelands_core::valid_time::bounds(&source);
            probed.extend(timed.filter(|n| self.probes_bound(&source, n)).cloned());
        }
        // The probe is the view's own, so its refusal names nothing of the
        // source to a reader `describe` withholds the definition from (024).
        let unnamed = |e| match self.sight.who().grants.schema {
            true => e,
            false => Error::refused(None, Refusal::NotGranted),
        };
        let probed = self.sight.check_probe(&source, probed.iter());
        probed.map_err(unnamed)?;
        Ok((source, where_, measures))
    }

    pub(super) fn sort_values(order: &[Order], r: &StoredRecord) -> Vec<Value> {
        order
            .iter()
            .map(|o| core_engines::sort_value(r, o).unwrap_or(Value::Null))
            .collect()
    }

    /// The one ordering, the engines' own: sort values in turn, missing
    /// last either way, then the key ascending.
    pub(super) fn compare(order: &[Order], a: &SortKey, b: &SortKey) -> Ordering {
        fn some(v: &Value) -> Option<&Value> {
            (!v.is_null()).then_some(v)
        }
        let keys = order.iter().zip(a.0.iter().zip(&b.0));
        keys.map(|(o, (x, y))| core_engines::cmp_key(some(x), some(y), o.dir))
            .find(|c| c.is_ne())
            .unwrap_or_else(|| a.1.as_str().cmp(b.1.as_str()))
    }

    /// The page a read asks for, at most `max` rows: the lake's
    /// `max_limit`, or for a whole-graph measure, which is computed whole
    /// for every page, its `max_graph_nodes` where that is larger (026).
    pub(crate) fn limit(&self, asked: Option<u32>, measure: bool) -> Result<usize> {
        let limits = &self.lake.limits;
        let max = match measure {
            true => limits.max_limit.max(limits.max_graph_nodes),
            false => limits.max_limit,
        };
        // A read that names no limit asks for nothing out of bounds.
        let l = asked.unwrap_or(limits.default_limit.min(max));
        if l == 0 || l > max {
            return Err(Error::invalid(Case::LimitRange { max }));
        }
        Ok(l as usize)
    }
}
