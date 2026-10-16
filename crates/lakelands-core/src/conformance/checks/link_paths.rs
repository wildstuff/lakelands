//! A path that begins at a relation carried by link records reads the one
//! link its record holds under it: a verb's `requires` and set values read
//! the link's fields and, through its `to`, the far record's (055,
//! extended).

use lakelands_api::error::Case;
use lakelands_api::key::link_key;
use lakelands_api::ontology::{
    Cardinality, Derived, MatchPair, RelationDecl, StorageKind, TypeDecl,
};
use lakelands_api::write::{Declaration, ProposalFilter, ProposedWrite};
use lakelands_api::{Access, Error, Find, Outcome, Principal, ReadAt, Refusal, Version, WriteOpts};
use serde_json::{Value, json};

use super::only::holder;
use super::paths::{accept, call, proposal_of, unlisted};
use crate::conformance::fixture::{
    Lake, Must, field, field_access, head_unmoved, limits, migration, patch, reference,
    refused_like, restrict, type_decl,
};
use crate::conformance::{Failure, Held, Store};

const LT: &str = "sys.link.deal.with";

check!(
    LINK_PATH_READS = a_verbs_path_reads_the_one_link_a_relation_holds,
    "055, extended (lakelands#567): a verb's `requires` and set values read the one link a record holds under a relation carried by link records: the link's fields (`with.days`) and, through its `to`, the far record's (`with.to.discount_days`), a link whose far end is retired refusing `not_live` on `to`; the verb runs where the link meets its condition, is refused precondition where it does not, copies a link's field, and a record with no such link reads null there; `exists(with)` holds where the record holds its link and not where it holds none; each step counts toward the lake's `max_depth`; a relation that may hold several links from a record and one carried by a field are refused at the declaration (`relation_holds_many`, `relation_via_field`), and one derived from values other than its target's key is (`match_not_key`), a relation's name alone and a field its links do not have are `unknown_field`, and so is a relation in a read's `where`; an author who may not know of the link type is told `unknown_field` on the verb's type for a path through it, whether the link has the field or not; a migration that retargets the relation under a verb that reads across it, or lets it hold several links from a record, is refused naming the verb, and so is the type declared again without a migration, by `declare` with the relation holding several links or a field under the relation's name, and by a set with the relation holding several links, each naming the verb; judged over the lake as it is, an author not shown the relation adds a field to the type, and one adding a field of the relation's name is told the type could not be declared"
);

check!(
    LINK_PATH_HELD =
        a_verb_that_reads_a_link_is_its_callers_only_where_the_relation_holds_nothing_from_it,
    "055, extended (lakelands#567): a verb whose path steps through a relation's link is its caller's only where nothing of the link type and its end types is held from the caller: one without `restricted`, one holding `only` on the link type, on the verb's own type or on the far end's type, one not shown the far end's type, not shown the link type or kept from a link field the verb reads (`from` for `exists(with)`), is refused not_granted, as is one whose `only` holds either end of the link a verb reaches over a derived relation onto the link type, before any record is read, appending nothing, whether the record's link is held, outside its `only`, absent, or the version it sends stale, and neither `describe` nor `describe_type` names the verb to it; one the relation holds nothing from calls as the link holds, held or not"
);

check!(
    LINK_PATH_PROPOSED = a_proposed_call_reaches_the_link_it_reads,
    "055 and 006, extended (lakelands#567): what a proposed call reaches holds the link it reads across a relation: a call proposed while its link was held is accepted, its proposer having been answered the link; one whose link is held since is refused not_granted at the accept, the proposer holding the link then no more, and so is one that reads the link alone whose far end is held since, where one whose far end was held at the proposal is accepted; neither is there to a reviewer the relation holds something from, nor one whose link is not held to a reviewer holding `only` on the far end; a declaration of such a verb is listed with it to a reviewer shown the relation and every field it reads across, with `schema` or without, whether the verb is that reviewer's to call or not, and not to one not shown the far type or kept from a link field or a far field, with `schema` or without, and the accept of a call whose link no longer meets its `requires` is told `requires_unmet` by a reviewer who does not read every field of the lake"
);

/// The deals, related to `target` by `with`, carried by link records
/// holding `note` and `days` as `with` says, and to customers by `pair`
/// (to many), `by` (carried by `cust`) and `alike` (derived).
fn deal(with: Cardinality, target: &str) -> TypeDecl {
    let (text, number) = (StorageKind::Text, StorageKind::Number);
    let fields = vec![field("grade", text), field("owner", text)];
    let mut deal = type_decl(
        "deal",
        [fields, vec![reference("cust", "customer")]].concat(),
    );
    let pairs = vec![MatchPair {
        from: "owner".into(),
        to: "owner".into(),
    }];
    deal.relations = vec![
        RelationDecl {
            cardinality: with,
            fields: vec![field("note", text), field("days", number)],
            ..RelationDecl::to("with", target)
        },
        RelationDecl {
            cardinality: Cardinality::OneToMany,
            ..RelationDecl::to("pair", "customer")
        },
        RelationDecl {
            via: Some("cust".into()),
            ..RelationDecl::to("by", "customer")
        },
        RelationDecl {
            derived: Some(Derived {
                pairs,
                filter: None,
            }),
            ..RelationDecl::to("alike", "customer")
        },
    ];
    deal
}

/// A verb on `deal`, declared as JSON.
fn verb(name: &str, risk: &str, requires: &str, set: (&str, &str)) -> Value {
    json!({"declare": "verb", "name": name, "on": "deal", "risk": risk, "requires": requires,
           "effect": "write", "set": [{"field": set.0, "value": set.1}]})
}

/// The refusal of `verb` on the deal `key` whose `requires` is unmet.
fn unmet(verb: &str, key: &str) -> Result<&'static str, Error> {
    let (verb, ty, key) = (verb.into(), "deal".into(), key.into());
    let case = Case::RequiresUnmet { verb, ty, key };
    Err(Error::Refused {
        field: None,
        reason: Refusal::Precondition(Box::new(case.into())),
    })
}

/// The refusal of a verb that is not its caller's.
fn nope() -> Result<&'static str, Error> {
    Err(Error::Refused {
        field: None,
        reason: Refusal::NotGranted,
    })
}

/// `who` linking the deal `from` to the customer `to` over `with`.
async fn link(l: &Lake, who: &Principal, from: &str, to: &str, fields: Value) -> Result<(), Error> {
    let (ty, with, opts) = ("deal".into(), "with".into(), WriteOpts::default());
    let (from, to) = (from.into(), to.into());
    let linked = l.cat.link(who, &ty, &from, &with, &to, patch(fields), opts);
    linked.await.map(|_| ())
}

/// The lake of these rows: the one of the path rows, deals linked over
/// `with` (`d1` to `c1`, `d2` to `c2`; `dh` to `c6`, which is held; `dl` to
/// `c7` by a held link; `do` to `c5`, owned by `b`), `d3` with no link,
/// and the verbs `close` and `bare`, which read across `with`.
async fn lake(store: &Store) -> Result<Lake, Failure> {
    let l = super::paths::lake(store).await?;
    l.declare(deal(Cardinality::ManyToOne, "customer")).await?;
    for (key, owner) in [("c5", "b"), ("c6", "a"), ("c7", "a")] {
        let fields = json!({"owner": owner, "discount_days": 20});
        l.upsert("customer", key, fields).await?;
    }
    for (deal, to, note, days) in [
        ("d1", "c1", "a", 20),
        ("d2", "c2", "b", 1),
        ("d3", "", "", 0),
        ("d5", "c1", "e", 20),
        ("dh", "c6", "x", 20),
        ("dl", "c7", "y", 1),
        ("do", "c5", "z", 30),
    ] {
        l.upsert("deal", deal, json!({"grade": "new", "owner": "a"}))
            .await?;
        if !to.is_empty() {
            let linked = link(&l, &l.own, deal, to, json!({"note": note, "days": days}));
            linked.await.must("linking a deal to its customer")?;
        }
    }
    restrict(store.as_ref(), "customer", "c6", "a hold").await?;
    restrict(
        store.as_ref(),
        LT,
        link_key(&"dl".into(), &"c7".into()),
        "a hold",
    )
    .await?;
    let requires = "with.days >= 10 and with.to.discount_days >= 10";
    l.declare_json(verb("close", "low", requires, ("grade", "with.note")))
        .await?;
    l.declare_json(verb(
        "bare",
        "low",
        "with.note is null",
        ("grade", "'bare'"),
    ))
    .await?;
    Ok(l)
}

async fn a_verbs_path_reads_the_one_link_a_relation_holds(store: Store) -> Held {
    let l = lake(&store).await?;
    let mut got = Vec::new();
    for (verb, deal) in [
        ("close", "d1"),
        ("close", "d2"),
        ("close", "d3"),
        ("bare", "d3"),
        ("bare", "d2"),
    ] {
        got.push(call(&l, &l.own, verb, deal).await);
    }
    let want = [
        Ok("applied"),
        unmet("close", "d2"),
        unmet("close", "d3"),
        Ok("applied"),
        unmet("bare", "d2"),
    ];
    ensure!(
        got == want,
        "a verb reads the link its deal holds and the link's far end: {got:?}, expected {want:?}"
    );
    let grade = l.read("deal", "d1").await?.fields.get("grade").cloned();
    ensure!(
        grade == Some(json!("a")),
        "a value is copied from the link; the deal holds {grade:?}"
    );
    // A link whose far end is retired goes on to it: `not_live` on `to`.
    l.upsert("customer", "c8", json!({"owner": "a", "discount_days": 20}))
        .await?;
    l.upsert("deal", "dr", json!({"grade": "new"})).await?;
    let linked = link(&l, &l.own, "dr", "c8", json!({"days": 20}));
    linked.await.must("linking `dr`")?;
    l.retire("customer", "c8").await?;
    let got = call(&l, &l.own, "close", "dr").await;
    let named = matches!(&got, Err(Error::Refused { field: Some(f), reason: Refusal::Constraint(said) })
        if f.as_str() == "to" && matches!(&said.case, Case::NotLive { key, .. } if key.as_str() == "c8"));
    ensure!(
        named,
        "a link whose far end is retired is read on to it, `not_live` on `to`; {got:?}"
    );

    // `exists` answers whether the record holds its link.
    l.declare_json(verb("linked", "low", "exists(with)", ("grade", "'linked'")))
        .await?;
    let got = [
        call(&l, &l.own, "linked", "d1").await,
        call(&l, &l.own, "linked", "d3").await,
    ];
    ensure!(
        got == [Ok("applied"), unmet("linked", "d3")],
        "`exists(with)` holds for a deal that holds its link and not for one without; {got:?}"
    );

    // The link is a step toward `max_depth`, and so is its `to`.
    let max = limits().max_depth;
    for (name, parents) in [("deep", max - 2), ("deeper", max - 1)] {
        let far = ".parent".repeat(parents as usize);
        let requires = format!("with.to{far}.discount_days is null");
        l.declare_json(verb(name, "low", &requires, ("grade", "'deep'")))
            .await?;
    }
    let deep = call(&l, &l.own, "deep", "d1").await;
    let deeper = call(&l, &l.own, "deeper", "d1").await;
    ensure!(
        deep == Ok("applied"),
        "a path of `max_depth` steps through a link is read; {deep:?}"
    );
    refused_as!(
        "a path through a link that takes more steps than `max_depth`",
        deeper,
        Case::StepTooDeep { max: m, asked } if *m == max && *asked == max + 1
    )?;

    // What a path cannot read is refused at the declaration.
    for (requires, want) in [
        (
            "pair.days = 1",
            json!({"code": "relation_holds_many", "relation": "pair"}),
        ),
        (
            "alike.days = 1",
            json!({"code": "match_not_key", "relation": "alike", "target": "customer"}),
        ),
        (
            "by.name = 'x'",
            json!({"code": "relation_via_field", "relation": "by", "field": "cust"}),
        ),
        (
            "with = 'x'",
            json!({"code": "unknown_field", "type": "deal", "field": "with"}),
        ),
        (
            "with.nope = 1",
            json!({"code": "unknown_field", "type": LT, "field": "nope"}),
        ),
    ] {
        let v = serde_json::from_value(verb("odd", "low", requires, ("grade", "'x'")));
        let declared = l.cat.declare(&l.own, v.expect("a declaration")).await;
        refused_like(requires, declared, &want)?;
    }
    let unknown = json!({"code": "unknown_field", "type": "deal", "field": "with"});
    // An author who may not know of the link type is told of the path what
    // a field `deal` does not have tells, whatever the link holds.
    let blind = json!({"default": "write", "schema": true, "types": {LT: {"default": "none"}}});
    let blind = holder(store.as_ref(), "blind", blind);
    for requires in ["with.note is null", "with.nope is null"] {
        let v = serde_json::from_value(verb("odd", "low", requires, ("grade", "'x'")));
        let declared = l.cat.declare(&blind, v.expect("a declaration")).await;
        refused_like(requires, declared, &unknown)?;
    }
    let find = Find {
        filter: Some("with.note = 'a'".into()),
        ..Find::default()
    };
    let found = l.cat.find(&l.own, &"deal".into(), find).await;
    refused_like("a read's `where` through a relation", found, &unknown)?;

    // A migration is judged against the verbs that read across the
    // relation: `close` reads its far end, `bare` the link alone.
    for (what, to, want) in [
        (
            "the relation retargeted under a verb that reads its far end",
            deal(Cardinality::ManyToOne, "term"),
            json!({"code": "unknown_field", "type": "term", "field": "discount_days",
                   "at": [{"code": "after_migration"}, {"code": "verb", "name": "close"}]}),
        ),
        (
            "the relation let hold several links from a record",
            deal(Cardinality::ManyToMany, "customer"),
            json!({"code": "relation_holds_many", "relation": "with",
                   "at": [{"code": "verb", "name": "bare"}]}),
        ),
    ] {
        refused_like(what, l.cat.migrate(&l.own, migration(to)).await, &want)?;
    }
    // And so is the type declared again where no migration is asked.
    let mut named = deal(Cardinality::ManyToOne, "customer");
    named.fields.push(field("with", StorageKind::Text));
    for (what, to, want) in [
        (
            "the relation declared again holding several links from a record",
            deal(Cardinality::ManyToMany, "customer"),
            json!({"code": "relation_holds_many", "relation": "with",
                   "at": [{"code": "verb", "name": "bare"}]}),
        ),
        (
            "a field declared under the relation's name",
            named,
            json!({"code": "steps_into_scalar", "field": "with",
                   "at": [{"code": "verb", "name": "bare"}]}),
        ),
    ] {
        refused_like(what, l.try_declare(to).await, &want)?;
    }
    let widened = vec![Declaration::Type(deal(Cardinality::ManyToMany, "customer"))];
    refused_like(
        "the relation widened in a set",
        l.cat.apply(&l.own, widened).await,
        &json!({"code": "relation_holds_many", "relation": "with",
                "at": [{"code": "verb", "name": "bare"}]}),
    )?;
    // An author not shown the relation adds a field: the verbs are judged
    // over the lake as it is, so nothing is said of what it is not shown.
    let author = json!({"default": "write", "schema": true,
                        "types": {"customer": {"default": "none"}}});
    let author = holder(store.as_ref(), "author", author);
    let (deal, memo) = ("deal".into(), field("memo", StorageKind::Text));
    let memo = l.cat.add_field(&author, &deal, memo).await;
    ensure!(
        memo.is_ok(),
        "an author not shown the relation adds a field to the type; {memo:?}"
    );
    let (deal, with) = ("deal".into(), field("with", StorageKind::Text));
    let named = l.cat.add_field(&author, &deal, with).await;
    let gone = json!({"code": "no_longer_declarable", "type": "deal"});
    refused_like(
        "an author not shown the relation adds a field of its name",
        named,
        &gone,
    )?;
    Ok(())
}

async fn a_verb_that_reads_a_link_is_its_callers_only_where_the_relation_holds_nothing_from_it(
    store: Store,
) -> Held {
    let (s, l) = (store.as_ref(), lake(&store).await?);
    let grants = |more: Value| {
        let mut g = json!({"default": "write", "restricted": true});
        if let (Some(g), Value::Object(more)) = (g.as_object_mut(), more) {
            g.extend(more);
        }
        g
    };
    let only = |ty: &str, f: &str, v: &str| json!({"types": {ty: {"only": {f: v}}}});
    let none = |ty: &str| json!({"types": {ty: {"default": "none"}}});
    let mut nonote = field_access(s, "nonote", LT, &[("note", Access::None)]);
    (nonote.grants.default, nonote.grants.restricted) = (Access::Write, true);
    let mut held = vec![
        holder(s, "plain", json!({"default": "write"})),
        holder(s, "onlylink", grants(only(LT, "note", "a"))),
        holder(s, "onlydeal", grants(only("deal", "owner", "a"))),
        holder(s, "onlyend", grants(only("customer", "owner", "a"))),
        holder(s, "noend", grants(none("customer"))),
        holder(s, "nolink", grants(none(LT))),
    ];
    held.push(nonote);
    let stale = WriteOpts {
        expected_version: Some(Version(7)),
        ..WriteOpts::default()
    };
    let before = s.head().await.must("reading the head")?;
    let mut red = Vec::new();
    for who in &held {
        for deal in ["d1", "d3", "dh", "dl", "do", "dz"] {
            for verb in ["close", "bare"] {
                let got = call(&l, who, verb, deal).await;
                if got != nope() {
                    red.push(format!(
                        "`{}` calling `{verb}` on `{deal}`: {got:?}",
                        who.id
                    ));
                }
            }
        }
        let (bare, d1) = ("bare".into(), "d1".into());
        let got = l
            .cat
            .call(who, &bare, Some(&d1), patch(json!({})), stale.clone());
        if !matches!(
            got.await,
            Err(Error::Refused {
                reason: Refusal::NotGranted,
                ..
            })
        ) {
            red.push(format!("`{}` with a stale version", who.id));
        }
        let d = l.cat.describe(who, ReadAt::default()).await;
        let d = d.must("describing the lake")?.data;
        let named = d
            .verbs
            .iter()
            .filter(|v| ["close", "bare"].contains(&v.name.as_str()));
        if named.count() > 0 {
            red.push(format!("`{}` is described the verbs {:?}", who.id, d.verbs));
        }
        let t = l
            .cat
            .describe_type(who, &"deal".into(), ReadAt::default())
            .await;
        let verbs = t.must("describing `deal`")?.data.verbs;
        if verbs
            .iter()
            .any(|v| ["close", "bare"].contains(&v.as_str()))
        {
            red.push(format!("`{}` is described `deal` with {verbs:?}", who.id));
        }
    }
    ensure!(
        red.is_empty(),
        "a verb that reads a link is not the caller's where the relation holds something \
         from it: {}",
        red.join("; ")
    );
    head_unmoved(s, before, "a call of a verb not the caller's").await?;
    // `exists(with)` reads the link's `from`, as a path through it does.
    l.declare_json(verb("linked", "low", "exists(with)", ("grade", "'linked'")))
        .await?;
    let mut nofrom = field_access(s, "nofrom", LT, &[("from", Access::Masked)]);
    (nofrom.grants.default, nofrom.grants.restricted) = (Access::Write, true);
    let got = call(&l, &nofrom, "linked", "d1").await;
    ensure!(
        got == nope(),
        "`exists(with)` is not the verb of a caller masked from the link's `from`; {got:?}"
    );
    // A derived relation onto the link type reaches a link by its ends'
    // keys: it is a caller's only where those ends hold nothing from it.
    let mut memo = type_decl(
        "memo",
        vec![
            reference("d", "deal"),
            reference("c", "customer"),
            field("note", StorageKind::Text),
        ],
    );
    let pairs = vec![
        MatchPair {
            from: "d".into(),
            to: "from".into(),
        },
        MatchPair {
            from: "c".into(),
            to: "to".into(),
        },
    ];
    memo.relations = vec![RelationDecl {
        derived: Some(Derived {
            pairs,
            filter: None,
        }),
        ..RelationDecl::to("onlink", LT)
    }];
    l.declare(memo).await?;
    l.upsert("memo", "m5", json!({"d": "do", "c": "c5"}))
        .await?;
    let nolink = json!({"declare": "verb", "name": "nolink", "on": "memo", "risk": "low",
        "requires": "not exists(onlink)", "effect": "write", "set": [{"field": "note", "value": "'n'"}]});
    l.declare_json(nolink).await?;
    for (id, ty, owner) in [("onlyend2", "customer", "a"), ("onlyfrom", "deal", "b")] {
        let who = holder(s, id, grants(only(ty, "owner", owner)));
        let head = s.head().await.must("reading the head")?;
        let (nolink, m5) = ("nolink".into(), "m5".into());
        let opts = WriteOpts::default();
        let got = l.cat.call(&who, &nolink, Some(&m5), patch(json!({})), opts);
        let got = got.await;
        let d = l.cat.describe(&who, ReadAt::default()).await;
        let d = d.must("describing the lake")?.data;
        let named = d.verbs.iter().any(|v| v.name.as_str() == "nolink");
        ensure!(
            matches!(
                got,
                Err(Error::Refused {
                    reason: Refusal::NotGranted,
                    ..
                })
            ) && !named,
            "a verb over a derived relation onto a link type is not the caller's whose `only` \
             on `{ty}` holds an end of the link: {got:?}, described {named}"
        );
        head_unmoved(s, head, "`nolink` by a caller an end is held from").await?;
    }
    let free = holder(s, "free", grants(json!({})));
    let mut got = Vec::new();
    for (verb, deal) in [("close", "dh"), ("close", "dl"), ("bare", "d3")] {
        got.push(call(&l, &free, verb, deal).await);
    }
    let want = [Ok("applied"), unmet("close", "dl"), Ok("applied")];
    ensure!(
        got == want,
        "one the relation holds nothing from calls as the link holds: {got:?}, expected {want:?}"
    );
    Ok(())
}

async fn a_proposed_call_reaches_the_link_it_reads(store: Store) -> Held {
    let (s, l) = (store.as_ref(), lake(&store).await?);
    let vet = verb("vet", "high", "with.days >= 10", ("grade", "'vetted'"));
    l.declare_json(vet).await?;
    let reviewer = |id: &str, restricted: bool| {
        let grants = json!({"default": "read", "review": true, "restricted": restricted});
        holder(s, id, grants)
    };
    let (plain, holds) = (reviewer("plain", false), reviewer("holds", true));

    // The link held since one proposal and before another: neither is
    // there to `plain`; the proposer of the first is not answered the link
    // it read, the second's was.
    let since = proposal_of(&l, &l.own, "vet", "d1").await?;
    restrict(s, LT, link_key(&"d1".into(), &"c1".into()), "a hold").await?;
    let held = proposal_of(&l, &l.own, "vet", "d1").await?;
    for p in [&since, &held] {
        unlisted(s, &l, &plain, p).await?;
    }
    let got = [
        accept(&l, &holds, &since).await,
        accept(&l, &holds, &held).await,
    ];
    ensure!(
        got == [nope(), Ok("applied")],
        "a link held since the proposal is not its proposer's, one held before it was; \
         {got:?}"
    );

    // A declaration of such a verb is listed to a reviewer shown the
    // relation, and a call's refusal at the accept is told it as it is.
    let requires = "with.note = 'a' and with.to.discount_days > 0";
    let peek = verb("peek", "low", requires, ("grade", "'p'"));
    let declarations = vec![serde_json::from_value(peek).expect("a declaration")];
    let declare = ProposedWrite::Declare {
        declarations,
        plan: None,
    };
    let proposed = l.cat.propose(&l.own, declare, "why".into()).await;
    let Ok(Outcome::Proposed { proposal: dp, .. }) = proposed else {
        return Err(Failure::new(format!(
            "the owner proposes `peek`; {proposed:?}"
        )));
    };
    let partial = json!({"default": "read", "review": true, "restricted": true,
                         "types": {"term": {"fields": {"days": "none"}}}});
    let partial = holder(s, "partial", partial);
    let schema = json!({"default": "read", "review": true, "schema": true});
    // One that may call the verb, and one it would not be (no `restricted`).
    let caller = json!({"default": "write", "review": true, "restricted": true});
    let callers = [holder(s, "schema", schema), holder(s, "caller", caller)];
    for who in std::iter::once(partial.clone()).chain(callers) {
        let listed = l.cat.proposals(&who, ProposalFilter::default()).await;
        let listed = listed.must("listing the proposals")?.data;
        ensure!(
            listed.iter().any(|e| e.id == dp && e.write.is_some()),
            "a declaration of a verb that reads a link is listed with it to `{}`, shown the \
             relation and what the verb reads",
            who.id
        );
    }
    // Not with it to one not shown the relation, or kept from the field.
    let blind = [
        json!({"default": "read", "review": true, "types": {"customer": {"default": "none"}}}),
        json!({"default": "read", "review": true, "types": {LT: {"fields": {"note": "none"}}}}),
        json!({"default": "read", "review": true, "schema": true,
               "types": {"customer": {"default": "none"}}}),
        json!({"default": "read", "review": true, "schema": true,
               "types": {"customer": {"fields": {"discount_days": "none"}}}}),
        json!({"default": "read", "review": true, "schema": true,
               "types": {LT: {"fields": {"note": "none"}}}}),
        json!({"default": "read", "review": true,
               "types": {"customer": {"fields": {"discount_days": "none"}}}}),
    ];
    for (i, grants) in blind.into_iter().enumerate() {
        let who = holder(s, &format!("blind{i}"), grants);
        let listed = l.cat.proposals(&who, ProposalFilter::default()).await;
        let listed = listed.must("listing the proposals")?.data;
        ensure!(
            !listed.iter().any(|e| e.id == dp && e.write.is_some()),
            "a declaration of a verb that reads a link is listed with it to `{}`",
            who.id
        );
    }
    let p = proposal_of(&l, &l.own, "vet", "d5").await?;
    // A reviewer holding `only` on the link's far end is not listed it.
    let only = json!({"default": "read", "review": true, "restricted": true,
                      "types": {"customer": {"only": {"owner": "a"}}}});
    unlisted(s, &l, &holder(s, "onlyend", only), &p).await?;
    let moved = link(&l, &l.own, "d5", "c1", json!({"days": 1}));
    moved.await.must("writing the link again")?;
    let got = accept(&l, &partial, &p).await;
    ensure!(
        got == unmet("vet", "d5"),
        "the accept of a call its link no longer meets is told so; {got:?}"
    );
    // The reach holds the link's ends: a verb that reads the link alone,
    // its far end held before the proposal (`dm`) and since (`dn`).
    l.declare_json(verb("tag", "high", "with.note = 'n'", ("grade", "'t'")))
        .await?;
    l.upsert("customer", "c9", json!({"owner": "a", "discount_days": 20}))
        .await?;
    for (deal, to) in [("dm", "c9"), ("dn", "c2")] {
        l.upsert("deal", deal, json!({"grade": "new", "owner": "a"}))
            .await?;
        link(&l, &l.own, deal, to, json!({"note": "n"}))
            .await
            .must("linking a deal")?;
    }
    restrict(s, "customer", "c9", "a hold").await?;
    let before = proposal_of(&l, &l.own, "tag", "dm").await?;
    let since = proposal_of(&l, &l.own, "tag", "dn").await?;
    restrict(s, "customer", "c2", "a hold").await?;
    let got = [
        accept(&l, &holds, &before).await,
        accept(&l, &holds, &since).await,
    ];
    ensure!(
        got == [Ok("applied"), nope()],
        "a call whose link's far end was held at the proposal is accepted, one whose end is \
         held since is not its proposer's; {got:?}"
    );
    Ok(())
}
