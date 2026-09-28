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
mod tests {
    use super::*;
    use serde_json::json;

    /// RFC 8785, Appendix B: IEEE 754 bit patterns and their canonical form.
    #[test]
    fn numbers_print_as_the_rfc_table_says() {
        for (bits, want) in [
            (0x0000000000000000_u64, "0"),
            (0x8000000000000000, "0"),
            (0x0000000000000001, "5e-324"),
            (0x8000000000000001, "-5e-324"),
            (0x7fefffffffffffff, "1.7976931348623157e+308"),
            (0xffefffffffffffff, "-1.7976931348623157e+308"),
            (0x4340000000000000, "9007199254740992"),
            (0xc340000000000000, "-9007199254740992"),
            (0x4430000000000000, "295147905179352830000"),
            (0x44b52d02c7e14af5, "9.999999999999997e+22"),
            (0x44b52d02c7e14af6, "1e+23"),
            (0x44b52d02c7e14af7, "1.0000000000000001e+23"),
            (0x444b1ae4d6e2ef4e, "999999999999999700000"),
            (0x444b1ae4d6e2ef4f, "999999999999999900000"),
            (0x444b1ae4d6e2ef50, "1e+21"),
            (0x3eb0c6f7a0b5ed8c, "9.999999999999997e-7"),
            (0x3eb0c6f7a0b5ed8d, "0.000001"),
            (0x41b3de4355555553, "333333333.3333332"),
            (0x41b3de4355555554, "333333333.33333325"),
            (0x41b3de4355555555, "333333333.3333333"),
            (0x41b3de4355555556, "333333333.3333334"),
            (0x41b3de4355555557, "333333333.33333343"),
            (0xbecbf647612f3696, "-0.0000033333333333333333"),
            (0x43143ff3c1cb0959, "1424953923781206.2"),
        ] {
            assert_eq!(number(f64::from_bits(bits)), want, "bits {bits:016x}");
        }
    }

    /// RFC 8785, section 3.2.3: members sort by UTF-16 code units, so a
    /// surrogate pair sorts before a character above it in the BMP.
    #[test]
    fn members_sort_by_utf16_code_units() {
        let v = json!({
            "\u{20ac}": "Euro Sign", "\r": "Carriage Return", "\u{fb33}": "Hebrew Letter Dalet With Dagesh",
            "1": "One", "\u{1f600}": "Emoji: Grinning Face", "\u{80}": "Control", "\u{f6}": "Latin Small Letter O With Diaeresis",
        });
        let names: Vec<String> = match serde_json::from_slice::<serde_json::Value>(&to_vec(&v)) {
            Ok(_) => {
                let s = String::from_utf8(to_vec(&v)).unwrap();
                let mut names = Vec::new();
                let mut rest = s.as_str();
                while let Some(i) = rest.find("\":\"") {
                    let before = &rest[..i];
                    let start = before.rfind(",\"").map(|j| j + 2).unwrap_or(2);
                    names.push(before[start..].to_owned());
                    rest = &rest[i + 3..];
                }
                names
            }
            Err(e) => panic!("{e}"),
        };
        assert_eq!(
            names,
            [
                "\\r",
                "1",
                "\u{80}",
                "\u{f6}",
                "\u{20ac}",
                "\u{1f600}",
                "\u{fb33}"
            ]
        );
    }

    #[test]
    fn strings_escape_only_what_json_requires() {
        let v = json!("a\"b\\c\u{8}\t\n\u{c}\r\u{1}\u{7f}\u{2028}é");
        assert_eq!(
            String::from_utf8(to_vec(&v)).unwrap(),
            "\"a\\\"b\\\\c\\b\\t\\n\\f\\r\\u0001\u{7f}\u{2028}é\""
        );
    }

    #[test]
    fn nesting_whitespace_and_literals() {
        let v = json!({"b": [1, 2.5, true, null, {"z": 1, "a": "x"}], "a": {}});
        assert_eq!(
            String::from_utf8(to_vec(&v)).unwrap(),
            r#"{"a":{},"b":[1,2.5,true,null,{"a":"x","z":1}]}"#
        );
    }

    /// A long decimal must parse to the nearest double, or the canonical
    /// bytes depend on the parser: the RFC's own example.
    #[test]
    fn a_long_mantissa_parses_to_the_nearest_double() {
        let v: serde_json::Value = serde_json::from_str("333333333.33333329").unwrap();
        assert_eq!(String::from_utf8(to_vec(&v)).unwrap(), "333333333.3333333");
        let v: serde_json::Value = serde_json::from_str("1E30").unwrap();
        assert_eq!(String::from_utf8(to_vec(&v)).unwrap(), "1e+30");
    }

    #[test]
    fn a_digest_is_sha256_in_lower_case_hex() {
        assert_eq!(
            sha256_hex(&json!({})),
            "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
        );
    }
}
