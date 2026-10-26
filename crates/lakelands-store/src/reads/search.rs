//! The search door: a text or vector search over a type, ranked and paged.

use super::*;

use lakelands_api::engine::{TextCounts, TextStats};
use lakelands_core::engines::Counts;

impl<'a> Reader<'a> {
    /// What `search` refuses before it ranks: the type its principal is
    /// shown, a view, a misfit, the fields it probes, the `where`. A
    /// hybrid search's text is embedded only once this answers (024): one
    /// refused calls nothing, and names no model.
    pub fn searched<'s>(&self, s: &'s Search) -> Result<Searched<'s>> {
        let decl = self.named_type(&s.ty)?.clone();
        // A view's records are computed on read and no index holds them, so
        // a search could only answer nothing (018).
        if decl.view.is_some() {
            let ty = s.ty.clone();
            return Err(Error::invalid(Case::ViewNotSearched { ty }));
        }
        if let Some((field, case)) = s.misfit() {
            let reason = Refusal::Invalid(Box::new(case.into()));
            return Err(Error::refused(field, reason));
        }
        let vectors = s.mode.vectors();
        let vector_fields: Vec<&FieldName> = vectors.iter().map(|v| v.0).collect();
        let fields = match &s.mode {
            SearchMode::Lexical { fields, .. } | SearchMode::Hybrid { fields, .. } => {
                fields.as_deref()
            }
            SearchMode::Vector { .. } => None,
        };
        self.sight.check_search(&decl, fields, &vector_fields)?;
        if let Some(weights) = own_text_weights(&s.mode) {
            self.sight.check_text_weights(&decl, weights)?;
        }
        // The names, the rights and the `where` are read at the read's
        // position; the rankings are judged by the head's declaration, whose
        // indexes they read, each field under the name it bears there (025).
        let texts = match fields {
            Some(n) => Box::new(n.iter()) as Box<dyn Iterator<Item = &FieldName>>,
            None => Box::new(self.sight.searched_fields(&decl)),
        };
        let ranks = self.ranks(&decl, vector_fields.iter().copied().chain(texts))?;
        // A vector ranking reads a vector field, and a hybrid search without
        // a vector reads one a model fills: one that cannot is refused
        // before any embedding is asked for (lakelands#743). A field the
        // head does not show its reader holds no vectors, and no model fills
        // it. A read that ranks nothing, a second stage's, judges none: the
        // read of its window did (067).
        let declared = |n: &FieldName| ranks.fields.get(n);
        let ranked = self.at_head || self.head.is_some();
        for field in &vector_fields {
            if let Some(f) = declared(field).filter(|f| f.ty.kind != StorageKind::Vector) {
                let kind = f.ty.kind;
                let case = Case::SearchNotVector {
                    field: (*field).clone(),
                    kind,
                };
                return Err(Error::invalid_at(field, case));
            }
        }
        if let SearchMode::Hybrid {
            field,
            vector: None,
            ..
        } = &s.mode
            && ranked
            && declared(field).is_none_or(|f| f.embed.is_none())
        {
            let case = Case::SearchNeedsVector {
                field: field.clone(),
            };
            return Err(Error::invalid_at(field, case));
        }
        // A query vector is judged by the field's declared length, never by
        // a stored vector, which another may own (024).
        let embedded = matches!(&s.mode, SearchMode::Hybrid { vector: None, .. });
        for (field, v, _) in vectors.iter().filter(|_| !embedded) {
            if let Some(d) = declared(field).and_then(|f| f.ty.dimensions)
                && v.len() != d as usize
            {
                let (query, declared) = (v.len(), d as usize);
                let field = field.as_str();
                return Err(core_engines::dimensions(query, declared, &s.ty, field));
            }
        }
        // A vector the caller names a model for is of that model: over a
        // field the reader is shown as an embedding it must be the field's
        // own, judged after its length (lakelands#994). Over any other
        // field the lake knows no model, and the axis only names it.
        for (field, named) in s.mode.models() {
            let held = declared(field).and_then(|f| f.embed.as_ref());
            if let (Some(query), Some(e)) = (named, held)
                && query != e.model
            {
                let field = field.as_str();
                return Err(core_engines::model(query, &e.model, &s.ty, field));
            }
        }
        let filter = self.record_filter(&decl, s.filter.as_ref(), &[Place::Where])?;
        Ok((decl, fields, filter, ranks))
    }

    /// The rankings over `named` fields of `decl` as the head declares
    /// them: the type and each field followed through the lake's renames
    /// since the read's position, and a field kept only where the reader,
    /// its grants followed to the head, is shown it there. Following a name
    /// never reaches a field the reader is not shown at the head (051). A
    /// read before the head that was not given the head ranks by nothing.
    fn ranks<'n>(
        &self,
        decl: &TypeDecl,
        named: impl Iterator<Item = &'n FieldName>,
    ) -> Result<Ranks> {
        let (shape, who) = match (self.at_head, &self.head) {
            (true, _) => (&self.shape, self.sight.who()),
            (false, Some(h)) => (&h.shape, &h.who),
            (false, None) => {
                let (ty, decl, fields) = (decl.name.clone(), None, BTreeMap::new());
                return Ok(Ranks { ty, decl, fields });
            }
        };
        let asked: Vec<FieldName> = named.cloned().collect();
        let mut now = asked.clone();
        let since = shape.renames.iter().filter(|r| r.at > self.position);
        let ty = followed(since, &decl.name, &mut now);
        let names: Vec<(FieldName, FieldName)> = asked.into_iter().zip(now).collect();
        let decl = shape.ontology.ty(&ty).ok().cloned();
        let sees = Sight::held(who, shape.as_ref());
        let shown = |now: &FieldName| {
            let d = decl.as_ref()?;
            let f = d.field(now)?;
            sees.field_access(d, f).reads().then(|| f.clone())
        };
        let fields = (names.iter())
            .filter_map(|(asked, now)| Some((asked.clone(), shown(now)?)))
            .collect();
        Ok(Ranks { ty, decl, fields })
    }

    /// `search`, `carried` the pull the door made this read for, if it did:
    /// it is judged here, at the position it was read at. A search with a
    /// second stage answers the stage's order, which the door handed it
    /// ([`Self::stage`]) or its cursor carries (067).
    pub fn search(&self, s: &Search, carried: Option<Pulled>) -> Result<Judged<Hits>> {
        if let Some(top) = self.rerank_top(s)? {
            return self.reranked(s, top).map(Ok);
        }
        let first = match self.first_stage(s, None, carried)? {
            Ok(first) => first,
            Err(moved) => return Ok(Err(moved)),
        };
        let k = s.k.max(1) as usize;
        let First {
            hits,
            ranked_by,
            axes,
            offset,
            read,
            skipped,
            marking,
        } = first;
        let more = hits.len() + skipped > offset + k;
        let mut page: Vec<Hit> = (hits.into_iter()).skip(offset - skipped).take(k).collect();
        if let Some(marks) = marking.and_then(|m| m.marks(s, self.named_type(&s.ty).ok()?)) {
            marks.mark(&mut page);
        }
        let next_cursor = more.then(|| {
            encode(&CursorState {
                at: self.position,
                after: None,
                offset: (offset + k) as u64,
                params: Params::new(),
                door: "search".into(),
                ty: Some(s.ty.clone()),
                of: read,
                now: Some(self.now),
                stage: None,
            })
        });
        Ok(Ok(Hits {
            hits: page,
            ranked_by,
            truncated: false,
            next_cursor,
            axes,
        }))
    }

    /// The first stage of a search: the hits it ranks this read may see,
    /// best first, `top` and one more of them for a search with a second
    /// stage, else the page and one more.
    pub(super) fn first_stage(
        &self,
        s: &Search,
        top: Option<usize>,
        mut carried: Option<Pulled>,
    ) -> Result<Judged<First>> {
        let (decl, text_fields, filter, ranks) = self.searched(s)?;
        let marking = self.marking(s, &decl, text_fields, &ranks);
        let vectors = s.mode.vectors();
        let vector_fields: Vec<&FieldName> = vectors.iter().map(|v| v.0).collect();
        // A search that names no fields reads the searchable fields the
        // principal reads (024), each before the head under its name there.
        let hidden = (decl.fields.iter())
            .any(|f| f.searchable && !self.sight.field_access(&decl, f).reads());
        let named = text_fields.map(|n| n.iter().collect::<Vec<_>>());
        let named = named.unwrap_or_else(|| self.sight.searched_fields(&decl).collect());
        let searched = || {
            let now = named.iter().map(|n| (*n, ranks.fields.get(*n)));
            now.filter_map(|(n, f)| Some((n, f.filter(|f| f.searchable)?)))
        };
        let known: Option<Vec<FieldName>> = (hidden || text_fields.is_some() || !self.at_head)
            .then(|| searched().map(|(_, f)| f.name.clone()).collect());
        let text_fields = known.as_deref();
        // A ranking the search does not weigh takes its type's setting at
        // the read's position (lakelands#561). A text match weighs what the
        // search gives its field, else the setting, under the field's name
        // at the head; a field not searched brings no weight
        // (lakelands#1230, lakelands#1237).
        let key = format!("{}{}", lakelands_api::seam::SEARCH_WEIGHTS_SETTING, s.ty);
        let set = self.shape.settings.get(&key);
        let (field_weights, text_default) = lakelands_api::seam::search_weights(set);
        let mut by_field = lakelands_api::seam::text_field_weights(set);
        let own = own_text_weights(&s.mode).into_iter().flatten();
        by_field.extend(own.map(|(f, w)| (f.to_string(), *w)));
        let text_weights = searched()
            .filter_map(|(n, f)| Some((f.name.clone(), *by_field.get(n.as_str())?)))
            .collect();
        let search_read = search_read(s);
        let offset = match &s.cursor {
            Some(c) => {
                let state = resume(c, "search", Some(&s.ty), &Params::new())?;
                same_read(&state, &search_read)?;
                state.offset as usize
            }
            None => 0,
        };
        let k = s.k.max(1) as usize;
        // One more than the page (or the second stage's window), to know
        // whether more exists.
        let want = top.map_or(offset + k + 1, |t| t + 1);
        // Without a query vector a search ranks its text alone; over several
        // vector fields it answers an index's set when any is indexed.
        let (mut by_text, mut indexed) = (true, false);
        if !matches!(&s.mode, SearchMode::Hybrid { vector: None, .. }) {
            for field in &vector_fields {
                let asked = |(d, f): (&TypeDecl, &FieldDecl)| self.indexed(d, &f.name);
                let at_head = ranks.decl.as_ref().zip(ranks.fields.get(*field));
                (by_text, indexed) = (
                    false,
                    indexed || at_head.map(asked).transpose()? == Some(true),
                );
            }
        }
        let ranked_by = match (&s.mode, indexed) {
            (SearchMode::Lexical { .. }, _) => "lexical",
            (SearchMode::Hybrid { .. }, _) if by_text => {
                "lexical (no query vector and no embedding backend)"
            }
            (SearchMode::Hybrid { .. }, true) => "hybrid (index)",
            (SearchMode::Hybrid { .. }, false) => "hybrid",
            (SearchMode::Vector { .. }, true) => "index",
            (SearchMode::Vector { .. }, false) => "vector",
        }
        .to_owned();
        // The rankings, each with its weight, in the order they are summed
        // (046): the text, then each vector field. A search of one ranking
        // fuses nothing and lists its axis without a count (lakelands#560).
        // The engine is asked for all at once under the head's names; a
        // ranking over no field the head shows the reader is not asked.
        let (mut asked, mut lists, mut axes) = (Rankings::default(), Vec::new(), Vec::new());
        let models = s.mode.models();
        let named = |f: &FieldName| {
            let m = models.iter().find(|(n, _)| *n == f);
            m.and_then(|(_, m)| m.map(str::to_owned))
        };
        let axis = |field: Option<&FieldName>| Axis {
            axis: (if field.is_some() { "vector" } else { "lexical" }).into(),
            field: field.cloned(),
            candidates: None,
            weight: None,
            model: field.and_then(named),
        };
        if let SearchMode::Lexical { text, .. } | SearchMode::Hybrid { text, .. } = &s.mode {
            let weight = match &s.mode {
                SearchMode::Hybrid { text_weight, .. } => text_weight.or(text_default),
                _ => None,
            };
            let ranked = text_fields != Some(&[]);
            if ranked {
                asked.text = Some((text.clone(), text_fields.map(<[FieldName]>::to_vec)));
                asked.text_weights = text_weights;
                asked.text_stats = self.text_stats(&ranks.ty, text, text_fields)?;
            }
            lists.push((weight.unwrap_or(1.0), ranked));
            axes.push(axis(None));
        }
        for (field, v, weight) in vectors.iter().filter(|_| !by_text) {
            let set = || field_weights.get(field.as_str()).copied();
            let at_head = ranks.fields.get(*field);
            asked
                .vectors
                .extend(at_head.map(|f| (f.name.to_string(), v.to_vec())));
            lists.push((weight.or_else(set).unwrap_or(1.0), at_head.is_some()));
            axes.push(axis(Some(field)));
        }
        // Under an index the engine streams a reader from the graph of its
        // class and ranks exactly one whose records are no class, so it is
        // told the class (078). Where it can judge the whole of this read's
        // selection it is told that, and its vector lists then hold only
        // what this read may see: a page reads its own records alone.
        let pushed = (!asked.vectors.is_empty())
            .then(|| self.pushed(&decl, filter.as_ref()))
            .flatten();
        let full = pushed.is_some();
        if full || indexed {
            let restricted = self.sight.sees_restricted();
            asked.seen = Some(pushed.unwrap_or(Scan {
                restricted,
                ..Scan::default()
            }));
            asked.exact = indexed && !self.whole(&decl);
        }
        // Candidates are pulled in growing batches until the page and one
        // more are visible or the engine has no more, so a filter, `valid_at`
        // or the grants never shorten a page silently. A list read scans its
        // index whatever it is asked for, so a fused search's first pull asks
        // for the whole window. A page is judged from one pull.
        let fuses = lists.len() > 1;
        let first = match (fuses, full) {
            (true, _) => self.lake.limits.max_limit as usize,
            (false, true) => want,
            (false, false) => want.saturating_mul(4).max(64),
        };
        let mut pull = u32::try_from(first).unwrap_or(u32::MAX);
        pull = pull.max(carried.as_ref().map_or(0, |p| p.k));
        // A reader a hiding narrows is ranked over every candidate as told.
        if self.sight.hides(&decl.name) {
            pull = u32::MAX;
        }
        let (hits, counts, skipped) = loop {
            let (read, judged) = match self.pull(&ranks.ty, &asked, pull, &mut carried)? {
                Ok(read) => read,
                Err(moved) => return Ok(Err(moved)),
            };
            let (mut read, judged) = (read.into_iter(), full && judged);
            let mut pulled: Vec<(f64, Ranking)> = (lists.iter())
                .map(|(w, ranked)| {
                    (
                        *w,
                        ranked.then(|| read.next()).flatten().unwrap_or_default(),
                    )
                })
                .collect();
            let filter = filter.as_ref();
            let done = if fuses {
                // The engine judges its vector lists, which follow the
                // text's.
                let from = if judged {
                    lists.len() - vectors.iter().filter(|_| !by_text).count()
                } else {
                    lists.len()
                };
                let lists = (pulled, from);
                let fused = self.fused(&decl, &ranks.ty, filter, lists, pull, want, s.with_meta)?;
                fused.map(|(hits, counts)| (hits, counts, 0))
            } else {
                let mut ranked = pulled.pop().map_or_else(Vec::new, |l| l.1);
                let exhausted = ranked.len() < pull as usize;
                // A judged list holds only what this read sees: the hits
                // before the page are not read.
                let skip = if judged && top.is_none() {
                    offset.min(ranked.len())
                } else {
                    0
                };
                let ranked = self.keyed_then(&ranks.ty, ranked.split_off(skip));
                let hits = self.visible_hits(&decl, filter, ranked, s.with_meta)?;
                (hits.len() + skip >= want || exhausted || pull == u32::MAX)
                    .then(|| (hits, Vec::new(), skip))
            };
            if let Some(done) = done {
                break done;
            }
            pull = pull.saturating_mul(4);
        };
        for ((a, n), (w, _)) in axes.iter_mut().zip(counts).zip(&lists) {
            (a.candidates, a.weight) = (Some(n), Some(*w));
        }
        Ok(Ok(First {
            hits,
            ranked_by,
            axes,
            offset,
            read: search_read,
            skipped,
            marking,
        }))
    }

    /// What `s`'s first stage searched, which its hits are marked by
    /// (025), where it asks for `matches` or has a second stage that may
    /// carry it: the fields its text was ranked in by the head, by their
    /// names at the read's position, and the language that ranked. A read
    /// not given the head ranked nothing and marks nothing.
    pub(super) fn marking(
        &self,
        s: &Search,
        decl: &TypeDecl,
        named: Option<&[FieldName]>,
        ranks: &Ranks,
    ) -> Option<Marking> {
        let asked = s.matches || s.rerank.is_some();
        if !asked || matches!(s.mode, SearchMode::Vector { .. }) {
            return None;
        }
        let head = ranks.decl.as_ref()?;
        let ranked = |n: &&FieldName| ranks.fields.get(*n).is_some_and(|f| f.searchable);
        let fields = match named {
            Some(n) => n.iter().filter(ranked).cloned().collect(),
            None => self
                .sight
                .searched_fields(decl)
                .filter(ranked)
                .cloned()
                .collect(),
        };
        let tag = head.language.clone().unwrap_or_default();
        Some(Marking { fields, tag })
    }

    /// The part of this read's selection an engine judges beside its
    /// index, at the head; none where any of it is the store's: a link
    /// judged by its ends, a filter the store evaluates, or a read before
    /// the head.
    fn pushed(&self, decl: &TypeDecl, filter: Option<&Expr>) -> Option<Scan> {
        let head = (self.head.as_ref()).is_some_and(|h| h.view.position() == self.view.position());
        if !(self.at_head || head) || self.sight.judges_ends(decl) || self.sight.narrows(&decl.name)
        {
            return None;
        }
        let equal = self.sight.admit(&decl.name).pairs()?.to_vec();
        let bounded = self.with_bound(decl, filter);
        if bounded.as_ref().is_some_and(|e| self.store_only(decl, e)) {
            return None;
        }
        Some(Scan {
            filter: bounded,
            equal,
            valid_at: self.valid_at_for(decl),
            retired: self.retired.get(),
            restricted: self.sight.sees_restricted(),
            ..Scan::default()
        })
    }

    /// The lists of one pull to the depth `k`, as the engine ranks `asked`
    /// under the head's name `ty`, all from one state: `carried`'s, where
    /// the door made this read for them and it asks the same. They are
    /// handed back unjudged where they were read at another position than
    /// this read's head: a write landed since it was taken. Beside them,
    /// whether the engine judged its vector lists by `asked.seen`.
    fn pull(
        &self,
        ty: &TypeName,
        asked: &Rankings,
        k: u32,
        carried: &mut Option<Pulled>,
    ) -> Result<Judged<(Vec<Ranking>, bool)>> {
        if asked.text.is_none() && asked.vectors.is_empty() {
            return Ok(Ok((Vec::new(), false)));
        }
        let head = (self.head.as_ref()).map_or(self.view.position(), |h| h.view.position());
        let same = |p: &Pulled| (&p.ty, &p.asked, p.k, p.position()) == (ty, asked, k, head);
        let pulled = match carried.take().filter(same) {
            Some(pulled) => pulled,
            None if !asked.vectors.is_empty() && self.lake.engine.vectors().is_none() => {
                return Err(Error::unbound(lakelands_api::error::Seam::VectorIndex));
            }
            None => Pulled {
                ty: ty.clone(),
                asked: asked.clone(),
                k,
                ranked: self.lake.engine.ranked(ty, asked, k)?,
            },
        };
        Ok(match pulled.position() == head {
            true => {
                let mut lists = pulled.ranked.lists;
                self.told_lists(ty, asked, &mut lists)?;
                Ok((lists, pulled.ranked.judged))
            }
            false => Err(pulled),
        })
    }

    /// `lists`, pulled for `asked` over `ty` at the head, as the head tells
    /// its reader each record (006): one a hiding selects there ranks by
    /// its text as told, and is no candidate of a vector the hiding lowers.
    /// As of a position, a field a hiding lowers on the record there is
    /// lowered too: a ranking reaches a field only where its reader is
    /// shown it there and at the head (025).
    fn told_lists(
        &self,
        ty: &TypeName,
        asked: &Rankings,
        lists: &mut [Vec<(RecordKey, f64)>],
    ) -> Result<()> {
        let (view, shape, who) = match (self.at_head, &self.head) {
            (true, _) => (self.view.as_ref(), &self.shape, self.sight.who()),
            (false, Some(h)) => (h.view.as_ref(), &h.shape, &h.who),
            (false, None) => return Ok(()),
        };
        let sees = Sight::held(who, shape.as_ref());
        let then = self.hidden_then(ty);
        let hides = sees.hides(ty) || then.is_some();
        let Some(decl) = shape.ontology.ty(ty).ok().filter(|_| hides) else {
            return Ok(());
        };
        let vectors = asked
            .vectors
            .iter()
            .map(|v| Some(FieldName::from(v.0.as_str())));
        let axes = asked.text.iter().map(|_| None).chain(vectors);
        for (list, axis) in lists.iter_mut().zip(axes) {
            let keys: Vec<RecordKey> = list.iter().map(|h| h.0.clone()).collect();
            let held = view.get_many(ty, &keys)?.into_iter();
            let held: BTreeMap<RecordKey, StoredRecord> =
                held.map(|r| (r.key.clone(), r)).collect();
            let was = self.lowered_then(ty, then.as_ref(), &keys)?;
            list.retain_mut(|(key, score)| {
                let Some(r) = held.get(key) else {
                    return true;
                };
                let mut lowered = (*sees.lowered(ty, &r.fields)).clone();
                lowered.extend(was.get(key).into_iter().flatten().cloned());
                match (&axis, &asked.text) {
                    _ if lowered.is_empty() => true,
                    (Some(v), _) => !lowered.contains_key(v),
                    (None, Some((text, fields))) => {
                        let told = sees.told(r).map(|mut t| {
                            lowered.keys().for_each(|f| _ = t.to_mut().fields.remove(f));
                            t
                        });
                        let w = &asked.text_weights;
                        let hit = told_lexical(decl, told, text, fields.as_deref(), w, asked);
                        hit.first().map(|h| *score = h.1).is_some()
                    }
                    (None, None) => true,
                }
            });
            core_engines::rank(list, u32::MAX);
        }
        Ok(())
    }

    /// Where the read is as of a position and its sight there hides on
    /// `ty`, named at the head: the head, and the type as named there.
    fn hidden_then(&self, ty: &TypeName) -> Option<(&Head<'a>, TypeName)> {
        let h = self.head.as_ref().filter(|_| !self.at_head)?;
        let since = h.shape.renames.iter().filter(|r| r.at > self.position);
        let back = |name: TypeName, r: &Renamed| match r.to.as_ref() == Some(&name) {
            true => r.ty.clone(),
            false => name,
        };
        let named = since.rev().fold(ty.clone(), back);
        self.sight.hides(&named).then_some((h, named))
    }

    /// The fields, by their names at the head, a hiding lowers on each of
    /// `keys` of `ty` as the read's position holds the record (006), with
    /// `then` the [`hidden_then`](Self::hidden_then) of `ty`.
    fn lowered_then(
        &self,
        ty: &TypeName,
        then: Option<&(&Head<'a>, TypeName)>,
        keys: &[RecordKey],
    ) -> Result<BTreeMap<RecordKey, Vec<(FieldName, Access)>>> {
        let mut was = BTreeMap::new();
        let Some((h, named)) = then else {
            return Ok(was);
        };
        let since = h.shape.renames.iter().filter(|r| r.at > self.position);
        let at: BTreeMap<RecordKey, &RecordKey> = (keys.iter())
            .filter_map(|k| Some((h.since.key_then(ty, k)?, k)))
            .collect();
        for r in (self.view).get_many(named, &at.keys().cloned().collect::<Vec<_>>())? {
            let (Some(k), low) = (at.get(&r.key), self.sight.lowered(named, &r.fields)) else {
                continue;
            };
            let mut fields: Vec<FieldName> = low.keys().cloned().collect();
            followed(since.clone(), named, &mut fields);
            was.insert(
                (*k).clone(),
                fields.into_iter().zip(low.values().copied()).collect(),
            );
        }
        Ok(was)
    }

    /// What the text's ranking of a search over `ty`, the type's name at
    /// the head, counts its statistics over (025): for a reader of a class,
    /// the live heads of that class, as the engine keeps them; for one of
    /// no class, what is counted here over the heads it is admitted there,
    /// as it is told them, so that nothing it may not see moves a score;
    /// as of a position, each without a field a hiding lowers on it there.
    fn text_stats(
        &self,
        ty: &TypeName,
        text: &str,
        fields: Option<&[FieldName]>,
    ) -> Result<Option<TextStats>> {
        let (view, shape, who) = match (self.at_head, &self.head) {
            (true, _) => (self.view.as_ref(), &self.shape, self.sight.who()),
            (false, Some(h)) => (h.view.as_ref(), &h.shape, &h.who),
            // No ranking reaches here; one that did counts no held record.
            (false, None) => return Ok(Some(TextStats::Unheld)),
        };
        let sees = Sight::held(who, shape.as_ref());
        let Ok(decl) = shape.ontology.ty(ty) else {
            let unheld = !sees.sees_restricted();
            return Ok(unheld.then_some(TextStats::Unheld));
        };
        let then = self.hidden_then(ty);
        let whole = sees.admit(ty).pairs().is_some_and(|p| p.is_empty())
            && !sees.judges_ends(decl)
            && !sees.narrows(ty)
            && then.is_none();
        if whole {
            return Ok((!sees.sees_restricted()).then_some(TextStats::Unheld));
        }
        const BATCH: u32 = 1024;
        let (mut counted, mut after) = (TextCounts::default(), None);
        loop {
            let scan = Scan {
                after: after.take(),
                limit: BATCH,
                restricted: true,
                ..Scan::default()
            };
            let rows = view.scan(ty, &scan)?;
            let fields_of: Vec<_> = rows.iter().map(|r| &r.fields).collect();
            let ends = sees.ends(decl, &fields_of, &mut |end, keys| view.get_many(end, keys))?;
            let keys: Vec<RecordKey> = rows.iter().map(|r| r.key.clone()).collect();
            let was = self.lowered_then(ty, then.as_ref(), &keys)?;
            let admitted = (rows.iter().zip(ends)).filter(|(r, end)| *end && sees.answers(ty, r));
            let told = admitted.filter_map(|(r, _)| sees.told(r)).map(|mut t| {
                let lowered = was.get(&t.key).into_iter().flatten();
                lowered.for_each(|(f, _)| _ = t.to_mut().fields.remove(f));
                t
            });
            core_engines::text_stats(decl, told, (text, fields), &mut counted);
            match rows.last() {
                Some(last) if rows.len() == BATCH as usize => {
                    let key = last.key.clone();
                    after = Some(After {
                        values: Vec::new(),
                        key,
                    });
                }
                _ => return Ok(Some(TextStats::Counted(counted))),
            }
        }
    }

    /// A fused search's hits: its lists fused by weighted reciprocal rank
    /// ([`fuse`]), each ranked among the hits this read may see and to the
    /// lake's `max_limit` of them, so a record the read may not see moves
    /// no score and every page reads one ranking (046). `lists`
    /// are one pull to the depth `pull`, with the first of them the engine
    /// judged by the whole selection; none is answered until the first
    /// `want` fused hits and their scores can no longer change, and each
    /// list is whole or holds its window, so that its count, answered
    /// beside the hits, is the same on every page: the search pulls again.
    #[allow(clippy::too_many_arguments)]
    fn fused(
        &self,
        decl: &TypeDecl,
        named: &TypeName,
        filter: Option<&Expr>,
        (lists, judged): (Vec<(f64, Ranking)>, usize),
        pull: u32,
        want: usize,
        with_meta: bool,
    ) -> Result<Option<(Vec<Hit>, Vec<u64>)>> {
        let window = self.lake.limits.max_limit as usize;
        // At the head, a read that sees every live record and narrows by
        // nothing sees every candidate an engine ranks, so it reads no
        // record to rank them. That rests on two seam facts: `ranked`
        // ranks live records only, restricted ones included (the engine
        // seam's `LEXICAL_HITS`), and the fold takes a retired record's
        // vectors out of the index, which is kept at the head (the suite's
        // `VECTOR_LEAVES_THE_NEIGHBOURS`). `SAME_LIST_UNREAD` holds this
        // path to the one that reads. Lists the engine judged by the
        // read's whole selection hold only what it sees, too.
        let open = self.at_head
            && self.whole(decl)
            && self.sight.sees_restricted()
            && filter.is_none()
            && !self.timed(decl);
        {
            // Each list's visible candidates to the window, and what an unranked
            // record could still gain from it: nothing once whole or full.
            let (mut keyed, mut gain, mut counted) = (Vec::new(), Vec::new(), true);
            for (i, (weight, candidates)) in lists.into_iter().enumerate() {
                let whole = candidates.len() < pull as usize;
                let mut seen: Vec<(RecordKey, f64)> = if open || i >= judged {
                    candidates
                } else {
                    let candidates = self.keyed_then(named, candidates);
                    (self.visible_hits(decl, filter, candidates, false)?)
                        .into_iter()
                        .map(|h| (h.record.key, h.score))
                        .collect()
                };
                let done = whole || seen.len() >= window || pull == u32::MAX;
                counted &= done;
                seen.truncate(window);
                gain.push(if done {
                    0.0
                } else {
                    weight / (60.0 + seen.len() as f64)
                });
                keyed.push((weight, seen));
            }
            let fused = fuse(&keyed);
            let ranked: Vec<BTreeSet<&RecordKey>> = (keyed.iter())
                .map(|(_, l)| l.iter().map(|(k, _)| k).collect())
                .collect();
            let slack = |key: &RecordKey| -> f64 {
                (ranked.iter().zip(&gain))
                    .filter(|(l, _)| !l.contains(key))
                    .map(|(_, g)| g)
                    .sum()
            };
            // Settled when the first `want` are ranked by every list that
            // can still rank them, and nothing behind them, seen or not,
            // could still reach the last of them.
            let unseen: f64 = gain.iter().sum();
            let settled = match fused.get(want.saturating_sub(1)) {
                None => unseen == 0.0,
                Some((_, last)) => {
                    fused[..want].iter().all(|(k, _)| slack(k) == 0.0)
                        && unseen < *last
                        && fused[want..].iter().all(|(k, s)| {
                            let g = slack(k);
                            g == 0.0 || s + g < *last
                        })
                }
            };
            if !(settled && counted) {
                return Ok(None);
            }
            let counts = keyed.iter().map(|(_, l)| l.len() as u64).collect();
            let first = fused.into_iter().take(want).collect();
            let hits = self.visible_hits(decl, filter, first, with_meta)?;
            Ok(Some((hits, counts)))
        }
    }

    /// An engine's candidates, ranked at the head where the type is
    /// `named`, under the keys they bear at the read's position: before the
    /// head, a re-key since may have moved them (lakelands#797). One whose
    /// key names another record now than then is not a candidate. Equal
    /// scores stay in the order of the keys at the head, the order the
    /// engines rank and a cursor pages in: sorting one batch again by the
    /// keys answered would answer some of a tie twice and some never (029).
    fn keyed_then(&self, named: &TypeName, ranked: Vec<(RecordKey, f64)>) -> Vec<(RecordKey, f64)> {
        let Some(h) = &self.head else {
            return ranked;
        };
        (ranked.into_iter())
            .filter_map(|(k, score)| Some((h.since.key_then(named, &k)?, score)))
            .collect()
    }

    /// Whether the lake asks an index for `field` of `decl`'s type at the
    /// head, where the engines keep their vector indexes: a search over it
    /// answers the index's set, at any position.
    fn indexed(&self, decl: &TypeDecl, field: &FieldName) -> Result<bool> {
        let key = core_engines::vector_index_key(&decl.name);
        let shape = self.head.as_ref().map_or(&self.shape, |h| &h.shape);
        let setting = shape.settings.get(&key);
        Ok(core_engines::indexed_fields(decl, setting).contains(field))
    }

    pub(super) fn visible_hits(
        &self,
        decl: &TypeDecl,
        filter: Option<&Expr>,
        ranked: Vec<(RecordKey, f64)>,
        with_meta: bool,
    ) -> Result<Vec<Hit>> {
        let keys: Vec<RecordKey> = ranked.iter().map(|(k, _)| k.clone()).collect();
        let rows = self.at_read(&decl.name, self.view.get_many(&decl.name, &keys)?);
        let (ends, admit) = (
            self.admitted_ends(decl, &rows)?,
            self.sight.admit(&decl.name),
        );
        let at_head = self.head_admitted(decl, &keys)?;
        let mut stored: BTreeMap<RecordKey, StoredRecord> = (rows.into_iter().zip(ends))
            .filter(|(r, ends)| *ends && at_head.as_ref().is_none_or(|h| h.contains(&r.key)))
            .map(|(r, _)| (r.key.clone(), r))
            .collect();
        let mut hits = Vec::new();
        for (key, score) in ranked {
            let Some(r) = stored.remove(&key) else {
                continue;
            };
            if !self.live_by(decl, &admit, &r) {
                continue;
            }
            if filter.is_some_and(|e| !self.holds(e, &r, true)) {
                continue;
            }
            if let Some(record) = self.sight.visible(decl, &r, with_meta) {
                let matches = BTreeMap::new();
                hits.push(Hit {
                    record,
                    score,
                    matches,
                });
            }
        }
        Ok(hits)
    }

    /// For a search as of a position by a holder of `only` on `decl`, the
    /// candidates admitted at the head as well, under the grant followed
    /// there and by the name and the key it bears there: a ranking by the text
    /// and vectors at the head must find no record by another's (051).
    /// A grant that cannot be applied to the head's declaration admits
    /// none there (lakelands#771). `None` where every candidate passes.
    fn head_admitted(
        &self,
        decl: &TypeDecl,
        keys: &[RecordKey],
    ) -> Result<Option<BTreeSet<RecordKey>>> {
        let Some(h) = self.head.as_ref().filter(|_| !self.whole(decl)) else {
            return Ok(None);
        };
        let (head, who, since, shape) = (&h.view, &h.who, &h.since, &h.shape);
        let name = since.name(&decl.name);
        let moved: Vec<RecordKey> = keys.iter().map(|k| since.key(&name, k)).collect();
        let sees = Sight::held(who, shape.as_ref());
        // A type the head does not declare has no record there.
        let Ok(at_head) = shape.ontology.ty(&name).map(|_| sees.admit(&name)) else {
            return Ok(Some(BTreeSet::new()));
        };
        let rows = head.get_many(&name, &moved)?;
        let fields: Vec<_> = rows.iter().map(|r| &r.fields).collect();
        let ends = sees.ends(decl, &fields, &mut |end, keys| head.get_many(end, keys))?;
        let admitted: BTreeSet<RecordKey> = (rows.into_iter().zip(ends))
            .filter(|(r, ends)| *ends && !r.retired && at_head.holds(&r.fields))
            .map(|(r, _)| r.key)
            .collect();
        let admitted = keys
            .iter()
            .zip(&moved)
            .filter(|(_, k)| admitted.contains(*k));
        Ok(Some(admitted.map(|(k, _)| k.clone()).collect()))
    }

    /// Refuse a walk around `here` over a relation that probes a field the
    /// principal may not read: the reference field that carries it, whose
    /// value each edge answers, or a derived relation's matched field, on
    /// either side, where an edge that appears or not answers whether the
    /// two values are equal (024, 044). A step is probed before it walks,
    /// and every hop again around the type it reached, over `relation` or,
    /// when it is `None`, over every relation it walks. Only a relation the
    /// hop walks and the principal is shown is probed, so a refusal never
    /// names a type or a field it is told is not there (006, extended):
    /// what remains is a masked field. A relation the step the hop is
    /// walked for does not walk (`reach`) is not probed, as it is not read.
    pub(super) fn probe_derived(
        &self,
        here: &TypeName,
        relation: Option<&RelationName>,
        walk: Walk,
        reach: &Reach,
    ) -> Result<()> {
        for (t, r) in self.shown_relations(Some(here)) {
            if relation.is_some_and(|n| r.name != *n) || !reach.walks(here, walk, t, r) {
                continue;
            }
            self.sight.check_probe(t, r.via.iter())?;
            let Some(d) = &r.derived else { continue };
            let [from, to] = d.reads();
            self.sight.check_probe(t, from.iter())?;
            let target = self.shape.ontology.ty(&r.target)?;
            self.sight.check_probe(target, to.iter())?;
        }
        Ok(())
    }
}

/// What a search's first stage searched, which its hits are marked by:
/// a second stage carries it in its order, so each of its pages marks
/// what a page of the first stage alone would (025).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Marking {
    fields: Vec<FieldName>,
    tag: String,
}

impl Marking {
    /// What `s` marks its hits of `decl` by, if it asks for `matches`.
    /// A second stage's text moves no mark.
    pub(super) fn marks(&self, s: &Search, decl: &TypeDecl) -> Option<Marks> {
        let (true, SearchMode::Lexical { text, .. } | SearchMode::Hybrid { text, .. }) =
            (s.matches, &s.mode)
        else {
            return None;
        };
        Some(Marks {
            decl: decl.clone(),
            fields: self.fields.clone(),
            query: core_engines::tokens(text, &self.tag),
            tag: self.tag.clone(),
        })
    }
}

/// What a search that asks for `matches` marks its page's hits by.
pub(super) struct Marks {
    decl: TypeDecl,
    fields: Vec<FieldName>,
    query: BTreeSet<String>,
    tag: String,
}

impl Marks {
    /// Where the text matched each of `hits`, in the record it answers.
    /// Only a page's hits are marked, after the cut, so no mark moves a
    /// ranking.
    pub(super) fn mark(&self, hits: &mut [Hit]) {
        for h in hits {
            let (fields, query) = (&self.fields, &self.query);
            h.matches = core_engines::matched(&self.decl, &h.record, fields, query, &self.tag);
        }
    }
}

/// [`core_engines::lexical`] of a record as its reader is told it, scored
/// over what the store counted for that reader (`asked.text_stats`): a
/// reader a hiding selects records for narrows, so is of no class, and is
/// always counted for.
fn told_lexical(
    decl: &TypeDecl,
    told: Option<std::borrow::Cow<'_, StoredRecord>>,
    text: &str,
    fields: Option<&[FieldName]>,
    weights: &BTreeMap<FieldName, f64>,
    asked: &Rankings,
) -> Vec<(RecordKey, f64)> {
    debug_assert!(matches!(asked.text_stats, Some(TextStats::Counted(_))));
    let none = TextCounts::default();
    let counted = match &asked.text_stats {
        Some(TextStats::Counted(counted)) => counted,
        _ => &none,
    };
    core_engines::lexical(
        decl,
        told,
        text,
        fields,
        weights,
        Counts::Counted(counted),
        1,
    )
}

/// `ty` as `renames`, in their order, name it after them, and each of its
/// `fields` renamed to the name it bears there.
fn followed<'r>(
    renames: impl Iterator<Item = &'r Renamed>,
    ty: &TypeName,
    fields: &mut [FieldName],
) -> TypeName {
    let mut ty = ty.clone();
    for r in renames {
        if r.ty != ty {
            continue;
        }
        for now in fields.iter_mut() {
            if let Some((_, to)) = r.renames.iter().find(|(from, _)| from == now) {
                *now = to.clone();
            }
        }
        if let Some(to) = &r.to {
            ty = to.clone();
        }
    }
    ty
}
