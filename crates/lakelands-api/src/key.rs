//! The key rule (`docs/log-format.md` §2.3): how a record's key is made
//! from its key fields' values, and read back into them.
//!
//! A key is its components joined by `:`, each component a key field's
//! value in its JSON string form with `%` written `%25` and `:` written
//! `%3A`. The rule is a pure function of the values, so the store, every
//! engine, the mock and a host that mirrors records between lakes compute
//! the same key from the same values; this module is the one definition
//! they all call.
//!
//! A link record's key is the rule over its two ends, `from` and `to`,
//! each end's own key taken as one text component ([`link_key`]); a
//! retired link's ends are read back with [`key_components`].

use std::collections::BTreeMap;

use serde_json::Value;

use crate::canonical;
use crate::id::{FieldName, RecordKey};
use crate::ontology::{StorageKind, TypeDecl};

/// The link records' own key fields.
pub const FROM: &str = "from";
/// See [`FROM`].
pub const TO: &str = "to";

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
pub fn record_key(ty: &TypeDecl, fields: &BTreeMap<FieldName, Value>) -> Option<RecordKey> {
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

/// The key of the link from `from` to `to`: the key rule over the pair,
/// each end's key one text component.
pub fn link_key(from: &RecordKey, to: &RecordKey) -> RecordKey {
    let end = |k: &RecordKey| {
        key_component(&Value::String(k.to_string())).expect("a text value is a component")
    };
    RecordKey::from(format!("{}:{}", end(from), end(to)))
}

/// Whether a field of this kind can be part of a record's key: the kinds
/// whose values have a JSON string form (`docs/log-format.md` §2.3).
pub fn keyable(kind: StorageKind) -> bool {
    use StorageKind as K;
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
    fn a_link_key_is_the_rule_over_its_two_ends() {
        assert_eq!(
            link_key(&"INV:1".into(), &"P-1".into()).as_str(),
            "INV%3A1:P-1"
        );
        assert_eq!(
            key_components(&link_key(&"INV:1".into(), &"P-1".into())),
            ["INV:1", "P-1"]
        );
    }
}
