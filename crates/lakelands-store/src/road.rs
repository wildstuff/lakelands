//! Work beside the writer runs on one road
//! (`docs/decisions/099-work-beside-the-writer-runs-in-parts.md`).
//!
//! An act or a build is a *job*: a walk on a view, off the writer; a
//! catch-up, off the writer, which folds the log past that view until a
//! read finds no more of it than one turn may read; then turns, each one entry of the writer's queue
//! ([`exec::write`]) held to a limit, each leaving the lake whole. The
//! first turn reads the tail past the view under the writer and hands it
//! to the job before it does anything else, so what the walk judged is
//! judged again over every write it did not see.
//!
//! **One job at a time, one turn at a time.** The runner is spawned as
//! [`crate::choose::choose_orders`] is, and queues a job's next turn only
//! when its last one has ended: a write sent meanwhile is already in the
//! queue in front of it, so it waits for at most the turn in hand. Jobs
//! run oldest first.
//!
//! **Nothing of a job is the runner's.** What a job has done is in the log
//! and in the acts' lists; what it holds in memory is its walk, which it
//! can walk again. The runner holds its lake weakly between turns, so a
//! lake whose last handle goes stops it there, and the writer's open that
//! follows asks the kinds in [`OWED`] what the log and the lists owe, and
//! starts the runner on it without waiting for it.

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, Weak};

use lakelands_api::engine::ProjectionView;
use lakelands_api::error::{Error, Situation};
use lakelands_api::event::Event;
use lakelands_api::seam::FeedFilter;
use lakelands_api::{Position, RecordKey, Result, SourceId, TypeName};
use lakelands_core::lock;

use crate::exec::{self, Turn};
use crate::lake::{Lake, Writer, read_only};

/// The lake's road: the jobs that wait, oldest first, and whether a
/// runner drives them.
#[derive(Default)]
pub(crate) struct Road {
    jobs: VecDeque<Held>,
    running: bool,
}

/// A job, shared between the runner and the turn it is lent to.
type Held = Arc<Mutex<Box<dyn Job>>>;

/// Whether a job has turns left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Turned {
    /// It takes another turn.
    More,
    /// It is whole.
    Whole,
}

/// One job on the road.
pub(crate) trait Job: Send + 'static {
    /// What of the log past the job's view reaches it.
    fn reach(&self) -> Reach;

    /// The walk, off the writer, on `view`: everything the turns need that
    /// grows with the lake.
    fn walk(&mut self, lake: &Lake, view: &dyn ProjectionView) -> Result<()>;

    /// The events of the tail that reached the job, in log order: off the
    /// writer while it catches up, and under it in its first turn, before
    /// [`Job::turn`].
    fn fold(&mut self, tail: &[Event]) -> Result<()>;

    /// One turn, under the writer: bounded, and leaving the lake whole.
    fn turn(&mut self, t: &Turn<'_>, w: &mut Writer) -> Result<Turned>;

    /// Off the writer, before each turn after the first: what the next
    /// turn needs that a view or a lock beside the writer gives.
    fn aside(&mut self, lake: &Lake) -> Result<()> {
        let _ = lake;
        Ok(())
    }

    /// The job ended, whole or not: where its caller waits, it is answered
    /// here. A caller who went away stops nothing.
    fn end(&mut self, ended: Result<()>) {
        let _ = ended;
    }
}

/// What of the tail reaches a job: every write by its sources, and every
/// write about a record of its types, of its keys where it names any.
#[derive(Clone, Debug, Default)]
pub(crate) struct Reach {
    pub sources: Vec<SourceId>,
    pub types: Vec<TypeName>,
    pub keys: BTreeSet<RecordKey>,
}

impl Reach {
    /// Whether `e` reaches the job.
    pub(crate) fn reaches(&self, e: &Event) -> bool {
        let by = e.source.as_ref().is_some_and(|s| self.sources.contains(s));
        by || lakelands_core::feed::about(&e.body).iter().any(|a| {
            a.ty.as_ref().is_some_and(|t| self.types.contains(t))
                && (self.keys.is_empty() || a.key.as_ref().is_some_and(|k| self.keys.contains(k)))
        })
    }
}

/// A kind of job a lake may owe: the jobs the log and the lists at `view`
/// leave owed of it, found off the writer.
pub(crate) type Owes = fn(&Lake, &dyn ProjectionView) -> Result<Vec<Box<dyn Job>>>;

/// Every kind of job a writer's open takes up: a purge a failure left
/// not whole ([`crate::purge::PURGED`]), a removal or a forgetting whose
/// `act_whole` is not in the log, the restate and reclaim a whole act owes
/// or a crash left, and the builds of the settings a projection marked as
/// building.
pub(crate) const OWED: &[Owes] = &[
    crate::purge::owed,
    crate::remove::owed,
    crate::forget::owed,
    crate::remove::settle_owed,
    crate::build::owed,
];

/// How many times a first turn finds the tail past its view too long
/// before the job answers unavailable, appending nothing.
pub(crate) const TRIES: u32 = 3;

/// Put `job` at the end of `lake`'s road, and start a runner where none
/// drives it.
pub(crate) fn send(lake: &Arc<Lake>, job: Box<dyn Job>) {
    let mut road = lock(&lake.road);
    road.jobs.push_back(Arc::new(Mutex::new(job)));
    if !std::mem::replace(&mut road.running, true) {
        let weak = Arc::downgrade(lake);
        tokio::spawn(drive(weak));
    }
}

/// At a writer's open: find what `kinds` owe, off the writer, and send it,
/// without holding the open up. The lake is held weakly, as the runner
/// holds it between turns: a look at what a lake owes does not keep the
/// last handle from letting the lake go, and finds nothing then.
pub(crate) fn resume(lake: &Arc<Lake>, kinds: &'static [Owes]) {
    if kinds.is_empty() || !lake.is_writer() {
        return;
    }
    let weak = Arc::downgrade(lake);
    tokio::spawn(async move {
        let finding = weak.clone();
        let owed = exec::run(move || {
            let Some(lake) = finding.upgrade() else {
                return Ok(Vec::new());
            };
            let view = lake.engine.view(None)?;
            let mut owed = Vec::new();
            for kind in kinds {
                owed.extend(kind(&lake, view.as_ref())?);
            }
            Ok(owed)
        });
        // What cannot be found now is found by the next open. The store
        // keeps no log of its own to tell it to; the job stays owed in the
        // log and the lists, where the next open looks again.
        let owed: Vec<Box<dyn Job>> = owed.await.unwrap_or_default();
        let Some(lake) = weak.upgrade() else { return };
        // A lake the caller has let go is not sent work again: what was
        // found stays owed for the next open, which looks again.
        if lake.is_let_go() {
            return;
        }
        for job in owed {
            send(&lake, job);
        }
    });
}

/// Drive the road until no job waits.
async fn drive(weak: Weak<Lake>) {
    loop {
        let next = {
            let Some(lake) = weak.upgrade() else { return };
            let mut road = lock(&lake.road);
            let next = road.jobs.pop_front();
            road.running = next.is_some();
            next
        };
        let Some(job) = next else { return };
        let ended = run(&weak, &job).await;
        lock(&job).end(ended);
    }
}

/// The lake, while a handle holds it; a runner whose lake went stops, and
/// what its job left is owed in the log.
fn held(weak: &Weak<Lake>) -> Result<Arc<Lake>> {
    weak.upgrade().ok_or_else(read_only)
}

/// One job, from its walk to its last turn.
async fn run(weak: &Weak<Lake>, job: &Held) -> Result<()> {
    let tail = held(weak)?.limits.part_tail.max(1);
    let mut past = walk(weak, job).await?;
    for _ in 0..TRIES {
        past = catch_up(weak, job, past, tail).await?;
        let (lake, j) = (held(weak)?, job.clone());
        let first = exec::write(&lake, move |t| {
            // The tail is read under the writer caught up, not under the
            // view as it stood: a fold that stopped short leaves the
            // projection at its last commit, and a view taken before the
            // catch-up would read the tail past a position the log has
            // already passed (099).
            let mut w = t.for_write()?;
            if w.lease.is_none() {
                return Err(read_only());
            }
            let view = t.engine.view(None)?;
            let reach = lock(&j).reach();
            let Some(read) = read_tail(t.lake(), view.as_ref(), &reach, past, tail)? else {
                return Ok(None);
            };
            drop(view);
            let mut job = lock(&j);
            job.fold(&read)?;
            job.turn(t, &mut w).map(Some)
        });
        let first = first.await?;
        // Between turns the runner holds its lake only weakly.
        drop(lake);
        match first {
            None => continue,
            Some(Turned::Whole) => return Ok(()),
            Some(Turned::More) => {}
        }
        loop {
            let (lake, j) = (held(weak)?, job.clone());
            let aside = (lake.clone(), job.clone());
            exec::run(move || lock(&aside.1).aside(&aside.0)).await?;
            let turned = exec::write(&lake, move |t| turn(t, lock(&j).as_mut())).await?;
            drop(lake);
            if turned == Turned::Whole {
                return Ok(());
            }
        }
    }
    Err(Error::unavailable(Situation::TailOverran {
        tries: TRIES,
        bound: tail,
    }))
}

/// The job's walk, on a view off the writer: the position it stands at.
async fn walk(weak: &Weak<Lake>, job: &Held) -> Result<Option<Position>> {
    let (lake, j) = (held(weak)?, job.clone());
    exec::run(move || {
        let view = lake.engine.view(None)?;
        lock(&j).walk(&lake, view.as_ref())?;
        Ok(view.position())
    })
    .await
}

/// Off the writer, fold the tail past `past` into the job in pieces of
/// `bound` events, until a read finds no more than `bound` of it: the
/// position the job stands at then, so that its first turn reads only
/// what was written since. A lake written faster than the job catches up stops it
/// after [`TRIES`] rounds, and its first turn finds the tail too long.
async fn catch_up(
    weak: &Weak<Lake>,
    job: &Held,
    mut past: Option<Position>,
    bound: u32,
) -> Result<Option<Position>> {
    for _ in 0..TRIES {
        let (lake, j) = (held(weak)?, job.clone());
        let moved = exec::run(move || {
            let view = lake.engine.view(None)?;
            let reach = lock(&j).reach();
            let from = past.map_or(Position::GENESIS, Position::next);
            let Some(head) = view.position().filter(|h| *h >= from) else {
                return Ok(None);
            };
            let fold = |read: &[Event]| lock(&j).fold(read);
            let piece = usize::try_from(bound).unwrap_or(usize::MAX);
            // A short tail is folded here too, and the first turn reads
            // only what was written after this read.
            let long = match found(view.as_ref(), &reach, from, head, u32::MAX)? {
                Found::At(at) => {
                    for at in at.chunks(piece) {
                        fold(&reached(&lake, &reach, at)?)?;
                    }
                    at.len() > piece
                }
                Found::Over => return Ok(None),
                Found::Unindexed => match logged(&lake, &reach, from, head, bound)? {
                    Some(read) => fold(&read).map(|()| false)?,
                    None => folded_logged(&lake, &reach, from, head, piece, fold).map(|()| true)?,
                },
            };
            Ok(Some((view.position(), long)))
        });
        match moved.await? {
            Some((at, long)) => {
                past = at;
                if !long {
                    break;
                }
            }
            None => break,
        }
    }
    Ok(past)
}

/// One turn of `job` at the writer, which this handle must hold: a turn
/// is a write, so it catches a writer that was behind up before it
/// decides anything.
fn turn(t: &Turn<'_>, job: &mut dyn Job) -> Result<Turned> {
    let mut w = t.for_write()?;
    if w.lease.is_none() {
        return Err(read_only());
    }
    job.turn(t, &mut w)
}

/// The events past `past` through `view`'s position that reach a job, in
/// log order; none where finding them reads more than `bound` of them.
/// They are found in the log index, by source for the sources and by type
/// (and key) for the types, so a lake written elsewhere costs nothing, and
/// read from the log at their positions. Where the index decides nothing,
/// as while a rebuild refolds, the log itself is read and every event
/// counts.
pub(crate) fn read_tail(
    lake: &Lake,
    view: &dyn ProjectionView,
    reach: &Reach,
    past: Option<Position>,
    bound: u32,
) -> Result<Option<Vec<Event>>> {
    let from = past.map_or(Position::GENESIS, Position::next);
    let Some(head) = view.position().filter(|h| *h >= from) else {
        return Ok(Some(Vec::new()));
    };
    match found(view, reach, from, head, bound)? {
        Found::At(at) => reached(lake, reach, &at).map(Some),
        Found::Over => Ok(None),
        Found::Unindexed => logged(lake, reach, from, head, bound),
    }
}

/// What the log index finds of a tail.
enum Found {
    /// The positions of the events that may reach the job, in log order,
    /// each once, however many of its rows name it.
    At(Vec<Position>),
    /// More than the bound.
    Over,
    /// The index decides nothing, and the log is read.
    Unindexed,
}

/// The positions from `from` through `head` the log index finds for
/// `reach`: by source, and by type and key, at most `bound` of them. A
/// seek by key reads the index once a key, so where the job names more
/// keys than the tail holds positions the tail is sought by type alone,
/// under the same bound, and [`reached`] keeps of it what touches the
/// job's keys. Either way the reads go with the tail, not with the job.
fn found(
    view: &dyn ProjectionView,
    reach: &Reach,
    from: Position,
    head: Position,
    bound: u32,
) -> Result<Found> {
    let by_key = reach.keys.len() as u64 <= head.0.saturating_sub(from.0).saturating_add(1);
    let room = bound.saturating_add(1);
    let filter = FeedFilter {
        types: reach.types.clone(),
        keys: match by_key {
            true => reach.keys.iter().cloned().collect(),
            false => Vec::new(),
        },
        ..FeedFilter::default()
    };
    let mut at = BTreeSet::new();
    let sources = reach.sources.iter().map(Some);
    let types = (!reach.types.is_empty()).then_some(None);
    for source in sources.chain(types) {
        let mut next = from;
        while next <= head {
            let m = match source {
                Some(s) => view.of_source(s, next, head, room)?,
                None => view.matching(&filter, next, head, room)?,
            };
            let Some(through) = m.through else {
                return Ok(Found::Unindexed);
            };
            at.extend(m.positions);
            if at.len() as u64 > u64::from(bound) {
                return Ok(Found::Over);
            }
            next = through.next();
        }
    }
    Ok(Found::At(at.into_iter().collect()))
}

/// The events at `at` that reach a job.
fn reached(lake: &Lake, reach: &Reach, at: &[Position]) -> Result<Vec<Event>> {
    let read = lake.read_these(at)?;
    Ok(read.into_iter().filter(|e| reach.reaches(e)).collect())
}

/// [`read_tail`] from the log, where the index decides nothing.
fn logged(
    lake: &Lake,
    reach: &Reach,
    from: Position,
    head: Position,
    bound: u32,
) -> Result<Option<Vec<Event>>> {
    let (mut out, mut rows) = (Vec::new(), 0u64);
    for page in lake.pages(from) {
        for e in page? {
            if e.position > head {
                return Ok(Some(out));
            }
            rows += 1;
            if rows > u64::from(bound) {
                return Ok(None);
            }
            if reach.reaches(&e) {
                out.push(e);
            }
        }
    }
    Ok(Some(out))
}

/// The catch-up's read of the log, where the index decides nothing: what
/// reaches the job, folded in pieces of at most `piece` events.
fn folded_logged(
    lake: &Lake,
    reach: &Reach,
    from: Position,
    head: Position,
    piece: usize,
    mut fold: impl FnMut(&[Event]) -> Result<()>,
) -> Result<()> {
    let mut out = Vec::new();
    'pages: for page in lake.pages(from) {
        for e in page? {
            if e.position > head {
                break 'pages;
            }
            if reach.reaches(&e) {
                out.push(e);
            }
            if out.len() == piece {
                fold(&std::mem::take(&mut out))?;
            }
        }
    }
    if out.is_empty() { Ok(()) } else { fold(&out) }
}

#[cfg(test)]
mod tests;
