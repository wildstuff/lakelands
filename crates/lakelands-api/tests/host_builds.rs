//! A host's code, seen from outside the crate: it builds the declarations
//! and descriptions with `..Default::default()` or the constructors, and
//! reads descriptions with `..`, so a member added to any of them in a
//! later release does not break it (lakelands#260). If one of these
//! stops compiling, a release breaks every host that builds this way.

use lakelands_api::describe::{FieldDescription, LakeDescription, TypeDescription, TypeLifecycle};
use lakelands_api::ontology::{FieldDecl, FieldType, RelationDecl, StorageKind, TypeDecl};
use lakelands_api::query::SearchMode;
use lakelands_api::{FieldName, TypeName};

#[test]
fn a_host_builds_every_declaration_naming_only_what_it_sets() {
    let invoice = TypeDecl {
        fields: vec![
            FieldDecl {
                searchable: true,
                ..FieldDecl::of("note", StorageKind::Text)
            },
            FieldDecl {
                required: true,
                ..FieldDecl::of("amount", StorageKind::Decimal)
            },
        ],
        relations: vec![RelationDecl::to("partner", "partner")],
        ..TypeDecl::named("invoice")
    };
    assert_eq!(invoice.name, TypeName::from("invoice"));
    assert_eq!(invoice.fields[1].ty, FieldType::kind(StorageKind::Decimal));
    assert_eq!(invoice.relations[0].target, TypeName::from("partner"));

    let bare = TypeDecl {
        name: "memo".into(),
        ..Default::default()
    };
    let field = FieldDecl {
        name: "text".into(),
        ..Default::default()
    };
    let relation = RelationDecl {
        name: "about".into(),
        target: "memo".into(),
        ..Default::default()
    };
    assert!(bare.fields.is_empty() && bare.annotations.is_empty());
    assert_eq!(
        field.ty.kind,
        StorageKind::Text,
        "a field's kind defaults to text"
    );
    assert_eq!(relation.name.as_str(), "about");
}

#[test]
fn a_host_builds_and_reads_every_description_naming_only_what_it_uses() {
    let field = FieldDescription {
        name: "amount".into(),
        ty: FieldType::kind(StorageKind::Decimal),
        ..Default::default()
    };
    let ty = TypeDescription {
        name: "invoice".into(),
        fields: vec![field],
        lifecycle: TypeLifecycle {
            version: 3,
            ..Default::default()
        },
        ..Default::default()
    };
    let lake = LakeDescription {
        lake: "finance".into(),
        types: vec![ty],
        ..Default::default()
    };

    let LakeDescription { types, .. } = &lake;
    let TypeDescription {
        name,
        fields,
        lifecycle: TypeLifecycle { version, .. },
        ..
    } = &types[0];
    let FieldDescription { name: field, .. } = &fields[0];
    assert_eq!((name.as_str(), *version), ("invoice", 3));
    assert_eq!(field, &FieldName::from("amount"));
}

#[test]
fn a_host_builds_every_search_mode_through_its_constructor() {
    for (mode, kind) in [
        (SearchMode::lexical("harbour"), "lexical"),
        (SearchMode::vector("v", vec![1.0, 0.0]), "vector"),
        (SearchMode::hybrid("harbour", "v"), "hybrid"),
    ] {
        let named = match &mode {
            SearchMode::Lexical { .. } => "lexical",
            SearchMode::Vector { .. } => "vector",
            SearchMode::Hybrid { .. } => "hybrid",
        };
        assert_eq!(named, kind);
    }
    // A mode built before the weights serialises as it did, so a cursor
    // handed out for it still continues it; the new members read by name.
    let plain = serde_json::to_value(SearchMode::vector("v", vec![1.0])).unwrap();
    let wire = serde_json::json!({"mode": "vector", "field": "v", "vector": [1.0]});
    assert_eq!(plain, wire);
    let wire = serde_json::json!({"mode": "vector", "field": "v", "vector": [1.0],
        "weight": 2.0, "also": [{"field": "w", "vector": [0.5]}]});
    let mode: SearchMode = serde_json::from_value(wire).unwrap();
    let fields: Vec<_> = mode
        .vectors()
        .into_iter()
        .map(|v| (v.0.as_str(), v.2))
        .collect();
    assert_eq!(fields, [("v", Some(2.0)), ("w", None)]);
    assert_eq!(mode.misfit(), None);
}
