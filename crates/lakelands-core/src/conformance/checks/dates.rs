//! The kind `date`: a calendar date, the instant at its UTC midnight kept
//! as `YYYY-MM-DD` (`docs/decisions/011-storage-kind-and-value-type.md`,
//! extended; `docs/log-format.md` §1.3).

use std::sync::Arc;

use lakelands_api::error::Case;
use lakelands_api::ontology::StorageKind::{Date, DateTime, Number, Text};
use lakelands_api::read::{Direction, OrderBy};
use lakelands_api::{Find, LakeStore, Params, Refusal, Value};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, bound, derived, describe_type, field, find_where, migration, order, param,
    reference, refused_on, try_upsert, type_decl, view_of,
};

check!(
    /// A date is written and read as `YYYY-MM-DD`, and compares as its midnight.
    DATE_KIND = a_date_is_read_as_its_day_and_compares_as_its_midnight,
    "011, extended (lakelands#702): a `date` field takes a date or an instant at a UTC midnight and reads `YYYY-MM-DD`; any other instant, an offset midnight too, is refused naming the field; a key made from a date is the date, and a write by it is admitted; a view grouped by a date is keyed by it; a date orders and equals as its midnight against a `date_time`, ties with one at that midnight, has dates for its `min` and `max`, compares with `today()`, buckets by `date_bucket` and subtracts to a duration; a derived date at a midnight lands and one off it holds no value; a view's parameter may be a date; `describe` says `date`"
);

check!(
    /// A migration converts to and from a date without loss.
    DATE_MIGRATES = a_migration_converts_dates_without_loss,
    "027 and 011, extended (lakelands#702): `date_time` to `date` is lossless over midnights and refused with its report, naming the record, over one that is not, as `text` to `date` is over a midnight at a non-zero offset; `text` to `date`, `date` to `date_time` at its midnight and `date` to `text` as written; a key field migrated to `date` re-keys its type, and a reference to a record is rewritten to its new key"
);

/// The invoices of both checks' rows: `due` a date, `at` an instant,
/// `issued` a date, `net` derived 30 days on and `odd` an hour on.
fn inv() -> lakelands_api::ontology::TypeDecl {
    type_decl(
        "inv",
        vec![
            field("due", Date),
            field("at", DateTime),
            field("issued", Date),
            derived("net", Date, "issued + 30d", &["issued"]),
            derived("odd", Date, "issued + 1h", &["issued"]),
        ],
    )
}

async fn a_date_is_read_as_its_day_and_compares_as_its_midnight(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    l.declare(inv()).await?;
    let mut rate = type_decl("rate", vec![field("cur", Text), field("day", Date)]);
    rate.key = vec!["cur".into(), "day".into()];
    l.declare(rate).await?;
    let mut wrong = Vec::new();
    // Written as a date and as instants at a UTC midnight, read as the date.
    for (key, due) in [
        ("a", "2026-07-05"),
        ("b", "2026-07-04T00:00:00Z"),
        ("c", "2026-07-10T00:00:00.000000Z"),
    ] {
        let at = if key == "a" {
            "2026-07-05"
        } else {
            "2026-07-05T12:00:00Z"
        };
        let fields = json!({"due": due, "at": at, "issued": "2026-07-01"});
        l.upsert("inv", key, fields).await?;
    }
    let a = l.read("inv", "a").await?;
    let b = l.read("inv", "b").await?;
    let read = |r: &lakelands_api::Record, f: &str| r.fields.get(f).cloned();
    if [read(&a, "due"), read(&b, "due")] != [Some(json!("2026-07-05")), Some(json!("2026-07-04"))]
    {
        wrong.push(format!(
            "`due` read back: {:?} and {:?}",
            a.fields, b.fields
        ));
    }
    // A derived date lands at a midnight; off it, no value.
    if (read(&a, "net"), read(&a, "odd")) != (Some(json!("2026-07-31")), None) {
        wrong.push(format!("`net` and `odd` of `a`: {:?}", a.fields));
    }
    // Any other instant is refused, naming the field.
    for due in ["2026-07-05T10:00:00Z", "2026-07-05T00:00:00+02:00", "July"] {
        let answer = try_upsert(l.cat.as_ref(), &l.own, "inv", "z", json!({"due": due})).await;
        let refused = matches!(refused_on(&answer),
            Some(("due", Refusal::Constraint(said)))
                if matches!(&said.case, Case::NotDate { value } if value == due));
        if !refused {
            wrong.push(format!("`due` written {due}: {answer:?}"));
        }
    }
    // A key made from a date is the date as written.
    let eur = json!({"cur": "EUR", "day": "2026-07-31T00:00:00Z"});
    let keyed = try_upsert(l.cat.as_ref(), &l.own, "rate", "EUR:2026-07-31", eur).await;
    let keys: Vec<String> = (l.all_found("rate", Find::default()).await?)
        .iter()
        .map(|r| r.key.to_string())
        .collect();
    if keyed.is_err() || keys != ["EUR:2026-07-31"] {
        wrong.push(format!(
            "`rate` keyed by its date: {keyed:?}, keys {keys:?}"
        ));
    }
    // A view grouped by a date is keyed by it.
    let by_due = vec![field("due", Date), field("n", Number)];
    l.declare(view_of(
        "by_due",
        "inv",
        &["due"],
        by_due,
        &[("n", "count()")],
    ))
    .await?;
    let keys: Vec<String> = (l.all_found("by_due", Find::default()).await?)
        .iter()
        .map(|r| r.key.to_string())
        .collect();
    if keys != ["2026-07-04", "2026-07-05", "2026-07-10"] {
        wrong.push(format!("`by_due`'s keys: {keys:?}"));
    }
    // Order and comparison as the midnight: `a`'s `at` is its due date's.
    let up = |f: &str| order(f, OrderBy::Value, Direction::Asc);
    let by_order = Find {
        order_by: vec![up("due")],
        ..Find::default()
    };
    for (what, find, want) in [
        ("ordered by `due`", by_order, "b a c"),
        ("`due = at`", find_where("due = at"), "a"),
        (
            "`(at - due) / 1d = 0`",
            find_where("(at - due) / 1d = 0"),
            "a",
        ),
        ("`due < today()`", find_where("due < today()"), "a b c"),
        (
            "`due = '2026-07-05T00:00:00Z'`",
            find_where("due = '2026-07-05T00:00:00Z'"),
            "a",
        ),
        (
            "`date_bucket(due, 'month') = issued`",
            find_where("date_bucket(due, 'month') = issued"),
            "a b c",
        ),
    ] {
        let got: Vec<String> = (l.all_found("inv", find).await?)
            .iter()
            .map(|r| r.key.to_string())
            .collect();
        if got.join(" ") != want {
            wrong.push(format!("{what}: {got:?}, not {want}"));
        }
    }
    // A date and a date-time at its midnight tie, and are ordered by key;
    // a text holding a date orders as one.
    l.declare(type_decl("seen", vec![field("t", Text)])).await?;
    for (key, t) in [
        ("z1", "2026-07-05"),
        ("z0", "2026-07-05T00:00:00.000000Z"),
        ("y", "2026-07-04T23:00:00Z"),
    ] {
        l.upsert("seen", key, json!({"t": t})).await?;
    }
    let by_t = Find {
        order_by: vec![up("t")],
        ..Find::default()
    };
    let got: Vec<String> = (l.all_found("seen", by_t).await?)
        .iter()
        .map(|r| r.key.to_string())
        .collect();
    if got != ["y", "z0", "z1"] {
        wrong.push(format!("`seen` ordered by `t`: {got:?}, not y z0 z1"));
    }
    // The least and the greatest of dates are dates.
    let least = Find {
        aggregate: serde_json::from_value(json!({"measures": [
            {"name": "first", "func": "min", "field": "due"},
            {"name": "last", "func": "max", "field": "due"}]}))
        .ok(),
        ..Find::default()
    };
    let found = l.cat.find(&l.own, &"inv".into(), least).await;
    let groups = match found.must("the least and the greatest `due`")?.data {
        lakelands_api::Found::Groups { rows } => rows,
        other => return Err(Failure::new(format!("an aggregate answered {other:?}"))),
    };
    let values: Vec<_> = groups.iter().map(|g| json!(g.values)).collect();
    if values != [json!({"first": "2026-07-04", "last": "2026-07-10"})] {
        wrong.push(format!("`min` and `max` of `due`: {values:?}"));
    }
    // A view's parameter may be a date.
    let mut on = view_of(
        "on_day",
        "inv",
        &[],
        vec![field("n", Number)],
        &[("n", "count()")],
    );
    if let Some(v) = on.view.as_mut() {
        v.params = vec![param("day", Date)];
        v.r#where = Some("due = param('day')".into());
    }
    l.declare(on).await?;
    let params: Params = [("day".into(), json!("2026-07-05"))].into();
    let n = (l.all_found("on_day", bound(params)).await?)
        .first()
        .and_then(|r| r.fields.get("n").cloned());
    if n != Some(json!(1)) {
        wrong.push(format!("`on_day` bound to 2026-07-05 counts {n:?}"));
    }
    let described = describe_type(l.cat.as_ref(), &l.own, "inv").await?;
    let due = described.fields.iter().find(|f| f.name.as_str() == "due");
    if due.is_none_or(|f| f.ty.kind != Date) {
        wrong.push(format!("`describe` of `due`: {due:?}"));
    }
    ensure!(wrong.is_empty(), "a date field:\n{}", wrong.join("\n"));
    Ok(())
}

async fn a_migration_converts_dates_without_loss(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let kinds = |on, t, d, s| {
        let fields = [("on", on), ("t", t), ("d", d), ("s", s)];
        type_decl("ev", fields.map(|(n, k)| field(n, k)).to_vec())
    };
    l.declare(kinds(DateTime, Text, Date, Date)).await?;
    let e1 = json!({"on": "2026-07-05", "t": "2026-07-06", "d": "2026-07-07", "s": "2026-07-08"});
    l.upsert("ev", "e1", e1).await?;
    l.upsert(
        "ev",
        "e2",
        json!({"on": "2026-07-05T10:00:00Z", "t": "2026-07-06T00:00:00+02:00"}),
    )
    .await?;
    let mut wrong = Vec::new();
    // Over an instant that is no midnight, `date_time → date` is lossy,
    // and so is `text → date` over a midnight at a non-zero offset.
    let to = kinds(Date, Date, Date, Date);
    let report = (l.cat.migrate(&l.own, migration(to.clone())).await)
        .must("the preflight of `on` to a date")?
        .data;
    for name in ["on", "t"] {
        let c = report.fields.iter().find(|f| f.field.as_str() == name);
        let named = c.map(|c| (c.convertible, c.not_convertible, c.sample_keys.clone()));
        if report.lossless || named != Some((1, 1, vec!["e2".into()])) {
            wrong.push(format!("`{name}` to a date over e2's: {report:?}"));
        }
    }
    let applied = l
        .cat
        .migrate_apply(&l.own, migration(to), &report.report_id)
        .await;
    if !matches!(
        &applied,
        Err(lakelands_api::Error::Refused {
            reason: Refusal::Migration { .. },
            ..
        })
    ) {
        wrong.push(format!(
            "applying `on` to a date over e2's 10:00: {applied:?}"
        ));
    }
    // Over midnights every conversion is lossless.
    l.upsert(
        "ev",
        "e2",
        json!({"on": "2026-07-05T00:00:00Z", "t": "2026-07-06T00:00:00Z"}),
    )
    .await?;
    l.migrate(kinds(Date, Date, DateTime, Text)).await?;
    let e1 = l.read("ev", "e1").await?;
    let want = json!({"on": "2026-07-05", "t": "2026-07-06",
                      "d": "2026-07-07T00:00:00.000000Z", "s": "2026-07-08"});
    if json!(e1.fields) != want {
        wrong.push(format!("e1 migrated: {:?}, not {want}", e1.fields));
    }
    // A key field migrated to a date re-keys its type, and a reference to
    // it follows.
    let mut rate = type_decl("rate", vec![field("cur", Text), field("day", DateTime)]);
    rate.key = vec!["cur".into(), "day".into()];
    l.declare(rate.clone()).await?;
    l.declare(type_decl("pay", vec![reference("rate", "rate")]))
        .await?;
    let old = "EUR:2026-07-31T00%3A00%3A00.000000Z";
    l.upsert("rate", old, json!({"cur": "EUR", "day": "2026-07-31"}))
        .await?;
    l.upsert("pay", "p1", json!({"rate": old})).await?;
    rate.fields[1] = field("day", Date);
    l.migrate(rate).await?;
    let keys: Vec<String> = (l.all_found("rate", Find::default()).await?)
        .iter()
        .map(|r| r.key.to_string())
        .collect();
    let p1 = l.read("pay", "p1").await?;
    if keys != ["EUR:2026-07-31"] || p1.fields.get("rate") != Some(&Value::from("EUR:2026-07-31")) {
        wrong.push(format!(
            "`rate` re-keyed by its date: keys {keys:?}, p1 {:?}",
            p1.fields
        ));
    }
    ensure!(
        wrong.is_empty(),
        "a migration to and from a date:\n{}",
        wrong.join("\n")
    );
    Ok(())
}
