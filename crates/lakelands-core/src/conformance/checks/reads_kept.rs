//! A declaration reads only fields its type has
//! (`docs/decisions/028-authoring-is-strict-and-whole.md`, extended), and
//! a migration keeps the fields a declaration in force reads
//! (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
//! extended).

use std::sync::Arc;

use lakelands_api::ontology::StorageKind::{self, Json, Text};
use lakelands_api::ontology::{RelationDecl, TypeDecl};
use lakelands_api::principal::TypeGrants;
use lakelands_api::write::{Declaration, Migration, ProposedWrite};
use lakelands_api::{Access, LakeStore};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, derived, field, field_access, head_unmoved, invalid_naming, migration, reads_all,
    type_decl,
};

check!(
    /// A migration keeps the fields a declaration reads.
    MIGRATION_KEEPS_READ_FIELDS = a_migration_keeps_the_fields_a_declaration_reads,
    "027, extended: a migration that drops a field read by a derived field of its type, by another type's derived relation to it, or by an object set, a trigger or a verb on it (its `requires`, a field it sets, the value it sets one from) is refused as invalid by its preflight, naming the reader and the field, and one that renames such a field names the new name too; so is one that drops a relation's field a trigger on the relation's link type reads, naming the link type; a migration that drops a field no declaration reads goes through; an author who is not shown the reader, as `describe` shows it, is told that a declaration reads the field and not its name, nor a field hidden from it, which its migration does not take away"
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
    // A reader its author is not shown is not named to it: `audit` reads
    // `bonus`, which `partial` may not know of, so `describe` leaves it out;
    // and `bonus` was not `partial`'s to leave out, so it stays (027).
    let invoice = |names: &[&str]| {
        let fields = names.iter().map(|n| field(n, Text));
        type_decl("invoice", fields.collect())
    };
    l.declare(invoice(&["no", "note", "bonus", "team"])).await?;
    let audit = json!({"declare": "trigger", "name": "audit", "on": "invoice",
                       "when": "note = 'x' and bonus = 'y'"});
    l.declare_json(audit).await?;
    let hidden = [("bonus", Access::None)];
    let partial = field_access(store.as_ref(), "partial", "invoice", &hidden);
    let m = migration(invoice(&["no", "team"]));
    let proposed = ProposedWrite::Migrate {
        migration: Box::new(m.clone()),
        report: Default::default(),
    };
    let why = || "no notes".to_owned();
    let refused = l.cat.propose(&partial, proposed.clone(), why()).await;
    let named =
        |e: &lakelands_api::Error| ["audit", "bonus"].iter().any(|n| e.to_string().contains(n));
    let named = refused.as_ref().is_err_and(named);
    let what = "a migration dropping `note`, proposed by one not shown its reader";
    invalid_naming(what, refused, &["a declaration reads `note`"])?;
    ensure!(
        !named,
        "{what}, must name neither `audit` nor the field `bonus`"
    );
    // One who reads every field is shown `audit` and told its name, with
    // the schema grant or without.
    let shown = &["trigger declaration `audit`", "reads `bonus`"];
    let all = reads_all(store.as_ref(), "all");
    let refused = l.cat.propose(&all, proposed, why()).await;
    invalid_naming("the same proposal by one shown its reader", refused, shown)?;
    let refused = l.cat.migrate(&l.own, m).await;
    invalid_naming("the same migration by the owner", refused, shown)?;
    Ok(())
}

check!(
    /// A migration's refusal names a view only to an author shown it.
    MIGRATION_NAMES_A_SHOWN_VIEW = a_migration_names_a_view_only_to_an_author_who_is_shown_it,
    "027, extended: a migration that takes a field a view reads, or changes a kind the view cannot be declared over, is refused naming the view only to an author `describe` shows the view's definition to, in the sentence it had; an author who is not shown the view's type, or is shown it with its definition withheld, is told that a declaration reads the field, or that a declaration over the type could no longer be declared, without the view's name"
);

async fn a_migration_names_a_view_only_to_an_author_who_is_shown_it(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let bill = |team: StorageKind, names: &[&str]| {
        let mut fields: Vec<_> = names.iter().map(|n| field(n, Text)).collect();
        fields.push(field("team", team));
        type_decl("bill", fields)
    };
    l.declare(bill(Text, &["note", "bonus"])).await?;
    let view = json!({"source": "bill", "group_by": ["team"], "where": "note = 'x' and bonus = 'y'",
                      "measures": [{"name": "n", "expr": "count()"}]});
    l.declare_json(
        json!({"declare": "type", "name": "by_team", "key": ["team"], "view": view,
        "fields": [{"name": "team", "type": {"kind": "text"}},
                   {"name": "n", "type": {"kind": "number"}}]}),
    )
    .await?;
    // `blind` has no access to the view's type; `partial` is shown it with
    // its definition withheld, since that reads `bonus`.
    let mut blind = reads_all(store.as_ref(), "blind");
    let none = TypeGrants {
        default: Some(Access::None),
        fields: Default::default(),
    };
    blind.grants.types.insert("by_team".into(), none);
    let partial = field_access(
        store.as_ref(),
        "partial",
        "bill",
        &[("bonus", Access::None)],
    );
    let proposed = |to: TypeDecl| ProposedWrite::Migrate {
        migration: Box::new(migration(to)),
        report: Default::default(),
    };
    let why = || "a change".to_owned();
    let unnamed = |what: &str, refused: lakelands_api::Result<_>, says: &str| {
        let named = |e: &lakelands_api::Error| e.to_string().contains("by_team");
        ensure!(
            !refused.as_ref().is_err_and(named),
            "{what} must not name the view `by_team`; it answered {refused:?}"
        );
        invalid_naming(what, refused, &[says])
    };
    // Each takes away what it knows of: `blind` both fields, `partial` `note`.
    for (who, gone) in [(&blind, "bonus"), (&partial, "note")] {
        let what = format!("a migration taking a field a view reads, by `{}`", who.id);
        let says = format!("a declaration reads `{gone}` of `bill`");
        let refused = l.cat.propose(who, proposed(bill(Text, &[])), why()).await;
        unnamed(&what, refused, &says)?;
    }
    // A kind no view groups by: no field is gone, and no reason is given.
    let retyped = proposed(bill(Json, &["note", "bonus"]));
    unnamed(
        "a migration that changes the kind a view groups by, by `blind`",
        l.cat.propose(&blind, retyped, why()).await,
        "a declaration over `bill` could no longer be declared",
    )?;
    // One who is shown the view is told its name, in the sentence as it was.
    let was = "refused: after the migration of `bill`, type `by_team`: its view reads `bonus` of \
               `bill`, which `bill` does not have";
    let all = reads_all(store.as_ref(), "all");
    let told = l.cat.propose(&all, proposed(bill(Text, &[])), why()).await;
    let said = told.as_ref().err().map(ToString::to_string);
    ensure!(
        said.as_deref() == Some(was),
        "an author shown the view is told, as before, {was:?}; it answered {told:?}"
    );
    Ok(())
}

check!(
    /// A declaration reads only fields its type has.
    DECLARATION_READS_ITS_TYPE = a_declaration_reads_only_fields_its_type_has,
    "028, extended: `declare` and `apply` refuse as invalid, naming the declaration and the field and appending nothing, a derived field, an object set's `where`, a trigger's `when` (`changed` and `previous` included) and a verb's `requires` or the value it sets a field from that read a field the type does not have, and so does a migration's preflight for the type it migrates to; `payload.<name>` in a verb's value, `setting`, `now`, `today` and a guard's `retired` read no field and are accepted"
);

async fn a_declaration_reads_only_fields_its_type_has(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let invoice = |last| type_decl("invoice", vec![field("status", Text), last]);
    l.declare(invoice(field("old", Text))).await?;
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
    // A migration's own declaration is held to it too.
    let loud = invoice(derived("loud", Text, "upper(ghost)", &[]));
    invalid_naming(
        "a migration to a type that reads `ghost`",
        l.cat.migrate(&l.own, migration(loud)).await,
        &["type declaration `invoice`", "reads `ghost`"],
    )?;
    for d in [
        verb("note", "status = setting('mode')", "payload.note"),
        trigger("retired() or changed(status) and previous(status) = 'x'"),
        set("status = 'x' and now() > today()"),
    ] {
        l.declare_json(d).await?;
    }
    Ok(())
}
