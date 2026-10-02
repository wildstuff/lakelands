use super::*;
use lakelands_api::ontology::{Assignment, FieldDecl, FieldType, Risk, StorageKind};
use lakelands_api::principal::{Grants, PrincipalKind, TypeGrants};
use lakelands_api::tools::{Door, tools};
use lakelands_api::{FieldName, Position};

fn field(name: &str, authority: Authority) -> FieldDecl {
    FieldDecl {
        name: name.into(),
        ty: FieldType::kind(StorageKind::Text),
        authority,
        searchable: name == "note",
        ..Default::default()
    }
}

fn declared() -> Vec<Declared> {
    let invoice = TypeDecl {
        name: "invoice".into(),
        fields: vec![
            field("amount", Authority::Source),
            field("verdict", Authority::Authored),
            field("note", Authority::Authored),
        ],
        field_meta: true,
        description: Some("A bill".into()),
        ..Default::default()
    };
    let secret = TypeDecl {
        name: "salary".into(),
        fields: vec![field("amount", Authority::Authored)],
        ..invoice.clone()
    };
    let settle = VerbDecl {
        name: "settle".into(),
        on: "invoice".into(),
        scope: Default::default(),
        payload: vec![],
        risk: Risk::Low,
        requires: None,
        effect: VerbEffect::Write {
            set: vec![Assignment {
                field: "verdict".into(),
                value: "'settled'".into(),
            }],
        },
        description: None,
    };
    [
        Declaration::Type(invoice),
        Declaration::Type(secret),
        Declaration::Verb(settle),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, declaration)| Declared {
        declaration,
        version: 1,
        schema_hash: Some(format!("h{i}")),
        since: Position(i as u64),
    })
    .collect()
}

fn who(fields: &[(&str, Access)]) -> Principal {
    let mut grants = Grants::default();
    grants.types.insert(
        "invoice".into(),
        TypeGrants {
            default: None,
            fields: fields
                .iter()
                .map(|(f, a)| (FieldName::from(*f), *a))
                .collect(),
        },
    );
    Principal {
        kind: PrincipalKind::Agent,
        id: "a".into(),
        lake: "finance".into(),
        grants,
    }
}

#[test]
fn what_a_principal_holds_no_grant_for_is_absent() {
    let d = lake(
        &"finance".into(),
        &declared(),
        &who(&[("amount", Access::Read)]),
    );
    assert_eq!(d.types.len(), 1, "salary is not listed at all");
    let t = &d.types[0];
    assert_eq!(
        t.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        ["amount"]
    );
    assert!(t.may.find && t.may.get && !t.may.upsert && !t.may.retire);
    assert!(d.verbs.is_empty() && t.verbs.is_empty());
}

#[test]
fn a_writer_of_the_verbs_fields_may_call_it() {
    let d = lake(
        &"finance".into(),
        &declared(),
        &who(&[("verdict", Access::Write)]),
    );
    assert_eq!(d.verbs.len(), 1);
    assert_eq!(
        d.types[0].verbs,
        ["settle".into()] as [lakelands_api::VerbName; 1]
    );
    assert!(d.types[0].may.upsert);
}

#[test]
fn the_owner_sees_everything_and_the_tools_follow() {
    let mut owner = who(&[]);
    owner.grants = Grants::owner();
    let d = lake(&"finance".into(), &declared(), &owner);
    assert_eq!(d.types.len(), 2);
    assert!(d.types.iter().all(|t| t.may.retire));
    assert_eq!(d.types[0].lifecycle.schema_hash, "h0");
    assert!(d.types[0].access.searchable);
    let doors: Vec<Door> = tools(&d).unwrap().into_iter().map(|t| t.door).collect();
    assert!(doors.contains(&Door::Retire {
        ty: "salary".into()
    }));
    assert!(doors.contains(&Door::Call {
        verb: "settle".into()
    }));
}

#[test]
fn describe_type_is_the_same_answer_for_one_type() {
    let reader = who(&[("amount", Access::Read)]);
    let d = lake(&"finance".into(), &declared(), &reader);
    assert_eq!(
        one_type(&"invoice".into(), &declared(), &reader).as_ref(),
        d.types.first()
    );
    assert!(one_type(&"salary".into(), &declared(), &reader).is_none());
}
