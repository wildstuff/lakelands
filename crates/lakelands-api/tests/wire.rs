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
            "message": "refused on `amount`: another authority writes this field"})
    );
    assert_eq!(
        e.to_string(),
        "refused on `amount`: another authority writes this field"
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

/// A function's `config` is judged as a written value is: an integer in
/// it beyond ±(2^53 − 1), in whatever form it arrived, is said at the
/// function and its `config`; no other declaration has one.
#[test]
fn a_functions_config_is_refused_for_an_integer_no_reader_reads_exactly() {
    use lakelands_api::error::{Case, Place};
    use lakelands_api::write::Declaration;
    let fault = |config: serde_json::Value| {
        let f = json!({"declare": "function", "name": "f", "backing": "http", "config": config});
        let d: Declaration = serde_json::from_value(f).unwrap();
        d.config_fault()
    };
    let at = vec![
        Place::Function { name: "f".into() },
        Place::Member {
            path: "config".into(),
        },
    ];
    for config in [json!(1e16), json!({"a": [{"b": 9007199254740993_u64}]})] {
        let said = fault(config.clone()).unwrap_or_else(|| panic!("{config}"));
        assert_eq!(said.at, at, "{config}");
        assert!(
            matches!(said.case, Case::IntegerTooLarge { .. }),
            "{config}"
        );
    }
    for config in [json!(null), json!({"a": 9007199254740991_u64, "b": 1e21})] {
        assert_eq!(fault(config.clone()), None, "{config}");
    }
    let ty: Declaration = serde_json::from_value(json!({"declare": "type", "name": "t"})).unwrap();
    assert_eq!(ty.config_fault(), None);
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
