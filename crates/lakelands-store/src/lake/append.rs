//! The write path: drafts sealed into the hash chain, appended durably and
//! folded, and the events they become landed.

use super::*;

/// Drafts sealed: the lease they go under, the events, and the events as
/// the engine stores them.
pub(crate) type Stamped = (Lease, Vec<Event>, Vec<CanonicalEvent>);

impl Lake {
    /// Seal drafts after the tip, append them durably, fold them. Answers
    /// the sealed events.
    pub fn append(&self, w: &mut Writer, drafts: Vec<Draft>) -> Result<Vec<Event>> {
        self.append_after(w, drafts, Reads::default())
    }

    /// [`Self::append`], for drafts decided over the records in `reads`:
    /// the fold takes each from there instead of reading it again, while
    /// its view is at the position they were read at.
    pub fn append_after(
        &self,
        w: &mut Writer,
        drafts: Vec<Draft>,
        reads: Reads,
    ) -> Result<Vec<Event>> {
        let stamped = self.stamp(w, drafts)?;
        self.append_stamped(w, stamped, reads)
    }

    /// Append events [`Self::stamp`] sealed durably, fold them, and answer
    /// them.
    pub(crate) fn append_stamped(
        &self,
        w: &mut Writer,
        (lease, events, stored): Stamped,
        reads: Reads,
    ) -> Result<Vec<Event>> {
        self.durable(w, &lease, &stored)?;
        self.landed(w, events, reads)
    }

    /// The engine's append of `stored` under `lease`, timed, a refusal
    /// counted; one that says the lease is gone marks the writer lost.
    /// Every append of the writer goes through it.
    pub(super) fn durable(
        &self,
        w: &mut Writer,
        lease: &Lease,
        stored: &[CanonicalEvent],
    ) -> Result<()> {
        let began = self.timer();
        if let Err(e) = self.engine.append(lease, stored) {
            self.count(Count::AppendFailures, 1);
            self.lose(w, &e);
            return Err(e);
        }
        self.since(Spread::Append, began);
        Ok(())
    }

    /// The events `drafts` become after the tip, or after what a batch
    /// staged past it, as the log writes them and as the engine stores
    /// them, under the writer's lease: refused where an append of them
    /// would be. Nothing is changed.
    ///
    /// The seal judges every event as the log's own reading will
    /// (`docs/log-format.md` §1.2, §1.3): one that holds a number written
    /// as an integer beyond ±(2^53 − 1), or a `known_at` or `seen_at` out
    /// of the years 0000 to 9999, refuses the whole append, whatever door
    /// its value came through. Every door refuses such a value itself and
    /// names where it stands; a refusal from here names no place, and is a
    /// defect of the door that let the value pass.
    pub(crate) fn stamp(&self, w: &Writer, drafts: Vec<Draft>) -> Result<Stamped> {
        let Some(lease) = w.lease.clone() else {
            return Err(no_lease());
        };
        // Nothing is appended while the projection is behind the log, and
        // nothing is folded this late: the drafts were decided already.
        if w.behind {
            return Err(Error::unavailable(Situation::DecidedBehind));
        }
        // A host's cause names an event the log holds: at or below the
        // head (021). What it names is the host's business.
        let head = w.tip.as_ref().map(|(p, _)| *p);
        if let Some(cause) = drafts
            .iter()
            .filter_map(|d| d.context.cause)
            .find(|c| Some(*c) > head)
        {
            let case = lakelands_api::error::Case::CausePastHead { cause, head };
            return Err(Error::invalid(case));
        }
        // A reaction that names a trigger's firing as its cause is bound by
        // that trigger's own ceiling too, as the head declares it (021).
        // It is read here, under the writer, so the firing the cause names
        // is the one this append follows, never one appended meanwhile.
        if let Some(d) = drafts.iter().find(|d| d.context.cause.is_some()) {
            self.check_trigger_ceiling(w, d.context)?;
        }
        let mut events = Vec::with_capacity(drafts.len());
        let mut stored: Vec<CanonicalEvent> = Vec::with_capacity(drafts.len());
        let after = w.staged.as_ref().or(w.tip.as_ref());
        let mut prev_pos = after.map(|(p, _)| *p);
        let mut prev_hash = after.map_or_else(Digest::zero, |(_, h)| h.clone());
        for d in drafts {
            let position = prev_pos.map_or(Position::GENESIS, Position::next);
            // Position 0 is written before any lease exists in the lake.
            let fence = if position == Position::GENESIS {
                0
            } else {
                lease.fence
            };
            let mut d = d;
            if let Some(sealed) = &d.sealed
                && let Some(source) = sealed.source.as_ref().or(d.source.as_ref())
            {
                let key = self.keys.sealed(source, sealed)?;
                crate::sealing::seal(&mut d.body, sealed, source, &key);
            }
            let (event, canon, fault) = log::seal_judged(&self.id, position, &prev_hash, fence, d);
            if let Some(case) = fault {
                return Err(Error::invalid(case));
            }
            prev_pos = Some(position);
            prev_hash = event.hash.clone();
            events.push(event);
            stored.push(canon);
        }
        Ok((lease, events, stored))
    }

    /// What follows once the engine holds `events`: the writer stands on
    /// them, and they are folded and published.
    pub(super) fn landed(
        &self,
        w: &mut Writer,
        events: Vec<Event>,
        reads: Reads,
    ) -> Result<Vec<Event>> {
        if let Some(last) = events.last() {
            w.tip = Some((last.position, last.hash.clone()));
            w.last_known_at = w.last_known_at.max(last.known_at);
        }
        // The events are durable whatever the fold answers: its failure is
        // told as that, so that it is not read as a write that did not land.
        let began = self.timer();
        self.fold_all(w, &events, reads, false).map_err(|e| {
            let at = events.first().map_or(0, |e| e.position.0);
            let (from, cause) = (Position(at), Box::new(e));
            Error::unavailable(Situation::NotFolded { from, cause })
        })?;
        self.since(Spread::Fold, began);
        self.count(Count::Appends, 1);
        self.count(Count::Events, events.len() as u64);
        // A shape holds over what is committed now unless an event ends a
        // range; it is carried only from where it stands, so a fold that
        // began after its range's end (another handle wrote, or a fold failed
        // and was caught up) leaves it to its own range.
        // An append whose only such events declare leaves what makes the
        // shape after it, the shape before and its declarations, for the
        // first read of its position to build (lakelands#1056).
        let declares = |e: &&Event| matches!(e.body, EventBody::Declared { .. });
        if let (Some(first), Some(last)) = (events.first(), events.last()) {
            let (first, mut held) = (first.position.0, lakelands_core::lock(&self.shapes));
            let mut ending = events.iter().filter(|e| ends_a_range(&e.body)).peekable();
            if ending.peek().is_none() {
                for (range, _) in held.iter_mut().filter(|(r, _)| r.end() + 1 == first) {
                    *range = *range.start()..=last.position.0;
                }
            } else if ending.all(|e| declares(&e)) {
                let before = held.iter().find(|(r, _)| r.end() + 1 == first);
                let links = |before: &HeldShape| match before {
                    HeldShape::Built(_) => 1,
                    HeldShape::Left(left) => left.links + 1,
                };
                let before = before.filter(|(_, b)| links(b) <= LEFT_LINKS);
                if let Some((_, before)) = before {
                    let declared = (events.iter())
                        .filter_map(|e| match &e.body {
                            EventBody::Declared {
                                declaration,
                                schema_hash,
                                version,
                                ..
                            } => Some(lakelands_api::engine::Declared {
                                declaration: declaration.clone(),
                                version: *version,
                                schema_hash: schema_hash.clone(),
                                since: e.position,
                            }),
                            _ => None,
                        })
                        .collect();
                    let left = Left {
                        before: Some(before.clone()),
                        declared,
                        links: links(before),
                    };
                    held.sort_by_key(|(range, _)| *range.end());
                    if held.len() > 1 {
                        held.remove(0);
                    }
                    let at = last.position.0..=last.position.0;
                    held.push((at, HeldShape::Left(Arc::new(left))));
                }
            }
        }
        // Published only once the fold has committed: a position `head()`
        // answers is one a read can be answered at, on every engine.
        self.publish(w);
        Ok(events)
    }
}
