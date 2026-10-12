//! A proposal is read as its reader's view
//! (`docs/decisions/013-verbs-run-by-declared-risk.md` and
//! `docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
//! each extended): `proposals` shows a proposed value where a read of the
//! record would show it, and the accept applies the logged write whole.

use lakelands_api::ontology::StorageKind;
use lakelands_api::write::{ProposalFilter, ProposedWrite};
use lakelands_api::{
    Access, Error, LakeStore, Missing, Outcome, Principal, ProposalId, Refusal, TypeName, WriteOpts,
};
use serde_json::{Value, json};

use crate::conformance::backing::EXECUTOR;
use crate::conformance::fixture::{Lake, Must, field_access, patch, type_of, value};
use crate::conformance::{Failure, Held, Store};

check!(
    VALUES_AS_THE_READERS_VIEW =
        a_proposal_shows_its_reader_the_values_it_may_read_and_is_accepted_whole,
    "013 and 024, extended (lakelands#499): `proposals` shows a proposed value where a read of the record would: a field its reader may not know of and a masked one are not in a proposed patch, a proposed call's payload is whole only to a reader who may read every field the verb sets, or see the type of a verb that reaches outside, and a write to a type the reader may not see is not listed (lakelands#762); every entry that holds more than its reader is shown says `more`, and no other does; the proposer reads its own proposal by the same rule; the accept by a reviewer listed the entry whole applies the logged write, a reviewer listed it with `more` is refused it as not granted, naming nothing, and rejects it (lakelands#867), and one not shown the type is answered as for a proposal that does not exist (lakelands#762)"
);

/// The agent `id` holding `review`, reading `invoice` but for its `salary`,
/// held at `salary`.
fn reviewer(store: &dyn LakeStore, id: &str, salary: Access) -> Principal {
    let mut who = field_access(store, id, "invoice", &[("salary", salary)]);
    who.grants.review = true;
    who
}

fn proposed(o: Outcome) -> Result<ProposalId, Failure> {
    match o {
        Outcome::Proposed { proposal, .. } => Ok(proposal),
        other => Err(Failure::new(format!(
            "a proposal answers `proposed`; it answered {other:?}"
        ))),
    }
}

/// Each proposal as `who` lists it, newest first: its patch or payload,
/// null where it shows no write, and whether it says there is more.
async fn listed(l: &Lake, who: &Principal) -> Result<Vec<Value>, Failure> {
    let all = l.cat.proposals(who, ProposalFilter::default()).await;
    let entries = all.must("listing proposals")?.data.into_iter();
    Ok(entries
        .map(|p| match p.write {
            Some(ProposedWrite::Upsert { patch, .. }) => json!([patch, p.more]),
            Some(ProposedWrite::Call { payload, .. }) => json!([payload, p.more]),
            _ => json!([null, p.more]),
        })
        .collect())
}

async fn a_proposal_shows_its_reader_the_values_it_may_read_and_is_accepted_whole(
    store: Store,
) -> Held {
    let (s, l, text) = (store.as_ref(), Lake::of(store.as_ref()), StorageKind::Text);
    let fields = [("no", text), ("note", text), ("salary", text)];
    l.declare(type_of("invoice", &fields)).await?;
    let stored = json!({"no": "1", "note": "n", "salary": "100"});
    l.upsert("invoice", "i1", stored).await?;
    l.declare_json(
        json!({"declare": "verb", "name": "set_salary", "on": "invoice", "risk": "high",
        "payload": [{"name": "amount", "type": {"kind": "text"}},
                    {"name": "reason", "type": {"kind": "text"}}],
        "effect": "write", "set": [{"field": "salary", "value": "payload.amount"}]}),
    )
    .await?;
    // The proposer writes both fields; later it may not know of `salary`.
    let writes = [("note", Access::Write), ("salary", Access::Write)];
    let clerk = field_access(s, "clerk", "invoice", &writes);
    let demoted = field_access(s, "clerk", "invoice", &[("salary", Access::None)]);
    let (whole, sent) = (
        json!({"note": "raise", "salary": "250"}),
        json!({"amount": "300", "reason": "merit"}),
    );
    let (ty, key) = (TypeName::from("invoice"), "i1".into());
    let raise = ProposedWrite::Upsert {
        ty,
        key,
        patch: patch(whole.clone()),
        opts: WriteOpts::default(),
    };
    let raise = l.cat.propose(&clerk, raise, "a raise".into()).await;
    let raise = proposed(raise.must("the clerk proposing the upsert")?)?;
    let (verb, key, opts) = ("set_salary".into(), "i1".into(), WriteOpts::default);
    let call = l
        .cat
        .call(&clerk, &verb, Some(&key), patch(sent.clone()), opts());
    let call = proposed(call.await.must("the clerk calling the high verb")?)?;
    // A verb that reaches outside sets no field: its payload is its type's.
    l.declare_json(
        json!({"declare": "verb", "name": "notify", "on": "invoice", "risk": "high",
        "payload": [{"name": "text", "type": {"kind": "text"}}],
        "effect": "external", "executor": EXECUTOR}),
    )
    .await?;
    let (notify, told) = ("notify".into(), json!({"text": "hello"}));
    let notify = l
        .cat
        .call(&l.own, &notify, Some(&key), patch(told.clone()), opts());
    proposed(notify.await.must("the owner calling the external verb")?)?;

    let blind = reviewer(s, "blind", Access::None);
    let mut unseeing = reviewer(s, "unseeing", Access::Read);
    let none = Some(Access::None);
    (unseeing.grants.types.values_mut()).for_each(|t| (t.default, t.fields) = (none, [].into()));
    // What an entry lists: the whole, or a part with `more`.
    let (all, part) = (|v: &Value| json!([v, false]), |v: Value| json!([v, true]));
    let (note, empty) = (|| part(json!({"note": "raise"})), || part(json!({})));
    let who = [
        "a reader of `salary`",
        "one who may not know of `salary`",
        "one who sees `salary` masked",
        "one who may not see `invoice`",
        "the proposer",
        "the proposer, `salary` hidden since",
    ];
    let rows = [
        (
            reviewer(s, "reads", Access::Read),
            vec![all(&told), all(&sent), all(&whole)],
        ),
        (blind.clone(), vec![all(&told), empty(), note()]),
        (
            reviewer(s, "masked", Access::Masked),
            vec![all(&told), empty(), note()],
        ),
        // Not shown `invoice`, it is listed none of them (lakelands#762).
        (unseeing.clone(), vec![]),
        (clerk, vec![all(&sent), all(&whole)]),
        (demoted, vec![empty(), note()]),
    ];
    let mut red = Vec::new();
    for (who, (reader, want)) in who.into_iter().zip(rows) {
        let got = listed(&l, &reader).await?;
        if got != want {
            let (want, got) = (json!(want), json!(got));
            red.push(format!("{who} must list {want}; it lists {got}"));
        }
    }
    ensure!(
        red.is_empty(),
        "`proposals` shows a proposed value where a read of the record would, and says where \
         there is more; newest first an external verb's payload, a call's and an upsert's \
         patch: {}",
        red.join("; ")
    );

    // Only a reviewer listed the entry whole accepts it, and the accept
    // applies what was logged; one listed it in part rejects it.
    let refused = Error::Refused {
        field: None,
        reason: Refusal::NotGranted,
    };
    let by_blind = l.cat.accept(&blind, &raise).await.map(|_| ());
    ensure!(
        by_blind == Err(refused.clone()),
        "the reviewer who may not know of `salary` accepting the upsert must be refused as not \
         granted, naming nothing; it answered {by_blind:?}"
    );
    let by_reader = l
        .cat
        .accept(&reviewer(s, "reads", Access::Read), &raise)
        .await;
    by_reader.must("the reviewer who reads `salary` accepting the upsert")?;
    let raised = l.read("invoice", "i1").await?;
    // One not shown `invoice` is not shown the proposal (lakelands#762).
    let by_unseeing = l.cat.accept(&unseeing, &call).await.map(|_| ());
    let missing = Missing::Proposal { id: call.clone() };
    ensure!(
        by_unseeing == Err(Error::NotFound { missing }),
        "the reviewer who may not see `invoice` accepting the call must be answered as for a \
         proposal that does not exist; it answered {by_unseeing:?}"
    );
    let by_blind = l.cat.accept(&blind, &call).await.map(|_| ());
    let rejected = l.cat.reject(&blind, &call, "not now".into()).await;
    rejected.must("the reviewer who may not know of `salary` rejecting the call")?;
    let called = l.read("invoice", "i1").await?;
    ensure!(
        by_blind == Err(refused)
            && raised.fields == patch(json!({"no": "1", "note": "raise", "salary": "250"}))
            && value(&called, "salary") == Some(&json!("250")),
        "the accept applies the logged write whole, and one listed in part is refused: after \
         the upsert `i1` must hold the note `raise` and the salary `250`, the call refused to \
         the reviewer who may not know of `salary` and rejected by it; it held {:?}, the accept \
         answered {by_blind:?}, then it held {:?}",
        raised.fields,
        called.fields
    );
    Ok(())
}
