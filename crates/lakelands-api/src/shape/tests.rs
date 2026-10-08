use super::*;
use crate::ontology::{Authority, Cardinality, FieldType, OnInputChange, StorageKind};

/// A small type written the way a bundle would declare it.
fn invoice() -> TypeDecl {
    serde_json::from_value(json!({
        "name": "invoice",
        "key": ["number"],
        "fields": [
            { "name": "number", "type": { "kind": "text" }, "required": true },
            { "name": "amount", "type": { "kind": "decimal", "value_type": "money" } },
            { "name": "customer", "type": { "kind": "reference", "target": "customer" } },
        ],
        "relations": [
            { "name": "billed_to", "target": "customer", "via": "customer" },
        ],
    }))
    .unwrap()
}

fn hash(ty: &TypeDecl) -> String {
    schema_hash(ty).unwrap()
}

fn with(edit: impl FnOnce(&mut TypeDecl)) -> TypeDecl {
    let mut ty = invoice();
    edit(&mut ty);
    ty
}

fn a_view(filter: &str, total: &str) -> TypeDecl {
    serde_json::from_value(json!({
        "name": "revenue",
        "view": {
            "source": "invoice",
            "where": Condition::from(filter),
            "group_by": ["customer"],
            "measures": [{ "name": "total", "expr": Condition::from(total) }],
        },
    }))
    .unwrap()
}

/// Pinned once, and checked against an independent SHA-256 of the
/// bytes in the next test: a change here is a change of the log format.
#[test]
fn the_hash_is_pinned_for_a_small_type() {
    assert_eq!(
        hash(&invoice()),
        "ce7fc1daa67c9853e3054fae92e9e188aa08039cc0ab8e460e3d73d9edd038d5"
    );
}

/// A view's conditions are hashed in their typed form, so the serde
/// form of `Expr` is part of the log format; pinned for that reason.
#[test]
fn the_hash_is_pinned_for_a_view() {
    assert_eq!(
        hash(&a_view("status = 'open'", "sum(amount)")),
        "39bc4bc1ad9646574531399af2cc2e01245a57989f4e61280ec94c5c2ec3918b"
    );
}

#[test]
fn the_hashed_object_is_exactly_the_specified_one() {
    let bytes = canonical::to_vec(&shape(&invoice()).unwrap());
    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        concat!(
            r#"{"fields":["#,
            r#"{"name":"amount","required":false,"type":{"kind":"decimal","value_type":"money"}},"#,
            r#"{"name":"customer","required":false,"type":{"kind":"reference","target":"customer"}},"#,
            r#"{"name":"number","required":true,"type":{"kind":"text"}}],"#,
            r#""key":["number"],"#,
            r#""relations":[{"cardinality":"many_to_one","fields":[],"name":"billed_to","target":"customer"}],"#,
            r#""view":null}"#,
        )
    );
}

#[test]
fn all_four_members_are_always_present() {
    let empty: TypeDecl = serde_json::from_value(json!({ "name": "empty" })).unwrap();
    assert_eq!(
        canonical::to_vec(&shape(&empty).unwrap()),
        br#"{"fields":[],"key":[],"relations":[],"view":null}"#
    );
}

#[test]
fn meaning_only_changes_leave_the_hash_alone() {
    let bare = hash(&invoice());
    let marked = with(|t| {
        t.description = Some("a bill we sent".into());
        t.label = Some("Invoice".into());
        t.valid_from = Some("issued".into());
        t.valid_to = Some("settled".into());
        t.field_meta = false;
        t.language = Some("de-CH".into());
        let f = &mut t.fields[1];
        f.description = Some("what is owed".into());
        f.label = Some("Amount".into());
        f.sensitive = true;
        f.searchable = true;
        f.authority = Authority::Source;
        f.derived = Some("1 + 1".into());
        f.depends_on = vec!["number".into()];
        f.on_input_change = OnInputChange::Clear;
        let r = &mut t.relations[0];
        r.via = None;
        r.inverse = Some("invoices".into());
        r.description = Some("who pays".into());
    });
    assert_eq!(bare, hash(&marked));
}

#[test]
fn every_shape_change_changes_the_hash() {
    let bare = hash(&invoice());
    let changes: Vec<(&str, TypeDecl)> = vec![
        (
            "a field added",
            with(|t| {
                let mut f = t.fields[0].clone();
                f.name = "note".into();
                f.required = false;
                t.fields.push(f);
            }),
        ),
        ("a field removed", with(|t| t.fields.truncate(1))),
        (
            "a field made required",
            with(|t| t.fields[1].required = true),
        ),
        (
            "a storage kind changed",
            with(|t| t.fields[0].ty = FieldType::kind(StorageKind::Number)),
        ),
        (
            "a value type changed",
            with(|t| t.fields[1].ty.value_type = Some("price".into())),
        ),
        (
            "a reference target changed",
            with(|t| t.fields[2].ty.target = Some("partner".into())),
        ),
        (
            "a vector length set",
            with(|t| t.fields[1].ty.dimensions = Some(3)),
        ),
        ("the key changed", with(|t| t.key = vec!["customer".into()])),
        ("the key emptied", with(|t| t.key.clear())),
        (
            "a relation retargeted",
            with(|t| t.relations[0].target = "partner".into()),
        ),
        (
            "a cardinality changed",
            with(|t| t.relations[0].cardinality = Cardinality::ManyToMany),
        ),
        (
            "a link field added",
            with(|t| {
                let f = t.fields[1].clone();
                t.relations[0].fields.push(f);
            }),
        ),
        ("a relation removed", with(|t| t.relations.clear())),
    ];
    for (what, ty) in changes {
        assert_ne!(bare, hash(&ty), "{what} must change the hash");
    }
}

#[test]
fn field_and_relation_order_is_not_shape() {
    let reordered = with(|t| {
        t.fields.reverse();
        let mut other = t.relations[0].clone();
        other.name = "also_billed_to".into();
        t.relations.push(other);
    });
    let other_order = with(|t| {
        let mut other = t.relations[0].clone();
        other.name = "also_billed_to".into();
        t.relations.insert(0, other);
    });
    assert_eq!(hash(&reordered), hash(&other_order));

    let links = |reverse: bool| {
        with(|t| {
            let mut fs = vec![t.fields[0].clone(), t.fields[1].clone()];
            if reverse {
                fs.reverse();
            }
            t.relations[0].fields = fs;
        })
    };
    assert_eq!(hash(&links(false)), hash(&links(true)));
}

#[test]
fn key_order_is_shape() {
    let ab = with(|t| t.key = vec!["number".into(), "customer".into()]);
    let ba = with(|t| t.key = vec!["customer".into(), "number".into()]);
    assert_ne!(hash(&ab), hash(&ba));
}

#[test]
fn a_view_change_is_a_shape_change() {
    let bare = hash(&a_view("amount > 0", "sum(amount)"));
    assert_ne!(bare, hash(&a_view("amount > 1", "sum(amount)")));
    assert_ne!(bare, hash(&a_view("amount > 0", "count()")));
    let plain = with(|_| {});
    assert_ne!(hash(&plain), bare);
}

#[test]
fn a_view_that_does_not_parse_has_no_hash() {
    let broken = a_view("amount >", "sum(amount)");
    match schema_hash(&broken) {
        Err(ViewConditionError { at, .. }) => assert_eq!(at, [Place::Where]),
        other => panic!("expected a condition error, got {other:?}"),
    }
    let broken = a_view("amount > 0", "sum(");
    match schema_hash(&broken) {
        Err(ViewConditionError { at, .. }) => {
            let name = "total".into();
            assert_eq!(at, [Place::Measure { name }]);
        }
        other => panic!("expected a condition error, got {other:?}"),
    }
}
