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
