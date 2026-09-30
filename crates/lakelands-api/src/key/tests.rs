use super::*;
use serde_json::json;

#[test]
fn a_key_is_its_fields_joined_and_encoded() {
    let ty: TypeDecl =
        serde_json::from_value(json!({"name": "t", "key": ["a", "b", "c"]})).unwrap();
    let f = |v: Value| serde_json::from_value(v).unwrap();
    assert_eq!(
        record_key(&ty, &f(json!({"a": "x:y%z", "b": 12, "c": true})))
            .unwrap()
            .as_str(),
        "x%3Ay%25z:12:true"
    );
    assert!(record_key(&ty, &f(json!({"a": "x", "b": 1}))).is_none());
    assert_eq!(
        key_components(&"x%3Ay%25z:12:true".into()),
        ["x:y%z", "12", "true"]
    );
    assert_eq!(
        key_components(&"a%3ab".into()),
        ["a%3ab"],
        "only `%3A` is `:`"
    );
}

#[test]
fn a_link_key_is_the_rule_over_its_two_ends() {
    assert_eq!(
        link_key(&"INV:1".into(), &"P-1".into()).as_str(),
        "INV%3A1:P-1"
    );
    assert_eq!(
        key_components(&link_key(&"INV:1".into(), &"P-1".into())),
        ["INV:1", "P-1"]
    );
}
