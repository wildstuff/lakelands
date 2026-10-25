//! A caller reads its own writes, and a write whose answer was lost is
//! retried safely
//! (`docs/decisions/076-a-lake-keeps-one-writer-and-a-dataset-that-outgrows-it-becomes-several-lakes.md`).
//!
//! A handle that serves reads beside the lake's writer may be behind it,
//! so a read that must not answer from before a write at a position
//! waits on `reaches` for it, and is told at its bound, never later,
//! that the lake has not got there. A write whose answer never reached
//! its caller, because its writer went away, is retried with its
//! idempotency key against the writer that took over, which answers the
//! first outcome and appends nothing.

use crate::conformance::fixture::{doors, get_at, owner};
use crate::conformance::prelude::*;
use lakelands_api::engine::OpenMode;

check!(
    REACHES = a_reach_answers_once_the_head_is_there_and_is_unavailable_at_its_bound,
    "076 (lakelands#1394): `reaches(at, within)` on the writer answers the head at once when it is at `at` or past it, answers once another caller's write puts it there within the bound, and with nothing written is `unavailable` as `not_folded_yet` naming `at` and the head, once the bound has passed and within a second of it"
);

check!(
    REACHES_UNLEASED = a_reach_without_the_lease_follows_the_writers_fold,
    "076 (lakelands#1394): `reaches(at, within)` on a handle opened read-only beside the lake's writer answers once the writer's later write is folded there, and with nothing written is `unavailable` as `not_folded_yet` naming `at` and what it has folded, once the bound has passed and within a second of it",
    given
);

check!(
    LOST_ANSWER = a_retry_after_the_writer_went_away_answers_the_first_outcome,
    "016 and 076 (lakelands#1394): a write made with an idempotency key, whose writer is closed and whose lake is opened again by another, is retried with that key: the retry answers the first write's outcome, appends nothing, and the record reads as the first write left it",
    reopens
);

/// The bound a quiet reach waits out.
const BOUND: Duration = Duration::from_millis(200);

/// How far past its bound a quiet reach may answer on a loaded machine.
const SLACK: Duration = Duration::from_secs(1);

/// A wait for an answer that should come.
const WAIT: Duration = Duration::from_secs(30);

async fn a_reach_answers_once_the_head_is_there_and_is_unavailable_at_its_bound(
    store: Store,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("note", vec![field("text", StorageKind::Text)]))
        .await?;
    let written = l
        .upsert("note", "n1", json!({"text": "one"}))
        .await?
        .position();
    let reached = store.reaches(written, Duration::ZERO).await;
    ensure!(
        reached.as_ref().is_ok_and(|p| *p >= written),
        "a reach of the write at {written}, with no time to wait, must answer at once; it \
         answered {reached:?}"
    );

    let at = written.next();
    let reach: Caller<'_, Result<Position, String>> =
        Box::pin(async { store.reaches(at, WAIT).await.map_err(|e| e.to_string()) });
    let write: Caller<'_, Result<Position, String>> = Box::pin(async {
        pause(Duration::from_millis(20)).await;
        let o = l.upsert("note", "n2", json!({"text": "two"})).await;
        o.map(|o| o.position()).map_err(|f| f.to_string())
    });
    let [reached, wrote] = <[_; 2]>::try_from(join_all(vec![reach, write]).await)
        .map_err(|_| Failure::new("two callers answer twice"))?;
    let wrote = wrote.map_err(Failure::new)?;
    ensure!(
        reached.as_ref().is_ok_and(|p| *p >= wrote && wrote >= at),
        "a reach of {at} while another caller wrote at {wrote} must answer once the head is \
         there; it answered {reached:?}"
    );

    let head = store.head().await.must("reading the head")?;
    let at = head.next();
    let started = std::time::Instant::now();
    let quiet = store.reaches(at, BOUND).await;
    let waited = started.elapsed();
    ensure!(
        matches!(
            quiet.as_ref().map_err(Error::situation),
            Err(Some(Situation::NotFoldedYet { at: a, folded })) if *a == at && *folded == head
        ),
        "a reach of {at} with nothing written must be unavailable as not_folded_yet naming \
         {at} and the head {head}; it answered {quiet:?}"
    );
    ensure!(
        waited >= BOUND && waited < BOUND + SLACK,
        "a quiet reach bound at {BOUND:?} must answer once the bound has passed and within \
         {SLACK:?} of it; it answered after {waited:?}"
    );
    head_unmoved(store.as_ref(), head, "the quiet reach").await
}

async fn a_reach_without_the_lease_follows_the_writers_fold(store: Store, given: Given) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("note", vec![field("text", StorageKind::Text)]))
        .await?;
    l.upsert("note", "n1", json!({"text": "one"})).await?;
    let lake = store.lake().clone();
    let open = |store, mode| {
        let none: crate::conformance::Lines = Box::new(|_, _| (Vec::new(), 0));
        (given.continued)(store, lake.clone(), none, mode, Seams::default())
    };
    let reader = (open(Some(store), OpenMode::Read).await)
        .and_then(|c| c.opened)
        .map_err(|e| Failure::new(format!("opening the lake read-only: {e}")))?;
    let writer = (open(None, OpenMode::Write).await)
        .and_then(|c| c.opened)
        .map_err(|e| Failure::new(format!("opening the lake's writer beside it: {e}")))?;
    let w = Lake::of(writer.as_ref());
    let at = writer
        .head()
        .await
        .must("reading the writer's head")?
        .next();
    let reach: Caller<'_, Result<Position, String>> =
        Box::pin(async { reader.reaches(at, WAIT).await.map_err(|e| e.to_string()) });
    let write: Caller<'_, Result<Position, String>> = Box::pin(async {
        pause(Duration::from_millis(20)).await;
        let o = w.upsert("note", "n2", json!({"text": "two"})).await;
        o.map(|o| o.position()).map_err(|f| f.to_string())
    });
    let [reached, wrote] = <[_; 2]>::try_from(join_all(vec![reach, write]).await)
        .map_err(|_| Failure::new("two callers answer twice"))?;
    let wrote = wrote.map_err(Failure::new)?;
    ensure!(
        reached.as_ref().is_ok_and(|p| *p >= wrote && wrote >= at),
        "a reach of {at} on a handle without the lease, while the writer wrote at {wrote}, \
         must answer once that write is folded; it answered {reached:?}"
    );

    let head = writer.head().await.must("reading the writer's head")?;
    let at = head.next();
    let started = std::time::Instant::now();
    let quiet = reader.reaches(at, BOUND).await;
    let waited = started.elapsed();
    ensure!(
        matches!(
            quiet.as_ref().map_err(Error::situation),
            Err(Some(Situation::NotFoldedYet { at: a, folded })) if *a == at && *folded >= wrote
        ),
        "a reach of {at} on a handle without the lease, with nothing written, must be \
         unavailable as not_folded_yet naming {at} and what it has folded; it answered \
         {quiet:?}"
    );
    ensure!(
        waited >= BOUND && waited < BOUND + SLACK,
        "a quiet reach bound at {BOUND:?} on a handle without the lease must answer once the \
         bound has passed and within {SLACK:?} of it; it answered after {waited:?}"
    );
    head_unmoved(writer.as_ref(), head, "the quiet reach").await?;
    reader.close().await.must("closing the read-only handle")?;
    writer.close().await.must("closing the writer")?;
    Ok(())
}

async fn a_retry_after_the_writer_went_away_answers_the_first_outcome(
    store: Store,
    reopen: Reopen,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("note", vec![field("text", StorageKind::Text)]))
        .await?;
    let keyed = || WriteOpts {
        expected_version: None,
        idempotency_key: Some(IdempotencyKey::from("lost-1")),
    };
    let note = TypeName::from("note");
    let first = (l.cat)
        .upsert(
            &l.own,
            &note,
            &"n1".into(),
            patch(json!({"text": "one"})),
            keyed(),
        )
        .await
        .must("writing `n1` with idempotency key `lost-1`")?;
    let store = reopen(store, Seams::default())
        .await
        .map_err(|e| Failure::new(format!("opening the lake again: {e}")))?;
    let head = store
        .head()
        .await
        .must("reading the head after the reopen")?;
    let (cat, own) = (doors(store.as_ref()), owner(store.as_ref()));
    let retry = cat
        .upsert(
            &own,
            &note,
            &"n1".into(),
            patch(json!({"text": "two"})),
            keyed(),
        )
        .await
        .must("retrying with the same key after the reopen")?;
    ensure!(
        retry == first,
        "a retry with the key of a write the last writer made must answer that write's \
         outcome {first:?}; it answered {retry:?}"
    );
    head_unmoved(store.as_ref(), head, "the retry").await?;
    let read = cat.get(&own, &note, get_at("n1", None)).await;
    ensure!(
        read.as_ref()
            .is_ok_and(|r| r.data.fields.get("text") == Some(&json!("one"))),
        "`n1` must read as the first write left it; it reads {read:?}"
    );
    Ok(())
}
