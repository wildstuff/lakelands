//! A function's outside call runs by its declared risk, and names the
//! write that set it off
//! (`docs/decisions/064-an-outside-call-runs-by-its-declared-risk.md`).
//!
//! A function above `low` is called only when a reviewer accepts the call:
//! a write proposes its record's call, a declaration or `recompute` its
//! sweep's calls as one. The checks count what the suite's scripted
//! backing ([`crate::conformance::backing`]) was called with, and read the
//! cause each call named.

use lakelands_api::ontology::Risk;
use lakelands_api::record::SourceRecord;
use lakelands_api::write::Outcome;

use crate::conformance::backing::{KIND, causes, made};
use crate::conformance::prelude::*;

check!(
    CALL_RUNS_BY_RISK = a_call_above_low_is_proposed_and_made_when_accepted,
    "064: a write that makes a call of a function above low due proposes that call, whoever writes and at any depth, once for its record while it is pending, and the same call on another record for that record; the backing is called when it is accepted, with the accept as its cause, and fills that record alone; a call the record no longer makes is refused as a conflict; a rejection is recorded as the call's output, and a record that makes its decided call again reads its output without a proposal; declared low again, a call made closes its record's proposal"
);

check!(
    CALL_DECIDED_ONCE = a_call_is_decided_once_whatever_proposed_it,
    "064: a decision on a call decides it for its record: one output recorded for a record's call, made or rejected, closes that record's pending proposal of it, so an accept of it is no conflict that leaves it pending and answers no success that makes nothing; another record that makes the same call, of the same type or another, proposes its own, an accept that fails is made once, and a rejection of a call its record left holds for that record when it makes the call again; a lake opened again, rebuilt or imported proposes no call it holds a proposal for and calls no function above low, and an imported lake proposes a new record's call of a decided input",
    given
);

check!(
    CALL_PROPOSAL_HOLDS_NO_INPUTS = a_proposed_call_holds_no_value_a_forgotten_source_sealed,
    "064 and 058: a proposed call holds the call's digest and no input: a call that reads a source's erasable value leaves that value in neither the log's `write_proposed` nor a listing once the source is forgotten, and the listing shows the record's inputs as the record holds them; forgetting the source retires a record it alone brought, and that retirement closes the record's pending call",
    reopens
);

check!(
    SWEEP_EXISTENCE_HIDES_NO_HOLD = a_sweeps_being_owed_tells_nothing_of_a_record_held,
    "064 and 006: a sweep is there to a reviewer who holds `restricted` and no `only`, and to no other, so that a sweep is owed tells nothing of a record held: a lake whose only owed call is a held record's and one that has no such record list alike to a reviewer without `restricted`",
    given
);

check!(
    DECISION_PER_RECORD = a_decision_on_a_held_records_call_tells_nothing_of_it,
    "064 and 006: a call that waits for a decision is proposed, decided and read per record, so a held record that makes the same call tells nothing: two lakes alike but for a held record whose call was made while the function was low, or proposed, accepted or rejected, before another record made the same call list that record's call alike to a writer and to a reviewer without `restricted`, and the record reads alike",
    given
);

check!(
    MADE_AGAIN_DECIDES_ANEW = a_record_made_again_reads_none_of_its_keys_decisions,
    "064 and 006: a call that waits for a decision is its record's as made, so a record made again at a retired key reads none of the decisions taken there: two lakes alike but for another's records outside a writer's `only` whose calls were left pending, rejected, accepted, or rejected after the record left the call, each then retired, answer that writer's records made again at those keys alike, but for their version, and list them alike to it and to a reviewer admitted its records alone; a retirement closes its record's pending calls",
    given
);

check!(
    MOVED_CALL_CLOSED = a_rekey_closes_a_moved_records_pending_call,
    "064 and 027: a re-key closes the pending call of each record it moves, caused by the `migrated`, since the call is digested on the key the record left; a write of the moved record proposes its call on its new key"
);

check!(
    MOVED_CALL_NOT_LISTED = a_call_a_rekey_moved_away_is_not_listed_with_the_record_moved_in,
    "064 and 027: a record's call is listed with the record that proposed it: two lakes alike but for another's record outside a reviewer's `only`, whose call was proposed before a re-key of the type its key rests on moved it away and moved the reviewer's record to its key, list alike to that reviewer",
    given
);

check!(
    REJECT_KEEPS_GOOD_OUTPUTS = a_sweeps_rejection_records_over_failures_alone,
    "064: a rejected sweep records the rejection over the failures it retried and over nothing else: an output that stands for a record is not replaced by it, and an output recorded while the function was low fills no record that makes its call anew, which proposes its own"
);

check!(
    HELD_CALL_NOT_THERE = a_held_records_call_is_not_there_to_one_not_answered_it,
    "064 and 006: the call proposed for a held record is not there to a reviewer who does not hold `restricted`, whose listing is as with no such record, and is there to one who does"
);

check!(
    SWEEP_PROPOSED = a_declaration_or_recompute_proposes_its_sweep_as_one,
    "064: a declaration or recompute of a function above low proposes the calls its sweep owes as one proposal, with their count and first keys; its reject records the rejection under each call and its accept makes them"
);

check!(
    SWEEP_LISTED_WHOLE = a_sweeps_proposal_is_whole_only_to_who_is_answered_every_record,
    "064 and 006: a proposed call is listed whole to a reviewer who reads every input and the field it fills, and of a sweep is answered every record of the type and holds `restricted`, whether or not a record is held; a sweep is there to a reviewer who holds `restricted` and no `only` alone; of those, a call is there in part, showing nothing, to one who may not read what it reads or fills, and it may be rejected but not accepted"
);

check!(
    CALL_NAMES_ITS_CAUSE = a_backing_is_told_the_write_that_set_its_call_off,
    "064: a backing is told the event that made its call due, its writer, the record, the source of an ingest, the write's context and an accept; the cause is no part of the call, so the same input is one call whoever writes it"
);

check!(
    EXPR_HAS_NO_RISK = an_expr_function_declares_no_risk_and_a_risk_is_described,
    "064: an `expr` function, which reaches nothing outside the lake, is refused a risk above low; a function's declared risk is what describe answers"
);

/// The seams a check that opens a second lake gives it.
fn backed_seams() -> Seams {
    Seams {
        backings: crate::conformance::backing::backings(),
        ..Seams::default()
    }
}

/// A lake alike to `store`'s in everything but what a check does in it.
async fn twin_of(store: &Store, given: &crate::conformance::Given) -> Result<Store, Failure> {
    let twin = (given.twin)(store.lake().clone(), backed_seams()).await;
    twin.map_err(|e| Failure::new(format!("the harness opened no lake: {e}")))
}

/// Every proposal `who` is shown, in the order they were made.
async fn every(l: &Lake, who: &Principal) -> Result<Vec<Proposal>, Failure> {
    let listed = l.cat.proposals(who, ProposalFilter::default()).await;
    let mut listed = listed.must("listing every proposal")?.data;
    listed.sort_by_key(|p| p.position);
    Ok(listed)
}

/// Declares `notify`, a function of the suite's backing at `risk` that
/// echoes `subject`, and the type `ticket`, whose `notice` it computes.
async fn tickets(store: &dyn LakeStore, token: &str, risk: Risk) -> Result<Lake, Failure> {
    tickets_of(store, json!({"token": token, "echo": "subject"}), risk).await
}

/// [`tickets`], with `notify` of the configuration `config`.
async fn tickets_of(store: &dyn LakeStore, config: Value, risk: Risk) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    notify(&l, config, risk).await?;
    let notice = computed_by("notice", StorageKind::Text, "notify", &["subject"]);
    l.declare(type_decl(
        "ticket",
        vec![
            field("subject", StorageKind::Text),
            field("note", StorageKind::Text),
            notice,
        ],
    ))
    .await?;
    Ok(l)
}

async fn notify(l: &Lake, config: Value, risk: Risk) -> Result<Outcome, Failure> {
    l.declare_json(json!({
        "declare": "function", "name": "notify", "backing": KIND, "risk": risk,
        "inputs": [{"name": "subject", "type": {"kind": "text"}}],
        "config": config
    }))
    .await
}

/// The pending proposals `who` is listed, oldest first: by state alone,
/// since one listed in part shows no kind.
async fn pending(l: &Lake, who: &Principal) -> Result<Vec<Proposal>, Failure> {
    let filter = ProposalFilter {
        state: Some(ProposalState::Pending),
        ..Default::default()
    };
    let mut listed = l
        .cat
        .proposals(who, filter)
        .await
        .must("listing proposals")?
        .data;
    listed.sort_by_key(|p| p.position);
    Ok(listed)
}

/// Accept `id` as `who`, answering the position of its `write_accepted`.
async fn accept(
    store: &dyn LakeStore,
    l: &Lake,
    who: &Principal,
    id: &ProposalId,
) -> Result<Position, Failure> {
    let before = store.head().await.must("reading the head")?;
    l.cat
        .accept(who, id)
        .await
        .must("accepting a proposed call")?;
    let events = events_after(store, before).await?;
    let at = events
        .iter()
        .find(|e| matches!(&e.body, EventBody::WriteAccepted { proposal } if proposal == id));
    at.map(|e| e.position)
        .ok_or_else(|| Failure::new(format!("accepting {id:?} appended no write_accepted")))
}

/// The pending proposal of `key`'s call.
fn of_key<'p>(listed: &'p [Proposal], key: &str) -> Option<&'p Proposal> {
    listed.iter().find(
        |p| matches!(&p.write, Some(ProposedWrite::Compute { key: k, .. }) if k.as_str() == key),
    )
}

/// A reviewer who holds `review` and `restricted`: a sweep is whole to no
/// other (064).
fn reviewer(store: &dyn LakeStore, schema: bool) -> Principal {
    let mut who = crate::conformance::fixture::reviewer(store, schema);
    who.grants.restricted = true;
    who
}

/// An agent that writes everything.
fn writer(store: &dyn LakeStore) -> Principal {
    let grants = Grants {
        default: Access::Write,
        ..Grants::default()
    };
    principal(store, PrincipalKind::Agent, "worker", grants)
}

fn notice_of(r: &lakelands_api::Record) -> Option<&Value> {
    value(r, "notice")
}

fn error_of(r: &lakelands_api::Record) -> Option<String> {
    r.meta.get("notice").and_then(|m| m.error.clone())
}

async fn a_call_above_low_is_proposed_and_made_when_accepted(store: Store) -> Held {
    let token = "call_runs_by_risk";
    let l = tickets(store.as_ref(), token, Risk::High).await?;
    let rev = reviewer(store.as_ref(), false);
    // The owner's write, and an agent's two reactions deep.
    l.upsert("ticket", "t1", json!({"subject": "a"})).await?;
    let head = store.head().await.must("reading the head")?;
    let deep = store.catalog(reactive(2, Some(head)));
    let worker = writer(store.as_ref());
    let (ty, t2) = (lakelands_api::TypeName::from("ticket"), "t2".into());
    let wrote = deep.upsert(
        &worker,
        &ty,
        &t2,
        patch(json!({"subject": "b"})),
        Default::default(),
    );
    wrote.await.must("the agent's reactive write")?;
    // The same call as t1's, while t1's is pending.
    l.upsert("ticket", "t3", json!({"subject": "a"})).await?;
    let listed = pending(&l, &rev).await?;
    let r1 = l.read("ticket", "t1").await?;
    ensure!(
        made(token) == 0 && notice_of(&r1).is_none() && error_of(&r1).is_none(),
        "`notify` is declared high, so no write calls its backing before an accept and \
         `notice` waits, cleared without an error; it was called {} times and t1 reads \
         {:?}, error {:?}",
        made(token),
        notice_of(&r1),
        error_of(&r1)
    );
    ensure!(
        listed.len() == 3
            && ["t1", "t2", "t3"]
                .iter()
                .all(|k| of_key(&listed, k).is_some()),
        "the owner's write of t1 and the agent's reactive write of t2 each propose their \
         call, and t3, which makes t1's call, proposes its own: a call that waits for a \
         decision is its record's; the pending calls listed are {:?}",
        listed.iter().map(|p| &p.write).collect::<Vec<_>>()
    );
    // No caller proposes a call: it is the store's to propose.
    let forged = of_key(&listed, "t1").and_then(|p| p.write.clone());
    let before = store.head().await.must("reading the head")?;
    let answer = match forged {
        Some(w) => l.cat.propose(&l.own, w, "forged".into()).await,
        None => return Err(Failure::new("t1's call is not listed".to_owned())),
    };
    ensure!(
        matches!(answer, Err(Error::Refused { .. })),
        "a call of a function is proposed by the store alone: the owner's `propose` of \
         one is refused; it answered {answer:?}"
    );
    head_unmoved(store.as_ref(), before, "a refused proposal of a call").await?;
    let p2 = of_key(&listed, "t2").map(|p| p.proposer.clone());
    ensure!(
        p2.as_deref() == Some(worker.writer().as_str()),
        "a proposed call is proposed as the writer that set it off, {}; t2's names {p2:?}",
        worker.writer()
    );

    // The accept makes the call, caused by the accept.
    let p1 = of_key(&listed, "t1")
        .map(|p| p.id.clone())
        .unwrap_or_default();
    let accepted = accept(store.as_ref(), &l, &rev, &p1).await?;
    let outputs = outputs_from(store.as_ref(), accepted).await?;
    let r1 = l.read("ticket", "t1").await?;
    ensure!(
        made(token) == 1
            && notice_of(&r1) == Some(&json!("a"))
            && outputs.iter().any(|e| e.cause == Some(accepted)),
        "accepting t1's call calls the backing once and fills `notice` from the output \
         recorded with the accept as its cause; it was called {} times, t1 reads {:?}, \
         and the outputs recorded since name {:?}",
        made(token),
        notice_of(&r1),
        outputs.iter().map(|e| e.cause).collect::<Vec<_>>()
    );

    // The accept fills the record that proposed it alone: t3's call waits
    // for its own decision.
    let r3 = l.read("ticket", "t3").await?;
    let left = pending(&l, &rev).await?;
    ensure!(
        notice_of(&r3).is_none() && error_of(&r3).is_none() && of_key(&left, "t3").is_some(),
        "t3 makes t1's call, but t1's accept decides t1's alone: t3 waits, its own call \
         pending; it reads {:?}, error {:?}, pending {:?}",
        notice_of(&r3),
        error_of(&r3),
        left.iter().map(|p| &p.write).collect::<Vec<_>>()
    );

    // A rejection is the call's output.
    let p2 = (of_key(&listed, "t2").map(|p| p.id.clone())).unwrap_or_default();
    l.cat
        .reject(&rev, &p2, "not now".into())
        .await
        .must("rejecting t2's call")?;
    let r2 = l.read("ticket", "t2").await?;
    let again = pending(&l, &rev).await?;
    ensure!(
        made(token) == 1
            && notice_of(&r2).is_none()
            && error_of(&r2).is_some_and(|e| e.contains("rejected") && e.contains("not now"))
            && of_key(&again, "t2").is_none(),
        "rejecting t2's call calls nothing, clears `notice` with the rejection and its \
         reason as its error, and proposes nothing again; it was called {} times, t2 \
         reads {:?}, error {:?}, pending {:?}",
        made(token),
        notice_of(&r2),
        error_of(&r2),
        again.iter().map(|p| &p.write).collect::<Vec<_>>()
    );

    // A decision holds for its record: t1 back on its accepted call reads
    // the output, and nothing is called or proposed for it.
    l.upsert("ticket", "t1", json!({"subject": "z"})).await?;
    let away = of_key(&pending(&l, &rev).await?, "t1").map(|p| p.id.clone());
    l.upsert("ticket", "t1", json!({"subject": "a"})).await?;
    let r1 = l.read("ticket", "t1").await?;
    let after = pending(&l, &rev).await?;
    let back = after.iter().filter(|p| {
        matches!(&p.write, Some(ProposedWrite::Compute { key, .. }) if key.as_str() == "t1")
            && Some(&p.id) != away.as_ref()
    });
    ensure!(
        made(token) == 1 && notice_of(&r1) == Some(&json!("a")) && back.count() == 0,
        "t1 moved away and back to the call its accept made: it reads that output, and \
         nothing is called or proposed again; it was called {} times and t1 reads {:?}",
        made(token),
        notice_of(&r1)
    );

    // A call the record no longer makes is refused; one whose record moved
    // without changing the call is accepted.
    l.upsert("ticket", "t5", json!({"subject": "c"})).await?;
    let moved = of_key(&pending(&l, &rev).await?, "t5").map(|p| p.id.clone());
    l.upsert("ticket", "t5", json!({"subject": "d"})).await?;
    let moved = moved.unwrap_or_default();
    let answer = l.cat.accept(&rev, &moved).await;
    let still = pending(&l, &rev).await?;
    ensure!(
        matches!(answer, Err(Error::Conflict { .. }))
            && still.iter().any(|p| p.id == moved)
            && made(token) == 1,
        "t5's call for subject c was proposed, then t5's subject became d: accepting the \
         first is refused as a conflict, and the proposal stays pending; it answered \
         {answer:?}"
    );
    l.upsert("ticket", "t6", json!({"subject": "e"})).await?;
    let same = of_key(&pending(&l, &rev).await?, "t6").map(|p| p.id.clone());
    l.upsert("ticket", "t6", json!({"note": "unrelated"}))
        .await?;
    let answer = l.cat.accept(&rev, &same.unwrap_or_default()).await;
    let r6 = l.read("ticket", "t6").await?;
    ensure!(
        answer.is_ok() && made(token) == 2 && notice_of(&r6) == Some(&json!("e")),
        "t6's `note` changed after its call was proposed, which leaves the call as it \
         was, so the accept makes it; it answered {answer:?}, called {} times, and t6 \
         reads {:?}",
        made(token),
        notice_of(&r6)
    );

    // Declared low again, the calls still owed are made, and each closes its
    // record's proposal.
    notify(&l, json!({"token": token, "echo": "subject"}), Risk::Low).await?;
    let left = pending(&l, &rev).await?;
    let r3 = l.read("ticket", "t3").await?;
    ensure!(
        of_key(&left, "t3").is_none() && notice_of(&r3) == Some(&json!("a")),
        "`notify` declared low again makes t3's call, which closes t3's proposal of it; \
         t3 reads {:?}, pending {:?}",
        notice_of(&r3),
        left.iter().map(|p| &p.write).collect::<Vec<_>>()
    );
    Ok(())
}

async fn a_declaration_or_recompute_proposes_its_sweep_as_one(store: Store) -> Held {
    let token = "sweep_proposed";
    let l = tickets(store.as_ref(), token, Risk::Low).await?;
    let rev = reviewer(store.as_ref(), false);
    for (k, s) in [("s1", "x"), ("s2", "y"), ("s3", "z")] {
        l.upsert("ticket", k, json!({"subject": s})).await?;
    }
    let before = made(token);
    // Declared again, high, with another configuration: every record's
    // call is due again.
    let config = json!({"token": token, "echo": "subject", "v": 2});
    notify(&l, config, Risk::High).await?;
    let listed = sweeps(&l, &rev).await?;
    ensure!(
        made(token) == before
            && listed.len() == 1
            && matches!(&listed[0].write, Some(ProposedWrite::ComputeSweep { count: 3, first, .. })
                if first.iter().map(|k| k.as_str()).eq(["s1", "s2", "s3"])),
        "declaring `notify` again, high, makes the calls of s1, s2 and s3 due: they are \
         proposed as one sweep, of count 3, naming s1, s2 and s3 first, and nothing is \
         called; it was called {} times more, and the sweeps pending are {:?}",
        made(token) - before,
        listed.iter().map(|p| &p.write).collect::<Vec<_>>()
    );
    l.cat
        .reject(&rev, &listed[0].id, "too many".into())
        .await
        .must("rejecting the sweep")?;
    for k in ["s1", "s2", "s3"] {
        let r = l.read("ticket", k).await?;
        ensure!(
            notice_of(&r).is_none() && error_of(&r).is_some_and(|e| e.contains("too many")),
            "rejecting the sweep records the rejection under each of its calls, so {k}'s \
             `notice` is cleared with it as its error; it reads {:?}, error {:?}",
            notice_of(&r),
            error_of(&r)
        );
    }
    // `recompute` calls the rejections again, as a sweep proposed as one.
    let own = &l.own;
    l.cat
        .recompute(own, Recompute::Function("notify".into()))
        .await
        .must("recomputing `notify`")?;
    let listed = sweeps(&l, &rev).await?;
    ensure!(
        made(token) == before && listed.len() == 1,
        "`recompute` of `notify` proposes its three calls as one sweep and calls nothing; \
         the sweeps pending are {:?}",
        listed.iter().map(|p| &p.write).collect::<Vec<_>>()
    );
    let accepted = accept(store.as_ref(), &l, &rev, &listed[0].id).await?;
    let outputs = outputs_from(store.as_ref(), accepted).await?;
    let filled = outputs.iter().filter(|e| e.cause == Some(accepted));
    ensure!(
        made(token) == before + 3 && filled.count() == 3,
        "accepting the sweep makes its three calls, each recorded with the accept as its \
         cause; it was called {} times",
        made(token) - before
    );
    for (k, s) in [("s1", "x"), ("s2", "y"), ("s3", "z")] {
        let r = l.read("ticket", k).await?;
        ensure!(
            notice_of(&r) == Some(&json!(s)),
            "after the sweep's accept {k} reads {s}, but reads {:?}",
            notice_of(&r)
        );
    }
    Ok(())
}

async fn a_call_is_decided_once_whatever_proposed_it(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let reopen = given.reopen.clone();
    let token = "call_decided_once";
    let config = json!({"token": token, "echo": "subject", "fail_on": "boom"});
    let l = tickets_of(store.as_ref(), config, Risk::High).await?;
    let rev = reviewer(store.as_ref(), false);
    let own = &l.own;
    // A write proposes its call; a sweep of the same call is rejected.
    l.upsert("ticket", "d1", json!({"subject": "x"})).await?;
    let first = of_key(&pending(&l, &rev).await?, "d1").map(|p| p.id.clone());
    let first = first.unwrap_or_default();
    l.cat
        .recompute(own, Recompute::Function("notify".into()))
        .await
        .must("recomputing `notify`")?;
    let sweep = sweeps(&l, &rev).await?;
    let sweep = sweep.first().map(|p| p.id.clone()).unwrap_or_default();
    l.cat
        .reject(&rev, &sweep, "no".into())
        .await
        .must("rejecting the sweep")?;
    let left = pending(&l, &rev).await?;
    let answer = l.cat.accept(&rev, &first).await;
    let r = l.read("ticket", "d1").await?;
    ensure!(
        of_key(&left, "d1").is_none()
            && answer.is_err()
            && made(token) == 0
            && error_of(&r).is_some_and(|e| e.contains("rejected: no")),
        "the sweep's rejection is the output of d1's call, which closes d1's own proposal: \
         nothing is pending for it, accepting it is refused and calls nothing, and d1 reads \
         the rejection; pending {:?}, accept {answer:?}, called {} times, d1's error {:?}",
        left.iter().map(|p| &p.write).collect::<Vec<_>>(),
        made(token),
        error_of(&r)
    );

    // A call another decision made is no conflict for the proposal of it.
    l.upsert("ticket", "d2", json!({"subject": "y"})).await?;
    let second = of_key(&pending(&l, &rev).await?, "d2").map(|p| p.id.clone());
    let second = second.unwrap_or_default();
    l.cat
        .recompute(own, Recompute::Function("notify".into()))
        .await
        .must("recomputing `notify` again")?;
    let sweep = sweeps(&l, &rev).await?;
    let sweep = sweep.first().map(|p| p.id.clone()).unwrap_or_default();
    accept(store.as_ref(), &l, &rev, &sweep).await?;
    let left = pending(&l, &rev).await?;
    let answer = l.cat.accept(&rev, &second).await;
    let r = l.read("ticket", "d2").await?;
    ensure!(
        of_key(&left, "d2").is_none()
            && !matches!(answer, Err(Error::Conflict { .. }))
            && notice_of(&r) == Some(&json!("y")),
        "the sweep's accept made d2's call, which closes d2's own proposal: nothing is \
         pending for it, accepting it is no conflict, and d2 reads its output; pending \
         {:?}, accept {answer:?}, d2 reads {:?}",
        left.iter().map(|p| &p.write).collect::<Vec<_>>(),
        notice_of(&r)
    );

    // A rejection of a call its record no longer makes holds for that
    // record, and for no other that makes the call.
    l.upsert("ticket", "m1", json!({"subject": "q"})).await?;
    let stale = of_key(&pending(&l, &rev).await?, "m1").map(|p| p.id.clone());
    let stale = stale.unwrap_or_default();
    l.upsert("ticket", "m1", json!({"subject": "r"})).await?;
    l.cat
        .reject(&rev, &stale, "stale".into())
        .await
        .must("rejecting the call m1 no longer makes")?;
    l.upsert("ticket", "m3", json!({"subject": "q"})).await?;
    let r3 = l.read("ticket", "m3").await?;
    let again = pending(&l, &rev).await?;
    ensure!(
        of_key(&again, "m3").is_some() && error_of(&r3).is_none(),
        "m1's call for subject q was rejected after m1 moved on: m3, which makes the same \
         call, proposes its own and reads no rejection; m3's error {:?}, pending {:?}",
        error_of(&r3),
        again.iter().map(|p| &p.write).collect::<Vec<_>>()
    );
    let m1 = of_key(&again, "m1").map(|p| p.id.clone());
    l.upsert("ticket", "m1", json!({"subject": "q"})).await?;
    let r1 = l.read("ticket", "m1").await?;
    let back = pending(&l, &rev).await?;
    let proposed = back.iter().filter(|p| {
        matches!(&p.write, Some(ProposedWrite::Compute { key, .. }) if key.as_str() == "m1")
            && Some(&p.id) != m1.as_ref()
    });
    ensure!(
        proposed.count() == 0 && error_of(&r1).is_some_and(|e| e.contains("rejected: stale")),
        "m1 back on subject q makes the call its rejection decided: it reads the rejection \
         and proposes nothing; m1's error {:?}",
        error_of(&r1)
    );

    // Records of two types that make one call each propose it; an accept
    // makes the one record's, once, failed or not.
    let notice = computed_by("notice", StorageKind::Text, "notify", &["subject"]);
    let memo = vec![field("subject", StorageKind::Text), notice];
    l.declare(type_decl("memo", memo)).await?;
    let n = made(token);
    l.upsert("ticket", "w1", json!({"subject": "same"})).await?;
    l.upsert("memo", "w2", json!({"subject": "same"})).await?;
    let listed = pending(&l, &rev).await?;
    let (w1, w2) = (of_key(&listed, "w1"), of_key(&listed, "w2"));
    let w2_id = w2.map(|p| p.id.clone());
    let w1 = w1.map(|p| p.id.clone()).unwrap_or_default();
    accept(store.as_ref(), &l, &rev, &w1).await?;
    let (t, m) = (l.read("ticket", "w1").await?, l.read("memo", "w2").await?);
    let left = pending(&l, &rev).await?;
    ensure!(
        made(token) == n + 1
            && notice_of(&t) == Some(&json!("same"))
            && notice_of(&m).is_none()
            && w2_id.is_some()
            && left.iter().any(|p| Some(&p.id) == w2_id.as_ref()),
        "w1 of `ticket` and w2 of `memo` make one call and propose it each: w1's accept \
         calls once and fills w1 alone, w2's stays pending; called {} times, they read {:?} \
         and {:?}",
        made(token) - n,
        notice_of(&t),
        notice_of(&m)
    );
    let n = made(token);
    l.upsert("ticket", "f1", json!({"subject": "boom"})).await?;
    l.upsert("memo", "f2", json!({"subject": "boom"})).await?;
    let listed = pending(&l, &rev).await?;
    let f2 = of_key(&listed, "f2").map(|p| p.id.clone());
    let f1 = of_key(&listed, "f1").map(|p| p.id.clone());
    accept(store.as_ref(), &l, &rev, &f1.unwrap_or_default()).await?;
    let (t, m) = (l.read("ticket", "f1").await?, l.read("memo", "f2").await?);
    ensure!(
        made(token) == n + 1 && error_of(&t).is_some() && error_of(&m).is_none(),
        "an accepted call that fails is made once: f1's accept calls once and f1 reads the \
         failure, while f2, of `memo`, waits; called {} times, errors {:?} and {:?}",
        made(token) - n,
        error_of(&t),
        error_of(&m)
    );
    accept(store.as_ref(), &l, &rev, &f2.unwrap_or_default()).await?;
    let m = l.read("memo", "f2").await?;
    ensure!(
        made(token) == n + 2 && error_of(&m).is_some(),
        "f2's own accept makes its call once more, and f2 reads the failure; called {} \
         times, error {:?}",
        made(token) - n,
        error_of(&m)
    );

    // A lake opened again proposes and calls nothing it did not.
    l.upsert("ticket", "d3", json!({"subject": "z"})).await?;
    let (before, n) = (pending(&l, &rev).await?.len(), made(token));
    let seams = backed_seams();
    let store = (reopen)(store, seams)
        .await
        .map_err(|e| Failure::new(format!("opening the lake again failed: {e}")))?;
    let (l, rev) = (Lake::of(store.as_ref()), reviewer(store.as_ref(), false));
    let after = pending(&l, &rev).await?.len();
    ensure!(
        (after, made(token)) == (before, n),
        "a lake opened again calls no function above low and proposes no call it holds a \
         proposal for: {before} pending and {n} calls became {after} and {}",
        made(token)
    );
    store.rebuild().await.must("rebuilding the projection")?;
    let after = pending(&l, &rev).await?.len();
    let d2 = l.read("ticket", "d2").await?;
    ensure!(
        (after, made(token)) == (before, n) && notice_of(&d2) == Some(&json!("y")),
        "a rebuilt lake calls no function above low and proposes no call it holds a \
         proposal for, and reads d2 as filled: {before} pending and {n} calls became \
         {after} and {}; d2 reads {:?}",
        made(token),
        notice_of(&d2)
    );
    // An export of it imported into a fresh lake proposes nothing and calls
    // nothing, and the imported lake reads d2 as filled.
    let scratch = super::export::Scratch::new(given.scratch.clone());
    let from = scratch.dir("from");
    crate::export::write(store.as_ref(), &from)
        .await
        .must("exporting the lake")?;
    let seams = backed_seams();
    let lake = store.lake().clone();
    let imported = (given.import)(from, lake, None, seams.clone()).await;
    let imported = imported
        .map_err(|e| Failure::new(format!("importing the export into a fresh lake: {e}")))?;
    let writer = (imported.writer)(imported.store, seams)
        .await
        .map_err(|e| Failure::new(format!("opening the imported lake for writing: {e}")))?;
    let (l, rev) = (Lake::of(writer.as_ref()), reviewer(writer.as_ref(), false));
    let after = pending(&l, &rev).await?.len();
    let d2 = l.read("ticket", "d2").await?;
    ensure!(
        (after, made(token)) == (before, n) && notice_of(&d2) == Some(&json!("y")),
        "an export imported into a fresh lake proposes no call it holds a proposal for and \
         calls no function above low, and reads d2 as filled: {before} pending and {n} calls \
         became {after} and {}; d2 reads {:?}",
        made(token),
        notice_of(&d2)
    );
    // A new record that makes d2's decided call proposes its own.
    l.upsert("ticket", "d4", json!({"subject": "y"})).await?;
    let d4 = l.read("ticket", "d4").await?;
    let listed = pending(&l, &rev).await?;
    ensure!(
        made(token) == n && notice_of(&d4).is_none() && of_key(&listed, "d4").is_some(),
        "on the imported lake d4 makes the call d2's decision made: it proposes its own \
         and waits; called {} times more, d4 reads {:?}",
        made(token) - n,
        notice_of(&d4)
    );
    Ok(())
}

async fn a_proposed_call_holds_no_value_a_forgotten_source_sealed(
    store: Store,
    reopen: Reopen,
) -> Held {
    use super::erasable::{Keys, again, person, with};
    let keys = std::sync::Arc::new(Keys::default());
    let store = again(&reopen, store, with(&keys), "with a key store").await?;
    let l = Lake::of(store.as_ref());
    let text = json!({"kind": "text"});
    let config = json!({"token": "call_proposal_holds_no_inputs", "echo": "name"});
    l.declare_json(
        json!({"declare": "function", "name": "shout", "backing": KIND,
        "risk": "high", "inputs": [{"name": "name", "type": text}], "config": config}),
    )
    .await?;
    let mut t = person();
    t.fields
        .push(computed_by("loud", StorageKind::Text, "shout", &["name"]));
    l.declare(t).await?;
    let rec = crate::conformance::fixture::source_record(
        "person",
        "p1",
        json!({"id": "p1", "name": "Alice", "city": "Oslo"}),
    );
    l.ingest("crm", vec![rec]).await.must("ingesting p1")?;
    let rev = reviewer(store.as_ref(), false);
    let listed = pending(&l, &rev).await?;
    let shown = listed.iter().find_map(|p| match &p.write {
        Some(ProposedWrite::Compute { inputs, .. }) => inputs.clone(),
        _ => None,
    });
    ensure!(
        listed.len() == 1 && shown == Some(json!({"name": "Alice"})),
        "p1's call is proposed once, and the listing shows its input as the record holds \
         it; it lists {:?}",
        listed.iter().map(|p| &p.write).collect::<Vec<_>>()
    );
    let logged = log_events(store.as_ref()).await?;
    let proposed = (logged.iter())
        .filter(|e| matches!(e.body, EventBody::WriteProposed { .. }))
        .map(|e| json!(e.body).to_string());
    ensure!(
        proposed.clone().count() == 1 && !proposed.clone().any(|b| b.contains("Alice")),
        "the log's `write_proposed` of the call names no input; it holds {:?}",
        proposed.collect::<Vec<_>>()
    );
    store
        .forget_source(&"crm".into(), None)
        .await
        .must("forgetting crm")?;
    let after = json!(pending(&l, &rev).await?).to_string();
    let log = json!(
        log_events(store.as_ref())
            .await?
            .iter()
            .map(|e| &e.body)
            .collect::<Vec<_>>()
    );
    ensure!(
        !after.contains("Alice") && !log.to_string().contains("Alice"),
        "after crm is forgotten neither the listing nor the log holds `Alice`"
    );
    // Forgetting crm retires p1, which crm alone brought, and the
    // retirement closes p1's call: the closing names it as its cause.
    let logged = log_events(store.as_ref()).await?;
    let retired = logged.iter().find(|e| {
        matches!(&e.body, EventBody::RecordRetired { ty, key, .. }
            if ty.as_str() == "person" && key.as_str() == "p1")
    });
    let closed = logged
        .iter()
        .filter(|e| matches!(e.body, EventBody::WriteRejected { .. }))
        .map(|e| e.cause)
        .collect::<Vec<_>>();
    let at = retired.map(|e| e.position);
    ensure!(
        at.is_some() && closed == vec![at] && pending(&l, &rev).await?.is_empty(),
        "forgetting crm retires p1 (at {at:?}) and closes its pending call, caused by that \
         retirement; the log's rejections are caused by {closed:?}"
    );
    Ok(())
}

/// The pending sweeps `who` is listed.
async fn sweeps(l: &Lake, who: &Principal) -> Result<Vec<Proposal>, Failure> {
    let listed = pending(l, who).await?;
    let sweep = |p: &Proposal| matches!(p.write, Some(ProposedWrite::ComputeSweep { .. }) | None);
    Ok(listed.into_iter().filter(sweep).collect())
}

async fn a_sweeps_proposal_is_whole_only_to_who_is_answered_every_record(store: Store) -> Held {
    let token = "sweep_listed_whole";
    let l = tickets(store.as_ref(), token, Risk::Low).await?;
    for (k, s) in [("h1", "x"), ("h2", "y")] {
        l.upsert("ticket", k, json!({"subject": s})).await?;
    }
    let config = |v: u32| json!({"token": token, "echo": "subject", "v": v});
    let plain = crate::conformance::fixture::reviewer(store.as_ref(), false);
    let mut held = plain.clone();
    held.id = "warden".into();
    held.grants.restricted = true;
    // With no record held, the sweep is whole to a reviewer with
    // `restricted`, its count among it, and in part to one without: what
    // is held is told to no one by the sweep's being whole.
    notify(&l, config(2), Risk::High).await?;
    let (to_held, to_plain) = (pending(&l, &held).await?, pending(&l, &plain).await?);
    ensure!(
        matches!(
            to_held.first().map(|p| (&p.write, p.more)),
            Some((Some(ProposedWrite::ComputeSweep { count: 2, .. }), false))
        ) && to_plain.is_empty(),
        "with no record of `ticket` held, a reviewer with `restricted` is listed the sweep \
         whole, count 2, and one without it is listed none; \
         listed {:?} and {:?}",
        to_held
            .iter()
            .map(|p| (&p.write, p.more))
            .collect::<Vec<_>>(),
        to_plain
            .iter()
            .map(|p| (&p.write, p.more))
            .collect::<Vec<_>>()
    );
    let first = to_held[0].id.clone();
    l.cat
        .reject(&held, &first, "again".into())
        .await
        .must("rejecting")?;

    // One record held: the sweep is there to a reviewer without `restricted`
    // as it is with no record held, which is not at all.
    restrict(store.as_ref(), "ticket", "h2", "a hold").await?;
    notify(&l, config(3), Risk::High).await?;
    let to_held = pending(&l, &held).await?;
    let sweep = to_held.first().map(|p| p.id.clone()).unwrap_or_default();
    let to_plain = pending(&l, &plain).await?;
    let to_plain: Vec<_> = to_plain.into_iter().filter(|p| p.id == sweep).collect();
    ensure!(
        to_plain.is_empty()
            && matches!(
                to_held.first().map(|p| (&p.write, p.more)),
                Some((Some(ProposedWrite::ComputeSweep { count: 2, .. }), false))
            ),
        "with h2 held, the sweep is not there to a reviewer without `restricted`, so its \
         being owed tells nothing of h2, and whole, count 2, to one who holds it; listed {:?} and {:?}",
        to_plain
            .iter()
            .map(|p| (&p.write, p.more))
            .collect::<Vec<_>>(),
        to_held
            .iter()
            .map(|p| (&p.write, p.more))
            .collect::<Vec<_>>()
    );
    let before = store.head().await.must("reading the head")?;
    let answer = l.cat.accept(&plain, &sweep).await;
    ensure!(
        matches!(answer, Err(Error::NotFound { .. })),
        "a reviewer the sweep is not there to is answered as for an id that is not there; it \
         answered {answer:?}"
    );
    head_unmoved(store.as_ref(), before, "an accept of a sweep not there").await?;
    // Nor is it there to a reviewer who holds `restricted` and an `only`.
    let mut some = held.clone();
    some.id = "some".into();
    let mut only = lakelands_api::principal::TypeGrants::default();
    only.only.insert("subject".into(), json!("x"));
    some.grants.types.insert("ticket".into(), only);
    let to_some = pending(&l, &some).await?;
    ensure!(
        !to_some.iter().any(|p| p.id == sweep),
        "a reviewer who holds `restricted` and an `only` on `ticket` is admitted some of \
         its records alone, so the sweep is not there to it; it is listed {:?}",
        to_some
            .iter()
            .map(|p| (&p.write, p.more))
            .collect::<Vec<_>>()
    );
    // Nor is a reviewer who may not read the field it fills listed it whole.
    let mut blind = held.clone();
    blind.id = "blind".into();
    let mut kept = lakelands_api::principal::TypeGrants::default();
    kept.fields.insert("notice".into(), Access::None);
    blind.grants.types.insert("ticket".into(), kept);
    let to_blind = pending(&l, &blind).await?;
    let to_blind: Vec<_> = to_blind.into_iter().filter(|p| p.id == sweep).collect();
    ensure!(
        matches!(
            to_blind.first().map(|p| (&p.write, p.more)),
            Some((None, true))
        ),
        "a reviewer who may not read `notice` is listed the sweep in part; it is listed \
         {:?}",
        to_blind
            .iter()
            .map(|p| (&p.write, p.more))
            .collect::<Vec<_>>()
    );
    let before = store.head().await.must("reading the head")?;
    let answer = l.cat.accept(&blind, &sweep).await;
    ensure!(
        matches!(answer, Err(Error::Refused { .. })),
        "a reviewer listed the sweep in part may not accept it; it answered {answer:?}"
    );
    head_unmoved(store.as_ref(), before, "a refused accept of a sweep").await?;
    l.cat
        .reject(&blind, &sweep, "in part".into())
        .await
        .must("rejecting in part")?;
    Ok(())
}

async fn a_sweeps_being_owed_tells_nothing_of_a_record_held(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let token = "sweep_existence";
    store.head().await.must("reading the head")?;
    // Two lakes alike but for a record: one holds h2, whose call failed,
    // the other has none. Only h2's call is owed to a `recompute`.
    let twin = twin_of(&store, &given).await?;
    let config = json!({"token": token, "echo": "subject", "fail_on": "y"});
    let mut listed = Vec::new();
    for (lake, held) in [(store.clone(), true), (twin.clone(), false)] {
        let l = Lake::of(lake.as_ref());
        notify(&l, config.clone(), Risk::Low).await?;
        let notice = computed_by("notice", StorageKind::Text, "notify", &["subject"]);
        let fields = vec![field("subject", StorageKind::Text), notice];
        l.declare(type_decl("ticket", fields)).await?;
        l.upsert("ticket", "h1", json!({"subject": "x"})).await?;
        if held {
            l.upsert("ticket", "h2", json!({"subject": "y"})).await?;
            restrict(lake.as_ref(), "ticket", "h2", "a hold").await?;
        }
        notify(&l, config.clone(), Risk::High).await?;
        l.cat
            .recompute(&l.own, Recompute::Function("notify".into()))
            .await
            .must("recomputing `notify`")?;
        let (plain, keeper) = (
            crate::conformance::fixture::reviewer(lake.as_ref(), false),
            reviewer(lake.as_ref(), false),
        );
        let (to_plain, to_keeper) = (sweeps(&l, &plain).await?, sweeps(&l, &keeper).await?);
        listed.push((to_plain.len(), to_keeper.len()));
    }
    ensure!(
        listed[0].0 == 0 && listed[1].0 == 0 && listed[0].1 > 0 && listed[1].1 == 0,
        "a sweep owed only for a held record is there to a reviewer with `restricted` and to \
         none without it, so that one is owed tells of no hold; the lake that holds h2 \
         lists (without, with) {:?}, the one that has none {:?}",
        listed[0],
        listed[1]
    );
    twin.close().await.must("closing the twin")?;
    Ok(())
}

async fn a_decision_on_a_held_records_call_tells_nothing_of_it(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let token = "decision_per_record";
    store.head().await.must("reading the head")?;
    let twin = twin_of(&store, &given).await?;
    // Lake A holds h1, h2 and h3, held, each with a call: h2's rejected, h3's
    // accepted, h1's pending. Lake B has none. Then x1, x2 and x3 make the
    // same calls in both.
    let mut seen = Vec::new();
    for (lake, held) in [(store.clone(), true), (twin.clone(), false)] {
        let l = tickets(lake.as_ref(), token, Risk::Low).await?;
        let keeper = reviewer(lake.as_ref(), false);
        // h0's call is made while `notify` is low, then the risk rises.
        if held {
            l.upsert("ticket", "h0", json!({"subject": "zero"})).await?;
            restrict(lake.as_ref(), "ticket", "h0", "a hold").await?;
        }
        let config = json!({"token": token, "echo": "subject"});
        notify(&l, config, Risk::High).await?;
        if held {
            for (k, s) in [("h1", "one"), ("h2", "two"), ("h3", "three")] {
                l.upsert("ticket", k, json!({"subject": s})).await?;
                restrict(lake.as_ref(), "ticket", k, "a hold").await?;
            }
            let listed = pending(&l, &keeper).await?;
            let id = |k| of_key(&listed, k).map(|p| p.id.clone()).unwrap_or_default();
            l.cat
                .reject(&keeper, &id("h2"), "h2 is under investigation".into())
                .await
                .must("rejecting h2's call")?;
            accept(lake.as_ref(), &l, &keeper, &id("h3")).await?;
        }
        let (worker, plain) = (
            writer(lake.as_ref()),
            crate::conformance::fixture::reviewer(lake.as_ref(), false),
        );
        let ty = lakelands_api::TypeName::from("ticket");
        let x = [
            ("x0", "zero"),
            ("x1", "one"),
            ("x2", "two"),
            ("x3", "three"),
        ];
        for (k, s) in x {
            let k = lakelands_api::RecordKey::from(k);
            let wrote = l.cat.upsert(
                &worker,
                &ty,
                &k,
                patch(json!({"subject": s})),
                Default::default(),
            );
            wrote.await.must("the worker's write")?;
        }
        let names = |listed: Vec<Proposal>| {
            let named = listed.into_iter().map(|p| match p.write {
                Some(ProposedWrite::Compute { key, .. }) => format!("compute {key}"),
                other => format!("{other:?}"),
            });
            named.collect::<Vec<_>>()
        };
        let mut read = Vec::new();
        for k in ["x0", "x1", "x2", "x3"] {
            let r = l.read("ticket", k).await?;
            read.push((notice_of(&r).cloned(), error_of(&r)));
        }
        let (to_worker, to_plain) = (pending(&l, &worker).await?, pending(&l, &plain).await?);
        seen.push((names(to_worker), names(to_plain), read));
    }
    ensure!(
        seen[0] == seen[1] && seen[1].0.len() == 4 && seen[1].1.len() == 4,
        "x0, x1, x2 and x3 make the calls of h0, made while `notify` was low, h1, pending, \
         h2, rejected, and h3, accepted, which are held: their own calls are proposed and \
         wait in both lakes, so the lake that \
         holds them lists (to the writer, to a reviewer without `restricted`) and reads \
         {:?}, and the one without {:?}",
        seen[0],
        seen[1]
    );
    twin.close().await.must("closing the twin")?;
    Ok(())
}

async fn a_record_made_again_reads_none_of_its_keys_decisions(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let token = "made_again_decides_anew";
    store.head().await.must("reading the head")?;
    let twin = twin_of(&store, &given).await?;
    let keys = [
        ("k1", "one"),
        ("k2", "two"),
        ("k3", "three"),
        ("k4", "four"),
    ];
    // Lake A holds o's k1 to k4, outside W's `only`: k1's call rejected,
    // k2's accepted, k3's pending, and k4's rejected after k4 left it; all
    // four retired. Lake B never had them. Then W makes k1 to k4 in both.
    let mut seen = Vec::new();
    for (lake, had) in [(store.clone(), true), (twin.clone(), false)] {
        let l = tickets(lake.as_ref(), token, Risk::High).await?;
        let keeper = reviewer(lake.as_ref(), false);
        if had {
            for (k, s) in keys {
                l.upsert("ticket", k, json!({"subject": s, "note": "o"}))
                    .await?;
            }
            let listed = pending(&l, &keeper).await?;
            let id = |k| of_key(&listed, k).map(|p| p.id.clone()).unwrap_or_default();
            l.cat
                .reject(&keeper, &id("k1"), "k1 is under investigation".into())
                .await
                .must("rejecting k1's call")?;
            accept(lake.as_ref(), &l, &keeper, &id("k2")).await?;
            l.upsert("ticket", "k4", json!({"subject": "four, moved"}))
                .await?;
            l.cat
                .reject(&keeper, &id("k4"), "k4 is secret".into())
                .await
                .must("rejecting the call k4 left")?;
            for (k, _) in keys {
                l.retire("ticket", k).await?;
            }
            let left = pending(&l, &keeper).await?;
            ensure!(
                left.is_empty(),
                "retiring k1 to k4 closes their pending calls; still pending: {:?}",
                left.iter().map(|p| &p.write).collect::<Vec<_>>()
            );
        }
        let only = |default: &str, review: bool| {
            json!({"default": default, "review": review, "types": {
                "ticket": {"default": default, "only": {"note": "w"}}}})
        };
        let w = super::only::holder(lake.as_ref(), "w", only("write", false));
        // A reviewer admitted W's records alone, without `restricted`.
        let rw = super::only::holder(lake.as_ref(), "rw", only("read", true));
        let ty = lakelands_api::TypeName::from("ticket");
        for (k, s) in keys {
            let k = lakelands_api::RecordKey::from(k);
            let fields = patch(json!({"subject": s, "note": "w"}));
            let wrote = l.cat.upsert(&w, &ty, &k, fields, Default::default());
            wrote.await.must("W's write")?;
        }
        let mut read = Vec::new();
        for (k, _) in keys {
            let r = crate::conformance::fixture::read(l.cat.as_ref(), &w, "ticket", k).await?;
            read.push((notice_of(&r).cloned(), error_of(&r)));
        }
        let told = |listed: Vec<Proposal>| {
            let told = listed.into_iter().map(|p| match p.write {
                Some(ProposedWrite::Compute { key, .. }) => {
                    format!("compute {key} {:?} {:?} {}", p.state, p.reason, p.proposer)
                }
                other => format!("{other:?}"),
            });
            told.collect::<Vec<_>>()
        };
        let (to_w, to_rw) = (told(every(&l, &w).await?), told(every(&l, &rw).await?));
        // A keeper lists o's decided calls, a call each, with no inputs: the
        // record there now is W's, and the log's are none (058).
        if had {
            let listed = every(&l, &keeper).await?;
            let decided: Vec<_> = (listed.iter())
                .filter(|p| p.state != ProposalState::Pending)
                .filter_map(|p| match &p.write {
                    Some(ProposedWrite::Compute { inputs, .. }) => Some(inputs),
                    _ => None,
                })
                .collect();
            ensure!(
                decided.len() == 5 && decided.iter().all(|i| i.is_none()),
                "o's five decided calls (k4 made two) are listed to a reviewer with `restricted` and no \
                 `only`, each with no inputs, though W's records stand at their keys; got {:?}",
                decided
            );
        }
        seen.push((read, told(pending(&l, &w).await?), to_w, to_rw));
    }
    ensure!(
        seen[0] == seen[1] && seen[1].1.len() == 4,
        "W makes k1 to k4 again at the keys where o's records outside W's `only` were \
         decided and retired: each proposes its own call and waits, so the lake that had \
         them reads and lists (W's reads, W's pending, every state to W and to a reviewer \
         with W's `only`) {:?}, and the one without {:?}",
        seen[0],
        seen[1]
    );
    twin.close().await.must("closing the twin")?;
    Ok(())
}

async fn a_rekey_closes_a_moved_records_pending_call(store: Store) -> Held {
    let token = "moved_call_closed";
    let l = Lake::of(store.as_ref());
    notify(&l, json!({"token": token, "echo": "subject"}), Risk::High).await?;
    let mut code = field("code", StorageKind::Text);
    code.required = true;
    let mut parcel = type_decl(
        "parcel",
        vec![
            code,
            field("subject", StorageKind::Text),
            computed_by("notice", StorageKind::Text, "notify", &["subject"]),
        ],
    );
    parcel.key = vec!["code".into()];
    l.declare(parcel.clone()).await?;
    l.upsert("parcel", "a", json!({"code": "a", "subject": "x"}))
        .await?;
    let keeper = reviewer(store.as_ref(), false);
    let before = pending(&l, &keeper).await?;
    ensure!(
        of_key(&before, "a").is_some(),
        "a's call is proposed before the re-key; pending {:?}",
        before.iter().map(|p| &p.write).collect::<Vec<_>>()
    );
    // Re-keyed by `subject`: `a` moves to `x`.
    parcel.key = vec!["subject".into()];
    parcel.fields[1].required = true;
    let to = lakelands_api::write::Migration::to(parcel);
    let report = l.cat.migrate(&l.own, to.clone()).await;
    let report = report.must("the preflight of the re-key")?.data;
    let applied = l.cat.migrate_apply(&l.own, to, &report.report_id).await;
    applied.must("re-keying `parcel`")?;
    let logged = log_events(store.as_ref()).await?;
    let migrated = (logged.iter())
        .find(|e| matches!(e.body, EventBody::Migrated { .. }))
        .map(|e| e.position);
    let closed: Vec<_> = (logged.iter())
        .filter(|e| matches!(e.body, EventBody::WriteRejected { .. }))
        .map(|e| e.cause)
        .collect();
    let after = pending(&l, &keeper).await?;
    ensure!(
        migrated.is_some() && closed == vec![migrated] && after.is_empty(),
        "the re-key at {migrated:?} closes a's pending call, caused by it: rejections caused \
         by {closed:?}, pending {:?}",
        after.iter().map(|p| &p.write).collect::<Vec<_>>()
    );
    // A write of x proposes its call on its new key.
    l.upsert("parcel", "x", json!({"code": "b", "subject": "x"}))
        .await?;
    let after = pending(&l, &keeper).await?;
    ensure!(
        after.len() == 1 && of_key(&after, "x").is_some(),
        "a write of the moved record x proposes its call on its new key; pending {:?}",
        after.iter().map(|p| &p.write).collect::<Vec<_>>()
    );
    Ok(())
}

async fn a_call_a_rekey_moved_away_is_not_listed_with_the_record_moved_in(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    let token = "moved_call_not_listed";
    store.head().await.must("reading the head")?;
    let twin = twin_of(&store, &given).await?;
    let text = json!({"kind": "text"});
    let bin = |key: &str| {
        json!({"declare": "type", "name": "bin", "key": [key],
               "fields": [{"name": "code", "type": text, "required": true},
                          {"name": "label", "type": text, "required": key == "label"}]})
    };
    // A slip is keyed by its bin, so a re-key of `bin` moves slips: the
    // reviewer's s1, on b1, moves to the key c2 that o's s2, on b2, leaves.
    let mut seen = Vec::new();
    for (lake, had) in [(store.clone(), true), (twin.clone(), false)] {
        let l = Lake::of(lake.as_ref());
        notify(&l, json!({"token": token, "echo": "subject"}), Risk::High).await?;
        l.declare_json(bin("code")).await?;
        l.declare_json(json!({"declare": "type", "name": "slip", "key": ["bin"],
            "fields": [{"name": "bin", "type": {"kind": "reference", "target": "bin"}, "required": true},
                       {"name": "subject", "type": text},
                       {"name": "note", "type": text},
                       {"name": "notice", "type": text, "authority": "derived",
                        "function": "notify", "depends_on": ["subject"]}]}))
            .await?;
        l.upsert("bin", "c1", json!({"code": "c1", "label": "c2"}))
            .await?;
        l.upsert("bin", "c2", json!({"code": "c2", "label": "c3"}))
            .await?;
        l.upsert(
            "slip",
            "c1",
            json!({"bin": "c1", "subject": "one", "note": "w"}),
        )
        .await?;
        if had {
            l.upsert(
                "slip",
                "c2",
                json!({"bin": "c2", "subject": "two", "note": "o"}),
            )
            .await?;
        }
        let to = lakelands_api::write::Migration::to(
            serde_json::from_value({
                let mut b = bin("label");
                b.as_object_mut().map(|o| o.remove("declare"));
                b
            })
            .expect("a type"),
        );
        let report = l.cat.migrate(&l.own, to.clone()).await;
        let report = report.must("the preflight of the re-key")?.data;
        let applied = l.cat.migrate_apply(&l.own, to, &report.report_id).await;
        applied.must("re-keying `bin`")?;
        let grants = json!({"default": "read", "review": true,
            "types": {"slip": {"default": "read", "only": {"note": "w"}}}});
        let rw = super::only::holder(lake.as_ref(), "rw", grants);
        let told = every(&l, &rw).await?.into_iter().map(|p| match p.write {
            Some(ProposedWrite::Compute { key, inputs, .. }) => {
                format!("compute {key} {:?} {inputs:?}", p.state)
            }
            other => format!("{other:?}"),
        });
        seen.push(told.collect::<Vec<_>>());
    }
    ensure!(
        seen[0] == seen[1],
        "a re-key of `bin` moves the reviewer's slip to c2, which o's slip, outside its \
         `only`, left after proposing its call there: the lake that had o's slip lists \
         {:?} to the reviewer, and the one without {:?}",
        seen[0],
        seen[1]
    );
    twin.close().await.must("closing the twin")?;
    Ok(())
}

async fn a_sweeps_rejection_records_over_failures_alone(store: Store) -> Held {
    let token = "reject_keeps_good";
    let l = Lake::of(store.as_ref());
    let config = json!({"token": token, "echo": "subject", "fail_on": "y"});
    notify(&l, config.clone(), Risk::Low).await?;
    let computed = computed_by("notice", StorageKind::Text, "notify", &["subject"]);
    let fields = vec![field("subject", StorageKind::Text), computed];
    l.declare(type_decl("ticket", fields)).await?;
    l.upsert("ticket", "g1", json!({"subject": "x"})).await?;
    l.upsert("ticket", "g2", json!({"subject": "y"})).await?;
    notify(&l, config, Risk::High).await?;
    let rev = reviewer(store.as_ref(), false);
    l.cat
        .recompute(&l.own, Recompute::Function("notify".into()))
        .await
        .must("recomputing `notify`")?;
    let owed = sweeps(&l, &rev).await?;
    ensure!(
        !owed.is_empty(),
        "redeclaring `notify` high and a `recompute` propose a sweep for g2's failed call"
    );
    for sweep in owed {
        l.cat
            .reject(&rev, &sweep.id, "no".into())
            .await
            .must("rejecting a sweep")?;
    }
    let (g1, g2) = (l.read("ticket", "g1").await?, l.read("ticket", "g2").await?);
    ensure!(
        notice_of(&g1) == Some(&json!("x"))
            && error_of(&g2).is_some_and(|e| e.contains("rejected")),
        "g1's call stands with its output and g2's failed call is rejected: rejecting the \
         sweep records over the failure alone; g1 reads {:?}, g2's error {:?}",
        notice_of(&g1),
        error_of(&g2)
    );
    // An output recorded before the risk rose is read for the record whose
    // value stands on it, and not for one that makes the call anew: g3,
    // which makes g1's call, proposes its own and waits.
    let (before, n) = (store.head().await.must("reading the head")?, made(token));
    l.upsert("ticket", "g3", json!({"subject": "x"})).await?;
    let g3 = l.read("ticket", "g3").await?;
    let since = events_after(store.as_ref(), before).await?;
    let outputs = since
        .iter()
        .filter(|e| matches!(e.body, EventBody::OutputRecorded { .. }));
    let listed = pending(&l, &rev).await?;
    ensure!(
        notice_of(&g3).is_none()
            && outputs.count() == 0
            && made(token) == n
            && of_key(&listed, "g3").is_some(),
        "g3 makes g1's call, whose output was recorded while `notify` was low: g3 proposes \
         its own and waits, with nothing called or recorded; g3 reads {:?}, pending {:?}",
        notice_of(&g3),
        listed.iter().map(|p| &p.write).collect::<Vec<_>>()
    );
    Ok(())
}

async fn a_held_records_call_is_not_there_to_one_not_answered_it(store: Store) -> Held {
    let token = "held_call";
    let l = tickets(store.as_ref(), token, Risk::High).await?;
    l.upsert("ticket", "k1", json!({"subject": "x"})).await?;
    restrict(store.as_ref(), "ticket", "k1", "a hold").await?;
    let plain = crate::conformance::fixture::reviewer(store.as_ref(), false);
    let keeper = reviewer(store.as_ref(), false);
    let (to_plain, to_keeper) = (pending(&l, &plain).await?, pending(&l, &keeper).await?);
    ensure!(
        to_plain.is_empty() && of_key(&to_keeper, "k1").is_some(),
        "the call of a held record is there to a reviewer who holds `restricted` and not to \
         one without it; listed {:?} and {:?}",
        to_plain.iter().map(|p| &p.write).collect::<Vec<_>>(),
        to_keeper.iter().map(|p| &p.write).collect::<Vec<_>>()
    );
    Ok(())
}

async fn a_backing_is_told_the_write_that_set_its_call_off(store: Store) -> Held {
    let token = "call_names_its_cause";
    let l = tickets(store.as_ref(), token, Risk::Low).await?;
    let own = l.own.writer();
    // An upsert, the owner's.
    let wrote = l.upsert("ticket", "c1", json!({"subject": "one"})).await?;
    let first = events_after(
        store.as_ref(),
        Position(wrote.position().0.saturating_sub(4)),
    )
    .await?
    .into_iter()
    .find(|e| matches!(&e.body, EventBody::RecordUpserted { key, .. } if key.as_str() == "c1"));
    let named = causes(token);
    let c = named.last().cloned().flatten();
    ensure!(
        c.as_ref()
            .is_some_and(|c| Some(c.position) == first.as_ref().map(|e| e.position)
                && c.writer == own
                && c.ty.as_str() == "ticket"
                && c.key.as_str() == "c1"
                && c.source.is_none()
                && c.accepted.is_none()),
        "the call c1's upsert made due names that upsert, its writer {own}, ticket c1 and \
         no source; it named {c:?}"
    );
    // An ingest's, with its source, and a reactive write's context.
    let notice = computed_by("notice", StorageKind::Text, "notify", &["subject"]);
    l.declare(type_decl(
        "lead",
        vec![sourced("subject", StorageKind::Text), notice],
    ))
    .await?;
    let rec = SourceRecord {
        ty: "lead".into(),
        key: "c2".into(),
        fields: patch(json!({"subject": "two"})),
        seen_at: None,
        upstream: None,
        source: None,
    };
    l.ingest("crm", vec![rec]).await.must("ingesting c2")?;
    let c = causes(token).last().cloned().flatten();
    ensure!(
        c.as_ref().is_some_and(
            |c| c.key.as_str() == "c2" && c.source.as_ref().map(|s| s.as_str()) == Some("crm")
        ),
        "the call an ingest from `crm` made due names `crm` as its source; it named {c:?}"
    );
    let head = store.head().await.must("reading the head")?;
    let deep = store.catalog(reactive(2, Some(head)));
    let (ty, c3) = (lakelands_api::TypeName::from("ticket"), "c3".into());
    let wrote = deep.upsert(
        &l.own,
        &ty,
        &c3,
        patch(json!({"subject": "three"})),
        Default::default(),
    );
    wrote.await.must("the reactive write")?;
    let c = causes(token).last().cloned().flatten();
    ensure!(
        c.as_ref()
            .is_some_and(|c| c.key.as_str() == "c3" && c.context.depth == 2),
        "a reactive write two deep hands its call that context; it named {c:?}"
    );
    // The same input from another writer is the same call: not made again.
    let n = made(token);
    let worker = writer(store.as_ref());
    let c4 = "c4".into();
    let again = l.cat.upsert(
        &worker,
        &ty,
        &c4,
        patch(json!({"subject": "one"})),
        Default::default(),
    );
    again.await.must("the second writer's write")?;
    ensure!(
        made(token) == n,
        "c4 makes the call c1 made, from another writer: the cause is no part of the \
         call, so nothing is called; it was called {} times more",
        made(token) - n
    );
    // A declaration's sweep names the `declared`, an accept names itself.
    let config = json!({"token": token, "echo": "subject", "v": 2});
    let declared = notify(&l, config, Risk::Low).await?;
    let swept = causes(token);
    let last = swept.last().cloned().flatten();
    ensure!(
        last.as_ref()
            .is_some_and(|c| c.writer == own && c.position <= declared.position()),
        "the calls a declaration makes due name the `declared` and its writer {own}; the \
         last named {last:?}"
    );
    let config = json!({"token": token, "echo": "subject", "v": 3});
    notify(&l, config, Risk::High).await?;
    let rev = reviewer(store.as_ref(), false);
    let listed = sweeps(&l, &rev).await?;
    let sweep = listed.first().map(|p| p.id.clone()).unwrap_or_default();
    let accepted = accept(store.as_ref(), &l, &rev, &sweep).await?;
    let c = causes(token).last().cloned().flatten();
    let by = c.as_ref().and_then(|c| c.accepted.clone());
    ensure!(
        by.as_ref()
            .is_some_and(|a| a.position == accepted && a.reviewer == rev.writer()),
        "a call made on an accept names the accept, at {}, by {}; it named {by:?}",
        accepted.0,
        rev.writer()
    );
    Ok(())
}

async fn an_expr_function_declares_no_risk_and_a_risk_is_described(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let before = store.head().await.must("reading the head")?;
    let answer = l
        .cat
        .declare(
            &l.own,
            serde_json::from_value(json!({
                "declare": "function", "name": "gross", "backing": "expr", "risk": "high",
                "inputs": [{"name": "net", "type": {"kind": "decimal"}}],
                "expr": "net * 1.19"
            }))
            .map_err(|e| Failure::new(format!("the check's function does not read: {e}")))?,
        )
        .await;
    ensure!(
        matches!(invalid_case(&answer), Some(Case::ExprRisk)),
        "an `expr` function reaches nothing outside the lake, so a risk above low is \
         refused; it answered {answer:?}"
    );
    head_unmoved(store.as_ref(), before, "a refused `expr` function").await?;
    notify(
        &l,
        json!({"token": "expr_has_no_risk", "echo": "subject"}),
        Risk::Medium,
    )
    .await?;
    let described = l.describe().await?;
    let risk = (described.functions.iter())
        .find(|f| f.name == "notify")
        .map(|f| f.risk);
    ensure!(
        risk == Some(Risk::Medium),
        "`notify` was declared medium, and describe answers it so; it answers {risk:?}"
    );
    Ok(())
}
