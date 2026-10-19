//! A type whose records only its declared verbs retire (lakelands#1148):
//! marked `retired_by: verbs`, its records are retired by a verb whose
//! effect is `retire`, under its `requires` and its risk, and `retire`, a
//! proposed `retire` and its accept are refused. A purge still retires
//! them, as the system. A refusal names the retire verbs its writer is
//! shown and may call, and none it is not, whatever the key.

use lakelands_api::error::Case;
use lakelands_api::event::EventBody;
use lakelands_api::ontology::RetiredBy;
use lakelands_api::write::{ProposedWrite, WriteOpts};
use lakelands_api::{Error, Outcome, Principal, Refusal};
use serde_json::{Value, json};

use super::only::holder;
use super::paths::{accept, call, proposal_of};
use crate::conformance::fixture::{
    Lake, Must, describe_type, events_after, get_at, head_unmoved, ingest, source_record, upsert,
};
use crate::conformance::{Failure, Held, Store};

check!(
    RETIRED_BY_VERBS = a_type_only_its_verbs_retire_is_refused_at_retire,
    "016, 013 and 027, extended (lakelands#1148): a type marked `retired_by: verbs` again is taken with no migration, its version moving and its schema hash not; from there `retire`, a proposed `retire`, the accept of one proposed before the mark and a `retire` of a key no record holds are refused `verbs_only`, naming the type's retire verbs in name order, appending nothing; a low retire verb whose `requires` is unmet is refused as a precondition, and met it retires the record, its `record_retired` caused by the `verb_called` and carrying the verb as its procedure; a medium one's proposal retires at its accept, the record leaving an object set as at `retire`; a caller who may not write every field of the type is refused the verb as not granted and is described neither verb; a purge of a source still retires a record it brought; `declare` refuses the mark on a view (`retired_by_view`), a retire verb on a relation's link type (`retire_verb_link`) and one on its type (`type_verb_writes`); `describe` names the mark and which verbs retire, tells no `retire` and makes no retire tool for the type; marked `retired_by: authors` again, `retire` retires, and so does a retire verb, its `record_retired` caused by its `verb_called`"
);

check!(
    RETIRED_BY_VERBS_TOLD = a_retire_refusal_names_only_the_verbs_its_writer_is_shown,
    "006, 013 and 016, extended (lakelands#1148): a refused `retire` of a type only its verbs retire names the retire verbs its writer is shown and may call, so one kept from a field a retire verb reads across a reference is not named that verb; where it is shown none, it is answered `wrong_door`, byte for byte as on a type marked with no retire verb, and so is such a reviewer at the accept of a retirement proposed before the mark by one shown the verb; a key no record holds is answered as a live one; one who may not write every field is answered `not_granted` as on an unmarked type, and one not shown the type `not_found` as for a type not declared"
);

/// `order`, marked `retired_by` as `by`: `no` (shared), `status` and
/// `secret`.
fn order(by: &str) -> Value {
    json!({"declare": "type", "name": "order", "retired_by": by, "fields": [
        {"name": "no", "type": {"kind": "text"}, "authority": "shared"},
        {"name": "status", "type": {"kind": "text"}, "initial": "open"},
        {"name": "secret", "type": {"kind": "text"}}]})
}

/// `order`, marked by `by`, with `archive` (retire, low, `status =
/// 'closed'`), `cancel` (retire, medium) and `close` (sets `status`).
async fn orders(store: &Store, by: &str) -> Result<Lake, Failure> {
    let l = Lake::of(store.as_ref());
    l.declare_json(order(by)).await?;
    for verb in [
        json!({"name": "archive", "risk": "low", "requires": "status = 'closed'", "effect": "retire"}),
        json!({"name": "cancel", "risk": "medium", "effect": "retire"}),
        json!({"name": "close", "risk": "low", "effect": "write",
            "set": [{"field": "status", "value": "'closed'"}]}),
    ] {
        let mut verb = verb;
        verb["declare"] = json!("verb");
        verb["on"] = json!("order");
        l.declare_json(verb).await?;
    }
    Ok(l)
}

fn verbs_only(verbs: &[&str]) -> Error {
    Error::Refused {
        field: None,
        reason: Refusal::VerbsOnly {
            verbs: verbs.iter().map(|v| (*v).into()).collect(),
        },
    }
}

/// `retire` of `ty` `key` by `who`.
async fn retire(l: &Lake, who: &Principal, ty: &str, key: &str) -> lakelands_api::Result<Outcome> {
    let (ty, key) = (ty.into(), key.into());
    l.cat.retire(who, &ty, &key, WriteOpts::default()).await
}

fn proposed_retire(key: &str) -> ProposedWrite {
    ProposedWrite::Retire {
        ty: "order".into(),
        key: key.into(),
        opts: WriteOpts::default(),
    }
}

/// Whether `order` `key` is live to the owner.
async fn live(l: &Lake, key: &str) -> Result<bool, Failure> {
    let ty = "order".into();
    match l.cat.get(&l.own, &ty, get_at(key, None)).await {
        Ok(_) => Ok(true),
        Err(Error::NotFound { .. }) => Ok(false),
        Err(e) => Err(Failure::new(format!("reading `order` `{key}`: {e}"))),
    }
}

/// The case of a refused declaration.
async fn declared_case(l: &Lake, decl: Value) -> Option<Case> {
    let decl = serde_json::from_value(decl).expect("a declaration");
    let said = l.cat.declare(&l.own, decl).await;
    said.err()
        .as_ref()
        .and_then(Error::case)
        .map(|s| s.case.clone())
}

async fn a_type_only_its_verbs_retire_is_refused_at_retire(store: Store) -> Held {
    let (s, l) = (store.as_ref(), orders(&store, "authors").await?);
    for k in ["o1", "o2", "o3", "o4"] {
        upsert(l.cat.as_ref(), &l.own, "order", k, json!({"no": k})).await?;
    }
    l.declare_json(
        json!({"declare": "object_set", "name": "open_orders", "of": "order",
        "where": "status = 'open'"}),
    )
    .await?;
    // A retirement proposed before the mark.
    let early = l
        .cat
        .propose(&l.own, proposed_retire("o4"), "by hand".into());
    let early = match early.await.must("proposing the retirement of o4")? {
        Outcome::Proposed { proposal, .. } => proposal,
        other => {
            return Err(Failure::new(format!(
                "a proposal of o4's retirement: {other:?}"
            )));
        }
    };
    // Marking it is a change of meaning.
    let was = l.describe_type("order").await?.lifecycle;
    l.declare_json(order("verbs")).await?;
    let is = l.describe_type("order").await?.lifecycle;
    ensure!(
        (is.version, &is.schema_hash) == (was.version + 1, &was.schema_hash),
        "marking `order` retired by its verbs moves its version and not its hash: {was:?} → {is:?}"
    );
    // Every author's door is refused, whatever the key.
    let before = s.head().await.must("reading the head")?;
    let retired = retire(&l, &l.own, "order", "o1").await;
    let proposed = l
        .cat
        .propose(&l.own, proposed_retire("o1"), "by hand".into())
        .await;
    let missing = retire(&l, &l.own, "order", "nowhere").await;
    let want = verbs_only(&["archive", "cancel"]);
    ensure!(
        retired.as_ref().err() == Some(&want)
            && proposed.as_ref().err() == Some(&want)
            && missing.as_ref().err() == Some(&want),
        "`retire` of o1, a proposed `retire` of it and a `retire` of a key no record holds are \
         refused `verbs_only` naming `archive` and `cancel`: {retired:?}, {proposed:?}, \
         {missing:?}"
    );
    let late = accept(&l, &l.own, &early).await;
    ensure!(
        late.as_ref().err() == Some(&want) && live(&l, "o4").await?,
        "the accept of o4's retirement, proposed before the mark, is refused `verbs_only` and \
         o4 stays live: {late:?}"
    );
    head_unmoved(s, before, "a retirement of a type only its verbs retire").await?;
    // Its verbs retire: under `requires`, by the call's own events.
    let unmet = call(&l, &l.own, "archive", "o1").await;
    let case = Case::RequiresUnmet {
        verb: "archive".into(),
        ty: "order".into(),
        key: "o1".into(),
    };
    let precondition = Error::Refused {
        field: None,
        reason: Refusal::Precondition(Box::new(case.into())),
    };
    ensure!(
        unmet.as_ref().err() == Some(&precondition) && live(&l, "o1").await?,
        "`archive` of o1, still open, is refused as a precondition: {unmet:?}"
    );
    call(&l, &l.own, "close", "o1").await.must("closing o1")?;
    let before = s.head().await.must("reading the head")?;
    let archived = call(&l, &l.own, "archive", "o1").await;
    let events = events_after(s, before).await?;
    let called = events
        .iter()
        .find(|e| matches!(&e.body, EventBody::VerbCalled { .. }));
    let ended = events
        .iter()
        .find(|e| matches!(&e.body, EventBody::RecordRetired { .. }));
    let by_the_call = matches!((called, ended), (Some(c), Some(r))
        if r.cause == Some(c.position)
            && r.procedure.as_ref().is_some_and(|p| p.name == "archive"));
    ensure!(
        archived == Ok("applied") && by_the_call && !live(&l, "o1").await?,
        "`archive` of o1, closed, retires it, its `record_retired` caused by the `verb_called` \
         and carrying `archive`: {archived:?}, {events:?}"
    );
    // A medium one retires at its accept, and the record leaves its set.
    let p = proposal_of(&l, &l.own, "cancel", "o2").await?;
    let before = s.head().await.must("reading the head")?;
    let cancelled = accept(&l, &l.own, &p).await;
    let events = events_after(s, before).await?;
    let left = events.iter().any(|e| {
        matches!(&e.body, EventBody::ObjectSetChanged { set, key, joined: false, .. }
            if set == "open_orders" && key.as_str() == "o2")
    });
    ensure!(
        cancelled == Ok("applied") && left && !live(&l, "o2").await?,
        "`cancel` of o2 retires it at its accept, o2 leaving `open_orders`: {cancelled:?}, \
         {events:?}"
    );
    // The caller needs what `retire` needs.
    let reader = holder(
        s,
        "reader",
        json!({"default": "write", "types": {"order": {"fields": {"secret": "read"}}}}),
    );
    let ungranted = call(&l, &reader, "cancel", "o3").await;
    let theirs = describe_type(l.cat.as_ref(), &reader, "order").await?.verbs;
    ensure!(
        matches!(
            &ungranted,
            Err(Error::Refused {
                reason: Refusal::NotGranted,
                ..
            })
        ) && theirs
            .iter()
            .all(|v| !["archive", "cancel"].contains(&v.as_str())),
        "one who may not write `secret` is not granted `cancel` ({ungranted:?}) and is described \
         neither retire verb ({theirs:?})"
    );
    // A purge retires what its source brought, as the system.
    let fed = vec![source_record("order", "o5", json!({"no": "o5"}))];
    ingest(l.cat.as_ref(), &l.own, "erp", fed)
        .await
        .must("ingesting o5")?;
    let (erp, ty) = ("erp".into(), "order".into());
    let purged = s.purge_source(&erp, Some(&ty), None).await;
    ensure!(
        purged.is_ok() && !live(&l, "o5").await?,
        "a purge of `erp` retires o5, which it brought: {purged:?}"
    );
    // What the declaration refuses.
    let view = json!({"declare": "type", "name": "open_view", "retired_by": "verbs",
        "fields": [{"name": "n", "type": {"kind": "number"}}],
        "view": {"source": "order", "measures": [{"name": "n", "expr": "count()"}]}});
    l.declare_json(json!({"declare": "type", "name": "crate", "fields": [{"name": "no", "type": {"kind": "text"}}],
        "relations": [{"name": "packs", "target": "order", "cardinality": "many_to_many"}]}))
        .await?;
    let retiring = |on: &str, scope: &str| {
        json!({"declare": "verb", "name": "drop", "on": on, "scope": scope, "risk": "low",
            "effect": "retire"})
    };
    let cases = [
        declared_case(&l, view).await,
        declared_case(&l, retiring("sys.link.crate.packs", "record")).await,
        declared_case(&l, retiring("order", "type")).await,
    ];
    let want = [
        Some(Case::RetiredByView),
        Some(Case::RetireVerbLink {
            ty: "sys.link.crate.packs".into(),
        }),
        Some(Case::TypeVerbWrites { ty: "order".into() }),
    ];
    ensure!(
        cases == want,
        "`declare` refuses the mark on a view, a retire verb on a link type and one on its \
         type: {cases:?}"
    );
    // `describe` names the mark, and tells no `retire`.
    let told = l.describe_type("order").await?;
    let lake = l.cat.describe(&l.own, Default::default()).await;
    let lake = lake.must("describing the lake")?.data;
    let retires = |n: &str| lake.verbs.iter().any(|v| v.name.as_str() == n && v.retires);
    let (archive, close) = (retires("archive"), retires("close"));
    let tools = lakelands_api::tools::tools(&lake);
    let retire_tool = (tools.unwrap_or_default().iter()).any(
        |t| matches!(&t.door, lakelands_api::tools::Door::Retire { ty } if ty.as_str() == "order"),
    );
    ensure!(
        told.lineage.retired_by == RetiredBy::Verbs
            && !told.may.retire
            && !retire_tool
            && archive
            && !close,
        "`describe` names `order` retired by its verbs ({:?}), tells no `retire` ({:?}), names \
         `archive` as retiring and `close` not ({archive}, {close}), and the lake makes no \
         retire tool for `order` ({retire_tool})",
        told.lineage,
        told.may
    );
    // Marked back, `retire` retires, and so does a retire verb.
    l.declare_json(order("authors")).await?;
    let back = retire(&l, &l.own, "order", "o3").await;
    ensure!(
        back.is_ok() && !live(&l, "o3").await?,
        "`order` marked `retired_by: authors` again, `retire` retires o3: {back:?}"
    );
    call(&l, &l.own, "close", "o4").await.must("closing o4")?;
    let before = s.head().await.must("reading the head")?;
    let archived = call(&l, &l.own, "archive", "o4").await;
    let events = events_after(s, before).await?;
    let called = events
        .iter()
        .find(|e| matches!(&e.body, EventBody::VerbCalled { .. }));
    let ended = events
        .iter()
        .find(|e| matches!(&e.body, EventBody::RecordRetired { .. }));
    let by_the_call = matches!((called, ended), (Some(c), Some(r)) if r.cause == Some(c.position));
    ensure!(
        archived == Ok("applied") && by_the_call && !live(&l, "o4").await?,
        "on `order` retired by its authors, `archive` of o4, closed, retires it, its \
         `record_retired` caused by the `verb_called`: {archived:?}, {events:?}"
    );
    Ok(())
}

async fn a_retire_refusal_names_only_the_verbs_its_writer_is_shown(store: Store) -> Held {
    let (s, l) = (store.as_ref(), orders(&store, "verbs").await?);
    // `diary` is retired by `jot`, and by `brood`, which reads `secret` of
    // its `owner`; `quiet` by `mope` alone, which reads it too; `hush` by
    // no verb; `plain` by its authors.
    l.declare_json(json!({"declare": "type", "name": "person", "fields": [
        {"name": "no", "type": {"kind": "text"}}, {"name": "secret", "type": {"kind": "text"}}]}))
        .await?;
    upsert(l.cat.as_ref(), &l.own, "person", "p1", json!({"no": "p1"})).await?;
    let typed = |ty: &str, by: &str| {
        json!({"declare": "type", "name": ty, "retired_by": by, "fields": [
            {"name": "no", "type": {"kind": "text"}},
            {"name": "owner", "type": {"kind": "reference", "target": "person"}},
            {"name": "secret", "type": {"kind": "text"}}]})
    };
    for (ty, by) in [
        ("diary", "verbs"),
        ("quiet", "authors"),
        ("hush", "authors"),
        ("plain", "authors"),
    ] {
        l.declare_json(typed(ty, by)).await?;
        upsert(
            l.cat.as_ref(),
            &l.own,
            ty,
            "k1",
            json!({"no": "k1", "owner": "p1"}),
        )
        .await?;
    }
    // The owner proposes `k2`'s retirement of `quiet` and of `hush` before
    // they are marked.
    let mut early = Vec::new();
    for ty in ["quiet", "hush"] {
        upsert(
            l.cat.as_ref(),
            &l.own,
            ty,
            "k2",
            json!({"no": "k2", "owner": "p1"}),
        )
        .await?;
        let write = ProposedWrite::Retire {
            ty: ty.into(),
            key: "k2".into(),
            opts: WriteOpts::default(),
        };
        match l.cat.propose(&l.own, write, "by hand".into()).await {
            Ok(Outcome::Proposed { proposal, .. }) => early.push(proposal),
            other => {
                return Err(Failure::new(format!(
                    "a proposal of {ty} k2's retirement: {other:?}"
                )));
            }
        }
        l.declare_json(typed(ty, "verbs")).await?;
    }
    for (name, on, requires) in [
        ("jot", "diary", None),
        ("brood", "diary", Some("owner.secret = 'x'")),
        ("mope", "quiet", Some("owner.secret = 'x'")),
    ] {
        let mut verb =
            json!({"declare": "verb", "name": name, "on": on, "risk": "low", "effect": "retire"});
        if let Some(r) = requires {
            verb["requires"] = json!(r);
        }
        l.declare_json(verb).await?;
    }
    upsert(l.cat.as_ref(), &l.own, "order", "k1", json!({"no": "k1"})).await?;
    // `nosy` writes every field of these types and is kept from a
    // person's `secret`: it is not shown a verb that reads it.
    let nosy = holder(
        s,
        "nosy",
        json!({"default": "write", "review": true,
            "types": {"person": {"fields": {"secret": "none"}}}}),
    );
    let before = s.head().await.must("reading the head")?;
    let owned = retire(&l, &l.own, "diary", "k1").await;
    let shown = retire(&l, &nosy, "diary", "k1").await;
    let hidden = retire(&l, &nosy, "quiet", "k1").await;
    let none = retire(&l, &nosy, "hush", "k1").await;
    let wrong = Error::Refused {
        field: None,
        reason: Refusal::WrongDoor,
    };
    let said = |r: &lakelands_api::Result<Outcome>| {
        r.as_ref()
            .err()
            .map(|e| serde_json::to_string(e).unwrap_or_default())
    };
    ensure!(
        owned.as_ref().err() == Some(&verbs_only(&["brood", "jot"]))
            && shown.as_ref().err() == Some(&verbs_only(&["jot"]))
            && hidden.as_ref().err() == Some(&wrong)
            && said(&hidden) == said(&none),
        "the owner is named `brood` and `jot` for `diary`, one kept from a person's `secret` \
         `jot` alone; for `quiet`, whose one retire verb it is not shown, it is answered \
         `wrong_door` byte for byte as for `hush`, which no verb retires: {owned:?}, {shown:?}, \
         {hidden:?}, {none:?}"
    );
    // A reviewer is answered at the accept as at its own `retire`: the
    // verbs the proposer is shown are not named to it where it is not.
    let late_hidden = accept(&l, &nosy, &early[0]).await;
    let late_none = accept(&l, &nosy, &early[1]).await;
    let late = |r: &Result<&str, Error>| {
        r.as_ref()
            .err()
            .map(|e| serde_json::to_string(e).unwrap_or_default())
    };
    ensure!(
        late_hidden.as_ref().err() == Some(&wrong) && late(&late_hidden) == late(&late_none),
        "a reviewer kept from a person's `secret` accepting `quiet` k2's retirement, proposed \
         before the mark by one shown `mope`, is answered `wrong_door` byte for byte as for \
         `hush` k2: {late_hidden:?}, {late_none:?}"
    );
    // A key no record holds is answered as a live one.
    let missing = retire(&l, &nosy, "diary", "nowhere").await;
    ensure!(
        said(&missing) == said(&shown),
        "a key no record holds is answered as k1: {missing:?}, {shown:?}"
    );
    // One who may not write every field is not granted it, as on a type
    // its authors retire.
    let reader = holder(
        s,
        "reader",
        json!({"default": "write", "types": {
            "order": {"fields": {"secret": "read"}}, "plain": {"fields": {"secret": "read"}}}}),
    );
    let marked = retire(&l, &reader, "order", "k1").await;
    let unmarked = retire(&l, &reader, "plain", "k1").await;
    ensure!(
        matches!(
            &marked,
            Err(Error::Refused {
                reason: Refusal::NotGranted,
                ..
            })
        ) && said(&marked) == said(&unmarked),
        "one who may only read `secret` is not granted `retire` of `order`, as of `plain`: \
         {marked:?}, {unmarked:?}"
    );
    // One not shown the type is answered as for a type not declared.
    let blind = holder(
        s,
        "blind",
        json!({"default": "write", "types": {"order": {"default": "none"}}}),
    );
    let unseen = retire(&l, &blind, "order", "k1").await;
    let undeclared = retire(&l, &blind, "nowhere", "k1").await;
    ensure!(
        matches!(&unseen, Err(Error::NotFound { .. }))
            && said(&unseen).map(|s| s.replace("order", "nowhere")) == said(&undeclared),
        "one not shown `order` is answered as for `nowhere`: {unseen:?}, {undeclared:?}"
    );
    head_unmoved(s, before, "a refused retirement").await?;
    Ok(())
}
