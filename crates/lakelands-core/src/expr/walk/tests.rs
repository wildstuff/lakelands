use super::*;
use serde_json::json;

fn declaration(v: serde_json::Value) -> Declaration {
    serde_json::from_value(v).unwrap()
}

#[test]
fn a_call_is_found_in_any_expression_a_declaration_keeps() {
    let buckets = |d: &Declaration| {
        declares(
            d,
            &|e| matches!(e, Expr::Call { func, .. } if *func == Func::DateBucket),
        )
    };
    let derived = declaration(json!({
        "declare": "type", "name": "t",
        "fields": [
            {"name": "at", "type": {"kind": "date_time"}},
            {"name": "month", "type": {"kind": "date_time"}, "authority": "derived",
             "derived": "date_bucket(at, 'month')", "depends_on": ["at"]}
        ]
    }));
    assert!(buckets(&derived));
    let trigger = declaration(json!({
        "declare": "trigger", "name": "g", "on": "t",
        "when": "date_bucket(at, 'day') > now()"
    }));
    assert!(buckets(&trigger));
    let plain = declaration(json!({
        "declare": "type", "name": "t",
        "fields": [{"name": "date_bucket", "type": {"kind": "text"}}]
    }));
    assert!(
        !buckets(&plain),
        "a field named date_bucket is no call of it"
    );
    let Declaration::Trigger(g) = &trigger else {
        panic!("a trigger");
    };
    let when = g.when.as_ref().unwrap();
    assert!(!names_text(when, &|t| t == "month"));
    assert!(names_text(when, &|t| t == "day"));
    let e = crate::describe::parsed(when).unwrap();
    assert!(calls(&e, &|f| f == Func::Now));
    assert!(!calls(&e, &|f| f == Func::Floor));
}
