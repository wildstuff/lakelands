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

use lakelands_api::event::EventBody;
use lakelands_api::ontology::StorageKind;
use lakelands_api::principal::WriteContext;
use lakelands_api::write::{Declaration, ProposedWrite};
use lakelands_api::{Catalog, LakeStore, Position, RecordKey, Refusal, TypeName, WriteOpts};
use serde_json::json;

use crate::conformance::Failure;
use crate::conformance::fixture::{
    Lake, Must, events_after, field, head_unmoved, ingest, limits, migration, no_such_field,
    nobody, patch, reactive, reason, refused_on, source_record, sourced, trigger_on, try_upsert,
    type_decl, upsert, value_type, write_verb,
};

check!(
    /// A host's cause lands on its write's own events.
    CAUSE_ON_OWN_EVENTS = a_reactive_write_with_a_cause_carries_it_on_each_of_its_events,
    "021, extended: a write through a catalog whose context names a cause carries it as the envelope's `cause` on each event the write itself appends (an upsert's record, a verb's `verb_called`, an ingest's record), while the events the store derives keep the store's cause (a trigger's firing names its write, a verb's write names its `verb_called`); no event's `context` carries `cause`"
);

check!(
    /// A cause past the head is refused.
    CAUSE_PAST_HEAD = a_cause_past_the_head_is_refused_and_appends_nothing,
    "021, extended: a write whose context names a cause past the lake's head is refused as invalid naming both positions, appending nothing, after the door's own checks (016, extended); a cause at the head, and at position 0, is taken"
);

check!(
    /// The ceiling holds on every door that writes.
    DEPTH_ON_EVERY_DOOR = the_reactive_ceiling_holds_on_every_door_that_writes,
    "021, as extended on 2026-09-25 (lakelands#257): a context deeper than the lake's max_reactive_depth is refused as too_deep naming that ceiling, appending nothing, through ingest, accept, reject, declare, apply, add_field, migrate_apply and migrate_value_type_apply as through the record doors"
);

check!(
    /// The context comes first on every door.
    CONTEXT_FIRST = every_write_door_checks_the_context_before_anything_else,
    "021, extended (lakelands#344): a context past the ceiling is refused as too_deep before anything else a door checks, so a request that also breaks the door's own first rule (a principal without grants, a link type at an ordinary door, an undeclared type, relation, verb or proposal) is refused as too_deep on every write door; a principal of another lake is refused not_granted before that"
);

check!(
    /// A trigger's ceiling is read where the cause is judged.
    TRIGGER_CEILING_AT_APPEND = a_triggers_ceiling_is_read_with_the_cause_at_the_append,
    "021, extended (lakelands#375): a reaction naming a cause past the head is refused as invalid even when the next append puts a firing there; once the firing is in the log, a reaction past its trigger's ceiling is refused too_deep naming the trigger, and that ceiling is read at the append, after the door's own checks, while the lake's ceiling comes first"
);

async fn a_reactive_write_with_a_cause_carries_it_on_each_of_its_events(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    let reading = type_decl("reading", vec![sourced("value", StorageKind::Number)]);
    for d in [
        Declaration::Type(type_decl("job", vec![field("status", StorageKind::Text)])),
        Declaration::Type(reading),
        trigger_on("opened", "job", None),
        write_verb("close", "job", "status", "'closed'"),
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

    let reaction = store.catalog(reactive(1, Some(fired)));
    let before = store.head().await.must("reading the head")?;
    upsert(
        reaction.as_ref(),
        own,
        "job",
        "j1",
        json!({"status": "open"}),
    )
    .await?;
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
    let l = Lake::of(store.as_ref());
    l.declare(type_decl("job", vec![field("status", StorageKind::Text)]))
        .await?;
    let own = l.own;
    let head = store.head().await.must("reading the head")?;
    let past = head.next();
    let write = |cat: Arc<dyn Catalog>, key: &'static str| {
        let own = own.clone();
        async move { try_upsert(cat.as_ref(), &own, "job", key, json!({"status": "open"})).await }
    };
    match write(store.catalog(reactive(1, Some(past))), "j1").await {
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
    write(store.catalog(reactive(1, Some(head))), "j2")
        .await
        .must("a write whose cause is the head")?;
    write(store.catalog(reactive(1, Some(Position::GENESIS))), "j3")
        .await
        .must("a write whose cause is position 0")?;
    // The cause is judged at the append, after the door's own checks.
    let late = store.catalog(reactive(
        1,
        Some(store.head().await.must("the head")?.next()),
    ));
    let nobody = nobody(store.as_ref());
    let job = TypeName::from("job");
    let ungranted = late
        .upsert(
            &nobody,
            &job,
            &"j4".into(),
            patch(json!({"status": "x"})),
            WriteOpts::default(),
        )
        .await;
    let stale = WriteOpts {
        expected_version: Some(lakelands_api::Version(9)),
        ..WriteOpts::default()
    };
    let conflict = late.retire(&own, &job, &"j2".into(), stale).await;
    ensure!(
        matches!(refused_on(&ungranted), Some(("status", _)))
            && matches!(conflict, Err(lakelands_api::Error::Conflict { .. })),
        "a cause past the head is judged after the door's own checks: a write of a field \
         nobody may know of answered {ungranted:?}, and a retire at a stale version \
         {conflict:?}"
    );
    Ok(())
}

async fn the_reactive_ceiling_holds_on_every_door_that_writes(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    for t in [
        type_decl("job", vec![field("status", StorageKind::Text)]),
        type_decl("reading", vec![sourced("value", StorageKind::Number)]),
    ] {
        l.declare(t).await?;
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
    let migration = migration(widened.clone());
    let report = cat
        .migrate(own, migration.clone())
        .await
        .must("preflighting a migration of `job`")?
        .data;

    let ceiling = limits().max_reactive_depth;
    let deep = store.catalog(reactive(ceiling + 1, None));
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
            Err(e) if matches!(reason(&e), Some(Refusal::TooDeep { max }) if *max == ceiling) => {}
            other => {
                return Err(Failure::new(format!(
                    "`{door}` through a context past the ceiling must be refused as too_deep \
                     naming the lake's ceiling {ceiling}; it answered {other:?}"
                )));
            }
        }
    }
    head_unmoved(store.as_ref(), head, "every write past the ceiling").await?;
    let vt = value_type("code", StorageKind::Text);
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
        Err(e) if matches!(reason(&e), Some(Refusal::TooDeep { max }) if *max == ceiling) => {}
        other => {
            return Err(Failure::new(format!(
                "`migrate_value_type_apply` through a context past the ceiling must be refused \
                 as too_deep naming the lake's ceiling {ceiling}; it answered {other:?}"
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

async fn every_write_door_checks_the_context_before_anything_else(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    // Nothing is declared and nobody holds a grant: every door would
    // refuse each request below for its own reasons, after the context.
    let nobody = nobody(store.as_ref());
    let deep = store.catalog(reactive(limits().max_reactive_depth + 1, None));
    let (lt, ty, key): (TypeName, TypeName, RecordKey) =
        ("sys.link.job.owner".into(), "nothing".into(), "k".into());
    let opts = WriteOpts::default;
    let vt = lakelands_api::ontology::ValueTypeDecl {
        one_of: vec!["a".into()],
        ..value_type("code", StorageKind::Text)
    };
    let declared = || Declaration::Type(type_decl("job", vec![field("n", StorageKind::Number)]));
    let migration = migration(type_decl("nothing", vec![]));
    let proposed = ProposedWrite::Upsert {
        ty: lt.clone(),
        key: key.clone(),
        patch: patch(json!({"n": 1})),
        opts: opts(),
    };
    let head = store.head().await.must("reading the head")?;
    let d = deep.as_ref();
    let p = &nobody;
    let tries: Vec<(&str, lakelands_api::Result<()>)> = vec![
        (
            "upsert",
            d.upsert(p, &lt, &key, patch(json!({"n": 1})), opts())
                .await
                .map(drop),
        ),
        ("retire", d.retire(p, &lt, &key, opts()).await.map(drop)),
        (
            "link",
            d.link(p, &ty, &key, &"r".into(), &key, patch(json!({})), opts())
                .await
                .map(drop),
        ),
        (
            "unlink",
            d.unlink(p, &ty, &key, &"r".into(), &key, opts())
                .await
                .map(drop),
        ),
        (
            "call",
            d.call(p, &"none".into(), Some(&key), patch(json!({})), opts())
                .await
                .map(drop),
        ),
        (
            "propose",
            d.propose(p, proposed, "why".into()).await.map(drop),
        ),
        ("accept", d.accept(p, &"none".into()).await.map(drop)),
        (
            "reject",
            d.reject(p, &"none".into(), "no".into()).await.map(drop),
        ),
        ("declare", d.declare(p, declared()).await.map(drop)),
        ("apply", d.apply(p, vec![declared()]).await.map(drop)),
        (
            "add_field",
            d.add_field(p, &ty, field("n", StorageKind::Number))
                .await
                .map(drop),
        ),
        (
            "migrate_apply",
            d.migrate_apply(p, migration, "r").await.map(drop),
        ),
        (
            "migrate_value_type_apply",
            d.migrate_value_type_apply(p, vt, "r").await.map(drop),
        ),
        (
            "ingest",
            ingest(
                d,
                p,
                "meter",
                vec![source_record("nothing", "r1", json!({"n": 1}))],
            )
            .await
            .map(drop),
        ),
    ];
    let mut red: Vec<String> = tries
        .into_iter()
        .filter(|(_, answer)| !matches!(reason(answer), Some(Refusal::TooDeep { .. })))
        .map(|(door, answer)| format!("`{door}` answered {answer:?}"))
        .collect();
    // The lake comes before the context: a principal of another lake.
    let stranger = lakelands_api::Principal {
        lake: "elsewhere".into(),
        ..nobody.clone()
    };
    let foreign = d
        .upsert(&stranger, &lt, &key, patch(json!({"n": 1})), opts())
        .await;
    if !matches!(reason(&foreign), Some(Refusal::NotGranted)) {
        red.push(format!("a principal of another lake answered {foreign:?}"));
    }
    ensure!(
        red.is_empty(),
        "through a context past the ceiling, every write door refuses as too_deep before \
         anything else it checks, after the lake: {}",
        red.join("; ")
    );
    head_unmoved(store.as_ref(), head, "the writes past the ceiling").await
}

async fn a_triggers_ceiling_is_read_with_the_cause_at_the_append(
    store: Arc<dyn LakeStore>,
) -> Result<(), Failure> {
    let l = Lake::of(store.as_ref());
    let Lake { cat, own } = &l;
    l.declare(type_decl("job", vec![field("n", StorageKind::Number)]))
        .await?;
    let Declaration::Trigger(mut once) = trigger_on("once", "job", None) else {
        unreachable!("trigger_on declares a trigger")
    };
    once.max_depth = Some(0);
    cat.declare(own, Declaration::Trigger(once))
        .await
        .must("declaring `once` with max_depth 0")?;
    let nobody = nobody(store.as_ref());
    let write = |who: lakelands_api::Principal, ctx: WriteContext, key: &'static str| {
        let store = store.clone();
        async move {
            let cat = store.catalog(ctx);
            try_upsert(cat.as_ref(), &who, "job", key, json!({"n": 1})).await
        }
    };
    // The write that fires `once` puts its record at head + 1 and the
    // firing at head + 2; a reaction naming head + 2 before it is past the
    // head.
    let head = store.head().await.must("reading the head")?;
    let firing = head.next().next();
    let early = write(own.clone(), reactive(1, Some(firing)), "r1").await;
    ensure!(
        matches!(reason(&early), Some(Refusal::Invalid { .. })),
        "a reaction naming {firing}, past the head {head}, is refused as invalid; it \
         answered {early:?}"
    );
    head_unmoved(
        store.as_ref(),
        head,
        "a reaction naming a cause past the head",
    )
    .await?;
    let fired = write(own.clone(), WriteContext::default(), "j1")
        .await
        .must("the write that fires `once`")?;
    ensure!(
        store.head().await.must("reading the head")? == firing,
        "the write at {} fires `once` at {firing}; the head is elsewhere",
        fired.position()
    );
    let mut red = Vec::new();
    for (what, who, too_deep) in [("the owner", own.clone(), true), ("nobody", nobody, false)] {
        let late = write(who, reactive(1, Some(firing)), "r2").await;
        let ok = if too_deep {
            matches!(
                refused_on(&late),
                Some(("once", Refusal::TooDeep { max: 0 }))
            )
        } else {
            // Nobody may know of `n`, so its write is answered as for a
            // field the type does not declare (006).
            no_such_field(&late, "n")
        };
        if !ok {
            red.push(format!("{what} answered {late:?}"));
        }
    }
    ensure!(
        red.is_empty(),
        "a reaction at depth 1 naming the firing of `once` (max_depth 0) is refused too_deep \
         naming `once`, read at the append after the door's own checks, so one without a \
         grant is refused by those first: {}",
        red.join("; ")
    );
    head_unmoved(
        store.as_ref(),
        firing,
        "the reactions past `once`'s ceiling",
    )
    .await
}
