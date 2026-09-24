//! What the log format derives from content: event ids and record keys
//! (`docs/log-format.md` §2.3 and §3).
//!
//! These are pure functions of the values they name, so a writer, a
//! verifier and the published vectors compute them the same way; the
//! store calls them, and so does the vector test.

use lakelands_api::event::EventId;
use lakelands_api::ontology::TypeDecl;
use lakelands_api::{
    FieldName, IdempotencyKey, LakeId, Patch, Position, RecordKey, SourceId, TypeName, Value,
};
use sha2::{Digest as _, Sha256};

use crate::canonical;

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

/// One key field's value as a component of a record's key: its JSON
/// string form — text as it is, a number in its canonical form, `true` or
/// `false` — with `%` written `%25` and `:` written `%3A`. `None` for a
/// value that cannot be part of a key (null, an array, an object).
pub fn key_component(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(_) => {
            String::from_utf8(canonical::to_vec(value)).expect("canonical JSON is UTF-8")
        }
        _ => return None,
    };
    Some(text.replace('%', "%25").replace(':', "%3A"))
}

/// The key a record of `ty` has, from its key fields' values: the
/// components joined by `:`. `None` when the type declares no key fields
/// or one of them has no value.
pub fn record_key(
    ty: &TypeDecl,
    fields: &std::collections::BTreeMap<FieldName, Value>,
) -> Option<RecordKey> {
    if ty.key.is_empty() {
        return None;
    }
    let parts: Option<Vec<String>> = ty
        .key
        .iter()
        .map(|k| fields.get(k).and_then(key_component))
        .collect();
    parts.map(|p| RecordKey::from(p.join(":")))
}

/// A key string's components, decoded: split on `:`, then `%3A` and `%25`
/// read back. Only those two upper-case escapes are the key's encoding; any
/// other `%` sequence is kept as it stands.
pub fn key_components(key: &RecordKey) -> Vec<String> {
    key.as_str()
        .split(':')
        .map(|c| c.replace("%3A", ":").replace("%25", "%"))
        .collect()
}

/// Whether a field of this kind can be part of a record's key: the kinds
/// whose values have a JSON string form (`docs/log-format.md` §2.3).
pub fn keyable(kind: lakelands_api::ontology::StorageKind) -> bool {
    use lakelands_api::ontology::StorageKind as K;
    matches!(
        kind,
        K::Text | K::Reference | K::Decimal | K::Number | K::Boolean | K::DateTime
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_key_is_its_fields_joined_and_encoded() {
        let ty: TypeDecl =
            serde_json::from_value(json!({"name": "t", "key": ["a", "b", "c"]})).unwrap();
        let f = |v: Value| serde_json::from_value(v).unwrap();
        assert_eq!(
            record_key(&ty, &f(json!({"a": "x:y%z", "b": 12, "c": true})))
                .unwrap()
                .as_str(),
            "x%3Ay%25z:12:true"
        );
        assert!(record_key(&ty, &f(json!({"a": "x", "b": 1}))).is_none());
        assert_eq!(
            key_components(&"x%3Ay%25z:12:true".into()),
            ["x:y%z", "12", "true"]
        );
        assert_eq!(
            key_components(&"a%3ab".into()),
            ["a%3ab"],
            "only `%3A` is `:`"
        );
    }

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
