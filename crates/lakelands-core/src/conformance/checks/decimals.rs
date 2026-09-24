//! Decimals stay exact (`docs/log-format.md` §5,
//! `docs/decisions/011-storage-kind-and-value-type.md`).
//!
//! A `decimal` value is a string of digits carried as written, so money
//! never passes through binary floating point: it reads back byte for
//! byte, and a sum over it is exact. Written once as a rule here because
//! every engine stores decimals differently, and a float anywhere on the
//! way is invisible until a total is a cent off.

use std::sync::Arc;

use lakelands_api::ontology::StorageKind;
use lakelands_api::read::{AggFunc, AggMeasure, Aggregate};
use lakelands_api::{FieldName, Find, Found, LakeStore, TypeName};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, all, dec, decimal, declare, doors, field, owner, type_decl, upsert, value,
};

check!(
    /// A decimal reads back as it was written.
    AS_WRITTEN = a_decimal_reads_back_exactly_as_it_was_written,
    "the log format: a decimal is a string of digits kept as written, trailing zeros and all, beyond any float's precision"
);

check!(
    /// A sum over decimals is exact.
    EXACT_SUM = a_sum_over_decimals_is_exact,
    "the engine seam: sum is exact for decimals, so 0.10 + 0.20 + 0.30 is 0.60 and not a float's approximation"
);

check!(
    /// A scaled decimal keeps its scale when derived and summed.
    SCALE_KEPT = a_scaled_decimal_keeps_its_scale_when_derived_and_summed,
    "011: a value is kept at its value type's scale, so a derived decimal is rounded to it before it is stored, and a sum over a scaled field answers at that scale"
);

const AMOUNTS: [&str; 6] = [
    "8500.125",
    "10.00",
    "100.5",
    "-0.10",
    "0",
    "12345678901234567890.123456789",
];

async fn a_decimal_reads_back_exactly_as_it_was_written(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let ty = type_decl("payment", vec![field("amount", StorageKind::Decimal)]);
    declare(cat.as_ref(), &own, ty).await?;
    for (i, amount) in AMOUNTS.iter().enumerate() {
        let key = format!("m{i}");
        upsert(
            cat.as_ref(),
            &own,
            "payment",
            &key,
            json!({"amount": amount}),
        )
        .await?;
    }

    let rows = all(cat.as_ref(), &own, "payment").await?;
    ensure!(
        rows.len() == AMOUNTS.len(),
        "`find` listed {} payments; {} were written",
        rows.len(),
        AMOUNTS.len()
    );
    for (i, amount) in AMOUNTS.iter().enumerate() {
        let key = format!("m{i}");
        let Some(rec) = rows.iter().find(|r| r.key.as_str() == key) else {
            return Err(Failure::new(format!("`find` did not list payment `{key}`")));
        };
        ensure!(
            value(rec, "amount") == Some(&json!(amount)),
            "the decimal {amount:?} written to `{key}` reads back as {:?}; a decimal is \
             kept exactly as written, as a string",
            value(rec, "amount")
        );
    }
    Ok(())
}

async fn a_sum_over_decimals_is_exact(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let ty = type_decl("payment", vec![field("amount", StorageKind::Decimal)]);
    declare(cat.as_ref(), &own, ty).await?;
    for (key, amount) in [("a", "0.10"), ("b", "0.20"), ("c", "0.30")] {
        upsert(
            cat.as_ref(),
            &own,
            "payment",
            key,
            json!({"amount": amount}),
        )
        .await?;
    }

    let find = Find {
        aggregate: Some(Aggregate {
            measures: vec![AggMeasure {
                name: FieldName::from("total"),
                func: AggFunc::Sum,
                field: Some(FieldName::from("amount")),
            }],
            group_by: Vec::new(),
        }),
        ..Find::default()
    };
    let reply = cat
        .find(&own, &TypeName::from("payment"), find)
        .await
        .must("summing `amount` over the payments")?;
    let Found::Groups { rows } = reply.data else {
        return Err(Failure::new(
            "`find` with an aggregate must answer groups; it answered records",
        ));
    };
    let [group] = rows.as_slice() else {
        return Err(Failure::new(format!(
            "a sum without `group_by` is one group; `find` answered {}",
            rows.len()
        )));
    };
    let total = group.values.get("total");
    ensure!(
        decimal(total) == Some(dec("0.60")),
        "the sum of the decimals 0.10, 0.20 and 0.30 must be exactly 0.60, as a decimal \
         string; it is {total:?}"
    );
    Ok(())
}

async fn a_scaled_decimal_keeps_its_scale_when_derived_and_summed(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let money: lakelands_api::write::Declaration = serde_json::from_value(json!({
        "declare": "value_type", "name": "money", "kind": "decimal", "scale": 2
    }))
    .map_err(|e| Failure::new(format!("the check's value type does not read: {e}")))?;
    cat.declare(&own, money)
        .await
        .must("declaring the value type `money`")?;
    let money_field = |name: &str| {
        let mut f = field(name, StorageKind::Decimal);
        f.ty.value_type = Some("money".into());
        f
    };
    let mut gross = money_field("gross");
    gross.derived = Some(lakelands_api::expr::Condition::from("net * 1.19"));
    gross.authority = lakelands_api::ontology::Authority::Derived;
    declare(
        cat.as_ref(),
        &own,
        type_decl("invoice", vec![money_field("net"), gross]),
    )
    .await?;
    for (key, net) in [("a", "4000.00"), ("b", "5000"), ("c", "0.05")] {
        upsert(cat.as_ref(), &own, "invoice", key, json!({"net": net})).await?;
    }
    let rows = all(cat.as_ref(), &own, "invoice").await?;
    let grosses: Vec<Option<&serde_json::Value>> = ["a", "b", "c"]
        .iter()
        .map(|k| {
            rows.iter()
                .find(|r| r.key.as_str() == *k)
                .and_then(|r| value(r, "gross"))
        })
        .collect();
    ensure!(
        grosses
            == [
                Some(&json!("4760.00")),
                Some(&json!("5950.00")),
                Some(&json!("0.06"))
            ],
        "`gross = net * 1.19` of 4000.00, 5000 and 0.05 in `money` (scale 2) stored {grosses:?}; \
         a derived value is rounded half away from zero to its scale before it is stored: \
         4760.00, 5950.00, 0.06"
    );
    let find = Find {
        aggregate: Some(Aggregate {
            measures: vec![AggMeasure {
                name: FieldName::from("total"),
                func: AggFunc::Sum,
                field: Some(FieldName::from("gross")),
            }],
            group_by: Vec::new(),
        }),
        ..Find::default()
    };
    let reply = cat
        .find(&own, &TypeName::from("invoice"), find)
        .await
        .must("summing `gross`")?;
    let Found::Groups { rows } = reply.data else {
        return Err(Failure::new("`find` with an aggregate must answer groups"));
    };
    let total = rows.first().and_then(|g| g.values.get("total"));
    ensure!(
        total == Some(&json!("10710.06")),
        "the sum of `gross` in `money` (scale 2) is {total:?}; it must be \"10710.06\", at the \
         field's scale"
    );
    Ok(())
}
