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

/// A number is too large as the canonical bytes write it: an integer in
/// plain digits beyond ±(2^53 − 1), whatever form it arrived in. What is
/// written is judged the same when it is read back.
#[test]
fn an_integer_is_too_large_in_the_form_it_is_written_in() {
    let max = 9_007_199_254_740_991_i64;
    let written = |v: &Value| String::from_utf8(to_vec(v)).unwrap();
    let again = |v: &Value| serde_json::from_slice::<Value>(&to_vec(v)).unwrap();
    for fits in [
        json!(max),
        json!(-max),
        json!(9007199254740991.0),
        json!(2.0),
        json!(0.5),
        json!(1e21),
        json!(-1e21),
        json!(1e300),
        json!("1e16"),
        json!({"a": [1, 2.5, null]}),
    ] {
        assert_eq!(integer_too_large(&fits), None, "{fits}");
        assert_eq!(integer_too_large(&again(&fits)), None, "{fits}, read back");
    }
    for (large, digits) in [
        (json!(max + 1), "9007199254740992"),
        (json!(-max - 1), "-9007199254740992"),
        (json!(u64::MAX), "18446744073709551615"),
        (json!(1e16), "10000000000000000"),
        (json!(-1e16), "-10000000000000000"),
        (json!(9007199254740993.0), "9007199254740992"),
        (json!(1e20), "100000000000000000000"),
    ] {
        assert_eq!(written(&large), digits);
        assert!(integer_too_large(&large).is_some(), "{large}");
        assert!(integer_too_large(&again(&large)).is_some(), "{digits}");
    }
    let nested = json!({"a": [1, {"b": 1e17}]});
    assert_eq!(integer_too_large(&nested), json!(1e17).as_number());
}

/// The path of the first such number, as a member is written: names
/// joined by dots, a list's items by their index.
#[test]
fn an_integer_too_large_is_found_with_its_path() {
    let at = |v: Value| integer_too_large_at(&v).map(|(path, n)| (path, n.to_string()));
    let found = |path: &str, n: &str| Some((path.to_owned(), n.to_owned()));
    assert_eq!(
        at(json!(9007199254740992_u64)),
        found("", "9007199254740992")
    );
    assert_eq!(
        at(json!({"a": 1, "b": {"c": [0, {"d": 1e16}]}})),
        found("b.c[1].d", "1e+16")
    );
    assert_eq!(at(json!([[1], [2, 1e20]])), found("[1][1]", "1e+20"));
    assert_eq!(at(json!({"a": [1e21, "1e16", 9007199254740991_u64]})), None);
}

/// An object sealed with `hash` is what hashing it and writing it again
/// with `hash` give, wherever `hash` sorts: first, between, last, alone.
#[test]
fn a_sealed_object_writes_its_members_once_and_hash_where_it_sorts() {
    for value in [
        json!({"id": 1, "z": [2]}),
        json!({"a": 1, "id": "x"}),
        json!({"a": {"b": 1}}),
        json!({}),
    ] {
        let members = value.as_object().unwrap();
        let (digest, bytes) = sealed(members, "hash");
        let mut with = members.clone();
        with.insert("hash".into(), json!(sha256_hex(&value)));
        assert_eq!(
            (digest, bytes),
            (sha256_hex(&value), to_vec(&Value::Object(with)))
        );
    }
}

fn written(f: f64) -> String {
    let mut out = Vec::new();
    number_into(f, &mut out);
    String::from_utf8(out).unwrap()
}

/// The buffer writer agrees with `number`, the oracle, on `f`.
fn agrees(f: f64) {
    assert_eq!(written(f), number(f), "bits {:016x}", f.to_bits());
}

/// A seeded xorshift stream: the same doubles on every run.
fn stream(seed: u64) -> impl Iterator<Item = u64> {
    let mut x = seed;
    std::iter::repeat_with(move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    })
}

#[test]
fn the_buffer_writer_agrees_with_number_on_random_doubles() {
    for bits in stream(0x9e37_79b9_7f4a_7c15).take(200_000) {
        let f = f64::from_bits(bits);
        if f.is_finite() {
            agrees(f);
        }
    }
    // Doubles that people write: short decimals, in all magnitudes.
    for bits in stream(42).take(100_000) {
        let m = (bits % 1_000_000) as f64;
        let e = ((bits >> 40) % 60) as i32 - 30;
        agrees(m * 10f64.powi(e));
        agrees(-m / 1000.0);
    }
}

#[test]
fn the_buffer_writer_agrees_with_number_on_the_edges() {
    for f in [
        0.0,
        -0.0,
        f64::MIN_POSITIVE,
        f64::MAX,
        f64::MIN,
        f64::EPSILON,
    ] {
        agrees(f);
    }
    // Subnormals and the extreme exponents.
    for bits in (1..2000_u64).chain((0x7fe0_0000_0000_0000..).take(2000)) {
        agrees(f64::from_bits(bits));
        agrees(-f64::from_bits(bits));
    }
    for bits in stream(7).take(5000) {
        agrees(f64::from_bits(bits & 0x000f_ffff_ffff_ffff)); // subnormal
    }
    // Integers around 2^53 and the other powers of two.
    for p in 0..64 {
        let base = (1u64 << p) as f64;
        for d in -3..=3 {
            agrees(base + f64::from(d));
            agrees(-(base + f64::from(d)));
        }
    }
    let start = 1u64 << 53;
    for i in start - 100..start + 100 {
        agrees(i as f64);
    }
}

/// One ulp on each side of every boundary where the notation changes:
/// 10^-7 and 10^-6 (plain or exponent), 10^20 and 10^21, and each power
/// of ten between.
#[test]
fn the_buffer_writer_agrees_with_number_at_the_notation_boundaries() {
    for e in -30..=30 {
        let f = 10f64.powi(e);
        let bits = f.to_bits();
        for d in -3..=3_i64 {
            agrees(f64::from_bits(bits.wrapping_add_signed(d)));
            agrees(-f64::from_bits(bits.wrapping_add_signed(d)));
        }
    }
    for f in [1e21, 1e-6, 999_999_999_999_999_900_000.0, 1e-7] {
        let bits = f64::to_bits(f);
        for d in -2..=2_i64 {
            agrees(f64::from_bits(bits.wrapping_add_signed(d)));
        }
    }
}

#[test]
fn the_buffer_writer_agrees_with_number_on_widened_f32() {
    for bits in stream(11).take(100_000) {
        let f = f32::from_bits(bits as u32);
        if f.is_finite() {
            agrees(f64::from(f));
        }
    }
    for f in [
        0.1_f32,
        0.2,
        1.0 / 3.0,
        f32::MAX,
        f32::MIN_POSITIVE,
        f32::EPSILON,
    ] {
        agrees(f64::from(f));
    }
}

#[test]
fn integers_print_as_their_digits() {
    for i in [
        0,
        1,
        -1,
        9,
        10,
        99,
        100,
        i64::MAX,
        i64::MIN,
        1 << 53,
        -(1 << 53),
    ] {
        assert_eq!(to_vec(&json!(i)), i.to_string().into_bytes());
    }
    for u in [0, u64::MAX, u64::MAX - 1, 1 << 63, 10_u64.pow(19)] {
        assert_eq!(to_vec(&json!(u)), u.to_string().into_bytes());
    }
    for bits in stream(3).take(10_000) {
        assert_eq!(
            to_vec(&json!(bits as i64)),
            (bits as i64).to_string().into_bytes()
        );
        assert_eq!(to_vec(&json!(bits)), bits.to_string().into_bytes());
    }
}

/// `to_vec` of a vector of doubles is the oracle's strings joined.
#[test]
fn a_vector_of_doubles_is_the_oracle_joined() {
    let items: Vec<f64> = stream(5)
        .take(384)
        .map(|b| (b >> 11) as f64 / (1u64 << 53) as f64 - 0.5)
        .collect();
    let want = items
        .iter()
        .map(|f| number(*f))
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(to_vec(&json!(items)), format!("[{want}]").into_bytes());
}
