//! What a batch of callers' writes must keep
//! (`docs/decisions/008-one-writer-per-lake.md`).
//!
//! Writes that reach the writer together are judged each on its own: a
//! write refused leaves its neighbours as they were, a compare-and-set or
//! an idempotency key is decided once however many callers send it, and a
//! ceiling is held per write, never over the batch. The rows drive the
//! callers as [`concurrency`](super::concurrency) does.

use lakelands_api::location::Limits;
use lakelands_api::principal::PrincipalKind::Agent;
use lakelands_api::seam::Seams;

use super::concurrency::items;
use crate::conformance::backing::made;
use crate::conformance::prelude::*;

check!(
    ONE_WINNER_ON_A_VERSION =
        callers_expecting_one_version_leave_one_winner_and_conflicts_naming_it,
    "008 and 015: callers writing one record at once with the same expected version get one `applied`, and every other a conflict naming the version the winner left"
);

check!(
    ONE_KEY_ONE_WRITE = callers_sending_one_idempotency_key_at_once_make_one_write,
    "008 and 016: callers sending one idempotency key at once make one write and are answered alike"
);

check!(
    DEEP_CONTEXT_ALONE = one_write_deeper_than_the_ceiling_is_refused_beside_writes_that_land,
    "008 and 021: among writes made at once, one whose context is deeper than the lake's ceiling is refused too_deep alone, and the others land"
);

check!(
    BELOW_SET_CEILING = writers_each_moving_one_below_the_set_ceiling_all_land,
    "008 and 055: writers at once, each moving one membership under a ceiling of two, all land; the ceiling is held per write, not over the writes together",
    given
);

check!(
    BACKED_OWN_FILL = concurrent_writes_of_different_inputs_each_read_their_own_fill,
    "008 and 022: writers at once each send their own input to a backed field, and each reads the fill of its own"
);

check!(
    KEY_PER_PRINCIPAL = one_idempotency_key_from_different_principals_makes_a_write_for_each,
    "008 and 016: principals sending one idempotency key at once make a write each, and each principal's callers are answered alike"
);

check!(
    ONE_LINK_LANDS = callers_linking_one_end_of_a_one_target_relation_at_once_land_one,
    "008 and 2.3: callers linking one record to different partners at once over a many_to_one relation land one link, the rest refused as a constraint on `to`"
);

check!(
    SETTING_WHOLE = a_setting_written_beside_its_reader_is_read_whole_or_not_at_all,
    "008 and seams: a reader at a position while a setting is written reads the setting as of that position, whole or not at all, never none and never both"
);

const N: usize = 8;

fn upserts_of(log: &[Event], key: &str) -> usize {
    let is = |e: &&Event| matches!(&e.body, EventBody::RecordUpserted { key: k, .. } if k.as_str() == key);
    log.iter().filter(is).count()
}

async fn callers_expecting_one_version_leave_one_winner_and_conflicts_naming_it(
    store: Store,
) -> Held {
    let l = items(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    l.upsert("item", "k", json!({"n": 0})).await?;
    let callers: Vec<Caller<'static, lakelands_api::Result<Outcome>>> = (0..N)
        .map(|w| {
            let (cat, own) = (cat.clone(), own.clone());
            let opts = WriteOpts {
                expected_version: Some(Version(1)),
                ..WriteOpts::default()
            };
            let caller: Caller<'static, _> = Box::pin(async move {
                let (ty, key) = (TypeName::from("item"), RecordKey::from("k"));
                cat.upsert(&own, &ty, &key, patch(json!({"n": w})), opts)
                    .await
            });
            caller
        })
        .collect();
    let answers = join_all(callers).await;
    let won: Vec<&Outcome> = answers.iter().filter_map(|a| a.as_ref().ok()).collect();
    let [
        Outcome::Applied {
            version: Some(left),
            ..
        },
    ] = won[..]
    else {
        return Err(Failure::new(format!(
            "{N} writes expected version 1 of `k` at once; exactly one is applied, and {} were: {answers:?}",
            won.len()
        )));
    };
    ensure!(
        *left == Version(2),
        "the winner made version {left} of `k`, which stood at 1; it makes version 2"
    );
    for lost in answers.iter().filter_map(|a| a.as_ref().err()) {
        ensure!(
            *lost
                == Error::Conflict {
                    expected: Version(1),
                    actual: *left
                },
            "a write expecting version 1 of `k`, beside the one that made {left}, answered {lost:?}; it is a conflict naming {left}"
        );
    }
    let log = log_events(store.as_ref()).await?;
    ensure!(
        upserts_of(&log, "k") == 2 && l.read("item", "k").await?.version == *left,
        "`k` was made at version 1 and by the winner alone; the log holds {} writes of it",
        upserts_of(&log, "k")
    );
    Ok(())
}

async fn callers_sending_one_idempotency_key_at_once_make_one_write(store: Store) -> Held {
    let l = items(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let callers: Vec<Caller<'static, lakelands_api::Result<Outcome>>> = (0..N)
        .map(|_| {
            let (cat, own) = (cat.clone(), own.clone());
            let opts = WriteOpts {
                idempotency_key: Some(IdempotencyKey::from("once")),
                ..WriteOpts::default()
            };
            let caller: Caller<'static, _> = Box::pin(async move {
                let (ty, key) = (TypeName::from("item"), RecordKey::from("k"));
                cat.upsert(&own, &ty, &key, patch(json!({"n": 1})), opts)
                    .await
            });
            caller
        })
        .collect();
    let answers = join_all(callers).await;
    ensure!(
        answers[0].is_ok() && answers.iter().all(|a| *a == answers[0]),
        "{N} callers sent one idempotency key at once; they are answered alike with the one write's outcome, and answered {answers:?}"
    );
    let log = log_events(store.as_ref()).await?;
    ensure!(
        upserts_of(&log, "k") == 1,
        "one key is one write; the log holds {} writes of `k`",
        upserts_of(&log, "k")
    );
    Ok(())
}

async fn one_write_deeper_than_the_ceiling_is_refused_beside_writes_that_land(
    store: Store,
) -> Held {
    let l = items(store.as_ref()).await?;
    let max = limits().max_reactive_depth;
    let callers: Vec<Caller<'static, lakelands_api::Result<Outcome>>> = (1..=max + 1)
        .rev()
        .map(|depth| {
            let (cat, own) = (store.catalog(reactive(depth, None)), l.own.clone());
            let caller: Caller<'static, _> = Box::pin(async move {
                let (ty, key) = (TypeName::from("item"), RecordKey::from(format!("d{depth}")));
                cat.upsert(
                    &own,
                    &ty,
                    &key,
                    patch(json!({"n": depth})),
                    WriteOpts::default(),
                )
                .await
            });
            caller
        })
        .collect();
    let answers = join_all(callers).await;
    let (deep, shallow) = answers.split_first().expect("the writes answered");
    ensure!(
        matches!(reason(deep), Some(Refusal::TooDeep { max: named }) if *named == max),
        "the write at depth {} answered {deep:?}; the lake's ceiling is {max}, and it is refused too_deep naming it",
        max + 1
    );
    ensure!(
        shallow.iter().all(|a| a.is_ok()),
        "the writes at depth 1 to {max}, beside the one past the ceiling, answered {shallow:?}; each lands"
    );
    let log = log_events(store.as_ref()).await?;
    ensure!(
        upserts_of(&log, &format!("d{}", max + 1)) == 0
            && (1..=max).all(|d| upserts_of(&log, &format!("d{d}")) == 1),
        "the log holds the writes at depth 1 to {max} once each, and none past it"
    );
    Ok(())
}

async fn writers_each_moving_one_below_the_set_ceiling_all_land(
    store: Store,
    given: Given,
) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare_json(json!({"declare": "type", "name": "sc_inv",
        "fields": [{"name": "customer", "type": {"kind": "text"}},
                   {"name": "amount", "type": {"kind": "decimal"}}]}))
        .await?;
    l.declare_json(
        json!({"declare": "type", "name": "sc_bal", "key": ["customer"],
        "fields": [{"name": "customer", "type": {"kind": "text"}},
                   {"name": "total", "type": {"kind": "decimal"}}],
        "view": {"source": "sc_inv", "group_by": ["customer"],
                 "measures": [{"name": "total", "expr": "sum(amount)"}]}}),
    )
    .await?;
    l.set("view_kept.sc_bal", json!(true)).await?;
    l.cat
        .declare(&l.own, object_set_on("sc_owing", "sc_bal", "total > 100"))
        .await
        .must("declaring `sc_owing`")?;
    let two = Limits {
        max_set_changes: 2,
        ..limits()
    };
    let relimit = (given.relimit)(store, Seams::default(), two);
    let store = relimit
        .await
        .map_err(|e| Failure::new(format!("opening at a ceiling of two: {e}")))?;
    let l = Lake::of(store.as_ref());
    let callers: Vec<Caller<'static, Result<Outcome, Failure>>> = (0..N)
        .map(|w| {
            let (cat, own) = (l.cat.clone(), l.own.clone());
            let caller: Caller<'static, _> = Box::pin(async move {
                let fields = json!({"customer": format!("c{w}"), "amount": "150"});
                crate::conformance::fixture::upsert(
                    cat.as_ref(),
                    &own,
                    "sc_inv",
                    &format!("i{w}"),
                    fields,
                )
                .await
            });
            caller
        })
        .collect();
    for r in join_all(callers).await {
        r?;
    }
    let owing = l
        .all_found("sc_bal", find_where("total > 100"))
        .await?
        .len();
    ensure!(
        owing == N,
        "{N} writers each took a customer into `sc_owing`, one membership each under a ceiling of two; all land, and {owing} customers are in it"
    );
    Ok(())
}

async fn concurrent_writes_of_different_inputs_each_read_their_own_fill(store: Store) -> Held {
    let token = "backed_own_fill";
    let l =
        super::backings::places(store.as_ref(), json!({"token": token, "echo": "code"})).await?;
    let callers: Vec<Caller<'static, Result<(String, Position), Failure>>> = (0..N)
        .map(|w| {
            let (cat, own) = (l.cat.clone(), l.own.clone());
            let caller: Caller<'static, _> = Box::pin(async move {
                let key = format!("p{w}");
                let fields = json!({"code": format!("C{w}")});
                let o =
                    crate::conformance::fixture::upsert(cat.as_ref(), &own, "place", &key, fields)
                        .await?;
                Ok((key, o.position()))
            });
            caller
        })
        .collect();
    for (w, r) in join_all(callers).await.into_iter().enumerate() {
        let (key, at) = r?;
        let got = l
            .cat
            .get(
                &l.own,
                &TypeName::from("place"),
                crate::conformance::fixture::get_at(&key, Some(at)),
            )
            .await
            .must(&format!("reading `{key}` as of {at}"))?
            .data;
        ensure!(
            got.fields.get("name") == Some(&json!(format!("C{w}"))),
            "`{key}` was written with code C{w}, beside {} other writers' inputs; as of {at} its filled `name` reads {:?}",
            N - 1,
            got.fields.get("name")
        );
    }
    let calls = made(token);
    ensure!(
        calls == N,
        "{N} inputs, one each, call the backing {N} times; it was called {calls}"
    );
    Ok(())
}

async fn one_idempotency_key_from_different_principals_makes_a_write_for_each(
    store: Store,
) -> Held {
    let l = items(store.as_ref()).await?;
    let callers: Vec<Caller<'static, (usize, lakelands_api::Result<Outcome>)>> = (0..N)
        .map(|c| {
            let w = c % 4;
            let who = principal(store.as_ref(), Agent, &format!("agent{w}"), Grants::owner());
            let cat = l.cat.clone();
            let opts = WriteOpts {
                idempotency_key: Some(IdempotencyKey::from("shared-key")),
                ..WriteOpts::default()
            };
            let caller: Caller<'static, _> = Box::pin(async move {
                let (ty, key) = (TypeName::from("item"), RecordKey::from(format!("p{w}")));
                (
                    w,
                    cat.upsert(&who, &ty, &key, patch(json!({"n": w})), opts)
                        .await,
                )
            });
            caller
        })
        .collect();
    let mut by: BTreeMap<usize, Vec<lakelands_api::Result<Outcome>>> = BTreeMap::new();
    for (w, a) in join_all(callers).await {
        by.entry(w).or_default().push(a);
    }
    let firsts: BTreeSet<Position> = by
        .values()
        .filter_map(|a| a[0].as_ref().ok().map(Outcome::position))
        .collect();
    ensure!(
        by.len() == 4
            && firsts.len() == 4
            && by
                .values()
                .all(|a| a[0].is_ok() && a.iter().all(|x| *x == a[0])),
        "four principals sent one idempotency key, each from two callers at once; each is one write of its own and its callers are answered alike: {by:?}"
    );
    let log = log_events(store.as_ref()).await?;
    ensure!(
        (0..4).all(|w| upserts_of(&log, &format!("p{w}")) == 1),
        "the log holds each principal's write of its record once"
    );
    Ok(())
}

async fn callers_linking_one_end_of_a_one_target_relation_at_once_land_one(store: Store) -> Held {
    let l = super::links::lake(store.as_ref()).await?;
    for p in ["p3", "p4"] {
        l.upsert("partner", p, json!({"name": p})).await?;
    }
    let callers: Vec<Caller<'static, lakelands_api::Result<Outcome>>> = (1..=4)
        .map(|p| {
            let (cat, own) = (l.cat.clone(), l.own.clone());
            let caller: Caller<'static, _> = Box::pin(async move {
                let (ty, rel) = (TypeName::from("invoice"), "approved_by".into());
                let (from, to) = (RecordKey::from("i1"), RecordKey::from(format!("p{p}")));
                cat.link(
                    &own,
                    &ty,
                    &from,
                    &rel,
                    &to,
                    patch(json!({})),
                    WriteOpts::default(),
                )
                .await
            });
            caller
        })
        .collect();
    let answers = join_all(callers).await;
    let landed = answers.iter().filter(|a| a.is_ok()).count();
    ensure!(
        landed == 1
            && answers
                .iter()
                .filter(|a| a.is_err())
                .all(|a| matches!(refused_on(a), Some(("to", Refusal::Constraint(..))))),
        "four callers linked `i1` to four partners at once over `approved_by`, many_to_one; one lands and the rest are refused as a constraint on `to`: {answers:?}"
    );
    let live = all(l.cat.as_ref(), &l.own, "sys.link.invoice.approved_by")
        .await?
        .len();
    ensure!(
        live == 1,
        "`i1` holds {live} live links over `approved_by`; it holds one"
    );
    Ok(())
}

async fn a_setting_written_beside_its_reader_is_read_whole_or_not_at_all(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("band", vec![field("level", StorageKind::Number)]))
        .await?;
    l.upsert("band", "low", json!({"level": 1})).await?;
    l.upsert("band", "high", json!({"level": 2})).await?;
    l.set("threshold", json!(1)).await?;
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (wl, flag) = (
        Lake {
            cat: l.cat.clone(),
            own: l.own.clone(),
        },
        done.clone(),
    );
    let writer: Caller<'static, Result<usize, Failure>> = Box::pin(async move {
        let mut out = Ok(0);
        for n in 0..20 {
            if let Err(e) = wl.set("threshold", json!(2 - n % 2)).await {
                out = Err(e);
                break;
            }
        }
        flag.store(true, Ordering::SeqCst);
        out
    });
    let (s, cat, own) = (store.clone(), l.cat.clone(), l.own.clone());
    let reader: Caller<'static, Result<usize, Failure>> = Box::pin(async move {
        let mut reads = 0;
        while !done.load(Ordering::SeqCst) {
            let at = s.head().await.must("reading the head")?;
            let by = Find {
                filter: Some(
                    "level = setting('threshold') and setting('threshold') = setting('threshold')"
                        .into(),
                ),
                at: read_at(Some(at)),
                ..Find::default()
            };
            let found = all_found(cat.as_ref(), &own, "band", by).await?.len();
            ensure!(
                found == 1,
                "`threshold` is 1 or 2 as of {at}, whichever write it was; a band read against `threshold` twice answered {found} bands, and one matches"
            );
            reads += 1;
        }
        Ok(reads)
    });
    let mut answers = join_all(vec![writer, reader]).await.into_iter();
    answers.next().expect("the writer answered")?;
    let reads = answers.next().expect("the reader answered")?;
    ensure!(
        reads > 1,
        "the reader made {reads} reads while the setting was written; the check needs reads during the writes"
    );
    Ok(())
}
