//! Aggregates over a set of records (`lakelands_api::read::Aggregate`,
//! `docs/decisions/011-storage-kind-and-value-type.md`).

use std::sync::Arc;

use lakelands_api::ontology::{FieldDecl, FieldType, StorageKind, ValueTypeDecl};
use lakelands_api::query::{Projection, QueryResult};
use lakelands_api::read::{AggFunc, Aggregate, ReadAt};
use lakelands_api::write::Declaration;
use lakelands_api::{Find, Found, LakeStore, Timestamp, TypeName, Value};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, agg, aggregate, all_found, count_view, dec, decimal, field, pattern, reads_all,
    restrict, type_decl, value_type,
};

check!(
    /// A count is a full walk's length.
    COUNT_IS_THE_WALK = a_count_answers_as_many_records_as_a_full_find_walk,
    "the read contract and 029: `find` with only `count()` answers as many records as a walk of every page of the same `find` returns, with no filter, a filter the engine applies and one only the store evaluates, at `valid_at`, at an earlier position and over a view, for a principal with the `restricted` grant and one without; a superseded state, a retired record and, without the grant, a restricted one are not counted"
);

check!(
    /// A group key reads a setting as `where` does.
    SETTING_IN_A_GROUP_KEY = a_group_key_reads_a_setting_as_a_where_reads_it,
    "the read contract (lakelands#563): an aggregate's named group key that calls `setting` reads the lake's setting, as the same call in `where` does, in `find` and in a pattern: `setting('threshold')` groups by its value, and in `find` `amount > setting('threshold')` splits the records it holds for from the rest"
);

check!(
    /// Nothing counts as zero.
    EMPTY_AGGREGATE = an_aggregate_over_no_records_answers_zero,
    "the read contract and 011: an aggregate over an empty set answers one group with a count of 0 and a sum of zero, kept at the field's scale"
);

async fn an_aggregate_over_no_records_answers_zero(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let money = ValueTypeDecl {
        scale: Some(2),
        ..value_type("money", StorageKind::Decimal)
    };
    cat.declare(own, Declaration::ValueType(money))
        .await
        .must("declaring the value type `money`")?;
    let amount = FieldDecl {
        ty: FieldType {
            value_type: Some("money".into()),
            ..FieldType::kind(StorageKind::Decimal)
        },
        ..field("amount", StorageKind::Decimal)
    };
    l.declare(type_decl("payment", vec![amount])).await?;

    let find = aggregate(vec![
        agg("n", AggFunc::Count, None),
        agg("total", AggFunc::Sum, Some("amount")),
    ]);
    let reply = cat
        .find(own, &TypeName::from("payment"), find)
        .await
        .must("aggregating over no payments")?;
    let Found::Groups { rows } = reply.data else {
        return Err(Failure::new("an aggregate answered records, not groups"));
    };
    ensure!(
        rows.len() == 1,
        "an aggregate without group_by over no records must answer one group; it \
         answered {} ({rows:?})",
        rows.len()
    );
    let values = &rows[0].values;
    ensure!(
        decimal(values.get("n")) == Some(dec("0")) && values.get("total") == Some(&json!("0.00")),
        "over no payments the count must be 0 and the sum of `amount` (scale 2) \
         \"0.00\", an aggregate's values being decimals; the group reads {values:?}"
    );
    Ok(())
}

async fn a_count_answers_as_many_records_as_a_full_find_walk(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let mut item = type_decl(
        "item",
        vec![
            field("from", StorageKind::DateTime),
            field("tag", StorageKind::Text),
            field("n", StorageKind::Number),
        ],
    );
    item.valid_from = Some("from".into());
    l.declare(item).await?;
    l.declare(count_view("by_tag", "item", "tag")).await?;
    for i in 0..30u32 {
        let tag = if i % 2 == 0 { "a" } else { "b" };
        let from = format!("2026-01-{:02}T00:00:00Z", i + 1);
        let key = format!("i{i:02}");
        l.upsert("item", &key, json!({"from": from, "tag": tag, "n": i}))
            .await?;
    }
    l.upsert("item", "i01", json!({"n": 101})).await?;
    for key in ["i03", "i04"] {
        l.retire("item", key).await?;
    }
    restrict(store.as_ref(), "item", "i05", "hold").await?;
    let before = store.head().await.must("reading the head")?;
    let later = json!({"from": "2026-02-01T00:00:00Z", "tag": "a", "n": 30});
    l.upsert("item", "i30", later).await?;
    let reader = reads_all(store.as_ref(), "reader");
    let mid = Timestamp::parse("2026-01-15T00:00:00Z").expect("an instant");
    let at = |as_of, valid_at| ReadAt { as_of, valid_at };
    let earlier = at(Some(before), None);
    let cases = [
        ("everything", "item", None, at(None, None)),
        ("n > 10", "item", Some("n > 10"), at(None, None)),
        (
            "a filter only the store evaluates",
            "item",
            Some("n > 10 and now() > now() - 1d"),
            at(None, None),
        ),
        ("valid at 2026-01-15", "item", None, at(None, Some(mid))),
        ("before i30 was written", "item", None, earlier),
        ("the view", "by_tag", None, at(None, None)),
    ];
    let mut everything = Vec::new();
    for (who, name) in [
        (own, "the owner"),
        (&reader, "a reader without `restricted`"),
    ] {
        for (what, ty, filter, at) in &cases {
            let find = || Find {
                filter: filter.map(Into::into),
                at: *at,
                ..Find::default()
            };
            let walked = all_found(cat.as_ref(), who, ty, find()).await?.len() as u64;
            let counting = Find {
                aggregate: aggregate(vec![agg("n", AggFunc::Count, None)]).aggregate,
                ..find()
            };
            let reply = cat
                .find(who, &(*ty).into(), counting)
                .await
                .must(&format!("{name} counting {what}"))?
                .data;
            let counted = match &reply {
                Found::Groups { rows } => rows
                    .first()
                    .and_then(|g| g.values.get("n"))
                    .and_then(Value::as_str)
                    .and_then(|n| n.parse::<u64>().ok()),
                Found::Records { .. } => None,
            };
            ensure!(
                counted == Some(walked),
                "{name} counting {what} must answer the {walked} records a full walk finds; \
                 it answered {reply:?}"
            );
            if *what == "everything" {
                everything.push(walked);
            }
        }
    }
    ensure!(
        everything == [29, 28],
        "31 records, two retired: the owner counts 29 and a reader without `restricted` 28, \
         `i05` being held; they counted {everything:?}"
    );
    Ok(())
}

async fn a_group_key_reads_a_setting_as_a_where_reads_it(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl(
        "bill",
        vec![field("amount", StorageKind::Decimal)],
    ))
    .await?;
    l.set("threshold", json!(10)).await?;
    for (key, amount) in [("b1", "5"), ("b2", "20"), ("b3", "30")] {
        l.upsert("bill", key, json!({"amount": amount})).await?;
    }
    let mut by: Aggregate = serde_json::from_value(json!({
        "measures": [{"name": "n", "func": "count"}],
        "group_by": [{"name": "t", "expr": "setting('threshold')"},
                     {"name": "over", "expr": "amount > setting('threshold')"}]}))
    .map_err(|e| Failure::new(format!("reading the aggregate: {e}")))?;
    let mut find = aggregate(Vec::new());
    find.aggregate = Some(by.clone());
    let reply = (l.cat).find(&l.own, &TypeName::from("bill"), find).await;
    let Found::Groups { rows } = reply.must("aggregating bills")?.data else {
        return Err(Failure::new("an aggregate answered records, not groups"));
    };
    by.group_by.truncate(1);
    let p = pattern("bill", None, Vec::new(), Projection::Aggregate(by));
    let reply = (l.cat).query(&l.own, p).await;
    let QueryResult::Groups { rows: walked } = reply.must("aggregating a pattern")?.data else {
        return Err(Failure::new("a pattern's aggregate answered no groups"));
    };
    let found =
        json!([[{"over": false, "t": "10"}, {"n": "1"}], [{"over": true, "t": "10"}, {"n": "2"}]]);
    let walk = json!([[{"t": "10"}, {"n": "3"}]]);
    for (how, rows, want) in [("find", rows, found), ("a pattern", walked, walk)] {
        let got: Vec<Value> = rows.iter().map(|g| json!([g.by, g.values])).collect();
        ensure!(
            json!(got) == want,
            "{how} grouped by `setting('threshold')`, the setting 10, over 5, 20 and 30 \
             answers {got:?}, not {want}"
        );
    }
    Ok(())
}
