//! A verb's conditions read the call's payload
//! (`docs/decisions/028-authoring-is-strict-and-whole.md` and
//! `docs/decisions/006-grants-not-roles.md`, each extended).
//!
//! A rule over what a call brings — a reading that may not go back, a
//! payment that may not overpay — is the verb's to keep, in the write it
//! guards, so no caller can race it. A condition may be named, so a
//! refused caller is told which rule it broke by a name, not by the text
//! of the rule.

use lakelands_api::error::Naming;
use lakelands_api::seam::SETTING_TYPE;

use super::only::holder;
use super::paths::accept;
use crate::conformance::prelude::*;

check!(
    PAYLOAD_REQUIRED = a_verbs_requires_reads_the_calls_payload_in_the_write_it_guards,
    "028 and 006, extended (lakelands#1330): in a verb's `requires`, `payload.<name>` reads the call's payload as its values do, judged in the write the call makes: `payload.odometer_km >= odometer_km` refuses a reading below the record's, and readings sent at once leave the highest; `payload.amount > 0` and `paid + payload.amount <= amount` refuse a payment that is not positive, a call with no `amount`, and one that overpays, each `requires_unmet` naming the part it fails, while one that settles the rest is applied; at an accept the payload judged is the proposal's, against the record as it is then; a `payload.<name>` the verb's payload does not declare is refused at declare `payload_no_member`, naming the verb's `requires`, and `payload.invoice.status` is a path of the record's own field `payload`, which a type without one refuses at declare, since a record the payload names is not reached; nothing refused is appended"
);

check!(
    PRECONDITION_NAMED = a_refused_precondition_is_told_by_its_name,
    "006, extended (lakelands#1330): a verb declares `preconditions`, each a `name` and the condition it `holds`, judged after its `requires` in order, as one `and`; a call that fails one is refused `requires_unmet` said within it, at the place `precondition` with its name, its failing conjunct as `part`; one that fails `requires` is said at no place, and a precondition is told to a caller only where its part is, so one not shown a setting it reads is told neither; a precondition named twice is refused `precondition_named_twice`, one whose name is no identifier is refused as an invalid name, one reading a payload member the verb does not declare `payload_no_member` naming the precondition, and a verb on its type with one `type_verb_requires`, each appending nothing"
);

/// A lake of vehicles read by an odometer and invoices paid by amount.
async fn lake(store: &Store) -> Result<Lake, Failure> {
    let (l, number) = (Lake::of(store.as_ref()), StorageKind::Number);
    l.declare(type_decl("vehicle", vec![field("odometer_km", number)]))
        .await?;
    let status = field("status", StorageKind::Text);
    let invoice = vec![field("amount", number), field("paid", number), status];
    l.declare(type_decl("invoice", invoice)).await?;
    Ok(l)
}

/// A write verb on `on` with one number in its payload, `member`, that
/// sets `set` and requires `requires`.
fn verb(name: &str, on: &str, member: &str, requires: &str, set: (&str, &str)) -> Value {
    json!({"declare": "verb", "name": name, "on": on, "risk": "low", "effect": "write",
        "payload": [{"name": member, "type": {"kind": "number"}}], "requires": requires,
        "set": [{"field": set.0, "value": set.1}]})
}

/// `verb` called by `who` on the `ty` `key` with `payload`.
async fn call(
    l: &Lake,
    who: &Principal,
    (verb, key): (&str, &str),
    payload: Value,
) -> Result<&'static str, Error> {
    let (verb, key, opts) = (verb.into(), key.into(), WriteOpts::default());
    match l
        .cat
        .call(who, &verb, Some(&key), patch(payload), opts)
        .await?
    {
        Outcome::Applied { .. } => Ok("applied"),
        Outcome::Proposed { .. } => Ok("proposed"),
    }
}

/// The refusal of `verb` on `ty` `key`, told `part`, within `precondition`.
fn unmet(
    (verb, ty, key): (&str, &str, &str),
    precondition: Option<&str>,
    part: Option<&str>,
) -> Result<&'static str, Error> {
    let said = Said::from(Case::RequiresUnmet {
        verb: verb.into(),
        ty: ty.into(),
        key: key.into(),
        part: part.map(|p| Box::new(Printed(p.into()))),
    });
    let said = match precondition {
        Some(name) => said.within(Place::Precondition { name: name.into() }),
        None => said,
    };
    Err(Error::Refused {
        field: None,
        reason: Refusal::Precondition(Box::new(said)),
    })
}

async fn a_verbs_requires_reads_the_calls_payload_in_the_write_it_guards(store: Store) -> Held {
    const BACK: &str = "payload.odometer_km >= odometer_km";
    const POSITIVE: &str = "payload.amount > 0";
    const OVER: &str = "paid + payload.amount <= amount";
    let l = lake(&store).await?;
    let read = verb(
        "read",
        "vehicle",
        "odometer_km",
        BACK,
        ("odometer_km", "payload.odometer_km"),
    );
    l.declare_json(read).await?;
    let rule = format!("{POSITIVE} and {OVER}");
    l.declare_json(verb(
        "pay",
        "invoice",
        "amount",
        &rule,
        ("paid", "paid + payload.amount"),
    ))
    .await?;
    let mut vet = verb(
        "vet",
        "invoice",
        "amount",
        &rule,
        ("paid", "paid + payload.amount"),
    );
    vet["risk"] = json!("high");
    l.declare_json(vet).await?;
    l.upsert("vehicle", "v1", json!({"odometer_km": 50}))
        .await?;
    l.upsert("vehicle", "v2", json!({"odometer_km": 50}))
        .await?;
    l.upsert(
        "invoice",
        "i1",
        json!({"amount": 100, "paid": 0, "status": "open"}),
    )
    .await?;
    l.upsert(
        "invoice",
        "i2",
        json!({"amount": 100, "paid": 0, "status": "open"}),
    )
    .await?;
    let own = &l.own;
    let km = |n: i64| json!({ "odometer_km": n });
    let pays = |n: i64| json!({ "amount": n });
    let mut got = Vec::new();
    for (at, payload) in [
        (("read", "v1"), km(60)),
        (("read", "v1"), km(55)),
        (("read", "v1"), km(60)),
        (("pay", "i1"), pays(60)),
        (("pay", "i1"), pays(50)),
        (("pay", "i1"), pays(0)),
        (("pay", "i1"), json!({})),
        (("pay", "i1"), pays(40)),
    ] {
        let before = store.head().await.must("reading the head")?;
        let answer = call(&l, own, at, payload).await;
        let after = store.head().await.must("reading the head")?;
        // A refused call appends nothing.
        ensure!(
            answer.is_ok() || after == before,
            "a refused call of `{}` appended: {answer:?}",
            at.0
        );
        got.push(answer);
    }
    let want = [
        Ok("applied"),
        unmet(("read", "vehicle", "v1"), None, Some(BACK)),
        Ok("applied"),
        Ok("applied"),
        unmet(("pay", "invoice", "i1"), None, Some(OVER)),
        unmet(("pay", "invoice", "i1"), None, Some(POSITIVE)),
        unmet(("pay", "invoice", "i1"), None, Some(POSITIVE)),
        Ok("applied"),
    ];
    let (v1, i1) = (
        l.read("vehicle", "v1").await?,
        l.read("invoice", "i1").await?,
    );
    ensure!(
        got == want
            && value(&v1, "odometer_km") == Some(&json!(60))
            && value(&i1, "paid") == Some(&json!(100)),
        "a verb's `requires` reads the call's payload: {got:?}, expected {want:?}; the \
         odometer reads {:?}, the invoice is paid {:?}",
        value(&v1, "odometer_km"),
        value(&i1, "paid")
    );

    // Readings sent at once are each judged against the record as the
    // write before it left it, so none lowers it.
    let callers = (60..=100).rev().step_by(5).map(|n| {
        let (cat, own) = (l.cat.clone(), l.own.clone());
        let call: Caller<'_, Result<&'static str, Error>> = Box::pin(async move {
            let (verb, key) = ("read".into(), "v2".into());
            let payload = patch(json!({ "odometer_km": n }));
            let called = cat.call(&own, &verb, Some(&key), payload, WriteOpts::default());
            called.await.map(|_| "applied")
        });
        call
    });
    let answers = join_all(callers.collect()).await;
    let lower = unmet(("read", "vehicle", "v2"), None, Some(BACK));
    let odd: Vec<_> = (answers.iter())
        .filter(|a| **a != Ok("applied") && **a != lower)
        .collect();
    let v2 = l.read("vehicle", "v2").await?;
    ensure!(
        odd.is_empty() && value(&v2, "odometer_km") == Some(&json!(100)),
        "readings 60 to 100 sent at once are each applied or refused as lowering the \
         odometer, and leave the highest; the odometer reads {:?}, other answers {odd:?}",
        value(&v2, "odometer_km")
    );

    // At an accept the proposal's payload is judged against the record
    // as it is then.
    let (vet, i2, opts) = ("vet".into(), "i2".into(), WriteOpts::default());
    let p = match l
        .cat
        .call(own, &vet, Some(&i2), patch(pays(60)), opts)
        .await
    {
        Ok(Outcome::Proposed { proposal, .. }) => proposal,
        other => return Err(Failure::new(format!("`vet` is proposed; {other:?}"))),
    };
    l.upsert("invoice", "i2", json!({"paid": 50})).await?;
    let before = store.head().await.must("reading the head")?;
    let accepted = accept(&l, own, &p).await;
    let want = unmet(("vet", "invoice", "i2"), None, Some(OVER));
    let after = store.head().await.must("reading the head")?;
    ensure!(
        accepted == want && after == before,
        "an accept judges the proposal's payload against the record as it is then: \
         {accepted:?}, expected {want:?}"
    );

    // A member the payload does not declare is refused at declare, and a
    // longer path is the record's own field `payload`.
    let before = store.head().await.must("reading the head")?;
    let ghost = verb(
        "ghost",
        "invoice",
        "amount",
        "payload.ghost > 0",
        ("paid", "1"),
    );
    let ghost = l.cat.declare(own, de(ghost)).await;
    let reach = json!({"declare": "verb", "name": "reach", "on": "invoice", "risk": "low",
        "effect": "write", "set": [{"field": "paid", "value": "1"}],
        "payload": [{"name": "invoice", "type": {"kind": "reference", "target": "invoice"}}],
        "requires": "payload.invoice.status = 'open'"});
    let reach = l.cat.declare(own, de(reach)).await;
    let at = Place::Verb {
        name: "ghost".into(),
    };
    let member = Case::PayloadNoMember {
        member: "ghost".into(),
    };
    let (ty, field) = ("invoice".into(), "payload".into());
    let unknown = Case::UnknownField { ty, field };
    ensure!(
        invalid_said(&ghost).is_some_and(|s| s.case == member && s.at == [at, Place::Requires])
            && invalid_case(&reach) == Some(&unknown),
        "`payload.ghost` is refused `payload_no_member` at the verb's `requires`, \
         `payload.invoice.status` as the unknown field `payload`: {ghost:?}, {reach:?}"
    );
    head_unmoved(store.as_ref(), before, "the refused declarations").await
}

async fn a_refused_precondition_is_told_by_its_name(store: Store) -> Held {
    const STATE: &str = "status = 'open'";
    const POSITIVE: &str = "payload.amount > 0";
    const OVER: &str = "paid + payload.amount <= amount";
    const CAP: &str = "payload.amount <= setting('cap')";
    let (s, l) = (store.as_ref(), lake(&store).await?);
    let named = |name: &str, holds: &str| json!({"name": name, "holds": holds});
    let mut settle = verb(
        "settle",
        "invoice",
        "amount",
        STATE,
        ("paid", "paid + payload.amount"),
    );
    settle["preconditions"] = json!([
        named("positive", POSITIVE),
        named("not_overpaid", OVER),
        named("capped", CAP)
    ]);
    l.declare_json(settle.clone()).await?;
    l.upsert(
        "invoice",
        "i1",
        json!({"amount": 100, "paid": 0, "status": "open"}),
    )
    .await?;
    l.upsert(
        "invoice",
        "i2",
        json!({"amount": 100, "paid": 0, "status": "shut"}),
    )
    .await?;
    l.set("cap", json!(30)).await?;
    let blind = holder(
        s,
        "blind",
        json!({"default": "write", "types": {
        SETTING_TYPE: {"default": "none"}}}),
    );
    let own = &l.own;
    let pays = |n: i64| json!({ "amount": n });
    let at = |key| ("settle", "invoice", key);
    let got = [
        call(&l, own, ("settle", "i2"), pays(10)).await,
        call(&l, own, ("settle", "i1"), pays(0)).await,
        call(&l, own, ("settle", "i1"), pays(500)).await,
        call(&l, own, ("settle", "i1"), pays(40)).await,
        call(&l, &blind, ("settle", "i1"), pays(40)).await,
        call(&l, &blind, ("settle", "i1"), pays(0)).await,
        call(&l, own, ("settle", "i1"), pays(20)).await,
    ];
    let want = [
        unmet(at("i2"), None, Some(STATE)),
        unmet(at("i1"), Some("positive"), Some(POSITIVE)),
        unmet(at("i1"), Some("not_overpaid"), Some(OVER)),
        unmet(at("i1"), Some("capped"), Some(CAP)),
        unmet(at("i1"), None, None),
        unmet(at("i1"), Some("positive"), Some(POSITIVE)),
        Ok("applied"),
    ];
    ensure!(
        got == want,
        "a refused precondition is told by its name, where its part is told: {got:?}, \
         expected {want:?}"
    );

    // Each refused at declare, appending nothing.
    let before = s.head().await.must("reading the head")?;
    let mut twice = settle.clone();
    twice["name"] = json!("twice");
    twice["preconditions"] = json!([named("positive", POSITIVE), named("positive", OVER)]);
    let mut odd = settle.clone();
    odd["name"] = json!("odd");
    odd["preconditions"] = json!([named("Not A Name", POSITIVE)]);
    let mut ghost = settle.clone();
    ghost["name"] = json!("ghost");
    ghost["preconditions"] = json!([named("haunted", "payload.ghost > 0")]);
    let typed = json!({"declare": "verb", "name": "typed", "on": "invoice", "scope": "type",
        "risk": "low", "effect": "external", "executor": "x",
        "preconditions": [named("positive", "1 > 0")]});
    let mut red = Vec::new();
    for (what, decl, case) in [
        (
            "twice",
            twice,
            Case::PreconditionNamedTwice {
                precondition: "positive".into(),
            },
        ),
        (
            "odd",
            odd,
            Case::NameNotIdentifier {
                what: Naming::Precondition,
                name: "Not A Name".into(),
            },
        ),
        (
            "ghost",
            ghost,
            Case::PayloadNoMember {
                member: "ghost".into(),
            },
        ),
        (
            "typed",
            typed,
            Case::TypeVerbRequires {
                ty: "invoice".into(),
            },
        ),
    ] {
        let got = l.cat.declare(&l.own, de(decl)).await;
        let fits = invalid_case(&got) == Some(&case);
        let placed = what != "ghost"
            || invalid_said(&got).is_some_and(|s| {
                s.at == [
                    Place::Verb {
                        name: "ghost".into(),
                    },
                    Place::Precondition {
                        name: "haunted".into(),
                    },
                ]
            });
        if !(fits && placed) {
            red.push(format!("`{what}` answered {got:?}"));
        }
    }
    ensure!(
        red.is_empty(),
        "a verb's preconditions are judged at declare: {}",
        red.join("; ")
    );
    head_unmoved(s, before, "the refused declarations").await
}

/// `json` as a declaration.
fn de(json: Value) -> Declaration {
    serde_json::from_value(json).expect("a declaration")
}
