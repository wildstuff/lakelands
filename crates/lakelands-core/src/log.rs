//! What the log format derives from content: event ids and record keys
//! (`docs/log-format.md` §2.3 and §3), and the members an event never
//! carries where it is ([`misplaced`]).
//!
//! These are pure functions of the values they name, so a writer, a
//! verifier and the published vectors compute them the same way; the
//! store calls them, and so does the vector test. The key rule itself
//! lives in `lakelands_api::key`, where a host reaches it too.

use lakelands_api::error::Flaw;
use lakelands_api::event::{Event, EventBody, EventId, LOG_FORMAT_VERSION};
use lakelands_api::{
    IdempotencyKey, LakeId, Patch, Position, ProposalId, RecordKey, SourceId, TypeName,
};
use sha2::{Digest as _, Sha256};

use crate::canonical;

pub use lakelands_api::key::{key_component, key_components, keyable, record_key};

/// SHA-256 over the parts joined by `0x00`, as an event id: the first 16
/// bytes, in lower-case hex.
fn id_of(parts: &[&[u8]]) -> EventId {
    EventId(canonical::hex(&Sha256::digest(parts.join(&0u8))[..16]))
}

/// The id of an ingested record: its source, type, key, the record's
/// version before the event (0 for a key not yet written) and its patch.
/// A redelivery against the same state has the same id; a value that
/// returns after a change (A, B, A) is a new event.
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

/// The id of an authored write with an idempotency key: one key from one
/// writer is one write in the whole lake.
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
pub fn output_id(lake: &LakeId, position: Position) -> EventId {
    id_of(&[
        b"lakelands:output:v1",
        lake.as_str().as_bytes(),
        position.0.to_string().as_bytes(),
    ])
}

/// The id of a proposal's acceptance (`docs/log-format.md` §3): an accept
/// sent again finds the acceptance by a seek.
pub fn accepted_id(lake: &LakeId, proposal: &ProposalId) -> EventId {
    id_of(&[
        b"lakelands:accepted:v1",
        lake.as_str().as_bytes(),
        proposal.as_str().as_bytes(),
    ])
}

/// The id Lakelands' store gives every other event: its lake and position.
pub fn positional_id(lake: &LakeId, position: Position) -> EventId {
    id_of(&[
        b"lakelands:event:v1",
        lake.as_str().as_bytes(),
        position.0.to_string().as_bytes(),
    ])
}

/// The flaw of a `lake_created` that says another log format than the
/// release's one, at whatever position it stands: its lake was made by
/// another release
/// (`docs/decisions/054-a-lake-is-written-in-one-format-a-release.md`).
/// A caller asks this of an event whose own `v` it has judged.
pub fn created_in_another(event: &Event) -> Option<Flaw> {
    let reads = u64::from(LOG_FORMAT_VERSION);
    match event.body {
        EventBody::LakeCreated { format } if u64::from(format) != reads => {
            let format = format.into();
            Some(Flaw::CreatedInAnother { format, reads })
        }
        _ => None,
    }
}

/// Why an event carries a member where it never sits, if it does:
/// `cause` inside `context`, since a host's cause is the envelope's
/// (`docs/decisions/021-the-write-context-belongs-to-the-host.md`); and
/// `upstream` on any kind but `record_ingested`, since only a source's
/// record is read from another lake (`docs/log-format.md` §1.1); and the
/// origin `initial`, a field's provenance alone, never an event's
/// (`docs/decisions/050-a-field-declares-its-initial-value.md`); and an
/// output both carried and named by `output_at`, or neither
/// (`docs/decisions/053-a-recorded-output-is-recorded-once.md`). No door
/// puts any there; a reader holds the rule for a log written by something
/// else.
pub fn misplaced(event: &Event) -> Option<Flaw> {
    if event.context.cause.is_some() {
        return Some(Flaw::CauseInContext);
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
    (event.upstream.is_some() && !matches!(event.body, EventBody::RecordIngested { .. }))
        .then_some(Flaw::UpstreamOn { kind })
}

/// An event body's kind, as the log writes it.
fn event_kind(body: &EventBody) -> String {
    serde_json::to_value(body)
        .ok()
        .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}

#[cfg(test)]
mod tests;
