//! A grant may admit every record to read and only some to write
//! (`docs/decisions/006-grants-not-roles.md`, lakelands#1490).
//!
//! `writes_only` on a type leaves its holder's reads as its other grants
//! give them and admits to its writes only the records whose fields hold
//! its values: the record at the key, live or retired, and the record the
//! write leaves. A link is its holder's to make by one end its pairs
//! hold, by both where the link reaches the other end's own answers; an
//! end of a type carrying no `writes_only` is held by none.

use lakelands_api::write::{Change, ChangeTo};
use lakelands_api::{RelationName, VerbName};

use super::change::line;
use super::only::{de, holder, not_granted, said};
use crate::conformance::fixture::{ingest, source_record};
use crate::conformance::prelude::*;

check!(
    WRITES_ONLY_DOORS = a_holder_of_writes_only_writes_its_records_alone,
    "006 (lakelands#1490): a holder of `writes_only` on a type reads every record it would read without it, and changes, makes, retires, makes again, calls a verb on and ingests only records whose fields hold its values, before and after the write; another's record, live or retired, a handover and a create that does not hold the pairs are refused `not_granted`, naming no field, appending nothing; a pair a create leaves through a field's `initial` or through an `authority: writer` field naming its own writer counts; a grant on a field of another kind, one the type does not declare, or with a value that is not text is refused at the doors that write the type as `writes_only_unappliable`, naming the type, and changes no read"
);

check!(
    WRITES_ONLY_LINKS = a_link_is_made_by_one_held_end,
    "006 (lakelands#1490): a holder of `writes_only` on a link's end types links a record its pairs hold to one they do not, and not two they do not; an end of a type carrying no `writes_only` is held by none, so a holder of it on the link type alone, or on one end and not the other, links nothing by that end and is offered no relation where no end can be held; where the relation is to-one at the end it may not write, both ends must be its own; a link record must hold the pairs its `writes_only` on the link type names, at its create and at an `unlink`; with `writes_only` on an end type and none on the link type every link of that relation is refused; each refused `not_granted`, appending nothing"
);

check!(
    WRITES_ONLY_REVIEWED = a_change_or_a_proposal_is_judged_as_the_write_is,
    "006 (lakelands#1490): a holder of `writes_only` plans a change to records and is told each record it may not write as refused `not_granted`, and `enact` refuses the whole change, appending nothing; its proposal of a write to another's record or of a verb on one is refused, its proposal on its own is pinned, accepted where nothing moved and refused `write_changed_since_proposed` once the record moved; a reviewer holding `writes_only` accepts and rejects only a proposal whose record and whose write its pairs admit; with `schema` it proposes a declaration and a migration while `declare`, `migrate` and an aging rule stay refused"
);

check!(
    WRITES_ONLY_DESCRIBED = describe_tells_writes_only_and_offers_no_refused_write,
    "006 (lakelands#1490): `describe_type` tells a holder its `writes_only`, offers `upsert` only where a create can hold its pairs (an `authority: writer` field whose pair is the holder's own writer included), a relation only where the link type's grant can be held, and no verb that assigns a field its pairs name; a grant on both `only` and `writes_only` reads what `only` admits and writes only what both admit; the proposal listing shows a reviewer whose `only` names an `authority: writer` field a create proposed by that writer with its write, and one proposed by another without it"
);

/// `job` (`team`, `state`, `n`; `for_site` and `at_site`, many to many
/// and many to one onto `site`, each link holding `team`), `site`
/// (`zone`), and the verbs `touch` (low, sets `n`), `rehome` (low, sets
/// `team`) and `ask` (high, sets `n`); jobs j1 (a, open), j2 (b, open), j3
/// (a, shut), sites s1 (x) and s2 (y).
async fn jobs(l: &Lake) -> Held {
    let zone = json!({"name": "zone", "type": {"kind": "text"}, "authority": "shared"});
    l.declare_json(json!({"declare": "type", "name": "site", "fields": [zone]}))
        .await?;
    l.declare_json(job()).await?;
    for (name, risk, field, value) in [
        ("touch", "low", "n", "1"),
        ("rehome", "low", "team", "'b'"),
        ("ask", "high", "n", "2"),
    ] {
        let set = json!([{"field": field, "value": value}]);
        l.declare_json(
            json!({"declare": "verb", "name": name, "on": "job", "risk": risk,
                              "effect": "write", "set": set}),
        )
        .await?;
    }
    for (key, zone) in [("s1", "x"), ("s2", "y")] {
        l.upsert("site", key, json!({"zone": zone})).await?;
    }
    for (key, team, state) in [
        ("j1", "a", "open"),
        ("j2", "b", "open"),
        ("j3", "a", "shut"),
    ] {
        l.upsert("job", key, json!({"team": team, "state": state}))
            .await?;
    }
    Ok(())
}

/// `job` as [`jobs`] declares it.
fn job() -> Value {
    let text = |name: &str| json!({"name": name, "type": {"kind": "text"}, "authority": "shared"});
    let n = json!({"name": "n", "type": {"kind": "number"}, "authority": "shared"});
    let team = json!([{"name": "team", "type": {"kind": "text"}}]);
    json!({"declare": "type", "name": "job",
        "fields": [text("team"), text("state"), n],
        "relations": [
            {"name": "for_site", "target": "site", "cardinality": "many_to_many", "fields": team},
            {"name": "at_site", "target": "site", "cardinality": "many_to_one", "fields": team}]})
}

/// The link types of `job`'s `for_site` and `at_site`.
fn link_types() -> (TypeName, TypeName) {
    let job = TypeName::from("job");
    let lt = |r: &str| link_type_name(&job, &r.into());
    (lt("for_site"), lt("at_site"))
}

/// `wes`: writes everything, may review and ingest, and holds
/// `writes_only` team a on `job` and on both its link types, zone x on
/// `site`, and `more` beside, its `types` added to hers.
fn wes(store: &dyn LakeStore, more: Value) -> Principal {
    let (for_site, at_site) = link_types();
    let team = json!({"writes_only": {"team": "a"}});
    let mut types = json!({"job": team, "site": {"writes_only": {"zone": "x"}},
                           for_site.as_str(): team, at_site.as_str(): team});
    let mut grants = json!({"default": "write", "review": true, "ingest": true});
    for (k, v) in more.as_object().into_iter().flatten() {
        match v.as_object().filter(|_| k == "types") {
            Some(added) => (added.iter()).for_each(|(t, g)| types[t] = g.clone()),
            None => grants[k] = v.clone(),
        }
    }
    grants["types"] = types;
    holder(store, "wes", grants)
}

/// Fails unless each row's answer is what it wants.
fn rows_hold(what: &str, rows: &[(String, String, String)]) -> Held {
    let wrong: Vec<_> = rows.iter().filter(|(_, said, want)| said != want).collect();
    ensure!(wrong.is_empty(), "{what}: {wrong:?}");
    Ok(())
}

/// The keys of `ty` `who`'s `find` answers, in order.
async fn keys(l: &Lake, who: &Principal, ty: &str) -> Result<String, Failure> {
    let found = all_found(l.cat.as_ref(), who, ty, Find::default()).await?;
    let mut keys: Vec<String> = found
        .into_iter()
        .map(|r| r.key.as_str().to_owned())
        .collect();
    keys.sort();
    Ok(keys.join(" "))
}

async fn a_holder_of_writes_only_writes_its_records_alone(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let cat = l.cat.as_ref();
    jobs(&l).await?;
    let team = json!({"name": "team", "type": {"kind": "text"}, "initial": "a"});
    let body = json!({"name": "body", "type": {"kind": "text"}});
    l.declare_json(json!({"declare": "type", "name": "task", "fields": [team, body]}))
        .await?;
    let by = json!({"name": "by", "type": {"kind": "text"}, "authority": "writer"});
    l.declare_json(json!({"declare": "type", "name": "memo", "fields": [body, by]}))
        .await?;
    l.retire("job", "j3").await?;
    l.upsert("job", "j4", json!({"team": "b"})).await?;
    l.retire("job", "j4").await?;
    let more = json!({"types": {"task": {"writes_only": {"team": "a"}},
                                "memo": {"writes_only": {"by": "agent:wes"}}}});
    let wes = wes(s, more);
    let reads = keys(&l, &wes, "job").await?;
    ensure!(
        reads == "j1 j2",
        "`wes` reads every live job, its `writes_only` aside; it reads {reads}"
    );
    let (ng, ok) = (said(not_granted_error()), "applied".to_owned());
    let mut rows = Vec::new();
    let mut row = |what: &str, said: String, want: &String| {
        rows.push((what.to_owned(), said, want.clone()));
    };
    let up = |ty: &'static str, key: &'static str, fields: Value| {
        let (ty, key, wes) = (TypeName::from(ty), RecordKey::from(key), &wes);
        async move {
            said(
                cat.upsert(wes, &ty, &key, de(fields), WriteOpts::default())
                    .await,
            )
        }
    };
    let head = s.head().await.must("the head")?;
    row(
        "another's j2 changed",
        up("job", "j2", json!({"n": 1})).await,
        &ng,
    );
    row(
        "j1 handed to b",
        up("job", "j1", json!({"team": "b"})).await,
        &ng,
    );
    row(
        "a create without the pair",
        up("job", "j8", json!({"n": 1})).await,
        &ng,
    );
    row(
        "a create of b's",
        up("job", "j8", json!({"team": "b"})).await,
        &ng,
    );
    row(
        "another's retired j4 made again",
        up("job", "j4", json!({"team": "a"})).await,
        &ng,
    );
    let (job, opts) = (TypeName::from("job"), WriteOpts::default);
    let retired = cat.retire(&wes, &job, &"j2".into(), opts()).await;
    row("`retire` of another's j2", said(retired), &ng);
    let (touch, j1, j2) = (
        VerbName::from("touch"),
        RecordKey::from("j1"),
        RecordKey::from("j2"),
    );
    let called = cat
        .call(&wes, &touch, Some(&j2), patch(json!({})), opts())
        .await;
    row("`touch` on another's j2", said(called), &ng);
    let fed = |key: &str, team: &str| {
        let r = source_record("job", key, json!({"team": team}));
        let fed = ingest(cat, &wes, "feed", vec![r]);
        async move { said(fed.await) }
    };
    row("an ingest onto another's j2", fed("j2", "a").await, &ng);
    row("an ingest of b's", fed("j7", "b").await, &ng);
    let ann = json!({"default": "write", "types": {"memo": {"writes_only": {"by": "agent:wes"}}}});
    let (ann, memo, m2) = (holder(s, "ann", ann), TypeName::from("memo"), "m2".into());
    let made = cat
        .upsert(&ann, &memo, &m2, de(json!({"body": "x"})), opts())
        .await;
    row("a memo another writer would make", said(made), &ng);
    rows_hold("every write `wes`'s pairs do not admit is refused", &rows)?;
    head_unmoved(s, head, "every refused write by `wes`").await?;
    let mut rows = Vec::new();
    let mut row = |what: &str, said: String, want: &String| {
        rows.push((what.to_owned(), said, want.clone()));
    };
    row(
        "her j1 changed",
        up("job", "j1", json!({"n": 1})).await,
        &ok,
    );
    row(
        "a create of hers",
        up("job", "j9", json!({"team": "a"})).await,
        &ok,
    );
    row(
        "her retired j3 made again",
        up("job", "j3", json!({"team": "a"})).await,
        &ok,
    );
    let retired = cat.retire(&wes, &job, &"j9".into(), opts()).await;
    row("`retire` of her j9", said(retired), &ok);
    row(
        "j9 made again",
        up("job", "j9", json!({"team": "a"})).await,
        &ok,
    );
    let called = cat
        .call(&wes, &touch, Some(&j1), patch(json!({})), opts())
        .await;
    row("`touch` on her j1", said(called), &ok);
    row("an ingest of hers", fed("j7", "a").await, &ok);
    row(
        "a task its `initial` holds",
        up("task", "t1", json!({"body": "x"})).await,
        &ok,
    );
    row(
        "a memo its writer holds",
        up("memo", "m1", json!({"body": "x"})).await,
        &ok,
    );
    rows_hold("every write `wes`'s pairs admit is applied", &rows)?;
    // A grant that cannot be applied refuses the writes, not the reads.
    let mut wrong = Vec::new();
    for grant in [json!({"n": "1"}), json!({"payer": "a"}), json!({"team": 1})] {
        let types = json!({"job": {"writes_only": grant}});
        let eve = holder(s, "eve", json!({"default": "write", "types": types}));
        let upserted = cat
            .upsert(&eve, &job, &j1, de(json!({"n": 2})), opts())
            .await;
        let retired = cat.retire(&eve, &job, &j1, opts()).await;
        let refused = |e: &lakelands_api::Result<Outcome>| {
            matches!(e.as_ref().err().and_then(Error::case),
                Some(Said { case: Case::WritesOnlyUnappliable { ty }, .. }) if ty == "job")
        };
        let reads = keys(&l, &eve, "job").await?;
        if !refused(&upserted) || !refused(&retired) || reads != "j1 j2 j3 j7 j9" {
            wrong.push(format!("{grant}: {upserted:?}, {retired:?}, reads {reads}"));
        }
    }
    ensure!(
        wrong.is_empty(),
        "a `writes_only` that cannot be applied refuses `upsert` and `retire` as \
         `writes_only_unappliable` naming `job`, and leaves the reads: {wrong:?}"
    );
    Ok(())
}

/// The refusal of a write not granted, naming no field.
fn not_granted_error() -> lakelands_api::Result<()> {
    Err(Error::Refused {
        field: None,
        reason: Refusal::NotGranted,
    })
}

async fn a_link_is_made_by_one_held_end(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let cat = l.cat.as_ref();
    jobs(&l).await?;
    let job = TypeName::from("job");
    let k = |key: &str| RecordKey::from(key);
    let (for_site, at_site) = (
        RelationName::from("for_site"),
        RelationName::from("at_site"),
    );
    let (j3, s1) = (k("j3"), k("s1"));
    let opts = WriteOpts::default;
    let owner = cat.link(
        &l.own,
        &job,
        &j3,
        &for_site,
        &s1,
        de(json!({"team": "b"})),
        opts(),
    );
    owner.await.must("the owner's link of j3 to s1, team b")?;
    let wes = wes(s, json!({}));
    let types = json!({"job": {"writes_only": {"team": "a"}}});
    let vic = holder(s, "vic", json!({"default": "write", "types": types}));
    // `pia` holds `writes_only` on `job` and its link types, none on
    // `site`; `lin` on the link types alone.
    let (fs, ats) = link_types();
    let team = json!({"writes_only": {"team": "a"}});
    let types = json!({"job": team, "site": {"default": "read"},
                       fs.as_str(): team, ats.as_str(): team});
    let pia = holder(s, "pia", json!({"default": "write", "types": types}));
    let types = json!({fs.as_str(): team, ats.as_str(): team});
    let lin = holder(s, "lin", json!({"default": "write", "types": types}));
    let link = async |who: &Principal, from: &str, rel: &RelationName, to: &str, fields: Value| {
        let (from, to) = (k(from), k(to));
        said(
            cat.link(who, &job, &from, rel, &to, de(fields), opts())
                .await,
        )
    };
    let unlink = async |from: &str| {
        let from = k(from);
        let to = if from.as_str() == "j3" {
            k("s1")
        } else {
            k("s2")
        };
        said(cat.unlink(&wes, &job, &from, &for_site, &to, opts()).await)
    };
    let (ng, ok) = (said(not_granted_error()), "applied".to_owned());
    let a = || json!({"team": "a"});
    let head = s.head().await.must("the head")?;
    let refused = [
        (
            "neither end hers",
            link(&wes, "j2", &for_site, "s2", a()).await,
        ),
        (
            "to-one at another's j2",
            link(&wes, "j2", &at_site, "s1", a()).await,
        ),
        (
            "a link of team b",
            link(&wes, "j1", &for_site, "s1", json!({"team": "b"})).await,
        ),
        (
            "a link without its team",
            link(&wes, "j1", &for_site, "s1", json!({})).await,
        ),
        (
            "no grant on the link type",
            link(&vic, "j1", &for_site, "s1", a()).await,
        ),
        ("an `unlink` of team b's", unlink("j3").await),
        (
            "a write to team b's link of j3 to s1",
            link(&wes, "j3", &for_site, "s1", a()).await,
        ),
        (
            "pia's j2 to s2, a site of no `writes_only`",
            link(&pia, "j2", &for_site, "s2", a()).await,
        ),
        (
            "lin's j1 to s1, no end held",
            link(&lin, "j1", &for_site, "s1", a()).await,
        ),
    ];
    let wrong: Vec<_> = refused.iter().filter(|(_, said)| *said != ng).collect();
    ensure!(
        wrong.is_empty(),
        "every link its holder may not make or unmake is refused `not_granted`: {wrong:?}"
    );
    head_unmoved(s, head, "every refused link by `wes`").await?;
    let applied = [
        (
            "her j1 to another's s2",
            link(&wes, "j1", &for_site, "s2", a()).await,
        ),
        (
            "a write to her link of j1 to s2",
            link(&wes, "j1", &for_site, "s2", json!({"team": "a"})).await,
        ),
        (
            "another's j2 to her s1",
            link(&wes, "j2", &for_site, "s1", a()).await,
        ),
        (
            "her j1, to-one, at another's s2",
            link(&wes, "j1", &at_site, "s2", a()).await,
        ),
        ("an `unlink` of her own", unlink("j1").await),
        (
            "pia's j1 to s1, a site of no `writes_only`",
            link(&pia, "j1", &for_site, "s1", a()).await,
        ),
    ];
    let wrong: Vec<_> = applied.iter().filter(|(_, said)| *said != ok).collect();
    ensure!(
        wrong.is_empty(),
        "a link with one end `wes` may write, its record holding team a, is hers: {wrong:?}"
    );
    let told = describe_type(cat, &lin, "job").await?;
    ensure!(
        told.may.link.is_empty(),
        "`describe_type` offers lin, who holds no end, no relation: {:?}",
        told.may
    );
    Ok(())
}

async fn a_change_or_a_proposal_is_judged_as_the_write_is(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let cat = l.cat.as_ref();
    jobs(&l).await?;
    let wes = wes(s, json!({"schema": true}));
    let change = |filter: &str, to: Value| Change::Records {
        ty: "job".into(),
        filter: Box::new(filter.into()),
        to: ChangeTo::Set(de(to)),
        only: BTreeMap::new(),
        skip_restricted: false,
    };
    let head = s.head().await.must("the head")?;
    let mixed = change("state = 'open'", json!({"n": 3}));
    let planned = cat
        .plan(&wes, mixed.clone())
        .await
        .must("`wes`'s plan")?
        .data;
    let told = line(&planned);
    ensure!(
        told.ends_with("refused NotGranted 1 [j2]"),
        "the plan tells j2 refused `not_granted`: {told}"
    );
    let enacted = cat.enact(&wes, mixed, &planned.report_id).await;
    not_granted("`enact` of a change one record refuses", enacted)?;
    head_unmoved(s, head, "a refused `enact`").await?;
    let hers = change("team = 'a'", json!({"n": 3}));
    let planned = cat
        .plan(&wes, hers.clone())
        .await
        .must("`wes`'s plan")?
        .data;
    let enacted = cat.enact(&wes, hers, &planned.report_id).await;
    enacted.must("`enact` of a change to her own")?;
    let off = change("team = 'a'", json!({"team": "b"}));
    let planned = cat.plan(&wes, off.clone()).await.must("`wes`'s plan")?.data;
    let told = line(&planned);
    ensure!(
        told.ends_with("refused NotGranted 2 [j1,j3]"),
        "the plan tells her records moved off her pairs refused `not_granted`: {told}"
    );
    let head = s.head().await.must("the head")?;
    let enacted = cat.enact(&wes, off, &planned.report_id).await;
    not_granted("`enact` of a change off her pairs", enacted)?;
    head_unmoved(s, head, "a refused `enact` off her pairs").await?;
    // Her proposals.
    let proposed = async |who: &Principal, key: &str, n: u32| {
        let write = json!({"write": "upsert", "type": "job", "key": key, "patch": {"n": n}});
        cat.propose(who, de(write), "a change".into()).await
    };
    let id = |o: lakelands_api::Result<Outcome>| match o {
        Ok(Outcome::Proposed { proposal, .. }) => Ok(proposal),
        other => Err(Failure::new(format!("a write must be proposed: {other:?}"))),
    };
    let head = s.head().await.must("the head")?;
    not_granted("a proposal on another's j2", proposed(&wes, "j2", 5).await)?;
    let (ask, opts) = (VerbName::from("ask"), WriteOpts::default);
    let j2 = RecordKey::from("j2");
    let asked = cat
        .call(&wes, &ask, Some(&j2), patch(json!({})), opts())
        .await;
    not_granted("`ask` on another's j2", asked)?;
    head_unmoved(s, head, "her refused proposals").await?;
    let kept = id(proposed(&wes, "j1", 5).await)?;
    let moved = id(proposed(&wes, "j3", 5).await)?;
    l.upsert("job", "j3", json!({"n": 8})).await?;
    cat.accept(&l.own, &kept).await.must("accepting her j1")?;
    let accepted = cat.accept(&l.own, &moved).await;
    ensure!(
        matches!(
            invalid_case(&accepted),
            Some(Case::WriteChangedSinceProposed { .. })
        ),
        "her pinned proposal on j3, moved since, is refused `write_changed_since_proposed`: \
         {accepted:?}"
    );
    // Her change to records holds her pairs beside it: its accept writes
    // hers alone. One whose `only` contradicts them is refused.
    let records = |filter: &str| ProposedWrite::Records {
        ty: "job".into(),
        filter: filter.into(),
        to: ChangeTo::Set(de(json!({"n": 4}))),
        only: BTreeMap::new(),
        skip_restricted: false,
        plan: Box::default(),
    };
    let filed = id(cat
        .propose(&wes, records("state = 'shut'"), "her shut".into())
        .await)?;
    l.upsert("job", "j4", json!({"team": "b", "state": "shut"}))
        .await?;
    let listed = cat.proposals(&l.own, ProposalFilter::default()).await;
    let listed = listed.must("the owner's listing")?.data;
    let found = listed.iter().find(|p| p.id == filed);
    let Some(change) = found.and_then(|p| p.write.as_ref()?.change()) else {
        return Err(Failure::new("the owner must be listed her change"));
    };
    let planned = cat.plan(&l.own, change).await.must("the owner's plan")?;
    let head = s.head().await.must("the head")?;
    let accepted = cat.accept_planned(&l.own, &filed, &planned.data.report_id);
    accepted.await.must("the owner's accept of her change")?;
    let events = events_after(s, head).await?;
    let wrote: Vec<_> = super::change::writes(&events)
        .into_iter()
        .map(|w| w.0)
        .collect();
    ensure!(
        wrote == ["j3"],
        "the accept of her change writes her shut j3 alone, never b's j4: {wrote:?}"
    );
    let both = json!({"job": {"only": {"team": "b"}, "writes_only": {"team": "a"}}});
    let cam = holder(s, "cam", json!({"default": "write", "types": both}));
    let none = records("state = 'gone'");
    let head = s.head().await.must("the head")?;
    let contradicted = cat.propose(&cam, none, "none".into()).await;
    not_granted(
        "a change whose `only` contradicts `writes_only`",
        contradicted,
    )?;
    head_unmoved(s, head, "a refused change by cam").await?;
    // A reviewer of `writes_only` decides what its pairs admit.
    let bob = holder(s, "bob", json!({"default": "write"}));
    let (mine, others) = (
        id(proposed(&bob, "j1", 9).await)?,
        id(proposed(&bob, "j2", 9).await)?,
    );
    let make = json!({"write": "upsert", "type": "job", "key": "j6", "patch": {"team": "b"}});
    let made = id(cat.propose(&bob, de(make), "b's".into()).await)?;
    let rae = holder(
        s,
        "rae",
        json!({"default": "write", "review": true, "types": {"job": {"writes_only": {"team": "a"}}}}),
    );
    let head = s.head().await.must("the head")?;
    not_granted("rae's accept on j2", cat.accept(&rae, &others).await)?;
    let rejected = cat.reject(&rae, &others, "no".into()).await;
    not_granted("rae's reject on j2", rejected)?;
    not_granted("rae's accept of b's j6", cat.accept(&rae, &made).await)?;
    head_unmoved(s, head, "rae's refused decisions").await?;
    cat.accept(&rae, &mine).await.must("rae's accept on j1")?;
    let again = id(proposed(&bob, "j1", 10).await)?;
    let rejected = cat.reject(&rae, &again, "no".into()).await;
    rejected.must("rae's reject on j1")?;
    // With `schema` she proposes a shape and changes none.
    let other = json!({"declare": "type", "name": "other",
                       "fields": [{"name": "n", "type": {"kind": "text"}}]});
    let declared = json!({"write": "declare", "declarations": [other]});
    let declared = cat.propose(&wes, de(declared), "a type".into()).await;
    id(declared)?;
    let mut wider: TypeDecl = de(job()
        .as_object()
        .cloned()
        .map(|mut j| {
            j.remove("declare");
            Value::Object(j)
        })
        .unwrap_or_default());
    wider.fields.push(field("extra", StorageKind::Text));
    let migrated = json!({"write": "migrate", "migration": {"to": wider}});
    id(cat.propose(&wes, de(migrated), "a field".into()).await)?;
    let head = s.head().await.must("the head")?;
    let applied = cat.apply(&wes, vec![de(other.clone())]).await;
    not_granted("`apply`", applied)?;
    not_granted("`declare`", cat.declare(&wes, de(other)).await)?;
    let m = crate::conformance::fixture::migration(wider);
    not_granted("`migrate`", cat.migrate(&wes, m).await)?;
    let rule = json!({"on": "job", "field": "state", "from": "open", "to": "shut", "when": "true"});
    let rules = TypeName::from(lakelands_api::seam::AGING_RULE_TYPE);
    let ruled = cat
        .upsert(&wes, &rules, &"r1".into(), de(rule), opts())
        .await;
    not_granted("an aging rule on `job`", ruled)?;
    head_unmoved(s, head, "her refused schema writes").await
}

async fn describe_tells_writes_only_and_offers_no_refused_write(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let cat = l.cat.as_ref();
    jobs(&l).await?;
    let body = json!({"name": "body", "type": {"kind": "text"}});
    let by = json!({"name": "by", "type": {"kind": "text"}, "authority": "writer"});
    l.declare_json(json!({"declare": "type", "name": "memo", "fields": [body, by]}))
        .await?;
    // What `describe_type` tells and offers her.
    let read_team =
        json!({"types": {"job": {"writes_only": {"team": "a"}, "fields": {"team": "read"}}}});
    let types = json!({"job": {"writes_only": {"team": "a"}}});
    let vic = holder(s, "vic", json!({"default": "write", "types": types}));
    let mut wrong = Vec::new();
    for (who, creates, links) in [
        (wes(s, json!({})), true, "at_site for_site"),
        (wes(s, read_team), false, "at_site for_site"),
        (vic, true, ""),
    ] {
        let told = describe_type(cat, &who, "job").await?;
        let may = &told.may;
        let pairs = serde_json::to_value(&may.writes_only).expect("a map reads");
        let mut link: Vec<&str> = may.link.iter().map(|r| r.as_str()).collect();
        link.sort();
        let verbs: Vec<&str> = told.verbs.iter().map(|v| v.as_str()).collect();
        if pairs != json!({"team": "a"})
            || may.upsert != creates
            || link.join(" ") != links
            || verbs != ["ask", "touch"]
        {
            wrong.push(format!("{}: {may:?}, verbs {verbs:?}", who.id));
        }
    }
    let memo = |id: &str, by: &str| {
        let types = json!({"memo": {"writes_only": {"by": by}}});
        holder(s, id, json!({"default": "write", "types": types}))
    };
    for (who, creates) in [
        (memo("wes", "agent:wes"), true),
        (memo("ann", "agent:wes"), false),
    ] {
        let told = describe_type(cat, &who, "memo").await?;
        if told.may.upsert != creates {
            wrong.push(format!("{} on memo: {:?}", who.id, told.may));
        }
    }
    let types = json!({"job": {"writes_only": {"n": "1"}}});
    let una = holder(s, "una", json!({"default": "write", "types": types}));
    let told = describe_type(cat, &una, "job").await?;
    if told.may.upsert || told.may.retire || !told.may.link.is_empty() {
        wrong.push(format!("una, her grant unappliable: {:?}", told.may));
    }
    ensure!(
        wrong.is_empty(),
        "`describe_type` tells `writes_only`, offers an upsert where a create holds the pairs, \
         a relation where its link type's grant holds, and no verb that moves a pair: {wrong:?}"
    );
    // With `only`: she reads what `only` admits, writes what both admit.
    let both = json!({"job": {"only": {"team": "a"}, "writes_only": {"state": "open"}}});
    let oli = holder(s, "oli", json!({"default": "write", "types": both}));
    let reads = keys(&l, &oli, "job").await?;
    let (job, opts) = (TypeName::from("job"), WriteOpts::default);
    let up = |key: &'static str, fields: Value| {
        let (key, oli, job) = (RecordKey::from(key), &oli, &job);
        async move { said(cat.upsert(oli, job, &key, de(fields), opts()).await) }
    };
    let (ng, ok) = (said(not_granted_error()), "applied".to_owned());
    let rows = [
        ("her open j1", up("j1", json!({"n": 1})).await, &ok),
        ("her shut j3", up("j3", json!({"n": 1})).await, &ng),
        ("her j1 shut", up("j1", json!({"state": "shut"})).await, &ng),
        ("another's open j2", up("j2", json!({"n": 1})).await, &ng),
        (
            "a create of both",
            up("j5", json!({"team": "a", "state": "open"})).await,
            &ok,
        ),
        (
            "a create without the state",
            up("j6", json!({"team": "a"})).await,
            &ng,
        ),
    ];
    let wrong: Vec<_> = rows
        .iter()
        .filter(|(_, said, want)| said != *want)
        .collect();
    ensure!(
        reads == "j1 j3" && wrong.is_empty(),
        "a holder of `only` and `writes_only` reads what `only` admits ({reads}) and writes \
         what both admit: {wrong:?}"
    );
    // A reviewer whose `only` names a writer field is shown a create its
    // writer proposed with its write, another's without.
    let ria = json!({"default": "read", "review": true,
                     "types": {"memo": {"only": {"by": "agent:bob"}}}});
    let ria = holder(s, "ria", ria);
    let mut ids = Vec::new();
    for (id, key) in [("bob", "m1"), ("cy", "m2")] {
        let who = holder(s, id, json!({"default": "write"}));
        let write = json!({"write": "upsert", "type": "memo", "key": key, "patch": {"body": "x"}});
        let Ok(Outcome::Proposed { proposal, .. }) =
            cat.propose(&who, de(write), "a memo".into()).await
        else {
            return Err(Failure::new(format!("`{id}`'s memo must be proposed")));
        };
        ids.push(proposal);
    }
    let listed = cat.proposals(&ria, ProposalFilter::default()).await;
    let listed = listed.must("ria's listing")?.data;
    let shown = |id: &ProposalId| {
        listed
            .iter()
            .find(|p| &p.id == id)
            .map(|p| p.write.is_some())
    };
    let (bobs, cys) = (shown(&ids[0]), shown(&ids[1]));
    ensure!(
        bobs == Some(true) && cys == Some(false),
        "bob's create of a memo is listed to ria with its write, cy's without: {bobs:?}, {cys:?}"
    );
    Ok(())
}
