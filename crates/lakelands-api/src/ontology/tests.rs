use super::*;
use serde_json::json;

fn ty(name: &str, fields: serde_json::Value) -> TypeDecl {
    serde_json::from_value(json!({ "name": name, "fields": fields })).unwrap()
}

fn lake() -> Vec<TypeDecl> {
    let reference = |name: &str, to: &str| json!({ "name": name, "type": { "kind": "reference", "target": to } });
    let text = |name: &str| json!({ "name": name, "type": { "kind": "text" } });
    vec![
        ty(
            "invoice",
            json!([
                text("number"),
                reference("customer", "customer"),
                text("note")
            ]),
        ),
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
