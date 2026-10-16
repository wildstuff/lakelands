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
    };
    let places: Vec<usize> = Authority::ALL.into_iter().map(place).collect();
    assert_eq!(places, (0..5).collect::<Vec<_>>());
}
