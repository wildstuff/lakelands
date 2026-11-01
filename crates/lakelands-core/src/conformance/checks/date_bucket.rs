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

use lakelands_api::Value;

use crate::conformance::fixture::{doors, owner, type_of};
use crate::conformance::prelude::*;

check!(
    GROUPS = find_groups_records_by_date_bucket_for_every_unit,
    "017, extended, and 029: find groups by a named date_bucket key for day, week (ISO, from Monday), month, quarter and year, across a year's end and a leap day, with the same groups, counts and exact sums on every engine, the bucket answered under the key's name as its starting instant, and the records without a date in the null group, last"
);

check!(
    WHERE = a_where_selects_the_records_of_one_date_bucket,
    "017, extended: a where comparing date_bucket(x, 'month') with an instant selects exactly that month's records; a unit outside the five is refused as invalid, naming it"
);

check!(
    DECLARED = a_declaration_may_call_date_bucket,
    "017, extended: a declaration may call date_bucket (a derived field, a view's where, an object set's filter), and a derived bucket answers finds by it"
);

check!(
    WINDOWS = find_groups_records_by_windows_of_a_width_from_an_origin,
    "017, extended (lakelands#559): date_bucket(x, width, origin) answers the start of the window of that width holding x, the windows counted from the origin in both directions, the same on every engine; a width without its origin, a width that is not above zero, a quoted width and a calendar unit with an origin are refused as invalid, each naming what to write; a declaration may call it by a width"
);

check!(
    ZONED = a_calendar_bucket_is_cut_at_local_midnight_in_a_named_zone,
    "017, extended (lakelands#1667): date_bucket(x, unit, zone) cuts a calendar unit at local midnight in an IANA zone: in Europe/Berlin the day of 2030-03-31 is 23 hours long and that of 2030-10-27 25, a week starts on the local Monday and a month, a quarter and a year on the local first, and in America/Havana the day of 2030-03-10, whose midnight the clocks skip, starts at its 01:00; the same groups, counts and sums on every engine; a derived field, a view's group term and a trigger's `when` may call it, the trigger firing where the local day holds; without a zone a bucket is cut in UTC as before; a zone the database does not name is refused `bucket_zone`, naming it, at a read, a derived field, a view's group term, a view's `where` and a trigger's `when`"
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
    keyed(&format!("date_bucket(on, '{unit}')"))
}

/// A find grouping by `bucket`, counting the entries and summing them.
fn keyed(bucket: &str) -> Find {
    Find {
        aggregate: Some(Aggregate {
            measures: vec![
                agg("n", AggFunc::Count, None),
                agg("total", AggFunc::Sum, Some("amount")),
            ],
            group_by: vec![GroupKey::Named {
                name: "bucket".into(),
                expr: Condition::Text(bucket.to_owned()),
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

async fn find_groups_records_by_date_bucket_for_every_unit(store: Store) -> Held {
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

async fn a_where_selects_the_records_of_one_date_bucket(store: Store) -> Held {
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
        matches!(invalid_case(&refused), Some(Case::BucketUnit { unit }) if unit == "fortnight"),
        "a unit outside the five is refused as invalid, naming it; find answered {refused:?}"
    );
    Ok(())
}

async fn a_declaration_may_call_date_bucket(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let declared = l.declare_json(json!({
        "declare": "type", "name": "entry",
        "fields": [
            {"name": "on", "type": {"kind": "date_time"}},
            {"name": "month", "type": {"kind": "date_time"}, "authority": "derived",
             "derived": "date_bucket(on, 'month')", "depends_on": ["on"]}
        ]
    }));
    declared.await?;
    for (k, on) in [
        ("a", "2028-01-05T10:00:00Z"),
        ("b", "2028-01-30T23:59:59Z"),
        ("c", "2028-02-01T00:00:00Z"),
    ] {
        l.upsert("entry", k, json!({ "on": on })).await?;
    }
    let rows = l.all_found("entry", find_where("month = '2028-01-01T00:00:00Z'"));
    let rows = rows.await?;
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
    Ok(())
}

async fn find_groups_records_by_windows_of_a_width_from_an_origin(store: Store) -> Held {
    let l = lake(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    // Thirty days from 2028-01-01: the seven days of December fall in the
    // window that starts thirty days before the origin, 2027-12-02; then
    // January 1 to 30, January 31 to the leap February's end, and March.
    let thirty = keyed("date_bucket(on, 30d, '2028-01-01T00:00:00Z')");
    let got = groups(store.as_ref(), thirty, "grouping by thirty days").await?;
    let starts = [
        ("2027-12-02", 7),
        ("2028-01-01", 30),
        ("2028-01-31", 30),
        ("2028-03-01", 5),
    ];
    let want = expect(&starts);
    ensure!(
        got == want,
        "grouped by windows of 30d from 2028-01-01, find answered\n  {got:?}\nand expected are\n  \
         {want:?}"
    );
    // Each case's sentence says how a call is written.
    let arguments = json!({"code": "bucket_arguments"});
    for (bucket, case) in [
        ("date_bucket(on, 30d)", &arguments),
        ("date_bucket(on, 0d, on)", &arguments),
        (
            "date_bucket(on, '30d', on)",
            &json!({"code": "bucket_unit", "unit": "30d"}),
        ),
        ("date_bucket(on, 'month', on)", &arguments),
    ] {
        let refused = cat.find(own, &TypeName::from("entry"), keyed(bucket)).await;
        ensure!(
            invalid_like(&refused, case),
            "`{bucket}` is refused as invalid, as {case}; find answered {refused:?}"
        );
    }
    // A declaration may call it.
    let declared = l.declare_json(json!({
        "declare": "type", "name": "shift",
        "fields": [
            {"name": "on", "type": {"kind": "date_time"}},
            {"name": "week", "type": {"kind": "date_time"}, "authority": "derived",
             "derived": "date_bucket(on, 7d, '2027-12-27T00:00:00Z')"}
        ]
    }));
    declared.await?;
    l.upsert("shift", "s", json!({"on": "2028-01-05T10:00:00Z"}))
        .await?;
    let week = l.read("shift", "s").await?;
    ensure!(
        crate::conformance::fixture::value(&week, "week")
            == Some(&json!("2028-01-03T00:00:00.000000Z")),
        "a derived window of seven days from Monday 2027-12-27 holds 2028-01-03 for 2028-01-05; \
         the record holds {:?}",
        week.fields
    );
    Ok(())
}

/// The entries around Europe/Berlin's changes of offset in 2030 and
/// America/Havana's in March, each of 1.25, and one undated.
const ZONED_ENTRIES: [(&str, &str); 9] = [
    ("a", "2030-03-30T22:30:00Z"),
    ("b", "2030-03-30T23:30:00Z"),
    ("c", "2030-03-31T21:30:00Z"),
    ("d", "2030-03-31T22:30:00Z"),
    ("e", "2030-10-26T22:30:00Z"),
    ("f", "2030-10-27T22:30:00Z"),
    ("g", "2030-10-27T23:30:00Z"),
    ("h", "2030-03-10T05:30:00Z"),
    ("i", "2030-03-10T04:30:00Z"),
];

async fn a_calendar_bucket_is_cut_at_local_midnight_in_a_named_zone(store: Store) -> Held {
    let (s, l) = (store.as_ref(), Lake::of(store.as_ref()));
    let Lake { cat, own } = &l;
    let field = |n: &str, k: &str| json!({"name": n, "type": {"kind": k}});
    let derived = |n: &str, e: &str| {
        json!({"name": n, "type": {"kind": "date_time"}, "authority": "derived",
               "derived": e, "depends_on": ["on"]})
    };
    l.declare_json(json!({"declare": "type", "name": "entry", "fields": [
        field("on", "date_time"), field("amount", "decimal"),
        derived("local", "date_bucket(on, 'day', 'Europe/Berlin')"),
        derived("utc", "date_bucket(on, 'day')")]}))
        .await?;
    for (k, on) in ZONED_ENTRIES {
        l.upsert("entry", k, json!({"on": on, "amount": "1.25"}))
            .await?;
    }
    l.upsert("entry", "undated", json!({"amount": "1.25"}))
        .await?;
    // (the bucket's start, the entries in it), the undated last.
    let want = |starts: &[(&str, u64)]| {
        let mut out: Vec<_> = starts
            .iter()
            .map(|(at, n)| {
                let total = dec("1.25") * rust_decimal::Decimal::from(*n);
                (Some(format!("{at}.000000Z")), *n, total)
            })
            .collect();
        out.push((None, 1, dec("1.25")));
        out
    };
    for (unit, starts) in [
        (
            "day",
            &[
                ("2030-03-09T23:00:00", 2),
                ("2030-03-29T23:00:00", 1),
                ("2030-03-30T23:00:00", 2),
                ("2030-03-31T22:00:00", 1),
                ("2030-10-26T22:00:00", 2),
                ("2030-10-27T23:00:00", 1),
            ][..],
        ),
        (
            "week",
            &[
                ("2030-03-03T23:00:00", 2),
                ("2030-03-24T23:00:00", 3),
                ("2030-03-31T22:00:00", 1),
                ("2030-10-20T22:00:00", 2),
                ("2030-10-27T23:00:00", 1),
            ][..],
        ),
        (
            "month",
            &[
                ("2030-02-28T23:00:00", 5),
                ("2030-03-31T22:00:00", 1),
                ("2030-09-30T22:00:00", 3),
            ][..],
        ),
        (
            "quarter",
            &[
                ("2029-12-31T23:00:00", 5),
                ("2030-03-31T22:00:00", 1),
                ("2030-09-30T22:00:00", 3),
            ][..],
        ),
        ("year", &[("2029-12-31T23:00:00", 9)][..]),
    ] {
        let bucket = format!("date_bucket(on, '{unit}', 'Europe/Berlin')");
        let got = groups(s, keyed(&bucket), &format!("grouping by `{bucket}`")).await?;
        let want = want(starts);
        ensure!(
            got == want,
            "grouped by `{bucket}`, find answered\n  {got:?}\nand expected are\n  {want:?}"
        );
    }
    // Havana's clocks skip the midnight of 2030-03-10: its day starts at
    // 01:00, 05:00 in UTC.
    let mut havana = keyed("date_bucket(on, 'day', 'America/Havana')");
    havana.filter = Some(Condition::Text("on < '2030-03-11T00:00:00Z'".into()));
    let got = groups(s, havana, "grouping by Havana's days").await?;
    let mut expected = want(&[("2030-03-09T05:00:00", 1), ("2030-03-10T05:00:00", 1)]);
    expected.pop();
    ensure!(
        got == expected,
        "grouped by Havana's days, find answered {got:?}; expected are {expected:?}"
    );
    // A derived field holds the local day; without a zone, the UTC day.
    let c = l.read("entry", "c").await?;
    let value = |f| crate::conformance::fixture::value(&c, f).cloned();
    ensure!(
        value("local") == Some(json!("2030-03-30T23:00:00.000000Z"))
            && value("utc") == Some(json!("2030-03-31T00:00:00.000000Z")),
        "`c`, at 2030-03-31T21:30Z, is in Berlin's day from 2030-03-30T23:00Z and in the UTC \
         day of 2030-03-31; it holds {:?}",
        c.fields
    );
    // A view's group term and a trigger's `when` may name a zone.
    let local = |zone: &str| {
        json!({"declare": "type", "name": "daily", "key": ["day"],
               "fields": [field("day", "date_time"), field("n", "number")],
               "view": {"source": "entry", "measures": [{"name": "n", "expr": "count()"}],
                        "group_by": [{"name": "day",
                                      "expr": format!("date_bucket(on, 'day', '{zone}')")}]}})
    };
    l.declare_json(local("Europe/Berlin")).await?;
    let key = lakelands_api::key::key_component(&json!("2030-03-30T23:00:00.000000Z"));
    let day = l.read("daily", &key.unwrap_or_default()).await?;
    ensure!(
        crate::conformance::fixture::value(&day, "n") == Some(&json!(2)),
        "the view grouped by Berlin's days holds two entries on 2030-03-31; it holds {:?}",
        day.fields
    );
    let trigger = |zone: &str| {
        json!({"declare": "trigger", "name": "sunday", "on": "entry",
               "when": format!("date_bucket(on, 'day', '{zone}') = '2030-03-30T23:00:00Z'")})
    };
    l.declare_json(trigger("Europe/Berlin")).await?;
    for (key, fires) in [("c", true), ("d", false)] {
        let at = l.upsert("entry", key, json!({"amount": "2"})).await?;
        let events = crate::conformance::fixture::events_from(s, at.position()).await?;
        let fired = events.iter().any(|e| matches!(&e.body,
            EventBody::TriggerFired { trigger, .. } if trigger.as_str() == "sunday"));
        ensure!(
            fired == fires,
            "a trigger whose `when` is Berlin's day of 2030-03-31 fires at `{key}`: {fires}; \
             it fired: {fired}"
        );
    }
    // A zone the database does not name, wherever date_bucket is called.
    let unknown = "Mars/Olympus";
    let refusal = json!({"code": "bucket_zone", "zone": unknown});
    let bucket = format!("date_bucket(on, 'day', '{unknown}')");
    let read = cat.find(own, &TypeName::from("entry"), keyed(&bucket)).await;
    refused_like("a read grouped by an unknown zone", read, &refusal)?;
    let mut viewed = local("Europe/Berlin");
    viewed["name"] = json!("seen");
    viewed["view"]["where"] = json!(format!("{bucket} is not null"));
    for (what, declared) in [
        (
            "a derived field",
            json!({"declare": "type", "name": "far", "fields": [
                field("on", "date_time"), derived("day", &bucket)]}),
        ),
        ("a view's group term", local(unknown)),
        ("a view's `where`", viewed),
        ("a trigger's `when`", trigger(unknown)),
    ] {
        let before = s.head().await.must("the head")?;
        let declared = serde_json::from_value::<Declaration>(declared)
            .map_err(|e| Failure::new(format!("{what} does not read: {e}")))?;
        let answer = cat.declare(own, declared).await;
        refused_like(&format!("{what} in an unknown zone"), answer, &refusal)?;
        head_unmoved(s, before, &format!("{what} in an unknown zone")).await?;
    }
    Ok(())
}
