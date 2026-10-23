//! The one place engine work leaves the async caller.
//!
//! Engines are synchronous: they do blocking storage work
//! (`docs/decisions/019-the-contract-is-object-safe.md`). Every call the
//! store makes into an engine — a read, a write, a fold — runs on the
//! runtime's blocking pool, so an async caller's executor never waits on
//! storage. A read goes through [`run`]; anything that takes the writer
//! goes through [`write()`], which queues it first. There is no other
//! hand-off in this crate: no nested runtime, no thread that parks the
//! caller.
//!
//! **The queue.** A lake's writes wait their turn in one queue, in the
//! order they came, before they take a thread: the first to find nobody
//! holding the turn takes a blocking thread and runs the entries from the
//! head until the queue is empty, so a crowd of writers parks no thread,
//! and a read beside them finds the pool free. The writer's lock stays;
//! the queue only decides who takes it next, which the lock itself does
//! not do fairly. The log's order is still the order the writer appends
//! in. An entry is a job that runs alone, or record writes as data, which
//! the turn decides together ([`crate::writes::write_many`]).

use std::any::Any;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, Weak};

use lakelands_api::Result;
use lakelands_api::error::{Detail, Error, Situation};

use crate::lake::Lake;
use crate::writes::Job;

/// A lake's queue: its entries, oldest first, and whether a thread holds
/// the turn and runs them.
#[derive(Default)]
pub(crate) struct Queue {
    entries: VecDeque<Entry>,
    held: bool,
}

/// One entry of the queue.
enum Entry {
    /// A job that runs alone, its answer given after it.
    Alone(Box<dyn FnOnce(&Turn<'_>) -> Reply + Send>),
    /// Record writes.
    Records(Vec<Job>),
}

/// An answer decided and not given yet. The turn gives it once it has let
/// the lake go, so a caller who drops the lake on its answer finds it
/// gone, its lease released, as when the work's own thread ended first.
pub(crate) type Reply = Box<dyn FnOnce() + Send>;

/// Run `work` on the blocking pool and answer its result. A panic inside
/// the work is reported as the lake being unavailable, never propagated
/// into the caller's task.
pub async fn run<T, F>(work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(answer) => answer,
        Err(e) => Err(stopped(&e)),
    }
}

/// A turn at `lake`'s writer: what [`Turn::writer`] and [`Turn::for_write`]
/// are taken with. The thread that holds the queue lends it to each
/// entry in turn: to a write's job queued by [`write()`], and to a batch of
/// record writes queued by [`records`] (`write_soon` lends it inline,
/// where no runtime is there to queue on), and it is lent to an owner of
/// the lake by [`Lake::turn`]; it reads as the lake.
pub struct Turn<'l>(&'l Lake);

impl<'l> Turn<'l> {
    /// The lake the turn is at.
    pub(crate) fn lake(&self) -> &'l Lake {
        self.0
    }
}

impl std::ops::Deref for Turn<'_> {
    type Target = Lake;

    fn deref(&self) -> &Lake {
        self.0
    }
}

impl Lake {
    /// A turn at the writer for the one who owns the lake, before it is
    /// shared: an open, an import, a test. Nobody else can queue a write.
    #[doc(hidden)]
    pub fn turn(&mut self) -> Turn<'_> {
        Turn(self)
    }
}

/// Run `work` when its turn in `lake`'s queue comes, lent the turn, and
/// answer its result as [`run`] does. The work runs on the thread that
/// holds the turn, so a caller who goes away does not stop a job between
/// its turn and its end, and the next job's turn comes when the work
/// ends, however it ends.
pub async fn write<T, F>(lake: &Arc<Lake>, work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&Turn<'_>) -> Result<T> + Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    let job = move |t: &Turn<'_>| -> Reply {
        let answer = catch_unwind(AssertUnwindSafe(|| work(t)));
        let answer = answer.unwrap_or_else(|p| Err(stopped_by_panic(p)));
        Box::new(move || drop(tx.send(answer)))
    };
    queue(lake, Entry::Alone(Box::new(job)));
    rx.await.unwrap_or_else(|_| Err(halted(true)))
}

/// Queue record writes as one entry; each job is answered on its own.
pub(crate) fn records(lake: &Arc<Lake>, jobs: Vec<Job>) {
    queue(lake, Entry::Records(jobs));
}

/// How many entries wait in `lake`'s queue.
pub(crate) fn queued(lake: &Lake) -> usize {
    lakelands_core::lock(&lake.queue).entries.len()
}

/// Put `entry` at the end of `lake`'s queue, and take the turn where
/// nobody holds it.
fn queue(lake: &Arc<Lake>, entry: Entry) {
    let mut q = lakelands_core::lock(&lake.queue);
    q.entries.push_back(entry);
    let held = std::mem::replace(&mut q.held, true);
    drop(q);
    if !held {
        hold(lake.queue.clone(), Arc::downgrade(lake));
    }
}

/// Hold the turn of `queue`'s lake on a blocking thread: run the entries
/// from the head until none is left, giving each entry's answers after it.
fn hold(queue: Arc<Mutex<Queue>>, lake: Weak<Lake>) {
    let mut holding = Holding {
        queue,
        lake,
        began: false,
        replies: Vec::new(),
    };
    drop(tokio::task::spawn_blocking(move || {
        holding.began = true;
        while let Some(entry) = holding.next() {
            holding.run(entry);
            holding.reply();
        }
    }));
}

/// The turn, held. It holds its lake only while an entry runs: an entry
/// whose lake is gone, every caller having let it go, is dropped unrun.
/// A panic that unwinds out of it gives the answers decided, answers the
/// jobs it took and had not answered `Stopped`, as their dropped answers
/// are read, and hands the turn on to the entries still queued; a hold
/// the runtime never ran (it was shutting down) lets the turn go and
/// answers every queued job `Stopped` the same way.
struct Holding {
    queue: Arc<Mutex<Queue>>,
    lake: Weak<Lake>,
    began: bool,
    replies: Vec<Reply>,
}

impl Holding {
    /// The head entry; with none left, the turn is let go.
    fn next(&self) -> Option<Entry> {
        let mut q = lakelands_core::lock(&self.queue);
        let entry = q.entries.pop_front();
        q.held = entry.is_some();
        entry
    }

    /// Run `entry` on the lake, while it is there; the lake is let go
    /// before any of its answers is given.
    fn run(&mut self, entry: Entry) {
        let Some(lake) = self.lake.upgrade() else {
            return;
        };
        match entry {
            Entry::Alone(job) => self.replies.push(job(&Turn(&lake))),
            Entry::Records(jobs) => {
                crate::writes::write_many(&Turn(&lake), jobs, &mut self.replies);
            }
        }
    }

    /// Give the answers decided.
    fn reply(&mut self) {
        for reply in self.replies.drain(..) {
            reply();
        }
    }
}

impl Drop for Holding {
    fn drop(&mut self) {
        if self.began && !std::thread::panicking() {
            return;
        }
        self.reply();
        let mut q = lakelands_core::lock(&self.queue);
        let unrun = match self.began {
            true => VecDeque::new(),
            false => std::mem::take(&mut q.entries),
        };
        q.held = !q.entries.is_empty();
        let held = q.held;
        drop(q);
        drop(unrun);
        if held {
            hold(self.queue.clone(), self.lake.clone());
        }
    }
}

/// [`write()`], without waiting for its answer; with no runtime to queue
/// on, run here and now, the writer's lock alone deciding when.
pub(crate) fn write_soon<F>(lake: &Arc<Lake>, work: F)
where
    F: FnOnce(&Turn<'_>) + Send + 'static,
{
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => {
            let lake = lake.clone();
            let job = async move {
                write(&lake, move |t| {
                    work(t);
                    Ok(())
                })
                .await
            };
            drop(runtime.spawn(job));
        }
        Err(_) => work(&Turn(lake)),
    }
}

/// The store's work stopped: whether it panicked or was cancelled, never
/// what a panic said, which may quote a record.
pub(crate) fn stopped(e: &tokio::task::JoinError) -> Error {
    halted(e.is_panic())
}

/// [`stopped`], for work that panicked where it was caught: what it said
/// is dropped unread.
pub(crate) fn stopped_by_panic(payload: Box<dyn Any + Send>) -> Error {
    drop(payload);
    halted(true)
}

/// The store's work stopped, as it panicked or as it was cancelled.
pub(crate) fn halted(panicked: bool) -> Error {
    let detail = Detail::scrubbed(if panicked {
        "it panicked"
    } else {
        "it was cancelled"
    });
    Error::unavailable(Situation::Stopped { detail })
}

#[cfg(test)]
mod tests;
