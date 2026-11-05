//! What the log format derives from content: event ids and record keys
//! (`docs/log-format.md` §2.3 and §3), and the members an event never
//! carries where it is ([`misplaced`]); and the stored form of §1.2, the
//! lines an event is restored from: a frame holding what a run of events
//! shares, and the `position` and `prev` the event before gives. A
//! segment's digest is over the restored events, whatever form holds them.
//!
//! These are pure functions of the values they name, so a writer, a
//! verifier and the published vectors compute them the same way; the
//! store calls them, and so does the vector test. The key rule itself
//! lives in `lakelands_api::key`, where a host reaches it too.

use std::sync::Arc;

use lakelands_api::engine::{CanonicalEvent, Frame};
use lakelands_api::error::{Case, Detail, Error, Flaw, Part, Result, Situation};
use lakelands_api::event::{
    Digest, Event, EventBody, EventId, LOG_FORMAT_VERSION, OLDEST_LOG_FORMAT,
};
use lakelands_api::{
    IdempotencyKey, LakeId, Patch, Position, ProposalId, RecordKey, SourceId, TypeName,
};
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

use crate::canonical;

pub use lakelands_api::key::record_key;

mod block;
pub use block::{
    BLOCK_BYTES, BLOCK_EVENTS, Block, Fault, Restored, Restoring, Unit, block_events, block_of,
    derived_id, inflate, is_block, read_unit,
};
#[doc(hidden)]
pub use lakelands_api::key::{key_component, key_components, keyable};

/// SHA-256 over the parts joined by `0x00`, as an event id: the first 16
/// bytes, in lower-case hex.
fn id_of(parts: &[&[u8]]) -> EventId {
    EventId(canonical::hex(&Sha256::digest(parts.join(&0u8))[..16]))
}

/// The id of an ingested record: its source, type, key, the record's
/// version before the event (0 for a key not yet written) and its patch.
/// A redelivery against the same state has the same id; a value that
/// returns after a change (A, B, A) is a new event.
#[doc(hidden)]
pub fn ingest_id(
    source: &SourceId,
    ty: &TypeName,
    key: &RecordKey,
    version_before: u64,
    patch: &Patch,
) -> EventId {
    let patch = canonical::of(patch);
    let (source, ty, key) = (source.as_str(), ty.as_str(), key.as_str());
    ingest_of(
        [source, ty, key].map(str::as_bytes),
        version_before,
        &patch,
        None,
    )
}

/// An ingested record's id (`docs/log-format.md` §3) from its parts as
/// bytes: its source, type and key, the version before, the patch's
/// canonical bytes and, where its sealed members are in one box, the box
/// as stored. [`ingest_id`], [`boxed_ingest_id`] and [`derived_id`] build
/// it here alone.
pub(crate) fn ingest_of(
    [source, ty, key]: [&[u8]; 3],
    version_before: u64,
    patch: &[u8],
    boxed: Option<&[u8]>,
) -> EventId {
    let before = version_before.to_string();
    let tag: &[u8] = match boxed {
        None => b"lakelands:ingest:v1",
        Some(_) => b"lakelands:ingest:v2",
    };
    let mut parts = vec![tag, source, ty, key, before.as_bytes(), patch];
    parts.extend(boxed);
    id_of(&parts)
}

/// The id of an ingested record whose sealed members are in one box
/// (`docs/log-format.md` §3): [`ingest_id`]'s parts under
/// `lakelands:ingest:v2`, and the box as stored. The same values against
/// the same state seal to the same box, so a redelivery keeps its id.
#[doc(hidden)]
pub fn boxed_ingest_id(
    source: &SourceId,
    ty: &TypeName,
    key: &RecordKey,
    version_before: u64,
    patch: &Patch,
    boxed: &str,
) -> EventId {
    let patch = canonical::of(patch);
    let (source, ty, key) = (source.as_str(), ty.as_str(), key.as_str());
    let parts = [source, ty, key].map(str::as_bytes);
    ingest_of(parts, version_before, &patch, Some(boxed.as_bytes()))
}

/// The id of a version of a quarantined record (`docs/log-format.md` §3): SHA-256 under `lakelands:quarantined:v1`
/// over the source, the id's delivery and index, the version before, the
/// cells as stored, the box as stored or empty, and the refusal's
/// canonical bytes. The same cells refused the same way at the same
/// version seal to the same box, so they keep the id.
#[doc(hidden)]
pub fn quarantined_id(
    id: &lakelands_api::quarantine::QuarantineId,
    version_before: u32,
    cells: &Patch,
    boxed: Option<&str>,
    refusal: &lakelands_api::Error,
) -> EventId {
    let refusal = canonical::of(&serde_json::to_value(refusal).unwrap_or_default());
    let (source, delivery) = (id.source.as_str(), id.delivery.as_str());
    let at = ([source, delivery].map(str::as_bytes), id.index);
    let boxed = boxed.unwrap_or_default().as_bytes();
    quarantined_of(
        at,
        version_before.into(),
        &canonical::of(cells),
        boxed,
        &refusal,
    )
}

/// A quarantined record's version's id (`docs/log-format.md` §3) from its
/// parts as bytes: its id's source and delivery, and its index, the
/// version before, the cells' canonical bytes, the box as stored or empty
/// and the refusal's canonical bytes. [`quarantined_id`] and
/// [`derived_id`] build it here alone.
pub(crate) fn quarantined_of(
    ([source, delivery], index): ([&[u8]; 2], u64),
    version_before: u64,
    cells: &[u8],
    boxed: &[u8],
    refusal: &[u8],
) -> EventId {
    let (index, before) = (index.to_string(), version_before.to_string());
    id_of(&[
        b"lakelands:quarantined:v1",
        source,
        delivery,
        index.as_bytes(),
        before.as_bytes(),
        cells,
        boxed,
        refusal,
    ])
}

/// The id of an authored write with an idempotency key: one key from one
/// writer is one write in the whole lake.
#[doc(hidden)]
pub fn authored_id(writer: &str, key: &IdempotencyKey) -> EventId {
    id_of(&[
        b"lakelands:authored:v1",
        writer.as_bytes(),
        key.as_str().as_bytes(),
    ])
}

/// The id of an aging transition (`docs/log-format.md` §3): the rule's
/// name, the record it moves and the date aged, in the one form of §1.3
/// at midnight. A date aged twice gives the same ids, and the store
/// appends nothing for an id already in the lake.
#[doc(hidden)]
pub fn aging_id(
    rule: &str,
    ty: &TypeName,
    key: &RecordKey,
    date: lakelands_api::Timestamp,
) -> EventId {
    id_of(&[
        b"lakelands:aging:v1",
        rule.as_bytes(),
        ty.as_str().as_bytes(),
        key.as_str().as_bytes(),
        date.to_string().as_bytes(),
    ])
}

/// The id of the answer recorded for the external verb the event at
/// `position` made due (`docs/log-format.md` §3): a retry finds it by a
/// seek, however far the log has grown since.
#[doc(hidden)]
pub fn output_id(lake: &LakeId, position: Position) -> EventId {
    id_of(&[
        b"lakelands:output:v1",
        lake.as_str().as_bytes(),
        position.0.to_string().as_bytes(),
    ])
}

/// The id of a proposal's acceptance (`docs/log-format.md` §3): an accept
/// sent again finds the acceptance by a seek.
#[doc(hidden)]
pub fn accepted_id(lake: &LakeId, proposal: &ProposalId) -> EventId {
    id_of(&[
        b"lakelands:accepted:v1",
        lake.as_str().as_bytes(),
        proposal.as_str().as_bytes(),
    ])
}

/// The id Lakelands' store gives every other event: its lake and position.
#[doc(hidden)]
pub fn positional_id(lake: &LakeId, position: Position) -> EventId {
    id_of(&[
        b"lakelands:event:v1",
        lake.as_str().as_bytes(),
        position.0.to_string().as_bytes(),
    ])
}

/// Whether this release reads an event written in log format `v`: from
/// [`OLDEST_LOG_FORMAT`], the format a release kept its promise from, to
/// the one it writes
/// (`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`).
#[doc(hidden)]
pub fn reads_format(v: u64) -> bool {
    (u64::from(OLDEST_LOG_FORMAT)..=u64::from(LOG_FORMAT_VERSION)).contains(&v)
}

/// The flaw of a `lake_created` that says another log format than its
/// own line, or one this release does not read, at whatever position it
/// stands: its lake was made by another release
/// (`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`).
/// A caller asks this of an event whose own `v` it has judged.
#[doc(hidden)]
pub fn created_in_another(event: &Event) -> Option<Flaw> {
    match event.body {
        EventBody::LakeCreated { format }
            if u64::from(format) != u64::from(event.v) || !reads_format(format.into()) =>
        {
            let (format, reads) = (format.into(), u64::from(LOG_FORMAT_VERSION));
            Some(Flaw::CreatedInAnother { format, reads })
        }
        _ => None,
    }
}

/// Whether `b` is padded standard base64 of at least the 16 bytes of an
/// AES-256-GCM-SIV tag, as a box is (`docs/log-format.md` §1.4).
fn padded(b: &str) -> bool {
    let body = b.trim_end_matches('=');
    let alphabet = |c: u8| c.is_ascii_alphanumeric() || c == b'+' || c == b'/';
    b.len().is_multiple_of(4)
        && b.len() >= 24
        && b.len() - body.len() <= 2
        && body.bytes().all(alphabet)
}

/// Why an event carries a member where it never sits, if it does:
/// `cause` inside `context`, since a host's cause is the envelope's
/// (`docs/decisions/021-the-write-context-belongs-to-the-host.md`); and
/// `on_behalf_of` inside `context`
/// (`docs/decisions/088-an-event-records-the-principal-a-write-was-made-for.md`); and
/// `upstream` on any kind but `record_ingested`, since only a source's
/// record is read from another lake (`docs/log-format.md` §1.1); and the
/// origin `initial`, a field's provenance alone, never an event's
/// (`docs/decisions/050-a-field-declares-its-initial-value.md`); and an
/// output both carried and named by `output_at`, or neither
/// (`docs/decisions/053-a-recorded-output-is-recorded-once.md`); and
/// `sealed` on any kind but `record_ingested` and `output_recorded`
/// (`docs/decisions/058-a-source-may-be-forgotten.md`); and a
/// `record_ingested` whose `sealed` names another source than its own,
/// whose values would be kept under that source's key (058); and
/// `delivery` on any kind but `record_ingested` and `record_quarantined`
/// (`docs/log-format.md` §1.1); and `sealed.box` on any but a
/// `record_ingested`, and a `record_ingested`'s `sealed` with no box, or
/// one of a form a reader without the key refuses (`docs/log-format.md`
/// §1.4). No door puts any there; a reader holds the
/// rule for a log written by something else.
#[doc(hidden)]
pub fn misplaced(event: &Event) -> Option<Flaw> {
    if event.context.cause.is_some() {
        return Some(Flaw::CauseInContext);
    }
    if event.context.on_behalf_of.is_some() {
        return Some(Flaw::OnBehalfOfInContext);
    }
    if let EventBody::OutputRecorded {
        output, output_at, ..
    } = &event.body
        && output.is_some() == output_at.is_some()
    {
        return Some(Flaw::OutputOnce);
    }
    if event.origin == lakelands_api::record::Origin::Initial {
        return Some(Flaw::InitialOrigin);
    }
    let kind = event_kind(&event.body);
    let seals = matches!(
        event.body,
        EventBody::RecordIngested { .. } | EventBody::OutputRecorded { .. }
    );
    if event.sealed.is_some() && !seals {
        return Some(Flaw::SealedOn { kind });
    }
    let named = event.sealed.as_ref().and_then(|s| s.source.as_ref());
    if let (EventBody::RecordIngested { .. }, Some(sealed)) = (&event.body, named)
        && event.source.as_ref() != Some(sealed)
    {
        let sealed = sealed.clone();
        return Some(Flaw::SealsAnother { sealed });
    }
    let boxed = event.sealed.as_ref().is_some_and(|s| s.boxed.is_some());
    let ingested = matches!(event.body, EventBody::RecordIngested { .. });
    if boxed && !ingested {
        return Some(Flaw::BoxedOn { kind, v: event.v });
    }
    if let (EventBody::RecordIngested { patch, .. }, Some(sealed)) = (&event.body, &event.sealed)
        && (sealed.fields.is_empty()
            || sealed.fields.iter().any(|f| patch.contains_key(f))
            || !sealed.boxed.as_deref().is_some_and(padded))
    {
        return Some(Flaw::BoxForm);
    }
    let quarantined = matches!(event.body, EventBody::RecordQuarantined { .. });
    if event.delivery.is_some() && !quarantined && !ingested {
        return Some(Flaw::DeliveryOn { kind, v: event.v });
    }
    (event.upstream.is_some() && !ingested).then_some(Flaw::UpstreamOn { kind })
}

/// An event body's kind, as the log writes it.
fn event_kind(body: &EventBody) -> String {
    serde_json::to_value(body)
        .ok()
        .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}

/// The top-level members a frame never holds.
const UNFRAMED: [&str; 5] = ["hash", "id", "position", "prev", "v"];

/// The members a frame holds one level into.
const NESTED: [&str; 2] = ["body", "sealed"];

/// The format a line or an event says, 0 where it says none.
fn format_of(line: &Value) -> u64 {
    line.get("v").and_then(Value::as_u64).unwrap_or(0)
}

/// Whether a lake, or an archive of it, is written in a format this
/// release reads, judged over its two ends before either is opened; a
/// lake this release does not read is refused by both numbers: it is
/// another release's, not a damaged one, and it is not opened.
///
/// Three numbers are judged, and the first this release does not read is
/// named: the first event's `v`, the format its `lake_created` says, and
/// the last event's `v`, each where it carries one. Two ends that
/// contradict each other in a way no release writes are damage, named at
/// the event that carries the contradiction: a `lake_created` whose own
/// line says a `v` other than the `format` it says (one release wrote the
/// line and its body at once), and a last event older than the first (a
/// release appends only onto a lake whose first event it reads). An end
/// that is no value at all, or no JSON, is left to the caller, which says
/// what is wrong with it.
///
/// The store calls this with the two events its engine keeps beside the
/// log, and [`crate::export::verify`] with an archive's first and last
/// event lines, so an archive is refused as the open of the same lake
/// refuses it, damage and all.
pub fn one_format(
    lake: &str,
    at: (Position, Position),
    first: Option<&Value>,
    tip: Option<&Value>,
) -> Result<()> {
    // Every line holds its `v`; a `lake_created` may leave members to its
    // frame, which the caller restores where it reads one.
    let number = |v: &Value| v.get("v").and_then(Value::as_u64);
    let made = |f: &&Value| f.pointer("/body/kind") == Some(&"lake_created".into());
    let created = first.filter(made).and_then(|f| f.pointer("/body/format"));
    let reads = u64::from(LOG_FORMAT_VERSION);
    if let (Some(line), Some(format)) = (first.and_then(number), created.and_then(Value::as_u64))
        && line != format
    {
        return Err(crate::damaged(
            Part::Log,
            Some(at.0),
            Flaw::CreatedInAnother { format, reads },
        ));
    }
    if let (Some(line), Some(last)) = (first.and_then(number), tip.and_then(number))
        && last < line
    {
        return Err(crate::damaged(
            Part::Log,
            Some(at.1),
            Flaw::FormatOther { v: last, reads },
        ));
    }
    let said = [
        first.map(format_of),
        created.map(|f| f.as_u64().unwrap_or(0)),
        tip.map(format_of),
    ];
    let unread = said.into_iter().flatten().find(|v| !reads_format(*v));
    let Some(found) = unread else { return Ok(()) };
    Err(Error::unavailable(Situation::LogFormat {
        lake: lake.to_owned(),
        found,
        reads: LOG_FORMAT_VERSION.into(),
        oldest: Some(OLDEST_LOG_FORMAT.into()),
    }))
}

/// Whether a line, read, is a frame line: an event holds neither `events`
/// nor `frame`.
pub fn is_frame(line: &Value) -> bool {
    line.get("events").is_some() || line.get("frame").is_some()
}

/// A frame line, read: its format, how many events it covers, and the
/// members it holds for them.
#[derive(Clone, Debug, PartialEq)]
pub struct Framed {
    /// Its `v`, its events' format.
    pub v: u64,
    /// Its `events`, how many lines after it it covers.
    pub events: u64,
    members: Map<String, Value>,
}

impl Framed {
    /// The frame `line` is, a line read as [`line_of`] reads one: it holds
    /// `events`, `frame` and `v` alone, `events` a count from 1, and `frame`
    /// an object of members a frame may hold, with no empty `body` or
    /// `sealed`; anything else is [`Flaw::NotAFrame`].
    pub fn of(line: Value) -> std::result::Result<Framed, Flaw> {
        let Value::Object(mut line) = line else {
            return Err(Flaw::NotAFrame);
        };
        let v = line.remove("v").and_then(|v| v.as_u64());
        let events = line.remove("events").and_then(|n| n.as_u64());
        let (Some(Value::Object(members)), Some(v), Some(events)) =
            (line.remove("frame"), v, events)
        else {
            return Err(Flaw::NotAFrame);
        };
        let nested = |name: &&str| match members.get(*name) {
            None => true,
            Some(Value::Object(inner)) => !inner.is_empty(),
            Some(_) => false,
        };
        let fits = line.is_empty()
            && (1..=u64::from(u32::MAX)).contains(&events)
            && !members.keys().any(|k| UNFRAMED.contains(&k.as_str()))
            && NESTED.iter().all(nested);
        match fits {
            true => Ok(Framed { v, events, members }),
            false => Err(Flaw::NotAFrame),
        }
    }
}

/// A line of the stored form, read: JSON, in a format this release reads
/// (its `v` read before anything else of it, a frame line's as an
/// event's), and canonical, judged in that order.
pub fn line_of(bytes: &[u8]) -> std::result::Result<Value, Flaw> {
    let line: Value = serde_json::from_slice(bytes).map_err(|e| Flaw::NotJson {
        detail: Detail::json(&e),
    })?;
    let v = format_of(&line);
    if !reads_format(v) {
        let reads = LOG_FORMAT_VERSION.into();
        return Err(Flaw::FormatOther { v, reads });
    }
    if canonical::to_vec(&line) != bytes {
        return Err(Flaw::NotCanonical);
    }
    Ok(line)
}

/// The event an event's line restores to (§1.2), the line read as
/// [`line_of`] reads one: its frame's members added to it, those under
/// `body` and `sealed` merged into its own one level deep, and, where it
/// holds none, the `position` and the `prev` of `derived`, those the
/// event before it gives. A frame line, a frame whose format is not the
/// line's, and a member the line holds that its frame holds too, are
/// refused; what is no event is left to its reader. It answers too whether
/// the line was the whole event: its frame added nothing, and nor did
/// `derived`.
pub fn restored(
    line: Value,
    frame: Option<&Framed>,
    derived: Option<(Position, &Digest)>,
) -> std::result::Result<(Value, bool), Flaw> {
    if is_frame(&line) {
        return Err(Flaw::FrameInFrame);
    }
    let v = format_of(&line);
    if let Some(f) = frame
        && f.v != v
    {
        return Err(Flaw::FrameOther { frame: f.v, v });
    }
    let Value::Object(mut event) = line else {
        return Ok((line, true));
    };
    let whole = frame.is_none() && event.contains_key("position") && event.contains_key("prev");
    for (name, value) in frame.map(|f| &f.members).into_iter().flatten() {
        let twice = |member: String| Err(Flaw::FramedTwice { member });
        match (value, NESTED.contains(&name.as_str())) {
            (Value::Object(framed), true) => {
                let held = event
                    .entry(name.clone())
                    .or_insert_with(|| Map::new().into());
                let Value::Object(held) = held else {
                    return twice(name.clone());
                };
                for (inner, value) in framed {
                    if held.insert(inner.clone(), value.clone()).is_some() {
                        return twice(format!("{name}.{inner}"));
                    }
                }
            }
            _ => {
                if event.insert(name.clone(), value.clone()).is_some() {
                    return twice(name.clone());
                }
            }
        }
    }
    if let Some((position, prev)) = derived {
        event.entry("position").or_insert_with(|| position.0.into());
        event.entry("prev").or_insert_with(|| prev.0.clone().into());
    }
    Ok((Value::Object(event), whole))
}

/// The canonical event an event at the engine seam restores to: its line
/// and its frame read as [`line_of`] reads one, the frame covering its
/// position, and its line restored by [`restored`] with the `position`
/// and `prev` the engine keeps beside it. A frame that does not say what
/// the engine keeps of it, or does not cover the event, is
/// [`Flaw::Disagrees`]. It answers too whether the event's line was the
/// whole event, so that its canonical bytes are the line's. A reader of
/// more than one event restores them with one [`Restorer`].
pub fn restore(stored: &CanonicalEvent) -> std::result::Result<(Value, bool), Flaw> {
    Restorer::default().restore(stored)
}

/// [`restore`] with the line and the frame read as JSON alone, in no
/// format judged: what an engine, which keeps events of any format, checks
/// an event's hash and digests a segment with.
pub fn restore_json(stored: &CanonicalEvent) -> std::result::Result<(Value, bool), Flaw> {
    Restorer::default().restore_json(stored)
}

/// A reader of events at the engine seam, which keeps the last block it
/// restored an event from (§1.5): the block is inflated whole once, every
/// byte of it checked by its zlib stream and its sizes, and each event it
/// holds is restored from its line there, the line the engine hands beside
/// it checked against it. An engine that changed a byte of a block, past
/// its first line too, is caught at the first event read from it.
#[derive(Debug, Default)]
pub struct Restorer {
    kept: Option<block::Kept>,
}

impl Restorer {
    /// [`restore`], from the block it keeps where the event's frame holds
    /// it.
    pub fn restore(&mut self, stored: &CanonicalEvent) -> std::result::Result<(Value, bool), Flaw> {
        self.read(stored, true)
    }

    /// [`restore_json`], from the block it keeps where the event's frame
    /// holds it.
    pub fn restore_json(
        &mut self,
        stored: &CanonicalEvent,
    ) -> std::result::Result<(Value, bool), Flaw> {
        self.read(stored, false)
    }

    /// [`restore`], each line read as [`line_of`] reads one, `strict`, else
    /// as JSON alone. An event a block holds is restored from the block.
    fn read(
        &mut self,
        stored: &CanonicalEvent,
        strict: bool,
    ) -> std::result::Result<(Value, bool), Flaw> {
        let read = if strict { line_of } else { json_of };
        let frame = match &stored.frame {
            None => None,
            Some(f) => {
                if let Some(kept) = self.kept.as_ref().filter(|k| k.holds(f, strict)) {
                    return kept.restore(stored);
                }
                if let Some(kept) = block::Kept::of(f, strict) {
                    let kept = self.kept.insert(kept?);
                    return kept.restore(stored);
                }
                let framed = Framed::of(read(&f.bytes)?)?;
                let within = stored.position.0.checked_sub(f.first.0);
                let covers = within.is_some_and(|i| i < u64::from(f.events));
                if framed.events != u64::from(f.events) || !covers {
                    return Err(Flaw::Disagrees);
                }
                Some(framed)
            }
        };
        let derived = Some((stored.position, &stored.prev));
        restored(read(&stored.bytes)?, frame.as_ref(), derived)
    }
}

/// A line read as JSON alone.
fn json_of(bytes: &[u8]) -> std::result::Result<Value, Flaw> {
    serde_json::from_slice(bytes).map_err(|e| Flaw::NotJson {
        detail: Detail::json(&e),
    })
}

/// A segment's digest (§4.2): SHA-256 over the canonical bytes of its
/// events as [`restore_json`] restores them, each followed by a line feed;
/// a line that is its whole event is hashed as it is kept.
pub fn digest_of<'a>(
    events: impl IntoIterator<Item = &'a CanonicalEvent>,
) -> std::result::Result<Digest, (Position, Flaw)> {
    let mut digesting = Digesting::default();
    digesting.add(events);
    digesting.digest()
}

/// The byte [`Digesting::state`] puts before the running hash's bytes: the
/// format and version of the state, so that a state a release would read
/// differently is refused rather than read wrongly.
const STATE_TAG: u8 = 1;

/// A segment's digest (§4.2) kept running as its events are appended, so
/// that a seal restores nothing: [`digest_of`] over every event added, or
/// the first that did not restore, after which it takes no more.
#[derive(Clone, Debug, Default)]
pub struct Digesting {
    hash: Sha256,
    broken: Option<(Position, Flaw)>,
}

impl Digesting {
    /// Adds `events`, one append's or more, restored as [`digest_of`]
    /// restores them.
    pub fn add<'a>(&mut self, events: impl IntoIterator<Item = &'a CanonicalEvent>) {
        let mut restorer = Restorer::default();
        for event in events {
            if self.broken.is_some() {
                return;
            }
            match restorer.restore_json(event) {
                Ok((_, true)) => self.restored(&event.bytes),
                Ok((restored, false)) => self.restored(&canonical::to_vec(&restored)),
                Err(flaw) => self.broken = Some((event.position, flaw)),
            }
        }
    }

    /// Adds one event by its canonical bytes as restored, which its reader
    /// restored already.
    pub fn restored(&mut self, bytes: &[u8]) {
        if self.broken.is_none() {
            self.hash.update(bytes);
            self.hash.update(b"\n");
        }
    }

    /// The digest of the events added, or where the first that did not
    /// restore stands and why.
    pub fn digest(&self) -> std::result::Result<Digest, (Position, Flaw)> {
        match &self.broken {
            Some(broken) => Err(broken.clone()),
            None => Ok(Digest(canonical::hex(&self.hash.clone().finalize()))),
        }
    }

    /// Its state as bytes, tagged with the format and version they are
    /// written in (`STATE_TAG`), which [`Digesting::of_state`] takes
    /// back; none once an event did not restore.
    pub fn state(&self) -> Option<Vec<u8>> {
        use sha2::digest::common::hazmat::SerializableState as _;
        self.broken.is_none().then(|| {
            let mut state = vec![STATE_TAG];
            state.extend_from_slice(self.hash.serialize().as_ref());
            state
        })
    }

    /// The digest whose [`Digesting::state`] `bytes` are, in this release's
    /// format and version; none for any other, which the caller digests
    /// from its restores instead.
    pub fn of_state(bytes: &[u8]) -> Option<Digesting> {
        use sha2::digest::common::hazmat::SerializableState;
        let (tag, state) = bytes.split_first()?;
        if *tag != STATE_TAG {
            return None;
        }
        let state = state.try_into().ok()?;
        let hash = Sha256::deserialize(state).ok()?;
        Some(Digesting { hash, broken: None })
    }
}

/// Where a reader of one segment's lines stands, read in order (§1.2,
/// §4.3): the frame whose lines follow, how many of them it has read and
/// has still to read, and the position and hash of the last event
/// [`Lines::read`] restored.
#[derive(Clone, Debug, Default)]
pub struct Lines {
    frame: Option<Framed>,
    left: u64,
    held: u64,
    before: Option<(Position, Digest)>,
}

impl Lines {
    /// Reads the segment's next line as it is stored, as JSON alone, after
    /// the lines read before it by this method: [`Lines::line`] after the
    /// event it last restored, none before the first. It answers the
    /// restored event's canonical bytes, a line that is its whole event as
    /// it is kept: what a segment's digest is taken over.
    pub fn read(&mut self, bytes: &[u8]) -> std::result::Result<Option<Vec<u8>>, Flaw> {
        let before = self.before.clone();
        let read = self.line(json_of(bytes)?, before.as_ref().map(|(p, h)| (*p, h)))?;
        let Some((event, whole)) = read else {
            return Ok(None);
        };
        let position = event.get("position").and_then(Value::as_u64).map(Position);
        let hash = event.get("hash").and_then(Value::as_str);
        self.before = position.zip(hash).map(|(p, h)| (p, Digest(h.to_owned())));
        Ok(Some(match whole {
            true => bytes.to_vec(),
            false => canonical::to_vec(&event),
        }))
    }

    /// Reads the segment's next line, read as [`line_of`] reads one,
    /// after the event `before`, the position and hash of the event before
    /// it in the segment, none at its first: `None` for a frame line, and
    /// else the event it restores. A frame line among the lines a frame
    /// covers, what [`restored`] refuses, a `position` or a `prev` that is
    /// not the one `before` gives, and a segment's first event line that
    /// does not hold both, are refused. It answers too whether the line was
    /// the whole event.
    pub fn line(
        &mut self,
        line: Value,
        before: Option<(Position, &Digest)>,
    ) -> std::result::Result<Option<(Value, bool)>, Flaw> {
        if is_frame(&line) {
            if self.left > 0 {
                return Err(Flaw::FrameInFrame);
            }
            let framed = Framed::of(line)?;
            (self.left, self.held) = (framed.events, 0);
            self.frame = Some(framed);
            return Ok(None);
        }
        let holds = |name| line.get(name).is_some();
        if before.is_none() && !(holds("position") && holds("prev")) {
            return Err(Flaw::StartUnheld);
        }
        let derived = before.map(|(p, hash)| (p.next(), hash));
        let frame = self.frame.as_ref().filter(|_| self.left > 0);
        let (event, whole) = restored(line, frame, derived)?;
        if let Some((expected, prev)) = derived {
            let found = event.get("position").and_then(Value::as_u64).map(Position);
            if found != Some(expected) {
                return Err(Flaw::Position { expected, found });
            }
            if event.get("prev").and_then(Value::as_str) != Some(prev.0.as_str()) {
                return Err(Flaw::PrevHash);
            }
        }
        if self.left > 0 {
            (self.left, self.held) = (self.left - 1, self.held + 1);
        }
        Ok(Some((event, whole)))
    }

    /// How many lines after the event last restored the frame covering it
    /// still covers: 0 at a frame's last event and at one no frame covers.
    pub fn left(&self) -> u64 {
        self.left
    }

    /// Refuses a segment that ends among the lines a frame covers.
    pub fn end(&self) -> std::result::Result<(), Flaw> {
        match self.left {
            0 => Ok(()),
            left => Err(Flaw::FrameShort {
                events: left + self.held,
                held: self.held,
            }),
        }
    }
}

/// The stored form of `events`, the canonical values of events that
/// follow each other in one segment, as one run (§1.2): a frame line,
/// where they are two or more, all of one format, holding exactly the
/// members they all share; and each event's line, which leaves out what
/// the frame holds and, where `holds` says it does not hold them, its
/// `position` and its `prev`. `holds` is asked of each event by its index
/// in the run and its value, and answers whether its line holds `position`,
/// and whether it holds `prev`.
pub fn stored_form(
    events: &[Value],
    holds: impl Fn(usize, &Value) -> (bool, bool),
) -> (Option<Vec<u8>>, Vec<Vec<u8>>) {
    form(events, |i, event| {
        let (position, prev) = holds(i, event);
        let names = [("position", !position), ("prev", !prev)];
        names
            .into_iter()
            .filter(|(_, l)| *l)
            .map(|(n, _)| n)
            .collect()
    })
}

/// [`stored_form`], each event's line leaving out the members `leave`
/// names of it, asked by its index in the run and its value.
fn form(
    events: &[Value],
    leave: impl Fn(usize, &Value) -> Vec<&'static str>,
) -> (Option<Vec<u8>>, Vec<Vec<u8>>) {
    let v = events.first().map_or(0, format_of);
    let framed = events.len() >= 2 && events.iter().all(|e| format_of(e) == v);
    let shared = match framed {
        true => shared(events),
        false => Map::new(),
    };
    let lines = events.iter().enumerate().map(|(i, event)| {
        let Value::Object(members) = event else {
            return canonical::to_vec(event);
        };
        let mut line = members.clone();
        for name in leave(i, event) {
            line.remove(name);
        }
        for (name, value) in &shared {
            match (value, line.get_mut(name)) {
                (Value::Object(framed), Some(Value::Object(held)))
                    if NESTED.contains(&name.as_str()) =>
                {
                    framed.keys().for_each(|k| drop(held.remove(k)));
                    if held.is_empty() {
                        line.remove(name);
                    }
                }
                _ => drop(line.remove(name)),
            }
        }
        canonical::to_vec(&Value::Object(line))
    });
    let lines = lines.collect();
    let frame = framed.then(|| {
        let line = serde_json::json!({"events": events.len(), "frame": shared, "v": v});
        canonical::to_vec(&line)
    });
    (frame, lines)
}

/// The members every one of `events` holds with an equal value that a
/// frame may hold: top-level ones, and those one level into `body` and
/// `sealed`.
fn shared(events: &[Value]) -> Map<String, Value> {
    let Some((Value::Object(first), rest)) = events.split_first() else {
        return Map::new();
    };
    let mut frame = Map::new();
    for (name, value) in first {
        if UNFRAMED.contains(&name.as_str()) {
            continue;
        }
        if NESTED.contains(&name.as_str()) {
            let Value::Object(inner) = value else {
                continue;
            };
            let all = |k: &String, v: &Value| {
                let held = |e: &Value| e.get(name).and_then(|n| n.get(k)) == Some(v);
                rest.iter().all(held)
            };
            let common: Map<String, Value> = inner
                .iter()
                .filter(|(k, v)| all(k, v))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            if !common.is_empty() {
                frame.insert(name.clone(), common.into());
            }
        } else if rest.iter().all(|e| e.get(name) == Some(value)) {
            frame.insert(name.clone(), value.clone());
        }
    }
    frame
}

/// Writes `events`, one append's, each holding its canonical bytes, in the
/// store's writer's form: an append of two or more events as blocks
/// (§1.5), and an event alone as its line (§1.2), holding `position` and
/// `prev` only where it is a segment's first event, the `lake_created` at
/// position 0 or a `checkpoint`. The append that holds a `lake_created`
/// is left whole, each line its event's canonical bytes: a release that
/// does not read the format finds its number on the first line, even while
/// it is the last.
pub fn frame_append(events: &mut [CanonicalEvent]) {
    let values: Vec<Value> = events
        .iter()
        .map(|e| serde_json::from_slice(&e.bytes).expect("an event's canonical bytes are JSON"))
        .collect();
    let makes = |e: &Value| e.pointer("/body/kind") == Some(&"lake_created".into());
    if values.iter().any(makes) {
        return;
    }
    if events.len() >= 2 {
        return block::into_blocks(events, &values);
    }
    let first = |_: usize, e: &Value| {
        let first = e.get("position") == Some(&0.into())
            || e.pointer("/body/kind") == Some(&"checkpoint".into());
        (first, first)
    };
    let (_, lines) = stored_form(&values, first);
    for (event, line) in events.iter_mut().zip(lines) {
        (event.bytes, event.frame) = (line, None);
    }
}

/// Refuses `events`, one append's or the appends of one commit in order,
/// unless every frame they share is held whole by them and in order: a
/// frame is a block's bytes (§1.5), whose first line says the frame's
/// first position and events; it begins at the position of the first event
/// that carries it, which is no event of another frame's, and every event
/// it covers carries it and no other does. The blocks of one append follow
/// each other, each naming its last position as `end`, and the last one
/// reaches it: no event alone on its line stands among them, and they end
/// within `events`.
pub fn frame_fault(events: &[CanonicalEvent]) -> Result<()> {
    let end = events.last().map_or(0, |e| e.position.0 + 1);
    // The frame the events now read carry, and how many more it covers.
    let mut open: Option<(&Arc<Frame>, u32)> = None;
    // The `end` of the append whose blocks have not reached it.
    let mut append: Option<Position> = None;
    let fault = |position| Err(Error::invalid(Case::AppendFrame { position }));
    for event in events {
        let fits = match (&event.frame, open) {
            (None, None) => append.is_none(),
            (Some(f), None) => {
                let block = block_of(f).and_then(|b| b.ok());
                let fits = block.as_ref().is_some_and(|b| {
                    b.first == f.first
                        && b.events == u64::from(f.events)
                        && b.end.0 < end
                        && append.is_none_or(|a| a == b.end)
                });
                append = block.filter(|b| b.end > b.last()).map(|b| b.end);
                f.first == event.position && f.events > 0 && fits
            }
            (Some(f), Some((held, _))) => f == held,
            (None, Some(_)) => false,
        };
        if !fits {
            return fault(event.position);
        }
        open = match (&event.frame, open) {
            (Some(_), Some((held, left))) => Some((held, left - 1)),
            (Some(f), None) => Some((f, f.events - 1)),
            _ => None,
        }
        .filter(|(_, left)| *left > 0);
    }
    match (open, append, events.last()) {
        (None, None, _) | (_, _, None) => Ok(()),
        (_, _, Some(last)) => fault(last.position),
    }
}

/// The last position of the append that wrote `stored`: the `end` its
/// block says (§1.5), else its own, since the store's writer stores every
/// append of two or more events as blocks. An append left whole reads as
/// one of its own.
pub fn append_end(stored: &CanonicalEvent) -> Position {
    let block = stored.frame.as_ref().and_then(|f| block_of(f)?.ok());
    block.map_or(stored.position, |b| b.end)
}

#[cfg(test)]
mod tests;
