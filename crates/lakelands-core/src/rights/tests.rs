use super::*;
use lakelands_api::ontology::StorageKind;
use lakelands_api::principal::{Grants, PrincipalKind, TypeGrants};
use lakelands_api::{FieldMeta, Origin, Position, Timestamp, Version};
use serde_json::json;

fn field(name: &str, authority: Authority, sensitive: bool) -> FieldDecl {
    FieldDecl {
        authority,
        sensitive,
        ..FieldDecl::of(name, StorageKind::Text)
    }
}

fn invoice() -> TypeDecl {
    TypeDecl {
        fields: vec![
            field("amount", Authority::Source, false),
            field("verdict", Authority::Authored, false),
            field("iban", Authority::Source, true),
            field("note", Authority::Shared, false),
            field("score", Authority::Derived, false),
        ],
        field_meta: true,
        ..TypeDecl::named("invoice")
    }
}

fn principal(default: Access, fields: &[(&str, Access)]) -> Principal {
    let fields = fields.iter().map(|(f, a)| (FieldName::from(*f), *a));
    let mut grants = Grants {
        default,
        ..Grants::default()
    };
    grants.types.insert(
        "invoice".into(),
        TypeGrants {
            default: None,
            fields: fields.collect(),
            ..Default::default()
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

/// A refusal naming `field`, for `reason`.
fn refused_on<T>(field: &str, reason: Refusal) -> Result<T, Error> {
    Err(Error::Refused {
        field: Some(field.into()),
        reason,
    })
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
fn probing_an_unreadable_field_is_refused_not_masked() {
    let who = principal(Access::Read, &[("iban", Access::Masked)]);
    let probe = |f: &str| check_probe(&who, &invoice(), [&FieldName::from(f)]);
    assert!(probe("amount").is_ok());
    assert_eq!(probe("iban"), refused_on("iban", Refusal::NotGranted));
    assert!(matches!(
        probe("nope"),
        Err(Error::Refused {
            reason: Refusal::Invalid { .. },
            ..
        })
    ));
}

#[test]
fn a_search_probes_every_searchable_field_it_reads_and_the_vector_it_names() {
    let mut decl = invoice();
    decl.fields[2].searchable = true; // iban, sensitive
    let reader = principal(Access::Read, &[("iban", Access::Masked)]);
    assert!(
        check_search(&reader, &decl, None, &[]).is_ok(),
        "`iban` is not searched"
    );
    assert_eq!(searched_fields(&reader, &decl).count(), 0);
    let granted = principal(Access::Read, &[]);
    assert!(check_search(&granted, &decl, None, &[]).is_ok());
    assert!(check_search(&granted, &decl, None, &[&"nope".into()]).is_err());
}

#[test]
fn a_search_limited_to_named_fields_probes_those_and_judges_reading_first() {
    let mut decl = invoice();
    decl.fields[2].searchable = true; // iban, sensitive
    decl.fields[3].searchable = true; // note
    let reader = principal(Access::Read, &[("iban", Access::Masked)]);
    let not_granted = |field: &str| refused_on(field, Refusal::NotGranted);
    let invalid =
        |field: &str, case: Case| refused_on(field, Refusal::Invalid(Box::new(case.into())));
    let search = |decl: &TypeDecl, named: &[&str]| {
        let named: Vec<FieldName> = named.iter().map(|&n| n.into()).collect();
        check_search(&reader, decl, Some(&named), &[])
    };
    let ty = || decl.name.clone();
    assert!(search(&decl, &["note"]).is_ok(), "`iban` is not named");
    assert_eq!(search(&decl, &["note", "iban"]), not_granted("iban"));
    let (ty, field) = (ty(), "verdict".into());
    let unsearchable = invalid("verdict", Case::NotSearchable { field, ty });
    assert_eq!(search(&decl, &["verdict"]), unsearchable);
    let (ty, field) = (decl.name.clone(), "nope".into());
    let unknown = invalid("nope", Case::UnknownField { ty, field });
    assert_eq!(search(&decl, &["nope"]), unknown);
    let empty = Err(crate::invalid(Case::SearchFieldsEmpty));
    assert_eq!(search(&decl, &[]), empty);
    decl.fields[2].searchable = false;
    assert_eq!(
        search(&decl, &["iban"]),
        not_granted("iban"),
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
        check_search(&reader, &decl, None, &[&vec]),
        refused_on("note", Refusal::NotGranted),
        "the vector is readable, but it encodes `note`"
    );
    assert!(
        check_search(&reader, &decl, None, &[]).is_ok(),
        "no field is searchable"
    );
    assert!(check_search(&principal(Access::Read, &[]), &decl, None, &[&vec]).is_ok());
}

#[test]
fn a_write_is_refused_whole_by_its_first_failing_field() {
    let owner = principal(Access::Write, &[]);
    let write = |who: &Principal, v: serde_json::Value| {
        let patch: BTreeMap<FieldName, Value> = serde_json::from_value(v).unwrap();
        check_write(who, &invoice(), Door::Authored, &patch)
    };
    assert!(write(&owner, json!({"verdict": "x", "note": "y"})).is_ok());
    assert_eq!(
        write(&owner, json!({"verdict": "x", "amount": "1"})),
        refused_on("amount", Refusal::WrongDoor)
    );
    assert_eq!(
        write(&owner, json!({"score": 1})),
        refused_on("score", Refusal::WrongDoor)
    );
    let limited = principal(Access::Read, &[("note", Access::Write)]);
    assert_eq!(
        write(&limited, json!({"note": "y", "verdict": "x"})),
        refused_on("verdict", Refusal::NotGranted)
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

#[test]
fn a_view_carries_its_source_only_where_it_groups_by_the_owner() {
    let decl = |v: serde_json::Value| -> TypeDecl { serde_json::from_value(v).expect("a type") };
    let view = |name: &str, by: &[&str], merge: bool| {
        let merge = if merge {
            json!([{"source": "claim", "measures": []}])
        } else {
            json!([])
        };
        decl(
            json!({"name": name, "view": {"source": "claim", "group_by": by, "merge": merge,
                    "measures": [{"name": "n", "expr": "count()"}]}}),
        )
    };
    let types = [
        decl(json!({"name": "claim", "fields": [{"name": "owner", "type": {"kind": "text"}}]})),
        view("by_owner", &["owner"], false),
        view("all", &[], false),
        view("merged", &["owner"], true),
    ];
    let find = |t: &TypeName| types.iter().find(|d| &d.name == t);
    let mut who = principal(Access::Read, &[]);
    let alice = TypeGrants {
        only: [("owner".into(), json!("alice"))].into(),
        ..TypeGrants::default()
    };
    who.grants.types.insert("claim".into(), alice);
    let pairs = vec![(FieldName::from("owner"), json!("alice"))];
    let only = |ty: &str| only(&who, &ty.into(), &find);
    assert_eq!(
        only("by_owner"),
        Effective::Pairs(pairs.clone()),
        "rule 16 maps the pairs"
    );
    assert_eq!(
        only("sys:kept.by_owner"),
        Effective::Pairs(pairs),
        "a kept name is its view"
    );
    assert_eq!(
        only("all"),
        Effective::ComputeOnly(vec![]),
        "no group holds the owner"
    );
    assert_eq!(
        only("merged"),
        Effective::ComputeOnly(vec![]),
        "a merge computes"
    );
    assert!(only("invoice").whole());
}
