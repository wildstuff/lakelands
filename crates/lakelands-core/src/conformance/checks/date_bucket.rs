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
use lakelands_api::read::{AggFunc, AggMeasure, Aggregate, GroupKey};
use lakelands_api::write::Declaration;
use lakelands_api::{Error, Find, Found, LakeStore, Refusal, TypeName, Value};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, dec, decimal, declare, doors, field, head_unmoved, owner, type_decl, upsert,
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
    /// A declaration refuses it.
    NOT_DECLARED = a_declaration_calling_date_bucket_is_refused_and_nothing_is_appended,
    "017, extended, and log-format §6: date_bucket is a read's function; a declaration calling it (a derived field, declared alone or in a bundle through apply) is refused as invalid, naming the function, and nothing is appended"
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

async fn lake(store: &dyn LakeStore) -> Result<(), Failure> {
    let cat = doors(store);
    let own = owner(store);
    declare(
        cat.as_ref(),
        &own,
        type_decl(
            "entry",
            vec![
                field("on", StorageKind::DateTime),
                field("amount", StorageKind::Decimal),
            ],
        ),
    )
    .await?;
    for (i, day) in days().iter().enumerate() {
        upsert(
            cat.as_ref(),
            &own,
            "entry",
            &format!("e{i:03}"),
            json!({"on": day, "amount": "1.25"}),
        )
        .await?;
    }
    upsert(
        cat.as_ref(),
        &own,
        "entry",
        "undated",
        json!({"amount": "1.25"}),
    )
    .await?;
    Ok(())
}

fn by(unit: &str) -> Find {
    Find {
        aggregate: Some(Aggregate {
            measures: vec![
                AggMeasure {
                    name: "n".into(),
                    func: AggFunc::Count,
                    field: None,
                },
                AggMeasure {
                    name: "total".into(),
                    func: AggFunc::Sum,
                    field: Some("amount".into()),
                },
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
    lake(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
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
    let refused = cat
        .find(
            &own,
            &TypeName::from("entry"),
            Find {
                filter: Some(Condition::Text(
                    "date_bucket(on, 'fortnight') is not null".into(),
                )),
                ..Find::default()
            },
        )
        .await;
    ensure!(
        matches!(&refused, Err(Error::Refused { reason: Refusal::Invalid { message }, .. }) if message.contains("fortnight")),
        "a unit outside the five is refused as invalid, naming it; find answered {refused:?}"
    );
    Ok(())
}

async fn a_declaration_calling_date_bucket_is_refused_and_nothing_is_appended(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let with_bucket = || -> Declaration {
        serde_json::from_value(json!({
            "declare": "type", "name": "entry",
            "fields": [
                {"name": "on", "type": {"kind": "date_time"}},
                {"name": "month", "type": {"kind": "date_time"}, "authority": "derived",
                 "derived": "date_bucket(on, 'month')"}
            ]
        }))
        .expect("a declaration")
    };
    let before = store.head().await.must("reading the head")?;
    for (door, answer) in [
        (
            "declare",
            cat.declare(&own, with_bucket()).await.map(|_| ()),
        ),
        (
            "apply",
            cat.apply(&own, vec![with_bucket()]).await.map(|_| ()),
        ),
    ] {
        ensure!(
            matches!(&answer, Err(Error::Refused { reason: Refusal::Invalid { message }, .. }) if message.contains("date_bucket")),
            "{door} of a derived field calling date_bucket is refused as invalid, naming the \
             function; it answered {answer:?}"
        );
    }
    head_unmoved(store.as_ref(), before, "a declaration calling date_bucket").await
}
