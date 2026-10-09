//! A verb reads only what its caller may read
//! (`docs/decisions/024-probing-a-field-needs-the-right-to-read-it.md`,
//! extended): a verb whose `requires` or set values read a field its
//! caller may not read would answer that field's value, by running or not,
//! or copy it whole, so it is not that caller's to call or to be shown.

use lakelands_api::error::Case;
use lakelands_api::error::Missing;
use lakelands_api::id::ProposalId;
use lakelands_api::ontology::StorageKind;
use lakelands_api::principal::TypeGrants;
use lakelands_api::write::{Proposal, ProposalFilter, ProposedWrite};
use lakelands_api::{Access, Error, Outcome, Principal, ReadAt, Refusal, TypeName, WriteOpts};
use serde_json::{Value, json};

use crate::conformance::fixture::{
    Lake, Must, derived, field, field_access, invalid_case, patch, reference, type_decl, type_of,
};
use crate::conformance::{Failure, Held, Store};

check!(
    VERB_READS_AS_A_FILTER = a_verb_that_reads_a_field_its_caller_may_not_read_is_not_its_to_call,
    "024, extended (lakelands#779): a verb whose `requires` or set values read a field its caller may not know of, sees masked or may only write, a field derived from one, or a reference to a type it is not shown, is refused not_granted to that caller, whole as a verb it may not write is, called directly, called at a risk that proposes it and proposed as a call, and to a reviewer accepting it, whatever the record holds; it appends nothing, `describe` names it neither on its type nor among the lake's verbs, and a call proposal of it is not there to that reviewer: the listing and a listing filtered to its type are those of the lake without it, and its accept and reject answer as an id no proposal holds does, also once the verb is declared again; a verb that reads no such field stays callable and named; a caller who reads the field calls it and is answered as the record holds"
);

/// `who` calling `verb` on `key` with no payload.
async fn call(l: &Lake, who: &Principal, verb: &str, key: &str) -> lakelands_api::Result<Outcome> {
    let opts = WriteOpts::default();
    l.cat
        .call(who, &verb.into(), Some(&key.into()), patch(json!({})), opts)
        .await
}

/// An answer as a row compares it: what it did, or its error whole.
fn said(a: &lakelands_api::Result<Outcome>) -> Result<&'static str, Error> {
    match a {
        Ok(Outcome::Applied { .. }) => Ok("applied"),
        Ok(Outcome::Proposed { .. }) => Ok("proposed"),
        Err(e) => Err(e.clone()),
    }
}

/// The verbs `who` is shown: on `memo`, and among the lake's.
async fn shown(l: &Lake, who: &Principal) -> Result<Value, Failure> {
    let d = l.cat.describe(who, ReadAt::default()).await;
    let d = d.must("describing the lake")?.data;
    let on = d.types.iter().filter(|t| t.name.as_str() == "memo");
    let on: Vec<_> = on
        .flat_map(|t| t.verbs.iter().map(|v| v.to_string()))
        .collect();
    let all: Vec<_> = d.verbs.iter().map(|v| v.name.to_string()).collect();
    Ok(json!([on, all]))
}

/// The answer for a proposal `id` that is not there.
fn absent<T>(id: &ProposalId) -> lakelands_api::Result<T> {
    let id = id.clone();
    Err(Error::NotFound {
        missing: Missing::Proposal { id },
    })
}

/// What `who` is listed: every proposal, and those on `memo`.
async fn listings(l: &Lake, who: &Principal) -> Result<[Vec<Proposal>; 2], Failure> {
    let mut lists = [Vec::new(), Vec::new()];
    for (list, ty) in lists.iter_mut().zip([None, Some(TypeName::from("memo"))]) {
        let filter = ProposalFilter {
            ty,
            ..ProposalFilter::default()
        };
        let got = l.cat.proposals(who, filter).await;
        *list = got.must("listing the proposals")?.data;
    }
    Ok(lists)
}

/// The owner calling `vet` on `c1`, which proposes it.
async fn vetted(l: &Lake) -> Result<lakelands_api::id::ProposalId, Failure> {
    match call(l, &l.own, "vet", "c1").await {
        Ok(Outcome::Proposed { proposal, .. }) => Ok(proposal),
        other => Err(Failure::new(format!(
            "the owner calling `vet` proposes it; {other:?}"
        ))),
    }
}

async fn a_verb_that_reads_a_field_its_caller_may_not_read_is_not_its_to_call(
    store: Store,
) -> Held {
    let (s, l, text) = (store.as_ref(), Lake::of(store.as_ref()), StorageKind::Text);
    l.declare(type_of("vault", &[("v", text)])).await?;
    let fields = ["status", "copy", "mark", "secret"].map(|n| field(n, text));
    let mut fields = fields.to_vec();
    fields.push(derived("shade", text, "secret", &["secret"]));
    fields.push(reference("partner", "vault"));
    l.declare(type_decl("memo", fields)).await?;
    l.upsert("vault", "v1", json!({"v": "v"})).await?;
    for (key, secret) in [("c1", "x"), ("c2", "y")] {
        let fields = json!({"status": "open", "secret": secret, "partner": "v1"});
        l.upsert("memo", key, fields).await?;
    }
    let unread = l.read("memo", "c1").await?.fields;
    let to = |status: &str| json!([{"field": "status", "value": format!("'{status}'")}]);
    for (name, risk, requires, set) in [
        ("approve", "low", Some("secret = 'x'"), to("approved")),
        (
            "leak",
            "low",
            None,
            json!([{"field": "copy", "value": "secret"}]),
        ),
        ("vet", "high", Some("secret = 'x'"), to("vetted")),
        ("close", "low", None, to("closed")),
        ("peek", "low", Some("shade = 'x'"), to("peeked")),
        ("vouch", "low", Some("partner = 'v1'"), to("vouched")),
        (
            "seal",
            "low",
            None,
            json!([{"field": "mark", "value": "'m'"}]),
        ),
    ] {
        let mut verb = json!({"declare": "verb", "name": name, "on": "memo", "risk": risk,
                              "effect": "write", "set": set});
        if let Some(r) = requires {
            verb["requires"] = json!(r);
        }
        l.declare_json(verb).await?;
    }

    // A verb the caller may not write, and one that reads what it may not
    // read, are answered alike.
    let nope = Err(Error::Refused {
        field: None,
        reason: Refusal::NotGranted,
    });
    let unmet = |verb: &str, key: &str| {
        let (verb, ty, key) = (verb.into(), "memo".into(), key.into());
        let case = Case::RequiresUnmet { verb, ty, key };
        Err(Error::Refused {
            field: None,
            reason: Refusal::Precondition(Box::new(case.into())),
        })
    };
    let (applied, proposed) = (Ok("applied"), Ok("proposed"));
    let refused = || {
        let mut want = vec![nope.clone(); 8];
        want[6] = applied.clone();
        want
    };
    let read = |shown: bool| {
        let (unmet, vouch) = (unmet("approve", "c2"), if shown { &applied } else { &nope });
        vec![
            nope.clone(),
            applied.clone(),
            unmet.clone(),
            applied.clone(),
            proposed.clone(),
            unmet,
            vouch.clone(),
            applied.clone(),
        ]
    };
    let all = ["approve", "close", "leak", "peek", "vet", "vouch"];
    let named = |n: &[&str]| json!([n, n]);
    // Each row: `secret` as held, whether `vault` is shown, the answers, the
    // verbs named, and whether `vet`'s proposals are there to the reviewer.
    let rows = [
        (
            Access::None,
            true,
            refused(),
            named(&["close", "vouch"]),
            false,
        ),
        (
            Access::Masked,
            true,
            refused(),
            named(&["close", "vouch"]),
            false,
        ),
        (
            Access::WriteOnly,
            true,
            refused(),
            named(&["close", "vouch"]),
            false,
        ),
        (Access::Read, false, read(false), named(&all[..5]), true),
        (Access::Read, true, read(true), named(&all), true),
    ];
    let mut red = Vec::new();
    for (secret, vault, want, verbs, listed) in rows {
        let held = format!("`secret` held {secret:?} and `vault` shown {vault}");
        let id = |who: &str| format!("{who}-{secret:?}-{vault}");
        let hidden = |mut p: Principal| {
            if !vault {
                let none = TypeGrants {
                    default: Some(Access::None),
                    ..Default::default()
                };
                p.grants.types.insert("vault".into(), none);
            }
            p
        };
        let (w, r) = (Access::Write, Access::Read);
        let caller = [("status", w), ("copy", w), ("secret", secret)];
        let caller = hidden(field_access(s, &id("caller"), "memo", &caller));
        let reviewer = [("status", r), ("copy", r), ("secret", secret)];
        let mut reviewer = hidden(field_access(s, &id("reviewer"), "memo", &reviewer));
        reviewer.grants.review = true;
        let mut got = Vec::new();
        for (verb, key) in [
            ("seal", "c1"),
            ("approve", "c1"),
            ("approve", "c2"),
            ("leak", "c1"),
            ("vet", "c1"),
        ] {
            got.push(said(&call(&l, &caller, verb, key).await));
        }
        let (verb, key, opts) = ("approve".into(), Some("c2".into()), WriteOpts::default());
        let payload = patch(json!({}));
        let write = ProposedWrite::Call {
            verb,
            key,
            payload,
            opts,
        };
        got.push(said(&l.cat.propose(&caller, write, "why".into()).await));
        got.push(said(&call(&l, &caller, "vouch", "c2").await));
        got.push(said(&call(&l, &caller, "peek", "c1").await));
        let before = listings(&l, &reviewer).await?;
        let (accepted, pending) = (vetted(&l).await?, vetted(&l).await?);
        let mut want = want;
        if listed {
            got.push(said(&l.cat.accept(&reviewer, &accepted).await));
            want.push(applied.clone());
        } else {
            let (why, gone) = ("why".to_owned(), absent::<&str>(&pending));
            got.push(said(&l.cat.accept(&reviewer, &accepted).await));
            got.push(said(&l.cat.reject(&reviewer, &pending, why).await));
            want.extend([absent(&accepted), gone]);
        }
        if got != want {
            red.push(format!(
                "with {held} the answers must be {want:?}; they were {got:?}"
            ));
        }
        let got = shown(&l, &caller).await?;
        if got != verbs {
            red.push(format!(
                "with {held} describe must name {verbs}; it names {got}"
            ));
        }
        let listing = listings(&l, &reviewer).await?;
        let entry = listing[0].iter().find(|p| p.id == pending);
        let got = entry.map(|p| (p.write.is_some(), p.more));
        if listed && got != Some((true, false)) {
            red.push(format!(
                "with {held} `vet`'s proposal must be listed whole; it was {got:?}"
            ));
        }
        // Not there: the listings are those of the lake before the owner's
        // two proposals, which is the lake without them.
        if !listed && listing != before {
            red.push(format!(
                "with {held} the listings must be {before:?} without `vet`'s proposals; \
                 they were {listing:?}"
            ));
        }
        if !secret.reads() {
            let record = l.read("memo", "c1").await?;
            if record.fields != unread {
                let got = &record.fields;
                red.push(format!(
                    "with {held} `c1` must stay {unread:?}; it holds {got:?}"
                ));
            }
        }
    }
    // An id no proposal holds is answered so, and a proposal of a verb
    // declared again since is not there before it is told redeclared.
    let none = ProposalId::from("none");
    let said_none = [
        said(&l.cat.accept(&l.own, &none).await),
        said(&l.cat.reject(&l.own, &none, "why".into()).await),
    ];
    if said_none != [absent(&none), absent(&none)] {
        red.push(format!(
            "an id no proposal holds is not found; {said_none:?}"
        ));
    }
    let pending = vetted(&l).await?;
    let again = json!({"declare": "verb", "name": "vet", "on": "memo", "risk": "high",
                       "requires": "secret = 'x'", "effect": "write", "set": to("checked")});
    l.declare_json(again).await?;
    let reviewer = |id: &str, secret| {
        let grants = [("status", Access::Read), ("secret", secret)];
        let mut p = field_access(s, id, "memo", &grants);
        p.grants.review = true;
        p
    };
    let (blind, sighted) = (
        reviewer("blind", Access::None),
        reviewer("sighted", Access::Read),
    );
    let got = [
        said(&l.cat.accept(&blind, &pending).await),
        said(&l.cat.reject(&blind, &pending, "why".into()).await),
    ];
    if got != [absent(&pending), absent(&pending)] {
        red.push(format!(
            "once `vet` is declared again its proposal is still not there to a reviewer \
             who may not read `secret`; {got:?}"
        ));
    }
    let redeclared = l.cat.accept(&sighted, &pending).await;
    let verb = "vet".into();
    if invalid_case(&redeclared) != Some(&Case::VerbRedeclared { verb }) {
        red.push(format!(
            "a reviewer who reads `secret` is told `vet` was declared again; {redeclared:?}"
        ));
    }
    ensure!(
        red.is_empty(),
        "a verb that reads a field its caller may not read is not that caller's to call, \
         propose, accept, be shown or have listed: {}",
        red.join("; ")
    );
    Ok(())
}
