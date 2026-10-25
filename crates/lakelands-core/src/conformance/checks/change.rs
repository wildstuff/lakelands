//! A change to the records a condition selects is planned before it acts
//! (`docs/decisions/081-a-change-to-the-records-a-condition-selects-is-planned-before-it-acts.md`).

use lakelands_api::write::{Change, ChangeTo, Plan, RecordsPlan};

use super::only::{de, holder};
use crate::conformance::prelude::*;

check!(
    CHANGE_PLANNED = a_change_to_records_is_planned_and_enacted_whole,
    "081: `plan` of a change to the records a condition selects appends nothing and tells how many it selects, the first five keys, and per field how many records it would change, one already holding the value not counted; a write elsewhere in the lake leaves its id as it was; `enact` with that id writes each selected record, one holding the value too, at the version read, in key order, in one append whose last position the outcome answers, under its caller as the writer, and a retirement retires each; after a selected record changed, a record joined the selection or one left it, the plan named before is refused `plan_stale`, appending nothing"
);

check!(
    CHANGE_REFUSED = a_change_to_records_is_refused_whole,
    "081, 059, 016: a refusal a selected record would meet is told in the plan with its count and first keys, the plan no longer lossless, and refuses the whole `enact`, appending nothing; a refusal that needs no record refuses the plan itself, as `upsert` refuses it: a value of the wrong kind, a field of a derived authority, a view; past the lake's `max_change_records` `plan` and `enact` are refused `too_large` naming the ceiling, never cut, and a selection at the ceiling is enacted",
    given
);

check!(
    CHANGE_READ = a_change_to_records_selects_what_its_caller_may_read,
    "081, 006, 051: the condition is its caller's read: a field it may write and not read is refused not granted, naming the field, and one it may not know of, or a type it is not shown, as undeclared, appending nothing; the selection is what its `find` with that condition answers, a record its `only` does not admit or a held one neither counted nor written; and a field it may only write is told as written on every selected record, whatever it held, and is written so"
);

check!(
    CHANGE_PROPOSED = a_change_to_records_is_proposed_whole,
    "081, 062, 013: a proposal of a change to records is one proposal holding the type, the condition, the values and the plan its proposer is shown, which its listing answers it, a holder of `only` its pairs beside the condition; a plain `accept` of it is refused `plan_stale`; `accept_planned` with the id of the plan its reviewer is shown enacts it written as the proposer and caused by the acceptance, selecting no record the proposer could not; after a selected record changed it is refused `plan_stale` and the proposal stays pending"
);

check!(
    CHANGE_REVIEWED = a_change_to_records_is_accepted_whole_or_not_at_all,
    "081, 006, 051: a reviewer accepts a proposed change to records whole, never a part of what its proposer selected, and nothing its proposer could not select: a proposer without `restricted` proposes with restricted records left out, so the plan of a reviewer that holds `restricted` and its accept leave a held record out too; a reviewer holding `only` on the type beyond the proposal's pairs is refused not granted, appending nothing, and the proposal stays pending, while one whose `only` is the proposal's pairs accepts it; a proposal whose proposer held `restricted` is refused not granted to a reviewer without it"
);

check!(
    CHANGE_LINKS = a_change_to_links_sets_their_own_fields_or_unlinks,
    "081, 035: on a link type a change sets the selected links' own fields, in key order in one append, or unlinks every selected link, the walk then following none; a value that names an end is refused `link_end_as_field`, appending nothing"
);

/// `staff` (dept, salary, status), its constraint refusing a frozen
/// status outside dept `a`; s1 to s6 in dept `a`, s4 `done`, s7 in dept
/// `b` and s8 in `c`.
async fn staff(l: &Lake) -> Result<(), Failure> {
    l.declare_json(json!({"declare": "type", "name": "staff", "fields": [
        {"name": "dept", "type": {"kind": "text"}},
        {"name": "salary", "type": {"kind": "number"}},
        {"name": "status", "type": {"kind": "text"}},
        {"name": "high", "type": {"kind": "boolean"}, "authority": "derived",
         "derived": "salary > 5000", "depends_on": ["salary"]}],
        "constraints": [{"name": "capped",
                         "holds": "not (status = 'frozen') or dept = 'a'"}]}))
        .await?;
    let rows = [
        ("s1", "a", 4000, "open"),
        ("s2", "a", 6000, "open"),
        ("s3", "a", 7000, "open"),
        ("s4", "a", 3000, "done"),
        ("s5", "a", 8000, "open"),
        ("s6", "a", 9000, "open"),
        ("s7", "b", 9000, "open"),
        ("s8", "c", 9500, "open"),
    ];
    for (key, dept, salary, status) in rows {
        let fields = json!({"dept": dept, "salary": salary, "status": status});
        l.upsert("staff", key, fields).await?;
    }
    Ok(())
}

/// A change to `staff` where `filter` holds, setting `set`.
fn set(filter: &str, set: Value) -> Change {
    records(filter, ChangeTo::Set(de(set)))
}

/// A change to `staff` where `filter` holds, doing `to`.
fn records(filter: &str, to: ChangeTo) -> Change {
    Change::Records {
        ty: "staff".into(),
        filter: Box::new(filter.into()),
        to,
        only: BTreeMap::new(),
        skip_restricted: false,
    }
}

/// A plan's section of records, or a failure naming `what`.
fn section<'a>(p: &'a Plan, what: &str) -> Result<&'a RecordsPlan, Failure> {
    (p.records.as_deref()).ok_or_else(|| Failure::new(format!("{what} told no records")))
}

/// A section as one line: the count, the first keys, each field's count,
/// and each refusal's code, count and first keys.
fn line(p: &Plan) -> String {
    let Some(r) = p.records.as_deref() else {
        return "no records".to_owned();
    };
    let keys = |k: &[RecordKey]| k.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(",");
    let mut s = format!("{} [{}]", r.selected, keys(&r.sample_keys));
    for f in &r.fields {
        s += &format!(" {} {}", f.field, f.records);
    }
    for g in &r.refused {
        let code = match &*g.refusal {
            Error::Refused { reason, .. } => format!("{reason:?}"),
            e => format!("{e:?}"),
        };
        let code = code.split(['(', ' ', '{']).next().unwrap_or_default();
        s += &format!(" refused {code} {} [{}]", g.records, keys(&g.sample_keys));
    }
    s
}

/// `who`'s plan of `change`.
async fn plan(l: &Lake, who: &Principal, change: &Change) -> Result<Plan, Failure> {
    let p = l.cat.plan(who, change.clone()).await;
    Ok(p.must(&format!("planning {change:?}"))?.data)
}

/// The keys and versions of `ty`'s records `who`'s `find` with `filter`
/// answers, in key order.
async fn found(
    l: &Lake,
    who: &Principal,
    filter: &str,
) -> Result<Vec<(RecordKey, Version)>, Failure> {
    let found = all_found(l.cat.as_ref(), who, "staff", find_where(filter)).await?;
    let mut out: Vec<_> = found.into_iter().map(|r| (r.key, r.version)).collect();
    out.sort();
    Ok(out)
}

/// The record writes among `events`: type, key, version, and whether a
/// retirement.
fn writes(events: &[Event]) -> Vec<(String, Version, bool)> {
    (events.iter())
        .filter_map(|e| match &e.body {
            EventBody::RecordUpserted { key, version, .. } => {
                Some((key.as_str().to_owned(), *version, false))
            }
            EventBody::RecordRetired { key, version, .. } => {
                Some((key.as_str().to_owned(), *version, true))
            }
            _ => None,
        })
        .collect()
}

/// Whether an answer is a refusal `plan_stale`.
fn is_stale<T>(a: &lakelands_api::Result<T>) -> bool {
    matches!(a, Err(e) if matches!(invalid_case(&Err::<(), _>(e.clone())), Some(Case::PlanStale)))
}

async fn a_change_to_records_is_planned_and_enacted_whole(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let c = l.cat.as_ref();
    staff(&l).await?;
    let done = set("dept = 'a'", json!({"status": "done"}));
    let head = s.head().await.must("the head")?;
    let planned = plan(&l, &l.own, &done).await?;
    head_unmoved(s, head, "a plan").await?;
    let told = line(&planned);
    ensure!(
        told == "6 [s1,s2,s3,s4,s5] status 5" && planned.lossless,
        "the plan of `done` on dept a must select six, name s1 to s5, and change the status of five, s4 holding it: {told}"
    );
    let selection = found(&l, &l.own, "dept = 'a'").await?;
    // A write elsewhere leaves the plan's id as it was.
    l.upsert("staff", "s7", json!({"salary": 9100})).await?;
    let again = plan(&l, &l.own, &done).await?;
    ensure!(
        again.report_id == planned.report_id,
        "a write to a record the change does not select must leave the plan's id as it was"
    );
    let head = s.head().await.must("the head")?;
    let enacted = c.enact(&l.own, done.clone(), &planned.report_id).await;
    let enacted = enacted.must("enacting the plan")?;
    let events = events_after(s, head).await?;
    let wrote = writes(&events);
    let expected: Vec<_> = (selection.iter())
        .map(|(k, v)| (k.as_str().to_owned(), Version(v.0 + 1), false))
        .collect();
    ensure!(
        wrote == expected && wrote.len() == events.len(),
        "`enact` must write each selected record, in key order, at the version read, and nothing else: {wrote:?}"
    );
    let last = events.last().map(|e| e.position);
    ensure!(
        matches!(&enacted, Outcome::Applied { position, .. } if Some(*position) == last),
        "the outcome must answer the last position of the change: {enacted:?}"
    );
    ensure!(
        events.iter().all(|e| e.writer == l.own.writer()),
        "each event of the change must be written by its caller"
    );
    // The selection moved, three ways: the plan named before is stale.
    let frozen = set("dept = 'a'", json!({"status": "open"}));
    let moves: [(&str, &str, Value); 3] = [
        ("a selected record changed", "s2", json!({"salary": 6100})),
        (
            "a record joined",
            "s8",
            json!({"dept": "a", "salary": 1, "status": "open"}),
        ),
        ("a record left", "s1", json!({"dept": "b"})),
    ];
    for (what, key, fields) in moves {
        let planned = plan(&l, &l.own, &frozen).await?;
        l.upsert("staff", key, fields).await?;
        let head = s.head().await.must("the head")?;
        let enacted = c.enact(&l.own, frozen.clone(), &planned.report_id).await;
        ensure!(
            is_stale(&enacted),
            "after {what}, `enact` must be refused `plan_stale`: {enacted:?}"
        );
        head_unmoved(s, head, what).await?;
    }
    // A retirement retires each selected record.
    let gone = records("dept = 'b'", ChangeTo::Retire);
    let planned = plan(&l, &l.own, &gone).await?;
    let head = s.head().await.must("the head")?;
    c.enact(&l.own, gone, &planned.report_id)
        .await
        .must("retiring dept b")?;
    let wrote = writes(&events_after(s, head).await?);
    let keys: Vec<_> = wrote.iter().map(|(k, _, r)| (k.as_str(), *r)).collect();
    ensure!(
        keys == [("s1", true), ("s7", true)],
        "retiring dept b must retire s1 and s7, in key order: {wrote:?}"
    );
    Ok(())
}

async fn a_change_to_records_is_refused_whole(store: Store, given: Given) -> Held {
    let mut few = limits();
    few.max_change_records = 3;
    let store = (given.relimit)(store, Seams::default(), few)
        .await
        .map_err(|e| Failure::new(format!("the harness opened no lake: {e}")))?;
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let c = l.cat.as_ref();
    staff(&l).await?;
    // s7 and s8 are outside dept a: a frozen status refuses them, and
    // the whole change.
    let frozen = set("salary > 5000", json!({"status": "frozen"}));
    let head = s.head().await.must("the head")?;
    let planned = l.cat.plan(&l.own, frozen.clone()).await;
    let refused = refused_unmoved(s, head, "a selection past the ceiling", planned).await?;
    ensure!(
        matches!(
            &refused,
            Error::Refused {
                reason: Refusal::TooLarge { max: 3 },
                ..
            }
        ),
        "a plan selecting six past a ceiling of three must be refused `too_large` naming 3: {refused:?}"
    );
    let enacted = c.enact(&l.own, frozen, "").await;
    let refused = refused_unmoved(s, head, "an enact past the ceiling", enacted).await?;
    ensure!(
        matches!(
            &refused,
            Error::Refused {
                reason: Refusal::TooLarge { max: 3 },
                ..
            }
        ),
        "an enact selecting six past a ceiling of three must be refused `too_large` naming 3: {refused:?}"
    );
    let frozen = set("salary > 8500", json!({"status": "frozen"}));
    let planned = plan(&l, &l.own, &frozen).await?;
    let told = line(&planned);
    ensure!(
        told == "3 [s6,s7,s8] status 3 refused Constraint 2 [s7,s8]" && !planned.lossless,
        "the plan must tell the constraint refusing s7 and s8, lossy: {told}"
    );
    let enacted = c.enact(&l.own, frozen, &planned.report_id).await;
    let refused = refused_unmoved(s, head, "a change one record refuses", enacted).await?;
    ensure!(
        matches!(
            reason(&Err::<(), _>(refused.clone())),
            Some(Refusal::Constraint(..))
        ),
        "`enact` must be refused as its records are: {refused:?}"
    );
    // Refused before a record is read, as `upsert` refuses it.
    let cases: [(&str, Change); 2] = [
        (
            "a value of the wrong kind",
            set("dept = 'a'", json!({"salary": "lots"})),
        ),
        ("a derived field", set("dept = 'a'", json!({"high": true}))),
    ];
    for (what, change) in cases {
        let Change::Records {
            to: ChangeTo::Set(patch),
            ..
        } = &change
        else {
            unreachable!("the cases set values")
        };
        let (ty, key) = (TypeName::from("staff"), RecordKey::from("s1"));
        let upsert = c.upsert(&l.own, &ty, &key, patch.clone(), WriteOpts::default());
        let (planned, upsert) = (c.plan(&l.own, change).await, upsert.await);
        refused_alike(what, &planned, upsert, ("plan", "upsert"))?;
        head_unmoved(s, head, what).await?;
    }
    // At the ceiling: enacted.
    let open = set("dept = 'b' or salary < 5000", json!({"status": "open"}));
    let planned = plan(&l, &l.own, &open).await?;
    let enacted = c.enact(&l.own, open, &planned.report_id).await;
    enacted.must("a change of three at a ceiling of three")?;
    Ok(())
}

async fn a_change_to_records_selects_what_its_caller_may_read(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let c = l.cat.as_ref();
    staff(&l).await?;
    l.declare_json(json!({"declare": "type", "name": "secret",
        "fields": [{"name": "x", "type": {"kind": "text"}}]}))
        .await?;
    // `hr` may write `salary` and not read it, and is not shown `secret`.
    let hr = holder(
        s,
        "hr",
        json!({"default": "write", "types": {
            "staff": {"fields": {"salary": "write_only", "high": "none"}},
            "secret": {"default": "none"}}}),
    );
    let head = s.head().await.must("the head")?;
    let oracle = c
        .plan(&hr, set("salary > 5000", json!({"status": "x"})))
        .await;
    ensure!(
        matches!(refused_on(&oracle), Some(("salary", Refusal::NotGranted))),
        "a condition on a field its caller may write and not read must be refused not granted, naming it: {oracle:?}"
    );
    let hidden = c
        .plan(&hr, set("high = true", json!({"status": "x"})))
        .await;
    ensure!(
        no_such_field(&hidden, "high"),
        "a condition on a field its caller may not know of must be refused as undeclared: {hidden:?}"
    );
    let unshown = Change::Records {
        ty: "secret".into(),
        filter: Box::new("x = 'y'".into()),
        to: ChangeTo::Retire,
        only: BTreeMap::new(),
        skip_restricted: false,
    };
    let unshown = c.plan(&hr, unshown).await;
    ensure!(
        matches!(
            &unshown,
            Err(Error::NotFound {
                missing: Missing::Type { .. }
            })
        ),
        "a type its caller is not shown must be refused as not declared: {unshown:?}"
    );
    head_unmoved(s, head, "a condition its caller may not read").await?;
    // A field it may only write is told as written on every record.
    let raise = set("dept = 'a'", json!({"salary": 6000}));
    let planned = plan(&l, &hr, &raise).await?;
    let told = line(&planned);
    ensure!(
        told == "6 [s1,s2,s3,s4,s5] salary 6",
        "a field its caller may only write must be told as written on every selected record, s2 holding it too: {told}"
    );
    let head = s.head().await.must("the head")?;
    c.enact(&hr, raise, &planned.report_id)
        .await
        .must("the raise")?;
    ensure!(
        writes(&events_after(s, head).await?).len() == 6,
        "a field its caller may only write must be written on every selected record"
    );
    // A holder of `only` and a held record: the selection is its `find`.
    restrict(s, "staff", "s3", "kept").await?;
    let mine = holder(
        s,
        "mine",
        json!({"default": "write", "types": {"staff": {"only": {"dept": "a"}}}}),
    );
    let mark = set("status != 'none'", json!({"status": "marked"}));
    let planned = plan(&l, &mine, &mark).await?;
    let selection = found(&l, &mine, "status != 'none'").await?;
    let keys: Vec<_> = selection.iter().map(|(k, _)| k.as_str()).collect();
    ensure!(
        keys == ["s1", "s2", "s4", "s5", "s6"],
        "the holder of `only` dept a must find its own unheld records: {keys:?}"
    );
    let told = line(&planned);
    ensure!(
        told == "5 [s1,s2,s4,s5,s6] status 5",
        "the selection must be what its caller's `find` answers: {told}"
    );
    let head = s.head().await.must("the head")?;
    c.enact(&mine, mark, &planned.report_id)
        .await
        .must("marking")?;
    let wrote: Vec<_> = writes(&events_after(s, head).await?)
        .into_iter()
        .map(|w| w.0)
        .collect();
    ensure!(
        wrote == keys,
        "the change must write what its caller's `find` answers, nothing else: {wrote:?}"
    );
    Ok(())
}

async fn a_change_to_records_is_proposed_whole(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let c = l.cat.as_ref();
    staff(&l).await?;
    let agent = holder(
        s,
        "agent",
        json!({"default": "write", "types": {"staff": {"only": {"dept": "a"}}}}),
    );
    let reviewer = holder(s, "reviewer", json!({"default": "write", "review": true}));
    let proposed = ProposedWrite::Records {
        ty: "staff".into(),
        filter: "salary > 5000".into(),
        to: ChangeTo::Set(de(json!({"status": "review"}))),
        only: BTreeMap::new(),
        skip_restricted: false,
        plan: Box::default(),
    };
    let head = s.head().await.must("the head")?;
    let filed = c.propose(&agent, proposed, "a review".into()).await;
    let Outcome::Proposed { proposal, .. } = filed.must("proposing the change")? else {
        return Err(Failure::new("a proposal must be filed"));
    };
    ensure!(
        events_after(s, head).await?.len() == 1,
        "a proposal of a change to records must be one event"
    );
    let listed = c.proposals(&agent, ProposalFilter::default()).await;
    let listed = listed.must("the proposer's listing")?.data;
    let Some(Proposal {
        write: Some(write), ..
    }) = listed.into_iter().find(|p| p.id == proposal)
    else {
        return Err(Failure::new("the proposer must be listed the proposal"));
    };
    let ProposedWrite::Records {
        only, plan: shown, ..
    } = &write
    else {
        return Err(Failure::new(format!(
            "the proposal must hold the change: {write:?}"
        )));
    };
    let told = line(shown);
    ensure!(
        only.get("dept") == Some(&json!("a")) && told == "4 [s2,s3,s5,s6] status 4",
        "the proposal must hold its proposer's pairs and the plan it was shown: {only:?} {told}"
    );
    let plain = c.accept(&reviewer, &proposal).await;
    ensure!(
        is_stale(&plain),
        "a plain accept must be refused `plan_stale`: {plain:?}"
    );
    let Some(change) = write.change() else {
        return Err(Failure::new("the proposal must name its change"));
    };
    // The reviewer's plan, stale after s2 changed: refused, still pending.
    let reviewed = plan(&l, &reviewer, &change).await?;
    section(&reviewed, "the reviewer's plan")?;
    l.upsert("staff", "s2", json!({"salary": 6200})).await?;
    let head = s.head().await.must("the head")?;
    let stale = c
        .accept_planned(&reviewer, &proposal, &reviewed.report_id)
        .await;
    ensure!(
        is_stale(&stale),
        "after s2 changed the accept must be refused `plan_stale`: {stale:?}"
    );
    head_unmoved(s, head, "a stale accept").await?;
    let pending = ProposalFilter {
        state: Some(ProposalState::Pending),
        ..ProposalFilter::default()
    };
    let still = c
        .proposals(&reviewer, pending)
        .await
        .must("the pending")?
        .data;
    ensure!(
        still.iter().any(|p| p.id == proposal),
        "a proposal refused `plan_stale` must stay pending"
    );
    // Against the plan made again: enacted as the proposer, s7 not selected.
    let reviewed = plan(&l, &reviewer, &change).await?;
    let accepted = c
        .accept_planned(&reviewer, &proposal, &reviewed.report_id)
        .await;
    accepted.must("accepting against the reviewer's plan")?;
    let events = events_after(s, head).await?;
    let accept = events.first().map(|e| e.position);
    let wrote: Vec<_> = writes(&events).into_iter().map(|w| w.0).collect();
    ensure!(
        wrote == ["s2", "s3", "s5", "s6"],
        "the accept must write the proposer's selection, in key order: {wrote:?}"
    );
    let theirs = (events.iter()).filter(|e| matches!(e.body, EventBody::RecordUpserted { .. }));
    ensure!(
        theirs
            .clone()
            .all(|e| e.writer == agent.writer() && e.cause == accept),
        "each write must be the proposer's, caused by the acceptance"
    );
    Ok(())
}

/// `who`'s proposal of a change to `staff` where `filter` holds, setting
/// `set`: its id and the change its listing answers its proposer.
async fn proposed(
    c: &dyn Catalog,
    who: &Principal,
    filter: &str,
    set: Value,
) -> Result<(ProposalId, ProposedWrite), Failure> {
    let write = ProposedWrite::Records {
        ty: "staff".into(),
        filter: filter.into(),
        to: ChangeTo::Set(de(set)),
        only: BTreeMap::new(),
        skip_restricted: false,
        plan: Box::default(),
    };
    let filed = c.propose(who, write, "a review".into()).await;
    let Outcome::Proposed { proposal, .. } = filed.must("proposing the change")? else {
        return Err(Failure::new("a proposal must be filed"));
    };
    let listed = c.proposals(who, ProposalFilter::default()).await;
    let listed = listed.must("the proposer's listing")?.data;
    let write = listed.into_iter().find(|p| p.id == proposal);
    let write = write.and_then(|p| p.write);
    write
        .map(|w| (proposal, w))
        .ok_or_else(|| Failure::new("the proposer must be listed its proposal"))
}

/// `reviewer`'s accept of `proposal` against its own plan of `write`.
async fn accepted(
    l: &Lake,
    reviewer: &Principal,
    (proposal, write): &(ProposalId, ProposedWrite),
) -> Result<lakelands_api::Result<Outcome>, Failure> {
    let Some(change) = write.change() else {
        return Err(Failure::new("the proposal must name its change"));
    };
    let reviewed = plan(l, reviewer, &change).await?;
    let c = l.cat.as_ref();
    Ok(c.accept_planned(reviewer, proposal, &reviewed.report_id)
        .await)
}

async fn a_change_to_records_is_accepted_whole_or_not_at_all(store: Store) -> Held {
    let (l, s) = (Lake::of(store.as_ref()), store.as_ref());
    let c = l.cat.as_ref();
    staff(&l).await?;
    restrict(s, "staff", "s3", "kept").await?;
    let agent = holder(s, "agent", json!({"default": "write"}));
    let keeper = holder(
        s,
        "keeper",
        json!({"default": "write", "review": true, "restricted": true}),
    );
    // The held s3 is left out of what the agent could not select.
    let review = proposed(c, &agent, "dept = 'a'", json!({"status": "review"})).await?;
    let Some(change) = review.1.change() else {
        return Err(Failure::new("the proposal must name its change"));
    };
    let told = line(&plan(&l, &keeper, &change).await?);
    ensure!(
        told == "5 [s1,s2,s4,s5,s6] status 5",
        "the reviewer's plan of a proposal must leave out the record held from its proposer: {told}"
    );
    let head = s.head().await.must("the head")?;
    accepted(&l, &keeper, &review)
        .await?
        .must("accepting against the reviewer's plan")?;
    let wrote: Vec<_> = writes(&events_after(s, head).await?)
        .into_iter()
        .map(|w| w.0)
        .collect();
    ensure!(
        wrote == ["s1", "s2", "s4", "s5", "s6"],
        "the accept must write no record held from its proposer: {wrote:?}"
    );
    // A reviewer holding `only` beyond the proposal's pairs: refused.
    let checked = proposed(c, &agent, "salary > 5000", json!({"status": "checked"})).await?;
    let only = |id: &str, dept: &str| {
        let grants = json!({"default": "write", "review": true,
            "types": {"staff": {"only": {"dept": dept}}}});
        holder(s, id, grants)
    };
    let head = s.head().await.must("the head")?;
    for reviewer in [only("only-a", "a"), only("only-b", "b")] {
        let refused = accepted(&l, &reviewer, &checked).await?;
        ensure!(
            matches!(reason(&refused), Some(Refusal::NotGranted)),
            "a reviewer holding `only` beyond the proposal's pairs must be refused not granted: {refused:?}"
        );
    }
    head_unmoved(s, head, "an accept of part of a proposal").await?;
    let pending = ProposalFilter {
        state: Some(ProposalState::Pending),
        ..ProposalFilter::default()
    };
    let still = c.proposals(&keeper, pending).await.must("the pending")?;
    ensure!(
        still.data.iter().any(|p| p.id == checked.0),
        "a proposal refused to a reviewer holding `only` must stay pending"
    );
    // One whose `only` is the proposal's pairs accepts it.
    let mine = holder(
        s,
        "mine",
        json!({"default": "write", "types": {"staff": {"only": {"dept": "a"}}}}),
    );
    let ours = proposed(c, &mine, "salary > 5000", json!({"status": "ours"})).await?;
    let head = s.head().await.must("the head")?;
    accepted(&l, &only("only-a", "a"), &ours)
        .await?
        .must("accepting within the proposal's pairs")?;
    let wrote: Vec<_> = writes(&events_after(s, head).await?)
        .into_iter()
        .map(|w| w.0)
        .collect();
    ensure!(
        wrote == ["s2", "s5", "s6"],
        "a reviewer whose `only` is the proposal's pairs must accept it whole: {wrote:?}"
    );
    // A proposer that held `restricted`: not a reviewer without it's.
    let boss = holder(s, "boss", json!({"default": "write", "restricted": true}));
    let held = proposed(c, &boss, "dept = 'a'", json!({"status": "boss"})).await?;
    let plain = holder(s, "plain", json!({"default": "write", "review": true}));
    let head = s.head().await.must("the head")?;
    let refused = accepted(&l, &plain, &held).await?;
    ensure!(
        matches!(reason(&refused), Some(Refusal::NotGranted)),
        "a proposal whose proposer held `restricted` must be refused not granted to a reviewer without it: {refused:?}"
    );
    head_unmoved(s, head, "an accept without `restricted`").await?;
    Ok(())
}

async fn a_change_to_links_sets_their_own_fields_or_unlinks(store: Store) -> Held {
    use super::links::{SEPT, lake, link_ok, walked};
    let l = lake(store.as_ref()).await?;
    let (c, s) = (l.cat.as_ref(), store.as_ref());
    for from in ["i1", "i2"] {
        link_ok(&l, from, "p1", json!({"since": SEPT})).await?;
    }
    let links = |filter: &str, to: ChangeTo| Change::Records {
        ty: "sys.link.invoice.approved_by".into(),
        filter: Box::new(filter.into()),
        to,
        only: BTreeMap::new(),
        skip_restricted: false,
    };
    let later = "2026-10-01T00:00:00.000000Z";
    let dated = links("to = 'p1'", ChangeTo::Set(de(json!({"since": later}))));
    let planned = plan(&l, &l.own, &dated).await?;
    let head = s.head().await.must("the head")?;
    c.enact(&l.own, dated, &planned.report_id)
        .await
        .must("dating the links")?;
    let events = events_after(s, head).await?;
    ensure!(
        writes(&events).len() == 2 && events.len() == 2,
        "a change to two links must write each, and nothing else: {:?}",
        writes(&events)
    );
    for from in ["i1", "i2"] {
        let out = walked(c, &l.own, "invoice", from, Walk::Out, None).await?;
        ensure!(
            out.len() == 1 && out[0].fields.get("since") == Some(&json!(later)),
            "the link from {from} must hold its new `since`: {out:?}"
        );
    }
    // A value naming an end: refused before a link is read.
    let head = s.head().await.must("the head")?;
    let moved = links("to = 'p1'", ChangeTo::Set(de(json!({"to": "p2"}))));
    let moved = c.plan(&l.own, moved).await;
    ensure!(
        matches!(invalid_case(&moved), Some(Case::LinkEndAsField { end }) if end == "to"),
        "a change that names a link's end must be refused `link_end_as_field`: {moved:?}"
    );
    head_unmoved(s, head, "a change naming an end").await?;
    // Unlinked, every one.
    let gone = links("to = 'p1'", ChangeTo::Retire);
    let planned = plan(&l, &l.own, &gone).await?;
    c.enact(&l.own, gone, &planned.report_id)
        .await
        .must("unlinking")?;
    let retired = writes(&events_after(s, head).await?);
    ensure!(
        retired.len() == 2 && retired.iter().all(|w| w.2),
        "a retirement of two links must unlink each: {retired:?}"
    );
    for from in ["i1", "i2"] {
        let out = walked(c, &l.own, "invoice", from, Walk::Out, None).await?;
        ensure!(
            out.is_empty(),
            "the walk from {from} must follow no link: {out:?}"
        );
    }
    Ok(())
}
