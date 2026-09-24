//! Values against their declared types, and the mock's stable hashes.

use lakelands_api::id::{IdempotencyKey, Position};
use lakelands_api::ontology::{FieldType, StorageKind, TypeDecl, ValueTypeDecl};
use lakelands_api::principal::Principal;
use lakelands_api::record::Value;
use lakelands_api::time::Timestamp;
use serde_json::json;
use sha2::{Digest as _, Sha256};

/// Why a value does not fit its field, for a person; `None` when it fits.
pub(crate) fn misfit(ty: &FieldType, v: &Value, value_types: &[ValueTypeDecl]) -> Option<String> {
    let is_decimal = |s: &str| {
        let s = s.strip_prefix('-').unwrap_or(s);
        let mut parts = s.splitn(2, '.');
        let whole = parts.next().unwrap_or_default();
        let frac = parts.next();
        !whole.is_empty()
            && whole.bytes().all(|b| b.is_ascii_digit())
            && frac.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()))
    };
    let fits = match ty.kind {
        StorageKind::Text | StorageKind::Reference | StorageKind::Bytes => v.is_string(),
        StorageKind::Number => v.is_number(),
        StorageKind::Decimal => v.as_str().is_some_and(is_decimal),
        StorageKind::Boolean => v.is_boolean(),
        StorageKind::DateTime => v.as_str().is_some_and(|s| Timestamp::parse(s).is_ok()),
        StorageKind::Json => true,
        StorageKind::Vector => v.as_array().is_some_and(|a| {
            a.iter().all(Value::is_number) && ty.dimensions.is_none_or(|d| a.len() == d as usize)
        }),
        StorageKind::Geo => v.get("type").is_some_and(Value::is_string),
    };
    if !fits {
        let kind = serde_json::to_value(ty.kind).unwrap_or_default();
        return Some(format!(
            "the value is not a valid {}",
            kind.as_str().unwrap_or("value")
        ));
    }
    let vt = ty
        .value_type
        .as_ref()
        .and_then(|n| value_types.iter().find(|d| &d.name == n))?;
    match v.as_str() {
        Some(s) if !vt.one_of.is_empty() && !vt.one_of.iter().any(|o| o == s) => {
            Some(format!("`{s}` is not one of {}", vt.one_of.join(", ")))
        }
        _ => None,
    }
}

/// A stored value converted to a new field type; `None` when it would
/// lose information.
pub(crate) fn convert(v: &Value, to: &FieldType) -> Option<Value> {
    if misfit(to, v, &[]).is_none() {
        return Some(v.clone());
    }
    let c = match (to.kind, v) {
        (StorageKind::Text | StorageKind::Decimal, Value::Number(n)) => {
            Value::String(n.to_string())
        }
        (StorageKind::Number, Value::String(s)) => {
            let n: serde_json::Number = s.parse().ok()?;
            (n.to_string() == *s).then_some(Value::Number(n))?
        }
        (StorageKind::Text, Value::Bool(b)) => Value::String(b.to_string()),
        _ => return None,
    };
    misfit(to, &c, &[]).is_none().then_some(c)
}

/// FNV-1a, 64 bits: small, stable across builds and platforms.
pub(crate) fn fnv(bytes: &[u8], seed: u64) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64 ^ seed;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// `n` groups of 16 hex digits over some bytes.
pub(crate) fn hex(bytes: &[u8], groups: u64) -> String {
    (0..groups)
        .map(|g| format!("{:016x}", fnv(bytes, g)))
        .collect()
}

/// A type's schema hash, as the store computes it: SHA-256 over the
/// canonical bytes of the type's shape object (`docs/log-format.md`
/// §2.2) — its key, its fields and its relations sorted by name, each
/// field by name, type and whether required — as 64 lower-case hex
/// digits. It is the store's hash for every type without a view. A view
/// is hashed with its conditions in the store's typed form, which the
/// mock does not build; the mock hashes a view's conditions as written,
/// so a view type's hash has the store's form but not its value.
pub(crate) fn schema_hash(t: &TypeDecl) -> String {
    let fields = |fs: &[lakelands_api::ontology::FieldDecl]| -> Vec<Value> {
        let mut sorted: Vec<_> = fs.iter().collect();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        sorted
            .into_iter()
            .map(|f| json!({"name": f.name, "type": f.ty, "required": f.required}))
            .collect()
    };
    let mut relations: Vec<_> = t.relations.iter().collect();
    relations.sort_by(|a, b| a.name.cmp(&b.name));
    let shape = json!({
        "key": t.key,
        "fields": fields(&t.fields),
        "relations": relations
            .into_iter()
            .map(|r| json!({
                "name": r.name,
                "target": r.target,
                "cardinality": r.cardinality,
                "fields": fields(&r.fields),
            }))
            .collect::<Vec<_>>(),
        "view": t.view,
    });
    sha256_hex(&canonical_bytes(&shape))
}

/// The canonical bytes of a JSON value (`docs/log-format.md` §1.2): no
/// whitespace, members sorted by their UTF-16 code units, numbers in
/// their one form. Strings are escaped as serde_json escapes them, which
/// is the canonical escaping.
pub(crate) fn canonical_bytes(v: &Value) -> Vec<u8> {
    fn write(v: &Value, out: &mut Vec<u8>) {
        match v {
            Value::Array(items) => {
                out.push(b'[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    write(item, out);
                }
                out.push(b']');
            }
            Value::Object(members) => {
                let mut sorted: Vec<_> = members.iter().collect();
                sorted.sort_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
                out.push(b'{');
                for (i, (name, member)) in sorted.into_iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    out.extend(serde_json::to_vec(name).expect("a string prints"));
                    out.push(b':');
                    write(member, out);
                }
                out.push(b'}');
            }
            Value::Number(_) => {
                out.extend(serde_json::to_vec(&canonical_number(v)).expect("a number prints"))
            }
            scalar => out.extend(serde_json::to_vec(scalar).expect("a scalar prints")),
        }
    }
    let mut out = Vec::new();
    write(v, &mut out);
    out
}

/// SHA-256 over some bytes, as lower-case hex.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A proposal's id, as the store gives it: the id of its `write_proposed`
/// event — the first 16 bytes of SHA-256 over its parts joined by a zero
/// byte, as 32 lower-case hex digits. An authored write with an
/// idempotency key is named by its writer and key; any other by its lake
/// and position.
pub(crate) fn proposal_id(who: &Principal, key: Option<&IdempotencyKey>, at: Position) -> String {
    let position = at.0.to_string();
    let writer = who.writer();
    let parts: [&[u8]; 3] = match key {
        Some(k) => [
            b"lakelands:authored:v1",
            writer.as_bytes(),
            k.as_str().as_bytes(),
        ],
        None => [
            b"lakelands:event:v1",
            who.lake.as_str().as_bytes(),
            position.as_bytes(),
        ],
    };
    let mut h = Sha256::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h.update([0u8]);
        }
        h.update(p);
    }
    h.finalize()[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A number in its one JSON form (`docs/log-format.md` §1.2): an integral
/// value as an integer, `2` and never `2.0`.
fn canonical_number(v: &Value) -> Value {
    match v.as_f64() {
        Some(f) if v.is_f64() && f.fract() == 0.0 && f.abs() < 9_007_199_254_740_992.0 => {
            Value::from(f as i64)
        }
        _ => v.clone(),
    }
}

/// A checked value in its one form: numbers as §1.2 writes them, inside
/// vectors and JSON values too, and a date-time in the one form of §1.3.
/// Every value the mock stores, keys and hashes is in this form, as the
/// store's are.
pub(crate) fn canonical(ty: &FieldType, v: &Value) -> Value {
    fn numbers(v: &Value) -> Value {
        match v {
            Value::Number(_) => canonical_number(v),
            Value::Array(a) => Value::Array(a.iter().map(numbers).collect()),
            Value::Object(m) => {
                Value::Object(m.iter().map(|(k, x)| (k.clone(), numbers(x))).collect())
            }
            other => other.clone(),
        }
    }
    match (ty.kind, v) {
        (StorageKind::DateTime, Value::String(s)) => Timestamp::parse(s)
            .map(|t| Value::String(t.to_string()))
            .unwrap_or_else(|_| v.clone()),
        (StorageKind::Number | StorageKind::Vector | StorageKind::Json | StorageKind::Geo, _) => {
            numbers(v)
        }
        _ => v.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schema_hash_is_the_log_formats() {
        // The type and the digest `lakelands-core` pins for §2.2: the
        // mock computes the same hash from the same specification.
        let invoice: TypeDecl = serde_json::from_value(json!({
            "name": "invoice",
            "key": ["number"],
            "fields": [
                { "name": "number", "type": { "kind": "text" }, "required": true },
                { "name": "amount", "type": { "kind": "decimal", "value_type": "money" } },
                { "name": "customer", "type": { "kind": "reference", "target": "customer" } },
            ],
            "relations": [
                { "name": "billed_to", "target": "customer", "via": "customer" },
            ],
        }))
        .unwrap();
        assert_eq!(
            schema_hash(&invoice),
            "ce7fc1daa67c9853e3054fae92e9e188aa08039cc0ab8e460e3d73d9edd038d5"
        );
    }
}
