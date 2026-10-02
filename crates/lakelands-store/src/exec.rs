//! The one place engine work leaves the async caller.
//!
//! Engines are synchronous: they do blocking storage work
//! (`docs/decisions/019-the-contract-is-object-safe.md`). Every call the
//! store makes into an engine — a read, a write, a fold — runs inside
//! [`run`], on the runtime's blocking pool, so an async caller's executor
//! never waits on storage. There is no other hand-off in this crate: no
//! nested runtime, no thread that parks the caller.

use lakelands_api::{Error, Result};

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
        Err(e) => Err(Error::Unavailable {
            message: format!("the store's work on this lake stopped unexpectedly: {e}"),
        }),
    }
}
