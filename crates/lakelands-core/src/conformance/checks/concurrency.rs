//! Many callers on one lake at once
//! (`docs/decisions/008-one-writer-per-lake.md`).
//!
//! A lake has one writing process, and that is no limit on its callers:
//! many principals write through the one writer, which orders their
//! appends. What a caller sees must not depend on who else was writing:
//! every write lands once, in one order of the log; every answer names a
//! position the lake holds and the write is visible from; a read at a
//! position answers the same whenever it is made.
//!
//! **How the callers are driven.** Each check runs its callers as futures
//! joined in the check's one task ([`join_all`]). Every call the store
//! makes of its engine runs on the store's blocking pool, so while one
//! caller waits for its write the others go on, and their writes reach the
//! store together, waiting their turn at the writer as the calls of many
//! clients do. Concurrent here means that, not threads of the check's
//! own.

use std::sync::atomic::{AtomicBool, AtomicUsize};

use lakelands_api::Get;
use lakelands_api::ontology::StorageKind::{Number, Text};

use crate::conformance::backing::made;
use crate::conformance::fixture::{self, type_of};
use crate::conformance::prelude::*;

check!(
    WRITES_LAND_ONCE = concurrent_writes_each_land_once_in_one_order_of_the_log,
    "008: many callers write through the lake's one writer; every write lands once, the log's positions follow one another without a gap, every answer names a position the write is visible from, and the chain verifies"
);

check!(
    DECLARATION_ORDERED = writes_and_a_declaration_made_at_once_are_ordered_as_one_log,
    "008 and 027: a declaration made while others write takes its place in the same order; every write after it sees the new field at its position, none before it does"
);

check!(
    READS_AT_A_POSITION = a_read_at_a_position_answers_the_same_during_writes_and_after,
    "008 and 009: a read at a position sees the lake as it was there, whoever is writing; `find` and `get` at one position agree, and answer the same when asked again later; a read made while more writes wait for the writer than the blocking pool holds threads waits for none of those beyond it"
);

check!(
    FEED_READERS_AGREE = two_feed_readers_polling_during_writes_see_one_log,
    "004 and 008: the feed is the log; two consumers polling from one position while others write see the same events, in the same order, each once"
);

check!(
    BACKED_UNDER_WRITES = concurrent_writes_see_their_own_filled_field_and_one_input_is_called_once,
    "008 and 022, extended: each write answers a position its own filled field is visible from, whoever else is writing, and one input is called once across all the writers"
);

const WRITERS: usize = 8;
const EACH: usize = 25;
/// The threads of the runtime's blocking pool, its default: the suite runs
/// on a runtime it does not size. A pool larger than this would leave the
/// check blind, never red.
const POOL: usize = 512;
/// The writes [`READS_AT_A_POSITION`] sends at once: past the [`POOL`], the
/// [`FIRST`] that land before the read, and four batches' bound. A batch
/// lands whole, and a read may be made beside the one in flight and the
/// next: two, which must stay under half of those beyond the pool.
fn waiting() -> usize {
    POOL + 2 * FIRST + 4 * fixture::limits().commit_window_events.max(1) as usize
}
/// The writes of those that land before the read is made.
const FIRST: usize = 64;

pub(super) async fn items(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    l.declare(type_of("item", &[("n", Number), ("by", Text)]))
        .await?;
    Ok(l)
}

/// What one write answered: the record, the value it wrote, the position.
type Written = (String, i64, Position);

/// One caller: `rounds` passes over its own `each` records, each write
/// with an idempotency key of its own.
fn writer(
    cat: Arc<dyn Catalog>,
    who: Principal,
    w: usize,
    each: usize,
    rounds: usize,
) -> Caller<'static, Result<Vec<Written>, Failure>> {
    Box::pin(async move {
        let mut out = Vec::new();
        for round in 0..rounds {
            for n in 0..each {
                let key = format!("w{w}-r{n}");
                let value = (round * 100 + n) as i64;
                let o = cat
                    .upsert(
                        &who,
                        &TypeName::from("item"),
                        &RecordKey::from(key.as_str()),
                        patch(json!({"n": value, "by": format!("w{w}")})),
                        WriteOpts {
                            idempotency_key: Some(IdempotencyKey::from(format!("{key}-{round}"))),
                            ..WriteOpts::default()
                        },
                    )
                    .await
                    .must(&format!("writer {w} upserting `{key}`"))?;
                out.push((key, value, o.position()));
            }
        }
        Ok(out)
    })
}

/// One caller writing the record every caller writes, `times` times,
/// answering `("shared", <the version it made>, <its position>)` each.
fn sharer(
    cat: Arc<dyn Catalog>,
    who: Principal,
    w: usize,
    times: usize,
) -> Caller<'static, Result<Vec<Written>, Failure>> {
    Box::pin(async move {
        let mut out = Vec::new();
        for _ in 0..times {
            let o = cat
                .upsert(
                    &who,
                    &TypeName::from("item"),
                    &RecordKey::from("shared"),
                    patch(json!({"by": format!("w{w}")})),
                    WriteOpts::default(),
                )
                .await
                .must(&format!("writer {w} upserting `shared`"))?;
            let lakelands_api::Outcome::Applied {
                version: Some(v), ..
            } = o
            else {
                return Err(Failure::new(format!(
                    "an upsert answers `applied` with the record's version; it answered {o:?}"
                )));
            };
            out.push(("shared".to_owned(), v.0 as i64, o.position()));
        }
        Ok(out)
    })
}

fn get_at(key: &str, as_of: Position) -> Get {
    Get {
        key: RecordKey::from(key),
        with_meta: false,
        at: read_at(Some(as_of)),
        params: Default::default(),
    }
}

async fn concurrent_writes_each_land_once_in_one_order_of_the_log(store: Store) -> Held {
    let l = items(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    // Each writer writes its own records, and every writer also writes
    // the one record they share.
    let mut callers: Vec<Caller<'static, Result<Vec<Written>, Failure>>> = (0..WRITERS)
        .map(|w| writer(cat.clone(), own.clone(), w, EACH, 2))
        .collect();
    callers.extend((0..WRITERS).map(|w| sharer(cat.clone(), own.clone(), w, 15)));
    // One caller among them writes a value its type refuses: that write
    // alone is refused, and the rest land as if it had not been made.
    let (rcat, rown) = (cat.clone(), own.clone());
    callers.push(Box::pin(async move {
        let bad = try_upsert(rcat.as_ref(), &rown, "item", "bad", json!({"n": "no"})).await;
        ensure!(
            matches!(refused_on(&bad), Some(("n", Refusal::Constraint(..)))),
            "a write of a text to `n`, a number, among the others answered {bad:?}; it is refused as a constraint on `n`"
        );
        Ok(Vec::new())
    }));
    let mut written = Vec::new();
    for r in join_all(callers).await {
        written.extend(r?);
    }
    let head = store.head().await.must("reading the head")?;
    let (shared, written): (Vec<Written>, Vec<Written>) =
        written.into_iter().partition(|(k, _, _)| k == "shared");
    let mut versions: Vec<i64> = shared.iter().map(|(_, v, _)| *v).collect();
    versions.sort();
    let want: Vec<i64> = (1..=shared.len() as i64).collect();
    ensure!(
        versions == want,
        "{} writes of the shared record must each make a version of their own, 1 to {}; \
         they answered versions {versions:?}",
        shared.len(),
        shared.len()
    );
    let written: Vec<Written> = written.into_iter().chain(shared.iter().cloned()).collect();

    let mut positions: Vec<Position> = written.iter().map(|(_, _, p)| *p).collect();
    positions.sort();
    positions.dedup();
    ensure!(
        positions.len() == written.len() && positions.last().is_some_and(|p| *p <= head),
        "{} writes answered {} distinct positions, the last {:?}, with the head at {head}; \
         each write lands once, at a position of its own the log holds",
        written.len(),
        positions.len(),
        positions.last()
    );
    for (key, value, at) in written.iter().filter(|(k, _, _)| k != "shared") {
        let got = cat
            .get(own, &TypeName::from("item"), get_at(key, *at))
            .await
            .must(&format!("reading `{key}` as of {at}"))?
            .data;
        ensure!(
            got.fields.get("n") == Some(&json!(value)),
            "the write of {value} to `{key}` answered position {at}; read as of it, `{key}` \
             holds {:?}: a write is visible from the position it answers",
            got.fields.get("n")
        );
    }
    for (_, version, at) in written.iter().filter(|(k, _, _)| k == "shared") {
        let got = cat
            .get(
                own,
                &TypeName::from("item"),
                Get {
                    with_meta: true,
                    ..get_at("shared", *at)
                },
            )
            .await
            .must(&format!("reading `shared` as of {at}"))?
            .data;
        ensure!(
            got.version.0 as i64 == *version,
            "a write to `shared` answered version {version} at {at}; read as of it, \
             `shared` is at version {}: a write is visible from the position it answers",
            got.version.0
        );
    }
    let live = all(cat.as_ref(), own, "item").await?;
    let live: Vec<_> = live
        .into_iter()
        .filter(|r| r.key.as_str() != "shared")
        .collect();
    ensure!(
        live.len() == WRITERS * EACH
            && live.iter().all(|r| {
                let n: i64 = r
                    .key
                    .as_str()
                    .rsplit('r')
                    .next()
                    .unwrap_or("")
                    .parse()
                    .unwrap_or(-1);
                r.fields.get("n") == Some(&json!(100 + n))
            }),
        "{WRITERS} writers each wrote {EACH} records twice; the lake must hold {} records, \
         each with its second value, and holds {} records",
        WRITERS * EACH,
        live.len()
    );
    let log = log_events(store.as_ref()).await?;
    ensure!(
        log.iter()
            .enumerate()
            .all(|(i, e)| e.position == Position(i as u64))
            && log.last().map(|e| e.position) == Some(head),
        "the log's positions follow one another from 0 to the head {head} without a gap; \
         it holds {} events, the last at {:?}",
        log.len(),
        log.last().map(|e| e.position)
    );
    // Each answer names the write's own position: in the one read of the
    // log above, the event there is that write, not a later one (015).
    for (key, _, at) in &written {
        let there = log.get(at.0 as usize);
        let own_event = there.is_some_and(|e| {
            e.position == *at
                && matches!(&e.body, EventBody::RecordUpserted { key: k, .. } if k.as_str() == key)
        });
        ensure!(
            own_event,
            "the write to `{key}` answered position {at}; the event there is {:?}: a write \
             answers its own position",
            there.map(|e| (e.position, e.body.kind()))
        );
    }
    ensure!(
        !log.iter().any(
            |e| matches!(&e.body, EventBody::RecordUpserted { key, .. } if key.as_str() == "bad")
        ),
        "the refused write of `bad` landed in the log"
    );
    let fut = store.verify(None, None);
    fut.await
        .must("verifying the hash chain after the concurrent writes")?;
    Ok(())
}

async fn writes_and_a_declaration_made_at_once_are_ordered_as_one_log(store: Store) -> Held {
    let l = items(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let mut callers: Vec<Caller<'static, Result<Vec<Written>, Failure>>> = (0..4)
        .map(|w| writer(cat.clone(), own.clone(), w, 20, 1))
        .collect();
    let (dcat, downer) = (cat.clone(), own.clone());
    callers.insert(
        2,
        Box::pin(async move {
            let o = dcat
                .add_field(
                    &downer,
                    &TypeName::from("item"),
                    field("note", StorageKind::Text),
                )
                .await
                .must("adding `note` to `item` while others write")?;
            let extra: Declaration =
                Declaration::Type(type_decl("extra", vec![field("x", StorageKind::Text)]));
            let fut = dcat.declare(&downer, extra);
            fut.await.must("declaring `extra` while others write")?;
            Ok(vec![("note".to_owned(), 0, o.position())])
        }),
    );
    let mut results = join_all(callers).await.into_iter();
    let mut written = Vec::new();
    let mut declared = None;
    for (i, r) in results.by_ref().enumerate() {
        let r = r?;
        if i == 2 {
            declared = r.first().map(|(_, _, p)| *p);
        } else {
            written.extend(r);
        }
    }
    let declared = declared.ok_or_else(|| Failure::new("the declaration answered nothing"))?;
    for (key, _, at) in &written {
        let d = cat
            .describe_type(own, &TypeName::from("item"), read_at(Some(*at)))
            .await
            .must(&format!("describing `item` as of {at}"))?
            .data;
        let has = d.fields.iter().any(|f| f.name.as_str() == "note");
        ensure!(
            has == (*at > declared),
            "`note` was added at {declared}; the write to `{key}` landed at {at}, and \
             `item` as of it {} `note`: a write after the declaration sees the field, one \
             before it does not",
            if has { "has" } else { "lacks" }
        );
    }
    Ok(())
}

/// What one read at a position answered: the head it read at, the keys
/// `find` answered there, and whether `get` found `w0-r0` there.
type Seen = (Position, Vec<String>, Option<Value>);

async fn seen_at(
    cat: &dyn Catalog,
    who: &Principal,
    at: Position,
) -> Result<(Vec<String>, Option<Value>), Failure> {
    let found = cat
        .find(
            who,
            &TypeName::from("item"),
            Find {
                limit: Some(1000),
                at: read_at(Some(at)),
                ..Find::default()
            },
        )
        .await
        .must(&format!("finding `item` as of {at}"))?
        .data;
    let lakelands_api::Found::Records { rows, .. } = found else {
        return Err(Failure::new("`find` without an aggregate answered groups"));
    };
    let mut keys: Vec<String> = rows.into_iter().map(|r| r.key.0).collect();
    keys.sort();
    let one = match cat
        .get(who, &TypeName::from("item"), get_at("w0-r0", at))
        .await
    {
        Ok(r) => r.data.fields.get("n").cloned(),
        Err(lakelands_api::Error::NotFound { .. }) => None,
        Err(e) => {
            return Err(Failure::new(format!(
                "getting `w0-r0` as of {at} failed: {e} ({e:?})"
            )));
        }
    };
    Ok((keys, one))
}

async fn a_read_at_a_position_answers_the_same_during_writes_and_after(store: Store) -> Held {
    let l = items(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let done = Arc::new(AtomicBool::new(false));
    let mut writers: Vec<Caller<'static, Result<Vec<Written>, Failure>>> = (0..4)
        .map(|w| writer(cat.clone(), own.clone(), w, 20, 2))
        .collect();
    let flag = done.clone();
    let all_writers = Box::pin(async move {
        let results = join_all(std::mem::take(&mut writers)).await;
        // Set on every way out, so a failing writer fails the check
        // instead of leaving the reader polling.
        flag.store(true, Ordering::SeqCst);
        let mut out = Vec::new();
        for r in results {
            out.extend(r?);
        }
        Ok::<_, Failure>(out)
    });
    let (rstore, rcat, rown, rdone) = (store.clone(), cat.clone(), own.clone(), done.clone());
    let reader = Box::pin(async move {
        let mut seen: Vec<Seen> = Vec::new();
        while !rdone.load(Ordering::SeqCst) {
            // The head, as a host's own reader or an unpinned turn takes it:
            // a position `head()` answers is one the lake can read at.
            let at = rstore.head().await.must("reading the head")?;
            let (keys, one) = seen_at(rcat.as_ref(), &rown, at).await?;
            ensure!(
                one.is_some() == keys.iter().any(|k| k == "w0-r0"),
                "as of {at}, `find` answered {} records {} `w0-r0`, and `get` of it answered \
                 {one:?}: two reads at one position agree",
                keys.len(),
                if keys.iter().any(|k| k == "w0-r0") {
                    "with"
                } else {
                    "without"
                }
            );
            seen.push((at, keys, one));
        }
        Ok::<_, Failure>(seen)
    });
    let mut both: Vec<Caller<'static, Result<Vec<Seen>, Failure>>> = Vec::new();
    both.push(Box::pin(
        async move { all_writers.await.map(|_| Vec::new()) },
    ));
    both.push(reader);
    let mut answers = join_all(both).await.into_iter();
    answers.next().expect("the writers answered")?;
    let seen = answers.next().expect("the reader answered")?;
    ensure!(
        seen.len() > 1,
        "the reader made {} reads while the writers ran; the check needs reads during \
         the writes",
        seen.len()
    );
    for (at, keys, one) in seen {
        let (again, one_again) = seen_at(cat.as_ref(), own, at).await?;
        ensure!(
            again == keys && one_again == one,
            "as of {at}, during the writes, `find` answered {} records and `get` of \
             `w0-r0` {one:?}; asked again after them, {} records and {one_again:?}: a \
             read at a position answers the same whenever it is made",
            keys.len(),
            again.len()
        );
    }
    // More writes sent at once than the blocking pool holds threads. A read
    // made once the first few landed waits for none of those queued beyond
    // the pool: fewer than half of them land meanwhile. A reader woken too
    // late to tell is sent again.
    for _ in 0..3 {
        let (before, during) = read_beside_waiting(cat, own).await?;
        let beyond = (waiting() - before).saturating_sub(POOL);
        if beyond >= FIRST {
            ensure!(
                during < beyond / 2,
                "{during} of {} writes sent at once landed while a read was made after \
                 the first {before}: a read waits for none of the {beyond} queued beyond the \
                 blocking pool",
                waiting()
            );
            return Ok(());
        }
    }
    Err(Failure::new(format!(
        "the reader of {} writes sent at once woke after too many had landed, \
         three times; the check could not tell",
        waiting()
    )))
}

/// Sends [`waiting`] writes at once and, once [`FIRST`] of them landed,
/// one read: how many had landed as it was made, and how many during it.
async fn read_beside_waiting(
    cat: &Arc<dyn Catalog>,
    own: &Principal,
) -> Result<(usize, usize), Failure> {
    let landed = Arc::new(AtomicUsize::new(0));
    let mut callers: Vec<Caller<'static, Result<(usize, usize), Failure>>> = (0..waiting())
        .map(|w| {
            let (write, landed) = (
                writer(cat.clone(), own.clone(), 100 + w, 1, 1),
                landed.clone(),
            );
            Box::pin(async move {
                write
                    .await
                    .map(|_| (landed.fetch_add(1, Ordering::SeqCst), 0_usize))
            }) as Caller<'static, _>
        })
        .collect();
    let (rcat, rown) = (cat.clone(), own.clone());
    callers.push(Box::pin(async move {
        while landed.load(Ordering::SeqCst) < FIRST {
            fixture::pause(std::time::Duration::from_millis(1)).await;
        }
        let before = landed.load(Ordering::SeqCst);
        let (ty, get) = ("item".into(), fixture::get_at("w0-r0", None));
        (rcat.get(&rown, &ty, get).await).must("getting `w0-r0` while the writes wait")?;
        Ok((before, landed.load(Ordering::SeqCst) - before))
    }));
    let mut answers = join_all(callers).await;
    let read = answers.pop().expect("the reader answered")?;
    answers.into_iter().collect::<Result<Vec<_>, _>>()?;
    Ok(read)
}

async fn two_feed_readers_polling_during_writes_see_one_log(store: Store) -> Held {
    let l = items(store.as_ref()).await?;
    let Lake { cat, own } = &l;
    let from = store.head().await.must("reading the head")?.next();
    let done = Arc::new(AtomicBool::new(false));
    let mut writers: Vec<Caller<'static, Result<Vec<Written>, Failure>>> = (0..4)
        .map(|w| writer(cat.clone(), own.clone(), w, 20, 1))
        .collect();
    let flag = done.clone();
    let mut callers: Vec<Caller<'static, Result<Vec<Position>, Failure>>> = Vec::new();
    callers.push(Box::pin(async move {
        let results = join_all(std::mem::take(&mut writers)).await;
        // Set on every way out, so a failing writer fails the check
        // instead of leaving the readers polling.
        flag.store(true, Ordering::SeqCst);
        for r in results {
            r?;
        }
        Ok(Vec::new())
    }));
    for page in [3u32, 5] {
        let (s, d) = (store.clone(), done.clone());
        callers.push(Box::pin(async move {
            let feed = s.feed();
            let mut next = from;
            let mut seen = Vec::new();
            loop {
                let finished = d.load(Ordering::SeqCst);
                let batch = feed
                    .poll(next, &FeedFilter::default(), page)
                    .await
                    .must("polling the feed during the writes")?;
                seen.extend(batch.events.iter().map(|e| e.position));
                next = batch.next;
                if finished && batch.events.is_empty() && next > batch.head {
                    return Ok(seen);
                }
            }
        }));
    }
    let mut answers = join_all(callers).await.into_iter();
    answers.next().expect("the writers answered")?;
    let one = answers.next().expect("the first reader answered")?;
    let two = answers.next().expect("the second reader answered")?;
    let head = store.head().await.must("reading the head")?;
    let expected: Vec<Position> = (from.0..=head.0).map(Position).collect();
    ensure!(
        one == expected && two == expected,
        "two readers polled from {from} in pages of 3 and 5 while four writers wrote; each \
         must see every event from {from} to the head {head} once, in order: they saw {} \
         and {} events",
        one.len(),
        two.len()
    );
    Ok(())
}

async fn concurrent_writes_see_their_own_filled_field_and_one_input_is_called_once(
    store: Store,
) -> Held {
    let token = "backed_under_writes";
    let l =
        super::backings::places(store.as_ref(), json!({"token": token, "echo": "code"})).await?;
    let Lake { cat, own } = &l;
    let callers: Vec<Caller<'static, Result<(String, Position), Failure>>> = (0..WRITERS)
        .map(|w| {
            let (cat, own) = (cat.clone(), own.clone());
            let caller: Caller<'static, Result<(String, Position), Failure>> =
                Box::pin(async move {
                    let key = format!("p{w}");
                    let o = cat
                        .upsert(
                            &own,
                            &TypeName::from("place"),
                            &RecordKey::from(key.as_str()),
                            patch(json!({"code": "AT"})),
                            WriteOpts::default(),
                        )
                        .await
                        .must(&format!("writer {w} upserting `{key}`"))?;
                    Ok((key, o.position()))
                });
            caller
        })
        .collect();
    let mut written = Vec::new();
    for r in join_all(callers).await {
        written.push(r?);
    }
    for (key, at) in &written {
        let got = cat
            .get(own, &TypeName::from("place"), get_at(key, *at))
            .await
            .must(&format!("reading `{key}` as of {at}"))?
            .data;
        ensure!(
            got.fields.get("name") == Some(&json!("AT")),
            "the write to `{key}` answered {at}; as of it, `{key}`'s filled `name` reads \
             {:?}: a write answers a position its own filled field is visible from",
            got.fields.get("name")
        );
    }
    let calls = made(token);
    ensure!(
        calls == 1,
        "{WRITERS} writers wrote the same input, code AT, at once; the backing must be \
         called once for it, not {calls} times"
    );
    Ok(())
}

check!(
    AGE_UNDER_WRITES = writes_made_while_a_rule_runs_land_before_or_after_its_append,
    "036 and seams, SystemVocabulary: `age` holds the writer's lock for a rule's whole run, so a write made meanwhile lands before or after the rule's append, never inside it, and every move is made from the record as it stood before that append"
);

async fn writes_made_while_a_rule_runs_land_before_or_after_its_append(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_of("task", &[("state", Text), ("note", Text)]))
        .await?;
    let keys: Vec<String> = (0..200).map(|n| format!("t{n}")).collect();
    for k in &keys {
        l.upsert("task", k, json!({"state": "open"})).await?;
    }
    l.upsert(
        lakelands_api::seam::AGING_RULE_TYPE,
        "go_stale",
        json!({"on": "task", "field": "state", "from": "open", "to": "stale", "when": "true"}),
    )
    .await?;

    // Four writers touch every task's `note` while one caller ages.
    let mut callers: Vec<Caller<'static, Held>> = (0..4)
        .map(|w| {
            let (cat, own, keys) = (cat.clone(), own.clone(), keys.clone());
            let caller: Caller<'static, Held> = Box::pin(async move {
                for k in &keys {
                    crate::conformance::fixture::upsert(
                        cat.as_ref(),
                        &own,
                        "task",
                        k,
                        json!({"note": format!("w{w}")}),
                    )
                    .await?;
                }
                Ok(())
            });
            caller
        })
        .collect();
    let ager = store.clone();
    callers.insert(
        0,
        Box::pin(async move {
            let today = Timestamp::from_date(2026, 3, 1).expect("a date");
            ager.age(today).await.must("aging while others write")?;
            Ok(())
        }),
    );
    for r in join_all(callers).await {
        r?;
    }

    let log = log_events(store.as_ref()).await?;
    let moves: Vec<_> = log
        .iter()
        .filter(|e| {
            e.procedure
                .as_ref()
                .is_some_and(|p| p.name == "aging:go_stale")
        })
        .collect();
    ensure!(
        moves.len() == keys.len(),
        "the rule moves each of the {} tasks once; the log holds {} moves",
        keys.len(),
        moves.len()
    );
    let (first, last) = (moves[0].position, moves[moves.len() - 1].position);
    ensure!(
        last.0 - first.0 + 1 == moves.len() as u64,
        "the rule's moves are one append: positions {first} to {last} hold {} moves, and \
         a write landed among them",
        moves.len()
    );
    for m in &moves {
        let EventBody::RecordUpserted { key, version, .. } = &m.body else {
            return Err(Failure::new(format!(
                "a move is a record_upserted; the event at {} is {}",
                m.position,
                m.body.kind()
            )));
        };
        let before = cat
            .get(
                own,
                &TypeName::from("task"),
                get_at(key.as_str(), Position(m.position.0 - 1)),
            )
            .await
            .must(&format!("reading `{key}` just before its move"))?
            .data;
        ensure!(
            version.0 == before.version.0 + 1,
            "`{key}` was at version {} just before the rule's append and the move made \
             version {}: the move is made from the record as it stood before the append",
            before.version.0,
            version.0
        );
    }
    Ok(())
}
