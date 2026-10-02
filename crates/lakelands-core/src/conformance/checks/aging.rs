//! Aging runs by date, once (`docs/decisions/036-aging-runs-by-date-once.md`,
//! `docs/seams.md` §`SystemVocabulary`).
//!
//! A `sys.aging_rule` record moves a field of every record of a type from
//! one value to another where its condition holds on the date `age` is
//! handed. The moves are ordinary writes by the system, one append per
//! rule, each with an id made from the rule, the record and the date, so
//! aging a date twice writes it once.

use std::sync::Arc;

use lakelands_api::event::{EventBody, Procedure};
use lakelands_api::ontology::StorageKind;
use lakelands_api::seam::{AGING_RULE_TYPE, FeedFilter};
use lakelands_api::write::{AgeReport, Declaration};
use lakelands_api::{Catalog, LakeStore, Origin, Principal, Refusal, Timestamp, WriteOrigin};
use serde_json::{Value, json};

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, count_view, declare, derived, field, field_access, head_unmoved, read, refused_on,
    sourced, trigger_on, type_decl, upsert,
};

check!(
    /// A rule moves only what holds `from` and meets `when`.
    MOVES_WHAT_HOLDS_FROM = a_rule_moves_only_the_records_holding_from_whose_condition_holds,
    "036: `age` moves every record of the rule's type, restricted ones included, whose field holds `from` and whose `when` holds, and no other"
);

check!(
    /// A date aged twice is written once.
    TWICE_IS_ONCE = aging_a_date_twice_appends_nothing_the_second_time,
    "036 and log-format §3: every move has an id from the rule, the record and the date, and a second `age` for the same date appends nothing"
);

check!(
    /// `today()` is the date handed in.
    TODAY_IS_THE_DATE = today_in_a_rule_is_the_date_aged_not_the_clock,
    "036: inside a rule's condition `today()` is the date `age` was handed, never the clock of the process running it"
);

check!(
    /// A condition that cannot be evaluated leaves the record.
    FAILED_GUARD_LEAVES = a_condition_that_cannot_be_evaluated_leaves_the_record,
    "036: a `when` that cannot be evaluated for a record leaves it as it is, is counted as a failed guard, and the run goes on"
);

check!(
    /// A move is the system's, by its rule, and reacts like any write.
    MOVE_PROVENANCE = a_move_is_written_by_the_system_as_its_rule,
    "036 and seams: a move is a `record_upserted` of origin `system` by `system:lakelands`, its procedure `aging:<rule>` at the rule record's version, scheduled, with no cause; a trigger reacts in the same append"
);

check!(
    /// A rule that does not fit is refused when it is written.
    RULE_CHECKED = a_rule_that_does_not_fit_is_refused_naming_its_field,
    "seams: a rule is refused as invalid, naming the rule field at fault, when `on` names no type, a `sys.` type or a view, `field` is derived or part of the key, `from` is no value of the field, `to` equals `from`, or `when` reads `previous`"
);

check!(
    /// Rules run in name order, one append each.
    NAME_ORDER = rules_run_in_name_order_one_append_each,
    "seams: `age` runs the rules in rule-name order, each rule's moves one append, and reports them in that order"
);

check!(
    /// `today` is a date.
    TODAY_AT_MIDNIGHT = age_at_a_time_other_than_midnight_is_refused,
    "seams: `age` is handed a date, at UTC midnight; any other time is refused as invalid, naming `today`, and appends nothing"
);

check!(
    /// A rule that no longer fits is skipped and named.
    NO_LONGER_FITS = a_rule_that_no_longer_fits_is_skipped_and_named,
    "seams: a rule the lake's shape no longer fits is skipped, named in the report with its reason, while the other rules run"
);

check!(
    /// A rule carries its author's rights.
    RULE_AUTHORITY = a_rule_needs_write_on_its_field_and_read_on_what_when_reads,
    "036 and 024: a rule is refused as not granted, naming the field, unless its author may write the field it moves and read every field its `when` reads; the run carries that authority"
);

check!(
    /// A move the store refuses leaves its record.
    REFUSED_MOVE_LEAVES = a_move_the_store_would_refuse_leaves_the_record,
    "036: a move the store would refuse for a record (its `to` a reference no longer live) leaves the record, is counted as a failed guard, and the other rules run"
);

fn date(y: i64, m: u32, d: u32) -> Timestamp {
    Timestamp::from_date(y, m, d).expect("a date of the calendar")
}

/// Writes one aging rule as the owner: on `target` (`type.field`), from
/// `from` to `to` when `when` holds.
async fn rule(
    l: &Lake,
    name: &str,
    target: &str,
    from: &str,
    to: &str,
    when: &str,
) -> Result<(), Failure> {
    let (on, field) = target
        .split_once('.')
        .expect("a rule's target is type.field");
    rule_as(
        l.cat.as_ref(),
        &l.own,
        name,
        on,
        field,
        json!(from),
        json!(to),
        when,
    )
    .await
}

/// Writes the rule `name` that moves an open invoice to overdue when
/// `when` holds.
async fn overdue(l: &Lake, name: &str, when: &str) -> Result<(), Failure> {
    rule(l, name, "invoice.status", "open", "overdue", when).await
}

/// Writes one aging rule as `who`.
#[allow(clippy::too_many_arguments)]
async fn rule_as(
    cat: &dyn Catalog,
    who: &Principal,
    name: &str,
    on: &str,
    field: &str,
    from: Value,
    to: Value,
    when: &str,
) -> Result<(), Failure> {
    upsert(
        cat,
        who,
        AGING_RULE_TYPE,
        name,
        json!({"on": on, "field": field, "from": from, "to": to, "when": when}),
    )
    .await?;
    Ok(())
}

async fn age(store: &dyn LakeStore, today: Timestamp) -> Result<AgeReport, Failure> {
    store.age(today).await.must(&format!("aging {today}"))
}

async fn status(l: &Lake, key: &str) -> Result<Value, Failure> {
    let (cat, who) = (l.cat.as_ref(), &l.own);
    let r = read(cat, who, "invoice", key).await?;
    Ok(r.fields.get("status").cloned().unwrap_or(Value::Null))
}

/// `invoice` with `status` (text) and `amount` (number).
async fn invoices(l: &Lake) -> Result<(), Failure> {
    let (cat, who) = (l.cat.as_ref(), &l.own);
    declare(
        cat,
        who,
        type_decl(
            "invoice",
            vec![
                field("status", StorageKind::Text),
                field("amount", StorageKind::Number),
                field("divisor", StorageKind::Number),
                field("due_on", StorageKind::DateTime),
            ],
        ),
    )
    .await?;
    Ok(())
}

async fn a_rule_moves_only_the_records_holding_from_whose_condition_holds(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    invoices(&l).await?;
    for (key, status, amount) in [
        ("due", "open", 200),
        ("small", "open", 50),
        ("paid", "paid", 200),
        ("held", "open", 300),
    ] {
        l.upsert("invoice", key, json!({"status": status, "amount": amount}))
            .await?;
    }
    store
        .restrict(&"invoice".into(), &"held".into(), "a retention hold".into())
        .await
        .must("restricting `held`")?;
    overdue(&l, "overdue", "amount > 100").await?;
    let report = age(store.as_ref(), date(2026, 3, 1)).await?;
    for (key, want) in [
        ("due", "overdue"),
        ("small", "open"),
        ("paid", "paid"),
        ("held", "overdue"),
    ] {
        let got = status(&l, key).await?;
        ensure!(
            got == json!(want),
            "after aging, `{key}` holds status {got}, and {want:?} was expected: a rule moves \
             exactly the records holding `from` whose `when` holds, restricted ones included"
        );
    }
    ensure!(
        report.transitions == 2 && report.rules.len() == 1 && report.rules[0].moved == 2,
        "two records moved, and the report says {report:?}"
    );
    let head = store.head().await.must("reading the head")?;
    ensure!(
        report.position == head,
        "the report names position {}, and the head is {head}",
        report.position
    );
    Ok(())
}

async fn aging_a_date_twice_appends_nothing_the_second_time(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    invoices(&l).await?;
    l.upsert("invoice", "i1", json!({"status": "open", "amount": 200}))
        .await?;
    overdue(&l, "overdue", "amount > 100").await?;
    let first = age(store.as_ref(), date(2026, 3, 1)).await?;
    ensure!(
        first.transitions == 1,
        "the first run moved {}",
        first.transitions
    );
    // Set back by hand: the day's move is in the lake, so the same date
    // does not move it again.
    l.upsert("invoice", "i1", json!({"status": "open"})).await?;
    let before = store.head().await.must("reading the head")?;
    let second = age(store.as_ref(), date(2026, 3, 1)).await?;
    ensure!(
        second.transitions == 0,
        "aging the same date again moved {} record(s)",
        second.transitions
    );
    head_unmoved(store.as_ref(), before, "aging a date a second time").await?;
    ensure!(
        second.position == before,
        "a run that appends nothing names the head, {before}, and named {}",
        second.position
    );
    let next = age(store.as_ref(), date(2026, 3, 2)).await?;
    ensure!(
        next.transitions == 1,
        "the next date is a new day and moves `i1` again; it moved {}",
        next.transitions
    );
    Ok(())
}

async fn today_in_a_rule_is_the_date_aged_not_the_clock(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    invoices(&l).await?;
    l.upsert(
        "invoice",
        "i1",
        json!({"status": "open", "due_on": "2020-01-01T00:00:00Z"}),
    )
    .await?;
    overdue(&l, "overdue", "due_on < today() - 30d").await?;
    // The clock is years past the due date; the date handed in is not.
    let early = age(store.as_ref(), date(2020, 1, 15)).await?;
    ensure!(
        early.transitions == 0 && status(&l, "i1").await? == json!("open"),
        "aged at 2020-01-15, fifteen days after the due date, `i1` moved: `today()` read \
         the clock, not the date handed in"
    );
    let late = age(store.as_ref(), date(2020, 3, 1)).await?;
    ensure!(
        late.transitions == 1 && status(&l, "i1").await? == json!("overdue"),
        "aged at 2020-03-01, sixty days after the due date, `i1` did not move"
    );
    Ok(())
}

async fn a_condition_that_cannot_be_evaluated_leaves_the_record(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    invoices(&l).await?;
    for (key, divisor) in [("a", 1), ("b", 0), ("c", 2)] {
        l.upsert(
            "invoice",
            key,
            json!({"status": "open", "amount": 100, "divisor": divisor}),
        )
        .await?;
    }
    overdue(&l, "overdue", "amount / divisor > 10").await?;
    let report = age(store.as_ref(), date(2026, 3, 1)).await?;
    ensure!(
        status(&l, "b").await? == json!("open"),
        "`b` divides by zero in `when`, and it moved"
    );
    for key in ["a", "c"] {
        ensure!(
            status(&l, key).await? == json!("overdue"),
            "`{key}`'s condition holds, and it did not move: a failed guard stopped the run"
        );
    }
    ensure!(
        report.failed_guards == 1 && report.rules[0].failed_guards == 1 && report.transitions == 2,
        "one guard failed and two records moved, and the report says {report:?}"
    );
    Ok(())
}

async fn a_move_is_written_by_the_system_as_its_rule(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    invoices(&l).await?;
    cat.declare(
        own,
        trigger_on("went_overdue", "invoice", Some("status = 'overdue'")),
    )
    .await
    .must("declaring the trigger `went_overdue`")?;
    l.upsert("invoice", "i1", json!({"status": "open", "amount": 200}))
        .await?;
    // Written twice, so the rule record's version is 2.
    for threshold in ["1000", "100"] {
        overdue(&l, "overdue", &format!("amount > {threshold}")).await?;
    }
    let before = store.head().await.must("reading the head")?;
    age(store.as_ref(), date(2026, 3, 1)).await?;
    let events = store
        .feed()
        .poll(before.next(), &FeedFilter::default(), 100)
        .await
        .must("reading the feed after the rule")?
        .events;
    let moved = events
        .iter()
        .find(|e| matches!(e.body, EventBody::RecordUpserted { .. }))
        .ok_or_else(|| Failure::new("aging appended no `record_upserted`"))?;
    let want = Procedure {
        name: "aging:overdue".into(),
        version: "2".into(),
    };
    ensure!(
        moved.origin == Origin::System
            && moved.writer == "system:lakelands"
            && moved.procedure.as_ref() == Some(&want)
            && moved.context.origin == WriteOrigin::Scheduled
            && moved.cause.is_none(),
        "the move's envelope is origin {:?}, writer {}, procedure {:?}, context {:?}, cause {:?}",
        moved.origin,
        moved.writer,
        moved.procedure,
        moved.context,
        moved.cause
    );
    let id = crate::log::aging_id("overdue", &"invoice".into(), &"i1".into(), date(2026, 3, 1));
    ensure!(
        moved.id == id,
        "the move's id is {:?}, and log-format §3 gives {id:?}",
        moved.id
    );
    let meta = l.read("invoice", "i1").await?.meta;
    let m = meta
        .get("status")
        .ok_or_else(|| Failure::new("the moved field has no provenance"))?;
    ensure!(
        m.origin == Origin::System
            && m.writer == "system:lakelands"
            && m.procedure_version.as_deref() == Some("2"),
        "the moved field's provenance is {m:?}"
    );
    let fired = events.iter().any(|e| {
        e.cause == Some(moved.position)
            && matches!(&e.body, EventBody::TriggerFired { trigger, .. } if trigger.as_str() == "went_overdue")
    });
    ensure!(
        fired,
        "the move made `status = 'overdue'` hold, and `went_overdue` did not fire in the same \
         append, caused by it"
    );
    Ok(())
}

async fn a_rule_that_does_not_fit_is_refused_naming_its_field(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let total = derived("total", StorageKind::Number, "amount * 2", &["amount"]);
    let mut ty = type_decl(
        "invoice",
        vec![
            field("number", StorageKind::Text),
            field("status", StorageKind::Text),
            field("amount", StorageKind::Number),
        ],
    );
    ty.fields.push(total);
    ty.key = vec!["number".into()];
    l.declare(ty).await?;
    l.declare(count_view("counts", "invoice", "status")).await?;
    let before = store.head().await.must("reading the head")?;
    let cases = [
        (
            "on",
            json!({"on": "nothing", "field": "status", "from": "open", "to": "overdue", "when": "true"}),
        ),
        (
            "field",
            json!({"on": "invoice", "field": "total", "from": 1, "to": 2, "when": "true"}),
        ),
        (
            "from",
            json!({"on": "invoice", "field": "amount", "from": "open", "to": 2, "when": "true"}),
        ),
        (
            "when",
            json!({"on": "invoice", "field": "status", "from": "open", "to": "overdue", "when": "previous(status) = 'open'"}),
        ),
        (
            "field",
            json!({"on": "invoice", "field": "number", "from": "a", "to": "b", "when": "true"}),
        ),
        (
            "to",
            json!({"on": "invoice", "field": "amount", "from": 1, "to": 1.0, "when": "true"}),
        ),
        (
            "on",
            json!({"on": "counts", "field": "n", "from": 1, "to": 2, "when": "true"}),
        ),
        (
            "on",
            json!({"on": "sys.setting", "field": "value", "from": 1, "to": 2, "when": "true"}),
        ),
    ];
    for (at, fields) in cases {
        let e = cat
            .upsert(
                own,
                &AGING_RULE_TYPE.into(),
                &"bad".into(),
                crate::conformance::fixture::patch(fields),
                Default::default(),
            )
            .await
            .must_fail(&format!("writing a rule whose `{at}` does not fit"))?;
        ensure!(
            matches!(refused_on(&e), Some((f, Refusal::Invalid { .. })) if f == at),
            "a rule whose `{at}` does not fit was refused as {e:?}, and invalid naming `{at}` \
             was expected"
        );
    }
    head_unmoved(store.as_ref(), before, "writing rules that do not fit").await
}

async fn rules_run_in_name_order_one_append_each(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    invoices(&l).await?;
    for key in ["i1", "i2"] {
        l.upsert("invoice", key, json!({"status": "open", "amount": 200}))
            .await?;
    }
    // Written in the other order: the name orders them.
    rule(&l, "b_second", "invoice.status", "overdue", "late", "true").await?;
    overdue(&l, "a_first", "true").await?;
    let before = store.head().await.must("reading the head")?;
    let report = age(store.as_ref(), date(2026, 3, 1)).await?;
    let names: Vec<&str> = report.rules.iter().map(|r| r.rule.as_str()).collect();
    ensure!(
        names == ["a_first", "b_second"],
        "the report names the rules {names:?}, and name order was expected"
    );
    let events = store
        .feed()
        .poll(before.next(), &FeedFilter::default(), 100)
        .await
        .must("reading the feed after the rules")?
        .events;
    let procedures: Vec<&str> = events
        .iter()
        .filter_map(|e| e.procedure.as_ref().map(|p| p.name.as_str()))
        .collect();
    ensure!(
        procedures
            == [
                "aging:a_first",
                "aging:a_first",
                "aging:b_second",
                "aging:b_second"
            ],
        "the moves are {procedures:?}: `a_first` runs first, one append of its two moves, and \
         `b_second` then moves what `a_first` made overdue"
    );
    for key in ["i1", "i2"] {
        ensure!(
            status(&l, key).await? == json!("late"),
            "`{key}` did not reach `late` through both rules in name order"
        );
    }
    Ok(())
}

async fn age_at_a_time_other_than_midnight_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let before = store.head().await.must("reading the head")?;
    let noon = Timestamp(date(2026, 3, 1).0 + 12 * 3_600 * 1_000_000);
    let e = store.age(noon).await.must_fail("aging at noon")?;
    ensure!(
        matches!(refused_on(&e), Some(("today", Refusal::Invalid { .. }))),
        "aging at noon was refused as {e:?}, and invalid naming `today` was expected"
    );
    head_unmoved(store.as_ref(), before, "aging at noon").await
}

async fn a_rule_that_no_longer_fits_is_skipped_and_named(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    invoices(&l).await?;
    l.declare(type_decl("ticket", vec![field("state", StorageKind::Text)]))
        .await?;
    l.upsert("invoice", "i1", json!({"status": "open"})).await?;
    l.upsert("ticket", "t1", json!({"state": "new"})).await?;
    rule(&l, "a_tickets", "ticket.state", "new", "stale", "true").await?;
    overdue(&l, "b_invoices", "true").await?;
    // `state` becomes the source's: the rule written for it no longer fits.
    let state = sourced("state", StorageKind::Text);
    l.declare(type_decl("ticket", vec![state])).await?;
    let report = age(store.as_ref(), date(2026, 3, 1)).await?;
    let skipped = report
        .rules
        .iter()
        .find(|r| r.rule == "a_tickets")
        .and_then(|r| r.skipped.clone());
    ensure!(
        skipped.as_deref().is_some_and(|s| s.contains("field")),
        "the rule `a_tickets` no longer fits its field, and the report says {report:?}; it \
         is skipped, naming why"
    );
    ensure!(
        status(&l, "i1").await? == json!("overdue"),
        "the rule `b_invoices` fits, and did not run beside the skipped one"
    );
    let t1 = l.read("ticket", "t1").await?;
    ensure!(
        t1.fields.get("state") == Some(&json!("new")),
        "the skipped rule moved `t1`"
    );
    Ok(())
}

/// A clerk who writes rules and, per type, the given field accesses.
fn clerk(store: &dyn LakeStore, id: &str, invoice: &[(&str, lakelands_api::Access)]) -> Principal {
    use lakelands_api::principal::TypeGrants;
    let mut clerk = field_access(store, id, "invoice", invoice);
    let rules = TypeGrants {
        default: Some(lakelands_api::Access::Write),
        fields: Default::default(),
    };
    clerk.grants.types.insert(AGING_RULE_TYPE.into(), rules);
    clerk
}

async fn a_rule_needs_write_on_its_field_and_read_on_what_when_reads(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    use lakelands_api::Access;
    let l = Lake::of(store.as_ref());
    let Lake { cat, .. } = &l;
    let ty = type_decl(
        "invoice",
        vec![
            field("status", StorageKind::Text),
            field("amount", StorageKind::Number),
            field("score", StorageKind::Number),
        ],
    );
    l.declare(ty).await?;
    let fields = |when: &str| {
        crate::conformance::fixture::patch(json!({"on": "invoice", "field": "status",
            "from": "open", "to": "overdue", "when": when}))
    };
    let before = store.head().await.must("reading the head")?;
    let cases = [
        // Reads everything, may not write `status`.
        (
            clerk(store.as_ref(), "reader", &[]),
            "amount > 100",
            "status",
        ),
        // Writes `status`, may not read `score`, which `when` probes.
        (
            clerk(
                store.as_ref(),
                "blind",
                &[("status", Access::Write), ("score", Access::None)],
            ),
            "score > 700",
            "score",
        ),
    ];
    for (who, when, at) in cases {
        let e = cat
            .upsert(
                &who,
                &AGING_RULE_TYPE.into(),
                &"r".into(),
                fields(when),
                Default::default(),
            )
            .await
            .must_fail(&format!("writing a rule as `{}`", who.writer()))?;
        ensure!(
            matches!(refused_on(&e), Some((f, Refusal::NotGranted)) if f == at),
            "`{}` wrote a rule over `{at}` and was refused as {e:?}; not granted, naming \
             `{at}`, was expected",
            who.writer()
        );
    }
    head_unmoved(
        store.as_ref(),
        before,
        "writing rules beyond the author's rights",
    )
    .await?;
    let able = clerk(store.as_ref(), "able", &[("status", Access::Write)]);
    cat.upsert(
        &able,
        &AGING_RULE_TYPE.into(),
        &"r".into(),
        fields("score > 700"),
        Default::default(),
    )
    .await
    .must("writing a rule as a clerk who may write `status` and read `score`")?;
    Ok(())
}

async fn a_move_the_store_would_refuse_leaves_the_record(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("partner", vec![field("name", StorageKind::Text)]))
        .await?;
    let mut owner_field = field("owner", StorageKind::Reference);
    owner_field.ty.target = Some("partner".into());
    l.declare(type_decl(
        "ticket",
        vec![owner_field, field("state", StorageKind::Text)],
    ))
    .await?;
    for p in ["p1", "p2"] {
        l.upsert("partner", p, json!({"name": p})).await?;
    }
    l.upsert("ticket", "t1", json!({"owner": "p1", "state": "new"}))
        .await?;
    rule(&l, "a_reassign", "ticket.owner", "p1", "p2", "true").await?;
    rule(&l, "b_stale", "ticket.state", "new", "stale", "true").await?;
    // `p2` retired after the rule was written: moving `owner` to it would
    // refer to a record no longer live.
    l.retire("partner", "p2").await?;
    let report = age(store.as_ref(), date(2026, 3, 1)).await?;
    let t1 = l.read("ticket", "t1").await?;
    ensure!(
        t1.fields.get("owner") == Some(&json!("p1")),
        "`t1` was moved to the retired `p2`: {:?}",
        t1.fields
    );
    ensure!(
        report.rules[0].failed_guards == 1 && report.rules[0].moved == 0,
        "the refused move is counted as a failed guard, and the report says {report:?}"
    );
    ensure!(
        t1.fields.get("state") == Some(&json!("stale")),
        "the rule after the refused move did not run: {:?}",
        t1.fields
    );
    Ok(())
}

check!(
    /// A rule under a retention hold is live and runs.
    HELD_RULE_RUNS = a_rule_under_a_retention_hold_is_live_and_runs,
    "036 and seams, SystemVocabulary: a hold withholds a record from reads, it does not stop it; a restricted rule record runs"
);

async fn a_rule_under_a_retention_hold_is_live_and_runs(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    invoices(&l).await?;
    l.upsert("invoice", "i1", json!({"status": "open", "amount": 200}))
        .await?;
    overdue(&l, "held_rule", "amount > 100").await?;
    store
        .restrict(
            &AGING_RULE_TYPE.into(),
            &"held_rule".into(),
            "a retention hold".into(),
        )
        .await
        .must("restricting the rule record")?;
    let report = age(store.as_ref(), date(2026, 3, 1)).await?;
    ensure!(
        status(&l, "i1").await? == json!("overdue")
            && report
                .rules
                .iter()
                .any(|r| r.rule == "held_rule" && r.moved == 1),
        "the rule record is under a retention hold, and a hold does not stop a record: \
         the rule runs and moves `i1`; the report says {report:?}"
    );
    Ok(())
}

check!(
    /// `setting()` in a rule reads the setting at the run, and a replay
    /// reads the moves.
    SETTING_IN_WHEN = a_rule_reads_the_setting_at_its_run_and_a_replay_reads_its_moves,
    "seams, SystemVocabulary: setting() in `when` reads the lake's settings as a filter does, with no grant on sys.setting asked of the rule's author, as of the head at the run; the move is an event, so a replay never evaluates `when` again"
);

async fn a_rule_reads_the_setting_at_its_run_and_a_replay_reads_its_moves(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    use lakelands_api::Access;
    use lakelands_api::principal::TypeGrants;
    use lakelands_api::seam::SETTING_TYPE;
    let l = Lake::of(store.as_ref());
    let Lake { cat, .. } = &l;
    invoices(&l).await?;
    for (key, amount) in [("big", 200), ("small", 50)] {
        l.upsert("invoice", key, json!({"status": "open", "amount": amount}))
            .await?;
    }
    // When the rule is written, `limit` is 1000: nothing would move.
    l.upsert(
        SETTING_TYPE,
        "limit",
        json!({"key": "limit", "value": 1000}),
    )
    .await?;
    // The rule's author may write `status` and read `amount`, and holds no
    // grant on the lake's settings at all.
    let mut author = clerk(
        store.as_ref(),
        "author",
        &[("status", Access::Write), ("amount", Access::Read)],
    );
    author.grants.types.insert(
        SETTING_TYPE.into(),
        TypeGrants {
            default: Some(Access::None),
            fields: Default::default(),
        },
    );
    rule_as(
        cat.as_ref(),
        &author,
        "over_limit",
        "invoice",
        "status",
        json!("open"),
        json!("overdue"),
        "amount > setting('limit')",
    )
    .await?;
    // Between the rule and the run, `limit` falls to 100: the run takes the
    // setting as it stands at the head when it runs.
    l.upsert(SETTING_TYPE, "limit", json!({"key": "limit", "value": 100}))
        .await?;
    age(store.as_ref(), date(2026, 3, 1)).await?;
    let (big, small) = (status(&l, "big").await?, status(&l, "small").await?);
    ensure!(
        big == json!("overdue") && small == json!("open"),
        "`limit` was 1000 when the rule was written and 100 when the lake aged: the run \
         takes 100, so `big` (200) moves and `small` (50) does not; they hold {big} and \
         {small}"
    );
    // The setting changes afterwards; the moves are events, so a rebuild
    // reads them as they were and never evaluates `when` again.
    l.upsert(
        SETTING_TYPE,
        "limit",
        json!({"key": "limit", "value": 1000}),
    )
    .await?;
    store.rebuild().await.must("rebuilding the projection")?;
    let after = status(&l, "big").await?;
    ensure!(
        after == json!("overdue"),
        "`limit` rose to 1000 after the run; a rebuild replays the move and does not \
         evaluate `when` again, so `big` stays overdue; it holds {after}"
    );
    Ok(())
}

check!(
    /// A rule the shape left behind is skipped, for each way it can be.
    SKIPPED_EVERY_WAY = a_rule_is_skipped_when_its_field_is_dropped_or_its_value_no_longer_admitted,
    "seams, SystemVocabulary, and 027 extended: a rule whose field a migration dropped, or whose `from` or `to` a tightened value type no longer admits, is skipped and named, never run half"
);

async fn a_rule_is_skipped_when_its_field_is_dropped_or_its_value_no_longer_admitted(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    use lakelands_api::ontology::ValueTypeDecl;
    use lakelands_api::write::Migration;
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let stage_type = |one_of: &[&str]| -> Result<ValueTypeDecl, Failure> {
        serde_json::from_value(json!({"name": "stage", "kind": "text", "one_of": one_of}))
            .map_err(|e| Failure::new(format!("the check's value type does not read: {e}")))
    };
    cat.declare(
        own,
        Declaration::ValueType(stage_type(&["open", "late", "closed"])?),
    )
    .await
    .must("declaring the value type `stage`")?;
    let mut phase = field("phase", StorageKind::Text);
    phase.ty = lakelands_api::ontology::FieldType {
        value_type: Some("stage".into()),
        ..lakelands_api::ontology::FieldType::kind(StorageKind::Text)
    };
    let with_note = type_decl(
        "case",
        vec![phase.clone(), field("note", StorageKind::Text)],
    );
    l.declare(with_note).await?;
    l.upsert("case", "c1", json!({"phase": "open"})).await?;
    rule(&l, "a_note", "case.note", "draft", "final", "true").await?;
    rule(&l, "b_late", "case.phase", "open", "late", "true").await?;

    // `note` holds no value, so dropping it is lossless.
    let m = Migration {
        to: type_decl("case", vec![phase]),
        renames: Vec::new(),
    };
    let report = cat
        .migrate(own, m.clone())
        .await
        .must("the preflight of dropping `note`")?
        .data;
    cat.migrate_apply(own, m, &report.report_id)
        .await
        .must("dropping `note`")?;
    // `late` leaves `stage`; every stored value (`open`) still fits.
    let tighter = stage_type(&["open", "closed"])?;
    let vt = cat
        .migrate_value_type(own, tighter.clone())
        .await
        .must("the preflight of tightening `stage`")?
        .data;
    cat.migrate_value_type_apply(own, tighter, &vt.report_id)
        .await
        .must("tightening `stage`")?;

    let report = age(store.as_ref(), date(2026, 3, 1)).await?;
    let skipped = |name: &str| {
        report
            .rules
            .iter()
            .find(|r| r.rule == name)
            .and_then(|r| r.skipped.clone())
    };
    ensure!(
        skipped("a_note").is_some_and(|s| s.contains("note"))
            && skipped("b_late").is_some_and(|s| s.contains("late") || s.contains("to")),
        "`note` was dropped and `late` left `stage`: both rules are skipped and named, \
         each with why; the report says {report:?}"
    );
    let c1 = l.read("case", "c1").await?;
    ensure!(
        c1.fields.get("phase") == Some(&json!("open")),
        "a skipped rule moves nothing: `c1` holds {:?}",
        c1.fields.get("phase")
    );
    Ok(())
}
