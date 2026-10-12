use super::*;
use serde_json::json;

#[test]
fn ids_depend_on_what_they_name() {
    let p: Patch = serde_json::from_value(json!({"a": 1})).unwrap();
    let id = |v: u64| ingest_id(&"bank".into(), &"t".into(), &"k".into(), v, &p);
    assert_eq!(id(0), id(0));
    assert_ne!(id(0), id(2), "a revert is a new event");
    assert_ne!(
        authored_id("agent:a", &"k".into()),
        authored_id("agent:b", &"k".into())
    );
    assert_eq!(positional_id(&"l".into(), Position(3)).0.len(), 32);
}

#[test]
fn upstream_belongs_to_record_ingested_alone() {
    let line = |body: serde_json::Value| -> Event {
        serde_json::from_value(json!({
            "v": lakelands_api::event::LOG_FORMAT_VERSION, "lake": "l", "position": 1, "id": "0".repeat(32),
            "prev": "0".repeat(64), "known_at": "2026-01-01T00:00:00.000000Z",
            "writer": "system:lakelands", "origin": "system",
            "context": {"origin": "direct", "depth": 0}, "fence": 1,
            "upstream": {"lake": "other", "position": 9},
            "body": body, "hash": "0".repeat(64)
        }))
        .unwrap()
    };
    let retired = line(json!({"kind": "record_retired", "type": "t", "key": "k", "version": 2}));
    let why = misplaced(&retired).expect("refused");
    let kind = "record_retired".to_owned();
    assert_eq!(why, lakelands_api::error::Flaw::UpstreamOn { kind });
    let ingested = line(json!({"kind": "record_ingested", "type": "t", "key": "k",
                                   "version": 1, "patch": {"f": 1}}));
    assert_eq!(misplaced(&ingested), None);
}

#[test]
fn an_output_is_carried_or_named_and_never_both() {
    let event = |members: serde_json::Value| -> Event {
        let mut body = json!({"kind": "output_recorded", "seam": "embed:m", "input": "0".repeat(64),
                              "field": ["t", "k", "v"]});
        body.as_object_mut()
            .unwrap()
            .extend(members.as_object().unwrap().clone());
        serde_json::from_value(json!({
            "v": lakelands_api::event::LOG_FORMAT_VERSION, "lake": "l", "position": 1,
            "id": "0".repeat(32), "prev": "0".repeat(64),
            "known_at": "2026-01-01T00:00:00.000000Z",
            "writer": "system:lakelands", "origin": "system",
            "context": {"origin": "direct", "depth": 0}, "fence": 1,
            "body": body, "hash": "0".repeat(64)
        }))
        .unwrap()
    };
    assert_eq!(misplaced(&event(json!({"output_at": 7}))), None);
    let carried = event(json!({"output": null}));
    assert!(
        matches!(&carried.body, EventBody::OutputRecorded { output: Some(v), .. } if v.is_null())
    );
    assert_eq!(misplaced(&carried), None);
    for members in [json!({"output": [1, 0], "output_at": 7}), json!({})] {
        assert_eq!(misplaced(&event(members)), Some(Flaw::OutputOnce));
    }
}
