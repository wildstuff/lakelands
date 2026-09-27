//! Link records: the one definition of a relation carried without `via`
//! (`docs/decisions/035-a-link-is-a-record.md`, `docs/log-format.md` §2.3).
//!
//! A relation `r` of type `t` declared without `via` is carried by records
//! of its **link type**, `sys.link.t.r`, which this module derives from the
//! relation, the same way for the store, every engine and the mock:
//!
//! - its key fields are `from` (a `reference` to `t`) and `to` (a
//!   `reference` to the relation's target), both required, so a link's key
//!   is `<from key>:<to key>` by the key rule and one relation holds one
//!   link per pair;
//! - its other fields are the relation's own `fields`, as declared;
//! - `from` and `to` take the relation's `authority`: `authored` links are
//!   written through the link doors, `source` links by ingest.
//!
//! [`link_edges`] is the rule every engine applies to answer a record's
//! edges over such relations: from the live link records at the view's
//! position, each an [`Edge`] carrying the link's own fields.

use std::collections::{BTreeMap, BTreeSet};

use lakelands_api::id::{FieldName, RecordKey, RelationName, TypeName};
use lakelands_api::ontology::{
    Authority, FieldDecl, FieldType, RelationDecl, StorageKind, TypeDecl,
};
use lakelands_api::query::{Edge, Walk};
use lakelands_api::record::{StoredRecord, Value};

/// The prefix of every link type's name.
pub const PREFIX: &str = "sys.link.";

/// The link records' own key fields.
pub const FROM: &str = "from";
/// See [`FROM`].
pub const TO: &str = "to";

/// The name of the link type of relation `r` on type `t`.
pub fn link_type_name(t: &TypeName, r: &RelationName) -> TypeName {
    TypeName::from(format!("{PREFIX}{t}.{r}"))
}

/// Whether a type is a link type.
pub fn is_link_type(name: &TypeName) -> bool {
    name.as_str().starts_with(PREFIX)
}

/// Whether a relation is carried by link records.
pub fn carried_by_links(r: &RelationDecl) -> bool {
    r.via.is_none()
}

fn key_field(name: &str, target: &TypeName, authority: Authority) -> FieldDecl {
    FieldDecl {
        name: name.into(),
        ty: FieldType {
            target: Some(target.clone()),
            ..FieldType::kind(StorageKind::Reference)
        },
        required: true,
        authority,
        derived: None,
        function: None,
        depends_on: vec![],
        on_input_change: Default::default(),
        sensitive: false,
        searchable: false,
        description: None,
        label: None,
        embed: None,
    }
}

/// The link type of relation `r` on type `t`.
pub fn link_type(t: &TypeName, r: &RelationDecl) -> TypeDecl {
    let mut fields = vec![
        key_field(FROM, t, r.authority),
        key_field(TO, &r.target, r.authority),
    ];
    fields.extend(r.fields.iter().cloned());
    TypeDecl {
        name: link_type_name(t, &r.name),
        key: vec![FROM.into(), TO.into()],
        fields,
        relations: vec![],
        view: None,
        valid_from: None,
        valid_to: None,
        field_meta: true,
        language: None,
        description: Some(format!(
            "The links of relation `{}` of `{t}` to `{}`.",
            r.name, r.target
        )),
        label: None,
    }
}

/// The link types a type's relations call for, in declaration order.
pub fn link_types_of(t: &TypeDecl) -> Vec<TypeDecl> {
    t.relations
        .iter()
        .filter(|r| carried_by_links(r))
        .map(|r| link_type(&t.name, r))
        .collect()
}

/// The relation a link type carries, found among `types`: the type whose
/// link types include it, and the relation.
pub fn relation_of<'a>(
    types: impl IntoIterator<Item = &'a TypeDecl>,
    link_type: &TypeName,
) -> Option<(&'a TypeDecl, &'a RelationDecl)> {
    types.into_iter().find_map(|t| {
        t.relations
            .iter()
            .filter(|r| carried_by_links(r))
            .find(|r| link_type_name(&t.name, &r.name) == *link_type)
            .map(|r| (t, r))
    })
}

/// The key of the link from `from` to `to`: the key rule over the pair.
pub fn link_key(from: &RecordKey, to: &RecordKey) -> RecordKey {
    let fields: BTreeMap<FieldName, Value> = [
        (FieldName::from(FROM), Value::String(from.to_string())),
        (FieldName::from(TO), Value::String(to.to_string())),
    ]
    .into();
    let decl = TypeDecl {
        name: TypeName::from(""),
        key: vec![FROM.into(), TO.into()],
        fields: vec![],
        relations: vec![],
        view: None,
        valid_from: None,
        valid_to: None,
        field_meta: true,
        language: None,
        description: None,
        label: None,
    };
    crate::log::record_key(&decl, &fields).expect("both components are text")
}

/// The edge a live link record stands for.
fn edge_of(t: &TypeName, r: &RelationDecl, link: &StoredRecord) -> Option<Edge> {
    let from = link.fields.get(FROM)?.as_str()?;
    let to = link.fields.get(TO)?.as_str()?;
    let fields = link
        .fields
        .iter()
        .filter(|(name, _)| name.as_str() != FROM && name.as_str() != TO)
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    Some(Edge {
        from: (t.clone(), RecordKey::from(from)),
        relation: r.name.clone(),
        to: (r.target.clone(), RecordKey::from(to)),
        fields,
    })
}

/// A record's edges over the relations carried by link records: out, the
/// links of `ty`'s relations whose `from` is `key`; in, the links of every
/// type's relations to `ty` whose `to` is `key`; over `relation` or all.
/// `links_where(link_type, field, key)` answers the link type's records at
/// the view's position whose `field` (`from` or `to`) holds `key`, found
/// through an index, or the link type's records there, whole; this keeps
/// the ones whose end is `key` and decides which count: only live links,
/// each once. What it is handed is counted (`engines::links_read`).
pub fn link_edges<'a, E>(
    ty: &TypeName,
    key: &RecordKey,
    relation: Option<&RelationName>,
    walk: Walk,
    types: impl IntoIterator<Item = &'a TypeDecl>,
    mut links_where: impl FnMut(&TypeName, &str, &RecordKey) -> Result<Vec<StoredRecord>, E>,
) -> Result<Vec<Edge>, E> {
    let out = matches!(walk, Walk::Out | Walk::Both);
    let inn = matches!(walk, Walk::In | Walk::Both);
    let mut edges = Vec::new();
    let mut seen: BTreeSet<(TypeName, RecordKey)> = BTreeSet::new();
    for t in types {
        for r in t
            .relations
            .iter()
            .filter(|r| carried_by_links(r) && relation.is_none_or(|want| *want == r.name))
        {
            let lt = link_type_name(&t.name, &r.name);
            let mut wanted: Vec<&str> = Vec::new();
            if out && t.name == *ty {
                wanted.push(FROM);
            }
            if inn && r.target == *ty {
                wanted.push(TO);
            }
            for field in wanted {
                let links = links_where(&lt, field, key)?;
                crate::engines::count_links(links.len());
                for link in links {
                    let at = link.fields.get(field).and_then(Value::as_str);
                    if link.retired || at != Some(key.as_str()) {
                        continue;
                    }
                    if seen.insert((lt.clone(), link.key.clone()))
                        && let Some(e) = edge_of(&t.name, r, &link)
                    {
                        edges.push(e);
                    }
                }
            }
        }
    }
    Ok(edges)
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(
            link_key(&"INV:1".into(), &"P-1".into()).as_str(),
            "INV%3A1:P-1"
        );
    }

    #[test]
    fn a_link_record_is_an_edge_out_of_its_from_and_into_its_to() {
        let mut invoice: TypeDecl = serde_json::from_value(json!({
            "name": "invoice", "fields": []
        }))
        .unwrap();
        invoice.relations = vec![relation()];
        let link = |from: &str, to: &str, retired: bool| StoredRecord {
            ty: "sys.link.invoice.approved_by".into(),
            key: link_key(&from.into(), &to.into()),
            version: lakelands_api::Version(1),
            fields: [
                ("from".into(), json!(from)),
                ("to".into(), json!(to)),
                ("since".into(), json!("2026-09-01T00:00:00.000000Z")),
            ]
            .into(),
            meta: Default::default(),
            position: lakelands_api::Position(3),
            retired,
            restricted: false,
        };
        let all = [link("i1", "p1", false), link("i2", "p1", true)];
        let lookup =
            |_: &TypeName, field: &str, key: &RecordKey| -> Result<Vec<StoredRecord>, ()> {
                Ok(all
                    .iter()
                    .filter(|l| l.fields[field] == json!(key.as_str()))
                    .cloned()
                    .collect())
            };
        let out = link_edges(
            &"invoice".into(),
            &"i1".into(),
            None,
            Walk::Out,
            [&invoice],
            lookup,
        )
        .unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].to, ("partner".into(), "p1".into()));
        assert_eq!(out[0].fields["since"], json!("2026-09-01T00:00:00.000000Z"));
        let inn = link_edges(
            &"partner".into(),
            &"p1".into(),
            None,
            Walk::In,
            [&invoice],
            lookup,
        )
        .unwrap();
        assert_eq!(inn.len(), 1, "the retired link does not count: {inn:?}");
        assert_eq!(inn[0].from, ("invoice".into(), "i1".into()));
    }
}
