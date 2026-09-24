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
