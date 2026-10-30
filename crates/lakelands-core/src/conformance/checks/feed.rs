//! The change feed is the log, and what a write makes due is in the same
//! append (`docs/decisions/004-the-change-feed-is-the-log.md`,
//! `docs/decisions/001-the-log-is-the-truth.md`).

use lakelands_api::SetOff;
use lakelands_api::ontology::StorageKind::{Number, Text};

use crate::conformance::fixture::{source_record, type_of, upsert};
use crate::conformance::prelude::*;

/// What a write's answer says it set off.
fn set_off(o: &Outcome) -> SetOff {
    match o {
        Outcome::Applied { set_off, .. } => set_off.clone(),
        Outcome::Proposed { .. } => SetOff::default(),
    }
}

/// The positions of the triggers fired in `events`.
fn fired_at(events: &[Event]) -> Vec<Position> {
    let fired = |e: &&Event| matches!(e.body, EventBody::TriggerFired { .. });
    events.iter().filter(fired).map(|e| e.position).collect()
}

check!(
    BY_KEY = a_feed_filtered_by_key_answers_that_records_events_only,
    "004 and lakelands#146: a feed filtered by types and keys answers exactly the events about those records, in log order, each once: its upserts, a trigger it fired, its object-set changes, a restrict, an unrestrict and its retire; with types empty a key matches in any type and verb_called passes too; kinds narrow it further; no event that names no record passes; and every filtered poll answers what the log does (lakelands#678)"
);

check!(
    BY_OLD_KEY = a_key_filter_does_not_follow_a_rekey,
    "004, 027 and lakelands#146: a feed filtered by a record's old key answers its events up to the migration that re-keyed it and none after; the new key answers what came after; and every filtered poll answers what the log does (lakelands#678)"
);

check!(
    FEED_IS_THE_LOG = the_feed_answers_the_log_from_a_position_by_kind_and_type,
    "004: `poll` answers the events from a position in log order, filtered by kind and type, with the position to poll from next and the head"
);

check!(
    REBUILD_LEAVES_THE_FEED = rebuilding_the_projection_changes_nothing_in_the_feed,
    "001 and 004: the log is the truth and the projection an index; deleting and rebuilding the projection leaves every event as it was, and every filtered poll answers what the log does (lakelands#678)"
);

check!(
    TRIGGER_FIRES = a_trigger_fires_on_every_write_whose_guard_holds,
    "004 and 015, extended: when a write makes a trigger's guard hold, `trigger_fired` is appended with it, and the write's answer names the trigger, `through` its position; a write under which the guard does not hold fires nothing and answers nothing more; a retry answers the same, and an acceptance what the write it applied set off"
);

check!(
    OBJECT_SET_CHANGES = an_object_set_change_is_written_only_when_membership_changes,
    "004 and 015, extended: a write that moves a record into or out of an object set appends `object_set_changed`, and its answer names the set as joined or left; one that leaves membership as it was appends none and names none"
);

check!(
    HIDDEN_TRIGGER = a_trigger_whose_guard_the_writer_may_not_read_is_not_named,
    "015, extended, and 024: a write's answer names a trigger it fired, or an object set its record joined or left, only if `describe` lists it to the writer; one whose condition reads a field the writer may not read is left out, and `through` still covers it"
);

check!(
    INDEX_UNMASKED = a_filtered_poll_answers_the_log_unmasked,
    "004, 051 and lakelands#678: the feed is the log unmasked and its index masks nothing: a poll filtered by a type, by the key of a restricted record, by that of a record an `only` grant would not admit and by a retired one's answers exactly what reading the whole log with no filter and keeping what the filter admits answers, whole, and moves `next` past the head as that read does; a reader who does not read everything is refused the feed at a door"
);

async fn a_filtered_poll_answers_the_log_unmasked(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    l.declare(type_of("doc", &[("owner", Text), ("text", Text)]))
        .await?;
    for (key, owner) in [("d1", "a"), ("d2", "b"), ("d3", "a")] {
        l.upsert("doc", key, json!({"owner": owner, "text": "first"}))
            .await?;
    }
    l.upsert("doc", "d2", json!({"text": "second"})).await?;
    store
        .restrict(&"doc".into(), &"d2".into(), "a hold".into())
        .await
        .must("restricting d2")?;
    l.retire("doc", "d3").await?;
    let held = polled(store.as_ref(), filter(&[], &["doc"], &["d2"])).await?;
    let kinds: Vec<&str> = held.iter().map(|e| e.body.kind()).collect();
    ensure!(
        kinds == ["record_upserted", "record_upserted", "record_restricted"],
        "the restricted record d2, which an `only` grant on owner `a` would not admit either, \
         answers its whole history to a filter by its key; it answered {kinds:?}"
    );
    the_index_answers_the_log(store.as_ref(), "a restricted and a retired record").await
}

check!(
    WAIT_ENDS_AT_THE_NEXT_APPEND = a_wait_ends_at_the_next_append,
    "004, extended (lakelands#1061): a wait on the feed past the head ends at the next append, while another caller writes, and answers the head a poll then reads up to, never past it: a poll from where the wait began answers that append; a wait past a head that nothing moves ends once its bound has passed and answers that head; a wait behind the head answers at once"
);

async fn a_wait_ends_at_the_next_append(store: Store) -> Held {
    use std::time::{Duration, Instant};
    let (long, short) = (Duration::from_secs(30), Duration::from_millis(300));
    let l = invoices(store.as_ref()).await?;
    let feed = store.feed();
    let head = store.head().await.must("reading the head")?;
    let t = Instant::now();
    let behind = feed.wait(Position(head.0 - 1), long).await;
    let (behind, at_once) = (behind.must("waiting behind the head")?, t.elapsed());
    let t = Instant::now();
    let idle = feed.wait(head, short).await;
    let (idle, idled) = (idle.must("waiting with nothing written")?, t.elapsed());
    ensure!(
        behind == head && at_once < long / 3 && idle == head && idled >= short && idled < long / 3,
        "a wait behind the head {head} answers it at once: {behind} after {at_once:?}; one past \
         it with nothing written answers it once its bound has passed: {idle} after {idled:?}"
    );
    let t = Instant::now();
    let waiter: Caller<'_, Result<(Position, Duration), Failure>> = Box::pin(async {
        let woke = feed.wait(head, long).await.must("waiting past the head")?;
        Ok((woke, t.elapsed()))
    });
    let writer: Caller<'_, Result<(Position, Duration), Failure>> = Box::pin(async {
        pause(Duration::from_millis(100)).await;
        let written = l.upsert("invoice", "i1", json!({"status": "open"})).await?;
        Ok((written.position(), t.elapsed()))
    });
    let [woke, written]: [_; 2] = join_all(vec![waiter, writer])
        .await
        .try_into()
        .expect("two callers");
    let ((woke, after), (written, at)) = (woke?, written?);
    let batch = feed.poll(head.next(), &FeedFilter::default(), 10).await;
    let batch = batch.must("polling from where the wait began")?;
    let polled: Vec<_> = batch.events.iter().map(|e| e.position).collect();
    ensure!(
        woke == written
            && after >= Duration::from_millis(100)
            && after < long / 3
            && polled == [written]
            && batch.head == woke,
        "a wait past {head} ends at the append at {written}, written after a pause of 100 ms \
         and answered after {at:?}: it answered {woke} after {after:?}, and a poll from {} answers {polled:?} with its head at {}",
        head.next(),
        batch.head
    );
    Ok(())
}

async fn invoices(store: &dyn LakeStore) -> Result<Lake, Failure> {
    let l = Lake::of(store);
    l.declare(type_of("invoice", &[("status", Text), ("note", Text)]))
        .await?;
    l.declare(type_of("memo", &[("text", Text)])).await?;
    Ok(l)
}

async fn all_events(store: &dyn LakeStore, from: Position) -> Result<Vec<Event>, Failure> {
    followed(store, from, &FeedFilter::default()).await
}

check!(
    MADE = a_record_event_says_whether_it_made_the_record,
    "004 and lakelands#572: `record_upserted` and `record_ingested` carry `made` where the write found no live record under the key and leave it out otherwise, so a consumer of the feed tells a record made from one changed without a read: a first creation with `version` 1, a record made again after its retirement with the version its history gives it, a change and a verb's write without; it is said where the event is made, of what the writer then holds: of two records of one ingest that name one key the first made it, and a proposal is judged when it is accepted, not when it was proposed"
);

check!(
    PAGED_FROM_NEXT = polling_from_next_in_small_pages_sees_every_matching_event_once_in_order,
    "004: a consumer that polls from the `next` each answer gives, in pages of one or two events and with writes landing between polls, sees every matching event once, in log order; this is the position a deliverer keeps"
);

check!(
    APPEND_ENDS = a_consumer_groups_the_firings_of_two_ingests_into_their_two_appends,
    "083, extended (lakelands#1540): `poll` names the last position of each append that wrote one of its events, read from the stored form, and `append_of` an event's; with the clock behind the log, so that every append reads one `known_at`, and two ingests under one delivery name, `[A1, A2]` then `[B1, B2]`, a consumer of `trigger_fired` alone groups the firings into exactly the two appends that wrote them, each ending at its ingest's window, alike in one poll and a firing a poll",
    given
);

check!(
    APPEND_DELIVERIES = one_append_carrying_two_deliveries_is_one_append,
    "083 and 087, extended (lakelands#1540): one ingest whose records name two deliveries, in one window, is one append: `poll` names one end for all its events, the window's position, alike in one poll and an event a poll"
);

check!(
    LIMIT_ZERO_IS_ONE = a_poll_with_a_limit_of_zero_answers_one_event,
    "seams, ChangeFeed: a limit of 0 is read as 1, so a consumer that passes 0 still moves forward instead of polling the same position for ever"
);

async fn a_record_event_says_whether_it_made_the_record(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let mut bill = type_of("bill", &[("amount", Number)]);
    bill.fields.push(sourced("paid", Number));
    l.declare(bill).await?;
    let zero = write_verb("zero", "bill", "amount", "0");
    l.declare_as(zero, "the verb `zero`").await?;
    let n = |n: u32| json!({ "amount": n });
    let propose = |key: &str| {
        let (ty, key, opts) = ("bill".into(), key.into(), WriteOpts::default());
        let patch = patch(n(5));
        cat.propose(
            own,
            ProposedWrite::Upsert {
                ty,
                key,
                patch,
                opts,
            },
            "later".into(),
        )
    };
    let accept = async |o: Outcome| match o {
        Outcome::Proposed { proposal, .. } => cat.accept(own, &proposal).await.must("accepting"),
        o => Err(Failure::new(format!("an upsert proposed answered {o:?}"))),
    };
    let from = store.head().await.must("reading the head")?.next();
    // `9` is made, changed, changed by a verb, retired and, by a proposal
    // made while it was live, made again; `8` is proposed before it is
    // there and accepted after; `7` comes twice in one ingest.
    l.upsert("bill", "9", n(1)).await?;
    l.upsert("bill", "9", n(2)).await?;
    let (verb, key) = ("zero".into(), "9".into());
    let called = cat.call(
        own,
        &verb,
        Some(&key),
        patch(json!({})),
        WriteOpts::default(),
    );
    called.await.must("calling `zero`")?;
    let live = propose("9").await.must("proposing over a live record")?;
    let absent = propose("8").await.must("proposing a record not there")?;
    l.retire("bill", "9").await?;
    l.upsert("bill", "8", n(1)).await?;
    accept(live).await?;
    accept(absent).await?;
    let twice = [1, 2].map(|p| source_record("bill", "7", json!({ "paid": p })));
    l.ingest("meter", twice.into()).await.must("ingesting")?;
    let said: Vec<String> = (all_events(store.as_ref(), from).await?.iter())
        .filter_map(|e| match &e.body {
            EventBody::RecordUpserted {
                key, version, made, ..
            }
            | EventBody::RecordIngested {
                key, version, made, ..
            } => Some(format!(
                "{key}:{}{}",
                version.0,
                if *made { " made" } else { "" }
            )),
            _ => None,
        })
        .collect();
    let want = [
        "9:1 made", "9:2", "9:3", "8:1 made", "9:5 made", "8:2", "7:1 made", "7:2",
    ];
    ensure!(
        said == want,
        "the record events, each as key, version and whether it says it made the record, are \
         {want:?}; the feed answers {said:?}"
    );
    Ok(())
}

async fn a_poll_with_a_limit_of_zero_answers_one_event(store: Store) -> Held {
    let l = invoices(store.as_ref()).await?;
    let from = store.head().await.must("reading the head")?.next();
    let first = l.upsert("invoice", "a", json!({"status": "open"}));
    let first = first.await?.position();
    l.upsert("invoice", "b", json!({"status": "open"})).await?;
    let batch = store
        .feed()
        .poll(from, &FeedFilter::default(), 0)
        .await
        .must("polling the feed with a limit of 0")?;
    let got: Vec<Position> = batch.events.iter().map(|e| e.position).collect();
    ensure!(
        got == [first] && batch.next == first.next(),
        "two invoices were written from {from}, the first at {first}; a poll from {from} \
         with a limit of 0 answered the events at {got:?} and next {}, where a limit of 0 \
         is read as 1: the event at {first}, and next {}",
        batch.next,
        first.next()
    );
    Ok(())
}

async fn the_feed_answers_the_log_from_a_position_by_kind_and_type(store: Store) -> Held {
    let l = invoices(store.as_ref()).await?;
    let from = store.head().await.must("reading the head")?.next();
    let a = l.upsert("invoice", "a", json!({"status": "open"}));
    let a = a.await?.position();
    let m = l.upsert("memo", "m", json!({"text": "hi"}));
    let m = m.await?.position();
    let b = l.upsert("invoice", "b", json!({"status": "open"}));
    let b = b.await?.position();
    let feed = store.feed();

    let everything = feed
        .poll(from, &FeedFilter::default(), 100)
        .await
        .must("polling the feed from before the writes")?;
    let positions: Vec<Position> = everything.events.iter().map(|e| e.position).collect();
    ensure!(
        positions == [a, m, b] && everything.next == b.next() && everything.head == b,
        "polling from {from} must answer the three writes at {a}, {m}, {b}, next {} and \
         head {b}; it answered {positions:?}, next {}, head {}",
        b.next(),
        everything.next,
        everything.head
    );

    let invoices_only = filter(&["record_upserted"], &["invoice"], &[]);
    let first = feed.poll(from, &invoices_only, 1);
    let first = first.await.must("polling for one upsert of `invoice`")?;
    let firsts: Vec<Position> = first.events.iter().map(|e| e.position).collect();
    ensure!(
        firsts == [a] && first.next == a.next(),
        "polling for one `invoice` upsert must answer {a} and next {}; it answered \
         {firsts:?}, next {}",
        a.next(),
        first.next
    );
    let rest = feed
        .poll(first.next, &invoices_only, 10)
        .await
        .must("polling on from where the last poll stopped")?;
    let rests: Vec<Position> = rest.events.iter().map(|e| e.position).collect();
    ensure!(
        rests == [b] && rest.next == b.next(),
        "polling on from {} must skip the memo and answer {b}, next {}; it answered \
         {rests:?}, next {}",
        first.next,
        b.next(),
        rest.next
    );
    let after = feed
        .poll(b.next(), &FeedFilter::default(), 10)
        .await
        .must("polling past the head")?;
    ensure!(
        after.events.is_empty(),
        "polling from past the head must answer nothing yet; it answered {} event(s)",
        after.events.len()
    );
    Ok(())
}

async fn rebuilding_the_projection_changes_nothing_in_the_feed(store: Store) -> Held {
    let l = invoices(store.as_ref()).await?;
    for (k, s) in [("a", "open"), ("b", "paid"), ("a", "held")] {
        l.upsert("invoice", k, json!({ "status": s })).await?;
    }
    let before = all_events(store.as_ref(), Position::GENESIS).await?;
    let head = store.head().await.must("reading the head")?;
    let through = store.rebuild().await.must("rebuilding the projection")?;
    let after = all_events(store.as_ref(), Position::GENESIS).await?;
    ensure!(
        through == head,
        "the rebuild answered position {through}, but the log's head is {head}"
    );
    ensure!(
        before == after,
        "the feed must read the same before and after a rebuild: it held {} events \
         before and {} after, or they differ",
        before.len(),
        after.len()
    );
    the_index_answers_the_log(store.as_ref(), "a rebuilt projection").await
}

async fn a_trigger_fires_on_every_write_whose_guard_holds(store: Store) -> Held {
    let l = invoices(store.as_ref()).await?;
    l.declare_json(json!({
        "declare": "trigger", "name": "paid", "on": "invoice", "when": "status = 'paid'"
    }))
    .await?;

    for (status, note, fires) in [
        ("open", "one", false),
        ("paid", "two", true),
        ("paid", "three", true),
        ("held", "four", false),
    ] {
        let from = store.head().await.must("reading the head")?.next();
        let answer = l.upsert("invoice", "i", json!({"status": status, "note": note}));
        let answer = answer.await?;
        let appended = all_events(store.as_ref(), from).await?;
        let at = fired_at(&appended);
        ensure!(
            at.len() == usize::from(fires),
            "writing status {status:?} must {}fire `paid` (guard `status = 'paid'`) in \
             its own append; the append held {:?}",
            if fires { "" } else { "not " },
            appended.iter().map(|e| e.body.kind()).collect::<Vec<_>>()
        );
        let want = SetOff {
            through: at.first().copied(),
            fired: at.iter().map(|_| "paid".into()).collect(),
            ..SetOff::default()
        };
        ensure!(
            set_off(&answer) == want,
            "the answer to writing status {status:?} must say it set off {want:?}; it \
             answered {answer:?}"
        );
    }
    // A retry answers what the first write set off; an acceptance, what the
    // write it applied set off.
    let Lake { cat, own } = &l;
    let (ty, j, paid) = (
        "invoice".into(),
        "j".into(),
        patch(json!({"status": "paid"})),
    );
    let once = WriteOpts {
        idempotency_key: Some("pay-j".into()),
        ..WriteOpts::default()
    };
    let first = cat.upsert(own, &ty, &j, paid.clone(), once.clone());
    let first = first.await.must("paying `j`")?;
    let again = cat.upsert(own, &ty, &j, paid.clone(), once).await;
    let again = again.must("paying `j` again under the same key")?;
    ensure!(
        again == first && set_off(&first).fired.len() == 1,
        "a retry answers the first write's outcome and what it set off, {first:?}; it \
         answered {again:?}"
    );
    let write = ProposedWrite::Upsert {
        ty,
        key: "k".into(),
        patch: paid,
        opts: WriteOpts::default(),
    };
    let proposed = cat.propose(own, write, "later".into()).await;
    let Outcome::Proposed { proposal, .. } = proposed.must("proposing to pay `k`")? else {
        return Err(Failure::new("`propose` answers a proposal"));
    };
    let accepted = cat.accept(own, &proposal).await.must("accepting it")?;
    ensure!(
        set_off(&accepted).fired == ["paid"],
        "accepting the payment of `k` answers that it fired `paid`; it answered {accepted:?}"
    );
    Ok(())
}

async fn a_trigger_whose_guard_the_writer_may_not_read_is_not_named(store: Store) -> Held {
    let l = invoices(store.as_ref()).await?;
    for (name, when) in [("noted", None), ("paid", Some("status = 'paid'"))] {
        let mut t = json!({"declare": "trigger", "name": name, "on": "invoice"});
        if let Some(w) = when {
            t["when"] = json!(w);
        }
        l.declare_json(t).await?;
    }
    for (name, filter) in [
        ("seen", "note = 'seen'"),
        ("seen_paid", "note = 'seen' and status = 'paid'"),
    ] {
        let set = json!({"declare": "object_set", "name": name, "of": "invoice", "where": filter});
        l.declare_json(set).await?;
    }
    l.upsert("invoice", "i", json!({"status": "paid"})).await?;
    // The clerk writes `note` and may not read `status`, which `paid`'s
    // guard and `seen_paid`'s filter read: both triggers fire and the
    // record joins both sets, and the answer names `noted` and `seen` only.
    let fields = [("note", Access::Write), ("status", Access::None)];
    let clerk = field_access(store.as_ref(), "clerk", "invoice", &fields);
    let from = store.head().await.must("reading the head")?.next();
    let note = json!({"note": "seen"});
    let answer = upsert(l.cat.as_ref(), &clerk, "invoice", "i", note).await?;
    let appended = all_events(store.as_ref(), from).await?;
    let at = fired_at(&appended);
    let want = SetOff {
        through: appended.last().map(|e| e.position),
        fired: vec!["noted".into()],
        joined: vec!["seen".into()],
        ..SetOff::default()
    };
    ensure!(
        at.len() == 2 && set_off(&answer) == want,
        "the clerk's write fired `noted` and `paid` at {at:?} and joined `seen` and \
         `seen_paid`; its answer names `noted` and `seen` only, `through` covering all \
         four: {want:?}; it answered {answer:?}"
    );
    Ok(())
}

async fn an_object_set_change_is_written_only_when_membership_changes(store: Store) -> Held {
    let l = invoices(store.as_ref()).await?;
    l.declare_json(json!({
        "declare": "object_set", "name": "open_ones", "of": "invoice", "where": "status = 'open'"
    }))
    .await?;

    for (fields, want) in [
        (json!({"status": "open"}), Some(true)),
        (json!({"note": "still open"}), None),
        (json!({"status": "paid"}), Some(false)),
        (json!({"note": "still paid"}), None),
    ] {
        let from = store.head().await.must("reading the head")?.next();
        let answer = l.upsert("invoice", "i", fields.clone()).await?;
        let appended = all_events(store.as_ref(), from).await?;
        let changes: Vec<bool> = appended
            .iter()
            .filter_map(|e| match &e.body {
                EventBody::ObjectSetChanged { set, joined, .. } if set == "open_ones" => {
                    Some(*joined)
                }
                _ => None,
            })
            .collect();
        ensure!(
            changes == want.into_iter().collect::<Vec<_>>(),
            "writing {fields} must append {} for `open_ones`; the append held {:?}",
            match want {
                Some(true) => "one change, joining".to_owned(),
                Some(false) => "one change, leaving".to_owned(),
                None => "no change, since membership stays as it was".to_owned(),
            },
            appended.iter().map(|e| e.body.kind()).collect::<Vec<_>>()
        );
        let named = set_off(&answer);
        let (joined, left) = (named.joined == ["open_ones"], named.left == ["open_ones"]);
        ensure!(
            (joined, left) == (want == Some(true), want == Some(false)),
            "the answer to writing {fields} names `open_ones` as {want:?} (joined, else \
             left); it answered {answer:?}"
        );
    }
    Ok(())
}

async fn polling_from_next_in_small_pages_sees_every_matching_event_once_in_order(
    store: Store,
) -> Held {
    let l = invoices(store.as_ref()).await?;
    let invoices_only = filter(&["record_upserted"], &["invoice"], &[]);
    let mut next = store.head().await.must("reading the head")?.next();
    let mut seen: Vec<Position> = Vec::new();
    let mut written: Vec<Position> = Vec::new();
    let feed = store.feed();
    for round in 0..6 {
        // Writes land between polls: one or two invoices back to back
        // (so a page can end right before another match), and a memo the
        // filter leaves out every other round.
        for n in 0..(2 - round % 2) {
            let key = format!("p{round}-{n}");
            written.push(
                l.upsert("invoice", &key, json!({"status": "open"}))
                    .await?
                    .position(),
            );
        }
        if round % 2 == 0 {
            let key = format!("m{round}");
            l.upsert("memo", &key, json!({"text": "aside"})).await?;
        }
        let limit = 1 + round % 2;
        let batch = feed.poll(next, &invoices_only, limit);
        let batch = batch.await.must("polling the feed from `next`")?;
        ensure!(
            batch.events.len() <= limit as usize && batch.next > next || batch.events.is_empty(),
            "a poll from {next} with limit {limit} answered {} events and next {}",
            batch.events.len(),
            batch.next
        );
        seen.extend(batch.events.iter().map(|e| e.position));
        next = batch.next;
    }
    // Drain what is left.
    loop {
        let batch = feed.poll(next, &invoices_only, 1);
        let batch = batch.await.must("polling the rest of the feed")?;
        if batch.events.is_empty() {
            break;
        }
        seen.extend(batch.events.iter().map(|e| e.position));
        next = batch.next;
    }
    ensure!(
        seen == written,
        "polling from `next` in pages of one and two, with writes between the polls, must see \
         each invoice write once and in log order: written at {written:?}, seen at {seen:?}"
    );
    Ok(())
}

/// The kinds of the events that are about a record.
const ABOUT_A_RECORD: [&str; 8] = [
    "record_upserted",
    "record_ingested",
    "record_retired",
    "record_restricted",
    "record_unrestricted",
    "trigger_fired",
    "object_set_changed",
    "verb_called",
];

/// A feed filter of these kinds, types and keys, each empty for all.
fn filter(kinds: &[&str], types: &[&str], keys: &[&str]) -> FeedFilter {
    FeedFilter {
        kinds: kinds.iter().map(|k| (*k).to_owned()).collect(),
        types: types.iter().map(|t| TypeName::from(*t)).collect(),
        keys: keys.iter().map(|k| RecordKey::from(*k)).collect(),
    }
}

async fn polled(store: &dyn LakeStore, filter: FeedFilter) -> Result<Vec<Event>, Failure> {
    followed(store, Position::GENESIS, &filter).await
}

/// Whether an event is about the record `key` of one of `types` (any type
/// when empty), read from its body's own members.
fn about(e: &Event, types: &[&str], key: &str) -> bool {
    let body = serde_json::to_value(&e.body).unwrap_or_default();
    let kind = e.body.kind();
    ABOUT_A_RECORD.contains(&kind)
        && body["key"] == key
        && (types.is_empty() || types.iter().any(|t| body["type"] == *t))
}

fn positions(events: &[Event]) -> Vec<u64> {
    events.iter().map(|e| e.position.0).collect()
}

async fn a_feed_filtered_by_key_answers_that_records_events_only(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    for ty in ["invoice", "credit"] {
        l.declare(type_of(ty, &[("status", Text), ("amount", Number)]))
            .await?;
    }
    for d in [
        json!({"declare": "trigger", "name": "paid", "on": "invoice", "when": "status = 'paid'"}),
        json!({"declare": "object_set", "name": "open_ones", "of": "invoice", "where": "status = 'open'"}),
        json!({"declare": "verb", "name": "approve", "on": "invoice", "risk": "low",
               "effect": "write", "set": [{"field": "status", "value": "'approved'"}]}),
    ] {
        l.declare_json(d).await?;
    }
    for (ty, key) in [
        ("invoice", "INV-1"),
        ("invoice", "INV-2"),
        ("credit", "INV-1"),
    ] {
        l.upsert(ty, key, json!({"status": "open", "amount": 1}))
            .await?;
    }
    cat.call(
        own,
        &"approve".into(),
        Some(&"INV-1".into()),
        crate::conformance::fixture::patch(json!({})),
        lakelands_api::WriteOpts::default(),
    )
    .await
    .must("calling `approve` on INV-1")?;
    l.upsert("invoice", "INV-1", json!({"status": "paid"}))
        .await?;
    let (invoice, inv1) = (
        TypeName::from("invoice"),
        lakelands_api::RecordKey::from("INV-1"),
    );
    let fut = store.restrict(&invoice, &inv1, "a hold".into());
    fut.await.must("restricting INV-1")?;
    let fut = store.unrestrict(&invoice, &inv1);
    fut.await.must("lifting the hold")?;
    for key in ["INV-2", "INV-1"] {
        l.retire(invoice.as_str(), key).await?;
    }

    let log = log_events(store.as_ref()).await?;

    // Types and keys: one record's history.
    let got = polled(store.as_ref(), filter(&[], &["invoice"], &["INV-1"])).await?;
    let want: Vec<Event> = log
        .iter()
        .filter(|e| about(e, &["invoice"], "INV-1"))
        .cloned()
        .collect();
    ensure!(
        got == want,
        "a feed filtered by `invoice` and `INV-1` answered positions {:?}; the events about \
         that record are at {:?}",
        positions(&got),
        positions(&want)
    );
    let kinds: std::collections::BTreeSet<&str> = got.iter().map(|e| e.body.kind()).collect();
    for kind in [
        "record_upserted",
        "trigger_fired",
        "object_set_changed",
        "record_restricted",
        "record_unrestricted",
        "record_retired",
    ] {
        ensure!(
            kinds.contains(kind),
            "INV-1's history holds a `{kind}` event; the keyed feed answered the kinds {kinds:?}"
        );
    }

    // Keys alone: the key in any type, and the verb called on it.
    let got = polled(store.as_ref(), filter(&[], &[], &["INV-1"])).await?;
    let want: Vec<Event> = log
        .iter()
        .filter(|e| about(e, &[], "INV-1"))
        .cloned()
        .collect();
    ensure!(
        got == want,
        "a feed filtered by the key `INV-1` alone answered positions {:?}; the events about a \
         record keyed INV-1, in any type, are at {:?}",
        positions(&got),
        positions(&want)
    );
    ensure!(
        got.iter().any(|e| e.body.kind() == "verb_called")
            && got
                .iter()
                .any(|e| serde_json::to_value(&e.body).unwrap_or_default()["type"] == "credit"),
        "with types empty, the key matches the `credit` of that key and the verb called on it; \
         the feed answered the kinds {:?}",
        got.iter().map(|e| e.body.kind()).collect::<Vec<_>>()
    );

    // Kinds narrow a key filter further.
    let got = polled(
        store.as_ref(),
        filter(&["record_upserted"], &["invoice"], &["INV-1"]),
    )
    .await?;
    let want: Vec<Event> = log
        .iter()
        .filter(|e| e.body.kind() == "record_upserted" && about(e, &["invoice"], "INV-1"))
        .cloned()
        .collect();
    ensure!(
        got == want && !got.is_empty(),
        "kinds and a key together answered positions {:?}; INV-1's upserts are at {:?}",
        positions(&got),
        positions(&want)
    );
    the_index_answers_the_log(store.as_ref(), "records of two types under one key").await
}

async fn a_key_filter_does_not_follow_a_rekey(store: Store) -> Held {
    let l = super::rekey::lake(store.as_ref(), super::rekey::IBANS).await?;
    let at = super::rekey::apply(store.as_ref()).await?;
    l.upsert("account", "DE1", json!({"holder": "after the re-key"}))
        .await?;
    let old = polled(store.as_ref(), filter(&[], &["account"], &["1"])).await?;
    ensure!(
        !old.is_empty() && old.iter().all(|e| e.position < at),
        "account 1's old key answers its events before the re-key at {at} and none after; \
         the feed answered positions {:?}",
        positions(&old)
    );
    let new = polled(store.as_ref(), filter(&[], &["account"], &["DE1"])).await?;
    ensure!(
        !new.is_empty() && new.iter().all(|e| e.position > at),
        "account 1's new key DE1 answers what came after the re-key at {at}; the feed \
         answered positions {:?}",
        positions(&new)
    );
    the_index_answers_the_log(store.as_ref(), "a lake re-keyed").await
}

/// The projection's index of the log answers what the log does
/// (lakelands#678): for every filter of a grid made from the lake's own
/// log (each kind, type, key and type with key it holds, kinds with types,
/// two of each, and a kind and a type it never holds), a poll from 0
/// answers exactly the events of the whole log, read with no filter, that
/// the filter admits, whole and in order; followed a match at a time it
/// answers them again, each poll moving `next` on, the last one past the
/// head. `what` names the lake in a failure.
pub(super) async fn the_index_answers_the_log(store: &dyn LakeStore, what: &str) -> Held {
    let log = log_events(store).await?;
    let mut kinds = Vec::new();
    let (mut types, mut keys, mut pairs, mut typed) = (vec![], vec![], vec![], vec![]);
    for e in &log {
        kinds.push(e.body.kind());
        for a in crate::feed::about(&e.body) {
            if let Some(ty) = &a.ty {
                types.push(ty.to_string());
                typed.push((e.body.kind(), ty.to_string()));
            }
            if let Some(key) = &a.key {
                keys.push(key.to_string());
                if let Some(ty) = &a.ty {
                    pairs.push((ty.to_string(), key.to_string()));
                }
            }
        }
    }
    for list in [&mut types, &mut keys] {
        list.sort();
        list.dedup();
    }
    kinds.sort();
    kinds.dedup();
    pairs.sort();
    pairs.dedup();
    typed.sort();
    typed.dedup();
    let mut grid = vec![
        filter(&["never_a_kind"], &[], &[]),
        filter(&[], &["no_such_type"], &[]),
    ];
    grid.extend(kinds.iter().map(|k| filter(&[k], &[], &[])));
    grid.extend(types.iter().map(|t| filter(&[], &[t], &[])));
    grid.extend(keys.iter().map(|k| filter(&[], &[], &[k])));
    grid.extend(pairs.iter().map(|(t, k)| filter(&[], &[t], &[k])));
    grid.extend(typed.iter().map(|(k, t)| filter(&[k], &[t], &[])));
    if let ([a, b, ..], [c, d, ..]) = (kinds.as_slice(), types.as_slice()) {
        grid.push(filter(&[a, b], &[], &[]));
        grid.push(filter(&[a, b], &[c, d], &[]));
    }
    let head = store.head().await.must("reading the head")?;
    for f in grid {
        let admits =
            |e: &&Event| crate::feed::admits(&f, e.body.kind(), &crate::feed::about(&e.body));
        let want: Vec<Event> = log.iter().filter(admits).cloned().collect();
        let got = followed(store, Position::GENESIS, &f).await?;
        ensure!(
            got == want,
            "{what}: a poll with {f:?} answered positions {:?}; the log holds {:?} that it admits",
            positions(&got),
            positions(&want)
        );
        let (mut from, mut paged, mut polls) = (Position::GENESIS, Vec::new(), 0);
        loop {
            let b = store.feed().poll(from, &f, 1).await;
            let b = b.must(&format!("polling with {f:?} from {from}"))?;
            ensure!(
                b.next > from && b.events.len() <= 1,
                "{what}: a poll with {f:?} from {from} and a limit of 1 answered {} events and \
                 next {}; it moves on and answers at most one",
                b.events.len(),
                b.next
            );
            (from, polls) = (b.next, polls + 1);
            paged.extend(b.events);
            if from > b.head {
                break;
            }
        }
        ensure!(
            paged == want && polls <= want.len() + 1 && from == head.next(),
            "{what}: following {f:?} a match at a time answered {:?} in {polls} polls and \
             stopped at {from}; it answers {:?}, in at most one poll more, and stops past the \
             head {head}",
            positions(&paged),
            positions(&want)
        );
    }
    Ok(())
}

/// Each event `filter` admits from `from` on, with the last position of
/// its append as `append_of` names it, polled `page` events at a time.
async fn ended(
    store: &dyn LakeStore,
    from: Position,
    filter: &FeedFilter,
    page: u32,
) -> Result<Vec<(Event, Option<Position>)>, Failure> {
    let (feed, mut next, mut out) = (store.feed(), from, Vec::new());
    loop {
        let batch = feed.poll(next, filter, page).await;
        let batch = batch.must(&format!("polling from {next}, {page} a poll"))?;
        let ends: Vec<_> = (batch.events.iter())
            .map(|e| batch.append_of(e.position))
            .collect();
        out.extend(batch.events.into_iter().zip(ends));
        if batch.next > batch.head {
            return Ok(out);
        }
        next = batch.next;
    }
}

/// The ends `ended` names grouped: each end with the positions it names,
/// in order.
fn grouped(ended: &[(Event, Option<Position>)]) -> Vec<(Option<Position>, Vec<Position>)> {
    let mut groups: Vec<(Option<Position>, Vec<Position>)> = Vec::new();
    for (e, end) in ended {
        match groups.last_mut() {
            Some((last, at)) if last == end => at.push(e.position),
            _ => groups.push((*end, vec![e.position])),
        }
    }
    groups
}

async fn a_consumer_groups_the_firings_of_two_ingests_into_their_two_appends(
    store: Store,
    given: crate::conformance::Given,
) -> Held {
    use crate::conformance::Lines;
    use lakelands_api::engine::OpenMode;
    use lakelands_api::event::EventId;
    use lakelands_api::{DeliveryId, IngestOpts};
    let l = Lake::of(store.as_ref());
    l.declare_json(json!({"declare": "type", "name": "reading", "fields": [
        {"name": "amount", "type": {"kind": "number"}, "authority": "source"}]}))
        .await?;
    l.declare_as(trigger_on("seen", "reading", None), "declaring `seen`")
        .await?;
    // The clock behind the log: a `lease_taken` a day ahead of it, below
    // the store, so each append after it reads its `known_at`.
    let lake = store.lake().clone();
    let events = log_events(store.as_ref()).await?;
    let taken = |e: &&Event| matches!(e.body, EventBody::LeaseTaken { .. });
    let (Some(taken), Some(last)) = (events.iter().find(taken), events.last()) else {
        return Err(Failure::new(
            "the lake's log holds its writer's `lease_taken`",
        ));
    };
    let (template, ahead) = (taken.clone(), Timestamp(last.known_at.0 + 86_400_000_000));
    let line: Lines = Box::new(move |(tip, prev), fence| {
        let (position, holder) = (tip.next(), "a writer ahead".into());
        let event = Event {
            position,
            id: EventId(format!("{:032x}", position.0)),
            prev,
            known_at: ahead,
            fence,
            body: EventBody::LeaseTaken { fence, holder },
            ..template
        };
        (vec![crate::conformance::fixture::canonical_line(event)], 0)
    });
    let c = (given.continued)(Some(store), lake, line, OpenMode::Write, Seams::default());
    let c = c
        .await
        .map_err(|e| Failure::new(format!("continuing the lake below the store: {e}")))?;
    let store = c
        .opened
        .map_err(|e| Failure::new(format!("opening the lake for a write after it: {e}")))?;
    let l = Lake::of(store.as_ref());
    let from = store.head().await.must("reading the head")?.next();
    let mut windows = Vec::new();
    for keys in [["A1", "A2"], ["B1", "B2"]] {
        let records = keys.map(|k| source_record("reading", k, json!({"amount": 1})));
        let records = records.to_vec();
        let opts = IngestOpts {
            delivery: Some(DeliveryId::from("nightly")),
            ..IngestOpts::default()
        };
        let source = SourceId::from("meter");
        let said = lakelands_api::ingest_all_with(l.cat.as_ref(), &l.own, &source, opts, records);
        let said = said
            .await
            .must(&format!("ingesting {keys:?} as `nightly`"))?;
        ensure!(
            said.len() == 1,
            "ingesting {keys:?} commits one window; it committed {said:?}"
        );
        windows.push(Some(said[0].position));
    }
    let all = ended(store.as_ref(), from, &FeedFilter::default(), 100).await?;
    let read: BTreeSet<_> = all.iter().map(|(e, _)| e.known_at).collect();
    ensure!(
        read == BTreeSet::from([ahead]),
        "every append after the line ahead reads its `known_at`, {ahead:?}; they read {read:?}"
    );
    let firings = FeedFilter {
        kinds: vec!["trigger_fired".into()],
        ..FeedFilter::default()
    };
    for page in [100, 1] {
        let fired = ended(store.as_ref(), from, &firings, page).await?;
        let groups = grouped(&fired);
        let ends: Vec<_> = groups.iter().map(|(end, _)| *end).collect();
        ensure!(
            groups.len() == 2 && groups.iter().all(|(_, at)| at.len() == 2) && ends == windows,
            "a consumer of `trigger_fired`, {page} a poll, groups the four firings of two \
             ingests into two appends of two, ending at the windows {windows:?}; it groups \
             {groups:?}"
        );
    }
    Ok(())
}

async fn one_append_carrying_two_deliveries_is_one_append(store: Store) -> Held {
    use lakelands_api::DeliveryId;
    let l = Lake::of(store.as_ref());
    l.declare_json(json!({"declare": "type", "name": "reading", "fields": [
        {"name": "amount", "type": {"kind": "number"}, "authority": "source"}]}))
        .await?;
    let from = store.head().await.must("reading the head")?.next();
    let records = [("r1", "monday"), ("r2", "monday"), ("r3", "tuesday")].map(|(k, d)| {
        let mut r = source_record("reading", k, json!({"amount": 1}));
        r.delivery = Some(DeliveryId::from(d));
        r
    });
    let said = l.ingest("meter", records.to_vec()).await;
    let said = said.must("ingesting three records naming two deliveries")?;
    ensure!(
        said.len() == 1,
        "three records naming two deliveries commit in one window; they committed {said:?}"
    );
    let end = Some(said[0].position);
    for page in [100, 1] {
        let all = ended(store.as_ref(), from, &FeedFilter::default(), page).await?;
        let groups = grouped(&all);
        ensure!(
            groups.len() == 1 && groups[0].0 == end && groups[0].1.len() >= 3,
            "the window's events, {page} a poll, are one append ending at {end:?}; the feed \
             groups them {groups:?}"
        );
    }
    Ok(())
}
