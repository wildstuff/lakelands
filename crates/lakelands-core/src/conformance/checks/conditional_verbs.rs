//! A verb that reads a conditional edge: `exists(<relation>)` and
//! `<relation>.<field>` over a derived relation onto its target's key, one
//! record at most, for callers the target holds nothing from
//! (`docs/decisions/055-a-path-steps-through-a-reference.md`, extended).

use super::conditional_edges::{DISCOUNTABLE, LATER, by_terms, invoice, lake, terms};
use super::only::holder;
use super::paths::{accept, call, proposal_of, unlisted, unmet};
use crate::conformance::prelude::*;

check!(
    SKONTO = a_verb_reads_a_conditional_edge_onto_its_targets_key,
    "055 and 044, extended (lakelands#960): a verb's `requires` and set values read a derived relation whose match names its target's key by `exists(<relation>)` and `<relation>.<field>`: `skonto` (`requires: exists(discountable)`, the discount copied from `discountable.rate`) runs on an open invoice inside its terms' period and is refused precondition, appending nothing, on a closed one, one past its period, one whose terms hold no period, one whose terms no record holds and one whose terms are retired, never `not_live`; a call proposed while the edge holds is refused precondition at an accept after the period is gone, appending nothing, and so after its terms are retired; one whose terms are held since the proposal is refused not_granted, appending nothing, where one whose terms were held when it was proposed is accepted, its proposer having been answered them; a proposal is not there to a reviewer the terms are held from, by the grant rule; a match of two pairs onto one key field holds only where both do; a derived relation whose match is not its target's key is refused `match_not_key` in a verb, by `exists` and by a path; a relation's name alone beside its `exists` is `unknown_field`; `exists` over a field, and in a read's `where`, is refused `exists_here`; a migration that takes the match off the target's key is refused naming the verb, and one that removes the relation as `unknown_field` as for a path"
);

check!(
    SKONTO_HELD = a_verb_that_reads_a_conditional_edge_is_its_callers_only_where_the_target_holds_nothing_from_it,
    "055, extended (lakelands#960): a verb that reads a derived relation is its caller's only where nothing of the target and of the verb's own type is held from the caller: one without `restricted`, one holding `only` on the target type or on the verb's type, one not shown the target type and one kept from a field the match or the `where` reads, on either side, is refused not_granted before any record is read, appending nothing, and neither `describe` nor `describe_type` names the verb to it; one the target holds nothing from calls as the edge holds, a held target record included"
);

/// What `skonto` requires, as a refusal names it.
const EDGE: &str = "exists(discountable)";

/// The lake of the edge rows, with `skonto`, its high-risk twin and the
/// field it sets, and the relation `by_rate`, matched off the key.
async fn skonto(store: &Store) -> Result<Lake, Failure> {
    lake(store.as_ref()).await?;
    let l = Lake::of(store.as_ref());
    for (name, risk) in [("skonto", "low"), ("skonto_hi", "high")] {
        l.declare_json(
            json!({"declare": "verb", "name": name, "on": "invoice", "risk": risk,
            "requires": EDGE, "effect": "write",
            "set": [{"field": "discount", "value": "discountable.rate"}]}),
        )
        .await?;
    }
    Ok(l)
}

async fn a_verb_reads_a_conditional_edge_onto_its_targets_key(store: Store) -> Held {
    let (s, l) = (store.as_ref(), skonto(&store).await?);
    let before = s.head().await.must("reading the head")?;
    let mut got = Vec::new();
    for key in ["i2", "i3", "i4", "i5"] {
        got.push(call(&l, &l.own, "skonto", key).await);
    }
    let want: Vec<_> = ["i2", "i3", "i4", "i5"]
        .map(|k| unmet("skonto", k, EDGE))
        .into();
    ensure!(
        got == want,
        "`skonto` is refused precondition where the edge does not hold: {got:?}, expected {want:?}"
    );
    head_unmoved(s, before, "an unmet `skonto`").await?;
    let got = call(&l, &l.own, "skonto", "i1").await;
    let discount = l
        .read("invoice", "i1")
        .await?
        .fields
        .get("discount")
        .cloned();
    ensure!(
        got == Ok("applied") && discount == Some(json!(2)),
        "`skonto` runs on `i1` and copies the terms' rate: {got:?}, discount {discount:?}"
    );
    // Retired terms are no match, never `not_live`.
    l.upsert(
        "terms",
        "t3",
        json!({"no": "t3", "discount_days": 14, "rate": 4}),
    )
    .await?;
    let fields = json!({"terms_no": "t3", "status": "open", "invoiced_on": "2099-01-01T00:00:00Z"});
    l.upsert("invoice", "i7", fields).await?;
    l.retire("terms", "t3").await?;
    let head = s.head().await.must("reading the head")?;
    let got = call(&l, &l.own, "skonto", "i7").await;
    head_unmoved(s, head, "`skonto` on retired terms").await?;
    ensure!(
        got == unmet("skonto", "i7", EDGE),
        "`skonto` on an invoice whose terms are retired is refused precondition; {got:?}"
    );
    // A call proposed while the edge holds is judged as it holds at the accept.
    l.upsert(
        "invoice",
        "i8",
        json!({"terms_no": "t1", "status": "open",
        "invoiced_on": "2099-01-01T00:00:00Z"}),
    )
    .await?;
    let p = proposal_of(&l, &l.own, "skonto_hi", "i8").await?;
    l.upsert(
        "invoice",
        "i8",
        json!({"invoiced_on": "2000-01-01T00:00:00Z"}),
    )
    .await?;
    let head = s.head().await.must("reading the head")?;
    let got = accept(&l, &l.own, &p).await;
    head_unmoved(s, head, "the accept of a `skonto` gone stale").await?;
    ensure!(
        got == unmet("skonto_hi", "i8", EDGE),
        "a proposed `skonto` whose period has passed since is refused at the accept; {got:?}"
    );
    // Its terms retired before the accept: no match. Held since the
    // proposal: the matched terms are in what it reaches, and its
    // proposer, answered them when it proposed, holds them no more. Not
    // there at all to a reviewer the terms are held from.
    let open = |terms: &str| json!({"terms_no": terms, "status": "open", "invoiced_on": LATER});
    for (no, rate) in [("t4", 3), ("t5", 3), ("t6", 3)] {
        let fields = json!({"no": no, "discount_days": 14, "rate": rate});
        l.upsert("terms", no, fields).await?;
    }
    for (key, terms) in [("i12", "t4"), ("i13", "t5"), ("i14", "t1"), ("i15", "t6")] {
        l.upsert("invoice", key, open(terms)).await?;
    }
    // Held before the proposal, its proposer was answered the terms then.
    restrict(s, "terms", "t6", "a hold").await?;
    let (retired, held, before) = (
        proposal_of(&l, &l.own, "skonto_hi", "i12").await?,
        proposal_of(&l, &l.own, "skonto_hi", "i13").await?,
        proposal_of(&l, &l.own, "skonto_hi", "i15").await?,
    );
    l.retire("terms", "t4").await?;
    restrict(s, "terms", "t5", "a hold").await?;
    let reviewer = json!({"default": "read", "review": true, "restricted": true});
    let reviewer = holder(s, "reviewer", reviewer);
    let head = s.head().await.must("reading the head")?;
    let got = [
        accept(&l, &reviewer, &retired).await,
        accept(&l, &reviewer, &held).await,
    ];
    head_unmoved(s, head, "the accept of a `skonto` whose terms went").await?;
    let got = [
        got[0].clone(),
        got[1].clone(),
        accept(&l, &reviewer, &before).await,
    ];
    let nope = Err(Error::Refused {
        field: None,
        reason: Refusal::NotGranted,
    });
    ensure!(
        got == [unmet("skonto_hi", "i12", EDGE), nope, Ok("applied")],
        "a proposed `skonto` whose terms are retired before the accept is refused \
         precondition, one whose terms are held since not_granted, and one whose terms were \
         held when it was proposed is accepted; {got:?}"
    );
    let p = proposal_of(&l, &l.own, "skonto_hi", "i14").await?;
    let only = json!({"default": "read", "review": true, "restricted": true,
                      "types": {"terms": {"only": {"no": "t2"}}}});
    unlisted(s, &l, &holder(s, "onlyterms", only), &p).await?;
    // A derived relation off its target's key is refused in a verb.
    let mut t = invoice(DISCOUNTABLE);
    let mut by_rate = by_terms("by_rate", "rated", None);
    if let Some(d) = by_rate.derived.as_mut() {
        d.pairs = vec![MatchPair {
            from: "discount".into(),
            to: "rate".into(),
        }];
    }
    t.relations.push(by_rate);
    // Two pairs onto one key field: each holds, or nothing matches.
    let mut twin = by_terms("twin", "twins", None);
    if let Some(d) = twin.derived.as_mut() {
        d.pairs.push(MatchPair {
            from: "alt_no".into(),
            to: "no".into(),
        });
    }
    t.relations.push(twin);
    l.declare(t.clone()).await?;
    l.declare_json(json!({"declare": "verb", "name": "two", "on": "invoice", "risk": "low",
        "requires": "exists(twin)", "effect": "write", "set": [{"field": "status", "value": "'two'"}]}))
        .await?;
    l.upsert("invoice", "i9", json!({"terms_no": "t1", "alt_no": "t2"}))
        .await?;
    l.upsert("invoice", "i10", json!({"terms_no": "t1", "alt_no": "t1"}))
        .await?;
    let got = [
        call(&l, &l.own, "two", "i9").await,
        call(&l, &l.own, "two", "i10").await,
    ];
    ensure!(
        got == [unmet("two", "i9", "exists(twin)"), Ok("applied")],
        "a match of two pairs onto one key field holds only where both do; {got:?}"
    );
    let not_key = json!({"code": "match_not_key", "relation": "by_rate", "target": "terms"});
    let exists_here = json!({"code": "exists_here"});
    let verb = |requires: &str| {
        json!({"declare": "verb", "name": "odd", "on": "invoice", "risk": "low",
               "requires": requires, "effect": "write", "set": [{"field": "status", "value": "'x'"}]})
    };
    for (what, d, want) in [
        ("`exists(by_rate)`", verb("exists(by_rate)"), &not_key),
        ("`by_rate.no`", verb("by_rate.no = 't1'"), &not_key),
        ("`exists(status)`", verb("exists(status)"), &exists_here),
        (
            "a relation's name alone beside its `exists`",
            verb("exists(discountable) and discountable is null"),
            &json!({"code": "unknown_field", "type": "invoice", "field": "discountable"}),
        ),
    ] {
        let head = s.head().await.must("reading the head")?;
        let d = serde_json::from_value(d).expect("a declaration");
        refused_like(what, l.cat.declare(&l.own, d).await, want)?;
        head_unmoved(s, head, what).await?;
    }
    let find = Find {
        filter: Some("exists(discountable)".into()),
        ..Find::default()
    };
    let found = l.cat.find(&l.own, &"invoice".into(), find).await;
    refused_like("`exists` in a read's `where`", found, &exists_here)?;
    // A migration that removes the relation is refused as one that takes
    // a field a path reads is.
    let mut gone = t.clone();
    gone.relations.retain(|r| r.name.as_str() != "discountable");
    let moved = l.cat.migrate(&l.own, migration(gone)).await;
    refused_like(
        "`invoice` without `discountable`",
        moved,
        &json!({"code": "unknown_field", "type": "invoice", "field": "discountable"}),
    )?;
    // Taking the match off the target's key is refused naming the verb.
    let mut rekeyed = terms();
    rekeyed.key = vec!["no".into(), "rate".into()];
    for f in rekeyed
        .fields
        .iter_mut()
        .filter(|f| ["no", "rate"].contains(&f.name.as_str()))
    {
        f.required = true;
    }
    let moved = l.cat.migrate(&l.own, migration(rekeyed)).await;
    refused_like(
        "`terms` keyed by more than the match",
        moved,
        &json!({"code": "match_not_key", "at": [{"code": "verb", "name": "skonto"}]}),
    )?;
    Ok(())
}

async fn a_verb_that_reads_a_conditional_edge_is_its_callers_only_where_the_target_holds_nothing_from_it(
    store: Store,
) -> Held {
    let (s, l) = (store.as_ref(), skonto(&store).await?);
    let grants = |more: Value| {
        let mut g = json!({"default": "write", "restricted": true});
        if let (Some(g), Value::Object(more)) = (g.as_object_mut(), more) {
            g.extend(more);
        }
        g
    };
    let only = |ty: &str, f: &str, v: &str| json!({"types": {ty: {"only": {f: v}}}});
    let mut masked = field_access(
        s,
        "masked",
        "terms",
        &[("discount_days", lakelands_api::Access::Masked)],
    );
    (masked.grants.default, masked.grants.restricted) = (lakelands_api::Access::Write, true);
    // And one masked from the match's own side.
    let masked_no = [("terms_no", lakelands_api::Access::Masked)];
    let mut maskedno = field_access(s, "maskedno", "invoice", &masked_no);
    (maskedno.grants.default, maskedno.grants.restricted) = (lakelands_api::Access::Write, true);
    let held = [
        holder(s, "plain", json!({"default": "write"})),
        holder(s, "onlyterms", grants(only("terms", "no", "t1"))),
        holder(s, "onlyinvoice", grants(only("invoice", "status", "open"))),
        holder(
            s,
            "noterms",
            grants(json!({"types": {"terms": {"default": "none"}}})),
        ),
        masked,
        maskedno,
    ];
    let before = s.head().await.must("reading the head")?;
    let mut red = Vec::new();
    for who in &held {
        for key in ["i1", "i2", "i9"] {
            let got = call(&l, who, "skonto", key).await;
            if !matches!(
                got,
                Err(Error::Refused {
                    reason: Refusal::NotGranted,
                    ..
                })
            ) {
                red.push(format!("`{}` calling `skonto` on `{key}`: {got:?}", who.id));
            }
        }
        let d = l
            .cat
            .describe(who, ReadAt::default())
            .await
            .must("describing")?
            .data;
        if d.verbs
            .iter()
            .any(|v| v.name.as_str().starts_with("skonto"))
        {
            red.push(format!("`{}` is described `skonto`", who.id));
        }
        let t = l
            .cat
            .describe_type(who, &"invoice".into(), ReadAt::default())
            .await;
        if let Ok(t) = t
            && t.data
                .verbs
                .iter()
                .any(|v| v.as_str().starts_with("skonto"))
        {
            red.push(format!("`{}` is described `invoice` with `skonto`", who.id));
        }
    }
    ensure!(
        red.is_empty(),
        "a verb that reads a conditional edge is not the caller's where its target holds \
         something from it: {}",
        red.join("; ")
    );
    head_unmoved(s, before, "a call of `skonto` not the caller's").await?;
    restrict(s, "terms", "t1", "a hold").await?;
    let free = holder(s, "free", grants(json!({})));
    let got = call(&l, &free, "skonto", "i1").await;
    ensure!(
        got == Ok("applied"),
        "one the target holds nothing from calls `skonto` as the edge holds, a held terms \
         included; {got:?}"
    );
    Ok(())
}
