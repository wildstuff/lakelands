//! A migration keeps the fields a declaration in force reads
//! (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
//! extended).

use std::sync::Arc;

use lakelands_api::LakeStore;
use lakelands_api::ontology::StorageKind::Text;
use lakelands_api::ontology::TypeDecl;
use lakelands_api::write::Migration;
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{Lake, derived, field, invalid_naming, migration, type_decl};

check!(
    /// A migration keeps the fields a declaration reads.
    MIGRATION_KEEPS_READ_FIELDS = a_migration_keeps_the_fields_a_declaration_reads,
    "027, extended: a migration that drops a field read by a derived field of its type, by another type's derived relation to it, or by an object set, a trigger or a verb on it (its `requires`, a field it sets, the value it sets one from) is refused as invalid by its preflight, naming the reader and the field, and one that renames such a field names the new name too; a migration that drops a field no declaration reads goes through"
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
    l.migrate(item(&all[..7])).await?;
    Ok(())
}
