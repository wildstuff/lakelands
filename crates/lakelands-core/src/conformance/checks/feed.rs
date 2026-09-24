//! The change feed is the log, and what a write makes due is in the same
//! append (`docs/decisions/004-the-change-feed-is-the-log.md`,
//! `docs/decisions/001-the-log-is-the-truth.md`).

use std::sync::Arc;

use lakelands_api::event::{Event, EventBody};
use lakelands_api::ontology::StorageKind;
use lakelands_api::seam::FeedFilter;
use lakelands_api::write::Declaration;
use lakelands_api::{LakeStore, Position, TypeName};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{Must, declare, doors, field, owner, type_decl, upsert};

check!(
    /// The feed answers the log from a position.
    FEED_IS_THE_LOG = the_feed_answers_the_log_from_a_position_by_kind_and_type,
    "004: `poll` answers the events from a position in log order, filtered by kind and type, with the position to poll from next and the head"
);

check!(
    /// A rebuild changes nothing the feed says.
    REBUILD_LEAVES_THE_FEED = rebuilding_the_projection_changes_nothing_in_the_feed,
    "001 and 004: the log is the truth and the projection an index; deleting and rebuilding the projection leaves every event as it was"
);

check!(
    /// A trigger fires in the same append as the write that makes it hold.
    TRIGGER_FIRES = a_trigger_fires_on_every_write_whose_guard_holds,
    "004: when a write makes a trigger's guard hold, `trigger_fired` is appended with it; a write under which the guard does not hold fires nothing"
);

check!(
    /// An object set changes only when membership does.
    OBJECT_SET_CHANGES = an_object_set_change_is_written_only_when_membership_changes,
    "004: a write that moves a record into or out of an object set appends `object_set_changed`; one that leaves membership as it was appends none"
);

async fn invoices(store: &dyn LakeStore) -> Result<(), Failure> {
    let cat = doors(store);
    let own = owner(store);
    declare(
        cat.as_ref(),
        &own,
        type_decl(
            "invoice",
            vec![
                field("status", StorageKind::Text),
                field("note", StorageKind::Text),
            ],
        ),
    )
    .await?;
    declare(
        cat.as_ref(),
        &own,
        type_decl("memo", vec![field("text", StorageKind::Text)]),
    )
    .await?;
    Ok(())
}

async fn all_events(store: &dyn LakeStore, from: Position) -> Result<Vec<Event>, Failure> {
    Ok(store
        .feed()
        .poll(from, &FeedFilter::default(), u32::MAX)
        .await
        .must("reading the feed")?
        .events)
}

check!(
    /// Paging with `next` loses and repeats nothing.
    PAGED_FROM_NEXT = polling_from_next_in_small_pages_sees_every_matching_event_once_in_order,
    "004: a consumer that polls from the `next` each answer gives, in pages of one or two events and with writes landing between polls, sees every matching event once, in log order; this is the position a deliverer keeps"
);

async fn the_feed_answers_the_log_from_a_position_by_kind_and_type(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    invoices(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let from = store.head().await.must("reading the head")?.next();
    let a = upsert(
        cat.as_ref(),
        &own,
        "invoice",
        "a",
        json!({"status": "open"}),
    )
    .await?
    .position();
    let m = upsert(cat.as_ref(), &own, "memo", "m", json!({"text": "hi"}))
        .await?
        .position();
    let b = upsert(
        cat.as_ref(),
        &own,
        "invoice",
        "b",
        json!({"status": "open"}),
    )
    .await?
    .position();
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

    let invoices_only = FeedFilter {
        kinds: vec!["record_upserted".into()],
        types: vec![TypeName::from("invoice")],
    };
    let first = feed
        .poll(from, &invoices_only, 1)
        .await
        .must("polling for one upsert of `invoice`")?;
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

async fn rebuilding_the_projection_changes_nothing_in_the_feed(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    invoices(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    for (k, s) in [("a", "open"), ("b", "paid"), ("a", "held")] {
        upsert(cat.as_ref(), &own, "invoice", k, json!({ "status": s })).await?;
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
    Ok(())
}

async fn a_trigger_fires_on_every_write_whose_guard_holds(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    invoices(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let trigger: Declaration = serde_json::from_value(json!({
        "declare": "trigger", "name": "paid", "on": "invoice", "when": "status = 'paid'"
    }))
    .map_err(|e| Failure::new(format!("the check's trigger does not read: {e}")))?;
    cat.declare(&own, trigger)
        .await
        .must("declaring the trigger `paid`")?;

    for (status, note, fires) in [
        ("open", "one", false),
        ("paid", "two", true),
        ("paid", "three", true),
        ("held", "four", false),
    ] {
        let from = store.head().await.must("reading the head")?.next();
        upsert(
            cat.as_ref(),
            &own,
            "invoice",
            "i",
            json!({"status": status, "note": note}),
        )
        .await?;
        let appended = all_events(store.as_ref(), from).await?;
        let did = appended
            .iter()
            .any(|e| matches!(&e.body, EventBody::TriggerFired { trigger, .. } if trigger.as_str() == "paid"));
        ensure!(
            did == fires,
            "writing status {status:?} must {}fire `paid` (guard `status = 'paid'`) in \
             its own append; the append held {:?}",
            if fires { "" } else { "not " },
            appended.iter().map(|e| e.body.kind()).collect::<Vec<_>>()
        );
    }
    Ok(())
}

async fn an_object_set_change_is_written_only_when_membership_changes(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    invoices(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let set: Declaration = serde_json::from_value(json!({
        "declare": "object_set", "name": "open_ones", "of": "invoice", "where": "status = 'open'"
    }))
    .map_err(|e| Failure::new(format!("the check's object set does not read: {e}")))?;
    cat.declare(&own, set)
        .await
        .must("declaring the object set `open_ones`")?;

    for (fields, want) in [
        (json!({"status": "open"}), Some(true)),
        (json!({"note": "still open"}), None),
        (json!({"status": "paid"}), Some(false)),
        (json!({"note": "still paid"}), None),
    ] {
        let from = store.head().await.must("reading the head")?.next();
        upsert(cat.as_ref(), &own, "invoice", "i", fields.clone()).await?;
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
    }
    Ok(())
}

async fn polling_from_next_in_small_pages_sees_every_matching_event_once_in_order(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    invoices(store.as_ref()).await?;
    let cat = doors(store.as_ref());
    let own = owner(store.as_ref());
    let invoices_only = FeedFilter {
        kinds: vec!["record_upserted".into()],
        types: vec!["invoice".into()],
    };
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
                upsert(
                    cat.as_ref(),
                    &own,
                    "invoice",
                    &key,
                    json!({"status": "open"}),
                )
                .await?
                .position(),
            );
        }
        if round % 2 == 0 {
            let key = format!("m{round}");
            upsert(cat.as_ref(), &own, "memo", &key, json!({"text": "aside"})).await?;
        }
        let limit = 1 + round % 2;
        let batch = feed
            .poll(next, &invoices_only, limit)
            .await
            .must("polling the feed from `next`")?;
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
        let batch = feed
            .poll(next, &invoices_only, 1)
            .await
            .must("polling the rest of the feed")?;
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
