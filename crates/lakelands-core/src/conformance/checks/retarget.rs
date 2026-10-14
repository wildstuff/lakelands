//! A reference given another target names a live record of it
//! (`docs/decisions/032-a-reference-names-a-live-record.md`, extended).

use lakelands_api::write::{Migration, Outcome, ProposedWrite, WriteOpts};
use lakelands_api::{Error, Principal, ProposalId, ReadAt, Refusal};
use serde_json::json;

use super::only::{de, holder, not_found};
use crate::conformance::fixture::{Lake, Must, migration};
use crate::conformance::{Failure, Held, Store};

check!(
    RETARGET_JUDGES_REFERENCES = a_retarget_converts_a_reference_only_onto_a_live_record,
    "032 and 027, extended (lakelands#1071): a migration that gives a reference field, or a relation and so its link type's `to`, another target converts a value only where it names a live record of the new target: `rf.r` holding a key of `trip` retargeted onto `tour`, and `pq.q` whose live link holds one, are lossy, naming the record that holds it, refused at their proposal, and applied once the new target holds the key; a preview made lossless whose target record is retired before its apply, or before the accept of its proposal, is refused there and changes nothing; a record of the new target withheld from the migrator is not live to it: lossy, refused at its apply and proposal, and a proposal made before the hold is refused at the accept of a reviewer who holds `restricted`, as it is judged as its proposer; a link whose end is withheld from it is not live to it either, as at its write door; a holder applies it, and its proposal, which reaches the held record, is listed to no reviewer without `restricted` (and to one while the target holds no hold), whose accept and reject are not found, and is accepted by one who holds it; to a reader kept from `trip` no answer of `find`, `get` or a report names its key at any point"
);

async fn a_retarget_converts_a_reference_only_onto_a_live_record(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let (cat, own) = (l.cat.as_ref(), &l.own);
    let name = json!({"name": "name", "type": {"kind": "text"}});
    let r = |to: &str| json!({"name": "r", "type": {"kind": "reference", "target": to}});
    let rf = |to: &str| json!({"name": "rf", "fields": [name, r(to)]});
    let pq = |to: &str| json!({"name": "pq", "fields": [name], "relations": [{"name": "q", "target": to}]});
    let plain = |n: &str| json!({"name": n, "fields": [name]});
    for t in [plain("trip"), plain("tour"), rf("trip"), pq("trip")] {
        l.declare(de(t)).await?;
    }
    for (ty, key, value) in [("trip", "t1", "t"), ("pq", "p1", "p")] {
        l.upsert(ty, key, json!({ "name": value })).await?;
    }
    l.upsert("rf", "a", json!({"r": "t1"})).await?;
    let (none, opts) = (Default::default(), WriteOpts::default());
    let (ty, from, relation, to) = ("pq".into(), "p1".into(), "q".into(), "t1".into());
    let linked = cat.link(own, &ty, &from, &relation, &to, none, opts).await;
    linked.must("linking p1 to t1")?;
    let grants = json!({"default": "read", "review": true, "schema": true,
                        "types": {"trip": {"default": "none"}}});
    let kept = holder(s, "kept", grants);
    // What `kept` is answered of `rf`, of `pq`'s links and of the owner's
    // proposals: never `t1`.
    let mut told = Vec::new();
    let (rf_, links) = ("rf".into(), "sys.link.pq.q".into());
    let reads = async |told: &mut Vec<String>, who: &Principal| {
        let found = cat.find(who, &rf_, Default::default()).await;
        let linked = cat.find(who, &links, Default::default()).await;
        let got = cat.get(who, &rf_, de(json!({"key": "a"}))).await;
        let listed = cat.proposals(who, Default::default()).await;
        told.push(format!("{found:?} {linked:?} {got:?} {listed:?}"));
    };
    reads(&mut told, &kept).await;
    let (to_rf, to_pq) = (migration(de(rf("tour"))), migration(de(pq("tour"))));
    let field = cat
        .migrate(own, to_rf.clone())
        .await
        .must("previewing `rf`")?
        .data;
    let link = cat
        .migrate(own, to_pq.clone())
        .await
        .must("previewing `pq`")?
        .data;
    let write = |m: &Migration| ProposedWrite::Migrate {
        migration: Box::new(m.clone()),
        report: Default::default(),
    };
    let proposed = cat.propose(own, write(&to_rf), "a".into()).await;
    let samples = &field.fields[0].sample_keys;
    ensure!(
        !field.lossless && !link.lossless && samples.len() == 1 && proposed.is_err(),
        "retargeted onto `tour`, which holds no `t1`, `rf.r` ({field:?}) and `pq.q` ({link:?}) are \
         lossy, naming `a`, and refused at their proposal ({proposed:?})"
    );
    // Lossless while `tour` holds `t1`; retired before the apply and the
    // accept, each is refused and the record is as it was.
    l.upsert("tour", "t1", json!({"name": "u"})).await?;
    let whole = cat
        .migrate(own, to_rf.clone())
        .await
        .must("previewing")?
        .data;
    let proposal = proposal_of(cat.propose(own, write(&to_rf), "b".into()).await)?;
    l.retire("tour", "t1").await?;
    let applied = cat
        .migrate_apply(own, to_rf.clone(), &whole.report_id)
        .await;
    let accepted = cat.accept(own, &proposal).await;
    reads(&mut told, &kept).await;
    let rf_now = cat.describe_type(own, &rf_, ReadAt::default()).await;
    let onto_trip = format!("{:?}", rf_now.must("describing `rf`")?.data.fields).contains("trip");
    ensure!(
        whole.lossless && applied.is_err() && accepted.is_err() && onto_trip,
        "a lossless retarget whose `t1` is retired since is refused at its apply ({applied:?}) \
         and its accept ({accepted:?}), `r` still onto `trip`"
    );
    // Restricted, `t1` is not live to `plain`, who lacks `restricted`:
    // lossy, refused at its apply and proposal, and its proposal of before
    // the hold refused at an accept as its own; a holder's is accepted.
    l.upsert("tour", "t1", json!({"name": "v"})).await?;
    let g = |r| json!({"default": "write", "review": true, "schema": true, "restricted": r});
    let (plain, keeper) = (holder(s, "plain", g(false)), holder(s, "keeper", g(true)));
    let early = proposal_of(cat.propose(&plain, write(&to_rf), "c".into()).await)?;
    // With no hold in `tour`, the owner's proposal is listed to `plain`.
    let listed = async |who| format!("{:?}", cat.proposals(who, Default::default()).await);
    let open = proposal_of(cat.propose(own, write(&to_rf), "g".into()).await)?;
    let seen = listed(&plain).await;
    ensure!(seen.contains(open.as_str()), "listed to `plain`: {seen}");
    let (tour, t1) = ("tour".into(), "t1".into());
    s.restrict(&tour, &t1, "a hold".into())
        .await
        .must("restricting")?;
    let seen = cat
        .migrate(&plain, to_rf.clone())
        .await
        .must("previewing")?;
    let (seen, m) = (seen.data, to_rf.clone());
    let applied = cat.migrate_apply(&plain, m, &seen.report_id).await;
    let proposed = cat.propose(&plain, write(&to_rf), "d".into()).await;
    let accepted = cat.accept(own, &early).await;
    ensure!(
        !seen.lossless && applied.is_err() && proposed.is_err() && accepted.is_err(),
        "onto a `t1` held from `plain`: {seen:?} {applied:?} {proposed:?} {accepted:?}"
    );
    // Each reaches `t1`: not listed to `plain`, not answered it, and not
    // found at its accept or reject; listed to `keeper`, who accepts `rf`'s.
    for (m, why) in [(&to_rf, "e"), (&to_pq, "f")] {
        let held = proposal_of(cat.propose(own, write(m), why.into()).await)?;
        let (hidden, shown) = (listed(&plain).await, listed(&keeper).await);
        let id = held.as_str();
        ensure!(
            !hidden.contains(id) && shown.contains(id),
            "{hidden} {shown}"
        );
        let rejected = cat.reject(&plain, &held, "no".into()).await;
        not_found("`plain`'s reject", rejected)?;
        not_found("`plain`'s accept", cat.accept(&plain, &held).await)?;
        if why == "e" {
            let accepted = cat.accept(&keeper, &held).await;
            accepted.must("accepting a holder's retarget")?;
        }
    }
    let p = cat.migrate(own, to_pq.clone()).await.must("previewing")?;
    let applied = cat.migrate_apply(own, to_pq, &p.data.report_id).await;
    applied.must("applying")?;
    // Onto a link whose `to` end is held from `plain`: not live to it, as
    // at its write door; live to the owner.
    l.upsert("trip", "p1:t1", json!({"name": "l"})).await?;
    l.declare(de(json!({"name": "rl", "fields": [name, r("trip")]})))
        .await?;
    l.upsert("rl", "b", json!({"r": "p1:t1"})).await?;
    let to_link = migration(de(
        json!({"name": "rl", "fields": [name, r("sys.link.pq.q")]}),
    ));
    let by = async |who| {
        cat.migrate(who, to_link.clone())
            .await
            .map(|p| p.data.lossless)
    };
    let (plains, owners) = (by(&plain).await, by(own).await);
    ensure!(
        matches!((&plains, &owners), (Ok(false), Ok(true))),
        "onto a link with a held end: {plains:?} {owners:?}"
    );
    let leaked: Vec<&String> = told.iter().filter(|t| t.contains("t1")).collect();
    ensure!(
        leaked.is_empty(),
        "`kept` is told a key of `trip`: {leaked:?}"
    );
    Ok(())
}

check!(
    LINK_ROWS = a_relations_retarget_is_listed_whole_to_a_reviewer_shown_its_link_type,
    "027 and 013, extended (lakelands#1092): a migration's report names a link type's field `<relation>.<field>` to a reader shown the relation who may know of that field on the link type, its ends included: a retarget, whose report names the link's `to` as `q.to`, is listed whole, `more` false and `q.to` named, to a reviewer shown everything, who accepts it and so applies it, and in part, `more` true and `q.to` not named, to a reviewer whose grant on the link type leaves `to` at `none`, whose accept is not granted; a link field `q.since` dropped, and one restated, are named to the first and listed in part, unnamed, to a reviewer whose grant leaves `since` at `none`"
);

async fn a_relations_retarget_is_listed_whole_to_a_reviewer_shown_its_link_type(
    store: Store,
) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let (cat, own) = (l.cat.as_ref(), &l.own);
    let name = json!({"name": "name", "type": {"kind": "text"}});
    let since = json!({"name": "since", "type": {"kind": "text"}});
    let pq = |to: &str, fields: serde_json::Value| {
        let q = json!({"name": "q", "target": to, "fields": fields});
        json!({"name": "pq", "fields": [name], "relations": [q]})
    };
    for n in ["trip", "tour"] {
        l.declare(de(json!({"name": n, "fields": [name]}))).await?;
        l.upsert(n, "t1", json!({ "name": n })).await?;
    }
    l.declare(de(pq("trip", json!([since])))).await?;
    l.upsert("pq", "p1", json!({"name": "p"})).await?;
    let (none, opts) = (Default::default(), WriteOpts::default());
    let (ty, from, relation, to) = ("pq".into(), "p1".into(), "q".into(), "t1".into());
    let linked = cat.link(own, &ty, &from, &relation, &to, none, opts).await;
    linked.must("linking p1 to t1")?;
    let grants = |link: serde_json::Value| {
        let mut g = json!({"default": "write", "review": true, "schema": true, "restricted": true});
        g["types"] = json!({ "sys.link.pq.q": link });
        g
    };
    let keeper = holder(s, "keeper", grants(json!({})));
    let blind = holder(s, "blind", grants(json!({"fields": {"to": "none"}})));
    let quiet = holder(s, "quiet", grants(json!({"fields": {"since": "none"}})));
    let propose = async |to: serde_json::Value, why: &str| {
        let write = ProposedWrite::Migrate {
            migration: Box::new(migration(de(to))),
            report: Default::default(),
        };
        proposal_of(cat.propose(own, write, why.into()).await)
    };
    // What a reviewer is listed of a proposal: whether more, and whether
    // its report names `field`.
    let listed = async |who: &Principal, id: &ProposalId, field: &str| {
        let all = cat.proposals(who, Default::default()).await;
        let all = all.must("listing the proposals")?.data;
        let p = all.into_iter().find(|p| &p.id == id);
        let p =
            p.ok_or_else(|| Failure::new(format!("`{}` is not listed the proposal", who.id)))?;
        let named = match &p.write {
            Some(ProposedWrite::Migrate { report, .. }) => {
                let report = serde_json::to_string(report).unwrap_or_default();
                report.contains(&format!("\"{field}\""))
            }
            _ => false,
        };
        Ok::<_, Failure>((p.more, named))
    };
    let id = propose(pq("tour", json!([since])), "onto tour").await?;
    let seen = listed(&blind, &id, "q.to").await?;
    ensure!(
        seen == (true, false),
        "kept from the link type's `to`, a reviewer is listed the retarget in part, `q.to` not named; it is listed (more, named) {seen:?}"
    );
    let refused = cat.accept(&blind, &id).await;
    ensure!(
        matches!(
            &refused,
            Err(Error::Refused {
                reason: Refusal::NotGranted,
                ..
            })
        ),
        "its accept by that reviewer is not granted; it answered {refused:?}"
    );
    let seen = listed(&keeper, &id, "q.to").await?;
    ensure!(
        seen == (false, true),
        "a reviewer shown everything is listed the retarget whole, `q.to` named; it is listed (more, named) {seen:?}"
    );
    cat.accept(&keeper, &id)
        .await
        .must("accepting the retarget as a reviewer shown everything")?;
    let d = l.describe_type("pq").await?;
    let target = d
        .relations
        .iter()
        .find(|r| r.name.as_str() == "q")
        .map(|r| r.target.to_string());
    ensure!(
        target.as_deref() == Some("tour"),
        "the accepted retarget is applied: `q` targets {target:?}"
    );
    // A link field dropped (`dropped`) and one restated (`restated`) are
    // judged on the link type as a conversion is.
    let described = json!({"name": "since", "type": {"kind": "text"}, "description": "When."});
    for (to, what) in [
        (pq("tour", json!([])), "a dropped link field"),
        (pq("tour", json!([described])), "a restated link field"),
    ] {
        let id = propose(to, what).await?;
        let (whole, part) = (
            listed(&keeper, &id, "q.since").await?,
            listed(&quiet, &id, "q.since").await?,
        );
        ensure!(
            whole == (false, true) && part == (true, false),
            "{what}: `q.since` is named whole to a reviewer shown everything ({whole:?}) and left out, with `more`, for one kept from `since` ({part:?}), as (more, named)"
        );
    }
    Ok(())
}

/// The id of a proposal that is due.
fn proposal_of(p: lakelands_api::Result<Outcome>) -> Result<ProposalId, Failure> {
    match p {
        Ok(Outcome::Proposed { proposal, .. }) => Ok(proposal),
        p => Err(Failure::new(format!("a proposal is due: {p:?}"))),
    }
}
