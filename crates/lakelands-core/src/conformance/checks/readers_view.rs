//! A proposal is read as its reader's view
//! (`docs/decisions/013-verbs-run-by-declared-risk.md` and
//! `docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
//! each extended): `proposals` shows a proposed value where a read of the
//! record would show it, and the accept applies the logged write whole.

use std::sync::Arc;

use lakelands_api::ontology::StorageKind;
use lakelands_api::write::{ProposalFilter, ProposedWrite};
use lakelands_api::{Access, LakeStore, Outcome, Principal, ProposalId, TypeName, WriteOpts};
use serde_json::{Value, json};

use crate::conformance::Failure;
use crate::conformance::backing::EXECUTOR;
use crate::conformance::fixture::{Lake, Must, field_access, patch, type_of, value};

check!(
    /// A proposal shows its reader the values it may read, and is accepted whole.
    VALUES_AS_THE_READERS_VIEW = a_proposal_shows_its_reader_the_values_it_may_read_and_is_accepted_whole,
    "013 and 024, extended (lakelands#499): `proposals` shows a proposed value where a read of the record would: a field its reader may not know of and a masked one are not in a proposed patch, a proposed call's payload is whole only to a reader who may read every field the verb sets, or see the type of a verb that reaches outside, and a write to a type the reader may not see is listed without its write; every entry that holds more than its reader is shown says `more`, and no other does; the proposer reads its own proposal by the same rule; the accept applies the logged write whole, what its reviewer was not shown too"
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
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
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
    // What an entry lists: the whole, a part with `more`, or no write.
    let (all, part) = (|v: &Value| json!([v, false]), |v: Value| json!([v, true]));
    let (note, empty, none) = (
        || part(json!({"note": "raise"})),
        || part(json!({})),
        || part(Value::Null),
    );
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
        (unseeing.clone(), vec![none(), none(), none()]),
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

    // The accept applies what was logged, not what its reviewer was shown.
    let by_blind = l.cat.accept(&blind, &raise).await;
    by_blind.must("the reviewer who may not know of `salary` accepting the upsert")?;
    let raised = l.read("invoice", "i1").await?;
    let by_unseeing = l.cat.accept(&unseeing, &call).await;
    by_unseeing.must("the reviewer who may not see `invoice` accepting the call")?;
    let called = l.read("invoice", "i1").await?;
    ensure!(
        raised.fields == patch(json!({"no": "1", "note": "raise", "salary": "250"}))
            && value(&called, "salary") == Some(&json!("300")),
        "the accept applies the logged write whole: after the upsert `i1` must hold the note \
         `raise` and the salary `250`, after the call the salary `300`; it held {:?}, then {:?}",
        raised.fields,
        called.fields
    );
    Ok(())
}
