//! What a write reads while an act is owed
//! (`docs/decisions/099-work-beside-the-writer-runs-in-parts.md`, "A write
//! reads through an owed act").
//!
//! Between an act's first turn and its restate the projection still holds
//! the values it takes: only the restate, scoped or whole, clears them.
//! A read answers them, as 065 says; a write does not. It reads the record
//! as the restate will leave it: the record's own events, folded again
//! through the erasure the owed acts name, in memory and for one key.
//!
//! The writer already holds what the fold met: the sources the log forgot
//! ([`Writer::forgotten`]) and the acts of removal ([`Writer::removals`]).
//! A record is folded again only when a field of it was written by one of
//! those sources, so a write elsewhere pays one test a record. Once an act
//! is done, its erasure is already in the projection and folding it again
//! changes nothing.
//!
//! [`Writer::forgotten`]: crate::lake::Writer
//! [`Writer::removals`]: crate::lake::Writer

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::engine::{Chosen, Declared, Matched, ProjectionView, Scan};
use lakelands_api::event::{Digest, Event, EventBody, EventId};
use lakelands_api::quarantine::{QuarantineFilter, Quarantined};
use lakelands_api::query::{Edge, Walk};
use lakelands_api::record::{Origin, StoredRecord};
use lakelands_api::seam::{FEED_EXAMINED, FeedFilter};
use lakelands_api::write::{Proposal, ProposalFilter};
use lakelands_api::{Position, RecordKey, RelationName, Result, SourceId, TypeName, Value};

use crate::lake::{Lake, Writer};
use crate::ontology::Ontology;
use crate::remove::Removal;
use crate::sealing::Keys;

/// A view a write reads: the projection, or the projection through the
/// acts still owed.
pub(crate) enum Viewed<'a> {
    /// Nothing is owed: the projection is the restate.
    Plain(&'a dyn ProjectionView),
    /// Some act may leave a value out: records its sources wrote are
    /// folded again.
    Through(Through<'a>),
}

impl<'a> Viewed<'a> {
    /// The view `w`'s writes decide in, reads through the acts the log
    /// leaves owed.
    pub(crate) fn of(view: &'a dyn ProjectionView, w: &'a Writer, lake: &'a Lake) -> Viewed<'a> {
        if w.forgotten.is_empty() && w.removals.is_empty() {
            return Viewed::Plain(view);
        }
        Viewed::Through(Through {
            inner: view,
            forgotten: &w.forgotten,
            removals: &w.removals,
            lake,
            ontology: &w.ontology,
            settings: &w.settings,
            plans: &w.plans,
            keys: lake.keys.as_ref(),
        })
    }

    /// The view to read.
    pub(crate) fn view(&self) -> &dyn ProjectionView {
        match self {
            Viewed::Plain(view) => *view,
            Viewed::Through(t) => t,
        }
    }
}

/// The projection, read through the owed acts: a record a source that
/// owes an act wrote is folded again through the erasure.
pub(crate) struct Through<'a> {
    inner: &'a dyn ProjectionView,
    forgotten: &'a BTreeSet<SourceId>,
    removals: &'a [Removal],
    lake: &'a Lake,
    ontology: &'a Ontology,
    settings: &'a BTreeMap<String, Value>,
    plans: &'a crate::record::Plans,
    keys: &'a Keys,
}

impl Through<'_> {
    /// Whether a field of `r` was written by a source that owes an act.
    fn touches(&self, r: &StoredRecord) -> bool {
        r.fields.keys().any(|f| {
            r.meta.get(f).is_some_and(|m| {
                m.origin == Origin::Source
                    && m.source.as_ref().is_some_and(|s| {
                        self.forgotten.contains(s) || self.removals.iter().any(|a| &a.source == s)
                    })
            })
        })
    }

    /// `r` as the restate will leave it: folded again from its own events
    /// through the erasure the owed acts name. A record no owed source
    /// touched is answered as it is.
    fn restated(&self, r: &StoredRecord) -> Result<Option<StoredRecord>> {
        if !self.touches(r) {
            return Ok(Some(r.clone()));
        }
        let Some(head) = self.inner.position() else {
            return Ok(Some(r.clone()));
        };
        let events = self.events_about(&r.ty, &r.key, head)?;
        let mut outputs = crate::backed::Outputs::planning(self.inner, self.plans, self.keys);
        let mut before: Option<StoredRecord> = None;
        for e in events {
            let e = self.erased(e);
            match &e.body {
                EventBody::OutputRecorded {
                    seam,
                    input,
                    output,
                    output_at,
                    field: Some((_, _, name)),
                } => {
                    let Some(output) = self.output(seam, input, output, *output_at)? else {
                        continue;
                    };
                    if let Some(r) = before.as_ref()
                        && let Some(next) = crate::record::fill(
                            self.ontology,
                            r,
                            name,
                            seam,
                            input,
                            &output,
                            &e,
                            self.settings,
                            &mut outputs,
                        )?
                    {
                        before = Some(next);
                    }
                }
                _ => {
                    if let Some(next) = crate::record::apply(
                        self.ontology,
                        before.as_ref(),
                        &e,
                        self.settings,
                        &mut outputs,
                    )? {
                        before = Some(next);
                    }
                }
            }
        }
        Ok(before)
    }

    /// The output an `output_recorded` carries, named where it names one
    /// the lake holds: `None` where the restate drops it.
    fn output(
        &self,
        seam: &str,
        input: &Digest,
        output: &Option<Value>,
        at: Option<Position>,
    ) -> Result<Option<Value>> {
        if let Some(output) = output {
            return Ok(Some(output.clone()));
        }
        let _ = at;
        Ok(self.inner.output(seam, input)?.map(|(_, v)| v))
    }

    /// The event as the restate folds it: a forgotten source's sealed
    /// values erased, and those an owed removal selects.
    fn erased(&self, e: Event) -> Event {
        let mut e = match crate::sealing::lifted(&e) {
            Some(lifted) => lifted,
            None => e,
        };
        let forgotten = e
            .sealed
            .as_ref()
            .and_then(|s| s.source.as_ref())
            .is_some_and(|s| self.forgotten.contains(s));
        if forgotten {
            crate::sealing::erase(&mut e);
        }
        match crate::remove::redacted(self.removals, self.ontology, &e) {
            Some((plain, _)) => plain,
            None => e,
        }
    }

    /// Every event about the record of `ty` at `key` through `head`, in
    /// log order: the feed index's positions where it decides, and the
    /// log where it does not.
    fn events_about(&self, ty: &TypeName, key: &RecordKey, head: Position) -> Result<Vec<Event>> {
        let filter = FeedFilter {
            types: vec![ty.clone()],
            keys: vec![key.clone()],
            ..FeedFilter::default()
        };
        let mut from = Position::GENESIS;
        let mut at = Vec::new();
        loop {
            let m = self.inner.matching(&filter, from, head, FEED_EXAMINED)?;
            at.extend(m.positions);
            let Some(through) = m.through else {
                return self.logged_about(ty, key, head);
            };
            if through >= head {
                break;
            }
            let next = through.next();
            if next <= from {
                break;
            }
            from = next;
        }
        self.lake.read_these(&at)
    }

    /// [`Self::events_about`] from the log, where the index decides
    /// nothing.
    fn logged_about(&self, ty: &TypeName, key: &RecordKey, head: Position) -> Result<Vec<Event>> {
        let mut out = Vec::new();
        'pages: for page in self.lake.pages(Position::GENESIS) {
            for e in page? {
                if e.position > head {
                    break 'pages;
                }
                let about = lakelands_core::feed::about(&e.body);
                if about
                    .iter()
                    .any(|a| a.ty.as_ref() == Some(ty) && a.key.as_ref() == Some(key))
                {
                    out.push(e);
                }
            }
        }
        Ok(out)
    }
}

impl ProjectionView for Through<'_> {
    fn position(&self) -> Option<Position> {
        self.inner.position()
    }

    fn declarations(&self) -> Result<Vec<Declared>> {
        self.inner.declarations()
    }

    fn replaced(&self, from: Position, to: Position) -> Result<Option<Vec<Declared>>> {
        self.inner.replaced(from, to)
    }

    fn chosen(&self, ty: &TypeName) -> Result<Chosen> {
        self.inner.chosen(ty)
    }

    fn get(&self, ty: &TypeName, key: &RecordKey) -> Result<Option<StoredRecord>> {
        match self.inner.get(ty, key)? {
            Some(r) => self.restated(&r),
            None => Ok(None),
        }
    }

    fn scan(&self, ty: &TypeName, scan: &Scan) -> Result<Vec<StoredRecord>> {
        let mut out = Vec::new();
        for r in self.inner.scan(ty, scan)? {
            if let Some(r) = self.restated(&r)? {
                out.push(r);
            }
        }
        Ok(out)
    }

    fn count(&self, ty: &TypeName, scan: &Scan) -> Result<u64> {
        self.inner.count(ty, scan)
    }

    fn get_many(&self, ty: &TypeName, keys: &[RecordKey]) -> Result<Vec<StoredRecord>> {
        let mut out = Vec::new();
        for r in self.inner.get_many(ty, keys)? {
            if let Some(r) = self.restated(&r)? {
                out.push(r);
            }
        }
        Ok(out)
    }

    fn edges(
        &self,
        ty: &TypeName,
        keys: &[RecordKey],
        relation: Option<&RelationName>,
        walk: Walk,
    ) -> Result<Vec<Vec<Edge>>> {
        self.inner.edges(ty, keys, relation, walk)
    }

    fn proposals(&self, filter: &ProposalFilter) -> Result<Vec<Proposal>> {
        self.inner.proposals(filter)
    }

    fn quarantined(&self, filter: &QuarantineFilter) -> Result<Vec<Quarantined>> {
        self.inner.quarantined(filter)
    }

    fn proposals_before(
        &self,
        filter: &ProposalFilter,
        before: Option<Position>,
    ) -> Result<Option<Vec<Proposal>>> {
        self.inner.proposals_before(filter, before)
    }

    fn is_member(&self, set: &str, ty: &TypeName, key: &RecordKey) -> Result<bool> {
        self.inner.is_member(set, ty, key)
    }

    fn members(&self, set: &str, ty: &TypeName) -> Result<BTreeSet<RecordKey>> {
        self.inner.members(set, ty)
    }

    fn applied(&self, id: &EventId) -> Result<Option<Position>> {
        self.inner.applied(id)
    }

    fn output(&self, seam: &str, input: &Digest) -> Result<Option<(Position, Value)>> {
        self.inner.output(seam, input)
    }

    fn matching(
        &self,
        filter: &FeedFilter,
        from: Position,
        through: Position,
        limit: u32,
    ) -> Result<Matched> {
        self.inner.matching(filter, from, through, limit)
    }

    fn of_source(
        &self,
        source: &SourceId,
        from: Position,
        through: Position,
        limit: u32,
    ) -> Result<Matched> {
        self.inner.of_source(source, from, through, limit)
    }

    fn order(&self, name: &TypeName, key: &RecordKey) -> Result<Option<Vec<u8>>> {
        self.inner.order(name, key)
    }

    fn order_page(
        &self,
        name: &TypeName,
        after: Option<&RecordKey>,
        n: u32,
    ) -> Result<Vec<(RecordKey, Vec<u8>)>> {
        self.inner.order_page(name, after, n)
    }

    fn thinned_acts(&self, ty: &TypeName) -> Result<Vec<lakelands_api::read::ThinnedAct>> {
        self.inner.thinned_acts(ty)
    }

    fn thinned_at(
        &self,
        ty: &TypeName,
        key: &RecordKey,
    ) -> Result<Option<lakelands_api::time::Timestamp>> {
        self.inner.thinned_at(ty, key)
    }
}
