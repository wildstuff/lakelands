//! A record's version, compare-and-set on it, and the idempotency key of
//! an authored write (`docs/glossary.md`,
//! `docs/decisions/016-two-write-doors-by-origin.md`, `docs/log-format.md`
//! §2.3 and §3).

use crate::conformance::fixture::{doors, get_at, owner};
use crate::conformance::prelude::*;

check!(
    VERSIONS_COUNT_CHANGES = versions_start_at_one_and_grow_by_one_with_every_change,
    "the glossary and the log format: a version starts at 1 and grows by one per event that changes the record, retirement included"
);

check!(
    STALE_EXPECTED_VERSION = a_write_expecting_a_superseded_version_is_refused_as_a_conflict,
    "015: a write naming a version the record has moved past fails as a conflict, and nothing of it lands"
);

check!(
    IDEMPOTENT_RETRY =
        a_retry_with_the_same_idempotency_key_answers_the_first_outcome_and_appends_nothing,
    "016 and the log format: one idempotency key from one principal is one write in the whole lake; a proposal made with a key carries it, and its retry answers the proposal and appends nothing"
);

check!(
    STALE_BEFORE_REQUIRES =
        a_verb_called_at_a_stale_version_is_a_conflict_before_its_requires_is_judged,
    "015 and 024, extended: a verb's call compares the expected version right after its record is found, so a call at a stale version is a conflict, never a precondition judged on a state the caller has not seen"
);

/// Writes `n` to the `counter` record `key` as `who`, with `opts`.
async fn put(
    cat: &dyn Catalog,
    who: &Principal,
    key: &str,
    n: u64,
    opts: WriteOpts,
) -> lakelands_api::Result<Outcome> {
    let counter = TypeName::from("counter");
    cat.upsert(who, &counter, &key.into(), patch(json!({"n": n})), opts)
        .await
}

fn version_of(outcome: &Outcome) -> Option<Version> {
    match outcome {
        Outcome::Applied { version, .. } => *version,
        Outcome::Proposed { .. } => None,
    }
}

async fn counter(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let cat = doors(store);
    let ty = type_decl("counter", vec![field("n", StorageKind::Number)]);
    declare(cat.as_ref(), &owner(store), ty).await?;
    Ok(Lake::of(store))
}

async fn versions_start_at_one_and_grow_by_one_with_every_change(store: Store) -> Held {
    let l = counter(store.as_ref()).await?;
    let counter = TypeName::from("counter");

    let one = l.upsert("counter", "k", json!({"n": 1})).await?;
    let two = l.upsert("counter", "k", json!({"n": 2})).await?;
    let retired = l.retire(counter.as_str(), "k").await?;
    let again = l.upsert("counter", "k", json!({"n": 3})).await?;

    let got: Vec<Option<u64>> = [&one, &two, &retired, &again]
        .iter()
        .map(|o| version_of(o).map(|v| v.0))
        .collect();
    ensure!(
        got == [Some(1), Some(2), Some(3), Some(4)],
        "write, write, retire, write again answered versions {got:?}; a record's \
         version starts at 1 and grows by one per change, and a retired key written \
         again continues its count, so they must be 1, 2, 3, 4"
    );
    let rec = l.read("counter", "k").await?;
    ensure!(
        rec.version == Version(4),
        "the record reads at version {}, but its last write answered version 4",
        rec.version
    );
    Ok(())
}

/// Options expecting the record at version `v`.
fn expecting(v: u64) -> WriteOpts {
    WriteOpts {
        expected_version: Some(Version(v)),
        idempotency_key: None,
    }
}

async fn a_write_expecting_a_superseded_version_is_refused_as_a_conflict(store: Store) -> Held {
    let l = counter(store.as_ref()).await?;
    let Lake { cat, own } = &l;

    put(cat.as_ref(), own, "k", 1, expecting(0))
        .await
        .must("creating `k` expecting version 0 (no such record yet)")?;
    put(cat.as_ref(), own, "k", 2, expecting(1))
        .await
        .must("writing `k` expecting its current version 1")?;

    let before = store.head().await.must("reading the head")?;
    let answer = put(cat.as_ref(), own, "k", 3, expecting(1)).await;
    let what = "writing `k` expecting version 1 after it moved to 2";
    let err = refused_unmoved(store.as_ref(), before, what, answer).await?;
    ensure!(
        err == Error::Conflict {
            expected: Version(1),
            actual: Version(2)
        },
        "a write expecting version 1 of a record at version 2 must fail as a conflict \
         (expected 1, actual 2); it failed with {err:?}"
    );
    let before = store.head().await.must("reading the head")?;
    let answer = put(cat.as_ref(), own, "k", 4, expecting(0)).await;
    let what = "creating `k` expecting version 0 when it already exists";
    let err = refused_unmoved(store.as_ref(), before, what, answer).await?;
    ensure!(
        err == Error::Conflict {
            expected: Version(0),
            actual: Version(2)
        },
        "a write expecting no record (version 0) on a record at version 2 must fail as \
         a conflict (expected 0, actual 2); it failed with {err:?}"
    );

    let rec = l.read("counter", "k").await?;
    ensure!(
        value(&rec, "n") == Some(&json!(2)) && rec.version == Version(2),
        "the losing writes must not land: `k` reads n = {:?} at version {}, expected \
         n = 2 at version 2",
        value(&rec, "n"),
        rec.version
    );
    Ok(())
}

async fn a_retry_with_the_same_idempotency_key_answers_the_first_outcome_and_appends_nothing(
    store: Store,
) -> Held {
    let l = counter(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let counter = TypeName::from("counter");
    let keyed = || WriteOpts {
        expected_version: None,
        idempotency_key: Some(IdempotencyKey::from("retry-1")),
    };

    let first = put(cat.as_ref(), own, "k", 1, keyed())
        .await
        .must("writing `k` with idempotency key `retry-1`")?;
    let head = store.head().await.must("reading the head")?;

    let retry = put(cat.as_ref(), own, "k", 1, keyed())
        .await
        .must("retrying the same write with the same key")?;
    ensure!(
        retry == first,
        "a retry with the same idempotency key must answer the first write's outcome \
         {first:?}; it answered {retry:?}"
    );
    let other = put(cat.as_ref(), own, "other", 5, keyed())
        .await
        .must("writing another record with the same key")?;
    ensure!(
        other == first,
        "one idempotency key from one principal is one write in the whole lake, whatever \
         it writes: reusing it for `other` must answer {first:?}; it answered {other:?}"
    );
    let after = store.head().await.must("reading the head")?;
    ensure!(
        after == head,
        "the retries moved the head from {head} to {after}; a retry appends nothing"
    );
    let missing = cat.get(own, &counter, get_at("other", None)).await;
    ensure!(
        matches!(missing, Err(Error::NotFound { .. })),
        "the write of `other` reused a spent key and must not have happened; `other` \
         reads {missing:?}"
    );

    let someone = principal(
        store.as_ref(),
        PrincipalKind::Agent,
        "someone-else",
        lakelands_api::Grants::owner(),
    );
    let theirs = put(cat.as_ref(), &someone, "k", 9, keyed())
        .await
        .must("another principal writing with the same key")?;
    ensure!(
        theirs != first && theirs.position() > after,
        "the key is scoped to its principal: another principal's write with the same \
         key is a write of its own and must append; it answered {theirs:?}"
    );

    let key = IdempotencyKey::from("retry-2");
    let propose = || {
        let write = ProposedWrite::Upsert {
            ty: counter.clone(),
            key: "p".into(),
            patch: patch(json!({"n": 3})),
            opts: WriteOpts {
                idempotency_key: Some(key.clone()),
                ..keyed()
            },
        };
        cat.propose(own, write, "for review".into())
    };
    let proposed = propose()
        .await
        .must("proposing `p` with idempotency key `retry-2`")?;
    let head = store.head().await.must("reading the head")?;
    let event = events_from(store.as_ref(), proposed.position()).await?;
    ensure!(
        matches!(proposed, Outcome::Proposed { .. })
            && event.first().and_then(|e| e.idempotency_key.as_ref()) == Some(&key),
        "a proposal made with idempotency key `retry-2` must answer `proposed` and its event \
         must carry the key; it answered {proposed:?}, and the event carries {:?}",
        event.first().map(|e| &e.idempotency_key)
    );
    let again = propose().await.must("retrying the proposal")?;
    let after = store.head().await.must("reading the head")?;
    ensure!(
        again == proposed && after == head,
        "a retried proposal with the same idempotency key must answer the first one, \
         {proposed:?}, and append nothing; it answered {again:?} and the head went from {head} \
         to {after}"
    );
    Ok(())
}

check!(
    CAS_ACROSS_RETIREMENT = a_write_expecting_the_version_before_a_retirement_is_a_conflict,
    "the glossary, the log format §2.3 and 015: a retired key keeps its count, so `expected_version` is compared with the retired record's version, a match re-creates it, and an older one is a conflict reporting that version"
);

async fn a_write_expecting_the_version_before_a_retirement_is_a_conflict(store: Store) -> Held {
    use crate::conformance::fixture::head_unmoved;
    let l = counter(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let counter = TypeName::from("counter");
    l.upsert("counter", "k", json!({"n": 1})).await?;
    l.retire(counter.as_str(), "k").await?;
    let before = store.head().await.must("reading the head")?;
    let err = put(cat.as_ref(), own, "k", 5, expecting(1))
        .await
        .must_fail("writing `k` expecting version 1 after its retirement moved it to 2")?;
    head_unmoved(
        store.as_ref(),
        before,
        "writing against a pre-retirement version",
    )
    .await?;
    ensure!(
        err == Error::Conflict {
            expected: Version(1),
            actual: Version(2)
        },
        "a write expecting version 1 of a key retired at version 2 must fail as a \
         conflict reporting the retired record's version (expected 1, actual 2); 0 is \
         only for a key never written; it failed with {err:?}"
    );
    let again = put(cat.as_ref(), own, "k", 3, expecting(2))
        .await
        .must("re-creating `k` expecting the retired record's version 2")?;
    ensure!(
        version_of(&again) == Some(Version(3)),
        "a write expecting the retired record's version must re-create it and continue \
         its count at 3; it answered {again:?}"
    );
    let err = put(cat.as_ref(), own, "k", 9, expecting(1))
        .await
        .must_fail("writing the re-created `k` expecting its first life's version 1")?;
    ensure!(
        err == Error::Conflict {
            expected: Version(1),
            actual: Version(3)
        },
        "a version from before the retirement must never apply to the record written \
         after it (expected 1, actual 3); it failed with {err:?}"
    );
    Ok(())
}

async fn a_verb_called_at_a_stale_version_is_a_conflict_before_its_requires_is_judged(
    store: Store,
) -> Held {
    let l = counter(store.as_ref()).await?;
    l.declare_json(
        json!({"declare": "verb", "name": "reset", "on": "counter", "risk": "low",
                          "requires": "n > 0", "effect": "write",
                          "set": [{"field": "n", "value": "0"}]}),
    )
    .await?;
    l.upsert("counter", "k", json!({"n": 1})).await?;
    l.upsert("counter", "k", json!({"n": 0})).await?;
    let before = store.head().await.must("reading the head")?;
    let stale = WriteOpts {
        expected_version: Some(Version(1)),
        ..WriteOpts::default()
    };
    let got = l
        .cat
        .call(
            &l.own,
            &"reset".into(),
            Some(&"k".into()),
            patch(json!({})),
            stale,
        )
        .await;
    ensure!(
        matches!(
            got,
            Err(Error::Conflict {
                expected: Version(1),
                actual: Version(2)
            })
        ),
        "`reset` called at version 1 on `k`, which is at version 2 and no longer holds \
         `n > 0`: the version is compared first, so it is a conflict naming 1 and 2; it \
         answered {got:?}"
    );
    head_unmoved(store.as_ref(), before, "a call at a stale version").await
}
