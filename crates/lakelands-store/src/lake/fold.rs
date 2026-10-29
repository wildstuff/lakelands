//! The fold: each event changes the projection exactly as `Lake::fold`
//! says, with the rebuild and the vector index kept beside it.

use super::*;

/// What the fold's own decision of a set reads ([`crate::writes::Zone`]).
macro_rules! folding {
    ($w:expr, $view:expr, $projection:expr, $max:expr, $e:expr) => {
        crate::writes::Folding {
            ontology: &$w.ontology,
            settings: &$w.settings,
            view: $view,
            projection: $projection,
            max: $max,
            at: $e,
        }
    };
}

/// What the kept views' hooks are handed in the fold of one event.
macro_rules! kept_at {
    ($w:expr, $view:expr, $projection:expr) => {
        kept::At {
            view: $view,
            projection: Some($projection),
            ontology: &$w.ontology,
            settings: &$w.settings,
        }
    };
}

impl Lake {
    /// Commit the projection through `through`, timed for a meter.
    fn commit_at(
        &self,
        projection: &dyn lakelands_api::engine::ProjectionStore,
        through: Position,
    ) -> Result<()> {
        let began = self.timer();
        projection.commit(through)?;
        self.since(Spread::Commit, began);
        self.level(Level::FoldedThrough, through.0);
        Ok(())
    }

    /// Fold events into the projection: an append's in one transaction, a
    /// page `of_log` in one unless a recomputation was kept as a step.
    ///
    /// The steps a `declared` left pending are flushed before the first
    /// event that reads a record ([`reads_a_record`]) and before the commit.
    /// A fold of the log then ends its transaction when the flush kept a
    /// step: it commits through the last event folded and begins again, so
    /// the next walk reads those records in its view and no step is stacked
    /// on another. Each commit moves the engine's `folded_through`, and
    /// what it commits is whole: every event through that position, their
    /// recomputations put. An append is never split, and it stacks only if
    /// it holds a second flush, events that read records between two runs
    /// of declarations: that flush applies the first's kept steps to every
    /// row again. No door drafts such an append.
    ///
    /// An append hands out the calls its events make due; a fold of the
    /// log hands out only those of an append whose fold failed.
    pub(super) fn fold_all(
        &self,
        w: &mut Writer,
        events: &[Event],
        reads: Reads,
        of_log: bool,
    ) -> Result<()> {
        let Some(last) = events.last() else {
            return Ok(());
        };
        // A scoped refold makes no call due: its events are folded already.
        let scoped = self.scoped.scoped();
        let owed = (!of_log).then_some(Position::GENESIS).or(w.failed);
        let owed = owed.filter(|_| !scoped);
        let projection = self.store();
        let (start, mut due) = (w.owed.len(), w.owed.len());
        // A migration carries the entries in place: a fold that fails puts
        // them back as its last commit left them, for the fold that
        // catches up to carry once.
        let carries = events
            .iter()
            .any(|e| matches!(e.body, EventBody::Migrated { .. }));
        let mut kept = carries.then(|| w.owed.clone());
        let mut ciphers = crate::sealing::Ciphers::default();
        let mut staged = Staged {
            reads,
            keys: Some(self.keys.clone()),
            ..Staged::default()
        };
        let result = (|| {
            // Only a damaged log ends inside a set: an append holds its set
            // whole, and a page of the log never ends inside one.
            if open_set(events) {
                return Err(damaged(Part::Log, Some(last.position), Flaw::EndsInsideSet));
            }
            // A `begin` that is refused leaves the writer behind like any
            // other failure of the fold: the events are in the log.
            projection.begin()?;
            let mut view = self.scoped.view(self.engine.view(None)?);
            for (i, e) in events.iter().enumerate() {
                self.scoped
                    .reads_at(Position(e.position.0.saturating_sub(1)));
                let flushed = reads_a_record(&e.body)
                    && self.flush(w, view.as_ref(), &mut staged, &events[..i], owed)?;
                if reads_a_record(&e.body) {
                    self.decide_whole(w, view.as_ref(), &mut staged)?;
                }
                if flushed && of_log && !scoped {
                    drop(view);
                    self.commit_at(projection, events[i - 1].position)?;
                    // What is committed stays owed, whatever fails next.
                    due = w.owed.len();
                    if let Some(kept) = kept.as_mut() {
                        kept.clone_from(&w.owed);
                    }
                    projection.begin()?;
                    // No door drafts one inside a set; a foreign log may.
                    staged = Staged {
                        with_the_set: std::mem::take(&mut staged.with_the_set),
                        keys: Some(self.keys.clone()),
                        ..Staged::default()
                    };
                    view = self.scoped.view(self.engine.view(None)?);
                }
                let owes = owed.is_some_and(|p| e.position >= p);
                self.fold(w, (view.as_ref(), &mut ciphers), &mut staged, e, owes)?;
            }
            self.scoped.reads_at(last.position);
            self.flush(w, view.as_ref(), &mut staged, events, owed)?;
            self.decide_whole(w, view.as_ref(), &mut staged)
        })()
        .and_then(|()| self.commit_at(projection, last.position));
        match result {
            Ok(()) => {
                w.behind = false;
                let causes = w.owed[start..].iter().map(|o| o.cause);
                OWING.with(|o| o.borrow_mut().extend(causes));
                Ok(())
            }
            Err(e) => {
                // The projection is as its last commit left it; so is what
                // the writer read from it while folding.
                projection.rollback();
                w.behind = true;
                self.lose(w, &e);
                match kept {
                    Some(kept) => w.owed = kept,
                    None => w.owed.truncate(due),
                }
                if !of_log && !scoped {
                    w.failed = Some(events[0].position);
                }
                self.reload(w);
                Err(e)
            }
        }
    }

    /// The writer's ontology and settings read again at the head, where
    /// they can be.
    fn reload(&self, w: &mut Writer) {
        if let Ok(view) = self.engine.view(None)
            && let Ok(o) = Ontology::at(view.as_ref())
        {
            if let Ok(s) = settings_in(view.as_ref(), &o) {
                w.settings = s;
            }
            w.ontology = o.named();
        }
    }

    /// What a batch's append leaves where it stopped midway, from `first`
    /// on: the projection as its last commit left it, and the writer
    /// behind, as a failed fold leaves it. What it owed, its lease and
    /// whether it lost that stay: a batched write makes no call due.
    pub(crate) fn stopped_append(&self, w: &mut Writer, first: Option<Position>) {
        self.engine.projection().rollback();
        (w.behind, w.staged) = (true, None);
        w.failed = first.or(w.failed);
        self.reload(w);
    }

    /// Fold one `migrated` event's step over every record it reaches: the
    /// migrated type's, for a re-key those of every type with moves and of
    /// every type that references one, and for a renamed relation carried
    /// by link records those of its link type, which move to the link type
    /// of its new name and leave their old keys ended. Each type is walked
    /// once, under the name the view knows it by, a page
    /// of the view at a time, and nothing of the walk is kept: a record
    /// this transaction wrote is stepped where it is kept, and a later read
    /// of any other goes through the step ([`Staged::record`]). An old key
    /// that is no one's new key ends: it reads as not found from here on,
    /// and a later record at it starts at version 1. Every listed move is
    /// applied, or the fold fails.
    fn step(
        &self,
        w: &Writer,
        view: &dyn ProjectionView,
        projection: &dyn lakelands_api::engine::ProjectionStore,
        staged: &mut Staged,
        step: Migrated,
    ) -> Result<()> {
        let migrated = &step.migration.to.name;
        let moved = step.decls.keys().chain(step.types.keys());
        let reached: BTreeSet<&TypeName> = moved.chain([migrated]).collect();
        // The old keys whose records moved.
        let mut applied = BTreeSet::new();
        let mut put = |pre: &StoredRecord, post: &StoredRecord| -> Result<()> {
            if post.ty == *migrated || post != pre {
                projection.put_record(post)?;
            }
            // A record of the migrated type, and every record whose key the
            // step moves, a link of a re-keyed end among them, takes its
            // vectors along; any other type's declaration is as it was.
            let moved = post.key != pre.key;
            let before = match pre.ty == *migrated {
                true => Some(&step.before),
                false => w.ontology.ty(&pre.ty).ok().filter(|_| moved),
            };
            if let Some(before) = before.filter(|_| post.ty == pre.ty) {
                let left = moved && !step.arrives(&pre.ty, &pre.key);
                self.reindex(&w.ontology, before, pre, post, left)?;
            }
            if post.ty != pre.ty {
                // A link type's records moved to another: its keys end.
                let ended = StoredRecord::ended(pre.ty.clone(), pre.key.clone(), step.at);
                projection.put_record(&ended)?;
                self.index_vectors(&w.ontology, &ended)?;
                self.index_vectors_as(&w.ontology, &pre.ty, post)?;
            }
            if post.key != pre.key {
                if !step.arrives(&pre.ty, &pre.key) {
                    let ended = StoredRecord::ended(pre.ty.clone(), pre.key.clone(), step.at);
                    projection.put_record(&ended)?;
                }
                applied.insert((pre.ty.clone(), pre.key.clone()));
            }
            Ok(())
        };
        for ty in &reached {
            // The view holds a record under the type it had before the
            // transaction: an earlier step of it may have moved the type.
            let ty = staged.known(ty);
            walk(view, ty, true, |row| {
                // A record this transaction wrote is its own, under the
                // key the earlier steps left it: tested before anything
                // of the view's row is converted.
                if staged.wrote(ty, &row.key) {
                    return Ok(());
                }
                let pre = staged.stepped(view, row)?;
                put(&pre, &step.apply(pre.clone())?)
            })?;
        }
        // What the transaction wrote is stepped where it is kept: all are
        // taken out before any is put back, a new key may be another's old.
        let (stepped, kept) = std::mem::take(&mut staged.records)
            .into_iter()
            .partition::<Vec<_>, _>(|((ty, _), _)| reached.contains(ty));
        staged.records = kept.into_iter().collect();
        for (_, pre) in stepped {
            let post = step.apply(pre.clone())?;
            put(&pre, &post)?;
            staged.put(post);
        }
        let listed = (step.moves.iter()).flat_map(|(ty, m)| m.keys().map(move |from| (ty, from)));
        for (ty, from) in listed {
            if !applied.contains(&(ty.clone(), from.clone())) {
                let (ty, key) = (ty.clone(), from.clone());
                return Err(damaged(
                    Part::Log,
                    Some(step.at),
                    Flaw::MovesUnheld { ty, key },
                ));
            }
        }
        // Object-set membership moves with the record: the new key is in
        // every set the old key was in, and an ended key is in none. Every
        // membership is read before any is put.
        let mut members = Vec::new();
        for s in w.ontology.object_sets.values().map(|s| &s.decl) {
            let Some(moves) = step.moves.get(&s.of) else {
                continue;
            };
            // Read once for the set, not looked up for each moved record; a
            // membership this page put is not in the view yet.
            let held = view.members(&s.name, &s.of)?;
            for (from, to) in moves {
                let put = staged
                    .members
                    .get(&(s.name.clone(), s.of.clone(), from.clone()));
                let member = put.copied().unwrap_or_else(|| held.contains(from));
                members.push((&s.name, &s.of, from, to, member));
            }
        }
        for (set, ty, from, to, member) in members {
            staged.member(projection, set, ty, to, member, step.at)?;
            if member && !step.arrives(ty, from) {
                staged.member(projection, set, ty, from, false, step.at)?;
            }
        }
        // A set of a link type whose records moved holds them under the
        // type they moved to: read under the type the view knows them by,
        // with what the transaction put laid over.
        for s in w.ontology.object_sets.values().map(|s| &s.decl) {
            let Some(to) = step.types.get(&s.of) else {
                continue;
            };
            let keys = staged.members(view, &s.name, (staged.known(&s.of), &s.of))?;
            for key in keys {
                staged.member(projection, &s.name, to, &key, true, step.at)?;
                staged.member(projection, &s.name, &s.of, &key, false, step.at)?;
            }
        }
        staged.steps.push(Step::Migrated(Box::new(step)));
        Ok(())
    }

    /// Fold one `value_type_migrated` event's step over every record of
    /// the types it renames in, retired and restricted ones included, as
    /// [`Self::step`] folds a migration's: each type walked once, a page
    /// of the view at a time, nothing of the walk kept, a record this
    /// transaction wrote stepped where it is kept, and a later read of any
    /// other taken through the step. Only a record the step changes is
    /// put; no key moves and no type.
    fn step_values(
        &self,
        view: &dyn ProjectionView,
        projection: &dyn lakelands_api::engine::ProjectionStore,
        staged: &mut Staged,
        step: Values,
    ) -> Result<()> {
        for ty in step.fields.keys() {
            walk(view, staged.known(ty), true, |row| {
                if staged.wrote(ty, &row.key) {
                    return Ok(());
                }
                let pre = staged.stepped(view, row)?;
                let post = step.apply(pre.clone());
                match post != pre {
                    true => projection.put_record(&post),
                    false => Ok(()),
                }
            })?;
            let written = (staged.records)
                .range_mut((ty.clone(), RecordKey::from(""))..)
                .take_while(|((t, _), _)| t == ty);
            for (_, r) in written {
                let post = step.apply(r.clone());
                if post != *r {
                    projection.put_record(&post)?;
                    *r = post;
                }
            }
        }
        staged.steps.push(Step::Values(Box::new(step)));
        Ok(())
    }

    /// Compute derived fields again for the `declared` events that are
    /// pending, each type walked once however many of them recompute it:
    /// every live record of the view the transaction did not write, as the
    /// kept steps leave it, and every one it wrote, where it is kept, goes
    /// through the pending steps in order. Each computes with its own
    /// entries and settings and the outputs recorded before its own event,
    /// and a record is put, and its vectors indexed, at each step that
    /// changes it: one state a step, as if each had walked at its event.
    /// A set of declarations is one step, at its last (`with_the_set`).
    /// Nothing of the view is kept, and what the transaction wrote stays
    /// kept, changed or not. A step whose event is at or past `owed`
    /// leaves a sweep owed of each type of whose records it made a call
    /// due, by step and type. A step that put nothing is dropped: a later read
    /// through it would change nothing either. Answers whether a step is
    /// kept.
    fn flush(
        &self,
        w: &mut Writer,
        view: &dyn ProjectionView,
        staged: &mut Staged,
        folded: &[Event],
        owed: Option<Position>,
    ) -> Result<bool> {
        let (false, Some(last)) = (staged.pending.is_empty(), folded.last()) else {
            return Ok(false);
        };
        let pending = std::mem::take(&mut staged.pending);
        let projection = self.store();
        let mut put = vec![false; pending.len()];
        let mut owing: BTreeSet<(usize, TypeName)> = BTreeSet::new();
        let mut through = |r: &mut StoredRecord, recorded: &Recorded| {
            for (i, (s, put)) in pending.iter().zip(&mut put).enumerate() {
                let owes = owed.is_some_and(|p| s.event.position >= p);
                let mut due: Vec<Due> = Vec::new();
                if let Some(next) = s.apply(r, view, recorded, owes.then_some(&mut due))? {
                    projection.put_record(&next)?;
                    self.index_vectors(&s.entries, &next)?;
                    (*r, *put) = (next, true);
                }
                if !due.is_empty() {
                    owing.insert((i, r.ty.clone()));
                }
            }
            Ok(())
        };
        let types = pending.iter().flat_map(|s| s.entries.types.keys());
        let types: BTreeSet<&TypeName> = types.collect();
        // The view holds a record under the type it had before the
        // transaction, and a kept step may have moved a type's records: so
        // each of the view's types is walked once, and a step computes a
        // row by the type it has now.
        let known: BTreeSet<&TypeName> = types.iter().map(|ty| staged.known(ty)).collect();
        for ty in known {
            walk(view, ty, false, |row| {
                // Tested before anything of the row is computed, by the
                // type and key the kept steps give it.
                if staged.wrote(ty, &row.key) {
                    return Ok(());
                }
                let mut r = staged.stepped(view, row)?;
                through(&mut r, &staged.outputs)
            })?;
        }
        for ty in types {
            let written = (staged.records)
                .range_mut((ty.clone(), RecordKey::from(""))..)
                .take_while(|((t, _), _)| t == ty);
            for (_, r) in written {
                through(r, &staged.outputs)?;
            }
        }
        for (i, ty) in owing {
            let event = &pending[i].event;
            let renames = matches!(event.body, EventBody::ValueTypeMigrated { .. });
            let sweep = Of::Sweep {
                after: None,
                retried: (!renames).then(BTreeSet::new),
                waiting: false,
            };
            w.owed.push(Owed::new(event, ty, sweep));
        }
        let kept = put.contains(&true);
        let steps = pending.into_iter().zip(put);
        staged
            .steps
            .extend(steps.filter_map(|(s, put)| put.then(|| Step::Recomputed(Box::new(s)))));
        let viewing = w.zone.as_ref().is_some_and(crate::writes::Zone::viewing);
        if viewing {
            kept::moved(staged);
        }
        kept::made(&kept_at!(w, view, projection), staged, last.position)?;
        if let Some(zone) = w.zone.as_ref().filter(|_| viewing) {
            let at = (last.position, last.known_at);
            let f = folding!(w, view, projection, self.limits.max_depth, at);
            let moved = kept::moved(staged);
            zone.groups(f, staged, &moved)?;
        }
        Ok(kept)
    }

    /// The sets the fold decides itself that a declaration, a migration or
    /// a setting moved, decided over all their records, after the flush
    /// computed again what those made due ([`crate::writes::Zone`]).
    fn decide_whole(
        &self,
        w: &mut Writer,
        view: &dyn ProjectionView,
        staged: &mut Staged,
    ) -> Result<()> {
        let projection = self.store();
        let max = self.limits.max_depth;
        let Some(zone) = w.zone.as_mut() else {
            return Ok(());
        };
        let at = (Position::GENESIS, Timestamp(0));
        let f = folding!(w, view, projection, max, at);
        zone.decide_whole(f, staged)
    }

    /// Fold the log from `from` on, a page at a time, each page in its own
    /// transaction that commits `folded_through` at its last event, or in
    /// several, each committing at the last event it folded
    /// ([`Self::fold_all`]). A fold stops in one of two ways: reading a
    /// page of the log fails, or folding it does, and the open transaction
    /// rolls back. Either way the projection is folded through the last
    /// commit, the writer is `behind`, and the head names that position;
    /// nothing is appended until the next fold (the catch-up before a
    /// write, a `rebuild`, or the next open's) has continued from there.
    /// Every read is answered at or before `folded_through`, so no read
    /// sees half a transaction. A fold that reaches the tip has handed out
    /// the calls of an append whose fold had failed.
    pub(super) fn fold_from(&self, w: &mut Writer, from: Position) -> Result<()> {
        // The sets it decides itself, from the first value it erases that
        // each reads, or from the first event where it begins midway in a
        // lake with an act (lakelands#1232).
        let midway = from != Position::GENESIS && self.acts_known(w)?;
        w.zone = Some(crate::writes::Zone::new(&w.ontology, midway));
        let folded = (|| {
            for page in self.pages(from) {
                self.fold_all(w, &page?, Reads::default(), true)?;
            }
            Ok(())
        })();
        w.zone = None;
        if folded.is_err() {
            // Whatever stopped it, a read of the log or a page's fold, the
            // projection is behind the log: nothing is appended until a
            // fold has caught up, and the head names the last commit, as
            // `head()` promises.
            w.behind = true;
            let through = self.engine.projection().folded_through().ok().flatten();
            self.set_head(through.map_or(0, |p| p.0));
        } else {
            w.failed = None;
        }
        folded
    }

    /// Whether the lake holds an act that erased values.
    pub(super) fn acts_known(&self, w: &Writer) -> Result<bool> {
        let view = self.engine.view(None)?;
        let none = Staged::default();
        Ok(!w.forgotten.is_empty()
            || !w.removals.is_empty()
            || !forgotten_list(view.as_ref(), &none)?.is_empty()
            || !crate::remove::removed_list(view.as_ref(), &none)?.is_empty())
    }

    /// The log's tip read again, which a write that stopped midway may
    /// have left the writer's off, and how far the projection is folded;
    /// a failed append that never landed is owed nothing.
    fn retip(&self, w: &mut Writer) -> Result<Option<Position>> {
        let folded;
        (folded, w.tip) = ends(self.engine.as_ref())?;
        if w.failed > w.tip.as_ref().map(|t| t.0) {
            w.failed = None;
        }
        Ok(folded)
    }

    /// Fold the events the log holds past the projection, a page at a
    /// time, and publish the head once they are folded.
    ///
    /// The writer's ontology and settings are read at the mark first: what
    /// it holds may be what a failed fold left, and a fold compares a
    /// declaration with the one the writer holds. A writer that cannot
    /// read them stays behind.
    pub(super) fn refold(&self, w: &mut Writer) -> Result<()> {
        let folded = self.retip(w)?;
        {
            let view = self.engine.view(None)?;
            w.ontology = Ontology::at(view.as_ref())?.named();
            w.settings = settings_in(view.as_ref(), &w.ontology)?;
        }
        let from = folded.map_or(Position::GENESIS, Position::next);
        self.fold_from(w, from)?;
        w.behind = false;
        self.publish(w);
        Ok(())
    }

    /// What one event does to the projection, opened with the ciphers its
    /// pass has built; the calls it makes due are handed out when it
    /// `owes` them.
    fn fold(
        &self,
        w: &mut Writer,
        (view, ciphers): (&dyn ProjectionView, &mut crate::sealing::Ciphers),
        staged: &mut Staged,
        e: &Event,
        owes: bool,
    ) -> Result<()> {
        // A quarantined record's cells are opened, erased and selected as a
        // landed value is (085).
        let lifted = crate::sealing::lifted(e);
        let e = lifted.as_ref().unwrap_or(e);
        let opened;
        // What a forgotten source sealed is erased: its key is destroyed,
        // and the log says it was forgotten. A key gone without that is
        // lost, and fails closed (058).
        let (mut erased, mut dropped) = (Vec::new(), false);
        let source = (e.sealed.as_ref()).and_then(|s| s.source.as_ref().or(e.source.as_ref()));
        // The index keeps the source as the log names it, before an erasure
        // takes the seal away (`indexed`).
        let indexed = indexed(e);
        // A value sealed apart and erased by its apart key alone still
        // tells the version it sealed with, whose first use it may be: the
        // list of versions stays the one the write kept (065).
        let mut kept_seal = e.sealed.clone();
        let e = match (&e.sealed, source) {
            (Some(sealed), Some(source)) => {
                let mut plain = e.clone();
                // With no key store at all, as an import made without one,
                // a value the log forgot is erased, and any other refused
                // as unbound rather than lost.
                // A seal of a delivery sealed apart needs its version and
                // its apart keys: it is erased where an act destroyed any
                // of those missing, and fails closed on the first (065).
                let (cipher, missing) = match ciphers.opened(&self.keys, source, sealed) {
                    Ok(Ok(cipher)) => (Ok(cipher), Vec::new()),
                    Ok(Err(missing)) => {
                        let (source, key) = (source.clone(), missing[0].clone());
                        let lost = Error::unavailable(Situation::KeyMissing { source, key });
                        (Err(lost), missing)
                    }
                    Err(e @ Error::Unbound { .. }) => {
                        let all = std::iter::once(&sealed.key).chain(&sealed.apart);
                        (Err(e), all.cloned().collect())
                    }
                    Err(e) => return Err(e),
                };
                match cipher {
                    Ok(cipher) => crate::sealing::open(&mut plain, cipher)?,
                    Err(_)
                        if self.erased_any(
                            w,
                            (view, staged),
                            source,
                            (&missing, e.position),
                        )? =>
                    {
                        if missing.contains(&sealed.key) {
                            kept_seal = None;
                        }
                        dropped = matches!(plain.body, EventBody::OutputRecorded { .. });
                        erased = crate::sealing::erase(&mut plain);
                    }
                    Err(lost) => return Err(lost),
                }
                opened = plain;
                &opened
            }
            _ => e,
        };
        // What an act of removal selects is erased too, sealed or not (065).
        let taken;
        let e = match crate::remove::redacted(&w.removals, &w.ontology, e) {
            Some((plain, cleared)) => {
                erased.extend(cleared);
                taken = plain;
                &taken
            }
            None => e,
        };
        let projection = self.store();
        // A set that reads what the fold erased is decided by the fold
        // from here on (lakelands#1232).
        if let Some(zone) = &mut w.zone {
            let erased = match &e.body {
                EventBody::RecordIngested { ty, .. } => Some((ty, erased.clone())),
                EventBody::OutputRecorded {
                    field: Some((ty, _, name)),
                    ..
                } if dropped => Some((ty, vec![name.clone()])),
                _ => None,
            };
            if let Some((ty, fields)) = erased.filter(|(_, f)| !f.is_empty()) {
                zone.erased(&w.ontology, ty, &fields);
            }
            if zone.viewing() {
                kept::moved(staged);
            }
        }
        // The versions of each source's key the log sealed with, each with
        // the position of its first use ([`crate::sealing::USED`]). The
        // apart keys of each event a delivery sealed apart are its own, in
        // the log ([`crate::sealing::Apart`]).
        if let (
            EventBody::RecordIngested { .. } | EventBody::RecordQuarantined { .. },
            Some(sealed),
            Some(source),
        ) = (&e.body, &kept_seal, &e.source)
        {
            let used = (source, sealed, e.position, e.known_at);
            let read = (&staged.outputs, view, &*self.keys);
            if let Some((at, list)) = crate::sealing::kept(read, used)? {
                projection.put_output(&at.0, &at.1, &list, e.position)?;
                staged
                    .outputs
                    .entry(at)
                    .or_default()
                    .push((e.position, list));
            }
        }
        // A name of the kept form, and a seam that begins `sys:`, are the
        // store's own, and a view's records are computed: no door declares,
        // writes or records under the one, nor writes the other, so only a
        // log another build wrote names them, and such a log is not folded.
        if let Some(flaw) = foreign(&w.ontology, &e.body) {
            return Err(damaged(Part::Log, Some(e.position), flaw));
        }
        projection.mark_applied(&lakelands_api::engine::Applied {
            id: e.id.clone(),
            at: e.position,
            kind: e.body.kind(),
            about: lakelands_core::feed::about(&e.body),
            source: indexed,
        })?;
        match &e.body {
            EventBody::LakeCreated { .. }
            | EventBody::LakeUpgraded { .. }
            | EventBody::LeaseTaken { .. }
            | EventBody::Checkpoint { .. }
            | EventBody::VerbCalled { .. }
            | EventBody::TriggerFired { .. }
            | EventBody::TriggerDropped { .. } => {}
            // A version is its id's row, what is erased of it gone (085).
            EventBody::RecordQuarantined { .. } => {
                if let Some(q) = crate::quarantine::version_row(&w.ontology, e) {
                    projection.put_quarantined(&q, e.position)?;
                    staged.quarantined.insert(q.id.clone(), q);
                }
            }
            // A close keeps no cell; a close of no row is the log's own,
            // and folds to its mark alone.
            EventBody::QuarantineClosed { id, why, .. } => {
                if let Some(q) = crate::quarantine::row(view, &staged.quarantined, id)? {
                    let close = lakelands_api::quarantine::QuarantineClose {
                        why: *why,
                        position: e.position,
                    };
                    let q = crate::quarantine::closed(q, close);
                    projection.put_quarantined(&q, e.position)?;
                    staged.quarantined.insert(q.id.clone(), q);
                }
            }
            // The act is owed until a rebuild after its keys were
            // destroyed has erased what they sealed, and the reclaim ran.
            EventBody::SourceForgotten { source, .. } => {
                self.act_at(e.position, e.known_at);
                w.forgotten.insert(source.clone());
                let erased = w.rebuilding && self.keys.ids(source)?.is_empty();
                let mut list = forgotten_list(view, staged)?;
                let state = if erased { "rebuilt" } else { "owed" };
                list.insert(source.clone(), state.to_owned());
                let (seam, at) = (crate::sealing::FORGOTTEN, crate::sealing::forgotten_at());
                let list = serde_json::to_value(&list).expect("a list serialises");
                projection.put_output(seam, &at, &list, e.position)?;
                staged
                    .outputs
                    .entry((seam.to_owned(), at))
                    .or_default()
                    .push((e.position, list));
            }
            // The act is owed until a rebuild that knew it before it began
            // has folded the log, and the reclaim ran (065).
            EventBody::ValuesRemoved { .. } => {
                self.act_at(e.position, e.known_at);
                let act = crate::remove::Removal::of(e).expect("the event is an act");
                let known = w.removals.contains(&act) && !w.ahead.remove(&act.at);
                // Its versions destroyed, as forgetting's keys must be: a
                // rebuild before the destroy leaves it owed.
                let ids = match act.keys.is_empty() {
                    true => Vec::new(),
                    false => self.keys.ids(&act.source)?,
                };
                let mut destroyed = !act.keys.iter().any(|k| ids.contains(k));
                for k in &act.apart {
                    destroyed = destroyed && !self.keys.holds(&act.source, k)?;
                }
                // One met untold fixes what identifies records here, as a
                // rebuild would be told it (lakelands#1228).
                if !w.removals.contains(&act) {
                    let key = (RENAMES.to_owned(), Digest::zero());
                    let renames = match staged.outputs.get(&key).and_then(|l| l.last()) {
                        Some((_, list)) => renames_in(Some(list.clone()))?,
                        None => renames_in(view.output(RENAMES, &key.1)?.map(|(_, l)| l))?,
                    };
                    let fixed = crate::remove::Fixed::of(&w.ontology, &renames, act.at);
                    w.removals.push(crate::remove::Removal { fixed, ..act });
                }
                let mut list = crate::remove::removed_list(view, staged)?;
                let state = if known && w.rebuilding && destroyed {
                    "rebuilt"
                } else {
                    "owed"
                };
                list.insert(e.position, state.to_owned());
                let (seam, at) = (crate::remove::REMOVED, crate::remove::removed_at());
                let list = crate::remove::list_value(&list);
                projection.put_output(seam, &at, &list, e.position)?;
                staged
                    .outputs
                    .entry((seam.to_owned(), at))
                    .or_default()
                    .push((e.position, list));
            }
            EventBody::Declared {
                declaration,
                schema_hash,
                version,
                with_the_set,
            } => {
                let rederive = computes_again(&w.ontology, declaration);
                // A set withdrawn holds no member from here on, with no
                // event for each: a read as of before still holds them.
                if let Declaration::Withdraw(s) = declaration
                    && s.kind == DeclarationKind::ObjectSet
                    && let Some(set) = w.ontology.set(&s.name)
                {
                    let (name, of) = (set.name.clone(), set.of.clone());
                    for key in staged.members(view, &name, (&of, &of))? {
                        staged.member(projection, &name, &of, &key, false, e.position)?;
                    }
                }
                w.ontology.put_at(
                    declaration.clone(),
                    *version,
                    schema_hash.clone(),
                    e.position,
                );
                projection.put_declaration(
                    declaration,
                    *version,
                    schema_hash.as_deref(),
                    e.position,
                )?;
                // A declaration of a set that goes on computes nothing yet:
                // what it would compute again is computed at the set's last
                // declaration, once, as that event leaves the ontology.
                staged.with_the_set.extend(rederive);
                let rederive = match with_the_set {
                    true => Vec::new(),
                    false => Vec::from_iter(std::mem::take(&mut staged.with_the_set)),
                };
                // A redeclared function, or revised derived expressions,
                // compute every live record's derived fields again, as of
                // this event: what that reads is kept as it stands now, and
                // the records are walked at the flush.
                staged.rederive(w, e, &rederive);
            }
            EventBody::Migrated {
                migration,
                schema_hash,
                version,
                rekeyed,
                ..
            } => {
                let before = w.ontology.ty(&migration.to.name)?.clone();
                let decl = Declaration::Type(migration.to.clone());
                // From format 15 on (lakelands#1070): a log of format 14
                // folds as it was written, and a rebuild answers as its lake did.
                let again = e.v >= 15 && computing_changed(&before, &migration.to);
                w.ontology.put_at(
                    decl.clone(),
                    *version,
                    Some(schema_hash.clone()),
                    e.position,
                );
                projection.put_declaration(&decl, *version, Some(schema_hash), e.position)?;
                // The lake's list of renames takes the fields this migration
                // renames, and each link type a renamed relation moves.
                let renamed = lakelands_core::links::renamed(migration, e.position);
                if !renamed.is_empty() {
                    let key = (RENAMES.to_owned(), Digest::zero());
                    let mut list = match staged.outputs.get(&key).and_then(|l| l.last()) {
                        Some((_, list)) => renames_in(Some(list.clone()))?,
                        None => renames_in(view.output(RENAMES, &key.1)?.map(|(_, l)| l))?,
                    };
                    list.extend(renamed);
                    let list = serde_json::json!(list);
                    projection.put_output(RENAMES, &key.1, &list, e.position)?;
                    // No step reads the renames: the last list is all that
                    // is kept of them.
                    staged.outputs.insert(key, vec![(e.position, list)]);
                }
                let step = Migrated::new(&w.ontology, e.position, before, migration, rekeyed)
                    .filling(&w.ontology, e);
                // A quarantined record's cells and type are named as the
                // head names them (085).
                for q in crate::quarantine::renamed(view, staged, migration, &step.types)? {
                    projection.put_quarantined(&q, e.position)?;
                    staged.quarantined.insert(q.id.clone(), q);
                }
                step.carry(&mut w.owed);
                self.step(w, view, projection, staged, step)?;
                // What reads a filled field is computed again, as of this
                // event, as a create computes it (050): queued as a value's
                // rename queues it.
                // So is a type whose computed fields it changes (lakelands#1070).
                let filled =
                    !migration.fill.is_empty() && computes(&w.ontology, &migration.to.name);
                if filled || again {
                    staged.rederive(w, e, std::slice::from_ref(&migration.to.name));
                }
            }
            EventBody::ValueTypeMigrated {
                declaration,
                version,
                renames,
                types,
                ..
            } => {
                // Only an imported or damaged log names other types than
                // those that use the value type: the writer named them.
                if using_types(&w.ontology, &declaration.name) != *types {
                    let value_type = declaration.name.to_string();
                    let flaw = Flaw::RenamesOtherTypes { value_type };
                    return Err(damaged(Part::Log, Some(e.position), flaw));
                }
                let decl = Declaration::ValueType(declaration.clone());
                w.ontology.put_at(decl.clone(), *version, None, e.position);
                projection.put_declaration(&decl, *version, None, e.position)?;
                let step = Values::new(&w.ontology, e.position, &declaration.name, renames);
                // What reads a renamed value is computed again, as of this
                // event, at the flush: a derived field, one computed by a
                // function, an embedding, whose calls are owed with this
                // event as their cause. Queued here, so that a kept view
                // over such a type is ended at this event.
                let rederive: Vec<TypeName> = (types.iter())
                    .filter(|t| computes(&w.ontology, t))
                    .cloned()
                    .collect();
                self.step_values(view, projection, staged, step)?;
                staged.rederive(w, e, &rederive);
            }
            EventBody::RecordUpserted { ty, key, .. }
            | EventBody::RecordIngested { ty, key, .. }
            | EventBody::RecordRetired { ty, key, .. }
            | EventBody::RecordRestricted { ty, key, .. }
            | EventBody::RecordUnrestricted { ty, key, .. } => {
                let before = staged.record(view, ty, key)?;
                let mut due = Vec::new();
                let mut outputs = Outputs {
                    view,
                    staged: Some(&staged.outputs),
                    before: None,
                    due: owes.then_some(&mut due),
                    waiting: false,
                    plans: &w.plans,
                    keys: Some(&self.keys),
                    apart: Some(&staged.apart),
                };
                let after =
                    record::apply(&w.ontology, before.as_ref(), e, &w.settings, &mut outputs)?;
                if !due.is_empty() {
                    w.owed
                        .push(Owed::new(e, ty.clone(), Of::Record(key.clone())));
                }
                if let Some(mut after) = after {
                    for f in &erased {
                        if let Some(m) = after.meta.get_mut(f) {
                            m.erased = true;
                        }
                    }
                    if ty.as_str() == SETTING_TYPE {
                        let fields = (!after.retired).then_some(&after.fields);
                        lakelands_api::settings::hold(&mut w.settings, key.as_str(), fields);
                    }
                    projection.put_record(&after)?;
                    self.index_vectors(&w.ontology, &after)?;
                    let before = before.as_ref();
                    kept::note(&kept_at!(w, view, projection), staged, before, &after)?;
                    staged.put(after);
                    self.decided(w, view, staged, e, (before, ty, key))?;
                }
            }
            EventBody::WriteProposed {
                proposal,
                write,
                why,
                ..
            } => {
                let p = Proposal {
                    id: proposal.clone(),
                    position: e.position,
                    proposer: e.writer.clone(),
                    proposed_at: e.known_at,
                    write: Some(write.clone()),
                    why: why.clone(),
                    state: ProposalState::Pending,
                    decided_by: None,
                    reason: None,
                    more: false,
                };
                projection.put_proposal(&p, e.position)?;
                staged.proposals.insert(proposal.clone(), p);
            }
            EventBody::WriteAccepted { proposal } | EventBody::WriteRejected { proposal, .. } => {
                let mut p = match staged.proposals.get(proposal) {
                    Some(p) => p.clone(),
                    None => find_proposal(view, proposal)?,
                };
                p.decided_by = Some(e.writer.clone());
                match &e.body {
                    EventBody::WriteRejected { reason, .. } => {
                        p.state = ProposalState::Rejected;
                        p.reason = Some(reason.clone());
                    }
                    _ => p.state = ProposalState::Accepted,
                }
                // A decision on a proposed call is carried out after the
                // append, as the event that set it off made it due (064).
                // A closure by the system, caused by the output that decided
                // the call or by its record's retirement, leaves nothing owed.
                if owes
                    && e.origin != lakelands_api::record::Origin::System
                    && let Some(
                        write
                        @ (ProposedWrite::Compute { .. } | ProposedWrite::ComputeSweep { .. }),
                    ) = &p.write
                {
                    let proposed = self.read_events(p.position, 1)?;
                    let proposed = proposed.first().filter(|x| x.position == p.position);
                    let by = match proposed.and_then(|x| x.cause) {
                        Some(at) => self.read_events(at, 1)?.into_iter().next(),
                        None => proposed.cloned(),
                    };
                    if let Some(by) = by {
                        w.owed.extend(Owed::decided(e, write, &by));
                    }
                }
                projection.put_proposal(&p, e.position)?;
                staged.proposals.insert(proposal.clone(), p);
            }
            // A set the fold decides itself holds what it decided.
            EventBody::ObjectSetChanged {
                set,
                ty,
                key,
                joined,
            } if !w.zone.as_ref().is_some_and(|z| z.decides(set)) => {
                staged.member(projection, set, ty, key, *joined, e.position)?
            }
            EventBody::ObjectSetChanged { .. } => {}
            EventBody::OutputRecorded {
                seam,
                input,
                output,
                output_at,
                field,
            } => {
                // An output a forgotten source's key sealed is erased, and
                // a fill that names it fills nothing.
                let held;
                let output = match (output, output_at) {
                    _ if dropped => None,
                    (Some(output), None) => {
                        projection.put_output(seam, input, output, e.position)?;
                        let recorded = staged.outputs.entry((seam.clone(), input.clone()));
                        recorded.or_default().push((e.position, output.clone()));
                        Some(output)
                    }
                    (None, Some(at)) => match named(view, &staged.outputs, seam, input, *at, e) {
                        Ok(named) => {
                            held = named;
                            Some(&held)
                        }
                        Err(_) if self.erased_output(w, view, staged, *at)? => None,
                        Err(e) => return Err(e),
                    },
                    _ => return Err(damaged(Part::Log, Some(e.position), Flaw::OutputOnce)),
                };
                if let (Some(output), Some((ty, key, name))) = (output, field)
                    && let Some(r) = staged.record(view, ty, key)?
                {
                    let mut due = Vec::new();
                    let mut outputs = Outputs {
                        view,
                        staged: Some(&staged.outputs),
                        before: None,
                        due: owes.then_some(&mut due),
                        waiting: false,
                        plans: &w.plans,
                        keys: Some(&self.keys),
                        apart: Some(&staged.apart),
                    };
                    let next = record::fill(
                        &w.ontology,
                        &r,
                        name,
                        seam,
                        input,
                        output,
                        e,
                        &w.settings,
                        &mut outputs,
                    )?;
                    if !due.is_empty() {
                        w.owed
                            .push(Owed::new(e, ty.clone(), Of::Record(key.clone())));
                    }
                    if let Some(next) = next {
                        projection.put_record(&next)?;
                        self.index_vectors(&w.ontology, &next)?;
                        kept::note(&kept_at!(w, view, projection), staged, Some(&r), &next)?;
                        staged.put(next);
                        self.decided(w, view, staged, e, (Some(&r), ty, key))?;
                    }
                }
            }
        }
        kept::settle(&kept_at!(w, view, projection), staged, e)?;
        if let Some(zone) = &mut w.zone {
            if zone.viewing() {
                let at = (e.position, e.known_at);
                let f = folding!(w, view, projection, self.limits.max_depth, at);
                let moved = kept::moved(staged);
                zone.groups(f, staged, &moved)?;
            }
            zone.shaped(&w.ontology, staged, e);
        }
        Ok(())
    }

    /// The record of `ty` at `key` the fold of `e` changed from `before`,
    /// decided in each set the fold decides itself.
    fn decided(
        &self,
        w: &Writer,
        view: &dyn ProjectionView,
        staged: &mut Staged,
        e: &Event,
        (before, ty, key): (Option<&StoredRecord>, &TypeName, &RecordKey),
    ) -> Result<()> {
        let Some(zone) = w.zone.as_ref().filter(|z| z.deciding()) else {
            return Ok(());
        };
        let Some(after) = staged.records.get(&(ty.clone(), key.clone())).cloned() else {
            return Ok(());
        };
        let (projection, at) = (self.store(), (e.position, e.known_at));
        let f = folding!(w, view, projection, self.limits.max_depth, at);
        zone.record(f, staged, (before, &after))
    }

    /// Whether the log forgot `source` (058): its list says so, or an act
    /// from `from` on, which a rebuild meets after the values it erases.
    /// A source once forgotten stays so.
    fn forgot(
        &self,
        w: &mut Writer,
        view: &dyn ProjectionView,
        staged: &Staged,
        source: &SourceId,
        from: Position,
    ) -> Result<bool> {
        if w.forgotten.contains(source) || forgotten_list(view, staged)?.contains_key(source) {
            return Ok(true);
        }
        for page in self.pages(from) {
            for e in page? {
                if matches!(&e.body, EventBody::SourceForgotten { source: s, .. } if s == source) {
                    w.forgotten.insert(source.clone());
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Whether the output recorded at `at` was erased: sealed under the
    /// key of a source the log forgot.
    fn erased_output(
        &self,
        w: &mut Writer,
        view: &dyn ProjectionView,
        staged: &Staged,
        at: Position,
    ) -> Result<bool> {
        let Some(e) = self.read_events(at, 1)?.pop().filter(|e| e.position == at) else {
            return Ok(false);
        };
        let sealed = e
            .sealed
            .as_ref()
            .filter(|_| matches!(e.body, EventBody::OutputRecorded { .. }));
        let Some((sealed, source)) = sealed.and_then(|s| Some((s, s.source.as_ref()?))) else {
            return Ok(false);
        };
        match self.keys.opened_sealed(source, sealed)? {
            Ok(_) => Ok(false),
            Err(missing) => self.erased_any(w, (view, staged), source, (&missing, at)),
        }
    }

    /// Whether any of `keys`, which the key store does not give, was
    /// destroyed by an act in the log ([`Self::erased_key`]).
    fn erased_any(
        &self,
        w: &mut Writer,
        (view, staged): (&dyn ProjectionView, &Staged),
        source: &SourceId,
        (keys, at): (&[lakelands_api::id::KeyId], Position),
    ) -> Result<bool> {
        for key in keys {
            if self.erased_key(w, (view, staged), source, (key, at))? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Whether version `key` of `source`'s key, which the key store does
    /// not give, was destroyed by an act in the log, a value it sealed at
    /// `at` then being erased: what the fold knows first, an act of removal
    /// that names it or the source forgotten (058, 065), and only then a
    /// look ahead in the log. A look ahead that finds the act, or the
    /// forgetting, makes it known, so each version is looked for once: the
    /// next value it sealed is answered by what the fold knows, and one no
    /// act names fails the fold closed.
    fn erased_key(
        &self,
        w: &mut Writer,
        (view, staged): (&dyn ProjectionView, &Staged),
        source: &SourceId,
        (key, at): (&lakelands_api::id::KeyId, Position),
    ) -> Result<bool> {
        if w.removals.iter().any(|r| r.destroyed(source, key))
            || w.forgotten.contains(source)
            || forgotten_list(view, staged)?.contains_key(source)
        {
            return Ok(true);
        }
        Ok(self.taken(w, view, source, (key, at))? || self.forgot(w, view, staged, source, at)?)
    }

    /// Whether an act of removal destroyed version `key` of `source`'s key
    /// (065): the acts a fold met or a rebuild was told of, those the
    /// projection lists, which a writer that opened since has not met, or
    /// else one from `from` on, which a fold that knows no act meets after
    /// the values it shreds, as an import's or one a crash stopped midway.
    fn taken(
        &self,
        w: &mut Writer,
        view: &dyn ProjectionView,
        source: &SourceId,
        (key, from): (&lakelands_api::id::KeyId, Position),
    ) -> Result<bool> {
        if w.removals.iter().any(|r| r.destroyed(source, key)) {
            return Ok(true);
        }
        for act in crate::remove::acts(self, view)? {
            if !w.removals.contains(&act) {
                w.removals.push(act);
            }
        }
        if w.removals.iter().any(|r| r.destroyed(source, key)) {
            return Ok(true);
        }
        for page in self.pages(from) {
            for e in page? {
                let act = crate::remove::Removal::of(&e);
                if let Some(act) = act.filter(|a| a.destroyed(source, key)) {
                    w.ahead.insert(act.at);
                    w.removals.push(act);
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Keep the engine's vector index in step with a record's vector
    /// fields: a live record's vectors are indexed, a retired or cleared
    /// one's removed.
    fn index_vectors(&self, ontology: &Ontology, r: &StoredRecord) -> Result<()> {
        self.index_vectors_as(ontology, &r.ty, r)
    }

    /// Keep the vector index in step with a record a migration's step
    /// reaches, of the migrated type or of one whose keys a re-key moves,
    /// its declaration `before` the migration: at its old key, the vectors of
    /// each field the declaration now in force does not hold are taken out,
    /// and every one where the key is `left` with no record moving to it;
    /// then its vectors are indexed by that declaration, but for a field it
    /// held as it holds it, under the same key, which is indexed already. A
    /// field renamed is ranked under its new name, and one removed by no
    /// name (lakelands#768).
    fn reindex(
        &self,
        ontology: &Ontology,
        before: &TypeDecl,
        pre: &StoredRecord,
        post: &StoredRecord,
        left: bool,
    ) -> Result<()> {
        let (Some(index), false) = (self.engine.vectors(), self.scoped.scoped()) else {
            return Ok(());
        };
        let now = ontology.ty(&post.ty).ok();
        let vector = |f: &&FieldDecl| f.ty.kind == StorageKind::Vector;
        for f in before.fields.iter().filter(vector) {
            let kept =
                now.is_some_and(|d| d.fields.iter().filter(vector).any(|g| g.name == f.name));
            if !kept || left {
                index.remove(&pre.ty, &pre.key, f.name.as_str())?;
            }
        }
        let had = |f: &FieldDecl| {
            before
                .fields
                .iter()
                .filter(vector)
                .any(|g| g.name == f.name)
        };
        let same = |f: &FieldDecl| {
            pre.key == post.key && had(f) && pre.fields.get(&f.name) == post.fields.get(&f.name)
        };
        self.index_vectors_where(ontology, &post.ty, post, |f| !same(f))
    }

    /// [`Self::index_vectors`] for a record whose vector fields are those
    /// `declared` declares: a link moved to a link type that is declared
    /// only by the next event has the fields of the one it came from.
    fn index_vectors_as(
        &self,
        ontology: &Ontology,
        declared: &TypeName,
        r: &StoredRecord,
    ) -> Result<()> {
        self.index_vectors_where(ontology, declared, r, |_| true)
    }

    /// [`Self::index_vectors_as`] for the vector fields `due` takes.
    fn index_vectors_where(
        &self,
        ontology: &Ontology,
        declared: &TypeName,
        r: &StoredRecord,
        due: impl Fn(&FieldDecl) -> bool,
    ) -> Result<()> {
        // A scoped refold restates no vectors: the records it restates hold
        // none (`scrub::falls_back`), and every other record's are those
        // its head holds.
        let (Some(index), Ok(decl), false) = (
            self.engine.vectors(),
            ontology.ty(declared),
            self.scoped.scoped(),
        ) else {
            return Ok(());
        };
        for f in (decl.fields.iter()).filter(|f| f.ty.kind == StorageKind::Vector && due(f)) {
            let vector: Option<Vec<f32>> = r
                .fields
                .get(&f.name)
                .and_then(Value::as_array)
                .filter(|_| !r.retired)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_f64)
                        .map(|x| x as f32)
                        .collect()
                });
            match vector {
                Some(v) => index.put(&r.ty, &r.key, f.name.as_str(), &v)?,
                None => index.remove(&r.ty, &r.key, f.name.as_str())?,
            }
        }
        Ok(())
    }

    /// [`Self::rebuild`], under the writer's lock its caller holds. The
    /// whole log is folded into the scratch beside the projection and
    /// swapped in (071); the projection answers as it was until then, and
    /// a rebuild that fails leaves it so. An engine that keeps no scratch
    /// is rebuilt in place.
    pub(super) fn rebuild_held(&self, w: &mut Writer) -> Result<()> {
        // No shape is kept from here until the fold is whole again, and no
        // list of a source's versions.
        // The acts of removal the projection lists are told to the fold
        // before it meets the values they select (065), each with what
        // identifies records at it, in place of one the fold met untold.
        // A list it cannot read tells none: the fold meets each act
        // unknown, and leaves it owed for the rebuild after, which knows it.
        let view = self.engine.view(None);
        let listed = view.and_then(|v| crate::remove::told(self, v.as_ref()));
        for act in listed.unwrap_or_default() {
            match w.removals.iter_mut().find(|r| **r == act) {
                Some(met) => *met = act,
                None => w.removals.push(act),
            }
        }
        self.reclaim_kept();
        self.rebuilds.fetch_add(1, Ordering::SeqCst);
        lakelands_core::lock(&self.shapes).clear();
        self.keys.unlist();
        let began = std::time::Instant::now();
        self.level(Level::Rebuilding, 1);
        // Told ended however the rebuild ends, a panic out of it included.
        let ended = Told(self, Level::Rebuilding, 0);
        let folded = (|| {
            // It reads the recorded outputs and makes no call due but a
            // failed append's; the calls a write before it made due stay
            // that write's.
            if let Swapped::In(_) = self.rebuilt_beside(w, Beside::Rebuild)? {
                return self.catch_up_in(w);
            }
            self.engine.projection().clear()?;
            self.retip(w)?;
            w.ontology = Ontology::default().named();
            w.settings = BTreeMap::new();
            // A page at a time: a rebuild in place that stops is continued
            // by the next fold from where it committed, as a failed fold is.
            w.rebuilding = true;
            let folded = self.fold_from(w, Position::GENESIS);
            w.rebuilding = false;
            folded
        })();
        drop(ended);
        self.rebuilds.fetch_add(1, Ordering::SeqCst);
        lakelands_core::lock(&self.shapes).clear();
        self.keys.unlist();
        folded?;
        self.measured(began, w.tip.as_ref().map_or(0, |t| t.0.0));
        // Everything the log holds is folded again: the head is its tip.
        self.publish(w);
        Ok(())
    }

    /// A lake over the scratch beside the projection `location` keeps,
    /// holding no lease, which folds into the scratch only; none where the
    /// engine keeps no scratch. Opening it opens the log and the scratch,
    /// never the projection, whatever layout made it (071).
    pub(super) fn scratch(location: &Location, seams: &Seams) -> Result<Option<Lake>> {
        let opener = opener(location, seams)?;
        match opener.open_scratch(location) {
            Ok(scratch) => {
                let engine: Arc<dyn LakeEngine> = Arc::from(scratch);
                Ok(Some(Lake::assemble(location, seams.clone(), engine, None)))
            }
            Err(Error::Unbound { .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Fold the log into this lake's scratch, on `w`, from where it is
    /// folded through to the log's tip, and mark it built there. `whole`
    /// empties it first, so the whole log is folded. The fold is the
    /// writer's, under the flag `w.rebuilding` holds, and is told the acts
    /// `w.removals` holds.
    pub(super) fn build(&self, w: &mut Writer, whole: bool) -> Result<Built> {
        if whole {
            self.engine.projection().clear()?;
        }
        let folded;
        (folded, w.tip) = ends(self.engine.as_ref())?;
        {
            let view = self.engine.view(None)?;
            w.ontology = Ontology::at(view.as_ref())?.named();
            w.settings = settings_in(view.as_ref(), &w.ontology)?;
        }
        let from = folded.map_or(Position::GENESIS, Position::next);
        self.fold_from(w, from)?;
        let through = self.engine.mark_built()?;
        let events = through.map_or(0, |t| (t.0 + 1).saturating_sub(from.0));
        Ok(Built {
            ahead: folded,
            through,
            events,
        })
    }

    /// Build the scratch beside this lake's projection as `how` says, and
    /// put it in the projection's place under the writer's lease: the
    /// writer's own lists of acts go with the fold and come back from it.
    /// Nothing done where the engine keeps no scratch, and where the log
    /// holds no event, which no mark keeps: the caller folds in place. Any
    /// other refusal of the swap leaves the projection as it was, and the
    /// scratch marked.
    ///
    /// No rebuild is refused for a reader (071): where the engine would
    /// refuse the swap while another handle reads the projection, the log
    /// is not folded beside it, and the caller folds in place, as before
    /// 071 — an upgrade's open too, whose build ahead is then removed, as
    /// it is where a reader came between the build and the swap.
    pub(super) fn rebuilt_beside(&self, w: &mut Writer, how: Beside) -> Result<Swapped> {
        let lease = w.lease.clone().ok_or_else(no_lease)?;
        // Asked first, so a fold beside is not spent on a swap refused.
        if !self.engine.may_swap()? {
            let spent = how == Beside::Resume && self.spend_ahead()?;
            return Ok(Swapped::Busy(spent));
        }
        let Some(mut builder) = Lake::scratch(&self.location, &self.seams)? else {
            return Ok(Swapped::Kept);
        };
        builder.keys = self.keys.clone();
        let before = (
            w.removals.clone(),
            w.forgotten.clone(),
            w.ahead.clone(),
            w.owed.clone(),
            w.failed,
        );
        let built = {
            let builder = builder.turn();
            let mut b = builder.writer();
            // What the writer holds of acts goes into the builder's
            // writer, and comes back in the same exchange.
            let lend = |w: &mut Writer, b: &mut Writer| {
                std::mem::swap(&mut w.removals, &mut b.removals);
                std::mem::swap(&mut w.forgotten, &mut b.forgotten);
                std::mem::swap(&mut w.ahead, &mut b.ahead);
                std::mem::swap(&mut w.owed, &mut b.owed);
                std::mem::swap(&mut w.failed, &mut b.failed);
            };
            lend(w, &mut b);
            b.rebuilding = how == Beside::Rebuild;
            let built = builder.build(&mut b, how != Beside::Resume);
            lend(w, &mut b);
            built?
        };
        // The builder lets the scratch go before it is swapped: no handle
        // may hold it open then. One marked at no event goes with it.
        drop(builder);
        let swapped = match built.through {
            Some(_) => self.engine.swap(&lease),
            None => Ok(None),
        };
        let busy = |e: &Error| matches!(e.situation(), Some(Situation::SwapBusy { .. }));
        match swapped {
            Ok(_) if built.through.is_some() => Ok(Swapped::In(built)),
            Ok(_) => Ok(Swapped::Kept),
            Err(e) if !busy(&e) => {
                self.lose(w, &e);
                Err(e)
            }
            Err(_) => {
                (w.removals, w.forgotten, w.ahead, w.owed, w.failed) = before;
                Lake::discard_ahead(&self.location, self.seams.clone())?;
                Ok(Swapped::Busy(built.ahead.is_some()))
            }
        }
    }

    /// Remove a build ahead an open that folds in place will not swap in:
    /// whether one was marked. One a builder holds stays, and is not
    /// counted.
    fn spend_ahead(&self) -> Result<bool> {
        match Lake::discard_ahead(&self.location, self.seams.clone()) {
            Ok(marked) => Ok(marked),
            Err(Error::Unbound { .. }) => Ok(false),
            Err(e) if matches!(e.situation(), Some(Situation::Rebuilding { .. })) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

impl Turn<'_> {
    /// Clear the projection and fold the whole log again. Only the lease's
    /// holder does: a handle without it would clear under the holder's fold,
    /// and a lost one takes the lease again first, as a write does.
    pub fn rebuild(&self) -> Result<Position> {
        let mut w = self.writer();
        self.regain(&mut w)?;
        if w.lease.is_none() {
            return Err(no_lease());
        }
        self.rebuild_held(&mut w)?;
        self.finish_forgetting(&mut w)?;
        Ok(w.tip.as_ref().map_or(Position::GENESIS, |t| t.0))
    }
}

/// What became of a build beside the projection.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Swapped {
    /// Folded beside the projection and put in its place.
    In(Built),
    /// Nothing built: the engine keeps no scratch, or the log holds no
    /// event. The caller folds in place.
    Kept,
    /// Not put in place, since another handle read the projection: the
    /// caller folds in place. True where a build ahead the open would
    /// have gone on from was removed.
    Busy(bool),
}

/// How a build beside the projection begins.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Beside {
    /// From where a scratch built ahead stopped, or the whole log where
    /// none is kept, as the open's catch-up folds: a writer's open of an
    /// older layout.
    Resume,
    /// The whole log, as the open's catch-up folds: an open to rebuild.
    Open,
    /// The whole log, told the acts of removal the projection lists, as
    /// [`Turn::rebuild`] folds.
    Rebuild,
}

/// What a build beside the projection folded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Built {
    /// Where a scratch built before was folded through, and this build
    /// went on from; none where it began at the log's first event.
    pub ahead: Option<Position>,
    /// Where the scratch is folded through and marked; none for a log that
    /// holds no event.
    pub through: Option<Position>,
    /// How many events this build folded.
    pub events: u64,
}

/// One event a transaction folded, kept in place of the records it
/// changed: what it does to a record. A later read of a record the
/// transaction did not write takes the view's row through every step, in
/// the order of their events ([`Staged::record`]).
pub(crate) enum Step {
    /// A `migrated` event.
    Migrated(Box<Migrated>),
    /// A `declared` event that computed derived fields again and changed a
    /// record.
    Recomputed(Box<Recomputed>),
    /// A `value_type_migrated` event.
    Values(Box<Values>),
}

impl Step {
    /// A record, not an ended key, after the step. A recomputation reads
    /// the outputs in `recorded` that stand before its own event, makes no
    /// call due and indexes nothing: the event that reads writes the
    /// record, and indexes what it writes.
    pub(super) fn apply(
        &self,
        r: StoredRecord,
        view: &dyn ProjectionView,
        recorded: &Recorded,
    ) -> Result<StoredRecord> {
        match self {
            Step::Migrated(s) => s.apply(r),
            Step::Recomputed(s) => Ok(s.apply(&r, view, recorded, None)?.unwrap_or(r)),
            Step::Values(s) => Ok(s.apply(r)),
        }
    }

    /// The migration, of a step that may move a key; a recomputation moves
    /// none.
    pub(super) fn migrated(&self) -> Option<&Migrated> {
        match self {
            Step::Migrated(s) => Some(s),
            Step::Recomputed(_) | Step::Values(_) => None,
        }
    }
}

/// One `declared` event that computes derived fields again: all that the
/// computation reads, as it stood at the event, so that the flush's walk
/// and a later read through the step compute the same, whatever the writer
/// holds by then.
pub(crate) struct Recomputed {
    /// The event, whole: its position, and the provenance it gives every
    /// field it changes.
    pub(super) event: Event,
    /// The types whose records it computes again, and what computing them
    /// reads of the ontology after the event ([`Ontology::computing`]):
    /// each entry shared with the writer's ontology and with the other
    /// steps that read it.
    pub(super) entries: Ontology,
    /// The settings at the event.
    pub(super) settings: Arc<BTreeMap<String, Value>>,
    /// The plans its records are computed by: its own, since a read
    /// computes through it without the writer.
    pub(super) plans: record::Plans,
    /// The lake's keys, for a call that reads a source's erasable values.
    pub(super) keys: Option<Arc<crate::sealing::Keys>>,
}

/// The types whose records `declaration`, over `o`, computes again: whose
/// fields name a function declared again, and a type in force declared
/// again with other computed fields (saying `write` where nothing was said
/// changes none) or with fields computed outside the lake, as an operator
/// retries a recorded failure (022, extended). The fold computes them again
/// and the declaration's append decides their sets (lakelands#941).
pub(crate) fn computes_again(o: &Ontology, declaration: &Declaration) -> Vec<TypeName> {
    match declaration {
        Declaration::Function(f) => (o.types.values())
            .filter(|t| {
                (t.decl.fields.iter()).any(|d| d.function.as_deref() == Some(f.name.as_str()))
            })
            .map(|t| t.decl.name.clone())
            .collect(),
        Declaration::Type(t) => match o.types.get(&t.name) {
            Some(old)
                if computing_changed(&old.decl, t) || crate::backed::computed_outside(o, t) =>
            {
                vec![t.name.clone()]
            }
            _ => vec![],
        },
        _ => vec![],
    }
}

/// The fields `declaration` computes again, per type: those naming a
/// function it declares; of a type it declares, those computed otherwise
/// than in force, and those computed outside the lake, which it settles
/// again (062). What reads them is not among them.
pub(crate) fn fields_again(o: &Ontology, declaration: &Declaration) -> Vec<(TypeName, FieldName)> {
    let computed = |f: &FieldDecl| f.derived.is_some() || f.function.is_some() || f.embed.is_some();
    let how = |f: &FieldDecl| {
        (
            f.derived.clone(),
            f.function.clone(),
            f.embed.clone(),
            f.at_read(),
        )
    };
    match declaration {
        Declaration::Function(fun) => (o.types.values())
            .flat_map(|t| {
                let named = |d: &&FieldDecl| d.function.as_deref() == Some(fun.name.as_str());
                (t.decl.fields.iter().filter(named)).map(|d| (t.decl.name.clone(), d.name.clone()))
            })
            .collect(),
        Declaration::Type(t) => {
            let Some(old) = o.types.get(&t.name) else {
                return vec![];
            };
            let outside = crate::backed::computed_outside(o, t);
            let backed = |n: &str| {
                o.functions
                    .get(n)
                    .is_some_and(|f| crate::backed::is_backed(&f.decl))
            };
            (t.fields.iter().filter(|f| computed(f)))
                .filter(|f| {
                    let was = old.decl.field(&f.name);
                    was.is_none_or(|was| how(was) != how(f))
                        || outside
                            && (f.embed.is_some() || f.function.as_deref().is_some_and(backed))
                })
                .map(|f| (t.name.clone(), f.name.clone()))
                .collect()
        }
        _ => vec![],
    }
}

/// Whether `new` computes its records otherwise than `old`: its derived
/// expressions, the functions its fields name, what its embeddings are made
/// from, or which are computed at the read (saying `write` where nothing
/// was said changes none). A migration that does computes them again
/// (lakelands#1070); a declaration does too, and settles a type computed
/// outside the lake again besides.
pub(crate) fn computing_changed(old: &TypeDecl, new: &TypeDecl) -> bool {
    let computes = |t: &TypeDecl| -> Vec<_> {
        (t.fields.iter())
            .filter(|f| f.derived.is_some() || f.function.is_some() || f.embed.is_some())
            .map(|f| {
                (
                    f.name.clone(),
                    f.derived.clone(),
                    f.function.clone(),
                    f.embed.clone(),
                    f.at_read(),
                )
            })
            .collect()
    };
    computes(old) != computes(new)
}

impl Recomputed {
    /// A record after the step when the step changes it: a live record of
    /// one of its types whose derived fields come out other than they are.
    /// A record of another type is as it was. The calls it makes due go to
    /// `due`.
    fn apply(
        &self,
        r: &StoredRecord,
        view: &dyn ProjectionView,
        recorded: &Recorded,
        due: Option<&mut Vec<Due>>,
    ) -> Result<Option<StoredRecord>> {
        if !self.entries.types.contains_key(&r.ty) {
            return Ok(None);
        }
        let mut outputs = Outputs {
            view,
            staged: Some(recorded),
            before: Some(self.event.position),
            due,
            waiting: false,
            plans: &self.plans,
            keys: self.keys.as_deref(),
            apart: None,
        };
        record::rederive(&self.entries, r, &self.event, &self.settings, &mut outputs)
    }
}

/// One `value_type_migrated` event a transaction folded: what it does to
/// a record, kept in place of the records it changed, as [`Migrated`] is.
pub(crate) struct Values {
    /// The event's position.
    at: Position,
    /// Per type that uses the value type, the fields it renames in: every
    /// field of the value type but one computed outside the lake, whose
    /// answer the lake does not write.
    pub(super) fields: BTreeMap<TypeName, Vec<FieldName>>,
    /// Old value to new value.
    renames: BTreeMap<String, String>,
}

impl Values {
    pub(super) fn new(
        o: &Ontology,
        at: Position,
        vt: &ValueTypeName,
        renames: &[(String, String)],
    ) -> Values {
        let fields = (o.types.values().map(|t| &t.decl))
            .filter(|t| t.view.is_none())
            .map(|t| (t.name.clone(), renamed_fields(o, t, vt)))
            .filter(|(_, fields)| !fields.is_empty())
            .collect();
        Values {
            at,
            fields,
            renames: renames.iter().cloned().collect(),
        }
    }

    /// A record after the step: each field it renames in that holds an
    /// old value holds the new one, at the step's position. Its version,
    /// its flags and each field's provenance are never touched, and a
    /// record holding no old value is as it was.
    pub(super) fn apply(&self, mut r: StoredRecord) -> StoredRecord {
        let Some(fields) = self.fields.get(&r.ty) else {
            return r;
        };
        for f in fields {
            let new = r.fields.get(f).and_then(Value::as_str);
            if let Some(new) = new.and_then(|v| self.renames.get(v)) {
                r.fields.insert(f.clone(), Value::from(new.as_str()));
                r.position = self.at;
            }
        }
        r
    }
}

/// The types that use a value type, in name order: each, not a view,
/// with a field of it. A `value_type_migrated` names them, and a fold
/// refuses one that names others.
pub(crate) fn using_types(o: &Ontology, vt: &ValueTypeName) -> Vec<TypeName> {
    let uses = |t: &TypeDecl| (t.fields.iter()).any(|f| f.ty.value_type.as_ref() == Some(vt));
    let types = o.types.values().map(|t| &t.decl);
    types
        .filter(|t| t.view.is_none() && uses(t))
        .map(|t| t.name.clone())
        .collect()
}

/// The fields of `t` a value type's renames reach: every field of the
/// value type, derived ones and those computed by an expression included,
/// but one computed outside the lake.
pub(crate) fn renamed_fields(o: &Ontology, t: &TypeDecl, vt: &ValueTypeName) -> Vec<FieldName> {
    let backed = |n: &str| {
        o.functions
            .get(n)
            .is_some_and(|f| crate::backed::is_backed(&f.decl))
    };
    let of = |f: &&lakelands_api::ontology::FieldDecl| {
        f.ty.value_type.as_ref() == Some(vt) && !f.function.as_deref().is_some_and(backed)
    };
    t.fields.iter().filter(of).map(|f| f.name.clone()).collect()
}

/// Whether a record of `t` holds a field the lake computes: a derived
/// field, one computed by a function, an embedding.
pub(crate) fn computes(o: &Ontology, t: &TypeName) -> bool {
    let computed = |f: &FieldDecl| f.derived.is_some() || f.function.is_some() || f.embed.is_some();
    o.ty(t).is_ok_and(|t| t.fields.iter().any(computed))
}

/// One `migrated` event a transaction folded: what it does to a record,
/// kept in place of the records it changed. The fold's walk, the records
/// the transaction wrote and a later read all take a record through
/// [`Migrated::apply`], so the three cannot disagree.
pub(crate) struct Migrated {
    /// The event's position.
    pub(super) at: Position,
    /// The migrated type's declaration before.
    before: TypeDecl,
    pub(super) migration: Migration,
    /// The moves the event lists, per type: old key to new key.
    moves: crate::rekey::Moves,
    /// The same, new key to old.
    pub(super) back: crate::rekey::Moves,
    /// The link types a renamed relation's records move between, old name
    /// to new: the type's whole, every key as it is (035, extended).
    pub(super) types: BTreeMap<TypeName, TypeName>,
    /// For a step with moves, as declared once the migration is in force:
    /// every type with moves, and every type, not a view, that has a
    /// reference field to one.
    pub(super) decls: BTreeMap<TypeName, TypeDecl>,
    /// What the migration fills (`Migration::fill`): each field's
    /// `initial` and its provenance at the step, worked out once.
    fills: Vec<(FieldName, Value, lakelands_api::record::FieldMeta)>,
    /// The fields it makes ones the store fills with the writer that made
    /// the record (084), and their provenance at the step but the writer.
    writers: Vec<(FieldName, lakelands_api::record::FieldMeta)>,
}

impl Migrated {
    pub(super) fn new(
        ontology: &Ontology,
        at: Position,
        before: TypeDecl,
        migration: &Migration,
        rekeyed: &[lakelands_api::write::Rekey],
    ) -> Migrated {
        let (mut moves, mut back) = (crate::rekey::Moves::new(), crate::rekey::Moves::new());
        for r in rekeyed {
            let ty = || r.ty.clone();
            moves
                .entry(ty())
                .or_default()
                .insert(r.from.clone(), r.to.clone());
            back.entry(ty())
                .or_default()
                .insert(r.to.clone(), r.from.clone());
        }
        let reaches = |d: &&TypeDecl| {
            moves.contains_key(&d.name)
                || d.view.is_none() && !crate::rekey::moved_references(d, &moves).is_empty()
        };
        let decls = (ontology.types.values().map(|t| &t.decl).filter(reaches))
            .map(|d| (d.name.clone(), d.clone()))
            .collect();
        let types = lakelands_core::links::moved(migration);
        Migrated {
            at,
            before,
            migration: migration.clone(),
            moves,
            back,
            types,
            decls,
            fills: Vec::new(),
            writers: Vec::new(),
        }
    }

    /// The step at `event`, with the values its migration fills, of
    /// origin `initial`, as a create fills them (050).
    fn filling(mut self, ontology: &Ontology, event: &Event) -> Migrated {
        let filled = |f: &&FieldDecl| self.migration.fill.contains(&f.name);
        let fills = (self.migration.to.fields.iter().filter(filled))
            .filter_map(|f| Some((f.name.clone(), record::initial_of(ontology, f)?)));
        let fills = fills
            .map(|(name, v)| (name, v, record::initial_meta(event)))
            .collect();
        self.fills = fills;
        // A plan that cannot be made is a record that cannot be converted:
        // `apply` answers it as damage for each record of the type, so with
        // none of them there is nothing to fill either.
        let writers = shape::becoming_writer(&self.before, &self.migration).unwrap_or_default();
        let meta = record::writer_meta(event, "");
        self.writers = writers.into_iter().map(|n| (n, meta.clone())).collect();
        self
    }

    /// A record, not an ended key, after the step. Of the migrated type:
    /// its fields converted, its per-field provenance carried over the
    /// renames for the fields it still has, at the step's position. Under
    /// a re-key: its references to moved keys rewritten and, when its own
    /// key moved, its new key; at the step's position when either changed
    /// it. Its version and its flags are never touched.
    pub(super) fn apply(&self, mut r: StoredRecord) -> Result<StoredRecord> {
        if r.ty == self.migration.to.name {
            let cannot = || {
                let (ty, key) = (r.ty.clone(), r.key.clone());
                damaged(Part::Log, Some(self.at), Flaw::CannotConvert { ty, key })
            };
            let converted = shape::migrate_record(&self.before, &self.migration, &r.fields)
                .map_err(|_| cannot())?
                .ok_or_else(cannot)?;
            let renames = &self.migration.renames;
            let renamed = |old: FieldName| match renames.iter().find(|(from, _)| *from == old) {
                Some((_, to)) => to.clone(),
                None => old,
            };
            let maker = record::maker(&r);
            // An erased value's provenance stays with its field (058).
            let declared = |f: &FieldName| self.migration.to.has_field(f);
            r.meta = (std::mem::take(&mut r.meta).into_iter())
                .map(|(old, meta)| (renamed(old), meta))
                .filter(|(new, m)| converted.contains_key(new) || m.erased && declared(new))
                .collect();
            r.fields = converted;
            r.position = self.at;
            // A live record takes what the migration fills where it holds
            // no value; a retired one is history, and takes its `initial`
            // if it is written again (050).
            for (name, v, meta) in self.fills.iter().filter(|_| !r.retired) {
                if !r.fields.contains_key(name) {
                    r.fields.insert(name.clone(), v.clone());
                    r.meta.insert(name.clone(), meta.clone());
                }
            }
            // And each field it makes the writer's takes the writer that
            // made the record, as far as the record tells (084), with its
            // provenance where the type keeps one, as a create leaves it.
            for (name, meta) in self.writers.iter().filter(|_| !r.retired) {
                r.fields.insert(name.clone(), Value::from(maker.clone()));
                if self.migration.to.field_meta {
                    let writer = maker.clone();
                    let meta = lakelands_api::record::FieldMeta {
                        writer,
                        ..meta.clone()
                    };
                    r.meta.insert(name.clone(), meta);
                }
            }
        }
        if let Some(decl) = self.decls.get(&r.ty) {
            let fields = crate::rekey::rewritten(decl, &r.fields, &self.moves);
            let to = self.moves.get(&r.ty).and_then(|m| m.get(&r.key));
            if to.is_some() || fields != r.fields {
                r.position = self.at;
            }
            r.key = to.cloned().unwrap_or(r.key);
            r.fields = fields;
        }
        if let Some(to) = self.types.get(&r.ty) {
            (r.ty, r.position) = (to.clone(), self.at);
        }
        Ok(r)
    }

    /// Carry what is owed over the step: a record's entry to its new key,
    /// an entry of a link type that moved to the type it moved to, and a
    /// sweep of a type whose keys moved back to its first key.
    fn carry(&self, owed: &mut [Owed]) {
        for o in owed {
            let (before, moves) = (o.clone(), self.moves.get(&o.ty));
            let restart = moves.is_some() && matches!(o.of, Of::Sweep { .. });
            match &mut o.of {
                Of::Record(key) => *key = moves.and_then(|m| m.get(key)).unwrap_or(key).clone(),
                Of::Sweep { after, .. } if restart => *after = None,
                Of::Sweep { .. } => {}
            }
            o.ty = self.types.get(&o.ty).unwrap_or(&o.ty).clone();
            if restart || *o != before {
                o.moved += 1;
            }
        }
    }

    /// Whether `key` is the new key of a move: a record arrives at it.
    fn arrives(&self, ty: &TypeName, key: &RecordKey) -> bool {
        self.back.get(ty).is_some_and(|m| m.contains_key(key))
    }

    /// Where the record of `ty` at `key` after the step was before it: the
    /// old key of the move that arrives there, else the key itself, under
    /// the type its records moved from, if they did; `None` for a key that
    /// ended at the step, which a record moved away from and none arrived
    /// at, as every key of a type whose records moved to another.
    pub(super) fn source<'a>(
        &'a self,
        ty: &'a TypeName,
        key: &'a RecordKey,
    ) -> Option<(&'a TypeName, &'a RecordKey)> {
        if self.types.contains_key(ty) {
            return None;
        }
        match self.back.get(ty).and_then(|m| m.get(key)) {
            Some(old) => Some((ty, old)),
            None => (self.moved(ty, key) == key).then_some((self.origin(ty), key)),
        }
    }

    /// The type whose records the step moved to `ty`, else `ty` itself.
    pub(super) fn origin<'a>(&'a self, ty: &'a TypeName) -> &'a TypeName {
        let from = self.types.iter().find(|(_, to)| *to == ty);
        from.map_or(ty, |(from, _)| from)
    }

    /// The key a record at `key` before the step has after it.
    pub(super) fn moved<'a>(&'a self, ty: &TypeName, key: &'a RecordKey) -> &'a RecordKey {
        let to = self.moves.get(ty).and_then(|m| m.get(key));
        to.unwrap_or(key)
    }
}
