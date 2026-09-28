//! What the log format derives from content: event ids and record keys
//! (`docs/log-format.md` §2.3 and §3).
//!
//! These are pure functions of the values they name, so a writer, a
//! verifier and the published vectors compute them the same way; the
//! store calls them, and so does the vector test. The key rule itself
//! lives in `lakelands_api::key`, where a host reaches it too.

use lakelands_api::event::EventId;
use lakelands_api::{IdempotencyKey, LakeId, Patch, Position, RecordKey, SourceId, TypeName};
use sha2::{Digest as _, Sha256};

use crate::canonical;

pub use lakelands_api::key::{key_component, key_components, keyable, record_key};

/// SHA-256 over the parts joined by `0x00`, as an event id: the first 16
/// bytes, in lower-case hex.
fn id_of(parts: &[&[u8]]) -> EventId {
    let mut h = Sha256::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h.update([0u8]);
        }
        h.update(p);
    }
    EventId(canonical::hex(&h.finalize()[..16]))
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

/// The id Lakelands' store gives every other event: its lake and position.
pub fn positional_id(lake: &LakeId, position: Position) -> EventId {
    id_of(&[
        b"lakelands:event:v1",
        lake.as_str().as_bytes(),
        position.0.to_string().as_bytes(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ids_depend_on_what_they_name() {
        let p: Patch = serde_json::from_value(json!({"a": 1})).unwrap();
        let id = |v: u64| ingest_id(&"bank".into(), &"t".into(), &"k".into(), v, &p);
        assert_eq!(id(0), id(0));
        assert_ne!(id(0), id(2), "a revert is a new event");
        assert_ne!(
            authored_id("agent:a", &"k".into()),
            authored_id("agent:b", &"k".into())
        );
        assert_eq!(positional_id(&"l".into(), Position(3)).0.len(), 32);
    }
}
