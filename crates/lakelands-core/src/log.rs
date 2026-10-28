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
use lakelands_api::error::{Case, Detail, Error, Flaw, Result};
use lakelands_api::event::{
    Digest, Event, EventBody, EventId, LOG_FORMAT_VERSION, OLDEST_LOG_FORMAT,
};
use lakelands_api::write::Selection;
use lakelands_api::{
    IdempotencyKey, LakeId, Patch, Position, ProposalId, RecordKey, SourceId, TypeName,
};
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

use crate::canonical;

pub use lakelands_api::key::record_key;
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
    id_of(&[
        b"lakelands:ingest:v1",
        source.as_str().as_bytes(),
        ty.as_str().as_bytes(),
        key.as_str().as_bytes(),
        version_before.to_string().as_bytes(),
        &canonical::of(patch),
    ])
}

/// The id of an ingested record whose sealed members are in one box, from
/// format 20 (`docs/log-format.md` §3): [`ingest_id`]'s parts under
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
    id_of(&[
        b"lakelands:ingest:v2",
        source.as_str().as_bytes(),
        ty.as_str().as_bytes(),
        key.as_str().as_bytes(),
        version_before.to_string().as_bytes(),
        &canonical::of(patch),
        boxed.as_bytes(),
    ])
}

/// The id of a version of a quarantined record, from format 21
/// (`docs/log-format.md` §3): SHA-256 under `lakelands:quarantined:v1`
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
    let refusal = serde_json::to_value(refusal).unwrap_or_default();
    id_of(&[
        b"lakelands:quarantined:v1",
        id.source.as_str().as_bytes(),
        id.delivery.as_str().as_bytes(),
        id.index.to_string().as_bytes(),
        version_before.to_string().as_bytes(),
        &canonical::of(cells),
        boxed.unwrap_or_default().as_bytes(),
        &canonical::of(&refusal),
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
/// [`OLDEST_LOG_FORMAT`], the first a release with real installations
/// wrote, to the one it writes
/// (`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`).
#[doc(hidden)]
pub fn reads_format(v: u64) -> bool {
    (u64::from(OLDEST_LOG_FORMAT)..=u64::from(LOG_FORMAT_VERSION)).contains(&v)
}

/// The log format that first holds `sealed` and `source_forgotten` (058).
const SEALED_FROM: u32 = 16;

/// The log format that first holds `apart`, a delivery's key outside its
/// source's versions, and a selection by `seen_before` or by `deliveries`
/// (065).
const APART_FROM: u32 = 18;

/// The log format that first seals a record's members in one box (083).
const BOXED_FROM: u32 = 20;

/// The log format that first holds a quarantined record (085).
const QUARANTINED_FROM: u32 = 21;

/// The log format that first holds `on_behalf_of` (088).
const ON_BEHALF_OF_FROM: u32 = 21;

/// An event as log format `v` wrote it, its members as the format this
/// release writes says them, for the release's own structs to read. Its
/// `v` stays the one it was written in, and its bytes and hash are the
/// stored ones: a converter changes what an event is read as, never the
/// log. Each step of the format is one function here, from 14 on.
///
/// - **14 → 15:** none. Format 15 lets a verb's path read across a
///   reference or a relation's link (055); a log of format 14 holds no
///   such declaration, and nothing else of an event changed.
/// - **15 → 16:** none. Format 16 is 0.15.0's one step: a log of format
///   15 holds none of the forms it adds, each named here by its change:
///   an event's `sealed` and a field's `erasable` (058), so each value of
///   format 15 is plaintext; and a set's `where` and a trigger's `when`
///   that read across an edge (055).
/// - **16 → 17:** none. Format 17 is 0.16.0's one step, taken before any
///   form it adds (lakelands#1164): a log of format 16 holds none of them.
///   `docs/log-format.md` §6 names each, by the change that adds it.
/// - **17 → 18:** none. Format 18 is 0.17.0's one step, taken before any
///   form it adds (lakelands#1214): a log of format 17 holds none of them,
///   a field's `admits` (069) and a verb's `granted` (070) among them.
/// - **18 → 19:** none. Format 19 is 0.18.0's one step, taken before any
///   form it adds (lakelands#1330): a log of format 18 holds none of them,
///   a verb's `preconditions` and `payload.<name>` in its `requires` (028)
///   among them.
/// - **19 → 20:** none. Format 20 is 0.20.0's one step, taken before any
///   form it adds (lakelands#1409): a log of format 19 holds none of them,
///   a proposal of a change to the records a condition selects (081) among
///   them.
/// - **20 → 21:** none. Format 21 is 0.21.0's one step, taken before any
///   form it adds (lakelands#1502): a log of format 20 holds none of them,
///   a `record_quarantined` and a `quarantine_closed` (085) among them.
#[doc(hidden)]
pub fn converted(v: u64, event: serde_json::Value) -> serde_json::Value {
    debug_assert!(reads_format(v), "format {v} is not one this release reads");
    event
}

/// A walk along a log's formats: a lake's format changes only where a
/// new writer upgrades it, and never falls. The format rises at the new
/// writer's `lease_taken`, the first event it appends, and that writer's
/// next event is the `lake_upgraded` from the format before to the new
/// one; a `lake_upgraded` anywhere else, and any other change, is a flaw
/// at its position (`docs/log-format.md` §6).
#[derive(Clone, Copy, Debug, Default)]
#[doc(hidden)]
pub struct FormatSteps {
    /// The format of the event before, if one was read.
    before: Option<u32>,
    /// The step a `lease_taken` began, which the next event must mark.
    owed: Option<(u32, u32)>,
    /// Whether the walk began right after a `lease_taken`, whose step, if
    /// it began one, it could not see.
    leased: bool,
}

impl FormatSteps {
    /// A walk that begins after `prev`, if any: a range that starts
    /// inside the log, `verify` from a position. Right after a
    /// `lease_taken` it takes a `lake_upgraded` whose `to` is the format
    /// there without knowing the format before the lease: a `verify` of
    /// the whole log is what judges the step.
    pub fn after(prev: Option<&Event>) -> FormatSteps {
        FormatSteps {
            before: prev.map(|e| e.v),
            owed: None,
            leased: prev.is_some_and(|e| matches!(e.body, EventBody::LeaseTaken { .. })),
        }
    }

    /// Judge the next event of the log.
    pub fn next(&mut self, event: &Event) -> Option<Flaw> {
        let (before, owed, v) = (self.before, self.owed.take(), event.v);
        let leased = std::mem::take(&mut self.leased);
        self.before = Some(v);
        let upgraded = |from: u32, to: u32| matches!(event.body, EventBody::LakeUpgraded { from: f, to: t } if f == from && t == to);
        let flaw = |before: u32| {
            Some(Flaw::FormatStep {
                v: v.into(),
                before: before.into(),
            })
        };
        if let Some((from, to)) = owed {
            return (!(v == to && upgraded(from, to)))
                .then(|| flaw(from))
                .flatten();
        }
        let before = before?;
        // Right after a lease the walk did not see rise: its marker.
        let unseen = matches!(event.body,
            EventBody::LakeUpgraded { from, to } if from < to && to == v && v == before);
        if leased && unseen {
            return None;
        }
        let marker = matches!(event.body, EventBody::LakeUpgraded { .. });
        let leased = matches!(event.body, EventBody::LeaseTaken { .. });
        if v == before && !marker {
            return None;
        }
        if v > before && leased {
            self.owed = Some((before, v));
            return None;
        }
        flaw(before)
    }

    /// The step a `lease_taken` began and no `lake_upgraded` marked yet,
    /// at the end of what was read: a flaw where the log ends there.
    pub fn unmarked(&self) -> Option<Flaw> {
        let (from, to) = self.owed?;
        Some(Flaw::FormatStep {
            v: to.into(),
            before: from.into(),
        })
    }
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
/// `on_behalf_of` inside `context`, or on an event of a format before 21
/// (`docs/decisions/088-an-event-records-the-principal-a-write-was-made-for.md`); and
/// `upstream` on any kind but `record_ingested`, since only a source's
/// record is read from another lake (`docs/log-format.md` §1.1); and the
/// origin `initial`, a field's provenance alone, never an event's
/// (`docs/decisions/050-a-field-declares-its-initial-value.md`); and an
/// output both carried and named by `output_at`, or neither
/// (`docs/decisions/053-a-recorded-output-is-recorded-once.md`); and
/// `sealed` on any kind but `record_ingested` and `output_recorded`, or
/// on an event of a format before 16
/// (`docs/decisions/058-a-source-may-be-forgotten.md`); and a
/// `record_ingested` whose `sealed` names another source than its own,
/// whose values would be kept under that source's key (058); and `delivery` on
/// any kind but `record_ingested`, or before format 17
/// (`docs/log-format.md` §1.1); and a `trigger_dropped`, or a
/// `trigger_fired`'s `change` or `created`, before format 17; and `apart`,
/// in `sealed` or on a `values_removed`, and a `values_removed` selecting
/// by `seen_before` or by `deliveries`, before format 18 (065); and
/// `sealed.box` on any but a `record_ingested` of format 20 on, or of a
/// form a reader without the key refuses (`docs/log-format.md` §1.4).
/// No door
/// puts any there; a reader holds the rule for a log written by something
/// else.
#[doc(hidden)]
pub fn misplaced(event: &Event) -> Option<Flaw> {
    if event.context.cause.is_some() {
        return Some(Flaw::CauseInContext);
    }
    if event.context.on_behalf_of.is_some() {
        return Some(Flaw::OnBehalfOfInContext);
    }
    if event.on_behalf_of.is_some() && event.v < ON_BEHALF_OF_FROM {
        let v = event.v;
        return Some(Flaw::OnBehalfOfBefore { v });
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
    if event.sealed.is_some() && event.v < SEALED_FROM {
        let v = event.v;
        return Some(Flaw::SealedBefore { v });
    }
    let named = event.sealed.as_ref().and_then(|s| s.source.as_ref());
    if let (EventBody::RecordIngested { .. }, Some(sealed)) = (&event.body, named)
        && event.source.as_ref() != Some(sealed)
    {
        let sealed = sealed.clone();
        return Some(Flaw::SealsAnother { sealed });
    }
    if matches!(event.body, EventBody::SourceForgotten { .. }) && event.v < SEALED_FROM {
        let v = event.v;
        return Some(Flaw::ForgottenBefore { v });
    }
    if matches!(event.body, EventBody::ValuesRemoved { .. }) && event.v < 17 {
        let v = event.v;
        return Some(Flaw::RemovedBefore { v });
    }
    let apart = match &event.body {
        EventBody::ValuesRemoved { apart, .. } => !apart.is_empty(),
        _ => false,
    };
    let apart = apart || event.sealed.as_ref().is_some_and(|s| !s.apart.is_empty());
    if apart && event.v < APART_FROM {
        let v = event.v;
        return Some(Flaw::ApartBefore { v });
    }
    let boxed = event.sealed.as_ref().is_some_and(|s| s.boxed.is_some());
    let ingest = matches!(event.body, EventBody::RecordIngested { .. });
    if boxed && (!ingest || event.v < BOXED_FROM) {
        let (kind, v) = (kind.clone(), event.v);
        return Some(Flaw::BoxedOn { kind, v });
    }
    if let (EventBody::RecordIngested { patch, .. }, Some(sealed)) = (&event.body, &event.sealed)
        && let Some(b) = &sealed.boxed
        && (sealed.fields.is_empty()
            || sealed.fields.iter().any(|f| patch.contains_key(f))
            || !padded(b))
    {
        return Some(Flaw::BoxForm);
    }
    if let EventBody::ValuesRemoved { selection, .. } = &event.body
        && event.v < APART_FROM
    {
        let by = match selection {
            Selection::SeenBefore(_) => "seen_before",
            Selection::Deliveries(_) => "deliveries",
            _ => "",
        };
        if !by.is_empty() {
            let (v, by) = (event.v, by.to_owned());
            return Some(Flaw::SelectsLater { v, by });
        }
    }
    let told = |c: &Option<_>, m: &Option<bool>| c.is_some() || m.is_some();
    if let EventBody::TriggerFired {
        change, created, ..
    } = &event.body
        && told(change, created)
        && event.v < 17
    {
        let v = event.v;
        return Some(Flaw::ChangeBefore { v });
    }
    if matches!(event.body, EventBody::TriggerDropped { .. }) && event.v < 17 {
        let v = event.v;
        return Some(Flaw::DroppedBefore { v });
    }
    let quarantine = matches!(
        event.body,
        EventBody::RecordQuarantined { .. } | EventBody::QuarantineClosed { .. }
    );
    if quarantine && event.v < QUARANTINED_FROM {
        let (kind, v) = (kind.clone(), event.v);
        return Some(Flaw::QuarantinedBefore { kind, v });
    }
    let quarantined = matches!(event.body, EventBody::RecordQuarantined { .. });
    let ingested = matches!(event.body, EventBody::RecordIngested { .. });
    if event.delivery.is_some() && !quarantined && (!ingested || event.v < 17) {
        let v = event.v;
        return Some(Flaw::DeliveryOn { kind, v });
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

/// The first log format whose lines are a stored form with frames and
/// derived members (`docs/log-format.md` §1.2): a line of a format before
/// it is a full event, taken as it stands.
pub const FRAMED_FROM: u64 = 20;

/// The top-level members a frame never holds.
const UNFRAMED: [&str; 5] = ["hash", "id", "position", "prev", "v"];

/// The members a frame holds one level into.
const NESTED: [&str; 2] = ["body", "sealed"];

/// The format a line or an event says, 0 where it says none.
fn format_of(line: &Value) -> u64 {
    line.get("v").and_then(Value::as_u64).unwrap_or(0)
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
    /// `events`, `frame` and `v` alone, `v` from [`FRAMED_FROM`] on,
    /// `events` a count from 1, and `frame` an object of members a frame
    /// may hold, with no empty `body` or `sealed`; anything else is
    /// [`Flaw::NotAFrame`].
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
            && v >= FRAMED_FROM
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
/// event before it gives. A line of a format before [`FRAMED_FROM`] is a
/// full event and is taken as it stands. A frame line, a frame whose
/// format is not the line's, and a member the line holds that its frame
/// holds too, are refused; what is no event is left to its reader. It
/// answers too whether the line was the whole event: its frame added
/// nothing, and nor did `derived`.
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
    if v < FRAMED_FROM {
        return Ok((Value::Object(event), true));
    }
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
/// whole event, so that its canonical bytes are the line's.
pub fn restore(stored: &CanonicalEvent) -> std::result::Result<(Value, bool), Flaw> {
    restore_read(stored, line_of)
}

/// [`restore`] with the line and the frame read as JSON alone, in no
/// format judged: what an engine, which keeps events of any format, checks
/// an event's hash and digests a segment with.
pub fn restore_json(stored: &CanonicalEvent) -> std::result::Result<(Value, bool), Flaw> {
    restore_read(stored, json_of)
}

/// A line read as JSON alone.
fn json_of(bytes: &[u8]) -> std::result::Result<Value, Flaw> {
    serde_json::from_slice(bytes).map_err(|e| Flaw::NotJson {
        detail: Detail::json(&e),
    })
}

/// [`restore`], each line read by `read`.
fn restore_read(
    stored: &CanonicalEvent,
    read: fn(&[u8]) -> std::result::Result<Value, Flaw>,
) -> std::result::Result<(Value, bool), Flaw> {
    let frame = match &stored.frame {
        None => None,
        Some(f) => {
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

/// A segment's digest (§4.2): SHA-256 over the canonical bytes of its
/// events as [`restore_json`] restores them, each followed by a line feed;
/// a line that is its whole event is hashed as it is kept.
pub fn digest_of<'a>(
    events: impl IntoIterator<Item = &'a CanonicalEvent>,
) -> std::result::Result<Digest, (Position, Flaw)> {
    let mut hash = Sha256::new();
    for event in events {
        let (restored, whole) = restore_json(event).map_err(|flaw| (event.position, flaw))?;
        match whole {
            true => hash.update(&event.bytes),
            false => hash.update(canonical::to_vec(&restored)),
        }
        hash.update(b"\n");
    }
    Ok(Digest(canonical::hex(&hash.finalize())))
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
    /// not the one `before` gives, and a segment's first event line of a
    /// format from [`FRAMED_FROM`] on that does not hold both, are
    /// refused. It answers too whether the line was the whole event.
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
        if before.is_none()
            && format_of(&line) >= FRAMED_FROM
            && !(holds("position") && holds("prev"))
        {
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
/// where they are two or more, all of one format from [`FRAMED_FROM`] on,
/// holding exactly the members they all share; and each event's line,
/// which leaves out what the frame holds and, where `holds` says it does
/// not hold them, its `position` and its `prev`. `holds` is asked of each
/// event by its index in the run and its value, and answers whether its
/// line holds `position`, and whether it holds `prev`; an event of a
/// format before [`FRAMED_FROM`] keeps every member.
pub fn stored_form(
    events: &[Value],
    holds: impl Fn(usize, &Value) -> (bool, bool),
) -> (Option<Vec<u8>>, Vec<Vec<u8>>) {
    let v = events.first().map_or(0, format_of);
    let framed = events.len() >= 2 && v >= FRAMED_FROM && events.iter().all(|e| format_of(e) == v);
    let shared = match framed {
        true => shared(events),
        false => Map::new(),
    };
    let lines = events.iter().enumerate().map(|(i, event)| {
        let Value::Object(members) = event else {
            return canonical::to_vec(event);
        };
        let mut line = members.clone();
        if format_of(event) >= FRAMED_FROM {
            let (position, prev) = holds(i, event);
            for (name, keep) in [("position", position), ("prev", prev)] {
                if !keep {
                    line.remove(name);
                }
            }
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
/// store's writer's form (§1.2): framed where they are two or more, with
/// exactly the members they all share, and each line holding `position`
/// and `prev` only where it is a segment's first event, the `lake_created`
/// at position 0 or a `checkpoint`. An append that holds a `lake_created`
/// or a `lake_upgraded` begins the lake's format and is left whole, each
/// line its event's canonical bytes: the release before reads it as lines
/// of its own form and names the format, even while it is the last.
pub fn frame_append(events: &mut [CanonicalEvent]) {
    let values: Vec<Value> = events
        .iter()
        .map(|e| serde_json::from_slice(&e.bytes).expect("an event's canonical bytes are JSON"))
        .collect();
    let begins = |e: &Value| {
        let kind = e.pointer("/body/kind").and_then(Value::as_str);
        matches!(kind, Some("lake_created" | "lake_upgraded"))
    };
    if values.iter().any(begins) {
        return;
    }
    let first = |_: usize, e: &Value| {
        let first = e.get("position") == Some(&0.into())
            || e.pointer("/body/kind") == Some(&"checkpoint".into());
        (first, first)
    };
    let (frame, lines) = stored_form(&values, first);
    let frame = frame.map(|bytes| {
        Arc::new(Frame {
            first: events[0].position,
            events: events.len() as u32,
            bytes,
        })
    });
    for (event, line) in events.iter_mut().zip(lines) {
        (event.bytes, event.frame) = (line, frame.clone());
    }
}

/// Refuses `events`, an append's, unless every frame they share is held
/// whole by them and in order: a frame begins at the position of the
/// first event that carries it, which is no event of another frame's,
/// every event it covers carries it and no other does, it covers no
/// position past the append's last, and its bytes hold no line feed.
pub fn frame_fault(events: &[CanonicalEvent]) -> Result<()> {
    let end = events.last().map_or(0, |e| e.position.0 + 1);
    // The frame the events now read carry, and how many more it covers.
    let mut open: Option<(&Arc<Frame>, u32)> = None;
    for event in events {
        let fits = match (&event.frame, open) {
            (None, None) => true,
            (Some(f), None) => {
                f.first == event.position
                    && f.events > 0
                    && f.first.0 + u64::from(f.events) <= end
                    && !f.bytes.contains(&b'\n')
            }
            (Some(f), Some((held, _))) => f == held,
            (None, Some(_)) => false,
        };
        if !fits {
            let position = event.position;
            return Err(Error::invalid(Case::AppendFrame { position }));
        }
        open = match (&event.frame, open) {
            (Some(_), Some((held, left))) => Some((held, left - 1)),
            (Some(f), None) => Some((f, f.events - 1)),
            _ => None,
        }
        .filter(|(_, left)| *left > 0);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
