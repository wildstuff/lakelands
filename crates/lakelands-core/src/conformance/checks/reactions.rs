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
use lakelands_api::ontology::StorageKind;
use lakelands_api::write::Declaration;
use lakelands_api::{LakeStore, Patch};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, events_after, field, object_set_on, trigger_on, type_decl, write_verb,
};

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
