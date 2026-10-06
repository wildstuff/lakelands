//! A declaration carries the host's annotations, which Lakelands stores,
//! replays and describes and never reads
//! (`docs/decisions/007-schema-hash-covers-shape.md`,
//! `docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md` and
//! `docs/decisions/027-shape-changes-are-declared-or-migrated.md`, each
//! extended).
//!
//! An annotation is meaning, never shape: changing it alone is a new
//! version of the declaration with the same schema hash. A field the
//! principal may not read is described without its annotations, in a type
//! and in a relation's link fields alike. Keys carry a namespace, two are
//! reserved, and one declaration holds a bounded amount.

use std::collections::BTreeMap;
use std::sync::Arc;

use lakelands_api::describe::TypeDescription;
use lakelands_api::event::EventBody;
use lakelands_api::ontology::{
    ANNOTATIONS_MAX_BYTES, Annotations, RelationDecl, StorageKind, TypeDecl,
};
use lakelands_api::write::Declaration;
use lakelands_api::{Access, LakeStore, Position, ReadAt, Refusal, TypeName};
use serde_json::{Value, json};

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, events_from, field, field_access, head_unmoved, reason, type_decl,
};

check!(
    /// Annotations come back as declared.
    ROUND_TRIP = annotations_round_trip_through_declare_the_feed_and_describe,
    "007, 024 and 027, extended: annotations on a type, a field, a relation and a relation's link field, declared or applied, come back exactly in the feed's `declared` event and in `describe` for the owner, in canonical form"
);

check!(
    /// An annotation change alone moves the version, not the hash.
    VERSION_NOT_HASH = an_annotation_change_alone_moves_the_version_and_not_the_schema_hash,
    "007 and 027, extended: re-declaring a type with only an annotation changed is taken by `declare`, moves its version by one and leaves its schema hash byte-identical"
);

check!(
    /// A masked field is described without its annotations.
    MASKED_WITHOUT = a_masked_field_is_described_without_its_annotations,
    "024, extended: a field the principal may not read is described by its shape alone, without its annotations, and so is a relation's link field it may not read (no description, label or annotations); the type's and the relation's own annotations are still shown"
);

check!(
    /// A bad annotation is refused, naming it.
    REFUSED_NAMING = an_unnamespaced_reserved_null_or_oversized_annotation_is_refused_naming_it,
    "the contract's annotation rules: a key without a namespace, a `null` value and a declaration holding more than 16 KiB of annotations are refused as invalid, a key in `sys.` or `lakelands.` as reserved, each naming the key or the declaration, with nothing appended; a declaration holding exactly 16 KiB is taken"
);

fn marks(pairs: &[(&str, Value)]) -> Annotations {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

fn room() -> TypeDecl {
    type_decl("room", vec![field("code", StorageKind::Text)])
}

/// A shelf annotated on the type, a field, a relation and its link field.
fn shelf() -> TypeDecl {
    let mut code = field("code", StorageKind::Text);
    code.annotations = marks(&[("acme.format", json!("shelf-code"))]);
    let mut secret = field("secret", StorageKind::Text);
    secret.sensitive = true;
    secret.annotations = marks(&[("acme.policy", json!({"export": false}))]);
    let mut note = field("note", StorageKind::Text);
    note.description = Some("why the shelf is in the room".into());
    note.label = Some("Note".into());
    note.annotations = marks(&[("acme.hint", json!("free text"))]);
    let mut t = type_decl("shelf", vec![code, secret]);
    t.annotations = marks(&[
        ("acme.icon", json!("books")),
        (
            "acme.package",
            json!({"name": "library", "version": "2.1.0", "weight": 1.5}),
        ),
    ]);
    t.relations = vec![RelationDecl {
        derived: None,
        annotations: marks(&[("acme.plural", json!("rooms"))]),
        name: "room".into(),
        target: "room".into(),
        fields: vec![note],
        ..Default::default()
    }];
    t
}

/// The annotation maps of a type as declared: its own, each field's, each
/// relation's and each link field's, by where they sit.
fn declared_maps(t: &TypeDecl) -> BTreeMap<String, Annotations> {
    Declaration::Type(t.clone())
        .annotation_maps()
        .into_iter()
        .map(|(at, m)| (at, m.clone()))
        .collect()
}

/// The same maps as `describe` answers them.
fn described_maps(d: &TypeDescription, fields: &[&str]) -> BTreeMap<String, Annotations> {
    let mut out = BTreeMap::new();
    out.insert(String::new(), d.annotations.clone());
    for (i, name) in fields.iter().enumerate() {
        let f = d.fields.iter().find(|f| f.name.as_str() == *name);
        out.insert(
            format!("fields[{i}]"),
            f.map(|f| f.annotations.clone()).unwrap_or_default(),
        );
    }
    for (i, r) in d.relations.iter().enumerate() {
        out.insert(format!("relations[{i}]"), r.annotations.clone());
        for (j, f) in r.fields.iter().enumerate() {
            out.insert(format!("relations[{i}].fields[{j}]"), f.annotations.clone());
        }
    }
    out
}

/// The canonical form of every map, as the log writes it.
fn canonical(maps: &BTreeMap<String, Annotations>) -> BTreeMap<String, Value> {
    maps.iter()
        .map(|(at, m)| {
            let bytes = lakelands_api::canonical::of(m);
            (
                at.clone(),
                serde_json::from_slice(&bytes).expect("canonical bytes are JSON"),
            )
        })
        .collect()
}

async fn declared_event(
    store: &dyn LakeStore,
    after: Position,
    name: &str,
) -> Result<TypeDecl, Failure> {
    let events = events_from(store, after).await?;
    events
        .into_iter()
        .rev()
        .find_map(|e| match e.body {
            EventBody::Declared {
                declaration: Declaration::Type(t),
                ..
            } if t.name.as_str() == name => Some(t),
            _ => None,
        })
        .ok_or_else(|| Failure::new(format!("the feed holds no `declared` event of `{name}`")))
}

async fn annotations_round_trip_through_declare_the_feed_and_describe(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let before = store.head().await.must("reading the head")?;
    l.declare(room()).await?;
    let shelf = shelf();
    l.declare(shelf.clone()).await?;
    let want = canonical(&declared_maps(&shelf));
    let logged = declared_event(store.as_ref(), before, "shelf").await?;
    let got = canonical(&declared_maps(&logged));
    ensure!(
        got == want,
        "the feed's `declared` event of `shelf` must carry its annotations as declared; it \
         carried {got:?}, declared {want:?}"
    );
    let d = l.describe_type("shelf").await?;
    let got = canonical(&described_maps(&d, &["code", "secret"]));
    ensure!(
        got == want,
        "`describe` must answer the owner every annotation of `shelf` as declared; it \
         answered {got:?}, declared {want:?}"
    );
    // Applied as a set, as a bundle is: the same.
    let mut crate_ = shelf.clone();
    crate_.name = "crate".into();
    crate_.annotations = marks(&[("acme.icon", json!("box"))]);
    cat.apply(own, vec![Declaration::Type(crate_.clone())])
        .await
        .must("applying the annotated `crate`")?;
    let d = l.describe_type("crate").await?;
    let (want, got) = (
        canonical(&declared_maps(&crate_)),
        canonical(&described_maps(&d, &["code", "secret"])),
    );
    ensure!(
        got == want,
        "`describe` must answer the owner every annotation of the applied `crate`; it \
         answered {got:?}, applied {want:?}"
    );
    Ok(())
}

async fn an_annotation_change_alone_moves_the_version_and_not_the_schema_hash(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    l.declare(room()).await?;
    let mut shelf = shelf();
    l.declare(shelf.clone()).await?;
    let lifecycle = |d: TypeDescription| d.lifecycle;
    let first = lifecycle(l.describe_type("shelf").await?);
    shelf
        .annotations
        .insert("acme.icon".into(), json!("bookcase"));
    shelf.fields[0]
        .annotations
        .insert("acme.width".into(), json!(80));
    l.declare(shelf).await?;
    let second = lifecycle(l.describe_type("shelf").await?);
    ensure!(
        second.version == first.version + 1,
        "an annotation change must move the version by one; it went from {} to {}",
        first.version,
        second.version
    );
    ensure!(
        second.schema_hash == first.schema_hash,
        "an annotation change must leave the schema hash byte-identical; it went from {} to {}",
        first.schema_hash,
        second.schema_hash
    );
    Ok(())
}

async fn a_masked_field_is_described_without_its_annotations(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, .. } = &l;
    l.declare(room()).await?;
    let shelf = shelf();
    l.declare(shelf.clone()).await?;
    // Reads everything but `secret` and the link field `note`, which it
    // sees masked.
    let mut clerk = field_access(
        store.as_ref(),
        "clerk",
        "shelf",
        &[("secret", Access::Masked)],
    );
    let note = lakelands_api::principal::TypeGrants {
        default: None,
        fields: [("note".into(), Access::Masked)].into(),
        ..Default::default()
    };
    clerk
        .grants
        .types
        .insert("sys.link.shelf.room".into(), note);
    let d = cat
        .describe_type(&clerk, &TypeName::from("shelf"), ReadAt::default())
        .await
        .must("describing `shelf` as the clerk")?
        .data;
    let secret = d
        .fields
        .iter()
        .find(|f| f.name.as_str() == "secret")
        .ok_or_else(|| Failure::new("the clerk must see `secret`'s shape"))?;
    ensure!(
        secret.access == Access::Masked && secret.annotations.is_empty(),
        "the masked `secret` must be described without its annotations; it was described \
         with access {:?} and {:?}",
        secret.access,
        secret.annotations
    );
    let code = d.fields.iter().find(|f| f.name.as_str() == "code");
    ensure!(
        code.is_some_and(|f| f.annotations == shelf.fields[0].annotations),
        "the readable `code` must keep its annotations; it was described as {code:?}"
    );
    ensure!(
        d.annotations == shelf.annotations,
        "the type's annotations must be shown to whoever it is described to; the clerk got \
         {:?}",
        d.annotations
    );
    let relation = d
        .relations
        .iter()
        .find(|r| r.name.as_str() == "room")
        .ok_or_else(|| Failure::new("the clerk must see the relation `room`"))?;
    ensure!(
        relation.annotations == shelf.relations[0].annotations,
        "the relation's annotations must be shown; the clerk got {:?}",
        relation.annotations
    );
    let note = relation
        .fields
        .iter()
        .find(|f| f.name.as_str() == "note")
        .ok_or_else(|| Failure::new("the relation must still name its link field `note`"))?;
    ensure!(
        note.annotations.is_empty() && note.description.is_none() && note.label.is_none(),
        "the link field `note` the clerk may not read must be described by its shape alone; \
         it carried annotations {:?}, description {:?} and label {:?}",
        note.annotations,
        note.description,
        note.label
    );
    Ok(())
}

async fn an_unnamespaced_reserved_null_or_oversized_annotation_is_refused_naming_it(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    // `{"acme.blob":""}` is 16 bytes; the string fills the rest.
    let blob = |n: usize| marks(&[("acme.blob", json!("x".repeat(n - 16)))]);
    let cases: Vec<(&str, Annotations, bool, &str)> = vec![
        (
            "an unnamespaced key",
            marks(&[("icon", json!("x"))]),
            false,
            "`icon`",
        ),
        (
            "a `sys.` key",
            marks(&[("sys.icon", json!("x"))]),
            true,
            "sys.icon",
        ),
        (
            "a `lakelands.` key",
            marks(&[("lakelands.icon", json!("x"))]),
            true,
            "lakelands.icon",
        ),
        (
            "a `null` value",
            marks(&[("acme.icon", Value::Null)]),
            false,
            "`acme.icon`",
        ),
        (
            "a declaration past the ceiling",
            blob(ANNOTATIONS_MAX_BYTES + 1),
            false,
            "`tag`",
        ),
    ];
    for (what, annotations, reserved, named) in cases {
        let mut t = type_decl("tag", vec![field("code", StorageKind::Text)]);
        t.fields[0].annotations = annotations;
        let before = store.head().await.must("reading the head")?;
        let e = match cat.declare(own, Declaration::Type(t)).await {
            Ok(_) => {
                return Err(Failure::new(format!(
                    "{what} must be refused; it was declared"
                )));
            }
            Err(e) => e,
        };
        let kind_ok = match reason(&e) {
            Some(Refusal::Reserved) => reserved,
            Some(Refusal::Invalid { .. }) => !reserved,
            _ => false,
        };
        ensure!(
            kind_ok,
            "{what} must be refused as {}; it was refused as {e:?}",
            if reserved { "reserved" } else { "invalid" }
        );
        let why = e.to_string();
        ensure!(
            why.contains(named),
            "{what} must be refused naming {named}; it was refused with: {why}"
        );
        head_unmoved(store.as_ref(), before, what).await?;
    }
    let mut t = type_decl("tag", vec![field("code", StorageKind::Text)]);
    t.fields[0].annotations = blob(ANNOTATIONS_MAX_BYTES);
    l.declare(t).await?;
    Ok(())
}
