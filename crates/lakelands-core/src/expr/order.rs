//! The one sort order over values, shared by the store and every engine.
//!
//! Values compare as the evaluator compares them (`super::eval::compare`):
//! numbers, and text that reads as a number, numerically; instants, and
//! text that reads as one, as instants; other text as text. A sort needs a
//! total order, and the evaluator's comparison has none across those kinds
//! (`9 < '10'` and `'10' < 'abc'` as text, yet `9` and `'abc'` do not
//! compare). So values are ranked by kind first — numbers, then instants,
//! then text, then booleans (`false` first), then JSON values by their
//! canonical bytes — and compared by the evaluator within a kind. Every
//! engine that orders a scan orders by this function, so a page set is the
//! same on every engine.

use std::cmp::Ordering;

use lakelands_api::Timestamp;
use lakelands_api::expr::CmpOp;
use serde_json::Value;

use super::eval::{Val, as_number, as_time, compare, from_json};

/// A value's rank among the kinds a sort orders.
fn rank(v: &Val) -> u8 {
    // The evaluator orders two values exactly when they are of one
    // comparable kind, so its answer against a probe of a kind says
    // whether the value reads as that kind.
    let orders_with = |probe: &Val| {
        compare(CmpOp::Le, v, probe).unwrap_or(false)
            || compare(CmpOp::Gt, v, probe).unwrap_or(false)
    };
    match v {
        Val::Bool(_) => 3,
        Val::Json(_) | Val::Null => 4,
        Val::Duration(_) => 5,
        _ if orders_with(&from_json(&Value::from(0))) => 0,
        _ if orders_with(&Val::Time(Timestamp(0))) => 1,
        _ => 2,
    }
}

/// Two present values, in ascending order.
pub fn cmp_values(a: &Value, b: &Value) -> Ordering {
    let (x, y) = (from_json(a), from_json(b));
    let (rx, ry) = (rank(&x), rank(&y));
    if rx != ry {
        return rx.cmp(&ry);
    }
    let holds = |op| compare(op, &x, &y).unwrap_or(false);
    if holds(CmpOp::Lt) {
        Ordering::Less
    } else if holds(CmpOp::Gt) {
        Ordering::Greater
    } else if holds(CmpOp::Eq) || rx < 3 {
        Ordering::Equal
    } else {
        // Booleans and JSON values have no order of their own; their
        // canonical bytes give one (`false` before `true`).
        crate::canonical::to_vec(a).cmp(&crate::canonical::to_vec(b))
    }
}

/// A present value's sort key: bytes whose order is [`cmp_values`]'
/// order, equal exactly where it calls two values equal. A sort that
/// computes each value's key once compares bytes, where [`cmp_values`]
/// ranks and parses both values on every comparison.
///
/// The key is the value's rank, then its kind's encoding:
/// - a number, or text that reads as one: its sign, then, for a number
///   that is not zero, its decimal exponent and its significant digits,
///   so `1`, `1.0` and `"1.00"` share a key. A negative number's exponent
///   and digits are complemented and end in `0xff`, so a longer one of
///   the same exponent, which is further from zero, sorts first;
/// - an instant, or text that reads as one: its microseconds, big-endian
///   with the sign bit flipped;
/// - other text: its UTF-8 bytes, the order `String` has;
/// - a boolean: `false` before `true`;
/// - a JSON value: its canonical bytes, as [`cmp_values`] orders them.
pub fn sort_key(value: &Value) -> Vec<u8> {
    // The kind is read as `rank` reads it, each parse done once: text is a
    // number when it reads as a decimal, else an instant when it reads as
    // one, else text.
    let v = from_json(value);
    let mut key = Vec::new();
    match &v {
        Val::Num(d) => number_key(*d, &mut key),
        Val::Time(t) => instant_key(*t, &mut key),
        Val::Text(s) => match (as_number(&v), as_time(&v)) {
            (Some(d), _) => number_key(d, &mut key),
            (None, Some(t)) => instant_key(t, &mut key),
            (None, None) => {
                key.push(2);
                key.extend_from_slice(s.as_bytes());
            }
        },
        Val::Bool(b) => key.extend([3, u8::from(*b)]),
        Val::Json(_) | Val::Null => {
            key.push(4);
            key.extend(crate::canonical::to_vec(value));
        }
        Val::Duration(d) => {
            key.push(5);
            key.extend_from_slice(&((*d as u64) ^ (1 << 63)).to_be_bytes());
        }
    }
    key
}

/// An instant's sort key: its rank, then its microseconds, big-endian
/// with the sign bit flipped.
fn instant_key(t: Timestamp, key: &mut Vec<u8>) {
    key.push(1);
    key.extend_from_slice(&((t.0 as u64) ^ (1 << 63)).to_be_bytes());
}

/// A number's part of its sort key.
fn number_key(d: rust_decimal::Decimal, key: &mut Vec<u8>) {
    key.push(0);
    if d.is_zero() {
        key.push(1);
        return;
    }
    let d = d.normalize();
    let digits = d.mantissa().unsigned_abs().to_string();
    // The value is 0.<digits> × 10^exponent.
    let exponent = digits.len() as i64 - i64::from(d.scale());
    let biased = (exponent + 0x8000) as u16;
    if d.is_sign_negative() {
        key.push(0);
        key.extend_from_slice(&(!biased).to_be_bytes());
        key.extend(digits.bytes().map(|b| !b));
        key.push(0xff);
    } else {
        key.push(2);
        key.extend_from_slice(&biased.to_be_bytes());
        key.extend(digits.bytes());
    }
}
