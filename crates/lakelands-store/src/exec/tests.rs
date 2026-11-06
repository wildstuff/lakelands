use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use lakelands_api::Location;
use lakelands_api::engine::OpenMode;
use lakelands_api::seam::Seams;
use lakelands_engine_mem::MemoryOpener;

use super::*;

fn lake() -> Arc<Lake> {
    let seams = Seams {
        engines: vec![Arc::new(MemoryOpener::new())],
        ..Seams::default()
    };
    let lake = Lake::open(&Location::memory("turns"), seams, OpenMode::Create);
    Arc::new(lake.expect("the lake opens"))
}

/// A write on `lake`, answered: a turn that never came back would hang
/// every write after it, so the test fails instead of waiting for one.
async fn answered<T, F>(lake: &Arc<Lake>, work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&Turn<'_>) -> Result<T> + Send + 'static,
{
    let job = write(lake, work);
    let answer = tokio::time::timeout(Duration::from_secs(10), job).await;
    answer.expect("the write is answered: its turn came")
}

/// A write whose work panics holding the writer gives its turn back as
/// it unwinds: it is answered `unavailable`, and the next write on the
/// lake takes the writer and is answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_that_panics_gives_its_turn_back() {
    let lake = lake();
    let panics = |t: &Turn<'_>| -> Result<()> {
        let _w = t.writer();
        panic!("the work fails holding the writer")
    };
    let answer = answered(&lake, panics).await;
    assert!(
        matches!(&answer, Err(Error::Unavailable { .. })),
        "a write that panics is answered as the lake being unavailable: {answer:?}"
    );
    let next = answered(&lake, |t| Ok(t.writer().owed.len())).await;
    assert!(next.is_ok(), "the next write is answered: {next:?}");
}

/// A caller that goes away while its write waits for its turn does not
/// take the write with it: the write still runs at its turn, and the
/// write queued after it is answered, after it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_write_whose_caller_went_away_still_runs() {
    let lake = lake();
    let (started, starting) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let first = {
        let lake = lake.clone();
        let holds = move |_: &Turn<'_>| {
            started
                .send(())
                .expect("the test waits for the turn to be held");
            released.recv().expect("the test lets the turn go");
            Ok(())
        };
        tokio::spawn(async move { answered(&lake, holds).await })
    };
    starting.await.expect("the first write holds its turn");

    let ran = Arc::new(AtomicBool::new(false));
    let waits = {
        let ran = ran.clone();
        write(&lake, move |t| {
            let _w = t.writer();
            ran.store(true, Ordering::SeqCst);
            Ok(())
        })
    };
    let gone = tokio::time::timeout(Duration::from_millis(100), waits).await;
    assert!(gone.is_err(), "the write waits behind the held turn");
    release
        .send(())
        .expect("the first write waits to be let go");
    let first = first.await.expect("the first write ends");
    assert!(first.is_ok(), "the first write is answered: {first:?}");

    let next = answered(&lake, |_| Ok(())).await;
    assert!(next.is_ok(), "the write after it is answered: {next:?}");
    assert!(
        ran.load(Ordering::SeqCst),
        "the write whose caller went away ran at its turn, before the next"
    );
}
