//! A declaration reads only fields its type has
//! (`docs/decisions/028-authoring-is-strict-and-whole.md`, extended), and
//! a migration keeps the fields a declaration in force reads
//! (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
//! extended).

use std::sync::Arc;

use lakelands_api::error::Case;
use lakelands_api::ontology::StorageKind::{self, Json, Text};
use lakelands_api::ontology::{RelationDecl, TypeDecl};
use lakelands_api::principal::TypeGrants;
use lakelands_api::write::{Declaration, Migration, ProposedWrite};
use lakelands_api::{Access, LakeStore};
use serde_json::{Value, json};

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, derived, field, field_access, head_unmoved, migration, reads_all, refused_like,
    said_like, type_decl,
};

/// A place a case is said at, on the wire: a declaration by its kind
/// (`type`, `object_set`, `trigger`, `verb`) and its name.
fn at(kind: &str, name: &str) -> Value {
    json!([{"code": kind, "name": name}])
}

/// Whether an error names `name` anywhere: in a parameter, a place or its
/// sentence, on the wire.
fn names(e: &lakelands_api::Error, name: &str) -> bool {
    serde_json::to_string(e).is_ok_and(|w| w.contains(name))
}

check!(
    /// A migration keeps the fields a declaration reads.
    MIGRATION_KEEPS_READ_FIELDS = a_migration_keeps_the_fields_a_declaration_reads,
    "027, extended: a migration that drops a field read by a derived field of its type, by another type's derived relation to it, or by an object set, a trigger or a verb on it (its `requires`, a field it sets, the value it sets one from, a field named `payload` read bare too) is refused as invalid by its preflight, naming the reader and the field, and one that renames such a field names the new name too; so is one that drops a relation's field a trigger on the relation's link type reads, naming the link type; a migration that drops a field no declaration reads goes through; an author who is not shown the reader, as `describe` shows it, is told that a declaration reads the field and not its name, nor a field hidden from it, which its migration does not take away"
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
    let all = ["a", "b", "c", "d", "e", "f", "g", "payload", "free"];
    l.declare(item(&all)).await?;
    for d in [
        json!({"declare": "object_set", "name": "s", "of": "item", "where": "b = 'x'"}),
        json!({"declare": "trigger", "name": "t", "on": "item", "when": "changed(c)"}),
        json!({"declare": "verb", "name": "v", "on": "item", "risk": "low", "requires": "f = 'x'",
               "effect": "write", "set": [{"field": "d", "value": "e"}]}),
        json!({"declare": "verb", "name": "w", "on": "item", "risk": "low",
               "effect": "write", "set": [{"field": "d", "value": "payload"}]}),
        json!({"declare": "type", "name": "other", "fields": [{"name": "k", "type": {"kind": "text"}}],
               "relations": [{"name": "its", "target": "item",
                              "derived": {"match": [{"from": "k", "to": "g"}]}}]}),
    ] {
        l.declare_json(d).await?;
    }
    let readers = [
        ("a", "type", "item"),
        ("b", "object_set", "s"),
        ("c", "trigger", "t"),
        ("d", "verb", "v"),
        ("e", "verb", "v"),
        ("f", "verb", "v"),
        ("g", "type", "other"),
        ("payload", "verb", "w"),
    ];
    for (gone, kind, reader) in readers {
        let kept: Vec<&str> = all.iter().copied().filter(|n| *n != gone).collect();
        let preflight = l.cat.migrate(&l.own, migration(item(&kept))).await;
        let what = format!("a migration dropping `{gone}`");
        let case = json!({"code": "migration_takes_read", "unnamed": false, "field": gone,
                          "type": "item", "at": at(kind, reader)});
        refused_like(&what, preflight, &case)?;
    }
    let renamed = Migration {
        to: item(&["a", "b2", "c", "d", "e", "f", "g", "payload", "free"]),
        renames: vec![("b".into(), "b2".into())],
        relation_renames: Vec::new(),
    };
    let case = json!({"code": "migration_takes_read", "field": "b", "renamed": "b2",
                      "at": at("object_set", "s")});
    let renamed = l.cat.migrate(&l.own, renamed).await;
    refused_like("a migration renaming `b`", renamed, &case)?;
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
    let case = json!({"code": "migration_takes_read", "field": "ok",
                      "type": "sys.link.doc.signed_by", "at": at("trigger", "lt")});
    refused_like(
        "a migration dropping the relation's field `ok`",
        l.cat.migrate(&l.own, migration(doc(vec![]))).await,
        &case,
    )?;
    l.migrate(item(&all[..8])).await?;
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
    let named = |e: &lakelands_api::Error| ["audit", "bonus"].iter().any(|n| names(e, n));
    let named = refused.as_ref().is_err_and(named);
    let what = "a migration dropping `note`, proposed by one not shown its reader";
    refused_as!(
        what,
        refused,
        Case::MigrationTakesRead { unnamed: true, field, ty, renamed: None }
            if field == "note" && ty == "invoice"
    )?;
    ensure!(
        !named,
        "{what}, must name neither `audit` nor the field `bonus`"
    );
    // One who reads every field is shown `audit` and told its name, with
    // the schema grant or without.
    let shown = json!({"code": "migration_takes_read", "unnamed": false, "field": "bonus",
                       "at": at("trigger", "audit")});
    let all = reads_all(store.as_ref(), "all");
    let refused = l.cat.propose(&all, proposed, why()).await;
    refused_like("the same proposal by one shown its reader", refused, &shown)?;
    let refused = l.cat.migrate(&l.own, m).await;
    refused_like("the same migration by the owner", refused, &shown)?;
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
        ..Default::default()
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
    let unnamed = |what: &str, refused: lakelands_api::Result<_>, says: Value| {
        ensure!(
            !refused.as_ref().is_err_and(|e| names(e, "by_team")),
            "{what} must not name the view `by_team`; it answered {refused:?}"
        );
        refused_like(what, refused, &says)
    };
    // Each takes away what it knows of: `blind` both fields, `partial` `note`.
    for (who, gone) in [(&blind, "bonus"), (&partial, "note")] {
        let what = format!("a migration taking a field a view reads, by `{}`", who.id);
        let says = json!({"code": "migration_takes_read", "unnamed": true, "field": gone,
                          "type": "bill"});
        let refused = l.cat.propose(who, proposed(bill(Text, &[])), why()).await;
        unnamed(&what, refused, says)?;
    }
    // A kind no view groups by: no field is gone, and no reason is given.
    let retyped = proposed(bill(Json, &["note", "bonus"]));
    unnamed(
        "a migration that changes the kind a view groups by, by `blind`",
        l.cat.propose(&blind, retyped, why()).await,
        json!({"code": "no_longer_declarable", "type": "bill"}),
    )?;
    // One who is shown the view is told its name, as the owner is told it.
    let at =
        json!([{"code": "after_migration", "name": "bill"}, {"code": "type", "name": "by_team"}]);
    let was = json!({"code": "unknown_field", "type": "bill", "field": "bonus", "at": at});
    let all = reads_all(store.as_ref(), "all");
    let told = l.cat.propose(&all, proposed(bill(Text, &[])), why()).await;
    let said = told.as_ref().err().and_then(|e| e.case());
    ensure!(
        said.is_some_and(|s| s.at.len() == 2 && said_like(s, &was)),
        "an author shown the view is told, as the owner is, {was}; it answered {told:?}"
    );
    Ok(())
}

check!(
    /// A declaration reads only fields its type has.
    DECLARATION_READS_ITS_TYPE = a_declaration_reads_only_fields_its_type_has,
    "028, extended: `declare` and `apply` refuse as invalid, naming the declaration and the field and appending nothing, a derived field, an object set's `where`, a trigger's `when` (`changed` and `previous` included) and a verb's `requires` or the value it sets a field from that read a field the type does not have, and so does a migration's preflight for the type it migrates to; in a verb's value only `payload.<name>` is the call's and reads no field, so a bare `payload`, a longer path and one inside a function read the record's field of that name and are refused on a type without it and taken on one that has it, and a member the verb's payload does not have is refused, naming it (lakelands#470); `setting`, `now`, `today` and a guard's `retired` read no field and are accepted"
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
        (("type", "memo"), memo),
        (("verb", "close"), verb("close", "ghost = 'x'", "'shut'")),
        (("verb", "copy"), verb("copy", "status = 'x'", "ghost")),
        (("trigger", "t"), trigger("ghost = 'x'")),
        (("trigger", "t"), trigger("changed(ghost)")),
        (("trigger", "t"), trigger("previous(ghost) = 'x'")),
        (("object_set", "s"), set("ghost = 'x'")),
    ] {
        let what = format!("declaring {d}");
        let d: Declaration = serde_json::from_value(d)
            .map_err(|e| Failure::new(format!("the check's declaration does not read: {e}")))?;
        let (kind, name) = reader;
        let case = json!({"code": "unknown_field", "field": "ghost", "at": at(kind, name)});
        refused_like(&what, l.cat.declare(&l.own, d.clone()).await, &case)?;
        refused_like(&what, l.cat.apply(&l.own, vec![d]).await, &case)?;
    }
    // In a verb's value only `payload.<name>` is the call's.
    let reads = json!({"code": "unknown_field", "field": "payload", "at": at("verb", "p")});
    let ghost = json!({"code": "payload_no_member", "member": "ghost", "at": at("verb", "p")});
    for (value, case) in [
        ("payload", &reads),
        ("payload.note.deep", &reads),
        ("concat(payload, '!')", &reads),
        ("payload.ghost", &ghost),
    ] {
        let d = verb("p", "status = 'x'", value);
        let d: Declaration = serde_json::from_value(d).expect("a verb");
        let what = format!("declaring a verb whose value is `{value}`");
        refused_like(&what, l.cat.declare(&l.own, d.clone()).await, case)?;
        refused_like(&what, l.cat.apply(&l.own, vec![d]).await, case)?;
    }
    head_unmoved(
        store.as_ref(),
        before,
        "a declaration reading a missing field",
    )
    .await?;
    // A migration's own declaration is held to it too.
    let loud = invoice(derived("loud", Text, "upper(ghost)", &[]));
    refused_like(
        "a migration to a type that reads `ghost`",
        l.cat.migrate(&l.own, migration(loud)).await,
        &json!({"code": "unknown_field", "field": "ghost", "at": at("type", "invoice")}),
    )?;
    // On a type that has a field `payload`, the bare name reads it.
    let fields = ["status", "old", "payload"].map(|n| field(n, Text));
    l.declare(type_decl("invoice", fields.into())).await?;
    for d in [
        verb("note", "status = setting('mode')", "payload.note"),
        verb("own", "status = 'x'", "concat(payload, payload.note)"),
        trigger("retired() or changed(status) and previous(status) = 'x'"),
        set("status = 'x' and now() > today()"),
    ] {
        l.declare_json(d).await?;
    }
    Ok(())
}
