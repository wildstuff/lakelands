//! A declaration reads only fields its type has
//! (`docs/decisions/028-authoring-is-strict-and-whole.md`, extended), and
//! a migration keeps the fields a declaration in force reads
//! (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
//! extended).

use std::sync::Arc;

use lakelands_api::LakeStore;
use lakelands_api::ontology::StorageKind::Text;
use lakelands_api::ontology::{RelationDecl, TypeDecl};
use lakelands_api::write::{Declaration, Migration};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, derived, field, head_unmoved, invalid_naming, migration, type_decl,
};

check!(
    /// A migration keeps the fields a declaration reads.
    MIGRATION_KEEPS_READ_FIELDS = a_migration_keeps_the_fields_a_declaration_reads,
    "027, extended: a migration that drops a field read by a derived field of its type, by another type's derived relation to it, or by an object set, a trigger or a verb on it (its `requires`, a field it sets, the value it sets one from) is refused as invalid by its preflight, naming the reader and the field, and one that renames such a field names the new name too; so is one that drops a relation's field a trigger on the relation's link type reads, naming the link type; a migration that drops a field no declaration reads goes through"
);

/// `item` with the text fields `names` and `loud`, derived from `a`.
fn item(names: &[&str]) -> TypeDecl {
    let mut fields: Vec<_> = names.iter().map(|n| field(n, Text)).collect();
    fields.push(derived("loud", Text, "upper(a)", &[]));
    type_decl("item", fields)
}

async fn a_migration_keeps_the_fields_a_declaration_reads(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let all = ["a", "b", "c", "d", "e", "f", "g", "free"];
    l.declare(item(&all)).await?;
    for d in [
        json!({"declare": "object_set", "name": "s", "of": "item", "where": "b = 'x'"}),
        json!({"declare": "trigger", "name": "t", "on": "item", "when": "changed(c)"}),
        json!({"declare": "verb", "name": "v", "on": "item", "risk": "low", "requires": "f = 'x'",
               "effect": "write", "set": [{"field": "d", "value": "e"}]}),
        json!({"declare": "type", "name": "other", "fields": [{"name": "k", "type": {"kind": "text"}}],
               "relations": [{"name": "its", "target": "item",
                              "derived": {"match": [{"from": "k", "to": "g"}]}}]}),
    ] {
        l.declare_json(d).await?;
    }
    let readers = [
        ("a", "type declaration `item`"),
        ("b", "object_set declaration `s`"),
        ("c", "trigger declaration `t`"),
        ("d", "verb declaration `v`"),
        ("e", "verb declaration `v`"),
        ("f", "verb declaration `v`"),
        ("g", "type declaration `other`"),
    ];
    for (gone, reader) in readers {
        let kept: Vec<&str> = all.iter().copied().filter(|n| *n != gone).collect();
        let preflight = l.cat.migrate(&l.own, migration(item(&kept))).await;
        let what = format!("a migration dropping `{gone}`");
        invalid_naming(&what, preflight, &[reader, &format!("reads `{gone}`")])?;
    }
    let renamed = Migration {
        to: item(&["a", "b2", "c", "d", "e", "f", "g", "free"]),
        renames: vec![("b".into(), "b2".into())],
    };
    invalid_naming(
        "a migration renaming `b`",
        l.cat.migrate(&l.own, renamed).await,
        &["object_set declaration `s`", "reads `b`", "renames to `b2`"],
    )?;
    // A relation's fields are its link type's: a trigger on the link type
    // keeps the one it reads.
    let doc = |fields| {
        let mut t = type_decl("doc", vec![field("n", Text)]);
        t.relations = vec![RelationDecl {
            fields,
            ..RelationDecl::to("signed_by", "item")
        }];
        t
    };
    l.declare(doc(vec![field("ok", Text)])).await?;
    let on_links = json!({"declare": "trigger", "name": "lt", "on": "sys.link.doc.signed_by",
                          "when": "ok = 'x'"});
    l.declare_json(on_links).await?;
    invalid_naming(
        "a migration dropping the relation's field `ok`",
        l.cat.migrate(&l.own, migration(doc(vec![]))).await,
        &[
            "trigger declaration `lt`",
            "reads `ok` of `sys.link.doc.signed_by`",
        ],
    )?;
    l.migrate(item(&all[..7])).await?;
    Ok(())
}

check!(
    /// A declaration reads only fields its type has.
    DECLARATION_READS_ITS_TYPE = a_declaration_reads_only_fields_its_type_has,
    "028, extended: `declare` and `apply` refuse as invalid, naming the declaration and the field and appending nothing, a derived field, an object set's `where`, a trigger's `when` (`changed` and `previous` included) and a verb's `requires` or the value it sets a field from that read a field the type does not have; `payload.<name>` in a verb's value, `setting`, `now`, `today` and a guard's `retired` read no field and are accepted"
);

async fn a_declaration_reads_only_fields_its_type_has(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("invoice", vec![field("status", Text)]))
        .await?;
    let verb = |name: &str, requires: &str, value: &str| {
        json!({"declare": "verb", "name": name, "on": "invoice", "risk": "low",
               "payload": [{"name": "note", "type": {"kind": "text"}}], "requires": requires,
               "effect": "write", "set": [{"field": "status", "value": value}]})
    };
    let trigger =
        |when: &str| json!({"declare": "trigger", "name": "t", "on": "invoice", "when": when});
    let set = |filter: &str| json!({"declare": "object_set", "name": "s", "of": "invoice", "where": filter});
    let memo = json!({"declare": "type", "name": "memo", "fields": [
        {"name": "loud", "type": {"kind": "text"}, "authority": "derived", "derived": "upper(ghost)"}]});
    let before = store.head().await.must("reading the head")?;
    for (reader, d) in [
        ("type declaration `memo`", memo),
        (
            "verb declaration `close`",
            verb("close", "ghost = 'x'", "'shut'"),
        ),
        (
            "verb declaration `copy`",
            verb("copy", "status = 'x'", "ghost"),
        ),
        ("trigger declaration `t`", trigger("ghost = 'x'")),
        ("trigger declaration `t`", trigger("changed(ghost)")),
        ("trigger declaration `t`", trigger("previous(ghost) = 'x'")),
        ("object_set declaration `s`", set("ghost = 'x'")),
    ] {
        let what = format!("declaring {d}");
        let d: Declaration = serde_json::from_value(d)
            .map_err(|e| Failure::new(format!("the check's declaration does not read: {e}")))?;
        let names = [reader, "reads `ghost`"];
        invalid_naming(&what, l.cat.declare(&l.own, d.clone()).await, &names)?;
        invalid_naming(&what, l.cat.apply(&l.own, vec![d]).await, &names)?;
    }
    head_unmoved(
        store.as_ref(),
        before,
        "a declaration reading a missing field",
    )
    .await?;
    for d in [
        verb("note", "status = setting('mode')", "payload.note"),
        trigger("retired() or changed(status) and previous(status) = 'x'"),
        set("status = 'x' and now() > today()"),
    ] {
        l.declare_json(d).await?;
    }
    Ok(())
}
