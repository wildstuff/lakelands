//! Verbs by declared risk, and the proposals a riskier write becomes
//! (`docs/decisions/013-verbs-run-by-declared-risk.md`,
//! `docs/decisions/015-errors-are-a-closed-set-writes-answer-an-outcome.md`,
//! `docs/decisions/004-the-change-feed-is-the-log.md`).

use std::sync::Arc;

use lakelands_api::event::EventBody;
use lakelands_api::ontology::StorageKind;
use lakelands_api::seam::FeedFilter;
use lakelands_api::write::{Declaration, ProposalFilter, ProposalState, ProposedWrite};
use lakelands_api::{
    Error, Grants, LakeStore, Outcome, Position, PrincipalKind, ProposalId, Refusal, Version,
    WriteOpts,
};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, declare, doors, field, field_writer, head_unmoved, owner, patch, principal, read,
    type_decl, upsert, value,
};

check!(
    /// A verb runs or waits by its declared risk.
    VERB_BY_RISK = a_low_verb_runs_and_a_higher_one_is_proposed,
    "013: a `low` verb that writes authored fields runs at once; any other risk is recorded as a proposal and runs only when accepted"
);

check!(
    /// A verb that reaches outside needs its executor.
    EXTERNAL_UNBOUND = an_external_verb_without_an_executor_is_unbound,
    "013 and 015: a verb whose effect reaches outside the lake runs only through a bound executor; without one the answer is `unbound`, and nothing is appended"
);

check!(
    /// A proposal is logged, and accepting it applies it as the proposer.
    ACCEPT_APPLIES = accepting_a_proposal_applies_its_write_as_the_proposers,
    "013 and 004: a proposal is a `write_proposed` event; accepting it appends `write_accepted`, then the verb's run caused by that acceptance and its write caused by that run, written as the proposer"
);

check!(
    /// A rejection writes nothing but itself, and decides for good.
    REJECT_AND_DECIDED = a_rejected_proposal_writes_nothing_and_a_decided_one_stays_decided,
    "013: rejecting appends `write_rejected` and nothing else; a decided proposal is refused as decided"
);

check!(
    /// Reviewers decide; proposers see their own.
    WHO_DECIDES = only_a_reviewer_decides_and_a_proposer_sees_its_own_proposals,
    "006, 013 and Catalog::proposals: accept and reject need the review grant; without it, `proposals` lists only the principal's own"
);

check!(
    /// A field a verb wrote names the verb's version.
    VERB_PROCEDURE_VERSION = a_verbs_write_carries_the_verbs_procedure_version,
    "010 and the log format §1.1: a value a verb produced carries, as its procedure_version, the version of the verb's declaration"
);

async fn ledger(store: &dyn LakeStore) -> Result<(), Failure> {
    let cat = doors(store);
    let own = owner(store);
    let ty = type_decl(
        "invoice",
        vec![
            field("status", StorageKind::Text),
            field("paid_ref", StorageKind::Text),
        ],
    );
    declare(cat.as_ref(), &own, ty).await?;
    for verb in [
        json!({"declare": "verb", "name": "approve", "on": "invoice", "risk": "low",
               "effect": "write", "set": [{"field": "status", "value": "'approved'"}]}),
        json!({"declare": "verb", "name": "pay", "on": "invoice", "risk": "high",
               "payload": [{"name": "reference", "type": {"kind": "text"}}],
               "effect": "write", "set": [{"field": "status", "value": "'paid'"},
                                          {"field": "paid_ref", "value": "payload.reference"}]}),
        json!({"declare": "verb", "name": "remind", "on": "invoice", "risk": "low",
               "effect": "external", "executor": "mailer"}),
    ] {
        let d: Declaration = serde_json::from_value(verb)
            .map_err(|e| Failure::new(format!("the check's verb does not read: {e}")))?;
        cat.declare(&own, d).await.must("declaring a verb")?;
    }
    upsert(
        cat.as_ref(),
        &own,
        "invoice",
        "i",
        json!({"status": "open"}),
    )
    .await?;
    Ok(())
}

async fn call(
    store: &dyn LakeStore,
    who: &lakelands_api::Principal,
    verb: &str,
    payload: serde_json::Value,
) -> lakelands_api::Result<Outcome> {
    doors(store)
        .call(
            who,
            &verb.into(),
            &"i".into(),
            patch(payload),
            WriteOpts::default(),
        )
        .await
}

fn proposal(o: &Outcome) -> Result<(Position, ProposalId), Failure> {
    match o {
        Outcome::Proposed {
            position, proposal, ..
        } => Ok((*position, proposal.clone())),
        other => Err(Failure::new(format!(
            "a high-risk verb must be proposed; it answered {other:?}"
        ))),
    }
}

async fn kinds_from(store: &dyn LakeStore, from: Position) -> Result<Vec<EventBody>, Failure> {
    let batch = store
        .feed()
        .poll(from, &FeedFilter::default(), 100)
        .await
        .must("reading the feed")?;
    Ok(batch.events.into_iter().map(|e| e.body).collect())
}

async fn a_low_verb_runs_and_a_higher_one_is_proposed(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    ledger(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let ran = call(store.as_ref(), &own, "approve", json!({}))
        .await
        .must("calling the low-risk `approve`")?;
    ensure!(
        matches!(
            ran,
            Outcome::Applied {
                version: Some(Version(2)),
                ..
            }
        ),
        "the low-risk `approve` must run at once and answer the record's new version \
         2; it answered {ran:?}"
    );
    let head = store.head().await.must("reading the head")?;
    ensure!(
        ran.position() == head,
        "the low-risk `approve` answered position {}, but its write is visible only \
         from the head {head}, which it must answer",
        ran.position()
    );
    let r = read(cat.as_ref(), &own, "invoice", "i").await?;
    ensure!(
        value(&r, "status") == Some(&json!("approved")),
        "after `approve` ran, `status` reads {:?}",
        value(&r, "status")
    );

    let waiting = call(store.as_ref(), &own, "pay", json!({"reference": "TX-1"}))
        .await
        .must("calling the high-risk `pay`")?;
    let (at, _) = proposal(&waiting)?;
    let head = store.head().await.must("reading the head")?;
    ensure!(
        at == head,
        "the proposal answered position {at}, but the head is {head}"
    );
    let r = read(cat.as_ref(), &own, "invoice", "i").await?;
    ensure!(
        value(&r, "status") == Some(&json!("approved")) && r.version == Version(2),
        "a proposed `pay` must not run: `i` reads status {:?} at version {}",
        value(&r, "status"),
        r.version
    );
    let logged = kinds_from(store.as_ref(), at).await?;
    ensure!(
        matches!(logged.as_slice(), [EventBody::WriteProposed { .. }]),
        "the proposal must be logged as one `write_proposed` event; the log from its \
         position holds {:?}",
        logged.iter().map(EventBody::kind).collect::<Vec<_>>()
    );
    Ok(())
}

async fn an_external_verb_without_an_executor_is_unbound(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    ledger(store.as_ref()).await?;
    let own = owner(store.as_ref());
    let before = store.head().await.must("reading the head")?;
    let err = call(store.as_ref(), &own, "remind", json!({}))
        .await
        .must_fail("calling `remind`, whose executor `mailer` is not bound")?;
    head_unmoved(store.as_ref(), before, "calling an unbound external verb").await?;
    ensure!(
        matches!(err, Error::Unbound { .. }),
        "calling an external verb with no executor bound must answer `unbound`; it \
         failed with {err:?}"
    );
    Ok(())
}

async fn accepting_a_proposal_applies_its_write_as_the_proposers(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    ledger(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let clerk = field_writer(store.as_ref(), "clerk", "invoice", &["status", "paid_ref"]);
    let reviewer = principal(
        store.as_ref(),
        PrincipalKind::Human,
        "reviewer",
        Grants {
            review: true,
            ..Grants::default()
        },
    );
    let (at, id) = proposal(
        &call(store.as_ref(), &clerk, "pay", json!({"reference": "TX-9"}))
            .await
            .must("the clerk calling `pay`")?,
    )?;
    let accepted = cat
        .accept(&reviewer, &id)
        .await
        .must("the reviewer accepting the proposal")?;
    let r = read(cat.as_ref(), &owner(store.as_ref()), "invoice", "i").await?;
    ensure!(
        value(&r, "status") == Some(&json!("paid"))
            && value(&r, "paid_ref") == Some(&json!("TX-9")),
        "accepting `pay` must apply it: `i` reads {:?}",
        r.fields
    );
    let events = store
        .feed()
        .poll(at, &FeedFilter::default(), 100)
        .await
        .must("reading the feed")?
        .events;
    let accept_at = events
        .iter()
        .find(|e| matches!(e.body, EventBody::WriteAccepted { .. }))
        .map(|e| (e.position, e.writer.clone()));
    let called = events
        .iter()
        .find(|e| matches!(e.body, EventBody::VerbCalled { .. }));
    let upserted = events
        .iter()
        .find(|e| matches!(e.body, EventBody::RecordUpserted { .. }));
    // The acceptance causes the verb's run, and the run causes its write
    // (the log format: a verb's writes follow, caused by `verb_called`).
    let accepted_at = accept_at.as_ref().map(|(p, _)| *p);
    ensure!(
        accept_at
            .as_ref()
            .is_some_and(|(_, w)| *w == reviewer.writer())
            && called.is_some_and(|c| c.writer == clerk.writer() && c.cause == accepted_at)
            && upserted.is_some_and(
                |u| u.writer == clerk.writer() && u.cause == called.map(|c| c.position)
            )
            && accepted.position() == store.head().await.must("reading the head")?,
        "accepting must log `write_accepted` by the reviewer, then `verb_called` by the \
         proposer caused by exactly that acceptance, then the write caused by exactly \
         that call, and answer the head; it answered {accepted:?}, and the log holds {:?}",
        events
            .iter()
            .map(|e| (e.position, e.body.kind(), e.writer.clone(), e.cause))
            .collect::<Vec<_>>()
    );
    let listed = cat
        .proposals(&reviewer, ProposalFilter::default())
        .await
        .must("listing proposals")?
        .data;
    ensure!(
        listed
            .iter()
            .any(|p| p.id == id && p.state == ProposalState::Accepted),
        "the proposal must now list as accepted; proposals list {listed:?}"
    );
    Ok(())
}

async fn a_rejected_proposal_writes_nothing_and_a_decided_one_stays_decided(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    ledger(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let (_, id) = proposal(
        &call(store.as_ref(), &own, "pay", json!({"reference": "TX-2"}))
            .await
            .must("calling `pay`")?,
    )?;
    let before = store.head().await.must("reading the head")?;
    let rejected = cat
        .reject(&own, &id, "not this month".into())
        .await
        .must("rejecting the proposal")?;
    let logged = kinds_from(store.as_ref(), before.next()).await?;
    ensure!(
        matches!(logged.as_slice(), [EventBody::WriteRejected { .. }])
            && matches!(rejected, Outcome::Applied { version: None, .. }),
        "a rejection must append one `write_rejected` and answer applied with no \
         version; it answered {rejected:?} and appended {:?}",
        logged.iter().map(EventBody::kind).collect::<Vec<_>>()
    );
    let r = read(cat.as_ref(), &own, "invoice", "i").await?;
    ensure!(
        value(&r, "status") == Some(&json!("open")),
        "a rejected `pay` must write nothing; `status` reads {:?}",
        value(&r, "status")
    );
    for what in ["accepting", "rejecting"] {
        let before = store.head().await.must("reading the head")?;
        let result = if what == "accepting" {
            cat.accept(&own, &id).await
        } else {
            cat.reject(&own, &id, "again".into()).await
        };
        let err = result.must_fail(&format!("{what} a rejected proposal"))?;
        head_unmoved(
            store.as_ref(),
            before,
            &format!("{what} a decided proposal"),
        )
        .await?;
        ensure!(
            matches!(
                err,
                Error::Refused {
                    reason: Refusal::Decided,
                    ..
                }
            ),
            "{what} a decided proposal must be refused as decided; it failed with {err:?}"
        );
    }
    Ok(())
}

async fn only_a_reviewer_decides_and_a_proposer_sees_its_own_proposals(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    ledger(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let clerk = field_writer(store.as_ref(), "clerk", "invoice", &["status", "paid_ref"]);
    let other = field_writer(store.as_ref(), "other", "invoice", &["status", "paid_ref"]);
    let (_, mine) = proposal(
        &call(store.as_ref(), &clerk, "pay", json!({"reference": "TX-3"}))
            .await
            .must("the clerk calling `pay`")?,
    )?;
    let (_, theirs) = proposal(
        &cat.propose(
            &own,
            ProposedWrite::Upsert {
                ty: "invoice".into(),
                key: "i".into(),
                patch: patch(json!({"status": "held"})),
                opts: WriteOpts::default(),
            },
            "a hold".into(),
        )
        .await
        .must("the owner proposing an upsert")?,
    )?;

    let before = store.head().await.must("reading the head")?;
    let err = cat
        .accept(&clerk, &mine)
        .await
        .must_fail("the clerk, without the review grant, accepting its own proposal")?;
    head_unmoved(store.as_ref(), before, "accepting without the review grant").await?;
    ensure!(
        matches!(
            err,
            Error::Refused {
                reason: Refusal::NotGranted,
                ..
            }
        ),
        "accepting without the review grant must be refused as not granted; it failed \
         with {err:?}"
    );

    let ids = |who: lakelands_api::Principal| {
        let cat = cat.clone();
        async move {
            cat.proposals(&who, ProposalFilter::default())
                .await
                .must("listing proposals")
                .map(|r| r.data.into_iter().map(|p| p.id).collect::<Vec<_>>())
        }
    };
    let clerk_sees = ids(clerk.clone()).await?;
    let other_sees = ids(other).await?;
    let owner_sees = ids(own).await?;
    ensure!(
        clerk_sees == [mine.clone()]
            && other_sees.is_empty()
            && owner_sees.contains(&mine)
            && owner_sees.contains(&theirs),
        "without the review grant a principal lists only its own proposals, a reviewer \
         all of them: the clerk listed {clerk_sees:?} (own {mine}), another principal \
         {other_sees:?}, the owner {owner_sees:?}"
    );
    Ok(())
}

async fn a_verbs_write_carries_the_verbs_procedure_version(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    ledger(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    // Declared once, so at version 1; the plain write before it has none.
    let before = read(cat.as_ref(), &own, "invoice", "i").await?;
    ensure!(
        before
            .meta
            .get("status")
            .is_some_and(|m| m.procedure_version.is_none()),
        "`status` written by `upsert` must carry no procedure_version; its provenance \
         reads {:?}",
        before.meta.get("status")
    );
    call(store.as_ref(), &own, "approve", json!({}))
        .await
        .must("calling the low-risk `approve`")?;
    let after = read(cat.as_ref(), &own, "invoice", "i").await?;
    let meta = after.meta.get("status");
    ensure!(
        meta.is_some_and(|m| m.procedure_version.as_deref() == Some("1")),
        "`status` set by `approve` (declared once, version 1) must carry \
         procedure_version \"1\"; its provenance reads {meta:?}"
    );
    Ok(())
}
