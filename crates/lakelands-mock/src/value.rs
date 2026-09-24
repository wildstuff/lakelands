//! Values against their declared types, and the mock's stable hashes.

use lakelands_api::ontology::{FieldType, StorageKind, TypeDecl, ValueTypeDecl};
use lakelands_api::record::Value;
use lakelands_api::time::Timestamp;
use serde_json::json;

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

/// A type's schema hash in the mock: over its shape (key, fields with
/// their types and whether required, relations, view), never over its
/// meaning. It is not the store's hash, only as sensitive to shape.
pub(crate) fn schema_hash(t: &TypeDecl) -> String {
    let fields = |fs: &[lakelands_api::ontology::FieldDecl]| -> Vec<Value> {
        fs.iter()
            .map(|f| json!([f.name, f.ty, f.required, f.derived]))
            .collect()
    };
    let shape = json!({
        "key": t.key,
        "fields": fields(&t.fields),
        "relations": t.relations.iter().map(|r| json!([r.name, r.target, r.cardinality, r.via, fields(&r.fields)])).collect::<Vec<_>>(),
        "view": t.view,
        "valid": [t.valid_from, t.valid_to],
    });
    hex(shape.to_string().as_bytes(), 1)
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
