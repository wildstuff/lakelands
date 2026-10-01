use super::*;
use serde_json::json;

fn declaration(v: serde_json::Value) -> Declaration {
    serde_json::from_value(v).unwrap()
}

#[test]
fn upstream_belongs_to_record_ingested_alone() {
    let line = |body: serde_json::Value| -> Event {
        serde_json::from_value(json!({
            "v": 3, "lake": "l", "position": 1, "id": "0".repeat(32),
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
    assert!(why.contains("`record_retired`"), "{why}");
    let ingested = line(json!({"kind": "record_ingested", "type": "t", "key": "k",
                                   "version": 1, "patch": {"f": 1}}));
    assert_eq!(misplaced(&ingested), None);
}

#[test]
fn a_declaration_calling_date_bucket_anywhere_needs_format_3() {
    let derived = declaration(json!({
        "declare": "type", "name": "t",
        "fields": [
            {"name": "at", "type": {"kind": "date_time"}},
            {"name": "month", "type": {"kind": "date_time"}, "authority": "derived",
             "derived": "date_bucket(at, 'month')", "depends_on": ["at"]}
        ]
    }));
    assert!(calls_date_bucket(&derived));
    let trigger = declaration(json!({
        "declare": "trigger", "name": "g", "on": "t",
        "when": "date_bucket(at, 'day') > now()"
    }));
    assert!(calls_date_bucket(&trigger));
    let plain = declaration(json!({
        "declare": "type", "name": "t",
        "fields": [{"name": "date_bucket", "type": {"kind": "text"}}]
    }));
    assert!(
        !calls_date_bucket(&plain),
        "a field named date_bucket is no call of it"
    );
}

#[test]
fn annotations_anywhere_in_a_declaration_need_format_5() {
    let annotated = declaration(json!({
        "declare": "type", "name": "t",
        "fields": [{"name": "a", "type": {"kind": "text"}}],
        "relations": [{"name": "r", "target": "u",
                       "fields": [{"name": "s", "type": {"kind": "text"},
                                   "annotations": {"acme.x": 1}}]}]
    }));
    let Declaration::Type(to) = annotated.clone() else {
        unreachable!()
    };
    let declared = EventBody::Declared {
        declaration: annotated.clone(),
        schema_hash: None,
        version: 1,
    };
    assert_eq!(needed(&declared, false), 5);
    let migration = lakelands_api::write::Migration {
        to: to.clone(),
        renames: vec![],
    };
    let migrated = EventBody::Migrated {
        migration: migration.clone(),
        schema_hash: String::new(),
        version: 2,
        report_id: String::new(),
        rekeyed: vec![],
    };
    assert_eq!(needed(&migrated, false), 5);
    for write in [
        ProposedWrite::Declare {
            declarations: vec![annotated],
        },
        ProposedWrite::Migrate {
            migration: Box::new(migration),
            report: Box::default(),
        },
    ] {
        let proposed = EventBody::WriteProposed {
            proposal: "p".into(),
            write,
            why: String::new(),
        };
        assert_eq!(needed(&proposed, false), 5);
    }
    let plain = EventBody::Declared {
        declaration: declaration(json!({
            "declare": "type", "name": "t", "annotations": {},
            "fields": [{"name": "a", "type": {"kind": "text"}, "annotations": {}}]
        })),
        schema_hash: None,
        version: 1,
    };
    assert_eq!(needed(&plain, false), 1, "an empty map is no annotation");
}

#[test]
fn a_derived_relation_needs_format_6_and_a_carried_one_does_not() {
    let with = |derived: bool| {
        let mut r = json!({"name": "customer", "target": "customer"});
        if derived {
            r["derived"] = json!({"match": [{"from": "customer_no", "to": "no"}]});
        }
        EventBody::Declared {
            declaration: declaration(json!({
                "declare": "type", "name": "invoice",
                "fields": [{"name": "customer_no", "type": {"kind": "text"}}],
                "relations": [r]
            })),
            schema_hash: None,
            version: 1,
        }
    };
    assert_eq!(needed(&with(true), false), 6);
    assert_eq!(needed(&with(false), false), 1);
}

#[test]
fn a_triggers_max_depth_needs_format_6() {
    let trigger = |max_depth: serde_json::Value| EventBody::Declared {
        declaration: declaration(json!({
            "declare": "trigger", "name": "t", "on": "job", "max_depth": max_depth
        })),
        schema_hash: None,
        version: 1,
    };
    assert_eq!(needed(&trigger(json!(1)), false), 6);
    assert_eq!(needed(&trigger(serde_json::Value::Null), false), 1);
}
