//! Aggregates grouped into date buckets with `date_bucket(x, unit)`, a
//! read's function (`docs/decisions/017-one-expression-grammar.md`,
//! extended).
//!
//! The lake: one `entry` a day from 2027-12-25 to 2028-03-05, across a
//! year's end and the leap day 2028-02-29, each of `amount` 1.25, and one
//! entry without a date. The buckets are counted by hand in each check,
//! so every engine is held to the same groups, counts and exact sums. A
//! bucket starts at midnight UTC: the day, Monday of the ISO week, the
//! first of the month, of the quarter and of the year.

use std::sync::Arc;

use lakelands_api::expr::Condition;
use lakelands_api::ontology::StorageKind;
use lakelands_api::read::{AggFunc, Aggregate, GroupKey};
use lakelands_api::write::Declaration;
use lakelands_api::{Find, Found, LakeStore, Refusal, TypeName, Value};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, agg, dec, decimal, doors, find_where, log_events, owner, reason, type_of,
};

check!(
    /// Records group into date buckets by unit.
    GROUPS = find_groups_records_by_date_bucket_for_every_unit,
    "017, extended, and 029: find groups by a named date_bucket key for day, week (ISO, from Monday), month, quarter and year, across a year's end and a leap day, with the same groups, counts and exact sums on every engine, the bucket answered under the key's name as its starting instant, and the records without a date in the null group, last"
);

check!(
    /// A where selects by bucket.
    WHERE = a_where_selects_the_records_of_one_date_bucket,
    "017, extended: a where comparing date_bucket(x, 'month') with an instant selects exactly that month's records; a unit outside the five is refused as invalid, naming it"
);

check!(
    /// A declaration may call it, from format 3 on.
    DECLARED = a_declaration_may_call_date_bucket_and_from_it_the_lake_writes_format_3,
    "017, extended, and log-format §6: a declaration may call date_bucket (a derived field, a view's where, an object set's filter); the event that first declares such a call is written in format version 3, as is every event after it, and a derived bucket answers finds by it"
);

/// The days of the lake, as `YYYY-MM-DD`.
fn days() -> Vec<String> {
    let mut out = Vec::new();
    let mut add = |y: u32, m: u32, from: u32, to: u32| {
        for d in from..=to {
            out.push(format!("{y:04}-{m:02}-{d:02}"));
        }
    };
    add(2027, 12, 25, 31);
    add(2028, 1, 1, 31);
    add(2028, 2, 1, 29);
    add(2028, 3, 1, 5);
    out
}

async fn lake(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    l.declare(type_of(
        "entry",
        &[
            ("on", StorageKind::DateTime),
            ("amount", StorageKind::Decimal),
        ],
    ))
    .await?;
    for (i, day) in days().iter().enumerate() {
        l.upsert(
            "entry",
            &format!("e{i:03}"),
            json!({"on": day, "amount": "1.25"}),
        )
        .await?;
    }
    l.upsert("entry", "undated", json!({"amount": "1.25"}))
        .await?;
    Ok(l)
}

fn by(unit: &str) -> Find {
    Find {
        aggregate: Some(Aggregate {
            measures: vec![
                agg("n", AggFunc::Count, None),
                agg("total", AggFunc::Sum, Some("amount")),
            ],
            group_by: vec![GroupKey::Named {
                name: "bucket".into(),
                expr: Condition::Text(format!("date_bucket(on, '{unit}')")),
            }],
        }),
        ..Find::default()
    }
}

/// The groups `find` answers, as (bucket, count, sum): the bucket as the
/// text it is answered in, `None` for the null group.
async fn groups(
    store: &dyn LakeStore,
    find: Find,
    what: &str,
) -> Result<Vec<(Option<String>, u64, rust_decimal::Decimal)>, Failure> {
    let reply = doors(store)
        .find(&owner(store), &TypeName::from("entry"), find)
        .await
        .must(what)?;
    let Found::Groups { rows } = reply.data else {
        return Err(Failure::new(format!("{what} answered records, not groups")));
    };
    rows.into_iter()
        .map(|g| {
            let bucket = match g.by.get("bucket") {
                Some(Value::String(s)) => Some(s.clone()),
                Some(Value::Null) | None => None,
                Some(other) => {
                    return Err(Failure::new(format!(
                        "{what}: a group's bucket is {other}, not an instant"
                    )));
                }
            };
            let n = decimal(g.values.get("n"))
                .and_then(|d| u64::try_from(d).ok())
                .ok_or_else(|| Failure::new(format!("{what}: a group has no count")))?;
            let total = decimal(g.values.get("total"))
                .ok_or_else(|| Failure::new(format!("{what}: a group has no sum")))?;
            Ok((bucket, n, total))
        })
        .collect()
}

/// The expected groups from (bucket's first day, days in it): each day is
/// 1.25, and the undated entry is the null group, last.
fn expect(buckets: &[(&str, u64)]) -> Vec<(Option<String>, u64, rust_decimal::Decimal)> {
    let mut out: Vec<_> = buckets
        .iter()
        .map(|(start, n)| {
            (
                Some(format!("{start}T00:00:00.000000Z")),
                *n,
                dec("1.25") * rust_decimal::Decimal::from(*n),
            )
        })
        .collect();
    out.push((None, 1, dec("1.25")));
    out
}

async fn find_groups_records_by_date_bucket_for_every_unit(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    lake(store.as_ref()).await?;
    // 2027-12-25 is a Saturday: its ISO week starts on Monday 2027-12-20;
    // from Monday 2027-12-27 to Sunday 2028-03-05 are ten whole weeks.
    let mut weeks = vec![("2027-12-20", 2)];
    let week_starts = [
        "2027-12-27",
        "2028-01-03",
        "2028-01-10",
        "2028-01-17",
        "2028-01-24",
        "2028-01-31",
        "2028-02-07",
        "2028-02-14",
        "2028-02-21",
        "2028-02-28",
    ];
    weeks.extend(week_starts.iter().map(|w| (*w, 7)));
    let days = days();
    let each_day: Vec<(&str, u64)> = days.iter().map(|d| (d.as_str(), 1)).collect();
    for (unit, buckets) in [
        ("day", each_day),
        ("week", weeks),
        (
            "month",
            vec![
                ("2027-12-01", 7),
                ("2028-01-01", 31),
                ("2028-02-01", 29),
                ("2028-03-01", 5),
            ],
        ),
        ("quarter", vec![("2027-10-01", 7), ("2028-01-01", 65)]),
        ("year", vec![("2027-01-01", 7), ("2028-01-01", 65)]),
    ] {
        let got = groups(store.as_ref(), by(unit), &format!("grouping by {unit}")).await?;
        let want = expect(&buckets);
        ensure!(
            got == want,
            "grouped by date_bucket(on, '{unit}'), find answered {} groups:\n  {got:?}\nand \
             {} are expected:\n  {want:?}",
            got.len(),
            want.len()
        );
    }
    Ok(())
}

async fn a_where_selects_the_records_of_one_date_bucket(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let mut find = by("month");
    find.filter = Some(Condition::Text(
        "date_bucket(on, 'month') = '2028-02-01T00:00:00.000000Z'".into(),
    ));
    let got = groups(store.as_ref(), find, "finding February by its bucket").await?;
    let want = vec![(
        Some("2028-02-01T00:00:00.000000Z".to_owned()),
        29,
        dec("36.25"),
    )];
    ensure!(
        got == want,
        "a where selecting February 2028's bucket answered {got:?}; the 29 days of the leap \
         February are expected: {want:?}"
    );
    let fortnight = find_where("date_bucket(on, 'fortnight') is not null");
    let refused = cat.find(own, &TypeName::from("entry"), fortnight).await;
    ensure!(
        matches!(reason(&refused), Some(Refusal::Invalid { message }) if message.contains("fortnight")),
        "a unit outside the five is refused as invalid, naming it; find answered {refused:?}"
    );
    Ok(())
}

async fn a_declaration_may_call_date_bucket_and_from_it_the_lake_writes_format_3(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let before = store.head().await.must("reading the head")?;
    let declared_at = l
        .declare_json(json!({
            "declare": "type", "name": "entry",
            "fields": [
                {"name": "on", "type": {"kind": "date_time"}},
                {"name": "month", "type": {"kind": "date_time"}, "authority": "derived",
                 "derived": "date_bucket(on, 'month')", "depends_on": ["on"]}
            ]
        }))
        .await?
        .position();
    for (k, on) in [
        ("a", "2028-01-05T10:00:00Z"),
        ("b", "2028-01-30T23:59:59Z"),
        ("c", "2028-02-01T00:00:00Z"),
    ] {
        l.upsert("entry", k, json!({ "on": on })).await?;
    }
    let rows = l
        .all_found("entry", find_where("month = '2028-01-01T00:00:00Z'"))
        .await?;
    let mut keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
    keys.sort();
    ensure!(
        keys == ["a", "b"],
        "the entries whose derived month is January must be `a` and `b`; the find answered \
         {keys:?}"
    );
    // A view's where and an object set's filter may call it too.
    let more: Vec<Declaration> = serde_json::from_value(json!([
        {"declare": "type", "name": "january_totals", "key": ["month"],
         "fields": [{"name": "month", "type": {"kind": "date_time"}},
                    {"name": "n", "type": {"kind": "number"}}],
         "view": {"source": "entry", "group_by": ["month"],
                  "where": "date_bucket(on, 'year') = '2028-01-01T00:00:00Z'",
                  "measures": [{"name": "n", "expr": "count(on)"}]}},
        {"declare": "object_set", "name": "early", "of": "entry",
         "where": "date_bucket(on, 'day') < '2028-01-10T00:00:00Z'"}
    ]))
    .expect("declarations");
    cat.apply(own, more)
        .await
        .must("applying a view and an object set calling date_bucket")?;
    let events = log_events(store.as_ref()).await?;
    for e in &events {
        let want_v3 = e.position >= declared_at;
        ensure!(
            (e.v == 3) == want_v3 && (want_v3 || e.position <= before),
            "the event at {} carries format {}; the declaration at {declared_at} first calls \
             date_bucket, so it and every event after it carry 3, and none before it does",
            e.position,
            e.v
        );
    }
    Ok(())
}
