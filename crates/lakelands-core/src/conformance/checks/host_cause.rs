//! A host names the cause of its write, and every write path holds the
//! reactive ceiling (`docs/decisions/021-the-write-context-belongs-to-the-host.md`,
//! extended).
//!
//! A host binds a catalog to a context whose `cause` names the position
//! its write follows from. The cause lands in the envelope of each event
//! the write itself appends; events the store derives from those keep the
//! store's cause, which chains back to it. A cause past the head is
//! refused. The depth ceiling holds on every door that writes, not only
//! on the record doors.

use std::sync::Arc;

use lakelands_api::event::{Event, EventBody};
use lakelands_api::expr::Condition;
use lakelands_api::ontology::{
    Assignment, Authority, Risk, StorageKind, TriggerDecl, VerbDecl, VerbEffect,
};
use lakelands_api::principal::{WriteContext, WriteOrigin};
use lakelands_api::seam::FeedFilter;
use lakelands_api::write::{Declaration, Migration, ProposedWrite};
use lakelands_api::{Catalog, LakeStore, Position, Refusal, TypeName, WriteOpts};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, doors, field, head_unmoved, ingest, limits, owner, patch, reason, source_record,
    type_decl,
};

check!(
    /// A host's cause lands on its write's own events.
    CAUSE_ON_OWN_EVENTS = a_reactive_write_with_a_cause_carries_it_on_each_of_its_events,
    "021, extended: a write through a catalog whose context names a cause carries it as the envelope's `cause` on each event the write itself appends (an upsert's record, a verb's `verb_called`, an ingest's record), while the events the store derives keep the store's cause (a trigger's firing names its write, a verb's write names its `verb_called`); no event's `context` carries `cause`"
);

check!(
    /// A cause past the head is refused.
    CAUSE_PAST_HEAD = a_cause_past_the_head_is_refused_and_appends_nothing,
    "021, extended: a write whose context names a cause past the lake's head is refused as invalid naming both positions, appending nothing; a cause at the head, and at position 0, is taken"
);

check!(
    /// The ceiling holds on every door that writes.
    DEPTH_ON_EVERY_DOOR = the_reactive_ceiling_holds_on_every_door_that_writes,
    "021, as extended on 2026-09-25 (lakelands#257): a context deeper than the lake's max_reactive_depth is refused as too_deep, appending nothing, through ingest, accept, reject, declare, apply, add_field, migrate_apply and migrate_value_type_apply as through the record doors"
);

fn host(depth: u32, cause: Option<Position>) -> WriteContext {
    WriteContext {
        origin: WriteOrigin::Reactive,
        depth,
        cause,
    }
}

/// The events after `after`, as the feed tells them.
async fn events_after(store: &dyn LakeStore, after: Position) -> Result<Vec<Event>, Failure> {
    Ok(store
        .feed()
        .poll(after.next(), &FeedFilter::default(), 10_000)
        .await
        .must("reading the feed")?
        .events)
}

async fn a_reactive_write_with_a_cause_carries_it_on_each_of_its_events(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let mut reading = type_decl("reading", vec![field("value", StorageKind::Number)]);
    reading.fields[0].authority = Authority::Source;
    for d in [
        Declaration::Type(type_decl("job", vec![field("status", StorageKind::Text)])),
        Declaration::Type(reading),
        Declaration::Trigger(TriggerDecl {
            max_depth: None,
            name: "opened".into(),
            on: "job".into(),
            when: None,
            description: None,
        }),
        Declaration::Verb(VerbDecl {
            name: "close".into(),
            on: "job".into(),
            scope: Default::default(),
            payload: vec![],
            risk: Risk::Low,
            requires: None,
            effect: VerbEffect::Write {
                set: vec![Assignment {
                    field: "status".into(),
                    value: Condition::from("'closed'"),
                }],
            },
            description: None,
        }),
    ] {
        cat.declare(own, d)
            .await
            .must("declaring the job, its trigger and verb, and the reading")?;
    }
    // The direct write whose trigger firing the host reacts to.
    let before = store.head().await.must("reading the head")?;
    l.upsert("job", "j0", json!({"status": "open"})).await?;
    let fired = events_after(store.as_ref(), before)
        .await?
        .into_iter()
        .find(|e| matches!(e.body, EventBody::TriggerFired { .. }))
        .ok_or_else(|| Failure::new("writing `j0` must fire the trigger `opened`"))?
        .position;

    let reaction = store.catalog(host(1, Some(fired)));
    let before = store.head().await.must("reading the head")?;
    reaction
        .upsert(
            own,
            &TypeName::from("job"),
            &"j1".into(),
            patch(json!({"status": "open"})),
            WriteOpts::default(),
        )
        .await
        .must("the host's reactive upsert of `j1`")?;
    reaction
        .call(
            own,
            &"close".into(),
            Some(&"j1".into()),
            patch(json!({})),
            WriteOpts::default(),
        )
        .await
        .must("the host's reactive call of `close` on `j1`")?;
    ingest(
        reaction.as_ref(),
        own,
        "meter",
        vec![source_record("reading", "r1", json!({"value": 7}))],
    )
    .await
    .must("the host's reactive ingest of `r1`")?;

    let events = events_after(store.as_ref(), before).await?;
    let mut seen = [false; 3];
    let mut verb_called = None;
    for e in &events {
        ensure!(
            e.context.cause.is_none(),
            "no event's `context` carries `cause`; the one at {} does",
            e.position
        );
        match &e.body {
            EventBody::VerbCalled { .. } => {
                ensure!(
                    e.cause == Some(fired),
                    "the reaction's `verb_called` must carry the host's cause {fired}; it \
                     carried {:?}",
                    e.cause
                );
                verb_called = Some(e.position);
                seen[1] = true;
            }
            EventBody::RecordUpserted { .. } if verb_called.is_some() => {
                ensure!(
                    e.cause == verb_called,
                    "the verb's write must keep the store's cause, its `verb_called` at {:?}; \
                     it carried {:?}",
                    verb_called,
                    e.cause
                );
            }
            EventBody::RecordUpserted { .. } => {
                ensure!(
                    e.cause == Some(fired),
                    "the reaction's `record_upserted` must carry the host's cause {fired}; it \
                     carried {:?}",
                    e.cause
                );
                seen[0] = true;
            }
            EventBody::RecordIngested { .. } => {
                ensure!(
                    e.cause == Some(fired),
                    "the reaction's `record_ingested` must carry the host's cause {fired}; it \
                     carried {:?}",
                    e.cause
                );
                seen[2] = true;
            }
            EventBody::TriggerFired { .. } => {
                let write = e
                    .cause
                    .and_then(|c| events.iter().find(|w| w.position == c));
                ensure!(
                    write.is_some_and(|w| matches!(w.body, EventBody::RecordUpserted { .. })),
                    "a trigger's firing must keep the store's cause, the write that fired it; \
                     the one at {} names {:?}",
                    e.position,
                    e.cause
                );
            }
            _ => {}
        }
    }
    ensure!(
        seen == [true; 3],
        "the reaction must have appended a `record_upserted`, a `verb_called` and a \
         `record_ingested`; seen {seen:?}"
    );
    Ok(())
}

async fn a_cause_past_the_head_is_refused_and_appends_nothing(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let own = owner(store.as_ref());
    crate::conformance::fixture::declare(
        doors(store.as_ref()).as_ref(),
        &own,
        type_decl("job", vec![field("status", StorageKind::Text)]),
    )
    .await?;
    let head = store.head().await.must("reading the head")?;
    let past = head.next();
    let write = |cat: Arc<dyn Catalog>, key: &'static str| {
        let own = own.clone();
        async move {
            cat.upsert(
                &own,
                &TypeName::from("job"),
                &key.into(),
                patch(json!({"status": "open"})),
                WriteOpts::default(),
            )
            .await
        }
    };
    match write(store.catalog(host(1, Some(past))), "j1").await {
        Ok(o) => {
            return Err(Failure::new(format!(
                "a cause {past} past the head {head} must be refused; the write landed at {}",
                o.position()
            )));
        }
        Err(e) => {
            ensure!(
                matches!(reason(&e), Some(Refusal::Invalid { .. })),
                "a cause past the head must be refused as invalid; it was refused as {e:?}"
            );
            let why = e.to_string();
            ensure!(
                why.contains(&past.to_string()) && why.contains(&head.to_string()),
                "the refusal must name the cause {past} and the head {head}; it said: {why}"
            );
        }
    }
    head_unmoved(store.as_ref(), head, "a write whose cause is past the head").await?;
    write(store.catalog(host(1, Some(head))), "j2")
        .await
        .must("a write whose cause is the head")?;
    write(store.catalog(host(1, Some(Position::GENESIS))), "j3")
        .await
        .must("a write whose cause is position 0")?;
    Ok(())
}

async fn the_reactive_ceiling_holds_on_every_door_that_writes(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let mut reading = type_decl("reading", vec![field("value", StorageKind::Number)]);
    reading.fields[0].authority = Authority::Source;
    for t in [
        type_decl("job", vec![field("status", StorageKind::Text)]),
        reading,
    ] {
        crate::conformance::fixture::declare(cat.as_ref(), own, t).await?;
    }
    let proposal = |key: &str| ProposedWrite::Upsert {
        ty: "job".into(),
        key: key.into(),
        patch: patch(json!({"status": "open"})),
        opts: WriteOpts::default(),
    };
    let pending = |o: lakelands_api::write::Outcome| match o {
        lakelands_api::write::Outcome::Proposed { proposal, .. } => Ok(proposal),
        other => Err(Failure::new(format!(
            "a proposal must be pending; it answered {other:?}"
        ))),
    };
    let accept_me = pending(
        cat.propose(own, proposal("p1"), "to accept".into())
            .await
            .must("proposing `p1`")?,
    )?;
    let reject_me = pending(
        cat.propose(own, proposal("p2"), "to reject".into())
            .await
            .must("proposing `p2`")?,
    )?;
    let mut widened = type_decl("job", vec![field("status", StorageKind::Text)]);
    widened.fields.push(field("note", StorageKind::Text));
    let migration = Migration {
        to: widened.clone(),
        renames: vec![],
    };
    let report = cat
        .migrate(own, migration.clone())
        .await
        .must("preflighting a migration of `job`")?
        .data;

    let deep = store.catalog(host(limits().max_reactive_depth + 1, None));
    let head = store.head().await.must("reading the head")?;
    let tries: Vec<(&str, lakelands_api::Result<()>)> = vec![
        (
            "ingest",
            ingest(
                deep.as_ref(),
                own,
                "meter",
                vec![source_record("reading", "r1", json!({"value": 7}))],
            )
            .await
            .map(drop),
        ),
        ("accept", deep.accept(own, &accept_me).await.map(drop)),
        (
            "reject",
            deep.reject(own, &reject_me, "no".into()).await.map(drop),
        ),
        (
            "declare",
            deep.declare(
                own,
                Declaration::Type(type_decl("other", vec![field("n", StorageKind::Number)])),
            )
            .await
            .map(drop),
        ),
        (
            "apply",
            deep.apply(
                own,
                vec![Declaration::Type(type_decl(
                    "another",
                    vec![field("n", StorageKind::Number)],
                ))],
            )
            .await
            .map(drop),
        ),
        (
            "add_field",
            deep.add_field(own, &"job".into(), field("extra", StorageKind::Text))
                .await
                .map(drop),
        ),
        (
            "migrate_apply",
            deep.migrate_apply(own, migration, &report.report_id)
                .await
                .map(drop),
        ),
    ];
    for (door, answer) in tries {
        match answer {
            Err(e) if matches!(reason(&e), Some(Refusal::TooDeep { .. })) => {}
            other => {
                return Err(Failure::new(format!(
                    "`{door}` through a context past the ceiling must be refused as too_deep; \
                     it answered {other:?}"
                )));
            }
        }
    }
    head_unmoved(store.as_ref(), head, "every write past the ceiling").await?;
    let vt = lakelands_api::ontology::ValueTypeDecl {
        name: "code".into(),
        kind: StorageKind::Text,
        constraint: None,
        one_of: vec![],
        format: None,
        scale: None,
        description: None,
    };
    cat.declare(own, Declaration::ValueType(vt.clone()))
        .await
        .must("declaring the value type `code`")?;
    let tightened = lakelands_api::ontology::ValueTypeDecl {
        one_of: vec!["a".into()],
        ..vt
    };
    let report = cat
        .migrate_value_type(own, tightened.clone())
        .await
        .must("preflighting a tightened `code`")?
        .data;
    let before = store.head().await.must("reading the head")?;
    match deep
        .migrate_value_type_apply(own, tightened, &report.report_id)
        .await
    {
        Err(e) if matches!(reason(&e), Some(Refusal::TooDeep { .. })) => {}
        other => {
            return Err(Failure::new(format!(
                "`migrate_value_type_apply` through a context past the ceiling must be refused \
                 as too_deep; it answered {other:?}"
            )));
        }
    }
    head_unmoved(
        store.as_ref(),
        before,
        "`migrate_value_type_apply` past the ceiling",
    )
    .await?;
    Ok(())
}
