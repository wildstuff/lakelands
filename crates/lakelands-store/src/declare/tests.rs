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

/// A type of one field and `relations`, each carried by link records.
fn linked(name: &str, relations: &[(&str, &str)]) -> Declaration {
    let rs = relations
        .iter()
        .map(|(r, to)| lakelands_api::ontology::RelationDecl::to(*r, *to));
    Declaration::Type(TypeDecl {
        fields: vec![FieldDecl::of("a", StorageKind::Text)],
        relations: rs.collect(),
        ..TypeDecl::named(name)
    })
}

#[test]
fn a_declarations_tool_names_are_judged_against_the_lakes_alone_as_the_whole_judges_them() {
    let mut lake = Ontology::default().named();
    for d in [
        linked("a", &[("b-c", "x")]),
        linked("x", &[]),
        linked("a_b", &[]),
    ] {
        crate::writes::put_as_appended(&mut lake, &d, Some);
    }
    // Clear where, and only where, the whole lake with it has no problem.
    let judged = |o: &Ontology, d: &Declaration, problem: bool, what: &str| {
        let clear = o.tool_names().is_some_and(|n| n.clear(o, d));
        let whole = tool_problems(o, d);
        assert_eq!(
            (clear, !whole.is_empty()),
            (!problem, problem),
            "{what}: {whole:?}"
        );
    };
    judged(
        &lake,
        &linked("invoice", &[("to", "x")]),
        false,
        "a new type",
    );
    judged(
        &lake,
        &linked("A-b", &[]),
        true,
        "a name that folds onto a type's",
    );
    judged(
        &lake,
        &linked("a", &[("b_c", "x")]),
        true,
        "a link type that folds onto the one kept",
    );
    judged(
        &lake,
        &linked("x", &[("door", "x")]),
        false,
        "a relation named like a door",
    );
    judged(
        &lake,
        &linked(&"l".repeat(70), &[]),
        true,
        "a name too long",
    );
    judged(
        &lake,
        &linked("a_b", &[("c", "x")]),
        true,
        "a link that folds onto another's",
    );
    // A type its author is not shown is no name to it.
    let mut seen = lake.clone();
    seen.types.remove(&TypeName::from("a_b"));
    judged(&seen, &linked("A-b", &[]), false, "a type not shown");
    // In a set of three, two put before the third is judged.
    let mut set = lake.clone();
    for d in [linked("m", &[]), linked("n_o", &[("to", "x")])] {
        crate::writes::put_as_appended(&mut set, &d, Some);
    }
    judged(
        &set,
        &linked("p", &[("to", "m")]),
        false,
        "the third of a set of three",
    );
    judged(
        &set,
        &linked("N-o", &[]),
        true,
        "the third folding onto the second",
    );
    // A lake that holds a problem tells it to every declaration.
    let mut held = lake.clone();
    held.put(linked("A_B", &[]), 1, None);
    judged(
        &held,
        &linked("invoice", &[]),
        true,
        "a problem the lake holds",
    );
    // What a type made gives way to what it makes, as a lake made whole.
    let mut moved = lake.clone();
    crate::writes::put_as_appended(&mut moved, &linked("a", &[("d", "x")]), Some);
    judged(
        &moved,
        &linked("a_b", &[("c", "x")]),
        true,
        "a link type kept after its relation goes",
    );
    let whole = Ontology::from_declared(&moved.declared()).named();
    assert_eq!(
        moved.tool_names(),
        whole.tool_names(),
        "the names kept are the lake's"
    );
}
