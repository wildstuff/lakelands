use super::*;
use crate::expr::parse;
use lakelands_api::read::AggMeasure;
use serde_json::json;

fn rows() -> Vec<BTreeMap<FieldName, Value>> {
    [
        json!({"debtor": "a", "amount": "10.10", "due": "2026-09-01"}),
        json!({"debtor": "a", "amount": "0.20", "due": "2026-08-01"}),
        json!({"debtor": "b", "amount": 5}),
        json!({"debtor": null, "amount": null}),
    ]
    .into_iter()
    .map(|v| serde_json::from_value(v).unwrap())
    .collect()
}

fn m(name: &str, func: AggFunc, field: Option<&str>) -> AggMeasure {
    AggMeasure {
        name: name.into(),
        func,
        field: field.map(FieldName::from),
    }
}

#[test]
fn sums_are_exact_and_skip_nulls() {
    let data = rows();
    let refs: Vec<_> = data.iter().collect();
    let out = aggregate(
        &refs,
        &Aggregate {
            measures: vec![
                m("total", AggFunc::Sum, Some("amount")),
                m("n", AggFunc::Count, None),
                m("with_amount", AggFunc::Count, Some("amount")),
                m("mean", AggFunc::Avg, Some("amount")),
                m("first_due", AggFunc::Min, Some("due")),
            ],
            group_by: vec![],
        },
        Timestamp(0),
    )
    .unwrap();
    assert_eq!(out.len(), 1);
    let v = &out[0].values;
    assert_eq!(v[&FieldName::from("total")], json!("15.30"));
    assert_eq!(v[&FieldName::from("n")], json!("4"));
    assert_eq!(v[&FieldName::from("with_amount")], json!("3"));
    assert_eq!(v[&FieldName::from("mean")], json!("5.10"));
    assert_eq!(v[&FieldName::from("first_due")], json!("2026-08-01"));
}

#[test]
fn groups_come_in_canonical_key_order_with_null_as_a_group() {
    let data = rows();
    let refs: Vec<_> = data.iter().collect();
    let out = aggregate(
        &refs,
        &Aggregate {
            measures: vec![m("total", AggFunc::Sum, Some("amount"))],
            group_by: vec!["debtor".into()],
        },
        Timestamp(0),
    )
    .unwrap();
    let keys: Vec<Value> = out
        .iter()
        .map(|g| g.by[&FieldName::from("debtor")].clone())
        .collect();
    assert_eq!(keys, [json!("a"), json!("b"), Value::Null]);
    assert_eq!(out[2].values[&FieldName::from("total")], json!("0"));
}

#[test]
fn no_records_still_answer_one_ungrouped_row() {
    let out = aggregate(
        &[],
        &Aggregate {
            measures: vec![
                m("n", AggFunc::Count, None),
                m("mean", AggFunc::Avg, Some("x")),
            ],
            group_by: vec![],
        },
        Timestamp(0),
    )
    .unwrap();
    assert_eq!(out[0].values[&FieldName::from("n")], json!("0"));
    assert_eq!(out[0].values[&FieldName::from("mean")], Value::Null);
}

#[test]
fn a_sum_over_text_that_is_not_a_number_is_an_error() {
    let data: Vec<BTreeMap<FieldName, Value>> =
        vec![serde_json::from_value(json!({"x": "abc"})).unwrap()];
    let refs: Vec<_> = data.iter().collect();
    assert!(
        aggregate(
            &refs,
            &Aggregate {
                measures: vec![m("s", AggFunc::Sum, Some("x"))],
                group_by: vec![]
            },
            Timestamp(0)
        )
        .is_err()
    );
}

#[test]
fn measures_see_the_measures_before_them() {
    let data: Vec<BTreeMap<FieldName, Value>> = [
        json!({"rev": "100", "cost": "60"}),
        json!({"rev": "50", "cost": "40"}),
    ]
    .into_iter()
    .map(|v| serde_json::from_value(v).unwrap())
    .collect();
    let refs: Vec<_> = data.iter().collect();
    let out = measures(
        &refs,
        &BTreeMap::new(),
        &[
            ("margin".into(), parse("sum(rev) - sum(cost)").unwrap()),
            ("rate".into(), parse("margin / sum(rev)").unwrap()),
        ],
        BTreeMap::new(),
        None,
        Timestamp(0),
    )
    .unwrap();
    assert_eq!(out[&FieldName::from("margin")], json!("50"));
    assert_eq!(
        out[&FieldName::from("rate")],
        json!("0.3333333333333333333333333333")
    );
}
