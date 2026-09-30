use super::*;
use serde_json::json;

fn t(s: &str) -> Timestamp {
    Timestamp::parse(s).unwrap()
}

#[test]
fn a_record_holds_from_its_start_until_its_end() {
    let at = t("2026-05-08T12:00:00Z");
    for (from, to, expected, why) in [
        ("2026-05-07", None, true, "started the day before, no end"),
        ("2026-05-08T12:00:00Z", None, true, "starts at that instant"),
        (
            "2026-05-08T12:00:00.000001Z",
            None,
            false,
            "starts a microsecond later",
        ),
        (
            "2026-05-08T14:00:00+02:00",
            None,
            true,
            "with an offset, the same instant",
        ),
        (
            "2026-05-01",
            Some("2026-05-08T12:00:00Z"),
            false,
            "ends at that instant",
        ),
        ("2026-05-01", Some("2026-05-09"), true, "ends later"),
        (
            "2026-05-01",
            Some("not a date"),
            true,
            "an unreadable end is no end",
        ),
    ] {
        let to = to.map(|s| json!(s));
        assert_eq!(
            holds(anchor(Some(&json!(from))), anchor(to.as_ref()), at),
            expected,
            "{why}"
        );
    }
}

#[test]
fn a_record_that_cannot_say_when_it_began_never_holds() {
    let at = t("2026-05-08");
    for from in [
        None,
        Some(json!("not-a-date")),
        Some(json!("2026-13-01")),
        Some(json!("")),
        Some(json!(20260508)),
    ] {
        assert!(!holds(anchor(from.as_ref()), None, at), "{from:?}");
    }
}

#[test]
fn a_type_that_is_not_valid_timed_always_holds() {
    let ty: TypeDecl = serde_json::from_value(json!({"name": "note"})).unwrap();
    assert!(record_holds(&ty, &BTreeMap::new(), t("2026-01-01")));
    let dated: TypeDecl =
        serde_json::from_value(json!({"name": "rate", "valid_from": "since", "valid_to": "until"}))
            .unwrap();
    let fields: BTreeMap<FieldName, Value> =
        serde_json::from_value(json!({"since": "2026-01-01", "until": "2027-01-01"})).unwrap();
    assert!(record_holds(&dated, &fields, t("2026-06-01")));
    assert!(!record_holds(&dated, &fields, t("2027-01-01")));
    assert!(!record_holds(&dated, &BTreeMap::new(), t("2026-06-01")));
}
