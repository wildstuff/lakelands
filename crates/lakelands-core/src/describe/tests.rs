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

/// The lake as `who` is shown `declared`, and one type of it.
fn lake(lake: &LakeId, declared: &Vec<Declared>, who: &Principal) -> LakeDescription {
    Sight::held(who, declared).describe(lake)
}

fn one_type(ty: &TypeName, d: &Vec<Declared>, who: &Principal) -> Option<TypeDescription> {
    Sight::held(who, d).describe_type(ty)
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

#[test]
fn views_an_imported_log_holds_in_a_cycle_are_each_judged_once() {
    let view = |name: &str, source: &str| {
        // `b` reads `verdict` of its source in its `where`.
        let filter = (name == "b").then_some("verdict = 'x'");
        let v = serde_json::json!({"source": source, "group_by": ["amount"], "measures": [], "where": filter});
        Declaration::Type(TypeDecl {
            name: name.into(),
            fields: vec![
                field("amount", Authority::Authored),
                field("verdict", Authority::Authored),
            ],
            view: Some(serde_json::from_value(v).expect("a view")),
            ..Default::default()
        })
    };
    // A chain of forty views onto the cycle.
    let chain = (0..40).map(|i| view(&format!("c{i}"), &format!("c{}", i + 1)));
    let mut cycle = declared();
    for declaration in chain.chain([view("c40", "a"), view("a", "b"), view("b", "a")]) {
        cycle.push(Declared {
            declaration,
            ..cycle[0].clone()
        });
    }
    let mut reader = who(&[]);
    reader.grants.default = Access::Read;
    // Shown with their definitions: the read that refuses the cycle is reached.
    let d = lake(&"finance".into(), &cycle, &reader);
    assert_eq!(d.types.iter().filter(|t| t.view.is_some()).count(), 43);
    // One kept from what the other groups by is shown neither definition.
    let none = [("amount".into(), Access::None)].into();
    let kept = TypeGrants {
        fields: none,
        ..Default::default()
    };
    reader.grants.types.insert("b".into(), kept);
    let d = lake(&"finance".into(), &cycle, &reader);
    assert_eq!(d.types.iter().filter(|t| t.view.is_some()).count(), 0);
    // One kept from what `b` filters by is shown every view's type and no
    // definition: each view is judged to the chain's end, and no bound on
    // the depth ends the judgement early.
    let none = [("verdict".into(), Access::None)].into();
    let kept = TypeGrants {
        fields: none,
        ..Default::default()
    };
    reader.grants.types = [("a".into(), kept)].into();
    let d = lake(&"finance".into(), &cycle, &reader);
    let views = d.types.iter().filter(|t| t.lineage.view_of.is_some());
    assert_eq!(
        views.map(|t| t.view.is_some()).collect::<Vec<_>>(),
        [false; 43]
    );
}

#[test]
fn a_declaration_is_shown_as_the_one_in_force_and_listed_as_it_is_proposed() {
    let trigger = |when: Option<&str>| {
        Declaration::Trigger(lakelands_api::ontology::TriggerDecl {
            max_depth: None,
            name: "held".into(),
            on: "invoice".into(),
            when: when.map(Into::into),
            description: None,
        })
    };
    let mut d = declared();
    d.push(Declared {
        declaration: trigger(Some("verdict = 'x'")),
        ..d[0].clone()
    });
    let (invoice, salary, settle) = (&d[0].declaration, &d[1].declaration, &d[2].declaration);
    // One who reads `amount` alone, one who reads `verdict` and may not
    // write it: each declaration, shown and listed.
    let rows = [
        (
            who(&[("amount", Access::Read)]),
            [true, false, false, false],
            [true, false, false, false],
        ),
        (
            who(&[("verdict", Access::Read)]),
            [true, false, false, true],
            [true, false, true, true],
        ),
    ];
    for (who, shown, listed) in rows {
        let sight = Sight::held(&who, &d);
        let all = [invoice, salary, settle, &d[3].declaration];
        assert_eq!(all.map(|x| sight.shows(x)), shown, "shown");
        // A verb is listed by the fields it names, not by who may call it.
        assert_eq!(all.map(|x| sight.lists(x)), listed, "listed");
        // The one in force is shown, whatever the one named says; the one
        // proposed is listed by what it says itself.
        assert_eq!(sight.shows(&trigger(None)), shown[3]);
        assert!(sight.lists(&trigger(None)));
    }
}
