//! A view's measure lands in the field the view declares for it, and a
//! value that field would not take written fails the measure
//! (`docs/decisions/023-one-evaluator-decides-what-an-expression-means.md`,
//! `docs/log-format.md` §1.2 and §1.3).

use lakelands_api::Value;

use crate::conformance::prelude::*;

check!(
    MEASURE_LANDS = a_measure_its_field_would_not_take_written_refuses_the_read,
    "023, 049 and the log format §1.2 and §1.3 (lakelands#839): a view's measure whose value its field would not take written fails as a measure that cannot be computed, and the read is refused as invalid naming the view, the measure and the group: a sum beyond ±(2^53 − 1) and below 10^21 into `number`, also beyond 64 bits and where it is not whole, an instant out of the years 0000 to 9999 by arithmetic or by a text's offset, a text into `number`, a noon into `date`; the same sums into `decimal` answer their digits, a value that is not whole answers the nearest double, and so does a sum of 10^21 or more, which is written as a double; a derived field and a verb's set land by the same rule, the double from 10^21 on and just below it no value and a refused call; a kept view flags the group, says `not_whole` and is refused as computed, at the head and as of the position, and is kept whole once the record is retired; `find`'s aggregate answers the digits"
);

const LARGEST: i64 = 9_007_199_254_740_991;

async fn a_measure_its_field_would_not_take_written_refuses_the_read(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let f = |n: &str, k: &str| json!({"name": n, "type": {"kind": k}});
    let fields = json!([
        f("g", "text"),
        f("x", "number"),
        f("at", "date_time"),
        f("note", "text")
    ]);
    l.declare_json(json!({"declare": "type", "name": "entry", "fields": fields}))
        .await?;
    for (key, g, x, at, note) in [
        (
            "a",
            "p",
            LARGEST,
            "9999-12-31T12:00:00Z",
            "9999-12-31T23:30:00-01:00",
        ),
        (
            "b",
            "p",
            LARGEST,
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:00:00Z",
        ),
        ("c", "q", 1, "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z"),
        (
            "d",
            "r",
            1,
            "0000-01-01T00:00:00Z",
            "0000-01-01T00:30:00+01:00",
        ),
    ] {
        let fields = json!({"g": g, "x": x, "at": at, "note": note});
        l.upsert("entry", key, fields).await?;
    }
    // One view a case: a measure that fails refuses every read of its view.
    let view = |name: &str, expr: &str, kind: &str| {
        let fields = json!([f("g", "text"), f("v", kind)]);
        let view = json!({"source": "entry", "group_by": ["g"], "measures": [{"name": "v", "expr": expr}]});
        l.declare_json(
            json!({"declare": "type", "name": name, "key": ["g"], "fields": fields, "view": view}),
        )
    };
    let read = |name: &str, find: Find| {
        let (cat, own, ty) = (l.cat.clone(), l.own.clone(), TypeName::from(name));
        async move { cat.find(&own, &ty, find).await }
    };
    let unfit = |view: &str, group: &str, why: &str, decimal: bool| {
        json!({"code": "measure_unfit", "group": group, "decimal": decimal, "why": {"code": why},
               "at": [{"code": "view", "name": view}, {"code": "measure", "name": "v"}]})
    };
    for (name, expr, kind, group, why) in [
        ("past53", "sum(x)", "number", "p", "integer_too_large"),
        (
            "past64",
            "sum(x * 1000)",
            "number",
            "p",
            "integer_too_large",
        ),
        ("broken", "sum(x * 1.3)", "number", "p", "integer_too_large"),
        ("late", "max(at + 1d)", "date_time", "p", "not_date_time"),
        (
            "early",
            "min(date_bucket(at, 'week'))",
            "date_time",
            "r",
            "not_date_time",
        ),
        ("offset", "max(note)", "date_time", "p", "not_date_time"),
        ("word", "max(note)", "number", "p", "value_shape"),
        ("noon", "max(at)", "date", "p", "not_date"),
    ] {
        view(name, expr, kind).await?;
        let what = format!("reading `{name}`, whose `{expr}` its `{kind}` field would not take");
        let expected = unfit(name, group, why, why == "integer_too_large");
        refused_like(&what, read(name, Find::default()).await, &expected)?;
    }
    // What its field takes is answered: the digits of a decimal, the
    // nearest double of a value that is not whole.
    let digits = |n: i128| Value::String(n.to_string());
    for (name, expr, kind, p) in [
        (
            "digits53",
            "sum(x)",
            "decimal",
            digits(2 * i128::from(LARGEST)),
        ),
        (
            "digits64",
            "sum(x * 1000)",
            "decimal",
            digits(2000 * i128::from(LARGEST)),
        ),
        (
            "double",
            "sum(x * 0.3)",
            "number",
            json!(5_404_319_552_844_595.0),
        ),
        // From 10^21 on a number is written as a double, and lands as one.
        (
            "double21",
            "sum(x * 100000)",
            "number",
            json!(1.801_439_850_948_198_2e21),
        ),
    ] {
        view(name, expr, kind).await?;
        let rows = match read(name, Find::default()).await.map(|r| r.data) {
            Ok(Found::Records { rows, .. }) => rows,
            other => return Err(Failure::new(format!("`{name}` is answered: {other:?}"))),
        };
        let v = rows
            .iter()
            .find(|r| r.key.as_str() == "p")
            .map(|r| &r.fields["v"]);
        let same = match (v, &p) {
            (Some(Value::Number(got)), Value::Number(want)) => got.as_f64() == want.as_f64(),
            (got, want) => got == Some(want),
        };
        ensure!(
            same && rows.len() == 3,
            "`{name}` answers `{expr}` over the group `p` as {p}, in its three groups; it answered {rows:?}"
        );
    }
    // A derived field and a verb's set land by the same rule: refused just
    // below 10^21, where a derived field holds no value, and the double
    // from there on.
    let by = |n: &str, expr: &str| json!({"name": n, "type": {"kind": "number"}, "authority": "derived", "derived": expr});
    let wide = json!([
        f("big", "decimal"),
        f("out", "number"),
        by("top", "big * 1000"),
        by("mid", "big * 10")
    ]);
    l.declare_json(json!({"declare": "type", "name": "wide", "fields": wide}))
        .await?;
    let set = json!([{"field": "out", "value": "big * payload.times"}]);
    l.declare_json(
        json!({"declare": "verb", "name": "scale", "on": "wide", "risk": "low",
        "effect": "write", "payload": [f("times", "number")], "set": set}),
    )
    .await?;
    l.upsert("wide", "w", json!({"big": "9223372036854775805"}))
        .await?;
    let scale = |times: i64| {
        let (cat, own, payload) = (l.cat.clone(), l.own.clone(), patch(json!({"times": times})));
        async move {
            let (verb, key) = ("scale".into(), "w".into());
            cat.call(&own, &verb, Some(&key), payload, Default::default())
                .await
        }
    };
    let below = format!("{:?}", scale(10).await);
    scale(1000).await.must("a verb's set of 10^21 and more")?;
    let w = l.read("wide", "w").await?.fields;
    let double = |n: &str| w.get(n).and_then(Value::as_f64);
    let top = Some(9.223_372_036_854_776e21);
    ensure!(
        below.contains("IntegerTooLarge")
            && double("out") == top
            && double("top") == top
            && !w.contains_key("mid"),
        "a value of 10^21 and more lands as its double in a derived field and by a verb's set, \
         and one just below is refused, the derived field holding no value: the call below \
         answered {below}, and the record holds {w:?}"
    );
    // Kept: the fold flags the group, and the view is computed and refused
    // as before; as of that position too, once the record is retired.
    l.set("view_kept.past53", json!(true)).await?;
    let kept =
        || async { Ok::<_, Failure>(json!(l.describe_type("past53").await?.lifecycle.kept)) };
    let flagged = json!({"state": "computed", "why": {"because": "not_whole", "not_exact": 1}});
    ensure!(
        kept().await? == flagged,
        "a kept view whose group holds a measure that does not land says {flagged}; it says {}",
        kept().await?
    );
    let expected = unfit("past53", "p", "integer_too_large", true);
    refused_like(
        "reading the kept `past53`",
        read("past53", Find::default()).await,
        &expected,
    )?;
    let before = l.retire("entry", "a").await?.position();
    let rows = match read("past53", Find::default()).await.map(|r| r.data) {
        Ok(Found::Records { rows, .. }) => rows,
        other => {
            return Err(Failure::new(format!(
                "`past53` is answered once `a` is retired: {other:?}"
            )));
        }
    };
    let p = rows
        .iter()
        .find(|r| r.key.as_str() == "p")
        .map(|r| r.fields["v"].clone());
    ensure!(
        p == Some(json!(LARGEST)) && kept().await? == json!({"state": "kept"}),
        "with `a` retired the group `p` sums to {LARGEST} and the view is kept whole; it answered \
         {p:?} and says {}",
        kept().await?
    );
    let then = lakelands_api::Position(before.0 - 1);
    refused_like(
        "reading `past53` as of the position before",
        read("past53", find_at(Some(then))).await,
        &expected,
    )?;
    // `find`'s aggregate lands in no field: it answers the digits.
    let by = json!({"measures": [{"name": "s", "func": "sum", "field": "x"}], "group_by": [{"name": "g", "expr": "g"}]});
    let mut find = aggregate(Vec::new());
    find.aggregate = serde_json::from_value(by).ok();
    find.at = find_at(Some(then)).at;
    let groups = read("entry", find).await.must("aggregating `entry`")?.data;
    let Found::Groups { rows } = &groups else {
        return Err(Failure::new(format!(
            "an aggregate answers groups: {groups:?}"
        )));
    };
    let s = rows
        .iter()
        .find(|g| g.by["g"] == "p")
        .map(|g| g.values["s"].clone());
    ensure!(
        s == Some(digits(2 * i128::from(LARGEST))),
        "`find`'s aggregate answers the sum of `p` as its digits; it answered {rows:?}"
    );
    Ok(())
}
