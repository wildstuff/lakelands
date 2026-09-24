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
use lakelands_api::expr::Condition;
use lakelands_api::ontology::{Authority, Measure, StorageKind, TriggerDecl, ViewDecl};
use lakelands_api::seam::{AGING_RULE_TYPE, FeedFilter};
use lakelands_api::write::{AgeReport, Declaration};
use lakelands_api::{
    Catalog, Error, LakeStore, Origin, Principal, Refusal, Timestamp, WriteOrigin,
};
use serde_json::{Value, json};

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, declare, doors, field, head_unmoved, owner, read, type_decl, upsert,
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

/// Writes one aging rule as `who`.
#[allow(clippy::too_many_arguments)]
async fn rule(
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

async fn status(cat: &dyn Catalog, who: &Principal, key: &str) -> Result<Value, Failure> {
    let r = read(cat, who, "invoice", key).await?;
    Ok(r.fields.get("status").cloned().unwrap_or(Value::Null))
}

/// `invoice` with `status` (text) and `amount` (number).
async fn invoices(cat: &dyn Catalog, who: &Principal) -> Result<(), Failure> {
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
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    invoices(cat.as_ref(), &own).await?;
    for (key, status, amount) in [
        ("due", "open", 200),
        ("small", "open", 50),
        ("paid", "paid", 200),
        ("held", "open", 300),
    ] {
        upsert(
            cat.as_ref(),
            &own,
            "invoice",
            key,
            json!({"status": status, "amount": amount}),
        )
        .await?;
    }
    store
        .restrict(&"invoice".into(), &"held".into(), "a retention hold".into())
        .await
        .must("restricting `held`")?;
    rule(
        cat.as_ref(),
        &own,
        "overdue",
        "invoice",
        "status",
        json!("open"),
        json!("overdue"),
        "amount > 100",
    )
    .await?;
    let report = age(store.as_ref(), date(2026, 3, 1)).await?;
    for (key, want) in [
        ("due", "overdue"),
        ("small", "open"),
        ("paid", "paid"),
        ("held", "overdue"),
    ] {
        let got = status(cat.as_ref(), &own, key).await?;
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
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    invoices(cat.as_ref(), &own).await?;
    upsert(
        cat.as_ref(),
        &own,
        "invoice",
        "i1",
        json!({"status": "open", "amount": 200}),
    )
    .await?;
    rule(
        cat.as_ref(),
        &own,
        "overdue",
        "invoice",
        "status",
        json!("open"),
        json!("overdue"),
        "amount > 100",
    )
    .await?;
    let first = age(store.as_ref(), date(2026, 3, 1)).await?;
    ensure!(
        first.transitions == 1,
        "the first run moved {}",
        first.transitions
    );
    // Set back by hand: the day's move is in the lake, so the same date
    // does not move it again.
    upsert(
        cat.as_ref(),
        &own,
        "invoice",
        "i1",
        json!({"status": "open"}),
    )
    .await?;
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
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    invoices(cat.as_ref(), &own).await?;
    upsert(
        cat.as_ref(),
        &own,
        "invoice",
        "i1",
        json!({"status": "open", "due_on": "2020-01-01T00:00:00Z"}),
    )
    .await?;
    rule(
        cat.as_ref(),
        &own,
        "overdue",
        "invoice",
        "status",
        json!("open"),
        json!("overdue"),
        "due_on < today() - 30d",
    )
    .await?;
    // The clock is years past the due date; the date handed in is not.
    let early = age(store.as_ref(), date(2020, 1, 15)).await?;
    ensure!(
        early.transitions == 0 && status(cat.as_ref(), &own, "i1").await? == json!("open"),
        "aged at 2020-01-15, fifteen days after the due date, `i1` moved: `today()` read \
         the clock, not the date handed in"
    );
    let late = age(store.as_ref(), date(2020, 3, 1)).await?;
    ensure!(
        late.transitions == 1 && status(cat.as_ref(), &own, "i1").await? == json!("overdue"),
        "aged at 2020-03-01, sixty days after the due date, `i1` did not move"
    );
    Ok(())
}

async fn a_condition_that_cannot_be_evaluated_leaves_the_record(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    invoices(cat.as_ref(), &own).await?;
    for (key, divisor) in [("a", 1), ("b", 0), ("c", 2)] {
        upsert(
            cat.as_ref(),
            &own,
            "invoice",
            key,
            json!({"status": "open", "amount": 100, "divisor": divisor}),
        )
        .await?;
    }
    rule(
        cat.as_ref(),
        &own,
        "overdue",
        "invoice",
        "status",
        json!("open"),
        json!("overdue"),
        "amount / divisor > 10",
    )
    .await?;
    let report = age(store.as_ref(), date(2026, 3, 1)).await?;
    ensure!(
        status(cat.as_ref(), &own, "b").await? == json!("open"),
        "`b` divides by zero in `when`, and it moved"
    );
    for key in ["a", "c"] {
        ensure!(
            status(cat.as_ref(), &own, key).await? == json!("overdue"),
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
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    invoices(cat.as_ref(), &own).await?;
    cat.declare(
        &own,
        Declaration::Trigger(TriggerDecl {
            name: "went_overdue".into(),
            on: "invoice".into(),
            when: Some(Condition::from("status = 'overdue'")),
            description: None,
        }),
    )
    .await
    .must("declaring the trigger `went_overdue`")?;
    upsert(
        cat.as_ref(),
        &own,
        "invoice",
        "i1",
        json!({"status": "open", "amount": 200}),
    )
    .await?;
    // Written twice, so the rule record's version is 2.
    for threshold in ["1000", "100"] {
        rule(
            cat.as_ref(),
            &own,
            "overdue",
            "invoice",
            "status",
            json!("open"),
            json!("overdue"),
            &format!("amount > {threshold}"),
        )
        .await?;
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
    let meta = read(cat.as_ref(), &own, "invoice", "i1").await?.meta;
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
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let mut total = field("total", StorageKind::Number);
    total.authority = Authority::Derived;
    total.derived = Some(Condition::from("amount * 2"));
    total.depends_on = vec!["amount".into()];
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
    declare(cat.as_ref(), &own, ty).await?;
    let mut counts = type_decl(
        "counts",
        vec![
            field("status", StorageKind::Text),
            field("n", StorageKind::Number),
        ],
    );
    counts.key = vec!["status".into()];
    counts.view = Some(ViewDecl {
        source: "invoice".into(),
        r#where: None,
        group_by: vec!["status".into()],
        measures: vec![Measure {
            name: "n".into(),
            expr: "count()".into(),
        }],
    });
    declare(cat.as_ref(), &own, counts).await?;
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
                &own,
                &AGING_RULE_TYPE.into(),
                &"bad".into(),
                crate::conformance::fixture::patch(fields),
                Default::default(),
            )
            .await
            .must_fail(&format!("writing a rule whose `{at}` does not fit"))?;
        ensure!(
            matches!(
                &e,
                Error::Refused { field: Some(f), reason: Refusal::Invalid { .. } } if f.as_str() == at
            ),
            "a rule whose `{at}` does not fit was refused as {e:?}, and invalid naming `{at}` \
             was expected"
        );
    }
    head_unmoved(store.as_ref(), before, "writing rules that do not fit").await
}

async fn rules_run_in_name_order_one_append_each(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    invoices(cat.as_ref(), &own).await?;
    for key in ["i1", "i2"] {
        upsert(
            cat.as_ref(),
            &own,
            "invoice",
            key,
            json!({"status": "open", "amount": 200}),
        )
        .await?;
    }
    // Written in the other order: the name orders them.
    rule(
        cat.as_ref(),
        &own,
        "b_second",
        "invoice",
        "status",
        json!("overdue"),
        json!("late"),
        "true",
    )
    .await?;
    rule(
        cat.as_ref(),
        &own,
        "a_first",
        "invoice",
        "status",
        json!("open"),
        json!("overdue"),
        "true",
    )
    .await?;
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
            status(cat.as_ref(), &own, key).await? == json!("late"),
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
        matches!(
            &e,
            Error::Refused { field: Some(f), reason: Refusal::Invalid { .. } } if f.as_str() == "today"
        ),
        "aging at noon was refused as {e:?}, and invalid naming `today` was expected"
    );
    head_unmoved(store.as_ref(), before, "aging at noon").await
}

async fn a_rule_that_no_longer_fits_is_skipped_and_named(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    invoices(cat.as_ref(), &own).await?;
    declare(
        cat.as_ref(),
        &own,
        type_decl("ticket", vec![field("state", StorageKind::Text)]),
    )
    .await?;
    upsert(
        cat.as_ref(),
        &own,
        "invoice",
        "i1",
        json!({"status": "open"}),
    )
    .await?;
    upsert(cat.as_ref(), &own, "ticket", "t1", json!({"state": "new"})).await?;
    rule(
        cat.as_ref(),
        &own,
        "a_tickets",
        "ticket",
        "state",
        json!("new"),
        json!("stale"),
        "true",
    )
    .await?;
    rule(
        cat.as_ref(),
        &own,
        "b_invoices",
        "invoice",
        "status",
        json!("open"),
        json!("overdue"),
        "true",
    )
    .await?;
    // `state` becomes the source's: the rule written for it no longer fits.
    let mut state = field("state", StorageKind::Text);
    state.authority = Authority::Source;
    declare(cat.as_ref(), &own, type_decl("ticket", vec![state])).await?;
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
        status(cat.as_ref(), &own, "i1").await? == json!("overdue"),
        "the rule `b_invoices` fits, and did not run beside the skipped one"
    );
    let t1 = read(cat.as_ref(), &own, "ticket", "t1").await?;
    ensure!(
        t1.fields.get("state") == Some(&json!("new")),
        "the skipped rule moved `t1`"
    );
    Ok(())
}

/// A clerk who writes rules and, per type, the given field accesses.
fn clerk(store: &dyn LakeStore, id: &str, invoice: &[(&str, lakelands_api::Access)]) -> Principal {
    use lakelands_api::principal::TypeGrants;
    use lakelands_api::{Access, Grants, PrincipalKind};
    let mut grants = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    grants.types.insert(
        AGING_RULE_TYPE.into(),
        TypeGrants {
            default: Some(Access::Write),
            fields: Default::default(),
        },
    );
    grants.types.insert(
        "invoice".into(),
        TypeGrants {
            default: None,
            fields: invoice.iter().map(|(f, a)| ((*f).into(), *a)).collect(),
        },
    );
    crate::conformance::fixture::principal(store, PrincipalKind::Agent, id, grants)
}

async fn a_rule_needs_write_on_its_field_and_read_on_what_when_reads(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    use lakelands_api::Access;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let ty = type_decl(
        "invoice",
        vec![
            field("status", StorageKind::Text),
            field("amount", StorageKind::Number),
            field("score", StorageKind::Number),
        ],
    );
    declare(cat.as_ref(), &own, ty).await?;
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
            matches!(
                &e,
                Error::Refused { field: Some(f), reason: Refusal::NotGranted } if f.as_str() == at
            ),
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
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(
        cat.as_ref(),
        &own,
        type_decl("partner", vec![field("name", StorageKind::Text)]),
    )
    .await?;
    let mut owner_field = field("owner", StorageKind::Reference);
    owner_field.ty.target = Some("partner".into());
    declare(
        cat.as_ref(),
        &own,
        type_decl(
            "ticket",
            vec![owner_field, field("state", StorageKind::Text)],
        ),
    )
    .await?;
    for p in ["p1", "p2"] {
        upsert(cat.as_ref(), &own, "partner", p, json!({"name": p})).await?;
    }
    upsert(
        cat.as_ref(),
        &own,
        "ticket",
        "t1",
        json!({"owner": "p1", "state": "new"}),
    )
    .await?;
    rule(
        cat.as_ref(),
        &own,
        "a_reassign",
        "ticket",
        "owner",
        json!("p1"),
        json!("p2"),
        "true",
    )
    .await?;
    rule(
        cat.as_ref(),
        &own,
        "b_stale",
        "ticket",
        "state",
        json!("new"),
        json!("stale"),
        "true",
    )
    .await?;
    // `p2` retired after the rule was written: moving `owner` to it would
    // refer to a record no longer live.
    cat.retire(&own, &"partner".into(), &"p2".into(), Default::default())
        .await
        .must("retiring `p2`")?;
    let report = age(store.as_ref(), date(2026, 3, 1)).await?;
    let t1 = read(cat.as_ref(), &own, "ticket", "t1").await?;
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
