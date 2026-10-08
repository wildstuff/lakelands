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
        with_the_set: false,
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
    let kind = "record_retired".to_owned();
    assert_eq!(why, lakelands_api::error::Flaw::UpstreamOn { kind });
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
        relation_renames: Vec::new(),
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
        with_the_set: false,
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
        below(&event).map(|f| f.to_string()).as_deref(),
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
        below(&event).map(|f| f.to_string()).as_deref(),
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
        below(&event(declared(verb(Some("type")))))
            .map(|f| f.to_string())
            .as_deref(),
        Some("a format 7 event declares a verb on its type, which format 8 added")
    );
    assert_eq!(
        below(&event(called(None)))
            .map(|f| f.to_string())
            .as_deref(),
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
        below(&event).map(|f| f.to_string()).as_deref(),
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
        below(&event_json(9, narrower))
            .map(|f| f.to_string())
            .as_deref(),
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
        below(&declared_event(10, &whole))
            .map(|f| f.to_string())
            .as_deref(),
        Some("a format 10 event declares a call of `floor`, which format 11 added")
    );
    // A bucket by a width is format 11's; by a calendar unit it stays 3's.
    let (window, month) = (
        days("date_bucket(a, 7d, a)"),
        days("date_bucket(a, 'month')"),
    );
    assert_eq!((need(&window), need(&month)), (11, 3));
    assert_eq!(
        below(&declared_event(10, &window))
            .map(|f| f.to_string())
            .as_deref(),
        Some(
            "a format 10 event declares a call of `date_bucket` by a width, which format 11 added"
        )
    );
    // A set that names a set is format 11's too.
    let urgent = json!({"declare": "object_set", "name": "urgent", "of": "t",
                        "where": "in_set('open')"});
    assert_eq!(need(&urgent), 11);
    assert_eq!(
        below(&declared_event(10, &urgent))
            .map(|f| f.to_string())
            .as_deref(),
        Some("a format 10 event declares a call of `in_set`, which format 11 added")
    );
}

#[test]
fn a_record_event_that_carries_made_needs_format_11_and_says_so_below_it() {
    let record = |kind: &str, made: bool| {
        let mut body = json!({"kind": kind, "type": "note", "key": "k", "version": 4});
        body["patch"] = json!({"f": 1});
        if made {
            body["made"] = json!(true);
        }
        body
    };
    for kind in ["record_upserted", "record_ingested"] {
        assert_eq!(needed(&event_json(11, record(kind, true)).body, false), 11);
        assert_eq!(
            below(&event_json(10, record(kind, true)))
                .map(|f| f.to_string())
                .as_deref(),
            Some("a format 10 event carries `made`, which format 11 added")
        );
        // A change says nothing, needs nothing, and is written without it.
        let changed = event_json(1, record(kind, false));
        let written = serde_json::to_string(&changed).unwrap();
        assert!(needed(&changed.body, false) == 1 && !written.contains("made"));
    }
}

#[test]
fn a_declared_initial_value_needs_format_11_and_says_so_below_it() {
    let f = |initial: bool| {
        let mut f = json!({"name": "f", "type": {"kind": "text"}});
        if initial {
            f["initial"] = json!("new");
        }
        f
    };
    // Wherever a field is declared, though only a type's own may hold it.
    let places = |i: bool| {
        [
            json!({"declare": "type", "name": "t", "fields": [f(i)]}),
            json!({"declare": "type", "name": "t", "fields": [],
                   "relations": [{"name": "r", "target": "t", "fields": [f(i)]}]}),
            json!({"declare": "verb", "name": "v", "on": "t", "risk": "low", "payload": [f(i)],
                   "effect": "write", "set": [{"field": "f", "value": "'x'"}]}),
            json!({"declare": "function", "name": "g", "backing": "expr", "expr": "f",
                   "inputs": [f(i)]}),
        ]
    };
    for (with, without) in places(true).iter().zip(&places(false)) {
        assert_eq!(needed(&declared_event(11, with).body, false), 11, "{with}");
        assert_eq!(
            needed(&declared_event(1, without).body, false),
            1,
            "{without}"
        );
        assert_eq!(
            below(&declared_event(10, with))
                .map(|f| f.to_string())
                .as_deref(),
            Some("a format 10 event declares an `initial` value, which format 11 added")
        );
    }
    let to = json!({"name": "t", "fields": [f(true)]});
    let migrated = json!({"kind": "migrated", "migration": {"to": to}, "version": 2,
                          "schema_hash": "0", "report_id": "r"});
    assert_eq!(needed(&event_json(11, migrated).body, false), 11);
    let declare = json!({"write": "declare", "declarations": [places(true)[0]]});
    let proposed = json!({"kind": "write_proposed", "proposal": "p", "write": declare, "why": "w"});
    assert_eq!(needed(&event_json(11, proposed).body, false), 11);
}

#[test]
fn a_write_of_a_view_kept_setting_needs_format_11_and_says_so_below_it() {
    let record = |kind: &str, key: &str| {
        let patch = json!({"key": key, "value": true});
        json!({"kind": kind, "type": "sys.setting", "key": key, "version": 1, "patch": patch})
    };
    let proposed = |ty: &str, key: &str| {
        let write = json!({"write": "upsert", "type": ty, "key": key, "patch": {"value": true}});
        json!({"kind": "write_proposed", "proposal": "p", "write": write, "why": "w"})
    };
    let need = |body: serde_json::Value| needed(&event_json(11, body).body, false);
    let kept = "view_kept.totals";
    for body in [
        record("record_upserted", kept),
        record("record_ingested", kept),
        proposed("sys.setting", kept),
    ] {
        assert_eq!(need(body.clone()), 11, "{body}");
        assert_eq!(
            below(&event_json(10, body))
                .map(|f| f.to_string())
                .as_deref(),
            Some("a format 10 event writes a `view_kept.` setting, which format 11 added")
        );
    }
    // Any other setting, and a record of another type under that key.
    assert_eq!(need(record("record_upserted", "sort_index.totals")), 1);
    assert_eq!(need(proposed("note", kept)), 1);
    let mut other = record("record_upserted", kept);
    other["type"] = json!("note");
    assert_eq!(need(other), 1);
}

#[test]
fn a_proposed_migration_whose_report_restates_needs_format_11_and_says_so_below_it() {
    let proposed = |restated: Option<serde_json::Value>| {
        let mut report = json!({"type": "t", "from_schema_hash": "a", "to_schema_hash": "a",
            "records_scanned": 0, "fields": [], "lossless": true, "report_id": "r"});
        report["restated"] = restated.unwrap_or(json!([]));
        let migration = json!({"to": {"name": "t", "fields": []}});
        let write = json!({"write": "migrate", "migration": migration, "report": report});
        json!({"kind": "write_proposed", "proposal": "p", "why": "w", "write": write})
    };
    let mark = json!([{"field": "f", "member": "sensitive", "was": true, "to": false}]);
    let restates = event_json(11, proposed(Some(mark.clone())));
    assert_eq!(needed(&restates.body, false), 11);
    assert_eq!(
        below(&event_json(10, proposed(Some(mark))))
            .map(|f| f.to_string())
            .as_deref(),
        Some(
            "a format 10 event proposes a migration whose report holds `restated`, which format \
             11 added"
        )
    );
    // One that restates nothing needs what it did, and is written without it.
    let plain = event_json(4, proposed(None));
    let written = serde_json::to_string(&plain).unwrap();
    assert!(needed(&plain.body, false) == 4 && !written.contains("restated"));
}

#[test]
fn a_declaration_with_the_set_needs_format_12_and_says_so_below_it() {
    let f = json!({"declare": "function", "name": "f", "backing": "expr", "expr": "n * 2",
                   "inputs": [{"name": "n", "type": {"kind": "number"}}]});
    let body = |set: bool| {
        json!({"kind": "declared", "declaration": f, "version": 1,
                                  "with_the_set": set})
    };
    let alone = json!({"kind": "declared", "declaration": f, "version": 1});
    assert_eq!(needed(&event_json(12, body(true)).body, false), 12);
    assert_eq!(needed(&event_json(12, alone).body, false), 1);
    assert_eq!(
        below(&event_json(11, body(true)))
            .map(|f| f.to_string())
            .as_deref(),
        Some("a format 11 event declares `with_the_set`, which format 12 added")
    );
}

#[test]
fn a_value_type_rename_needs_format_12_and_says_so_below_it() {
    let vt = json!({"name": "s", "kind": "text", "one_of": ["b"]});
    let migrated = json!({"kind": "value_type_migrated", "declaration": vt, "version": 2,
                          "renames": [["a", "b"]], "report_id": "r", "types": ["t"]});
    let proposed = |renames: serde_json::Value| {
        json!({"kind": "write_proposed", "proposal": "p", "why": "w",
               "write": {"write": "migrate_value_type", "to": vt, "renames": renames}})
    };
    let renames = event_json(12, proposed(json!([["a", "b"]])));
    assert_eq!(needed(&event_json(12, migrated.clone()).body, false), 12);
    assert_eq!(needed(&renames.body, false), 12);
    // A proposal that renames nothing needs what a tightening did.
    assert_eq!(needed(&event_json(10, proposed(json!([]))).body, false), 10);
    assert_eq!(
        below(&event_json(11, migrated))
            .map(|f| f.to_string())
            .as_deref(),
        Some("a format 11 event renames a value type's values, which format 12 added")
    );
    assert_eq!(
        below(&event_json(11, proposed(json!([["a", "b"]]))))
            .map(|f| f.to_string())
            .as_deref(),
        Some(
            "a format 11 event proposes a value type's migration that renames values, which \
             format 12 added"
        )
    );
}
