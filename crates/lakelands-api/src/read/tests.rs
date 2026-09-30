use super::*;

#[test]
fn a_latest_key_carries_no_field_on_the_wire_and_every_other_names_one() {
    let latest: Order = serde_json::from_value(serde_json::json!({"by": "latest", "dir": "desc"}))
        .expect("a latest key reads without a field");
    assert!(latest.field.as_str().is_empty());
    assert_eq!(latest.misfit(), None);
    assert_eq!(
        serde_json::to_value(&latest).unwrap(),
        serde_json::json!({"by": "latest", "dir": "desc"}),
        "echoed back, a latest key names no field"
    );
    let named = Order {
        field: "amount".into(),
        ..latest.clone()
    };
    assert_eq!(named.misfit().map(|m| m.0), Some(Some("amount".into())));
    let unnamed = Order {
        by: OrderBy::KnownAt,
        ..latest
    };
    assert_eq!(unnamed.misfit().map(|m| m.0), Some(None));
}

#[test]
fn a_group_key_reads_either_form_and_refuses_an_unknown_member() {
    let field: GroupKey = serde_json::from_str(r#""partner""#).unwrap();
    assert_eq!(field, GroupKey::from("partner"));
    let named: GroupKey =
        serde_json::from_str(r#"{"name": "m", "expr": "date_bucket(on, 'month')"}"#).unwrap();
    assert_eq!(named.name().as_str(), "m");
    let extra = serde_json::from_str::<GroupKey>(r#"{"name": "m", "expr": "x", "unit": "day"}"#);
    assert!(extra.is_err(), "an unknown member is refused: {extra:?}");
}
