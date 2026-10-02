//! Verbs by declared risk, and the proposals a riskier write becomes
//! (`docs/decisions/013-verbs-run-by-declared-risk.md`,
//! `docs/decisions/015-errors-are-a-closed-set-writes-answer-an-outcome.md`,
//! `docs/decisions/004-the-change-feed-is-the-log.md`).

use std::sync::Arc;

use lakelands_api::event::EventBody;
use lakelands_api::ontology::StorageKind;
use lakelands_api::seam::FeedFilter;
use lakelands_api::write::{ProposalFilter, ProposalState, ProposedWrite};
use lakelands_api::{Error, LakeStore, Outcome, Position, ProposalId, Refusal, Version, WriteOpts};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::backing::EXECUTOR;
use crate::conformance::fixture::{
    Lake, Must, doors, field, field_writer, head_unmoved, outputs_from, owner, patch, read, reason,
    reviewer, type_decl, value,
};

check!(
    /// A verb runs or waits by its declared risk.
    VERB_BY_RISK = a_low_verb_runs_and_a_higher_one_is_proposed,
    "013: a `low` verb that writes authored fields runs at once; any other risk is recorded as a proposal and runs only when accepted"
);

check!(
    /// A verb that reaches outside needs its executor.
    EXTERNAL_UNBOUND = an_external_verb_without_an_executor_is_unbound,
    "013 and 015: a verb whose effect reaches outside the lake runs only through a bound executor; without one, calling it at any risk or proposing a call of it answers `unbound` naming the executor, and nothing is appended"
);

check!(
    /// A riskier call that could not run now is not proposed.
    NOT_PROPOSED_IF_IT_COULD_NOT_RUN = a_riskier_call_that_could_not_run_now_is_not_proposed,
    "013, extended, and 024, extended: a call that would become a proposal is checked as if it ran now: at a stale version it is a conflict, and where its `requires` does not hold it is refused as a precondition, the version first; nothing is appended"
);

check!(
    /// Accept applies what was checked.
    ACCEPT_AFTER_REDECLARE = accepting_a_call_after_its_verb_was_declared_differently_is_refused,
    "013, extended (lakelands#371): accepting a call whose verb was declared differently after the call was proposed, its effect or its payload changed, is refused as invalid naming the verb and leaves the proposal pending, as a scope that no longer fits does; a verb declared again with the same content, or changed and changed back, is accepted"
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

async fn ledger(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    let ty = type_decl(
        "invoice",
        vec![
            field("status", StorageKind::Text),
            field("paid_ref", StorageKind::Text),
        ],
    );
    l.declare(ty).await?;
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
        l.declare_json(verb).await?;
    }
    l.upsert("invoice", "i", json!({"status": "open"})).await?;
    Ok(l)
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
            Some(&"i".into()),
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
    let l = ledger(store.as_ref()).await?;
    let Lake { own, .. } = &l;
    let ran = call(store.as_ref(), own, "approve", json!({}))
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
    let r = l.read("invoice", "i").await?;
    ensure!(
        value(&r, "status") == Some(&json!("approved")),
        "after `approve` ran, `status` reads {:?}",
        value(&r, "status")
    );

    let waiting = call(store.as_ref(), own, "pay", json!({"reference": "TX-1"}))
        .await
        .must("calling the high-risk `pay`")?;
    let (at, _) = proposal(&waiting)?;
    let head = store.head().await.must("reading the head")?;
    ensure!(
        at == head,
        "the proposal answered position {at}, but the head is {head}"
    );
    let r = l.read("invoice", "i").await?;
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
    let l = ledger(store.as_ref()).await?;
    l.declare_json(
        json!({"declare": "verb", "name": "chase", "on": "invoice", "risk": "high",
                          "effect": "external", "executor": "mailer"}),
    )
    .await?;
    let Lake { own, cat } = &l;
    let before = store.head().await.must("reading the head")?;
    let proposed = |verb: &str| ProposedWrite::Call {
        verb: verb.into(),
        key: Some("i".into()),
        payload: patch(json!({})),
        opts: WriteOpts::default(),
    };
    for (what, answer) in [
        (
            "calling the low-risk `remind`",
            call(store.as_ref(), own, "remind", json!({})).await,
        ),
        (
            "calling the high-risk `chase`, which would be proposed",
            call(store.as_ref(), own, "chase", json!({})).await,
        ),
        (
            "proposing a call of `remind`",
            cat.propose(own, proposed("remind"), "later".into()).await,
        ),
        (
            "proposing a call of `chase`",
            cat.propose(own, proposed("chase"), "later".into()).await,
        ),
    ] {
        ensure!(
            matches!(&answer, Err(Error::Unbound { seam }) if seam.contains("mailer")),
            "{what}, whose executor `mailer` is not bound, must answer `unbound` naming \
             `mailer`, and never record a proposal that could not run; it answered {answer:?}"
        );
    }
    head_unmoved(store.as_ref(), before, "the unbound external verbs").await?;
    Ok(())
}

async fn a_riskier_call_that_could_not_run_now_is_not_proposed(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = ledger(store.as_ref()).await?;
    for verb in [
        json!({"declare": "verb", "name": "settle", "on": "invoice", "risk": "high",
               "requires": "status = 'open'", "effect": "write",
               "set": [{"field": "status", "value": "'settled'"}]}),
        json!({"declare": "verb", "name": "dun", "on": "invoice", "risk": "high",
               "requires": "status = 'open'", "effect": "external", "executor": EXECUTOR}),
    ] {
        l.declare_json(verb).await?;
    }
    l.upsert("invoice", "i", json!({"status": "closed"}))
        .await?;
    let before = store.head().await.must("reading the head")?;
    let mut red = Vec::new();
    for verb in ["settle", "dun"] {
        for (version, proposed) in [
            (None, false),
            (Some(1), false),
            (None, true),
            (Some(1), true),
        ] {
            let opts = WriteOpts {
                expected_version: version.map(Version),
                ..WriteOpts::default()
            };
            let got = if proposed {
                let write = ProposedWrite::Call {
                    verb: verb.into(),
                    key: Some("i".into()),
                    payload: patch(json!({})),
                    opts,
                };
                l.cat.propose(&l.own, write, "later".into()).await
            } else {
                let key = Some("i".into());
                l.cat
                    .call(&l.own, &verb.into(), key.as_ref(), patch(json!({})), opts)
                    .await
            };
            let ok = match (&got, version) {
                (Err(Error::Conflict { expected, actual }), Some(_)) => {
                    *expected == Version(1) && *actual == Version(2)
                }
                (other, None) => matches!(reason(other), Some(Refusal::Precondition { .. })),
                _ => false,
            };
            if !ok {
                let door = if proposed { "propose" } else { "call" };
                red.push(format!("{door} `{verb}` at {version:?} answered {got:?}"));
            }
        }
    }
    ensure!(
        red.is_empty(),
        "`i` is at version 2 and closed, so a high-risk call on it could not run now and is \
         not proposed: at version 1 it is a conflict, the version first; with no version, \
         a precondition: {}",
        red.join("; ")
    );
    head_unmoved(store.as_ref(), before, "calls that could not run").await
}

async fn accepting_a_call_after_its_verb_was_declared_differently_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = ledger(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let review = reviewer(store.as_ref(), false);
    let write = |name: &str, status: &str| {
        json!({"declare": "verb", "name": name, "on": "invoice", "risk": "medium",
               "effect": "write", "set": [{"field": "status", "value": format!("'{status}'")}]})
    };
    let external = |name: &str, scope: &str| {
        json!({"declare": "verb", "name": name, "on": "invoice", "risk": "medium",
               "scope": scope, "effect": "external", "executor": EXECUTOR})
    };
    let c2_again = json!({"declare": "verb", "name": "c2", "on": "invoice", "risk": "medium",
        "payload": [{"name": "reason", "type": {"kind": "text"}, "required": true}],
        "effect": "write", "set": [{"field": "status", "value": "'approved'"},
                                   {"field": "paid_ref", "value": "payload.reason"}]});
    // Each row: the verb as proposed, what it is declared as before the
    // accept, and whether the accept applies.
    let rows = [
        (
            "C1, its effect changed",
            write("c1", "approved"),
            vec![write("c1", "rejected")],
            false,
        ),
        (
            "C2, a required payload field added",
            write("c2", "approved"),
            vec![c2_again],
            false,
        ),
        (
            "C3, its scope turned to the type",
            external("c3", "record"),
            vec![external("c3", "type")],
            false,
        ),
        (
            "nothing declared again",
            write("c4", "approved"),
            vec![],
            true,
        ),
        (
            "declared again, the same",
            write("c5", "approved"),
            vec![write("c5", "approved")],
            true,
        ),
        (
            "changed and changed back",
            write("c6", "approved"),
            vec![write("c6", "rejected"), write("c6", "approved")],
            true,
        ),
    ];
    let mut red = Vec::new();
    for (what, proposed, again, applies) in rows {
        let name = proposed["name"].as_str().expect("a name").to_owned();
        l.declare_json(proposed).await?;
        let (_, id) = proposal(
            &call(store.as_ref(), own, &name, json!({}))
                .await
                .must(what)?,
        )?;
        for d in again {
            l.declare_json(d).await?;
        }
        let head = store.head().await.must("reading the head")?;
        let got = cat.accept(&review, &id).await;
        let state = cat
            .proposals(&review, ProposalFilter::default())
            .await
            .must("listing proposals")?
            .data
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| p.state);
        let ok = if applies {
            got.is_ok() && state == Some(ProposalState::Accepted)
        } else {
            let names = got
                .as_ref()
                .err()
                .is_some_and(|e| e.to_string().contains(&format!("`{name}`")));
            matches!(reason(&got), Some(Refusal::Invalid { .. }))
                && names
                && state == Some(ProposalState::Pending)
                && store.head().await.must("reading the head")? == head
        };
        if !ok {
            red.push(format!(
                "{what}: accept answered {got:?}, the proposal is {state:?}"
            ));
        }
    }
    ensure!(
        red.is_empty(),
        "the reviewer applies what was checked: an accept after the verb was declared \
         differently is refused as invalid naming it, the proposal pending; the same content \
         is accepted: {}",
        red.join("; ")
    );
    Ok(())
}

async fn accepting_a_proposal_applies_its_write_as_the_proposers(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    ledger(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let clerk = field_writer(store.as_ref(), "clerk", "invoice", &["status", "paid_ref"]);
    let reviewer = reviewer(store.as_ref(), false);
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
    let l = ledger(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let (_, id) = proposal(
        &call(store.as_ref(), own, "pay", json!({"reference": "TX-2"}))
            .await
            .must("calling `pay`")?,
    )?;
    let before = store.head().await.must("reading the head")?;
    let rejected = cat
        .reject(own, &id, "not this month".into())
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
    let r = l.read("invoice", "i").await?;
    ensure!(
        value(&r, "status") == Some(&json!("open")),
        "a rejected `pay` must write nothing; `status` reads {:?}",
        value(&r, "status")
    );
    for what in ["accepting", "rejecting"] {
        let before = store.head().await.must("reading the head")?;
        let result = if what == "accepting" {
            cat.accept(own, &id).await
        } else {
            cat.reject(own, &id, "again".into()).await
        };
        let err = result.must_fail(&format!("{what} a rejected proposal"))?;
        head_unmoved(
            store.as_ref(),
            before,
            &format!("{what} a decided proposal"),
        )
        .await?;
        ensure!(
            matches!(reason(&err), Some(Refusal::Decided)),
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
        matches!(reason(&err), Some(Refusal::NotGranted)),
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
    let l = ledger(store.as_ref()).await?;
    let Lake { own, .. } = &l;
    // Declared once, so at version 1; the plain write before it has none.
    let before = l.read("invoice", "i").await?;
    ensure!(
        before
            .meta
            .get("status")
            .is_some_and(|m| m.procedure_version.is_none()),
        "`status` written by `upsert` must carry no procedure_version; its provenance \
         reads {:?}",
        before.meta.get("status")
    );
    call(store.as_ref(), own, "approve", json!({}))
        .await
        .must("calling the low-risk `approve`")?;
    let after = l.read("invoice", "i").await?;
    let meta = after.meta.get("status");
    ensure!(
        meta.is_some_and(|m| m.procedure_version.as_deref() == Some("1")),
        "`status` set by `approve` (declared once, version 1) must carry \
         procedure_version \"1\"; its provenance reads {meta:?}"
    );
    Ok(())
}

check!(
    /// An external verb's output is recorded and the verb runs once; a
    /// retry answers the first call's outcome, its position and output.
    EXECUTOR_RECORDED = an_external_verbs_output_or_failure_is_recorded_and_it_runs_once_per_position,
    "seams, VerbExecutor, and 013: the executor's answer is recorded as output_recorded caused by the event that made the verb due and returned in the outcome's output; a failure is recorded as {\"error\": …} at a real position; a retry of the same call answers the first outcome, its position and output, and like a rebuild runs nothing again"
);

async fn an_external_verbs_output_or_failure_is_recorded_and_it_runs_once_per_position(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    use crate::conformance::backing::{EXECUTOR, executed};
    use lakelands_api::IdempotencyKey;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_decl("job", vec![field("status", StorageKind::Text)]))
        .await?;
    l.declare_json(json!({
        "declare": "verb", "name": "dispatch", "on": "job", "risk": "low",
        "payload": [{"name": "fail", "type": {"kind": "boolean"}}],
        "effect": "external", "executor": EXECUTOR}))
        .await?;
    let key = "executor-job";
    l.upsert("job", key, json!({"status": "open"})).await?;
    let dispatch = |payload: serde_json::Value, opts: WriteOpts| {
        let cat = cat.clone();
        let own = own.clone();
        async move {
            cat.call(
                &own,
                &"dispatch".into(),
                Some(&key.into()),
                patch(payload),
                opts,
            )
            .await
        }
    };

    let once = WriteOpts {
        idempotency_key: Some(IdempotencyKey::from("dispatch-once")),
        ..WriteOpts::default()
    };
    let first = dispatch(json!({}), once.clone())
        .await
        .must("calling `dispatch`")?;
    let Outcome::Applied {
        position,
        output: Some(output),
        ..
    } = &first
    else {
        return Err(Failure::new(format!(
            "an external verb answers `applied` with its executor's output; `dispatch` \
             answered {first:?}"
        )));
    };
    ensure!(
        *output == json!({"done": "dispatch", "record": key}) && executed(key) == 1,
        "the executor answered {{\"done\": \"dispatch\", \"record\": \"{key}\"}} once; the \
         outcome carries {output} after {} runs",
        executed(key)
    );
    let recorded = outputs_from(store.as_ref(), *position).await?;
    ensure!(
        recorded.iter().any(|e| e.cause == Some(*position)
            && matches!(&e.body, lakelands_api::event::EventBody::OutputRecorded { output: o, .. } if o == output)),
        "the executor's answer is recorded as `output_recorded` caused by the event at \
         {position}; the log holds {recorded:?}"
    );

    // A retry answers the first call's outcome, position and output
    // alike, and runs nothing again.
    let again = dispatch(json!({}), once)
        .await
        .must("retrying `dispatch`")?;
    ensure!(
        again == first && executed(key) == 1,
        "a retry with the same idempotency key answers the first outcome, {first:?}, and \
         runs nothing again: it answered {again:?}, and the executor ran {} times",
        executed(key)
    );

    let failed = dispatch(json!({"fail": true}), WriteOpts::default())
        .await
        .must("calling `dispatch` so that its executor fails")?;
    let head = store.head().await.must("reading the head")?;
    ensure!(
        matches!(&failed, Outcome::Applied { position, output: Some(o), .. }
            if *position <= head
                && o.as_object().is_some_and(|m| m.len() == 1 && m["error"].is_string())),
        "an executor that fails does not undo the verb: the outcome is `applied` at a real \
         position with {{\"error\": …}} as its output; it answered {failed:?}"
    );
    let failed_at = failed.position();
    let recorded = outputs_from(store.as_ref(), failed_at).await?;
    ensure!(
        recorded.iter().any(|e| e.cause == Some(failed_at)
            && matches!(&e.body, lakelands_api::event::EventBody::OutputRecorded { output: o, .. }
                if o.as_object().is_some_and(|m| m.len() == 1 && m["error"].is_string()))),
        "the executor's failure is recorded as `output_recorded` {{\"error\": …}}, caused by \
         the event at {failed_at}; the log holds {recorded:?}"
    );

    let runs = executed(key);
    store.rebuild().await.must("rebuilding the projection")?;
    ensure!(
        executed(key) == runs,
        "a rebuild reads the recorded outputs and runs no verb again; the executor ran {} \
         times, {runs} before the rebuild",
        executed(key)
    );
    Ok(())
}

check!(
    /// A retry answers the call's own outcome, not a later write naming
    /// the call as its cause.
    RETRY_EXTERNAL_PAST_HOST = a_retried_external_verb_answers_its_call_though_a_host_write_names_it,
    "013 and 021, extended: a host write naming an external verb's `verb_called` as its cause does not become the call's outcome; a retry with the call's idempotency key answers the verb's position and its recorded output, for a verb on a record and one on its type"
);

check!(
    /// A write verb's retry answers its own write.
    RETRY_WRITE_PAST_HOST = a_retried_write_verb_answers_its_own_write_though_a_host_write_names_it,
    "013 and 021, extended: a retry of a write verb answers the record write the verb itself made and its version, though a later host write names the verb's `verb_called` as its cause"
);

check!(
    /// A retry of a proposed call answers the proposal, after its
    /// acceptance too.
    RETRY_PROPOSED_PAST_ACCEPT = a_retried_proposed_call_answers_its_proposal_after_it_is_accepted,
    "013 and 015: a retry of a call that was proposed answers the proposal, its first outcome, after the proposal was accepted and a host write named the accepted call as its cause"
);

/// A host's write of `job` `key`, naming `cause` as what it follows from.
async fn host_write(store: &dyn LakeStore, key: &str, cause: Position) -> Result<Outcome, Failure> {
    use lakelands_api::principal::{WriteContext, WriteOrigin};
    store
        .catalog(WriteContext {
            origin: WriteOrigin::Reactive,
            depth: 1,
            cause: Some(cause),
        })
        .upsert(
            &owner(store),
            &"job".into(),
            &key.into(),
            patch(json!({"status": "done"})),
            WriteOpts::default(),
        )
        .await
        .must("the host writing the effect's result, naming the call as its cause")
}

/// `job`, and the verbs on it: `ship` and `ship_any` external (the latter
/// on its type) at `risk`, and `close` a write.
async fn jobs(store: &dyn LakeStore, risk: &str) -> Result<Lake, Failure> {
    use crate::conformance::backing::EXECUTOR;
    let l = Lake::of(store);
    l.declare(type_decl("job", vec![field("status", StorageKind::Text)]))
        .await?;
    for verb in [
        json!({"declare": "verb", "name": format!("ship_{risk}"), "on": "job", "risk": risk,
               "effect": "external", "executor": EXECUTOR}),
        json!({"declare": "verb", "name": format!("ship_any_{risk}"), "on": "job",
               "scope": "type", "risk": risk, "effect": "external", "executor": EXECUTOR}),
        json!({"declare": "verb", "name": "close", "on": "job", "risk": "low",
               "effect": "write", "set": [{"field": "status", "value": "'closed'"}]}),
    ] {
        l.declare_json(verb).await?;
    }
    Ok(l)
}

/// Call `verb` on `key` (or on its type) with the idempotency key `once`.
async fn call_once(
    l: &Lake,
    verb: &str,
    key: Option<&str>,
    once: &str,
) -> lakelands_api::Result<Outcome> {
    let key = key.map(lakelands_api::RecordKey::from);
    l.cat
        .call(
            &l.own,
            &verb.into(),
            key.as_ref(),
            patch(json!({})),
            WriteOpts {
                idempotency_key: Some(lakelands_api::IdempotencyKey::from(once)),
                ..WriteOpts::default()
            },
        )
        .await
}

async fn a_retried_external_verb_answers_its_call_though_a_host_write_names_it(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = jobs(store.as_ref(), "low").await?;
    l.upsert("job", "retry-ext", json!({"status": "open"}))
        .await?;
    for (verb, key) in [("ship_low", Some("retry-ext")), ("ship_any_low", None)] {
        let once = format!("{verb}-once");
        let first = call_once(&l, verb, key, &once)
            .await
            .must("calling the external verb")?;
        let Outcome::Applied {
            position,
            output: Some(_),
            ..
        } = &first
        else {
            return Err(Failure::new(format!(
                "`{verb}` answers `applied` with its output; it answered {first:?}"
            )));
        };
        host_write(store.as_ref(), "retry-ext", *position).await?;
        let again = call_once(&l, verb, key, &once)
            .await
            .must("retrying the external verb")?;
        ensure!(
            again == first,
            "a retry of `{verb}` answers the first outcome, {first:?}, though a host write names \
             the call as its cause; it answered {again:?}"
        );
    }
    Ok(())
}

async fn a_retried_write_verb_answers_its_own_write_though_a_host_write_names_it(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = jobs(store.as_ref(), "low").await?;
    l.upsert("job", "retry-write", json!({"status": "open"}))
        .await?;
    let first = call_once(&l, "close", Some("retry-write"), "close-once")
        .await
        .must("calling `close`")?;
    let Outcome::Applied {
        position,
        version: Some(_),
        ..
    } = &first
    else {
        return Err(Failure::new(format!(
            "`close` answers its write with the record's version; it answered {first:?}"
        )));
    };
    let called = store
        .feed()
        .poll(Position(0), &FeedFilter::default(), 10_000)
        .await
        .must("reading the feed")?
        .events
        .into_iter()
        .rev()
        .find(|e| matches!(e.body, EventBody::VerbCalled { .. }) && e.position < *position)
        .map(|e| e.position)
        .ok_or_else(|| Failure::new("`close` appends `verb_called` before its write"))?;
    host_write(store.as_ref(), "retry-write", called).await?;
    let again = call_once(&l, "close", Some("retry-write"), "close-once")
        .await
        .must("retrying `close`")?;
    ensure!(
        again == first,
        "a retry of `close` answers its own write, {first:?}, though a later host write names \
         its `verb_called` at {called} as cause; it answered {again:?}"
    );
    Ok(())
}

async fn a_retried_proposed_call_answers_its_proposal_after_it_is_accepted(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = jobs(store.as_ref(), "medium").await?;
    l.upsert("job", "retry-prop", json!({"status": "open"}))
        .await?;
    let first = call_once(&l, "ship_medium", Some("retry-prop"), "ship-medium-once")
        .await
        .must("calling `ship_medium`")?;
    let (at, id) = proposal(&first)?;
    let reviewer = reviewer(store.as_ref(), false);
    l.cat
        .accept(&reviewer, &id)
        .await
        .must("the reviewer accepting the proposal")?;
    let called = kinds_from(store.as_ref(), at)
        .await?
        .iter()
        .position(|b| matches!(b, EventBody::VerbCalled { .. }))
        .map(|i| Position(at.0 + i as u64))
        .ok_or_else(|| Failure::new("accepting the proposal appends `verb_called`"))?;
    host_write(store.as_ref(), "retry-prop", called).await?;
    let again = call_once(&l, "ship_medium", Some("retry-prop"), "ship-medium-once")
        .await
        .must("retrying `ship_medium`")?;
    ensure!(
        again == first,
        "a retry of a proposed call answers the proposal, {first:?}, after its acceptance and a \
         host write naming the accepted call; it answered {again:?}"
    );
    Ok(())
}
