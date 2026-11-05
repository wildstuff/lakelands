//! Thinning a type's raw rows past a horizon, as one act in parts beside
//! the writer (`docs/decisions/102-a-thinned-series-keeps-its-roll-ups.md`,
//! on the road of `docs/decisions/099-work-beside-the-writer-runs-in-parts.md`).
//!
//! **The selection** is the records' state at the act's first position: a
//! key is taken when its record stood there, live or retired, with its
//! field certainly before the horizon. An instant is before it when it is
//! earlier; a date when its day has ended in every zone, the day's
//! midnight in UTC plus 36 hours at or before the horizon, so that no
//! zone's reading of the calendar holds part of the day after it. An
//! absent or null value, or one that does not read as its kind, is never
//! taken. A key an earlier act took has no state and is not seen.
//!
//! **The act.** The walk selects on a view off the writer; the first turn
//! reads again, under the writer, the keys the tail past that view wrote,
//! refuses a thinning that would strand a group a kept view does not hold
//! exactly, and appends the first `values_thinned`. Each part names at
//! most `part_events` keys, and the last is followed in its own append by
//! the `act_whole` that names the first part's position. The fold does
//! the rest: it strands the groups, drops the states and puts the
//! tombstones. A selection that is empty appends nothing.
//!
//! **Owed.** An act whose parts no `act_whole` naming its first event
//! ends is found at a writer's open through the type's act row, and taken
//! up again from its first position: the selection is read there again,
//! at the view before that position, where the keys the act already took
//! have no state, so nothing is held between the parts but the keys still
//! to go.

use std::collections::BTreeSet;

use lakelands_api::engine::ProjectionView;
use lakelands_api::error::Case;
use lakelands_api::event::{Event, EventBody};
use lakelands_api::ontology::StorageKind;
use lakelands_api::record::StoredRecord;
use lakelands_api::seam::FeedFilter;
use lakelands_api::write::{Thin, ThinReport};
use lakelands_api::{Error, Position, RecordKey, Result, Timestamp, TypeName};

use crate::exec::Turn;
use crate::lake::{Lake, Writer};
use crate::log::Draft;
use crate::ontology::Ontology;
use crate::road::{Job, Reach, Turned};
use crate::writes;

/// How a date is judged against the horizon: its UTC midnight plus this
/// is when the day has ended in every zone.
const DAY_ENDED: i64 = 36 * 3_600_000_000;

/// The act of thinning `ty`, walked, then taken in turns.
struct Thinning {
    ty: TypeName,
    thin: Thin,
    /// The keys still to take, in key order.
    keys: BTreeSet<RecordKey>,
    /// The keys the tail past the walk's view wrote, judged again in the
    /// first turn.
    moved: BTreeSet<RecordKey>,
    /// The act's first position, once it is in the log or found owed.
    act: Option<Position>,
    records: u64,
    through: Position,
    answer: Option<tokio::sync::oneshot::Sender<Result<ThinReport>>>,
}

impl Job for Thinning {
    fn reach(&self) -> Reach {
        Reach {
            sources: Vec::new(),
            types: vec![self.ty.clone()],
            keys: BTreeSet::new(),
        }
    }

    fn walk(&mut self, lake: &Lake, view: &dyn ProjectionView) -> Result<()> {
        let (ty, thin) = (&self.ty, &self.thin);
        if self.act.is_none() {
            self.act = open(lake, view, ty, thin)?;
        }
        match self.act {
            Some(at) => {
                let then = lake.engine.view(Some(Position(at.0 - 1)))?;
                self.keys = selection(then.as_ref(), ty, thin)?;
            }
            None => self.keys = selection(view, ty, thin)?,
        }
        Ok(())
    }

    fn fold(&mut self, tail: &[Event]) -> Result<()> {
        for e in tail {
            for about in lakelands_core::feed::about(&e.body) {
                if about.ty.as_ref() == Some(&self.ty) {
                    self.moved.extend(about.key);
                }
            }
        }
        Ok(())
    }

    fn turn(&mut self, t: &Turn<'_>, w: &mut Writer) -> Result<Turned> {
        let lake = t;
        let view = lake.engine.view(None)?;
        let begins = self.act.is_none();
        if begins {
            // The selection is the state at the act's first position, which
            // this turn's append is: the keys the tail wrote are judged
            // again as they stand.
            let date = by_date(&w.ontology, &self.ty, &self.thin)?;
            for key in std::mem::take(&mut self.moved) {
                self.keys.remove(&key);
                if let Some(r) = view.get(&self.ty, &key)?
                    && before(&r, &self.thin, date)
                {
                    self.keys.insert(key);
                }
            }
            if self.keys.is_empty() {
                self.through = view.position().unwrap_or(Position::GENESIS);
                return Ok(Turned::Whole);
            }
            exact(view.as_ref(), &w.ontology, &self.ty)?;
        }
        let now = lake.clock(w);
        let room = usize::try_from(lake.limits.part_events.max(1)).unwrap_or(usize::MAX);
        let act = self.act.unwrap_or_else(|| writes::position_after(w, 0));
        let mut keys = Vec::new();
        while keys.len() < room
            && let Some(key) = self.keys.pop_first()
        {
            keys.push(key);
        }
        self.records += keys.len() as u64;
        let mut drafts = vec![Draft::system(
            now,
            EventBody::ValuesThinned {
                ty: self.ty.clone(),
                by: self.thin.by.clone(),
                before: self.thin.before,
                keys,
            },
        )];
        let whole = self.keys.is_empty();
        if whole {
            drafts.push(Draft::system(now, EventBody::ActWhole { act }));
        }
        drop(view);
        lake.append(&mut *w, drafts)?;
        if begins {
            lake.owe_thinning(&self.ty, &self.thin.by, self.thin.before, act);
        }
        if whole {
            lake.thinning_whole(&self.ty, &self.thin.by, self.thin.before);
        }
        self.through = w.tip.as_ref().map_or(Position::GENESIS, |t| t.0);
        self.act = Some(act);
        Ok(if whole { Turned::Whole } else { Turned::More })
    }

    fn end(&mut self, ended: Result<()>) {
        if let Some(answer) = self.answer.take() {
            let _ = answer.send(ended.map(|()| self.report()));
        }
    }
}

impl Thinning {
    fn new(ty: TypeName, thin: Thin, act: Option<Position>) -> Thinning {
        Thinning {
            ty,
            thin,
            keys: BTreeSet::new(),
            moved: BTreeSet::new(),
            act,
            records: 0,
            through: Position::GENESIS,
            answer: None,
        }
    }

    fn report(&self) -> ThinReport {
        ThinReport {
            ty: self.ty.clone(),
            by: self.thin.by.clone(),
            before: self.thin.before,
            records: self.records,
            through: self.through,
            reclaimed: None,
        }
    }
}

/// Whether `ty`'s field `thin.by` holds dates rather than instants; any
/// other kind, or no such field, is refused `ThinBy`.
fn by_date(o: &Ontology, ty: &TypeName, thin: &Thin) -> Result<bool> {
    match o.ty(ty)?.field(&thin.by).map(|f| f.ty.kind) {
        Some(StorageKind::DateTime) => Ok(false),
        Some(StorageKind::Date) => Ok(true),
        _ => Err(Error::invalid(Case::ThinBy {
            ty: ty.clone(),
            field: thin.by.clone(),
        })),
    }
}

/// Whether `r`'s field is certainly before the horizon.
fn before(r: &StoredRecord, thin: &Thin, date: bool) -> bool {
    let Some(at) = r.fields.get(&thin.by).and_then(|v| v.as_str()) else {
        return false;
    };
    let Ok(t) = Timestamp::parse(at) else {
        return false;
    };
    match date {
        true => t.0.saturating_add(DAY_ENDED) <= thin.before.0,
        false => t < thin.before,
    }
}

/// The keys of `ty` the act takes, as `view` holds the type.
fn selection(view: &dyn ProjectionView, ty: &TypeName, thin: &Thin) -> Result<BTreeSet<RecordKey>> {
    let date = by_date(&Ontology::at(view)?, ty, thin)?;
    let mut keys = BTreeSet::new();
    crate::lake::walk(view, ty, true, |r| {
        if before(&r, thin, date) {
            keys.insert(r.key);
        }
        Ok(())
    })?;
    Ok(keys)
}

/// Refuse `KeptNotExact` where a kept view that reads `ty`, itself or
/// through another view, holds a group that is not exact now. The rule is
/// conservative: an inexact group refuses the act whether or not the act
/// would strand it, since which group a taken record's past versions
/// stood in is the fold's to find, and a group inexact only at an earlier
/// position refuses nothing.
fn exact(view: &dyn ProjectionView, o: &Ontology, ty: &TypeName) -> Result<()> {
    let mut reach = BTreeSet::from([ty.clone()]);
    loop {
        let more: Vec<TypeName> = (o.types.values())
            .filter_map(|e| Some((&e.decl.name, e.decl.view.as_ref()?)))
            .filter(|(name, v)| {
                !reach.contains(*name)
                    && std::iter::once(&v.source)
                        .chain(v.merge.iter().map(|m| &m.source))
                        .any(|s| reach.contains(s))
            })
            .map(|(name, _)| name.clone())
            .collect();
        if more.is_empty() {
            break;
        }
        reach.extend(more);
    }
    reach.remove(ty);
    for name in reach {
        let Some(state) = crate::kept::state(view, &name)? else {
            continue;
        };
        if state.dropping || state.not_exact == [0, 0] {
            continue;
        }
        return Err(Error::invalid(Case::KeptNotExact {
            view: name,
            ty: ty.clone(),
        }));
    }
    Ok(())
}

/// The first position of the act on `ty` by `thin` the log began and did
/// not end, if there is one.
fn open(
    lake: &Lake,
    view: &dyn ProjectionView,
    ty: &TypeName,
    thin: &Thin,
) -> Result<Option<Position>> {
    let row =
        (view.thinned_acts(ty)?.into_iter()).find(|a| a.by == thin.by && a.before == thin.before);
    match row {
        Some(_) => first(lake, view, ty, thin),
        None => Ok(None),
    }
}

/// The first part of an act on `ty` by `thin` that no `act_whole` of its
/// own ended: read from the log's parts and ends, which the feed's index
/// finds. Only an `act_whole` naming the act's first event ends it; one
/// another act appended beside it does not.
fn first(
    lake: &Lake,
    view: &dyn ProjectionView,
    ty: &TypeName,
    thin: &Thin,
) -> Result<Option<Position>> {
    let Some(head) = view.position() else {
        return Ok(None);
    };
    let filter = FeedFilter {
        kinds: vec!["values_thinned".into(), "act_whole".into()],
        ..Default::default()
    };
    let (mut at, mut from) = (Vec::new(), Position::GENESIS);
    let events = loop {
        let m = view.matching(&filter, from, head, 1024)?;
        at.extend(m.positions);
        match m.through {
            Some(t) if t < head => from = t.next(),
            Some(_) => break lake.read_these(&at)?,
            None => {
                let mut all = Vec::new();
                for page in lake.pages(Position::GENESIS) {
                    all.extend(page?.into_iter().filter(|e| e.position <= head));
                }
                break all;
            }
        }
    };
    let mut ours = None;
    for e in events {
        match e.body {
            EventBody::ValuesThinned {
                ty: t, by, before, ..
            } => {
                if ours.is_none() && &t == ty && by == thin.by && before == thin.before {
                    ours = Some(e.position);
                }
            }
            EventBody::ActWhole { act } if ours == Some(act) => ours = None,
            _ => {}
        }
    }
    Ok(ours)
}

/// The thinning acts the log began and did not end, each with the
/// position it began at, noted on the lake so a write meets its line
/// before the fold puts the parts' tombstones (102). A log the feed's
/// index finds no part in owes none, and its declarations are not read.
pub(crate) fn owing(
    lake: &Lake,
    view: &dyn ProjectionView,
) -> Result<Vec<(TypeName, Thin, Position)>> {
    let Some(head) = view.position() else {
        return Ok(Vec::new());
    };
    let filter = FeedFilter {
        kinds: vec!["values_thinned".into()],
        ..Default::default()
    };
    let m = view.matching(&filter, Position::GENESIS, head, 1)?;
    if m.through.is_some() && m.positions.is_empty() {
        return Ok(Vec::new());
    }
    let ontology = Ontology::at(view)?;
    let mut owing = Vec::new();
    for ty in ontology.types.keys() {
        for row in view.thinned_acts(ty)? {
            let thin = Thin {
                by: row.by,
                before: row.before,
            };
            if let Some(at) = first(lake, view, ty, &thin)? {
                lake.owe_thinning(ty, &thin.by, thin.before, at);
                owing.push((ty.clone(), thin, at));
            }
        }
    }
    Ok(owing)
}

/// The acts a writer's open finds the log began and did not end, each
/// taken up again from its first position.
pub(crate) fn owed(lake: &Lake, view: &dyn ProjectionView) -> Result<Vec<Box<dyn Job>>> {
    Ok(owing(lake, view)?
        .into_iter()
        .map(|(ty, thin, at)| Box::new(Thinning::new(ty, thin, Some(at))) as Box<dyn Job>)
        .collect())
}

/// The horizon of an owed thinning of `ty` whose selection at the act's
/// first position takes `key`, though no part has taken it yet: a write
/// at that key is refused as one at a key the act already took is (102).
/// The selection is read again at the view before the act, the same way
/// the act's walk reads it.
pub(crate) fn owed_takes(lake: &Lake, ty: &TypeName, key: &RecordKey) -> Result<Option<Timestamp>> {
    for (by, horizon, at) in lake.thinnings_owed(ty) {
        let thin = Thin {
            by,
            before: horizon,
        };
        let then = lake.engine.view(Some(Position(at.0.saturating_sub(1))))?;
        let Ok(date) = by_date(&Ontology::at(then.as_ref())?, ty, &thin) else {
            continue;
        };
        if let Some(r) = then.get(ty, key)?
            && before(&r, &thin, date)
        {
            return Ok(Some(horizon));
        }
    }
    Ok(None)
}

/// Send the thinning of `ty` along the lake's road, answering `answer`
/// when it is whole: the store's door, which holds no writer lock while
/// the act runs.
pub(crate) fn start(
    lake: &std::sync::Arc<Lake>,
    ty: TypeName,
    thin: Thin,
    answer: tokio::sync::oneshot::Sender<Result<ThinReport>>,
) {
    let mut job = Thinning::new(ty, thin, None);
    job.answer = Some(answer);
    crate::road::send(lake, Box::new(job));
}

/// What the act would take at the head, appending nothing: its count, and
/// the refusals it would meet.
pub(crate) fn preflight(lake: &Lake, ty: &TypeName, thin: &Thin) -> Result<ThinReport> {
    let view = lake.engine.view(None)?;
    let ontology = Ontology::at(view.as_ref())?;
    let mut job = Thinning::new(ty.clone(), thin.clone(), None);
    job.walk(lake, view.as_ref())?;
    if !job.keys.is_empty() {
        exact(view.as_ref(), &ontology, ty)?;
    }
    job.records = job.keys.len() as u64;
    job.through = view.position().unwrap_or(Position::GENESIS);
    Ok(job.report())
}
