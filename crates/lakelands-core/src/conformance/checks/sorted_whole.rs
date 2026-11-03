//! What a lake ordered by sorting the type is told to its host
//! (lakelands#1029).
//!
//! A `find` ordered first by a field the lake keeps no order index for
//! sorts the type (041). The lake counts those pages per field, with their
//! time, and `describe_type` tells them to one who reads the field, so a
//! host names in `sort_index.<type>` what its agents order by, from what
//! they did, not a guess.

use lakelands_api::read::{Find, Order};

use crate::conformance::prelude::*;

check!(
    SORTED_WHOLE_TOLD = what_was_ordered_by_a_whole_sort_is_told,
    "041, extended (lakelands#1029): each `find` page ordered first, by value, by a scalar field the lake keeps no order index for is counted for that field with its time, and `describe_type` tells the count in `sorted_whole` to a holder of `schema` who reads the field, and to no other reader, whatever records are held from it; an order by key, by `known_at` or of a field it indexes is not counted, and a field named in `sort_index.<type>` is told in `sorted` and no longer in `sorted_whole`"
);

check!(
    SORTED_WHOLE_BESIDE_A_LOOKUP_BUILD = a_whole_page_beside_a_lookup_build_is_weighed,
    "041 and 099, R7 (lakelands#1029): a `lookup_index.<type>` setting names no `sorted` field and builds in parts on an engine that keeps an exact-text index, and a page ordered first by its field still sorts the type whole meanwhile: `describe_type` names the field `building` and goes on telling its `sorted_whole` count, and the count goes on after the build is whole",
    given
);

async fn a_whole_page_beside_a_lookup_build_is_weighed(store: Store, given: Given) -> Held {
    let size = u64::from(given.large_lake.clamp(400, 800));
    let limit = u32::try_from(size).unwrap_or(u32::MAX);
    let store = super::builds::chunked_alone(&given, store).await?;
    let l = super::builds::rows(store.as_ref(), size).await?;
    let before = whole_page(&l, "s", limit).await?;
    ensure!(
        before.len() == usize::try_from(size).unwrap_or(usize::MAX),
        "a page ordered by `s` sorts the whole type before the setting: {} keys, not {size}",
        before.len()
    );
    // `rows` writes `s` as `s{i % 17}` under the key `r{i:05}`, a text
    // field sorted by its value: a page sorted whole holds the values in
    // their order, ties in any order.
    let rank = |k: &str| -> Option<String> {
        let i: u64 = k.strip_prefix('r')?.parse().ok()?;
        Some(format!("s{}", i % 17))
    };
    let values = |keys: &[String]| -> Vec<String> { keys.iter().filter_map(|k| rank(k)).collect() };
    ensure!(
        values(&before).windows(2).all(|w| w[0] <= w[1]),
        "a page ordered by `s` sorts the whole type: its keys are in the field's order"
    );
    let setter = {
        let l = &l;
        async move {
            l.set("lookup_index.row", json!(["s"])).await?;
            Ok::<_, Failure>(())
        }
    };
    let beside = std::sync::Arc::new(std::sync::Mutex::new(
        None::<(bool, lakelands_api::describe::TypeDescription, Vec<String>)>,
    ));
    let reader = {
        let beside = beside.clone();
        let l = &l;
        async move {
            // An engine whose exact-text index is built at the setting's
            // own turn has no parts to read beside: the mark is seen, or
            // the build is whole before a read can be taken beside it.
            let building = super::builds::until_building_within(l, "row", "s", 400).await?;
            let d = l.describe_type("row").await?;
            let during = whole_page(l, "s", limit).await?;
            *beside.lock().expect("the read beside the build") = Some((building, d, during));
            Ok::<_, Failure>(())
        }
    };
    let out = join_all(vec![Box::pin(setter), Box::pin(reader)]).await;
    out[0]
        .as_ref()
        .map_err(|e| Failure::new(format!("the setting: {e}")))?;
    out[1]
        .as_ref()
        .map_err(|e| Failure::new(format!("a read beside the build: {e}")))?;
    let (building, d, during) = beside
        .lock()
        .expect("the read beside the build")
        .take()
        .ok_or_else(|| Failure::new("the read beside the build answered nothing"))?;
    ensure!(
        during == before,
        "a page ordered by `s` beside the setting is the whole type in its order, what it \
         answered before it: {} keys, not {}",
        during.len(),
        before.len()
    );
    let names = |fs: &[lakelands_api::FieldName]| -> Vec<String> {
        fs.iter().map(|f| f.to_string()).collect()
    };
    if building {
        // The build has parts here: its mark names the field, and does not
        // hide the count of the page taken before the setting.
        ensure!(
            names(&d.lifecycle.building) == ["s"],
            "`describe` names the setting's field `building`; it said {:?}",
            names(&d.lifecycle.building)
        );
        ensure!(
            d.lifecycle.sorted.is_empty(),
            "a `lookup_index.<type>` setting names no `sorted` field; `describe` said {:?}",
            names(&d.lifecycle.sorted)
        );
        ensure!(
            weighed(&d, "s") == Some(1),
            "a page ordered by `s` sorts the whole type while the setting builds, and its \
             count is told: `describe` said {:?}",
            weighed(&d, "s")
        );
    }
    // The page beside the setting was a whole sort too: the count goes on.
    let d = l.describe_type("row").await?;
    ensure!(
        d.lifecycle.building.is_empty(),
        "the setting is whole and nothing is `building`; `describe` said {:?}",
        names(&d.lifecycle.building)
    );
    ensure!(
        d.lifecycle.sorted.is_empty(),
        "a `lookup_index.<type>` setting names no `sorted` field; `describe` said {:?}",
        names(&d.lifecycle.sorted)
    );
    ensure!(
        weighed(&d, "s") == Some(2),
        "both pages ordered by `s` were whole sorts and the setting's build does not hide \
         them: `describe` said {:?}",
        weighed(&d, "s")
    );
    Ok(())
}

/// The keys of a page ordered by `field`'s value, the whole type.
async fn whole_page(l: &Lake, field: &str, limit: u32) -> Result<Vec<String>, Failure> {
    let find = Find {
        order_by: vec![Order {
            field: field.into(),
            by: OrderBy::Value,
            dir: Direction::Asc,
        }],
        limit: Some(limit),
        ..Find::default()
    };
    match l
        .cat
        .find(&l.own, &"row".into(), find)
        .await
        .must("a page ordered by a field")?
        .data
    {
        lakelands_api::read::Found::Records { rows, .. } => {
            Ok(rows.iter().map(|r| r.key.to_string()).collect())
        }
        other => Err(Failure::new(format!(
            "a page of records answered {other:?}"
        ))),
    }
}

/// The pages `field` was ordered by a whole sort, as `describe` tells them.
fn weighed(d: &lakelands_api::describe::TypeDescription, field: &str) -> Option<u64> {
    d.lifecycle
        .sorted_whole
        .iter()
        .find(|s| s.field.as_str() == field)
        .map(|s| s.pages)
}

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
