//! Paging and ordering of `find` (`lakelands_api::read`, `docs/glossary.md`).
//!
//! A page set is a total order over the matched records: walking the
//! cursor visits every record once, with no repeat and no gap. Ordering
//! arranges the set and never narrows it: a record without a value for
//! the sort field is still listed, last, in either direction. Both are
//! where two engines drift apart silently, each consistent with itself.

use std::collections::BTreeSet;

use lakelands_api::location::Limits;
use lakelands_api::ontology::StorageKind;
use lakelands_api::query::{Projection, QueryResult};
use lakelands_api::read::{After, AggFunc, Aggregate, Direction, Found, Order, OrderBy};
use lakelands_api::seam::Seams;
use lakelands_api::{
    Access, Catalog, Cursor, Find, Grants, LakeStore, Principal, PrincipalKind, Record, Refusal,
};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, agg, all_found, count_view, is_invalid, limits, order, pattern, principal,
    refused_on, type_of, value,
};
use crate::conformance::{Failure, Given, Held, Store};

check!(
    EVERY_RECORD_ONCE = paging_through_find_visits_every_record_exactly_once,
    "the read contract: a cursor continues where its page ended, so a page set is a total order with no repeat and no gap"
);

check!(
    TOTAL_AND_OFFSET = a_page_answers_its_total_and_an_offset_jumps_to_the_walks_slice,
    "045 and 029: `total` answers as many records as a full walk finds, for a filter the engine applies and one only the store evaluates, a view and a reader without `restricted`, the same on every page of a cursor walk while another writer writes; `offset` answers the full walk's slice from it, by key, by a field and by a field with an order index, a view too, and its cursor continues the walk; `offset` beside `cursor`, `after` or `aggregate`, and `total` beside `aggregate`, are refused as invalid, and an offset past `max_offset` as too_large naming it"
);

check!(
    MISSING_SORT_VALUES_LAST = records_without_the_sort_field_are_listed_last_in_either_direction,
    "the read contract: records without a value for a sort key sort last in either direction; ordering never drops them"
);

check!(
    UNNAMED_LIMIT = a_read_that_names_no_limit_takes_the_smaller_bound,
    "the location's limits: a read that names no `limit` takes `default_limit`, or `max_limit` where that is smaller, so it is never refused for a bound it did not ask for; a `limit` the caller names above `max_limit` is still refused as invalid",
    given
);

check!(
    EACH_HOLDER_ONCE = a_walk_under_an_exact_text_in_an_indexed_order_visits_each_holder_once,
    "041, extended (lakelands#695): a `find` by an exact text in the order of a field with an order index, either way, paged to the end by three and by five, fewer than the rows a page passes over, answers each record holding the text once and in the field's order, ties by key; the retired holders are passed over, and the restricted ones for a reader without `restricted`; so does a walk by three and by five most of whose pages pass over more rows than their first batches hold, where an engine may go on reading another way, with a retired and a restricted record among the rows it passes over before and after, and the last row it read before going on one the page answers, so it is answered once"
);

async fn counter(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    let ty = type_of(
        "counter",
        &[("n", StorageKind::Number), ("tag", StorageKind::Text)],
    );
    l.declare(ty).await?;
    Ok(l)
}

fn keys(rows: &[Record]) -> Vec<String> {
    rows.iter().map(|r| r.key.as_str().to_owned()).collect()
}

async fn paging_through_find_visits_every_record_exactly_once(store: Store) -> Held {
    const TOTAL: usize = 25;
    let l = counter(store.as_ref()).await?;
    for i in 0..TOTAL {
        let key = format!("k{i:03}");
        l.upsert("counter", &key, json!({"n": i % 4})).await?;
    }

    for order_by in [
        Vec::new(),
        vec![order("n", OrderBy::Value, Direction::Desc)],
    ] {
        let find = Find {
            order_by: order_by.clone(),
            limit: Some(7),
            ..Find::default()
        };
        let seen = keys(&l.all_found("counter", find).await?);
        let unique: BTreeSet<&String> = seen.iter().collect();
        ensure!(
            unique.len() == seen.len(),
            "paging by 7 (order {order_by:?}) repeated a record: the page order is not \
             total. Walked {seen:?}"
        );
        ensure!(
            seen.len() == TOTAL,
            "paging by 7 (order {order_by:?}) walked {} records of {TOTAL}; a page set \
             must reach every record. Walked {seen:?}",
            seen.len()
        );
    }
    Ok(())
}

async fn records_without_the_sort_field_are_listed_last_in_either_direction(store: Store) -> Held {
    let l = counter(store.as_ref()).await?;
    // Interleaved by key, so a tail that only happened to be in key order
    // cannot pass by accident.
    l.upsert("counter", "k1", json!({"n": 10})).await?;
    l.upsert("counter", "k2", json!({"tag": "b"})).await?;
    l.upsert("counter", "k3", json!({"n": 30})).await?;
    l.upsert("counter", "k4", json!({"tag": "d"})).await?;

    for (dir, valued) in [
        (Direction::Asc, ["k1", "k3"]),
        (Direction::Desc, ["k3", "k1"]),
    ] {
        let order = vec![order("n", OrderBy::Value, dir)];
        let whole = Find {
            order_by: order.clone(),
            limit: Some(50),
            ..Find::default()
        };
        let rows = l.all_found("counter", whole).await?;
        let listed = keys(&rows);
        ensure!(
            listed.len() == 4
                && listed[..2] == valued
                && rows[2..].iter().all(|r| value(r, "n").is_none()),
            "ordering by `n` {dir:?} must list {valued:?} and then the two records without \
             `n` (k2, k4) last; it listed {listed:?}"
        );

        let one_by_one = Find {
            order_by: order,
            limit: Some(1),
            ..Find::default()
        };
        let walked = keys(&l.all_found("counter", one_by_one).await?);
        ensure!(
            walked == listed,
            "paging one record at a time by `n` {dir:?} walked {walked:?}, but the whole \
             page listed {listed:?}; the cursor must cross from the valued records to \
             those without a value and lose none"
        );
    }
    Ok(())
}

/// One page of `find`: its keys, its cursor and its total.
async fn one_page(
    cat: &dyn Catalog,
    who: &Principal,
    ty: &str,
    find: Find,
) -> Result<(Vec<String>, Option<Cursor>, Option<u64>), Failure> {
    match cat
        .find(who, &ty.into(), find)
        .await
        .must(&format!("a page of `{ty}`"))?
        .data
    {
        Found::Records {
            rows,
            next_cursor,
            total,
        } => Ok((
            rows.iter().map(|r| r.key.to_string()).collect(),
            next_cursor,
            total,
        )),
        Found::Groups { .. } => Err(Failure::new("a page of records answered groups")),
    }
}

async fn a_page_answers_its_total_and_an_offset_jumps_to_the_walks_slice(store: Store) -> Held {
    let l = counter(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let cat = cat.as_ref();
    l.declare(count_view("by_tag", "counter", "tag")).await?;
    for i in 0..30u32 {
        let tag = if i % 3 == 0 { "a" } else { "b" };
        l.upsert(
            "counter",
            &format!("c{i:02}"),
            json!({"n": (i * 7) % 30, "tag": tag}),
        )
        .await?;
    }
    l.retire("counter", "c03").await?;
    store
        .restrict(&"counter".into(), &"c04".into(), "hold".into())
        .await
        .must("restricting c04")?;
    let reader = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    let reader = principal(store.as_ref(), PrincipalKind::Agent, "reader", reader);
    let walk = |who, ty, find| async move {
        let all = all_found(cat, who, ty, find).await?;
        Ok::<Vec<String>, Failure>(all.iter().map(|r| r.key.to_string()).collect())
    };
    for who in [own, &reader] {
        for (ty, filter) in [
            ("counter", None),
            ("counter", Some("n > 10")),
            ("counter", Some("n > 10 and now() > now() - 1d")),
            ("by_tag", None),
        ] {
            let find = || Find {
                filter: filter.map(Into::into),
                ..Find::default()
            };
            let all = walk(who, ty, find()).await?.len() as u64;
            let asked = Find {
                total: true,
                limit: Some(5),
                ..find()
            };
            let (_, _, total) = one_page(cat, who, ty, asked).await?;
            ensure!(
                total == Some(all),
                "a page of `{ty}` where {filter:?} with `total` answers the {all} records a full \
                 walk finds; it answered {total:?}"
            );
        }
    }
    let first = Find {
        total: true,
        limit: Some(5),
        ..Find::default()
    };
    let (_, next, before) = one_page(cat, own, "counter", first.clone()).await?;
    l.upsert("counter", "c30", json!({"n": 30, "tag": "a"}))
        .await?;
    let on = Find {
        cursor: next.clone(),
        ..first.clone()
    };
    let (_, _, pinned) = one_page(cat, own, "counter", on).await?;
    let (_, _, head) = one_page(cat, own, "counter", first).await?;
    ensure!(
        pinned == before && head == before.map(|n| n + 1),
        "a later page of a walk counts at the walk's position ({before:?}) while a write lands, \
         and a page read anew counts the head; they counted {pinned:?} and {head:?}"
    );
    let by = |orders: Vec<Order>| Find {
        order_by: orders,
        ..Find::default()
    };
    let n = |dir| vec![order("n", OrderBy::Value, dir)];
    let mut orders = vec![("key", Vec::new()), ("n descending", n(Direction::Desc))];
    l.set("sort_index.counter", json!(["n"])).await?;
    orders.push(("the indexed n", n(Direction::Asc)));
    for (what, order_by) in orders {
        let all = walk(own, "counter", by(order_by.clone())).await?;
        let jump = Find {
            offset: Some(7),
            limit: Some(5),
            ..by(order_by.clone())
        };
        let (page, next, _) = one_page(cat, own, "counter", jump).await?;
        let on = Find {
            cursor: next,
            limit: Some(5),
            ..by(order_by)
        };
        let (after, _, _) = one_page(cat, own, "counter", on).await?;
        ensure!(
            page == all[7..12] && after == all[12..17],
            "ordered by {what}, an offset of 7 answers the walk's rows 7 to 11 and its cursor \
             rows 12 to 16 ({:?}, {:?}); they answered {page:?} and {after:?}",
            &all[7..12],
            &all[12..17]
        );
    }
    let all = walk(own, "by_tag", Find::default()).await?;
    let jump = Find {
        offset: Some(1),
        ..Find::default()
    };
    let (page, _, _) = one_page(cat, own, "by_tag", jump).await?;
    ensure!(
        page == all[1..],
        "a view skips by offset as a stored type does: {:?}; it answered {page:?}",
        &all[1..]
    );
    let count = || Aggregate {
        measures: vec![agg("n", AggFunc::Count, None)],
        group_by: Vec::new(),
    };
    let jump = |find: Find| Find {
        offset: Some(1),
        ..find
    };
    for (what, find) in [
        (
            "`offset` beside a cursor",
            jump(Find {
                cursor: next,
                ..Find::default()
            }),
        ),
        (
            "`offset` beside `after`",
            jump(Find {
                after: Some(After {
                    values: Vec::new(),
                    key: "c01".into(),
                }),
                ..Find::default()
            }),
        ),
        (
            "`offset` beside an aggregate",
            jump(Find {
                aggregate: Some(count()),
                ..Find::default()
            }),
        ),
        (
            "`total` beside an aggregate",
            Find {
                total: true,
                aggregate: Some(count()),
                ..Find::default()
            },
        ),
    ] {
        let r = cat.find(own, &"counter".into(), find).await;
        ensure!(
            is_invalid(&r),
            "{what} is refused as invalid; it answered {r:?}"
        );
    }
    let max = limits().max_offset;
    let far = Find {
        offset: Some(max + 1),
        ..Find::default()
    };
    let r = cat.find(own, &"counter".into(), far).await;
    ensure!(
        matches!(refused_on(&r), Some(("offset", Refusal::TooLarge { max: m })) if *m == max),
        "an offset past the lake's {max} is refused as too_large, naming `offset`; it answered \
         {r:?}"
    );
    Ok(())
}

async fn a_walk_under_an_exact_text_in_an_indexed_order_visits_each_holder_once(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    let fields = [
        ("n", StorageKind::Number),
        ("tag", StorageKind::Text),
        ("m", StorageKind::Number),
    ];
    l.declare(type_of("counter", &fields)).await?;
    let Lake { cat, own } = &l;
    l.set("sort_index.counter", json!(["n"])).await?;
    // Two records in three hold `a`, and each `n` is held twice. Of the
    // holders one in five is retired and one in seven restricted, so a
    // page of three passes over rows and asks its engine for more than
    // one batch of them.
    let mut holders = Vec::new();
    for i in (0..60u32).filter(|i| i % 3 != 0) {
        holders.push((format!("c{i:02}"), (i * 7) % 30, i % 5 == 1, i % 7 == 2));
    }
    for i in 0..60u32 {
        let tag = if i % 3 == 0 { "b" } else { "a" };
        let fields = json!({"n": (i * 7) % 30, "tag": tag});
        l.upsert("counter", &format!("c{i:02}"), fields).await?;
    }
    // 72 hold `c`, each `n` twice. Of every sixteen of them in `n`'s
    // order three hold `m = 1`: one as it is, one retired and one
    // restricted. A page under `m = 1` passes over eight rows and more
    // for each record it answers, so most pages of a walk ask their
    // engine for more batches than an engine reads one by one (041,
    // extended: three), and what the engine reads before and after holds
    // a retired and a restricted record that the page would answer
    // otherwise. Pages of three and of five put an answered record first
    // after the hand-over, and after a later batch. In the second
    // sixteen the last record holds `m = 1` too, as it is: a page whose
    // three batches end on it has read, as its last row before the
    // hand-over, one it answers, and an engine that went on from that
    // row, not after it, would answer it twice.
    let key_of = |j: u32| format!("d{:02}", (j * 29) % 72);
    let mut rare = Vec::new();
    for j in 0..72u32 {
        let key = key_of(j);
        // Of the two that share the second sixteen's last `n`, the later
        // in key order.
        let sixteenth = j / 16 == 1 && matches!(j % 16, 14 | 15) && key > key_of(j ^ 1);
        let m = u32::from(matches!(j % 16, 0 | 1 | 5) || sixteenth);
        l.upsert("counter", &key, json!({"n": j / 2, "tag": "c", "m": m}))
            .await?;
        if m == 1 {
            rare.push((key, j / 2, j % 16 == 1, j % 16 == 5));
        }
    }
    for (key, _, retired, restricted) in holders.iter().chain(&rare) {
        if *retired {
            l.retire("counter", key).await?;
        } else if *restricted {
            let (ty, key) = ("counter".into(), key.as_str().into());
            let held = store.restrict(&ty, &key, "hold".into()).await;
            held.must("restricting a holder")?;
        }
    }
    let reader = Grants {
        default: Access::Read,
        ..Grants::default()
    };
    let reader = principal(store.as_ref(), PrincipalKind::Agent, "reader", reader);
    // A page of three asks its engine for a power of two, one of five
    // does not: an engine that fetches in powers of two then holds rows
    // past a batch for the next.
    let (all, few) = (("tag = 'a'", &holders), ("tag = 'c' and m = 1", &rare));
    let walks = [
        (all, own, true, 3),
        (all, own, true, 5),
        (all, &reader, false, 3),
        (few, own, true, 3),
        (few, own, true, 5),
        (few, &reader, false, 3),
        (few, &reader, false, 5),
    ];
    for ((filter, holders), who, sees_restricted, limit) in walks {
        for dir in [Direction::Asc, Direction::Desc] {
            let mut expected: Vec<(u32, &String)> = (holders.iter())
                .filter(|h| !h.2 && (sees_restricted || !h.3))
                .map(|h| (h.1, &h.0))
                .collect();
            expected.sort_by(|a, b| match dir {
                Direction::Asc => a.cmp(b),
                Direction::Desc => b.0.cmp(&a.0).then(a.1.cmp(b.1)),
            });
            let expected: Vec<&String> = expected.into_iter().map(|(_, key)| key).collect();
            let find = Find {
                filter: Some(filter.into()),
                order_by: vec![order("n", OrderBy::Value, dir)],
                limit: Some(limit),
                ..Find::default()
            };
            let walked = keys(&all_found(cat.as_ref(), who, "counter", find).await?);
            ensure!(
                walked.iter().eq(expected.iter().copied()),
                "paged by {limit} under `{filter}` in the indexed `n`'s order {dir:?}, a reader \
                 who sees restricted records ({sees_restricted}) is answered each record holding \
                 the text once, in order, the retired ones passed over: {expected:?}; the walk \
                 answered {walked:?}"
            );
        }
    }
    Ok(())
}

async fn a_read_that_names_no_limit_takes_the_smaller_bound(store: Store, given: Given) -> Held {
    let l = counter(store.as_ref()).await?;
    for i in 0..7 {
        l.upsert("counter", &format!("k{i}"), json!({"n": i}))
            .await?;
    }
    let max = Limits {
        max_limit: 5,
        ..limits()
    };
    let reopened = (given.relimit)(store, Seams::default(), max).await;
    let store = reopened.map_err(|e| Failure::new(format!("reopening at max_limit 5: {e}")))?;
    let Lake { cat, own } = Lake::of(store.as_ref());
    let (rows, next, _) = one_page(cat.as_ref(), &own, "counter", Find::default()).await?;
    ensure!(
        rows.len() == 5 && next.is_some(),
        "a `find` naming no limit under `max_limit` 5 must answer a page of 5 and a cursor; \
         it answered {rows:?}, cursor {next:?}"
    );
    let p = pattern("counter", None, Vec::new(), Projection::default());
    let QueryResult::Rows { rows, next_cursor } = cat
        .query(&own, p.clone())
        .await
        .must("a `query` naming no limit")?
        .data
    else {
        return Err(Failure::new("a `query` of rows answered another shape"));
    };
    ensure!(
        rows.len() == 5 && next_cursor.is_some(),
        "a `query` naming no limit under `max_limit` 5 must answer 5 rows and a cursor; it \
         answered {} rows, cursor {next_cursor:?}",
        rows.len()
    );
    let six = Find {
        limit: Some(6),
        ..Find::default()
    };
    let err = cat
        .find(&own, &"counter".into(), six)
        .await
        .must_fail("a `find` naming a limit of 6 under `max_limit` 5")?;
    ensure!(
        is_invalid(&err),
        "a `limit` of 6 above `max_limit` 5 must be refused as invalid; it failed with {err:?}"
    );
    Ok(())
}
