use super::*;
use crate::error::RecordName;
use crate::{Error, Missing};

fn row(source: &str, delivery: &str, index: u64, closed: bool) -> Quarantined {
    Quarantined {
        id: QuarantineId {
            source: source.into(),
            delivery: delivery.into(),
            index,
        },
        ty: "invoice".into(),
        key: "k".into(),
        version: 1,
        position: Position(index + 1),
        quarantined_at: Timestamp(0),
        cells: Patch::new(),
        masked: Vec::new(),
        sensitive: Vec::new(),
        seen_at: None,
        upstream: None,
        refusal: Error::not_granted(),
        more: false,
        closed: closed.then_some(QuarantineClose {
            why: ClosedWhy::Dismissed,
            position: Position(9),
        }),
    }
}

#[test]
fn a_filter_answers_in_id_order_after_its_cursor_and_by_state() {
    let all = vec![
        row("s2", "d1", 0, false),
        row("s1", "d2", 0, true),
        row("s1", "d1", 3, false),
        row("s1", "d1", 1, false),
    ];
    let ids = |f: &QuarantineFilter| -> Vec<String> {
        f.answer(all.clone())
            .iter()
            .map(|q| q.id.to_string())
            .collect()
    };
    let every = QuarantineFilter::default();
    assert_eq!(ids(&every), ["s1/d1/1", "s1/d1/3", "s1/d2/0", "s2/d1/0"]);
    let open = QuarantineFilter {
        source: Some("s1".into()),
        state: Some(QuarantineState::Open),
        ..QuarantineFilter::default()
    };
    assert_eq!(ids(&open), ["s1/d1/1", "s1/d1/3"]);
    let page = QuarantineFilter {
        after: Some(row("s1", "d1", 1, false).id),
        limit: Some(2),
        ..QuarantineFilter::default()
    };
    assert_eq!(ids(&page), ["s1/d1/3", "s1/d2/0"]);
    let one = QuarantineFilter {
        source: Some("s1".into()),
        delivery: Some("d1".into()),
        index: Some(3),
        ..QuarantineFilter::default()
    };
    assert_eq!(ids(&one), ["s1/d1/3"]);
}

#[test]
fn a_refusal_past_the_bound_carries_quarantine_full_beside_its_own_on_the_wire() {
    let e = Error::Ingested {
        index: 4,
        from: 2,
        record: RecordName::Of {
            ty: "invoice".into(),
            key: "k".into(),
        },
        error: Box::new(Error::not_granted()),
        quarantine_full: Some(QuarantineFull { max: 3 }),
    };
    let wire = serde_json::to_value(&e).unwrap();
    assert_eq!(wire["error"], "refused");
    assert_eq!(wire["quarantine_full"], serde_json::json!({"max": 3}));
    let back: Error = serde_json::from_value(wire).unwrap();
    assert_eq!(back, e);
    assert!(e.to_string().contains("not quarantined"));
    let missing = Error::NotFound {
        missing: Missing::Quarantined {
            id: row("s", "d", 1, false).id,
        },
    };
    let back: Error = serde_json::from_value(serde_json::to_value(&missing).unwrap()).unwrap();
    assert_eq!(back, missing);
}

#[test]
fn a_refusal_told_as_its_kind_alone_names_nothing_and_reads_back() {
    let field: crate::FieldName = "amount".into();
    let e = Error::refused(Some(field), crate::Refusal::WrongDoor);
    let alone = e.kind_alone();
    let wire = serde_json::to_value(&alone).unwrap();
    assert_eq!(wire["error"], "refused");
    assert_eq!(wire["reason"]["refusal"], "wrong_door");
    assert!(wire.get("field").is_none());
    let back: Error = serde_json::from_value(wire).unwrap();
    assert_eq!(back, alone);
    let missing = Error::NotFound {
        missing: Missing::Quarantined {
            id: row("s", "d", 1, false).id,
        },
    };
    let wire = serde_json::to_value(missing.kind_alone()).unwrap();
    assert_eq!(wire["error"], "not_found");
    assert!(wire.get("missing").is_none());
}
