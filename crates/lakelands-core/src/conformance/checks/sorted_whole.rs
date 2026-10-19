//! What a lake ordered by sorting the type is told to its host
//! (lakelands#1029).
//!
//! A `find` ordered first by a field the lake keeps no order index for
//! sorts the type (041). The lake counts those pages per field, with their
//! time, and `describe_type` tells them to one who reads the field, so a
//! host names in `sort_index.<type>` what its agents order by, from what
//! they did, not a guess.

use lakelands_api::read::{Direction, Find, Order, OrderBy};
use lakelands_api::{Access, TypeName};
use serde_json::json;

use crate::conformance::fixture::{Lake, Must, describe_type, field_access, reads_all, restrict};
use crate::conformance::{Held, Store};

check!(
    SORTED_WHOLE_TOLD = what_was_ordered_by_a_whole_sort_is_told,
    "041, extended (lakelands#1029): each `find` page ordered first, by value, by a scalar field the lake keeps no order index for is counted for that field with its time, and `describe_type` tells the count in `sorted_whole` to a holder of `schema` who reads the field, and to no other reader, whatever records are held from it; an order by key, by `known_at` or of a field it indexes is not counted, and a field named in `sort_index.<type>` is told in `sorted` and no longer in `sorted_whole`"
);

async fn what_was_ordered_by_a_whole_sort_is_told(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare_json(json!({"declare": "type", "name": "row", "key": ["no"],
        "fields": [{"name": "no", "type": {"kind": "text"}},
                   {"name": "n", "type": {"kind": "number"}},
                   {"name": "secret", "type": {"kind": "number"}}]}))
        .await?;
    for (k, n) in [("a", 3), ("b", 1), ("c", 2)] {
        l.upsert("row", k, json!({"no": k, "n": n, "secret": n * 10}))
            .await?;
    }
    let ty = TypeName::from("row");
    let page = async |field: &str, by: OrderBy| {
        let order_by = vec![Order {
            field: field.into(),
            by,
            dir: Direction::Asc,
        }];
        let f = Find {
            order_by,
            ..Find::default()
        };
        l.cat.find(&l.own, &ty, f).await.must("an ordered page")
    };
    for _ in 0..2 {
        page("n", OrderBy::Value).await?;
    }
    page("secret", OrderBy::Value).await?;
    page("n", OrderBy::KnownAt).await?;
    l.cat
        .find(&l.own, &ty, Find::default())
        .await
        .must("a page in key order")?;
    let told = |d: lakelands_api::describe::TypeDescription| {
        let w = d.lifecycle.sorted_whole.iter();
        w.map(|s| (s.field.to_string(), s.pages))
            .collect::<Vec<_>>()
    };
    let owner = told(l.describe_type("row").await?);
    let mut kept = field_access(store.as_ref(), "kept", "row", &[("secret", Access::None)]);
    kept.grants.schema = true;
    let theirs = told(describe_type(l.cat.as_ref(), &kept, "row").await?);
    ensure!(
        owner == [("n".to_owned(), 2), ("secret".to_owned(), 1)] && theirs == [("n".to_owned(), 2)],
        "the owner is told `n` sorted twice and `secret` once, one kept from `secret` only \
         `n`, and nothing of a key order or a `known_at` order: {owner:?}, {theirs:?}"
    );
    // Indexed, it is told in `sorted` and no more in `sorted_whole`.
    l.set("sort_index.row", json!(["n"])).await?;
    page("n", OrderBy::Value).await?;
    let d = l.describe_type("row").await?;
    let (sorted, whole) = (d.lifecycle.sorted.clone(), told(d));
    ensure!(
        sorted == [lakelands_api::FieldName::from("n")] && whole == [("secret".to_owned(), 1)],
        "once `n` is indexed it is told in `sorted`, and only `secret` in `sorted_whole`: \
         {sorted:?}, {whole:?}"
    );
    // The page the index answered was not counted.
    l.retire(lakelands_api::seam::SETTING_TYPE, "sort_index.row")
        .await?;
    let again = told(l.describe_type("row").await?);
    ensure!(
        again == [("n".to_owned(), 2), ("secret".to_owned(), 1)],
        "the page the index read is not counted: {again:?}"
    );
    // Twins, one holding a record restricted from the reader: one without
    // `schema` is told nothing of either, and a holder of it alike.
    for (twin, held) in [("twin_a", true), ("twin_b", false)] {
        l.declare_json(json!({"declare": "type", "name": twin, "key": ["no"],
            "fields": [{"name": "no", "type": {"kind": "text"}},
                       {"name": "n", "type": {"kind": "number"}}]}))
            .await?;
        for k in ["a", "b"] {
            l.upsert(twin, k, json!({"no": k, "n": 1})).await?;
        }
        if held {
            restrict(store.as_ref(), twin, "b", "held").await?;
        }
        let order_by = vec![Order {
            field: "n".into(),
            by: OrderBy::Value,
            dir: Direction::Asc,
        }];
        let f = Find {
            order_by,
            ..Find::default()
        };
        l.cat
            .find(&l.own, &TypeName::from(twin), f)
            .await
            .must("an ordered page")?;
    }
    let plain = reads_all(store.as_ref(), "plain");
    let mut architect = reads_all(store.as_ref(), "architect");
    architect.grants.schema = true;
    let mut said = Vec::new();
    for twin in ["twin_a", "twin_b"] {
        let p = told(describe_type(l.cat.as_ref(), &plain, twin).await?);
        let a = told(describe_type(l.cat.as_ref(), &architect, twin).await?);
        said.push((p, a));
    }
    let one = vec![("n".to_owned(), 1)];
    ensure!(
        said.iter().all(|(p, a)| p.is_empty() && *a == one),
        "one without `schema` is told nothing of either twin, a holder of it the same of both: \
         {said:?}"
    );
    Ok(())
}
