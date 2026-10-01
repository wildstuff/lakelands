//! A verb's write reacts like any write
//! (`docs/decisions/004-the-change-feed-is-the-log.md`,
//! `docs/decisions/013-verbs-run-by-declared-risk.md`).
//!
//! A trigger fires on every write of its type whose guard holds, and an
//! object set records a record joining it, whichever door wrote the
//! record: an `upsert`, an ingest, or the write a verb makes when it runs.
//! Both facts are in the log, in the same append, caused by that write.

use std::sync::Arc;

use lakelands_api::event::EventBody;
use lakelands_api::expr::Condition;
use lakelands_api::ontology::{Assignment, Risk, StorageKind, TriggerDecl, VerbDecl, VerbEffect};
use lakelands_api::seam::FeedFilter;
use lakelands_api::write::{Declaration, ObjectSetDecl};
use lakelands_api::{LakeStore, Patch};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{Lake, Must, field, type_decl};

check!(
    /// A trigger fires, and a set changes, on a verb's write.
    VERB_WRITE_REACTS = a_trigger_fires_on_a_verbs_write,
    "004 and 013: a verb's write fires the triggers whose guard holds and changes object sets in the same append, caused by that write, as any other write does"
);

async fn a_trigger_fires_on_a_verbs_write(store: Arc<dyn LakeStore>) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let declarations = [
        Declaration::Type(type_decl(
            "invoice",
            vec![field("status", StorageKind::Text)],
        )),
        Declaration::Verb(VerbDecl {
            name: "settle".into(),
            on: "invoice".into(),
            scope: Default::default(),
            payload: vec![],
            risk: Risk::Low,
            requires: None,
            effect: VerbEffect::Write {
                set: vec![Assignment {
                    field: "status".into(),
                    value: Condition::from("'paid'"),
                }],
            },
            description: None,
        }),
        Declaration::Trigger(TriggerDecl {
            max_depth: None,
            name: "paid".into(),
            on: "invoice".into(),
            when: Some(Condition::from("status = 'paid'")),
            description: None,
        }),
        Declaration::ObjectSet(ObjectSetDecl {
            name: "settled".into(),
            of: "invoice".into(),
            filter: Condition::from("status = 'paid'"),
            description: None,
        }),
    ];
    for d in declarations {
        cat.declare(own, d)
            .await
            .must("declaring the invoice, its verb, trigger and set")?;
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
    let feed = store
        .feed()
        .poll(opened.position().next(), &FeedFilter::default(), 100)
        .await
        .must("reading the feed after the upsert")?;
    let written = feed
        .events
        .iter()
        .find(|e| matches!(e.body, EventBody::RecordUpserted { .. }))
        .ok_or_else(|| Failure::new("the verb `settle` wrote no record event"))?;
    ensure!(
        written.position == ran.position(),
        "the verb's outcome names position {}, and its record write is at {}",
        ran.position(),
        written.position
    );
    let fired = feed.events.iter().any(|e| {
        e.cause == Some(written.position)
            && matches!(&e.body, EventBody::TriggerFired { trigger, .. } if trigger.as_str() == "paid")
    });
    ensure!(
        fired,
        "the verb's write made `status = 'paid'` hold, and the trigger `paid` did not fire, \
         caused by that write; the feed after the upsert holds {:?}",
        feed.events
            .iter()
            .map(|e| e.body.kind())
            .collect::<Vec<_>>()
    );
    let joined = feed.events.iter().any(|e| {
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
