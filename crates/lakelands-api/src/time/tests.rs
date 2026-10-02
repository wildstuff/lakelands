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
