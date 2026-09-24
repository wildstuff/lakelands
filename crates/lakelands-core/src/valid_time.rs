//! When a record holds in the world: the one `valid_at` rule.
//!
//! A valid-timed type names a field holding when a record became true
//! (`valid_from`) and optionally one holding when it stopped (`valid_to`).
//! A read at `valid_at = t` sees a record when
//!
//! ```text
//! valid_from <= t  and  (valid_to is absent  or  t < valid_to)
//! ```
//!
//! The interval is closed at its start and open at its end, so two records
//! where one ends exactly when the next begins never both hold at that
//! instant.
//!
//! A record whose `valid_from` is absent or is not an instant is **not**
//! valid at any `valid_at`: a record that cannot say when it became true
//! cannot claim to have been true at a given moment. Such a record is
//! still read by a query without `valid_at`. An unreadable `valid_to` is
//! treated as absent — the record has not been shown to end.
//!
//! The rule lives here, once, because every engine that pushes `valid_at`
//! into its own query must admit exactly the records this function
//! admits; the conformance suite compares them.

use lakelands_api::ontology::TypeDecl;
use lakelands_api::{FieldName, Timestamp, Value};
use std::collections::BTreeMap;

/// An anchor value as an instant: a date-time string in any form
/// `Timestamp::parse` accepts. Anything else is not an anchor.
pub fn anchor(value: Option<&Value>) -> Option<Timestamp> {
    value
        .and_then(Value::as_str)
        .and_then(|s| Timestamp::parse(s).ok())
}

/// Whether an interval holds at `t`.
pub fn holds(valid_from: Option<Timestamp>, valid_to: Option<Timestamp>, t: Timestamp) -> bool {
    match valid_from {
        Some(from) => from <= t && valid_to.is_none_or(|to| t < to),
        None => false,
    }
}

/// Whether a record of `ty` holds at `t`. A type that is not valid-timed
/// holds at every `t`.
pub fn record_holds(ty: &TypeDecl, fields: &BTreeMap<FieldName, Value>, t: Timestamp) -> bool {
    let Some(from_field) = &ty.valid_from else {
        return true;
    };
    let to = ty.valid_to.as_ref().and_then(|f| anchor(fields.get(f)));
    holds(anchor(fields.get(from_field)), to, t)
}

#[cfg(test)]
mod tests {
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
        let dated: TypeDecl = serde_json::from_value(
            json!({"name": "rate", "valid_from": "since", "valid_to": "until"}),
        )
        .unwrap();
        let fields: BTreeMap<FieldName, Value> =
            serde_json::from_value(json!({"since": "2026-01-01", "until": "2027-01-01"})).unwrap();
        assert!(record_holds(&dated, &fields, t("2026-06-01")));
        assert!(!record_holds(&dated, &fields, t("2027-01-01")));
        assert!(!record_holds(&dated, &BTreeMap::new(), t("2026-06-01")));
    }
}
