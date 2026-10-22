//! A holder of `only` proposes writes to its own records, each pinned to
//! the record and its type as they stood, and decides the proposals it is
//! shown whole (`docs/decisions/051-a-grant-may-admit-only-some-records.md`).

use lakelands_api::VerbName;
use lakelands_api::ontology::StorageKind::{Number, Text};
use lakelands_api::write::Outcome;

use super::only::{alice, claims, de, holder, said};
use crate::conformance::backing::{KIND, plug, unplug};
use crate::conformance::prelude::*;

check!(
    ONLY_PROPOSES = a_holder_of_only_proposes_its_own_records,
    "051 and 013 (lakelands#935): a holder of `only` proposes no change of the ontology, refused before its declaration is read, and no write to a type it is not shown, not found before the record checks; it proposes an upsert, a retirement and a high-risk call on its own records, each logged `pinned`, another's proposals not; on another's record a proposal that would pass is not granted and one that reads the record is not found (a proposed retirement the rights grid compares with a key no record holds, lakelands#962); a proposal is refused where a field its `only` names is derived or backed, before the record checks, or where the verb assigns one, called or proposed, in a call's one order: after its scope, before an external verb's executor is asked and before its payload is read (lakelands#851); neither `describe` nor `describe_type` lists it such a verb; the accept of a pinned proposal applies while its record and type stand as proposed, and is refused once a rederive, a backed output, a record made again at the key, a redeclaration of the type that leaves its records as they are, a migration or a re-key moved them, or, for a link, once either end record moved"
);

check!(
    ONLY_DECIDES = a_holder_of_only_decides_what_it_is_shown_whole,
    "051: a reviewer holding `only` accepts and rejects a proposal its listing shows it whole; one on another's record, a retirement too and by one who holds `restricted` as well, is not there to it, its accept and reject answered as an id never made, decided or not, to the byte (lakelands#1016), appending nothing, so a guessed id tells it nothing; one listed to it in part, its own record handed to another or a free key made another's, it neither accepts nor rejects, each refused as not granted, appending nothing; a key whose record another retired, and which is not held, is free to it: a proposal that makes it the holder's is listed whole and accepted; and one whose record the write would leave outside its pairs, as the accept plans it (its derived values computed, a call's assignments made, by a constant, from the payload or from the record, on a link too), is listed to it in part and neither accepted nor rejected, while a call that leaves the record its own is listed whole and accepted, and one that would make another's record its own is not there to it (lakelands#914)"
);

/// The call of `verb` on the record `key` with `payload`.
fn call(verb: &str, key: &str, payload: Value) -> Value {
    json!({"write": "call", "verb": verb, "key": key, "payload": payload})
}

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
    l.declare(type_decl("vault", vec![field("n", Text)]))
        .await?;
    // A verb on `ticket` that reaches outside the lake, its executor not bound.
    l.declare_json(
        json!({"declare": "verb", "name": "mail", "on": "ticket", "risk": "high",
        "effect": "external", "executor": "nobody"}),
    )
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
    let none = json!({"default": "none"});
    let types = json!({"claim": hers, "bill": hers, "ticket": hers, "vault": none});
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
    // A call is judged in one order, called or proposed: its scope, the
    // grant, then its payload. Bob is refused for the scope alone.
    let (give, c1) = (VerbName::from("give"), RecordKey::from("c1"));
    let keyless = cat.call(&bob, &give, None, Default::default(), Default::default());
    let keyless = said(keyless.await);
    for (what, key, payload, want) in [
        ("no key", None, json!({}), &keyless),
        ("an undeclared member", Some(&c1), json!({"x": 1}), &ng),
    ] {
        let sent = crate::conformance::fixture::patch(payload.clone());
        let called = cat.call(&alice, &give, key, sent, Default::default()).await;
        row(&format!("`give` called with {what}"), said(called), want);
        let w = json!({"write": "call", "verb": "give", "key": key, "payload": payload});
        let proposed = proposes(&l, &alice, w).await.unwrap_err_or();
        row(&format!("`give` proposed with {what}"), proposed, want);
    }
    // Nor is the executor asked: `mail`'s is not bound, which bob, who
    // needs no pin, is told, and she is not.
    let (mail, t1) = (VerbName::from("mail"), RecordKey::from("t1"));
    let nobody = cat.call(
        &bob,
        &mail,
        Some(&t1),
        Default::default(),
        Default::default(),
    );
    let unbound = said(nobody.await);
    row(
        "bob's call of `mail`",
        unbound.contains("nobody").to_string(),
        "true",
    );
    let called = cat.call(
        &alice,
        &mail,
        Some(&t1),
        Default::default(),
        Default::default(),
    );
    row(
        "`mail` called, its executor unbound",
        said(called.await),
        &ng,
    );
    let w = json!({"write": "call", "verb": "mail", "key": "t1", "payload": {}});
    let proposed = proposes(&l, &alice, w).await.unwrap_err_or();
    row("`mail` proposed, its executor unbound", proposed, &ng);
    let p = proposes(&l, &alice, upsert("ticket", "t1", "n", json!("x"))).await;
    row(
        "a proposal on a type whose owner is derived",
        p.unwrap_err_or(),
        &ng,
    );
    // 013's order: no shape before the declaration is read, the type
    // shown and the pin before the record checks.
    let bad = json!({"write": "declare", "declarations": [{"declare": "type", "name": "no such"}]});
    let unpinned = upsert("ticket", "t1", "zz", json!("x"));
    for (what, write, want) in [
        ("a declaration of no identifier, proposed", bad, &ng),
        ("an unpinnable proposal of no field", unpinned, &ng),
        (
            "a proposal of no field of a type not shown",
            upsert("vault", "v1", "zz", json!(1)),
            &gone.to_owned(),
        ),
    ] {
        row(
            what,
            proposes(&l, &alice, write).await.unwrap_err_or(),
            want,
        );
    }
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
        said::<()>(Err(lakelands_api::Error::invalid(
            Case::WriteChangedSinceProposed { ty, key },
        )))
    };
    let own = &l.own;
    let accept = |p: ProposalId| async move { said(cat.accept(own, &p).await) };
    let amount = |key: &str| upsert("bill", key, "amount", json!(5));
    // A rederive: `double` is declared again.
    let p = proposes(&l, &alice, amount("k1"));
    let p = p.await.map_err(Failure::new)?;
    l.declare_json(json!({"declare": "function", "name": "double", "backing": "expr",
                          "inputs": [{"name": "amount", "type": {"kind": "number"}}], "expr": "amount * 3"}))
        .await?;
    row("after a rederive", accept(p).await, &moved("k1"));
    // A backed output: `k2`'s name, cleared by a failed call, is filled.
    let p = proposes(&l, &alice, amount("k2"));
    let p = p.await.map_err(Failure::new)?;
    let recomputed = cat.recompute(&l.own, Recompute::Function("pin_lookup".into()));
    let recomputed = recomputed.await;
    recomputed.must("recomputing `pin_lookup`")?;
    let name = l.read("bill", "k2").await?.fields.get("name").cloned();
    ensure!(
        name == Some(json!("k2")),
        "`k2`'s name must be filled; it reads {name:?}"
    );
    row("after a backed output", accept(p).await, &moved("k2"));
    // A record made again at the key.
    let p = proposes(&l, &alice, amount("k3"));
    let p = p.await.map_err(Failure::new)?;
    l.retire("bill", "k3").await?;
    l.upsert(
        "bill",
        "k3",
        json!({"owner": "alice", "code": "k3", "amount": 1}),
    )
    .await?;
    row("after `k3` is made again", accept(p).await, &moved("k3"));
    // The type declared again, its records untouched; migrated; re-keyed.
    let p = proposes(&l, &alice, amount("k1"));
    let p = p.await.map_err(Failure::new)?;
    bill.description = Some("a bill".into());
    l.declare(bill.clone()).await?;
    row(
        "after `bill` is declared again",
        accept(p).await,
        &moved("k1"),
    );
    let p = proposes(&l, &alice, amount("k1"));
    let p = p.await.map_err(Failure::new)?;
    bill.fields.push(field("extra", Text));
    l.migrate(bill.clone()).await?;
    row("after `bill` is migrated", accept(p).await, &moved("k1"));
    let p = proposes(&l, &alice, amount("k4"));
    let p = p.await.map_err(Failure::new)?;
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
    let ended = said::<()>(Err(lakelands_api::Error::invalid(
        Case::WriteChangedSinceProposed { ty, key },
    )));
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
    // Verbs that set the owner by a constant, from the payload, from the
    // record, and a link's label (lakelands#914).
    let to = json!([{"name": "to", "type": {"kind": "text"}}]);
    for (name, on, field, value, payload) in [
        ("give", "claim", "owner", "'bob'", json!([])),
        ("hand", "claim", "owner", "payload.to", to),
        (
            "swap",
            "claim",
            "owner",
            "if(amount > 5, 'bob', owner)",
            json!([]),
        ),
        ("take", "claim", "owner", "'alice'", json!([])),
        ("mark", "sys.link.claim.on_trip", "label", "'x'", json!([])),
    ] {
        let set = json!([{"field": field, "value": value}]);
        l.declare_json(
            json!({"declare": "verb", "name": name, "on": on, "risk": "high",
                              "effect": "write", "set": set, "payload": payload}),
        )
        .await?;
    }
    // `ticket`'s owner is derived from `n`: what a write leaves is known
    // only once it is computed.
    l.declare_json(json!({"declare": "type", "name": "ticket", "fields": [
        {"name": "n", "type": {"kind": "text"}},
        {"name": "owner", "type": {"kind": "text"}, "authority": "derived", "derived": "n"}]}))
        .await?;
    l.upsert("ticket", "t1", json!({"n": "alice"})).await?;
    l.upsert("claim", "c4", json!({"owner": "bob"})).await?;
    l.retire("claim", "c4").await?;
    let (carol, bob) = (
        holder(store.as_ref(), "carol", json!({"default": "write"})),
        holder(store.as_ref(), "bob", json!({"default": "write"})),
    );
    let mut ids = Vec::new();
    for (who, write) in [
        (&carol, upsert("claim", "c1", "note", json!("q1"))),
        (&carol, upsert("claim", "c1", "note", json!("q2"))),
        (&carol, call("give", "c1", json!({}))),
        (&bob, upsert("claim", "c2", "note", json!("q4"))),
        (&bob, upsert("claim", "c2", "note", json!("q5"))),
        (&carol, upsert("ticket", "t1", "n", json!("bob"))),
        (
            &bob,
            json!({"write": "retire", "type": "claim", "key": "c2"}),
        ),
        // Listed to her in part: her `c1` handed to bob, a free key made
        // his; and whole: bob's retired `c4`, a free key, made hers.
        (&carol, upsert("claim", "c1", "owner", json!("bob"))),
        (&carol, upsert("claim", "c9", "owner", json!("bob"))),
        (&carol, upsert("claim", "c4", "owner", json!("alice"))),
        (&carol, call("hand", "c1", json!({"to": "bob"}))),
        (&carol, call("swap", "c1", json!({}))),
        (&carol, call("swap", "c3", json!({}))),
        (&carol, call("take", "c2", json!({}))),
        (&carol, call("mark", "c1:t1", json!({}))),
    ] {
        ids.push(proposes(&l, who, write).await.map_err(Failure::new)?);
    }
    cat.accept(&l.own, &ids[4])
        .await
        .must("the owner's accept of bob's second proposal")?;
    let hers = json!({"only": {"owner": "alice"}});
    let link = json!({"only": {"label": "trip"}});
    let types = json!({"claim": hers, "ticket": hers, "sys.link.claim.on_trip": link});
    // With `restricted` she is judged by her `only` all the same: a
    // retirement names a record alone, and is judged by a read of it.
    let more = json!({"default": "write", "review": true, "restricted": true, "types": types});
    let alice = alice(store.as_ref(), more);
    let ng = not_granted();
    let mut rows: Vec<(String, String, String)> = Vec::new();
    let mut row = |what: &str, said: String, want: &str| {
        rows.push((what.to_owned(), said, want.to_owned()));
    };
    let head = store.head().await.must("reading the head")?;
    // A proposal on another's record is not there to her, decided or
    // not: its accept and reject answer as an id never made, to the byte
    // (lakelands#1016).
    let none = ProposalId::from("none");
    for (what, id) in [
        ("bob's pending proposal", &ids[3]),
        ("bob's decided proposal", &ids[4]),
        ("a retirement of bob's `c2`", &ids[6]),
        ("a call that would make bob's `c2` hers", &ids[13]),
    ] {
        let decide = async |id: &ProposalId| {
            let accepted = cat.accept(&alice, id).await;
            [accepted, cat.reject(&alice, id, "no".into()).await]
        };
        let (decided, never) = (decide(id).await, decide(&none).await);
        let as_never = decided.iter().zip(&never).all(|(d, n)| {
            matches!((d, n), (Err(d), Err(n)) if alike_renamed("", d, n, ("none", id.as_str())).is_ok())
        });
        row(
            &format!("the accept and reject of {what}"),
            format!("as an id never made: {as_never}; {decided:?}"),
            &format!("as an id never made: true; {decided:?}"),
        );
    }
    // An entry listed to her in part she neither accepts nor rejects: a
    // record the write leaves outside her pairs, as the accept plans it
    // (lakelands#914).
    let listed = cat.proposals(&alice, Default::default()).await;
    let listed = listed.must("alice's listing")?.data;
    let shown = |id: &ProposalId| {
        let p = listed.iter().find(|p| &p.id == id);
        format!("{:?}", p.map(|p| (p.write.is_some(), p.more)))
    };
    for (what, id) in [
        ("her `c1` handed to bob", &ids[7]),
        ("a free `c9` made bob's", &ids[8]),
        ("a call that hands her `c1` to bob", &ids[2]),
        ("a write that derives bob her `t1`", &ids[5]),
        ("a call that hands her `c1` to the payload's bob", &ids[10]),
        ("a call that hands her `c1` to bob by its amount", &ids[11]),
        ("a call that marks her link out", &ids[14]),
    ] {
        row(&format!("{what}, listed"), shown(id), "Some((false, true))");
        let accepted = cat.accept(&alice, id).await.map(|_| ());
        row(&format!("the accept of {what}"), said(accepted), &ng);
        let rejected = cat.reject(&alice, id, "no".into()).await.map(|_| ());
        row(&format!("the reject of {what}"), said(rejected), &ng);
    }
    head_unmoved(store.as_ref(), head, "every refused decision by alice").await?;
    // Bob's retired `c4` is a free key to her, as to her writes: its
    // remaking as hers is listed whole, and she accepts it, as a call
    // that leaves her `c3` hers.
    for (what, id) in [
        ("bob's retired `c4` made hers", &ids[9]),
        ("a call that leaves her `c3` hers by its amount", &ids[12]),
    ] {
        row(&format!("{what}, listed"), shown(id), "Some((true, false))");
        let accepted = cat.accept(&alice, id).await.map(|_| ());
        row(&format!("the accept of {what}"), said(accepted), "applied");
    }
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
