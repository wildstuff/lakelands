//! A refused `requires` names the part of it the record fails, so its
//! caller knows which rule to meet; one who may not read what that part
//! reads is not told it, nor one not shown a setting it or a part
//! before it reads.

use lakelands_api::seam::SETTING_TYPE;

use super::only::holder;
use super::paths::{accept, call, lake, proposal_of, unlisted, unmet, verb};
use crate::conformance::prelude::*;

check!(
    REQUIRES_PART = a_refused_requires_names_the_part_the_record_fails,
    "006 and 059, extended (lakelands#1286): a call or accept refused `requires_unmet` names as `part` what of the verb's `requires` the record fails, in the text form: of an `and`, an `and` within it split in turn, the first conjunct in declaration order that does not hold, so a record that fails a business rule after a state it meets is told the rule, one that fails the state is told the state and one that fails both the state; a condition of any other shape, an `or` and a `not` of an `and` among them, is named whole, and so is the part an accept fails to a reviewer who does not read every field; a caller who may not read a field the part reads is refused not_granted, and a proposal of the call is not there to such a reviewer; a part that reads `setting('<key>')` is named to a caller shown the setting and left out to one who is not, the refusal otherwise the same: a declared setting is shown to one who may write it, any other key to a reader of `sys.setting` that no `only` holds there, and at an accept the caller judged is the reviewer, not the proposer; a part after one that reads a setting is left out alike, since naming it tells that the setting held; an `or` within an `and` is named whole"
);

async fn a_refused_requires_names_the_part_the_record_fails(store: Store) -> Held {
    const RULE: &str = "customer.discount_days >= days";
    const STATE: &str = "status = 'open'";
    const TAKE: &str = "status = 'open' and customer.discount_days >= days";
    const NOT: &str = "not (status = 'shut' and days >= 10)";
    const EITHER: &str = "status = 'open' or customer.discount_days >= days";
    let (s, l) = (store.as_ref(), lake(&store).await?);
    for (key, customer) in [("i4", "c1"), ("i5", "c2")] {
        let fields = json!({"status": "shut", "days": 10, "customer": customer});
        l.upsert("invoice", key, fields).await?;
    }
    let nest = format!("days >= 10 and ({TAKE})");
    let within = format!("days >= 10 and ({EITHER})");
    for (name, risk, requires) in [
        ("nest", "low", nest.as_str()),
        ("within", "low", within.as_str()),
        ("either", "low", EITHER),
        ("deny", "low", NOT),
        ("vet", "high", TAKE),
    ] {
        l.declare_json(verb(name, risk, Some(requires), ("copy", "1")))
            .await?;
    }
    let p = proposal_of(&l, &l.own, "vet", "i1").await?;
    let mut got = Vec::new();
    for (verb, key) in [
        ("take", "i1"),
        ("take", "i2"),
        ("take", "i4"),
        ("take", "i5"),
        ("nest", "i5"),
        ("within", "i5"),
        ("either", "i5"),
        ("deny", "i5"),
        ("term", "i2"),
    ] {
        got.push(call(&l, &l.own, verb, key).await);
    }
    let want = [
        Ok("applied"),
        unmet("take", "i2", RULE),
        unmet("take", "i4", STATE),
        unmet("take", "i5", STATE),
        unmet("nest", "i5", STATE),
        unmet("within", "i5", EITHER),
        unmet("either", "i5", EITHER),
        unmet("deny", "i5", NOT),
        unmet("term", "i2", "customer.term.days >= days"),
    ];
    ensure!(
        got == want,
        "a refused `requires` names the part the record fails: {got:?}, expected {want:?}"
    );

    // A part that reads a setting is named to one shown the setting and
    // left out to one who is not: a declared one to one who may write it,
    // any other key to a reader of `sys.setting` that no `only` holds
    // there. At an accept the one told is the reviewer. A part after one
    // that reads a setting is left out to one not shown it, since naming
    // the part tells that the setting held.
    const CAP: &str = "days <= setting('cap')";
    const MODE: &str = "setting('mode') = 'yes'";
    const GATE: &str = "setting('gate') = 'yes'";
    let cap = json!({"declare": "setting", "key": "cap", "type": {"kind": "decimal"},
        "group": "ops"});
    l.declare_json(cap).await?;
    let (capped, gated) = (format!("{STATE} and {CAP}"), format!("{STATE} and {MODE}"));
    let gate = format!("{GATE} and {STATE}");
    for (name, risk, requires) in [
        ("capped", "low", &capped),
        ("capping", "high", &capped),
        ("gated", "low", &gated),
        ("gate", "low", &gate),
        ("gating", "high", &gate),
    ] {
        l.declare_json(verb(name, risk, Some(requires), ("copy", "1")))
            .await?;
    }
    let ops = json!({"default": "write", "review": true, "types": {
        SETTING_TYPE: {"default": "write", "only": {"group": "ops"}}}});
    let plain = json!({"default": "write", "review": true, "types": {
        SETTING_TYPE: {"default": "read"}}});
    let (ops, plain) = (holder(s, "ops", ops), holder(s, "plain", plain));
    l.set("cap", json!("20")).await?;
    l.set("mode", json!("no")).await?;
    l.set("gate", json!("yes")).await?;
    let q = proposal_of(&l, &ops, "capping", "i2").await?;
    l.set("cap", json!("5")).await?;
    let open = json!({"status": "open", "days": 10, "customer": "c1"});
    l.upsert("invoice", "i6", open).await?;
    let g = proposal_of(&l, &l.own, "gating", "i6").await?;
    let shut = json!({"status": "shut", "days": 10, "customer": "c1"});
    l.upsert("invoice", "i6", shut).await?;
    let got = [
        call(&l, &ops, "capped", "i2").await,
        call(&l, &plain, "capped", "i2").await,
        accept(&l, &l.own, &q).await,
        accept(&l, &plain, &q).await,
        call(&l, &l.own, "gated", "i2").await,
        call(&l, &ops, "gated", "i2").await,
        call(&l, &l.own, "gate", "i4").await,
        call(&l, &ops, "gate", "i4").await,
        accept(&l, &l.own, &g).await,
        accept(&l, &ops, &g).await,
    ];
    let want = [
        unmet("capped", "i2", CAP),
        untold("capped", "i2"),
        unmet("capping", "i2", CAP),
        untold("capping", "i2"),
        unmet("gated", "i2", MODE),
        untold("gated", "i2"),
        unmet("gate", "i4", STATE),
        untold("gate", "i4"),
        unmet("gating", "i6", STATE),
        untold("gating", "i6"),
    ];
    ensure!(
        got == want,
        "a part reading a setting, or after one that does, is named to one shown it and left \
         out to one who is not, the reviewer at an accept: {got:?}, expected {want:?}"
    );

    // One who may not read the rule's field is told no part, not even the
    // state it may read: the verb is not its own.
    let mut blind = field_access(s, "blind", "customer", &[("discount_days", Access::None)]);
    (blind.grants.default, blind.grants.review) = (Access::Write, true);
    let mut partial = field_access(s, "partial", "customer", &[("owner", Access::None)]);
    partial.grants.review = true;
    let told = accept(&l, &partial, &p).await;
    let nope = Err(Error::Refused {
        field: None,
        reason: Refusal::NotGranted,
    });
    let (fails, state) = (
        call(&l, &blind, "take", "i2").await,
        call(&l, &blind, "take", "i4").await,
    );
    ensure!(
        told == unmet("vet", "i1", STATE) && fails == nope && state == nope,
        "an accept is told the part it fails; one who may not read the rule is not granted \
         the call where the record fails the rule or the state; {told:?}, {fails:?}, {state:?}"
    );
    unlisted(s, &l, &blind, &p).await
}

/// The refusal `unmet` names, without its part.
fn untold(verb: &str, key: &str) -> Result<&'static str, Error> {
    let (verb, ty, key) = (verb.into(), "invoice".into(), key.into());
    let part = None;
    let case = Case::RequiresUnmet {
        verb,
        ty,
        key,
        part,
    };
    Err(Error::Refused {
        field: None,
        reason: Refusal::Precondition(Box::new(case.into())),
    })
}
