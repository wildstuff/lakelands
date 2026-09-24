//! Derived fields: computed by the fold, written by no door
//! (`lakelands_api::ontology::Authority::Derived`, `docs/log-format.md`
//! §2.3).
//!
//! An author declares `gross = amount * 1.19` once and writes only
//! `amount`; the store fills `gross` in the same event, with `derived` as
//! its origin, and a replay reproduces it. A store that accepted the
//! declaration and then answered records without the field would leave
//! every total built on it quietly short, which is why the check reads
//! the field back rather than trusting the declaration.

use std::sync::Arc;

use lakelands_api::ontology::{Authority, FieldDecl, StorageKind};
use lakelands_api::{Error, FieldName, LakeStore, Origin, Refusal, TypeName};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, dec, decimal, declare, doors, field, get_at, head_unmoved, owner, patch, read, type_decl,
    upsert,
};

check!(
    /// The fold computes a derived field.
    COMPUTED_BY_THE_FOLD = a_derived_field_is_computed_by_the_fold_on_every_write,
    "the ontology and the log format: the fold computes a derived field from its expression in the event that changes its inputs"
);

check!(
    /// No door writes a derived field.
    NO_DOOR_WRITES_IT = a_derived_field_written_through_a_door_is_refused_as_the_wrong_door,
    "015 and the ontology: no door writes a derived field; a patch carrying one is refused whole as the wrong door"
);

check!(
    /// A derived field added later is computed from then on.
    ADDED_LATER = a_derived_field_added_to_a_live_type_is_computed_on_later_writes,
    "the ontology: a derived field added with add_field is kept by the fold for every write after it, like one declared with the type"
);

fn gross() -> FieldDecl {
    let mut gross = field("gross", StorageKind::Decimal);
    gross.authority = Authority::Derived;
    gross.derived = Some("amount * 1.19".into());
    gross.depends_on = vec![FieldName::from("amount")];
    gross
}

async fn a_derived_field_is_computed_by_the_fold_on_every_write(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let ty = type_decl(
        "invoice",
        vec![field("amount", StorageKind::Decimal), gross()],
    );
    declare(cat.as_ref(), &own, ty).await?;

    let first = upsert(
        cat.as_ref(),
        &own,
        "invoice",
        "i1",
        json!({"amount": "100.00"}),
    )
    .await?;
    let rec = read(cat.as_ref(), &own, "invoice", "i1").await?;
    let got = rec.fields.get("gross");
    ensure!(
        decimal(got) == Some(dec("119")),
        "`gross` is declared as amount * 1.19; after writing amount 100.00 it must read \
         exactly 119 as a decimal string, but reads {got:?}. A declaration the store \
         accepted is a field it owes"
    );
    let origin = rec.meta.get("gross").map(|m| m.origin);
    ensure!(
        origin == Some(Origin::Derived),
        "a field the fold writes carries origin `derived` in its provenance; `gross` \
         carries {origin:?}"
    );

    upsert(
        cat.as_ref(),
        &own,
        "invoice",
        "i1",
        json!({"amount": "200.00"}),
    )
    .await?;
    let rec = read(cat.as_ref(), &own, "invoice", "i1").await?;
    let got = rec.fields.get("gross");
    ensure!(
        decimal(got) == Some(dec("238")),
        "after amount changed to 200.00, `gross` must follow to 238; it reads {got:?}"
    );

    let then = cat
        .get(
            &own,
            &TypeName::from("invoice"),
            get_at("i1", Some(first.position())),
        )
        .await
        .must("reading `i1` as of its first write")?;
    let got = then.data.fields.get("gross");
    ensure!(
        decimal(got) == Some(dec("119")),
        "as of the first write, `gross` must read 119 as it did then; it reads {got:?}"
    );
    Ok(())
}

async fn a_derived_field_written_through_a_door_is_refused_as_the_wrong_door(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let ty = type_decl(
        "invoice",
        vec![field("amount", StorageKind::Decimal), gross()],
    );
    declare(cat.as_ref(), &own, ty).await?;

    let before = store.head().await.must("reading the head")?;
    let err = cat
        .upsert(
            &own,
            &TypeName::from("invoice"),
            &"i1".into(),
            patch(json!({"amount": "100.00", "gross": "1.00"})),
            Default::default(),
        )
        .await
        .must_fail("the owner writing the derived field `gross`")?;
    head_unmoved(
        store.as_ref(),
        before,
        "the owner writing the derived field `gross`",
    )
    .await?;
    ensure!(
        matches!(
            &err,
            Error::Refused { field: Some(f), reason: Refusal::WrongDoor } if f.as_str() == "gross"
        ),
        "a patch carrying the derived field `gross` must be refused as the wrong door on \
         `gross`, even for the owner; it failed with {err:?}"
    );
    let missing = cat
        .get(&own, &TypeName::from("invoice"), get_at("i1", None))
        .await;
    ensure!(
        matches!(missing, Err(Error::NotFound { .. })),
        "the refused patch created `i1` anyway: {missing:?}"
    );
    Ok(())
}

async fn a_derived_field_added_to_a_live_type_is_computed_on_later_writes(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let ty = type_decl("invoice", vec![field("amount", StorageKind::Decimal)]);
    declare(cat.as_ref(), &own, ty).await?;
    upsert(
        cat.as_ref(),
        &own,
        "invoice",
        "i1",
        json!({"amount": "100.00"}),
    )
    .await?;

    cat.add_field(&own, &TypeName::from("invoice"), gross())
        .await
        .must("adding the derived field `gross` to the live type `invoice`")?;

    upsert(
        cat.as_ref(),
        &own,
        "invoice",
        "i2",
        json!({"amount": "100.00"}),
    )
    .await?;
    let rec = read(cat.as_ref(), &own, "invoice", "i2").await?;
    let got = rec.fields.get("gross");
    ensure!(
        decimal(got) == Some(dec("119")),
        "`gross` was added by add_field; a record written afterwards with amount 100.00 \
         must read gross 119, but reads {got:?}"
    );

    upsert(
        cat.as_ref(),
        &own,
        "invoice",
        "i1",
        json!({"amount": "10.00"}),
    )
    .await?;
    let rec = read(cat.as_ref(), &own, "invoice", "i1").await?;
    let got = rec.fields.get("gross");
    ensure!(
        decimal(got) == Some(dec("11.9")),
        "`i1` was written before `gross` existed; once its amount changes to 10.00, \
         the fold must compute gross 11.9, but it reads {got:?}"
    );
    Ok(())
}
