use super::*;
use lakelands_api::Error;
use serde_json::json;

fn call(expr: Value, inputs: Value) -> FunctionCall {
    FunctionCall {
        lake: "l".into(),
        function: "gross".into(),
        inputs,
        config: json!({ "expr": expr }),
        cause: None,
    }
}

#[test]
fn an_expression_computes_over_its_inputs_exactly() {
    let got = ExprBacking::compute(
        &call(json!("net * 1.19"), json!({"net": "4000.00"})),
        Timestamp(0),
    );
    assert_eq!(got.unwrap(), json!("4760.0000"));
}

#[test]
fn a_call_without_an_expression_or_with_one_that_fails_is_refused() {
    let none = ExprBacking::compute(
        &FunctionCall {
            config: json!({}),
            ..call(json!("1"), json!({}))
        },
        Timestamp(0),
    );
    assert!(matches!(none, Err(Error::Refused { .. })));
    let fails = ExprBacking::compute(&call(json!("a / 0"), json!({"a": 1})), Timestamp(0));
    assert!(matches!(fails, Err(Error::Refused { .. })));
}
