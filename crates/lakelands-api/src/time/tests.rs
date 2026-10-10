use super::*;

#[test]
fn the_epoch_prints_in_the_one_canonical_form() {
    assert_eq!(Timestamp(0).to_string(), "1970-01-01T00:00:00.000000Z");
}

#[test]
fn printing_and_parsing_round_trip() {
    for s in [
        "2026-09-24T05:57:11.123456Z",
        "1969-12-31T23:59:59.999999Z",
        "2000-02-29T00:00:00.000000Z",
        "0001-01-01T00:00:00.000000Z",
    ] {
        assert_eq!(Timestamp::parse(s).unwrap().to_string(), s);
    }
}

#[test]
fn impossible_dates_and_loose_forms_are_refused() {
    for s in [
        "2026-02-29",
        "2026-13-01",
        "2026-09-24T24:00:00Z",
        "2026-09-24T05:57:11",
        "2026-09-24T05:57:11.1234567Z",
        "26-09-24",
        "2026-09-24T05:57:11+0200",
    ] {
        assert!(Timestamp::parse(s).is_err(), "{s} was accepted");
    }
}

/// An offset cannot carry an instant out of the years the one form
/// writes: what is parsed prints in a form that parses to it again.
#[test]
fn an_offset_out_of_the_years_of_the_one_form_is_refused() {
    for s in [
        "9999-12-31T23:30:00-01:00",
        "0000-01-01T00:30:00+01:00",
        "9999-12-31T23:59:59.999999-00:01",
        "0000-01-01T00:00:00+00:01",
    ] {
        assert!(Timestamp::parse(s).is_err(), "{s} was accepted");
    }
    for (s, one) in [
        ("9999-12-31T23:30:00+01:00", "9999-12-31T22:30:00.000000Z"),
        ("0000-01-01T00:30:00-01:00", "0000-01-01T01:30:00.000000Z"),
        ("9999-12-31T23:59:59.999999Z", "9999-12-31T23:59:59.999999Z"),
        ("0000-01-01", "0000-01-01T00:00:00.000000Z"),
    ] {
        let t = Timestamp::parse(s).unwrap();
        assert_eq!(t.to_string(), one);
        assert_eq!(Timestamp::parse(one), Ok(t));
    }
}

/// A text that is only compared denotes its instant in whatever year
/// its offset leaves it; `parse`, which reads what is written, refuses it.
#[test]
fn a_text_denotes_its_instant_out_of_the_years_too() {
    let last = Timestamp::parse("9999-12-31T23:59:59.999999Z").unwrap();
    let first = Timestamp::parse("0000-01-01").unwrap();
    for (s, micros) in [
        (
            "9999-12-31T23:30:00-01:00",
            last.0 + 1 + 30 * 60 * 1_000_000,
        ),
        ("0000-01-01T00:30:00+01:00", first.0 - 30 * 60 * 1_000_000),
    ] {
        assert_eq!(Timestamp::denoted(s), Ok(Timestamp(micros)), "{s}");
        assert!(Timestamp::parse(s).is_err(), "{s}");
    }
    // The bound, for an instant that was never parsed.
    for (t, within) in [
        (first, true),
        (last, true),
        (Timestamp(first.0 - 1), false),
        (Timestamp(last.0 + 1), false),
        (Timestamp(i64::MIN), false),
        (Timestamp(i64::MAX), false),
    ] {
        assert_eq!(t.in_years(), within, "{}", t.0);
    }
    for s in [
        "9999-12-31T23:30:00+01:00",
        "0000-01-01",
        "2026-09-24t05:57:11.5z",
    ] {
        assert_eq!(Timestamp::denoted(s), Timestamp::parse(s), "{s}");
    }
    for s in ["soon", "2026-02-30", "2026-09-24T05:57:11"] {
        assert!(Timestamp::denoted(s).is_err(), "{s}");
    }
}
