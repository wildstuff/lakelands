//! Valid-time cells of the reads at `valid_at` (lakelands#909,
//! lakelands#910, lakelands#911, lakelands#912), run by the rows of
//! `masking`: a `valid_to` alone, the declaration a read at a position
//! follows, a kept view with a valid time of its own, and a view over
//! merged sources (`docs/decisions/006-grants-not-roles.md`,
//! `docs/decisions/028-authoring-is-strict-and-whole.md`).

use crate::conformance::fixture::{declared_below, get_at};
use crate::conformance::prelude::*;

/// A read at `day` of 2026, as of `as_of`.
fn on(day: Option<&str>, as_of: Option<Position>) -> ReadAt {
    let at = |d: &str| Timestamp::parse(&format!("2026-{d}T00:00:00Z")).expect("an instant");
    ReadAt {
        valid_at: day.map(at),
        as_of,
        clock: None,
    }
}

/// What a door answered, in short: the keys and each `n` it answered, in
/// order, or its refusal.
fn said<T: serde::Serialize>(r: lakelands_api::Result<lakelands_api::Reply<T>>) -> String {
    fn keys(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(m) => m.iter().for_each(|(k, x)| match k.as_str() {
                k if k == "key" || k.ends_with(".key") => out.push(x.to_string()),
                k if k == "n" || k.ends_with(".n") => out.push(format!("n={x}")),
                _ => keys(x, out),
            }),
            Value::Array(a) => a.iter().for_each(|x| keys(x, out)),
            _ => {}
        }
    }
    match r {
        Ok(r) => {
            let mut out = Vec::new();
            keys(&json!(r.data), &mut out);
            out.join(" ")
        }
        Err(Error::Refused {
            field,
            reason: Refusal::NotGranted,
        }) => format!("not granted {}", field.as_ref().map_or("-", |f| f.as_str())),
        Err(e) => match invalid_case(&Err::<(), _>(e.clone())) {
            Some(Case::MeasureFails { .. }) => "a measure fails".into(),
            _ => e.to_string(),
        },
    }
}

/// What `who` is answered of `ty` at `at`: `find`, a count, `get` of
/// `key` and a pattern started on the type.
async fn doors(l: &Lake, who: &Principal, ty: &str, key: &str, at: ReadAt) -> [String; 4] {
    let (cat, t) = (l.cat.as_ref(), TypeName::from(ty));
    let find = Find {
        at,
        ..Find::default()
    };
    let mut count = aggregate(vec![agg("n", AggFunc::Count, None)]);
    count.at = at;
    let mut get = get_at(key, None);
    get.at = at;
    let mut start = pattern(ty, None, vec![], Projection::Rows { fields: vec![] });
    start.at = at;
    [
        said(cat.find(who, &t, find).await),
        said(cat.find(who, &t, count).await),
        said(cat.get(who, &t, get).await),
        said(cat.query(who, start).await),
    ]
}

/// The type the check declares, from its JSON.
fn decl(v: Value) -> TypeDecl {
    serde_json::from_value(v).expect("the check's declaration reads")
}

/// A `valid_to` without a `valid_from` is refused where it is declared,
/// applied and migrated to; a type in force that holds it, as a log the
/// store did not write holds it, keeps it, and is read at `valid_at` as
/// a type with no valid time by every reader, at the source and through a
/// view. Answers the lake opened again.
pub(super) async fn a_valid_to_alone(store: Store, given: &Given) -> Result<Store, Failure> {
    let fields = json!([{"name": "end", "type": {"kind": "date_time"}}, {"name": "g", "type": {"kind": "text"}}]);
    let due = decl(json!({"name": "due", "valid_to": "end", "fields": fields}));
    let l = Lake::of(store.as_ref());
    l.declare(decl(json!({"name": "due0", "fields": fields})))
        .await?;
    let moved = TypeDecl {
        name: "due0".into(),
        ..due.clone()
    };
    let d = || Declaration::Type(due.clone());
    let refusals = [
        ("declare", l.cat.declare(&l.own, d()).await.map(drop)),
        ("apply", l.cat.apply(&l.own, vec![d()]).await.map(drop)),
        (
            "a migration",
            l.cat.migrate(&l.own, migration(moved)).await.map(drop),
        ),
    ];
    for (door, r) in refusals {
        let alone =
            matches!(invalid_case(&r), Some(Case::ValidToAlone { field }) if field == "end");
        ensure!(
            alone && refused_on(&r).is_some_and(|(f, _)| f == "end"),
            "{door} of a `valid_to` with no `valid_from` is refused naming `end`; it answered {r:?}"
        );
    }
    let store = declared_below(store, given, vec![Declaration::Type(due.clone())]).await?;
    let l = Lake::of(store.as_ref());
    l.declare_json(json!({"declare": "type", "name": "due_per", "key": ["g"],
        "fields": [{"name": "g", "type": {"kind": "text"}}, {"name": "n", "type": {"kind": "number"}}],
        "view": {"source": "due", "group_by": ["g"], "measures": [{"name": "n", "expr": "count()"}]}}))
        .await?;
    l.upsert(
        "due",
        "d1",
        json!({"g": "x", "end": "2026-06-01T00:00:00Z"}),
    )
    .await?;
    l.upsert("due", "d2", json!({"g": "x"})).await?;
    // Nothing filters by it: `d1`, ended in June, is read in July.
    let july = on(Some("07-01"), None);
    let owner = doors(&l, &l.own, "due", "d1", july).await;
    ensure!(
        owner[0] == r#""d1" "d2""#,
        "`due` in July answers `d1` and `d2`, its `valid_to` filtering nothing; it answered {owner:?}"
    );
    // Read at `valid_at` as a type with no valid time, by every reader.
    for access in [Access::Masked, Access::WriteOnly, Access::None] {
        let who = field_access(store.as_ref(), "clerk", "due", &[("end", access)]);
        for (ty, key) in [("due", "d1"), ("due_per", "x")] {
            for at in [july, ReadAt::default()] {
                let (want, got) = (
                    doors(&l, &l.own, ty, key, at).await,
                    doors(&l, &who, ty, key, at).await,
                );
                ensure!(
                    got == want,
                    "`end` {access:?}, `{ty}` at {:?}: {got:?}, not {want:?}",
                    at.valid_at
                );
            }
        }
    }
    // In force, it keeps it: declared again as it is, and with a field more.
    l.declare(due.clone()).await?;
    let mut more = due;
    more.fields.push(field("note", StorageKind::Text));
    l.declare(more).await?;
    Ok(store)
}

/// A read as of a position follows the valid time declared there: its
/// filter and its probe, not the head's.
pub(super) async fn the_valid_time_as_of_a_position(store: &Store) -> Held {
    let l = Lake::of(store.as_ref());
    let dt = |n: &str| json!({"name": n, "type": {"kind": "date_time"}});
    let shift = |timed: bool| {
        let mut t = decl(json!({"name": "shift", "fields": [dt("start")]}));
        t.valid_from = timed.then(|| "start".into());
        t
    };
    let slot = |from: &str| {
        decl(json!({"name": "slot", "valid_from": from, "fields": [dt("a"), dt("b")]}))
    };
    l.declare(shift(false)).await?;
    l.declare(slot("a")).await?;
    l.upsert("shift", "s1", json!({"start": "2026-09-01T00:00:00Z"}))
        .await?;
    l.upsert(
        "slot",
        "s1",
        json!({"a": "2026-09-01T00:00:00Z", "b": "2026-03-01T00:00:00Z"}),
    )
    .await?;
    let then = store.head().await.must("the head")?;
    l.declare(shift(true)).await?;
    l.declare(slot("b")).await?;
    let (past, now) = (on(Some("07-01"), Some(then)), on(Some("07-01"), None));
    let masked = |ty, f| field_access(store.as_ref(), "clerk", ty, &[(f, Access::Masked)]);
    let (s1, none) = (r#""s1""#, "");
    let ng = |f: &str| format!("not granted {f}");
    let cells = [
        (&l.own, "shift", past, s1.into()),
        (&l.own, "shift", now, none.into()),
        (&l.own, "slot", past, none.into()),
        (&l.own, "slot", now, s1.into()),
        (&masked("shift", "start"), "shift", past, s1.into()),
        (&masked("shift", "start"), "shift", now, ng("start")),
        (&masked("slot", "a"), "slot", past, ng("a")),
        (&masked("slot", "a"), "slot", now, s1.into()),
        (&masked("slot", "b"), "slot", past, none.into()),
        (&masked("slot", "b"), "slot", now, ng("b")),
    ];
    for (who, ty, at, want) in cells {
        let find = Find {
            at,
            ..Find::default()
        };
        let got = said(l.cat.find(who, &TypeName::from(ty), find).await);
        ensure!(
            got == want,
            "`{}` reading `{ty}` in July as of {:?} is answered {want:?}, by the valid time \
             declared there; it was answered {got:?}",
            who.id,
            at.as_of
        );
    }
    Ok(())
}

/// A kept view with a valid time of its own is computed at `valid_at`
/// for every reader, whatever it is shown of that valid time, at every
/// door alike; and a view over merged sources makes every probe before
/// it reads, so a masked reader is refused in either order.
pub(super) async fn a_kept_view_and_merged_sources_at_a_valid_time(store: &Store) -> Held {
    let l = Lake::of(store.as_ref());
    let (tx, num) = (
        |n: &str| json!({"name": n, "type": {"kind": "text"}}),
        |n: &str| json!({"name": n, "type": {"kind": "number"}}),
    );
    let dt = |n: &str| json!({"name": n, "type": {"kind": "date_time"}});
    l.declare(decl(
        json!({"name": "offer", "valid_from": "start", "fields": [dt("start"), dt("opened"), tx("g")]}),
    ))
    .await?;
    l.declare(decl(
        json!({"name": "ov", "key": ["g"], "valid_from": "since",
        "fields": [tx("g"), num("n"), dt("since")],
        "view": {"source": "offer", "group_by": ["g"], "measures": [
            {"name": "n", "expr": "count()"}, {"name": "since", "expr": "min(start)"}]}}),
    ))
    .await?;
    l.declare(decl(json!({"name": "base", "fields": [tx("g")]})))
        .await?;
    l.declare(decl(
        json!({"name": "part", "fields": [tx("g"), num("n"), num("z")]}),
    ))
    .await?;
    let source = |s: &str, m: &str, e: &str| json!({"source": s, "group_by": ["g"], "measures": [{"name": m, "expr": e}]});
    let (part, offer) = (
        source("part", "per", "sum(n) / sum(z)"),
        source("offer", "c", "count()"),
    );
    for (name, order) in [("m1", [&part, &offer]), ("m2", [&offer, &part])] {
        l.declare(decl(json!({"name": name, "key": ["g"],
            "fields": [tx("g"), num("n"), num("per"), num("c")],
            "view": {"source": "base", "group_by": ["g"], "measures": [{"name": "n", "expr": "count()"}],
                "merge": [order[0], order[1]]}})))
        .await?;
    }
    // `y` opens in December: a view of it, valid from then, is not valid
    // in July.
    let at = |d: &str| format!("2026-{d}T00:00:00Z");
    for (key, g, start, opened) in [
        ("o1", "x", "02-01", "01-01"),
        ("o2", "x", "01-01", "01-01"),
        ("o8", "y", "02-01", "12-01"),
    ] {
        l.upsert(
            "offer",
            key,
            json!({"g": g, "start": at(start), "opened": at(opened)}),
        )
        .await?;
    }
    l.upsert("base", "b1", json!({"g": "x"})).await?;
    l.upsert("part", "p1", json!({"g": "x", "n": 1, "z": 0}))
        .await?;
    l.set("view_kept.ov", json!(true)).await?;
    // In January `o2` alone holds: one group of one; the kept records
    // hold two.
    let january = on(Some("01-15"), None);
    let owner = doors(&l, &l.own, "ov", "x", january).await;
    let kept = doors(&l, &l.own, "ov", "x", ReadAt::default()).await;
    ensure!(
        owner[0] == r#"n=1 "x""# && kept[0] == r#"n=2 "x" n=1 "y""#,
        "`ov` in January is computed, one group of one, and kept without a valid time; \
         it answered {owner:?} and {kept:?}"
    );
    // Dated by the grant on the view, its own valid time is not probed
    // (092, lakelands#1571).
    for access in [Access::None, Access::Masked, Access::WriteOnly] {
        let who = field_access(store.as_ref(), "clerk", "ov", &[("since", access)]);
        let dated = super::masking::dated_by(store.as_ref(), "ov", ("since", access));
        for at in [january, ReadAt::default()] {
            let owner = doors(&l, &l.own, "ov", "x", at).await;
            let mut want = owner.clone();
            if access != Access::None && at.valid_at.is_some() {
                want = std::array::from_fn(|_| "not granted since".into());
            }
            let got = doors(&l, &who, "ov", "x", at).await;
            let by_it = doors(&l, &dated, "ov", "x", at).await;
            ensure!(
                got == want && by_it == owner,
                "`since` {access:?} at {:?}: {got:?}, not {want:?}; dated by it {by_it:?}",
                at.valid_at
            );
        }
    }
    // Merged sources: the probe of `offer`'s `start` before any measure.
    for access in [Access::None, Access::Masked, Access::WriteOnly] {
        let who = field_access(store.as_ref(), "clerk", "offer", &[("start", access)]);
        for view in ["m1", "m2"] {
            for at in [on(Some("07-01"), None), ReadAt::default()] {
                let refused = access != Access::None && at.valid_at.is_some();
                let want = ["a measure fails", "not granted -"][usize::from(refused)];
                for got in doors(&l, &who, view, "x", at).await {
                    ensure!(
                        got == want,
                        "`start` {access:?}, `{view}` at {:?}: {got}",
                        at.valid_at
                    );
                }
            }
        }
    }
    // A computed view's own valid time keeps its records as a type's
    // does: `oa` valid from its first opening, `ob` until its last start
    // besides, which a reader at `none` on it is answered as `oa`.
    let own = |name: &str, until: bool| {
        let mut v = json!({"name": name, "key": ["g"], "valid_from": "since",
            "fields": [tx("g"), dt("since"), dt("until")],
            "view": {"source": "offer", "group_by": ["g"], "measures": [
                {"name": "since", "expr": "min(opened)"}, {"name": "until", "expr": "max(start)"}]}});
        if until {
            v["valid_to"] = json!("until");
        }
        decl(v)
    };
    l.declare(own("oa", false)).await?;
    l.declare(own("ob", true)).await?;
    let july = on(Some("07-01"), None);
    let blind = field_access(store.as_ref(), "clerk", "ob", &[("until", Access::None)]);
    let dated = super::masking::dated_by(store.as_ref(), "ob", ("until", Access::None));
    let told = [
        doors(&l, &l.own, "oa", "x", july).await,
        doors(&l, &l.own, "ob", "x", july).await,
        doors(&l, &blind, "ob", "x", july).await,
        doors(&l, &dated, "ob", "x", july).await,
    ];
    ensure!(
        told[0][0] == r#""x""# && told[1][0].is_empty() && told[2] == told[0] && told[3] == told[1],
        "in July `oa` answers `x` alone, `ob` nothing, and `ob` to a reader at `none` on its \
         end what `oa` answers at every door, and to one dated by it what `ob` does; they \
         answered {told:?}"
    );
    // Kept views whose valid time stands only in a merged source, a kept
    // view of the valid-timed offers, and one over them, are computed at
    // a valid time: in January `o2` alone holds.
    let m = |n: &str, e: &str| json!({"name": n, "expr": e});
    let views = json!([
        {"name": "m_in", "key": ["g"], "fields": [tx("g"), num("c")],
         "view": {"source": "offer", "group_by": ["g"], "measures": [m("c", "count()")]}},
        {"name": "t_out", "key": ["g"], "fields": [tx("g"), num("n"), num("c")],
         "view": {"source": "base", "group_by": ["g"], "measures": [m("n", "count()")],
                  "merge": [{"source": "m_in", "group_by": ["g"], "measures": [m("c", "sum(c)")]}]}},
        {"name": "t_top", "key": ["g"], "fields": [tx("g"), num("cc")],
         "view": {"source": "t_out", "group_by": ["g"], "measures": [m("cc", "sum(c)")]}}]);
    for v in views.as_array().into_iter().flatten() {
        l.declare(decl(v.clone())).await?;
        l.set(
            &format!("view_kept.{}", v["name"].as_str().unwrap_or_default()),
            json!(true),
        )
        .await?;
    }
    let mut january = Vec::new();
    for (v, m) in [("t_out", "c"), ("t_top", "cc")] {
        let mut get = get_at("x", None);
        get.at = on(Some("01-15"), None);
        let got = l.cat.get(&l.own, &TypeName::from(v), get).await;
        january.push(got.map(|r| r.data.fields.get(m).cloned()).ok().flatten());
    }
    ensure!(
        january == [Some(json!(1)), Some(json!(1))],
        "in January `t_out` and `t_top` count `o2` alone, as they compute; they answered {january:?}"
    );
    Ok(())
}

/// A kept view with no valid time in its tree answers a read at `valid_at`
/// from its kept records, as a read without one does: the same records,
/// in the heap of the page it asks, where its computation holds every
/// group. Without a meter the heap is not asked.
pub(super) async fn a_kept_view_with_no_valid_time_at_one(store: &Store, given: &Given) -> Held {
    let l = Lake::of(store.as_ref());
    let tx = |n: &str| json!({"name": n, "type": {"kind": "text"}});
    l.declare(decl(json!({"name": "slip", "fields": [tx("k")]})))
        .await?;
    l.declare(decl(json!({"name": "slips", "key": ["k"],
        "fields": [tx("k"), {"name": "n", "type": {"kind": "number"}}],
        "view": {"source": "slip", "group_by": ["k"], "measures": [{"name": "n", "expr": "count()"}]}})))
    .await?;
    for i in 0..SLIPS {
        l.upsert("slip", &format!("s{i}"), json!({"k": format!("k{i:05}")}))
            .await?;
    }
    l.set("view_kept.slips", json!(true)).await?;
    let ty = TypeName::from("slips");
    let mut read = Vec::new();
    for at in [ReadAt::default(), on(Some("01-15"), None)] {
        let meter = given.meter.as_ref();
        let before = meter.map(|m| {
            m.reset_peak();
            m.live()
        });
        let find = Find {
            at,
            limit: Some(10),
            ..Find::default()
        };
        let page = l
            .cat
            .find(&l.own, &ty, find)
            .await
            .must("a page of `slips`")?;
        let held = meter.zip(before).map(|(m, b)| m.peak().saturating_sub(b));
        // The cursor carries the read's valid time; the rows are asked.
        read.push((json!(page.data)["rows"].clone(), held));
    }
    ensure!(
        read[0].0 == read[1].0,
        "a page of `slips` at a valid time answers as without one: {read:?}"
    );
    if let [(_, Some(plain)), (_, Some(timed))] = &read[..] {
        ensure!(
            *timed <= 2 * plain + (256 << 10),
            "a page of `slips`, with no valid time in its tree, holds {plain} bytes without a \
             valid time and {timed} at one: it is computed, not read from its kept records"
        );
    }
    Ok(())
}

/// The groups of the kept view [`a_kept_view_with_no_valid_time_at_one`]
/// reads a page of.
const SLIPS: usize = 2_000;
