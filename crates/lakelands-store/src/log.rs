//! The log, as the store writes and reads it (`docs/log-format.md`).
//!
//! The store is the only party that turns an event into bytes: it
//! canonicalises the event without its `hash`, hashes those bytes, links
//! the event to its predecessor, and hands the engine the canonical bytes
//! of the whole event (`docs/decisions/020-events-are-hash-chained-over-canonical-bytes.md`).
//! Reading back, it parses the bytes, recomputes the hash and checks the
//! link, so an engine that altered a byte is caught at the first read.

use lakelands_api::engine::CanonicalEvent;
use lakelands_api::error::{Case, Detail, Flaw, Part};
use lakelands_api::event::{
    Digest, Event, EventBody, EventId, LOG_FORMAT_BASE, LOG_FORMAT_VERSION, Procedure,
};
use lakelands_api::principal::WriteContext;
use lakelands_api::record::Origin;
use lakelands_api::{Error, IdempotencyKey, LakeId, Position, Result, SourceId, Timestamp};
use lakelands_core::canonical;

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
    /// log (format version 3).
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
    /// The kind and payload.
    pub body: EventBody,
}

impl Draft {
    /// A system event: written by the store itself.
    pub fn system(known_at: Timestamp, body: EventBody) -> Draft {
        Draft {
            upstream: None,
            known_at,
            writer: "system:lakelands".into(),
            origin: Origin::System,
            source: None,
            seen_at: None,
            procedure: None,
            context: WriteContext::default(),
            cause: None,
            idempotency_key: None,
            id: None,
            body,
        }
    }
}

pub use lakelands_core::log::{
    accepted_id, aging_id, authored_id, ingest_id, output_id, positional_id,
};

fn hash_without_hash(event: &Event) -> Digest {
    let mut value = serde_json::to_value(event).expect("an event serialises to JSON");
    value
        .as_object_mut()
        .expect("an event is an object")
        .remove("hash");
    Digest(canonical::sha256_hex(&value))
}

/// Seal a draft at a position, after `prev`, under a lease's fence, in
/// the lake's first format version.
pub fn seal(
    lake: &LakeId,
    position: Position,
    prev: &Digest,
    fence: u64,
    d: Draft,
) -> (Event, CanonicalEvent) {
    seal_at(LOG_FORMAT_BASE, lake, position, prev, fence, d)
}

/// Seal a draft in format version `v`.
pub fn seal_at(
    v: u32,
    lake: &LakeId,
    position: Position,
    prev: &Digest,
    fence: u64,
    d: Draft,
) -> (Event, CanonicalEvent) {
    let (event, stored, _) = seal_judged(v, lake, position, prev, fence, d);
    (event, stored)
}

/// [`seal_at`], with what the log's own reading would not pass in the
/// event, if anything (`docs/log-format.md` §1.2, §1.3): a number its
/// bytes write as an integer beyond ±(2^53 − 1), anywhere in it, or a
/// `known_at` or `seen_at` out of the years the one form writes.
pub(crate) fn seal_judged(
    v: u32,
    lake: &LakeId,
    position: Position,
    prev: &Digest,
    fence: u64,
    d: Draft,
) -> (Event, CanonicalEvent, Option<Case>) {
    let id = d.id.unwrap_or_else(|| positional_id(lake, position));
    let mut event = Event {
        upstream: d.upstream,
        v,
        lake: lake.clone(),
        position,
        id,
        prev: prev.clone(),
        known_at: d.known_at,
        writer: d.writer,
        origin: d.origin,
        source: d.source,
        seen_at: d.seen_at,
        procedure: d.procedure,
        // The host's cause lands in the envelope of an event the store set
        // no cause for, and never in `context` (021).
        context: WriteContext {
            cause: None,
            ..d.context
        },
        cause: d.cause.or(d.context.cause),
        fence,
        idempotency_key: d.idempotency_key,
        body: d.body,
        hash: Digest(String::new()),
    };
    // The event's value is built once: hashed without `hash`, judged, and
    // written with it.
    let mut value = serde_json::to_value(&event).expect("an event serialises to JSON");
    let members = value.as_object_mut().expect("an event is an object");
    members.remove("hash");
    event.hash = Digest(canonical::sha256_hex(&value));
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
    let hash = serde_json::Value::String(event.hash.0.clone());
    let members = value.as_object_mut().expect("an event is an object");
    members.insert("hash".to_owned(), hash);
    let stored = CanonicalEvent {
        position,
        id: event.id.clone(),
        prev: event.prev.clone(),
        hash: event.hash.clone(),
        fence,
        bytes: canonical::to_vec(&value),
    };
    (event, stored, fault)
}

fn corrupt(position: Position, flaw: Flaw) -> Error {
    lakelands_core::damaged(Part::Log, Some(position), flaw)
}

/// Parse a stored event and verify it: the bytes are canonical, the hash
/// is the hash of the event without it, and the members the engine keeps
/// beside the bytes agree with them. A reader refuses a format version it
/// does not know rather than guessing.
pub fn open(stored: &CanonicalEvent) -> Result<Event> {
    let at = stored.position;
    let value: serde_json::Value = serde_json::from_slice(&stored.bytes).map_err(|e| {
        let detail = Detail::json(&e);
        corrupt(at, Flaw::NotJson { detail })
    })?;
    let v = value
        .get("v")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if v > u64::from(LOG_FORMAT_VERSION) {
        let (lowest, highest) = (0, LOG_FORMAT_VERSION.into());
        return Err(corrupt(at, Flaw::FormatOutside { v, lowest, highest }));
    }
    if canonical::to_vec(&value) != stored.bytes {
        return Err(corrupt(at, Flaw::NotCanonical));
    }
    let event: Event = serde_json::from_value(value).map_err(|e| {
        let detail = Detail::json(&e);
        corrupt(at, Flaw::NotAnEvent { detail })
    })?;
    // Members read leniently (a date-time in another form) would print
    // differently: the event must re-serialise to exactly its bytes.
    if canonical::of(&event) != stored.bytes {
        return Err(corrupt(at, Flaw::NotOneForm));
    }
    // A member or a call a version does not have is refused, never read.
    if let Some(flaw) = lakelands_core::versions::below(&event)
        .or_else(|| lakelands_core::versions::misplaced(&event))
    {
        return Err(corrupt(at, flaw));
    }
    if hash_without_hash(&event) != event.hash {
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
