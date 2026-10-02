//! Values against their declared types, and the mock's stable ids.

use lakelands_api::id::{IdempotencyKey, Position};
use lakelands_api::ontology::{FieldType, StorageKind, ValueTypeDecl};
use lakelands_api::principal::Principal;
use lakelands_api::record::Value;
use lakelands_api::time::Timestamp;
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
