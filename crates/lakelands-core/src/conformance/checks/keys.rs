//! A record's key and its key fields say the same thing
//! (`docs/log-format.md` §2.3).
//!
//! A type that declares key fields keys its records by their values:
//! each value's JSON string form, `%` and `:` percent-encoded, joined by
//! `:`. A writer may give the key, the key fields, or both; a key that
//! disagrees with its key fields is refused, and a record created by its
//! key alone gets its key fields from it.

use std::sync::Arc;

use lakelands_api::ontology::StorageKind;
use lakelands_api::{FieldName, LakeStore, Refusal, TypeName};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, field, head_unmoved, patch, reason, type_decl, value,
};

check!(
    /// A key that disagrees with the key fields is refused.
    KEY_AND_KEY_FIELDS_AGREE = a_key_that_disagrees_with_its_key_fields_is_refused,
    "the log format: a record's key is its key fields' values joined by `:`; a write that says otherwise is refused and appends nothing"
);

check!(
    /// The key alone creates a record with its key fields set.
    KEY_FIELDS_FROM_THE_KEY = a_record_created_by_its_key_alone_gets_its_key_fields,
    "the log format: a writer may give only the key; the key fields are read from its components"
);

fn keyed() -> lakelands_api::ontology::TypeDecl {
    let mut t = type_decl(
        "line",
        vec![
            field("invoice", StorageKind::Text),
            field("n", StorageKind::Number),
            field("note", StorageKind::Text),
        ],
    );
    t.key = vec![FieldName::from("invoice"), FieldName::from("n")];
    t
}

async fn a_key_that_disagrees_with_its_key_fields_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(keyed()).await?;
    l.upsert(
        "line",
        "INV%3A7%251:2",
        json!({"invoice": "INV:7%1", "n": 2, "note": "a"}),
    )
    .await?;

    let before = store.head().await.must("reading the head")?;
    let err = cat
        .upsert(
            own,
            &TypeName::from("line"),
            &"INV%3A7%251:3".into(),
            patch(json!({"invoice": "INV:7%1", "n": 2})),
            Default::default(),
        )
        .await
        .must_fail("writing `line` `INV%3A7%251:3` with key fields that make `…:2`")?;
    ensure!(
        matches!(reason(&err), Some(Refusal::Invalid { .. })),
        "a key that disagrees with its key fields must be refused as invalid; it failed with {err:?}"
    );
    head_unmoved(
        store.as_ref(),
        before,
        "a write whose key disagrees with its key fields",
    )
    .await
}

async fn a_record_created_by_its_key_alone_gets_its_key_fields(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    l.declare(keyed()).await?;
    l.upsert("line", "INV%3A9:4", json!({"note": "from the key"}))
        .await?;
    let r = l.read("line", "INV%3A9:4").await?;
    ensure!(
        value(&r, "invoice") == Some(&json!("INV:9")) && value(&r, "n") == Some(&json!(4)),
        "a record created by the key `INV%3A9:4` must hold invoice = \"INV:9\" and n = 4; it holds {:?}",
        r.fields
    );
    Ok(())
}
