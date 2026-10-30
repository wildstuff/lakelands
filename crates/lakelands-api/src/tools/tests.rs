use super::*;
use crate::describe::{ExecutorDescription, LakeRights, TypeAccess, TypeLifecycle, TypeRights};
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

fn authored(name: &str, kind: StorageKind, access: Access) -> FieldDescription {
    field(name, kind, Authority::Authored, access)
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
    create: true,
    create_why: None,
    retire: true,
    link: Vec::new(),
    only: BTreeMap::new(),
    only_any: BTreeMap::new(),
    writes_only: BTreeMap::new(),
};
const READ: TypeRights = TypeRights {
    find: true,
    get: true,
    upsert: false,
    create: false,
    create_why: None,
    retire: false,
    link: Vec::new(),
    only: BTreeMap::new(),
    only_any: BTreeMap::new(),
    writes_only: BTreeMap::new(),
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

/// A verb on `invoice`, record-scoped and undescribed.
fn verb(name: &str, risk: Risk, external: bool, payload: Vec<FieldDescription>) -> VerbDescription {
    VerbDescription {
        name: name.into(),
        on: "invoice".into(),
        scope: Default::default(),
        payload,
        risk,
        external,
        executor: None,
        retires: false,
        description: None,
    }
}

/// A lake of `types`, with no verb and no lake-wide right.
fn lake_of(types: Vec<TypeDescription>) -> LakeDescription {
    lake(types, vec![], LakeRights::default())
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
            authored("verdict", StorageKind::Text, Access::Write),
            field(
                "score",
                StorageKind::Number,
                Authority::Derived,
                Access::Read,
            ),
            authored("note", StorageKind::Text, Access::Read),
        ],
    )
}

#[test]
fn a_type_gives_four_doors_and_the_generic_tools_follow_rights() {
    let t = tools_ok(&lake(vec![invoice()], vec![], OWNER));
    assert_eq!(
        names(&t),
        [
            "accept",
            "add_field",
            "cypher",
            "declare_type",
            "describe",
            "invoice_change",
            "invoice_find",
            "invoice_get",
            "invoice_retire",
            "invoice_upsert",
            "migrate",
            "pin",
            "proposals",
            "propose",
            "query",
            "recompute",
            "reject",
            "search",
            "subscribe",
            "unsubscribe",
        ]
    );
}

#[test]
fn upsert_offers_only_authored_fields_the_principal_may_write() {
    let t = tools_ok(&lake_of(vec![invoice()]));
    let upsert = t.iter().find(|t| t.name == "invoice_upsert").unwrap();
    let fields = upsert.input_schema["properties"]["fields"]["properties"]
        .as_object()
        .unwrap();
    assert_eq!(fields.keys().collect::<Vec<_>>(), ["verdict"]);
}

#[test]
fn a_verb_is_one_tool_and_says_whether_it_runs() {
    let pay = verb(
        "pay_invoice",
        Risk::High,
        true,
        vec![authored("amount", StorageKind::Decimal, Access::Write)],
    );
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

/// The text an agent reads says whether the verb's executor is bound in
/// this lake, and what a call does without it; an answer that does not
/// tell (an older server) keeps the general sentence.
#[test]
fn a_verb_tool_says_whether_its_executor_is_bound() {
    use Risk::{High, Low};
    let ends = [
        "`ledger`, which this lake's host binds.",
        "in this lake, so a call is refused and writes nothing.",
        "so a call or a proposal is refused and writes nothing.",
        "where none is bound, a call is refused and writes nothing.",
    ];
    let cases = [
        (Low, Some(true)),
        (Low, Some(false)),
        (High, Some(false)),
        (Low, None),
    ];
    for ((risk, bound), ends) in cases.into_iter().zip(ends) {
        let name = "ledger".to_owned();
        let v = VerbDescription {
            executor: bound.map(|bound| ExecutorDescription { name, bound }),
            ..verb("remind", risk, true, vec![])
        };
        let t = tools_ok(&lake(vec![], vec![v], LakeRights::default()));
        let d = &t.iter().find(|t| t.name == "remind").unwrap().description;
        assert!(d.ends_with(ends), "{d}");
        assert_eq!(d.contains("its executor `ledger`"), bound.is_some(), "{d}");
    }
}

#[test]
fn twenty_types_offer_about_a_hundred_tools() {
    let types = (0..20).map(|i| ty(&format!("t{i}"), ALL, vec![])).collect();
    let t = tools_ok(&lake(types, vec![], LakeRights::default()));
    // describe, pin, subscribe, unsubscribe, query, cypher, search, propose
    assert_eq!(t.len(), 100 + 8);
}

#[test]
fn names_are_sanitised_sorted_and_valid_for_mcp() {
    let t = tools_ok(&lake_of(vec![
        ty("sys.source", READ, vec![]),
        ty("Invoice-Line", ALL, vec![]),
    ]));
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

/// A derived relation has no link door (044), so its type and name make
/// no tool name: a verb named as that door would be is the lake's tool.
#[test]
fn a_derived_relation_takes_no_tool_name() {
    let mut invoice = invoice();
    invoice.relations = vec![crate::ontology::RelationDecl {
        derived: Some(crate::ontology::Derived {
            pairs: Vec::new(),
            filter: None,
        }),
        ..crate::ontology::RelationDecl::to("approved_by", "partner")
    }];
    let named = verb("invoice_approved_by_link", Risk::Low, false, vec![]);
    let t = tools_ok(&lake(vec![invoice], vec![named], LakeRights::default()));
    assert!(names(&t).contains(&"invoice_approved_by_link"), "{t:?}");
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
    let t = tools_ok(&lake_of(vec![invoice, link_type]));
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
    let without = tools_ok(&lake_of(vec![invoice.clone()]));
    assert!(!names(&without).iter().any(|n| n.ends_with("link")));
    invoice.may.link = vec!["approved_by".into()];
    let t = tools_ok(&lake_of(vec![invoice, link_type]));
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
    let l = lake(vec![invoice()], vec![], OWNER);
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
    let colliding = lake_of(vec![ty("a.b", READ, vec![]), ty("a_b", READ, vec![])]);
    let problems = tools(&colliding).unwrap_err();
    assert!(
        problems
            .iter()
            .any(|p| matches!(p, NameProblem::Collision { name, .. } if name == "a_b_find"))
    );

    let long = lake_of(vec![ty(&"x".repeat(70), READ, vec![])]);
    assert!(
        tools(&long)
            .unwrap_err()
            .iter()
            .any(|p| matches!(p, NameProblem::TooLong { .. }))
    );

    let empty = verb("", Risk::Low, false, vec![]);
    assert!(matches!(
        tools(&lake(vec![], vec![empty], LakeRights::default())).unwrap_err()[..],
        [NameProblem::Empty { .. }]
    ));
}

#[test]
fn a_verb_payload_offers_only_fields_the_principal_may_write() {
    let verb = verb(
        "settle",
        Risk::Low,
        false,
        vec![
            authored("amount", StorageKind::Decimal, Access::Write),
            authored("approver", StorageKind::Text, Access::Read),
        ],
    );
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
                    | Door::Change { .. }
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
    // A door that binds a seam is the host's, and no tool's.
    assert!(
        names(&t).iter().all(|n| !n.contains("bind")),
        "{:?}",
        names(&t)
    );
}

/// `cypher` is offered exactly where `query` is: beside it for a lake
/// with a type the principal may find, and with neither for one without.
#[test]
fn cypher_is_offered_where_query_is() {
    let unread = TypeRights {
        find: false,
        get: false,
        ..ALL
    };
    for (t, offered) in [(invoice(), true), (ty("invoice", unread, vec![]), false)] {
        let t = tools_ok(&lake_of(vec![t]));
        let has = |d: &Door| t.iter().any(|t| t.door == *d);
        assert_eq!(
            (has(&Door::Query), has(&Door::Cypher)),
            (offered, offered),
            "{:?}",
            names(&t)
        );
    }
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

#[test]
fn a_date_field_is_a_string_of_the_format_date() {
    let ty = crate::ontology::FieldType::kind(StorageKind::Date);
    let schema = field_schema(&ty, &lake_of(Vec::new()));
    assert_eq!(
        schema,
        serde_json::json!({"type": "string", "format": "date"})
    );
}

/// The outcome schema names every member an outcome answers, each member
/// set, so a member added to `Outcome` is described the same day
/// (lakelands#1105).
#[test]
fn the_outcome_schema_names_every_member_an_outcome_answers() {
    use crate::write::{DeclarationKind, InForce, Outcome, SetOff};
    let applied = Outcome::Applied {
        position: crate::Position(4),
        version: Some(crate::Version(2)),
        output: Some(serde_json::json!({})),
        set_off: SetOff {
            through: Some(crate::Position(5)),
            fired: vec!["t".into()],
            joined: vec!["s".into()],
            left: vec!["s".into()],
        },
        renamed: vec![("a".into(), "b".into())],
        declared: vec![InForce {
            kind: DeclarationKind::Type,
            name: "invoice".into(),
            version: 1,
            since: crate::Position(4),
            appended: true,
        }],
        plan: None,
    };
    let proposed = Outcome::Proposed {
        position: crate::Position(4),
        proposal: "p".into(),
        why: "w".into(),
        plan: None,
    };
    let schema = outcome_schema();
    let described = schema["properties"].as_object().expect("properties");
    for outcome in [applied, proposed] {
        let answered = serde_json::to_value(&outcome).expect("an outcome prints");
        for member in answered.as_object().expect("an object").keys() {
            assert!(
                described.contains_key(member),
                "the outcome schema lacks `{member}`"
            );
        }
    }
    let kinds = &described["declared"]["items"]["properties"]["kind"]["enum"];
    let named: Vec<_> = DeclarationKind::ALL
        .iter()
        .map(|k| serde_json::json!(k))
        .collect();
    assert_eq!(
        kinds,
        &serde_json::json!(named),
        "the kinds are every kind, as answered"
    );
}

/// A type the principal may write has a `<type>_change` tool: `set` takes
/// the fields it may write, `retire` is there only where it may retire,
/// and a type it may only read has none (081).
#[test]
fn a_change_tool_follows_what_its_principal_may_write() {
    let t = tools_ok(&lake(vec![invoice()], vec![], OWNER));
    let change = t.iter().find(|t| t.name == "invoice_change").unwrap();
    assert_eq!(
        change.door,
        Door::Change {
            ty: "invoice".into()
        }
    );
    let props = &change.input_schema["properties"];
    for member in ["where", "set", "retire", "plan", "plan_id"] {
        assert!(props.get(member).is_some(), "{member}");
    }
    assert!(props["set"]["properties"].get("verdict").is_some());
    assert!(props["set"]["properties"].get("score").is_none());
    for named in ["plan_stale", "too_large"] {
        assert!(change.description.contains(named), "{}", change.description);
    }
    let mut reader = invoice();
    reader.may.upsert = false;
    reader.may.retire = false;
    let t = tools_ok(&lake(vec![reader], vec![], OWNER));
    assert!(t.iter().all(|t| t.name != "invoice_change"));
}

#[test]
fn a_change_tool_on_a_link_type_sets_its_own_fields_or_unlinks_and_names_no_end() {
    let (invoice, mut link_type) = linked_invoice();
    link_type.may = ALL;
    let t = tools_ok(&lake_of(vec![invoice, link_type]));
    let change = t
        .iter()
        .find(|t| t.name == "sys_link_invoice_approved_by_change")
        .expect("the change tool of the link type");
    let set = &change.input_schema["properties"]["set"]["properties"];
    assert!(set["since"].is_object(), "{set}");
    assert!(set.get("score").is_none(), "a read-only field: {set}");
    assert!(
        set.get("from").is_none() && set.get("to").is_none(),
        "{set}"
    );
    assert!(
        change.description.contains("unlink"),
        "{}",
        change.description
    );
}

/// What walk 7's agent was refused for not knowing is said where it
/// reads first: a find's cursor goes back with the `where` and
/// `order_by` of its first page, a find's list is written in
/// parentheses, and a step's depth takes both members, a graph or a path
/// being one walk only where every step's `min` is 0.
#[test]
fn a_cursor_a_list_and_a_depth_say_what_their_refusals_ask() {
    let t = tools_ok(&lake_of(vec![invoice()]));
    let door = |d: Door| t.iter().find(|t| t.door == d).expect("the door is offered");
    let find = &door(Door::Find {
        ty: "invoice".into(),
    })
    .input_schema["properties"];
    let said = |v: &Value| v["description"].as_str().unwrap_or_default().to_owned();
    let cursor = said(&find["cursor"]);
    for words in ["`next_cursor`", "`where`", "`order_by`"] {
        assert!(cursor.contains(words), "the cursor says {words}: {cursor}");
    }
    let condition = said(&find["where"]);
    assert!(condition.contains("in ('open', 'paid')"), "{condition}");
    let query = &door(Door::Query).input_schema["properties"];
    let depth = said(&query["steps"]["items"]["properties"]["depth"]);
    for words in ["both members", "`to`", "`min` 0", "chain"] {
        assert!(depth.contains(words), "the depth says {words}: {depth}");
    }
}

/// Walk 8's texts: a find's `where` says a text is matched by a function,
/// not by a word between its operands, and a search's `mode` and `field`
/// say which members each mode needs, so a hybrid search names its field.
#[test]
fn a_where_and_a_search_mode_say_what_their_refusals_ask() {
    let t = tools_ok(&lake_of(vec![invoice()]));
    let door = |d: Door| t.iter().find(|t| t.door == d).expect("the door is offered");
    let said = |v: &Value| v["description"].as_str().unwrap_or_default().to_owned();
    let find = &door(Door::Find {
        ty: "invoice".into(),
    })
    .input_schema["properties"];
    let condition = said(&find["where"]);
    for words in ["contains(name, 'Kest')", "starts_with(name, 'Kest')"] {
        assert!(
            condition.contains(words),
            "the where says {words}: {condition}"
        );
    }
    let search = &door(Door::Search).input_schema["properties"];
    let mode = said(&search["mode"]);
    for words in ["`text`", "`field`", "`vector`", "hybrid"] {
        assert!(mode.contains(words), "the mode says {words}: {mode}");
    }
    let field = said(&search["field"]);
    assert!(
        field.contains("hybrid"),
        "the field says hybrid needs it: {field}"
    );
}

#[test]
fn a_search_text_says_a_word_matches_only_as_written() {
    let t = tools_ok(&lake_of(vec![invoice()]));
    let search = t.iter().find(|t| t.door == Door::Search).expect("search");
    let text = &search.input_schema["properties"]["text"]["description"];
    let text = text.as_str().unwrap_or_default();
    for words in ["misspelled", "starts_with"] {
        assert!(text.contains(words), "the text says {words}: {text}");
    }
}

/// Walk 6's gaps: `query`'s `project` shows an aggregate's members, a
/// step's `depth` requires both its members, and a cursor says it goes
/// back with the request of its first page.
#[test]
fn a_query_schema_shows_an_aggregate_a_depth_and_a_cursor() {
    let t = tools_ok(&lake_of(vec![invoice()]));
    let door = |d: Door| t.iter().find(|t| t.door == d).expect("the door is offered");
    let said = |v: &Value| v["description"].as_str().unwrap_or_default().to_owned();
    let query = &door(Door::Query).input_schema["properties"];
    let project = &query["project"]["properties"];
    for member in ["group_by", "measures"] {
        assert!(said(&project[member]).contains("aggregate"), "{project}");
    }
    let measure = &project["measures"]["items"];
    assert_eq!(measure["required"], serde_json::json!(["name", "func"]));
    assert!(measure["properties"]["field"].is_object(), "{measure}");
    assert!(said(&project["fields"]).contains("rows"), "{project}");
    let depth = &query["steps"]["items"]["properties"]["depth"];
    assert_eq!(depth["required"], serde_json::json!(["min", "max"]));
    assert!(
        said(&depth["properties"]["min"]).contains("0 keeps"),
        "{depth}"
    );
    assert!(
        said(&depth["properties"]["max"]).contains("`max_depth`"),
        "{depth}"
    );
    assert!(said(depth).contains("one hop when left out"), "{depth}");
    let find = &door(Door::Find {
        ty: "invoice".into(),
    })
    .input_schema["properties"];
    for (cursor, same) in [
        (&find["cursor"], "the same `where` and `order_by`"),
        (&query["cursor"], "the same pattern"),
    ] {
        let text = said(cursor);
        assert!(
            text.contains("`next_cursor` of an earlier answer"),
            "{text}"
        );
        assert!(text.contains(same), "{text}");
    }
}

#[test]
fn a_tool_name_too_long_names_the_declaration_it_comes_from() {
    let long = "x".repeat(70);
    let from = |p: Vec<NameProblem>| -> Vec<NameSource> {
        p.into_iter()
            .map(|p| match p {
                NameProblem::TooLong { from, .. } => from.expect("the store fills it"),
                p => panic!("{p:?}"),
            })
            .collect()
    };
    let ty: TypeName = long.as_str().into();
    let door = from(name_problems(std::slice::from_ref(&ty), &[], &[]));
    assert!(
        door.iter()
            .all(|f| *f == NameSource::Type { ty: ty.clone() })
    );
    assert!(!door.is_empty());
    let (t, relation): (TypeName, RelationName) = ("a".into(), "r".repeat(70).as_str().into());
    let link = from(name_problems(&[], &[], &[(t.clone(), relation.clone())]));
    assert_eq!(link.len(), 2, "one for each link door");
    assert!(link.iter().all(|f| *f
        == NameSource::Link {
            relation: relation.clone(),
            ty: t.clone()
        }));
}
