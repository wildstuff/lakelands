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
//! **The queue.** A lake's writes wait their turn in an async queue, in
//! the order they came, before they take a thread: one write at a time
//! holds a blocking thread on the writer's lock, so a crowd of writers
//! parks no thread, and a read beside them finds the pool free. The
//! writer's lock stays; the queue only decides who takes it next, which
//! the lock itself does not do fairly. The log's order is still the order
//! the writer appends in.

use std::sync::Arc;

use lakelands_api::Result;
use lakelands_api::error::{Detail, Error, Situation};

use crate::lake::Lake;

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
/// are taken with. It is lent to a write's job by `write` (and by
/// `write_soon` inline, where no runtime is there to queue it), and to
/// an owner of the lake by [`Lake::turn`]; it reads as the lake.
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

/// Run `work` on the blocking pool when its turn in `lake`'s queue comes,
/// lent the turn, and answer its result as [`run`] does. The turn goes
/// into the blocking work with it, and one that is waited for is waited
/// for in a task of its own, so a caller who goes away does not stop a
/// job between its turn and its end, and the next job's turn comes when
/// the work ends, however it ends. A free turn is taken at once: the
/// queue hands a turn that comes free to its first waiter, so no write
/// already waiting is passed.
pub async fn write<T, F>(lake: &Arc<Lake>, work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&Turn<'_>) -> Result<T> + Send + 'static,
{
    let (lake, turns) = (lake.clone(), lake.turns.clone());
    let at = move |turn| {
        run(move || {
            let _turn = turn;
            work(&Turn(&lake))
        })
    };
    match turns.clone().try_lock_owned() {
        Ok(turn) => at(turn).await,
        Err(_) => {
            let job = tokio::spawn(async move { at(turns.lock_owned().await).await });
            job.await.unwrap_or_else(|e| Err(stopped(&e)))
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
    let detail = Detail::scrubbed(if e.is_panic() {
        "it panicked"
    } else {
        "it was cancelled"
    });
    Error::unavailable(Situation::Stopped { detail })
}

#[cfg(test)]
mod tests;
