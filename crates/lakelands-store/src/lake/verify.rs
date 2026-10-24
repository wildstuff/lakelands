//! Verify, checkpoint, compact and close: the checks over the hash chain
//! and the segments, and the ends of a handle's life.

use super::*;

impl Lake {
    /// Check the hash chain between two positions, every checkpoint in
    /// that range against the segments the engine keeps, that each
    /// segment after the first begins with one, and that no event holds a
    /// number written as an integer beyond ±(2^53 − 1), which the seal
    /// lets none write and another reader would hash differently.
    pub fn verify(
        &self,
        from: Option<Position>,
        through: Option<Position>,
    ) -> Result<lakelands_api::Verification> {
        let from = from.unwrap_or(Position::GENESIS);
        let prev = if from == Position::GENESIS {
            None
        } else {
            self.read_events(Position(from.0 - 1), 1)?.pop()
        };
        let mut before = prev.as_ref().map(|e| (e.position, e.hash.clone()));
        let mut formats = lakelands_core::log::FormatSteps::after(prev.as_ref());
        let mut fence = prev.as_ref().map(|e| e.fence);
        let mut known = prev.map(|e| e.known_at);
        let segments = self.engine.segments()?;
        // Where a segment after the first begins: behind each sealed one.
        let begins: BTreeSet<Position> = segments.iter().map(|s| s.last.next()).collect();
        let mut last = None;
        // Page by page, never past `through`: an event after it is not
        // read, so damage there does not answer for the range asked
        // (lakelands#1156).
        let mut at = from;
        while through.is_none_or(|t| at <= t) {
            let left = through.map_or(u64::from(PAGE), |t| (t.0 - at.0).saturating_add(1));
            let page = self.read_events(at, left.min(u64::from(PAGE)) as u32)?;
            let Some(next) = page.last().map(|e| e.position.next()) else {
                break;
            };
            at = next;
            for e in page {
                log::check_link(before.as_ref().map(|(p, h)| (p, h)), &e)?;
                // No number written as an integer beyond ±(2^53 − 1)
                // (`docs/log-format.md` §1.2). The seal lets none in; a
                // log that holds one was not written through it.
                let written = serde_json::to_value(&e).expect("an event serialises to JSON");
                if let Some(n) = lakelands_core::canonical::integer_too_large(&written) {
                    let flaw = Flaw::IntegerTooLarge { n: n.clone() };
                    return Err(damaged(Part::Log, Some(e.position), flaw));
                }
                // `known_at` never goes back (`docs/log-format.md` §1.3).
                if let Some(k) = known
                    && e.known_at < k
                {
                    let (known_at, before) = (e.known_at, k);
                    let flaw = Flaw::KnownAtBack { known_at, before };
                    return Err(damaged(Part::Log, Some(e.position), flaw));
                }
                known = Some(e.known_at);
                // A lake's format changes only where a new writer upgrades
                // it, and never falls (054).
                if let Some(flaw) = formats.next(&e) {
                    return Err(damaged(Part::Log, Some(e.position), flaw));
                }
                // Fences never fall (`docs/log-format.md` §4): the engines
                // refuse a fall at append and at import, and this finds one
                // written below them.
                if let Some(floor) = fence.filter(|f| e.fence < *f) {
                    let flaw = Flaw::FenceBelow {
                        fence: e.fence,
                        floor,
                    };
                    return Err(damaged(Part::Log, Some(e.position), flaw));
                }
                fence = Some(e.fence);
                if let EventBody::Checkpoint {
                    through,
                    hash,
                    segments: listed,
                } = &e.body
                {
                    check_checkpoint(&e, *through, hash, listed)?;
                    check_segments(listed, &segments, e.position)?;
                } else if begins.contains(&e.position) {
                    return Err(damaged(Part::Log, Some(e.position), Flaw::NoCheckpoint));
                }
                before = Some((e.position, e.hash.clone()));
                last = Some(e);
            }
        }
        let last =
            last.ok_or_else(|| Error::refused(None, Refusal::OutOfRange { head: self.head() }))?;
        // A writer's `lease_taken` that raised the format at the head, with
        // no `lake_upgraded` after it.
        if let Some(flaw) = formats.unmarked().filter(|_| last.position == self.head()) {
            return Err(damaged(Part::Log, Some(last.position), flaw));
        }
        Ok(lakelands_api::Verification {
            from,
            through: last.position,
            hash: last.hash,
        })
    }

    /// The last position no later than the head whose event's `known_at`
    /// is at or before `t`; none before the lake's first event. A binary
    /// search, reading one event per probe: `known_at` never decreases
    /// along the log (`docs/log-format.md` §1.3).
    pub fn position_at(&self, t: Timestamp) -> Result<Option<Position>> {
        let head = self.head();
        let known_at = |p: u64| -> Result<Timestamp> {
            self.read_events(Position(p), 1)?
                .first()
                .map(|e| e.known_at)
                .ok_or_else(|| {
                    let _ = head;
                    let flaw = Flaw::EndsBefore {
                        position: Position(p),
                    };
                    damaged(Part::Log, None, flaw)
                })
        };
        if known_at(0)? > t {
            return Ok(None);
        }
        // `lo` is always a position at or before `t`.
        let (mut lo, mut hi) = (0, head.0);
        while lo < hi {
            let mid = lo + (hi - lo).div_ceil(2);
            if known_at(mid)? <= t {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        Ok(Some(Position(lo)))
    }

    /// For each embedding field of `ty` at the head: the live records with
    /// a vector, from the engine's index, and those without one whose
    /// `embed.from` fields hold text, counted over pages of the records
    /// the engine selects by `<field> is null`, restricted ones included.
    pub fn vector_coverage(
        &self,
        ty: &TypeName,
    ) -> Result<Vec<lakelands_api::read::VectorCoverage>> {
        let view = self.engine.view(None)?;
        let decl = Ontology::at(view.as_ref())?.ty(ty)?.clone();
        let mut out = Vec::new();
        for f in &decl.fields {
            let Some(embed) = &f.embed else {
                continue;
            };
            let embedded = match self.engine.vectors() {
                Some(index) => index.count(ty, f.name.as_str())?,
                None => 0,
            };
            let mut scan = lakelands_api::engine::Scan {
                filter: Some(lakelands_api::expr::Expr::IsNull {
                    arg: Box::new(lakelands_api::expr::Expr::Path {
                        path: vec![f.name.clone()],
                    }),
                }),
                limit: PAGE,
                restricted: true,
                ..Default::default()
            };
            let mut due = 0;
            loop {
                let rows = view.scan(ty, &scan)?;
                due += rows
                    .iter()
                    .filter(|r| crate::backed::embed_text(embed, &r.fields).is_some())
                    .count() as u64;
                match rows.last() {
                    Some(last) if rows.len() == PAGE as usize => {
                        scan.after = Some(lakelands_api::engine::After {
                            values: vec![],
                            key: last.key.clone(),
                        });
                    }
                    _ => break,
                }
            }
            out.push(lakelands_api::read::VectorCoverage {
                field: f.name.clone(),
                embedded,
                due,
            });
        }
        Ok(out)
    }
}

impl Turn<'_> {
    /// Seal the active segment and write the checkpoint that opens the
    /// next, as one step.
    pub fn checkpoint(&self) -> Result<Position> {
        // A segment is sealed only over events the projection has folded.
        self.seal(&mut *self.for_write()?)
    }

    /// Seal the active segment where the engine reports it past
    /// `limits.seal_after_bytes`, on the held writer between two entries
    /// of the queue, so never inside a batch (074). A writer that is
    /// behind, lost or without its lease does not seal, and the size is
    /// weighed again only once the tip has moved: a seal that fails is
    /// tried after the next append, the write before it answered by its
    /// own events.
    pub(crate) fn seal_by_size(&self) {
        let mut w = self.writer();
        let tip = w.tip.as_ref().map(|t| t.0);
        if w.lease.is_none() || w.lost || w.behind || w.sized == tip {
            return;
        }
        w.sized = tip;
        let (size, bound) = (self.engine.active_segment_bytes(), self.limits.seal_bound());
        if let Some(n) = size {
            self.level(Level::ActiveSegmentBytes, n);
        }
        if matches!((size, bound), (Some(n), Some(b)) if n >= b) {
            let _ = self.seal(&mut w);
        }
    }

    fn seal(&self, w: &mut Writer) -> Result<Position> {
        let lease = w.lease.clone().ok_or_else(read_only)?;
        let Some((through, hash)) = w.tip.clone() else {
            return Ok(Position::GENESIS);
        };
        let before = self.engine.segments()?;
        let now = self.clock(w);
        // The engine closes the segment and begins the next with the
        // checkpoint in one step: one refused before it has sealed
        // nothing, and is sent again. It is the checkpoint's append, timed
        // and counted as every other.
        let mut stamped = None;
        let began = self.timer();
        let sealed = self.engine.seal_segment(&lease, false, &mut |sealed| {
            let segments = before.iter().chain([sealed]).cloned().collect();
            let body = EventBody::Checkpoint {
                through,
                hash: hash.clone(),
                segments,
            };
            let (_, events, stored) = self.stamp(w, vec![Draft::system(now, body)])?;
            stamped = Some(events);
            Ok(stored)
        });
        if sealed.is_err() {
            self.count(Count::AppendFailures, 1);
        }
        sealed?;
        let Some(events) = stamped else {
            return Ok(through);
        };
        self.since(Spread::Append, began);
        let events = self.landed(w, events, Reads::default())?;
        self.count(Count::Seals, 1);
        Ok(events.last().map_or(through, |e| e.position))
    }

    /// Give back the storage the projection no longer uses. It runs under
    /// the writer's lock, so a write waits and lands before or after it;
    /// it appends nothing, and the head stays where it is
    /// (`docs/decisions/040-maintenance-never-changes-an-answer.md`). A
    /// lost writer takes the lease again before it, as a write does: its
    /// `lease_taken` changes no answer. It is an act's own reclaim: one
    /// that runs clears a reclaim owed (lakelands#1179).
    pub fn compact(&self) -> Result<lakelands_api::write::CompactReport> {
        let mut w = self.writer();
        self.regain(&mut w)?;
        if w.lease.is_none() {
            return Err(read_only());
        }
        self.reclaim()?
    }

    /// Give the lease up. The handle reads on; it no longer writes.
    pub fn close(&self) -> Result<()> {
        let mut w = self.writer();
        if let Some(lease) = w.lease.take() {
            // The head stays the last folded position; only the lease goes.
            self.unleased();
            self.engine.release_lease(&lease)?;
        }
        Ok(())
    }
}

/// A checkpoint's own claims (`docs/log-format.md` §2.6, §4.3): it follows
/// the segment it seals, repeats its last hash, and lists sealed ranges
/// that start at 0, follow each other with no gap or overlap, and end
/// where it says the sealed segment ends.
fn check_checkpoint(
    e: &Event,
    through: Position,
    hash: &Digest,
    listed: &[SegmentDigest],
) -> Result<()> {
    let at = e.position;
    let broken = |flaw: Flaw| damaged(Part::Checkpoint, Some(at), flaw);
    if e.position.0 != through.0 + 1 {
        return Err(broken(Flaw::CheckpointFollows { through }));
    }
    if *hash != e.prev {
        return Err(broken(Flaw::CheckpointHash));
    }
    let mut next = 0u64;
    for s in listed {
        if s.first.0 != next || s.last < s.first {
            let (first, last) = (s.first, s.last);
            return Err(broken(Flaw::CheckpointRange { first, last, next }));
        }
        next = s.last.0 + 1;
    }
    if next != through.0 + 1 {
        return Err(broken(Flaw::CheckpointEnd { through }));
    }
    Ok(())
}

fn check_segments(listed: &[SegmentDigest], kept: &[SegmentDigest], at: Position) -> Result<()> {
    for s in listed {
        let first = s.first;
        let flaw = match kept.iter().find(|k| k.first == s.first) {
            Some(k) if k == s => continue,
            Some(_) => Flaw::SegmentDiffers { first, at },
            None => Flaw::SegmentMissing { first, at },
        };
        return Err(damaged(Part::Segment, Some(first), flaw));
    }
    Ok(())
}
