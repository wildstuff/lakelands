use super::*;
use serde_json::json;

/// A source record reads the members it has, and refuses one it does
/// not, at its top and in its `upstream`: a misspelled `source` is never
/// read as absent (lakelands#1081).
#[test]
fn a_source_record_refuses_a_member_it_does_not_have() {
    let read = |v: serde_json::Value| serde_json::from_value::<SourceRecord>(v);
    let record = json!({
        "type": "invoice", "key": "k", "fields": {},
        "source": "dev-a", "upstream": {"lake": "l", "position": 1},
    });
    let r = read(record).expect("every member is a source record's");
    assert_eq!(r.source, Some(SourceId::from("dev-a")));
    for unknown in [
        json!({"type": "invoice", "key": "k", "fields": {}, "Source": "dev-a"}),
        json!({"type": "invoice", "key": "k", "fields": {},
               "upstream": {"lake": "l", "position": 1, "at": 2}}),
    ] {
        let err = read(unknown.clone()).expect_err("an unknown member is refused");
        assert!(
            err.to_string().contains("unknown field"),
            "{unknown}: {err}"
        );
    }
}
