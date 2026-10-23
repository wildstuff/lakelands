//! A declaration reads only fields its type has
//! (`docs/decisions/028-authoring-is-strict-and-whole.md`, extended), and
//! a migration keeps the fields a declaration in force reads
//! (`docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
//! extended).

use lakelands_api::ontology::StorageKind::{Json, Text};
use lakelands_api::write::Outcome;

use crate::conformance::fixture::{derived, said_like};
use crate::conformance::prelude::*;

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
    MIGRATION_KEEPS_READ_FIELDS = a_migration_keeps_the_fields_a_declaration_reads,
    "027, extended: a migration that drops a field read by a derived field of its type, by another type's derived relation to it, or by an object set, a trigger or a verb on it (its `requires`, a field it sets, the value it sets one from, a field named `payload` read bare too) is refused as invalid by its preflight, naming the reader and the field, and one that renames such a field names the new name too; so is one that drops a relation's field a trigger on the relation's link type reads, naming the link type; a migration that drops a field no declaration reads goes through; an author who is not shown the reader, as `describe` shows it, is told that a declaration reads the field and not its name, nor a field hidden from it, which its migration does not take away"
);

/// `item` with the text fields `names` and `loud`, derived from `a`.
fn item(names: &[&str]) -> TypeDecl {
    let mut fields: Vec<_> = names.iter().map(|n| field(n, Text)).collect();
    fields.push(derived("loud", Text, "upper(a)", &[]));
    type_decl("item", fields)
}

async fn a_migration_keeps_the_fields_a_declaration_reads(store: Store) -> Held {
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
        renames: vec![("b".into(), "b2".into())],
        ..Migration::to(item(&[
            "a", "b2", "c", "d", "e", "f", "g", "payload", "free",
        ]))
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
    RELATION_KEPT_FOR_A_VERB = a_relation_stays_while_a_verb_is_declared_on_its_link_type,
    "027 and 035, extended: a migration that removes a relation carried by link records is refused as invalid while a verb in force is declared on the relation's link type, by the preflight, by the apply and by the accept of a removal proposed before the verb, with the head unmoved, while one that keeps the relation is applied, naming a verb to an author `describe` shows it to, one who may call it, whichever of several that is, and telling another only that a declaration is declared there, with no name and no verb's place: one who writes nothing at a proposal, and one kept from the link type at the preflight, the apply and a proposal, while as a reviewer, listed the removal in part, it is refused the accept as not granted (lakelands#867); the verb's call is still answered; once every such verb is declared again on another type the removal is applied, a `link` sent again under its key finds no relation, and a verb declared on the link type it leaves is refused as on a type that is not declared"
);

async fn a_relation_stays_while_a_verb_is_declared_on_its_link_type(store: Store) -> Held {
    const LT: &str = "sys.link.doc.signed_by";
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_decl("signer", vec![field("name", Text)]))
        .await?;
    let doc = |fields: &[&str], relations: &[&str]| {
        let mut t = type_decl("doc", fields.iter().map(|n| field(n, Text)).collect());
        let signed = |r: &&str| RelationDecl {
            fields: vec![field("ok", Text), field("seen", Text)],
            ..RelationDecl::to(*r, "signer")
        };
        t.relations = relations.iter().map(signed).collect();
        t
    };
    l.declare(doc(&["n", "spare"], &["signed_by"])).await?;
    let why = || "no signers".to_owned();
    let removing = |fields: &[&str]| ProposedWrite::Migrate {
        migration: Box::new(migration(doc(fields, &[]))),
        report: Default::default(),
    };
    // Proposed while no verb stands on the link type, accepted after.
    let early = cat.propose(own, removing(&["n", "spare"]), why()).await;
    let Outcome::Proposed { proposal, .. } = early.must("proposing the removal early")? else {
        return Err(Failure::new("a proposal answers `proposed`"));
    };
    let verb = |name: &str, on: &str, sets: &str| {
        let set = json!([{"field": sets, "value": "'x'"}]);
        json!({"declare": "verb", "name": name, "on": on, "risk": "low", "effect": "write",
               "set": set})
    };
    // Two verbs on it: `approve`, first by name, sets a field `writer` may not.
    l.declare_json(verb("audit", LT, "seen")).await?;
    l.declare_json(verb("approve", LT, "ok")).await?;
    l.upsert("signer", "s1", json!({"name": "s"})).await?;
    l.upsert("doc", "d1", json!({"n": "1"})).await?;
    let (ty, from, rel, to) = ("doc".into(), "d1".into(), "signed_by".into(), "s1".into());
    let keyed = || lakelands_api::write::WriteOpts {
        idempotency_key: Some("first".into()),
        ..Default::default()
    };
    let linked = cat.link(own, &ty, &from, &rel, &to, Default::default(), keyed());
    linked.await.must("linking d1 to s1")?;
    let named = |verb: &str| {
        json!({"code": "migration_takes_link_type", "unnamed": false, "relation": "signed_by",
               "type": LT, "at": at("verb", verb)})
    };
    let first = named("approve");
    // One kept from the link type is shown `doc` with `signed_by` and
    // neither the link type nor a verb. It is told that a declaration
    // stands on the link type its relation names, and not which: at every
    // door, beside the owner, who is named the verb.
    let grants = json!({"default": "write", "schema": true, "review": true, "restricted": true,
        "types": {LT: {"default": "none"}}});
    let grants = serde_json::from_value(grants).expect("grants read");
    let kept = principal(store.as_ref(), PrincipalKind::Agent, "kept", grants);
    fn untold<T: std::fmt::Debug>(what: &str, a: lakelands_api::Result<T>) -> Held {
        let unnamed = json!({"code": "migration_takes_link_type", "unnamed": true,
            "relation": "signed_by", "type": LT});
        let said = ["approve", "audit", "verb"];
        let told = |e: &lakelands_api::Error| said.iter().any(|n| names(e, n));
        let told = a.as_ref().is_err_and(told);
        let wire = a.as_ref().err().map(|e| json!(e).to_string());
        refused_like(what, a, &unnamed)?;
        ensure!(
            !told,
            "{what}, must name no verb and place it at none; it said {wire:?}"
        );
        Ok(())
    }
    let before = store.head().await.must("reading the head")?;
    let accepted = cat.accept(own, &proposal).await;
    refused_like("the accept of the removal proposed early", accepted, &first)?;
    let accepted = cat.accept(&kept, &proposal).await.map(|_| ());
    let not_granted = Err(lakelands_api::Error::Refused {
        field: None,
        reason: lakelands_api::Refusal::NotGranted,
    });
    ensure!(
        accepted == not_granted,
        "the accept by a reviewer kept from the link type, listed the removal in part, must be \
         refused as not granted, naming nothing; it answered {accepted:?}"
    );
    head_unmoved(store.as_ref(), before, "a refused accept of the removal").await?;
    // A migration that keeps the relation is not the verb's to refuse.
    l.migrate(doc(&["n"], &["signed_by"])).await?;
    let before = store.head().await.must("reading the head")?;
    let (removal, proposed) = (migration(doc(&["n"], &[])), removing(&["n"]));
    let preflight = cat.migrate(own, removal.clone()).await;
    refused_like("the preflight of the removal", preflight, &first)?;
    let applied = cat.migrate_apply(own, removal.clone(), "none").await;
    refused_like("the apply of the removal", applied, &first)?;
    let preflight = cat.migrate(&kept, removal.clone()).await;
    untold("the preflight by one kept from the link type", preflight)?;
    let applied = cat.migrate_apply(&kept, removal, "none").await;
    untold("the apply by one kept from the link type", applied)?;
    let refused = cat.propose(&kept, proposed.clone(), why()).await;
    untold(
        "the removal proposed by one kept from the link type",
        refused,
    )?;
    // `describe` shows a verb to one who may call it: an author who writes
    // `seen` is told the verb it is shown, one who writes nothing only that
    // a declaration is declared there.
    let writer = field_writer(store.as_ref(), "writer", LT, &["seen"]);
    let refused = cat.propose(&writer, proposed.clone(), why()).await;
    let what = "the removal proposed by one who may call `audit` alone";
    refused_like(what, refused, &named("audit"))?;
    let refused = cat
        .propose(&reads_all(store.as_ref(), "reader"), proposed, why())
        .await;
    let told = |e: &lakelands_api::Error| ["approve", "audit"].iter().any(|n| names(e, n));
    let told = refused.as_ref().is_err_and(told);
    let what = "the removal proposed by one not shown the verb";
    refused_as!(
        what,
        refused,
        Case::MigrationTakesLinkType { unnamed: true, relation, ty }
            if relation == "signed_by" && ty == LT
    )?;
    ensure!(!told, "{what}, must name neither verb");
    head_unmoved(store.as_ref(), before, "a refused removal of a relation").await?;
    let key = crate::links::link_key(&from, &to);
    let approve = "approve".into();
    let called = cat.call(
        own,
        &approve,
        Some(&key),
        Default::default(),
        Default::default(),
    );
    called
        .await
        .must("calling `approve` on the link, its relation kept")?;
    // Declared again on another type, the verbs name the link type no more.
    l.declare_json(verb("audit", "doc", "n")).await?;
    l.declare_json(verb("approve", "doc", "n")).await?;
    l.migrate(doc(&["n"], &[])).await?;
    // The link's own retry does not outlive its relation (035).
    let again = cat.link(own, &ty, &from, &rel, &to, Default::default(), keyed());
    let gone = json!({"code": "no_link_relation"});
    refused_like("the `link` sent again under its key", again.await, &gone)?;
    let late: Declaration = serde_json::from_value(verb("late", LT, "ok"))
        .map_err(|e| Failure::new(format!("the check's verb does not read: {e}")))?;
    let late = cat.declare(own, late).await;
    let undeclared = json!({"code": "undeclared", "type": LT});
    refused_like(
        "a verb on the link type the removal left",
        late,
        &undeclared,
    )?;
    Ok(())
}

check!(
    RELATION_KEPT_FOR_A_TARGET = a_relation_stays_while_another_relation_targets_its_link_type,
    "027 and 035, extended (lakelands#920): a migration that removes a relation carried by link records is refused as invalid while a relation or a reference field that stands after it refers to the relation's link type, a relation of another type or of its own, or a field of a type or of another relation's link records, by the preflight, by the apply, by a proposal and by the accept of a removal proposed before that relation, with the head unmoved, while one that keeps the relation is applied; the relation or the field that refers is named with its type to an author `describe` shows it to, whichever of several that is, and another, kept from its type or from the link type, is told only that a declaration refers to the link type; a migration that removes the relation and the one of its own type that targets it together is applied once nothing else refers, lists both under `dropped` and retires the links of both"
);

async fn a_relation_stays_while_another_relation_targets_its_link_type(store: Store) -> Held {
    use crate::conformance::fixture::get_at;
    const LT: &str = "sys.link.doc.signed_by";
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_decl("signer", vec![field("name", Text)]))
        .await?;
    // `doc` with `signed_by`, to a signer, and `cites`, to a link of it.
    let relations = |names: &[&str]| {
        let to = |r: &&str| RelationDecl::to(*r, if *r == "signed_by" { "signer" } else { LT });
        names.iter().map(to).collect::<Vec<_>>()
    };
    let doc = |names: &[&str]| {
        let mut t = type_decl("doc", vec![field("n", Text)]);
        t.relations = relations(names);
        t
    };
    l.declare(doc(&["signed_by"])).await?;
    let why = || "no signers".to_owned();
    let removing = |names: &[&str]| ProposedWrite::Migrate {
        migration: Box::new(migration(doc(names))),
        report: Default::default(),
    };
    // Proposed while no relation targets the link type, accepted after.
    let early = cat.propose(own, removing(&[]), why()).await;
    let Outcome::Proposed { proposal, .. } = early.must("proposing the removal early")? else {
        return Err(Failure::new("a proposal answers `proposed`"));
    };
    let minute = |names: &[&str]| {
        let mut t = type_decl("minute", vec![field("t", Text)]);
        t.relations = relations(names);
        t
    };
    l.declare(minute(&["recorded_in"])).await?;
    l.upsert("signer", "s1", json!({"name": "s"})).await?;
    l.upsert("doc", "d1", json!({"n": "1"})).await?;
    l.upsert("minute", "m1", json!({"t": "1"})).await?;
    let signed = crate::links::link_key(&"d1".into(), &"s1".into());
    let link = |ty: &'static str, from: &'static str, rel: &'static str, to: &str| {
        let (ty, from, rel, to) = (ty.into(), from.into(), rel.into(), to.into());
        let (cat, own) = (cat.clone(), own.clone());
        async move {
            let linked = cat.link(
                &own,
                &ty,
                &from,
                &rel,
                &to,
                Default::default(),
                Default::default(),
            );
            linked.await.must("linking").map(drop)
        }
    };
    link("doc", "d1", "signed_by", "s1").await?;
    link("minute", "m1", "recorded_in", signed.as_str()).await?;
    let named = |ty: &str, relation: &str| {
        json!({"code": "migration_takes_target", "unnamed": false, "relation": "signed_by",
               "type": LT,
               "at": [{"code": "type", "name": ty}, {"code": "relation", "name": relation}]})
    };
    let first = named("minute", "recorded_in");
    // `kept` is not shown `minute`; `apart` is shown it without the
    // relation, whose target it is kept from. Each is told that a
    // relation targets the link type, and not which, at every door.
    let schema = |name: &str, none: &str| {
        let grants = json!({"default": "write", "schema": true, "review": true, "restricted": true,
            "types": {none: {"default": "none"}}});
        let grants = serde_json::from_value(grants).expect("grants read");
        principal(store.as_ref(), PrincipalKind::Agent, name, grants)
    };
    let (kept, apart) = (schema("kept", "minute"), schema("apart", LT));
    fn untold<T: std::fmt::Debug>(what: &str, a: lakelands_api::Result<T>) -> Held {
        let unnamed = json!({"code": "migration_takes_target", "unnamed": true,
            "relation": "signed_by", "type": LT});
        let said = ["minute", "recorded_in", "cites", "memo", "about"];
        let told = |e: &lakelands_api::Error| said.iter().any(|n| names(e, n));
        let told = a.as_ref().is_err_and(told);
        let wire = a.as_ref().err().map(|e| json!(e).to_string());
        refused_like(what, a, &unnamed)?;
        ensure!(
            !told,
            "{what}, must name no relation and place it at none; it said {wire:?}"
        );
        Ok(())
    }
    let before = store.head().await.must("reading the head")?;
    let accepted = cat.accept(own, &proposal).await;
    refused_like("the accept of the removal proposed early", accepted, &first)?;
    let what = "the accept by a reviewer kept from the type that targets the link type";
    untold(what, cat.accept(&kept, &proposal).await)?;
    let removal = migration(doc(&[]));
    let preflight = cat.migrate(own, removal.clone()).await;
    refused_like("the preflight of the removal", preflight, &first)?;
    let applied = cat.migrate_apply(own, removal.clone(), "none").await;
    refused_like("the apply of the removal", applied, &first)?;
    let proposed = cat.propose(own, removing(&[]), why()).await;
    refused_like("the removal proposed", proposed, &first)?;
    for who in [&kept, &apart] {
        let id = who.id.as_str();
        let preflight = cat.migrate(who, removal.clone()).await;
        untold(&format!("the preflight by `{id}`"), preflight)?;
        let applied = cat.migrate_apply(who, removal.clone(), "none").await;
        untold(&format!("the apply by `{id}`"), applied)?;
        let proposed = cat.propose(who, removing(&[]), why()).await;
        untold(&format!("the removal proposed by `{id}`"), proposed)?;
    }
    head_unmoved(store.as_ref(), before, "a refused removal of a relation").await?;
    // A migration that keeps the relation is not refused: `cites`, of
    // `doc` itself, targets the link type too, and is the one named now,
    // to `kept` as well, who is shown it.
    l.migrate(doc(&["signed_by", "cites"])).await?;
    link("doc", "d1", "cites", signed.as_str()).await?;
    let own_type = named("doc", "cites");
    let removal = migration(doc(&["cites"]));
    for who in [own, &kept] {
        let preflight = cat.migrate(who, removal.clone()).await;
        let what = format!("the removal that keeps `cites`, by `{}`", who.id.as_str());
        refused_like(&what, preflight, &own_type)?;
    }
    // Removed together with `cites`, `minute`'s relation still refuses it.
    let both = migration(doc(&[]));
    let preflight = cat.migrate(own, both.clone()).await;
    refused_like(
        "the removal of both while `minute` targets",
        preflight,
        &first,
    )?;
    l.migrate(minute(&[])).await?;
    // A reference field into the link type refuses it as a relation does:
    // named to the owner, unnamed to one kept from the link type.
    let memo = |fields: Vec<_>| type_decl("memo", fields);
    let about = crate::conformance::fixture::reference("about", LT);
    l.declare(memo(vec![field("t", Text), about])).await?;
    let at_field = json!({"code": "migration_takes_target", "unnamed": false,
        "relation": "signed_by", "type": LT,
        "at": [{"code": "type", "name": "memo"}, {"code": "field", "name": "about"}]});
    let preflight = cat.migrate(own, both.clone()).await;
    refused_like(
        "the removal while `memo.about` refers",
        preflight,
        &at_field,
    )?;
    let preflight = cat.migrate(&apart, both.clone()).await;
    untold("the same removal, by `apart`", preflight)?;
    l.migrate(memo(vec![field("t", Text)])).await?;
    // So does one on another relation's link records: `minute`'s `seen`,
    // whose link to `s1` names the link `d1:s1` in `pref`.
    let pref = crate::conformance::fixture::reference("pref", LT);
    let seen = |fields: Vec<_>| {
        let mut t = minute(&[]);
        t.relations = vec![RelationDecl {
            fields,
            ..RelationDecl::to("seen", "signer")
        }];
        t
    };
    l.migrate(seen(vec![pref])).await?;
    let (ty, from, rel, to) = ("minute".into(), "m1".into(), "seen".into(), "s1".into());
    let pref = crate::conformance::fixture::patch(json!({"pref": signed.as_str()}));
    let linked = cat.link(own, &ty, &from, &rel, &to, pref, Default::default());
    linked
        .await
        .must("linking m1 to s1, naming the link d1:s1")?;
    let at_link_field = json!({"code": "migration_takes_target", "unnamed": false,
        "relation": "signed_by", "type": LT,
        "at": [{"code": "type", "name": "sys.link.minute.seen"},
               {"code": "field", "name": "pref"}]});
    let preflight = cat.migrate(own, both.clone()).await;
    refused_like(
        "the removal while a link field of `seen` refers",
        preflight,
        &at_link_field,
    )?;
    l.migrate(seen(vec![])).await?;
    let preflight = cat.migrate(own, removal).await;
    refused_like("the removal that keeps `cites`", preflight, &own_type)?;
    let report = cat.migrate(own, both.clone()).await;
    let report = report.must("the preflight of removing both")?.data;
    let dropped = report.dropped;
    cat.migrate_apply(own, both, &report.report_id)
        .await
        .must("removing `signed_by` and `cites` together")?;
    let cited = crate::links::link_key(&"d1".into(), &signed);
    let lt = |r: &str| crate::links::link_type_name(&"doc".into(), &r.into());
    let gone = |r: &'static str, key: &lakelands_api::RecordKey| {
        let (cat, own, get) = (cat.clone(), own.clone(), get_at(key.as_str(), None));
        async move { cat.get(&own, &lt(r), get).await.is_err() }
    };
    let retired = gone("signed_by", &signed).await && gone("cites", &cited).await;
    ensure!(
        dropped == [("signed_by".into(), 1), ("cites".into(), 1)] && retired,
        "with no other relation targeting `{LT}`, the migration that removes `signed_by` and \
         `cites` together lists each under `dropped` with its one link and retires both; it \
         listed {dropped:?}, and both are gone: {retired}"
    );
    Ok(())
}

check!(
    MIGRATION_NAMES_A_SHOWN_VIEW = a_migration_names_a_view_only_to_an_author_who_is_shown_it,
    "027, extended: a migration that takes a field a view reads, or changes a kind the view cannot be declared over, is refused naming the view only to an author `describe` shows the view's definition to, in the sentence it had; an author who is not shown the view's type, or is shown it with its definition withheld, is told that a declaration reads the field, or that a declaration over the type could no longer be declared, without the view's name"
);

async fn a_migration_names_a_view_only_to_an_author_who_is_shown_it(store: Store) -> Held {
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
    DECLARATION_READS_ITS_TYPE = a_declaration_reads_only_fields_its_type_has,
    "028, extended: `declare` and `apply` refuse as invalid, naming the declaration and the field and appending nothing, a derived field, an object set's `where`, a trigger's `when` (`changed` and `previous` included) and a verb's `requires` or the value it sets a field from that read a field the type does not have, and so does a migration's preflight for the type it migrates to; in a verb's value only `payload.<name>` is the call's and reads no field, so a bare `payload`, a longer path and one inside a function read the record's field of that name and are refused on a type without it and taken on one that has it, and a member the verb's payload does not have is refused, naming it (lakelands#470); `setting`, `now`, `today` and a guard's `retired` read no field and are accepted"
);

async fn a_declaration_reads_only_fields_its_type_has(store: Store) -> Held {
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
        set("status = 'x' and upper(old) = 'Y'"),
    ] {
        l.declare_json(d).await?;
    }
    Ok(())
}
