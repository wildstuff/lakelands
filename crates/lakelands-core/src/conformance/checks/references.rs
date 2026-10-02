//! A `reference` names a record that exists
//! (`docs/decisions/011-storage-kind-and-value-type.md`, the log format
//! §5: a reference is the target record's key).

use std::sync::Arc;

use lakelands_api::ontology::{Authority, StorageKind};
use lakelands_api::{Error, LakeStore, TypeName, WriteOpts};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, committed, field, head_unmoved, patch, reference, refused_on, source_record,
    type_decl, value, windows_reach_head,
};

check!(
    /// A reference must name a record the lake holds.
    REFERENCE_MUST_RESOLVE = a_write_referencing_a_record_that_does_not_exist_is_refused,
    "011 and the log format §5: a reference is the key of a record of its target type; a write naming no such record is refused whole, one naming an existing record is accepted"
);

check!(
    /// Within one ingest, an earlier record is already there.
    REFERENCE_WITHIN_A_WINDOW = a_reference_to_a_record_earlier_in_the_same_ingest_resolves,
    "016 and the log format §5: an ingest's records are written in order, so a record may refer to one delivered before it in the same window"
);

async fn partners_and_invoices(
    store: &dyn LakeStore,
    authority: Authority,
) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    let mut name = field("name", StorageKind::Text);
    name.authority = authority;
    l.declare(type_decl("partner", vec![name])).await?;
    let mut partner = reference("partner", "partner");
    partner.authority = authority;
    l.declare(type_decl("invoice", vec![partner])).await?;
    Ok(l)
}

async fn a_write_referencing_a_record_that_does_not_exist_is_refused(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = partners_and_invoices(store.as_ref(), Authority::Authored).await?;
    let Lake { cat, own } = &l;
    let before = store.head().await.must("reading the head")?;
    let err = cat
        .upsert(
            own,
            &TypeName::from("invoice"),
            &"i".into(),
            patch(json!({"partner": "nobody"})),
            WriteOpts::default(),
        )
        .await
        .must_fail("writing an invoice whose `partner` names no partner")?;
    head_unmoved(store.as_ref(), before, "writing a dangling reference").await?;
    ensure!(
        matches!(refused_on(&err), Some(("partner", _))) || matches!(err, Error::NotFound { .. }),
        "a reference to a partner that does not exist must be refused on `partner`; \
         it failed with {err:?}"
    );
    l.upsert("partner", "acme", json!({"name": "Acme"})).await?;
    l.upsert("invoice", "i", json!({"partner": "acme"})).await?;
    let r = l.read("invoice", "i").await?;
    ensure!(
        value(&r, "partner") == Some(&json!("acme")),
        "a reference to the existing `acme` must be written; `partner` reads {:?}",
        value(&r, "partner")
    );
    Ok(())
}

async fn a_reference_to_a_record_earlier_in_the_same_ingest_resolves(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = partners_and_invoices(store.as_ref(), Authority::Source).await?;
    let records = vec![
        source_record("partner", "acme", json!({"name": "Acme"})),
        source_record("invoice", "i", json!({"partner": "acme"})),
    ];
    let windows = l
        .ingest("erp", records)
        .await
        .must("ingesting a partner and then an invoice referring to it")?;
    windows_reach_head(store.as_ref(), &windows, "ingesting partner then invoice").await?;
    ensure!(
        committed(&windows) == (2, 0),
        "both records must be committed; the windows committed {:?}",
        committed(&windows)
    );
    let r = l.read("invoice", "i").await?;
    ensure!(
        value(&r, "partner") == Some(&json!("acme")),
        "the invoice must refer to `acme`; `partner` reads {:?}",
        value(&r, "partner")
    );
    Ok(())
}
