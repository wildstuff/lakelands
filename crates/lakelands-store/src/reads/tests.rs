use super::*;

#[test]
fn fusion_rewards_agreement_and_breaks_ties_by_key() {
    let a = vec![("x".into(), 1.0), ("y".into(), 0.5)];
    let b = vec![("y".into(), 0.9), ("x".into(), 0.1)];
    let fused = fuse(&[a, b]);
    assert_eq!(fused[0].0.as_str(), "x");
    assert_eq!(fused[1].0.as_str(), "y");
    let only = fuse(&[vec![("b".into(), 1.0)], vec![("a".into(), 1.0)]]);
    assert_eq!(
        only.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
}

#[test]
fn a_cursor_round_trips_and_a_foreign_one_is_refused() {
    let c = encode(&CursorState {
        at: Position(7),
        after: Some((vec![Value::from(1)], "k".into())),
        offset: 0,
        params: Default::default(),
        door: "find".into(),
        ty: Some("note".into()),
        of: String::new(),
    });
    let back = decode(&c).unwrap();
    assert_eq!(back.at, Position(7));
    assert!(decode(&Cursor("zz".into())).is_err());
    assert!(decode(&Cursor("7b7d".into())).is_err());
}

#[test]
fn only_filters_an_engine_can_apply_alone_are_handed_down() {
    let parse = |t: &str| lakelands_core::expr::parse(t).unwrap();
    assert!(!needs_store(&parse("amount > 10 and status != 'paid'")));
    for kept in [
        "due < now()",
        "rate = setting('vat_rate')",
        "known_at(amount) > due",
        "matches(number, 'INV-[0-9]+')",
        "not (matches(number, 'X') or amount > 1)",
    ] {
        assert!(needs_store(&parse(kept)), "`{kept}` was handed down");
    }
}
