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
use lakelands_api::event::{Digest, Event, EventBody, EventId, LOG_FORMAT_VERSION, Procedure};
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
            writer: "system:lakelands".into(),
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
    accepted_id, aging_id, authored_id, ingest_id, output_id, positional_id,
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
    Event {
        upstream: d.upstream,
        v: LOG_FORMAT_VERSION,
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
        delivery: d.delivery,
        sealed: d.sealed,
        body: d.body,
        hash: Digest(String::new()),
    }
}

/// The format version a stored event's line says, where it is JSON.
pub(crate) fn written_format(stored: &CanonicalEvent) -> Option<u64> {
    serde_json::from_slice::<serde_json::Value>(&stored.bytes)
        .ok()
        .map(|v| format_of(&v))
}

/// The format version a stored event's line says, 0 where it says none.
fn format_of(value: &serde_json::Value) -> u64 {
    value
        .get("v")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
}

/// The log format a lake's newest event is written in, judged at its two
/// ends before the lake is opened; a lake this release does not read is
/// refused by both numbers: it is another release's, not a damaged one,
/// and it is not opened. `ends` are the log's first and last event
/// ([`EngineOpener::log_ends`](lakelands_api::engine::EngineOpener::log_ends)).
/// Three numbers are judged, and the first this release does not read is
/// named: the first event's `v`, the format its `lake_created` says, and
/// the last event's `v`; a tip older than the lake's first event is
/// refused as one not read too. An end that is no JSON is left to
/// [`open`], which says what is wrong with it. No log answers the
/// release's own format.
pub fn one_format(lake: &LakeId, ends: Option<&(CanonicalEvent, CanonicalEvent)>) -> Result<u32> {
    let Some((first, tip)) = ends else {
        return Ok(LOG_FORMAT_VERSION);
    };
    let read = |e: &CanonicalEvent| serde_json::from_slice::<serde_json::Value>(&e.bytes).ok();
    let (first, tip) = (read(first), read(tip));
    let made = |f: &&serde_json::Value| f.pointer("/body/kind") == Some(&"lake_created".into());
    let created = (first.as_ref().filter(made)).and_then(|f| f.pointer("/body/format"));
    let (began, newest) = (first.as_ref().map(format_of), tip.as_ref().map(format_of));
    let made_in = created.map(|f| f.as_u64().unwrap_or(0));
    let said = [began, made_in, newest];
    let reads = u64::from(LOG_FORMAT_VERSION);
    // A tip below the first event's format, or a `lake_created` that says
    // another format than its own line, is read in none.
    let other_than_its_line = began.zip(made_in).filter(|(b, m)| b != m).map(|(_, m)| m);
    let fell = began.zip(newest).filter(|(b, n)| n < b).map(|(_, n)| n);
    let fell = other_than_its_line.or(fell);
    let unread = said
        .into_iter()
        .flatten()
        .find(|v| !lakelands_core::log::reads_format(*v));
    match unread.or(fell) {
        Some(found) => {
            let lake = lake.to_string();
            let oldest = Some(lakelands_api::event::OLDEST_LOG_FORMAT.into());
            let situation = lakelands_api::error::Situation::LogFormat {
                lake,
                found,
                reads,
                oldest,
            };
            Err(Error::unavailable(situation))
        }
        None => Ok(newest.map_or(LOG_FORMAT_VERSION, |n| n as u32)),
    }
}

/// Refuses a reader's open of a lake whose newest event is in a format
/// older than the release's: only a writer's open upgrades it, and an
/// older projection is not read with newer code.
pub fn read_as_is(lake: &LakeId, newest: u32) -> Result<()> {
    if newest == LOG_FORMAT_VERSION {
        return Ok(());
    }
    let (lake, found, reads) = (lake.to_string(), newest.into(), LOG_FORMAT_VERSION.into());
    let situation = lakelands_api::error::Situation::LogUpgradeOwed { lake, found, reads };
    Err(Error::unavailable(situation))
}

fn corrupt(position: Position, flaw: Flaw) -> Error {
    lakelands_core::damaged(Part::Log, Some(position), flaw)
}

/// Parse a stored event and verify it: the bytes are canonical, the hash
/// is the hash of the event without it, and the members the engine keeps
/// beside the bytes agree with them. An event in another format version
/// than the release's one is refused before anything else of it is read
/// (`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`).
pub fn open(stored: &CanonicalEvent) -> Result<Event> {
    let at = stored.position;
    let value: serde_json::Value = serde_json::from_slice(&stored.bytes).map_err(|e| {
        let detail = Detail::json(&e);
        corrupt(at, Flaw::NotJson { detail })
    })?;
    let v = format_of(&value);
    if !lakelands_core::log::reads_format(v) {
        let reads = LOG_FORMAT_VERSION.into();
        return Err(corrupt(at, Flaw::FormatOther { v, reads }));
    }
    if canonical::to_vec(&value) != stored.bytes {
        return Err(corrupt(at, Flaw::NotCanonical));
    }
    // An older event is read as the release's format says its members; its
    // bytes and its hash stay as stored.
    let hash = hash_without_hash(value.clone());
    let value = lakelands_core::log::converted(v, value);
    let converted = canonical::to_vec(&value);
    let event: Event = serde_json::from_value(value).map_err(|e| {
        let detail = Detail::json(&e);
        corrupt(at, Flaw::NotAnEvent { detail })
    })?;
    // Members read leniently (a date-time in another form) would print
    // differently: the event must re-serialise to exactly its bytes.
    if canonical::of(&event) != converted {
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
