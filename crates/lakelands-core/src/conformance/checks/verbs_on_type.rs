//! A verb called on its type, with no record
//! (`docs/decisions/013-verbs-run-by-declared-risk.md` and
//! `docs/decisions/021-the-write-context-belongs-to-the-host.md`, each
//! extended).
//!
//! An effect that creates something outside the lake has no record to be
//! called on yet. A verb declared with `scope: type` is called with no
//! key, under the grants of its type, by its declared risk, and its
//! executor's answer is recorded as for any external verb.

use std::sync::Arc;

use lakelands_api::event::EventBody;
use lakelands_api::ontology::{StorageKind, VerbScope};
use lakelands_api::principal::{Access, TypeGrants};
use lakelands_api::seam::FeedFilter;
use lakelands_api::write::{Declaration, ProposalFilter, ProposedWrite};
use lakelands_api::{
    Grants, IdempotencyKey, LakeStore, Outcome, Position, Principal, PrincipalKind, ReadAt,
    Refusal, TypeName, Version, WriteOpts,
};
use serde_json::{Value, json};

use crate::conformance::Failure;
use crate::conformance::backing::{EXECUTOR, executed};
use crate::conformance::fixture::{
    Lake, Must, field, head_unmoved, invalid_naming, owner, patch, principal, reason, reviewer,
    type_decl,
};

check!(
    /// A verb's events name no type.
    VERB_EVENTS_UNTYPED = a_feed_filtered_by_type_passes_none_of_a_verbs_events,
    "004 and seams §ChangeFeed: a verb's events name no type, even for a verb on its type, so a feed filtered by that type passes none of `verb_called`, `write_proposed`, `write_accepted` and `output_recorded`, which a feed with no types passes; `proposals` filtered by the type lists the proposed call"
);

check!(
    /// A verb on its type runs under its type's grants and records its
    /// output.
    ON_TYPE_RUNS = a_verb_on_its_type_runs_under_its_types_grants_and_records_its_output,
    "013, extended: a `low` external verb declared with `scope: type` is described with that scope and called with no key by a principal with write on its type; it answers `applied` with its executor's output, its `verb_called` carries no key and is written in log format 8, and the output is recorded as `output_recorded` caused by it"
);

check!(
    /// A verb on its type at a higher risk is proposed, and runs when
    /// accepted.
    ON_TYPE_PROPOSED = a_verb_on_its_type_at_medium_risk_is_proposed_and_runs_when_accepted,
    "013, extended: a `medium` verb on its type is recorded as a proposal whose call carries no key and runs nothing; accepting it runs its executor once and records the output, caused by the acceptance"
);

check!(
    /// A verb on its type needs write on its type.
    ON_TYPE_GRANTS = a_verb_on_its_type_without_write_on_its_type_is_not_granted,
    "006 and 013, extended: a principal that may write another type but only read the verb's type is refused not_granted when it calls the verb on that type, and nothing is appended"
);

check!(
    /// A retry of a verb on its type answers the first outcome.
    ON_TYPE_IDEMPOTENT = a_verb_on_its_type_retried_with_its_key_answers_the_first_outcome,
    "013, extended, and 015: a verb on its type called again with the same idempotency key answers the first outcome, appends no second `verb_called` and runs its executor once"
);

check!(
    /// A key that does not fit the verb's scope is refused, naming the
    /// verb.
    ON_TYPE_KEY = a_key_that_does_not_fit_the_verbs_scope_is_refused_naming_the_verb,
    "013, extended: a key passed to a verb on its type, none passed to a verb on a record, and an expected version with no key are each refused as invalid naming the verb, before grants are read, and nothing is appended"
);

check!(
    /// A verb on its type neither writes nor requires.
    ON_TYPE_DECLARED = a_verb_on_its_type_with_a_write_effect_or_requires_is_refused,
    "013, extended: a verb declared with `scope: type` and a `write` effect, or with `requires`, is refused as invalid naming the verb, since it has no record to set or to read, and nothing is appended"
);

/// A principal that reads everything and writes only `ty`.
fn type_writer(store: &dyn LakeStore, id: &str, ty: &str) -> Principal {
    let mut grants = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    grants.types.insert(
        TypeName::from(ty),
        TypeGrants {
            default: Some(Access::Write),
            fields: Default::default(),
        },
    );
    principal(store, PrincipalKind::Agent, id, grants)
}

/// `who` calling `verb` on its type with `payload`, with no options.
async fn on_type(
    l: &Lake,
    who: &Principal,
    verb: &str,
    payload: Value,
) -> lakelands_api::Result<Outcome> {
    let opts = WriteOpts::default();
    l.cat
        .call(who, &verb.into(), None, patch(payload), opts)
        .await
}

/// `payment` and `partner`, and the external verbs on them: `name` on
/// `payment`'s type at `risk`, and `register` on a `partner` record.
async fn ledger(store: &dyn LakeStore, name: &str, risk: &str) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    l.declare(type_decl("payment", vec![field("ref", StorageKind::Text)]))
        .await?;
    l.declare(type_decl("partner", vec![field("name", StorageKind::Text)]))
        .await?;
    for verb in [
        json!({"declare": "verb", "name": name, "on": "payment", "scope": "type", "risk": risk,
               "payload": [{"name": "amount", "type": {"kind": "number"}}],
               "effect": "external", "executor": EXECUTOR}),
        json!({"declare": "verb", "name": "register", "on": "partner", "risk": "low",
               "effect": "external", "executor": EXECUTOR}),
    ] {
        l.declare_json(verb).await?;
    }
    Ok(l)
}

async fn events_from(
    store: &dyn LakeStore,
    from: Position,
) -> Result<Vec<lakelands_api::event::Event>, Failure> {
    Ok(store
        .feed()
        .poll(from, &FeedFilter::default(), 1_000)
        .await
        .must("reading the feed")?
        .events)
}

/// The `verb_called` events of `verb` in `events`, with their keys.
fn calls_of(events: &[lakelands_api::event::Event], verb: &str) -> Vec<(Position, bool)> {
    events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::VerbCalled { verb: v, key, .. } if v.as_str() == verb => {
                Some((e.position, key.is_some()))
            }
            _ => None,
        })
        .collect()
}

async fn a_verb_on_its_type_runs_under_its_types_grants_and_records_its_output(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = ledger(store.as_ref(), "create_payment", "low").await?;
    let clerk = type_writer(store.as_ref(), "clerk", "payment");
    let described = l
        .cat
        .describe(&clerk, ReadAt::default())
        .await
        .must("describing the lake")?
        .data;
    let scope = described
        .verbs
        .iter()
        .find(|v| v.name.as_str() == "create_payment")
        .map(|v| v.scope);
    ensure!(
        scope == Some(VerbScope::Type),
        "`create_payment` is described as a verb on its type; it was described with {scope:?}"
    );
    let before = store.head().await.must("reading the head")?;
    let runs = executed("create_payment");
    let outcome = on_type(&l, &clerk, "create_payment", json!({"amount": 12}))
        .await
        .must("the clerk calling `create_payment` on its type")?;
    let Outcome::Applied {
        position,
        output: Some(output),
        ..
    } = &outcome
    else {
        return Err(Failure::new(format!(
            "a low verb on its type answers `applied` with its executor's output; it answered \
             {outcome:?}"
        )));
    };
    ensure!(
        *output == json!({"done": "create_payment", "record": null})
            && executed("create_payment") == runs + 1,
        "the executor answered {{\"done\": \"create_payment\", \"record\": null}} once; the \
         outcome carries {output} after {} runs",
        executed("create_payment") - runs
    );
    let events = events_from(store.as_ref(), before.next()).await?;
    let calls = calls_of(&events, "create_payment");
    ensure!(
        calls == [(*position, false)],
        "one `verb_called` without a key at {position}; the log holds {calls:?}"
    );
    let called_in = events.iter().find(|e| e.position == *position).map(|e| e.v);
    ensure!(
        called_in == Some(8),
        "a call with no key is written in log format 8; it was written in {called_in:?}"
    );
    ensure!(
        events.iter().any(|e| e.cause == Some(*position)
            && matches!(&e.body, EventBody::OutputRecorded { output: o, .. } if o == output)),
        "the executor's answer is recorded as `output_recorded` caused by the `verb_called` at \
         {position}"
    );
    Ok(())
}

async fn a_verb_on_its_type_at_medium_risk_is_proposed_and_runs_when_accepted(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = ledger(store.as_ref(), "open_ticket", "medium").await?;
    let clerk = type_writer(store.as_ref(), "clerk", "payment");
    let runs = executed("open_ticket");
    let proposed = on_type(&l, &clerk, "open_ticket", json!({"amount": 3}))
        .await
        .must("the clerk calling `open_ticket` on its type")?;
    let Outcome::Proposed {
        position, proposal, ..
    } = &proposed
    else {
        return Err(Failure::new(format!(
            "a medium verb on its type is proposed; it answered {proposed:?}"
        )));
    };
    let events = events_from(store.as_ref(), *position).await?;
    ensure!(
        matches!(events.first().map(|e| &e.body), Some(EventBody::WriteProposed {
            write: ProposedWrite::Call { key: None, verb, .. }, ..
        }) if verb.as_str() == "open_ticket")
            && executed("open_ticket") == runs,
        "the proposal records a call of `open_ticket` with no key and runs nothing; the log \
         holds {:?}, and the executor ran {} times",
        events.first().map(|e| &e.body),
        executed("open_ticket") - runs
    );
    let reviewer = reviewer(store.as_ref(), false);
    l.cat
        .accept(&reviewer, proposal)
        .await
        .must("the reviewer accepting the proposal")?;
    ensure!(
        executed("open_ticket") == runs + 1,
        "accepting runs the executor once; it ran {} times",
        executed("open_ticket") - runs
    );
    let events = events_from(store.as_ref(), *position).await?;
    let at = events
        .iter()
        .find(|e| matches!(&e.body, EventBody::WriteAccepted { proposal: p } if p == proposal))
        .map(|e| e.position)
        .ok_or_else(|| Failure::new("accepting appends `write_accepted`"))?;
    ensure!(
        calls_of(&events, "open_ticket")
            .iter()
            .all(|(_, keyed)| !keyed)
            && events.iter().any(|e| e.cause == Some(at)
                && matches!(&e.body, EventBody::OutputRecorded { output, .. }
                    if *output == json!({"done": "open_ticket", "record": null}))),
        "the acceptance at {at} runs `open_ticket` with no key and records the executor's \
         answer caused by it; the log holds {:?}",
        events.iter().map(|e| &e.body).collect::<Vec<_>>()
    );
    Ok(())
}

async fn a_verb_on_its_type_without_write_on_its_type_is_not_granted(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = ledger(store.as_ref(), "settle_payment", "low").await?;
    let outsider = type_writer(store.as_ref(), "outsider", "partner");
    let before = store.head().await.must("reading the head")?;
    let runs = executed("settle_payment");
    match on_type(&l, &outsider, "settle_payment", json!({})).await {
        Err(e) if matches!(reason(&e), Some(Refusal::NotGranted)) => {}
        other => {
            return Err(Failure::new(format!(
                "a principal that only reads `payment` is refused not_granted calling a verb on \
                 it; it answered {other:?}"
            )));
        }
    }
    ensure!(
        executed("settle_payment") == runs,
        "a refused call runs no executor"
    );
    head_unmoved(
        store.as_ref(),
        before,
        "calling `settle_payment` without write on `payment`",
    )
    .await
}

async fn a_verb_on_its_type_retried_with_its_key_answers_the_first_outcome(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = ledger(store.as_ref(), "book_payment", "low").await?;
    let before = store.head().await.must("reading the head")?;
    let runs = executed("book_payment");
    let once = WriteOpts {
        idempotency_key: Some(IdempotencyKey::from("book-once")),
        ..WriteOpts::default()
    };
    let call = || {
        let (cat, own, once) = (l.cat.clone(), l.own.clone(), once.clone());
        async move {
            cat.call(
                &own,
                &"book_payment".into(),
                None,
                patch(json!({"amount": 5})),
                once,
            )
            .await
        }
    };
    let first = call().await.must("calling `book_payment`")?;
    let again = call().await.must("calling `book_payment` again")?;
    ensure!(
        first.position() == again.position(),
        "a retry with the same idempotency key answers the first outcome's position, {}; it \
         answered {again:?}",
        first.position()
    );
    let events = events_from(store.as_ref(), before.next()).await?;
    let calls = calls_of(&events, "book_payment");
    ensure!(
        calls.len() == 1 && executed("book_payment") == runs + 1,
        "one `verb_called` and one run of the executor; the log holds {calls:?}, and the \
         executor ran {} times",
        executed("book_payment") - runs
    );
    Ok(())
}

async fn a_key_that_does_not_fit_the_verbs_scope_is_refused_naming_the_verb(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = ledger(store.as_ref(), "issue_payment", "low").await?;
    l.upsert("partner", "p1", json!({"name": "Acme"})).await?;
    // A principal with no grant at all: the key is checked before the
    // grants are, so the refusal is `invalid`, not `not_granted`.
    let nobody = principal(
        store.as_ref(),
        PrincipalKind::Agent,
        "nobody",
        Grants::default(),
    );
    let at_version = WriteOpts {
        expected_version: Some(Version(1)),
        ..WriteOpts::default()
    };
    for (what, verb, key, opts) in [
        (
            "a key passed to `issue_payment`, a verb on its type",
            "issue_payment",
            Some("x"),
            WriteOpts::default(),
        ),
        (
            "no key passed to `register`, a verb on a record",
            "register",
            None,
            WriteOpts::default(),
        ),
        (
            "an expected version passed to `issue_payment` with no key",
            "issue_payment",
            None,
            at_version,
        ),
    ] {
        let before = store.head().await.must("reading the head")?;
        let key = key.map(lakelands_api::RecordKey::from);
        invalid_naming(
            what,
            l.cat
                .call(&nobody, &verb.into(), key.as_ref(), patch(json!({})), opts)
                .await,
            &[verb],
        )?;
        head_unmoved(store.as_ref(), before, what).await?;
    }
    Ok(())
}

async fn a_verb_on_its_type_with_a_write_effect_or_requires_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("payment", vec![field("ref", StorageKind::Text)]))
        .await?;
    let own = owner(store.as_ref());
    for (what, verb) in [
        (
            "a verb on its type with a write effect",
            json!({"declare": "verb", "name": "stamp", "on": "payment", "scope": "type",
                   "risk": "low", "effect": "write", "set": [{"field": "ref", "value": "'x'"}]}),
        ),
        (
            "a verb on its type with `requires`",
            json!({"declare": "verb", "name": "guarded", "on": "payment", "scope": "type",
                   "risk": "low", "requires": "ref is not null",
                   "effect": "external", "executor": EXECUTOR}),
        ),
    ] {
        let name = verb["name"].as_str().unwrap_or_default().to_owned();
        let d: Declaration = serde_json::from_value(verb)
            .map_err(|e| Failure::new(format!("the check's verb does not read: {e}")))?;
        let before = store.head().await.must("reading the head")?;
        invalid_naming(what, l.cat.declare(&own, d).await, &[&name])?;
        head_unmoved(store.as_ref(), before, what).await?;
    }
    Ok(())
}

async fn a_feed_filtered_by_type_passes_none_of_a_verbs_events(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = ledger(store.as_ref(), "open_ticket", "medium").await?;
    let from = store.head().await.must("reading the head")?.next();
    let proposed = on_type(&l, &l.own, "open_ticket", json!({"amount": 3}))
        .await
        .must("calling `open_ticket` on its type")?;
    let Outcome::Proposed { proposal, .. } = &proposed else {
        return Err(Failure::new(format!(
            "a medium verb on its type is proposed; it answered {proposed:?}"
        )));
    };
    let filter = ProposalFilter {
        ty: Some("payment".into()),
        ..ProposalFilter::default()
    };
    let listed = l
        .cat
        .proposals(&l.own, filter)
        .await
        .must("listing `payment`'s proposals")?
        .data;
    ensure!(
        listed.iter().any(|p| &p.id == proposal),
        "proposals filtered by `payment` list the call of `open_ticket`, a verb on it; they \
         listed {listed:?}"
    );
    l.cat
        .accept(&reviewer(store.as_ref(), false), proposal)
        .await
        .must("accepting the call")?;
    let kinds = |types: Vec<TypeName>| {
        let store = store.clone();
        async move {
            let filter = FeedFilter {
                types,
                ..FeedFilter::default()
            };
            let batch = store.feed().poll(from, &filter, 100).await;
            Ok::<_, Failure>(
                batch
                    .must("reading the feed")?
                    .events
                    .iter()
                    .map(|e| e.body.kind())
                    .collect::<Vec<_>>(),
            )
        }
    };
    let verbs = [
        "write_proposed",
        "write_accepted",
        "verb_called",
        "output_recorded",
    ];
    let all = kinds(Vec::new()).await?;
    ensure!(
        verbs.iter().all(|k| all.contains(k)),
        "a feed with no types passes all four of the call's events {verbs:?}; it passed {all:?}"
    );
    let typed = kinds(vec!["payment".into()]).await?;
    ensure!(
        typed.iter().all(|k| !verbs.contains(k)),
        "a verb's events name no type, so a feed filtered by `payment` passes none of \
         {verbs:?}; it passed {typed:?}"
    );
    Ok(())
}
