//! A host's own `sys.` types reach a lake through its system vocabulary
//! (`docs/decisions/003-reservation-by-namespace.md`, `docs/seams.md`,
//! `SystemVocabulary`). The store consults the vocabulary each time a
//! writer opens the lake and declares the types it allows and the lake
//! does not hold yet, so a type the host adds later reaches a lake that
//! already exists, and a lake the host withholds a type from never holds
//! it.

use std::sync::Arc;

use lakelands_api::ontology::StorageKind;
use lakelands_api::seam::Seams;
use lakelands_api::{LakeStore, ReadAt, TypeName};
use serde_json::json;

use crate::conformance::fixture::{doors, field, owner, read, type_decl, upsert};
use crate::conformance::{Failure, Reopen, backing};

check!(
    /// A host type is declared when a writer opens the lake.
    HOST_TYPES_AT_OPEN = a_host_type_is_declared_when_a_writer_opens_the_lake,
    "003 and seams, SystemVocabulary: each writer open declares the host's sys. types the lake may hold and does not hold yet; a type added later reaches the existing lake, and a withheld one never does",
    reopens
);

fn reopened(vocabulary: &str, e: lakelands_api::Error) -> Failure {
    Failure::new(format!(
        "opening the lake again with a vocabulary {vocabulary} failed: {e} ({e:?})"
    ))
}

async fn holds(store: &dyn LakeStore, ty: &str) -> bool {
    doors(store)
        .describe_type(&owner(store), &TypeName::from(ty), ReadAt::default())
        .await
        .is_ok()
}

async fn a_host_type_is_declared_when_a_writer_opens_the_lake(
    store: Arc<dyn LakeStore>,
    reopen: Reopen,
) -> Result<(), Failure> {
    let note = || type_decl("sys.host_note", vec![field("text", StorageKind::Text)]);
    let tag = || type_decl("sys.host_tag", vec![field("label", StorageKind::Text)]);
    let hidden = || type_decl("sys.host_hidden", vec![field("x", StorageKind::Text)]);
    let with = |vocabulary| Seams {
        backings: backing::backings(),
        vocabulary: Some(vocabulary),
        ..Seams::default()
    };
    ensure!(
        !holds(store.as_ref(), "sys.host_note").await,
        "a lake opened with no host vocabulary already holds `sys.host_note`"
    );

    let store = reopen(
        store,
        with(backing::vocabulary(
            vec![note(), hidden()],
            &["sys.host_hidden"],
        )),
    )
    .await
    .map_err(|e| reopened("adding `sys.host_note`", e))?;
    ensure!(
        holds(store.as_ref(), "sys.host_note").await,
        "a writer opened the lake with a vocabulary adding `sys.host_note`; the lake does \
         not hold it"
    );
    ensure!(
        !holds(store.as_ref(), "sys.host_hidden").await,
        "the vocabulary withholds `sys.host_hidden` from this lake; the lake holds it all \
         the same"
    );
    upsert(
        doors(store.as_ref()).as_ref(),
        &owner(store.as_ref()),
        "sys.host_note",
        "n",
        json!({"text": "kept"}),
    )
    .await?;

    let store = reopen(
        store,
        with(backing::vocabulary(
            vec![note(), tag(), hidden()],
            &["sys.host_hidden"],
        )),
    )
    .await
    .map_err(|e| reopened("that grew by `sys.host_tag`", e))?;
    ensure!(
        holds(store.as_ref(), "sys.host_tag").await,
        "the host added `sys.host_tag` after the lake existed; the next writer's open did \
         not declare it"
    );
    let kept = read(
        doors(store.as_ref()).as_ref(),
        &owner(store.as_ref()),
        "sys.host_note",
        "n",
    )
    .await?;
    ensure!(
        kept.fields.get("text") == Some(&json!("kept")),
        "`sys.host_note` `n` was written before the vocabulary grew; after the next open \
         it reads {kept:?}"
    );
    ensure!(
        !holds(store.as_ref(), "sys.host_hidden").await,
        "the vocabulary still withholds `sys.host_hidden`; the lake holds it after the \
         second open"
    );
    Ok(())
}
