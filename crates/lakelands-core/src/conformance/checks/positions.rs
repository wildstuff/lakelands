//! Every answer carries its position, and a read can be pinned to an
//! earlier one (`docs/decisions/009-every-response-carries-its-position.md`).
//!
//! An `as_of` read is the lake as it was right after folding that
//! position: a write's outcome names the position its change is visible
//! from, so reading as of that position sees the write, and reading as of
//! the position before it does not. This is the check an engine cannot
//! pass by keeping only current state.
//!
//! `position_at` turns an instant into such a position: the last one, no
//! later than the head, whose event's `known_at` is at or before it
//! (`docs/log-format.md` §1.3).

use std::sync::Arc;

use lakelands_api::ontology::StorageKind;
use lakelands_api::{Error, Find, LakeStore, Missing, Position, ReadAt, Refusal, TypeName};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, all, before, declare, doors, field, get_at, owner, reason, type_decl, value,
};

check!(
    /// An instant answers the last position at or before it.
    POSITION_AT_AN_INSTANT = position_at_answers_the_last_position_at_or_before_an_instant,
    "009 and log-format §1.3: position_at answers, at each event's known_at, the last event at or before it; between two instants the earlier one's last; before the first event none; past the head the head; and a read as of the answer sees the lake as it stood then"
);

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

async fn counter(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let cat = doors(store);
    let ty = type_decl("counter", vec![field("n", StorageKind::Number)]);
    declare(cat.as_ref(), &owner(store), ty).await?;
    Ok(Lake::of(store))
}

async fn every_reply_carries_the_position_it_reflects(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let ty = type_decl("counter", vec![field("n", StorageKind::Number)]);
    let declared = l.declare(ty).await?;
    let first = l.upsert("counter", "k", json!({"n": 1})).await?;
    let second = l.upsert("counter", "k", json!({"n": 2})).await?;
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
        .get(own, &counter, get_at("k", None))
        .await
        .must("reading `k` at the head")?;
    let found = cat
        .find(own, &counter, Find::default())
        .await
        .must("finding counters at the head")?;
    let described = cat
        .describe(own, ReadAt::default())
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
        .get(own, &counter, get_at("k", Some(first.position())))
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
    let l = counter(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let first = l.upsert("counter", "k", json!({"n": 1})).await?;
    l.upsert("counter", "k", json!({"n": 2})).await?;
    l.upsert("counter", "other", json!({"n": 9})).await?;

    let counter = TypeName::from("counter");
    let live = cat
        .get(own, &counter, get_at("k", None))
        .await
        .must("reading `k` at the head")?;
    ensure!(
        value(&live.data, "n") == Some(&json!(2)),
        "the head must show the last write, n = 2; it shows {:?}",
        value(&live.data, "n")
    );

    let at = get_at("k", Some(first.position()));
    let then = cat
        .get(own, &counter, at.clone())
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
        .get(own, &counter, at)
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
    let rows = crate::conformance::fixture::all_found(cat.as_ref(), own, "counter", find).await?;
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
    let l = counter(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let first = l.upsert("counter", "k", json!({"n": 1})).await?;

    let earlier = before(&first);
    let got = cat
        .get(own, &TypeName::from("counter"), get_at("k", Some(earlier)))
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
    let l = counter(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let counter = TypeName::from("counter");
    let written = l.upsert("counter", "k", json!({"n": 7})).await?;
    l.retire(counter.as_str(), "k").await?;

    let live = cat.get(own, &counter, get_at("k", None)).await;
    ensure!(
        matches!(live, Err(Error::NotFound { .. })),
        "a retired record must not be found at the head; the read answered {live:?}"
    );
    let rows = all(cat.as_ref(), own, "counter").await?;
    ensure!(
        rows.is_empty(),
        "`find` at the head must not list a retired record; it listed {:?}",
        rows.iter().map(|r| r.key.as_str()).collect::<Vec<_>>()
    );

    let then = cat
        .get(own, &counter, get_at("k", Some(written.position())))
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
    let l = counter(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    l.upsert("counter", "k", json!({"n": 1})).await?;
    let head = store.head().await.must("reading the head")?;

    let beyond = Position(head.0 + 1000);
    let got = cat
        .get(own, &TypeName::from("counter"), get_at("k", Some(beyond)))
        .await;
    ensure!(
        matches!(reason(&got), Some(Refusal::OutOfRange { head: h }) if *h == head),
        "a read as of position {beyond}, past the head {head}, must be refused as out of \
         range naming the head; it answered {got:?}"
    );
    Ok(())
}

async fn position_at_answers_the_last_position_at_or_before_an_instant(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_decl("tick", vec![field("n", StorageKind::Number)]))
        .await?;
    // One record per append, each under a key of its own.
    let mut written = Vec::new();
    for n in 0..12 {
        let key = format!("t{n}");
        let o = l.upsert("tick", &key, json!({"n": n})).await?;
        written.push((key, n, o.position()));
    }
    let head = store.head().await.must("reading the head")?;
    let events = store
        .feed()
        .poll(
            Position::GENESIS,
            &lakelands_api::seam::FeedFilter::default(),
            10_000,
        )
        .await
        .must("reading the log")?
        .events;
    let at = |t: lakelands_api::Timestamp| {
        let store = store.clone();
        async move {
            store
                .position_at(t)
                .await
                .must(&format!("asking position_at({t})"))
        }
    };
    // The last position no later than the head at or before `t`.
    let expected = |t: lakelands_api::Timestamp| {
        events
            .iter()
            .rev()
            .find(|e| e.position <= head && e.known_at <= t)
            .map(|e| e.position)
    };
    let mut instants: Vec<lakelands_api::Timestamp> = events.iter().map(|e| e.known_at).collect();
    instants.dedup();
    for (i, t) in instants.iter().enumerate() {
        let got = at(*t).await?;
        ensure!(
            got == expected(*t),
            "position_at({t}), an event's known_at, answers {got:?}; the last event at or \
             before it is {:?}",
            expected(*t)
        );
        if let Some(next) = instants.get(i + 1)
            && next.0 - t.0 >= 2
        {
            let between = lakelands_api::Timestamp(t.0 + 1);
            let got = at(between).await?;
            ensure!(
                got == expected(*t),
                "position_at({between}), between {t} and {next}, answers {got:?}; the last event \
                 at {t} is {:?}",
                expected(*t)
            );
        }
    }
    let before_first = lakelands_api::Timestamp(instants[0].0 - 1);
    let got = at(before_first).await?;
    ensure!(
        got.is_none(),
        "position_at({before_first}), before the lake's first event, answers {got:?}; none \
         is expected"
    );
    let far = lakelands_api::Timestamp(instants[instants.len() - 1].0 + 3_600_000_000);
    let got = at(far).await?;
    ensure!(
        got == Some(head),
        "position_at({far}), past every event, answers {got:?}; the head {head} is expected"
    );
    // A read as of the answer sees each write, made at that instant or
    // before it.
    for (key, n, position) in &written {
        let t = events
            .iter()
            .find(|e| e.position == *position)
            .map(|e| e.known_at)
            .ok_or_else(|| Failure::new(format!("the log holds position {position}")))?;
        let answer = at(t)
            .await?
            .ok_or_else(|| Failure::new(format!("position_at({t}) answers none")))?;
        let r = cat
            .get(own, &TypeName::from("tick"), get_at(key, Some(answer)))
            .await
            .must(&format!("reading `tick` `{key}` as of {answer}"))?;
        ensure!(
            value(&r.data, "n") == Some(&json!(n)),
            "`tick` `{key}` was written at {t}; as of position_at({t}) = {answer} it reads {:?}",
            value(&r.data, "n")
        );
    }
    Ok(())
}
