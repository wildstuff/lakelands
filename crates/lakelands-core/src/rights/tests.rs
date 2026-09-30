use super::*;
use lakelands_api::ontology::{FieldType, StorageKind};
use lakelands_api::principal::{Grants, PrincipalKind, TypeGrants};
use lakelands_api::{FieldMeta, Origin, Position, Timestamp, Version};
use serde_json::json;

fn field(name: &str, authority: Authority, sensitive: bool) -> FieldDecl {
    FieldDecl {
        annotations: Default::default(),
        name: name.into(),
        ty: FieldType::kind(StorageKind::Text),
        required: false,
        authority,
        derived: None,
        function: None,
        depends_on: vec![],
        on_input_change: Default::default(),
        sensitive,
        searchable: false,
        description: None,
        label: None,
        embed: None,
    }
}

fn invoice() -> TypeDecl {
    TypeDecl {
        annotations: Default::default(),
        name: "invoice".into(),
        key: vec![],
        fields: vec![
            field("amount", Authority::Source, false),
            field("verdict", Authority::Authored, false),
            field("iban", Authority::Source, true),
            field("note", Authority::Shared, false),
            field("score", Authority::Derived, false),
        ],
        relations: vec![],
        view: None,
        valid_from: None,
        valid_to: None,
        field_meta: true,
        language: None,
        description: None,
        label: None,
    }
}

fn principal(default: Access, fields: &[(&str, Access)]) -> Principal {
    let mut grants = Grants {
        default,
        ..Grants::default()
    };
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

fn stored() -> StoredRecord {
    let meta = FieldMeta {
        upstream: None,
        origin: Origin::Source,
        known_at: Timestamp(0),
        seen_at: None,
        position: Position(1),
        writer: "service:feed".into(),
        source: None,
        procedure_version: None,
        error: None,
    };
    StoredRecord {
        ty: "invoice".into(),
        key: "INV-1".into(),
        version: Version(2),
        fields: serde_json::from_value(json!({
            "amount": "10.00", "verdict": "ok", "iban": "DE00", "note": null
        }))
        .unwrap(),
        meta: [("amount".into(), meta.clone()), ("iban".into(), meta)].into(),
        position: Position(1),
        retired: false,
        restricted: false,
    }
}

#[test]
fn a_reader_sees_granted_fields_and_a_sensitive_field_only_as_masked() {
    let who = principal(Access::None, &[("amount", Access::Read)]);
    let r = visible(&who, &invoice(), &stored(), true).unwrap();
    assert_eq!(r.fields.keys().collect::<Vec<_>>(), ["amount"]);
    assert_eq!(r.masked, [FieldName::from("iban")]);
    assert_eq!(
        r.meta.keys().collect::<Vec<_>>(),
        ["amount"],
        "no meta for a masked field"
    );
}

#[test]
fn meta_comes_only_when_asked() {
    let who = principal(Access::Read, &[]);
    assert!(
        visible(&who, &invoice(), &stored(), false)
            .unwrap()
            .meta
            .is_empty()
    );
}

#[test]
fn retired_and_restricted_records_are_withheld() {
    let who = principal(Access::Read, &[]);
    let mut s = stored();
    s.restricted = true;
    assert!(visible(&who, &invoice(), &s, false).is_none());
    let mut reviewer = who.clone();
    reviewer.grants.restricted = true;
    assert!(visible(&reviewer, &invoice(), &s, false).is_some());
    s.retired = true;
    assert!(visible(&reviewer, &invoice(), &s, false).is_none());
}

#[test]
fn probing_an_unreadable_field_is_refused_not_masked() {
    let who = principal(Access::Read, &[("iban", Access::Masked)]);
    assert!(check_probe(&who, &invoice(), [&FieldName::from("amount")]).is_ok());
    assert_eq!(
        check_probe(&who, &invoice(), [&FieldName::from("iban")]),
        Err(Error::Refused {
            field: Some("iban".into()),
            reason: Refusal::NotGranted
        })
    );
    assert!(matches!(
        check_probe(&who, &invoice(), [&FieldName::from("nope")]),
        Err(Error::Refused {
            reason: Refusal::Invalid { .. },
            ..
        })
    ));
}

#[test]
fn a_search_probes_every_searchable_field_and_the_vector_it_names() {
    let mut decl = invoice();
    decl.fields[2].searchable = true; // iban, sensitive
    let reader = principal(Access::Read, &[("iban", Access::Masked)]);
    assert_eq!(
        check_search(&reader, &decl, None, None),
        Err(Error::Refused {
            field: Some("iban".into()),
            reason: Refusal::NotGranted
        })
    );
    let granted = principal(Access::Read, &[]);
    assert!(check_search(&granted, &decl, None, None).is_ok());
    assert!(check_search(&granted, &decl, None, Some(&"nope".into())).is_err());
}

#[test]
fn a_search_limited_to_named_fields_probes_those_and_judges_reading_first() {
    let mut decl = invoice();
    decl.fields[2].searchable = true; // iban, sensitive
    decl.fields[3].searchable = true; // note
    let reader = principal(Access::Read, &[("iban", Access::Masked)]);
    let refused = |field: &str, invalid: bool| {
        Err(Error::Refused {
            field: Some(field.into()),
            reason: if invalid {
                Refusal::Invalid {
                    message: String::new(),
                }
            } else {
                Refusal::NotGranted
            },
        })
    };
    let search = |decl: &TypeDecl, named: &[&str]| {
        let named: Vec<FieldName> = named.iter().map(|&n| n.into()).collect();
        check_search(&reader, decl, Some(&named), None).map_err(|e| match e {
            Error::Refused {
                field,
                reason: Refusal::Invalid { .. },
            } => Error::Refused {
                field,
                reason: Refusal::Invalid {
                    message: String::new(),
                },
            },
            e => e,
        })
    };
    assert!(search(&decl, &["note"]).is_ok(), "`iban` is not named");
    assert_eq!(search(&decl, &["note", "iban"]), refused("iban", false));
    assert_eq!(search(&decl, &["verdict"]), refused("verdict", true));
    assert_eq!(search(&decl, &["nope"]), refused("nope", true));
    assert!(matches!(
        search(&decl, &[]),
        Err(Error::Refused {
            field: None,
            reason: Refusal::Invalid { .. }
        })
    ));
    decl.fields[2].searchable = false;
    assert_eq!(
        search(&decl, &["iban"]),
        refused("iban", false),
        "an unreadable field is not granted, whether it is searchable or not"
    );
}

#[test]
fn a_search_over_an_embedded_vector_probes_the_fields_it_is_embedded_from() {
    let mut decl = invoice();
    decl.fields.push(FieldDecl {
        embed: Some(lakelands_api::ontology::EmbedDecl {
            from: vec!["verdict".into(), "note".into()],
            model: "m".into(),
        }),
        ..field("vec", Authority::Derived, false)
    });
    let vec = FieldName::from("vec");
    let reader = principal(Access::Read, &[("note", Access::Masked)]);
    assert_eq!(
        check_search(&reader, &decl, None, Some(&vec)),
        Err(Error::Refused {
            field: Some("note".into()),
            reason: Refusal::NotGranted
        }),
        "the vector is readable, but it encodes `note`"
    );
    assert!(
        check_search(&reader, &decl, None, None).is_ok(),
        "no field is searchable"
    );
    assert!(check_search(&principal(Access::Read, &[]), &decl, None, Some(&vec)).is_ok());
}

#[test]
fn a_write_is_refused_whole_by_its_first_failing_field() {
    let owner = principal(Access::Write, &[]);
    let patch =
        |v: serde_json::Value| -> BTreeMap<FieldName, Value> { serde_json::from_value(v).unwrap() };
    assert!(
        check_write(
            &owner,
            &invoice(),
            Door::Authored,
            &patch(json!({"verdict": "x", "note": "y"}))
        )
        .is_ok()
    );
    assert_eq!(
        check_write(
            &owner,
            &invoice(),
            Door::Authored,
            &patch(json!({"verdict": "x", "amount": "1"}))
        ),
        Err(Error::Refused {
            field: Some("amount".into()),
            reason: Refusal::WrongDoor
        })
    );
    assert_eq!(
        check_write(
            &owner,
            &invoice(),
            Door::Authored,
            &patch(json!({"score": 1}))
        ),
        Err(Error::Refused {
            field: Some("score".into()),
            reason: Refusal::WrongDoor
        })
    );
    let limited = principal(Access::Read, &[("note", Access::Write)]);
    assert_eq!(
        check_write(
            &limited,
            &invoice(),
            Door::Authored,
            &patch(json!({"note": "y", "verdict": "x"}))
        ),
        Err(Error::Refused {
            field: Some("verdict".into()),
            reason: Refusal::NotGranted
        })
    );
}

#[test]
fn ingest_needs_the_ingest_grant_and_writes_source_and_shared_fields_only() {
    let mut feed = principal(Access::Write, &[]);
    let patch: BTreeMap<FieldName, Value> =
        serde_json::from_value(json!({"amount": "1", "note": "n"})).unwrap();
    assert_eq!(
        check_write(&feed, &invoice(), Door::Source, &patch),
        Err(Error::Refused {
            field: None,
            reason: Refusal::NotGranted
        })
    );
    feed.grants.ingest = true;
    assert!(check_write(&feed, &invoice(), Door::Source, &patch).is_ok());
    let authored: BTreeMap<FieldName, Value> =
        serde_json::from_value(json!({"verdict": "x"})).unwrap();
    assert!(matches!(
        check_write(&feed, &invoice(), Door::Source, &authored),
        Err(Error::Refused {
            reason: Refusal::WrongDoor,
            ..
        })
    ));
}

#[test]
fn system_types_are_recognised_by_prefix_alone() {
    assert!(is_system(&"sys.setting".into()));
    assert!(is_system(&"sys.anything_new".into()));
    assert!(!is_system(&"system_note".into()));
    assert!(!is_system(&"a.sys.b".into()));
}
