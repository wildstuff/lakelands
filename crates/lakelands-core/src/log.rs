//! What the log format derives from content: event ids and record keys
//! (`docs/log-format.md` §2.3 and §3), and the members an event never
//! carries where it is ([`misplaced`]).
//!
//! These are pure functions of the values they name, so a writer, a
//! verifier and the published vectors compute them the same way; the
//! store calls them, and so does the vector test. The key rule itself
//! lives in `lakelands_api::key`, where a host reaches it too.

use lakelands_api::error::Flaw;
use lakelands_api::event::{Event, EventBody, EventId, LOG_FORMAT_VERSION, OLDEST_LOG_FORMAT};
use lakelands_api::{
    IdempotencyKey, LakeId, Patch, Position, ProposalId, RecordKey, SourceId, TypeName,
};
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
///   15 holds none of the forms it adds, each named here by its change.
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
#[doc(hidden)]
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
