use lakelands_api::error::Case;
use lakelands_api::ontology::{FieldDecl, StorageKind, TypeDecl};
use lakelands_api::write::Declaration;

use super::*;

/// A type of `fields`, all text.
fn ty(fields: &[&str]) -> TypeDecl {
    let fields = fields.iter().map(|f| FieldDecl::of(*f, StorageKind::Text));
    TypeDecl {
        fields: fields.collect(),
        ..TypeDecl::named("t")
    }
}

fn kept(o: &Ontology, fields: &[&str]) -> bool {
    let d = Declaration::Type(ty(fields));
    let e = check_declaration(o, &d, false).err();
    matches!(
        e.as_ref().and_then(Error::case).map(|s| &s.case),
        Some(Case::FieldNameKept { .. })
    )
}

#[test]
fn a_new_field_that_begins_with_an_underscore_is_refused_and_one_the_type_holds_is_kept() {
    // A lake written before the rule, whose type holds `_old`.
    let mut o = Ontology::default();
    o.put(Declaration::Type(ty(&["a", "_old"])), 1, None);
    assert!(
        !kept(&o, &["a", "_old", "b"]),
        "a field the type holds keeps its name"
    );
    assert!(kept(&o, &["a", "_old", "_new"]), "a new one is refused");
    assert!(
        kept(&Ontology::default(), &["_x"]),
        "on a type that is new too"
    );
}
