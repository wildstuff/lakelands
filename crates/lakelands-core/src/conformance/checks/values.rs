//! Values against their storage kinds and value types
//! (`docs/decisions/011-storage-kind-and-value-type.md`, `docs/log-format.md`
//! §5).

use std::sync::Arc;

use lakelands_api::ontology::{FieldDecl, FieldType, StorageKind, ValueTypeDecl};
use lakelands_api::write::Declaration;
use lakelands_api::{Error, LakeStore, Refusal, TypeName, WriteOpts};
use serde_json::{Value, json};

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, declare, doors, field, head_unmoved, owner, patch, read, type_decl, upsert, value,
};

check!(
    /// An instant reads back in its one written form.
    DATE_TIME_CANONICAL = date_time_values_read_back_in_their_one_canonical_form,
    "the log format §5: a date-time is kept in UTC as YYYY-MM-DDTHH:MM:SS.ffffffZ, whatever offset it was written with"
);

check!(
    /// Bytes have one spelling.
    BYTES_ONE_SPELLING = bytes_are_padded_standard_base64_and_nothing_else,
    "the log format §5: bytes are canonical standard base64, padded and with zero pad bits; unpadded, url-safe or non-canonical spellings are refused"
);

check!(
    /// A vector has its declared length.
    VECTOR_LENGTH = a_vector_of_the_wrong_length_is_refused,
    "011: a vector field declares its dimensions, and a value of another length does not fit it"
);

check!(
    /// A value type's rules hold on every write.
    VALUE_TYPE_RULES = a_value_types_one_of_format_and_scale_hold_on_every_write,
    "011: a value's `one_of`, named `format` and decimal `scale` are the value type's, checked on every write; scale rounds half away from zero"
);

check!(
    /// Null clears a field whatever its value type says.
    NULL_CLEARS_ANY_TYPE = null_clears_a_field_whatever_its_value_type_allows,
    "012 and 011: `null` in a patch clears the field; it is not a value the value type must allow"
);

async fn one_field(store: &dyn LakeStore, name: &str, ty: FieldType) -> Result<(), Failure> {
    let cat = doors(store);
    let decl = type_decl(
        name,
        vec![FieldDecl {
            ty,
            ..field("v", StorageKind::Text)
        }],
    );
    declare(cat.as_ref(), &owner(store), decl).await?;
    Ok(())
}

/// Write `v` to `thing` and expect a refusal on field `v`, with nothing
/// appended.
async fn refused(store: &dyn LakeStore, v: Value, why: &str) -> Result<(), Failure> {
    refused_in(store, "thing", v, why).await
}

async fn refused_in(store: &dyn LakeStore, ty: &str, v: Value, why: &str) -> Result<(), Failure> {
    let cat = doors(store);
    let before = store.head().await.must("reading the head")?;
    let what = format!("writing {v} ({why})");
    let err = cat
        .upsert(
            &owner(store),
            &TypeName::from(ty),
            &"k".into(),
            patch(json!({ "v": v })),
            WriteOpts::default(),
        )
        .await
        .must_fail(&what)?;
    head_unmoved(store, before, &what).await?;
    ensure!(
        matches!(
            &err,
            Error::Refused { field: Some(f), reason: Refusal::Constraint { .. } | Refusal::Invalid { .. } }
                if f.as_str() == "v"
        ),
        "{what} must be refused on field `v`; it failed with {err:?}"
    );
    Ok(())
}

async fn date_time_values_read_back_in_their_one_canonical_form(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    one_field(
        store.as_ref(),
        "thing",
        FieldType::kind(StorageKind::DateTime),
    )
    .await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    for (written, canonical) in [
        ("2026-09-24T07:57:11+02:00", "2026-09-24T05:57:11.000000Z"),
        ("2026-09-24", "2026-09-24T00:00:00.000000Z"),
        ("2026-09-24T05:57:11.5Z", "2026-09-24T05:57:11.500000Z"),
    ] {
        upsert(cat.as_ref(), &own, "thing", "k", json!({ "v": written })).await?;
        let r = read(cat.as_ref(), &own, "thing", "k").await?;
        ensure!(
            value(&r, "v") == Some(&json!(canonical)),
            "`{written}` must read back as `{canonical}`; it reads {:?}",
            value(&r, "v")
        );
    }
    refused(store.as_ref(), json!("2026-09-24T05:57:11"), "no offset").await?;
    refused(store.as_ref(), json!("2026-02-30"), "no such day").await
}

async fn bytes_are_padded_standard_base64_and_nothing_else(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    one_field(store.as_ref(), "thing", FieldType::kind(StorageKind::Bytes)).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    for ok in ["aGVsbG8=", "aGk=", "+/+/", ""] {
        upsert(cat.as_ref(), &own, "thing", "k", json!({ "v": ok })).await?;
        let r = read(cat.as_ref(), &own, "thing", "k").await?;
        ensure!(
            value(&r, "v") == Some(&json!(ok)),
            "the bytes `{ok}` must read back as written; they read {:?}",
            value(&r, "v")
        );
    }
    for bad in ["aGVsbG8", "-_-_", "aGl=", "a=bc"] {
        refused(store.as_ref(), json!(bad), "not padded standard base64").await?;
    }
    Ok(())
}

async fn a_vector_of_the_wrong_length_is_refused(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
    let ty = FieldType {
        dimensions: Some(3),
        ..FieldType::kind(StorageKind::Vector)
    };
    one_field(store.as_ref(), "thing", ty).await?;
    let cat = doors(store.as_ref());
    upsert(
        cat.as_ref(),
        &owner(store.as_ref()),
        "thing",
        "k",
        json!({"v": [0.1, 0.2, 0.3]}),
    )
    .await?;
    refused(
        store.as_ref(),
        json!([0.1, 0.2]),
        "two numbers for three dimensions",
    )
    .await?;
    refused(
        store.as_ref(),
        json!([0.1, 0.2, 0.3, 0.4]),
        "four numbers for three",
    )
    .await?;
    refused(store.as_ref(), json!([0.1, "x", 0.3]), "not all numbers").await
}

fn value_type(name: &str, kind: StorageKind) -> ValueTypeDecl {
    ValueTypeDecl {
        name: name.into(),
        kind,
        constraint: None,
        one_of: Vec::new(),
        format: None,
        scale: None,
        description: None,
    }
}

async fn typed(store: &dyn LakeStore, ty: &str, vt: ValueTypeDecl) -> Result<(), Failure> {
    let cat = doors(store);
    let own = owner(store);
    let kind = vt.kind;
    let name = vt.name.clone();
    cat.declare(&own, Declaration::ValueType(vt))
        .await
        .must(&format!("declaring value type `{name}`"))?;
    one_field(
        store,
        ty,
        FieldType {
            value_type: Some(name),
            ..FieldType::kind(kind)
        },
    )
    .await
}

async fn a_value_types_one_of_format_and_scale_hold_on_every_write(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());

    let status = ValueTypeDecl {
        one_of: vec!["open".into(), "paid".into()],
        ..value_type("status", StorageKind::Text)
    };
    typed(store.as_ref(), "thing", status).await?;
    upsert(cat.as_ref(), &own, "thing", "k", json!({"v": "paid"})).await?;
    refused(store.as_ref(), json!("lost"), "not one of open, paid").await?;

    let email = ValueTypeDecl {
        format: Some("email".into()),
        ..value_type("email", StorageKind::Text)
    };
    typed(store.as_ref(), "contact", email).await?;
    upsert(
        cat.as_ref(),
        &own,
        "contact",
        "k",
        json!({"v": "ada@example.org"}),
    )
    .await?;
    refused_in(
        store.as_ref(),
        "contact",
        json!("ada at example"),
        "not an email",
    )
    .await?;

    let money = ValueTypeDecl {
        scale: Some(2),
        ..value_type("money", StorageKind::Decimal)
    };
    typed(store.as_ref(), "payment", money).await?;
    for (written, kept) in [
        ("1.005", "1.01"),
        ("-1.005", "-1.01"),
        ("2.004", "2.00"),
        ("7", "7.00"),
    ] {
        upsert(cat.as_ref(), &own, "payment", "k", json!({ "v": written })).await?;
        let r = read(cat.as_ref(), &own, "payment", "k").await?;
        ensure!(
            value(&r, "v") == Some(&json!(kept)),
            "a `money` value (scale 2) written as {written} must be kept as {kept}, \
             rounded half away from zero; it reads {:?}",
            value(&r, "v")
        );
    }

    let unknown = ValueTypeDecl {
        format: Some("shoe_size".into()),
        ..value_type("odd", StorageKind::Text)
    };
    let before = store.head().await.must("reading the head")?;
    cat.declare(&own, Declaration::ValueType(unknown))
        .await
        .must_fail("declaring a value type with the unknown format `shoe_size`")?;
    head_unmoved(store.as_ref(), before, "declaring an unknown format").await?;
    Ok(())
}

async fn null_clears_a_field_whatever_its_value_type_allows(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let status = ValueTypeDecl {
        one_of: vec!["open".into(), "paid".into()],
        ..value_type("status", StorageKind::Text)
    };
    typed(store.as_ref(), "thing", status).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    upsert(cat.as_ref(), &own, "thing", "k", json!({"v": "open"})).await?;
    upsert(cat.as_ref(), &own, "thing", "k", json!({"v": null})).await?;
    let r = read(cat.as_ref(), &own, "thing", "k").await?;
    ensure!(
        value(&r, "v").is_none(),
        "`null` must clear `v` although null is not one of its allowed values; `v` \
         reads {:?}",
        value(&r, "v")
    );
    Ok(())
}

check!(
    /// A decimal is written as a string of digits, never a number.
    DECIMAL_NOT_A_NUMBER = a_decimal_written_as_a_json_number_is_refused,
    "the log format §5: a decimal's one JSON form is a string of digits, so a JSON number, which may already have lost digits, is refused"
);

async fn a_decimal_written_as_a_json_number_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    one_field(
        store.as_ref(),
        "thing",
        FieldType::kind(StorageKind::Decimal),
    )
    .await?;
    let cat = doors(store.as_ref());
    upsert(
        cat.as_ref(),
        &owner(store.as_ref()),
        "thing",
        "k",
        json!({"v": "12.50"}),
    )
    .await?;
    refused(store.as_ref(), json!(12.5), "a decimal as a JSON number").await?;
    refused(store.as_ref(), json!(12), "a decimal as a JSON integer").await?;
    refused(store.as_ref(), json!("12,50"), "a decimal with a comma").await
}
