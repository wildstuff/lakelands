//! One feed read per lake and filter, shared by the server's feed
//! consumers (lakelands#1113).
//!
//! Every SSE stream and every MCP pump follows the lake's change feed. A
//! [`Hub`] reads it once for all the consumers of one lake that follow the
//! same kinds and types, and hands each batch to every one of them, so the
//! writer pays one read per append for each distinct filter, never one per
//! consumer. A stream's record keys are applied to the hub's batch by the
//! stream itself: a thousand streams on a thousand keys share one hub.
//!
//! **Joining.** A [`Follow`] subscribes to its hub first, then reads the
//! feed alone from its own position until a batch reaches the head, and
//! only then takes the hub's batches, passing over by position what it
//! already read. Each hub batch names where its read began, so a follower
//! that would miss a position reads alone instead. A follower whose client
//! is slow falls behind the hub's bounded channel ([`BACKLOG`] batches),
//! and reads alone from its own position again: the hub never waits for
//! a consumer. The batches a follower reads are those its own reads would
//! have answered: what it sends, and the `: next` it tells, are a solo
//! reader's; only when they arrive may differ.
//!
//! **What is shared is the log.** The hub reads the feed with no
//! principal, as each consumer did before it. An SSE stream is only for a
//! reader who reads everything (037), and a pump shows each event only as
//! its own principal may see it, through its own reads.
//!
//! **Pacing.** The hub waits for the next append past the head, the pause
//! bounding the wait, and reads once the floor has passed since the last
//! read an append woke (lakelands#1061). A follower's own reads behind the
//! head follow one another at once, as before; one after it had caught up
//! waits out the floor from that read.
//!
//! **Lifetime.** A hub lives while a follower holds it; its task ends at
//! its next wake after the last one goes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use bytes::Bytes;
use lakelands_api::event::{Event, EventBody};
use lakelands_api::id::{Position, RecordKey, TypeName};
use lakelands_api::seam::{ChangeFeed, FeedBatch, FeedFilter};
use lakelands_api::{LakeStore, Result};
use tokio::sync::broadcast;
use tokio::time::Instant;

/// How many batches a hub holds for a follower that has not taken them.
pub const BACKLOG: usize = 64;

/// How many events a hub reads at once.
const READ: u32 = 256;

/// One batch of the feed as a read answered it, with where that read
/// began, and its events written once as SSE frames for every stream that
/// sends them all.
#[derive(Debug)]
pub struct Batch {
    /// The position the read began at.
    pub from: Position,
    /// What it answered.
    pub batch: FeedBatch,
    /// The SSE frames of `batch`'s events, made at their first use.
    frames: OnceLock<Vec<Bytes>>,
    /// Whether the hub's read failed: a follower reads alone, and is told
    /// the failure by its own read, as before the hub.
    failed: bool,
}

impl Batch {
    /// What a read from `from` answered.
    fn of(from: Position, batch: FeedBatch) -> Arc<Batch> {
        Arc::new(Batch {
            from,
            batch,
            frames: OnceLock::new(),
            failed: false,
        })
    }

    /// A hub's read from `from` that failed.
    fn failed(from: Position) -> Arc<Batch> {
        let batch = FeedBatch {
            events: Vec::new(),
            next: from,
            head: from,
            overruled: Vec::new(),
            appends: Vec::new(),
        };
        Arc::new(Batch {
            from,
            batch,
            frames: OnceLock::new(),
            failed: true,
        })
    }

    /// The events' frames, as `frame` writes one, told whether the batch
    /// names it overruled and where its append ends.
    pub fn frames(&self, frame: fn(&Event, bool, Option<Position>) -> Bytes) -> &[Bytes] {
        let overruled = |e: &Event| self.batch.overruled.binary_search(&e.position).is_ok();
        self.frames.get_or_init(|| {
            self.batch
                .events
                .iter()
                .map(|e| frame(e, overruled(e), self.batch.append_of(e.position)))
                .collect()
        })
    }
}

/// The feed of one lake, for one filter's kinds and types, read once.
struct Hub {
    tx: broadcast::Sender<Arc<Batch>>,
    /// Its lake, held while the hub lives, so that no other store takes
    /// its address and with it this hub's key: a hub serves one lake only.
    _store: Arc<dyn LakeStore>,
}

/// A hub's lake, by its store's handle, and the kinds and types it reads.
type Key = (usize, Vec<String>, Vec<TypeName>);

/// The hubs that live, each by its key; one no follower holds is gone.
fn hubs() -> &'static Mutex<HashMap<Key, Weak<Hub>>> {
    static HUBS: OnceLock<Mutex<HashMap<Key, Weak<Hub>>>> = OnceLock::new();
    HUBS.get_or_init(Default::default)
}

/// The hub of `store` for `kinds` and `types`, started at the head when it
/// is new.
async fn hub(
    store: &Arc<dyn LakeStore>,
    mut shared: FeedFilter,
    pace: (Duration, Duration),
) -> Result<Arc<Hub>> {
    shared.kinds.sort();
    shared.kinds.dedup();
    shared.types.sort();
    shared.types.dedup();
    shared.keys.clear();
    let handle = Arc::as_ptr(store).cast::<()>() as usize;
    let key = (handle, shared.kinds.clone(), shared.types.clone());
    if let Some(h) = lock(hubs()).get(&key).and_then(Weak::upgrade) {
        return Ok(h);
    }
    let start = store.head().await?.next();
    let mut map = lock(hubs());
    if let Some(h) = map.get(&key).and_then(Weak::upgrade) {
        return Ok(h);
    }
    map.retain(|_, h| h.strong_count() > 0);
    let hub = Arc::new(Hub {
        tx: broadcast::channel(BACKLOG).0,
        _store: store.clone(),
    });
    map.insert(key, Arc::downgrade(&hub));
    drop(map);
    tokio::spawn(read(
        Arc::downgrade(&hub),
        store.feed(),
        shared,
        start,
        pace,
    ));
    Ok(hub)
}

/// The map's lock, whatever a thread that held it last did.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// A hub's reading: a batch at a time from `at`, each handed to every
/// follower, until none holds the hub.
async fn read(
    hub: Weak<Hub>,
    feed: Arc<dyn ChangeFeed>,
    filter: FeedFilter,
    mut at: Position,
    (pause, floor): (Duration, Duration),
) {
    // Whether the next read follows a wake by an append, and when the last
    // such read ended: the floor counts from it (lakelands#1061).
    let (mut woken, mut paced) = (false, None);
    loop {
        let Some(h) = hub.upgrade() else { return };
        let batch = match feed.poll(at, &filter, READ).await {
            Ok(b) => b,
            Err(_) => {
                // Each follower reads alone, and its own read tells it.
                let _ = h.tx.send(Batch::failed(at));
                drop(h);
                tokio::time::sleep(pause).await;
                continue;
            }
        };
        if woken {
            paced = Some(Instant::now());
        }
        let caught_up = caught_up(&batch, at);
        let head = batch.head;
        let from = std::mem::replace(&mut at, batch.next);
        // No follower takes it now: each reads alone until it joins.
        let _ = h.tx.send(Batch::of(from, batch));
        drop(h);
        if caught_up {
            woken = match at > head {
                true => (feed.wait(head, pause).await).is_ok_and(|h| h > head),
                false => {
                    tokio::time::sleep(pause).await;
                    false
                }
            };
            if let (true, Some(p)) = (woken, paced) {
                tokio::time::sleep_until(p + floor).await;
            }
        }
    }
}

/// One consumer's reading of the feed through its hub.
pub struct Follow {
    feed: Arc<dyn ChangeFeed>,
    filter: FeedFilter,
    hub: Arc<Hub>,
    rx: broadcast::Receiver<Arc<Batch>>,
    /// Whether it takes the hub's batches, its own reads having reached
    /// the head.
    joined: bool,
    floor: Duration,
    /// When its last read alone that reached the head ended.
    caught: Option<Instant>,
    /// Batches taken from the hub, and reads made alone.
    pub reads: (u64, u64),
}

/// What [`Follow::next`] answers.
pub enum Next {
    /// A batch; the events at `at` and after are new, and of them those
    /// [`Follow::admits`] admits are the follower's.
    Batch(Arc<Batch>),
    /// Nothing new within the time asked.
    Quiet,
}

impl Follow {
    /// A follower of `store`'s feed through `filter`: its kinds and types
    /// choose the hub; its keys are applied by the follower.
    pub async fn new(
        store: &Arc<dyn LakeStore>,
        filter: FeedFilter,
        pace: (Duration, Duration),
    ) -> Result<Follow> {
        let hub = hub(store, filter.clone(), pace).await?;
        let rx = hub.tx.subscribe();
        Ok(Follow {
            feed: store.feed(),
            filter,
            hub,
            rx,
            joined: false,
            floor: pace.1,
            caught: None,
            reads: (0, 0),
        })
    }

    /// The next batch from `at`: a read of its own while it is behind the
    /// head, then the hub's; `Quiet` when nothing came within `within`.
    pub async fn next(&mut self, at: Position, within: Duration) -> Result<Next> {
        // One bound for the call: a quiet hub's batches, which move nothing,
        // are passed over without lengthening it.
        let until = Instant::now() + within;
        loop {
            if self.joined {
                match tokio::time::timeout_at(until, self.rx.recv()).await {
                    Err(_) => return Ok(Next::Quiet),
                    Ok(Ok(b)) if b.failed => self.joined = false,
                    Ok(Ok(b)) if b.batch.next <= at => continue,
                    Ok(Ok(b)) if b.from <= at => {
                        self.reads.0 += 1;
                        return Ok(Next::Batch(b));
                    }
                    // A gap: what lies between is read alone.
                    Ok(Ok(_)) => self.joined = false,
                    // Behind the hub's batches (it holds the hub, so the
                    // channel never closes): read alone from where it is.
                    Ok(Err(_)) => {
                        self.rx = self.hub.tx.subscribe();
                        self.joined = false;
                    }
                }
                continue;
            }
            if let Some(c) = self.caught {
                tokio::time::sleep_until(c + self.floor).await;
                self.caught = None;
            }
            let batch = self.feed.poll(at, &self.filter, READ).await?;
            self.reads.1 += 1;
            if caught_up(&batch, at) {
                self.joined = true;
                self.caught = Some(Instant::now());
            }
            return Ok(Next::Batch(Batch::of(at, batch)));
        }
    }

    /// Whether an event of a batch is the follower's: of its kinds and
    /// types, which the hub's read applied, and of its keys. A read of its
    /// own applied all of them, which this repeats.
    pub fn admits(&self, e: &Event) -> bool {
        keyed(&self.filter.types, &self.filter.keys, &e.body)
    }
}

/// Whether a batch read from `from` reached the head, so its reader
/// waits before it reads again: a batch with no events may stop short of
/// it, after [`lakelands_api::seam::FEED_EXAMINED`] positions examined.
/// One whose `next` did not move from `from` waits too, rather than read
/// the same place at once again.
pub fn caught_up(batch: &FeedBatch, from: Position) -> bool {
    batch.next > batch.head || batch.next == from
}

/// Whether an event passes a filter's keys, with its types: an event of a
/// record (`record_*`, `trigger_fired`, `trigger_dropped`,
/// `object_set_changed`) of one of
/// `types` (or any type when none) with one of `keys`, or a `verb_called`
/// on one of `keys` when no type is named, as the feed's own filter has it
/// (`lakelands_core::feed::admits`). No keys pass every event.
fn keyed(types: &[TypeName], keys: &[RecordKey], body: &EventBody) -> bool {
    if keys.is_empty() {
        return true;
    }
    let (ty, key) = match body {
        EventBody::RecordUpserted { ty, key, .. }
        | EventBody::RecordIngested { ty, key, .. }
        | EventBody::RecordRetired { ty, key, .. }
        | EventBody::RecordRestricted { ty, key, .. }
        | EventBody::RecordUnrestricted { ty, key, .. }
        | EventBody::TriggerFired { ty, key, .. }
        | EventBody::TriggerDropped { ty, key, .. }
        | EventBody::ObjectSetChanged { ty, key, .. } => (Some(ty), Some(key)),
        EventBody::VerbCalled { key, .. } => (None, key.as_ref()),
        _ => (None, None),
    };
    let ty_ok = types.is_empty() || ty.is_some_and(|t| types.contains(t));
    ty_ok && key.is_some_and(|k| keys.contains(k))
}

#[cfg(test)]
mod tests;
