//! The find and get doors: a type's records, filtered, ordered and paged,
//! and one record by key.

use super::*;

impl<'a> Reader<'a> {
    /// `find`.
    pub fn find(&self, ty: &TypeName, find: &Find) -> Result<Found> {
        // A page ordered first by a field the lake keeps no order index for
        // sorts the type: counted with its time, for `describe_type` to
        // tell, and never by a value (lakelands#1029).
        let whole = self.sorts_whole(ty, find);
        let started = std::time::Instant::now();
        let found = self.find_ordered(ty, find);
        if let (Some(field), Ok(_)) = (whole, &found) {
            let micros = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
            let mut tally = lakelands_core::lock(&self.lake.sorted_whole);
            let (pages, took) = tally.entry((ty.clone(), field.clone())).or_default();
            (*pages, *took) = (*pages + 1, took.saturating_add(micros));
            drop(tally);
            self.lake.note_whole(ty, &field);
        } else if let Some(o) = find.order_by.first().filter(|o| o.by == OrderBy::Value) {
            self.lake.note_used(ty, &o.field);
        }
        found
    }

    /// The field a `find` orders first by, where the lake keeps no order
    /// index for it at the head: a type's own scalar field, by its value,
    /// on a page of records (041).
    fn sorts_whole(&self, ty: &TypeName, find: &Find) -> Option<FieldName> {
        let first = find.order_by.first().filter(|o| o.by == OrderBy::Value);
        let o = first.filter(|_| find.aggregate.is_none())?;
        let decl = (self.shape.ontology.ty(ty).ok()).filter(|d| d.view.is_none())?;
        let sortable =
            |f: &&FieldDecl| f.name == o.field && lakelands_api::seam::sortable(&f.ty.kind);
        let f = decl.fields.iter().find(sortable)?;
        let setting = self.shape.settings.get(&core_engines::sort_index_key(ty));
        let sorted = core_engines::sorted_fields(ty, decl, setting);
        // A field the lake chose an index for is not sorted whole, nor
        // one it is building.
        let chosen = self.view.chosen(ty).unwrap_or_default();
        let held = chosen.kept.contains(&f.name) || chosen.building.contains(&f.name);
        (!sorted.contains(&f.name) && !held).then(|| f.name.clone())
    }

    fn find_ordered(&self, ty: &TypeName, find: &Find) -> Result<Found> {
        let decl = self.named_type(ty)?.clone();
        // An aggregate and a count are over the set, under `whole_sets_only`
        // its whole or nothing (006).
        if find.aggregate.is_some() || find.total || find.retired {
            self.sight.check_whole_set(ty)?;
        }
        let params = self.bind_params(&decl, &find.params)?;
        for o in &find.order_by {
            if let Some((field, case)) = o.misfit() {
                let reason = Refusal::Invalid(Box::new(case.into()));
                return Err(Error::refused(field, reason));
            }
            // A view's records are computed at the read's position; none
            // has an event of its own to order by, nor a provenance (028,
            // extended).
            if o.by == OrderBy::Latest && decl.view.is_some() {
                let ty = Some(ty.clone());
                return Err(Error::invalid(Case::LatestNoChange { ty }));
            }
            if o.by != OrderBy::Value && decl.view.is_some() {
                let case = Case::NoRecordsProvenance;
                return Err(Error::invalid_at(&o.field, case));
            }
        }
        let filter = find
            .filter
            .as_ref()
            .map(|c| self.named_sets(&decl.name, c, &[Place::Where]))
            .transpose()?;
        let mut probed: Vec<FieldName> = filter
            .as_ref()
            .map(|e| core_derive::reads(e).into_iter().collect())
            .unwrap_or_default();
        let mut asked: Vec<(Expr, Vec<Place>)> = Vec::new();
        probed.extend(
            find.order_by
                .iter()
                .filter(|o| o.by != OrderBy::Latest)
                .map(|o| o.field.clone()),
        );
        if let Some(a) = &find.aggregate {
            let fields = a.measures.iter().flat_map(|m| m.field.iter().chain(&m.by));
            probed.extend(fields.cloned());
            // A field key probes its field; a named key, every field its
            // expression reads (024).
            let keys = aggregate::group_keys(&a.group_by).map_err(Error::invalid)?;
            for (name, expr) in keys {
                match expr {
                    None => probed.push(name),
                    Some(e) => {
                        let at = vec![Place::Group { name }];
                        let e = crate::declare::record_stands(decl.view.is_none(), e, &at)?;
                        probed.extend(core_derive::reads(&e));
                        asked.push((e, at));
                    }
                }
            }
        }
        self.sight.check_probe(&decl, probed.iter())?;
        for o in find.order_by.iter().filter(|o| o.by != OrderBy::Latest) {
            Self::steady_order(&decl, o.field.as_str())?;
        }
        // What can never be evaluated is refused, once the reader is known
        // to read the fields it names (028, extended).
        for (e, at) in filter
            .iter()
            .map(|e| (e.clone(), vec![Place::Where]))
            .chain(asked)
        {
            crate::declare::evaluates(&e, &decl, &at)?;
        }
        let asked = find
            .after
            .as_ref()
            .map(|a| self.resume_after(&decl, find, a))
            .transpose()?;
        let skip = self.offset(find)?;
        if let Some(a) = &find.aggregate {
            // Counting alone holds no record: the count answers it.
            let counts = a.group_by.is_empty()
                && a.measures
                    .iter()
                    .all(|m| m.func == AggFunc::Count && m.field.is_none() && m.by.is_none());
            if counts && !a.measures.is_empty() {
                let n = Value::String(self.count(&decl, filter.as_ref())?.to_string());
                let values = a.measures.iter().map(|m| (m.name.clone(), n.clone()));
                return Ok(Found::Groups {
                    rows: vec![Group {
                        by: BTreeMap::new(),
                        values: values.collect(),
                    }],
                });
            }
            return self.aggregated(&decl, filter.as_ref(), a);
        }
        let limit = self.limit(find.limit, false)?;
        let find_read = read_of((&find.filter, &find.order_by, find.at.valid_at));
        let after = match &find.cursor {
            Some(c) => {
                let state = resume(c, "find", Some(ty), &params)?;
                let after = state.after.clone();
                // A cursor holds one sort value per key of the order its
                // first page was read in; under another number of keys it
                // places no page, and is refused before anything compares.
                if after
                    .as_ref()
                    .is_some_and(|(v, _)| v.len() != find.order_by.len())
                {
                    return Err(Error::invalid(Case::CursorOtherOrder));
                }
                same_read(&state, &find_read)?;
                after
            }
            None => asked,
        };
        let row = |r: StoredRecord| ((Self::sort_values(&find.order_by, &r), r.key.clone()), r);
        let mut keyed: Vec<(SortKey, StoredRecord)> = if let Some((stored, whole)) =
            self.kept(&decl)?
        {
            // A kept view's page is the engine's, as a stored type's.
            let faces = self.differing(&stored, &decl.name, whole)?;
            let faces = faces.into_iter().filter_map(|(_, w)| w);
            let differ = whole.then(|| faces.collect::<Vec<_>>());
            let page = |after, want| {
                let (f, o) = (filter.as_ref(), &find.order_by);
                self.kept_rows(&stored, &decl.name, f, o, differ.as_deref(), after, want)
            };
            let after = Self::skipped(&find.order_by, after, skip, &page)?;
            page(after, limit + 1)?.into_iter().map(row).collect()
        } else if decl.view.is_some()
            || self
                .sight
                .narrows_reading(&decl.name, find.order_by.iter().map(|o| &o.field))
        {
            // A view's records are computed from its source, whole, and
            // settings a reader may not be told, and a field a hiding
            // lowers, are ordered as told.
            let all = self.records(&decl, filter.as_ref())?;
            let mut keyed: Vec<_> = all.into_iter().map(row).collect();
            keyed.sort_by(|a, b| Self::compare(&find.order_by, &a.0, &b.0));
            if let Some(after) = &after {
                keyed.retain(|(k, _)| Self::compare(&find.order_by, k, after) == Ordering::Greater);
            }
            keyed.drain(..skip.min(keyed.len()));
            keyed
        } else {
            let page = |after, want| self.page(&decl, filter.as_ref(), &find.order_by, after, want);
            let after = Self::skipped(&find.order_by, after, skip, &page)?;
            let rows = page(after, limit + 1)?;
            rows.into_iter().map(row).collect()
        };
        let more = keyed.len() > limit;
        keyed.truncate(limit);
        let next_cursor = keyed.last().filter(|_| more).map(|(k, _)| {
            encode(&CursorState {
                at: self.position,
                after: Some(k.clone()),
                offset: 0,
                params: params.clone(),
                door: "find".into(),
                ty: Some(ty.clone()),
                of: find_read.clone(),
                now: Some(self.now),
                stage: None,
                unranked: None,
            })
        });
        let rows = keyed
            .into_iter()
            .filter_map(|(_, r)| self.sight.visible(&decl, &r, find.with_meta))
            .collect();
        // Counted at the same position, so every page of a walk agrees.
        let total = (find.total)
            .then(|| self.count(&decl, filter.as_ref()))
            .transpose()?;
        let retired = (find.retired)
            .then(|| self.retired_count(&decl, filter.as_ref(), total))
            .transpose()?;
        Ok(Found::Records {
            rows,
            next_cursor,
            total,
            retired,
        })
    }

    /// The rows `find` skips before its page: none unless it names an
    /// `offset`, which is refused beside a cursor, an `after` or an
    /// aggregate (each already says where the answer starts, or has no
    /// rows), and past the lake's `max_offset` as too large; `total` and
    /// `retired` are refused beside an aggregate too
    /// (`docs/decisions/045-a-table-reads-a-page-and-its-total.md`).
    fn offset(&self, find: &Find) -> Result<usize> {
        if find.total && find.aggregate.is_some() {
            return Err(Error::invalid(Case::TotalWithAggregate));
        }
        if find.retired && find.aggregate.is_some() {
            return Err(Error::invalid(Case::RetiredWithAggregate));
        }
        let Some(skip) = find.offset else {
            return Ok(0);
        };
        let beside = [
            (find.cursor.is_some(), "cursor"),
            (find.after.is_some(), "after"),
            (find.aggregate.is_some(), "aggregate"),
        ];
        if let Some((_, other)) = beside.iter().find(|(given, _)| *given) {
            let other = (*other).to_owned();
            return Err(Error::invalid(Case::OffsetBeside { other }));
        }
        let max = self.lake.limits.max_offset;
        if skip > max {
            return Err(Error::refused(
                Some("offset".into()),
                Refusal::TooLarge { max },
            ));
        }
        Ok(skip as usize)
    }

    /// The sort position after the first `skip` rows `page` reads in
    /// `order` after `after`, read a page at a time and dropped, so an
    /// offset holds a page whatever it skips.
    fn skipped(
        order: &[Order],
        mut after: Option<SortKey>,
        mut skip: usize,
        page: &impl Fn(Option<SortKey>, usize) -> Result<Vec<StoredRecord>>,
    ) -> Result<Option<SortKey>> {
        while skip > 0 {
            let want = skip.min(COUNT_PAGE);
            let rows = page(after.clone(), want)?;
            if let Some(last) = rows.last() {
                after = Some((Self::sort_values(order, last), last.key.clone()));
            }
            if rows.len() < want {
                break;
            }
            skip -= want;
        }
        Ok(after)
    }

    /// The sort position a caller's `after` names, each value in its
    /// field's canonical form, so it resumes exactly where a cursor holding
    /// the same row would. Refused as invalid, naming the member, beside a
    /// cursor or an aggregate, with a value count other than `order_by`'s,
    /// or with a value the key's field cannot hold.
    fn resume_after(&self, decl: &TypeDecl, find: &Find, a: &ReadAfter) -> Result<SortKey> {
        if find.cursor.is_some() {
            return Err(Error::invalid(Case::AfterWithCursor));
        }
        if find.aggregate.is_some() {
            return Err(Error::invalid(Case::AfterWithAggregate));
        }
        if a.values.len() != find.order_by.len() {
            let (values, keys) = (a.values.len(), find.order_by.len());
            return Err(Error::invalid(Case::AfterCount { values, keys }));
        }
        let mut values = Vec::with_capacity(a.values.len());
        for (i, (v, o)) in a.values.iter().zip(&find.order_by).enumerate() {
            let unfit = |why: Case| {
                let (index, field, why) = (i, o.field.clone(), Box::new(why));
                Error::invalid_at(&o.field, Case::AfterUnfit { index, field, why })
            };
            let v = match (o.by, v) {
                (_, Value::Null) => Value::Null,
                (OrderBy::Latest, Value::Number(n)) if n.is_u64() => v.clone(),
                (OrderBy::Latest, _) => {
                    return Err(unfit(Case::PositionNotInteger));
                }
                (OrderBy::KnownAt | OrderBy::SeenAt, Value::String(t)) => {
                    let value = || Case::NotDateTime { value: t.clone() };
                    let t = Timestamp::parse(t).map_err(|_| unfit(value()))?;
                    Value::String(t.to_string())
                }
                (OrderBy::KnownAt | OrderBy::SeenAt, _) => {
                    return Err(unfit(Case::InstantNotText));
                }
                (OrderBy::Value, v) => match decl.field(&o.field) {
                    Some(f) => {
                        let vt =
                            f.ty.value_type
                                .as_ref()
                                .and_then(|n| self.shape.ontology.value_types.get(n.as_str()))
                                .map(|e| &e.decl);
                        lakelands_core::value::check(v, &f.ty, vt).map_err(unfit)?
                    }
                    None => v.clone(),
                },
            };
            values.push(v);
        }
        Ok((values, a.key.clone()))
    }

    /// `find`'s aggregate over the records `filter` holds for, folded into
    /// their groups as they are read.
    fn aggregated(&self, decl: &TypeDecl, filter: Option<&Expr>, a: &Aggregate) -> Result<Found> {
        let named = |e: Said| {
            Error::invalid(e.within(Place::Aggregate {
                name: decl.name.clone(),
            }))
        };
        let mut grouping = aggregate::grouping(a, self.now).map_err(named)?;
        self.each_member(decl, filter, COUNT_PAGE, |r| {
            grouping.add((&r.fields, Some(&r.meta)), Some(self.told()));
            Ok(())
        })?;
        let mut groups = aggregate::answer(grouping, a).map_err(named)?;
        // A sum, minimum, maximum, first or last of a decimal field whose
        // value type declares a scale answers at that scale, so a sum over
        // nothing is `0.00` and not `0`.
        for m in &a.measures {
            if matches!(m.func, AggFunc::Count | AggFunc::Avg) {
                continue;
            }
            let Some(scale) = m
                .field
                .as_ref()
                .and_then(|f| decl.field(f))
                .and_then(|f| f.ty.value_type.as_ref())
                .and_then(|vt| self.shape.ontology.value_types.get(vt.as_str()))
                .and_then(|vt| vt.decl.scale)
            else {
                continue;
            };
            for g in &mut groups {
                if let Some(Value::String(s)) = g.values.get_mut(&m.name)
                    && let Ok(mut d) = s.parse::<rust_decimal::Decimal>()
                    && d.scale() <= scale
                {
                    d.rescale(scale);
                    *s = d.to_string();
                }
            }
        }
        Ok(Found::Groups { rows: groups })
    }

    /// `get`.
    pub fn get(&self, ty: &TypeName, get: &Get) -> Result<Record> {
        let decl = self.named_type(ty)?.clone();
        self.bind_params(&decl, &get.params)?;
        let missing = || Error::NotFound {
            missing: Missing::Record {
                ty: ty.clone(),
                key: get.key.clone(),
            },
        };
        let stored = if let Some(kept) = self.kept(&decl)? {
            self.kept_by_key(ty, kept, std::slice::from_ref(&get.key))?
                .pop()
        } else if decl.view.is_some() {
            self.records(&decl, None)?
                .into_iter()
                .find(|r| r.key == get.key)
        } else {
            let held = self.at_read(ty, self.view.get(ty, &get.key)?.into_iter().collect())?;
            let ends = self.admitted_ends(&decl, &held)?;
            let mut held = held.into_iter().zip(ends);
            held.find(|(r, ends)| *ends && self.live(&decl, r))
                .map(|(r, _)| r)
        };
        stored
            .and_then(|r| self.sight.visible(&decl, &r, get.with_meta))
            .ok_or_else(missing)
    }
}
