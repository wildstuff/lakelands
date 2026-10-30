//! `first(f, by)` and `last(f, by)`: the value of `f` on the member
//! whose `by` is the least, or the greatest
//! (`docs/decisions/018-one-pattern-query.md`).

use lakelands_api::Value;

use crate::conformance::fixture::order;
use crate::conformance::prelude::*;

check!(
    BY_ORDER = a_first_and_a_last_answer_what_a_find_ordered_by_their_by_answers_first,
    "018, extended (lakelands#1603): `first(f, by)` and `last(f, by)` answer `f` of the member whose `by` is the least and the greatest, ties broken by key order, null where that member's `f` is null and where no member has a `by`; a `find` grouped by a text field and a named `date_bucket` hour answers, in each group, for `first` and `last` of a decimal and of a text by an instant and by a decimal, and for `min` and `max` beside them, what a `find` of the group's members with a `by`, ordered by it ascending and descending with a `limit` of 1, answers, at a pinned position after later writes and at the head, and ungrouped too, and a pattern's aggregate over the readings alone answers, by their paths, what that `find` answers, equal `by`s taken in key order there too; `first` without `by` or without a field, and `by` on `max` and on `count`, are refused, naming the call, and a `by` the type does not declare as an unknown field",
    given
);

/// The origin of the hours.
const ORIGIN: &str = "2026-01-01T00:00:00Z";

/// The measures: name, function, field and the field it orders by.
const MEASURES: [(&str, &str, &str, &str); 7] = [
    ("lo", "min", "v", "v"),
    ("hi", "max", "v", "v"),
    ("opened", "first", "v", "at"),
    ("closed", "last", "v", "at"),
    ("early", "first", "note", "at"),
    ("late", "last", "note", "at"),
    ("least", "first", "note", "v"),
];

async fn a_first_and_a_last_answer_what_a_find_ordered_by_their_by_answers_first(
    store: Store,
    _: Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare_json(json!({"declare": "type", "name": "reading", "fields": [
        {"name": "meter", "type": {"kind": "text"}}, {"name": "at", "type": {"kind": "date_time"}},
        {"name": "v", "type": {"kind": "decimal"}}, {"name": "note", "type": {"kind": "text"}}]}))
        .await?;
    let at = |m: u32| format!("2026-01-01T{:02}:{:02}:00Z", m / 60, m % 60);
    for (key, meter, minute, v, note) in [
        ("r01", Some("m1"), Some(10), Some("5"), Some("a")),
        ("r02", Some("m1"), Some(50), Some("2"), Some("b")),
        ("r03", Some("m1"), Some(30), None, Some("c")),
        ("r04", Some("m1"), Some(80), Some("7"), None),
        ("r05", Some("m2"), Some(10), Some("3"), Some("e")),
        ("r06", Some("m2"), Some(10), Some("1"), Some("d")),
        ("r07", Some("m2"), Some(70), Some("4"), Some("f")),
        ("r08", Some("m2"), Some(70), Some("4"), Some("g")),
        ("r09", Some("m1"), None, Some("9"), Some("h")),
        ("r10", None, Some(20), Some("6"), Some("i")),
        ("r11", Some("m3"), Some(130), None, None),
    ] {
        let fields = json!({"meter": meter, "at": minute.map(at), "v": v, "note": note});
        l.upsert("reading", key, fields).await?;
    }
    let pinned = store.head().await.must("the head")?;
    l.upsert("reading", "r01", json!({"v": "0", "at": at(5)}))
        .await?;
    l.upsert(
        "reading",
        "r12",
        json!({"meter": "m2", "at": at(65), "v": "8"}),
    )
    .await?;
    l.retire("reading", "r02").await?;
    let mut wrong = Vec::new();
    for (as_of, grouped) in [(Some(pinned), true), (None, true), (Some(pinned), false)] {
        wrong.extend(compared(&l, as_of, grouped).await?);
        wrong.extend(walked(&l, as_of, grouped).await?);
    }
    ensure!(
        wrong.is_empty(),
        "a `first`, `last`, `min` or `max` must answer what a find of the group ordered by its \
         `by` with a limit of 1 answers first: {}",
        wrong.join("; ")
    );
    for (measure, said) in [
        (
            json!({"name": "o", "func": "first", "field": "v"}),
            Case::CallArity {
                name: "first".into(),
                given: 1,
            },
        ),
        (
            json!({"name": "o", "func": "last", "by": "at"}),
            Case::AggregateNeedsField {
                name: "last".into(),
            },
        ),
        (
            json!({"name": "o", "func": "count", "by": "at"}),
            Case::CallArity {
                name: "count".into(),
                given: 1,
            },
        ),
        (
            json!({"name": "o", "func": "first", "field": "v", "by": "nosuch"}),
            Case::UnknownField {
                ty: "reading".into(),
                field: "nosuch".into(),
            },
        ),
        (
            json!({"name": "o", "func": "max", "field": "v", "by": "at"}),
            Case::CallArity {
                name: "max".into(),
                given: 2,
            },
        ),
    ] {
        let find = aggregated(vec![measure.clone()], false);
        let answer = l.cat.find(&l.own, &TypeName::from("reading"), find).await;
        let got = invalid_case(&answer).cloned();
        if got.as_ref() != Some(&said) {
            wrong.push(format!("{measure}: {got:?}, not {said:?}"));
        }
    }
    ensure!(
        wrong.is_empty(),
        "a measure misusing `by` must be refused, naming the call: {}",
        wrong.join("; ")
    );
    Ok(())
}

/// The aggregate of `measures`, grouped by meter and hour or by nothing,
/// its fields named under `path` (`reading.` in a pattern's rows).
fn grouping(measures: Vec<Value>, grouped: bool, path: &str) -> Value {
    let hour = format!("date_bucket({path}at, 1h, '{ORIGIN}')");
    let by = if grouped {
        json!([format!("{path}meter"), {"name": "hour", "expr": hour}])
    } else {
        json!([])
    };
    json!({"measures": measures, "group_by": by})
}

/// A find of `measures`, grouped by meter and hour or by nothing.
fn aggregated(measures: Vec<Value>, grouped: bool) -> Find {
    Find {
        aggregate: serde_json::from_value(grouping(measures, grouped, "")).ok(),
        ..Find::default()
    }
}

/// The measures, their fields named under `path`.
fn measured(path: &str) -> Vec<Value> {
    let measures = MEASURES.map(|(name, func, field, by)| {
        let by = matches!(func, "first" | "last").then(|| format!("{path}{by}"));
        json!({"name": name, "func": func, "field": format!("{path}{field}"), "by": by})
    });
    measures.to_vec()
}

/// Groups as `<by> => <values>`, sorted.
fn told(rows: &[lakelands_api::read::Group]) -> Vec<String> {
    let mut told: Vec<String> = (rows.iter())
        .map(|g| format!("{:?} => {:?}", Vec::from_iter(g.by.values()), g.values))
        .collect();
    told.sort();
    told
}

/// Where a pattern over the readings alone and the find disagree, as
/// of `as_of`: equal `by`s are taken in key order in both.
async fn walked(l: &Lake, as_of: Option<Position>, grouped: bool) -> Result<Vec<String>, Failure> {
    let a = grouping(measured("reading."), grouped, "reading.");
    let a = serde_json::from_value(a).map_err(|e| Failure::new(format!("the aggregate: {e}")))?;
    let mut p = pattern("reading", None, Vec::new(), Projection::Aggregate(a));
    p.at.as_of = as_of;
    let QueryResult::Groups { rows } = l.cat.query(&l.own, p).await.must("a pattern")?.data else {
        return Err(Failure::new("a pattern's aggregate answered no groups"));
    };
    let mut find = aggregated(measured(""), grouped);
    find.at.as_of = as_of;
    let found = l.cat.find(&l.own, &TypeName::from("reading"), find).await;
    let Found::Groups { rows: found } = found.must("finding the readings")?.data else {
        return Err(Failure::new("an aggregate answered records, not groups"));
    };
    let (walked, found) = (told(&rows), told(&found));
    Ok((walked != found)
        .then(|| format!("a pattern as of {as_of:?} answers {walked:?}, a find {found:?}"))
        .into_iter()
        .collect())
}

/// Where the grouped find and the ordered finds disagree, as of `as_of`.
async fn compared(
    l: &Lake,
    as_of: Option<Position>,
    grouped: bool,
) -> Result<Vec<String>, Failure> {
    let mut find = aggregated(measured(""), grouped);
    find.at.as_of = as_of;
    let ty = TypeName::from("reading");
    let found = l.cat.find(&l.own, &ty, find).await;
    let Found::Groups { rows } = found.must("finding the readings by meter and hour")?.data else {
        return Err(Failure::new("an aggregate answered records, not groups"));
    };
    let mut wrong = Vec::new();
    for group in rows {
        let is = |field: &str, v: Option<&Value>| {
            let field = match field {
                "hour" => format!("date_bucket(at, 1h, '{ORIGIN}')"),
                _ => field.to_owned(),
            };
            match v {
                None | Some(Value::Null) => format!("{field} is null"),
                Some(v) => format!("{field} = {}", v.to_string().replace('"', "'")),
            }
        };
        let within: Vec<String> = ["meter", "hour"]
            .iter()
            .filter(|_| grouped)
            .map(|f| is(f, group.by.get(*f)))
            .collect();
        for (name, func, field, by) in MEASURES {
            let mut filter = within.clone();
            filter.push(format!("{by} is not null"));
            let dir = if matches!(func, "min" | "first") {
                Direction::Asc
            } else {
                Direction::Desc
            };
            let mut find = find_where(&filter.join(" and "));
            find.order_by = vec![order(by, OrderBy::Value, dir)];
            find.limit = Some(1);
            find.at.as_of = as_of;
            let top = l.cat.find(&l.own, &ty, find).await;
            let top = top.must("finding a group's readings in order")?.data;
            let Found::Records { rows: records, .. } = top else {
                return Err(Failure::new("an ordered find answered groups, not records"));
            };
            let want = records.first().and_then(|r| r.fields.get(field).cloned());
            let want = want.unwrap_or(Value::Null);
            let got = group.values.get(name).cloned().unwrap_or(Value::Null);
            if got != want {
                let (at, of) = (as_of.map(|p| p.0), &group.by);
                wrong.push(format!(
                    "`{name}` of {of:?} as of {at:?} is {got}, not {want}"
                ));
            }
        }
    }
    Ok(wrong)
}
