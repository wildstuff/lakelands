use super::*;

fn event() -> serde_json::Value {
    serde_json::json!({
        "v": 1, "lake": "finance", "position": 0, "id": "0".repeat(32),
        "prev": Digest::zero(), "known_at": "2026-09-24T00:00:00.000000Z",
        "writer": "system:lakelands", "origin": "system",
        "context": {"origin": "direct", "depth": 0}, "fence": 0,
        "body": {"kind": "lake_created", "format": 1}, "hash": Digest::zero(),
    })
}

#[test]
fn a_complete_event_reads() {
    let e: Event = serde_json::from_value(event()).unwrap();
    assert_eq!(e.body, EventBody::LakeCreated { format: 1 });
}

#[test]
fn unknown_members_and_kinds_are_refused_and_context_is_required() {
    let mut extra = event();
    extra["colour"] = serde_json::json!("blue");
    assert!(serde_json::from_value::<Event>(extra).is_err());

    let mut body = event();
    body["body"]["extra"] = serde_json::json!(1);
    assert!(serde_json::from_value::<Event>(body).is_err());

    let mut kind = event();
    kind["body"] = serde_json::json!({"kind": "record_teleported"});
    assert!(serde_json::from_value::<Event>(kind).is_err());

    let mut no_context = event();
    no_context.as_object_mut().unwrap().remove("context");
    assert!(serde_json::from_value::<Event>(no_context).is_err());
}

#[test]
fn every_kind_serialises_under_the_name_it_reports() {
    let bodies = [
        EventBody::LakeCreated { format: 1 },
        EventBody::WriteAccepted {
            proposal: ProposalId::from("p1"),
        },
        EventBody::RecordRetired {
            ty: "invoice".into(),
            key: "INV-1".into(),
            version: Version(3),
        },
    ];
    for body in bodies {
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["kind"], body.kind());
    }
}
