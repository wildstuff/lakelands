//! Notifications over the change feed: what a session is told without
//! polling.
//!
//! Every session has a [`Watch`] and a [`pump`]. The pump reads the lake's
//! feed as the host does, through `LakeStore::feed` with no principal,
//! never through a door. So the rule that the raw feed over a network door
//! is for principals who read everything
//! (`docs/decisions/037-the-feed-over-a-network-door.md`) is untouched:
//! nothing raw leaves. Each event is shown only as the session's principal
//! could see it then, through the contract's own reads.
//!
//! - **`notifications/tools/list_changed`** goes to every session when the
//!   feed shows a `declared`, `migrated` or `value_type_migrated` event,
//!   whoever caused it: the tools are generated from the ontology the
//!   principal sees, which shows a value type's values as an `enum`.
//! - **`notifications/lakelands/changed`** goes to a subscribed session,
//!   once per event of the kinds it asked for ([`CHANGE_KINDS`] by
//!   default) from its position on, when the principal may `get` the
//!   record at the event's position (at the position before, for a
//!   retirement). It names the position, the kind, the type, the key and,
//!   for a write, the changed fields the principal is shown (masked ones
//!   by name, hidden ones not at all); a write with none left is not
//!   sent. Never a value. The write that made the record (its event says
//!   `made`) changed the fields its patch names and every field of the
//!   type that declares an `initial` value: the fold filled those the
//!   patch left out. Which fields declare one is read from the
//!   principal's own description, so a field it reads masked, whose
//!   `initial` it is not shown, is told by its patch alone. A write of a
//!   setting is told by the record as the principal is told it before
//!   and after (`docs/decisions/063-every-setting-is-in-one-list.md`): a
//!   field only where the value it is told changed; a record it is no
//!   longer told as retired, to a subscription that asks for
//!   retirements; one it is told first as made.
//! - **`trigger_fired`, `trigger_dropped` and `object_set_changed`** say
//!   that a condition held for the record, and a trigger's guard or a
//!   set's filter may read any field of its type. So they are told only
//!   to a principal who reads
//!   every field of the type in full at that position: a condition over a
//!   field it may not read, or reads masked, is never probed this way
//!   (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`).
//!   A condition over `sys.setting` is told only to one from whom nothing
//!   is held, the only principal no setting is withheld from.
//!   One the feed names overruled, decided over a value an act erased
//!   since, is told to no one
//!   (`docs/decisions/065-part-of-a-source-may-be-taken-back.md`).
//!
//! **What a gap says.** The positions a session is told are a subset of
//! the log's, so a gap between two says that something happened the
//! principal may not see, or is not told. That is no more than every
//! read's `position` already says: the lake's head moves for every event.
//!
//! **Retry, never loss.** A read that refuses (the principal may not see
//! the record) hides the event. A read that fails for any other reason
//! (the lake is unavailable for a moment, or its reads have not yet
//! reached the position the feed already shows) hides nothing: the pump
//! waits and reads the same event again, so a transient failure delays a
//! notification and never drops it.
//!
//! **What one change costs.** Each change a session is told costs one
//! read: the record at the event's position, which decides whether the
//! principal may see it then; a setting's write costs one more, the
//! record before it. The fields a type shows the principal,
//! which of them it is shown declare an `initial` value, and whether it
//! reads the type whole are read once per type and kept for the pump's
//! life.
//! A declaration or migration of anything, or a pass that does not go on
//! where the last one ended, drops them: the pump reads the feed in order
//! and never filters declarations out, so no stale view outlives the
//! declaration that changed it. The principal and its grants
//! are fixed for the session the pump belongs to, so the view needs no
//! other invalidation — and must never be shared across sessions.
//!
//! **The queue.** The pump puts notifications on a bounded queue
//! ([`QUEUE`]) and polls the feed only when the queue takes them, so a
//! client that does not read makes the session lag, and nothing is
//! dropped. Each change is tagged with the subscription it was made for
//! ([`Queued`]); a consumer drops one made for a subscription the session
//! has since replaced, resumed or ended, so nothing is told twice or after
//! `unsubscribe`. A tools change is never stale. The session's position is
//! the only state kept (`docs/decisions/004-the-change-feed-is-the-log.md`).
//! What a replaced stream had in flight is told again only to a stream
//! that resumes with `Last-Event-ID`; a tools change a closing stream did
//! not take opens the next stream. The consumer's check of a change's
//! tag and its send are not one step: a change it has just checked when
//! the session subscribes again can still go out, and the pump then tells
//! it again. So at most one change in flight may be told twice; each
//! carries its position, which a client already deduplicates by.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lakelands_api::error::{Error, Refusal};
use lakelands_api::event::{Event, EventBody};
use lakelands_api::id::{Position, RecordKey, TypeName};
use lakelands_api::principal::{Grants, PrincipalKind};
use lakelands_api::read::{Get, ReadAt};
use lakelands_api::seam::{ChangeFeed, FeedFilter, SETTING_TYPE};
use lakelands_api::tools::CHANGE_KINDS;
use lakelands_api::{Catalog, LakeStore, Principal, Record, WriteContext};
use serde_json::{Map, Value, json};
use tokio::sync::{Notify, mpsc};

/// How many notifications wait for a slow client before the pump stops
/// reading the feed.
pub const QUEUE: usize = 256;

/// The method of a change notification.
pub const CHANGED: &str = "notifications/lakelands/changed";

/// The kind a retirement is told as.
const RETIRED: &str = "record_retired";

/// Whether a subscription reads the feed's events of `kind`: those it
/// asked for, and a write where it asked for retirements of settings, as
/// a setting's write may be told as one (063). No source writes a
/// setting.
fn reads_kind(s: &Subscription, kind: &str) -> bool {
    let asked = |k: &str| s.kinds.iter().any(|a| a == k);
    let settings = (s.types.as_ref()).is_none_or(|t| t.iter().any(|t| t.as_str() == SETTING_TYPE));
    asked(kind) || (asked(RETIRED) && settings && kind == "record_upserted")
}

/// One message on a session's queue.
#[derive(Clone, Debug, PartialEq)]
pub struct Queued {
    /// The subscription a change was made for; `None` for a tools
    /// change, which is never stale.
    pub generation: Option<u64>,
    /// The JSON-RPC notification.
    pub message: Value,
}

/// What a session asked to be told of.
#[derive(Clone, Debug, PartialEq)]
struct Subscription {
    kinds: Vec<String>,
    /// Fixed at `subscribe` when given; otherwise every type the principal
    /// may read when the change happens.
    types: Option<Vec<TypeName>>,
    /// Fixed at `subscribe` when given: only changes of records with these
    /// keys, of the types above, or of any type when none are named.
    keys: Option<Vec<RecordKey>>,
}

#[derive(Debug)]
struct Inner {
    subscription: Option<Subscription>,
    /// Moves with every subscribe, unsubscribe and resume, so the pump
    /// starts again from the new position and stale changes are dropped.
    generation: u64,
    /// The next position a subscription is told of.
    next: Position,
    /// The next position whose declarations are told of.
    declared: Position,
    /// The tools changes the pump has queued.
    tools_made: u64,
    /// How many of them the session's last answered `tools/list` covers.
    tools_listed: u64,
}

/// One session's subscription and positions, shared with its pump.
#[derive(Debug)]
pub struct Watch {
    inner: Mutex<Inner>,
    wake: Notify,
}

impl Watch {
    /// A watch that tells of nothing until its pump starts it at the
    /// lake's head, and of changes only after the session subscribes.
    pub fn new() -> Arc<Watch> {
        Arc::new(Watch {
            inner: Mutex::new(Inner {
                subscription: None,
                generation: 0,
                next: Position::GENESIS,
                declared: Position::GENESIS,
                tools_made: 0,
                tools_listed: 0,
            }),
            wake: Notify::new(),
        })
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Tell of declarations after `head` only: what came before is what
    /// the session's first `tools/list` already shows.
    fn start(&self, head: Position) {
        let mut i = self.inner();
        i.declared = i.declared.max(head.next());
        if i.subscription.is_none() {
            i.next = i.next.max(head.next());
        }
    }

    /// Subscribe from `from`, replacing any subscription.
    pub(crate) fn subscribe(
        &self,
        from: Position,
        kinds: Vec<String>,
        types: Option<Vec<TypeName>>,
        keys: Option<Vec<RecordKey>>,
    ) {
        {
            let mut i = self.inner();
            i.subscription = Some(Subscription { kinds, types, keys });
            i.next = from;
            i.generation += 1;
        }
        self.wake.notify_one();
    }

    /// End the subscription; answers the position it had reached.
    pub(crate) fn unsubscribe(&self) -> Position {
        let mut i = self.inner();
        i.subscription = None;
        i.generation += 1;
        i.next
    }

    /// Tell of changes again from `from` on, as a stream that reconnects
    /// after the last event it received asks.
    pub fn resume(&self, from: Position) {
        {
            let mut i = self.inner();
            if i.subscription.is_some() {
                i.next = from;
                i.generation += 1;
            }
        }
        self.wake.notify_one();
    }

    /// Whether a queued message is still to be sent: a tools change
    /// always, a change only while its subscription holds.
    pub fn current(&self, q: &Queued) -> bool {
        q.generation.is_none_or(|g| g == self.inner().generation)
    }

    /// The tools changes queued so far; read before the tools a
    /// `tools/list` answers, so a change made meanwhile stays owed.
    pub(crate) fn tools_made(&self) -> u64 {
        self.inner().tools_made
    }

    /// The session answered `tools/list` with the tools as they stood
    /// after `made` changes: those changes are no longer owed.
    pub(crate) fn listed(&self, made: u64) {
        let mut i = self.inner();
        i.tools_listed = i.tools_listed.max(made);
    }

    /// Whether a tools change was queued that no `tools/list` has
    /// answered since. A tools change has no position to resume from, and
    /// a stream that took it may have closed before its client read it:
    /// only the session listing its tools again shows the change arrived.
    pub fn tools_owed(&self) -> bool {
        let i = self.inner();
        i.tools_made > i.tools_listed
    }

    fn snapshot(&self) -> (u64, Option<Subscription>, Position, Position) {
        let i = self.inner();
        (i.generation, i.subscription.clone(), i.next, i.declared)
    }

    /// Move past `to`, unless the subscription changed meanwhile.
    fn advance(&self, generation: u64, to: Position) {
        let mut i = self.inner();
        if i.generation == generation && i.next < to {
            i.next = to;
        }
        if i.declared < to {
            i.declared = to;
        }
    }

    fn generation(&self) -> u64 {
        self.inner().generation
    }
}

/// How the view answers an event.
enum Seen {
    /// Told, with these params.
    Shown(Value),
    /// Not for this principal.
    Hidden,
    /// The lake could not answer now: read it again later.
    Retry,
}

/// A read's failure: the principal may not see it, or the lake could not
/// answer now. A read at a position past the head the reads have reached
/// is the second: the feed can show an event a moment before the reads
/// do, so the same read succeeds when asked again.
fn failed(e: &Error) -> Seen {
    match e {
        Error::Refused {
            reason: Refusal::OutOfRange { .. },
            ..
        } => Seen::Retry,
        Error::Refused { .. } | Error::NotFound { .. } => Seen::Hidden,
        _ => Seen::Retry,
    }
}

/// Read the feed for one session until its queue is dropped: send
/// `tools/list_changed` on a declaration, and a change notification for
/// every event its subscription shows.
pub async fn pump(
    store: Arc<dyn LakeStore>,
    who: Principal,
    watch: Arc<Watch>,
    queue: mpsc::Sender<Queued>,
    (pause, floor): (Duration, Duration),
) {
    let catalog = store.catalog(WriteContext::default());
    // The head the session starts at; a lake that cannot answer is asked
    // again after the pause.
    loop {
        match store.head().await {
            Ok(head) => {
                watch.start(head);
                break;
            }
            Err(_) => {
                if queue.is_closed() {
                    return;
                }
                let _ = wait(&watch, &queue, pause, None).await;
            }
        }
    }
    let host = host(store.lake());
    let mut view = View::new(catalog.as_ref(), &who, &host);
    // Where the view's reads go on from; `None` after a pass broke off.
    let mut view_at: Option<Position> = None;
    // The pump reads the lake's hub for its kinds (lakelands#1113): a new
    // subscription leaves its hub for its own, joining it as every
    // follower joins.
    let mut follow: Option<(u64, Vec<String>, crate::hub::Follow)> = None;
    'poll: loop {
        if queue.is_closed() {
            return;
        }
        let (generation, subscription, next, declared) = watch.snapshot();
        let mut kinds: Vec<String> = vec![
            "declared".into(),
            "migrated".into(),
            "value_type_migrated".into(),
        ];
        let from = match &subscription {
            Some(s) => {
                let read = CHANGE_KINDS.iter().filter(|k| reads_kind(s, k));
                kinds.extend(read.map(|k| (*k).to_owned()));
                next.min(declared)
            }
            None => declared,
        };
        let filter = FeedFilter {
            kinds,
            types: vec![],
            keys: Vec::new(),
        };
        let same = |(g, k, _): &(u64, Vec<String>, _)| *g == generation && *k == filter.kinds;
        let (_, _, f) = match follow.take().filter(same) {
            Some(held) => follow.insert(held),
            None => {
                let kinds = filter.kinds.clone();
                match crate::hub::Follow::new(&store, filter, (pause, floor)).await {
                    Ok(f) => follow.insert((generation, kinds, f)),
                    Err(_) => {
                        let _ = wait(&watch, &queue, pause, None).await;
                        continue;
                    }
                }
            }
        };
        // It waits on the hub, or for a new subscription, or the queue's end.
        let got = tokio::select! {
            got = f.next(from, pause) => got,
            _ = watch.wake.notified() => continue 'poll,
            _ = queue.closed() => return,
        };
        let shared = match got {
            Ok(crate::hub::Next::Batch(b)) => b,
            Ok(crate::hub::Next::Quiet) => continue,
            Err(_) => {
                // The lake cannot be read now; try again after the pause.
                let _ = wait(&watch, &queue, pause, None).await;
                continue;
            }
        };
        let batch = &shared.batch;
        // The view holds only while the passes read on without a gap.
        if view_at != Some(from) {
            view.forget();
        }
        view_at = None;
        for e in batch.events.iter().filter(|e| e.position >= from) {
            if watch.generation() != generation {
                continue 'poll;
            }
            if matches!(
                e.body.kind(),
                "declared" | "migrated" | "value_type_migrated"
            ) {
                view.forget();
            }
            let queued = match e.body.kind() {
                "declared" | "migrated" | "value_type_migrated" if e.position >= declared => {
                    watch.inner().tools_made += 1;
                    Some(Queued {
                        generation: None,
                        message: json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}),
                    })
                }
                "declared" | "migrated" | "value_type_migrated" => None,
                _ if batch.overruled.binary_search(&e.position).is_ok() => None,
                kind => match &subscription {
                    Some(s) if e.position >= next && reads_kind(s, kind) => {
                        match view.of(e, s).await {
                            Seen::Shown(params) => Some(Queued {
                                generation: Some(generation),
                                message: json!({"jsonrpc": "2.0", "method": CHANGED, "params": params}),
                            }),
                            Seen::Hidden => None,
                            Seen::Retry => {
                                // Not past this event: read it again.
                                view_at = Some(e.position);
                                let _ = wait(&watch, &queue, pause, None).await;
                                continue 'poll;
                            }
                        }
                    }
                    _ => None,
                },
            };
            if let Some(q) = queued
                && queue.send(q).await.is_err()
            {
                return;
            }
            watch.advance(generation, e.position.next());
        }
        let to = batch.next.max(from);
        watch.advance(generation, to);
        view_at = Some(to);
    }
}

/// The lake as its host reads it: every field of every type.
fn host(lake: &lakelands_api::LakeId) -> Principal {
    Principal {
        kind: PrincipalKind::System,
        id: "lakelands".into(),
        lake: lake.clone(),
        grants: Grants::owner(),
    }
}

/// Wait for the pause, or for the lake's next append past `next`'s
/// position, bounded by the pause; for a new subscription; or for the
/// queue's end. Whether an append ended it.
async fn wait(
    watch: &Watch,
    queue: &mpsc::Sender<Queued>,
    pause: Duration,
    next: Option<(&dyn ChangeFeed, Position)>,
) -> bool {
    let next = async {
        match next {
            Some((feed, after)) => (feed.wait(after, pause).await).is_ok_and(|h| h > after),
            None => {
                tokio::time::sleep(pause).await;
                false
            }
        }
    };
    tokio::select! {
        woken = next => woken,
        _ = watch.wake.notified() => false,
        _ = queue.closed() => false,
    }
}

/// Events as one principal may see them, reading through its doors.
struct View<'a> {
    catalog: &'a dyn Catalog,
    who: &'a Principal,
    host: &'a Principal,
    /// The field names shown to the principal, and those of them it is
    /// shown declare an `initial` value, by type, since the last
    /// declaration the pump passed.
    shown: BTreeMap<TypeName, Shown>,
    /// Whether the principal reads every field of a type in full, by type,
    /// since the last declaration the pump passed.
    whole: BTreeMap<TypeName, bool>,
    /// The sets and triggers the principal's description names, since the
    /// last declaration the pump passed.
    listed: Option<BTreeSet<String>>,
}

/// The field names a type shows the principal, and those of them it is
/// shown declare an `initial` value.
#[derive(Clone)]
struct Shown {
    fields: Vec<String>,
    initial: Vec<String>,
}

impl<'a> View<'a> {
    fn new(catalog: &'a dyn Catalog, who: &'a Principal, host: &'a Principal) -> View<'a> {
        View {
            catalog,
            who,
            host,
            shown: BTreeMap::new(),
            whole: BTreeMap::new(),
            listed: None,
        }
    }

    /// Drop what was kept of the types' views.
    fn forget(&mut self) {
        self.shown.clear();
        self.whole.clear();
        self.listed = None;
    }

    /// How the principal may see an event.
    async fn of(&mut self, e: &Event, s: &Subscription) -> Seen {
        let at = e.position;
        let mut extra = Map::new();
        let mut condition = false;
        let mut named = None;
        let mut kind = e.body.kind();
        let mut made = matches!(
            e.body,
            EventBody::RecordUpserted { made: true, .. }
                | EventBody::RecordIngested { made: true, .. }
        );
        // A box holds the members it seals out of the patch (format 20).
        let boxed = (e.sealed.iter())
            .filter(|s| s.boxed.is_some())
            .flat_map(|s| s.fields.iter().map(|f| f.to_string()));
        let (ty, key, mut fields, read_at) = match &e.body {
            EventBody::RecordUpserted { ty, key, patch, .. }
            | EventBody::RecordIngested { ty, key, patch, .. } => {
                let mut named: Vec<_> = patch.keys().map(|k| k.to_string()).chain(boxed).collect();
                named.sort();
                (ty, key, Some(named), at)
            }
            EventBody::RecordRetired { ty, key, .. } => {
                (ty, key, None, Position(at.0.saturating_sub(1)))
            }
            EventBody::OutputRecorded {
                field: Some((ty, key, field)),
                ..
            } => (ty, key, Some(vec![field.to_string()]), at),
            EventBody::TriggerFired {
                trigger, ty, key, ..
            }
            | EventBody::TriggerDropped {
                trigger, ty, key, ..
            } => {
                extra.insert("trigger".into(), json!(trigger));
                condition = true;
                named = Some(format!("trigger {trigger}"));
                (ty, key, None, at)
            }
            EventBody::ObjectSetChanged {
                set,
                ty,
                key,
                joined,
            } => {
                extra.insert("set".into(), json!(set));
                extra.insert("joined".into(), json!(joined));
                condition = true;
                named = Some(format!("set {set}"));
                (ty, key, None, at)
            }
            _ => return Seen::Hidden,
        };
        if let Some(types) = &s.types
            && !types.contains(ty)
        {
            return Seen::Hidden;
        }
        // Filtered here, not in the pump's feed filter, which must go on
        // showing the declarations.
        if let Some(keys) = &s.keys
            && !keys.contains(key)
        {
            return Seen::Hidden;
        }
        let written = matches!(e.body, EventBody::RecordUpserted { .. });
        let setting = written && ty.as_str() == SETTING_TYPE;
        if !setting && !s.kinds.iter().any(|k| k == kind) {
            return Seen::Hidden;
        }
        // The record as the principal could read it then: a record it may
        // not read, or of a type it may not read, is never named.
        let got = self.read(ty, key, read_at).await;
        if !setting && let Err(seen) = got {
            return seen;
        }
        // A setting's write as its principal is told the record before it
        // and after: what it is not told alone is not told, and a record
        // it is no longer told is told as retired (063).
        if setting {
            let was = match at.0.checked_sub(1) {
                Some(p) => self.told(ty, key, Position(p)).await,
                None => Ok(None),
            };
            let (was, now) = match (was, got.map(Some).or_else(untold)) {
                (Ok(was), Ok(now)) => (was, now),
                (Err(seen), _) | (_, Err(seen)) => return seen,
            };
            match (was, now) {
                (None, None) => return Seen::Hidden,
                (Some(_), None) => (kind, fields) = (RETIRED, None),
                // As the write that made it, which names every field it holds.
                (None, Some(now)) => {
                    let held = now.fields.keys().chain(&now.masked);
                    (made, fields) = (true, Some(held.map(|f| f.to_string()).collect()));
                }
                (Some(was), Some(now)) => {
                    let differs = |f: &String| {
                        let f = lakelands_api::FieldName::from(f.as_str());
                        was.fields.get(&f) != now.fields.get(&f)
                    };
                    fields = fields.map(|f| f.into_iter().filter(differs).collect());
                }
            }
            if !s.kinds.iter().any(|k| k == kind) {
                return Seen::Hidden;
            }
        }
        if condition {
            match self.whole(ty, read_at).await {
                Ok(true) => {}
                Ok(false) => return Seen::Hidden,
                Err(seen) => return seen,
            }
        }
        // A set or a trigger its description does not name is not its: one
        // across an edge whose far side is held from it (055, extended).
        if let Some(named) = named {
            match self.listed(read_at).await {
                Ok(l) if l.contains(&named) => {}
                Ok(_) => return Seen::Hidden,
                Err(seen) => return seen,
            }
        }
        let fields = match fields {
            None => None,
            Some(mut changed) => {
                let shown = match self.shown(ty, read_at).await {
                    Ok(s) => s,
                    Err(seen) => return seen,
                };
                // The event that made the record gave each field that
                // declares `initial` its value too, by its patch or by the
                // fill, which no patch names.
                if made {
                    changed.extend(shown.initial);
                    changed.sort();
                    changed.dedup();
                }
                let shown = shown.fields;
                let kept: Vec<String> = changed.into_iter().filter(|f| shown.contains(f)).collect();
                if kept.is_empty() {
                    return Seen::Hidden;
                }
                Some(kept)
            }
        };
        let mut params = Map::from_iter([
            ("lake".to_owned(), json!(e.lake)),
            ("position".to_owned(), json!(at.0)),
            ("kind".to_owned(), json!(kind)),
            ("type".to_owned(), json!(ty)),
            ("key".to_owned(), json!(key)),
        ]);
        if let Some(f) = fields {
            params.insert("fields".into(), json!(f));
        }
        params.extend(extra);
        Seen::Shown(Value::Object(params))
    }

    /// The record `key` of `ty` as the principal may read it at `at`.
    async fn read(&self, ty: &TypeName, key: &RecordKey, at: Position) -> Result<Record, Seen> {
        let get = Get {
            key: key.clone(),
            with_meta: false,
            at: as_of(at),
            params: Default::default(),
        };
        let got = self.catalog.get(self.who, ty, get).await;
        got.map(|r| r.data).map_err(|e| failed(&e))
    }

    /// The record `key` of `ty` as the principal is told it at `at`;
    /// `None` where it is not.
    async fn told(
        &self,
        ty: &TypeName,
        key: &RecordKey,
        at: Position,
    ) -> Result<Option<Record>, Seen> {
        self.read(ty, key, at).await.map(Some).or_else(untold)
    }

    /// The field names of `ty` the principal is shown at `at`: those it
    /// may read, and the masked ones by name; and those of them whose
    /// `initial` value it is shown.
    async fn shown(&mut self, ty: &TypeName, at: Position) -> Result<Shown, Seen> {
        if let Some(s) = self.shown.get(ty) {
            return Ok(s.clone());
        }
        let d = self
            .catalog
            .describe_type(self.who, ty, as_of(at))
            .await
            .map_err(|e| failed(&e))?;
        let appears = d.data.fields.iter().filter(|f| f.access.appears());
        let name = |f: &lakelands_api::describe::FieldDescription| f.name.to_string();
        let s = Shown {
            fields: appears.clone().map(name).collect(),
            initial: appears.filter(|f| f.initial.is_some()).map(name).collect(),
        };
        self.shown.insert(ty.clone(), s.clone());
        Ok(s)
    }

    /// Whether the principal reads every field of `ty` in full at `at`,
    /// as the host sees the type: none hidden, none masked.
    async fn whole(&mut self, ty: &TypeName, at: Position) -> Result<bool, Seen> {
        if let Some(w) = self.whole.get(ty) {
            return Ok(*w);
        }
        // A setting may be withheld from one from whom something is held,
        // so a condition over it is never its (063).
        if ty.as_str() == SETTING_TYPE && !self.who.grants.reads_everything() {
            self.whole.insert(ty.clone(), false);
            return Ok(false);
        }
        let d = self
            .catalog
            .describe_type(self.host, ty, as_of(at))
            .await
            .map_err(|e| failed(&e))?;
        // As the principal's own description shows it, which leaves out a
        // field its grants leave readable but it may not know of, as a
        // reference to a type it is not shown (024, extended).
        let mine = self
            .catalog
            .describe_type(self.who, ty, as_of(at))
            .await
            .map_err(|e| failed(&e))?;
        let reads = |n: &lakelands_api::FieldName| {
            (mine.data.fields.iter()).any(|f| &f.name == n && f.access.reads())
        };
        let w = d.data.fields.iter().all(|f| reads(&f.name));
        self.whole.insert(ty.clone(), w);
        Ok(w)
    }
}

impl View<'_> {
    /// The sets and triggers the principal's description names.
    async fn listed(&mut self, at: Position) -> Result<&BTreeSet<String>, Seen> {
        if self.listed.is_none() {
            let d = (self.catalog.describe(self.who, as_of(at)).await).map_err(|e| failed(&e))?;
            let sets = d.data.object_sets.iter().map(|s| format!("set {}", s.name));
            let triggers = d
                .data
                .triggers
                .iter()
                .map(|t| format!("trigger {}", t.name));
            self.listed = Some(sets.chain(triggers).collect());
        }
        Ok(self.listed.get_or_insert_default())
    }
}

/// A read that hides the record answers none; any other failure is its.
fn untold(seen: Seen) -> Result<Option<Record>, Seen> {
    match seen {
        Seen::Hidden => Ok(None),
        other => Err(other),
    }
}

fn as_of(at: Position) -> ReadAt {
    ReadAt {
        as_of: Some(at),
        valid_at: None,
        clock: None,
    }
}

/// The kinds a subscription takes: every change kind when none is named;
/// the kind named that is not a change kind, when one is.
pub(crate) fn kinds(asked: Option<Vec<String>>) -> Result<Vec<String>, String> {
    match asked {
        None => Ok(CHANGE_KINDS.iter().map(|k| (*k).to_owned()).collect()),
        Some(k) => match k.iter().find(|k| !CHANGE_KINDS.contains(&k.as_str())) {
            Some(bad) => Err(bad.clone()),
            None => Ok(k),
        },
    }
}
