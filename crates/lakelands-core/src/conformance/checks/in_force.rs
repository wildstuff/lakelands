//! A field computed at the read paired with another type's record in
//! force (`docs/decisions/052-a-field-may-be-computed-at-the-read.md`,
//! extended; lakelands#1606): `in_force(tariff.price, tariff.from <= at,
//! tariff.meter = meter)` reads `price` of the record of `tariff` whose
//! `from` is the greatest at or below `at` among those of the same
//! `meter` the reader's `find` on `tariff` answers, null where none is.

use lakelands_api::seam::SETTING_TYPE;
use rust_decimal::Decimal;

use super::only::holder;
use crate::conformance::fixture::{get_at, order};
use crate::conformance::prelude::*;

check!(
    IN_FORCE_PAIRED = a_reading_is_paired_with_the_tariff_in_force,
    "052, extended (lakelands#1606): `in_force(tariff.price, tariff.from <= at, tariff.meter = meter)` answers each reading the `price` of its meter's tariff whose `from` is the greatest at or below its `at`, a `from` equal to `at` in force, tied the first by key; a reading before every tariff, of a meter no tariff has, or of no meter, null, another meter's never; `get`, `find`, a pattern's rows and a view's `sum(kwh * price)` agree; as of the position before a tariff is written a reading pairs the one before it and at the head the new one, which moves the view's sum; and `view_kept` for the view is refused `kept_at_read`, naming `price`"
);

check!(
    IN_FORCE_SIGHT = a_reading_is_paired_among_the_tariffs_its_reader_is_answered,
    "052, extended (lakelands#1606): a reading pairs among the tariffs its reader's `find` on `tariff` answers: a restricted tariff is passed over for a reader without `restricted` and paired for one granted it, a reader with `only` on `tariff` pairs among what `only` admits, a tariff whose `from` a hiding lowers is passed over, one whose `price` a hiding lowers reads the field as none, never as the tariff before it; a reader not shown `tariff` is not shown the field, one not shown a field `tariff.dear` is derived from is not shown `dear` paired, and one masked from `tariff.price` reads it masked"
);

check!(
    IN_FORCE_REFUSED = in_force_is_refused_where_it_cannot_pair,
    "052, extended (lakelands#1606): `in_force` in a constraint, in a field computed at the write or in a `find` is refused `in_force_here`; an order bound other than `T.by <= by`, a pairing with the type itself or with a view `in_force_form`; a matched pair of dates or of two kinds `in_force_pair`; a type that is none `undeclared`, a field it lacks `unknown_field`, two arguments `not_parsed`, a date paired into a decimal `never_lands`; `order_by` on the field `order_at_read`; and declaring `tariff` again without `from`, or migrating it so, is refused naming `from`"
);

/// The pairing every check declares, of `price` by `from` and `meter`.
const PAIRED: &str = "in_force(tariff.price, tariff.from <= at, tariff.meter = meter)";

/// `tariff` and `reading`, its `price` derived from `paired` at the read.
async fn declared(l: &Lake, paired: &str) -> Held {
    let f = |n: &str, k: &str| json!({"name": n, "type": {"kind": k}});
    let tariff = [f("meter", "text"), f("from", "date"), f("price", "decimal")];
    let dear = json!({"name": "dear", "type": {"kind": "decimal"}, "authority": "derived",
        "derived": "price"});
    let tariff = json!({"declare": "type", "name": "tariff",
        "fields": [tariff[0], tariff[1], tariff[2], f("hand", "text"), dear]});
    l.declare_json(tariff).await?;
    l.declare_json(reading(&[("price", paired)]))
        .await
        .map(drop)
}

/// `reading`, each of `paired` a field derived from it at the read.
fn reading(paired: &[(&str, &str)]) -> Value {
    let f = |n: &str, k: &str| json!({"name": n, "type": {"kind": k}});
    let mut fields = vec![f("meter", "text"), f("at", "date"), f("kwh", "decimal")];
    fields.extend(paired.iter().map(|(n, p)| {
        json!({"name": n, "type": {"kind": "decimal"}, "authority": "derived",
            "derived": p, "computed_at": "read"})
    }));
    json!({"declare": "type", "name": "reading", "fields": fields})
}

/// Puts the tariffs `(key, meter, from, price, hand)`.
async fn tariffs(l: &Lake, all: &[(&str, &str, &str, &str, &str)]) -> Result<Position, Failure> {
    let mut at = Position::GENESIS;
    for (key, meter, from, price, hand) in all {
        let t = json!({"meter": meter, "from": from, "price": price, "hand": hand});
        at = l.upsert("tariff", key, t).await?.position();
    }
    Ok(at)
}

/// Paired prices, by key.
type Prices = BTreeMap<String, Option<Decimal>>;

/// Each record's `price`, by key.
fn prices(rows: &[Record]) -> Prices {
    (rows.iter())
        .map(|r| (r.key.to_string(), decimal(r.fields.get("price"))))
        .collect()
}

/// What `who` reads on `key` of `reading` as of `as_of`.
async fn get(
    l: &Lake,
    who: &Principal,
    key: &str,
    as_of: Option<Position>,
) -> Result<Record, Failure> {
    let r = l.cat.get(who, &"reading".into(), get_at(key, as_of)).await;
    Ok(r.must("a reading")?.data)
}

async fn a_reading_is_paired_with_the_tariff_in_force(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let (c, own) = (l.cat.as_ref(), &l.own);
    declared(&l, PAIRED).await?;
    l.declare_json(json!({"declare": "type", "name": "cost",
        "fields": [{"name": "total", "type": {"kind": "decimal"}}],
        "view": {"source": "reading", "measures": [{"name": "total", "expr": "sum(kwh * price)"}]}}))
        .await?;
    let t = [
        ("T1", "M1", "2026-01-01", "10", "a"),
        ("T3", "M1", "2026-02-01", "15", "a"),
        ("T2", "M1", "2026-02-01", "14", "a"),
        ("T9", "M2", "2026-01-01", "99", "a"),
    ];
    tariffs(&l, &t).await?;
    let mut before = Position::GENESIS;
    for (key, meter, at, kwh) in [
        ("R1", json!("M1"), "2025-12-01", "1"),
        ("R2", json!("M1"), "2026-01-01", "1"),
        ("R3", json!("M1"), "2026-02-15", "2"),
        ("R4", json!("M1"), "2026-04-01", "3"),
        ("R5", json!("M3"), "2026-02-01", "1"),
        ("R6", Value::Null, "2026-02-01", "1"),
    ] {
        let r = json!({"meter": meter, "at": at, "kwh": kwh});
        before = l.upsert("reading", key, r).await?.position();
    }
    tariffs(&l, &[("T4", "M1", "2026-03-01", "20", "a")]).await?;
    for (as_of, r4, total) in [(Some(before), "14", "80"), (None, "20", "98")] {
        let n = |v: &str| Some(dec(v));
        let want: Prices = [
            ("R1", None),
            ("R2", n("10")),
            ("R3", n("14")),
            ("R4", n(r4)),
            ("R5", None),
            ("R6", None),
        ]
        .map(|(k, v)| (k.to_string(), v))
        .into();
        let found = prices(&all_found(c, own, "reading", find_at(as_of)).await?);
        let mut got = Vec::new();
        for key in want.keys() {
            got.push(get(&l, own, key, as_of).await?);
        }
        let got = prices(&got);
        let mut rows = pattern("reading", None, vec![], Projection::Rows { fields: vec![] });
        rows.at = read_at(as_of);
        let rows = c.query(own, rows).await.must("a pattern's rows")?.data;
        let QueryResult::Rows { rows, .. } = rows else {
            return Err(Failure::new("a pattern's rows answer rows"));
        };
        let row = |r: &BTreeMap<String, Value>| {
            let key = r.get("reading.key")?.as_str()?.to_string();
            Some((key, decimal(r.get("reading.price"))))
        };
        let paired: Prices = rows.iter().filter_map(row).collect();
        let view = all_found(c, own, "cost", find_at(as_of)).await?;
        let sum = decimal(view.first().and_then(|r| r.fields.get("total")));
        ensure!(
            [&found, &got, &paired].iter().all(|p| **p == want) && sum == Some(dec(total)),
            "as of {as_of:?}, each reading pairs {want:?} and `cost` sums {total}: `find` \
             answers {found:?}, `get` {got:?}, a pattern {paired:?} and the view {view:?}"
        );
    }
    let kept = json!({"key": "view_kept.cost", "value": true});
    let kept = try_upsert(c, own, SETTING_TYPE, "view_kept.cost", kept).await;
    ensure!(
        matches!(invalid_case(&kept), Some(Case::KeptAtRead { field, source }) if field == "price" && source == "reading"),
        "`view_kept.cost` over a field paired at the read is refused: {kept:?}"
    );
    Ok(())
}

async fn a_reading_is_paired_among_the_tariffs_its_reader_is_answered(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    declared(&l, PAIRED).await?;
    let dear = PAIRED.replace("tariff.price", "tariff.dear");
    l.declare_json(reading(&[("price", PAIRED), ("dear", &dear)]))
        .await?;
    let t = [
        ("S1", "M", "2026-01-01", "10", "a"),
        ("S2", "M", "2026-02-01", "20", "a"),
        ("S3", "M", "2026-03-01", "30", "b"),
        ("S4", "M", "2026-04-01", "40", "c"),
    ];
    tariffs(&l, &t).await?;
    restrict(store.as_ref(), "tariff", "S2", "kept").await?;
    for (key, at) in [
        ("P2", "2026-02-15"),
        ("P3", "2026-03-15"),
        ("P4", "2026-04-15"),
    ] {
        let r = json!({"meter": "M", "at": at, "kwh": "1"});
        l.upsert("reading", key, r).await?;
    }
    let s = store.as_ref();
    let hide = |v: &str, f: &str| json!({"field": "hand", "values": [v], "fields": {f: "none"}});
    let only = json!({"restricted": true, "types": {"tariff": {"only": {"hand": "a"}}}});
    let hid = json!({"tariff": {"hide_where": [hide("b", "from"), hide("c", "price")]}});
    let readers = [
        ("plain", json!({}), ["10", "30", "40"]),
        ("boss", json!({"restricted": true}), ["20", "30", "40"]),
        ("a", only, ["20", "20", "20"]),
        ("hid", json!({"types": hid}), ["10", "10", ""]),
    ];
    for (id, mut grants, want) in readers {
        grants["default"] = json!("read");
        let who = holder(s, id, grants);
        let mut got = Vec::new();
        for key in ["P2", "P3", "P4"] {
            got.push(decimal(get(&l, &who, key, None).await?.fields.get("price")));
        }
        let want = want.map(|p| (!p.is_empty()).then(|| dec(p)));
        ensure!(
            got == want,
            "`{id}` pairs P2, P3 and P4 with {want:?} among the tariffs it is answered: {got:?}"
        );
    }
    let blind = json!({"default": "read", "types": {"tariff": {"default": "none"}}});
    let blind = holder(s, "blind", blind);
    let masked = field_access(s, "masked", "tariff", &[("price", Access::Masked)]);
    let sly = field_access(s, "sly", "tariff", &[("price", Access::None)]);
    let unseen = get(&l, &blind, "P2", None).await?;
    let masked = get(&l, &masked, "P2", None).await?;
    let sly = get(&l, &sly, "P2", None).await?;
    ensure!(
        !sly.fields.contains_key("price") && !sly.fields.contains_key("dear"),
        "a reader not shown `tariff.price` is not shown `tariff.dear`, derived from it, nor \
         either paired at the read: {sly:?}"
    );
    let marked = |r: &Record| r.masked.iter().any(|f| f.as_str() == "price");
    ensure!(
        !unseen.fields.contains_key("price")
            && !marked(&unseen)
            && !masked.fields.contains_key("price")
            && marked(&masked),
        "a reader not shown `tariff` is not shown `price`, one masked from `tariff.price` \
         reads it masked: {unseen:?}, {masked:?}"
    );
    Ok(())
}

async fn in_force_is_refused_where_it_cannot_pair(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let (c, own) = (l.cat.as_ref(), &l.own);
    declared(&l, PAIRED).await?;
    l.declare_json(json!({"declare": "type", "name": "flat",
        "fields": [{"name": "n", "type": {"kind": "number"}}],
        "view": {"source": "tariff", "measures": [{"name": "n", "expr": "count()"}]}}))
        .await?;
    let priced = |paired: &str, at_read: bool| {
        let mut f = json!({"name": "price", "type": {"kind": "decimal"},
            "authority": "derived", "derived": paired});
        if at_read {
            f["computed_at"] = json!("read");
        }
        f
    };
    let odd = |f: Value, c: Value| {
        let k = |n: &str, k: &str| json!({"name": n, "type": {"kind": k}});
        json!({"declare": "type", "name": "odd", "fields": [k("meter", "text"),
            k("at", "date"), k("from", "date"), k("kwh", "decimal"), f], "constraints": c})
    };
    let read = |p: &str| odd(priced(p, true), json!([]));
    let held = json!([{"name": "c", "holds": format!("{PAIRED} > 0")}]);
    let mut tried = vec![
        (
            odd(json!({"name": "x", "type": {"kind": "text"}}), held),
            "in_force_here",
        ),
        (odd(priced(PAIRED, false), json!([])), "in_force_here"),
    ];
    for (args, case) in [
        (
            "tariff.price, tariff.from >= at, tariff.meter = meter",
            "in_force_form",
        ),
        (
            "odd.kwh, odd.from <= at, odd.meter = meter",
            "in_force_form",
        ),
        ("flat.n, flat.n <= kwh, flat.n = kwh", "in_force_form"),
        (
            "tariff.price, tariff.from <= at, tariff.from = at",
            "in_force_pair",
        ),
        (
            "tariff.price, tariff.from <= at, tariff.meter = kwh",
            "in_force_pair",
        ),
        (
            "none.price, none.from <= at, none.meter = meter",
            "undeclared",
        ),
        (
            "tariff.cost, tariff.from <= at, tariff.meter = meter",
            "unknown_field",
        ),
        ("tariff.price, tariff.from <= at", "not_parsed"),
        (
            "tariff.from, tariff.from <= at, tariff.meter = meter",
            "never_lands",
        ),
    ] {
        tried.push((read(&format!("in_force({args})")), case));
    }
    for (d, case) in tried {
        let d = serde_json::from_value(d).map_err(|e| Failure::new(e.to_string()))?;
        let a = c.declare(own, d).await;
        let said = invalid_case(&a).map(|c| json!(c)["code"].clone());
        ensure!(said == Some(json!(case)), "refused `{case}`: {a:?}");
    }
    let q = find_where(&format!("{PAIRED} is null"));
    let found = c.find(own, &"reading".into(), q).await;
    let by = Find {
        order_by: vec![order("price", OrderBy::Value, Direction::Asc)],
        ..Find::default()
    };
    let ordered = c.find(own, &"reading".into(), by).await;
    ensure!(
        matches!(invalid_case(&found), Some(Case::InForceHere))
            && matches!(invalid_case(&ordered), Some(Case::OrderAtRead { .. })),
        "`in_force` in a `find` is refused `in_force_here`, an order by `price` \
         `order_at_read`: {found:?}, {ordered:?}"
    );
    let (text, price) = (StorageKind::Text, field("price", StorageKind::Decimal));
    let without = vec![field("meter", text), price, field("hand", text)];
    let without = type_decl("tariff", without);
    let declared = l.try_declare(without.clone()).await;
    let migrated = l.cat.migrate(own, migration(without)).await;
    for a in [declared.map(drop), migrated.map(drop)] {
        ensure!(
            matches!(invalid_case(&a), Some(Case::FieldReadElsewhere { field, .. }) if field == "from"),
            "`tariff` without `from`, which `reading` pairs by, is refused naming it: {a:?}"
        );
    }
    Ok(())
}
