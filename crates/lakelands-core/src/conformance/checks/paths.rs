//! A path steps through a reference to the one record it names and reads
//! a field there: a verb's `requires` and the values it sets read so, at
//! the call, the record as a write that sends the reference would find it.

use lakelands_api::error::{Case, Missing};
use lakelands_api::event::EventBody;
use lakelands_api::ontology::{Cardinality, FieldDecl, RelationDecl, StorageKind};
use lakelands_api::write::ProposalFilter;
use lakelands_api::{
    Access, Error, Find, LakeStore, Outcome, Principal, ProposalId, ReadAt, Record, Refusal,
    Version, WriteOpts,
};
use serde_json::{Value, json};

use super::only::holder;
use crate::conformance::fixture::{
    Lake, Must, derived, events_after, field, field_access, get_at, head_unmoved, invalid_case,
    limits, migration, patch, reference, refused_like, refused_on, restrict, source_record,
    sourced, try_upsert, type_decl,
};
use crate::conformance::{Failure, Held, Store};

check!(
    PATH_STEPS = a_verbs_condition_reads_a_field_of_the_record_a_reference_names,
    "055 (lakelands#567): a verb's `requires` and set values read a field of the record a reference names, by one step (`customer.discount_days`) and by two (`customer.term.days`), at the call: the verb runs where the far record meets the condition and is refused precondition where it does not, a far record changed between two calls is read as it stands, a value is copied from it, and a reference that is null reads null; what the call wrote stays when the far record changes after it, and a rebuild gives it again; a path of the lake's `max_depth` steps is read and a call whose path takes more is refused; a far field the type does not have, one computed at the read, a step into a far scalar and `changed` of a path are refused at the declaration, and a derived field keeps `steps_into_reference`; a migration that takes a field a verb reads across is refused, naming the verb, and so is taking such a field to the read; the preflight judges every verb on the type it migrates or reading across into it again, so a reference retargeted under a path, on the verb's type or a far one, a far reference made text, and a kind changed under a verb's own condition are refused, naming the verb to an author shown it and as a declaration no longer declarable to one who is not"
);

/// `who` calling `verb` on the invoice `key` with no payload: what it did,
/// or its error whole.
pub(super) async fn call(
    l: &Lake,
    who: &Principal,
    verb: &str,
    key: &str,
) -> Result<&'static str, Error> {
    let (verb, key, opts) = (verb.into(), key.into(), WriteOpts::default());
    let answer = l.cat.call(who, &verb, Some(&key), patch(json!({})), opts);
    match answer.await? {
        Outcome::Applied { .. } => Ok("applied"),
        Outcome::Proposed { .. } => Ok("proposed"),
    }
}

/// The refusal of `verb` on the invoice `key` whose `requires` is unmet.
pub(super) fn unmet(verb: &str, key: &str) -> Result<&'static str, Error> {
    let (verb, ty, key) = (verb.into(), "invoice".into(), key.into());
    let case = Case::RequiresUnmet { verb, ty, key };
    Err(Error::Refused {
        field: None,
        reason: Refusal::Precondition(Box::new(case.into())),
    })
}

/// A write verb on `invoice`, declared as JSON.
pub(super) fn verb(name: &str, risk: &str, requires: Option<&str>, set: (&str, &str)) -> Value {
    let mut verb = json!({"declare": "verb", "name": name, "on": "invoice", "risk": risk,
        "effect": "write", "set": [{"field": set.0, "value": set.1}]});
    if let Some(r) = requires {
        verb["requires"] = json!(r);
    }
    verb
}

/// The fields of `customer`, with `rate` or without, `soon` derived as
/// `soon` says.
fn customer(rate: bool, soon: &str) -> Vec<FieldDecl> {
    let (text, number) = (StorageKind::Text, StorageKind::Number);
    let mut fields = vec![field("name", text), field("discount_days", number)];
    fields.extend([field("owner", text), reference("term", "term")]);
    fields.push(reference("parent", "customer"));
    fields.push(derived(
        "soon",
        StorageKind::Boolean,
        soon,
        &["discount_days"],
    ));
    fields.extend(rate.then(|| field("rate", number)));
    fields
}

/// The lake of these rows: terms, customers that name one, invoices that
/// name a customer, and the cash-discount verbs on an invoice.
pub(super) async fn lake(store: &Store) -> Result<Lake, Failure> {
    let (l, text, number) = (
        Lake::of(store.as_ref()),
        StorageKind::Text,
        StorageKind::Number,
    );
    l.declare(type_decl("term", vec![field("days", number)]))
        .await?;
    l.declare(type_decl("customer", customer(true, "discount_days > 5")))
        .await?;
    let mut invoice = vec![field("status", text), field("days", number)];
    invoice.extend([field("copy", number), reference("customer", "customer")]);
    l.declare(type_decl("invoice", invoice)).await?;
    for (key, days) in [("t1", 30), ("t2", 5)] {
        l.upsert("term", key, json!({"days": days})).await?;
    }
    for (key, days, term) in [("c1", 14, "t1"), ("c2", 3, "t2")] {
        let fields = json!({"name": key, "owner": "a", "discount_days": days, "rate": 2,
            "term": term});
        l.upsert("customer", key, fields).await?;
    }
    for (key, customer) in [
        ("i1", json!("c1")),
        ("i2", json!("c2")),
        ("i3", Value::Null),
    ] {
        let fields = json!({"status": "open", "days": 10, "customer": customer});
        l.upsert("invoice", key, fields).await?;
    }
    for v in [
        verb(
            "take",
            "low",
            Some("status = 'open' and customer.discount_days >= days"),
            ("status", "'taken'"),
        ),
        verb(
            "term",
            "low",
            Some("customer.term.days >= days"),
            ("status", "'termed'"),
        ),
        verb("rate", "low", None, ("copy", "customer.rate * 2")),
        verb(
            "free",
            "low",
            Some("customer.discount_days is null"),
            ("status", "'free'"),
        ),
        verb("soon", "low", Some("customer.soon"), ("copy", "5")),
        verb("bump", "low", Some("days + 1 > 10"), ("copy", "8")),
    ] {
        l.declare_json(v).await?;
    }
    Ok(l)
}

check!(
    PATH_NOT_REACHED = a_call_that_reads_across_to_a_record_its_caller_is_not_answered_is_refused,
    "055 and 032, extended (lakelands#567): at a call a record read across a reference is reached exactly where a write that sends the reference would be accepted: a caller whose `requires` or set value comes to a customer that is restricted, outside its `only`, retired or taken by a source's purge is refused `not_live` naming the reference and the key the invoice holds, the answer its own upsert sending that key gets, whole, and the same whatever the customer's fields hold, also where the condition would hold over null; so is a second step to a held record, naming the far reference; nothing is appended; a condition decided before it comes to the path reads no record and runs; a stale version is told before the reach; a caller answered the record calls as it holds"
);

check!(
    PATH_READ_AS_A_FILTER =
        a_verb_that_reads_across_is_its_callers_only_where_it_reads_the_far_field,
    "055 and 006, extended (lakelands#567): a verb that reads a field across a reference is refused not_granted, appending nothing, to a caller who may not know of the far field, sees it masked or may only write it, who is not shown the far type, or who may not read a reference or a field further on the way, and `describe` does not name it to that caller; a verb that reads no such field stays that caller's, and a caller who reads them all calls both"
);

check!(
    PATH_LINK_ENDS = a_verb_on_a_link_type_reads_its_ends,
    "055 and 035, extended (lakelands#567): a link's `from` and `to` are references of the link record, so a verb on a link type reads a field of either end by the one rule (`from.grade`, `to.discount_days`): it runs where the ends meet its `requires`, is refused precondition where one does not, and sets the link's fields from its ends; a link with an end held from its caller is not there to the call, which is answered not found before `requires` is read, as a `get` of the link answers it, and an upsert proposed with a reference onto such a link is not there to a reviewer it is held from, and a call reading across a reference onto it is refused as its caller's upsert sending that reference; a migration of the type that carries the link type judges a verb on it that reads only its far end again, so retargeting the relation under it is refused"
);

async fn a_verb_on_a_link_type_reads_its_ends(store: Store) -> Held {
    const LT: &str = "sys.link.deal.with";
    let (s, l) = (store.as_ref(), lake(&store).await?);
    let (text, number) = (StorageKind::Text, StorageKind::Number);
    let mut deal = type_decl("deal", vec![field("grade", text)]);
    deal.relations = vec![RelationDecl {
        name: "with".into(),
        target: "customer".into(),
        cardinality: Cardinality::ManyToOne,
        fields: vec![field("note", text), field("days", number)],
        ..Default::default()
    }];
    l.declare(deal).await?;
    let seal = json!({"declare": "verb", "name": "seal", "on": LT, "risk": "low",
        "requires": "from.grade = 'a' and to.discount_days >= 10", "effect": "write",
        "set": [{"field": "days", "value": "to.discount_days + 1"},
                {"field": "note", "value": "from.grade"}]});
    l.declare_json(seal).await?;
    l.upsert("customer", "c4", json!({"owner": "a", "discount_days": 14}))
        .await?;
    let (ty, with, opts) = ("deal".into(), "with".into(), WriteOpts::default);
    for (deal, grade, customer) in [
        ("d1", "a", "c1"),
        ("d2", "a", "c2"),
        ("d3", "b", "c1"),
        ("d4", "a", "c4"),
    ] {
        l.upsert("deal", deal, json!({"grade": grade})).await?;
        let (from, to, none) = (deal.into(), customer.into(), patch(json!({})));
        let linked = l.cat.link(&l.own, &ty, &from, &with, &to, none, opts());
        linked.await.must("linking a deal to its customer")?;
    }
    restrict(s, "customer", "c4", "a hold").await?;
    let links = l.all_found(LT, Find::default()).await?;
    let key = |deal: &str| {
        let of = |r: &&Record| r.fields.get("from") == Some(&json!(deal));
        let found = links.iter().find(of).map(|r| r.key.as_str().to_owned());
        found.unwrap_or_default()
    };
    let unmet = |deal: &str| {
        let (verb, ty, key) = ("seal".into(), LT.into(), key(deal).as_str().into());
        let case = Case::RequiresUnmet { verb, ty, key };
        Err(Error::Refused {
            field: None,
            reason: Refusal::Precondition(Box::new(case.into())),
        })
    };
    let plain = holder(s, "plain", json!({"default": "write"}));
    let mut got = Vec::new();
    for deal in ["d1", "d2", "d3"] {
        got.push(call(&l, &plain, "seal", &key(deal)).await);
    }
    let want = [Ok("applied"), unmet("d2"), unmet("d3")];
    ensure!(
        got == want,
        "a verb on a link type reads both ends of its link: {got:?}, expected {want:?}"
    );
    let sealed = l.read(LT, &key("d1")).await?.fields;
    let set = (sealed.get("days"), sealed.get("note"));
    ensure!(
        set == (Some(&json!(15)), Some(&json!("a"))),
        "the link's fields are set from its ends; it holds {sealed:?}"
    );
    // A link with a held end is not there to a caller without the grant.
    let (d4, lt) = (key("d4"), LT.into());
    let get = l.cat.get(&plain, &lt, get_at(&d4, None)).await;
    let (get, called) = (get.map(|_| "found"), call(&l, &plain, "seal", &d4).await);
    let gone = |a: &Result<&str, Error>| matches!(a, Err(Error::NotFound { .. }));
    ensure!(
        gone(&get) && get == called,
        "a link with a held end is not found by its caller's `get` and its call alike; \
         {get:?}, {called:?}"
    );
    // An upsert proposed with a reference onto that link is not there to
    // a reviewer it is held from (lakelands#929).
    l.declare(type_decl(
        "memo",
        vec![reference("deal", LT), field("n", number)],
    ))
    .await?;
    let upsert = json!({"write": "upsert", "type": "memo", "key": "m1", "patch": {"deal": d4}});
    let proposed = l.cat.propose(
        &l.own,
        serde_json::from_value(upsert).expect("a write"),
        "why".into(),
    );
    let Ok(Outcome::Proposed { proposal, .. }) = proposed.await else {
        return Err(Failure::new("the owner proposes the memo".to_owned()));
    };
    let mut reviewer = plain.clone();
    reviewer.grants.review = true;
    unlisted(s, &l, &reviewer, &proposal).await?;
    let held = call(&l, &l.own, "seal", &d4).await;
    ensure!(
        held == Ok("applied"),
        "one answered both ends calls the verb on the link; {held:?}"
    );
    // A call reading across a reference onto that link, as its caller's
    // upsert sending the reference.
    let peek = json!({"declare": "verb", "name": "peek", "on": "memo", "risk": "low",
        "requires": "deal.note is null", "effect": "write", "set": [{"field": "n", "value": "1"}]});
    l.declare_json(peek).await?;
    l.upsert("memo", "m2", json!({"deal": d4})).await?;
    let sent = try_upsert(l.cat.as_ref(), &plain, "memo", "m3", json!({"deal": d4}));
    let (sent, got) = (
        sent.await.map(|_| "applied"),
        call(&l, &plain, "peek", "m2").await,
    );
    ensure!(
        got.is_err() && got == sent,
        "a call reading across to a link with a held end; {got:?}, expected {sent:?}"
    );

    // A migration of `deal` rewrites its link type: a verb on it that reads
    // only the far end is judged again, `due` before `seal` by name.
    let due = json!({"declare": "verb", "name": "due", "on": LT, "risk": "low",
        "requires": "to.discount_days >= 0", "effect": "write",
        "set": [{"field": "note", "value": "'due'"}]});
    l.declare_json(due).await?;
    let mut moved = type_decl("deal", vec![field("grade", text)]);
    moved.relations = vec![RelationDecl {
        name: "with".into(),
        target: "term".into(),
        cardinality: Cardinality::ManyToOne,
        fields: vec![field("note", text), field("days", number)],
        ..Default::default()
    }];
    let far = json!({"code": "unknown_field", "type": "term", "field": "discount_days",
                     "at": [{"code": "after_migration"}, {"code": "verb", "name": "due"}]});
    let refused = l.cat.migrate(&l.own, migration(moved)).await;
    refused_like(
        "a link type's end retargeted under a verb that reads only that end",
        refused,
        &far,
    )
}

check!(
    PATH_PROPOSED = a_proposed_call_reads_across_again_at_its_accept,
    "055, 006 and 013, extended (lakelands#567, lakelands#929): a call that reads across a reference is judged again at its accept, the far record read as it stands then: a `requires` it no longer meets refuses the accept precondition and a far record held since the proposal refuses it `not_live`, the proposal left pending and accepted once the record is as it was; what the call reaches holds the records it reads across, and a proposal that names a record its reviewer is not answered beside its own, read across by a call or named by an upsert's reference (lakelands#929), is not there to that reviewer: not listed, its accept and reject answered as an id no proposal holds, appending nothing, whether the record was held when it was proposed or since, for a reviewer without `restricted` and one holding `only` on the far type, while a reviewer answered the record decides it; a call proposed by a holder of `only` on the far type is logged pinned and its accept refused `write_changed_since_proposed` once a record it reads across has an event after the proposal, another's proposal is pinned to none; what a call read across when it was proposed counts beside what it reads at the accept, so a call a setting no longer brings to its customer is still not there to a reviewer the customer is held from, and still refused once its customer moved where it is pinned"
);

/// `who` calling the high-risk `vet` on the invoice `key`: its proposal.
async fn vetted(l: &Lake, who: &Principal, key: &str) -> Result<ProposalId, Failure> {
    proposal_of(l, who, "vet", key).await
}

/// `who` calling the high-risk `verb` on the invoice `key`: its proposal.
pub(super) async fn proposal_of(
    l: &Lake,
    who: &Principal,
    verb: &str,
    key: &str,
) -> Result<ProposalId, Failure> {
    let (name, key, opts) = (verb.into(), key.into(), WriteOpts::default());
    let called = l.cat.call(who, &name, Some(&key), patch(json!({})), opts);
    match called.await {
        Ok(Outcome::Proposed { proposal, .. }) => Ok(proposal),
        other => Err(Failure::new(format!(
            "calling `{verb}` on `{}` proposes it; {other:?}",
            key.as_str()
        ))),
    }
}

/// The accept of `p` by `who`: what it did, or its error whole.
pub(super) async fn accept(
    l: &Lake,
    who: &Principal,
    p: &ProposalId,
) -> Result<&'static str, Error> {
    l.cat.accept(who, p).await.map(|_| "applied")
}

/// Fails unless `p` is not there to `who`: not listed, its accept and
/// reject answered as an id no proposal holds, appending nothing.
pub(super) async fn unlisted(s: &dyn LakeStore, l: &Lake, who: &Principal, p: &ProposalId) -> Held {
    let before = s.head().await.must("reading the head")?;
    let listed = l.cat.proposals(who, ProposalFilter::default()).await;
    let listed = listed.must("listing the proposals")?.data;
    let gone = |a: &lakelands_api::Result<Outcome>| matches!(a, Err(Error::NotFound { missing: Missing::Proposal { id } }) if id == p);
    let (accepted, rejected) = (
        l.cat.accept(who, p).await,
        l.cat.reject(who, p, "why".into()).await,
    );
    ensure!(
        !listed.iter().any(|e| &e.id == p) && gone(&accepted) && gone(&rejected),
        "a proposal naming a record `{}` is not answered is not there to it; listed {}, \
         accept {accepted:?}, reject {rejected:?}",
        who.id,
        listed.iter().any(|e| &e.id == p)
    );
    head_unmoved(s, before, "a decision of a proposal that is not there").await
}

async fn a_proposed_call_reads_across_again_at_its_accept(store: Store) -> Held {
    let (s, l) = (store.as_ref(), lake(&store).await?);
    let requires = Some("customer.discount_days >= days");
    l.declare_json(verb("vet", "high", requires, ("status", "'vetted'")))
        .await?;
    for key in ["c7", "c8"] {
        let fields = json!({"owner": "a", "discount_days": 14});
        l.upsert("customer", key, fields).await?;
        let fields = json!({"status": "open", "days": 10, "customer": key});
        l.upsert("invoice", &key.replace('c', "i"), fields).await?;
    }
    let reviewer = |id: &str, more: Value| {
        let mut grants = json!({"default": "read", "review": true});
        if let (Some(g), Value::Object(more)) = (grants.as_object_mut(), more) {
            g.extend(more);
        }
        holder(s, id, grants)
    };
    let (plain, holds) = (
        reviewer("plain", json!({})),
        reviewer("holds", json!({"restricted": true})),
    );
    let head = || async { store.head().await.must("reading the head") };

    // Judged again at the accept, as the customer stands then.
    let p = vetted(&l, &l.own, "i1").await?;
    l.upsert("customer", "c1", json!({"discount_days": 3}))
        .await?;
    let stale = accept(&l, &plain, &p).await;
    l.upsert("customer", "c1", json!({"discount_days": 14}))
        .await?;
    let fresh = accept(&l, &plain, &p).await;
    ensure!(
        stale == unmet("vet", "i1") && fresh == Ok("applied"),
        "an accept reads the customer as it stands: unmet while it does not meet `requires`, \
         the proposal left pending, and applied once it does; {stale:?}, then {fresh:?}"
    );

    // A customer held since the proposal is not reached by the proposer,
    // who held nothing when it proposed; to a reviewer without the grant
    // the proposal is not there.
    let p = vetted(&l, &l.own, "i1").await?;
    restrict(s, "customer", "c1", "a hold").await?;
    unlisted(s, &l, &plain, &p).await?;
    let sighted = accept(&l, &holds, &p).await;
    let named = matches!(
        refused_on(&sighted),
        Some(("customer", Refusal::Constraint(said)))
            if matches!(&said.case, Case::NotLive { key, .. } if key.as_str() == "c1")
    );
    ensure!(
        named,
        "a customer held since the proposal: a reviewer with `restricted` is told `not_live` \
         on `customer`; {sighted:?}"
    );
    let (ty, key) = ("customer".into(), "c1".into());
    store.unrestrict(&ty, &key).await.must("lifting the hold")?;
    let lifted = accept(&l, &plain, &p).await;
    ensure!(
        lifted == Ok("applied"),
        "the proposal stayed pending and is accepted once the hold is lifted; {lifted:?}"
    );

    // One its proposer held when it proposed, by a call and by an upsert's
    // reference.
    restrict(s, "customer", "c1", "a hold").await?;
    let upsert = json!({"write": "upsert", "type": "invoice", "key": "i9",
                        "patch": {"status": "open", "customer": "c1"}});
    let upsert = serde_json::from_value(upsert).expect("a write");
    let proposed = l.cat.propose(&l.own, upsert, "why".into()).await;
    let Ok(Outcome::Proposed { proposal: up, .. }) = proposed else {
        return Err(Failure::new(format!(
            "the owner proposes the upsert; {proposed:?}"
        )));
    };
    let p = vetted(&l, &l.own, "i1").await?;
    for p in [&p, &up] {
        unlisted(s, &l, &plain, p).await?;
    }
    let got = [accept(&l, &holds, &p).await, accept(&l, &holds, &up).await];
    ensure!(
        got == [Ok("applied"), Ok("applied")],
        "a reviewer who holds `restricted` accepts a call that read a held customer and an \
         upsert naming it; {got:?}"
    );

    // A holder of `only` on the far type is pinned to what its call reads.
    let only = |owner: &str| json!({"types": {"customer": {"only": {"owner": owner}}}});
    let mut a = reviewer("a", only("a"));
    a.grants.default = Access::Write;
    let from = head().await?;
    let (moved, kept) = (vetted(&l, &a, "i7").await?, vetted(&l, &a, "i8").await?);
    let free = vetted(&l, &l.own, "i7").await?;
    let logged = events_after(s, from).await?;
    let pins: Vec<_> = (logged.iter())
        .filter_map(|e| match &e.body {
            EventBody::WriteProposed { pinned, .. } => Some(*pinned),
            _ => None,
        })
        .collect();
    ensure!(
        pins == [true, true, false],
        "a call proposed by a holder of `only` on the type it reads across is pinned, \
         another's is not; {pins:?}"
    );
    l.upsert("customer", "c7", json!({"name": "moved"})).await?;
    let got = [
        accept(&l, &holds, &moved).await,
        accept(&l, &holds, &kept).await,
        accept(&l, &holds, &free).await,
    ];
    let changed = matches!(
        got[0].as_ref().err().and_then(|e| invalid_case(&Err::<(), _>(e.clone())).cloned()),
        Some(Case::WriteChangedSinceProposed { key: Some(key), .. }) if key.as_str() == "i7"
    );
    ensure!(
        changed && got[1..] == [Ok("applied"), Ok("applied")],
        "a pinned call is refused once a record it reads across changed, one whose records \
         stand is accepted, and one that is not pinned reads the record as it stands; {got:?}"
    );

    // A reviewer holding `only` on the far type decides what it is answered.
    let (theirs, other) = (reviewer("theirs", only("a")), reviewer("other", only("b")));
    let p = vetted(&l, &l.own, "i8").await?;
    let upsert = json!({"write": "upsert", "type": "invoice", "key": "i10",
                        "patch": {"status": "open", "customer": "c8"}});
    let proposed = l.cat.propose(
        &l.own,
        serde_json::from_value(upsert).expect("a write"),
        "why".into(),
    );
    let Ok(Outcome::Proposed { proposal: up, .. }) = proposed.await else {
        return Err(Failure::new("the owner proposes the upsert".to_owned()));
    };
    for p in [&p, &up] {
        unlisted(s, &l, &other, p).await?;
    }
    let got = accept(&l, &theirs, &p).await;
    ensure!(
        got == Ok("applied"),
        "a reviewer holding `only` on the customers decides a call that reads its own \
         customer; {got:?}"
    );

    // What a call read across when it was proposed counts beside what it
    // reads now: a setting decides whether `gate` comes to the customer.
    let requires = Some("setting('open') = 'yes' or customer.discount_days >= days");
    l.declare_json(verb("gate", "high", requires, ("status", "'gated'")))
        .await?;
    l.set("open", json!("no")).await?;
    let pinned = proposal_of(&l, &a, "gate", "i8").await?;
    let read_then = proposal_of(&l, &l.own, "gate", "i7").await?;
    l.set("open", json!("yes")).await?;
    l.upsert("customer", "c8", json!({"name": "moved"})).await?;
    restrict(s, "customer", "c7", "a hold").await?;
    unlisted(s, &l, &plain, &read_then).await?;
    let got = accept(&l, &holds, &pinned).await;
    let changed = matches!(
        got.as_ref().err().and_then(|e| invalid_case(&Err::<(), _>(e.clone())).cloned()),
        Some(Case::WriteChangedSinceProposed { key: Some(key), .. }) if key.as_str() == "i8"
    );
    ensure!(
        changed,
        "a pinned call whose customer moved is refused though the call no longer reads it; {got:?}"
    );
    Ok(())
}

/// The customers and invoices of the rows of a record not reached: `ch`
/// held, `co` another owner's, `cr` retired, `c6` whose term `th` is
/// held, each with an invoice (`ih`, `io`, `ir`, `i6`) that would be taken.
async fn unreached(s: &Store, l: &Lake) -> Held {
    l.upsert("term", "th", json!({"days": 30})).await?;
    for (key, owner, term) in [
        ("ch", "a", "t1"),
        ("co", "b", "t1"),
        ("cr", "a", "t1"),
        ("c6", "a", "th"),
    ] {
        let fields = json!({"owner": owner, "discount_days": 14, "rate": 2, "term": term});
        l.upsert("customer", key, fields).await?;
        let fields = json!({"status": "open", "days": 10, "customer": key});
        l.upsert("invoice", &key.replace('c', "i"), fields).await?;
    }
    l.retire("customer", "cr").await?;
    restrict(s.as_ref(), "customer", "ch", "a hold").await?;
    restrict(s.as_ref(), "term", "th", "a hold").await?;
    Ok(())
}

async fn a_call_that_reads_across_to_a_record_its_caller_is_not_answered_is_refused(
    store: Store,
) -> Held {
    let l = lake(&store).await?;
    unreached(&store, &l).await?;
    for v in [
        verb(
            "open",
            "low",
            Some("status = 'open' or customer.discount_days > 100"),
            ("copy", "1"),
        ),
        verb(
            "shut",
            "low",
            Some("status = 'shut' and customer.discount_days > 100"),
            ("copy", "1"),
        ),
    ] {
        l.declare_json(v).await?;
    }
    let grants = json!({"default": "write", "types": {"customer": {"only": {"owner": "a"}}}});
    let a = holder(store.as_ref(), "a", grants);
    // The answer of the caller's own write that sends the reference.
    let mut sent = Vec::new();
    for key in ["ch", "co", "cr"] {
        let made = try_upsert(
            l.cat.as_ref(),
            &a,
            "invoice",
            "new",
            json!({"customer": key}),
        );
        sent.push(made.await.map(|_| "applied"));
    }
    let made = try_upsert(
        l.cat.as_ref(),
        &a,
        "customer",
        "new",
        json!({"owner": "a", "term": "th"}),
    );
    sent.push(made.await.map(|_| "applied"));
    ensure!(
        sent.iter().all(|s| matches!(
            s,
            Err(Error::Refused {
                reason: Refusal::Constraint(_),
                ..
            })
        )),
        "a write that sends a reference to a record its writer is not answered is refused; {sent:?}"
    );
    let mut red = Vec::new();
    for round in 0..2 {
        let before = store.head().await.must("reading the head")?;
        for verb in ["take", "free", "rate"] {
            for (invoice, sent) in ["ih", "io", "ir"].iter().zip(&sent) {
                let got = call(&l, &a, verb, invoice).await;
                if &got != sent {
                    red.push(format!("`{verb}` on `{invoice}` (round {round}): {got:?}"));
                }
            }
        }
        let got = call(&l, &a, "term", "i6").await;
        if got != sent[3] {
            red.push(format!("`term` on `i6` (round {round}): {got:?}"));
        }
        head_unmoved(
            store.as_ref(),
            before,
            "a call that reads across to no record",
        )
        .await?;
        // The held records change; the answers do not.
        for key in ["ch", "co"] {
            let fields = json!({"discount_days": 1, "rate": 7});
            l.upsert("customer", key, fields).await?;
        }
        l.upsert("term", "th", json!({"days": 1})).await?;
    }
    ensure!(
        red.is_empty(),
        "a call that reads across to a record its caller is not answered is answered as its write that sends the reference is ({sent:?}), whatever the record holds: {}",
        red.join("; ")
    );
    // A stale version is told before the reach.
    let before = store.head().await.must("reading the head")?;
    let (take, ih) = ("take".into(), "ih".into());
    let opts = WriteOpts {
        expected_version: Some(Version(7)),
        ..WriteOpts::default()
    };
    let stale = l
        .cat
        .call(&a, &take, Some(&ih), patch(json!({})), opts)
        .await;
    ensure!(
        matches!(stale, Err(Error::Conflict { .. })),
        "a stale version is told before a record not reached; {stale:?}"
    );
    let shut = call(&l, &a, "shut", "ih").await;
    ensure!(
        shut == unmet("shut", "ih"),
        "a condition decided before its path reads no record; {shut:?}"
    );
    head_unmoved(
        store.as_ref(),
        before,
        "a call that reads across to no record",
    )
    .await?;
    let open = call(&l, &a, "open", "ih").await;
    ensure!(
        open == Ok("applied"),
        "a condition decided before its path reads no record and runs; {open:?}"
    );
    // One answered the records calls as they hold.
    l.upsert("customer", "ch", json!({"discount_days": 14}))
        .await?;
    let got = [
        call(&l, &l.own, "take", "ih").await,
        call(&l, &l.own, "take", "io").await,
        call(&l, &l.own, "free", "ir").await,
    ];
    let want = [Ok("applied"), unmet("take", "io"), sent[2].clone()];
    ensure!(
        got == want,
        "a caller answered the customers calls as they hold, and a retired one is live to none: \
         {got:?}, expected {want:?}"
    );
    // A far record a source's purge took is live to none either.
    let number = StorageKind::Number;
    l.declare(type_decl("vendor", vec![sourced("days", number)]))
        .await?;
    l.declare(type_decl("bill", vec![reference("vendor", "vendor")]))
        .await?;
    let ingested = l.ingest(
        "erp",
        vec![source_record("vendor", "v1", json!({"days": 9}))],
    );
    ingested.await.must("ingesting the vendor")?;
    l.upsert("bill", "b1", json!({"vendor": "v1"})).await?;
    let pay = json!({"declare": "verb", "name": "pay", "on": "bill", "risk": "low",
        "requires": "vendor.days is null", "effect": "write",
        "set": [{"field": "vendor", "value": "'v1'"}]});
    l.declare_json(pay).await?;
    let purged = store.purge_source(&"erp".into(), None, None).await;
    purged.must("purging the vendor's source")?;
    let sent = try_upsert(
        l.cat.as_ref(),
        &l.own,
        "bill",
        "b2",
        json!({"vendor": "v1"}),
    );
    let sent = sent.await.map(|_| "applied");
    let got = call(&l, &l.own, "pay", "b1").await;
    ensure!(
        got.is_err() && got == sent,
        "a call reading across to a purged record is answered as a write sending its key; \
         {got:?}, expected {sent:?}"
    );
    Ok(())
}

async fn a_verb_that_reads_across_is_its_callers_only_where_it_reads_the_far_field(
    store: Store,
) -> Held {
    let (s, l) = (store.as_ref(), lake(&store).await?);
    let nope = Err(Error::Refused {
        field: None,
        reason: Refusal::NotGranted,
    });
    let hidden = |ty: &str| json!({"default": "write", "types": {ty: {"default": "none"}}});
    let mut red = Vec::new();
    let on = |id: &str, ty: &str, field: &str, access| field_access(s, id, ty, &[(field, access)]);
    // Each: the caller, and whether `take`, which reads
    // `customer.discount_days`, and `term`, which reads `customer.term.days`,
    // are its to call.
    for (who, take, term) in [
        (field_access(s, "reads", "customer", &[]), true, true),
        (
            on("none", "customer", "discount_days", Access::None),
            false,
            true,
        ),
        (
            on("masked", "customer", "discount_days", Access::Masked),
            false,
            true,
        ),
        (
            on("blind", "customer", "discount_days", Access::WriteOnly),
            false,
            true,
        ),
        (on("way", "customer", "term", Access::None), true, false),
        (on("second", "term", "days", Access::Masked), true, false),
        (holder(s, "far", hidden("customer")), false, false),
    ] {
        let (mut who, id) = (who.clone(), who.id);
        who.grants.default = Access::Write;
        let mut got = Vec::new();
        for verb in ["take", "term"] {
            let before = store.head().await.must("reading the head")?;
            let answer = call(&l, &who, verb, "i1").await;
            let moved = store.head().await.must("reading the head")? != before;
            got.push((answer, moved));
        }
        let answer = |mine| {
            if mine {
                (Ok("applied"), true)
            } else {
                (nope.clone(), false)
            }
        };
        let want = [answer(take), answer(term)];
        if got != want {
            red.push(format!("`{id}` must be answered {want:?}; it was {got:?}"));
        }
        let d = l.cat.describe(&who, ReadAt::default()).await;
        let d = d.must("describing the lake")?.data;
        let named = |verb: &str| d.verbs.iter().any(|v| v.name.as_str() == verb);
        if [named("take"), named("term")] != [take, term] {
            red.push(format!("`{id}` is described the verbs {:?}", d.verbs));
        }
        l.upsert("invoice", "i1", json!({"status": "open"})).await?;
    }
    ensure!(
        red.is_empty(),
        "a verb that reads across is its caller's only where the caller reads the far field: {}",
        red.join("; ")
    );
    Ok(())
}

async fn a_verbs_condition_reads_a_field_of_the_record_a_reference_names(store: Store) -> Held {
    let l = lake(&store).await?;
    let mut got = Vec::new();
    for (verb, key) in [
        ("take", "i2"),
        ("take", "i1"),
        ("term", "i2"),
        ("rate", "i2"),
        ("free", "i2"),
        ("free", "i3"),
    ] {
        got.push(call(&l, &l.own, verb, key).await);
    }
    // The customer changed: the next call reads it as it stands.
    l.upsert("customer", "c2", json!({"discount_days": 20, "term": "t1"}))
        .await?;
    for verb in ["take", "term"] {
        got.push(call(&l, &l.own, verb, "i2").await);
    }
    let (applied, unmet2) = (Ok("applied"), |v| unmet(v, "i2"));
    let want = [
        unmet2("take"),
        applied.clone(),
        unmet2("term"),
        applied.clone(),
        unmet2("free"),
        applied.clone(),
        applied.clone(),
        applied.clone(),
    ];
    ensure!(
        got == want,
        "the calls must answer {want:?} as the far record stands at each; they answered {got:?}"
    );
    // What a call wrote is the record's: the customer's rate changing after
    // it changes nothing on the invoice.
    l.upsert("customer", "c2", json!({"rate": 9})).await?;
    let i2 = l.read("invoice", "i2").await?.fields;
    let (copy, status) = (i2.get("copy"), i2.get("status"));
    ensure!(
        copy == Some(&json!(4)) && status == Some(&json!("termed")),
        "`i2` holds twice its customer's rate at the call and `termed`; it holds {i2:?}"
    );

    // The fold reads no customer: the log folded again gives the record.
    store.rebuild().await.must("rebuilding the projection")?;
    let again = l.read("invoice", "i2").await?.fields;
    ensure!(
        again == i2,
        "a rebuild gives `i2` as the call left it, {i2:?}; it gives {again:?}"
    );

    // What can never be read is refused where it is declared.
    let declared = |name: &str, requires: &str| {
        l.cat.declare(
            &l.own,
            serde_json::from_value(verb(name, "low", Some(requires), ("status", "'x'")))
                .expect("a verb"),
        )
    };
    for (requires, case) in [
        (
            "customer.nope = 1",
            json!({"code": "unknown_field", "type": "customer", "field": "nope"}),
        ),
        (
            "customer.term.nope = 1",
            json!({"code": "unknown_field", "type": "term", "field": "nope"}),
        ),
        (
            "customer.name.first = 'a'",
            json!({"code": "steps_into_scalar", "field": "name"}),
        ),
    ] {
        let what = format!("a verb that requires `{requires}`");
        refused_like(&what, declared("bad", requires).await, &case)?;
    }
    refused_as!(
        "`changed` of a path",
        declared("bad", "changed(customer.name)").await,
        Case::CallTakesField { .. }
    )?;
    // A field computed at the read is not stored: none is read across.
    let late = json!({"declare": "type", "name": "clock", "fields": [
        {"name": "due", "type": {"kind": "number"}},
        {"name": "late", "type": {"kind": "boolean"}, "authority": "derived",
         "derived": "due < 5", "computed_at": "read"}]});
    l.declare_json(late).await?;
    l.declare(type_decl("memo", vec![reference("clock", "clock")]))
        .await?;
    let on_memo = json!({"declare": "verb", "name": "bad", "on": "memo", "risk": "low",
        "requires": "clock.late", "effect": "write", "set": [{"field": "clock", "value": "'c'"}]});
    let on_memo = serde_json::from_value(on_memo).expect("a verb");
    let at_read = json!({"code": "reads_at_read", "field": "late", "type": "clock"});
    let refused = l.cat.declare(&l.own, on_memo).await;
    refused_like(
        "a verb that reads across a field computed at the read",
        refused,
        &at_read,
    )?;
    let mut soon = customer(true, "discount_days > 5");
    for f in soon.iter_mut().filter(|f| f.name.as_str() == "soon") {
        f.computed_at = Some(lakelands_api::ontology::ComputedAt::Read);
    }
    refused_as!(
        "taking to the read a field a verb reads across",
        l.try_declare(type_decl("customer", soon)).await,
        Case::ReadAsStored
    )?;
    // A migration keeps what a verb reads across.
    let taken = migration(type_decl("customer", customer(false, "discount_days > 5")));
    let kept = json!({"code": "migration_takes_read", "field": "rate", "type": "customer",
                      "at": [{"code": "verb", "name": "rate"}]});
    let refused = l.cat.migrate(&l.own, taken).await;
    refused_like(
        "a migration that takes a field a verb reads across",
        refused,
        &kept,
    )?;
    // The preflight judges every verb on the type it migrates again: a
    // reference retargeted under a path, and a kind changed under a
    // verb's own condition, are refused naming the verb to an author shown
    // it and as a declaration that could no longer be declared to another.
    let (text, number) = (StorageKind::Text, StorageKind::Number);
    let invoice = |days: StorageKind, to: &str| {
        let fields = vec![
            field("status", text),
            field("days", days),
            field("copy", number),
        ];
        type_decl(
            "invoice",
            [fields, vec![reference("customer", to)]].concat(),
        )
    };
    let blind = holder(
        store.as_ref(),
        "blind",
        json!({"default": "write", "schema": true,
               "types": {"customer": {"fields": {"discount_days": "none"}}}}),
    );
    let gone = json!({"code": "no_longer_declarable", "type": "invoice"});
    let sides = json!({"code": "kind_sides", "at": [{"code": "after_migration"},
                                                   {"code": "verb", "name": "bump"}]});
    for (what, to, far, to_blind) in [
        (
            "a reference retargeted under a verb's path",
            invoice(number, "term"),
            json!({"code": "unknown_field", "type": "term", "field": "discount_days"}),
            &gone,
        ),
        (
            "a kind changed under a verb's own condition",
            invoice(StorageKind::Boolean, "customer"),
            sides.clone(),
            &sides,
        ),
    ] {
        let refused = l.cat.migrate(&l.own, migration(to.clone())).await;
        refused_like(what, refused, &far)?;
        let refused = l.cat.migrate(&blind, migration(to)).await;
        refused_like(&format!("{what}, to `blind`"), refused, to_blind)?;
    }
    // And every verb that reads across into the type it migrates: `term`
    // on the invoice reads `customer.term.days`.
    let nodays = holder(
        store.as_ref(),
        "nodays",
        json!({"default": "write", "schema": true,
               "types": {"term": {"fields": {"days": "none"}}}}),
    );
    for (what, term, far) in [
        (
            "a far type's reference retargeted under a verb's path",
            reference("term", "customer"),
            json!({"code": "unknown_field", "type": "customer", "field": "days",
                   "at": [{"code": "after_migration"}, {"code": "verb", "name": "term"}]}),
        ),
        (
            "a far type's reference made text under a verb's path",
            field("term", text),
            json!({"code": "steps_into_scalar", "field": "term"}),
        ),
    ] {
        let mut moved = customer(true, "discount_days > 5");
        for f in moved.iter_mut().filter(|f| f.name.as_str() == "term") {
            *f = term.clone();
        }
        let refused = l
            .cat
            .migrate(&l.own, migration(type_decl("customer", moved.clone())));
        refused_like(what, refused.await, &far)?;
        // To one kept from `term.days` the verb `term` is not shown.
        let refused = l
            .cat
            .migrate(&nodays, migration(type_decl("customer", moved)));
        let gone = json!({"code": "no_longer_declarable", "type": "customer"});
        refused_like(&format!("{what}, to `nodays`"), refused.await, &gone)?;
    }

    // A path takes at most the lake's `max_depth` steps.
    let max = limits().max_depth;
    for (name, steps) in [("deep", max), ("deeper", max + 1)] {
        let far = ".parent".repeat(steps as usize - 1);
        let requires = format!("customer{far}.discount_days is null");
        let v = verb(name, "low", Some(&requires), ("copy", "7"));
        l.declare_json(v).await?;
    }
    let deep = call(&l, &l.own, "deep", "i1").await;
    let deeper = call(&l, &l.own, "deeper", "i1").await;
    ensure!(
        deep == Ok("applied"),
        "a path of `max_depth` steps is read, null where a reference on the way is; {deep:?}"
    );
    refused_as!(
        "a call whose path takes more steps than `max_depth`",
        deeper,
        Case::StepTooDeep { max: m, asked } if *m == max && *asked == max + 1
    )?;
    let steps = json!({"code": "steps_into_reference", "field": "customer"});
    let derives = type_decl(
        "derives",
        vec![
            reference("customer", "customer"),
            derived("name", StorageKind::Text, "customer.name", &["customer"]),
        ],
    );
    refused_like(
        "a derived field that steps across",
        l.try_declare(derives).await,
        &steps,
    )?;
    Ok(())
}
