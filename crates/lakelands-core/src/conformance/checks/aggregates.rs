//! Aggregates over a set of records (`lakelands_api::read::Aggregate`,
//! `docs/decisions/011-storage-kind-and-value-type.md`).

use std::sync::Arc;

use lakelands_api::ontology::{FieldDecl, FieldType, StorageKind, ValueTypeDecl};
use lakelands_api::read::{AggFunc, AggMeasure, Aggregate};
use lakelands_api::write::Declaration;
use lakelands_api::{Find, Found, LakeStore, TypeName};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{Lake, Must, dec, decimal, field, type_decl};

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
        name: "money".into(),
        kind: StorageKind::Decimal,
        constraint: None,
        one_of: Vec::new(),
        format: None,
        scale: Some(2),
        description: None,
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

    let find = Find {
        aggregate: Some(Aggregate {
            measures: vec![
                AggMeasure {
                    name: "n".into(),
                    func: AggFunc::Count,
                    field: None,
                },
                AggMeasure {
                    name: "total".into(),
                    func: AggFunc::Sum,
                    field: Some("amount".into()),
                },
            ],
            group_by: Vec::new(),
        }),
        ..Find::default()
    };
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
