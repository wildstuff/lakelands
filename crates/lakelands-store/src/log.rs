//! The log, as the store writes and reads it (`docs/log-format.md`).
//!
//! The store is the only party that turns an event into bytes: it
//! canonicalises the event without its `hash`, hashes those bytes, links
//! the event to its predecessor, and hands the engine the canonical bytes
//! of the whole event (`docs/decisions/020-events-are-hash-chained-over-canonical-bytes.md`).
//! Reading back, it parses the bytes, recomputes the hash and checks the
//! link, so an engine that altered a byte is caught at the first read.

use lakelands_api::engine::CanonicalEvent;
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

pub use lakelands_core::log::{aging_id, authored_id, ingest_id, positional_id};

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
        context: d.context,
        cause: d.cause,
        fence,
        idempotency_key: d.idempotency_key,
        body: d.body,
        hash: Digest(String::new()),
    };
    event.hash = hash_without_hash(&event);
    let bytes = canonical::of(&event);
    let stored = CanonicalEvent {
        position,
        id: event.id.clone(),
        prev: event.prev.clone(),
        hash: event.hash.clone(),
        fence,
        bytes,
    };
    (event, stored)
}

fn corrupt(position: Position, why: impl std::fmt::Display) -> Error {
    Error::Unavailable {
        message: format!("the log is damaged at position {position}: {why}"),
    }
}

/// Parse a stored event and verify it: the bytes are canonical, the hash
/// is the hash of the event without it, and the members the engine keeps
/// beside the bytes agree with them. A reader refuses a format version it
/// does not know rather than guessing.
pub fn open(stored: &CanonicalEvent) -> Result<Event> {
    let value: serde_json::Value = serde_json::from_slice(&stored.bytes)
        .map_err(|e| corrupt(stored.position, format!("not JSON: {e}")))?;
    let v = value
        .get("v")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if v > u64::from(LOG_FORMAT_VERSION) {
        return Err(corrupt(
            stored.position,
            format!(
                "written in log format {v}, and this reader knows format {LOG_FORMAT_VERSION} at most"
            ),
        ));
    }
    if canonical::to_vec(&value) != stored.bytes {
        return Err(corrupt(stored.position, "the bytes are not canonical"));
    }
    let event: Event = serde_json::from_value(value)
        .map_err(|e| corrupt(stored.position, format!("not an event: {e}")))?;
    // Members read leniently (a date-time in another form) would print
    // differently: the event must re-serialise to exactly its bytes.
    if canonical::of(&event) != stored.bytes {
        return Err(corrupt(
            stored.position,
            "a member is not in its one written form",
        ));
    }
    // A member or a call a version does not have is refused, never read.
    if let Some(why) = lakelands_core::versions::below(&event)
        .or_else(|| lakelands_core::versions::misplaced(&event))
    {
        return Err(corrupt(stored.position, why));
    }
    if hash_without_hash(&event) != event.hash {
        return Err(corrupt(
            stored.position,
            "the hash does not match the event",
        ));
    }
    if event.position != stored.position
        || event.id != stored.id
        || event.prev != stored.prev
        || event.hash != stored.hash
        || event.fence != stored.fence
    {
        return Err(corrupt(
            stored.position,
            "the engine's record of the event disagrees with its bytes",
        ));
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
    if event.position != want_pos {
        return Err(corrupt(
            event.position,
            format!("expected position {want_pos}"),
        ));
    }
    if event.prev != want_prev {
        return Err(corrupt(
            event.position,
            "it does not name the hash of the event before it",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
