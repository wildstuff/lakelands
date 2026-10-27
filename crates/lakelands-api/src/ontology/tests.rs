use super::*;
use serde_json::json;

fn ty(name: &str, fields: serde_json::Value) -> TypeDecl {
    serde_json::from_value(json!({ "name": name, "fields": fields })).unwrap()
}

fn lake() -> Vec<TypeDecl> {
    let reference = |name: &str, to: &str| json!({ "name": name, "type": { "kind": "reference", "target": to } });
    let text = |name: &str| json!({ "name": name, "type": { "kind": "text" } });
    let mut invoice = ty(
        "invoice",
        json!([
            text("number"),
            reference("customer", "customer"),
            text("note")
        ]),
    );
    invoice.relations = serde_json::from_value(json!([
        { "name": "with", "target": "customer" },
        { "name": "by", "target": "customer", "via": "customer" }
    ]))
    .unwrap();
    let link = json!([
        reference("from", "invoice"),
        reference("to", "customer"),
        text("note")
    ]);
    vec![
        invoice,
        ty("sys.link.invoice.with", link),
        ty(
            "customer",
            json!([
                text("name"),
                reference("term", "term"),
                reference("lost", "gone")
            ]),
        ),
        ty("term", json!([text("days")])),
    ]
}

/// The path's fields as `type.field`, in the order it reads them.
fn read(path: &str) -> Vec<String> {
    let types = lake();
    let by_name = |n: &TypeName| types.iter().find(|t| &t.name == n);
    let path: Vec<FieldName> = path.split('.').map(FieldName::from).collect();
    let steps = types[0].stepped(&path, &by_name);
    steps
        .iter()
        .map(|(t, f)| format!("{}.{}", t.name, f.name))
        .collect()
}

#[test]
fn a_path_steps_through_each_reference_to_the_field_it_names() {
    assert_eq!(read("note"), ["invoice.note"]);
    assert_eq!(read("customer"), ["invoice.customer"]);
    assert_eq!(read("customer.name"), ["invoice.customer", "customer.name"]);
    let two = ["invoice.customer", "customer.term", "term.days"];
    assert_eq!(read("customer.term.days"), two);
}

#[test]
fn a_path_stops_where_there_is_no_record_to_step_to() {
    // Past a field that is not a reference, what is left digs into its value.
    assert_eq!(read("note.lines.first"), ["invoice.note"]);
    // A field the type does not have reads nothing, here or further on.
    assert_eq!(read("total"), Vec::<String>::new());
    assert_eq!(read("customer.total"), ["invoice.customer"]);
    // A reference to a type the lake does not hold steps nowhere.
    assert_eq!(
        read("customer.lost.name"),
        ["invoice.customer", "customer.lost"]
    );
}

#[test]
fn a_path_steps_from_a_relation_carried_by_links_to_its_link() {
    let (from, note) = ("sys.link.invoice.with.from", "sys.link.invoice.with.note");
    assert_eq!(read("with.note"), [from, note]);
    let far = [from, "sys.link.invoice.with.to", "customer.name"];
    assert_eq!(read("with.to.name"), far);
    // Its name alone, and a relation carried by a field, step nowhere.
    assert_eq!(read("with"), Vec::<String>::new());
    assert_eq!(read("by.name"), Vec::<String>::new());
}

/// `Authority::ALL` holds every authority in its place: a new one stops
/// this compiling until it is placed, since the type is `#[non_exhaustive]`
/// only outside this crate.
#[test]
fn every_authority_is_listed() {
    let place = |a: Authority| match a {
        Authority::Authored => 0,
        Authority::Source => 1,
        Authority::Shared => 2,
        Authority::Derived => 3,
        Authority::Verb => 4,
        Authority::Writer => 5,
    };
    let places: Vec<usize> = Authority::ALL.into_iter().map(place).collect();
    assert_eq!(places, (0..6).collect::<Vec<_>>());
}

#[test]
fn a_constraint_reads_its_record_alone_as_declare_judges_it() {
    let text = |name: &str| json!({ "name": name, "type": { "kind": "text" } });
    let number = |name: &str| json!({ "name": name, "type": { "kind": "number" } });
    let mut t = ty(
        "t",
        json!([
            text("no"),
            number("n"),
            {"name": "late", "type": {"kind": "boolean"}, "authority": "derived",
             "derived": "n > 1", "depends_on": ["n"], "computed_at": "read"},
            {"name": "v", "type": {"kind": "vector", "dimensions": 2},
             "embed": {"from": ["no"], "model": "m"}},
            {"name": "scored", "type": {"kind": "number"}, "authority": "derived",
             "function": "score"},
            {"name": "doubled", "type": {"kind": "number"}, "authority": "derived",
             "function": "double"}
        ]),
    );
    t.relations = serde_json::from_value(json!([{ "name": "next", "target": "t" }])).unwrap();
    let functions: Vec<crate::write::FunctionDecl> = serde_json::from_value(json!([
        {"name": "score", "backing": "http"},
        {"name": "double", "backing": "expr", "expr": "n * 2"}
    ]))
    .unwrap();
    let function = |n: &str| functions.iter().find(|f| f.name == n);
    let reach = |holds: &str| {
        let c = ConstraintDecl {
            name: "c".into(),
            holds: holds.into(),
            description: None,
            at_ingest: AtIngest::Refuse,
        };
        t.constraint_reach(&c, function).map(|_| ())
    };
    let beyond = |holds: &str| match reach(holds) {
        Err(Case::ConstraintReadsBeyond { reads, .. }) => Some(*reads),
        _ => None,
    };
    let field = |name: &str| FieldName::from(name);
    for (holds, reads) in [
        ("next.n > 1", Beyond::Path),
        ("in_set('s')", Beyond::InSet),
        ("setting('cap') = 'x'", Beyond::Setting),
        ("now() > now()", Beyond::Clock),
        ("today() = today()", Beyond::Clock),
        ("exists(next)", Beyond::Exists),
        (
            "late",
            Beyond::ComputedAtRead {
                field: field("late"),
            },
        ),
        ("v is null", Beyond::ComputedOutside { field: field("v") }),
        (
            "scored > 0",
            Beyond::ComputedOutside {
                field: field("scored"),
            },
        ),
        // The first that holds, in declare's order.
        ("next.n > 1 and in_set('s')", Beyond::Path),
        ("in_set('s') and setting('cap') = 'x'", Beyond::InSet),
        ("setting('cap') = 'x' and now() > now()", Beyond::Setting),
        ("now() > now() and exists(next)", Beyond::Clock),
        ("exists(next) and late", Beyond::Exists),
        (
            "late and nope",
            Beyond::ComputedAtRead {
                field: field("late"),
            },
        ),
    ] {
        assert_eq!(beyond(holds), Some(reads), "{holds}");
    }
    for holds in [
        "n > 0",
        "doubled > 0",
        "known_at(n) > seen_at(n)",
        "changed(n) or previous(n) < n",
    ] {
        assert_eq!(reach(holds), Ok(()), "{holds}");
    }
    assert!(
        matches!(reach("nope > 0"), Err(Case::UnknownField { ty, field })
            if ty.as_str() == "t" && field.as_str() == "nope")
    );
    assert!(matches!(reach("n >"), Err(Case::NotParsed { .. })));
    // Checked in a guard's place before what it reads is asked.
    assert!(matches!(
        reach("count(n) > now()"),
        Err(Case::AggregateHere { .. })
    ));
    assert!(matches!(
        reach("param('p') = 1"),
        Err(Case::ParamOutsideView)
    ));
}

#[test]
fn a_verb_without_preconditions_reads_and_writes_as_before() {
    let verb = json!({"name": "pay", "on": "invoice", "risk": "low", "effect": "write",
        "payload": [], "requires": "status = 'open'", "set": [{"field": "paid", "value": "1"}]});
    let v: VerbDecl = serde_json::from_value(verb.clone()).unwrap();
    assert!(v.preconditions.is_empty());
    let back = serde_json::to_value(&v).unwrap();
    assert!(back.get("preconditions").is_none(), "{back}");
    let mut named = verb;
    named["preconditions"] = json!([{"name": "open", "holds": "status = 'open'"}]);
    let v: VerbDecl = serde_json::from_value(named).unwrap();
    assert_eq!(v.conditions().count(), 2);
    assert_eq!(
        serde_json::to_value(&v).unwrap()["preconditions"][0]["name"],
        "open"
    );
}
