use super::*;
use crate::shape::schema_hash;
use lakelands_api::ontology::{Authority, FieldType, StorageKind};
use serde_json::json;

fn base() -> TypeDecl {
    serde_json::from_value(json!({
        "name": "t",
        "key": ["a"],
        "fields": [
            { "name": "a", "type": { "kind": "text" }, "required": true },
            { "name": "b", "type": { "kind": "number" }, "required": true },
            { "name": "c", "type": { "kind": "text" } },
        ],
    }))
    .unwrap()
}

fn field(name: &str, kind: StorageKind, required: bool) -> FieldDecl {
    serde_json::from_value(json!({
        "name": name,
        "type": FieldType::kind(kind),
        "required": required,
    }))
    .unwrap()
}

fn relation(name: &str, target: &str) -> RelationDecl {
    serde_json::from_value(json!({ "name": name, "target": target, "via": "a" })).unwrap()
}

fn with(edit: impl FnOnce(&mut TypeDecl)) -> TypeDecl {
    let mut t = base();
    edit(&mut t);
    t
}

fn declares(after: &TypeDecl) -> Vec<Reason> {
    match classify(&base(), after) {
        Change::Declare { changes } => changes,
        other => panic!("expected a declaration, got {other:?}"),
    }
}

fn migrates(before: &TypeDecl, after: &TypeDecl) -> Vec<Reason> {
    match classify(before, after) {
        Change::Migrate { reasons } => reasons,
        other => panic!("expected a migration, got {other:?}"),
    }
}

#[test]
fn an_identical_revision_is_declared_with_no_changes() {
    assert_eq!(declares(&base()), vec![]);
}

#[test]
fn a_change_of_meaning_is_declared_with_no_changes() {
    let after = with(|t| {
        t.description = Some("a thing".into());
        t.fields[0].label = Some("A".into());
        t.fields[1].authority = Authority::Derived;
        t.fields[2].searchable = true;
        t.fields[2].sensitive = true;
        t.fields.reverse();
    });
    assert_eq!(declares(&after), vec![]);
    assert_eq!(schema_hash(&base()), schema_hash(&after));
}

#[test]
fn adding_a_required_field_needs_a_migration() {
    let after = with(|t| t.fields.push(field("d", StorageKind::DateTime, true)));
    assert_eq!(
        migrates(&base(), &after),
        vec![Reason::FieldAdded {
            field: "d".into(),
            relation: None,
            required: true
        }]
    );
}

#[test]
fn adding_a_relation_is_declared() {
    let after = with(|t| t.relations.push(relation("links", "other")));
    assert_eq!(
        declares(&after),
        vec![Reason::RelationAdded {
            relation: "links".into()
        }]
    );
}

#[test]
fn required_to_optional_is_declared() {
    let after = with(|t| t.fields[1].required = false);
    assert_eq!(
        declares(&after),
        vec![Reason::FieldOptional {
            field: "b".into(),
            relation: None
        }]
    );
}

#[test]
fn every_part_of_a_field_type_is_a_retype() {
    let edits: [fn(&mut FieldType); 4] = [
        |ty| ty.kind = StorageKind::Decimal,
        |ty| ty.value_type = Some("email".into()),
        |ty| ty.target = Some("other".into()),
        |ty| ty.dimensions = Some(4),
    ];
    for edit in edits {
        let after = with(|t| edit(&mut t.fields[2].ty));
        let reasons = migrates(&base(), &after);
        assert!(
            matches!(&reasons[..], [Reason::FieldRetyped { field, .. }] if field.as_str() == "c"),
            "{reasons:?}"
        );
    }
}

#[test]
fn a_key_change_needs_a_migration_and_key_order_counts() {
    let two = with(|t| t.key = vec!["a".into(), "b".into()]);
    let swapped = with(|t| t.key = vec!["b".into(), "a".into()]);
    assert!(matches!(
        &migrates(&base(), &two)[..],
        [Reason::KeyChanged { .. }]
    ));
    assert!(matches!(
        &migrates(&two, &swapped)[..],
        [Reason::KeyChanged { .. }]
    ));
}

#[test]
fn removing_a_relation_needs_a_migration() {
    let before = with(|t| t.relations.push(relation("links", "other")));
    assert_eq!(
        migrates(&before, &base()),
        vec![Reason::RelationRemoved {
            relation: "links".into()
        }]
    );
}

#[test]
fn retargeting_a_relation_needs_a_migration() {
    let before = with(|t| t.relations.push(relation("links", "other")));
    let after = with(|t| t.relations.push(relation("links", "elsewhere")));
    assert!(matches!(
        &migrates(&before, &after)[..],
        [Reason::RelationRetargeted { .. }]
    ));
}

#[test]
fn a_widening_cardinality_is_declared_a_narrowing_one_migrated() {
    let at = |c: Cardinality| {
        with(|t| {
            let mut r = relation("links", "other");
            r.cardinality = c;
            t.relations.push(r);
        })
    };
    use Cardinality::*;
    for (from, to) in [
        (OneToOne, ManyToOne),
        (OneToOne, OneToMany),
        (ManyToOne, ManyToMany),
    ] {
        assert!(
            !classify(&at(from), &at(to)).needs_migration(),
            "{from:?} -> {to:?}"
        );
    }
    for (from, to) in [
        (ManyToMany, ManyToOne),
        (ManyToOne, OneToOne),
        (OneToMany, ManyToOne),
    ] {
        assert!(
            classify(&at(from), &at(to)).needs_migration(),
            "{from:?} -> {to:?}"
        );
    }
}

#[test]
fn link_fields_follow_the_field_rules() {
    let before = with(|t| t.relations.push(relation("links", "other")));
    let add = |required: bool| {
        let mut t = before.clone();
        t.relations[0]
            .fields
            .push(field("since", StorageKind::DateTime, required));
        t
    };
    assert!(!classify(&before, &add(false)).needs_migration());
    assert_eq!(
        migrates(&before, &add(true)),
        vec![Reason::FieldAdded {
            field: "since".into(),
            relation: Some("links".into()),
            required: true
        }]
    );
    assert_eq!(
        migrates(&add(false), &before),
        vec![Reason::FieldRemoved {
            field: "since".into(),
            relation: Some("links".into())
        }]
    );
}

#[test]
fn a_rename_reads_as_a_removal_and_an_addition() {
    let after = with(|t| t.fields[2].name = "c2".into());
    assert_eq!(
        migrates(&base(), &after),
        vec![Reason::FieldRemoved {
            field: "c".into(),
            relation: None
        }]
    );
}

#[test]
fn a_migration_lists_every_reshaping_reason_and_only_those() {
    let after = with(|t| {
        t.fields.retain(|f| f.name.as_str() != "c");
        t.fields[1].ty = FieldType::kind(StorageKind::Decimal);
        t.fields.push(field("d", StorageKind::Text, false));
        t.key = vec!["b".into()];
    });
    let reasons = migrates(&base(), &after);
    assert_eq!(reasons.len(), 3, "{reasons:?}");
    assert!(reasons.iter().all(reshapes));
}

fn view(filter: &str) -> TypeDecl {
    serde_json::from_value(json!({
            "name": "v",
            "view": { "source": "t", "where": filter, "measures": [{ "name": "n", "expr": "count()" }] },
        }))
        .unwrap()
}

#[test]
fn a_view_stores_nothing_so_its_changes_are_declared() {
    assert_eq!(
        classify(&view("b > 0"), &view("b > 1")),
        Change::Declare {
            changes: vec![Reason::ViewChanged]
        }
    );
    // Text and typed forms of the same view are the same view.
    let typed = {
        let mut v = view("b > 0");
        v.view.as_mut().unwrap().r#where = Some(crate::expr::parse("b > 0").unwrap().into());
        v
    };
    assert_eq!(
        classify(&view("b > 0"), &typed),
        Change::Declare { changes: vec![] }
    );
}

#[test]
fn a_stored_type_turned_into_a_view_needs_a_migration() {
    let v = with(|t| t.view = view("b > 0").view);
    assert!(migrates(&base(), &v).contains(&Reason::BecameView));
    assert!(!classify(&v, &base()).needs_migration());
}

#[test]
fn an_unchanged_hash_is_always_declared_without_changes() {
    let revisions = [
        with(|t| t.fields.reverse()),
        with(|t| t.fields[0].description = Some("x".into())),
        with(|t| t.relations.push(relation("r", "o"))),
        with(|t| t.fields[1].required = false),
    ];
    for after in revisions {
        let same = schema_hash(&base()) == schema_hash(&after);
        let empty = classify(&base(), &after) == Change::Declare { changes: vec![] };
        assert_eq!(same, empty, "{after:?}");
    }
}

#[test]
fn reasons_read_as_sentences() {
    let r = Reason::FieldRetyped {
        field: "b".into(),
        relation: None,
        from: FieldType::kind(StorageKind::Number),
        to: FieldType::kind(StorageKind::Decimal),
    };
    assert_eq!(
        r.to_string(),
        "field `b` changes type from number to decimal"
    );
}
