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

use std::sync::Arc;

use lakelands_api::event::EventBody;
use lakelands_api::seam::FeedFilter;
use lakelands_api::write::{
    Declaration, Migration, ProposalFilter, ProposalKind, ProposalState, ProposedWrite,
};
use lakelands_api::{
    Error, Grants, LakeStore, Outcome, Position, PrincipalKind, ProposalId, ReadAt, Refusal,
    TypeName,
};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, doors, field_writer, head_unmoved, owner, principal, upsert,
};

check!(
    /// A proposed declaration takes effect only when accepted.
    DECLARED_ON_ACCEPT = a_proposed_declaration_takes_effect_only_when_accepted,
    "013, extended, and 007: a principal without the schema grant may propose declarations; nothing is declared and no schema hash moves until a reviewer with review and schema accepts, and then the declaration is logged at the acceptance, written as the proposer and caused by the acceptance"
);

check!(
    /// Accepting a schema proposal needs the schema grant.
    SCHEMA_TO_ACCEPT = accepting_a_schema_proposal_needs_the_schema_grant,
    "013, extended: a reviewer with review alone may not accept a declaration or a migration; the refusal is not_granted, nothing is appended, and the proposal stays pending"
);

check!(
    /// A migration proposal carries its report and is preflighted again.
    MIGRATION_PREFLIGHTED_AGAIN = a_migration_proposal_carries_its_report_and_is_preflighted_again_at_accept,
    "013, extended, and 027: a proposed migration carries the report its preflight answered then; at the acceptance it is preflighted again on the head, and one that would now lose information is refused with the fresh report, the proposal pending and the type unchanged"
);

check!(
    /// A rejected schema proposal changes nothing.
    REJECTED_UNTOUCHED = a_rejected_schema_proposal_leaves_the_ontology_untouched,
    "013, extended: a rejected declaration leaves the ontology as it was, and proposals can be listed by kind"
);

/// A reviewer holding `review`, and `schema` when asked.
fn reviewer(store: &dyn LakeStore, schema: bool) -> lakelands_api::Principal {
    principal(
        store,
        PrincipalKind::Human,
        if schema { "architect" } else { "reviewer" },
        Grants {
            review: true,
            schema,
            ..Grants::default()
        },
    )
}

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

async fn declared(store: &dyn LakeStore, ty: &str) -> Result<bool, Failure> {
    let lake = doors(store)
        .describe(&owner(store), ReadAt::default())
        .await
        .must("describing the lake")?
        .data;
    Ok(lake.types.iter().any(|t| t.name.as_str() == ty))
}

async fn a_proposed_declaration_takes_effect_only_when_accepted(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let who = agent(store.as_ref());
    let id = proposed(
        &cat.propose(
            &who,
            ProposedWrite::Declare {
                declarations: vec![ticket()],
            },
            "a place for tickets".into(),
        )
        .await
        .must("proposing a type without the schema grant")?,
    )?;
    ensure!(
        !declared(store.as_ref(), "ticket").await?,
        "a proposed type must not be declared before it is accepted"
    );
    let before = store.head().await.must("reading the head")?;
    let accepted = cat
        .accept(&reviewer(store.as_ref(), true), &id)
        .await
        .must("a reviewer with review and schema accepting the declaration")?;
    let at = accepted.position();
    ensure!(
        declared(store.as_ref(), "ticket").await?,
        "an accepted type must be declared"
    );
    let events = store
        .feed()
        .poll(Position(before.0 + 1), &FeedFilter::default(), 100)
        .await
        .must("reading the log")?
        .events;
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
    upsert(
        cat.as_ref(),
        &owner(store.as_ref()),
        "ticket",
        "t1",
        json!({"title": "first"}),
    )
    .await?;
    Ok(())
}

async fn accepting_a_schema_proposal_needs_the_schema_grant(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let id = proposed(
        &cat.propose(
            &agent(store.as_ref()),
            ProposedWrite::Declare {
                declarations: vec![ticket()],
            },
            "a place for tickets".into(),
        )
        .await
        .must("proposing a type")?,
    )?;
    let before = store.head().await.must("reading the head")?;
    let refused = cat
        .accept(&reviewer(store.as_ref(), false), &id)
        .await
        .must_fail("a reviewer without the schema grant accepting a declaration")?;
    ensure!(
        matches!(
            refused,
            Error::Refused {
                reason: Refusal::NotGranted,
                ..
            }
        ),
        "accepting a declaration without the schema grant must be refused as not granted; it \
         was refused as {refused:?}"
    );
    head_unmoved(store.as_ref(), before, "a refused accept").await?;
    let pending = cat
        .proposals(
            &reviewer(store.as_ref(), false),
            ProposalFilter {
                state: Some(ProposalState::Pending),
                ..ProposalFilter::default()
            },
        )
        .await
        .must("listing the pending proposals")?
        .data;
    ensure!(
        pending.iter().any(|p| p.id == id) && !declared(store.as_ref(), "ticket").await?,
        "after the refused accept the proposal must stay pending and nothing be declared"
    );
    Ok(())
}

async fn a_migration_proposal_carries_its_report_and_is_preflighted_again_at_accept(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let memo: Declaration = serde_json::from_value(json!({
        "declare": "type", "name": "memo",
        "fields": [{"name": "title", "type": {"kind": "text"}},
                   {"name": "pages", "type": {"kind": "text"}}]
    }))
    .expect("a declaration");
    cat.declare(own, memo.clone())
        .await
        .must("declaring `memo`")?;
    l.upsert("memo", "a", json!({"pages": "3"})).await?;
    let Declaration::Type(mut to) = memo else {
        unreachable!("a type")
    };
    for f in &mut to.fields {
        if f.name.as_str() == "pages" {
            f.ty = lakelands_api::ontology::FieldType::kind(
                lakelands_api::ontology::StorageKind::Number,
            );
        }
    }
    let retype = Migration {
        to,
        renames: vec![],
    };
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
        .and_then(|p| match &p.write {
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
        matches!(&refused, Error::Refused { reason: Refusal::Migration { report }, .. } if !report.lossless),
        "the accept must be refused with the fresh preflight's report, not lossless; it was \
         refused as {refused:?}"
    );
    head_unmoved(store.as_ref(), before, "a refused migration accept").await?;
    let still = cat
        .proposals(
            &reviewer(store.as_ref(), true),
            ProposalFilter {
                state: Some(ProposalState::Pending),
                ..ProposalFilter::default()
            },
        )
        .await
        .must("listing the pending proposals")?
        .data;
    let pages = cat
        .describe_type(own, &TypeName::from("memo"), ReadAt::default())
        .await
        .must("describing `memo`")?
        .data
        .fields
        .into_iter()
        .find(|f| f.name.as_str() == "pages")
        .map(|f| f.ty.kind);
    ensure!(
        still.iter().any(|p| p.id == id)
            && pages == Some(lakelands_api::ontology::StorageKind::Text),
        "after the refused accept the proposal must stay pending and `pages` stay text; it is \
         {pages:?}"
    );
    Ok(())
}

async fn a_rejected_schema_proposal_leaves_the_ontology_untouched(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let id = proposed(
        &cat.propose(
            &agent(store.as_ref()),
            ProposedWrite::Declare {
                declarations: vec![ticket()],
            },
            "a place for tickets".into(),
        )
        .await
        .must("proposing a type")?,
    )?;
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
