//! Every answer carries its position, and a read can be pinned to an
//! earlier one (`docs/decisions/009-every-response-carries-its-position.md`).
//!
//! An `as_of` read is the lake as it was right after folding that
//! position: a write's outcome names the position its change is visible
//! from, so reading as of that position sees the write, and reading as of
//! the position before it does not. This is the check an engine cannot
//! pass by keeping only current state.

use std::sync::Arc;

use lakelands_api::ontology::StorageKind;
use lakelands_api::{Error, Find, LakeStore, Missing, Position, ReadAt, Refusal, TypeName};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Must, all, before, declare, doors, field, get_at, owner, type_decl, upsert, value,
};

check!(
    /// Writes and reads both answer their position.
    EVERY_REPLY_CARRIES_ITS_POSITION = every_reply_carries_the_position_it_reflects,
    "009: every answer to a read or a write carries the log position it reflects, so what a caller saw can be read again"
);

check!(
    /// `as_of` a write's position sees that write and nothing later.
    AS_OF_SEES_THE_STATE_THERE = a_read_as_of_a_write_sees_that_write_and_nothing_later,
    "009: a read pinned with as_of answers the lake as it was at that position, not the head"
);

check!(
    /// `as_of` the position before a write does not see it.
    AS_OF_BEFORE_A_WRITE = a_read_as_of_the_position_before_a_write_does_not_see_it,
    "009: the position a write answers is the first from which it is visible; one earlier, it is not"
);

check!(
    /// Retiring removes a record from the head, not from history.
    RETIRE_KEEPS_HISTORY = retiring_a_record_does_not_erase_its_history,
    "the log format: a retired record is invisible at and after its retirement, and its history is kept"
);

check!(
    /// A position the log has not reached is refused.
    PAST_THE_HEAD = a_read_as_of_a_position_past_the_head_is_refused_as_out_of_range,
    "015: a position that is not in the lake's log is refused as out of range, naming the head"
);

async fn counter(store: &dyn LakeStore) -> Result<(), Failure> {
    let cat = doors(store);
    let ty = type_decl("counter", vec![field("n", StorageKind::Number)]);
    declare(cat.as_ref(), &owner(store), ty).await?;
    Ok(())
}

async fn every_reply_carries_the_position_it_reflects(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let ty = type_decl("counter", vec![field("n", StorageKind::Number)]);
    let declared = declare(cat.as_ref(), &own, ty).await?;
    let first = upsert(cat.as_ref(), &own, "counter", "k", json!({"n": 1})).await?;
    let second = upsert(cat.as_ref(), &own, "counter", "k", json!({"n": 2})).await?;
    ensure!(
        declared.position() < first.position() && first.position() < second.position(),
        "three writes in a row answered positions {}, {} and {}; each write appends, \
         so they must increase",
        declared.position(),
        first.position(),
        second.position()
    );

    let head = store.head().await.must("reading the head")?;
    let counter = TypeName::from("counter");
    let got = cat
        .get(&own, &counter, get_at("k", None))
        .await
        .must("reading `k` at the head")?;
    let found = cat
        .find(&own, &counter, Find::default())
        .await
        .must("finding counters at the head")?;
    let described = cat
        .describe(&own, ReadAt::default())
        .await
        .must("describing the lake")?;
    for (door, position) in [
        ("get", got.position),
        ("find", found.position),
        ("describe", described.position),
    ] {
        ensure!(
            second.position() <= position && position <= head,
            "`{door}` at the head answered position {position}; the last write is visible \
             from {} and the head is {head}, so a read at the head reflects a position \
             between the two",
            second.position()
        );
    }

    let pinned = cat
        .get(&own, &counter, get_at("k", Some(first.position())))
        .await
        .must("reading `k` as of the first write")?;
    ensure!(
        pinned.position == first.position(),
        "a read pinned as_of {} answered position {}; a pinned read reflects exactly \
         the position it was pinned to",
        first.position(),
        pinned.position
    );
    Ok(())
}

async fn a_read_as_of_a_write_sees_that_write_and_nothing_later(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    counter(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let first = upsert(cat.as_ref(), &own, "counter", "k", json!({"n": 1})).await?;
    upsert(cat.as_ref(), &own, "counter", "k", json!({"n": 2})).await?;
    upsert(cat.as_ref(), &own, "counter", "other", json!({"n": 9})).await?;

    let counter = TypeName::from("counter");
    let live = cat
        .get(&own, &counter, get_at("k", None))
        .await
        .must("reading `k` at the head")?;
    ensure!(
        value(&live.data, "n") == Some(&json!(2)),
        "the head must show the last write, n = 2; it shows {:?}",
        value(&live.data, "n")
    );

    let at = get_at("k", Some(first.position()));
    let then = cat
        .get(&own, &counter, at.clone())
        .await
        .must("reading `k` as of the first write")?;
    ensure!(
        value(&then.data, "n") == Some(&json!(1)),
        "as of position {} (the first write), `k` must read n = 1; it reads {:?}. \
         The engine is serving the head, not the lake as it was there",
        first.position(),
        value(&then.data, "n")
    );
    let again = cat
        .get(&own, &counter, at)
        .await
        .must("reading `k` as of the first write again")?;
    ensure!(
        then == again,
        "the same as_of read answered twice differently: {then:?} then {again:?}"
    );

    let find = Find {
        at: ReadAt {
            as_of: Some(first.position()),
            valid_at: None,
        },
        ..Find::default()
    };
    let rows = crate::conformance::fixture::all_found(cat.as_ref(), &own, "counter", find).await?;
    let keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
    ensure!(
        keys == ["k"],
        "`find` as of the first write must see only `k` (`other` was written later); \
         it answered {keys:?}"
    );
    Ok(())
}

async fn a_read_as_of_the_position_before_a_write_does_not_see_it(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    counter(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let first = upsert(cat.as_ref(), &own, "counter", "k", json!({"n": 1})).await?;

    let earlier = before(&first);
    let got = cat
        .get(&own, &TypeName::from("counter"), get_at("k", Some(earlier)))
        .await;
    ensure!(
        matches!(
            &got,
            Err(Error::NotFound { missing: Missing::Record { key, .. } }) if key.as_str() == "k"
        ),
        "the write creating `k` answered position {}; as of position {earlier} the record \
         does not exist yet and must be not found, but the read answered {got:?}",
        first.position()
    );
    Ok(())
}

async fn retiring_a_record_does_not_erase_its_history(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    counter(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let counter = TypeName::from("counter");
    let written = upsert(cat.as_ref(), &own, "counter", "k", json!({"n": 7})).await?;
    cat.retire(&own, &counter, &"k".into(), Default::default())
        .await
        .must("retiring `k`")?;

    let live = cat.get(&own, &counter, get_at("k", None)).await;
    ensure!(
        matches!(live, Err(Error::NotFound { .. })),
        "a retired record must not be found at the head; the read answered {live:?}"
    );
    let rows = all(cat.as_ref(), &own, "counter").await?;
    ensure!(
        rows.is_empty(),
        "`find` at the head must not list a retired record; it listed {:?}",
        rows.iter().map(|r| r.key.as_str()).collect::<Vec<_>>()
    );

    let then = cat
        .get(&own, &counter, get_at("k", Some(written.position())))
        .await
        .must("reading `k` as of its write, before it was retired")?;
    ensure!(
        value(&then.data, "n") == Some(&json!(7)),
        "as of its write, the retired record must still read n = 7; it reads {:?}. \
         Retiring must not reach back through the log",
        value(&then.data, "n")
    );
    Ok(())
}

async fn a_read_as_of_a_position_past_the_head_is_refused_as_out_of_range(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    counter(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    upsert(cat.as_ref(), &own, "counter", "k", json!({"n": 1})).await?;
    let head = store.head().await.must("reading the head")?;

    let beyond = Position(head.0 + 1000);
    let got = cat
        .get(&own, &TypeName::from("counter"), get_at("k", Some(beyond)))
        .await;
    ensure!(
        matches!(
            &got,
            Err(Error::Refused { reason: Refusal::OutOfRange { head: h }, .. }) if *h == head
        ),
        "a read as of position {beyond}, past the head {head}, must be refused as out of \
         range naming the head; it answered {got:?}"
    );
    Ok(())
}
