use lakelands_api::expr::{Condition, Expr, Literal};
use lakelands_api::ontology::{
    Assignment, Authority, Cardinality, FieldDecl, FieldType, Measure, OnInputChange, RelationDecl,
    Risk, StorageKind, TriggerDecl, TypeDecl, ValueTypeDecl, VerbDecl, VerbEffect, VerbScope,
    ViewDecl, ViewSource,
};
use lakelands_api::write::{Declaration, FunctionDecl, ObjectSetDecl};
use lakelands_api::{FieldName, RelationName, TypeName, ValueTypeName, VerbName};

use super::lower::kind_name;
use super::{Bundle, BundleError, Problem, load_dir, parse};

fn files(files: &[(&str, &str)]) -> Vec<(String, String)> {
    files
        .iter()
        .map(|(p, c)| ((*p).to_owned(), (*c).to_owned()))
        .collect()
}

fn load(fs: &[(&str, &str)]) -> Result<Bundle, Vec<BundleError>> {
    parse(&files(fs))
}

fn refused(fs: &[(&str, &str)]) -> Vec<BundleError> {
    match load(fs) {
        Ok(b) => panic!("the bundle should be refused, it gave {b:?}"),
        Err(errors) => errors,
    }
}

/// Exactly one problem, of the kind expected.
fn one(fs: &[(&str, &str)], problem: Problem) -> BundleError {
    let errors = refused(fs);
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert_eq!(errors[0].problem, problem, "{errors:#?}");
    errors.into_iter().next().unwrap()
}

/// The type `name` the bundle declares.
fn ty<'b>(b: &'b Bundle, name: &str) -> &'b TypeDecl {
    let found = b.declarations.iter().find_map(|d| match d {
        Declaration::Type(t) if t.name.as_str() == name => Some(t),
        _ => None,
    });
    found.unwrap_or_else(|| panic!("the bundle declares no type `{name}`"))
}

const PARTNER: (&str, &str) = (
    "types/partner.yaml",
    "name: partner\nkey: id\nfields:\n  id: text\n",
);

/// The type the documented merge reads beside the invoices.
const PAYMENT: (&str, &str) = (
    "types/payment.yaml",
    "name: payment\nfields:\n  payer: reference(partner)\n  amount: decimal\n",
);

fn field(name: &str, ty: FieldType) -> FieldDecl {
    FieldDecl {
        name: FieldName::new(name),
        ty,
        authority: Authority::Authored,
        on_input_change: OnInputChange::Keep,
        ..Default::default()
    }
}

fn kind(k: StorageKind) -> FieldType {
    FieldType::kind(k)
}

fn text(s: &str) -> Condition {
    Condition::Text(s.to_owned())
}

// ---------------------------------------------------------------------
// A complete bundle
// ---------------------------------------------------------------------

const COMPLETE: [(&str, &str); 10] = [
    (
        "value_types/iban.yaml",
        r#"
name: iban
kind: text
constraint: "len(value) >= 15"
format: iban
description: An international bank account number.
"#,
    ),
    (
        "value_types/invoice_status.yml",
        "name: invoice_status\nkind: text\none_of: [open, paid]\n",
    ),
    (
        "types/partner.yaml",
        r#"
name: partner
key: id
fields:
  id: text
  account: iban
"#,
    ),
    (
        "types/invoice.yaml",
        r#"
name: invoice
label: Invoice
description: A bill sent to a partner.
key: [number]
valid_from: issued_on
field_meta: false
fields:
  number: text
  issued_on: date_time
  status: { type: invoice_status, searchable: true }
  partner: reference(partner)
  net: { type: decimal, required: true }
  gross:
    type: decimal
    authority: derived
    derived: "net * 1.19"
    depends_on: net
    on_input_change: clear
  summary_embedding: { type: { kind: vector, dimensions: 3 } }
relations:
  billed_to:
    target: partner
    via: partner
    inverse: invoices
    fields:
      since: date_time
"#,
    ),
    (
        "types/open_totals.yaml",
        r#"
name: open_totals
fields:
  partner: reference(partner)
  total: decimal
view:
  source: invoice
  where: "status = 'open'"
  group_by: partner
  measures:
    total: "sum(net)"
"#,
    ),
    (
        "functions/gross.yaml",
        r#"
name: gross
inputs:
  net: decimal
expr: "net * 1.19"
"#,
    ),
    (
        "verbs/pay_invoice.yaml",
        r#"
name: pay_invoice
on: invoice
risk: low
requires: "status = 'open'"
payload:
  paid_on: { type: date_time, required: true }
set:
  status: "'paid'"
"#,
    ),
    (
        "verbs/send_reminder.yaml",
        "name: send_reminder\non: invoice\nrisk: medium\nexecutor: mailer\n",
    ),
    (
        "triggers/invoice_paid.yaml",
        "name: invoice_paid\non: invoice\nwhen: \"changed(status) and status = 'paid'\"\n",
    ),
    (
        "object_sets/open_invoices.yaml",
        "name: open_invoices\nof: invoice\nwhere: \"status = 'open'\"\n",
    ),
];

fn complete_declarations() -> Vec<Declaration> {
    let status_ty = FieldType {
        value_type: Some(ValueTypeName::new("invoice_status")),
        ..kind(StorageKind::Text)
    };
    let iban_ty = FieldType {
        value_type: Some(ValueTypeName::new("iban")),
        ..kind(StorageKind::Text)
    };
    let invoice = TypeDecl {
        name: TypeName::new("invoice"),
        key: vec![FieldName::new("number")],
        fields: vec![
            field("number", kind(StorageKind::Text)),
            field("issued_on", kind(StorageKind::DateTime)),
            FieldDecl {
                searchable: true,
                ..field("status", status_ty)
            },
            field(
                "partner",
                FieldType {
                    target: Some(TypeName::new("partner")),
                    ..kind(StorageKind::Reference)
                },
            ),
            FieldDecl {
                required: true,
                ..field("net", kind(StorageKind::Decimal))
            },
            FieldDecl {
                authority: Authority::Derived,
                derived: Some(text("net * 1.19")),
                depends_on: vec![FieldName::new("net")],
                on_input_change: OnInputChange::Clear,
                ..field("gross", kind(StorageKind::Decimal))
            },
            field(
                "summary_embedding",
                FieldType {
                    dimensions: Some(3),
                    ..kind(StorageKind::Vector)
                },
            ),
        ],
        relations: vec![RelationDecl {
            name: RelationName::new("billed_to"),
            target: TypeName::new("partner"),
            cardinality: Cardinality::ManyToOne,
            inverse: Some(RelationName::new("invoices")),
            via: Some(FieldName::new("partner")),
            fields: vec![field("since", kind(StorageKind::DateTime))],
            ..Default::default()
        }],
        valid_from: Some(FieldName::new("issued_on")),
        description: Some("A bill sent to a partner.".to_owned()),
        label: Some("Invoice".to_owned()),
        ..Default::default()
    };
    let plain_type = |name: &str| TypeDecl {
        name: TypeName::new(name),
        field_meta: true,
        ..Default::default()
    };
    vec![
        Declaration::ValueType(ValueTypeDecl {
            name: ValueTypeName::new("iban"),
            kind: StorageKind::Text,
            constraint: Some(text("len(value) >= 15")),
            one_of: Vec::new(),
            format: Some("iban".to_owned()),
            scale: None,
            description: Some("An international bank account number.".to_owned()),
        }),
        Declaration::ValueType(ValueTypeDecl {
            name: ValueTypeName::new("invoice_status"),
            kind: StorageKind::Text,
            constraint: None,
            one_of: vec!["open".to_owned(), "paid".to_owned()],
            format: None,
            scale: None,
            description: None,
        }),
        Declaration::Type(invoice),
        Declaration::Type(TypeDecl {
            view: Some(Box::new(ViewDecl {
                source: TypeName::new("invoice"),
                r#where: Some(text("status = 'open'")),
                group_by: vec![FieldName::new("partner")],
                measures: vec![Measure {
                    name: FieldName::new("total"),
                    expr: text("sum(net)"),
                }],
                params: Default::default(),
                merge: Default::default(),
            })),
            fields: vec![
                field(
                    "partner",
                    FieldType {
                        target: Some(TypeName::new("partner")),
                        ..kind(StorageKind::Reference)
                    },
                ),
                field("total", kind(StorageKind::Decimal)),
            ],
            ..plain_type("open_totals")
        }),
        Declaration::Type(TypeDecl {
            key: vec![FieldName::new("id")],
            fields: vec![
                field("id", kind(StorageKind::Text)),
                field("account", iban_ty),
            ],
            ..plain_type("partner")
        }),
        Declaration::Function(FunctionDecl {
            name: "gross".to_owned(),
            backing: "expr".to_owned(),
            inputs: vec![field("net", kind(StorageKind::Decimal))],
            expr: Some(text("net * 1.19")),
            config: serde_json::Value::Null,
            description: None,
        }),
        Declaration::Verb(VerbDecl {
            name: VerbName::new("pay_invoice"),
            on: TypeName::new("invoice"),
            scope: Default::default(),
            payload: vec![FieldDecl {
                required: true,
                ..field("paid_on", kind(StorageKind::DateTime))
            }],
            risk: Risk::Low,
            requires: Some(text("status = 'open'")),
            effect: VerbEffect::Write {
                set: vec![Assignment {
                    field: FieldName::new("status"),
                    value: text("'paid'"),
                }],
            },
            description: None,
        }),
        Declaration::Verb(VerbDecl {
            name: VerbName::new("send_reminder"),
            on: TypeName::new("invoice"),
            scope: Default::default(),
            payload: Vec::new(),
            risk: Risk::Medium,
            requires: None,
            effect: VerbEffect::External {
                executor: "mailer".to_owned(),
            },
            description: None,
        }),
        Declaration::Trigger(TriggerDecl {
            max_depth: None,
            name: "invoice_paid".into(),
            on: TypeName::new("invoice"),
            when: Some(text("changed(status) and status = 'paid'")),
            description: None,
        }),
        Declaration::ObjectSet(ObjectSetDecl {
            name: "open_invoices".to_owned(),
            of: TypeName::new("invoice"),
            filter: text("status = 'open'"),
            description: None,
        }),
    ]
}

#[test]
fn a_complete_bundle_becomes_its_declarations_in_lane_then_name_order() {
    let bundle = load(&COMPLETE).expect("the complete bundle loads");
    assert_eq!(bundle.declarations, complete_declarations());
}

#[test]
fn the_declarations_survive_their_json_form() {
    let bundle = load(&COMPLETE).expect("the complete bundle loads");
    for d in &bundle.declarations {
        let json = serde_json::to_string(d).expect("serialises");
        let back: Declaration = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(&back, d, "{json}");
    }
}

#[test]
fn the_order_of_the_files_does_not_change_the_output() {
    let mut reversed = COMPLETE;
    reversed.reverse();
    assert_eq!(load(&reversed), load(&COMPLETE));
}

#[test]
fn value_types_and_functions_come_before_the_types_that_use_them() {
    let bundle = load(&COMPLETE).expect("loads");
    let lane = |d: &Declaration| match d {
        Declaration::ValueType(_) => 0,
        Declaration::Type(_) => 1,
        Declaration::Function(_) => 2,
        Declaration::Verb(_) => 3,
        Declaration::Trigger(_) => 4,
        Declaration::ObjectSet(_) => 5,
    };
    let lanes: Vec<u8> = bundle.declarations.iter().map(lane).collect();
    let mut sorted = lanes.clone();
    sorted.sort();
    assert_eq!(lanes, sorted);
}

#[test]
fn an_empty_bundle_declares_nothing() {
    assert_eq!(
        load(&[]),
        Ok(Bundle {
            declarations: vec![],
            aging_rules: vec![],
        })
    );
}

const INVOICE: (&str, &str) = (
    "types/invoice.yaml",
    "name: invoice\nkey: number\nfields:\n  number: text\n  status: text\n  due_on: date_time\n",
);

#[test]
fn an_aging_rule_is_the_fields_of_its_record_sorted_by_name() {
    let rule = |name: &str| {
        format!(
            "name: {name}\non: invoice\nfield: status\nfrom: open\nto: overdue\n\
             when: \"due_on < today() - 30d\"\n"
        )
    };
    let (b, a) = (rule("b_late"), rule("a_overdue"));
    let bundle = load(&[
        INVOICE,
        ("aging_rules/1.yaml", &b),
        ("aging_rules/2.yaml", &a),
    ])
    .expect("loads");
    let names: Vec<&serde_json::Value> = bundle.aging_rules.iter().map(|p| &p["name"]).collect();
    assert_eq!(
        names,
        [
            &serde_json::json!("a_overdue"),
            &serde_json::json!("b_late")
        ]
    );
    assert_eq!(
        serde_json::to_value(&bundle.aging_rules[0]).unwrap(),
        serde_json::json!({"name": "a_overdue", "on": "invoice", "field": "status",
                           "from": "open", "to": "overdue", "when": "due_on < today() - 30d"})
    );
    // A rule's name is its own: a trigger may carry it too.
    let trigger = ("triggers/t.yaml", "name: a_overdue\non: invoice\n");
    assert!(load(&[INVOICE, ("aging_rules/2.yaml", &a), trigger]).is_ok());
}

#[test]
fn an_aging_rule_is_checked_as_a_bundle_file() {
    let rule = |extra: &str| {
        format!(
            "name: r\non: invoice\nfield: status\nfrom: open\nto: overdue\nwhen: \"true\"\n{extra}"
        )
    };
    let e = one(
        &[INVOICE, ("aging_rules/r.yaml", &rule("every: day\n"))],
        Problem::Malformed,
    );
    assert!(e.message.contains("every"), "{e:?}");
    let field = rule("").replace("field: status", "field: state");
    let e = one(
        &[INVOICE, ("aging_rules/r.yaml", &field)],
        Problem::UnknownReference,
    );
    assert!(e.message.contains("state"), "{e:?}");
    // A type the bundle does not declare is the lake's to check.
    let on = rule("").replace("on: invoice", "on: bill");
    assert!(load(&[("aging_rules/r.yaml", &on)]).is_ok());
    let when = rule("").replace("when: \"true\"", "when: \"status =\"");
    one(
        &[INVOICE, ("aging_rules/r.yaml", &when)],
        Problem::BadExpression,
    );
    let r = rule("");
    one(
        &[
            INVOICE,
            ("aging_rules/a.yaml", &r),
            ("aging_rules/b.yaml", &r),
        ],
        Problem::DuplicateName,
    );
}

#[test]
fn every_authored_key_of_a_field_arrives() {
    let bundle = load(&[(
        "types/memo.yaml",
        r#"
name: memo
fields:
  body:
    type: text
    required: true
    authority: shared
    sensitive: true
    searchable: true
    description: What the memo says.
    label: Body
"#,
    )])
    .expect("loads");
    let Declaration::Type(t) = &bundle.declarations[0] else {
        panic!("a type")
    };
    assert_eq!(
        t.fields[0],
        FieldDecl {
            required: true,
            authority: Authority::Shared,
            sensitive: true,
            searchable: true,
            description: Some("What the memo says.".to_owned()),
            label: Some("Body".to_owned()),
            ..field("body", kind(StorageKind::Text))
        }
    );
}

#[test]
fn an_embedding_arrives_with_its_fields_and_model_and_a_bare_field_is_one_source() {
    let bundle = load(&[(
        "types/memo.yaml",
        r#"
name: memo
fields:
  title: text
  body: text
  both:
    type: vector(2)
    authority: derived
    embed: { from: [title, body], model: tiny }
  one:
    type: vector(2)
    authority: derived
    embed: { from: body, model: tiny }
"#,
    )])
    .expect("loads");
    let Declaration::Type(t) = &bundle.declarations[0] else {
        panic!("a type")
    };
    let embed = |from: &[&str]| {
        Some(lakelands_api::ontology::EmbedDecl {
            from: from.iter().map(|f| FieldName::new(*f)).collect(),
            model: "tiny".into(),
        })
    };
    assert_eq!(t.fields[2].embed, embed(&["title", "body"]));
    assert_eq!(t.fields[3].embed, embed(&["body"]));
    let unknown = refused(&[(
        "types/memo.yaml",
        "name: memo\nfields:\n  v:\n    type: vector(2)\n    embed: { from: body, model: m, dims: 2 }\n",
    )]);
    assert!(!unknown.is_empty());
}

#[test]
fn a_field_keeps_the_order_it_was_written_in() {
    let bundle = load(&[(
        "types/t.yaml",
        "name: t\nfields:\n  z: text\n  a: text\n  m: text\n",
    )])
    .expect("loads");
    let Declaration::Type(t) = &bundle.declarations[0] else {
        panic!("a type")
    };
    let names: Vec<&str> = t.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["z", "a", "m"]);
}

#[test]
fn the_short_field_types_mean_their_object_forms() {
    let bundle = load(&[
        PARTNER,
        (
            "value_types/partner_ref.yaml",
            "name: partner_ref\nkind: reference\n",
        ),
        ("value_types/embedding.yaml", "name: embedding\nkind: vector\n"),
        (
            "types/t.yaml",
            "name: t\nfields:\n  a: partner_ref(partner)\n  b: embedding(8)\n  c: vector( 4 )\n  d: geo\n",
        ),
    ])
    .expect("loads");
    let t = ty(&bundle, "t");
    let tys: Vec<&FieldType> = t.fields.iter().map(|f| &f.ty).collect();
    assert_eq!(
        tys,
        [
            &FieldType {
                value_type: Some(ValueTypeName::new("partner_ref")),
                target: Some(TypeName::new("partner")),
                ..kind(StorageKind::Reference)
            },
            &FieldType {
                value_type: Some(ValueTypeName::new("embedding")),
                dimensions: Some(8),
                ..kind(StorageKind::Vector)
            },
            &FieldType {
                dimensions: Some(4),
                ..kind(StorageKind::Vector)
            },
            &kind(StorageKind::Geo),
        ]
    );
}

#[test]
fn a_typed_expression_is_accepted_as_written() {
    let bundle = load(&[
        PARTNER,
        (
            "object_sets/all.yaml",
            "name: all\nof: partner\nwhere: { op: lit, value: { type: bool, value: true } }\n",
        ),
    ])
    .expect("loads");
    let Declaration::ObjectSet(s) = &bundle.declarations[1] else {
        panic!("an object set")
    };
    assert_eq!(
        s.filter,
        Condition::Expr(Expr::Lit {
            value: Literal::Bool(true)
        })
    );
}

#[test]
fn a_function_with_another_backing_carries_its_configuration() {
    let bundle = load(&[(
        "functions/geocode.yaml",
        "name: geocode\nbacking: http\ninputs:\n  address: text\nconfig:\n  url: https://geo.example/lookup\n  retries: 2\n",
    )])
    .expect("loads");
    let Declaration::Function(f) = &bundle.declarations[0] else {
        panic!("a function")
    };
    assert_eq!(f.backing, "http");
    assert_eq!(f.expr, None);
    assert_eq!(
        f.config,
        serde_json::json!({ "url": "https://geo.example/lookup", "retries": 2 })
    );
}

#[test]
fn a_sys_type_may_be_referred_to() {
    load(&[(
        "types/t.yaml",
        "name: t\nfields:\n  from: reference(sys.source)\n",
    )])
    .expect("the lake checks sys. references, not the bundle");
}

/// The examples in the module documentation are a valid bundle.
#[test]
fn the_documented_examples_load() {
    let doc: String = include_str!("mod.rs")
        .lines()
        .filter_map(|l| l.strip_prefix("//!"))
        .map(|l| l.strip_prefix(' ').unwrap_or(l))
        .collect::<Vec<_>>()
        .join("\n");
    let mut fs = vec![PARTNER, PAYMENT];
    let mut owned = Vec::new();
    for block in doc.split("```yaml\n").skip(1) {
        let body = block.split("```").next().unwrap_or_default();
        let has = |key: &str| body.lines().any(|l| l.starts_with(key));
        let lane = if has("from:") {
            "aging_rules"
        } else if has("kind:") {
            "value_types"
        } else if has("risk:") {
            "verbs"
        } else if has("of:") {
            "object_sets"
        } else if has("expr:") || has("backing:") {
            "functions"
        } else if has("when:") {
            "triggers"
        } else {
            "types"
        };
        owned.push((format!("{lane}/{}.yaml", owned.len()), body.to_owned()));
    }
    assert_eq!(owned.len(), 11, "every documented example is found");
    fs.extend(owned.iter().map(|(p, c)| (p.as_str(), c.as_str())));
    load(&fs).expect("the documented examples load");
}

#[test]
fn the_documented_kinds_are_the_storage_kinds() {
    let doc = include_str!("mod.rs");
    for k in StorageKind::ALL {
        assert!(doc.contains(&format!("`{}`", kind_name(k))), "{k:?}");
    }
}

#[test]
fn load_dir_reads_what_parse_is_given_and_skips_hidden_entries() {
    let dir = std::env::temp_dir().join(format!("lakelands-bundle-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, contents) in COMPLETE {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, contents).unwrap();
    }
    std::fs::write(dir.join("README.md"), "A bundle.").unwrap();
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(dir.join(".git/config.yaml"), "not: a lane").unwrap();
    std::fs::write(dir.join("types/.draft.yaml"), "junk: [").unwrap();
    let loaded = load_dir(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(loaded, load(&COMPLETE));
}

#[test]
fn load_dir_refuses_a_missing_directory() {
    let dir = std::env::temp_dir().join("lakelands-bundle-test-does-not-exist");
    let errors = load_dir(&dir).expect_err("nothing to read");
    assert_eq!(errors[0].problem, Problem::Unreadable);
}

// ---------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------

#[test]
fn every_problem_is_reported_at_once_by_file() {
    let errors = refused(&[
        ("verbs/v.yaml", "name: v\non: nowhere\nrisk: low\nset: {}\n"),
        ("types/t.yaml", "name: t\nfields:\n  a: nothing\n"),
    ]);
    let files: Vec<&str> = errors.iter().map(|e| e.file.as_str()).collect();
    assert_eq!(files, ["types/t.yaml", "verbs/v.yaml"]);
}

#[test]
fn a_yaml_file_outside_the_lanes_is_refused() {
    let e = one(
        &[("ontology/seeds.yaml", "types: []\n")],
        Problem::UnknownLane,
    );
    assert_eq!(e.file, "ontology/seeds.yaml");
    assert!(e.message.contains("`types/`"), "{e}");
}

#[test]
fn a_directory_inside_a_lane_is_refused() {
    one(
        &[("types/billing/invoice.yaml", "name: invoice\n")],
        Problem::UnknownLane,
    );
}

#[test]
fn a_file_in_a_lane_that_is_not_yaml_is_refused() {
    one(&[("types/invoice.json", "{}")], Problem::NotYaml);
}

#[test]
fn a_file_outside_the_lanes_that_is_not_yaml_is_ignored() {
    assert_eq!(load(&[("README.md", "# Bundle")]), load(&[]));
}

#[test]
fn an_unknown_key_on_a_type_is_refused_by_name() {
    let e = one(
        &[("types/feed.yaml", "name: feed\norigin: source\n")],
        Problem::Malformed,
    );
    assert!(e.message.contains("unknown field `origin`"), "{e}");
}

#[test]
fn an_unknown_key_on_a_field_is_refused_by_name() {
    let e = one(
        &[(
            "types/feed.yaml",
            "name: feed\nfields:\n  amount:\n    type: decimal\n    origin: source\n",
        )],
        Problem::Malformed,
    );
    assert!(e.message.contains("unknown field `origin`"), "{e}");
}

#[test]
fn an_unknown_key_in_every_lane_is_refused() {
    for (path, doc) in [
        ("value_types/x.yaml", "name: x\nkind: text\nextra: 1\n"),
        ("functions/x.yaml", "name: x\nexpr: \"1\"\nextra: 1\n"),
        (
            "verbs/x.yaml",
            "name: x\non: partner\nrisk: low\nset: {}\nextra: 1\n",
        ),
        ("triggers/x.yaml", "name: x\non: partner\nextra: 1\n"),
        (
            "object_sets/x.yaml",
            "name: x\nof: partner\nwhere: \"true\"\nextra: 1\n",
        ),
        (
            "aging_rules/x.yaml",
            "name: x\non: partner\nfield: id\nfrom: 1\nto: 2\nwhen: \"true\"\nextra: 1\n",
        ),
    ] {
        let e = one(&[PARTNER, (path, doc)], Problem::Malformed);
        assert!(e.message.contains("`extra`"), "{path}: {e}");
    }
}

#[test]
fn a_missing_key_is_refused() {
    let e = one(
        &[PARTNER, ("verbs/v.yaml", "name: v\non: partner\nset: {}\n")],
        Problem::Malformed,
    );
    assert!(e.message.contains("`risk`"), "{e}");
}

#[test]
fn a_field_named_twice_is_refused() {
    let e = one(
        &[("types/t.yaml", "name: t\nfields:\n  a: text\n  a: number\n")],
        Problem::Malformed,
    );
    assert!(e.message.contains('a'), "{e}");
}

#[test]
fn a_name_declared_twice_in_the_bundle_is_refused() {
    let e = one(
        &[
            PARTNER,
            ("triggers/partner.yaml", "name: partner\non: partner\n"),
        ],
        Problem::DuplicateName,
    );
    assert_eq!(e.file, "types/partner.yaml");
    assert!(e.message.contains("triggers/partner.yaml"), "{e}");
}

#[test]
fn a_sys_type_is_refused() {
    let e = one(
        &[("types/setting.yaml", "name: sys.setting\n")],
        Problem::ReservedName,
    );
    assert!(e.message.contains("sys."), "{e}");
}

#[test]
fn a_value_type_named_like_a_storage_kind_is_refused() {
    one(
        &[("value_types/decimal.yaml", "name: decimal\nkind: text\n")],
        Problem::ReservedName,
    );
}

#[test]
fn names_that_are_not_ascii_identifiers_are_refused() {
    for name in ["a\\0b", "a-b", "grüße", "1a"] {
        one(
            &[("types/t.yaml", &format!("name: \"{name}\"\n"))],
            Problem::BadName,
        );
        one(
            &[(
                "types/t.yaml",
                &format!("name: t\nfields:\n  \"{name}\": text\n"),
            )],
            Problem::BadName,
        );
    }
    one(
        &[("types/t.yaml", "name: t\nfields:\n  a.b: text\n")],
        Problem::BadName,
    );
}

#[test]
fn an_empty_name_is_refused() {
    let errors = refused(&[("triggers/t.yaml", "name: \"\"\non: sys.source\n")]);
    assert!(
        errors.iter().any(|e| e.problem == Problem::BadName),
        "{errors:#?}"
    );
}

#[test]
fn types_whose_tool_names_collide_are_refused() {
    let e = one(
        &[
            ("types/a.yaml", "name: a.b\n"),
            ("types/b.yaml", "name: a_b\n"),
        ],
        Problem::ToolName,
    );
    assert_eq!(e.file, "types/b.yaml");
    assert!(e.message.contains("types/a.yaml"), "{e}");
}

#[test]
fn a_verb_named_like_a_generic_tool_is_refused() {
    let e = one(
        &[
            PARTNER,
            (
                "verbs/d.yaml",
                "name: describe\non: partner\nrisk: low\nset: {}\n",
            ),
        ],
        Problem::ToolName,
    );
    assert_eq!(e.file, "verbs/d.yaml");
    assert!(e.message.contains("generic tool"), "{e}");
}

#[test]
fn a_type_whose_tool_names_run_long_is_refused() {
    let name = "t".repeat(60);
    let doc = format!("name: {name}\n");
    let e = one(&[("types/long.yaml", doc.as_str())], Problem::ToolName);
    assert!(e.message.contains("longer than 64"), "{e}");
}

#[test]
fn a_reference_to_an_undeclared_type_is_refused() {
    let e = one(
        &[(
            "types/invoice.yaml",
            "name: invoice\nfields:\n  partner: reference(partner)\n",
        )],
        Problem::UnknownReference,
    );
    assert!(e.message.contains("`partner`"), "{e}");
}

#[test]
fn a_relation_to_an_undeclared_type_is_refused() {
    one(
        &[(
            "types/invoice.yaml",
            "name: invoice\nrelations:\n  billed_to: { target: partner }\n",
        )],
        Problem::UnknownReference,
    );
}

#[test]
fn a_verb_trigger_set_or_view_on_an_undeclared_type_is_refused() {
    for (path, doc) in [
        ("verbs/v.yaml", "name: v\non: ghost\nrisk: low\nset: {}\n"),
        ("triggers/t.yaml", "name: t\non: ghost\n"),
        (
            "object_sets/s.yaml",
            "name: s\nof: ghost\nwhere: \"true\"\n",
        ),
        (
            "types/v.yaml",
            "name: v\nview:\n  source: ghost\n  measures:\n    n: \"count()\"\n",
        ),
    ] {
        let e = one(&[(path, doc)], Problem::UnknownReference);
        assert!(e.message.contains("`ghost`"), "{path}: {e}");
    }
}

#[test]
fn an_undeclared_value_type_is_refused() {
    let e = one(
        &[("types/t.yaml", "name: t\nfields:\n  account: iban\n")],
        Problem::UnknownReference,
    );
    assert!(e.message.contains("`iban`"), "{e}");
    one(
        &[(
            "types/t.yaml",
            "name: t\nfields:\n  account: { type: { kind: text, value_type: iban } }\n",
        )],
        Problem::UnknownReference,
    );
}

#[test]
fn a_file_that_does_not_parse_is_reported_once_not_again_by_its_users() {
    let errors = refused(&[
        ("value_types/iban.yaml", "name: iban\nkind: txt\n"),
        ("types/t.yaml", "name: t\nfields:\n  account: iban\n"),
    ]);
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert_eq!(errors[0].file, "value_types/iban.yaml");
}

#[test]
fn a_risk_other_than_low_medium_or_high_is_refused() {
    let e = one(
        &[
            PARTNER,
            (
                "verbs/v.yaml",
                "name: v\non: partner\nrisk: critical\nset: {}\n",
            ),
        ],
        Problem::Malformed,
    );
    assert!(e.message.contains("critical"), "{e}");
    one(
        &[
            PARTNER,
            ("verbs/v.yaml", "name: v\non: partner\nrisk: Low\nset: {}\n"),
        ],
        Problem::Malformed,
    );
}

#[test]
fn a_text_expression_that_does_not_parse_is_refused_wherever_it_is() {
    for (path, doc) in [
        (
            "value_types/x.yaml",
            "name: x\nkind: text\nconstraint: \"len(\"\n",
        ),
        (
            "types/x.yaml",
            "name: x\nfields:\n  a: { type: number, derived: \"a +\" }\n",
        ),
        (
            "types/x.yaml",
            "name: x\nview:\n  source: partner\n  where: \"= 1\"\n  measures:\n    n: \"count()\"\n",
        ),
        (
            "types/x.yaml",
            "name: x\nview:\n  source: partner\n  measures:\n    n: \"nosuch(1)\"\n",
        ),
        ("functions/x.yaml", "name: x\nexpr: \"1 +\"\n"),
        (
            "verbs/x.yaml",
            "name: x\non: partner\nrisk: low\nrequires: \"and\"\nset: {}\n",
        ),
        (
            "verbs/x.yaml",
            "name: x\non: partner\nrisk: low\nset:\n  id: \"'open\"\n",
        ),
        (
            "triggers/x.yaml",
            "name: x\non: partner\nwhen: \"changed(\"\n",
        ),
        (
            "object_sets/x.yaml",
            "name: x\nof: partner\nwhere: \"a = = b\"\n",
        ),
    ] {
        let e = one(&[PARTNER, (path, doc)], Problem::BadExpression);
        assert!(e.message.contains("does not parse"), "{path}: {e}");
    }
}

#[test]
fn a_reference_without_its_target_is_refused() {
    one(
        &[("types/t.yaml", "name: t\nfields:\n  p: reference\n")],
        Problem::BadFieldType,
    );
}

#[test]
fn a_vector_without_its_length_is_refused() {
    for ty in [
        "vector",
        "vector(0)",
        "vector(many)",
        "{ type: { kind: vector } }",
    ] {
        let doc = format!("name: t\nfields:\n  v: {ty}\n");
        one(&[("types/t.yaml", doc.as_str())], Problem::BadFieldType);
    }
}

#[test]
fn an_argument_where_none_belongs_is_refused() {
    for ty in [
        "decimal(2)",
        "text(",
        "reference()",
        "{ type: { kind: text, target: partner } }",
        "{ type: { kind: text, dimensions: 3 } }",
    ] {
        let doc = format!("name: t\nfields:\n  v: \"{ty}\"\n");
        let doc = if ty.starts_with('{') {
            format!("name: t\nfields:\n  v: {ty}\n")
        } else {
            doc
        };
        one(
            &[PARTNER, ("types/t.yaml", doc.as_str())],
            Problem::BadFieldType,
        );
    }
}

#[test]
fn a_value_type_over_another_kind_than_the_field_is_refused() {
    let e = one(
        &[
            ("value_types/iban.yaml", "name: iban\nkind: text\n"),
            (
                "types/t.yaml",
                "name: t\nfields:\n  a: { type: { kind: decimal, value_type: iban } }\n",
            ),
        ],
        Problem::BadFieldType,
    );
    assert!(e.message.contains("`text`"), "{e}");
}

#[test]
fn one_of_on_a_value_type_that_is_not_text_is_refused() {
    one(
        &[(
            "value_types/n.yaml",
            "name: n\nkind: number\none_of: [\"1\"]\n",
        )],
        Problem::Contradiction,
    );
}

#[test]
fn a_verb_needs_exactly_one_effect() {
    one(
        &[
            PARTNER,
            ("verbs/v.yaml", "name: v\non: partner\nrisk: low\n"),
        ],
        Problem::Contradiction,
    );
    one(
        &[
            PARTNER,
            (
                "verbs/v.yaml",
                "name: v\non: partner\nrisk: low\nset: {}\nexecutor: mailer\n",
            ),
        ],
        Problem::Contradiction,
    );
}

#[test]
fn a_verb_doc_carries_its_scope() {
    let verb = |body: &str| {
        let b = load(&[PARTNER, ("verbs/v.yaml", body)]).unwrap();
        b.declarations
            .into_iter()
            .find_map(|d| match d {
                Declaration::Verb(v) => Some(v),
                _ => None,
            })
            .unwrap()
    };
    let on_type = verb("name: v\non: partner\nscope: type\nrisk: low\nexecutor: crm\n");
    assert_eq!(on_type.scope, VerbScope::Type);
    let on_record = verb("name: v\non: partner\nrisk: low\nexecutor: crm\n");
    assert_eq!(
        on_record.scope,
        VerbScope::Record,
        "a record is the default"
    );
    one(
        &[
            PARTNER,
            (
                "verbs/v.yaml",
                "name: v\non: partner\nscope: table\nrisk: low\nexecutor: crm\n",
            ),
        ],
        Problem::Malformed,
    );
}

#[test]
fn a_function_has_exactly_one_definition() {
    let e = one(&[("functions/f.yaml", "name: f\n")], Problem::Contradiction);
    assert!(e.message.contains("`expr`"), "{e}");
    one(
        &[("functions/f.yaml", "name: f\nbacking: http\nexpr: \"1\"\n")],
        Problem::Contradiction,
    );
}

#[test]
fn a_type_that_names_fields_it_does_not_have_is_refused() {
    for body in [
        "name: t\nkey: [nope]\nfields:\n  a: text\n",
        "name: t\nvalid_from: nope\nfields:\n  a: text\n",
        "name: t\nfields:\n  a: text\n  b: { type: text, depends_on: nope }\n",
        "name: t\nfields:\n  a: text\nrelations:\n  r: { target: t, via: nope }\n",
    ] {
        one(&[("types/t.yaml", body)], Problem::UnknownReference);
    }
}

#[test]
fn a_view_groups_only_by_fields_of_its_source() {
    one(
        &[
            ("types/s.yaml", "name: s\nfields:\n  a: text\n"),
            (
                "types/v.yaml",
                "name: v\nview:\n  source: s\n  group_by: nope\n  measures:\n    n: \"count()\"\n",
            ),
        ],
        Problem::UnknownReference,
    );
}

#[test]
fn an_inverse_may_not_take_a_name_its_target_already_uses() {
    one(
        &[
            (
                "types/a.yaml",
                "name: a\nrelations:\n  to_b: { target: b, inverse: back }\n",
            ),
            (
                "types/b.yaml",
                "name: b\nrelations:\n  back: { target: a }\n",
            ),
        ],
        Problem::Contradiction,
    );
}

#[test]
fn a_json_field_may_not_carry_the_name_of_a_type() {
    one(
        &[
            ("types/partner.yaml", "name: partner\nfields:\n  a: text\n"),
            ("types/t.yaml", "name: t\nfields:\n  partner: json\n"),
        ],
        Problem::Contradiction,
    );
    assert!(
        load(&[
            ("types/partner.yaml", "name: partner\nfields:\n  a: text\n"),
            (
                "types/t.yaml",
                "name: t\nfields:\n  partner: reference(partner)\n"
            ),
        ])
        .is_ok(),
        "a reference field may share its target's name"
    );
}

#[test]
fn annotations_lower_as_json_and_a_bad_key_is_reported_against_its_file() {
    let shelf = (
        "types/shelf.yaml",
        "name: shelf\nkey: code\nannotations:\n  acme.icon: books\n  acme.package: {name: library, weight: 1.5}\nfields:\n  code:\n    type: text\n    annotations:\n      acme.format: shelf-code\nrelations:\n  partner:\n    target: partner\n    annotations:\n      acme.plural: partners\n",
    );
    let bundle = load(&[PARTNER, shelf]).expect("the bundle loads");
    let t = ty(&bundle, "shelf");
    assert_eq!(t.annotations["acme.icon"], serde_json::json!("books"));
    assert_eq!(
        t.annotations["acme.package"],
        serde_json::json!({"name": "library", "weight": 1.5})
    );
    assert_eq!(
        t.fields[0].annotations["acme.format"],
        serde_json::json!("shelf-code")
    );
    assert_eq!(
        t.relations[0].annotations["acme.plural"],
        serde_json::json!("partners")
    );
    let bad = (
        "types/shelf.yaml",
        "name: shelf\nannotations:\n  icon: books\nfields:\n  code: text\n",
    );
    let e = one(&[bad], Problem::Malformed);
    assert_eq!(e.file, "types/shelf.yaml");
    assert!(e.message.contains("`icon`"), "{}", e.message);
    let reserved = (
        "types/shelf.yaml",
        "name: shelf\nannotations:\n  sys.icon: books\nfields:\n  code: text\n",
    );
    let e = one(&[reserved], Problem::ReservedName);
    assert!(e.message.contains("sys.icon"), "{}", e.message);
}

#[test]
fn a_derived_relation_lowers_from_its_yaml_match() {
    let invoice = (
        "types/invoice.yaml",
        "name: invoice\nkey: number\nfields:\n  number: text\n  partner_id: text\nrelations:\n  partner:\n    target: partner\n    derived:\n      match:\n        - from: partner_id\n          to: id\n",
    );
    let bundle = load(&[PARTNER, invoice]).expect("the bundle loads");
    let t = ty(&bundle, "invoice");
    let d = t.relations[0]
        .derived
        .as_ref()
        .expect("the relation is derived");
    assert_eq!(d.pairs.len(), 1);
    assert_eq!(d.pairs[0].from.as_str(), "partner_id");
    assert_eq!(d.pairs[0].to.as_str(), "id");
}

#[test]
fn a_triggers_max_depth_lowers_from_its_yaml() {
    let trigger = (
        "triggers/once.yaml",
        "name: once\non: partner\nmax_depth: 1\n",
    );
    let bundle = load(&[PARTNER, trigger]).expect("the bundle loads");
    let t = bundle
        .declarations
        .iter()
        .find_map(|d| match d {
            Declaration::Trigger(t) => Some(t),
            _ => None,
        })
        .expect("the trigger is declared");
    assert_eq!(t.max_depth, Some(1));
}

#[test]
fn a_views_parameters_arrive_in_the_order_they_were_written() {
    let view = (
        "types/window.yaml",
        "name: window\nview:\n  source: invoice\n  params:\n    until: date_time\n    state: text\n  \
         where: \"due_on <= param('until') and status = param('state')\"\n  group_by: status\n  \
         measures:\n    invoices: \"count()\"\n",
    );
    let bundle = load(&[INVOICE, view]).expect("a view with parameters loads");
    let view = ty(&bundle, "window").view.as_ref();
    let params = view
        .map(|v| v.params.clone())
        .expect("the view is declared");
    let got: Vec<(&str, StorageKind)> = params.iter().map(|p| (p.name.as_str(), p.kind)).collect();
    assert_eq!(
        got,
        [
            ("until", StorageKind::DateTime),
            ("state", StorageKind::Text)
        ]
    );
}

#[test]
fn a_view_parameter_of_no_storage_kind_is_refused_naming_it() {
    let view = (
        "types/window.yaml",
        "name: window\nview:\n  source: invoice\n  params:\n    until: date\n  \
         where: \"due_on <= param('until')\"\n  measures:\n    invoices: \"count()\"\n",
    );
    let e = one(&[INVOICE, view], Problem::BadFieldType);
    assert!(
        e.message.contains("`until`") && e.message.contains("`date`"),
        "{e}"
    );
}

#[test]
fn a_views_merged_source_arrives_whole() {
    let view = (
        "types/paid.yaml",
        "name: paid\nfields:\n  partner: reference(partner)\n  invoices: number\n  paid: decimal\n\
         view:\n  source: invoice\n  group_by: partner\n  measures:\n    invoices: \"count()\"\n  \
         merge:\n    - source: payment\n      where: \"amount > 0\"\n      group_by: payer\n      \
         measures:\n        paid: \"sum(amount)\"\n",
    );
    let invoice = (
        "types/invoice.yaml",
        "name: invoice\nfields:\n  partner: reference(partner)\n",
    );
    let bundle = load(&[PARTNER, PAYMENT, invoice, view]).expect("a view with a merge loads");
    let view = ty(&bundle, "paid").view.as_ref();
    let merge = view.map(|v| v.merge.clone()).expect("the view is declared");
    assert_eq!(
        merge,
        [ViewSource {
            source: TypeName::new("payment"),
            r#where: Some(text("amount > 0")),
            group_by: vec![FieldName::new("payer")],
            measures: vec![Measure {
                name: FieldName::new("paid"),
                expr: text("sum(amount)"),
            }],
        }]
    );
}
