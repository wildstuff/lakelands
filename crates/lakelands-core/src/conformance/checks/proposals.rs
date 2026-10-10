//! Verbs by declared risk, and the proposals a riskier write becomes
//! (`docs/decisions/013-verbs-run-by-declared-risk.md`,
//! `docs/decisions/015-errors-are-a-closed-set-writes-answer-an-outcome.md`,
//! `docs/decisions/004-the-change-feed-is-the-log.md`).

use lakelands_api::error::{Case, Seam};
use lakelands_api::event::EventBody;
use lakelands_api::ontology::StorageKind;
use lakelands_api::write::{Proposal, ProposalFilter, ProposalState, ProposedWrite};
use lakelands_api::{
    Error, LakeStore, Missing, Outcome, Position, Principal, ProposalId, RecordKey, Refusal,
    Version, WriteOpts,
};
use serde_json::{Value, json};

use super::only::{alice, claim_with, claims, de, holder};
use crate::conformance::backing::EXECUTOR;
use crate::conformance::fixture::{
    Lake, Must, doors, events_from, field, field_writer, head_unmoved, invalid_case, log_events,
    outputs_from, owner, patch, reactive, read, reason, refused_on, reviewer, try_upsert,
    type_decl, value,
};
use crate::conformance::{Failure, Held, Store};

check!(
    VERB_BY_RISK = a_low_verb_runs_and_a_higher_one_is_proposed,
    "013: a `low` verb that writes authored fields runs at once; any other risk is recorded as a proposal and runs only when accepted; a call is refused for its payload before its record, for its record before its version, and a retry answers before the grants (013, extended); a payload member the verb does not declare, even a field of its type, is refused naming the verb and the member, not as a field the type lacks (lakelands#656)"
);

check!(
    EXTERNAL_UNBOUND = an_external_verb_without_an_executor_is_unbound,
    "013 and 015: a verb whose effect reaches outside the lake runs only through a bound executor; `describe` names the executor and says beforehand that it is not bound; without one, calling it at any risk or proposing a call of it answers `unbound` naming the executor, after the grant and before the payload (013, extended), and nothing is appended"
);

check!(
    NOT_PROPOSED_IF_IT_COULD_NOT_RUN = a_riskier_call_that_could_not_run_now_is_not_proposed,
    "013, extended, and 024, extended: a call that would become a proposal is checked as if it ran now: at a stale version it is a conflict, and where its `requires` does not hold it is refused as a precondition, the version first; nothing is appended"
);

check!(
    ACCEPT_AFTER_REDECLARE = accepting_a_call_after_its_verb_was_declared_differently_is_refused,
    "013, extended (lakelands#371): accepting a call whose verb was declared differently after the call was proposed, its effect or its payload changed, is refused as invalid naming the verb and leaves the proposal pending, as a scope that no longer fits does; a verb declared again with the same content, or changed and changed back, is accepted"
);

check!(
    ACCEPT_APPLIES = accepting_a_proposal_applies_its_write_as_the_proposers,
    "013 and 004: a proposal is a `write_proposed` event; accepting it appends `write_accepted`, then the verb's run caused by that acceptance and its write caused by that run, written as the proposer"
);

check!(
    REJECT_AND_DECIDED = a_rejected_proposal_writes_nothing_and_a_decided_one_stays_decided,
    "013: rejecting appends `write_rejected` and nothing else; a decided proposal is refused as decided"
);

check!(
    WHO_DECIDES = only_a_reviewer_decides_and_a_proposer_sees_its_own_proposals,
    "006, 013 and Catalog::proposals: accept and reject need the review grant, checked before the proposal is looked up (013, extended); without it, `proposals` lists only the principal's own"
);

check!(
    NOT_SHOWN_NOT_DECIDED = a_proposal_on_a_type_its_reviewer_is_not_shown_is_not_there,
    "006 and 013, extended (lakelands#762): to a reviewer not shown a type a proposal names, or for a link type either end type, the proposal is not there: `proposals` does not list it, and `accept` and `reject` answer as for a proposal id that does not exist, word for word, before anything else of it is told: an upsert, a call on a link, a pinned one whose hidden end moved since, a migration, a bundle that redeclares the type, puts a verb on it, or relates or refers a new type to it, and a migration that drops a relation to it, pending or decided; a restatement of a type related to it through a computed link field, as a bundle and as a migration, is listed and decided; nothing is appended; every other proposal is listed as to a reviewer shown everything, and decided; a reviewer shown the type decides it"
);

check!(
    TOLD_AS_LISTED = an_accepts_refusal_names_what_its_reviewers_listing_shows,
    "006 and 013, extended (lakelands#780): a refusal of the proposer's write at its accept is judged by the reviewer's view, as its listing is: whole to a reviewer shown each field, type, key and value it names, and to one kept from any of them as a write changed since it was proposed, naming only the type and key written: a reference to a record retired since, a value its field no longer takes; a call whose `requires` no longer holds is whole to a reviewer who reads what it reads, and not there to one kept from it (lakelands#779); a conflict names versions only and is whole to each; nothing is appended and the proposal stays pending"
);

check!(
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
    let events = events_from(store, from).await?;
    Ok(events.into_iter().map(|e| e.body).collect())
}

async fn a_low_verb_runs_and_a_higher_one_is_proposed(store: Store) -> Held {
    let l = ledger(store.as_ref()).await?;
    let Lake { own, .. } = &l;
    let ran = call(store.as_ref(), own, "approve", json!({}));
    let ran = ran.await.must("calling the low-risk `approve`")?;
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

    let waiting = call(store.as_ref(), own, "pay", json!({"reference": "TX-1"}));
    let waiting = waiting.await.must("calling the high-risk `pay`")?;
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
    // In call's order (013, extended): the payload before the record, the
    // record before the version, and a retry before the grants.
    let approve = |who: &lakelands_api::Principal, key: &str, payload, opts| {
        let (cat, who, key) = (
            l.cat.clone(),
            who.clone(),
            lakelands_api::RecordKey::from(key),
        );
        async move {
            cat.call(&who, &"approve".into(), Some(&key), patch(payload), opts)
                .await
        }
    };
    let at = WriteOpts {
        expected_version: Some(Version(3)),
        ..WriteOpts::default()
    };
    // `invoice` has `status`; `approve`'s payload does not.
    let payload = approve(own, "gone", json!({"status": 1}), WriteOpts::default()).await;
    let record = approve(own, "gone", json!({}), at).await;
    let once = WriteOpts {
        idempotency_key: Some("once".into()),
        ..WriteOpts::default()
    };
    let first = approve(own, "i", json!({}), once.clone());
    let first = first.await.must("approving `i` once")?;
    let bare = lakelands_api::Principal {
        grants: Default::default(),
        ..own.clone()
    };
    let retry = approve(&bare, "i", json!({}), once).await;
    ensure!(
        matches!(
            refused_on(&payload),
            Some(("status", Refusal::Invalid(said)))
                if matches!(&said.case, Case::PayloadUndeclared { verb, member }
                    if verb == "approve" && member == "status")
        ) && matches!(record, Err(Error::NotFound { .. }))
            && retry.as_ref().ok() == Some(&first),
        "a payload member `approve` does not declare, on a key never written, answered \
         {payload:?}; a call there at a version answered {record:?}; a retry without \
         grants answered {retry:?} where the first was {first:?}"
    );
    Ok(())
}

async fn an_external_verb_without_an_executor_is_unbound(store: Store) -> Held {
    let l = ledger(store.as_ref()).await?;
    l.declare_json(
        json!({"declare": "verb", "name": "chase", "on": "invoice", "risk": "high",
                          "effect": "external", "executor": "mailer"}),
    )
    .await?;
    for v in ["remind", "chase"] {
        let said = l.executor_of(v).await?;
        ensure!(
            said == Some(("mailer".into(), false)),
            "`describe` says before any call that `{v}`'s executor `mailer` is not bound; it \
             said {said:?}"
        );
    }
    let Lake { own, cat } = &l;
    let before = store.head().await.must("reading the head")?;
    let proposed = |verb: &str, payload| ProposedWrite::Call {
        verb: verb.into(),
        key: Some("i".into()),
        payload: patch(payload),
        opts: WriteOpts::default(),
    };
    let propose = |verb: &str, payload| cat.propose(own, proposed(verb, payload), "later".into());
    let reader = crate::conformance::fixture::reads_all(store.as_ref(), "reader");
    let ungranted = call(store.as_ref(), &reader, "remind", json!({})).await;
    ensure!(
        matches!(reason(&ungranted), Some(Refusal::NotGranted)),
        "the grant comes before the executor: `remind` called by one who only reads answered \
         {ungranted:?}"
    );
    for (what, answer) in [
        (
            "calling the low-risk `remind`",
            call(store.as_ref(), own, "remind", json!({})).await,
        ),
        (
            "calling `remind` with a payload it does not take, judged after the executor",
            call(store.as_ref(), own, "remind", json!({"bogus": 1})).await,
        ),
        (
            "calling the high-risk `chase`, which would be proposed",
            call(store.as_ref(), own, "chase", json!({})).await,
        ),
        (
            "proposing a call of `remind`",
            propose("remind", json!({})).await,
        ),
        (
            "proposing `remind` with a payload it does not take",
            propose("remind", json!({"bogus": 1})).await,
        ),
        (
            "proposing a call of `chase`",
            propose("chase", json!({})).await,
        ),
    ] {
        ensure!(
            matches!(answer.as_ref().err().and_then(Error::seam), Some(Seam::Executor { name }) if name == "mailer"),
            "{what}, whose executor `mailer` is not bound, must answer `unbound` naming \
             `mailer`, and never record a proposal that could not run; it answered {answer:?}"
        );
    }
    head_unmoved(store.as_ref(), before, "the unbound external verbs").await?;
    Ok(())
}

async fn a_riskier_call_that_could_not_run_now_is_not_proposed(store: Store) -> Held {
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
                (other, None) => matches!(reason(other), Some(Refusal::Precondition(..))),
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

async fn accepting_a_call_after_its_verb_was_declared_differently_is_refused(store: Store) -> Held {
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
            // A verb turned to its type takes no key, whatever its declaration.
            let names = matches!(invalid_case(&got),
                Some(Case::VerbRedeclared { verb } | Case::VerbTypeTakesNoKey { verb, .. })
                    if verb == name.as_str());
            names
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

async fn accepting_a_proposal_applies_its_write_as_the_proposers(store: Store) -> Held {
    ledger(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let clerk = field_writer(store.as_ref(), "clerk", "invoice", &["status", "paid_ref"]);
    let reviewer = reviewer(store.as_ref(), false);
    let (at, id) = proposal(
        &call(store.as_ref(), &clerk, "pay", json!({"reference": "TX-9"}))
            .await
            .must("the clerk calling `pay`")?,
    )?;
    let accepted = cat.accept(&reviewer, &id);
    let accepted = accepted.await.must("the reviewer accepting the proposal")?;
    let r = read(cat.as_ref(), &owner(store.as_ref()), "invoice", "i").await?;
    ensure!(
        value(&r, "status") == Some(&json!("paid"))
            && value(&r, "paid_ref") == Some(&json!("TX-9")),
        "accepting `pay` must apply it: `i` reads {:?}",
        r.fields
    );
    let events = events_from(store.as_ref(), at).await?;
    let find = |is: fn(&EventBody) -> bool| events.iter().find(|e| is(&e.body));
    let accept_at = find(|b| matches!(b, EventBody::WriteAccepted { .. }))
        .map(|e| (e.position, e.writer.clone()));
    let called = find(|b| matches!(b, EventBody::VerbCalled { .. }));
    let upserted = find(|b| matches!(b, EventBody::RecordUpserted { .. }));
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
    let listed = cat.proposals(&reviewer, ProposalFilter::default());
    let listed = listed.await.must("listing proposals")?.data;
    ensure!(
        listed
            .iter()
            .any(|p| p.id == id && p.state == ProposalState::Accepted),
        "the proposal must now list as accepted; proposals list {listed:?}"
    );
    Ok(())
}

async fn a_rejected_proposal_writes_nothing_and_a_decided_one_stays_decided(store: Store) -> Held {
    let l = ledger(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let (_, id) = proposal(
        &call(store.as_ref(), own, "pay", json!({"reference": "TX-2"}))
            .await
            .must("calling `pay`")?,
    )?;
    let before = store.head().await.must("reading the head")?;
    let rejected = cat.reject(own, &id, "not this month".into());
    let rejected = rejected.await.must("rejecting the proposal")?;
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
        let decided = format!("{what} a decided proposal");
        head_unmoved(store.as_ref(), before, &decided).await?;
        ensure!(
            matches!(reason(&err), Some(Refusal::Decided)),
            "{what} a decided proposal must be refused as decided; it failed with {err:?}"
        );
    }
    Ok(())
}

async fn only_a_reviewer_decides_and_a_proposer_sees_its_own_proposals(store: Store) -> Held {
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
    // The review grant before the proposal is looked up (013, extended).
    let unknown = ProposalId::from("none");
    let accept = cat.accept(&clerk, &unknown).await;
    let reject = cat.reject(&clerk, &unknown, "no".into()).await;
    ensure!(
        matches!(reason(&accept), Some(Refusal::NotGranted))
            && matches!(reason(&reject), Some(Refusal::NotGranted)),
        "deciding a proposal that does not exist, without the review grant, answers \
         {accept:?} and {reject:?}"
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

async fn a_proposal_on_a_type_its_reviewer_is_not_shown_is_not_there(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    claims(&l, false).await?;
    // Kept from `trip`, so from `claim`'s link type over `on_trip` too.
    let kept = holder(
        s,
        "kept",
        json!({"default": "read", "review": true, "schema": true,
               "types": {"trip": {"default": "none"}}}),
    );
    let on_trip = crate::links::link_type_name(&"claim".into(), &"on_trip".into());
    let set = json!([{"field": "label", "value": "'x'"}]);
    l.declare_json(
        json!({"declare": "verb", "name": "relabel", "on": on_trip.as_str(),
                          "risk": "high", "effect": "write", "set": set}),
    )
    .await?;
    let link = crate::links::link_key(&"c1".into(), &"t1".into());
    let relabel = async |who: &Principal| {
        let (verb, none, opts) = ("relabel".into(), Default::default(), Default::default());
        let called = l.cat.call(who, &verb, Some(&link), none, opts).await;
        proposal(&called.must("a `relabel` of `c1:t1`")?).map(|(_, id)| id)
    };
    let propose = async |write: Value| {
        let proposed = l.cat.propose(&l.own, de(write), "the owner's".into()).await;
        proposal(&proposed.must("the owner's proposal")?).map(|(_, id)| id)
    };
    let upsert = propose(json!({"write": "upsert", "type": "trip", "key": "t1",
                                "patch": {"name": "paris"}}))
    .await?;
    let call = relabel(&l.own).await?;
    // Hers is pinned: once `t1`, its hidden end, moves, its accept would
    // name the link type and its key.
    let pinned = relabel(&alice(s, json!({"default": "write"}))).await?;
    let text = StorageKind::Text;
    let wider = type_decl("trip", vec![field("name", text), field("extra", text)]);
    let migrate = propose(json!({"write": "migrate", "migration": {"to": wider}})).await?;
    // `claim` without its relation to `trip`: a relation dropped.
    let mut unlinked = claim_with("owner");
    unlinked.relations.clear();
    let dropping = propose(json!({"write": "migrate", "migration": {"to": unlinked}})).await?;
    // Bundles: `trip` redeclared, a verb on it, a new type related to it.
    let declare = async |d: Value| propose(json!({"write": "declare", "declarations": [d]})).await;
    let fields = |kind: &str| {
        json!([{"name": "name", "type": {"kind": "text"}},
               {"name": "extra3", "type": {"kind": kind}}])
    };
    let redeclared =
        declare(json!({"declare": "type", "name": "trip", "fields": fields("text")})).await?;
    let verb = declare(
        json!({"declare": "verb", "name": "rename", "on": "trip", "risk": "low",
                              "effect": "write", "set": [{"field": "name", "value": "'x'"}]}),
    )
    .await?;
    let related = declare(json!({"declare": "type", "name": "leg",
                                 "fields": [{"name": "n", "type": {"kind": "text"}}],
                                 "relations": [{"name": "of_trip", "target": "trip"}]}))
    .await?;
    let referring = declare(json!({"declare": "type", "name": "stop",
                                   "fields": [{"name": "t", "type": {"kind": "reference",
                                                                     "target": "trip"}}]}))
    .await?;
    // The redeclaration no longer fits: its refusal would name `extra3`.
    l.declare_json(json!({"declare": "type", "name": "trip", "fields": fields("number")}))
        .await?;
    l.upsert("trip", "t1", json!({"name": "rome"})).await?;
    let decided = propose(json!({"write": "upsert", "type": "trip", "key": "t1",
                                 "patch": {"name": "oslo"}}))
    .await?;
    let accepted = l.cat.accept(&l.own, &decided).await;
    accepted.must("the owner accepting an upsert of `t1`")?;
    // `fare` relates to `trip` through a link field and one computed from
    // it: restated as it stands, with a field added, it names no `trip`,
    // the computed field's `depends_on` as the lake keeps it or not.
    let link = json!([{"name": "amt", "type": {"kind": "number"}},
                      {"name": "big", "type": {"kind": "boolean"}, "authority": "derived",
                       "derived": "amt > 10"}]);
    let fare = |more: Option<&str>| {
        let mut fields = vec![json!({"name": "n", "type": {"kind": "text"}})];
        fields.extend(more.map(|m| json!({"name": m, "type": {"kind": "text"}})));
        json!({"name": "fare", "fields": fields,
               "relations": [{"name": "of_trip", "target": "trip", "fields": link}]})
    };
    let mut declared = fare(None);
    declared["declare"] = json!("type");
    l.declare_json(declared).await?;
    let mut again = fare(Some("extra"));
    again["declare"] = json!("type");
    let restating = [
        declare(again).await?,
        propose(json!({"write": "migrate", "migration": {"to": fare(Some("more"))}})).await?,
    ];
    let other = propose(json!({"write": "upsert", "type": "claim", "key": "c3",
                               "patch": {"note": "late"}}))
    .await?;
    // Its listing is the one of a lake without those proposals: every
    // entry a reviewer shown everything lists but theirs, each whole.
    let shown = reviewer(s, true);
    let hidden = [
        &upsert,
        &call,
        &pinned,
        &migrate,
        &redeclared,
        &verb,
        &related,
        &referring,
        &dropping,
        &decided,
    ];
    let listed = async |who: &Principal| {
        let all = l.cat.proposals(who, ProposalFilter::default()).await;
        Ok::<_, Failure>(all.must("listing proposals")?.data)
    };
    let mut want = listed(&shown).await?;
    want.retain(|p| !hidden.contains(&&p.id));
    let got = listed(&kept).await?;
    // The same entries; each whole but `fare` restated, whose relation to
    // `trip` is trimmed from it, as a relation not shown is (lakelands#662).
    let ids = |l: &[Proposal]| l.iter().map(|p| p.id.clone()).collect::<Vec<_>>();
    let whole = |l: &[Proposal]| {
        let whole = l.iter().filter(|p| !restating.contains(&p.id));
        whole.map(|p| json!(p)).collect::<Vec<_>>()
    };
    let trimmed = (got.iter().filter(|p| restating.contains(&p.id)))
        .all(|p| p.more && !json!(p.write).to_string().contains("\"trip\""));
    ensure!(
        ids(&got) == ids(&want)
            && whole(&got) == whole(&want)
            && trimmed
            && [&other, &restating[0], &restating[1]]
                .iter()
                .all(|id| want.iter().any(|p| &p.id == *id)),
        "a reviewer not shown `trip` lists no proposal on it, and every other as one shown \
         everything does, `fare` restated without its relation to `trip`: it lists {}, where \
         {} is due",
        json!(got),
        json!(want)
    );
    let absent = |id: &ProposalId| -> lakelands_api::Result<()> {
        let missing = Missing::Proposal { id: id.clone() };
        Err(Error::NotFound { missing })
    };
    let none = ProposalId::from("none");
    let mut wrong = Vec::new();
    let before = store.head().await.must("reading the head")?;
    for (what, id) in [
        ("an id that does not exist", &none),
        ("an upsert of `t1`", &upsert),
        ("a call on the link `c1:t1`", &call),
        ("a pinned call on `c1:t1`, `t1` moved since", &pinned),
        ("a migration of `trip`", &migrate),
        (
            "a bundle redeclaring `trip`, which no longer fits",
            &redeclared,
        ),
        ("a bundle declaring a verb on `trip`", &verb),
        ("a bundle declaring a type related to `trip`", &related),
        (
            "a bundle declaring a type with a reference to `trip`",
            &referring,
        ),
        (
            "a migration of `claim` dropping its relation to `trip`",
            &dropping,
        ),
        ("a decided upsert of `t1`", &decided),
    ] {
        let accept = l.cat.accept(&kept, id).await.map(|_| ());
        let reject = l.cat.reject(&kept, id, "no".into()).await.map(|_| ());
        for (door, said) in [("accept", accept), ("reject", reject)] {
            if said != absent(id) {
                wrong.push(format!("{door} of {what} answered {said:?}"));
            }
        }
    }
    ensure!(
        wrong.is_empty(),
        "to a reviewer not shown `trip`, a proposal on it or on a link to it is one that does \
         not exist: {}",
        wrong.join("; ")
    );
    head_unmoved(s, before, "deciding a proposal on a hidden type").await?;
    let rejected = l.cat.reject(&kept, &other, "no".into()).await;
    rejected.must("the reviewer not shown `trip` rejecting an upsert of `claim`")?;
    for id in &restating {
        let rejected = l.cat.reject(&kept, id, "no".into()).await;
        rejected.must("the reviewer not shown `trip` rejecting `fare` restated")?;
    }
    let accepted = l.cat.accept(&shown, &call).await;
    accepted.must("a reviewer shown `trip` accepting the call")?;
    let rejected = l.cat.reject(&shown, &upsert, "no".into()).await;
    rejected.must("a reviewer shown `trip` rejecting the upsert")?;
    Ok(())
}

async fn a_verbs_write_carries_the_verbs_procedure_version(store: Store) -> Held {
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
    let fut = call(store.as_ref(), own, "approve", json!({}));
    fut.await.must("calling the low-risk `approve`")?;
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
    EXECUTOR_RECORDED =
        an_external_verbs_output_or_failure_is_recorded_and_it_runs_once_per_position,
    "seams, VerbExecutor, and 013: `describe` says the bound executor is bound; the executor's answer is recorded as output_recorded caused by the event that made the verb due and returned in the outcome's output; a failure is recorded as {\"error\": …} at a real position; a retry of the same call answers the first outcome, its position and output, `null` included, and like a rebuild runs nothing again"
);

async fn an_external_verbs_output_or_failure_is_recorded_and_it_runs_once_per_position(
    store: Store,
) -> Held {
    use crate::conformance::backing::executed;
    use lakelands_api::IdempotencyKey;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_decl("job", vec![field("status", StorageKind::Text)]))
        .await?;
    l.declare_json(json!({
        "declare": "verb", "name": "dispatch", "on": "job", "risk": "low",
        "payload": [{"name": "fail", "type": {"kind": "boolean"}},
                    {"name": "null", "type": {"kind": "boolean"}}],
        "effect": "external", "executor": EXECUTOR}))
        .await?;
    let said = l.executor_of("dispatch").await?;
    ensure!(
        said == Some((EXECUTOR.into(), true)),
        "`describe` says `dispatch`'s executor `{EXECUTOR}`, which the runner binds, is \
         bound; it said {said:?}"
    );
    let key = "executor-job";
    l.upsert("job", key, json!({"status": "open"})).await?;
    let dispatch = |payload: serde_json::Value, opts: WriteOpts| {
        let (cat, own) = (cat.clone(), own.clone());
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
    let first = dispatch(json!({}), once.clone());
    let first = first.await.must("calling `dispatch`")?;
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
            && matches!(&e.body, lakelands_api::event::EventBody::OutputRecorded { output: Some(o), .. } if o == output)),
        "the executor's answer is recorded as `output_recorded` caused by the event at \
         {position}; the log holds {recorded:?}"
    );

    // A retry answers the first call's outcome, position and output
    // alike, and runs nothing again.
    let again = dispatch(json!({}), once);
    let again = again.await.must("retrying `dispatch`")?;
    ensure!(
        again == first && executed(key) == 1,
        "a retry with the same idempotency key answers the first outcome, {first:?}, and \
         runs nothing again: it answered {again:?}, and the executor ran {} times",
        executed(key)
    );

    // An answer of `null` is an answer: a retry answers it.
    let null = WriteOpts {
        idempotency_key: Some(IdempotencyKey::from("dispatch-null")),
        ..WriteOpts::default()
    };
    let first = dispatch(json!({"null": true}), null.clone()).await;
    let again = dispatch(json!({"null": true}), null).await;
    ensure!(
        matches!(&first, Ok(Outcome::Applied { output: Some(o), .. }) if o.is_null())
            && again == first
            && executed(key) == 2,
        "an executor that answered `null` is answered: the call and its retry answer \
         `applied` with that output, run once; they answered {first:?} and {again:?}"
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
            && matches!(&e.body, lakelands_api::event::EventBody::OutputRecorded { output: Some(o), .. }
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
    RETRY_EXTERNAL_PAST_HOST =
        a_retried_external_verb_answers_its_call_though_a_host_write_names_it,
    "013 and 021, extended: a host write naming an external verb's `verb_called` as its cause does not become the call's outcome; a retry with the call's idempotency key answers the verb's position and its recorded output, for a verb on a record and one on its type"
);

check!(
    RETRY_WRITE_PAST_HOST = a_retried_write_verb_answers_its_own_write_though_a_host_write_names_it,
    "013 and 021, extended: a retry of a write verb answers the record write the verb itself made and its version, though a later host write names the verb's `verb_called` as its cause"
);

check!(
    RETRY_PROPOSED_PAST_ACCEPT = a_retried_proposed_call_answers_its_proposal_after_it_is_accepted,
    "013 and 015: a retry of a call that was proposed answers the proposal, its first outcome, after the proposal was accepted and a host write named the accepted call as its cause"
);

check!(
    ACCEPT_AGAIN = an_accept_sent_again_answers_by_the_verb_as_it_was_accepted,
    "013, extended (lakelands#616): an accept sent again for a call of an external verb answers the first acceptance, its recorded output included, and runs nothing, whatever the verb is declared as since; for a write verb it is `decided`, though the verb is declared external since"
);

check!(
    UNANSWERED = a_call_sent_again_while_its_executor_runs_is_unanswered,
    "013, extended (lakelands#616): while the executor of an external call still runs, the call sent again under its key, through `call`, `upsert`, `link`, `unlink` or `propose`, runs nothing and is refused as `unanswered`, naming the call's position, and so is the accept sent again while the executor of an accepted call runs; once the executor has answered, each answers the first outcome, its output included"
);

/// A host's write of `job` `key`, naming `cause` as what it follows from.
async fn host_write(store: &dyn LakeStore, key: &str, cause: Position) -> Result<Outcome, Failure> {
    let host = store.catalog(reactive(1, Some(cause)));
    let done = json!({"status": "done"});
    try_upsert(host.as_ref(), &owner(store), "job", key, done)
        .await
        .must("the host writing the effect's result, naming the call as its cause")
}

/// `job`, and the verbs on it: `ship` and `ship_any` external (the latter
/// on its type) at `risk`, and `close` a write; then its record `key`, open.
async fn jobs(store: &dyn LakeStore, risk: &str, key: &str) -> Result<Lake, Failure> {
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
    l.upsert("job", key, json!({"status": "open"})).await?;
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
    let opts = WriteOpts {
        idempotency_key: Some(once.into()),
        ..WriteOpts::default()
    };
    l.cat
        .call(&l.own, &verb.into(), key.as_ref(), patch(json!({})), opts)
        .await
}

async fn a_retried_external_verb_answers_its_call_though_a_host_write_names_it(
    store: Store,
) -> Held {
    let l = jobs(store.as_ref(), "low", "retry-ext").await?;
    for (verb, key) in [("ship_low", Some("retry-ext")), ("ship_any_low", None)] {
        let once = format!("{verb}-once");
        let first = call_once(&l, verb, key, &once);
        let first = first.await.must("calling the external verb")?;
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
        let again = call_once(&l, verb, key, &once);
        let again = again.await.must("retrying the external verb")?;
        ensure!(
            again == first,
            "a retry of `{verb}` answers the first outcome, {first:?}, though a host write names \
             the call as its cause; it answered {again:?}"
        );
    }
    Ok(())
}

async fn a_retried_write_verb_answers_its_own_write_though_a_host_write_names_it(
    store: Store,
) -> Held {
    let l = jobs(store.as_ref(), "low", "retry-write").await?;
    let first = call_once(&l, "close", Some("retry-write"), "close-once");
    let first = first.await.must("calling `close`")?;
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
    let called = log_events(store.as_ref())
        .await?
        .into_iter()
        .rev()
        .find(|e| matches!(e.body, EventBody::VerbCalled { .. }) && e.position < *position)
        .map(|e| e.position)
        .ok_or_else(|| Failure::new("`close` appends `verb_called` before its write"))?;
    host_write(store.as_ref(), "retry-write", called).await?;
    let again = call_once(&l, "close", Some("retry-write"), "close-once");
    let again = again.await.must("retrying `close`")?;
    ensure!(
        again == first,
        "a retry of `close` answers its own write, {first:?}, though a later host write names \
         its `verb_called` at {called} as cause; it answered {again:?}"
    );
    Ok(())
}

async fn a_retried_proposed_call_answers_its_proposal_after_it_is_accepted(store: Store) -> Held {
    let l = jobs(store.as_ref(), "medium", "retry-prop").await?;
    let first = call_once(&l, "ship_medium", Some("retry-prop"), "ship-medium-once");
    let first = first.await.must("calling `ship_medium`")?;
    let (at, id) = proposal(&first)?;
    let reviewer = reviewer(store.as_ref(), false);
    let fut = l.cat.accept(&reviewer, &id);
    fut.await.must("the reviewer accepting the proposal")?;
    let called = kinds_from(store.as_ref(), at)
        .await?
        .iter()
        .position(|b| matches!(b, EventBody::VerbCalled { .. }))
        .map(|i| Position(at.0 + i as u64))
        .ok_or_else(|| Failure::new("accepting the proposal appends `verb_called`"))?;
    host_write(store.as_ref(), "retry-prop", called).await?;
    let again = call_once(&l, "ship_medium", Some("retry-prop"), "ship-medium-once");
    let again = again.await.must("retrying `ship_medium`")?;
    ensure!(
        again == first,
        "a retry of a proposed call answers the proposal, {first:?}, after its acceptance and a \
         host write naming the accepted call; it answered {again:?}"
    );
    Ok(())
}

async fn an_accept_sent_again_answers_by_the_verb_as_it_was_accepted(store: Store) -> Held {
    use crate::conformance::backing::executed;
    let l = jobs(store.as_ref(), "medium", "again-ext").await?;
    let verb = |name: &str, effect: serde_json::Value| {
        let mut verb = json!({"declare": "verb", "name": name, "on": "job", "risk": "medium"});
        if let (Some(v), Some(e)) = (verb.as_object_mut(), effect.as_object()) {
            v.extend(e.clone());
        }
        verb
    };
    let write = json!({"effect": "write", "set": [{"field": "status", "value": "'held'"}]});
    let external = json!({"effect": "external", "executor": EXECUTOR});
    l.declare_json(verb("hold", write.clone())).await?;
    l.upsert("job", "again-write", json!({"status": "open"}))
        .await?;
    let review = reviewer(store.as_ref(), false);
    // Each row: the verb, its record, and what it is declared as after
    // the acceptance.
    for (name, key, effect) in [
        ("ship_medium", "again-ext", write),
        ("hold", "again-write", external),
    ] {
        let first = call_once(&l, name, Some(key), &format!("{name}-again")).await;
        let (_, id) = proposal(&first.must(name)?)?;
        let first = l.cat.accept(&review, &id).await.must("accepting")?;
        let ran = executed(key);
        l.declare_json(verb(name, effect)).await?;
        let again = l.cat.accept(&review, &id).await;
        let ok = match &first {
            Outcome::Applied {
                output: Some(_), ..
            } => again.as_ref() == Ok(&first),
            _ => matches!(reason(&again), Some(Refusal::Decided)),
        };
        ensure!(
            ok && executed(key) == ran && executed("again-ext") == 1,
            "an accept sent again answers the first acceptance of an external call, its output \
             included, and `decided` for a write verb, by `{name}` as it was accepted, and runs \
             nothing: accepted {first:?}, sent again {again:?}, run {} times",
            executed(key)
        );
    }
    Ok(())
}

/// The answers of one caller of a check: what it sent, and the answer.
type Tries<'a> = Vec<(&'a str, lakelands_api::Result<Outcome>)>;

async fn a_call_sent_again_while_its_executor_runs_is_unanswered(store: Store) -> Held {
    use crate::conformance::backing::{executed, release};
    use crate::conformance::fixture::{Caller, join_all, pause};
    let l = Lake::of(store.as_ref());
    l.declare_json(json!({"declare": "type", "name": "held",
        "fields": [{"name": "n", "type": {"kind": "text"}}],
        "relations": [{"name": "next", "target": "held"}]}))
        .await?;
    for risk in ["low", "medium"] {
        l.declare_json(
            json!({"declare": "verb", "name": format!("run_{risk}"), "on": "held",
            "risk": risk, "effect": "external", "executor": EXECUTOR}),
        )
        .await?;
    }
    let (ty, call, accept): (_, RecordKey, RecordKey) =
        ("held".into(), "held-call".into(), "held-accept".into());
    for key in [&call, &accept] {
        l.upsert("held", key.as_str(), json!({"n": "a"})).await?;
    }
    let (cat, own, next) = (l.cat.as_ref(), &l.own, &"next".into());
    let (low, payload) = ("run_low".into(), || patch(json!({})));
    let n = || patch(json!({"n": "b"}));
    let opts = |key: Option<&str>| WriteOpts {
        idempotency_key: key.map(Into::into),
        ..WriteOpts::default()
    };
    let proposed = |verb: &str, key: &RecordKey, once| ProposedWrite::Call {
        verb: verb.into(),
        key: Some(key.clone()),
        payload: payload(),
        opts: opts(once),
    };
    let first = cat.propose(own, proposed("run_medium", &accept, None), "held".into());
    let (_, id) = proposal(&first.await.must("proposing `run_medium`")?)?;
    let review = reviewer(store.as_ref(), false);
    // Wait until the executor holds the call on `key`.
    let held = |key: &'static str| async move {
        for _ in 0..1_000 {
            if executed(key) == 1 {
                return;
            }
            pause(std::time::Duration::from_millis(5)).await;
        }
    };
    let once = || opts(Some("held-once"));
    let callers: Vec<Caller<'_, Tries<'_>>> = vec![
        Box::pin(async {
            let first = cat.call(own, &low, Some(&call), payload(), once()).await;
            vec![("the call", first)]
        }),
        Box::pin(async {
            held("held-call").await;
            let tries = vec![
                (
                    "call",
                    cat.call(own, &low, Some(&call), payload(), once()).await,
                ),
                ("upsert", cat.upsert(own, &ty, &call, n(), once()).await),
                (
                    "link",
                    cat.link(own, &ty, &call, next, &accept, payload(), once())
                        .await,
                ),
                (
                    "unlink",
                    cat.unlink(own, &ty, &call, next, &accept, once()).await,
                ),
                ("propose", {
                    let again = proposed("run_low", &call, Some("held-once"));
                    cat.propose(own, again, "again".into()).await
                }),
            ];
            release("held-call");
            tries
        }),
        Box::pin(async { vec![("the acceptance", cat.accept(&review, &id).await)] }),
        Box::pin(async {
            held("held-accept").await;
            let again = cat.accept(&review, &id).await;
            release("held-accept");
            vec![("accept", again)]
        }),
    ];
    let answered = join_all(callers).await;
    for (firsts, tries) in [(&answered[0], &answered[1]), (&answered[2], &answered[3])] {
        let (what, first) = &firsts[0];
        let Ok(
            first @ Outcome::Applied {
                output: Some(_),
                position,
                ..
            },
        ) = first
        else {
            return Err(Failure::new(format!(
                "{what} of an external verb answers `applied` with its output; it answered \
                 {first:?}"
            )));
        };
        // An acceptance is named by its own position, before its call's.
        let by_call = *what == "the call";
        for (door, again) in tries {
            let named = match again {
                Err(Error::Refused {
                    reason: Refusal::Unanswered(said),
                    ..
                }) => match &said.case {
                    Case::Unanswered {
                        position: at,
                        accepted,
                    } => *accepted != by_call && (!by_call || *at == *position),
                    _ => false,
                },
                _ => false,
            };
            ensure!(
                named,
                "sent again through `{door}` while the executor runs, {what} is refused as \
                 `unanswered`, naming its position (the call's: {position}); it answered \
                 {again:?}"
            );
        }
        let again = match *what {
            "the call" => cat.call(own, &low, Some(&call), payload(), once()).await,
            _ => cat.accept(&review, &id).await,
        };
        ensure!(
            again.as_ref() == Ok(first),
            "once the executor answered, {what} sent again answers its first outcome {first:?}; \
             it answered {again:?}"
        );
    }
    ensure!(
        executed("held-call") == 1 && executed("held-accept") == 1,
        "the executor runs once for each position, not for a retry: it ran {} and {} times",
        executed("held-call"),
        executed("held-accept")
    );
    Ok(())
}

async fn an_accepts_refusal_names_what_its_reviewers_listing_shows(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let text = json!({"kind": "text"});
    let inv = |level: Value| {
        json!({"name": "inv", "fields": [{"name": "note", "type": text},
            {"name": "level", "type": level},
            {"name": "name", "type": {"kind": "reference", "target": "vault"}}]})
    };
    let vault = json!({"name": "vault", "fields": [{"name": "name", "type": text}]});
    l.declare(de(vault)).await?;
    l.declare(de(inv(text.clone()))).await?;
    l.declare_json(
        json!({"declare": "value_type", "name": "grade", "kind": "text",
                          "one_of": ["low", "high"]}),
    )
    .await?;
    for (verb, more) in [
        (
            "refile",
            json!({"payload": [{"name": "to", "type": {"kind": "reference", "target": "vault"}}],
                          "set": [{"field": "name", "value": "payload.to"}]}),
        ),
        (
            "close",
            json!({"requires": "level = 'low'", "set": [{"field": "note", "value": "'shut'"}]}),
        ),
    ] {
        let mut d = json!({"declare": "verb", "name": verb, "on": "inv", "risk": "high",
                           "effect": "write"});
        d.as_object_mut()
            .into_iter()
            .for_each(|d| d.extend(more.as_object().cloned().unwrap_or_default()));
        l.declare_json(d).await?;
    }
    for (ty, key, fields) in [
        ("vault", "v1", json!({"name": "v1"})),
        ("vault", "v2", json!({"name": "v2"})),
        ("inv", "i2", json!({"note": "n"})),
        ("inv", "i3", json!({"note": "n"})),
        ("inv", "i6", json!({"level": "low"})),
    ] {
        l.upsert(ty, key, fields).await?;
    }
    let held = |id: &str, default: &str| {
        let kept = json!({"inv": {"fields": {"name": "none", "level": "none"}}});
        let types = if id == "typed" {
            json!({"vault": {"default": "none"}})
        } else {
            kept
        };
        let grants =
            json!({"default": default, "review": true, "restricted": true, "types": types});
        holder(s, id, grants)
    };
    let (field, typed, clerk) = (
        held("field", "read"),
        held("typed", "read"),
        held("clerk", "write"),
    );
    let (all, shown) = (
        holder(
            s,
            "all",
            json!({"default": "read", "review": true, "restricted": true}),
        ),
        reviewer(s, false),
    );
    let propose = async |who: &Principal, write: Value| {
        let proposed = l.cat.propose(who, de(write), "a write".into()).await;
        proposal(&proposed.must("a proposal")?).map(|(_, id)| id)
    };
    let upsert = |key: &str, patch: Value| {
        json!({"write": "upsert", "type": "inv", "key": key, "patch": patch,
               "opts": {"expected_version": (key == "i2").then_some(1)}})
    };
    let call = |verb: &str, key: &str, payload: Value| json!({"write": "call", "verb": verb, "key": key, "payload": payload});
    let retired = propose(&l.own, upsert("i1", json!({"note": "v1", "name": "v1"}))).await?;
    let retyped = propose(&l.own, upsert("i4", json!({"level": "mid"}))).await?;
    let moved = propose(&l.own, upsert("i2", json!({"note": "b"}))).await?;
    let refiled = propose(&l.own, call("refile", "i2", json!({"to": "v2"}))).await?;
    let closed = propose(&l.own, call("close", "i6", json!({}))).await?;
    let gone = propose(
        &l.own,
        json!({"write": "retire", "type": "inv", "key": "i3"}),
    )
    .await?;
    for v in ["v1", "v2"] {
        l.retire("vault", v).await?;
    }
    l.retire("inv", "i3").await?;
    l.upsert("inv", "i2", json!({"note": "m"})).await?;
    l.upsert("inv", "i6", json!({"level": "high"})).await?;
    l.migrate(de(inv(json!({"kind": "text", "value_type": "grade"}))))
        .await?;
    let stale = |key: &str| {
        let (ty, key) = ("inv".into(), Some(key.into()));
        json!(Error::invalid(Case::WriteChangedSinceProposed { ty, key }))
    };
    let head = s.head().await.must("reading the head")?;
    // The proposal, its key, the reviewers answered it as stale and those
    // answered as `all`, which is shown everything, and what `all` is told.
    let rows = [
        (
            &retired,
            "i1",
            vec![&field, &typed],
            vec![&shown],
            "not_live",
        ),
        (
            &retyped,
            "i4",
            vec![&field],
            vec![&typed, &shown],
            "not_one_of",
        ),
        (
            &moved,
            "i2",
            vec![],
            vec![&field, &typed, &shown],
            "conflict",
        ),
        (
            &refiled,
            "i2",
            vec![&field, &typed],
            vec![&shown],
            "not_live",
        ),
        (
            &closed,
            "i6",
            vec![],
            vec![&typed, &shown],
            "requires_unmet",
        ),
        (
            &gone,
            "i3",
            vec![],
            vec![&field, &typed, &shown],
            "not_found",
        ),
    ];
    let told = async |who: &Principal, id| json!(l.cat.accept(who, id).await.err());
    let mut wrong = Vec::new();
    for (id, key, hidden, whole, said) in rows {
        let to_all = told(&all, id).await;
        if !to_all.to_string().contains(&format!("\"{said}\"")) {
            wrong.push(format!("`{key}` to `all`: {to_all}, not {said}"));
        }
        for (who, expected) in (hidden.iter().map(|w| (w, stale(key))))
            .chain(whole.iter().map(|w| (w, to_all.clone())))
        {
            let got = told(who, id).await;
            if got != expected {
                wrong.push(format!("`{key}` to `{}`: {got}, not {expected}", who.id));
            }
        }
    }
    // `close` reads `level`, so its proposal is not there to a reviewer
    // kept from it (024, extended): answered as an id no proposal holds.
    for who in [&field, &clerk] {
        let got = told(who, &closed).await;
        let missing = lakelands_api::error::Missing::Proposal { id: closed.clone() };
        let absent = json!(Some(Error::NotFound { missing }));
        if got != absent {
            wrong.push(format!("`i6` to `{}`: {got}, not {absent}", who.id));
        }
    }
    let moved_head = s.head().await.must("reading the head")? != head;
    let listed = l.cat.proposals(&l.own, ProposalFilter::default()).await;
    let listed = listed.must("the owner's listing")?.data;
    let pending = listed.iter().all(|p| p.state == ProposalState::Pending);
    ensure!(
        wrong.is_empty() && !moved_head && pending,
        "an accept's refusal must name to its reviewer only what its listing shows it, \
         else answer a write changed since it was proposed, append nothing (the head \
         moved: {moved_head}) and leave each pending ({pending}): {wrong:#?}"
    );
    Ok(())
}
