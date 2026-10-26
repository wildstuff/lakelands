//! What the store hands its engine is the log's stored form
//! (`docs/log-format.md` §1.2).
//!
//! The store builds each event's line, and the frame an append's events
//! share, and the engine keeps both as handed; no door shows the form, so
//! the check reads the log below the store, through the harness's engine
//! opener as a fault wrap is handed it.

use std::sync::Mutex;

use lakelands_api::engine::{EngineOpener, OpenMode};

use crate::conformance::fixture::{self, source_record, type_of};
use crate::conformance::prelude::*;

check!(
    APPEND_FRAMED = an_append_of_events_that_share_members_is_framed_and_one_event_is_not,
    "log-format §1.2: the writer stores an append of two or more events of one format that share members as one frame, its first position the append's first, covering every event of the append, and each event's line; an append of one event is stored as its line alone, with no frame; each restores to an event of its hash",
    given
);

check!(
    CREATION_WHOLE = the_append_that_creates_a_lake_is_stored_whole,
    "log-format §1.2: the append that creates a lake, its `lake_created` and what follows it in one append, is stored whole, with no frame and each event's line its canonical bytes, `position` and `prev` included, so a reader of the format before reads it as lines of its own and names the format the lake is in",
    given
);

/// `store`'s lake opened again through the harness's engine opener, the
/// opener, and the lake's location: the log is read below the store
/// through them.
async fn below(
    store: Store,
    given: &Given,
) -> Result<(Store, Arc<dyn EngineOpener>, lakelands_api::Location), Failure> {
    let kept: Arc<Mutex<Option<Arc<dyn EngineOpener>>>> = Arc::default();
    let keep = kept.clone();
    let wrap: crate::conformance::Wrap = Arc::new(move |inner: Arc<dyn EngineOpener>| {
        *keep.lock().unwrap_or_else(|e| e.into_inner()) = Some(inner.clone());
        inner
    });
    let store = (given.fault)(store, Seams::default(), wrap).await;
    let store = store.map_err(|e| Failure::new(format!("opening the lake again: {e}")))?;
    let opener = kept.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let (Some(opener), Some(location)) = (opener, given.location.clone()) else {
        return Err(Failure::new(
            "the harness opens the lake through its engine opener and names its location",
        ));
    };
    Ok((store, opener, location))
}

async fn the_append_that_creates_a_lake_is_stored_whole(store: Store, given: Given) -> Held {
    // The harness's lake is just created: its head is its creation's last
    // event.
    let made = store.head().await.must("reading the head")?;
    let (_store, opener, location) = below(store, &given).await?;
    let engine = opener.open(&location, OpenMode::Read);
    let engine = engine.map_err(|e| Failure::new(format!("opening the lake to read: {e}")))?;
    let stored = engine.read(Position(0), made.0 as u32 + 1);
    let stored = stored.must("reading the log below the store")?;
    ensure!(
        made.0 >= 1 && stored.len() as u64 == made.0 + 1,
        "the lake's creation appends two or more events, and the log below the store holds \
         {} events of 0 through {made}",
        stored.len()
    );
    for e in &stored {
        let restored = crate::log::restore(e);
        let whole = restored.as_ref().is_ok_and(|(r, whole)| {
            *whole
                && crate::canonical::to_vec(r) == e.bytes
                && crate::engines::hash_of(r.clone()) == e.hash.0
        });
        ensure!(
            e.frame.is_none() && whole,
            "the creation's event at {} is stored with no frame as its canonical bytes, of its \
             hash {}; it is stored as {:?} with the frame {:?}",
            e.position,
            e.hash.0,
            String::from_utf8_lossy(&e.bytes),
            e.frame.as_deref()
        );
    }
    Ok(())
}

async fn an_append_of_events_that_share_members_is_framed_and_one_event_is_not(
    store: Store,
    given: Given,
) -> Held {
    let (store, opener, location) = below(store, &given).await?;
    let l = Lake::of(store.as_ref());
    l.declare(type_decl(
        "reading",
        vec![sourced("amount", StorageKind::Number)],
    ))
    .await?;
    l.declare(type_of("note", &[("text", StorageKind::Text)]))
        .await?;
    let before = store.head().await.must("reading the head")?;
    let records = (0..4)
        .map(|i| source_record("reading", &format!("r{i}"), json!({ "amount": i })))
        .collect();
    let what = "ingesting four readings in one delivery";
    fixture::delivered(store.as_ref(), &l, "meter", records, what).await?;
    let delivered = store.head().await.must("reading the head")?;
    l.upsert("note", "n", json!({ "text": "one" })).await?;
    let head = store.head().await.must("reading the head")?;
    ensure!(
        delivered.0 >= before.0 + 2 && head == delivered.next(),
        "a delivery of four readings appends two or more events, and an upsert of one note one; \
         they appended from {before} through {delivered}, and then through {head}"
    );
    let engine = opener.open(&location, OpenMode::Read);
    let engine = engine.map_err(|e| Failure::new(format!("opening the lake to read: {e}")))?;
    let from = before.next();
    let stored = engine.read(from, (head.0 - before.0) as u32);
    let stored = stored.must("reading the log below the store")?;
    let n = delivered.0 - before.0;
    let (appended, one) = stored.split_at(n as usize);
    let frame = appended.first().and_then(|e| e.frame.clone());
    let framed = frame
        .as_ref()
        .is_some_and(|f| f.first == from && u64::from(f.events) == n)
        && appended.iter().all(|e| e.frame == frame);
    ensure!(
        framed,
        "the delivery's {n} events, appended at once, are stored with one frame from {from} \
         covering them all, and they carry {:?}",
        appended
            .iter()
            .map(|e| e.frame.as_deref())
            .collect::<Vec<_>>()
    );
    ensure!(
        one.len() == 1 && one[0].frame.is_none(),
        "the upsert's one event is stored as its line alone, with no frame, and it is stored as \
         {one:?}"
    );
    for e in &stored {
        let restored = crate::log::restore(e);
        ensure!(
            restored
                .as_ref()
                .is_ok_and(|(r, _)| crate::engines::hash_of(r.clone()) == e.hash.0),
            "the event at {} restores to an event of its hash {}, and it restores to {restored:?}",
            e.position,
            e.hash.0
        );
    }
    Ok(())
}
