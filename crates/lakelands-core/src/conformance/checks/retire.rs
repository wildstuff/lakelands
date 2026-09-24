//! Who may retire a record (`docs/decisions/006-grants-not-roles.md`,
//! `lakelands_api::describe::TypeRights`).

use std::sync::Arc;

use lakelands_api::ontology::{Authority, StorageKind};
use lakelands_api::{Access, Error, LakeStore, ReadAt, Refusal, TypeName, WriteOpts};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, declare, doors, field, field_writer, head_unmoved, owner, type_decl, upsert,
};

check!(
    /// Retiring takes a write grant on every field.
    RETIRE_NEEDS_EVERY_FIELD = a_principal_who_may_write_only_some_fields_cannot_retire,
    "006 and TypeRights::retire: retiring removes every field of a record, so it needs a write grant on every field of the type"
);

check!(
    /// A type whose fields all come from a source has no `upsert`.
    SOURCE_ONLY_HAS_NO_UPSERT = a_type_whose_fields_all_come_from_a_source_has_no_upsert,
    "016 and TypeRights::upsert: upsert writes authored fields; a type with none has no upsert for anyone, the owner included: describe says so, and the door refuses it as the wrong door, naming the field"
);

async fn a_type_whose_fields_all_come_from_a_source_has_no_upsert(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let sourced = |name: &str, kind| {
        let mut f = field(name, kind);
        f.authority = Authority::Source;
        f
    };
    declare(
        cat.as_ref(),
        &own,
        type_decl(
            "rate",
            vec![
                sourced("currency", StorageKind::Text),
                sourced("value", StorageKind::Decimal),
            ],
        ),
    )
    .await?;
    let d = cat
        .describe_type(&own, &TypeName::from("rate"), ReadAt::default())
        .await
        .must("describing `rate` as the owner")?
        .data;
    ensure!(
        !d.may.upsert && d.may.find && d.may.get,
        "every field of `rate` comes from a source, so no one may upsert it, the owner \
         included, while the owner still finds and gets it; describe says {:?}",
        d.may
    );
    // The door agrees with describe: a direct upsert is the wrong door.
    let before = store.head().await.must("reading the head")?;
    let err = cat
        .upsert(
            &own,
            &TypeName::from("rate"),
            &"eur".into(),
            crate::conformance::fixture::patch(json!({"currency": "EUR"})),
            WriteOpts::default(),
        )
        .await
        .must_fail("the owner upserting `rate`, whose fields all come from a source")?;
    head_unmoved(store.as_ref(), before, "upserting a source-only type").await?;
    ensure!(
        matches!(
            &err,
            Error::Refused {
                field: Some(f),
                reason: Refusal::WrongDoor,
            } if f.as_str() == "currency"
        ),
        "an upsert of `rate`'s source field `currency` must be refused as the wrong door, \
         naming `currency`; it failed with {err:?}"
    );
    Ok(())
}

async fn a_principal_who_may_write_only_some_fields_cannot_retire(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(
        cat.as_ref(),
        &own,
        type_decl(
            "invoice",
            vec![
                field("status", StorageKind::Text),
                field("amount", StorageKind::Decimal),
            ],
        ),
    )
    .await?;
    upsert(
        cat.as_ref(),
        &own,
        "invoice",
        "i",
        json!({"status": "open", "amount": "1.00"}),
    )
    .await?;
    let clerk = field_writer(store.as_ref(), "clerk", "invoice", &["status"]);
    let invoice = TypeName::from("invoice");

    let d = cat
        .describe_type(&clerk, &invoice, ReadAt::default())
        .await
        .must("describing `invoice` as the clerk")?
        .data;
    ensure!(
        d.may.upsert && !d.may.retire,
        "a clerk who may write only `status` may upsert but not retire; describe says \
         {:?}",
        d.may
    );
    let before = store.head().await.must("reading the head")?;
    let err = cat
        .retire(&clerk, &invoice, &"i".into(), WriteOpts::default())
        .await
        .must_fail("the clerk retiring `i`")?;
    head_unmoved(store.as_ref(), before, "the clerk retiring `i`").await?;
    ensure!(
        matches!(
            err,
            Error::Refused {
                reason: Refusal::NotGranted,
                ..
            }
        ),
        "retiring as a principal who may write only `status` must be refused as not \
         granted; it failed with {err:?}"
    );
    // Write on the type as a whole, except `amount`: still not every field.
    let mut most = field_writer(store.as_ref(), "most", "invoice", &["status"]);
    let grants = most
        .grants
        .types
        .get_mut(&invoice)
        .expect("field_writer grants the type");
    grants.default = Some(Access::Write);
    grants.fields.insert("amount".into(), Access::Read);
    let before = store.head().await.must("reading the head")?;
    cat.retire(&most, &invoice, &"i".into(), WriteOpts::default())
        .await
        .must_fail("retiring `i` with write on the type but not on `amount`")?;
    head_unmoved(store.as_ref(), before, "retiring without write on `amount`").await?;

    let all = field_writer(store.as_ref(), "keeper", "invoice", &["status", "amount"]);
    cat.retire(&all, &invoice, &"i".into(), WriteOpts::default())
        .await
        .must("retiring `i` as a principal who may write every field")?;
    Ok(())
}
