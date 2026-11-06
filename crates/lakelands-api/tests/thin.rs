//! 102's thinning at S1: the event kind, the public types and the wire
//! forms a client reads. Nothing here performs the act; the host's call
//! and the engines' tables land with later steps.

use lakelands_api::describe::TypeDescription;
use lakelands_api::event::{EventBody, KINDS};
use lakelands_api::read::{Found, Group, ThinnedAct};
use lakelands_api::record::Record;
use lakelands_api::write::{CompactReport, Thin, ThinReport};
use lakelands_api::{FieldName, Position, Timestamp};
use serde_json::json;

fn at(s: &str) -> Timestamp {
    Timestamp::parse(s).expect("an instant")
}

fn act() -> ThinnedAct {
    ThinnedAct {
        ty: "tick".into(),
        by: "at".into(),
        before: at("2026-06-01"),
        through: Position(41),
        gone: 1_200,
    }
}

#[test]
fn a_thinning_part_is_a_kind_of_format_23_and_reads_back() {
    assert!(KINDS.contains(&"values_thinned"));
    let body = EventBody::ValuesThinned {
        ty: "tick".into(),
        by: "at".into(),
        before: at("2026-06-01"),
        keys: vec!["t1".into(), "t2".into()],
    };
    assert_eq!(body.kind(), "values_thinned");
    let wire = json!({
        "kind": "values_thinned",
        "type": "tick",
        "by": "at",
        "before": "2026-06-01T00:00:00.000000Z",
        "keys": ["t1", "t2"],
    });
    assert_eq!(serde_json::to_value(&body).unwrap(), wire);
    assert_eq!(serde_json::from_value::<EventBody>(wire).unwrap(), body);
}

#[test]
fn a_read_says_the_acts_and_the_stranded_count_and_omits_them_when_there_are_none() {
    let told = json!([{
        "type": "tick", "by": "at", "before": "2026-06-01T00:00:00.000000Z",
        "through": 41, "gone": 1200,
    }]);
    let records: Found = serde_json::from_value(json!({
        "shape": "records", "rows": [], "thinned": told, "stranded": 2,
    }))
    .expect("a records page");
    let Found::Records {
        thinned, stranded, ..
    } = records.clone()
    else {
        panic!("a records page");
    };
    assert_eq!(thinned, vec![act()]);
    assert_eq!(stranded, Some(2));
    let wire = serde_json::to_value(&records).unwrap();
    assert_eq!(wire["thinned"], told);
    assert_eq!(wire["stranded"], 2);

    let groups: Found = serde_json::from_value(json!({
        "shape": "groups", "rows": [], "thinned": told, "stranded": 2,
    }))
    .expect("a groups page");
    assert_eq!(serde_json::to_value(&groups).unwrap()["thinned"], told);

    // A page that names neither says neither, as an older server's does.
    let bare: Found = serde_json::from_value(json!({"shape": "records", "rows": []}))
        .expect("a page with no acts");
    let bare = serde_json::to_value(&bare).unwrap();
    assert!(bare.get("thinned").is_none(), "{bare}");
    assert!(bare.get("stranded").is_none(), "{bare}");
}

#[test]
fn a_record_and_a_group_bear_the_stranded_mark_only_when_it_holds() {
    let mark = |v: serde_json::Value, holds: bool| {
        let record: Record = serde_json::from_value(v).expect("a record of type and key alone");
        assert_eq!(record.stranded, holds);
        assert_eq!(
            serde_json::to_value(&record)
                .unwrap()
                .get("stranded")
                .is_some(),
            holds
        );
    };
    mark(
        json!({"type": "tick", "key": "t1", "version": 1, "fields": {}}),
        false,
    );
    mark(
        json!({"type": "daily", "key": "g1", "version": 1, "fields": {}, "stranded": true}),
        true,
    );
    let group: Group = serde_json::from_value(json!({"by": {}, "values": {}})).unwrap();
    assert!(!group.stranded);
    assert!(
        serde_json::to_value(&group)
            .unwrap()
            .get("stranded")
            .is_none()
    );
    let group: Group =
        serde_json::from_value(json!({"by": {}, "values": {}, "stranded": true})).unwrap();
    assert!(group.stranded);
}

#[test]
fn a_description_names_the_acts_and_the_stranded_groups() {
    let bare = TypeDescription::default();
    let wire = serde_json::to_value(&bare).unwrap();
    assert!(wire.get("thinned").is_none(), "{wire}");
    assert!(wire.get("stranded").is_none(), "{wire}");

    let told = TypeDescription {
        thinned: vec![act()],
        stranded: 3,
        ..TypeDescription::default()
    };
    let wire = serde_json::to_value(&told).unwrap();
    assert_eq!(
        wire["thinned"],
        json!([{
            "type": "tick", "by": "at", "before": "2026-06-01T00:00:00.000000Z",
            "through": 41, "gone": 1200,
        }])
    );
    assert_eq!(wire["stranded"], 3);
    assert_eq!(
        serde_json::from_value::<TypeDescription>(wire).unwrap(),
        told
    );
}

#[test]
fn a_thin_names_its_field_and_horizon_and_its_report_names_the_act() {
    let thin: Thin = serde_json::from_value(json!({"by": "at", "before": "2026-06-01"})).unwrap();
    assert_eq!(thin.by, FieldName::from("at"));
    assert_eq!(thin.before, at("2026-06-01"));
    assert!(
        serde_json::from_value::<Thin>(json!({"by": "at", "before": "2026-06-01", "extra": 1}))
            .is_err(),
        "a thin names no field it does not have"
    );

    let report = ThinReport {
        ty: "tick".into(),
        by: "at".into(),
        before: at("2026-06-01"),
        records: 1_200,
        through: Position(41),
        reclaimed: None,
    };
    let wire = serde_json::to_value(&report).unwrap();
    assert_eq!(wire["type"], "tick");
    assert_eq!(wire["records"], 1_200);
    assert_eq!(wire["through"], 41);
    assert!(
        wire.get("reclaimed").is_none(),
        "a preflight reclaims nothing"
    );

    let whole = ThinReport {
        reclaimed: Some(CompactReport {
            before: Some(9_000),
            after: Some(1_000),
            head: Position(41),
        }),
        ..report
    };
    let wire = serde_json::to_value(&whole).unwrap();
    assert_eq!(
        wire["reclaimed"],
        json!({"before": 9_000, "after": 1_000, "head": 41})
    );
}
