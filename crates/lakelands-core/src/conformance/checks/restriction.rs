//! A retention hold withholds a record without erasing it
//! (`docs/log-format.md` §2.3, the store facade's `restrict`).
//!
//! A restricted record is absent for a principal without the
//! `restricted` grant and present, unchanged, for one with it; its history
//! stays readable at earlier positions; lifting the hold brings it back.

use std::sync::Arc;

use lakelands_api::ontology::StorageKind;
use lakelands_api::principal::{Grants, PrincipalKind};
use lakelands_api::{Error, LakeStore, TypeName};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, declare, doors, field, get_at, owner, principal, type_decl, upsert,
};

check!(
    /// A restricted record is withheld from principals without the grant.
    HOLD_WITHHOLDS_AND_KEEPS = a_restricted_record_is_withheld_but_kept,
    "the facade: restrict hides a record from principals without the restricted grant, keeps it for those with it, keeps its history, and unrestrict lifts it"
);

async fn a_restricted_record_is_withheld_but_kept(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(
        cat.as_ref(),
        &own,
        type_decl("file", vec![field("body", StorageKind::Text)]),
    )
    .await?;
    let written = upsert(cat.as_ref(), &own, "file", "f1", json!({"body": "kept"})).await?;
    let reader = principal(
        store.as_ref(),
        PrincipalKind::Agent,
        "reader",
        Grants {
            default: lakelands_api::Access::Read,
            ..Grants::default()
        },
    );
    let ty = TypeName::from("file");

    let held = store
        .restrict(&ty, &"f1".into(), "statutory retention".into())
        .await
        .must("restricting `file` `f1`")?;
    let hidden = cat.get(&reader, &ty, get_at("f1", None)).await;
    ensure!(
        matches!(hidden, Err(Error::NotFound { .. })),
        "a principal without the restricted grant must not find `f1` after the hold; it answered {hidden:?}"
    );
    let seen = cat
        .get(&own, &ty, get_at("f1", None))
        .await
        .must("reading `f1` with the restricted grant")?;
    ensure!(
        seen.data.fields.get("body") == Some(&json!("kept")) && seen.position == held,
        "the owner must read `f1` unchanged at the hold's position {held}; it read {:?} at {}",
        seen.data.fields,
        seen.position
    );
    let then = cat
        .get(&reader, &ty, get_at("f1", Some(written.position())))
        .await
        .must("reading `f1` before the hold")?;
    ensure!(
        then.data.fields.get("body") == Some(&json!("kept")),
        "history before the hold stays readable: `f1` at {} must hold its body",
        written.position()
    );
    store
        .unrestrict(&ty, &"f1".into())
        .await
        .must("lifting the hold on `f1`")?;
    cat.get(&reader, &ty, get_at("f1", None))
        .await
        .must("reading `f1` after the hold is lifted")?;
    Ok(())
}
