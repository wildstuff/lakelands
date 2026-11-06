use super::*;
use crate::conformance::fixture::stored_record;
use lakelands_api::FieldName;
use serde_json::json;

fn relation() -> RelationDecl {
    serde_json::from_value(json!({
        "name": "approved_by", "target": "partner",
        "fields": [{"name": "since", "type": {"kind": "date_time"}}]
    }))
    .unwrap()
}

#[test]
fn the_link_type_is_keyed_by_the_pair_and_holds_the_relations_fields() {
    let t = link_type(&"invoice".into(), &relation());
    assert_eq!(t.name.as_str(), "sys.link.invoice.approved_by");
    assert_eq!(t.key, vec![FieldName::from("from"), FieldName::from("to")]);
    let names: Vec<&str> = t.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["from", "to", "since"]);
    assert_eq!(t.fields[0].ty.target.as_ref().unwrap().as_str(), "invoice");
    assert_eq!(t.fields[1].ty.target.as_ref().unwrap().as_str(), "partner");
    assert!(t.fields[0].required && t.fields[1].required);
}

#[test]
fn a_link_record_is_an_edge_out_of_its_from_and_into_its_to() {
    let mut invoice: TypeDecl = serde_json::from_value(json!({
        "name": "invoice", "fields": []
    }))
    .unwrap();
    invoice.relations = vec![relation()];
    let link = |from: &str, to: &str, retired: bool| StoredRecord {
        retired,
        ..stored_record(
            "sys.link.invoice.approved_by",
            link_key(&from.into(), &to.into()),
            [
                ("from".into(), json!(from)),
                ("to".into(), json!(to)),
                ("since".into(), json!("2026-09-01T00:00:00.000000Z")),
            ]
            .into(),
            3,
        )
    };
    let all = [link("i1", "p1", false), link("i2", "p1", true)];
    let lookup = |_: &TypeName, field: &str, keys: &[RecordKey]| -> Result<Vec<StoredRecord>, ()> {
        Ok(all
            .iter()
            .filter(|l| keys.iter().any(|k| l.fields[field] == json!(k.as_str())))
            .cloned()
            .collect())
    };
    let out = link_edges(
        &"invoice".into(),
        &["i1".into()],
        None,
        Walk::Out,
        [&invoice],
        lookup,
    )
    .unwrap()
    .remove(0);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].to, ("partner".into(), "p1".into()));
    assert_eq!(out[0].fields["since"], json!("2026-09-01T00:00:00.000000Z"));
    let inn = link_edges(
        &"partner".into(),
        &["p1".into()],
        None,
        Walk::In,
        [&invoice],
        lookup,
    )
    .unwrap()
    .remove(0);
    assert_eq!(inn.len(), 1, "the retired link does not count: {inn:?}");
    assert_eq!(inn[0].from, ("invoice".into(), "i1".into()));
}
