//! A declaration or a migration proposed like a write
//! (`docs/decisions/013-verbs-run-by-declared-risk.md`, extended;
//! `docs/decisions/027-shape-changes-are-declared-or-migrated.md`,
//! `docs/decisions/007-schema-hash-covers-shape.md`).
//!
//! A principal without the `schema` grant proposes a change of the
//! ontology; nothing changes until a reviewer holding `review` and
//! `schema` accepts it. The change is checked at the proposal as its door
//! would check it, and again at the acceptance against the head, where a
//! change the head no longer admits is refused and the proposal stays
//! pending.

use lakelands_api::write::ProposalKind;

use crate::conformance::fixture::{doors, reviewer, type_of};
use crate::conformance::prelude::*;

check!(
    DECLARED_ON_ACCEPT = a_proposed_declaration_takes_effect_only_when_accepted,
    "013, extended, and 007: a principal without the schema grant may propose declarations; nothing is declared and no schema hash moves until a reviewer with review and schema accepts, and then the declaration is logged at the acceptance, written as the proposer and caused by the acceptance"
);

check!(
    SCHEMA_TO_ACCEPT = accepting_a_schema_proposal_needs_the_schema_grant,
    "013, extended: a reviewer with review alone may not accept a declaration or a migration; the refusal is not_granted, nothing is appended, and the proposal stays pending"
);

check!(
    MIGRATION_ON_ITS_SHAPE = a_proposed_migration_is_accepted_only_on_the_shape_it_was_proposed_on,
    "013, extended (lakelands#489): a migration's `to` is the type's whole shape, so a proposed migration is accepted only while the type's schema hash is the one its report was made on; on a type declared or migrated since, the accept is refused as invalid, nothing is appended, the proposal stays pending and a field added since keeps its values; on the shape it was proposed on it applies"
);

check!(
    MIGRATION_PREFLIGHTED_AGAIN =
        a_migration_proposal_carries_its_report_and_is_preflighted_again_at_accept,
    "013, extended, and 027: a proposed migration carries the report its preflight answered then; at the acceptance it is preflighted again on the head, and one that would now lose information is refused with the fresh report, the proposal pending and the type unchanged"
);

check!(
    REJECTED_UNTOUCHED = a_rejected_schema_proposal_leaves_the_ontology_untouched,
    "013, extended: a rejected declaration leaves the ontology as it was, and proposals can be listed by kind"
);
check!(
    TIGHTENING_PROPOSED = a_value_types_tightening_is_proposed_without_schema_and_applied_on_accept,
    "013 and 027, extended (lakelands#476): a principal without the schema grant may propose a value type that accepts less; the proposal is one event and holds the preflight's report, whose uses a reader of the listing sees only in fields it may read, the totals whole; accepting it takes schema beside review, and then the value type is in force and a value it no longer accepts is refused"
);

check!(
    TIGHTENING_HELD_BACK =
        a_tightening_is_refused_while_a_value_would_not_fit_or_the_value_type_changed,
    "013 and 027, extended (lakelands#476): a value type's tightening is refused at the proposal while a stored value would not fit, as invalid, saying how many and naming none, with nothing appended; at the acceptance it is preflighted again and refused the same way, and it is refused when the value type was declared again since the proposal; each time the proposal stays pending and the value type as it was; a narrowed declaration's refusal points to the proposal"
);

/// A principal without the `schema` grant that writes `memo`'s title.
fn agent(store: &dyn LakeStore) -> lakelands_api::Principal {
    field_writer(store, "agent", "memo", &["title"])
}

fn ticket() -> Declaration {
    serde_json::from_value(json!({
        "declare": "type", "name": "ticket",
        "fields": [{"name": "title", "type": {"kind": "text"}}]
    }))
    .expect("a declaration")
}

fn proposed(o: &Outcome) -> Result<ProposalId, Failure> {
    match o {
        Outcome::Proposed { proposal, .. } => Ok(proposal.clone()),
        other => Err(Failure::new(format!(
            "a proposal answers `proposed`; it answered {other:?}"
        ))),
    }
}

/// The agent's proposal of the type `ticket`.
async fn propose_ticket(store: &dyn LakeStore) -> Result<ProposalId, Failure> {
    let write = ProposedWrite::Declare {
        declarations: vec![ticket()],
        plan: None,
    };
    let why = "a place for tickets".into();
    let o = doors(store).propose(&agent(store), write, why).await;
    proposed(&o.must("the agent proposing the type `ticket`")?)
}

async fn declared(store: &dyn LakeStore, ty: &str) -> Result<bool, Failure> {
    let lake = Lake::of(store).describe().await?;
    Ok(lake.types.iter().any(|t| t.name.as_str() == ty))
}

/// Whether the reviewer (holding `schema` when asked) lists the proposal
/// `id` among the pending ones, asked by state.
async fn pending(store: &dyn LakeStore, schema: bool, id: &ProposalId) -> Result<bool, Failure> {
    let state = ProposalFilter {
        state: Some(ProposalState::Pending),
        ..ProposalFilter::default()
    };
    let who = reviewer(store, schema);
    let listed = doors(store).proposals(&who, state).await;
    let listed = listed.must("listing the pending proposals")?.data;
    Ok(listed.iter().any(|p| p.id == *id))
}

async fn a_proposed_declaration_takes_effect_only_when_accepted(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let who = agent(store.as_ref());
    let id = propose_ticket(store.as_ref()).await?;
    ensure!(
        !declared(store.as_ref(), "ticket").await?,
        "a proposed type must not be declared before it is accepted"
    );
    let before = store.head().await.must("reading the head")?;
    let accepted = l
        .cat
        .accept(&reviewer(store.as_ref(), true), &id)
        .await
        .must("a reviewer with review and schema accepting the declaration")?;
    let at = accepted.position();
    ensure!(
        declared(store.as_ref(), "ticket").await?,
        "an accepted type must be declared"
    );
    let events = events_after(store.as_ref(), before).await?;
    let acceptance = events
        .iter()
        .find(|e| matches!(e.body, EventBody::WriteAccepted { .. }))
        .map(|e| e.position);
    let declaration = events
        .iter()
        .find(|e| matches!(&e.body, EventBody::Declared { declaration: Declaration::Type(t), .. } if t.name.as_str() == "ticket"));
    ensure!(
        declaration.is_some_and(|e| e.writer == who.writer() && e.cause == acceptance)
            && acceptance.is_some_and(|a| a <= at),
        "the accepted declaration must be logged after `write_accepted`, written as the proposer \
         `{}` and caused by the acceptance; the log holds {:?}",
        who.writer(),
        events
            .iter()
            .map(|e| (e.position, e.writer.clone(), e.cause))
            .collect::<Vec<_>>()
    );
    l.upsert("ticket", "t1", json!({"title": "first"})).await?;
    Ok(())
}

async fn accepting_a_schema_proposal_needs_the_schema_grant(store: Store) -> Held {
    let cat = doors(store.as_ref());
    let id = propose_ticket(store.as_ref()).await?;
    let before = store.head().await.must("reading the head")?;
    let refused = cat
        .accept(&reviewer(store.as_ref(), false), &id)
        .await
        .must_fail("a reviewer without the schema grant accepting a declaration")?;
    ensure!(
        matches!(reason(&refused), Some(Refusal::NotGranted)),
        "accepting a declaration without the schema grant must be refused as not granted; it \
         was refused as {refused:?}"
    );
    head_unmoved(store.as_ref(), before, "a refused accept").await?;
    ensure!(
        pending(store.as_ref(), false, &id).await? && !declared(store.as_ref(), "ticket").await?,
        "after the refused accept the proposal must stay pending and nothing be declared"
    );
    Ok(())
}

async fn a_migration_proposal_carries_its_report_and_is_preflighted_again_at_accept(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let cat = &l.cat;
    let memo: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "memo",
        "fields": [{"name": "title", "type": {"kind": "text"}},
                   {"name": "pages", "type": {"kind": "text"}}]
    }))
    .expect("a declaration");
    l.declare_as(memo.clone(), "declaring `memo`").await?;
    l.upsert("memo", "a", json!({"pages": "3"})).await?;
    let Declaration::Type(mut to) = memo else {
        unreachable!("a type")
    };
    for f in &mut to.fields {
        if f.name.as_str() == "pages" {
            f.ty = FieldType::kind(StorageKind::Number);
        }
    }
    let retype = migration(to);
    let id = proposed(
        &cat.propose(
            &agent(store.as_ref()),
            ProposedWrite::Migrate {
                migration: Box::new(retype.clone()),
                report: Default::default(),
            },
            "pages are numbers".into(),
        )
        .await
        .must("proposing a lossless retype without the schema grant")?,
    )?;
    let listed = cat
        .proposals(
            &reviewer(store.as_ref(), true),
            ProposalFilter {
                kind: Some(ProposalKind::Migrate),
                ..ProposalFilter::default()
            },
        )
        .await
        .must("listing the migration proposals")?
        .data;
    let report = listed
        .iter()
        .find(|p| p.id == id)
        .and_then(|p| match p.write.as_ref()? {
            ProposedWrite::Migrate { report, .. } => Some(report.clone()),
            _ => None,
        });
    ensure!(
        report
            .as_ref()
            .is_some_and(|r| r.lossless && r.records_scanned == 1),
        "the proposal must carry the preflight's report (lossless, one record scanned); it \
         carries {report:?}"
    );
    // The lake moves: a record the retype would lose.
    l.upsert("memo", "b", json!({"pages": "many"})).await?;
    let before = store.head().await.must("reading the head")?;
    let refused = cat
        .accept(&reviewer(store.as_ref(), true), &id)
        .await
        .must_fail("accepting a migration that would now lose `pages` of `b`")?;
    ensure!(
        matches!(reason(&refused), Some(Refusal::Migration { report }) if !report.lossless),
        "the accept must be refused with the fresh preflight's report, not lossless; it was \
         refused as {refused:?}"
    );
    head_unmoved(store.as_ref(), before, "a refused migration accept").await?;
    let pages = l
        .describe_type("memo")
        .await?
        .fields
        .into_iter()
        .find(|f| f.name.as_str() == "pages")
        .map(|f| f.ty.kind);
    ensure!(
        pending(store.as_ref(), true, &id).await? && pages == Some(StorageKind::Text),
        "after the refused accept the proposal must stay pending and `pages` stay text; it is \
         {pages:?}"
    );
    Ok(())
}

async fn a_rejected_schema_proposal_leaves_the_ontology_untouched(store: Store) -> Held {
    let cat = doors(store.as_ref());
    let id = propose_ticket(store.as_ref()).await?;
    let by_kind = cat
        .proposals(
            &reviewer(store.as_ref(), true),
            ProposalFilter {
                kind: Some(ProposalKind::Declare),
                ty: Some(TypeName::from("ticket")),
                ..ProposalFilter::default()
            },
        )
        .await
        .must("listing the declaration proposals of `ticket`")?
        .data;
    ensure!(
        by_kind.iter().map(|p| &p.id).collect::<Vec<_>>() == [&id],
        "listing by kind `declare` and type `ticket` must answer the one proposal"
    );
    cat.reject(&reviewer(store.as_ref(), true), &id, "not now".into())
        .await
        .must("rejecting the declaration")?;
    ensure!(
        !declared(store.as_ref(), "ticket").await?,
        "a rejected declaration must leave the ontology untouched"
    );
    Ok(())
}

/// The agent's proposal of a migration to `to`.
async fn propose_shape(
    store: &dyn LakeStore,
    to: lakelands_api::ontology::TypeDecl,
    why: &str,
) -> Result<ProposalId, Failure> {
    let write = ProposedWrite::Migrate {
        migration: Box::new(migration(to)),
        report: Default::default(),
    };
    let (cat, who) = (doors(store), agent(store));
    proposed(&cat.propose(&who, write, why.into()).await.must(why)?)
}

async fn a_proposed_migration_is_accepted_only_on_the_shape_it_was_proposed_on(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let cat = &l.cat;
    let memo = |fields: &[(&str, StorageKind)]| type_of("memo", fields);
    let text = StorageKind::Text;
    l.declare(memo(&[("title", text), ("pages", text)])).await?;
    l.upsert("memo", "a", json!({"title": "t", "pages": "3"}))
        .await?;
    let architect = reviewer(store.as_ref(), true);

    // On the shape it was proposed on, it applies.
    let retype = memo(&[("title", text), ("pages", StorageKind::Number)]);
    let retype = propose_shape(store.as_ref(), retype, "numbers").await?;
    cat.accept(&architect, &retype)
        .await
        .must("accepting the retype on the shape it was proposed on")?;

    // The type gains a field after the next proposal, and a record holds it.
    let drop_title = memo(&[("pages", StorageKind::Number)]);
    let drop_title = propose_shape(store.as_ref(), drop_title, "no titles").await?;
    let grown = memo(&[
        ("title", text),
        ("pages", StorageKind::Number),
        ("owner", text),
    ]);
    l.declare(grown).await?;
    l.upsert("memo", "a", json!({"owner": "ada"})).await?;
    let before = store.head().await.must("reading the head")?;
    let refused = cat.accept(&architect, &drop_title).await;
    let a = l.read("memo", "a").await?;
    ensure!(
        matches!(reason(&refused), Some(Refusal::Invalid(..)))
            && pending(store.as_ref(), true, &drop_title).await?
            && crate::conformance::fixture::value(&a, "owner") == Some(&json!("ada")),
        "a migration proposed before `owner` was added must be refused as invalid when accepted \
         after, stay pending, and leave `owner` with its value; the accept answered {refused:?} \
         and `a` reads {:?}",
        a.fields
    );
    head_unmoved(store.as_ref(), before, "the refused accept").await
}

const STATES: &str = "ticket_state";

fn states(one_of: &[&str]) -> lakelands_api::ontology::ValueTypeDecl {
    serde_json::from_value(json!({"name": STATES, "kind": "text", "one_of": one_of}))
        .expect("a value type")
}

/// `ticket_state` over open, paid and void, used by `ticket.state` and by
/// `ticket.inner`; `t1` is open and `t2` paid in both. Answers the lake and
/// a clerk without the `schema` grant who may not know of `inner`.
async fn tickets(store: &dyn LakeStore) -> Result<(Lake, lakelands_api::Principal), Failure> {
    let l = Lake::of(store);
    declare_states(&l, &["open", "paid", "void"]).await?;
    let state = json!({"kind": "text", "value_type": STATES});
    l.declare_json(json!({"declare": "type", "name": "ticket", "fields": [
        {"name": "state", "type": state}, {"name": "inner", "type": state}]}))
        .await?;
    for (key, s) in [("t1", "open"), ("t2", "paid")] {
        l.upsert("ticket", key, json!({"state": s, "inner": s}))
            .await?;
    }
    let hidden = [("inner", lakelands_api::Access::None)];
    Ok((l, field_access(store, "clerk", "ticket", &hidden)))
}

async fn declare_states(l: &Lake, one_of: &[&str]) -> Result<Outcome, Failure> {
    let d = Declaration::ValueType(states(one_of));
    l.cat.declare(&l.own, d).await.must("declaring the states")
}

/// The uses the proposal's report names to `who`, how many records it
/// scanned and whether it fits.
async fn report_of(
    l: &Lake,
    who: &lakelands_api::Principal,
    id: &ProposalId,
) -> Result<(Vec<String>, u64, bool), Failure> {
    let listed = l.cat.proposals(who, ProposalFilter::default()).await;
    let mine = listed.must("listing the proposals")?.data;
    match mine.into_iter().find(|p| p.id == *id).and_then(|p| p.write) {
        Some(ProposedWrite::MigrateValueType { report, .. }) => {
            let uses = (report.fields.iter())
                .map(|u| format!("{}.{}={}", u.ty, u.field, u.values_scanned));
            Ok((uses.collect(), report.records_scanned, report.fits))
        }
        other => Err(Failure::new(format!("the proposal is listed as {other:?}"))),
    }
}

/// `who` proposing `ticket_state` over `one_of`.
async fn propose_states(
    l: &Lake,
    who: &lakelands_api::Principal,
    one_of: &[&str],
) -> lakelands_api::Result<Outcome> {
    let write = ProposedWrite::MigrateValueType {
        to: Box::new(states(one_of)),
        renames: Vec::new(),
        report: Default::default(),
    };
    l.cat.propose(who, write, "fewer states".into()).await
}

async fn a_value_types_tightening_is_proposed_without_schema_and_applied_on_accept(
    store: Store,
) -> Held {
    let (l, clerk) = tickets(store.as_ref()).await?;
    let before = store.head().await.must("reading the head")?;
    let o = propose_states(&l, &clerk, &["open", "paid"]).await;
    let id = proposed(&o.must("the clerk proposing `ticket_state` without `void`")?)?;
    let events = events_after(store.as_ref(), before).await?.len();
    let (whole, shown) = (
        report_of(&l, &l.own, &id).await?,
        report_of(&l, &clerk, &id).await?,
    );
    let both = vec!["ticket.state=2".to_owned(), "ticket.inner=2".to_owned()];
    ensure!(
        events == 1
            && whole == (both, 2, true)
            && shown == (vec!["ticket.state=2".to_owned()], 2, true),
        "the proposal must be one event whose report fits and names both \
         uses to the owner, and to its proposer, who may not know of `inner`, only `state`, \
         the totals whole; it was written as {events} events, the owner reads {whole:?}, the clerk {shown:?}"
    );
    let refused = l.cat.accept(&reviewer(store.as_ref(), false), &id).await;
    ensure!(
        matches!(reason(&refused), Some(Refusal::NotGranted))
            && pending(store.as_ref(), true, &id).await?,
        "accepting a tightening without the schema grant must be refused as not granted, the \
         proposal pending; it answered {refused:?}"
    );
    let architect = reviewer(store.as_ref(), true);
    let fut = l.cat.accept(&architect, &id);
    fut.await.must("accepting the tightening")?;
    let void = l.upsert("ticket", "t3", json!({"state": "void"})).await;
    let paid = l.upsert("ticket", "t3", json!({"state": "paid"})).await;
    ensure!(
        void.is_err() && paid.is_ok(),
        "once accepted, `ticket_state` must refuse `void` and still take `paid`; writing \
         `void` answered {void:?}"
    );
    Ok(())
}

async fn a_tightening_is_refused_while_a_value_would_not_fit_or_the_value_type_changed(
    store: Store,
) -> Held {
    let (l, clerk) = tickets(store.as_ref()).await?;
    let architect = reviewer(store.as_ref(), true);
    // Refused as `says`, and nothing of its wire form names `t3`.
    let held = |answer: &lakelands_api::Result<Outcome>, says: Value| {
        let wire = answer.as_ref().err().map(|e| json!(e).to_string());
        invalid_like(answer, &says) && !wire.unwrap_or_default().contains("t3")
    };
    let narrowed = ProposedWrite::Declare {
        declarations: vec![Declaration::ValueType(states(&["open", "paid"]))],
        plan: None,
    };
    let pointed = l.cat.propose(&clerk, narrowed, "fewer".into()).await;
    ensure!(
        // The case whose sentence says to propose it as a value type's migration.
        held(&pointed, json!({"code": "value_type_tightens"})),
        "a narrowed declaration's refusal must point to the proposal; it answered {pointed:?}"
    );
    // Proposed while it fits; then a `void` is written, a second proposal
    // made, and the value type declared again with one state more.
    let o = propose_states(&l, &clerk, &["open", "paid"]).await;
    let waiting = proposed(&o.must("proposing while every value fits")?)?;
    l.upsert("ticket", "t3", json!({"state": "void"})).await?;
    let before = store.head().await.must("reading the head")?;
    let misfit = json!({"code": "value_type_not_tightened", "misfits": 1, "first": []});
    let (late, again) = (
        l.cat.accept(&architect, &waiting).await,
        propose_states(&l, &clerk, &["open", "paid"]).await,
    );
    ensure!(
        held(&late, misfit.clone()) && held(&again, misfit),
        "with one `void` stored, the accept and a new proposal must each be refused as invalid, \
         saying the count and naming no record; they answered {late:?} and {again:?}"
    );
    head_unmoved(store.as_ref(), before, "a tightening that does not fit").await?;
    l.upsert("ticket", "t3", json!({"state": "open"})).await?;
    declare_states(&l, &["open", "paid", "void", "held"]).await?;
    let before = store.head().await.must("reading the head")?;
    let stale = l.cat.accept(&architect, &waiting).await;
    ensure!(
        held(&stale, json!({"code": "value_type_changed_since_proposed"}))
            && pending(store.as_ref(), true, &waiting).await?,
        "a tightening proposed before `held` was added must be refused as invalid when accepted \
         after, and stay pending; the accept answered {stale:?}"
    );
    head_unmoved(store.as_ref(), before, "the refused accept").await?;
    l.upsert("ticket", "t3", json!({"state": "void"}))
        .await
        .map(|_| ())
}
