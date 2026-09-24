//! The source door (`docs/decisions/016-two-write-doors-by-origin.md`,
//! `docs/log-format.md` §3).
//!
//! `ingest` is content-addressed: a record delivered twice with the same
//! values appends nothing the second time, even within one delivery,
//! while the same key with new values is a genuine change and moves the
//! version. And the two doors keep the authority line: a source writes
//! only source fields, an author only authored ones, and a record or
//! patch that crosses the line is refused whole.

use std::sync::Arc;

use lakelands_api::ontology::{Authority, StorageKind, TypeDecl};
use lakelands_api::{Error, LakeStore, Refusal, TypeName, Version};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, committed, declare, doors, field, get_at, head_unmoved, ingest, owner, patch, read,
    source_record, type_decl, value, windows_reach_head,
};

check!(
    /// The same content ingested twice is written once.
    SAME_CONTENT_TWICE = the_same_content_ingested_twice_appends_nothing_the_second_time,
    "016 and the log format: ingest is content-addressed, so a redelivery of the same values appends nothing, within one delivery or across two"
);

check!(
    /// The same key twice with new values advances the version.
    SAME_KEY_TWICE = the_same_key_twice_in_one_ingest_with_new_values_advances_the_version,
    "the log format: every event that changes a record moves its version by one, including two in one delivery"
);

check!(
    /// A source record carrying an authored field is refused.
    FOREIGN_FIELD_REFUSED = an_ingested_record_carrying_a_field_the_source_does_not_write_is_refused_whole,
    "016 and the log format: ingest writes only source and shared fields; a record carrying any other is refused whole, never trimmed"
);

check!(
    /// An upsert carrying a source field is refused.
    UPSERT_REFUSES_SOURCE_FIELD = an_upsert_carrying_a_field_only_a_source_writes_is_refused_as_the_wrong_door,
    "016 and 015: an author never overwrites a figure the source is the authority on; the patch is refused as the wrong door"
);

/// `feed`: a source's `body` and an author's `verdict`.
fn feed() -> TypeDecl {
    let mut body = field("body", StorageKind::Text);
    body.authority = Authority::Source;
    type_decl("feed", vec![body, field("verdict", StorageKind::Text)])
}

async fn the_same_content_ingested_twice_appends_nothing_the_second_time(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(cat.as_ref(), &own, feed()).await?;

    let once = || vec![source_record("feed", "k1", json!({"body": "unchanged"}))];
    let first = ingest(cat.as_ref(), &own, "bank", once())
        .await
        .must("ingesting `k1`")?;
    windows_reach_head(store.as_ref(), &first, "ingesting `k1`").await?;
    ensure!(
        committed(&first) == (1, 0),
        "the first delivery of `k1` must commit 1 record and 0 duplicates; the windows \
         answered {first:?}"
    );
    let head = store.head().await.must("reading the head")?;
    let second = ingest(cat.as_ref(), &own, "bank", once())
        .await
        .must("ingesting `k1` again with the same values")?;
    windows_reach_head(
        store.as_ref(),
        &second,
        "ingesting `k1` again with the same values",
    )
    .await?;
    ensure!(
        committed(&second) == (0, 1),
        "delivering `k1` again with the same values must commit 0 records and 1 \
         duplicate; the windows answered {second:?}"
    );
    let after = store.head().await.must("reading the head")?;
    ensure!(
        after == head,
        "the redelivery moved the head from {head} to {after}; the same content appends \
         nothing"
    );

    let twice = vec![
        source_record("feed", "k2", json!({"body": "same"})),
        source_record("feed", "k2", json!({"body": "same"})),
    ];
    let windows = ingest(cat.as_ref(), &own, "bank", twice)
        .await
        .must("ingesting `k2` twice in one delivery")?;
    windows_reach_head(
        store.as_ref(),
        &windows,
        "ingesting `k2` twice in one delivery",
    )
    .await?;
    ensure!(
        committed(&windows) == (1, 1),
        "one delivery carrying `k2` twice with the same values must commit 1 record and \
         1 duplicate; the windows answered {windows:?}"
    );
    for key in ["k1", "k2"] {
        let rec = read(cat.as_ref(), &own, "feed", key).await?;
        ensure!(
            rec.version == Version(1),
            "`{key}` was delivered twice with the same values and must stay at version 1; \
             it is at version {}",
            rec.version
        );
    }
    Ok(())
}

async fn the_same_key_twice_in_one_ingest_with_new_values_advances_the_version(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(cat.as_ref(), &own, feed()).await?;

    let delivery = vec![
        source_record("feed", "k1", json!({"body": "first draft"})),
        source_record("feed", "k1", json!({"body": "second draft"})),
    ];
    let windows = ingest(cat.as_ref(), &own, "bank", delivery)
        .await
        .must("ingesting `k1` twice with different values")?;
    windows_reach_head(
        store.as_ref(),
        &windows,
        "ingesting `k1` twice with different values",
    )
    .await?;
    ensure!(
        committed(&windows) == (2, 0),
        "two different values for `k1` are two changes, not a duplicate: the windows must \
         commit 2 records and 0 duplicates; they answered {windows:?}"
    );
    let rec = read(cat.as_ref(), &own, "feed", "k1").await?;
    ensure!(
        value(&rec, "body") == Some(&json!("second draft")) && rec.version == Version(2),
        "the last record of the delivery stands, at the version after the first one's: \
         `k1` must read \"second draft\" at version 2, and reads {:?} at version {}",
        value(&rec, "body"),
        rec.version
    );
    Ok(())
}

async fn an_ingested_record_carrying_a_field_the_source_does_not_write_is_refused_whole(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(cat.as_ref(), &own, feed()).await?;

    let delivery = vec![source_record(
        "feed",
        "k1",
        json!({"body": "text", "verdict": "approved"}),
    )];
    let before = store.head().await.must("reading the head")?;
    let err = ingest(cat.as_ref(), &own, "bank", delivery)
        .await
        .must_fail("ingesting a record that carries the authored field `verdict`")?;
    head_unmoved(
        store.as_ref(),
        before,
        "ingesting a record that carries the authored field `verdict`",
    )
    .await?;
    ensure!(
        matches!(
            &err,
            Error::Refused { reason: Refusal::WrongDoor, field }
                if field.as_ref().is_none_or(|f| f.as_str() == "verdict")
        ),
        "a source record carrying the authored field `verdict` must be refused as the \
         wrong door (naming `verdict` if it names a field); it failed with {err:?}"
    );
    let missing = cat
        .get(&own, &TypeName::from("feed"), get_at("k1", None))
        .await;
    ensure!(
        matches!(missing, Err(Error::NotFound { .. })),
        "the refused record was written in part: `k1` reads {missing:?}. A record \
         carrying a field the source does not write is refused whole, never trimmed"
    );
    Ok(())
}

async fn an_upsert_carrying_a_field_only_a_source_writes_is_refused_as_the_wrong_door(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    declare(cat.as_ref(), &own, feed()).await?;

    let before = store.head().await.must("reading the head")?;
    let err = cat
        .upsert(
            &own,
            &TypeName::from("feed"),
            &"k1".into(),
            patch(json!({"body": "typed in", "verdict": "approved"})),
            Default::default(),
        )
        .await
        .must_fail("the owner upserting the source field `body`")?;
    head_unmoved(
        store.as_ref(),
        before,
        "the owner upserting the source field `body`",
    )
    .await?;
    ensure!(
        matches!(
            &err,
            Error::Refused { field: Some(f), reason: Refusal::WrongDoor } if f.as_str() == "body"
        ),
        "an upsert carrying `body`, which only a source writes, must be refused as the \
         wrong door on `body`, even for the owner; it failed with {err:?}"
    );
    let missing = cat
        .get(&own, &TypeName::from("feed"), get_at("k1", None))
        .await;
    ensure!(
        matches!(missing, Err(Error::NotFound { .. })),
        "the refused upsert was applied in part: `k1` reads {missing:?}"
    );
    Ok(())
}
