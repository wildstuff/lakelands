use lakelands_api::Position;
use lakelands_api::engine::About;
use lakelands_api::seam::FeedFilter;

use super::{Seek, WINDOW, admits, decides_through, seek};

fn about(ty: Option<&str>, key: Option<&str>) -> About {
    About {
        ty: ty.map(Into::into),
        key: key.map(Into::into),
    }
}

fn filter(kinds: &[&str], types: &[&str], keys: &[&str]) -> FeedFilter {
    FeedFilter {
        kinds: kinds.iter().map(|k| k.to_string()).collect(),
        types: types.iter().map(|&t| t.into()).collect(),
        keys: keys.iter().map(|&k| k.into()).collect(),
    }
}

#[test]
fn a_filter_admits_by_kind_and_by_a_type_and_key_of_one_entry() {
    let record = [about(Some("invoice"), Some("INV-1"))];
    let migrated = [
        about(Some("invoice"), None),
        about(Some("sys.link.invoice.to"), None),
    ];
    let verb = [about(None, Some("INV-1"))];
    let nothing = [about(None, None)];
    let rows: [(FeedFilter, &str, &[About], bool); 12] = [
        (filter(&[], &[], &[]), "checkpoint", &nothing, true),
        (
            filter(&["checkpoint"], &[], &[]),
            "checkpoint",
            &nothing,
            true,
        ),
        (
            filter(&["trigger_fired"], &[], &[]),
            "checkpoint",
            &nothing,
            false,
        ),
        (
            filter(&[], &["invoice"], &[]),
            "checkpoint",
            &nothing,
            false,
        ),
        (
            filter(&[], &["invoice"], &[]),
            "record_upserted",
            &record,
            true,
        ),
        (
            filter(&[], &["invoice"], &["INV-1"]),
            "record_upserted",
            &record,
            true,
        ),
        (
            filter(&[], &["invoice"], &["INV-2"]),
            "record_upserted",
            &record,
            false,
        ),
        (
            filter(&[], &["sys.link.invoice.to"], &[]),
            "migrated",
            &migrated,
            true,
        ),
        // A migration names no key, so no key filter admits it.
        (
            filter(&[], &["invoice"], &["INV-1"]),
            "migrated",
            &migrated,
            false,
        ),
        // A verb's call names no type: a key alone admits it, a type never.
        (filter(&[], &[], &["INV-1"]), "verb_called", &verb, true),
        (
            filter(&[], &["invoice"], &["INV-1"]),
            "verb_called",
            &verb,
            false,
        ),
        (
            filter(&["record_retired"], &["invoice"], &[]),
            "record_upserted",
            &record,
            false,
        ),
    ];
    for (f, kind, about, want) in rows {
        assert_eq!(admits(&f, kind, about), want, "{f:?} over {kind} {about:?}");
    }
}

#[test]
fn a_filter_is_sought_by_its_keys_then_types_then_kinds_and_bounded_only_by_type_and_kind() {
    let all = filter(&["k"], &["t"], &["x"]);
    assert!(matches!(seek(&all), Some(Seek::Keys(_))));
    assert!(matches!(
        seek(&filter(&["k"], &["t"], &[])),
        Some(Seek::Types(_))
    ));
    assert!(matches!(
        seek(&filter(&["k"], &[], &[])),
        Some(Seek::Kinds(_))
    ));
    assert_eq!(seek(&filter(&[], &[], &[])), None);
    let (from, far) = (Position(10), Position(1_000_000));
    assert_eq!(decides_through(&all, from, far), far);
    assert_eq!(decides_through(&filter(&["k"], &[], &[]), from, far), far);
    let windowed = filter(&["k"], &["t"], &[]);
    assert_eq!(
        decides_through(&windowed, from, far),
        Position(10 + WINDOW - 1)
    );
    assert_eq!(decides_through(&windowed, from, Position(20)), Position(20));
}
