use super::*;

/// A record of `t` at `key`, held or not.
fn record(key: &str, held: bool) -> StoredRecord {
    let mut r = StoredRecord::ended("t".into(), key.into(), Position(1));
    (r.retired, r.restricted) = (false, held);
    r
}

#[test]
fn a_record_the_head_does_not_hold_keeps_its_hold() {
    let rows = vec![record("gone", true), record("freed", true)];
    let keys = ["gone".into(), "freed".into()];
    let held = held_as(rows, &keys, vec![record("freed", false)]);
    let held: Vec<_> = held
        .iter()
        .map(|r| (r.key.as_str(), r.restricted))
        .collect();
    // `gone` has no counterpart at the head: it is never released by that.
    assert_eq!(held, [("gone", true), ("freed", false)]);
}
