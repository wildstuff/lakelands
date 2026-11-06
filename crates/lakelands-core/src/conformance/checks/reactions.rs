//! A verb's write reacts like any write
//! (`docs/decisions/004-the-change-feed-is-the-log.md`,
//! `docs/decisions/013-verbs-run-by-declared-risk.md`).
//!
//! A trigger fires on every write of its type whose guard holds, and an
//! object set records a record joining it, whichever door wrote the
//! record: an `upsert`, an ingest, or the write a verb makes when it runs.
//! Both facts are in the log, in the same append, caused by that write.

use lakelands_api::Patch;

use crate::conformance::fixture::type_of;
use crate::conformance::prelude::*;

check!(
    VERB_WRITE_REACTS = a_trigger_fires_on_a_verbs_write,
    "004 and 013: a verb's write fires the triggers whose guard holds and changes object sets in the same append, caused by that write, as any other write does"
);

async fn a_trigger_fires_on_a_verbs_write(store: Store) -> Held {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let declarations = [
        Declaration::Type(type_decl(
            "invoice",
            vec![field("status", StorageKind::Text)],
        )),
        write_verb("settle", "invoice", "status", "'paid'"),
        trigger_on("paid", "invoice", Some("status = 'paid'")),
        object_set_on("settled", "invoice", "status = 'paid'"),
    ];
    for d in declarations {
        l.declare_as(d, "declaring the invoice, its verb, trigger and set")
            .await?;
    }
    let opened = l.upsert("invoice", "i1", json!({"status": "open"})).await?;
    let ran = cat
        .call(
            own,
            &"settle".into(),
            Some(&"i1".into()),
            Patch::new(),
            Default::default(),
        )
        .await
        .must("calling `settle` on `i1`")?;
    let events = events_after(store.as_ref(), opened.position()).await?;
    let written = events
        .iter()
        .find(|e| matches!(e.body, EventBody::RecordUpserted { .. }))
        .ok_or_else(|| Failure::new("the verb `settle` wrote no record event"))?;
    ensure!(
        written.position == ran.position(),
        "the verb's outcome names position {}, and its record write is at {}",
        ran.position(),
        written.position
    );
    let fired = events.iter().any(|e| {
        e.cause == Some(written.position)
            && matches!(&e.body, EventBody::TriggerFired { trigger, .. } if trigger.as_str() == "paid")
    });
    ensure!(
        fired,
        "the verb's write made `status = 'paid'` hold, and the trigger `paid` did not fire, \
         caused by that write; the feed after the upsert holds {:?}",
        events.iter().map(|e| e.body.kind()).collect::<Vec<_>>()
    );
    let joined = events.iter().any(|e| {
        e.cause == Some(written.position)
            && matches!(
                &e.body,
                EventBody::ObjectSetChanged { set, joined: true, .. } if set == "settled"
            )
    });
    ensure!(
        joined,
        "the verb's write moved `i1` into the object set `settled`, and no change of that set \
         was written, caused by that write"
    );
    Ok(())
}

check!(
    SET_READ_BY_ITS_FILTER = a_sets_filter_from_describe_finds_the_members_its_events_record,
    "004, and seams `Reading an object set`: for records written after a set is declared, `find` under the filter `describe` answers for the set returns exactly the records the latest `object_set_changed` of each says are in it, and `(f1) and not (f2)` returns the members of one set that are not in the other"
);

async fn a_sets_filter_from_describe_finds_the_members_its_events_record(store: Store) -> Held {
    use StorageKind::{Number, Text};
    let l = Lake::of(store.as_ref());
    l.declare(type_of("invoice", &[("status", Text), ("amount", Number)]))
        .await?;
    for (name, filter) in [("open", "status = 'open'"), ("large", "amount > 100")] {
        l.declare_as(object_set_on(name, "invoice", filter), name)
            .await?;
    }
    // Written after both sets are declared, `i5` leaving `open` again; a set
    // declared over records already written is SET_DECLARED_OVER_RECORDS's.
    let from = store.head().await.must("reading the head")?.next();
    let rows = [("i1", "open", 50), ("i2", "open", 200), ("i3", "paid", 300)];
    for (key, status, amount) in rows
        .into_iter()
        .chain([("i5", "open", 0), ("i5", "paid", 0)])
    {
        let fields = json!({"status": status, "amount": amount});
        l.upsert("invoice", key, fields).await?;
    }
    // Each record's latest `object_set_changed`, per set.
    let mut last = BTreeMap::new();
    for e in events_from(store.as_ref(), from).await? {
        if let EventBody::ObjectSetChanged {
            set, key, joined, ..
        } = e.body
        {
            last.insert((set, key.to_string()), joined);
        }
    }
    let members = |set: &str| -> BTreeSet<String> {
        let joined = last.iter().filter(|((s, _), joined)| s == set && **joined);
        joined.map(|((_, key), _)| key.clone()).collect()
    };
    let d = l.describe().await?;
    let filter = |set: &str| {
        let s = d.object_sets.iter().find(|s| s.name == set);
        s.and_then(|s| json!(s.filter).as_str().map(str::to_owned))
            .unwrap_or_default()
    };
    let (open, large) = (members("open"), members("large"));
    let both = format!("({}) and not ({})", filter("open"), filter("large"));
    for (f, want) in [
        (filter("open"), open.clone()),
        (filter("large"), large.clone()),
        (both, &open - &large),
    ] {
        let found = l.all_found("invoice", find_where(&f)).await?;
        let got: BTreeSet<String> = found.into_iter().map(|r| r.key.to_string()).collect();
        ensure!(
            got == want && !want.is_empty(),
            "`find` under {f:?} answers {got:?}; the events record {want:?}"
        );
    }
    Ok(())
}
