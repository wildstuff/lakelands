//! The contract's JSON forms, as a transport or a remote client sees
//! them. These are the shapes the doors put on the wire; a change here is
//! a change every client notices.

use lakelands_api::expr::{CmpOp, Condition, Expr, Literal};
use lakelands_api::query::{Pattern, Projection, Walk};
use lakelands_api::write::ProposedWrite;
use lakelands_api::{Error, Find, Outcome, Position, ReadAt, Refusal, Timestamp, Version};
use serde_json::json;

#[test]
fn find_takes_where_as_text_and_the_time_axes_flat() {
    let find: Find = serde_json::from_value(json!({
        "where": "status = 'open'",
        "order_by": [{"field": "due", "dir": "desc"}],
        "limit": 10,
        "as_of": 42,
        "valid_at": "2026-09-01",
        "with_meta": true,
    }))
    .unwrap();
    assert_eq!(find.filter, Some(Condition::Text("status = 'open'".into())));
    assert_eq!(
        find.at,
        ReadAt {
            as_of: Some(Position(42)),
            valid_at: Some(Timestamp::parse("2026-09-01").unwrap()),
            clock: None,
        }
    );
    assert!(find.with_meta);
}

#[test]
fn a_condition_is_text_or_the_typed_form_and_both_round_trip() {
    let typed = Condition::Expr(Expr::Cmp {
        cmp: CmpOp::Gt,
        lhs: Box::new(Expr::Path {
            path: vec!["amount".into()],
        }),
        rhs: Box::new(Expr::Lit {
            value: Literal::Number("100".into()),
        }),
    });
    for c in [Condition::Text("amount > 100".into()), typed] {
        let back: Condition = serde_json::from_value(serde_json::to_value(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }
}

#[test]
fn a_write_answers_applied_or_proposed_and_both_carry_a_position() {
    let applied = Outcome::Applied {
        position: Position(7),
        version: Some(Version(3)),
        output: None,
        set_off: Default::default(),
        renamed: Vec::new(),
        declared: Vec::new(),
        plan: None,
    };
    assert_eq!(
        serde_json::to_value(&applied).unwrap(),
        json!({"outcome": "applied", "position": 7, "version": 3})
    );
    let proposed: Outcome = serde_json::from_value(json!({
        "outcome": "proposed", "position": 8, "proposal": "p-1", "why": "risk high",
    }))
    .unwrap();
    assert_eq!(proposed.position(), Position(8));
}

#[test]
fn errors_are_a_closed_tagged_set_with_a_sentence_for_people() {
    let e = Error::Refused {
        field: Some("amount".into()),
        reason: Refusal::WrongDoor,
    };
    assert_eq!(
        serde_json::to_value(&e).unwrap(),
        json!({"error": "refused", "field": "amount", "reason": {"refusal": "wrong_door"},
            "message": "refused on `amount`: another authority does this, through another door"})
    );
    assert_eq!(
        e.to_string(),
        "refused on `amount`: another authority does this, through another door"
    );
}

#[test]
fn the_named_graph_reads_are_patterns() {
    let n = Pattern::neighbors("invoice".into(), "INV-1".into(), "debtor".into(), Walk::Out);
    assert_eq!(n.steps.len(), 1);
    let s = Pattern::subgraph("invoice".into(), "INV-1".into(), vec!["debtor".into()], 3);
    assert_eq!(s.project, Projection::Graph);
    let p = Pattern::path(
        ("invoice".into(), "INV-1".into()),
        ("partner".into(), "P-9".into()),
        vec!["debtor".into()],
        4,
    );
    assert_eq!(p.to, Some(("partner".into(), "P-9".into())));
}

#[test]
fn a_patch_is_sparse_and_null_clears() {
    let w: ProposedWrite = serde_json::from_value(json!({
        "write": "upsert", "type": "invoice", "key": "INV-1",
        "patch": {"note": null, "verdict": "disputed"},
    }))
    .unwrap();
    let ProposedWrite::Upsert { patch, .. } = w else {
        panic!("not an upsert")
    };
    assert_eq!(patch.len(), 2);
    assert!(patch["note"].is_null());
}

#[test]
fn a_typed_expression_refuses_members_it_does_not_know() {
    let ok = json!({"op": "path", "path": ["amount"]});
    assert!(serde_json::from_value::<Expr>(ok).is_ok());
    let extra = json!({"op": "path", "path": ["amount"], "colour": "blue"});
    assert!(serde_json::from_value::<Expr>(extra).is_err());
    let lit = json!({"op": "lit", "value": {"type": "number", "value": "1", "unit": "eur"}});
    assert!(serde_json::from_value::<Expr>(lit).is_err());
}

/// Reads a declaration that must be refused, and answers the sentence.
fn refused(declaration: serde_json::Value) -> String {
    let e = serde_json::from_value::<lakelands_api::write::Declaration>(declaration)
        .expect_err("a member the declaration does not take is refused");
    e.to_string()
}

fn text(name: &str) -> serde_json::Value {
    json!({"name": name, "type": {"kind": "text"}})
}

#[test]
fn a_declaration_refuses_a_member_it_does_not_know_naming_it_and_where() {
    // (declaration, the member, where it sits; "" for the top)
    let cases = [
        (
            json!({"declare": "type", "name": "t", "fields": [text("a")], "icon": "x"}),
            "icon",
            "",
        ),
        (
            json!({"declare": "type", "name": "t",
                   "fields": [{"name": "a", "type": {"kind": "text"}, "format": "iban"}]}),
            "format",
            "fields[0]",
        ),
        (
            json!({"declare": "type", "name": "t",
                   "fields": [{"name": "a", "type": {"kind": "decimal", "precision": 2}}]}),
            "precision",
            "fields[0].type",
        ),
        (
            json!({"declare": "type", "name": "t", "fields": [text("a")],
                   "relations": [{"name": "r", "target": "u", "colour": "red"}]}),
            "colour",
            "relations[0]",
        ),
        (
            json!({"declare": "type", "name": "t", "fields": [text("a")],
                   "relations": [{"name": "r", "target": "u",
                                  "fields": [{"name": "share", "type": {"kind": "number"}, "unit": "%"}]}]}),
            "unit",
            "relations[0].fields[0]",
        ),
        (
            json!({"declare": "type", "name": "v",
                   "view": {"source": "t", "measures": [], "having": "x > 1"}}),
            "having",
            "view",
        ),
        (
            json!({"declare": "type", "name": "v",
                   "view": {"source": "t", "measures": [{"name": "n", "expr": "count()", "as": "number"}]}}),
            "as",
            "view.measures[0]",
        ),
        (
            json!({"declare": "value_type", "name": "money", "kind": "decimal", "unit": "EUR"}),
            "unit",
            "",
        ),
        (
            json!({"declare": "verb", "name": "pay", "on": "t", "risk": "low",
                   "effect": "write", "set": [], "undo": "unpay"}),
            "undo",
            "",
        ),
        (
            json!({"declare": "verb", "name": "pay", "on": "t", "risk": "low", "effect": "write",
                   "set": [{"field": "a", "value": "'x'", "when": "true"}]}),
            "when",
            "set[0]",
        ),
        (
            json!({"declare": "verb", "name": "pay", "on": "t", "risk": "low", "effect": "write",
                   "set": [], "payload": [{"name": "p", "type": {"kind": "text"}, "hint": "?"}]}),
            "hint",
            "payload[0]",
        ),
        (
            json!({"declare": "trigger", "name": "late", "on": "t", "then": "notify"}),
            "then",
            "",
        ),
        (
            json!({"declare": "object_set", "name": "open", "of": "t", "where": "true", "limit": 5}),
            "limit",
            "",
        ),
        (
            json!({"declare": "function", "name": "f", "backing": "http", "timeout": 5}),
            "timeout",
            "",
        ),
        (
            json!({"declare": "function", "name": "f", "backing": "expr", "expr": "x",
                   "inputs": [{"name": "x", "type": {"kind": "number"}, "unit": "m"}]}),
            "unit",
            "inputs[0]",
        ),
    ];
    // The reader names the declaration and the path to the member; which
    // member it is, the doors find from the parse (lakelands#650).
    for (declaration, member, at) in cases {
        let said = refused(declaration.clone());
        let path = match at {
            "" => member.to_owned(),
            at => format!("{at}.{member}"),
        };
        assert!(
            said.contains(&format!(": at `{path}`: ")),
            "{declaration}: {said}"
        );
        let name = declaration["name"].as_str().unwrap();
        assert!(said.contains(&format!("`{name}`")), "{declaration}: {said}");
    }
}

#[test]
fn a_verb_refuses_the_member_of_the_other_effect() {
    let said = refused(
        json!({"declare": "verb", "name": "pay", "on": "t", "risk": "low",
                              "effect": "write", "set": [], "executor": "bank"}),
    );
    assert!(said.contains("`executor`"), "{said}");
    let said = refused(
        json!({"declare": "verb", "name": "pay", "on": "t", "risk": "high",
                              "effect": "external", "executor": "bank", "set": []}),
    );
    assert!(said.contains("`set`"), "{said}");
}

#[test]
fn a_proposed_declaration_or_migration_refuses_a_member_it_does_not_know() {
    let declare = json!({"write": "declare", "declarations": [
        {"declare": "type", "name": "t", "fields": [text("a")]},
        {"declare": "type", "name": "u", "fields": [{"name": "b", "type": {"kind": "text"}, "format": "iban"}]},
    ]});
    let said = serde_json::from_value::<ProposedWrite>(declare)
        .unwrap_err()
        .to_string();
    assert!(said.contains("`u`: at `fields[0].format`: "), "{said}");
    let migrate = json!({"write": "migrate", "migration": {
        "to": {"name": "t", "fields": [text("a")]}, "renames": [], "reason": "why"}});
    let said = serde_json::from_value::<ProposedWrite>(migrate)
        .unwrap_err()
        .to_string();
    assert!(said.contains("`reason`"), "{said}");
    let migrate = json!({"write": "migrate", "migration": {
        "to": {"name": "t", "fields": [text("a")], "icon": "x"}}});
    let said = serde_json::from_value::<ProposedWrite>(migrate)
        .unwrap_err()
        .to_string();
    assert!(said.contains("`icon`"), "{said}");
}

#[test]
fn a_strict_declaration_is_read_back_from_what_it_writes() {
    let declarations = [
        json!({"declare": "type", "name": "invoice", "key": ["number"],
               "fields": [
                   {"name": "number", "type": {"kind": "text"}, "required": true},
                   {"name": "partner", "type": {"kind": "reference", "target": "partner"}},
                   {"name": "amount", "type": {"kind": "decimal", "value_type": "money"},
                    "description": "gross", "label": "Amount"},
                   {"name": "late", "type": {"kind": "boolean"}, "authority": "derived",
                    "derived": "due < now()", "computed_at": "write", "depends_on": ["due"]},
                   {"name": "due", "type": {"kind": "date_time"}},
               ],
               "relations": [{"name": "paid_by", "target": "payment", "cardinality": "many_to_many",
                              "fields": [text("note")]}],
               "language": "de", "field_meta": false}),
        json!({"declare": "type", "name": "open_by_partner",
               "view": {"source": "invoice", "where": "late", "group_by": ["partner"],
                        "measures": [{"name": "total", "expr": "sum(amount)"}]}}),
        json!({"declare": "value_type", "name": "money", "kind": "decimal", "scale": 2}),
        json!({"declare": "verb", "name": "settle", "on": "invoice", "risk": "low",
               "payload": [text("note")], "effect": "write",
               "set": [{"field": "late", "value": "false"}]}),
        json!({"declare": "verb", "name": "remind", "on": "invoice", "risk": "high",
               "requires": "late", "effect": "external", "executor": "mail"}),
        json!({"declare": "trigger", "name": "went_late", "on": "invoice", "when": "changed(late)"}),
        json!({"declare": "object_set", "name": "late_ones", "of": "invoice", "where": "late"}),
        json!({"declare": "function", "name": "double", "backing": "expr",
               "inputs": [{"name": "x", "type": {"kind": "number"}}], "expr": "x * 2"}),
    ];
    for declaration in declarations {
        let read: lakelands_api::write::Declaration = serde_json::from_value(declaration.clone())
            .unwrap_or_else(|e| panic!("{declaration}: {e}"));
        let written = serde_json::to_value(&read).unwrap();
        let again: lakelands_api::write::Declaration =
            serde_json::from_value(written.clone()).unwrap_or_else(|e| panic!("{written}: {e}"));
        assert_eq!(again, read);
        assert_eq!(serde_json::to_value(&again).unwrap(), written);
        assert_eq!(written["declare"], declaration["declare"]);
    }
}

/// A declaration is judged as a written value is: an integer anywhere
/// in it beyond ±(2^53 − 1), in whatever form it arrived, is said at the
/// declaration and the path of the member that holds it.
#[test]
fn a_declaration_is_refused_for_an_integer_no_reader_reads_exactly() {
    use lakelands_api::error::{Case, Place};
    use lakelands_api::write::Declaration;
    let fault = |d: serde_json::Value| {
        let d: Declaration = serde_json::from_value(d).unwrap();
        d.integer_fault()
    };
    let function = |config: serde_json::Value| json!({"declare": "function", "name": "f", "backing": "http", "config": config});
    let duration =
        |micros: u64| json!({"op": "lit", "value": {"type": "duration", "value": micros}});
    let set = |micros: u64| json!({"declare": "object_set", "name": "s", "of": "t", "where": duration(micros)});
    let text =
        |filter: &str| json!({"declare": "object_set", "name": "s", "of": "t", "where": filter});
    let (f, s) = (
        Place::Function { name: "f".into() },
        Place::ObjectSet { name: "s".into() },
    );
    for (d, place, path) in [
        (function(json!(1e16)), &f, "config"),
        (
            function(json!({"a": [{"b": 9007199254740993_u64}]})),
            &f,
            "config.a[0].b",
        ),
        (set(9007199254740992), &s, "where.value.value"),
        (text("now() > 104250d"), &s, "where"),
        // Another member holding the same text is not the one named.
        (
            json!({"declare": "object_set", "name": "s", "of": "t",
                   "description": "now() > 104250d", "where": "now() > 104250d"}),
            &s,
            "where",
        ),
    ] {
        let said = fault(d.clone()).unwrap_or_else(|| panic!("{d}"));
        let path = path.to_owned();
        assert_eq!(said.at, [place.clone(), Place::Member { path }], "{d}");
        assert!(matches!(said.case, Case::IntegerTooLarge { .. }), "{d}");
    }
    for d in [
        function(json!(null)),
        function(json!({"a": 9007199254740991_u64, "b": 1e21})),
        set(9007199254740991),
        text("now() > 104249d"),
        // A number literal is kept as it is written, in both forms.
        text("n > 9007199254740993"),
        text("not an expression 110000d"),
    ] {
        assert_eq!(fault(d.clone()), None, "{d}");
    }
}

/// Every member of a declaration that holds an expression is judged: a
/// span no duration holds, in text form, is said at that member.
#[test]
fn an_expression_in_text_form_is_judged_wherever_a_declaration_holds_one() {
    use lakelands_api::error::Place;
    use lakelands_api::write::Declaration;
    let long = "now() > 104250d";
    let measure = json!([{"name": "n", "expr": long}]);
    let derived =
        json!({"name": "f", "type": {"kind": "boolean"}, "authority": "derived", "derived": long});
    let ty = |more: serde_json::Value| {
        let mut d = json!({"declare": "type", "name": "t"});
        d.as_object_mut()
            .unwrap()
            .extend(more.as_object().unwrap().clone());
        d
    };
    let verb = |more: serde_json::Value| {
        let mut d = json!({"declare": "verb", "name": "v", "on": "t", "risk": "low", "effect": "write", "set": []});
        d.as_object_mut()
            .unwrap()
            .extend(more.as_object().unwrap().clone());
        d
    };
    for (d, path) in [
        (
            ty(json!({"fields": [derived.clone()]})),
            "fields[0].derived",
        ),
        (
            ty(json!({"relations": [{"name": "r", "target": "t", "fields": [derived]}]})),
            "relations[0].fields[0].derived",
        ),
        (
            ty(json!({"view": {"source": "s", "where": long, "measures": []}})),
            "view.where",
        ),
        (
            ty(json!({"view": {"source": "s", "measures": measure}})),
            "view.measures[0].expr",
        ),
        (
            ty(
                json!({"view": {"source": "s", "measures": [], "merge": [{"source": "u", "where": long, "measures": []}]}}),
            ),
            "view.merge[0].where",
        ),
        (
            ty(
                json!({"view": {"source": "s", "measures": [], "merge": [{"source": "u", "measures": measure}]}}),
            ),
            "view.merge[0].measures[0].expr",
        ),
        (
            json!({"declare": "value_type", "name": "x", "kind": "date_time", "constraint": long}),
            "constraint",
        ),
        (verb(json!({"requires": long})), "requires"),
        (
            verb(json!({"set": [{"field": "f", "value": long}]})),
            "set[0].value",
        ),
        (
            json!({"declare": "trigger", "name": "g", "on": "t", "when": long}),
            "when",
        ),
        (
            json!({"declare": "function", "name": "e", "backing": "expr", "expr": long}),
            "expr",
        ),
        (
            json!({"declare": "object_set", "name": "s", "of": "t", "where": long}),
            "where",
        ),
    ] {
        let read: Declaration =
            serde_json::from_value(d.clone()).unwrap_or_else(|e| panic!("{d}: {e}"));
        let said = read.integer_fault().unwrap_or_else(|| panic!("{d}"));
        let member = Place::Member {
            path: path.to_owned(),
        };
        assert_eq!(said.at.last(), Some(&member), "{d}");
    }
}

#[test]
fn an_annotation_is_refused_naming_the_declaration_the_key_and_where_it_sits() {
    use lakelands_api::error::{Case, Place};
    use lakelands_api::write::{AnnotationFault, Declaration};
    let fault = |d: serde_json::Value| {
        serde_json::from_value::<Declaration>(d)
            .unwrap()
            .annotation_fault()
    };
    let (ty, verb, function) = (
        Place::Type { name: "t".into() },
        Place::Verb { name: "v".into() },
        Place::Function { name: "f".into() },
    );
    let field = |name: &str| Place::Field { name: name.into() };
    let key = |key: &str| Place::Annotation { key: key.into() };
    let long = format!("acme.{}", "k".repeat(124));
    let cases = [
        (
            json!({"declare": "type", "name": "t", "annotations": {long.clone(): 1}}),
            vec![ty.clone(), key(&long)],
            Case::AnnotationKeyLong { max: 128 },
        ),
        (
            json!({"declare": "type", "name": "t", "fields": [
                {"name": "a", "type": {"kind": "text"}, "annotations": {"acme.n": 9007199254740992_u64}}]}),
            vec![ty.clone(), field("a"), key("acme.n")],
            Case::IntegerTooLarge {
                n: 9007199254740992_u64.into(),
            },
        ),
        (
            json!({"declare": "type", "name": "t", "annotations": {"acme.n": 1e16}}),
            vec![ty.clone(), key("acme.n")],
            Case::IntegerTooLarge {
                n: serde_json::Number::from_f64(1e16).expect("finite"),
            },
        ),
        (
            json!({"declare": "type", "name": "t", "relations": [{"name": "r", "target": "u",
                "fields": [{"name": "s", "type": {"kind": "text"}, "annotations": {"acme.": 1}}]}]}),
            vec![
                ty.clone(),
                Place::Relation { name: "r".into() },
                field("s"),
                key("acme."),
            ],
            Case::AnnotationKeyForm,
        ),
        (
            json!({"declare": "verb", "name": "v", "on": "t", "risk": "low", "effect": "write", "set": [],
                "payload": [{"name": "p", "type": {"kind": "text"}, "annotations": {"1acme.x": 1}}]}),
            vec![verb, field("p"), key("1acme.x")],
            Case::AnnotationKeyForm,
        ),
        (
            json!({"declare": "function", "name": "f", "backing": "http",
                "inputs": [{"name": "x", "type": {"kind": "text"}, "annotations": {"acme.x": null}}]}),
            vec![function, field("x"), key("acme.x")],
            Case::AnnotationNull,
        ),
    ];
    for (d, at, case) in cases {
        let Some(AnnotationFault::Invalid { said }) = fault(d.clone()) else {
            panic!("{d} must be refused as invalid");
        };
        assert_eq!((said.at, said.case), (at, case), "{d}");
    }
    // A key of exactly 128 bytes is taken, one of 129 is not.
    let at_limit = format!("acme.{}", "k".repeat(123));
    assert_eq!(
        fault(json!({"declare": "type", "name": "t", "annotations": {at_limit: 1}})),
        None
    );
    let Some(AnnotationFault::Reserved { key, said }) =
        fault(json!({"declare": "type", "name": "t", "annotations": {"lakelands.x": 1}}))
    else {
        panic!("`lakelands.` is reserved");
    };
    assert_eq!(key, "lakelands.x");
    assert_eq!(
        said.to_string(),
        "type `t`, annotation `lakelands.x`: the namespace `lakelands.` is reserved"
    );
}

/// A type's rights from an older server, which says nothing of `create`,
/// read as a create its `upsert` speaks for (lakelands#1188).
#[test]
fn rights_without_create_read_as_open() {
    let old = json!({"find": true, "get": true, "upsert": true, "retire": false});
    let may: lakelands_api::describe::TypeRights = serde_json::from_value(old).unwrap();
    assert!(may.create && may.create_why.is_none(), "{may:?}");
}
