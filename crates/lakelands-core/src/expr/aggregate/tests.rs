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
        &BTreeMap::new(),
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
        &BTreeMap::new(),
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
        &BTreeMap::new(),
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
            &BTreeMap::new(),
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
    let named = [
        ("margin".into(), parse("sum(rev) - sum(cost)").unwrap()),
        ("rate".into(), parse("margin / sum(rev)").unwrap()),
    ];
    let out = folded(&data, &named).unwrap();
    assert_eq!(out[&FieldName::from("margin")], json!("50"));
    assert_eq!(
        out[&FieldName::from("rate")],
        json!("0.3333333333333333333333333333")
    );
}

/// One group's measure `expr` over `rows`: its value, or its error.
fn measured(rows: &[Value], expr: &str) -> std::result::Result<Value, String> {
    let rows: Vec<Row> = (rows.iter().cloned())
        .map(|v| serde_json::from_value(v).unwrap())
        .collect();
    let named = [(FieldName::from("m"), parse(expr).unwrap())];
    folded(&rows, &named)
        .map(|mut row| row.remove("m").unwrap())
        .map_err(|e| e.to_string())
}

/// One group's `named` measures over `rows`, folded in one at a time.
fn folded(rows: &[Row], named: &[(FieldName, Expr)]) -> Result<Row> {
    let mut group = Grouping::new(Vec::new(), named, Timestamp(0));
    rows.iter().for_each(|r| group.add(r, None));
    let (by, folds) = group.groups().pop().unwrap_or_default();
    measures(&folds, &by, named, Row::new(), None, Timestamp(0))
}

#[test]
fn a_group_folded_a_record_at_a_time_answers_what_folding_it_at_once_did() {
    let mix = [
        json!({"x": "abc", "d": 1}),
        json!({"x": "1", "d": 0}),
        json!({"x": 2, "d": 2}),
    ];
    let max = "79228162514264337593543950335";
    let big = [json!({"x": max}), json!({"x": max}), json!({"x": "abc"})];
    let dec = [json!({"x": "1.10"}), json!({"x": "2.205"}), json!({})];
    let tie = [json!({"x": 1}), json!({"x": "1.0"}), json!({"x": "1.00"})];
    let text = "sum() met the text 'abc', which is not a number";
    let zero = "division by zero";
    let over = &lakelands_api::error::Case::Overflow.to_string();
    let cases: [(&[Value], &str, std::result::Result<&str, &str>); 21] = [
        // An argument that fails on a later record comes before a value
        // that is not a number on an earlier one, for every aggregate.
        (&mix, "sum(x)", Err(text)),
        (&mix, "avg(x)", Err(text)),
        (&mix, "sum(if(d > 0, x, x / d))", Err(zero)),
        (&mix, "min(if(d > 0, x, x / d))", Err(zero)),
        (&mix, "count(if(d > 0, x, x / d))", Err(zero)),
        (&mix, "sum(x) + sum(if(d > 0, x, x / d))", Err(text)),
        // An aggregate in a branch not taken is never read.
        (
            &mix,
            "if(count() < 0, sum(if(d > 0, x, x / d)), 7)",
            Ok("7"),
        ),
        (&mix, "coalesce(count(), sum(x))", Ok("3")),
        (&mix, "coalesce(min(none), sum(x))", Err(text)),
        // The first error of a sum stands, whatever follows it.
        (&big, "sum(x)", Err(over)),
        (&big, "avg(x)", Err(over)),
        (&mix, "min(x)", Ok("1")),
        (&mix, "max(x)", Ok("abc")),
        (&mix, "min(d - 1) + max(d) + avg(d + d) + count(x)", Ok("6")),
        (&mix, "sum(d) + sum(d)", Ok("6")),
        (&dec, "sum(x)", Ok("3.305")),
        (&dec, "avg(x)", Ok("1.65250")),
        (&dec, "count(x) + count()", Ok("5")),
        // Of values that compare equal, the first met is kept.
        (&tie, "min(x)", Ok("1")),
        (&tie, "max(x)", Ok("1")),
        (&[], "sum(x) + count() + coalesce(avg(x), 0)", Ok("0")),
    ];
    for (rows, expr, want) in cases {
        let want = want.map(|v| json!(v)).map_err(String::from);
        assert_eq!(measured(rows, expr), want, "{expr}");
    }
    assert_eq!(measured(&[], "min(x)"), Ok(Value::Null));
}

#[test]
fn a_sum_reads_at_the_largest_scale_among_its_values_in_any_order() {
    let cases: [(&[&str], &str, &str); 11] = [
        // A zero hands the other number back as it stands when the two
        // are added; the sum reads the zero's places all the same.
        (&["5", "0.00"], "sum(x)", "5.00"),
        (&["0.00", "5"], "sum(x)", "5.00"),
        (&["1.10", "-1.10", "6"], "sum(x)", "6.00"),
        (&["6", "1.10", "-1.10"], "sum(x)", "6.00"),
        (&["1.10", "-1.10", "6"], "avg(x)", "2.00"),
        (&["6", "-1.10", "1.10"], "avg(x)", "2.00"),
        (&["6", "-1.10", "1.10"], "sum(x) * 2", "12.00"),
        // A zero is positive, at the scale its values carried.
        (&["-0.00"], "sum(x)", "0.00"),
        (&["1.5", "-1.50"], "sum(x)", "0.00"),
        // Past what is exact the sum is what adding in order gives: the
        // 38 digits these two need are rounded to the 28 a decimal holds.
        (
            &["10000000000000000000", "0.0000000000000000001"],
            "sum(x)",
            "10000000000000000000.000000000",
        ),
        (
            &["0.0000000000000000001", "10000000000000000000"],
            "sum(x)",
            "10000000000000000000.000000000",
        ),
    ];
    for (xs, expr, want) in cases {
        let rows: Vec<Value> = xs.iter().map(|x| json!({"x": x})).collect();
        assert_eq!(measured(&rows, expr), Ok(json!(want)), "{expr} over {xs:?}");
    }
}
