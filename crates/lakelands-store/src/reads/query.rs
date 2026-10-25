//! The query door: a pattern over records and the edges between them,
//! read at the head or at a position.

use super::*;

impl<'a> Reader<'a> {
    /// `query`: the one pattern query.
    pub fn query(&self, p: &Pattern, carried: Option<Pulled>) -> Result<Judged<QueryResult>> {
        // Its steps as declared: a step naming an inverse is rewritten below.
        let mut p = p.clone();
        let p = &mut p;
        let Some(start_ty) = p.start.ty.clone() else {
            return self.untyped(p);
        };
        let start_decl = self.named_type(&start_ty)?.clone();
        if p.start.rerank.is_some() && p.start.search.is_none() {
            return Err(Error::invalid_at(
                &"rerank".into(),
                Case::RerankWithoutSearch,
            ));
        }
        let params = self.bind_params(&start_decl, &p.start.params)?;
        let limit = self.limit(p.limit, measures::is_measure(&p.project))?;
        let max_depth = self.lake.limits.max_depth;
        // Rows and an aggregate bind each record under a name, once (018,
        // extended); nothing else binds by name.
        let binds = matches!(
            p.project,
            Projection::Rows { .. } | Projection::Aggregate(_)
        );
        let given = std::iter::once(&p.start.named).chain(p.steps.iter().map(|s| &s.named));
        for name in given.flatten() {
            if !binds {
                return Err(Error::invalid(Case::PatternAsNotRows));
            }
            if let Some(fault) = lakelands_core::names::fault(Naming::Binding, name) {
                return Err(Error::invalid(fault));
            }
        }
        tree(p, binds)?;
        if measures::is_measure(&p.project) {
            measures::check(p)?;
        } else if !matches!(p.project, Projection::Rows { .. }) && !p.order_by.is_empty() {
            // A graph and a path come by depth, type and key, an
            // aggregate's groups by their key (025): an order is refused.
            return Err(Error::invalid(Case::PatternOrderNotRows));
        }
        // A row is a projection of a walk, not a record: it has no change
        // of its own and no provenance to order by (028, 025). Refused
        // before a record is read, whatever the walk would meet.
        for o in &p.order_by {
            if let Some((field, case)) = o.misfit() {
                let reason = Refusal::Invalid(Box::new(case.into()));
                return Err(Error::refused(field, reason));
            }
            match o.by {
                OrderBy::Value => {}
                OrderBy::Latest => return Err(Error::invalid(Case::LatestNoChange { ty: None })),
                _ => {
                    let case = Case::NoRecordsProvenance;
                    return Err(Error::invalid_at(&o.field, case));
                }
            }
        }
        // What a pattern says and an arm cannot honour is refused: a path
        // is a graph of one walk from one start. Rows are told so before
        // their steps are judged, which a path's would not pass.
        let reaches = p.project == Projection::Reach;
        if (binds || reaches) && p.to.is_some() {
            return Err(Error::invalid(Case::PatternToNotGraph));
        }
        // A reach of no steps could never answer a row.
        if reaches && p.steps.is_empty() {
            let measure = "reach".to_owned();
            return Err(Error::invalid(Case::MeasureNoSteps { measure }));
        }
        let start_name = p.start.named.clone();
        let mut names = vec![start_name.unwrap_or_else(|| start_decl.name.to_string())];
        // The type each step reaches, checked against the declarations.
        let mut types = vec![start_decl.name.clone()];
        // Each name bound over several types: where its step stands, and
        // the types it reaches.
        let mut several: Vec<Option<(Place, BTreeSet<TypeName>)>> = vec![None];
        let mut filters = Vec::new();
        let mut reach: Vec<Reach> = Vec::new();
        let measure = measures::is_measure(&p.project);
        // A graph whose steps all start at depth 0 is one walk over any of
        // their relations from the start (018); any other is a chain, and
        // so is a reach.
        let chain = reaches || p.steps.iter().any(|s| s.depth.min > 0);
        let lenient = match p.project {
            Projection::Graph => !chain,
            _ => p.project == Projection::Components,
        };
        let last = p.steps.len().saturating_sub(1);
        // A step another walks from reaches one type (080): the next, where
        // it names no `from`, or a later one whose `from` names it.
        let walked_from: Vec<bool> = (0..p.steps.len())
            .map(|i| {
                let s = &p.steps[i];
                let next = p.steps.get(i + 1).is_some_and(|n| n.from.is_none());
                // Its name: its `as`, else its type or its relation.
                let ty = s.ty.as_ref().map(TypeName::as_str);
                let own = match s.named.as_deref() {
                    Some(n) => [Some(n), None],
                    None => [ty, Some(s.relation.as_str())],
                };
                let named = |n: &Step| n.from.as_deref().is_some_and(|f| own.contains(&Some(f)));
                next || p.steps[i + 1..].iter().any(named)
            })
            .collect();
        // Each step's slots, and whether an optional step binds each slot.
        let (mut shapes, mut optional) = (Vec::new(), vec![false]);
        // Whether a step's `where` reads a name bound before it (080).
        let mut across = Vec::new();
        for (i, step) in p.steps.iter_mut().enumerate() {
            // A place names the step's relation as its caller wrote it.
            let at = Place::Step {
                step: i + 1,
                relation: step.relation.clone(),
            };
            let binds_none = step.join.is_some() || step.absent;
            // The slots a step walks from and joins: names bound before it,
            // the start's at 0 (080). A measure's steps are the relations
            // around its records, each read from the start, not a chain, as
            // a neighbourhood's and a path's are; a chain's step walks from
            // the one before it.
            let slot = |member: &str, n: &String| {
                let (member, name) = (member.to_owned(), n.clone());
                let case = Case::StepNameUnbound { member, name };
                let slot = names.iter().position(|b| !b.is_empty() && b == n);
                slot.ok_or_else(|| Error::invalid(Said::from(case).within(at.clone())))
            };
            let (from, join) = match binds {
                true => (
                    (step.from.as_ref()).map_or(Ok(i), |n| slot("from", n))?,
                    (step.join.as_ref()).map(|n| slot("join", n)).transpose()?,
                ),
                false => {
                    let walks = if measure || !chain { 0 } else { i };
                    if step.from.as_ref().is_some_and(|n| *n != names[walks]) {
                        let (member, projection) = ("from".to_owned(), projection_name(&p.project));
                        let case = Case::PatternTreeMember { member, projection };
                        return Err(Error::invalid(Said::from(case).within(at)));
                    }
                    (walks, None)
                }
            };
            let past = std::iter::once(from).chain(join).find(|k| optional[*k]);
            if let Some(k) = past.filter(|_| !step.optional) {
                let case = Case::StepPastOptional {
                    name: names[k].clone(),
                };
                return Err(Error::invalid(Said::from(case).within(at)));
            }
            optional.push(step.optional);
            let (absent, opt) = (step.absent, step.optional);
            shapes.push(graph::Shape {
                from,
                join,
                optional: opt,
                absent,
            });
            let here = match (measure, binds) {
                (true, _) => start_decl.name.clone(),
                (false, true) => types[from].clone(),
                (false, false) => types.last().expect("the start is in").clone(),
            };
            let owner = self.as_declared(&here, step);
            let owner = owner.as_ref();
            // Rows are walked as a chain whatever their depths.
            let walked = measure || chain || binds;
            let reached = match walked {
                true => self.step_target(&here, &step.relation, step.walk, (lenient, owner))?,
                // A relation no type its reader is shown declares is walked
                // from nowhere: hidden or absent, it is refused alike (006).
                false if !self.out_any(&step.relation) => {
                    let (ty, relation, walk) = (here, step.relation.clone(), step.walk);
                    return Err(Error::invalid(Case::NoRelationToWalk {
                        ty,
                        relation,
                        walk,
                    }));
                }
                false => None,
            };
            // A neighbourhood and a path keep a record at every hop; a chain
            // walks a step's whole range first. A measure takes no `type`.
            let far = self.through(step, !(binds || chain), owner);
            let owner = owner.cloned();
            reach.push(Reach { owner, far });
            self.probe_derived(&here, Some(&step.relation), step.walk, &reach[i])?;
            // Such a step reaches every type of its whole range from `here`,
            // and its `type` may name any of them; a neighbourhood's reaches
            // every type its relation does. It is judged once the relation
            // is probed around `here`, so one held from the field that
            // carries it there is answered that first.
            let all = match walked {
                true => self.ranged(step, &here, reach[i].owner.as_ref()),
                false => self.neighbours(step, None, reach[i].owner.as_ref()),
            };
            // A join that can reach no type its name binds could never hold.
            if let Some(j) = join {
                let one = || BTreeSet::from([types[j].clone()]);
                let bound = several[j].as_ref().map_or_else(one, |(_, all)| all.clone());
                if bound.is_disjoint(&all) {
                    let name = names[j].clone();
                    let (bound, reached) = (bound.iter().collect(), all.iter().collect());
                    let case = Case::JoinNeverHolds {
                        name,
                        bound,
                        reached,
                    };
                    return Err(Error::invalid(Said::from(case).within(at)));
                }
            }
            // The step after another in a chain is judged from one type.
            // Rows and an aggregate may bind a step no other walks from over
            // several types (018, extended): its records hold their key,
            // `_type`, `_depth` and the fields every type declares alike; a
            // `where` or a path that reads another field is refused. A join
            // and an absent step bind none.
            let leaf = binds_none || !walked_from[i];
            let open = binds && leaf && step.ty.is_none() && all.len() > 1;
            let one = (binds && !open) || (chain && !measure && i < last);
            let reached = self.neighbour_target(&at, step, &all, one)?.or(reached);
            // At a valid time a walk keeps each record by its own type's
            // valid time, so a step of a hop or more probes every type it
            // can meet (024, lakelands#831): from the declarations, since a
            // refusal that waited for a record would say one was reached.
            if self.valid_at.is_some() && step.depth.max > 0 {
                for t in self.met(step, (!lenient).then_some(&here), &reach[i]) {
                    self.visible_type(&t)?;
                }
            }
            let reached = reached.or_else(|| step.ty.clone()).unwrap_or(here);
            self.visible_type(&reached)?;
            // A view with parameters is bound only by a read that names it
            // (018): a step that arrives at one other than the start has no
            // values to read it with, so the pattern is refused before any
            // record is read, for every type the step reaches.
            for ty in all
                .iter()
                .chain([&reached])
                .filter(|t| **t != start_decl.name)
            {
                let decl = self.sight.ty(ty)?;
                if let Some(param) = decl.view.as_ref().and_then(|v| v.params.first()) {
                    return Err(Error::invalid(
                        Said::from(unbound(ty, param, true)).within(at.clone()),
                    ));
                }
            }
            // An open step's name is its relation as its caller wrote it,
            // which its place keeps: the inverse's where it named that.
            let written = match &at {
                Place::Step { relation, .. } => relation.to_string(),
                _ => step.relation.to_string(),
            };
            let name = match (&step.named, open) {
                _ if binds_none => String::new(),
                (Some(name), _) => name.clone(),
                (None, true) => written,
                (None, false) => reached.to_string(),
            };
            several.push((open && !binds_none).then(|| (at.clone(), all.clone())));
            if binds && !binds_none && names.contains(&name) {
                let said = Said::from(Case::PatternBindsTwice { name });
                return Err(Error::invalid(said.within(at)));
            }
            names.push(name);
            // A `where` over several types is read against each, once
            // every field it reads is declared alike by all of them.
            let wide = step.ty.is_none() && all.len() > 1;
            let over = if wide {
                all
            } else {
                BTreeSet::from([reached.clone()])
            };
            // Rows read each name bound before the step, the start's and a
            // sibling branch's included, as `<name>.<field>` (080).
            let earlier: Vec<Earlier> = (0..=i)
                .filter(|k| binds && !names[*k].is_empty())
                .map(|k| {
                    let one = || (BTreeSet::from([types[k].clone()]), None);
                    let wide = |(at, all): &(Place, _)| (BTreeSet::clone(all), Some(at.clone()));
                    let (bound, at) = several[k].as_ref().map_or_else(one, wide);
                    (names[k].as_str(), bound, at)
                })
                .collect();
            let parsed = step.filter.as_ref().and_then(|c| c.typed().ok());
            // A first segment reads an earlier name where no type the step
            // reaches has a field or relation of it its reader may know of.
            let name_of = |f: &FieldName| {
                let reads =
                    |t: &TypeName| (self.shape.ontology.ty(t)).is_ok_and(|d| self.reads_name(d, f));
                (earlier.iter().find(|(n, ..)| *n == f.as_str())).filter(|_| over.iter().all(reads))
            };
            for f in parsed.iter().flat_map(Expr::fields_read) {
                if wide && name_of(f).is_none() {
                    self.alike(&at, &over, f)?;
                }
            }
            // A name over several types reads a field all declare alike,
            // judged where the name was bound.
            let mut paths = Vec::new();
            parsed
                .iter()
                .for_each(|e| lakelands_api::expr::paths(e, &mut paths));
            for p in paths.iter().filter(|p| p.len() > 1) {
                let Some((_, all, Some(bound))) = name_of(&p[0]) else {
                    continue;
                };
                if !["key", DEPTH, TYPE].contains(&p[1].as_str()) {
                    self.alike(bound, all, &p[1])?;
                }
            }
            let at = [at, Place::Where];
            let (mut filter, mut reads) = (BTreeMap::new(), BTreeSet::new());
            for t in over.iter().filter(|_| step.filter.is_some()) {
                let decl = self.sight.ty(t)?;
                let judged = self.step_filter(decl, step.filter.as_ref(), &at, &earlier);
                let judged = judged.map_err(|e| if wide { of_type(t, e) } else { e })?;
                if let Some((e, named)) = judged {
                    filter.insert(t.clone(), e);
                    reads.extend(named);
                }
            }
            // A join or an absent step binds no name to bind null, so it is
            // optional only as a step of an optional branch. A `where` reads
            // an optional name as absent where its branch has no match.
            if binds_none && past.is_none() && step.optional {
                let case = Case::StepOptionalNoName {
                    join: step.join.is_some(),
                };
                return Err(Error::invalid(Said::from(case).within(at[0].clone())));
            }
            filters.push((step.filter.is_some()).then_some(filter));
            across.push(!reads.is_empty());
            types.push(reached);
        }
        // A depth no walk takes is refused for what the pattern says,
        // before a record is read: with no start there is no walk to say it.
        for s in &p.steps {
            graph::check_depth(s.depth, max_depth)?;
        }
        // A pattern judged before its start's reranker is asked reads
        // nothing: a judgement belongs above this return, a read below.
        if self.second.borrow().judging {
            let rows = Vec::new();
            return Ok(Ok(QueryResult::Rows {
                rows,
                next_cursor: None,
            }));
        }
        let (reader, reach) = (self, &reach[..]);
        // Rows, a reach and every walk a step's `where` reads hold what the
        // walk reads, so that `where` reads each record its visibility read
        // brought for the batch (lakelands#940, lakelands#1027). A graph or
        // a measure without one walks more records than it answers.
        let wheres = p.steps.iter().any(|s| s.filter.is_some());
        let records = (binds || reaches || wheres).then(RefCell::default);
        let hidden = Visible {
            reader,
            reach,
            records,
        };
        if measure {
            return self.measure(p, limit, carried, &hidden);
        }
        if p.to.is_some() && chain {
            return Err(Error::invalid(Case::PatternToChain));
        }
        // A cursor is checked before any arm answers: a path and an
        // aggregate hand out none, so every cursor is another read's there.
        let (offset, after) = offset(p.cursor.as_ref(), p, &params)?;
        if let (Some(_), Some(c)) = (&p.start.rerank, &p.cursor) {
            let state = decode(c)?;
            // A cursor this lake handed out holds at most the window.
            let top = p.start.rerank.as_ref().map_or(0, |r| r.top as usize);
            match state.stage {
                Some(st) if st.order.len() <= top => self.second.borrow_mut().resumed = Some(st),
                Some(_) => return Err(Error::invalid(Case::CursorForeign)),
                None => {}
            }
        }
        // Whether a record reached over step `i` is kept and walked from:
        // of the step's `type`, and its `where` holds of it.
        let keep = |i: usize, node: &graph::Node| -> Result<bool> {
            if p.steps[i].ty.as_ref().is_some_and(|t| *t != node.0) {
                return Ok(false);
            }
            let Some(f) = &filters[i] else {
                return Ok(true);
            };
            let Some(f) = f.get(&node.0) else {
                return Ok(false);
            };
            // One that reads another name holds beside a row's bindings.
            if across[i] {
                return Ok(true);
            }
            let r = hidden.record(node)?;
            Ok(r.is_some_and(|r| self.holds(f, &r, true)))
        };
        // The start: the record a key names, what a search finds, or the
        // records a filter selects, which rows read a page at a time.
        let few: Option<Vec<StoredRecord>> = if let Some(key) = &p.start.key {
            if self.start_found(p, key)? {
                Some(self.get_one(&start_ty, key)?.into_iter().collect())
            } else {
                Some(Vec::new())
            }
        } else if let Some(mode) = &p.start.search {
            let keys = match self.start_hits(p, mode, carried)? {
                Ok(keys) => keys,
                Err(moved) => return Ok(Err(moved)),
            };
            let held = keys
                .iter()
                .filter_map(|k| self.view.get(&start_ty, k).ok().flatten());
            Some(self.at_read(&start_ty, held.collect()))
        } else {
            None
        };
        let at = [Place::Start, Place::Where];
        let filter = self.record_filter(&start_decl, p.start.filter.as_ref(), &at)?;
        // A chain, a path and rows of a view read every start; a
        // neighbourhood's starts are its first records, so it reads as many
        // as its page reaches and one more (lakelands#963).
        let hood = p.project == Projection::Graph && !chain && p.to.is_none();
        // A neighbourhood's cursor names the last start its page held
        // (lakelands#1050). A graph's other `after` is another read's.
        let after_start = match &after {
            Some((v, key)) if hood && v.as_slice() == [Value::from(0u64)] => Some(key.clone()),
            Some(_) if p.project == Projection::Graph => {
                return Err(Error::invalid(Case::CursorForeign));
            }
            _ => None,
        };
        let mut inside = None;
        let few = match few {
            None if hood && start_decl.view.is_none() => {
                let more = limit.saturating_add(1);
                let after = after_start.clone().map(|key| (Vec::new(), key));
                let read = match after {
                    Some(after) => {
                        self.page(&start_decl, filter.as_ref(), &[], Some(after), more)?
                    }
                    None => Vec::new(),
                };
                if read.len() > limit {
                    inside = after_start;
                    Some(read)
                } else {
                    let want = offset.saturating_add(limit).saturating_add(1);
                    Some(self.page(&start_decl, filter.as_ref(), &[], None, want)?)
                }
            }
            None if !binds || start_decl.view.is_some() => {
                Some(self.records(&start_decl, filter.as_ref())?)
            }
            few => few,
        };
        let from: Vec<graph::Node> = (few.iter().flatten())
            .map(|s| (s.ty.clone(), s.key.clone()))
            .collect();
        // A path, from its one start.
        if let Some((to_ty, to_key)) = &p.to {
            let (nodes, edges) = match from.as_slice() {
                [] => (Vec::new(), Vec::new()),
                [one] => {
                    let to = (to_ty.clone(), to_key.clone());
                    graph::shortest_path(&hidden, one, &to, &p.steps, max_depth, keep)?.map_or_else(
                        Default::default,
                        |path| {
                            // Along a path a record's depth is its hop.
                            let hops = (0..).zip(path.nodes);
                            let nodes = hops.map(|(depth, node)| graph::Reached { node, depth });
                            (nodes.collect(), path.edges)
                        },
                    )
                }
                more => {
                    let count = more.len();
                    return Err(Error::invalid(Case::PathStarts { count }));
                }
            };
            return self.graph_result(nodes, edges, None).map(Ok);
        }
        // A graph: several starts are one walk, each record at the fewest
        // hops any start reaches it in, ordered by (depth, type, key) as
        // one walk orders what it reaches (026, 029), a page at a time.
        if p.project == Projection::Graph {
            let bound = (max_depth, self.lake.limits.max_graph_nodes);
            let g = match (chain, &inside) {
                (true, _) => graph::spanned(&hidden, &from, &p.steps, bound, offset, limit, keep)?,
                (false, Some(last)) => {
                    // The page's starts are held for a step's `where`, and a
                    // link is walked back to a start's type alone.
                    hidden.remember(few.iter().flatten().cloned());
                    let only = |r: &Reach| {
                        let far = r.far.as_ref().is_none_or(|f| f.contains(&start_ty));
                        let far = Some(far.then(|| start_ty.clone()).into_iter().collect());
                        Reach {
                            owner: r.owner.clone(),
                            far,
                        }
                    };
                    let back: Vec<Reach> = reach.iter().map(only).collect();
                    let back = Visible {
                        reader,
                        reach: &back,
                        records: None,
                    };
                    // The starts an earlier page held, of the page's far
                    // records: of the start's type, at or before the
                    // cursor's key, and selected by its filter, read together.
                    let earlier = |far: &BTreeSet<graph::Node>| -> Result<BTreeSet<graph::Node>> {
                        // What the walk holds is not read again.
                        let held = |n: &graph::Node| {
                            hidden
                                .records
                                .as_ref()
                                .and_then(|h| h.borrow().get(n).cloned())
                        };
                        let (mut rows, mut keys) = (Vec::new(), Vec::new());
                        for n in far.iter().filter(|n| n.0 == start_ty && n.1 <= *last) {
                            match held(n) {
                                Some(r) => rows.push(r),
                                None => keys.push(n.1.clone()),
                            }
                        }
                        if !keys.is_empty() {
                            rows.extend(self.get_many(&start_ty, &keys)?);
                        }
                        Ok(rows
                            .into_iter()
                            .filter(|r| filter.as_ref().is_none_or(|f| self.holds(f, r, true)))
                            .map(|r| (r.ty, r.key))
                            .collect())
                    };
                    graph::starts_page(&hidden, &back, &from, &p.steps, limit, earlier, keep)?
                }
                (false, None) => {
                    graph::subgraph(&hidden, &from, &p.steps, max_depth, offset, limit, keep)?
                }
            };
            // Where the page ends inside the starts, the next one begins
            // after its last start.
            let last_start = (g.nodes.last()).filter(|r| r.depth == 0 && !chain);
            let at = last_start.map(|r| (vec![Value::from(0u64)], r.node.1.clone()));
            let next = g
                .truncated
                .then(|| self.page_cursor(p, (offset + limit, at), params));
            return self.graph_result(g.nodes, g.edges, next).map(Ok);
        }
        // What each step reaches (lakelands#991): a step walked whole before
        // the next, each record once at the first step, in the order
        // (step, depth, type, key). A page walks until it holds one row
        // more than it answers, so a later page walks again the steps up
        // to the one its cursor names; the cursor is the last row's.
        if reaches {
            type Row = (u64, u64, String, RecordKey);
            let past: Option<Row> = match &after {
                None => None,
                Some((v, key)) => match v.as_slice() {
                    [Value::Number(s), Value::Number(d), Value::String(t)] => {
                        let (s, d) = (s.as_u64(), d.as_u64());
                        let at = s.zip(d).map(|(s, d)| (s, d, t.clone(), key.clone()));
                        Some(at.ok_or_else(|| Error::invalid(Case::CursorForeign))?)
                    }
                    _ => return Err(Error::invalid(Case::CursorForeign)),
                },
            };
            let (mut rows, mut more): (Vec<Row>, bool) = (Vec::new(), false);
            let bound = (max_depth, self.lake.limits.max_graph_nodes);
            graph::reach(&hidden, &from, &p.steps, bound, keep, |step, first| {
                for r in first {
                    let row = (
                        step as u64,
                        u64::from(r.depth),
                        r.node.0.to_string(),
                        r.node.1,
                    );
                    if past.as_ref().is_some_and(|p| row <= *p) {
                        continue;
                    }
                    if rows.len() == limit {
                        more = true;
                        return Ok(false);
                    }
                    rows.push(row);
                }
                Ok(true)
            })?;
            let next_cursor = (more.then(|| rows.last()).flatten()).map(|(s, d, t, k)| {
                let at = vec![Value::from(*s), Value::from(*d), Value::String(t.clone())];
                self.page_cursor(p, (0, Some((at, k.clone()))), params)
            });
            let cell = |(s, d, t, k): Row| {
                BTreeMap::from([
                    ("step".to_owned(), Value::from(s)),
                    ("depth".to_owned(), Value::from(d)),
                    ("type".to_owned(), Value::String(t)),
                    ("key".to_owned(), Value::String(k.to_string())),
                ])
            };
            let rows = rows.into_iter().map(cell).collect();
            return Ok(Ok(QueryResult::Rows { rows, next_cursor }));
        }
        // Rows are ordered, projected and measured by their paths,
        // `name.field`; a name they do not hold is refused (018, extended).
        // An order or a measure probes its field as `find`'s do (024), as
        // a named key's fields are.
        let bound = |n: &str| !n.is_empty() && names.iter().any(|b| b == n);
        // A path under a name over several types reads a field every one
        // declares alike, against each (`true`), else is refused. Its `key`,
        // `_type` and `_depth` are the lake's columns, never probed.
        let typed = |n: &str, f: FieldName| {
            let i = names.iter().position(|b| b == n).expect("a bound name");
            match &several[i] {
                Some(_) if ["key", DEPTH, TYPE].contains(&f.as_str()) => Ok(Vec::new()),
                Some((at, all)) => {
                    self.alike(at, all, &f)?;
                    Ok(all.iter().map(|t| (t.clone(), f.clone(), true)).collect())
                }
                None => Ok(vec![(types[i].clone(), f, false)]),
            }
        };
        let path = |name: &str| {
            let (n, f) = aggregate::row_path(name, &bound).map_err(Error::invalid)?;
            typed(&n, f)
        };
        let ordered: Vec<Vec<_>> = (p.order_by.iter())
            .map(|o| path(o.field.as_str()))
            .collect::<Result<_>>()?;
        let mut paths: Vec<Read> = ordered.into_iter().flatten().collect();
        let orders = paths.len();
        let shared: Vec<Shared> = (several.iter())
            .map(|s| s.as_ref().map(|(_, all)| self.shared(all)))
            .collect();
        // An open name's fields are shaped from a type it reaches: the
        // first hop's may not be one over a range.
        let shaped: Vec<TypeName> = (types.iter().zip(&several))
            .map(|(t, s)| {
                s.as_ref()
                    .and_then(|(_, all)| all.first())
                    .unwrap_or(t)
                    .clone()
            })
            .collect();
        let row = self.row_shape(&names, &shaped, &shared);
        let mut over_rows = None;
        if let Projection::Aggregate(a) = &p.project {
            let (a, read) = aggregate::over_rows(a, &bound).map_err(Error::invalid)?;
            for (n, f) in read {
                paths.extend(typed(&n, f)?);
            }
            over_rows = Some(a);
        }
        self.probe_rows(&paths, orders)?;
        // A projected field is named, not probed: one its reader may not
        // know of is one its type does not have, and a masked one is left
        // out of the row, as an absent value is.
        if let Projection::Rows { fields } = &p.project {
            let read = fields.iter().map(|f| path(f)).collect::<Result<Vec<_>>>()?;
            for (t, f, _) in read.into_iter().flatten() {
                let decl = self.shape.ontology.ty(&t)?;
                if !self.lakes(decl, &f) {
                    self.sight.check_known(decl, &f)?;
                }
            }
        }
        // What a named key can never compute is refused, as `find` refuses
        // it, over the row's shape (028, extended).
        if let Some(a) = &over_rows {
            let keys = aggregate::group_keys(&a.group_by).map_err(Error::invalid)?;
            for (name, e) in keys {
                if let Some(e) = e {
                    crate::declare::evaluates(&e, &row, &[Place::Group { name }])?;
                }
            }
        }
        // Rows: one per match, walked a page of starts at a time
        // (lakelands#940). The starts come in key order, or in an order the
        // engine gives them (041): each path a stored field of the start's
        // own, as `page` reads.
        let mut keep = keep;
        let ordered = few.is_none().then(|| {
            let own = |o: &Order| {
                let (n, f) = o.field.as_str().split_once('.')?;
                let field = FieldName::from(f);
                let stored = start_decl
                    .fields
                    .iter()
                    .any(|x| x.name == field && !x.at_read());
                let own = n == names[0] && stored && !self.lakes(&start_decl, &field);
                own.then(|| Order { field, ..o.clone() })
            };
            p.order_by.iter().map(own).collect::<Option<Vec<Order>>>()
        });
        // A record a row binds, as a `where` of a step after it reads it.
        let object = |node: &graph::Node, depth: u32, open: Option<&Shared>| -> Result<Value> {
            let rec = hidden.record(node)?;
            let bound = self.bound((node, depth, open.and_then(Option::as_ref)), rec)?;
            Ok(Value::Object(bound.into_iter().collect()))
        };
        // Whether a record step `i` keeps holds its `where` beside what the
        // steps before it bound: a name its type has no field or relation of
        // its reader may know of is that name's record, null where an
        // optional step bound none.
        let beside = |i: usize,
                      start: &graph::Node,
                      prefix: &[Option<graph::Reached>],
                      r: &graph::Reached|
         -> Result<bool> {
            let Some(f) = filters[i]
                .as_ref()
                .and_then(|f| f.get(&r.node.0))
                .filter(|_| across[i])
            else {
                return Ok(true);
            };
            let Some(mut rec) = hidden.record(&r.node)? else {
                return Ok(false);
            };
            let decl = self.shape.ontology.ty(&r.node.0)?;
            for (k, name) in names[..=i].iter().enumerate() {
                let name = FieldName::from(name.as_str());
                if name.as_str().is_empty() || !self.reads_name(decl, &name) {
                    continue;
                }
                let at = match k {
                    0 => Some((start, 0)),
                    k => prefix[k - 1].as_ref().map(|r| (&r.node, r.depth)),
                };
                let v = match at {
                    Some((node, depth)) => object(node, depth, shared.get(k))?,
                    None => Value::Null,
                };
                rec.fields.insert(name, v);
            }
            Ok(self.holds(f, &rec, true))
        };
        let row = |start: &graph::Node, binding: &[Option<graph::Reached>]| -> Result<_> {
            let mut row = BTreeMap::new();
            self.bind(&mut row, &names[0], (start, 0, None), hidden.record(start)?)?;
            for ((r, name), open) in binding.iter().zip(&names[1..]).zip(&shared[1..]) {
                match r {
                    _ if name.is_empty() => {}
                    // An optional step's name with no match (080).
                    None => {
                        for f in ["key", DEPTH, TYPE] {
                            row.insert(format!("{name}.{f}"), Value::Null);
                        }
                    }
                    Some(r) => {
                        let at = (&r.node, r.depth, open.as_ref());
                        self.bind(&mut row, name, at, hidden.record(&r.node)?)?;
                    }
                }
            }
            if let Projection::Rows { fields } = &p.project
                && !fields.is_empty()
            {
                row.retain(|k, _| fields.iter().any(|f| f == k));
            }
            Ok(row)
        };
        let walk = |starts: &Starts,
                    each: &mut EachBinding,
                    keep: &mut dyn FnMut(usize, &graph::Node) -> Result<bool>| {
            let nodes: Vec<graph::Node> = starts
                .iter()
                .map(|(_, r)| (r.ty.clone(), r.key.clone()))
                .collect();
            hidden.remember(starts.iter().map(|(_, r)| r.clone()));
            let at: BTreeMap<&graph::Node, &SortKey> =
                nodes.iter().zip(starts.iter().map(|(k, _)| k)).collect();
            let tree = (&p.steps[..], &shapes[..]);
            let walked = graph::tree_each(
                &hidden,
                &nodes,
                tree,
                max_depth,
                (keep, &beside),
                |start, b| each(at[start], start, b),
            );
            hidden.forget(&nodes);
            walked
        };
        let streamed = match p.order_by.is_empty() {
            true => Some(Vec::new()),
            false => ordered.flatten(),
        };
        let streamed = streamed.filter(|_| over_rows.is_none());
        // A cursor's start is a position in the order the starts are read
        // in, one of the starts at hand where they are few, and none where
        // every row is read: any other is not one this lake handed out.
        let fits = |(v, k): &SortKey| match (&streamed, &few) {
            (None, _) => false,
            (Some(o), Some(few)) => v.len() == o.len() && few.iter().any(|r| r.key == *k),
            (Some(o), None) => v.len() == o.len(),
        };
        if after.as_ref().is_some_and(|a| !fits(a)) {
            return Err(Error::invalid(Case::CursorForeign));
        }
        if let Some(order) = streamed {
            // The page and one row more, in the starts' order; a cursor
            // names the last start whose rows are all answered, and how
            // many of the next one's are.
            let (mut rows, mut skipped, mut more) = (Vec::new(), 0, None);
            let (mut done, mut at, mut of_at) = (after.clone(), None::<SortKey>, 0);
            self.start_pages(
                few,
                (&start_decl, filter.as_ref(), &order),
                after,
                &mut |starts| {
                    walk(
                        starts,
                        &mut |here, start, binding| {
                            if at.as_ref() != Some(here) {
                                if of_at > 0 {
                                    done = at.take();
                                }
                                (at, of_at) = (Some(here.clone()), 0);
                            }
                            of_at += 1;
                            if skipped < offset {
                                skipped += 1;
                                return Ok(true);
                            }
                            if rows.len() == limit {
                                more = Some((done.clone(), of_at - 1));
                                return Ok(false);
                            }
                            rows.push(row(start, binding)?);
                            Ok(true)
                        },
                        &mut keep,
                    )?;
                    Ok(more.is_none())
                },
            )?;
            let next_cursor = more.map(|(after, n)| self.page_cursor(p, (n, after), params));
            return Ok(Ok(QueryResult::Rows { rows, next_cursor }));
        }
        // Every row: an aggregate's whole, an order's as their sort values
        // and records until the page is cut. An order over a step's record,
        // and an aggregate, hold at most `max_graph_nodes` rows, past which
        // the read is refused, never cut, as a whole-graph measure is (026).
        let own = |o: &Order| {
            o.field
                .as_str()
                .split_once('.')
                .is_some_and(|(n, _)| n == names[0])
        };
        let bounded = over_rows.is_some() || !p.order_by.iter().all(own);
        let max = self.lake.limits.max_graph_nodes;
        let member = ["order_by", "project"][usize::from(over_rows.is_some())];
        let (mut rows, mut held) = (Vec::new(), Vec::new());
        self.start_pages(
            few,
            (&start_decl, filter.as_ref(), &[]),
            None,
            &mut |starts| {
                walk(
                    starts,
                    &mut |_, start, binding| {
                        if bounded && rows.len() + held.len() >= max as usize {
                            let (field, reason) = (Some(member.into()), Refusal::TooLarge { max });
                            return Err(Error::refused(field, reason));
                        }
                        let r = row(start, binding)?;
                        let value = |o: &Order| r.get(o.field.as_str()).cloned();
                        let vals = p.order_by.iter().map(|o| value(o).unwrap_or(Value::Null));
                        let vals: Vec<_> = vals.collect();
                        match over_rows {
                            Some(_) => rows.push(r),
                            None => held.push((vals, start.clone(), binding.to_vec())),
                        }
                        Ok(true)
                    },
                    &mut keep,
                )?;
                Ok(true)
            },
        )?;
        if let Some(a) = &over_rows {
            let as_fields: Vec<BTreeMap<FieldName, Value>> = rows
                .iter()
                .map(|r| {
                    r.iter()
                        .map(|(k, v)| (FieldName::from(k.as_str()), v.clone()))
                        .collect()
                })
                .collect();
            let refs: Vec<&BTreeMap<FieldName, Value>> = as_fields.iter().collect();
            let groups =
                aggregate::aggregate(&refs, a, self.told(), self.now).map_err(Error::invalid)?;
            return Ok(Ok(QueryResult::Groups { rows: groups }));
        }
        let order: Vec<Order> = p.order_by.clone();
        let mut keyed: Vec<_> = (held.into_iter().enumerate())
            .map(|(i, (vals, start, b))| ((vals, RecordKey::from(format!("{i:020}"))), (start, b)))
            .collect();
        keyed.sort_by(|a, b| Self::compare(&order, &a.0, &b.0));
        // Sorted first: an order may read a path `fields` leaves out. The
        // page's rows alone are bound again.
        let next_cursor = (keyed.len() > offset + limit)
            .then(|| self.page_cursor(p, (offset + limit, None), params));
        let page = keyed.into_iter().skip(offset).take(limit);
        let rows = page.map(|(_, (s, b))| row(&s, &b)).collect::<Result<_>>()?;
        Ok(Ok(QueryResult::Rows { rows, next_cursor }))
    }

    /// Hands `each` the starts of a pattern's rows a page at a time, each
    /// with its position (its sort values and key), after `after`: the
    /// records of `few` in their order, or those of the stored type a
    /// filter selects in `order`, read by `page`, until `each` answers
    /// that it has what it needs.
    fn start_pages(
        &self,
        few: Option<Vec<StoredRecord>>,
        (decl, filter, order): (&TypeDecl, Option<&Expr>, &[Order]),
        mut after: Option<SortKey>,
        each: &mut dyn FnMut(&Starts) -> Result<bool>,
    ) -> Result<()> {
        let at = |r: &StoredRecord| (Self::sort_values(order, r), r.key.clone());
        if let Some(few) = few {
            let from = after
                .and_then(|(_, k)| few.iter().position(|r| r.key == k))
                .map_or(0, |i| i + 1);
            let few: Vec<_> = few.into_iter().skip(from).map(|r| (at(&r), r)).collect();
            for page in few.chunks(graph::PAGE) {
                if !each(page)? {
                    break;
                }
            }
            return Ok(());
        }
        // An engine page in a field's order sorts the type again, so an
        // ordered read asks for twice as many starts each time, as `page`
        // does within a call.
        let mut want = graph::PAGE;
        loop {
            let page = self.page(decl, filter, order, after.take(), want)?;
            let page: Vec<_> = page.into_iter().map(|r| (at(&r), r)).collect();
            after = page.last().map(|(k, _)| k.clone());
            if page.is_empty() || !each(&page)? || page.len() < want {
                return Ok(());
            }
            want *= 1 + usize::from(!order.is_empty());
        }
    }

    /// Whether `get` answers the record a pattern's start names by key,
    /// with the start's view parameters.
    pub(super) fn start_found(&self, p: &Pattern, key: &RecordKey) -> Result<bool> {
        let get = Get {
            key: key.clone(),
            with_meta: false,
            at: ReadAt::default(),
            params: p.start.params.clone(),
        };
        match self.get(start_type(p)?, &get) {
            Ok(_) => Ok(true),
            Err(Error::NotFound { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// The keys a pattern's start search finds, best first.
    pub(super) fn start_hits(
        &self,
        p: &Pattern,
        mode: &SearchMode,
        carried: Option<Pulled>,
    ) -> Result<Judged<Vec<RecordKey>>> {
        let s = self.start_search(p, mode)?;
        let found = self.search(&s, carried)?;
        Ok(found.map(|f| f.hits.into_iter().map(|h| h.record.key).collect()))
    }

    /// Refuse an order or a measure over a path of a pattern's rows whose
    /// field the principal may not read or the type does not declare, as
    /// `find` refuses one (024, 006). The lake's columns probe nothing.
    /// The first `ordered` paths are orders, refused by a field computed at
    /// the read. A path over several types (`wide`) is refused naming its
    /// type.
    fn probe_rows(&self, paths: &[Read], ordered: usize) -> Result<()> {
        for (i, (ty, field, wide)) in paths.iter().enumerate() {
            let decl = self.shape.ontology.ty(ty)?;
            if self.lakes(decl, field) {
                continue;
            }
            let probed = self.sight.check_probe(decl, [field]);
            probed.map_err(|e| if *wide { of_type(ty, e) } else { e })?;
            if i < ordered {
                Self::steady_order(decl, field.as_str())?;
            }
        }
        Ok(())
    }

    /// A step's `where` over one record of `decl`, named `at`, after the
    /// principal is found to read every field it reads (024), and the
    /// names bound before the step it reads (080): a path whose first
    /// segment is one of `earlier`, which `decl` has no field or relation
    /// of its reader may know of, reads that name's record, each field
    /// probed on every type the name binds and the lake's columns on none.
    fn step_filter(
        &self,
        decl: &TypeDecl,
        c: Option<&lakelands_api::expr::Condition>,
        at: &[Place],
        earlier: &[Earlier],
    ) -> Result<Option<(Expr, BTreeSet<String>)>> {
        let Some(c) = c else { return Ok(None) };
        let e = self.named_sets(&decl.name, c, at)?;
        let named = |p: &[FieldName]| {
            let name = (p.len() > 1 && self.reads_name(decl, &p[0])).then_some(&p[0])?;
            earlier.iter().find(|(n, ..)| *n == name.as_str())
        };
        let mut paths = Vec::new();
        lakelands_api::expr::paths(&e, &mut paths);
        let mut own = core_derive::reads(&e);
        let mut across = BTreeSet::new();
        for p in &paths {
            let Some((_, all, _)) = named(p) else {
                continue;
            };
            own.remove(&p[0]);
            across.insert(p[0].to_string());
            for t in all {
                let of = self.sight.ty(t)?;
                if !self.lakes(of, &p[1]) {
                    let probed = self.sight.check_probe(of, [&p[1]]);
                    probed.map_err(|e| if all.len() > 1 { of_type(t, e) } else { e })?;
                }
            }
        }
        self.sight.check_probe(decl, &own)?;
        let far = |t: &TypeName| self.shape.ontology.ty(t).ok();
        let field = |p: &[FieldName]| {
            let Some((_, all, _)) = named(p) else {
                let read = decl.stepped(p, &|_| None);
                return read.last().map(|(_, f)| (f.ty.kind, read.len()));
            };
            let of = far(all.first()?)?;
            if self.lakes(of, &p[1]) {
                let number = p[1].as_str() == DEPTH;
                return Some((
                    if number {
                        StorageKind::Number
                    } else {
                        StorageKind::Text
                    },
                    2,
                ));
            }
            // The name's record holds its own fields, as the step's does.
            let read = of.stepped(&p[1..], &|_| None);
            read.last().map(|(_, f)| (f.ty.kind, read.len() + 1))
        };
        let kind = lakelands_api::expr::kind::of(&e, &field);
        kind.map_err(|why| Error::invalid(crate::declare::at(at, why)))?;
        Ok(Some((e, across)))
    }

    /// Whether a path whose first segment is `first` may read a name a row
    /// binds over a record of `decl`: no field or relation of it its reader
    /// may know of is so named, so one it may not know of never shows.
    fn reads_name(&self, decl: &TypeDecl, first: &FieldName) -> bool {
        let shown =
            |r: &RelationDecl| r.name.as_str() == first.as_str() && self.sight.relation(decl, r);
        !(decl.has_field(first) && self.knows(decl, first)) && !decl.relations.iter().any(shown)
    }

    /// Whether a row's `<name>.<field>` is one of the lake's own columns,
    /// not a field of the record: `_depth` and `_type` always (018,
    /// extended), and its key where [`Self::keyed`]. It is never probed.
    fn lakes(&self, decl: &TypeDecl, field: &FieldName) -> bool {
        [DEPTH, TYPE].contains(&field.as_str()) || self.keyed(decl, field)
    }

    /// How the types a step reaches declare `field`, where not alike: one
    /// its reader may not know of is missing (024), and one whose field
    /// type (kind, value type, target or dimensions) is not the one most
    /// declare, the first by name on a tie, is said otherwise. None where
    /// all declare it alike.
    fn unlike(&self, all: &BTreeSet<TypeName>, field: &FieldName) -> Option<Unlike> {
        let declared = |t: &TypeName| {
            let decl = self.shape.ontology.ty(t).ok()?;
            let f = decl.field(field)?;
            self.knows(decl, field).then_some(&f.ty)
        };
        let each: Vec<_> = all.iter().map(|t| (t, declared(t))).collect();
        let count = |d: &FieldType| each.iter().filter(|(_, e)| *e == Some(d)).count();
        let most = (each.iter().rev().filter_map(|(_, d)| *d)).max_by_key(|d| count(d));
        let (mut missing, mut other) = (Vec::new(), Vec::new());
        for (t, d) in &each {
            match d {
                None => missing.push(*t),
                Some(d) if Some(*d) != most => other.push(*t),
                Some(_) => {}
            }
        }
        let missing: Names = missing.into_iter().collect();
        let other: Names = other.into_iter().collect();
        (!missing.0.is_empty() || !other.0.is_empty()).then(|| (field.clone(), missing, other))
    }

    /// Refuse a field a step over `all`, at `at`, reads and they do not
    /// all declare alike ([`Self::unlike`]).
    fn alike(&self, at: &Place, all: &BTreeSet<TypeName>, field: &FieldName) -> Result<()> {
        match self.unlike(all, field) {
            Some(u) => Err(several_types(at, all, Some(u))),
            None => Ok(()),
        }
    }

    /// The fields a name over the types `all` holds: those every one of
    /// them declares alike, but `key`, `_depth` and `_type`, which are the
    /// lake's columns there.
    fn shared(&self, all: &BTreeSet<TypeName>) -> BTreeSet<FieldName> {
        let Some(decl) = all.first().and_then(|t| self.shape.ontology.ty(t).ok()) else {
            return BTreeSet::new();
        };
        let lakes = |f: &FieldName| ["key", DEPTH, TYPE].contains(&f.as_str());
        (decl.fields.iter().map(|f| &f.name))
            .filter(|f| !lakes(f) && self.unlike(all, f).is_none())
            .cloned()
            .collect()
    }

    /// Whether a row's `<name>.<field>` is its record's key: the field is
    /// `key`, and the type has no field of that name its reader may know
    /// of (024).
    fn keyed(&self, decl: &TypeDecl, field: &FieldName) -> bool {
        field.as_str() == "key"
            && !decl
                .fields
                .iter()
                .any(|f| &f.name == field && self.knows(decl, field))
    }

    /// The relations the principal is shown ([`Sight::relation`])
    /// with their owners: those that leave or reach `around`, or all.
    /// A relation it is not shown is one no type declares, at every step,
    /// inverse name and edge.
    pub(super) fn shown_relations(
        &self,
        around: Option<&TypeName>,
    ) -> Vec<(&TypeDecl, &RelationDecl)> {
        let near =
            |t: &TypeDecl, r: &RelationDecl| around.is_none_or(|a| &t.name == a || &r.target == a);
        let types = self.shape.ontology.types.values().map(|e| &e.decl);
        let all = types.flat_map(|t| t.relations.iter().map(move |r| (t, r)));
        all.filter(|(t, r)| near(t, r) && self.sight.relation(t, r))
            .collect()
    }

    /// Whether a type the principal is shown declares a relation of this
    /// name, so that a step over it from elsewhere walks nothing.
    pub(super) fn out_any(&self, relation: &RelationName) -> bool {
        (self.shown_relations(None).iter()).any(|(_, r)| &r.name == relation)
    }

    /// [`Self::as_declared`] where the start names no type: a step names a
    /// relation some type the principal is shown declares, or else the
    /// `inverse` of each relation it is shown that bears it, answered with
    /// its owner. None where a relation is declared under the name.
    pub(super) fn inverses_anywhere(&self, name: &RelationName) -> Vec<(RelationName, TypeName)> {
        let shown = self.shown_relations(None);
        if (shown.iter()).any(|(_, r)| r.name == *name) {
            return Vec::new();
        }
        let inverse = shown
            .into_iter()
            .filter(|(_, r)| r.inverse.as_ref() == Some(name));
        inverse
            .map(|(t, r)| (r.name.clone(), t.name.clone()))
            .collect()
    }

    /// Rewrite a step naming a relation's `inverse` to that relation,
    /// walked the other way. A relation declared under the name wins, so a
    /// step is read as an inverse only when it walks no relation of that
    /// name from `here`. An inverse is one relation (044, extended): its
    /// owner is answered, for the step to be judged and walked by.
    fn as_declared(&self, here: &TypeName, step: &mut Step) -> Option<TypeName> {
        let (name, walk) = (&step.relation, step.walk);
        // Whether a relation leaves `here` (out) or reaches it (in) by the
        // way the step walks.
        let leaves = |t: &TypeDecl, w: Walk| &t.name == here && w != Walk::In;
        let reaches = |r: &RelationDecl, w: Walk| &r.target == here && w != Walk::Out;
        let flipped = match walk {
            Walk::Out => Walk::In,
            Walk::In => Walk::Out,
            Walk::Both => Walk::Both,
        };
        let walked = |t: &TypeDecl, r: &RelationDecl, w: Walk| leaves(t, w) || reaches(r, w);
        let shown = self.shown_relations(Some(here));
        if (shown.iter()).any(|(t, r)| &r.name == name && walked(t, r, walk)) {
            return None;
        }
        let inverse = |(t, r): &&(&TypeDecl, &RelationDecl)| {
            r.inverse.as_ref() == Some(name) && walked(t, r, flipped)
        };
        let (t, r) = shown.iter().find(inverse)?;
        (step.relation, step.walk) = (r.name.clone(), flipped);
        Some(t.name.clone())
    }

    /// The relations a step walks by its name among those shown around
    /// `around`, or anywhere: its `owner`'s alone where it names an inverse.
    pub(super) fn named(
        &self,
        around: Option<&TypeName>,
        relation: &RelationName,
        owner: Option<&TypeName>,
    ) -> Vec<(&TypeDecl, &RelationDecl)> {
        let mut shown = self.shown_relations(around);
        shown.retain(|(t, r)| &r.name == relation && owner.is_none_or(|o| *o == t.name));
        shown
    }

    /// The types a typed step's walk passes through, its `type` among
    /// them: a neighbourhood and a path keep a record at `every_hop`, so
    /// only that type; a chain's step keeps at its end, so each type its
    /// `type` is reached from in the hops left as well.
    fn through(
        &self,
        step: &Step,
        every_hop: bool,
        owner: Option<&TypeName>,
    ) -> Option<BTreeSet<TypeName>> {
        let mut far = BTreeSet::from([step.ty.clone()?]);
        let hops = if every_hop { 1 } else { step.depth.max };
        let shown = self.named(None, &step.relation, owner);
        for _ in 1..hops.min(self.lake.limits.max_depth) {
            // One hop back from what the hops after it lead to.
            let known = far.clone();
            for (t, r) in &shown {
                if step.walk != Walk::In && known.contains(&r.target) {
                    far.insert(t.name.clone());
                }
                if step.walk != Walk::Out && known.contains(&t.name) {
                    far.insert(r.target.clone());
                }
            }
            if far.len() == known.len() {
                break;
            }
        }
        Some(far)
    }

    /// The types a step reaches from `here`, or for a neighbourhood from
    /// wherever its relation is declared: its target walked out, its owner
    /// walked in.
    fn neighbours(
        &self,
        step: &Step,
        here: Option<&TypeName>,
        owner: Option<&TypeName>,
    ) -> BTreeSet<TypeName> {
        let at = |t: &TypeName| here.is_none_or(|h| h == t);
        let mut reached = BTreeSet::new();
        for (t, r) in self.named(here, &step.relation, owner) {
            (step.walk != Walk::In && at(&t.name)).then(|| reached.insert(r.target.clone()));
            (step.walk != Walk::Out && at(&r.target)).then(|| reached.insert(t.name.clone()));
        }
        reached
    }

    /// The types a step's records may be of from `here`, over its whole
    /// range: `here` where it starts at depth 0, and each type a hop
    /// reaches. A range that reaches none is judged by its first hop.
    fn ranged(&self, step: &Step, here: &TypeName, owner: Option<&TypeName>) -> BTreeSet<TypeName> {
        let mut reached = BTreeSet::new();
        let mut at = BTreeSet::from([here.clone()]);
        for hop in 0..=step.depth.max.min(self.lake.limits.max_depth) {
            if hop > 0 {
                at = (at.iter())
                    .flat_map(|t| self.neighbours(step, Some(t), owner))
                    .collect();
            }
            // A hop of the range that adds no type is the last that can.
            let known = reached.len();
            if hop >= step.depth.min {
                reached.extend(at.iter().cloned());
                if reached.len() == known {
                    break;
                }
            }
        }
        match reached.is_empty() {
            true => self.neighbours(step, Some(here), owner),
            false => reached,
        }
    }

    /// The types of the records a step of a hop or more can meet: the far
    /// end of each relation its [`Reach`] walks, a hop at a time from `here`,
    /// or with none, as a neighbourhood, a path and a component walk it,
    /// wherever it is declared. A read at a valid time probes each.
    pub(super) fn met(
        &self,
        step: &Step,
        here: Option<&TypeName>,
        reach: &Reach,
    ) -> BTreeSet<TypeName> {
        let far = |t: &TypeName| reach.far.as_ref().is_none_or(|f| f.contains(t));
        let ends = |t: Option<&TypeName>| self.neighbours(step, t, reach.owner.as_ref());
        let Some(here) = here else {
            return ends(None).into_iter().filter(far).collect();
        };
        let (mut met, mut at) = (BTreeSet::new(), BTreeSet::from([here.clone()]));
        for _ in 0..step.depth.max.min(self.lake.limits.max_depth) {
            let next = at.iter().flat_map(|t| ends(Some(t))).filter(far);
            at = next.filter(|t| met.insert(t.clone())).collect();
        }
        met
    }

    /// The type a step reaches, one of `reached`: the one it names as its
    /// `type`, or the only one. A step over several types that must reach
    /// `one` names its `type`; a `type` it does not reach is refused,
    /// naming every type it does.
    fn neighbour_target(
        &self,
        at: &Place,
        step: &Step,
        reached: &BTreeSet<TypeName>,
        one: bool,
    ) -> Result<Option<TypeName>> {
        let Some(first) = reached.first() else {
            return Ok(None);
        };
        let said = match &step.ty {
            Some(t) if reached.contains(t) => return Ok(Some(t.clone())),
            Some(t) => {
                let also = (reached.len() > 1).then(|| reached.iter().skip(1).collect());
                let (reached, ty) = (first.clone(), t.clone());
                Case::StepReaches { reached, ty, also }
            }
            None if reached.len() < 2 => return Ok(Some(first.clone())),
            None if !one => return Ok(None),
            None => return Err(several_types(at, reached, None)),
        };
        Err(Error::invalid(Said::from(said).within(at.clone())))
    }

    /// The type a step over `relation` reaches from `here`, walking `walk`.
    fn step_target(
        &self,
        here: &TypeName,
        relation: &RelationName,
        walk: Walk,
        (lenient, owner): (bool, Option<&TypeName>),
    ) -> Result<Option<TypeName>> {
        self.shape.ontology.ty(here)?;
        let shown = self.named(Some(here), relation, owner);
        let out = (shown.iter())
            .find(|(t, _)| &t.name == here)
            .map(|(_, r)| r.target.clone());
        let inn = (shown.iter())
            .find(|(_, r)| &r.target == here)
            .map(|(t, _)| t.name.clone());
        let found = match walk {
            Walk::Out => out,
            Walk::In => inn,
            Walk::Both => out.or(inn),
        };
        match found {
            Some(t) => Ok(Some(t)),
            None if lenient && self.out_any(relation) => Ok(None),
            None => {
                let (ty, relation) = (here.clone(), relation.clone());
                Err(Error::invalid(Case::NoRelationToWalk {
                    ty,
                    relation,
                    walk,
                }))
            }
        }
    }

    /// A pattern's row as one type: each field of a bound name's type its
    /// reader may know of under `<name>.<field>`, and `<name>.key` where no
    /// field is named `key`. `_depth` and `_type` are left out: they are
    /// the lake's columns. A name over several types holds the fields they
    /// share alone.
    fn row_shape(&self, names: &[String], types: &[TypeName], open: &[Shared]) -> TypeDecl {
        let mut row = TypeDecl::default();
        for ((name, ty), open) in names.iter().zip(types).zip(open) {
            let Some(t) = self.shape.ontology.ty(ty).ok().filter(|_| !name.is_empty()) else {
                continue;
            };
            let at = |f: &str| FieldName::from(format!("{name}.{f}"));
            let lakes = |f: &FieldName| [DEPTH, TYPE].contains(&f.as_str());
            let held = |f: &FieldName| open.as_ref().is_none_or(|s| s.contains(f));
            let known = (t.fields.iter())
                .filter(|f| held(&f.name) && self.knows(t, &f.name) && !lakes(&f.name));
            let fields: Vec<_> = known
                .map(|f| lakelands_api::ontology::FieldDecl {
                    name: at(f.name.as_str()),
                    ..f.clone()
                })
                .collect();
            if !fields.iter().any(|f| f.name == at("key")) {
                row.fields.push(lakelands_api::ontology::FieldDecl {
                    name: at("key"),
                    ..Default::default()
                });
            }
            row.fields.extend(fields);
        }
        row
    }

    /// A record's values into a pattern's row under `<name>.<field>`, as
    /// [`Self::bound`] holds them.
    fn bind(
        &self,
        row: &mut BTreeMap<String, Value>,
        name: &str,
        at: (&graph::Node, u32, Option<&BTreeSet<FieldName>>),
        rec: Option<StoredRecord>,
    ) -> Result<()> {
        let bound = self.bound(at, rec)?;
        row.extend(bound.into_iter().map(|(f, v)| (format!("{name}.{f}"), v)));
        Ok(())
    }

    /// A record's values as a pattern's row holds them under its name, none
    /// where it may not be seen. `key` is the record's key, unless its type
    /// declares a field `key` its reader may know of: then that field's
    /// value, null where none. `_depth` is the hops its step took, 0 for
    /// the start, and `_type` its type, whatever a lake written before the
    /// rule holds in fields so named. A name over several types (`open`)
    /// holds the key, type, depth and the fields its types share.
    fn bound(
        &self,
        (node, depth, open): (&graph::Node, u32, Option<&BTreeSet<FieldName>>),
        rec: Option<StoredRecord>,
    ) -> Result<Vec<(String, Value)>> {
        let decl = self.shape.ontology.ty(&node.0)?;
        let Some(rec) = rec.and_then(|r| self.sight.visible(decl, &r, false)) else {
            return Ok(Vec::new());
        };
        let key = match open.is_some() || self.keyed(decl, &FieldName::from("key")) {
            true => Value::String(node.1.to_string()),
            false => Value::Null,
        };
        let held = |f: &FieldName| open.is_none_or(|s| s.contains(f));
        let fields = rec.fields.into_iter().filter(|(f, _)| held(f));
        let mut out = vec![("key".to_owned(), key)];
        out.extend(fields.map(|(f, v)| (f.to_string(), v)));
        out.push((DEPTH.to_owned(), Value::from(depth)));
        out.push((TYPE.to_owned(), Value::from(node.0.as_str())));
        Ok(out)
    }

    /// The records a graph reached as the principal may see them, each with
    /// its depth.
    fn graph_result(
        &self,
        nodes: Vec<graph::Reached>,
        edges: Vec<Edge>,
        next_cursor: Option<Cursor>,
    ) -> Result<QueryResult> {
        // Each type's records read in one ask, never one by one
        // (lakelands#1027).
        let all: Vec<graph::Node> = nodes.iter().map(|r| r.node.clone()).collect();
        let mut read: BTreeMap<graph::Node, StoredRecord> = BTreeMap::new();
        for (ty, keys) in graph::by_type(&all) {
            let rows = self.get_many(ty, &keys)?;
            read.extend(rows.into_iter().map(|r| ((r.ty.clone(), r.key.clone()), r)));
        }
        let (mut records, mut depths) = (Vec::new(), Vec::new());
        for r in &nodes {
            let Some(rec) = read.remove(&r.node) else {
                continue;
            };
            let decl = self.shape.ontology.ty(&r.node.0)?;
            if let Some(record) = self.sight.visible(decl, &rec, false) {
                records.push(record);
                depths.push(r.depth);
            }
        }
        Ok(QueryResult::Graph {
            nodes: records,
            depths,
            edges,
            truncated: next_cursor.is_some(),
            next_cursor,
        })
    }
}

/// A name bound before a step, the types it binds, and where its step
/// stands where those are several.
type Earlier<'a> = (&'a str, BTreeSet<TypeName>, Option<Place>);

/// Refuses, before a step is judged (080), what makes a pattern a tree
/// where its projection answers by a walk's own shape (`binds` false), and
/// what a join and an absent step take though they bind no name: an `as`,
/// and a join's `type` and `where`, which are judged where its name was
/// bound. A step with no `from` after one of them has no name to walk
/// from.
pub(super) fn tree(p: &Pattern, binds: bool) -> Result<()> {
    for (i, step) in p.steps.iter().enumerate() {
        let at = Place::Step {
            step: i + 1,
            relation: step.relation.clone(),
        };
        let refused = |case: Case| Err(Error::invalid(Said::from(case).within(at.clone())));
        let join = step.join.is_some();
        if !binds {
            let members = [
                ("join", join),
                ("optional", step.optional),
                ("absent", step.absent),
                ("from", p.start.ty.is_none() && step.from.is_some()),
            ];
            if let Some((member, _)) = members.iter().find(|(_, given)| *given) {
                let (member, projection) = ((*member).to_owned(), projection_name(&p.project));
                return refused(Case::PatternTreeMember { member, projection });
            }
            continue;
        }
        let takes = [
            ("as", step.named.is_some()),
            ("type", join && step.ty.is_some()),
            ("where", join && step.filter.is_some()),
        ];
        let taken = takes
            .iter()
            .find(|(_, given)| *given && (join || step.absent));
        if let Some((member, _)) = taken {
            let member = (*member).to_owned();
            return refused(Case::StepBindsNoName { member, join });
        }
        let before = i.checked_sub(1).map(|k| &p.steps[k]);
        if step.from.is_none() && before.is_some_and(|b| b.join.is_some() || b.absent) {
            return refused(Case::StepAfterNoName);
        }
    }
    Ok(())
}
