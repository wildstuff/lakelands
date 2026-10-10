//! Canonical JSON bytes, as RFC 8785 (the JSON Canonicalization Scheme)
//! defines them.
//!
//! Every hash Lakelands computes over JSON — an event's hash, the schema
//! hash, the preimage of a content-derived event id, the input digest of a
//! recorded seam output — is taken over these bytes, so that any
//! implementation in any language computes the same digest
//! (`docs/log-format.md` §1.2, `docs/decisions/020-events-are-hash-chained-over-canonical-bytes.md`).
//!
//! The rules: object members sorted by the UTF-16 code units of their
//! names; no whitespace; strings with only the escapes JSON requires
//! (`\"`, `\\`, the short forms `\b \t \n \f \r`, and `\u00xx` in lower
//! case for the other control characters); numbers in the ECMAScript
//! shortest round-trip form. Integers stored exactly by `serde_json` are
//! printed as their digits; Lakelands keeps every integer it hashes within
//! 2^53 − 1, where the two forms agree.

use serde_json::Value;
use sha2::{Digest as _, Sha256};

/// The canonical bytes of a JSON value.
pub fn to_vec(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write(value, &mut out);
    out
}

/// The canonical bytes of any serialisable value.
///
/// A non-finite float has no JSON form and `serde_json` turns it into
/// `null`; no contract type carries a float that can be non-finite (every
/// number the store hashes arrived as JSON, which cannot hold one), so
/// this never meets one in practice.
///
/// # Panics
///
/// If `value` does not serialise to JSON at all (a map with non-string
/// keys). No contract type does.
pub fn of<T: serde::Serialize>(value: &T) -> Vec<u8> {
    to_vec(&serde_json::to_value(value).expect("contract types serialise to JSON"))
}

/// The first number anywhere in `value` that its canonical bytes write as
/// an integer beyond ±(2^53 − 1) (`docs/log-format.md` §1.2). A number is
/// judged as it is written, not as it arrived: a double of 2^53 or more is
/// integer-valued, and below 10^21 the canonical form writes it as its
/// plain digits, so `1e16` is the integer `10000000000000000`. From 10^21
/// on a number is written with an exponent, which every reader reads as
/// the same double.
pub fn integer_too_large(value: &Value) -> Option<&serde_json::Number> {
    const MAX: f64 = ((1u64 << 53) - 1) as f64;
    match value {
        Value::Number(n) => {
            let size = n.as_f64().map_or(0.0, f64::abs);
            (size > MAX && size < 1e21).then_some(n)
        }
        Value::Array(items) => items.iter().find_map(integer_too_large),
        Value::Object(m) => m.values().find_map(integer_too_large),
        _ => None,
    }
}

/// SHA-256 of a value's canonical bytes, as 64 lower-case hex digits.
pub fn sha256_hex(value: &Value) -> String {
    hex(&Sha256::digest(to_vec(value)))
}

/// Lower-case hexadecimal.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[usize::from(b >> 4)] as char);
        s.push(DIGITS[usize::from(b & 0xf)] as char);
    }
    s
}

fn write(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Bool(true) => out.extend_from_slice(b"true"),
        Value::Bool(false) => out.extend_from_slice(b"false"),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.extend_from_slice(i.to_string().as_bytes());
            } else if let Some(u) = n.as_u64() {
                out.extend_from_slice(u.to_string().as_bytes());
            } else {
                let f = n.as_f64().expect("a JSON number is an integer or a float");
                out.extend_from_slice(number(f).as_bytes());
            }
        }
        Value::String(s) => string(s, out),
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
            let mut sorted: Vec<(&String, &Value)> = members.iter().collect();
            sorted.sort_by(|(a, _), (b, _)| a.encode_utf16().cmp(b.encode_utf16()));
            out.push(b'{');
            for (i, (name, member)) in sorted.into_iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                string(name, out);
                out.push(b':');
                write(member, out);
            }
            out.push(b'}');
        }
    }
}

fn string(s: &str, out: &mut Vec<u8>) {
    out.push(b'"');
    for c in s.chars() {
        match c {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\u{8}' => out.extend_from_slice(b"\\b"),
            '\t' => out.extend_from_slice(b"\\t"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\u{c}' => out.extend_from_slice(b"\\f"),
            '\r' => out.extend_from_slice(b"\\r"),
            c if u32::from(c) < 0x20 => {
                out.extend_from_slice(format!("\\u{:04x}", u32::from(c)).as_bytes());
            }
            c => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out.push(b'"');
}

/// A finite double in the ECMAScript `Number.prototype.toString` form, which
/// RFC 8785 adopts: the shortest digits that round-trip, placed by the
/// ECMAScript rules for plain and exponent notation.
pub fn number(f: f64) -> String {
    assert!(f.is_finite(), "canonical JSON has no NaN or infinity");
    if f == 0.0 {
        return "0".into(); // -0 prints as 0
    }
    let sign = if f < 0.0 { "-" } else { "" };
    let (digits, n) = shortest(f.abs());
    let k = digits.len() as i32;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let exp = if e >= 0 {
            format!("+{e}")
        } else {
            e.to_string()
        };
        if k == 1 {
            format!("{digits}e{exp}")
        } else {
            format!("{}.{}e{exp}", &digits[..1], &digits[1..])
        }
    };
    format!("{sign}{body}")
}

/// The shortest round-trip decimal digits of a positive finite double and
/// its decimal exponent `n`, such that the value is `0.digits × 10^n`.
fn shortest(f: f64) -> (String, i32) {
    let mut buf = ryu::Buffer::new();
    let printed = buf.format_finite(f);
    let (mantissa, exp) = match printed.split_once('e') {
        Some((m, e)) => (m, e.parse::<i32>().expect("ryu prints an integer exponent")),
        None => (printed, 0),
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let mut digits: String = format!("{int}{frac}");
    let mut n = int.len() as i32 + exp;
    let leading = digits.len() - digits.trim_start_matches('0').len();
    digits.drain(..leading);
    n -= leading as i32;
    let trimmed = digits.trim_end_matches('0').len();
    digits.truncate(trimmed);
    (digits, n)
}

#[cfg(test)]
mod tests;
