//! `judge` (lakelands#1670,
//! `docs/decisions/113-a-judge-answers-what-rules-say-of-records.md`):
//! what a type's rules say of records, stored or not yet sent, answered
//! with the refusal a write gives and nothing appended.

use lakelands_api::judge::{Judge, JudgeReport, Rules, Subject, Unjudged, UnjudgedWhy};

use crate::conformance::fixture::source_record;
use crate::conformance::prelude::*;

check!(
    JUDGE_WRITES_NOTHING = a_judge_appends_nothing,
    "113: a judge of stored keys, of candidates, and of a record that breaks a refusing rule appends nothing: the head, the log's length and every field.s meta are what they were, and a write of the same record is still refused"
);

check!(
    JUDGE_SAYS_WHAT_THE_WRITE_SAYS = a_judge_refuses_as_the_write_does,
    "113: for a record and a declared rule that refuses, the judge's refusal is the write's, word for word, naming the constraint, the type and the key; where the rule is not judged whole, the breaker it reports for the part it judged is that refusal, the unjudged part named beside it; an admitting rule the record breaks is reported too"
);

check!(
    JUDGE_NAMES_WHAT_ONLY_A_WRITE_DECIDES = a_judge_names_what_only_a_write_decides,
    "113: over stored records a rule reading `previous()` or `changed()` is answered `unjudged` `TheWriteAlone` once for the rule, never a pass, its other conjuncts still judged; a candidate is judged as the ingest window judges it, so none is `TheWriteAlone` for it"
);

check!(
    JUDGE_JUDGES_WHAT_THE_WRITE_JUDGES = a_judge_judges_what_the_write_judges,
    "113: a candidate for a stored record is judged by the rules its write would judge: a rule over `secret`, which a feed of `n` does not send, breaks the stored record and is not judged for the candidate; a candidate leaving the record as it stands is refused by none, though the stored record breaks the rule"
);

check!(
    JUDGE_READS_AT_THE_READS_CLOCK = a_judge_reads_at_the_reads_clock,
    "113, 052: a declared rule and a proposed rule reading `today()` over stored keys each break, at each of the read's clocks, exactly the keys a `find` of `not(<rule>)` answers there: one clock rule for both forms"
);

check!(
    JUDGE_NAMES_WHAT_NO_SCOPE_ANSWERS = a_judge_names_what_no_scope_answers,
    "113: a proposed rule calling `in_set()` is refused whole `invalid` with `InSetHere`; one calling `exists()` over a stored key and over a candidate alike answers `unjudged` `Unevaluable` with `ExistsHere`, named with each record and never a pass"
);

check!(
    JUDGE_ANSWERS_NO_FIELD = a_judge_answers_no_field,
    "113, 105: a declared rule the caller is not shown is neither judged nor named, and naming it is refused `not_found`; a proposed rule reading a masked field is refused `not_granted` naming the field; no stored value appears in an answer"
);

check!(
    JUDGE_IS_BOUNDED = a_judge_is_bounded,
    "113: more keys, or more candidates, than `limits.max_limit` is refused `too_large` on `records`, never cut; `max_limit` of them is judged whole"
);

check!(
    JUDGE_JUDGES_ONE_TYPE = a_judge_judges_one_type,
    "113: a key the named type does not hold is refused `not_found`; a rule the type does not declare is refused `not_found`; a candidate of another type is refused whole `invalid` with `CandidateOfAnotherType`, nothing judged"
);

check!(
    JUDGE_IS_NO_TOOL = a_judge_is_no_tool,
    "113: the catalog a principal receives names no tool that judges"
);

/// `slip`, under `constraints`: `id` and `n` its source's, `secret`
/// authored, `next` a link to another slip.
fn slip(constraints: Value) -> Value {
    json!({"declare": "type", "name": "slip", "key": ["id"], "fields": [
        {"name": "id", "type": {"kind": "text"}, "authority": "source"},
        {"name": "n", "type": {"kind": "number"}, "authority": "source"},
        {"name": "secret", "type": {"kind": "number"}}],
        "relations": [{"name": "next", "target": "slip"}],
        "constraints": constraints})
}

/// Slips `s1` (`n` 1) and `s2` (`n` 7), fed, each with an authored
/// `secret`; `n` must be positive at a write.
async fn slips(l: &Lake) -> Held {
    slips_under(l, json!([{"name": "positive", "holds": "n > 0"}])).await
}

/// [`slips`], `slip` under `constraints`.
async fn slips_under(l: &Lake, constraints: Value) -> Held {
    l.declare_json(slip(constraints)).await?;
    let fed =
        [("s1", 1), ("s2", 7)].map(|(k, n)| source_record("slip", k, json!({"id": k, "n": n})));
    l.ingest("ledger", fed.into())
        .await
        .must("feeding the slips")?;
    for k in ["s1", "s2"] {
        l.upsert("slip", k, json!({"secret": 4242})).await?;
    }
    Ok(())
}

/// A feed of slip `key` with `n`, as the owner.
async fn feed(
    l: &Lake,
    key: &str,
    n: i64,
) -> lakelands_api::Result<Vec<lakelands_api::write::Committed>> {
    let r = source_record("slip", key, json!({"id": key, "n": n}));
    l.ingest("ledger", vec![r]).await
}

fn keys(ks: &[&str]) -> Subject {
    Subject::Keys(ks.iter().map(|k| RecordKey::from(*k)).collect())
}

fn candidates(ty: &str, rs: &[(&str, i64)]) -> Subject {
    let rs = rs
        .iter()
        .map(|(k, n)| source_record(ty, k, json!({"id": k, "n": n})));
    Subject::Candidates(rs.collect())
}

fn declared() -> Rules {
    Rules::Declared { names: None }
}

fn proposed(name: &str, holds: &str) -> Rules {
    Rules::Proposed {
        name: name.into(),
        holds: holds.into(),
    }
}

async fn judged(
    l: &Lake,
    who: &Principal,
    subject: Subject,
    rules: Rules,
    at: ReadAt,
) -> lakelands_api::Result<JudgeReport> {
    let judge = Judge { subject, rules, at };
    let said = l.cat.judge(who, &"slip".into(), judge).await;
    said.map(|r| r.data)
}

async fn judge(l: &Lake, subject: Subject, rules: Rules) -> Result<JudgeReport, Failure> {
    judged(l, &l.own, subject, rules, ReadAt::default())
        .await
        .must("a judge")
}

/// The keys a report says break a rule, and the constraints they break.
fn broken(r: &JudgeReport) -> BTreeSet<(String, String)> {
    let named = |x: &Refusal| match x {
        Refusal::Constraint(s) => match &s.case {
            Case::ConstraintBroken { constraint, .. } => Some(constraint.to_string()),
            _ => None,
        },
        _ => None,
    };
    let each = r.records.iter().flat_map(|j| {
        let key = j.key.to_string();
        j.refusals
            .iter()
            .filter_map(named)
            .map(move |c| (key.clone(), c))
    });
    each.collect()
}

fn pairs(p: &[(&str, &str)]) -> BTreeSet<(String, String)> {
    p.iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
}

async fn a_judge_appends_nothing(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    slips(&l).await?;
    let head = store.head().await.must("the head")?;
    let log = log_events(store.as_ref()).await?.len();
    let known = [l.read("slip", "s1").await?, l.read("slip", "s2").await?];
    let s1 = judge(&l, keys(&["s1", "s2"]), proposed("big", "n > 5")).await?;
    let s2 = judge(&l, keys(&["s1"]), declared()).await?;
    let s3 = judge(&l, candidates("slip", &[("s3", -1)]), declared()).await?;
    ensure!(
        broken(&s1) == pairs(&[("s1", "big")]) && broken(&s3) == pairs(&[("s3", "positive")]),
        "the judges answer: {s1:?}, {s2:?}, {s3:?}"
    );
    head_unmoved(store.as_ref(), head, "a judge").await?;
    let after = log_events(store.as_ref()).await?.len();
    let again = [l.read("slip", "s1").await?, l.read("slip", "s2").await?];
    let same = |a: &Record, b: &Record| a.meta == b.meta;
    ensure!(
        after == log && same(&known[0], &again[0]) && same(&known[1], &again[1]),
        "a judge appends nothing: the log held {log} events and holds {after}"
    );
    let write = feed(&l, "s3", -1).await;
    ensure!(
        write.is_err(),
        "a write of a record a judge broke is still refused: {write:?}"
    );
    Ok(())
}

async fn a_judge_refuses_as_the_write_does(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    slips(&l).await?;
    let write = feed(&l, "s3", -1).await;
    let write = reason(&write).cloned();
    let said = judge(&l, candidates("slip", &[("s3", -1)]), declared()).await?;
    let first = said.records.first().map(|r| r.refusals.clone());
    ensure!(
        write.is_some() && first == Some(write.clone().into_iter().collect()),
        "the judge's refusal of `s3` is the write's, {write:?}: it answers {said:?}"
    );
    // A rule judged in part: `n > 5` breaks `s1`, `previous(n)` is the
    // write's alone, named beside it.
    let part = judge(
        &l,
        keys(&["s1"]),
        proposed("grows", "n > 5 and previous(n) < n"),
    )
    .await?;
    let alone = Unjudged {
        constraint: "grows".into(),
        why: UnjudgedWhy::TheWriteAlone,
    };
    ensure!(
        broken(&part) == pairs(&[("s1", "grows")]) && part.unjudged == [alone],
        "the part judged breaks `s1` and the part only a write decides is named: {part:?}"
    );
    // An admitting rule a record breaks is reported, as a walk reports it.
    let admit = json!([{"name": "positive", "holds": "n > 0", "at_ingest": "admit"}]);
    l.declare_json(slip(admit)).await?;
    feed(&l, "s4", -2).await.must("an admitted feed")?;
    let admitted = judge(&l, candidates("slip", &[("s4", -3)]), declared()).await?;
    ensure!(
        broken(&admitted) == pairs(&[("s4", "positive")]),
        "an admitting rule `s4` breaks is reported: {admitted:?}"
    );
    Ok(())
}

async fn a_judge_names_what_only_a_write_decides(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    slips(&l).await?;
    let rule = || proposed("steady", "n > 2 and changed(n)");
    let stored = judge(&l, keys(&["s1", "s2"]), rule()).await?;
    let alone = Unjudged {
        constraint: "steady".into(),
        why: UnjudgedWhy::TheWriteAlone,
    };
    ensure!(
        stored.unjudged == [alone] && broken(&stored) == pairs(&[("s1", "steady")]),
        "over stored keys `changed(n)` is named once, `n > 2` still judged: {stored:?}"
    );
    let sent = judge(&l, candidates("slip", &[("s1", 0), ("s2", 8)]), rule()).await?;
    ensure!(
        sent.unjudged.is_empty() && broken(&sent) == pairs(&[("s1", "steady")]),
        "a candidate is judged as the window judges it, every part: `s1` falls to 0, \
         `s2` changes to 8; nothing is the write's alone: {sent:?}"
    );
    Ok(())
}

async fn a_judge_judges_what_the_write_judges(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    slips(&l).await?;
    let small = || proposed("small", "secret < 100");
    let stored = judge(&l, keys(&["s1"]), small()).await?;
    let sent = judge(&l, candidates("slip", &[("s1", 3)]), small()).await?;
    let quiet = |r: &JudgeReport| r.records.iter().all(|j| j.refusals.is_empty());
    ensure!(
        broken(&stored) == pairs(&[("s1", "small")]) && quiet(&sent) && sent.records.len() == 1,
        "`small` breaks the stored `s1`; a feed of `n` sends no `secret`, so the write \
         judges no `small`: {stored:?}, {sent:?}"
    );
    let big = || proposed("big", "n > 9");
    let stored = judge(&l, keys(&["s2"]), big()).await?;
    let same = judge(&l, candidates("slip", &[("s2", 7)]), big()).await?;
    ensure!(
        broken(&stored) == pairs(&[("s2", "big")]) && quiet(&same) && same.records.len() == 1,
        "`big` breaks the stored `s2`; a candidate leaving it as it stands is refused by \
         none: {stored:?}, {same:?}"
    );
    Ok(())
}

async fn a_judge_reads_at_the_reads_clock(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare_json(
        json!({"declare": "type", "name": "slip", "key": ["id"], "fields": [
        {"name": "id", "type": {"kind": "text"}, "authority": "source"},
        {"name": "due", "type": {"kind": "date"}, "authority": "source"}],
        "constraints": [{"name": "current", "holds": "due >= today()", "at_ingest": "admit"}]}),
    )
    .await?;
    let due = |k: &str, d: &str| source_record("slip", k, json!({"id": k, "due": d}));
    let first = vec![
        due("s1", "2000-01-01"),
        due("s2", "2400-01-01"),
        due("s3", "2999-01-01"),
    ];
    let fed = l.ingest("ledger", first).await.must("feeding the slips")?;
    let at = fed.last().map(|c| c.position);
    l.ingest("ledger", vec![due("s4", "2000-01-01")])
        .await
        .must("feeding `s4`")?;
    let instant = |s: &str| {
        s.parse::<Timestamp>()
            .map_err(|_| Failure::new("an instant"))
    };
    let (valid, host) = (
        instant("2500-01-01T00:00:00Z")?,
        instant("2450-01-01T00:00:00Z")?,
    );
    let asked = [
        (None, None, None),
        (at, None, None),
        (None, Some(valid), None),
        (at, Some(valid), None),
        (None, None, Some(host)),
    ];
    for (as_of, valid_at, clock) in asked {
        let read = ReadAt {
            as_of,
            valid_at,
            clock,
        };
        let held: &[&str] = match as_of {
            Some(_) => &["s1", "s2", "s3"],
            None => &["s1", "s2", "s3", "s4"],
        };
        let find = Find {
            filter: Some("not(due >= today())".into()),
            at: read,
            ..Find::default()
        };
        let out = l.all_found("slip", find).await?;
        let out: BTreeSet<String> = out.iter().map(|r| r.key.to_string()).collect();
        for (form, rules, name) in [
            ("declared", declared(), "current"),
            ("proposed", proposed("due", "due >= today()"), "due"),
        ] {
            let said = judged(&l, &l.own, keys(held), rules, read).await;
            let said = said.must("a judge at a clock")?;
            let want = (out.iter()).map(|k| (k.clone(), name.to_owned())).collect();
            ensure!(
                broken(&said) == want,
                "at {read:?}, the {form} rule breaks exactly the keys the read rules out, \
                 {out:?}: {said:?}"
            );
        }
    }
    Ok(())
}

async fn a_judge_names_what_no_scope_answers(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    slips(&l).await?;
    let set = judged(
        &l,
        &l.own,
        keys(&["s1"]),
        proposed("x", "in_set('s')"),
        ReadAt::default(),
    );
    let set = set.await;
    ensure!(
        matches!(invalid_case(&set), Some(Case::InSetHere)),
        "`in_set()` in a proposed rule is refused whole `invalid`: {set:?}"
    );
    let rule = || proposed("x", "exists(next)");
    for (what, subject) in [
        ("a stored key", keys(&["s1"])),
        ("a candidate", candidates("slip", &[("s5", 1)])),
    ] {
        let said = judged(&l, &l.own, subject, rule(), ReadAt::default()).await;
        let named = said.as_ref().is_ok_and(|r| {
            r.unjudged.iter().any(|u| {
                matches!(
                    &u.why,
                    UnjudgedWhy::Unevaluable {
                        index: 0,
                        case: Case::ExistsHere
                    }
                )
            }) && broken(r).is_empty()
        });
        let refused = matches!(invalid_case(&said), Some(Case::ExistsHere));
        ensure!(
            named || refused,
            "`exists()` over {what} is named `Unevaluable`, never a pass: {said:?}"
        );
    }
    Ok(())
}

async fn a_judge_answers_no_field(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let both = json!([{"name": "positive", "holds": "n > 0"},
                      {"name": "kept", "holds": "secret > 0", "at_ingest": "admit"}]);
    slips_under(&l, both).await?;
    let fields = [
        ("id", Access::Read),
        ("n", Access::Read),
        ("secret", Access::Masked),
    ];
    let who = field_access(store.as_ref(), "masked", "slip", &fields);
    let all = judged(&l, &who, keys(&["s1"]), declared(), ReadAt::default()).await;
    let all = all.must("a judge of every rule shown")?;
    let text = serde_json::to_string(&all).unwrap_or_default();
    ensure!(
        !text.contains("kept") && !text.contains("4242"),
        "a rule over a masked field is neither judged nor named, and no stored value \
         appears: {text}"
    );
    let names = Rules::Declared {
        names: Some(vec!["kept".into()]),
    };
    let named = judged(&l, &who, keys(&["s1"]), names, ReadAt::default()).await;
    ensure!(
        matches!(named, Err(Error::NotFound { .. })),
        "naming a rule not shown is refused `not_found`: {named:?}"
    );
    let probe = judged(
        &l,
        &who,
        keys(&["s1"]),
        proposed("p", "secret > 0"),
        ReadAt::default(),
    );
    let probe = probe.await;
    ensure!(
        matches!(refused_on(&probe), Some(("secret", Refusal::NotGranted))),
        "a proposed rule reading a masked field is refused `not_granted` on it: {probe:?}"
    );
    Ok(())
}

async fn a_judge_is_bounded(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    slips(&l).await?;
    let max = limits().max_limit as usize;
    let many: Vec<RecordKey> = (0..=max).map(|_| RecordKey::from("s1")).collect();
    let sent: Vec<SourceRecord> = (0..=max)
        .map(|i| {
            source_record(
                "slip",
                &format!("c{i}"),
                json!({"id": format!("c{i}"), "n": 1}),
            )
        })
        .collect();
    for (what, subject) in [
        ("keys", Subject::Keys(many.clone())),
        ("candidates", Subject::Candidates(sent)),
    ] {
        let said = judged(&l, &l.own, subject, declared(), ReadAt::default()).await;
        ensure!(
            matches!(refused_on(&said), Some(("records", Refusal::TooLarge { max: m })) if *m as usize == max),
            "{} {what} is refused `too_large` on `records`: {:?}",
            max + 1,
            said.as_ref().map(|r| r.records.len())
        );
    }
    let whole = judge(&l, Subject::Keys(many[..max].to_vec()), declared()).await?;
    ensure!(
        whole.records.len() == max,
        "{max} keys are judged whole: {} answered",
        whole.records.len()
    );
    Ok(())
}

async fn a_judge_judges_one_type(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    slips(&l).await?;
    l.declare_json(json!({"declare": "type", "name": "memo", "fields": [
        {"name": "n", "type": {"kind": "number"}}]}))
        .await?;
    let absent = judged(
        &l,
        &l.own,
        keys(&["s1", "zz"]),
        declared(),
        ReadAt::default(),
    )
    .await;
    let names = Rules::Declared {
        names: Some(vec!["nope".into()]),
    };
    let rule = judged(&l, &l.own, keys(&["s1"]), names, ReadAt::default()).await;
    ensure!(
        matches!(absent, Err(Error::NotFound { .. }))
            && matches!(
                rule,
                Err(Error::NotFound {
                    missing: Missing::Constraint { .. }
                })
            ),
        "a key the type does not hold and a rule it does not declare are refused \
         `not_found`: {absent:?}, {rule:?}"
    );
    let Subject::Candidates(mut mixed) = candidates("slip", &[("s5", 1)]) else {
        return Err(Failure::new("candidates"));
    };
    mixed.push(source_record("memo", "m1", json!({"n": 1})));
    let said = judged(
        &l,
        &l.own,
        Subject::Candidates(mixed),
        declared(),
        ReadAt::default(),
    );
    let said = said.await;
    let want = Case::CandidateOfAnotherType {
        named: "slip".into(),
        carried: "memo".into(),
    };
    ensure!(
        invalid_case(&said) == Some(&want),
        "a candidate of another type is refused whole as {want:?}: {said:?}"
    );
    Ok(())
}

async fn a_judge_is_no_tool(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    slips(&l).await?;
    let lake = l.describe().await?;
    let named = tools(&lake).map_err(|p| Failure::new(format!("no tools: {p:?}")))?;
    let judging: Vec<_> = (named.iter())
        .filter(|t| t.name.contains("judge"))
        .map(|t| t.name.clone())
        .collect();
    ensure!(judging.is_empty(), "no tool judges: {judging:?}");
    Ok(())
}
