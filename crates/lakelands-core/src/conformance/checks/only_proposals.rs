//! A holder of `only` proposes writes to its own records, each pinned to
//! the record and its type as they stood, and decides the proposals it is
//! shown whole (`docs/decisions/051-a-grant-may-admit-only-some-records.md`).

use lakelands_api::error::Case;
use lakelands_api::event::EventBody;
use lakelands_api::ontology::StorageKind::{Number, Text};
use lakelands_api::write::{Outcome, Recompute};
use lakelands_api::{Error, Principal, ProposalId, RecordKey, Refusal, VerbName};
use serde_json::{Value, json};

use super::only::{alice, claims, de, holder, said};
use crate::conformance::backing::{KIND, plug, unplug};
use crate::conformance::fixture::{
    Lake, Must, computed_by, events_from, field, head_unmoved, type_decl,
};
use crate::conformance::{Failure, Held, Store};

check!(
    ONLY_PROPOSES = a_holder_of_only_proposes_its_own_records,
    "051: a holder of `only` proposes an upsert, a retirement and a high-risk call on its own records, each logged `pinned`, another's proposals not; on another's record a proposal that would pass is not granted and one that reads the record is not found; a proposal is refused where a field its `only` names is derived or backed, or where the verb assigns one, and neither `describe` nor `describe_type` lists it such a verb; the accept of a pinned proposal applies while its record and type stand as proposed, and is refused once a rederive, a backed output, a record made again at the key, a redeclaration of the type that leaves its records as they are, a migration or a re-key moved them, or, for a link, once either end record moved"
);

check!(
    ONLY_DECIDES = a_holder_of_only_decides_what_it_is_shown_whole,
    "051: a reviewer holding `only` accepts and rejects a proposal its listing shows it whole; one on another's record it is refused, appending nothing, before it is told the proposal is decided, so a guessed id tells it nothing; and its accept is refused where the write, its derived values computed, would leave the record outside its pairs"
);

/// `who`'s proposal of the write `json`, its id or what refused it.
async fn proposes(l: &Lake, who: &Principal, json: Value) -> Result<ProposalId, String> {
    match l.cat.propose(who, de(json), "mine".into()).await {
        Ok(Outcome::Proposed { proposal, .. }) => Ok(proposal),
        other => Err(said(other)),
    }
}

/// The write of `field` to `value` on `ty`'s record `key`.
fn upsert(ty: &str, key: &str, field: &str, value: Value) -> Value {
    json!({"write": "upsert", "type": ty, "key": key, "patch": {field: value}})
}

/// What a refusal as not granted, naming no field, says.
fn not_granted() -> String {
    said::<()>(Err(Error::Refused {
        field: None,
        reason: Refusal::NotGranted,
    }))
}

/// Fails unless every row answered what it wants.
fn all_right(rows: &[(String, String, String)], what: &str) -> Held {
    let wrong: Vec<_> = rows.iter().filter(|(_, said, want)| said != want).collect();
    ensure!(wrong.is_empty(), "{what}: {wrong:?}");
    Ok(())
}

async fn a_holder_of_only_proposes_its_own_records(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let cat = l.cat.as_ref();
    let start = claims(&l, false).await?;
    for (name, field, value) in [
        ("propose_pay", "note", "'paid'"),
        ("give", "owner", "'bob'"),
    ] {
        let set = json!([{"field": field, "value": value}]);
        l.declare_json(
            json!({"declare": "verb", "name": name, "on": "claim", "risk": "high",
                              "effect": "write", "set": set}),
        )
        .await?;
    }
    // `bill`, keyed by its code: `twice` derived by `double`, `name` filled
    // by the backing; `ticket`, whose owner is derived.
    let (n, code) = (
        json!({"name": "amount", "type": {"kind": "number"}}),
        json!({"name": "code", "type": {"kind": "text"}}),
    );
    l.declare_json(
        json!({"declare": "function", "name": "double", "backing": "expr",
                          "inputs": [n], "expr": "amount * 2"}),
    )
    .await?;
    let token = "only_pinned";
    l.declare_json(
        json!({"declare": "function", "name": "pin_lookup", "backing": KIND,
                          "inputs": [code], "config": {"token": token, "echo": "code"}}),
    )
    .await?;
    let mut bill = type_decl(
        "bill",
        vec![
            field("owner", Text),
            field("code", Text),
            field("amount", Number),
            computed_by("twice", Number, "double", &["amount"]),
            computed_by("name", Text, "pin_lookup", &["code"]),
        ],
    );
    bill.key = vec!["code".into()];
    // Required, as a key field is, and the owner too, for the re-key.
    bill.fields[..2].iter_mut().for_each(|f| f.required = true);
    l.declare(bill.clone()).await?;
    l.declare_json(json!({"declare": "type", "name": "ticket", "fields": [
        {"name": "n", "type": {"kind": "text"}},
        {"name": "owner", "type": {"kind": "text"}, "authority": "derived", "derived": "n"}]}))
        .await?;
    for key in ["k1", "k2", "k3", "k4"] {
        if key == "k2" {
            unplug(token);
        }
        let made = json!({"owner": "alice", "code": key, "amount": 1});
        l.upsert("bill", key, made).await?;
    }
    plug(token);
    let hers = json!({"only": {"owner": "alice"}});
    let types = json!({"claim": hers, "bill": hers, "ticket": hers});
    let alice = alice(store.as_ref(), json!({"default": "write", "types": types}));
    let bob = holder(store.as_ref(), "bob", json!({"default": "write"}));
    let (ng, gone) = (not_granted(), "not found");
    let mut rows: Vec<(String, String, String)> = Vec::new();
    let mut row = |what: &str, said: String, want: &str| {
        rows.push((what.to_owned(), said, want.to_owned()));
    };
    let call = |who: Principal, verb: &'static str, key: &'static str| async move {
        let (verb, key) = (VerbName::from(verb), RecordKey::from(key));
        cat.call(
            &who,
            &verb,
            Some(&key),
            Default::default(),
            Default::default(),
        )
        .await
    };
    // Her own records she proposes to write; each proposal is pinned.
    let mut pinned = Vec::new();
    for write in [
        upsert("claim", "c1", "note", json!("x")),
        json!({"write": "retire", "type": "claim", "key": "c3"}),
    ] {
        let p = proposes(&l, &alice, write).await;
        row(
            "her proposal",
            p.as_ref().map_or_else(Clone::clone, |_| "proposed".into()),
            "proposed",
        );
        pinned.extend(p.ok());
    }
    match call(alice.clone(), "propose_pay", "c1").await {
        Ok(Outcome::Proposed { proposal, .. }) => pinned.push(proposal),
        other => row("a high-risk verb on her `c1`", said(other), "proposed"),
    }
    let bobs = proposes(&l, &bob, upsert("claim", "c2", "note", json!("y"))).await;
    let bobs = bobs.map_err(Failure::new)?;
    // Another's record is not hers to propose for; nor is a pair that
    // anything but the write could move.
    let head = store.head().await.must("reading the head")?;
    let p = proposes(&l, &alice, upsert("claim", "c2", "note", json!("x"))).await;
    row("a proposal on bob's `c2`", p.unwrap_err_or(), &ng);
    let p = proposes(&l, &alice, upsert("claim", "c9", "owner", json!("bob"))).await;
    row(
        "a proposal of a claim with bob's owner",
        p.unwrap_err_or(),
        &ng,
    );
    let p = proposes(
        &l,
        &alice,
        json!({"write": "retire", "type": "claim", "key": "c2"}),
    )
    .await;
    row("a retirement of bob's `c2`", p.unwrap_err_or(), gone);
    row(
        "a high-risk verb on bob's `c2`",
        said(call(alice.clone(), "propose_pay", "c2").await),
        gone,
    );
    row(
        "a verb that assigns her owner",
        said(call(alice.clone(), "give", "c1").await),
        &ng,
    );
    let given = json!({"write": "call", "verb": "give", "key": "c1", "payload": {}});
    row(
        "a proposal of that verb",
        proposes(&l, &alice, given).await.unwrap_err_or(),
        &ng,
    );
    let p = proposes(&l, &alice, upsert("ticket", "t1", "n", json!("x"))).await;
    row(
        "a proposal on a type whose owner is derived",
        p.unwrap_err_or(),
        &ng,
    );
    head_unmoved(store.as_ref(), head, "every refused proposal by alice").await?;
    let told = cat
        .describe_type(&alice, &"claim".into(), Default::default())
        .await;
    let verbs = told.must("describing `claim` to alice")?.data.verbs;
    let verbs: Vec<&str> = verbs.iter().map(VerbName::as_str).collect();
    row("the verbs described to her", verbs.join(" "), "propose_pay");
    let lake = cat.describe(&alice, Default::default()).await;
    let lake = lake.must("describing the lake to alice")?.data.verbs;
    let lake: Vec<&str> = lake.iter().map(|v| v.name.as_str()).collect();
    row(
        "the lake's verbs described to her",
        lake.join(" "),
        "propose_pay",
    );
    // The log says which proposals are pinned.
    let mut logged = Vec::new();
    for e in events_from(store.as_ref(), start).await? {
        if let EventBody::WriteProposed {
            proposal,
            pinned: true,
            ..
        } = &e.body
        {
            logged.push(proposal.clone());
        }
    }
    row(
        "the pinned proposals",
        format!("{logged:?}"),
        &format!("{pinned:?}"),
    );
    ensure!(!logged.contains(&bobs), "bob's proposal must not be pinned");
    // Her proposal applies while its record and type stand as proposed,
    // and is refused once something moved them.
    let accepted = cat.accept(&l.own, &pinned[0]).await;
    row(
        "the accept of her proposal",
        said(accepted.map(|_| ())),
        "applied",
    );
    let moved = |key: &str| {
        let (ty, key) = ("bill".into(), Some(key.into()));
        said::<()>(Err(crate::invalid(Case::WriteChangedSinceProposed {
            ty,
            key,
        })))
    };
    let own = &l.own;
    let accept = |p: ProposalId| async move { said(cat.accept(own, &p).await) };
    let amount = |key: &str| upsert("bill", key, "amount", json!(5));
    // A rederive: `double` is declared again.
    let p = proposes(&l, &alice, amount("k1"))
        .await
        .map_err(Failure::new)?;
    l.declare_json(json!({"declare": "function", "name": "double", "backing": "expr",
                          "inputs": [{"name": "amount", "type": {"kind": "number"}}], "expr": "amount * 3"}))
        .await?;
    row("after a rederive", accept(p).await, &moved("k1"));
    // A backed output: `k2`'s name, cleared by a failed call, is filled.
    let p = proposes(&l, &alice, amount("k2"))
        .await
        .map_err(Failure::new)?;
    let recomputed = cat
        .recompute(&l.own, Recompute::Function("pin_lookup".into()))
        .await;
    recomputed.must("recomputing `pin_lookup`")?;
    let name = l.read("bill", "k2").await?.fields.get("name").cloned();
    ensure!(
        name == Some(json!("k2")),
        "`k2`'s name must be filled; it reads {name:?}"
    );
    row("after a backed output", accept(p).await, &moved("k2"));
    // A record made again at the key.
    let p = proposes(&l, &alice, amount("k3"))
        .await
        .map_err(Failure::new)?;
    l.retire("bill", "k3").await?;
    l.upsert(
        "bill",
        "k3",
        json!({"owner": "alice", "code": "k3", "amount": 1}),
    )
    .await?;
    row("after `k3` is made again", accept(p).await, &moved("k3"));
    // The type declared again, its records untouched; migrated; re-keyed.
    let p = proposes(&l, &alice, amount("k1"))
        .await
        .map_err(Failure::new)?;
    bill.description = Some("a bill".into());
    l.declare(bill.clone()).await?;
    row(
        "after `bill` is declared again",
        accept(p).await,
        &moved("k1"),
    );
    let p = proposes(&l, &alice, amount("k1"))
        .await
        .map_err(Failure::new)?;
    bill.fields.push(field("extra", Text));
    l.migrate(bill.clone()).await?;
    row("after `bill` is migrated", accept(p).await, &moved("k1"));
    let p = proposes(&l, &alice, amount("k4"))
        .await
        .map_err(Failure::new)?;
    bill.key = vec!["code".into(), "owner".into()];
    l.migrate(bill).await?;
    row("after `bill` is re-keyed", accept(p).await, &moved("k4"));
    // A link is its holder's by its end records: her pinned proposal on
    // the link `c1:t1` is refused once `c1` moved, though the link did not.
    let on_trip = crate::links::link_type_name(&"claim".into(), &"on_trip".into());
    let set = json!([{"field": "label", "value": "'x'"}]);
    l.declare_json(
        json!({"declare": "verb", "name": "relabel", "on": on_trip.as_str(),
                          "risk": "high", "effect": "write", "set": set}),
    )
    .await?;
    let link = crate::links::link_key(&"c1".into(), &"t1".into());
    let (none, opts) = (Default::default(), Default::default());
    let relabel = VerbName::from("relabel");
    let proposed = cat.call(&alice, &relabel, Some(&link), none, opts);
    let Ok(Outcome::Proposed { proposal, .. }) = proposed.await else {
        return Err(Failure::new("her `relabel` of `c1:t1` must be proposed"));
    };
    l.upsert("claim", "c1", json!({"owner": "bob"})).await?;
    let (ty, key) = (on_trip, Some(link));
    let ended = said::<()>(Err(crate::invalid(Case::WriteChangedSinceProposed {
        ty,
        key,
    })));
    row(
        "after `c1`, her link's end, is handed to bob",
        accept(proposal).await,
        &ended,
    );
    all_right(
        &rows,
        "a holder of `only` proposes its own records' writes, pinned to them",
    )
}

/// What refused a proposal, or that it was made.
trait Refused {
    fn unwrap_err_or(self) -> String;
}

impl Refused for Result<ProposalId, String> {
    fn unwrap_err_or(self) -> String {
        self.err().unwrap_or_else(|| "proposed".into())
    }
}

async fn a_holder_of_only_decides_what_it_is_shown_whole(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let cat = l.cat.as_ref();
    claims(&l, false).await?;
    let set = json!([{"field": "owner", "value": "'bob'"}]);
    l.declare_json(
        json!({"declare": "verb", "name": "give", "on": "claim", "risk": "high",
                          "effect": "write", "set": set}),
    )
    .await?;
    // `ticket`'s owner is derived from `n`: what a write leaves is known
    // only once it is computed.
    l.declare_json(json!({"declare": "type", "name": "ticket", "fields": [
        {"name": "n", "type": {"kind": "text"}},
        {"name": "owner", "type": {"kind": "text"}, "authority": "derived", "derived": "n"}]}))
        .await?;
    l.upsert("ticket", "t1", json!({"n": "alice"})).await?;
    let (carol, bob) = (
        holder(store.as_ref(), "carol", json!({"default": "write"})),
        holder(store.as_ref(), "bob", json!({"default": "write"})),
    );
    let mut ids = Vec::new();
    for (who, write) in [
        (&carol, upsert("claim", "c1", "note", json!("q1"))),
        (&carol, upsert("claim", "c1", "note", json!("q2"))),
        (
            &carol,
            json!({"write": "call", "verb": "give", "key": "c1", "payload": {}}),
        ),
        (&bob, upsert("claim", "c2", "note", json!("q4"))),
        (&bob, upsert("claim", "c2", "note", json!("q5"))),
        (&carol, upsert("ticket", "t1", "n", json!("bob"))),
    ] {
        ids.push(proposes(&l, who, write).await.map_err(Failure::new)?);
    }
    cat.accept(&l.own, &ids[4])
        .await
        .must("the owner's accept of bob's second proposal")?;
    let hers = json!({"only": {"owner": "alice"}});
    let types = json!({"claim": hers, "ticket": hers});
    let more = json!({"default": "write", "review": true, "types": types});
    let alice = alice(store.as_ref(), more);
    let ng = not_granted();
    let mut rows: Vec<(String, String, String)> = Vec::new();
    let mut row = |what: &str, said: String, want: &str| {
        rows.push((what.to_owned(), said, want.to_owned()));
    };
    let head = store.head().await.must("reading the head")?;
    for (what, id) in [("bob's pending", &ids[3]), ("bob's decided", &ids[4])] {
        let accepted = cat.accept(&alice, id).await.map(|_| ());
        row(
            &format!("the accept of {what} proposal"),
            said(accepted),
            &ng,
        );
        let rejected = cat.reject(&alice, id, "no".into()).await.map(|_| ());
        row(
            &format!("the reject of {what} proposal"),
            said(rejected),
            &ng,
        );
    }
    let given = cat.accept(&alice, &ids[2]).await.map(|_| ());
    row(
        "the accept of a call that hands her `c1` to bob",
        said(given),
        &ng,
    );
    let derived = cat.accept(&alice, &ids[5]).await.map(|_| ());
    row(
        "the accept of a write that derives bob her `t1`",
        said(derived),
        &ng,
    );
    head_unmoved(store.as_ref(), head, "every refused decision by alice").await?;
    let accepted = cat.accept(&alice, &ids[0]).await.map(|_| ());
    row(
        "the accept of a change to her `c1`",
        said(accepted),
        "applied",
    );
    let rejected = cat.reject(&alice, &ids[1], "no".into()).await.map(|_| ());
    row(
        "the reject of a change to her `c1`",
        said(rejected),
        "applied",
    );
    let note = l.read("claim", "c1").await?.fields.get("note").cloned();
    row(
        "her `c1`'s note",
        format!("{note:?}"),
        "Some(String(\"q1\"))",
    );
    all_right(
        &rows,
        "a holder of `only` decides only what it is shown whole",
    )
}
