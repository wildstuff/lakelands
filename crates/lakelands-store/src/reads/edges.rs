//! The edge reads: the graph a query walks, seen as its principal is
//! answered it.

use super::*;

/// The edges of a derived relation around each of some records of one
/// type, one list per key, resolved from the view by identity of the
/// matched values: out to the target's records that hold the owner's
/// values, in from the owners that hold the target's. A relation side is
/// one scan, paged by [`MATCHED`] records. Nothing is stored
/// (`docs/decisions/044-a-relation-may-be-derived-from-matching-values.md`).
fn derived_edges(
    r: &Reader<'_>,
    ty: &TypeName,
    keys: &[RecordKey],
    (relation, shown): (Option<&RelationName>, &[(&TypeDecl, &RelationDecl)]),
    walk: Walk,
) -> Result<Vec<Vec<Edge>>> {
    // A relation the walker is not shown is never matched (006, extended).
    let wanted = |x: &RelationDecl| {
        (x.derived.is_some() && relation.is_none_or(|n| *n == x.name))
            && shown.iter().any(|(_, s)| std::ptr::eq(*s, x))
    };
    let owned: Vec<&RelationDecl> = match (walk, r.shape.ontology.ty(ty)) {
        (Walk::Out | Walk::Both, Ok(t)) => t.relations.iter().filter(|x| wanted(x)).collect(),
        _ => Vec::new(),
    };
    let targeted: Vec<(&TypeName, &RelationDecl)> = if walk == Walk::Out {
        Vec::new()
    } else {
        r.shape
            .ontology
            .types
            .values()
            .flat_map(|e| e.decl.relations.iter().map(move |x| (&e.decl.name, x)))
            .filter(|(_, x)| x.target == *ty && wanted(x))
            .collect()
    };
    let mut all = vec![Vec::new(); keys.len()];
    if owned.is_empty() && targeted.is_empty() {
        return Ok(all);
    }
    let nears: BTreeMap<RecordKey, StoredRecord> = r
        .get_many(ty, keys)?
        .into_iter()
        .map(|near| (near.key.clone(), near))
        .collect();
    // An owned relation leaves `near`; a targeted one reaches it. Each
    // has its condition read once; one that cannot be read connects nothing.
    let filter = |x: &RelationDecl| {
        let c = x.derived.as_ref().and_then(|d| d.filter.as_ref());
        c.map(|c| condition(c, Eval::Record, &[]).ok())
    };
    let leaving = owned.iter().map(|x| (&x.target, *x, true));
    let reaching = targeted.iter().map(|&(owner, x)| (owner, x, false));
    let walked: Vec<_> = (leaving.chain(reaching))
        .map(|(other, x, out)| (other, x, out, filter(x)))
        .filter(|w| !matches!(w.3, Some(None)))
        .collect();
    // The values a near record holds for one side of a match: none when
    // one is missing, for null never matches.
    let equal = |near: &StoredRecord, pairs: &[MatchPair], out: bool| {
        let pair = |p: &MatchPair| {
            let (here, there) = if out {
                (&p.from, &p.to)
            } else {
                (&p.to, &p.from)
            };
            let v = near.fields.get(here).filter(|v| !v.is_null())?;
            Some((there.clone(), v.clone()))
        };
        pairs
            .iter()
            .map(pair)
            .collect::<Option<Vec<(FieldName, Value)>>>()
    };
    // One scan a relation side for the whole batch (lakelands#971).
    for (other, x, out, filter) in &walked {
        let (other, x, out) = (*other, *x, *out);
        let pairs = &x.derived.as_ref().expect("derived").pairs;
        let tuples: Vec<Option<Vec<(FieldName, Value)>>> = keys
            .iter()
            .map(|k| nears.get(k).and_then(|n| equal(n, pairs, out)))
            .collect();
        // Tuples are the same exactly where their canonical bytes are.
        let id = |t: &Vec<(FieldName, Value)>| canonical::of(t);
        let asked: BTreeMap<Vec<u8>, &Vec<(FieldName, Value)>> =
            tuples.iter().flatten().map(|t| (id(t), t)).collect();
        let asked: Vec<Vec<(FieldName, Value)>> = asked.into_values().cloned().collect();
        // A page at a time, in key order: a tuple's matches may span pages,
        // and each start's edges still come in key order.
        let mut after = None;
        loop {
            let found;
            (found, after) = r.matching(other, &asked, after.take())?;
            let mut by: BTreeMap<Vec<u8>, Vec<&StoredRecord>> = BTreeMap::new();
            for m in &found {
                let held = pairs.iter().map(|p| {
                    let there = if out { &p.to } else { &p.from };
                    (
                        there.clone(),
                        m.fields.get(there).cloned().unwrap_or(Value::Null),
                    )
                });
                by.entry(id(&held.collect())).or_default().push(m);
            }
            for ((key, list), tuple) in keys.iter().zip(all.iter_mut()).zip(&tuples) {
                let (Some(near), Some(tuple)) = (nears.get(key), tuple) else {
                    continue;
                };
                for m in by.get(&id(tuple)).into_iter().flatten() {
                    let ends = match out {
                        true => [&near.fields, &m.fields],
                        false => [&m.fields, &near.fields],
                    };
                    let scope = Ends {
                        ends,
                        settings: r.told(),
                        now: r.now,
                    };
                    if let Some(Some(e)) = filter
                        && !expr::holds(e, &scope).unwrap_or(false)
                    {
                        continue;
                    }
                    let (this, that) = ((ty.clone(), key.clone()), (other.clone(), m.key.clone()));
                    let (from, to) = if out { (this, that) } else { (that, this) };
                    list.push(Edge {
                        from,
                        relation: x.name.clone(),
                        to,
                        fields: BTreeMap::new(),
                    });
                }
            }
            if after.is_none() {
                break;
            }
        }
    }
    Ok(all)
}

/// The two records of a derived relation's edge, as its `where` reads
/// them: `from.<field>` the owner's, `to.<field>` the target's, with the
/// lake's settings and the read's clock (044, extended).
pub(crate) struct Ends<'a> {
    pub ends: [&'a BTreeMap<FieldName, Value>; 2],
    pub settings: &'a BTreeMap<String, Value>,
    pub now: Timestamp,
}

impl expr::Scope for Ends<'_> {
    fn value(&self, path: &[FieldName]) -> Option<Value> {
        let (end, rest) = path.split_first()?;
        let i = [lakelands_api::key::FROM, lakelands_api::key::TO]
            .iter()
            .position(|e| end.as_str() == *e)?;
        expr::eval::lookup(self.ends[i], rest)
    }

    fn setting(&self, key: &str) -> Option<Value> {
        self.settings.get(key).cloned()
    }

    fn now(&self) -> Timestamp {
        self.now
    }
}

/// Whether the key `key` of `ty` may be named to the principal of `sees`:
/// it is shown the type and answered the record at the key in `view`,
/// retired or not, as `restricted` is judged, and a link with its ends,
/// as a read of it answers (lakelands#871). A key no record holds keeps
/// nothing from it. A read the engine fails is the caller's error, never
/// a hold (lakelands#927). One who holds `restricted` and no `only` is
/// answered every record of a type it is shown, a setting it may not be
/// told aside ([`Sight::told`]), and is asked no read.
pub(crate) fn answered(
    view: &dyn ProjectionView,
    sees: &Sight,
    ty: &TypeName,
    key: &RecordKey,
) -> Result<bool> {
    answered_of(view, sees, ty, &[key], 1)
}

/// Whether [`answered`] holds of `need` or more of `keys`, read a batch at
/// a time until enough are.
pub(crate) fn answered_of(
    view: &dyn ProjectionView,
    sees: &Sight,
    ty: &TypeName,
    keys: &[&RecordKey],
    need: usize,
) -> Result<bool> {
    let Ok(decl) = sees.ty(ty) else {
        return Ok(false);
    };
    if sees.sees_all() && !sees.narrows(ty) {
        return Ok(keys.len() >= need);
    }
    let mut found = 0;
    for page in keys.chunks(COUNT_PAGE) {
        let keys: Vec<RecordKey> = page.iter().map(|k| (*k).clone()).collect();
        let rows = view.get_many(ty, &keys)?;
        let ends = match rows.is_empty() {
            true => Vec::new(),
            false => {
                let fields: Vec<_> = rows.iter().map(|r| &r.fields).collect();
                sees.ends(decl, &fields, &mut |end, keys| view.get_many(end, keys))?
            }
        };
        let held = rows
            .iter()
            .zip(ends)
            .filter(|(r, ends)| !(*ends && sees.answers(ty, r) && sees.told(r).is_some()));
        found += page.len() - held.count();
        if found >= need {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The walks' view of the lake for one principal: a record is visible when
/// it is live, not restricted unless the principal may see restricted
/// records, valid at the read's `valid_at`, and of a type the principal
/// may see.
pub(super) struct Visible<'r, 'a> {
    pub(super) reader: &'r Reader<'a>,
    pub(super) reach: &'r [Reach],
    /// For rows, a reach and a walk a step's `where` reads, the records the
    /// walk has read, each read once for the walk, a step's `where` and a
    /// row (lakelands#940, lakelands#1027).
    pub(super) records: Option<RefCell<BTreeMap<graph::Node, StoredRecord>>>,
}

impl Visible<'_, '_> {
    pub(super) fn record(&self, node: &graph::Node) -> Result<Option<StoredRecord>> {
        if let Some(r) = self
            .records
            .as_ref()
            .and_then(|h| h.borrow().get(node).cloned())
        {
            return Ok(Some(r));
        }
        let r = self.reader.get_one(&node.0, &node.1)?;
        self.remember(r.clone());
        Ok(r)
    }

    pub(super) fn remember(&self, rows: impl IntoIterator<Item = StoredRecord>) {
        if let Some(held) = &self.records {
            let rows = rows.into_iter().map(|r| ((r.ty.clone(), r.key.clone()), r));
            held.borrow_mut().extend(rows);
        }
    }

    /// Lets the records of a page of starts go: no other page reads them.
    pub(super) fn forget(&self, nodes: &[graph::Node]) {
        if let Some(held) = &self.records {
            let mut held = held.borrow_mut();
            nodes.iter().for_each(|n| _ = held.remove(n));
        }
    }
}

/// What a step walks of the relations that bear its relation's name
/// (lakelands#829): a step naming an inverse walks the one relation that
/// declares it, its `owner`'s, and a step with a `type` only towards the
/// types its walk passes through, `far`. A relation a step does not walk
/// is neither probed nor read, so an answer holds what was probed and a
/// refusal names only what the walk would have read.
pub(super) struct Reach {
    pub(super) owner: Option<TypeName>,
    pub(super) far: Option<BTreeSet<TypeName>>,
}

impl Reach {
    /// Whether the step walks relation `r` of `owner` around `here`:
    /// out to its target, or in from its owner.
    pub(super) fn walks(
        &self,
        here: &TypeName,
        walk: Walk,
        owner: &TypeDecl,
        r: &RelationDecl,
    ) -> bool {
        let far = |t: &TypeName| self.far.as_ref().is_none_or(|f| f.contains(t));
        let out = owner.name == *here && walk != Walk::In && far(&r.target);
        let inn = r.target == *here && walk != Walk::Out && far(&owner.name);
        self.owner.as_ref().is_none_or(|o| *o == owner.name) && (out || inn)
    }
}

impl EdgeSource for Visible<'_, '_> {
    fn edges(
        &self,
        ty: &TypeName,
        keys: &[RecordKey],
        relation: Option<&RelationName>,
        walk: Walk,
        step: Option<usize>,
    ) -> Result<Vec<Vec<Edge>>> {
        let r = self.reader;
        const ALL: &Reach = &Reach {
            owner: None,
            far: None,
        };
        let reach = step.and_then(|i| self.reach.get(i)).unwrap_or(ALL);
        // A hop past the first reaches types the step's probe did not.
        r.probe_derived(ty, relation, walk, reach)?;
        let mut shown = r.shown_relations(Some(ty));
        shown.retain(|(t, x)| reach.walks(ty, walk, t, x));
        let mut lists = ProjectionView::edges(r.view.as_ref(), ty, keys, relation, walk)?;
        for (list, derived) in
            lists
                .iter_mut()
                .zip(derived_edges(r, ty, keys, (relation, &shown), walk)?)
        {
            list.extend(derived);
        }
        // An edge of a relation the principal is not shown, or the step
        // does not walk, answers nothing, and no link record is read for
        // it: a step over another type's relation of the same name walks
        // its own (024, extended).
        let read =
            |e: &Edge| (shown.iter()).any(|(t, x)| t.name == e.from.0 && x.name == e.relation);
        lists.iter_mut().for_each(|list| list.retain(read));
        // A link's edge comes from its own record: walked only with the
        // `restricted` grant if restricted, its fields as far as readable.
        let carried = |e: &Edge| {
            r.shape
                .ontology
                .ty(&e.from.0)
                .ok()
                .and_then(|d| d.relations.iter().find(|x| x.name == e.relation))
                .is_some_and(RelationDecl::carried_by_links)
        };
        let link_of = |e: &Edge| {
            (
                lakelands_core::links::link_type_name(&e.from.0, &e.relation),
                lakelands_core::links::link_key(&e.from.1, &e.to.1),
            )
        };
        let mut wanted: BTreeMap<TypeName, Vec<RecordKey>> = BTreeMap::new();
        for e in lists.iter().flatten().filter(|e| carried(e)) {
            let (lt, lk) = link_of(e);
            wanted.entry(lt).or_default().push(lk);
        }
        let mut links: BTreeMap<(TypeName, RecordKey), StoredRecord> = BTreeMap::new();
        for (lt, lks) in wanted {
            for link in r.view.get_many(&lt, &lks)? {
                links.insert((lt.clone(), link.key.clone()), link);
            }
        }
        let mut out = Vec::with_capacity(lists.len());
        for list in lists {
            let mut kept = Vec::with_capacity(list.len());
            for mut e in list {
                if carried(&e) {
                    let (lt, lk) = link_of(&e);
                    let Some(link) = links.get(&(lt.clone(), lk)) else {
                        continue;
                    };
                    // `only` on the link type admits it by its fields (051).
                    let decl = r.shape.ontology.ty(&lt);
                    let restricted = r.sight.withheld(link);
                    if decl.as_ref().map_or(restricted, |d| !r.live(d, link)) {
                        continue;
                    }
                    if let Ok(decl) = decl {
                        e.fields.retain(|name, _| {
                            decl.fields.iter().any(|f| {
                                &f.name == name && rights::field_access(r.who, decl, f).reads()
                            })
                        });
                    }
                }
                kept.push(e);
            }
            out.push(kept);
        }
        Ok(out)
    }

    fn visible(&self, nodes: &[graph::Node]) -> Result<Vec<bool>> {
        let r = self.reader;
        let mut shown: BTreeSet<graph::Node> = BTreeSet::new();
        for (ty, keys) in graph::by_type(nodes) {
            let Ok(decl) = r.sight.ty(ty) else {
                continue;
            };
            let rows = r.get_many(ty, &keys)?;
            self.remember(rows.iter().cloned());
            let (ends, admit) = (r.admitted_ends(decl, &rows)?, r.sight.admit(ty));
            for (rec, _) in rows.into_iter().zip(ends).filter(|(_, ends)| *ends) {
                if r.live_by(decl, &admit, &rec) {
                    shown.insert((rec.ty, rec.key));
                }
            }
        }
        Ok(nodes.iter().map(|n| shown.contains(n)).collect())
    }
}

/// Reciprocal rank fusion of ranked lists, each with its weight: a list
/// adds `weight / (60 + rank)` for every key it ranks (rank from 0, best
/// first), in the lists' order. Ties are broken by the key answered, the
/// one a record bears at the read's position, so the fused order is the
/// same on every engine and on every page (029). A search hands it only
/// what the read may see (046).
pub fn fuse(lists: &[(f64, Vec<(RecordKey, f64)>)]) -> Vec<(RecordKey, f64)> {
    const K: f64 = 60.0;
    let mut score: BTreeMap<RecordKey, f64> = BTreeMap::new();
    for (weight, list) in lists {
        for (rank, (key, _)) in list.iter().enumerate() {
            *score.entry(key.clone()).or_default() += weight / (K + rank as f64);
        }
    }
    let mut out: Vec<(RecordKey, f64)> = score.into_iter().collect();
    out.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    out
}
