//! Who may retire a record (`docs/decisions/006-grants-not-roles.md`,
//! `lakelands_api::describe::TypeRights`), and what a read before the
//! retirement still sees.

use lakelands_api::ontology::StorageKind::{Decimal, Text};

use crate::conformance::fixture::{get_at, type_of};
use crate::conformance::prelude::*;

check!(
    RETIRE_NEEDS_EVERY_FIELD = a_principal_who_may_write_only_some_fields_cannot_retire,
    "006 and TypeRights::retire: retiring removes every field of a record, so it needs a write grant on every field of the type, checked before the record is looked up, and the record before the expected version (016, extended); a retirement that is proposed answers in the same order"
);

check!(
    SOURCE_ONLY_HAS_NO_UPSERT = a_type_whose_fields_all_come_from_a_source_has_no_upsert,
    "016 and TypeRights::upsert: upsert writes authored fields; a type with none has no upsert for anyone, the owner included: describe says so, and the door refuses it as the wrong door, naming the field"
);

async fn a_type_whose_fields_all_come_from_a_source_has_no_upsert(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_decl(
        "rate",
        vec![
            sourced("currency", StorageKind::Text),
            sourced("value", StorageKind::Decimal),
        ],
    ))
    .await?;
    let d = cat
        .describe_type(own, &TypeName::from("rate"), ReadAt::default())
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
            own,
            &TypeName::from("rate"),
            &"eur".into(),
            crate::conformance::fixture::patch(json!({"currency": "EUR"})),
            WriteOpts::default(),
        )
        .await
        .must_fail("the owner upserting `rate`, whose fields all come from a source")?;
    head_unmoved(store.as_ref(), before, "upserting a source-only type").await?;
    ensure!(
        matches!(refused_on(&err), Some(("currency", Refusal::WrongDoor))),
        "an upsert of `rate`'s source field `currency` must be refused as the wrong door, \
         naming `currency`; it failed with {err:?}"
    );
    Ok(())
}

check!(
    READABLE_BEFORE_RETIREMENT = a_record_retired_at_a_position_is_readable_just_before_it,
    "009 and 001: a read at the position before a retirement sees the record as it was, to a principal who could read it then; a session told of the retirement reads it there"
);

async fn a_principal_who_may_write_only_some_fields_cannot_retire(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, .. } = &l;
    l.declare(type_of("invoice", &[("status", Text), ("amount", Decimal)]))
        .await?;
    l.upsert("invoice", "i", json!({"status": "open", "amount": "1.00"}))
        .await?;
    let clerk = field_writer(store.as_ref(), "clerk", "invoice", &["status"]);
    let invoice = TypeName::from("invoice");

    let d = cat.describe_type(&clerk, &invoice, ReadAt::default());
    let d = d.await.must("describing `invoice` as the clerk")?.data;
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
        matches!(reason(&err), Some(Refusal::NotGranted)),
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
    // In retire's order, the grant before the record, the record before
    // the version (016, extended).
    let gone = lakelands_api::RecordKey::from("gone");
    let ungranted = cat.retire(&clerk, &invoice, &gone, WriteOpts::default());
    let ungranted = ungranted.await;
    let at = WriteOpts {
        expected_version: Some(lakelands_api::Version(3)),
        ..WriteOpts::default()
    };
    let missing = cat.retire(&all, &invoice, &gone, at.clone()).await;
    ensure!(
        matches!(reason(&ungranted), Some(Refusal::NotGranted))
            && matches!(missing, Err(lakelands_api::Error::NotFound { .. })),
        "retiring a key never written is refused for the grant first, then as not found \
         before a version is compared; it answered {ungranted:?} and {missing:?}"
    );
    let proposed = async |who, opts| {
        let (ty, key) = (invoice.clone(), gone.clone());
        let write = lakelands_api::write::ProposedWrite::Retire { ty, key, opts };
        cat.propose(who, write, "gone".into()).await
    };
    let ungranted = proposed(&clerk, WriteOpts::default()).await;
    let missing = proposed(&all, at).await;
    ensure!(
        matches!(reason(&ungranted), Some(Refusal::NotGranted))
            && matches!(missing, Err(lakelands_api::Error::NotFound { .. })),
        "a retirement that is proposed answers in `retire`'s order, the grant, then the \
         record, then the version; it answered {ungranted:?} and {missing:?}"
    );
    cat.retire(&all, &invoice, &"i".into(), WriteOpts::default())
        .await
        .must("retiring `i` as a principal who may write every field")?;
    Ok(())
}

async fn a_record_retired_at_a_position_is_readable_just_before_it(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, .. } = &l;
    l.declare(type_of("invoice", &[("status", Text)])).await?;
    l.upsert("invoice", "i1", json!({"status": "open"})).await?;
    let reader = field_writer(store.as_ref(), "reader", "invoice", &[]);
    let invoice = TypeName::from("invoice");
    let retired = l.retire(invoice.as_str(), "i1").await?.position();
    let before = lakelands_api::Position(retired.0 - 1);
    let then = cat
        .get(&reader, &invoice, get_at("i1", Some(before)))
        .await
        .must(&format!(
            "reading `i1` at {before}, just before its retirement"
        ))?
        .data;
    ensure!(
        then.fields.get("status") == Some(&json!("open")),
        "`i1` was retired at {retired}; a reader reading it at {before} must see it as it was, \
         `status` \"open\"; it read {:?}",
        then.fields
    );
    let now = cat.get(&reader, &invoice, get_at("i1", Some(retired)));
    let now = now.await;
    ensure!(
        matches!(now, Err(Error::NotFound { .. })),
        "at {retired}, the position of its retirement, `i1` is gone; the read answered {:?}",
        now.map(|r| r.data.key)
    );
    Ok(())
}
