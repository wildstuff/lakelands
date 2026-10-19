//! An id the log separates by NUL holds none (`docs/log-format.md` §3).
//!
//! An event's id is a hash of its parts joined by NUL, so a source id, a
//! writer or an idempotency key holding NUL could make two events one.
//! Each is refused as invalid at every door that takes it, and nothing
//! is appended; a source id is refused empty too, as a key is.

use lakelands_api::ontology::StorageKind;
use lakelands_api::{
    Grants, IdempotencyKey, PrincipalKind, ReadAt, SourceRecord, TypeName, WriteOpts,
};
use serde_json::json;

use crate::conformance::fixture::{
    Lake, Must, field, ingest, invalid_like, patch, principal, refused_unmoved, source_record,
    sourced, type_decl,
};
use crate::conformance::{Held, Store};

check!(
    NO_NUL_IN_A_LOGGED_ID = a_logged_id_holding_nul_is_refused,
    "the log format §3: a source id that is empty or holds NUL, and an idempotency key or a writer that holds NUL, are refused as invalid at every door that takes them, and append nothing"
);

async fn a_logged_id_holding_nul_is_refused(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let feed = vec![
        sourced("body", StorageKind::Text),
        field("note", StorageKind::Text),
    ];
    l.declare(type_decl("feed", feed)).await?;
    let head = store.head().await.must("reading the head")?;
    let source_id = json!({"code": "source_id_form"});
    // The call's source, and a record's own, alone or after a good one.
    let good = || source_record("feed", "k1", json!({"body": "x"}));
    let own = |s: &str| SourceRecord {
        source: Some(s.into()),
        ..source_record("feed", "k2", json!({"body": "y"}))
    };
    let deliveries = [
        ("", vec![good()]),
        ("ba\0nk", vec![good()]),
        ("bank", vec![own("dev\0a")]),
        ("bank", vec![good(), own("dev\0a")]),
        ("bank", vec![own("")]),
    ];
    for (source, records) in deliveries {
        let what = format!("an ingest from {source:?} of {records:?}");
        let answer = l.ingest(source, records).await;
        let err = refused_unmoved(store.as_ref(), head, &what, answer).await?;
        ensure!(
            invalid_like(&err, &source_id),
            "{what} was refused as {err:?}"
        );
    }
    let nul = "dev\0a".into();
    let what = "a purge preflight of `dev\\0a`";
    let answer = store.purge_preflight(&nul, None).await;
    let err = refused_unmoved(store.as_ref(), head, what, answer).await?;
    ensure!(
        invalid_like(&err, &source_id),
        "{what} was refused as {err:?}"
    );
    let what = "a purge of `dev\\0a`";
    let answer = store.purge_source(&nul, None, None).await;
    let err = refused_unmoved(store.as_ref(), head, what, answer).await?;
    ensure!(
        invalid_like(&err, &source_id),
        "{what} was refused as {err:?}"
    );

    let (ty, key) = (TypeName::from("feed"), "k3".into());
    let opts = WriteOpts {
        idempotency_key: Some(IdempotencyKey::from("try\0one")),
        ..WriteOpts::default()
    };
    let what = "an upsert with the idempotency key `try\\0one`";
    let note = || patch(json!({"note": "n"}));
    let answer = l.cat.upsert(&l.own, &ty, &key, note(), opts).await;
    let err = refused_unmoved(store.as_ref(), head, what, answer).await?;
    let idempotency = json!({"code": "idempotency_key_nul"});
    ensure!(
        invalid_like(&err, &idempotency),
        "{what} was refused as {err:?}"
    );

    // The writer, at a write door, the source door and a read.
    let who = principal(
        store.as_ref(),
        PrincipalKind::Agent,
        "a\0b",
        Grants::owner(),
    );
    let writer = json!({"code": "writer_nul"});
    let what = "an upsert as `agent:a\\0b`";
    let answer = l
        .cat
        .upsert(&who, &ty, &key, note(), WriteOpts::default())
        .await;
    let err = refused_unmoved(store.as_ref(), head, what, answer).await?;
    ensure!(invalid_like(&err, &writer), "{what} was refused as {err:?}");
    let what = "an ingest as `agent:a\\0b`";
    let answer = ingest(l.cat.as_ref(), &who, "bank", vec![good()]).await;
    let err = refused_unmoved(store.as_ref(), head, what, answer).await?;
    ensure!(invalid_like(&err, &writer), "{what} was refused as {err:?}");
    let what = "a describe as `agent:a\\0b`";
    let answer = l.cat.describe(&who, ReadAt::default()).await;
    let err = answer.must_fail(what)?;
    ensure!(invalid_like(&err, &writer), "{what} was refused as {err:?}");
    Ok(())
}
