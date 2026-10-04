use super::*;
use serde_json::json;

fn declaration(v: serde_json::Value) -> Declaration {
    serde_json::from_value(v).unwrap()
}

fn declared(declaration: Declaration) -> EventBody {
    EventBody::Declared {
        declaration,
        schema_hash: None,
        version: 1,
    }
}

/// An event of log format `v` at position 1 whose body is `body`.
fn event_json(v: u32, body: serde_json::Value) -> Event {
    serde_json::from_value(json!({
        "v": v, "lake": "l", "position": 1, "id": "0".repeat(32),
        "prev": "0".repeat(64), "known_at": "2026-01-01T00:00:00.000000Z",
        "writer": "system:lakelands", "origin": "system",
        "context": {"origin": "direct", "depth": 0}, "fence": 1,
        "body": body, "hash": "0".repeat(64)
    }))
    .unwrap()
}

/// An event of log format `v` declaring `declaration`, at version 1.
fn declared_event(v: u32, declaration: impl serde::Serialize) -> Event {
    event_json(
        v,
        json!({"kind": "declared", "declaration": declaration, "version": 1}),
    )
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
    let declared = declared(annotated.clone());
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
        declared(declaration(json!({
            "declare": "type", "name": "invoice",
            "fields": [{"name": "customer_no", "type": {"kind": "text"}}],
            "relations": [r]
        })))
    };
    assert_eq!(needed(&with(true), false), 6);
    assert_eq!(needed(&with(false), false), 1);
}

#[test]
fn a_triggers_max_depth_needs_format_6() {
    let trigger = |max_depth: serde_json::Value| {
        declared(declaration(json!({
            "declare": "trigger", "name": "t", "on": "job", "max_depth": max_depth
        })))
    };
    assert_eq!(needed(&trigger(json!(1)), false), 6);
    assert_eq!(needed(&trigger(serde_json::Value::Null), false), 1);
}

#[test]
fn a_declaration_calling_retired_needs_format_7_and_says_so_below_it() {
    let guard = |when: &str| {
        declaration(json!({
            "declare": "trigger", "name": "t", "on": "job", "when": when
        }))
    };
    assert_eq!(needed(&declared(guard("not retired()")), false), 7);
    assert_eq!(needed(&declared(guard("status = 'retired'")), false), 1);
    let proposed = EventBody::WriteProposed {
        proposal: "p".into(),
        write: ProposedWrite::Declare {
            declarations: vec![guard("retired()")],
        },
        why: String::new(),
    };
    assert_eq!(needed(&proposed, false), 7, "a proposal keeps it too");
    let event = declared_event(6, guard("retired()"));
    assert_eq!(
        below(&event).as_deref(),
        Some("a format 6 event declares a call of `retired`, which format 7 added")
    );
}

#[test]
fn a_view_with_parameters_needs_format_8_and_says_so_below_it() {
    let view = |params: serde_json::Value, filter: &str| {
        declaration(json!({
            "declare": "type", "name": "window", "key": ["account"],
            "fields": [{"name": "account", "type": {"kind": "text"}},
                       {"name": "n", "type": {"kind": "number"}}],
            "view": {"source": "payment", "params": params, "where": filter,
                     "group_by": ["account"],
                     "measures": [{"name": "n", "expr": "count()"}]}
        }))
    };
    let anchored = view(
        json!([{"name": "anchor", "kind": "date_time"}]),
        "date <= param('anchor')",
    );
    assert_eq!(needed(&declared(anchored.clone()), false), 8);
    assert_eq!(
        needed(&declared(view(json!([]), "date <= '2026-06-30'")), false),
        1
    );
    let event = declared_event(7, anchored);
    assert_eq!(
        below(&event).as_deref(),
        Some("a format 7 event declares a view's parameters, which format 8 added")
    );
}

#[test]
fn a_verb_on_its_type_needs_format_8_declared_called_or_proposed() {
    let verb = |scope: Option<&str>| {
        let mut v = json!({"declare": "verb", "name": "create", "on": "payment",
                           "risk": "low", "effect": "external", "executor": "erp"});
        if let Some(s) = scope {
            v["scope"] = json!(s);
        }
        declaration(v)
    };
    assert_eq!(needed(&declared(verb(Some("type"))), false), 8);
    assert_eq!(needed(&declared(verb(None)), false), 1);
    let called = |key: Option<&str>| EventBody::VerbCalled {
        verb: "create".into(),
        key: key.map(Into::into),
        payload: Default::default(),
    };
    assert_eq!(needed(&called(None), false), 8);
    assert_eq!(needed(&called(Some("k")), false), 1);
    let proposed = |key: Option<&str>| EventBody::WriteProposed {
        proposal: "p".into(),
        write: ProposedWrite::Call {
            verb: "create".into(),
            key: key.map(Into::into),
            payload: Default::default(),
            opts: Default::default(),
        },
        why: String::new(),
    };
    assert_eq!(needed(&proposed(None), false), 8);
    assert_eq!(needed(&proposed(Some("k")), false), 1);
    let event = |body: EventBody| -> Event {
        let mut e: Event = event_json(
            7,
            json!({"kind": "checkpoint", "through": 0, "hash": "0".repeat(64), "segments": []}),
        );
        e.body = body;
        e
    };
    assert_eq!(
        below(&event(declared(verb(Some("type"))))).as_deref(),
        Some("a format 7 event declares a verb on its type, which format 8 added")
    );
    assert_eq!(
        below(&event(called(None))).as_deref(),
        Some("a format 7 event calls a verb with no key, which format 8 added")
    );
}

#[test]
fn a_view_that_merges_sources_needs_format_9_and_says_so_below_it() {
    let view = |merge: serde_json::Value| {
        declaration(json!({
            "declare": "type", "name": "balance", "key": ["day"],
            "fields": [{"name": "day", "type": {"kind": "date_time"}},
                       {"name": "kin", "type": {"kind": "number"}},
                       {"name": "kout", "type": {"kind": "number"}}],
            "view": {"source": "food", "group_by": ["day"],
                     "measures": [{"name": "kin", "expr": "count()"}],
                     "merge": merge}
        }))
    };
    let merged = view(json!([{"source": "workout", "group_by": ["date"],
                             "measures": [{"name": "kout", "expr": "count()"}]}]));
    assert_eq!(needed(&declared(merged.clone()), false), 9);
    assert_eq!(needed(&declared(view(json!([]))), false), 1);
    let event = declared_event(8, merged);
    assert_eq!(
        below(&event).as_deref(),
        Some("a format 8 event declares a view that merges sources, which format 9 added")
    );
}

#[test]
fn a_proposed_tightening_of_a_value_type_needs_format_10_and_says_so_below_it() {
    let proposed = |write: serde_json::Value| json!({"kind": "write_proposed", "proposal": "p", "write": write, "why": "w"});
    let to = json!({"name": "state", "kind": "text", "one_of": ["open"]});
    let narrower = proposed(json!({"write": "migrate_value_type", "to": to}));
    let declared = proposed(json!({"write": "declare", "declarations": [
        {"declare": "value_type", "name": "state", "kind": "text", "one_of": ["open"]}]}));
    let need = |v, body: &serde_json::Value| needed(&event_json(v, body.clone()).body, false);
    assert_eq!((need(10, &narrower), need(4, &declared)), (10, 4));
    assert_eq!(
        below(&event_json(9, narrower)).as_deref(),
        Some("a format 9 event proposes a value type's tightening, which format 10 added")
    );
}

#[test]
fn a_declared_call_of_floor_needs_format_11_and_says_so_below_it() {
    let days = |expr: &str| {
        json!({"declare": "type", "name": "t", "fields": [
            {"name": "a", "type": {"kind": "date_time"}},
            {"name": "days", "type": {"kind": "number"}, "authority": "derived", "derived": expr}]})
    };
    let (whole, exact) = (days("floor((now() - a) / 1d)"), days("(now() - a) / 1d"));
    let need = |d: &serde_json::Value| needed(&declared_event(11, d).body, false);
    assert_eq!((need(&whole), need(&exact)), (11, 1));
    assert_eq!(
        below(&declared_event(10, &whole)).as_deref(),
        Some("a format 10 event declares a call of `floor`, which format 11 added")
    );
}
