//! A write touches only the fields it carries
//! (`docs/decisions/012-writes-touch-only-their-fields.md`,
//! `docs/decisions/006-grants-not-roles.md`).
//!
//! Two writers share one record: `amount` is the importer's, `verdict`
//! the reviewer's. Each may write only its own field, a patch is merged
//! per field, and a field that depends on another is cleared or kept by
//! the fold, never by the caller.

use std::sync::Arc;

use lakelands_api::ontology::{OnInputChange, StorageKind};
use lakelands_api::{Error, FieldName, LakeStore, Refusal, TypeName};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, declare, doors, field, field_writer, get_at, head_unmoved, owner, read, type_decl,
    upsert, value,
};

check!(
    /// Writer A's rewrite leaves writer B's field standing.
    REWRITE_LEAVES_OTHER_WRITERS_FIELDS = a_rewrite_by_one_writer_leaves_another_writers_fields_alone,
    "012: a patch is merged per field, so a writer that sends only its own fields cannot erase anyone else's"
);

check!(
    /// A dependent field declared `clear` is cleared by the fold.
    CLEAR_ON_INPUT_CHANGE = changing_an_input_clears_a_dependent_field_declared_clear,
    "012: with on_input_change clear, the fold clears the dependent field in the same event that changes its input"
);

check!(
    /// A dependent field declared `keep` stays, visibly older.
    KEEP_ON_INPUT_CHANGE = changing_an_input_keeps_a_dependent_field_declared_keep_and_shows_it_older,
    "012 and 010: with on_input_change keep, the value stays and its provenance shows it is older than its input"
);

check!(
    /// One forbidden field refuses the whole patch.
    UNGRANTED_FIELD_REFUSES_WHOLE_PATCH = a_patch_with_a_field_the_principal_may_not_write_is_refused_whole,
    "006 and 012: a write carrying a field its principal may not write is refused whole, never applied in part"
);

check!(
    /// Absent is unchanged, `null` clears.
    SPARSE_PATCH = a_field_absent_from_a_patch_is_unchanged_and_null_clears_it,
    "012 and the log format: an upsert is a sparse patch; an absent field is unchanged and null clears the field"
);

check!(
    /// A field the type does not declare refuses the whole patch.
    UNDECLARED_FIELD_REFUSES_WHOLE_PATCH = a_patch_with_an_undeclared_field_is_refused_whole,
    "015: a field the lake's map does not declare is refused as invalid, and nothing of the patch is written"
);

/// Declares `invoice` with the importer's `amount` and the reviewer's
/// `verdict`, the latter depending on `amount` as `on_change` says.
async fn two_writer_invoice(
    store: &dyn LakeStore,
    on_change: OnInputChange,
) -> Result<(), Failure> {
    let cat = doors(store);
    let mut verdict = field("verdict", StorageKind::Text);
    verdict.depends_on = vec![FieldName::from("amount")];
    verdict.on_input_change = on_change;
    let ty = type_decl(
        "invoice",
        vec![field("amount", StorageKind::Decimal), verdict],
    );
    declare(cat.as_ref(), &owner(store), ty).await?;
    Ok(())
}

async fn a_rewrite_by_one_writer_leaves_another_writers_fields_alone(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let mut ty = type_decl(
        "invoice",
        vec![
            field("amount", StorageKind::Decimal),
            field("verdict", StorageKind::Text),
        ],
    );
    ty.fields.push(field("note", StorageKind::Text));
    declare(cat.as_ref(), &owner(store.as_ref()), ty).await?;
    let a = field_writer(store.as_ref(), "importer", "invoice", &["amount", "note"]);
    let b = field_writer(store.as_ref(), "reviewer", "invoice", &["verdict"]);

    upsert(
        cat.as_ref(),
        &a,
        "invoice",
        "i1",
        json!({"amount": "100.00", "note": "first"}),
    )
    .await?;
    upsert(
        cat.as_ref(),
        &b,
        "invoice",
        "i1",
        json!({"verdict": "approved"}),
    )
    .await?;
    upsert(
        cat.as_ref(),
        &a,
        "invoice",
        "i1",
        json!({"amount": "120.00"}),
    )
    .await?;

    let rec = read(cat.as_ref(), &owner(store.as_ref()), "invoice", "i1").await?;
    ensure!(
        value(&rec, "verdict") == Some(&json!("approved")),
        "the importer rewrote `amount` and the reviewer's `verdict` is now {:?}; \
         a write touches only the fields it carries, so it must still be \"approved\"",
        value(&rec, "verdict")
    );
    ensure!(
        value(&rec, "amount") == Some(&json!("120.00")),
        "the importer's rewrite of `amount` to \"120.00\" reads back as {:?}",
        value(&rec, "amount")
    );
    ensure!(
        value(&rec, "note") == Some(&json!("first")),
        "the importer's second patch did not carry `note`, which is now {:?}; \
         an absent field is unchanged, even for the writer who set it",
        value(&rec, "note")
    );
    Ok(())
}

async fn changing_an_input_clears_a_dependent_field_declared_clear(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    two_writer_invoice(store.as_ref(), OnInputChange::Clear).await?;
    let cat = doors(store.as_ref());
    let a = field_writer(store.as_ref(), "importer", "invoice", &["amount"]);
    let b = field_writer(store.as_ref(), "reviewer", "invoice", &["verdict"]);
    let own = owner(store.as_ref());

    upsert(
        cat.as_ref(),
        &a,
        "invoice",
        "i1",
        json!({"amount": "100.00"}),
    )
    .await?;
    upsert(
        cat.as_ref(),
        &b,
        "invoice",
        "i1",
        json!({"verdict": "approved"}),
    )
    .await?;
    let rec = read(cat.as_ref(), &own, "invoice", "i1").await?;
    ensure!(
        value(&rec, "verdict") == Some(&json!("approved")),
        "writing `verdict` itself must not clear it, yet it reads {:?}",
        value(&rec, "verdict")
    );

    let changed = upsert(
        cat.as_ref(),
        &a,
        "invoice",
        "i1",
        json!({"amount": "120.00"}),
    )
    .await?;
    let rec = read(cat.as_ref(), &own, "invoice", "i1").await?;
    ensure!(
        value(&rec, "verdict").is_none(),
        "`verdict` depends on `amount` with on_input_change clear; `amount` changed \
         and `verdict` still reads {:?}. The fold must clear it; no caller does",
        value(&rec, "verdict")
    );

    let ty = TypeName::from("invoice");
    let at_change = cat
        .get(&own, &ty, get_at("i1", Some(changed.position())))
        .await
        .must("reading `i1` as of the write that changed `amount`")?;
    ensure!(
        value(&at_change.data, "verdict").is_none(),
        "as of position {} (the write that changed `amount`), `verdict` still reads {:?}; \
         the fold clears it in the same event that changes the input",
        changed.position(),
        value(&at_change.data, "verdict")
    );
    Ok(())
}

async fn changing_an_input_keeps_a_dependent_field_declared_keep_and_shows_it_older(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    two_writer_invoice(store.as_ref(), OnInputChange::Keep).await?;
    let cat = doors(store.as_ref());
    let a = field_writer(store.as_ref(), "importer", "invoice", &["amount"]);
    let b = field_writer(store.as_ref(), "reviewer", "invoice", &["verdict"]);

    upsert(
        cat.as_ref(),
        &a,
        "invoice",
        "i1",
        json!({"amount": "100.00"}),
    )
    .await?;
    upsert(
        cat.as_ref(),
        &b,
        "invoice",
        "i1",
        json!({"verdict": "approved"}),
    )
    .await?;
    upsert(
        cat.as_ref(),
        &a,
        "invoice",
        "i1",
        json!({"amount": "120.00"}),
    )
    .await?;

    let rec = read(cat.as_ref(), &owner(store.as_ref()), "invoice", "i1").await?;
    ensure!(
        value(&rec, "verdict") == Some(&json!("approved")),
        "`verdict` is declared on_input_change keep; after `amount` changed it reads {:?} \
         instead of staying \"approved\"",
        value(&rec, "verdict")
    );
    let (Some(v), Some(amt)) = (rec.meta.get("verdict"), rec.meta.get("amount")) else {
        return Err(Failure::new(format!(
            "a read with with_meta must carry provenance for every field with a value; \
             it carried it for {:?}",
            rec.meta.keys().collect::<Vec<_>>()
        )));
    };
    ensure!(
        v.position < amt.position && v.known_at <= amt.known_at,
        "the kept `verdict` must show it is older than its input: its provenance says \
         position {} / known_at {}, `amount`'s says position {} / known_at {}",
        v.position,
        v.known_at,
        amt.position,
        amt.known_at
    );
    ensure!(
        v.writer == b.writer(),
        "the kept `verdict` was written by `{}`, but its provenance names `{}`",
        b.writer(),
        v.writer
    );
    Ok(())
}

async fn a_patch_with_a_field_the_principal_may_not_write_is_refused_whole(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    two_writer_invoice(store.as_ref(), OnInputChange::Keep).await?;
    let cat = doors(store.as_ref());
    let a = field_writer(store.as_ref(), "importer", "invoice", &["amount"]);
    let own = owner(store.as_ref());

    upsert(
        cat.as_ref(),
        &a,
        "invoice",
        "i1",
        json!({"amount": "100.00"}),
    )
    .await?;
    let head = store.head().await.must("reading the head")?;

    let err = cat
        .upsert(
            &a,
            &TypeName::from("invoice"),
            &"i1".into(),
            crate::conformance::fixture::patch(json!({"amount": "999.00", "verdict": "approved"})),
            Default::default(),
        )
        .await
        .must_fail("the importer writing `amount` and the reviewer's `verdict` together")?;
    ensure!(
        matches!(
            &err,
            Error::Refused { field: Some(f), reason: Refusal::NotGranted } if f.as_str() == "verdict"
        ),
        "a patch carrying `verdict`, which the importer holds no write grant for, must be \
         refused as not granted on `verdict`; it failed with {err:?}"
    );

    let rec = read(cat.as_ref(), &own, "invoice", "i1").await?;
    ensure!(
        value(&rec, "amount") == Some(&json!("100.00")) && value(&rec, "verdict").is_none(),
        "the refused patch was applied in part: `amount` reads {:?} and `verdict` {:?}; \
         a refused write changes nothing",
        value(&rec, "amount"),
        value(&rec, "verdict")
    );
    let after = store.head().await.must("reading the head")?;
    ensure!(
        after == head,
        "the refused patch moved the head from {head} to {after}; a refused write appends nothing"
    );
    Ok(())
}

async fn a_field_absent_from_a_patch_is_unchanged_and_null_clears_it(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let ty = type_decl(
        "note",
        vec![
            field("title", StorageKind::Text),
            field("body", StorageKind::Text),
            field("pages", StorageKind::Number),
        ],
    );
    declare(cat.as_ref(), &own, ty).await?;

    upsert(
        cat.as_ref(),
        &own,
        "note",
        "n1",
        json!({"title": "draft", "body": "text", "pages": 3}),
    )
    .await?;
    upsert(cat.as_ref(), &own, "note", "n1", json!({"body": null})).await?;
    let rec = read(cat.as_ref(), &own, "note", "n1").await?;
    ensure!(
        value(&rec, "body").is_none(),
        "a patch setting `body` to null must clear it; it reads {:?}",
        value(&rec, "body")
    );
    ensure!(
        value(&rec, "title") == Some(&json!("draft")) && value(&rec, "pages") == Some(&json!(3)),
        "a patch carrying only `body` changed the fields it did not carry: \
         `title` reads {:?}, `pages` {:?}",
        value(&rec, "title"),
        value(&rec, "pages")
    );

    upsert(cat.as_ref(), &own, "note", "n1", json!({"title": "final"})).await?;
    let rec = read(cat.as_ref(), &own, "note", "n1").await?;
    ensure!(
        value(&rec, "body").is_none() && value(&rec, "title") == Some(&json!("final")),
        "after clearing `body` and then writing only `title`, the record reads {:?}; \
         `body` must stay cleared and `title` must be \"final\"",
        rec.fields
    );
    Ok(())
}

async fn a_patch_with_an_undeclared_field_is_refused_whole(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let ty = type_decl("memo", vec![field("text", StorageKind::Text)]);
    declare(cat.as_ref(), &own, ty).await?;
    upsert(cat.as_ref(), &own, "memo", "m1", json!({"text": "hello"})).await?;

    for key in ["m1", "m2"] {
        let before = store.head().await.must("reading the head")?;
        let err = cat
            .upsert(
                &own,
                &TypeName::from("memo"),
                &key.into(),
                crate::conformance::fixture::patch(
                    json!({"text": "changed", "remark": "call before dunning"}),
                ),
                Default::default(),
            )
            .await
            .must_fail(&format!(
                "writing the undeclared field `remark` to `memo` `{key}`"
            ))?;
        head_unmoved(
            store.as_ref(),
            before,
            &format!("writing the undeclared field `remark` to `memo` `{key}`"),
        )
        .await?;
        ensure!(
            matches!(
                &err,
                Error::Refused { reason: Refusal::Invalid { .. }, field }
                    if field.as_ref().is_none_or(|f| f.as_str() == "remark")
            ),
            "a patch carrying `remark`, which `memo` does not declare, must be refused as \
             invalid (naming `remark` if it names a field); it failed with {err:?}"
        );
    }

    let rec = read(cat.as_ref(), &own, "memo", "m1").await?;
    ensure!(
        value(&rec, "text") == Some(&json!("hello")) && value(&rec, "remark").is_none(),
        "the refused patch was applied in part to `m1`: it reads {:?}",
        rec.fields
    );
    let missing = cat
        .get(&own, &TypeName::from("memo"), get_at("m2", None))
        .await;
    ensure!(
        matches!(missing, Err(Error::NotFound { .. })),
        "the refused patch to the new key `m2` created the record anyway: {missing:?}"
    );
    Ok(())
}
