//! A lease another writer took
//! (`docs/decisions/008-one-writer-per-lake.md`): the handle that held it
//! learns so at its next append, which the engine refuses, and is no
//! writer from then on. Its next write takes the lease again first, as
//! after a session lost under it, and while the other holds the lease
//! that take is refused and nothing is appended. The other writer takes
//! the lease through the harness's fault wrap.

use super::forgotten::{Fails, failing};
use crate::conformance::fixture::{doors, owner, type_of};
use crate::conformance::prelude::*;

check!(
    TAKEN_LEASE = a_handle_whose_lease_another_writer_took_is_no_writer,
    "008: a write through a handle whose lease another writer took is refused and appends nothing, and the handle is no writer after it; its next write takes the lease again first and, while the other holds it, is refused as held, appends nothing, and leaves the handle no writer (lakelands#1385)",
    given
);

async fn a_handle_whose_lease_another_writer_took_is_no_writer(store: Store, given: Given) -> Held {
    let fails: Fails = Arc::default();
    let opened = (given.fault)(store, Seams::default(), failing(&fails));
    let store = opened
        .await
        .map_err(|e| Failure::new(format!("opening the lake again for writing: {e}")))?;
    let l = Lake::of(store.as_ref());
    l.declare(type_of("note", &[("text", StorageKind::Text)]))
        .await?;
    let (cat, who) = (doors(store.as_ref()), owner(store.as_ref()));
    let before = store.head().await.must("reading the head")?;
    fails[8].store(1, Ordering::SeqCst);
    let first = try_upsert(cat.as_ref(), &who, "note", "n1", json!({"text": "a"})).await;
    let what = "a write once another writer took the lease";
    let err = refused_unmoved(store.as_ref(), before, what, first).await?;
    ensure!(
        fails[8].load(Ordering::SeqCst) == 0,
        "{what} was refused before it reached the engine: {err}"
    );
    ensure!(
        !store.is_writer(),
        "{what} was refused ({err}), yet the handle still says it is the writer: it holds the \
         lease no more until it takes it again"
    );
    let next = try_upsert(cat.as_ref(), &who, "note", "n2", json!({"text": "b"})).await;
    let what = "the next write, while the other writer holds the lease";
    let err = refused_unmoved(store.as_ref(), before, what, next).await?;
    let held = matches!(
        err.situation(),
        Some(Situation::LeaseHeld { .. } | Situation::LeaseLocked { .. })
    );
    ensure!(
        held && !store.is_writer(),
        "{what} takes the lease again first, and is refused as held, the handle no writer; it \
         answered {err}, the handle a writer: {}",
        store.is_writer()
    );
    Ok(())
}
