//! A verb may be kept to the callers granted it: one declared `granted`
//! is callable, proposable, accepted and described only where the
//! caller's grants on its type name it, the field rights on top.

use lakelands_api::error::Missing;
use lakelands_api::tools::Door;

use super::only::holder;
use super::paths::{accept, call, lake, proposal_of, unlisted, verb};
use crate::conformance::fixture::upsert;
use crate::conformance::prelude::*;

check!(
    VERB_GRANTED = a_verb_declared_granted_is_kept_to_the_callers_granted_it,
    "070 (lakelands#1287): a verb declared `granted` is called by one whose grants on its type name it and refused not_granted to one with every field right but not the grant, before any record is read (a key no record holds is answered not_granted, not not_found), the owner included; it is proposed neither through `propose` nor by its risk without the grant, a proposal of it is not there to a reviewer without the grant and is applied by one with it; `describe` names it neither on its type nor among the lake's verbs to one without the grant, and no MCP tool is made of it; the grant does not stand in for a field right; a refused `upsert` of a field only verbs write, and a refused `retire` of a type only verbs retire, name a granted verb that writes it only to one granted it, answering `wrong_door` to one without; a grant naming no verb or one not declared `granted` changes nothing, and an ungated verb on the type is callable as before; redeclared without the flag, the owner calls it, and a proposal of it kept from a reviewer is there to it again, judged as the lake now declares the verb, which was declared again since; a proposal of one withdrawn since is still not there to a reviewer without the grant"
);

/// The verbs `who` is shown on `invoice` and among the lake's, and
/// whether an MCP tool calls each of `names`.
async fn shown(l: &Lake, who: &Principal, names: &[&str]) -> Result<Value, Failure> {
    let d = l.cat.describe(who, ReadAt::default()).await;
    let d = d.must("describing the lake")?.data;
    let on = d.types.iter().filter(|t| t.name.as_str() == "invoice");
    let on = on.flat_map(|t| &t.verbs).map(|v| v.as_str());
    let made = tools(&d).map_err(|e| Failure::new(format!("the tools are made: {e:?}")))?;
    let tool = |n: &str| {
        let call = |t: &&lakelands_api::tools::ToolSpec| matches!(&t.door, Door::Call { verb } if verb.as_str() == n);
        made.iter().any(|t| call(&t))
    };
    let mut on: Vec<&str> = on.filter(|v| names.contains(v)).collect();
    on.sort_unstable();
    let all = names
        .iter()
        .filter(|n| d.verbs.iter().any(|v| v.name.as_str() == **n));
    let made = names.iter().filter(|n| tool(n));
    Ok(json!([
        on,
        all.collect::<Vec<_>>(),
        made.collect::<Vec<_>>()
    ]))
}

/// `who` proposing a call of `executed` on `i2` through `propose`.
async fn propose(l: &Lake, who: &Principal) -> Result<&'static str, Error> {
    let (verb, key, opts) = ("executed".into(), Some("i2".into()), WriteOpts::default());
    let payload = patch(json!({}));
    let write = ProposedWrite::Call {
        verb,
        key,
        payload,
        opts,
    };
    l.cat
        .propose(who, write, "why".into())
        .await
        .map(|_| "proposed")
}

async fn a_verb_declared_granted_is_kept_to_the_callers_granted_it(store: Store) -> Held {
    let (s, l) = (store.as_ref(), lake(&store).await?);
    let granted = |mut v: Value| {
        v["granted"] = json!(true);
        v
    };
    l.declare_json(granted(verb(
        "executed",
        "low",
        None,
        ("status", "'executed'"),
    )))
    .await?;
    l.declare_json(granted(verb(
        "vouch",
        "high",
        None,
        ("status", "'vouched'"),
    )))
    .await?;
    l.declare_json(granted(verb("seal", "high", None, ("status", "'sealed'"))))
        .await?;
    // `bill`: `status` only verbs write, records only verbs retire, by
    // `pay` and `void`, both granted.
    l.declare_json(
        json!({"declare": "type", "name": "bill", "retired_by": "verbs", "fields": [
        {"name": "no", "type": {"kind": "text"}, "authority": "shared"},
        {"name": "status", "type": {"kind": "text"}, "authority": "verb", "initial": "open"}]}),
    )
    .await?;
    for (name, effect) in [("pay", "write"), ("void", "retire")] {
        let mut v = json!({"declare": "verb", "name": name, "on": "bill", "risk": "low",
            "effect": effect, "granted": true});
        if effect == "write" {
            v["set"] = json!([{"field": "status", "value": "'paid'"}]);
        }
        l.declare_json(v).await?;
    }
    let names = ["bump", "executed", "vouch"];
    let verbs = json!({"invoice": {"verbs": ["executed", "vouch", "seal", "ghost", "bump"]},
        "bill": {"verbs": ["pay", "void"]}});
    let exec = holder(
        s,
        "exec",
        json!({"default": "write", "review": true, "types": verbs}),
    );
    let plain = holder(s, "plain", json!({"default": "write", "review": true}));
    let verbs = json!({"invoice": {"verbs": ["executed"], "fields": {"status": "read"}}});
    let narrow = holder(s, "narrow", json!({"default": "write", "types": verbs}));
    // A reviewer who may not read another type, for whom the lake at a
    // proposal is read too: the grant is judged as the lake declares the
    // verb now.
    let terms = json!({"term": {"default": "none"}});
    let aside = json!({"default": "write", "review": true, "types": terms});
    let aside = holder(s, "aside", aside);
    let nope = Err(Error::Refused {
        field: None,
        reason: Refusal::NotGranted,
    });
    let missing = Err(Error::NotFound {
        missing: Missing::Record {
            ty: "invoice".into(),
            key: "none".into(),
        },
    });
    let p = proposal_of(&l, &exec, "vouch", "i3").await?;
    let held = proposal_of(&l, &exec, "vouch", "i1").await?;
    let got = [
        call(&l, &plain, "executed", "i1").await,
        call(&l, &plain, "executed", "none").await,
        call(&l, &l.own, "executed", "i1").await,
        call(&l, &narrow, "executed", "i1").await,
        call(&l, &plain, "vouch", "i1").await,
        propose(&l, &plain).await,
        call(&l, &plain, "bump", "i1").await,
        call(&l, &exec, "bump", "i1").await,
        call(&l, &exec, "executed", "none").await,
        call(&l, &exec, "executed", "i1").await,
        propose(&l, &exec).await,
    ];
    let want = [
        nope.clone(),
        nope.clone(),
        nope.clone(),
        nope.clone(),
        nope.clone(),
        nope,
        Ok("applied"),
        Ok("applied"),
        missing,
        Ok("applied"),
        Ok("proposed"),
    ];
    ensure!(
        got == want,
        "a verb declared `granted` is called and proposed only by one granted it, before \
         any record is read, its field rights on top: {got:?}, expected {want:?}"
    );
    // A refusal's hint names the granted verb only to its grantees.
    upsert(l.cat.as_ref(), &l.own, "bill", "b1", json!({"no": "b1"})).await?;
    let (ty, key) = ("bill".into(), "b1".into());
    let mut told = Vec::new();
    for who in [&plain, &exec] {
        let up = try_upsert(l.cat.as_ref(), who, "bill", "b1", json!({"status": "paid"})).await;
        let out = l.cat.retire(who, &ty, &key, WriteOpts::default()).await;
        told.extend([up.err(), out.err()]);
    }
    let refused = |field: Option<&str>, reason| {
        let field = field.map(Into::into);
        Some(Error::Refused { field, reason })
    };
    let only = |v: &str| Refusal::VerbsOnly {
        verbs: vec![v.into()],
    };
    let want = [
        refused(Some("status"), Refusal::WrongDoor),
        refused(None, Refusal::WrongDoor),
        refused(Some("status"), only("pay")),
        refused(None, only("void")),
    ];
    ensure!(
        told == want,
        "a refused write of a field only verbs write, and a refused retire of a type only \
         verbs retire, name a granted verb only to one granted it: {told:?}, expected {want:?}"
    );
    let (kept, granted) = (
        shown(&l, &plain, &names).await?,
        shown(&l, &exec, &names).await?,
    );
    let (none, all) = (
        json!([["bump"], ["bump"], ["bump"]]),
        json!([names, names, names]),
    );
    ensure!(
        kept == none && granted == all,
        "`describe` and the MCP tools name a verb declared `granted` only to one granted it: \
         {kept} and {granted}, expected {none} and {all}"
    );
    unlisted(s, &l, &plain, &p).await?;
    unlisted(s, &l, &aside, &held).await?;
    // Withdrawn since, the verb is judged as the lake declared it at the
    // proposal.
    let sealed = proposal_of(&l, &exec, "seal", "i2").await?;
    let out = json!({"declare": "withdraw", "kind": "verb", "name": "seal"});
    l.declare_json(out).await?;
    unlisted(s, &l, &plain, &sealed).await?;
    let accepted = accept(&l, &exec, &p).await;
    ensure!(
        accepted == Ok("applied"),
        "a reviewer granted the verb accepts its proposal: {accepted:?}"
    );
    l.declare_json(verb("executed", "low", None, ("status", "'executed'")))
        .await?;
    let again = call(&l, &l.own, "executed", "i2").await;
    ensure!(
        again == Ok("applied"),
        "redeclared without `granted`, the owner calls the verb: {again:?}"
    );
    l.declare_json(verb("vouch", "high", None, ("status", "'vouched'")))
        .await?;
    let shown = l.cat.accept(&aside, &held).await;
    let verb = "vouch".into();
    ensure!(
        invalid_case(&shown) == Some(&Case::VerbRedeclared { verb }),
        "redeclared without `granted`, a proposal of the verb is there to a reviewer it was \
         kept from, judged as the lake declares the verb now, and told it was declared \
         again: {shown:?}"
    );
    Ok(())
}
