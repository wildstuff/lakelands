//! Paging and ordering of `find` (`lakelands_api::read`, `docs/glossary.md`).
//!
//! A page set is a total order over the matched records: walking the
//! cursor visits every record once, with no repeat and no gap. Ordering
//! arranges the set and never narrows it: a record without a value for
//! the sort field is still listed, last, in either direction. Both are
//! where two engines drift apart silently, each consistent with itself.

use std::collections::BTreeSet;
use std::sync::Arc;

use lakelands_api::ontology::StorageKind;
use lakelands_api::read::{Direction, Order, OrderBy};
use lakelands_api::{FieldName, Find, LakeStore, Record};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{Lake, declare, doors, field, owner, type_decl, value};

check!(
    /// Walking the cursor visits each record once.
    EVERY_RECORD_ONCE = paging_through_find_visits_every_record_exactly_once,
    "the read contract: a cursor continues where its page ended, so a page set is a total order with no repeat and no gap"
);

check!(
    /// Records without the sort value come last, both ways.
    MISSING_SORT_VALUES_LAST = records_without_the_sort_field_are_listed_last_in_either_direction,
    "the read contract: records without a value for a sort key sort last in either direction; ordering never drops them"
);

async fn counter(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let cat = doors(store);
    let ty = type_decl(
        "counter",
        vec![
            field("n", StorageKind::Number),
            field("tag", StorageKind::Text),
        ],
    );
    declare(cat.as_ref(), &owner(store), ty).await?;
    Ok(Lake::of(store))
}

fn keys(rows: &[Record]) -> Vec<String> {
    rows.iter().map(|r| r.key.as_str().to_owned()).collect()
}

async fn paging_through_find_visits_every_record_exactly_once(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    const TOTAL: usize = 25;
    let l = counter(store.as_ref()).await?;
    for i in 0..TOTAL {
        let key = format!("k{i:03}");
        l.upsert("counter", &key, json!({"n": i % 4})).await?;
    }

    for order_by in [
        Vec::new(),
        vec![Order {
            field: FieldName::from("n"),
            by: OrderBy::Value,
            dir: Direction::Desc,
        }],
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

async fn records_without_the_sort_field_are_listed_last_in_either_direction(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
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
        let order = vec![Order {
            field: FieldName::from("n"),
            by: OrderBy::Value,
            dir,
        }];
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
