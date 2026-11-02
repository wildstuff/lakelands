//! The log, as the store writes and reads it (`docs/log-format.md`).
//!
//! The store is the only party that turns an event into bytes: it
//! canonicalises the event without its `hash`, hashes those bytes, links
//! the event to its predecessor, and hands the engine the canonical bytes
//! of the whole event (`docs/decisions/020-events-are-hash-chained-over-canonical-bytes.md`).
//! It writes each append in the log's stored form
//! (`docs/log-format.md` §1.2, §1.5): blocks where it is two or more
//! events, each compressed with its lines framed and the members the
//! block decides left out, and `position` and `prev` left to the event
//! before but on a segment's first line
//! ([`lakelands_core::log::frame_append`]). Reading back, it restores the
//! event from its line, its block and the members the engine keeps beside
//! them, recomputes the hash and checks the link. A block is inflated whole
//! and kept while its events are read ([`lakelands_core::log::Restorer`]),
//! and the line the engine hands beside it must be the block's, so an
//! engine that altered a byte, of a line or anywhere in a block, is caught
//! at the first read of an event it restores.

use lakelands_api::engine::CanonicalEvent;
use lakelands_api::error::{Case, Detail, Flaw, Part};
use lakelands_api::event::{Digest, Event, EventBody, EventId, LOG_FORMAT_VERSION, Procedure};
use lakelands_api::principal::WriteContext;
use lakelands_api::record::Origin;
use lakelands_api::{Error, IdempotencyKey, LakeId, Position, Result, SourceId, Timestamp};
use lakelands_core::canonical;
use lakelands_core::log::Restorer;

/// Everything about an event the writer decides; the store fills in the
/// rest (format version, lake, position, id, link, fence, hash).
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    /// The lake's wall time.
    pub known_at: Timestamp,
    /// `<kind>:<id>`.
    pub writer: String,
    /// Which authority wrote it.
    pub origin: Origin,
    /// The source, for an event from one.
    pub source: Option<SourceId>,
    /// When the source observed the values.
    pub seen_at: Option<Timestamp>,
    /// Where the values were read from, for values from another lake's
    /// log.
    pub upstream: Option<lakelands_api::Upstream>,
    /// The procedure that produced the values.
    pub procedure: Option<Procedure>,
    /// The host's context.
    pub context: WriteContext,
    /// The event this one follows from.
    pub cause: Option<Position>,
    /// The caller's idempotency key.
    pub idempotency_key: Option<IdempotencyKey>,
    /// Chosen id, for content-derived and keyed events; positional when
    /// absent.
    pub id: Option<EventId>,
    /// The delivery a source's record came in, on a `record_ingested`.
    pub delivery: Option<lakelands_api::DeliveryId>,
    /// What the source's key seals of the body, which the draft holds in
    /// plaintext: the append seals it, and the fold opens it.
    pub sealed: Option<lakelands_api::event::Sealed>,
    /// The kind and payload.
    pub body: EventBody,
}

impl Draft {
    /// A system event: written by the store itself.
    pub fn system(known_at: Timestamp, body: EventBody) -> Draft {
        Draft {
            upstream: None,
            known_at,
            writer: crate::record::STORE_WRITER.into(),
            origin: Origin::System,
            source: None,
            seen_at: None,
            procedure: None,
            context: WriteContext::default(),
            cause: None,
            idempotency_key: None,
            id: None,
            delivery: None,
            sealed: None,
            body,
        }
    }
}

pub use lakelands_core::log::{
    accepted_id, aging_id, authored_id, boxed_ingest_id, ingest_id, output_id, positional_id,
    quarantined_id,
};

/// The hash of an event's line as it was written, without its `hash`.
fn hash_without_hash(mut value: serde_json::Value) -> Digest {
    if let Some(members) = value.as_object_mut() {
        members.remove("hash");
    }
    Digest(canonical::sha256_hex(&value))
}

/// Seal a draft at a position, after `prev`, under a lease's fence, in
/// the release's one format version.
pub fn seal(
    lake: &LakeId,
    position: Position,
    prev: &Digest,
    fence: u64,
    d: Draft,
) -> (Event, CanonicalEvent) {
    let (event, stored, _) = seal_judged(lake, position, prev, fence, d);
    (event, stored)
}

/// [`seal`], with what the log's own reading would not pass in the
/// event, if anything (`docs/log-format.md` §1.2, §1.3): a number its
/// bytes write as an integer beyond ±(2^53 − 1), anywhere in it, or a
/// `known_at` or `seen_at` out of the years the one form writes.
pub(crate) fn seal_judged(
    lake: &LakeId,
    position: Position,
    prev: &Digest,
    fence: u64,
    d: Draft,
) -> (Event, CanonicalEvent, Option<Case>) {
    let mut event = unsealed(lake, position, prev, fence, d);
    // The event's value is built once: its members written once, hashed
    // without `hash`, judged, and stored with it.
    let mut value = serde_json::to_value(&event).expect("an event serialises to JSON");
    let members = value.as_object_mut().expect("an event is an object");
    members.remove("hash");
    let (hash, bytes) = canonical::sealed(members, "hash");
    event.hash = Digest(hash);
    let out_of_years = [Some(event.known_at), event.seen_at]
        .into_iter()
        .flatten()
        .find(|t| !t.in_years());
    let fault = match canonical::integer_too_large(&value) {
        Some(n) => Some(Case::IntegerTooLarge { n: n.clone() }),
        None => out_of_years.map(|t| Case::NotDateTime {
            value: t.to_string(),
        }),
    };
    let stored = CanonicalEvent {
        position,
        id: event.id.clone(),
        prev: event.prev.clone(),
        hash: event.hash.clone(),
        fence,
        bytes,
        frame: None,
    };
    (event, stored, fault)
}

/// The event `d` makes at `position`, not yet sealed: no hash, no bytes.
/// What computes a record from an event reads its envelope and body, never
/// its hash, so a plan builds this and the log seals it once.
pub(crate) fn unsealed(
    lake: &LakeId,
    position: Position,
    prev: &Digest,
    fence: u64,
    d: Draft,
) -> Event {
    let id = d.id.unwrap_or_else(|| positional_id(lake, position));
    let mut context = d.context;
    // Whom the write is for goes with its writer, in the envelope: an
    // event the store writes as itself names none, and `context` never
    // carries it. Nor does such an event carry the host's occasion (088).
    let own = d.writer == crate::record::STORE_WRITER;
    let on_behalf_of = (context.on_behalf_of.take()).filter(|_| !own);
    context.occasion = context.occasion.filter(|_| !own);
    Event {
        upstream: d.upstream,
        v: LOG_FORMAT_VERSION,
        lake: lake.clone(),
        position,
        id,
        prev: prev.clone(),
        known_at: d.known_at,
        writer: d.writer,
        on_behalf_of,
        origin: d.origin,
        source: d.source,
        seen_at: d.seen_at,
        procedure: d.procedure,
        // The host's cause lands in the envelope of an event the store set
        // no cause for, and never in `context` (021).
        cause: d.cause.or(context.cause.take()),
        context,
        fence,
        idempotency_key: d.idempotency_key,
        delivery: d.delivery,
        sealed: d.sealed,
        body: d.body,
        hash: Digest(String::new()),
    }
}

/// The format version a stored event's line says, 0 where it says none.
fn format_of(value: &serde_json::Value) -> u64 {
    value
        .get("v")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
}

/// Whether a lake is written in a format this release reads, judged at
/// its two ends before the lake is opened; a lake this release does not
/// read is refused by both numbers: it is another release's, not a damaged
/// one, and it is not opened. `ends` are the log's first and last event
/// ([`EngineOpener::log_ends`](lakelands_api::engine::EngineOpener::log_ends)).
/// Three numbers are judged, and the first this release does not read is
/// named: the first event's `v`, the format its `lake_created` says, and
/// the last event's `v`. An end that is no JSON is left to [`open`], which
/// says what is wrong with it.
pub fn one_format(lake: &LakeId, ends: Option<&(CanonicalEvent, CanonicalEvent)>) -> Result<()> {
    let Some((first, tip)) = ends else {
        return Ok(());
    };
    // Every line holds its `v`; a `lake_created` may leave members to its
    // frame.
    let read = |e: &CanonicalEvent| {
        let line = || serde_json::from_slice::<serde_json::Value>(&e.bytes).ok();
        lakelands_core::log::restore(e)
            .ok()
            .map(|(e, _)| e)
            .or_else(line)
    };
    let (first, tip) = (read(first), read(tip));
    let made = |f: &&serde_json::Value| f.pointer("/body/kind") == Some(&"lake_created".into());
    let created = (first.as_ref().filter(made)).and_then(|f| f.pointer("/body/format"));
    let said = [
        first.as_ref().map(format_of),
        created.map(|f| f.as_u64().unwrap_or(0)),
        tip.as_ref().map(format_of),
    ];
    let unread = said
        .into_iter()
        .flatten()
        .find(|v| !lakelands_core::log::reads_format(*v));
    let Some(found) = unread else { return Ok(()) };
    let situation = lakelands_api::error::Situation::LogFormat {
        lake: lake.to_string(),
        found,
        reads: LOG_FORMAT_VERSION.into(),
        oldest: Some(lakelands_api::event::OLDEST_LOG_FORMAT.into()),
    };
    Err(Error::unavailable(situation))
}

fn corrupt(position: Position, flaw: Flaw) -> Error {
    lakelands_core::damaged(Part::Log, Some(position), flaw)
}

/// Restore a stored event and verify it: its line and its block are
/// canonical and restore it (`lakelands_core::log::restore`), the hash is
/// the hash of the restored event without it, and the members the engine
/// keeps beside the line agree with it. An event in another format
/// version than the release's one is refused before anything else of it
/// is read (`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`).
pub fn open(stored: &CanonicalEvent) -> Result<Event> {
    open_with(&mut Restorer::default(), stored)
}

/// [`open`], the event restored by `restorer`, which keeps the last block
/// it restored one from: what a reader of more than one event opens them
/// with.
pub fn open_with(restorer: &mut Restorer, stored: &CanonicalEvent) -> Result<Event> {
    let at = stored.position;
    let (value, whole) = restorer.restore(stored).map_err(|f| corrupt(at, f))?;
    let hash = hash_without_hash(value.clone());
    // Where the line is the whole event its bytes are the stored ones,
    // already checked: no second pass.
    let bytes = if whole {
        stored.bytes.clone()
    } else {
        canonical::to_vec(&value)
    };
    let event: Event = serde_json::from_value(value).map_err(|e| {
        let detail = Detail::json(&e);
        corrupt(at, Flaw::NotAnEvent { detail })
    })?;
    // Members read leniently (a date-time in another form) would print
    // differently: the event must re-serialise to exactly its bytes.
    if canonical::of(&event) != bytes {
        return Err(corrupt(at, Flaw::NotOneForm));
    }
    // A member where it never sits is refused, never read; and so is a
    // `lake_created` that says another format than its line.
    let flaw = lakelands_core::log::misplaced(&event);
    if let Some(flaw) = flaw.or_else(|| lakelands_core::log::created_in_another(&event)) {
        return Err(corrupt(at, flaw));
    }
    if hash != event.hash {
        return Err(corrupt(at, Flaw::HashMismatch));
    }
    if event.position != stored.position
        || event.id != stored.id
        || event.prev != stored.prev
        || event.hash != stored.hash
        || event.fence != stored.fence
    {
        return Err(corrupt(at, Flaw::Disagrees));
    }
    Ok(event)
}

/// Check that events follow each other: consecutive positions, each
/// naming the hash of the one before.
pub fn check_link(before: Option<(&Position, &Digest)>, event: &Event) -> Result<()> {
    let (want_pos, want_prev) = match before {
        Some((p, h)) => (p.next(), h.clone()),
        None => (Position::GENESIS, Digest::zero()),
    };
    let at = event.position;
    if event.position != want_pos {
        let (expected, found) = (want_pos, None);
        return Err(corrupt(at, Flaw::Position { expected, found }));
    }
    if event.prev != want_prev {
        return Err(corrupt(at, Flaw::PrevHash));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
