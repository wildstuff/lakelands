//! What the store hands its engine is the log's stored form
//! (`docs/log-format.md` §1.2).
//!
//! The store builds each event's line, and the block an append's events
//! share (§1.5), and the engine keeps both as handed; no door shows the form, so
//! the check reads the log below the store, through the harness's engine
//! opener as a fault wrap is handed it.

use std::sync::Mutex;

use lakelands_api::engine::{EngineOpener, OpenMode};

use crate::conformance::fixture::{self, source_record, type_of};
use crate::conformance::prelude::*;

check!(
    APPEND_OF_ONE_STAYS_A_LINE =
        an_append_of_events_that_share_members_is_framed_and_one_event_is_not,
    "098 and log-format §1.2, §1.5: the writer stores an append of two or more events as one block, its first position the append's first, covering every event of the append, and each event's line; an append of one event is stored as its line alone, with no block, holding its `hash` and neither `position` nor `prev`; each restores to an event of its hash",
    given
);

check!(
    CREATION_WHOLE = the_append_that_creates_a_lake_is_stored_whole,
    "log-format §1.2: the append that creates a lake, its `lake_created` and what follows it in one append, is stored whole, with no frame and each event's line its canonical bytes, `position` and `prev` included, so a reader of the format before reads it as lines of its own and names the format the lake is in",
    given
);

check!(
    BLOCK_RESTORES_EVERY_EVENT = an_append_of_more_than_a_block_holds_restores_every_event,
    "098 and log-format §1.5: a delivery of 1 100 records in one window is stored as two blocks, of 1 000 events and of 100, each naming the append's last position as its `end`; each event's line leaves out `hash`, `position`, `prev` and `v`, and leaves out `id` exactly where its event decides it; every event restores to the hash and the id the engine keeps for it",
    given
);

check!(
    FEED_APPENDS_FROM_BLOCKS = the_feed_tells_an_append_from_its_blocks,
    "098 and log-format §1.5: the feed tells each event of an append stored in two blocks the append's last position, each block's `end`, also once the lake is opened again and reads them from its engine, and an event appended alone itself",
    given
);

check!(
    SEAL_SIZED_BY_RESTORED_BYTES = the_active_segment_is_sized_by_its_restored_bytes,
    "098 and 074: an engine that answers the bytes of its active segment answers them as restored, each line's bytes and its line feed and each block's `raw`, never a block's stored bytes, so that `limits.seal_after_bytes` cuts at a size its events take; an engine that answers none is never sealed by size",
    given
);

/// The records of the delivery a block cannot hold whole.
const BIG: u32 = 1_100;

/// A lake with a `reading` type and a `note` type, a delivery of [`BIG`]
/// readings in one window, one append, and then one note alone; the
/// lake's position before the delivery, the delivery's last, and the
/// note's.
pub(super) async fn big(store: &dyn LakeStore) -> Result<(Position, Position, Position), Failure> {
    use lakelands_api::{IngestOpts, ingest_all_with};
    let l = Lake::of(store);
    l.declare(type_decl(
        "reading",
        vec![sourced("amount", StorageKind::Number)],
    ))
    .await?;
    l.declare(type_of("note", &[("text", StorageKind::Text)]))
        .await?;
    let before = store.head().await.must("reading the head")?;
    let records = (0..BIG)
        .map(|i| source_record("reading", &format!("r{i}"), json!({ "amount": i })))
        .collect();
    let opts = IngestOpts {
        commit_window: Some(BIG),
        ..IngestOpts::default()
    };
    let source = lakelands_api::SourceId::from("meter");
    let what = format!("ingesting {BIG} readings in one window");
    ingest_all_with(l.cat.as_ref(), &l.own, &source, opts, records)
        .await
        .must(&what)?;
    let delivered = store.head().await.must("reading the head")?;
    l.upsert("note", "n", json!({ "text": "one" })).await?;
    let head = store.head().await.must("reading the head")?;
    ensure!(
        delivered.0 >= before.0 + u64::from(BIG) && head == delivered.next(),
        "a delivery of {BIG} readings in one window appends {BIG} events or more, and an upsert \
         one; they appended from {before} through {delivered}, and then through {head}"
    );
    Ok((before, delivered, head))
}

/// The log of `engine` from `from` through `to`, read a page at a time.
fn stored_from(
    engine: &dyn lakelands_api::engine::LakeEngine,
    from: Position,
    to: Position,
) -> Result<Vec<lakelands_api::engine::CanonicalEvent>, Failure> {
    let mut out: Vec<lakelands_api::engine::CanonicalEvent> = Vec::new();
    let mut at = from;
    while at <= to {
        let page = engine.read(at, (to.0 + 1 - at.0).min(256) as u32);
        let page = page.must("reading the log below the store")?;
        let Some(last) = page.last() else { break };
        at = last.position.next();
        out.extend(page);
    }
    Ok(out)
}

async fn an_append_of_more_than_a_block_holds_restores_every_event(
    store: Store,
    given: Given,
) -> Held {
    let (store, opener, location) = below(store, &given).await?;
    let (before, delivered, _) = big(store.as_ref()).await?;
    let engine = opener.open(&location, OpenMode::Read);
    let engine = engine.map_err(|e| Failure::new(format!("opening the lake to read: {e}")))?;
    let from = before.next();
    let stored = stored_from(engine.as_ref(), from, delivered)?;
    let mut blocks: Vec<(Position, u32, Option<Position>)> = Vec::new();
    for e in &stored {
        let Some(f) = e.frame.as_ref().filter(|f| f.first == e.position) else {
            continue;
        };
        let end = crate::log::block_of(f).and_then(|b| b.ok()).map(|b| b.end);
        blocks.push((f.first, f.events, end));
    }
    let n = (delivered.0 - before.0) as u32;
    let want = vec![
        (from, 1_000, Some(delivered)),
        (Position(from.0 + 1_000), n - 1_000, Some(delivered)),
    ];
    ensure!(
        stored.iter().all(|e| e.frame.is_some()) && blocks == want,
        "the delivery's {n} events are stored as the blocks {want:?}, by their first position, \
         their events and their `end`, and they are stored as {blocks:?}"
    );
    let mut left_out = 0;
    for e in &stored {
        let line: Value = serde_json::from_slice(&e.bytes)
            .map_err(|err| Failure::new(format!("the line of {} is JSON: {err}", e.position)))?;
        let restored = crate::log::restore(e);
        let Ok((event, _)) = restored else {
            return Err(Failure::new(format!(
                "the event at {} restores, and it is refused as {restored:?}",
                e.position
            )));
        };
        let decided = crate::log::derived_id(&event).as_ref() == Some(&e.id);
        left_out += usize::from(line.get("id").is_none());
        ensure!(
            ["hash", "position", "prev", "v"]
                .iter()
                .all(|m| line.get(*m).is_none())
                && line.get("id").is_none() == decided
                && crate::engines::hash_of(event.clone()) == e.hash.0
                && event["id"] == json!(e.id.0)
                && event["hash"] == json!(e.hash.0),
            "the event at {} is stored as a line leaving out what its block decides, and its \
             `id` where its event decides it, and restores to its hash {} and its id {}; its \
             line is {line} and it restores to {event}",
            e.position,
            e.hash.0,
            e.id.0
        );
    }
    ensure!(
        left_out > 0,
        "an ingested reading's id is one its event decides, and no line of the delivery leaves \
         its `id` out"
    );
    Ok(())
}

async fn the_feed_tells_an_append_from_its_blocks(store: Store, given: Given) -> Held {
    let (before, delivered, head) = big(store.as_ref()).await?;
    let (again, _, _) = below(store.clone(), &given).await?;
    for (s, what) in [(&store, "the lake"), (&again, "the lake opened again")] {
        let ends = super::export::append_ends(s.as_ref()).await?;
        let wrong: Vec<_> = (before.0 + 1..=head.0)
            .map(Position)
            .filter(|p| ends.get(p) != Some(if *p == head { &head } else { &delivered }))
            .collect();
        ensure!(
            wrong.is_empty(),
            "{what}'s feed tells each event of the delivery {delivered}, its blocks' `end`, and \
             the note {head}; it tells otherwise at {:?}",
            wrong
                .iter()
                .take(5)
                .map(|p| (p, ends.get(p)))
                .collect::<Vec<_>>()
        );
    }
    Ok(())
}

async fn the_active_segment_is_sized_by_its_restored_bytes(store: Store, given: Given) -> Held {
    let (store, opener, location) = below(store, &given).await?;
    let (_, _, head) = big(store.as_ref()).await?;
    let engine = opener.open(&location, OpenMode::Read);
    let engine = engine.map_err(|e| Failure::new(format!("opening the lake to read: {e}")))?;
    let Some(bytes) = engine.active_segment_bytes() else {
        return Ok(());
    };
    let segments = engine.segments().must("listing the segments")?;
    let from = segments.last().map_or(Position::GENESIS, |s| s.last.next());
    let (mut raw, mut kept) = (0, 0);
    for e in stored_from(engine.as_ref(), from, head)? {
        match &e.frame {
            Some(f) if f.first == e.position => {
                let block = crate::log::block_of(f).and_then(|b| b.ok());
                let block = block.ok_or_else(|| Failure::new("a stored block names its bytes"))?;
                (raw, kept) = (raw + block.raw, kept + f.bytes.len() as u64 + 1);
            }
            Some(_) => {}
            None => {
                (raw, kept) = (
                    raw + e.bytes.len() as u64 + 1,
                    kept + e.bytes.len() as u64 + 1,
                )
            }
        }
    }
    ensure!(
        bytes == raw && raw > kept,
        "the active segment from {from} through {head} answers {bytes} bytes, its units' as \
         restored, {raw}, more than their stored {kept}"
    );
    Ok(())
}

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
    let line: Option<Value> = one
        .first()
        .and_then(|e| serde_json::from_slice(&e.bytes).ok());
    let line = line.unwrap_or_default();
    ensure!(
        one.len() == 1
            && one[0].frame.is_none()
            && line.get("hash").is_some()
            && line.get("position").is_none()
            && line.get("prev").is_none(),
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
