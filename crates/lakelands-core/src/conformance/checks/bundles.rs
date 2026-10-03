//! A bundle reaches a lake whole
//! (`docs/decisions/028-authoring-is-strict-and-whole.md`).
//!
//! `apply` validates a set of declarations as one: a declaration may name
//! a type the set declares after it, and the set is appended in one
//! append. A set with a problem appends nothing, and its refusal names
//! every problem.

use std::sync::Arc;

use lakelands_api::ontology::{FieldType, StorageKind};
use lakelands_api::write::{Declaration, Outcome};
use lakelands_api::{Error, LakeStore};

use crate::conformance::Failure;
use crate::conformance::fixture::{Lake, Must, field, type_decl};

check!(
    /// A bundle with forward references is applied whole or not at all.
    BUNDLE_WHOLE = a_bundle_with_forward_references_is_applied_whole_or_not_at_all,
    "028: apply validates a set of declarations together, so a reference may point forward within it, and appends all of it in one append or none of it"
);

/// A type whose one field refers to `target`.
fn referring(name: &str, target: &str) -> Declaration {
    let mut to = field("to", StorageKind::Reference);
    to.ty = FieldType {
        target: Some(target.into()),
        ..FieldType::kind(StorageKind::Reference)
    };
    Declaration::Type(type_decl(name, vec![to]))
}

async fn a_bundle_with_forward_references_is_applied_whole_or_not_at_all(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let before = store.head().await.must("reading the head")?;
    let applied = cat
        .apply(
            own,
            vec![
                referring("invoice", "partner"),
                Declaration::Type(type_decl("partner", vec![field("name", StorageKind::Text)])),
            ],
        )
        .await
        .must("applying `invoice`, which refers forward to `partner`, with `partner`")?;
    let after = store.head().await.must("reading the head")?;
    ensure!(
        matches!(applied, Outcome::Applied { position, .. } if position == after)
            && after.0 == before.0 + 2,
        "applying two declarations answered {applied:?} and moved the head from {before} to \
         {after}; it must append both in one append and answer the last position"
    );
    let described = l.describe().await?;
    let names: Vec<String> = described.types.iter().map(|t| t.name.to_string()).collect();
    ensure!(
        names.iter().any(|n| n == "invoice") && names.iter().any(|n| n == "partner"),
        "after the bundle the lake describes {names:?}; it must hold `invoice` and `partner`"
    );

    let refused = cat
        .apply(
            own,
            vec![
                Declaration::Type(type_decl("fine", vec![field("n", StorageKind::Number)])),
                referring("orphan", "ghost"),
                referring("stray", "nowhere"),
            ],
        )
        .await;
    let unmoved = store.head().await.must("reading the head")?;
    let named_all = match &refused {
        Err(Error::Refused { .. }) => {
            let text = format!("{refused:?}");
            text.contains("orphan") && text.contains("stray")
        }
        _ => false,
    };
    ensure!(
        named_all && unmoved == after,
        "a bundle with two broken references answered {refused:?} and moved the head from \
         {after} to {unmoved}; it must be refused whole, naming both problems, appending nothing"
    );
    let described = l.describe().await?;
    ensure!(
        described.types.iter().all(|t| t.name.as_str() != "fine"),
        "the refused bundle's valid declaration `fine` reached the lake"
    );
    Ok(())
}
