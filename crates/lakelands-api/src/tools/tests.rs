use super::*;
use crate::describe::{LakeRights, TypeAccess, TypeLifecycle, TypeLineage, TypeRights};
use crate::ontology::{Authority, FieldDecl, Risk};
use crate::principal::Access;

fn field(name: &str, kind: StorageKind, authority: Authority, access: Access) -> FieldDescription {
    let decl = FieldDecl {
        name: name.into(),
        ty: FieldType::kind(kind),
        authority,
        ..Default::default()
    };
    FieldDescription::of(&decl, access)
}

fn ty(name: &str, may: TypeRights, fields: Vec<FieldDescription>) -> TypeDescription {
    TypeDescription {
        name: name.into(),
        fields,
        access: TypeAccess {
            searchable: true,
            field_meta: true,
            ..Default::default()
        },
        may,
        lineage: TypeLineage {
            ..Default::default()
        },
        lifecycle: TypeLifecycle {
            version: 1,
            schema_hash: "h".into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

const ALL: TypeRights = TypeRights {
    find: true,
    get: true,
    upsert: true,
    retire: true,
    link: Vec::new(),
};
const READ: TypeRights = TypeRights {
    find: true,
    get: true,
    upsert: false,
    retire: false,
    link: Vec::new(),
};

fn lake(
    types: Vec<TypeDescription>,
    verbs: Vec<VerbDescription>,
    may: LakeRights,
) -> LakeDescription {
    LakeDescription {
        lake: "finance".into(),
        types,
        verbs,
        may,
        ..Default::default()
    }
}

fn tools_ok(lake: &LakeDescription) -> Vec<ToolSpec> {
    tools(lake).expect("the test lake's names are valid")
}

fn names(tools: &[ToolSpec]) -> Vec<&str> {
    tools.iter().map(|t| t.name.as_str()).collect()
}

fn invoice() -> TypeDescription {
    ty(
        "invoice",
        ALL,
        vec![
            field(
                "amount",
                StorageKind::Decimal,
                Authority::Source,
                Access::Write,
            ),
            field(
                "verdict",
                StorageKind::Text,
                Authority::Authored,
                Access::Write,
            ),
            field(
                "score",
                StorageKind::Number,
                Authority::Derived,
                Access::Read,
            ),
            field("note", StorageKind::Text, Authority::Authored, Access::Read),
        ],
    )
}

#[test]
fn a_type_gives_four_doors_and_the_generic_tools_follow_rights() {
    let owner = LakeRights {
        review: true,
        schema: true,
        ingest: true,
    };
    let t = tools_ok(&lake(vec![invoice()], vec![], owner));
    assert_eq!(
        names(&t),
        [
            "accept",
            "add_field",
            "declare_type",
            "describe",
            "invoice_find",
            "invoice_get",
            "invoice_retire",
            "invoice_upsert",
            "migrate",
            "pin",
            "proposals",
            "propose",
            "query",
            "reject",
            "search",
            "subscribe",
            "unsubscribe",
        ]
    );
}

#[test]
fn upsert_offers_only_authored_fields_the_principal_may_write() {
    let t = tools_ok(&lake(vec![invoice()], vec![], LakeRights::default()));
    let upsert = t.iter().find(|t| t.name == "invoice_upsert").unwrap();
    let fields = upsert.input_schema["properties"]["fields"]["properties"]
        .as_object()
        .unwrap();
    assert_eq!(fields.keys().collect::<Vec<_>>(), ["verdict"]);
}

#[test]
fn a_verb_is_one_tool_and_says_whether_it_runs() {
    let pay = VerbDescription {
        name: "pay_invoice".into(),
        on: "invoice".into(),
        scope: Default::default(),
        payload: vec![field(
            "amount",
            StorageKind::Decimal,
            Authority::Authored,
            Access::Write,
        )],
        risk: Risk::High,
        external: true,
        description: None,
    };
    let t = tools_ok(&lake(vec![], vec![pay], LakeRights::default()));
    let v = t.iter().find(|t| t.name == "pay_invoice").unwrap();
    assert_eq!(
        v.door,
        Door::Call {
            verb: "pay_invoice".into()
        }
    );
    assert!(v.description.contains("proposal"));
}

#[test]
fn twenty_types_offer_about_eighty_tools() {
    let types = (0..20).map(|i| ty(&format!("t{i}"), ALL, vec![])).collect();
    let t = tools_ok(&lake(types, vec![], LakeRights::default()));
    // describe, pin, subscribe, unsubscribe, query, search, propose
    assert_eq!(t.len(), 80 + 7);
}

#[test]
fn names_are_sanitised_sorted_and_valid_for_mcp() {
    let t = tools_ok(&lake(
        vec![
            ty("sys.source", READ, vec![]),
            ty("Invoice-Line", ALL, vec![]),
        ],
        vec![],
        LakeRights::default(),
    ));
    assert!(names(&t).contains(&"sys_source_find"));
    assert!(names(&t).contains(&"invoice_line_upsert"));
    let mut sorted = names(&t);
    sorted.sort();
    assert_eq!(sorted, names(&t));
    for tool in &t {
        assert!(!tool.name.is_empty() && tool.name.len() <= MAX_TOOL_NAME);
        assert!(
            tool.name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        );
    }
}

#[test]
fn colliding_and_overlong_names_are_reported() {
    let problems = name_problems(
        &["a.b".into(), "a_b".into(), "x".repeat(70).into()],
        &["search".into(), "a_b_find".into()],
        &[],
    );
    let collided: Vec<&str> = problems
        .iter()
        .filter_map(|p| match p {
            NameProblem::Collision { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert!(collided.contains(&"a_b_find"));
    assert!(collided.contains(&"a_b_get"));
    assert!(collided.contains(&"search"));
    assert!(
        problems
            .iter()
            .any(|p| matches!(p, NameProblem::TooLong { .. }))
    );
    assert!(name_problems(&["invoice".into()], &["pay_invoice".into()], &[]).is_empty());
}

/// `invoice` with `approved_by`, a relation carried by link records
/// with a writable `since` and a read-only `score`, and its link type as
/// the principal is described it.
fn linked_invoice() -> (TypeDescription, TypeDescription) {
    let mut invoice = invoice();
    invoice.relations = vec![crate::ontology::RelationDecl {
        name: "approved_by".into(),
        target: "partner".into(),
        authority: Authority::Authored,
        ..Default::default()
    }];
    invoice.may.link = vec!["approved_by".into()];
    let authored = |name: &str, kind, access| field(name, kind, Authority::Authored, access);
    let link_type = ty(
        "sys.link.invoice.approved_by",
        READ,
        vec![
            authored("from", StorageKind::Reference, Access::Write),
            authored("to", StorageKind::Reference, Access::Write),
            authored("since", StorageKind::DateTime, Access::Write),
            authored("score", StorageKind::Number, Access::Read),
        ],
    );
    for f in &link_type.fields {
        if f.name.as_str() == "since" || f.name.as_str() == "score" {
            invoice.relations[0].fields.push(FieldDecl {
                name: f.name.clone(),
                ty: f.ty.clone(),
                authority: Authority::Authored,
                ..Default::default()
            });
        }
    }
    (invoice, link_type)
}

#[test]
fn a_link_tool_offers_only_the_link_fields_the_principal_may_write() {
    let (invoice, link_type) = linked_invoice();
    let t = tools_ok(&lake(
        vec![invoice, link_type],
        vec![],
        LakeRights::default(),
    ));
    let link = t
        .iter()
        .find(|t| t.name == "invoice_approved_by_link")
        .unwrap();
    let fields = &link.input_schema["properties"]["fields"]["properties"];
    assert!(fields["since"].is_object(), "{fields}");
    assert!(
        fields.get("score").is_none(),
        "a read-only link field is not offered: {fields}"
    );
    assert!(fields.get("from").is_none() && fields.get("to").is_none());
}

#[test]
fn a_link_relation_gets_link_and_unlink_tools_named_by_type_and_relation() {
    let (mut invoice, link_type) = linked_invoice();
    // Offered only for the relations the principal may link.
    invoice.may.link.clear();
    let without = tools_ok(&lake(vec![invoice.clone()], vec![], LakeRights::default()));
    assert!(!names(&without).iter().any(|n| n.ends_with("link")));
    invoice.may.link = vec!["approved_by".into()];
    let t = tools_ok(&lake(
        vec![invoice, link_type],
        vec![],
        LakeRights::default(),
    ));
    let link = t
        .iter()
        .find(|t| t.name == "invoice_approved_by_link")
        .expect("the link tool");
    assert_eq!(
        link.door,
        Door::Link {
            ty: "invoice".into(),
            relation: "approved_by".into()
        }
    );
    assert!(link.input_schema["properties"]["fields"]["properties"]["since"].is_object());
    assert_eq!(link.input_schema["required"], json!(["key", "to"]));
    let unlink = t
        .iter()
        .find(|t| t.name == "invoice_approved_by_unlink")
        .expect("the unlink tool");
    assert_eq!(
        unlink.door,
        Door::Unlink {
            ty: "invoice".into(),
            relation: "approved_by".into()
        }
    );
    // Two types' relations whose names join to one tool name collide.
    let problems = name_problems(
        &["a".into(), "a_b".into()],
        &[],
        &[("a".into(), "b_c".into()), ("a_b".into(), "c".into())],
    );
    assert!(
        problems
            .iter()
            .any(|p| matches!(p, NameProblem::Collision { name, .. } if name == "a_b_c_link")),
        "{problems:?}"
    );
}

#[test]
fn the_generator_is_deterministic() {
    let l = lake(
        vec![invoice()],
        vec![],
        LakeRights {
            review: true,
            schema: true,
            ingest: true,
        },
    );
    assert_eq!(
        serde_json::to_string(&tools(&l).unwrap()).unwrap(),
        serde_json::to_string(&tools(&l).unwrap()).unwrap()
    );
}

const OWNER: LakeRights = LakeRights {
    review: true,
    schema: true,
    ingest: true,
};

#[test]
fn the_generator_refuses_collisions_and_overlong_or_empty_names_itself() {
    let colliding = lake(
        vec![ty("a.b", READ, vec![]), ty("a_b", READ, vec![])],
        vec![],
        LakeRights::default(),
    );
    let problems = tools(&colliding).unwrap_err();
    assert!(
        problems
            .iter()
            .any(|p| matches!(p, NameProblem::Collision { name, .. } if name == "a_b_find"))
    );

    let long = lake(
        vec![ty(&"x".repeat(70), READ, vec![])],
        vec![],
        LakeRights::default(),
    );
    assert!(
        tools(&long)
            .unwrap_err()
            .iter()
            .any(|p| matches!(p, NameProblem::TooLong { .. }))
    );

    let empty = VerbDescription {
        name: "".into(),
        on: "invoice".into(),
        scope: Default::default(),
        payload: vec![],
        risk: Risk::Low,
        external: false,
        description: None,
    };
    assert!(matches!(
        tools(&lake(vec![], vec![empty], LakeRights::default())).unwrap_err()[..],
        [NameProblem::Empty { .. }]
    ));
}

#[test]
fn a_verb_payload_offers_only_fields_the_principal_may_write() {
    let verb = VerbDescription {
        name: "settle".into(),
        on: "invoice".into(),
        scope: Default::default(),
        payload: vec![
            field(
                "amount",
                StorageKind::Decimal,
                Authority::Authored,
                Access::Write,
            ),
            field(
                "approver",
                StorageKind::Text,
                Authority::Authored,
                Access::Read,
            ),
        ],
        risk: Risk::Low,
        external: false,
        description: None,
    };
    let t = tools_ok(&lake(vec![], vec![verb], LakeRights::default()));
    let payload = &t.iter().find(|t| t.name == "settle").unwrap().input_schema["properties"]["payload"]
        ["properties"];
    assert!(payload.get("amount").is_some());
    assert!(payload.get("approver").is_none());
}

fn walk_for_empty_enums(v: &Value, path: &str, found: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            if m.get("enum")
                .and_then(Value::as_array)
                .is_some_and(|a| a.is_empty())
            {
                found.push(path.to_owned());
            }
            for (k, v) in m {
                walk_for_empty_enums(v, &format!("{path}.{k}"), found);
            }
        }
        Value::Array(a) => a.iter().for_each(|v| walk_for_empty_enums(v, path, found)),
        _ => {}
    }
}

#[test]
fn no_schema_carries_an_empty_enum() {
    for l in [
        lake(vec![], vec![], OWNER),
        lake(vec![ty("bare", ALL, vec![])], vec![], OWNER),
    ] {
        let mut found = Vec::new();
        for t in tools_ok(&l) {
            walk_for_empty_enums(&t.input_schema, &t.name, &mut found);
            walk_for_empty_enums(&t.output_schema, &t.name, &mut found);
        }
        assert!(found.is_empty(), "empty enums at {found:?}");
    }
}

#[test]
fn the_generic_tools_emitted_are_exactly_the_generic_list() {
    let (invoice, link_type) = linked_invoice();
    let t = tools_ok(&lake(vec![invoice, link_type], vec![], OWNER));
    assert!(
        names(&t).contains(&"invoice_approved_by_link"),
        "the fixture's link relation has its tools"
    );
    let mut emitted: Vec<&str> = t
        .iter()
        .filter(|t| {
            !matches!(
                t.door,
                Door::Find { .. }
                    | Door::Get { .. }
                    | Door::Upsert { .. }
                    | Door::Retire { .. }
                    | Door::Link { .. }
                    | Door::Unlink { .. }
                    | Door::Call { .. }
            )
        })
        .map(|t| t.name.as_str())
        .collect();
    emitted.sort();
    let mut listed = GENERIC_TOOLS.to_vec();
    listed.sort();
    assert_eq!(emitted, listed);
}

#[test]
fn the_argument_schemas_reach_every_argument_the_doors_take() {
    let t = tools_ok(&lake(vec![invoice()], vec![], OWNER));
    let arg = |tool: &str, path: &[&str]| {
        let mut v = &t.iter().find(|t| t.name == tool).unwrap().input_schema["properties"];
        for p in path {
            v = &v[*p];
        }
        v.clone()
    };
    assert!(arg("invoice_find", &["order_by", "items", "properties", "by"]).is_object());
    assert!(arg("query", &["to"]).is_object());
    assert!(arg("query", &["order_by"]).is_object());
    assert!(arg("describe", &["valid_at"]).is_object());
    assert!(arg("search", &["cursor"]).is_object());
    assert!(arg("proposals", &["state"]).is_object());
}
